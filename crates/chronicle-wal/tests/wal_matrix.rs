//! Deterministic real-WAL-file matrix (migration 3.2).
//!
//! Portable replacement for the broad `cargo test -p chronicle-wal` invocation
//! inside privileged scenarios: proves encoding/checksum, write->scan recovery,
//! corruption fail-closed, and retention/fault behavior through the public API
//! on real segment files. No root, eBPF, or Multipass required.

use chronicle_capture::{ClockIdentity, MonotonicTimestamp};
use chronicle_common::RecordingId;
use chronicle_wal::{
    CleanupFault, CleanupOutcome, CommitMarker, DEFAULT_MAX_RECORD_BYTES, GroupCommitWalWriter,
    MIN_SEGMENT_BYTES, RecordKind, RecordingLock, RetentionError, SEGMENT_HEADER_LEN,
    TERMINAL_WAL_LOSS_SCHEMA_VERSION, TerminalWalLoss, TerminalWalLossAmbiguity,
    TerminalWalLossInterval, TerminalWalLossReason, WalError, WalRecordEnvelope,
    cleanup_finalized_segment_with_policy, decode_commit_marker, decode_segment_header,
    decode_terminal_wal_loss, encode_commit_marker, encode_envelope, encode_segment_header,
    encode_terminal_wal_loss, recover_cleanup_with_policy, scan_wal,
};
use std::fs;
use std::path::{Path, PathBuf};

fn temp_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "chronicle-wal-matrix-{tag}-{}",
        uuid::Uuid::new_v4()
    ))
}

fn first_segment(wal: &Path) -> PathBuf {
    let segments = wal.join("segments");
    let mut names: Vec<PathBuf> = fs::read_dir(&segments)
        .expect("segments dir")
        .map(|entry| entry.expect("entry").path())
        .collect();
    names.sort();
    names.into_iter().next().expect("at least one segment")
}

fn write_small_wal(wal: &Path, recording_id: RecordingId) {
    let mut writer =
        GroupCommitWalWriter::create(wal, recording_id, MIN_SEGMENT_BYTES, 1, 0).unwrap();
    writer
        .append(RecordKind::CaptureEvent, 1, 0, b"committed one".to_vec(), 0)
        .unwrap();
    assert_eq!(writer.flush(1).unwrap().unwrap().marker_sequence, 2);
    writer
        .append(RecordKind::CaptureEvent, 1, 0, b"committed two".to_vec(), 2)
        .unwrap();
    writer
        .append(RecordKind::LossWindow, 1, 0, b"loss".to_vec(), 2)
        .unwrap();
    assert_eq!(writer.flush(3).unwrap().unwrap().marker_sequence, 5);
    drop(writer);
}

#[test]
fn encode_decode_round_trip() {
    let header = chronicle_wal::SegmentHeader {
        recording_id: RecordingId::new(),
        segment_ordinal: 1,
        first_sequence: 1,
        created_unix_nanos: 1_700_000_000_000_000_000,
    };
    let bytes = encode_segment_header(&header);
    assert_eq!(bytes.len(), SEGMENT_HEADER_LEN);
    assert_eq!(decode_segment_header(&bytes).unwrap(), header);

    let envelope = WalRecordEnvelope::unplaced(
        header.recording_id,
        8,
        RecordKind::CaptureEvent,
        1,
        0,
        b"payload".to_vec(),
    );
    let encoded = encode_envelope(&envelope).unwrap();
    assert!(!encoded.is_empty());

    let marker = CommitMarker {
        durable_through_sequence: 8,
        durable_record_count: 8,
        durable_payload_bytes: 123,
        batch_first_sequence: 1,
        batch_last_sequence: 8,
        batch_sha256: [7u8; 32],
    };
    assert_eq!(
        decode_commit_marker(&encode_commit_marker(&marker)).unwrap(),
        marker
    );

    let clock = ClockIdentity {
        boot_id: "boot-1".to_owned(),
    };
    let loss = TerminalWalLoss {
        interval: TerminalWalLossInterval {
            start: MonotonicTimestamp {
                clock: clock.clone(),
                nanoseconds: 1,
            },
            end: MonotonicTimestamp {
                clock,
                nanoseconds: 2,
            },
        },
        discarded_records: 3,
        discarded_payload_bytes: 9,
        reason: TerminalWalLossReason::WalHardLimit,
        ambiguity: TerminalWalLossAmbiguity::UnknownDownstreamEffects,
    };
    let payload = encode_terminal_wal_loss(&loss).unwrap();
    assert_eq!(
        decode_terminal_wal_loss(TERMINAL_WAL_LOSS_SCHEMA_VERSION, &payload).unwrap(),
        loss
    );
}

#[test]
fn write_scan_recover_is_deterministic() {
    let root = temp_root("recover");
    let recording_id = RecordingId::new();
    write_small_wal(&root, recording_id);

    let first = scan_wal(&root, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
    let second = scan_wal(&root, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
    assert_eq!(first, second, "recovery scan must be deterministic");
    assert_eq!(first.authority.marker_sequence, Some(5));
    assert_eq!(first.authority.durable_through_sequence, Some(4));
    assert_eq!(first.authoritative_marker_count, 2);
    // Committed includes data records plus commit markers.
    let sequences: Vec<u64> = first.committed.iter().map(|r| r.sequence).collect();
    assert_eq!(sequences, [1, 2, 3, 4, 5]);

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn envelope_corruption_fails_closed() {
    let root = temp_root("envelope");
    let recording_id = RecordingId::new();
    write_small_wal(&root, recording_id);
    let segment = first_segment(&root);
    let mut bytes = fs::read(&segment).unwrap();
    // Flip a byte well inside the envelope payload region (past header + magic).
    let mid = (bytes.len() - SEGMENT_HEADER_LEN) / 2 + SEGMENT_HEADER_LEN + 8;
    bytes[mid] ^= 0x01;
    fs::write(&segment, bytes).unwrap();

    let error = scan_wal(&root, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap_err();
    assert!(
        matches!(
            error,
            WalError::EnvelopeChecksum { .. }
                | WalError::Checksum { .. }
                | WalError::InvalidEnvelopeMagic { .. }
                | WalError::InvalidMagic { .. }
                | WalError::EnvelopeRecordingIdMismatch
                | WalError::SegmentIdentityMismatch(_)
        ),
        "unexpected corruption error: {error}"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn segment_header_corruption_fails_closed() {
    let root = temp_root("header");
    let recording_id = RecordingId::new();
    write_small_wal(&root, recording_id);
    let segment = first_segment(&root);
    let mut bytes = fs::read(&segment).unwrap();
    bytes[0] ^= 0x01; // corrupt magic byte
    fs::write(&segment, bytes).unwrap();

    let error = scan_wal(&root, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap_err();
    assert!(
        matches!(error, WalError::InvalidSegmentMagic),
        "got {error}"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn truncated_segment_never_counts_as_durable() {
    let root = temp_root("truncate");
    let recording_id = RecordingId::new();
    write_small_wal(&root, recording_id);
    let segment = first_segment(&root);
    let bytes = fs::read(&segment).unwrap();
    fs::write(&segment, &bytes[..SEGMENT_HEADER_LEN + 40]).unwrap(); // cut mid-envelope

    let scan = scan_wal(&root, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
    assert!(
        scan.partial_tail.is_some(),
        "truncated tail must be detected, not silently trusted"
    );
    assert_eq!(
        scan.authority.durable_through_sequence, None,
        "no durability claimed past the cut"
    );
    assert!(
        scan.committed.is_empty(),
        "truncated bytes never count as committed"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn writer_lock_fault_is_detected() {
    let root = temp_root("lock");
    let recording_id = RecordingId::new();
    let writer =
        GroupCommitWalWriter::create(&root, recording_id, MIN_SEGMENT_BYTES, 1, 0).unwrap();
    assert!(matches!(
        RecordingLock::acquire(&root),
        Err(WalError::RecordingLockHeld)
    ));
    drop(writer);
    assert!(RecordingLock::acquire(&root).is_ok());
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn retention_cleanup_fault_matrix() {
    let root = temp_root("retention");
    let source = root.join("finalized-segment.chwal");
    fs::create_dir_all(&root).unwrap();
    fs::write(&source, vec![0xabu8; 4096]).unwrap();
    let trash = root.join("trash");
    let intent = root.join("cleanup-intent.json");

    // Production-disabled path is rejected without touching the source.
    let disabled = cleanup_finalized_segment_with_policy(
        &source,
        &trash,
        &intent,
        "tx-disabled",
        None,
        false,
        None,
    );
    assert!(matches!(disabled, Err(RetentionError::ProductionDisabled)));
    assert!(
        source.is_file(),
        "source untouched when production disabled"
    );

    // Fault after intent write -> Interrupted; recovery completes the cleanup.
    let faulted = cleanup_finalized_segment_with_policy(
        &source,
        &trash,
        &intent,
        "tx-1",
        None,
        true,
        Some(CleanupFault::AfterIntent),
    );
    assert!(matches!(faulted, Err(RetentionError::Interrupted)));
    assert!(intent.is_file(), "intent persisted before interruption");
    let recovered = recover_cleanup_with_policy(&intent, true).unwrap();
    assert!(matches!(recovered, CleanupOutcome::Recovered));
    assert!(!source.exists(), "source moved to trash by recovery");

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn retention_digest_mismatch_is_rejected() {
    let root = temp_root("digest");
    let source = root.join("segment.chwal");
    fs::create_dir_all(&root).unwrap();
    fs::write(&source, vec![0x01u8; 256]).unwrap();
    let wrong = "0".repeat(64);
    let error = cleanup_finalized_segment_with_policy(
        &source,
        root.join("trash"),
        root.join("intent.json"),
        "tx-digest",
        Some(&wrong),
        true,
        None,
    )
    .unwrap_err();
    assert!(matches!(error, RetentionError::DigestMismatch));
    assert!(source.is_file(), "source untouched on digest mismatch");
    fs::remove_dir_all(&root).unwrap();
}
