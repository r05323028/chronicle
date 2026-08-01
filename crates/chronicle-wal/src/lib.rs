//! WAL v1 append-only framing, segmented writer, and recovery.

use chronicle_capture::{ClockIdentity, MonotonicTimestamp};
use chronicle_common::RecordingId;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use rustix::fs::{CWD, FlockOperation, RenameFlags, flock, renameat_with};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

mod manifest;
mod retention;
pub use manifest::{
    CheckpointReference, ManifestError, ManifestPolicy, ManifestRetention, MarkerSummary,
    SegmentDeletion, SegmentLifecycle, WAL_MANIFEST_FILE, WAL_MANIFEST_SCHEMA_VERSION,
    WalManifestSegment, WalManifestV1, capture_sealed_segment, load_manifest, rebuild_manifest,
    write_manifest_atomic,
};
pub use retention::{
    CleanupFault, CleanupIntent, CleanupOutcome, LifecycleEpochEntry, LifecycleIndexV1,
    LifecycleProof, RETENTION_PRODUCTION_ENABLED, RetentionError, SegmentLifecycleRecord,
    cleanup_finalized_segment, cleanup_finalized_segment_verified,
    cleanup_finalized_segment_with_policy, recover_cleanup, recover_cleanup_with_policy,
};

pub const WAL_FORMAT_VERSION: u16 = 1;
pub const SEGMENT_HEADER_VERSION: u16 = 1;
pub const SEGMENT_HEADER_LEN: usize = 64;
const SEGMENT_HEADER_LEN_U32: u32 = 64;
const SEGMENT_HEADER_LEN_U64: u64 = 64;
pub const ENVELOPE_VERSION: u16 = 1;
pub const WAL_RECORD_SCHEMA_VERSION: u16 = 1;
pub const ENVELOPE_HEADER_LEN: usize = 48;
const ENVELOPE_HEADER_LEN_U32: u32 = 48;
pub const DEFAULT_MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
pub const COMMIT_MARKER_VERSION: u16 = 1;
pub const TERMINAL_WAL_LOSS_SCHEMA_VERSION: u16 = 1;
pub const COMMIT_MARKER_PAYLOAD_LEN: usize = 76;
pub const COMMIT_MARKER_FRAME_LEN: usize = ENVELOPE_HEADER_LEN + COMMIT_MARKER_PAYLOAD_LEN;
pub const GROUP_COMMIT_BYTES: u64 = 4 * 1024 * 1024;
pub const GROUP_COMMIT_INTERVAL_MILLIS: u64 = 10;
pub const DEFAULT_SEGMENT_AGE_MILLIS: u64 = 5 * 60 * 1_000;
pub const DEFAULT_SEGMENT_BYTES: u64 = 256 * 1024 * 1024;
pub const MIN_SEGMENT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_SEGMENT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const DEFAULT_MAX_WAL_BYTES: u64 = MAX_SEGMENT_BYTES;
const SEGMENT_MAGIC: [u8; 4] = *b"CHS1";
const ENVELOPE_MAGIC: [u8; 4] = *b"CHE1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentHeader {
    pub recording_id: RecordingId,
    pub segment_ordinal: u64,
    pub first_sequence: u64,
    pub created_unix_nanos: i64,
}

impl SegmentHeader {
    pub fn validate_identity(
        &self,
        recording_id: RecordingId,
        segment_ordinal: u64,
        first_sequence: u64,
    ) -> Result<(), WalError> {
        if self.recording_id != recording_id {
            return Err(WalError::SegmentIdentityMismatch("recording ID"));
        }
        if self.segment_ordinal != segment_ordinal {
            return Err(WalError::SegmentIdentityMismatch("segment ordinal"));
        }
        if self.first_sequence != first_sequence {
            return Err(WalError::SegmentIdentityMismatch("first sequence"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum RecordKind {
    CaptureEvent = 1,
    LossWindow = 2,
    CommitMarker = 3,
    TerminalWalLoss = 4,
}

impl RecordKind {
    pub const ALL: [Self; 4] = [
        Self::CaptureEvent,
        Self::LossWindow,
        Self::CommitMarker,
        Self::TerminalWalLoss,
    ];
}

impl TryFrom<u16> for RecordKind {
    type Error = WalError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::CaptureEvent),
            2 => Ok(Self::LossWindow),
            3 => Ok(Self::CommitMarker),
            4 => Ok(Self::TerminalWalLoss),
            other => Err(WalError::UnknownRecordKind(other)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalRecordProvenance {
    pub segment_ordinal: u64,
    pub segment_first_sequence: u64,
    pub byte_offset: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalRecordEnvelope {
    pub recording_id: RecordingId,
    pub sequence: u64,
    pub provenance: Option<WalRecordProvenance>,
    pub kind: RecordKind,
    pub schema_version: u16,
    pub flags: u16,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalWalLossInterval {
    pub start: MonotonicTimestamp,
    pub end: MonotonicTimestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TerminalWalLossReason {
    WalHardLimit = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TerminalWalLossAmbiguity {
    UnknownDownstreamEffects = 1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalWalLoss {
    pub interval: TerminalWalLossInterval,
    pub discarded_records: u64,
    pub discarded_payload_bytes: u64,
    pub reason: TerminalWalLossReason,
    pub ambiguity: TerminalWalLossAmbiguity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredTerminalWalLoss {
    pub sequence: u64,
    pub loss: TerminalWalLoss,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMarker {
    pub durable_through_sequence: u64,
    pub durable_record_count: u64,
    pub durable_payload_bytes: u64,
    pub batch_first_sequence: u64,
    pub batch_last_sequence: u64,
    pub batch_sha256: [u8; 32],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitAuthority {
    pub marker_sequence: Option<u64>,
    pub durable_through_sequence: Option<u64>,
    pub durable_record_count: u64,
    pub durable_payload_bytes: u64,
    pub segment_ordinal: Option<u64>,
}

impl WalRecordEnvelope {
    pub fn unplaced(
        recording_id: RecordingId,
        sequence: u64,
        kind: RecordKind,
        schema_version: u16,
        flags: u16,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            recording_id,
            sequence,
            provenance: None,
            kind,
            schema_version,
            flags,
            payload,
        }
    }

    #[must_use]
    pub fn with_provenance(mut self, provenance: WalRecordProvenance) -> Self {
        self.provenance = Some(provenance);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalCheckpoint {
    pub segment_first_sequence: u64,
    pub byte_offset: u64,
    pub next_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadOutcome {
    Record(WalRecordEnvelope),
    End,
    PartialTail { offset: u64 },
}

#[derive(Debug, Error)]
pub enum WalError {
    #[error("WAL I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("invalid WAL magic at byte offset {offset}")]
    InvalidMagic { offset: u64 },
    #[error("invalid WAL segment magic")]
    InvalidSegmentMagic,
    #[error("truncated WAL segment header: expected 64 bytes, got {actual}")]
    TruncatedSegmentHeader { actual: usize },
    #[error("unsupported WAL segment header version {0}")]
    UnsupportedSegmentHeaderVersion(u16),
    #[error("invalid WAL segment header length {0}")]
    InvalidSegmentHeaderLength(u32),
    #[error("WAL segment header reserved bytes are nonzero")]
    NonzeroSegmentHeaderReserved,
    #[error("WAL segment header checksum mismatch: expected {expected:#010x}, got {actual:#010x}")]
    SegmentHeaderChecksum { expected: u32, actual: u32 },
    #[error("WAL segment identity mismatch: {0}")]
    SegmentIdentityMismatch(&'static str),
    #[error("invalid WAL envelope magic at byte offset {offset}")]
    InvalidEnvelopeMagic { offset: u64 },
    #[error("unsupported WAL envelope version {version}")]
    UnsupportedEnvelopeVersion { version: u16, kind: u16 },
    #[error("invalid WAL envelope header length {0}")]
    InvalidEnvelopeHeaderLength(u32),
    #[error("WAL envelope recording ID does not match segment header")]
    EnvelopeRecordingIdMismatch,
    #[error("invalid WAL segment filename {0}")]
    InvalidSegmentFileName(String),
    #[error(
        "WAL segment filename first sequence {file_first_sequence} does not match header {header_first_sequence}"
    )]
    SegmentFileNameMismatch {
        file_first_sequence: u64,
        header_first_sequence: u64,
    },
    #[error("missing WAL segment ordinal {expected}; found {actual}")]
    MissingSegmentOrdinal { expected: u64, actual: u64 },
    #[error("duplicate WAL segment ordinal {0}")]
    DuplicateSegmentOrdinal(u64),
    #[error("duplicate WAL segment first sequence {0}")]
    DuplicateSegmentFirstSequence(u64),
    #[error("WAL segment first sequence {current} does not increase after {previous}")]
    NonIncreasingSegmentFirstSequence { previous: u64, current: u64 },
    #[error(
        "WAL envelope checksum mismatch at sequence {sequence}: expected {expected:#010x}, got {actual:#010x}"
    )]
    EnvelopeChecksum {
        sequence: u64,
        kind: RecordKind,
        expected: u32,
        actual: u32,
    },
    #[error("unsupported WAL version {0}")]
    UnsupportedVersion(u16),
    #[error("unsupported WAL record schema version {version} for kind {kind}")]
    UnsupportedRecordSchemaVersion { kind: u16, version: u16 },
    #[error("unknown WAL record kind {0}")]
    UnknownRecordKind(u16),
    #[error("WAL record payload is too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("WAL record is {record_bytes} bytes; configured maximum is {max_record_bytes}")]
    RecordTooLarge {
        record_bytes: usize,
        max_record_bytes: usize,
    },
    #[error(
        "WAL checksum mismatch at sequence {sequence}: expected {expected:#010x}, got {actual:#010x}"
    )]
    Checksum {
        sequence: u64,
        expected: u32,
        actual: u32,
    },
    #[error("WAL sequence {current} does not follow {previous}")]
    OutOfOrder { previous: u64, current: u64 },
    #[error("WAL sequence space exhausted at {sequence}")]
    SequenceExhausted { sequence: u64 },
    #[error("WAL sequence {actual} does not match expected sequence {expected}")]
    UnexpectedSequence { expected: u64, actual: u64 },
    #[error("segment size must exceed WAL header size")]
    InvalidSegmentSize,
    #[error("WAL writer has no active segment")]
    NoActiveSegment,
    #[error("segment size must be between 16 MiB and 4 GiB")]
    InvalidSegmentSizeRange,
    #[error("segment age must be greater than zero")]
    InvalidSegmentAge,
    #[error("WAL envelope recording ID does not match writer")]
    WriterRecordingIdMismatch,
    #[error("WAL envelope already has segment provenance")]
    EnvelopeAlreadyPlaced,
    #[error(
        "WAL envelope frame is {record_bytes} bytes and cannot fit segment size {segment_bytes}"
    )]
    EnvelopeExceedsSegment {
        record_bytes: u64,
        segment_bytes: u64,
    },
    #[error("WAL segment publication is unsupported on this platform")]
    UnsupportedSegmentPublication,
    #[error("live WAL snapshot is uncertain and should be retried")]
    RetryableSnapshotUncertainty,
    #[error("WAL segment ordinal space is exhausted")]
    SegmentOrdinalExhausted,
    #[error("commit marker payload must be exactly 76 bytes; got {actual}")]
    InvalidCommitMarkerLength { actual: usize },
    #[error("unsupported terminal WAL-loss schema version {0}")]
    UnsupportedTerminalWalLossVersion(u16),
    #[error("invalid terminal WAL-loss payload: {0}")]
    InvalidTerminalWalLoss(&'static str),
    #[error("terminal WAL-loss clock identity exceeds 65535 bytes: {actual}")]
    TerminalWalLossClockTooLong { actual: usize },
    #[error("unsupported commit marker version {0}")]
    UnsupportedCommitMarkerVersion(u16),
    #[error("commit marker reserved field is nonzero: {0}")]
    InvalidCommitMarkerReserved(u16),
    #[error("commit marker envelope has wrong kind or schema")]
    InvalidCommitMarkerEnvelope,
    #[error("commit marker batch is empty")]
    EmptyCommitBatch,
    #[error("WAL sequence {sequence} is missing placed provenance")]
    MissingProvenance { sequence: u64 },
    #[error("commit marker batch contains marker sequence {sequence}")]
    CommitBatchContainsMarker { sequence: u64 },
    #[error("commit marker recording identity mismatch")]
    CommitRecordingIdMismatch,
    #[error("commit marker segment provenance mismatch")]
    CommitSegmentMismatch,
    #[error("commit marker boundary mismatch: expected {expected}, got {actual}")]
    CommitBoundaryMismatch { expected: u64, actual: u64 },
    #[error("commit marker durable-through mismatch: expected {expected}, got {actual}")]
    CommitDurableThroughMismatch { expected: u64, actual: u64 },
    #[error("commit marker cumulative record count mismatch: expected {expected}, got {actual}")]
    CommitRecordCountMismatch { expected: u64, actual: u64 },
    #[error("commit marker cumulative payload bytes mismatch: expected {expected}, got {actual}")]
    CommitPayloadBytesMismatch { expected: u64, actual: u64 },
    #[error("commit marker batch digest mismatch")]
    CommitDigestMismatch,
    #[error("commit marker arithmetic overflow")]
    CommitArithmeticOverflow,
    #[error("reserved commit marker does not fit current segment")]
    CommitMarkerWouldRotate,
    #[error("total WAL byte limit must be at least segment size and at most 4 GiB")]
    InvalidTotalWalSize,
    #[error("physical WAL limit {limit} bytes cannot fit required {required} bytes")]
    WalCapacityExceeded { required: u64, limit: u64 },
    #[error("recording WAL lock is already held")]
    RecordingLockHeld,
    #[error("recording WAL has no published WAL segments")]
    NoPublishedSegments,
    #[error("partial WAL tail appears before final segment")]
    PartialTailBeforeFinalSegment,
    #[error("reopen requires a recovery-authoritative commit marker")]
    NoAuthoritativeCommit,
    #[error("complete commit marker appears after uncommitted WAL suffix began")]
    CommitMarkerAfterUncommitted,
    #[error("WAL segment first sequence mismatch: expected {expected}, got {actual}")]
    RecoverySegmentFirstSequenceMismatch { expected: u64, actual: u64 },
}

impl WalError {
    pub const fn is_capacity_exhausted(&self) -> bool {
        matches!(self, Self::WalCapacityExceeded { .. })
    }
}

pub fn encode_segment_header(header: &SegmentHeader) -> [u8; SEGMENT_HEADER_LEN] {
    let mut bytes = [0_u8; SEGMENT_HEADER_LEN];
    bytes[0..4].copy_from_slice(&SEGMENT_MAGIC);
    bytes[4..6].copy_from_slice(&WAL_FORMAT_VERSION.to_le_bytes());
    bytes[6..8].copy_from_slice(&SEGMENT_HEADER_VERSION.to_le_bytes());
    bytes[8..12].copy_from_slice(&SEGMENT_HEADER_LEN_U32.to_le_bytes());
    bytes[12..28].copy_from_slice(header.recording_id.0.as_bytes());
    bytes[28..36].copy_from_slice(&header.segment_ordinal.to_le_bytes());
    bytes[36..44].copy_from_slice(&header.first_sequence.to_le_bytes());
    bytes[44..52].copy_from_slice(&header.created_unix_nanos.to_le_bytes());
    let checksum = crc32c::crc32c(&bytes[..60]);
    bytes[60..64].copy_from_slice(&checksum.to_le_bytes());
    bytes
}

pub fn decode_segment_header(bytes: &[u8]) -> Result<SegmentHeader, WalError> {
    if bytes.len() < SEGMENT_HEADER_LEN {
        return Err(WalError::TruncatedSegmentHeader {
            actual: bytes.len(),
        });
    }
    let bytes = &bytes[..SEGMENT_HEADER_LEN];
    if bytes[..4] != SEGMENT_MAGIC {
        return Err(WalError::InvalidSegmentMagic);
    }
    let format_version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if format_version != WAL_FORMAT_VERSION {
        return Err(WalError::UnsupportedVersion(format_version));
    }
    let header_version = u16::from_le_bytes([bytes[6], bytes[7]]);
    if header_version != SEGMENT_HEADER_VERSION {
        return Err(WalError::UnsupportedSegmentHeaderVersion(header_version));
    }
    let header_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    if header_len != SEGMENT_HEADER_LEN_U32 {
        return Err(WalError::InvalidSegmentHeaderLength(header_len));
    }
    if bytes[52..60] != [0; 8] {
        return Err(WalError::NonzeroSegmentHeaderReserved);
    }
    let expected = u32::from_le_bytes([bytes[60], bytes[61], bytes[62], bytes[63]]);
    let actual = crc32c::crc32c(&bytes[..60]);
    if actual != expected {
        return Err(WalError::SegmentHeaderChecksum { expected, actual });
    }
    let mut recording_id = [0_u8; 16];
    recording_id.copy_from_slice(&bytes[12..28]);
    Ok(SegmentHeader {
        recording_id: RecordingId(uuid::Uuid::from_bytes(recording_id)),
        segment_ordinal: u64::from_le_bytes([
            bytes[28], bytes[29], bytes[30], bytes[31], bytes[32], bytes[33], bytes[34], bytes[35],
        ]),
        first_sequence: u64::from_le_bytes([
            bytes[36], bytes[37], bytes[38], bytes[39], bytes[40], bytes[41], bytes[42], bytes[43],
        ]),
        created_unix_nanos: i64::from_le_bytes([
            bytes[44], bytes[45], bytes[46], bytes[47], bytes[48], bytes[49], bytes[50], bytes[51],
        ]),
    })
}

pub fn encode_envelope(envelope: &WalRecordEnvelope) -> Result<Vec<u8>, WalError> {
    if envelope.schema_version != WAL_RECORD_SCHEMA_VERSION {
        return Err(WalError::UnsupportedRecordSchemaVersion {
            kind: envelope.kind as u16,
            version: envelope.schema_version,
        });
    }
    let payload_len = u32::try_from(envelope.payload.len())
        .map_err(|_| WalError::PayloadTooLarge(envelope.payload.len()))?;
    let mut bytes = Vec::with_capacity(ENVELOPE_HEADER_LEN + envelope.payload.len());
    bytes.extend_from_slice(&ENVELOPE_MAGIC);
    bytes.extend_from_slice(&ENVELOPE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(envelope.kind as u16).to_le_bytes());
    bytes.extend_from_slice(&envelope.flags.to_le_bytes());
    bytes.extend_from_slice(&envelope.schema_version.to_le_bytes());
    bytes.extend_from_slice(&ENVELOPE_HEADER_LEN_U32.to_le_bytes());
    bytes.extend_from_slice(&payload_len.to_le_bytes());
    bytes.extend_from_slice(envelope.recording_id.0.as_bytes());
    bytes.extend_from_slice(&envelope.sequence.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&envelope.payload);
    let checksum = envelope_checksum(&bytes[..44], &envelope.payload);
    bytes[44..48].copy_from_slice(&checksum.to_le_bytes());
    Ok(bytes)
}

fn envelope_checksum(fixed_header: &[u8], payload: &[u8]) -> u32 {
    let mut checksum_input = Vec::with_capacity(fixed_header.len() + payload.len());
    checksum_input.extend_from_slice(fixed_header);
    checksum_input.extend_from_slice(payload);
    crc32c::crc32c(&checksum_input)
}

pub fn encode_commit_marker(marker: &CommitMarker) -> [u8; COMMIT_MARKER_PAYLOAD_LEN] {
    let mut bytes = [0_u8; COMMIT_MARKER_PAYLOAD_LEN];
    bytes[0..2].copy_from_slice(&COMMIT_MARKER_VERSION.to_le_bytes());
    bytes[4..12].copy_from_slice(&marker.durable_through_sequence.to_le_bytes());
    bytes[12..20].copy_from_slice(&marker.durable_record_count.to_le_bytes());
    bytes[20..28].copy_from_slice(&marker.durable_payload_bytes.to_le_bytes());
    bytes[28..36].copy_from_slice(&marker.batch_first_sequence.to_le_bytes());
    bytes[36..44].copy_from_slice(&marker.batch_last_sequence.to_le_bytes());
    bytes[44..76].copy_from_slice(&marker.batch_sha256);
    bytes
}

pub fn decode_commit_marker(payload: &[u8]) -> Result<CommitMarker, WalError> {
    if payload.len() != COMMIT_MARKER_PAYLOAD_LEN {
        return Err(WalError::InvalidCommitMarkerLength {
            actual: payload.len(),
        });
    }
    let version = u16::from_le_bytes([payload[0], payload[1]]);
    if version != COMMIT_MARKER_VERSION {
        return Err(WalError::UnsupportedCommitMarkerVersion(version));
    }
    let reserved = u16::from_le_bytes([payload[2], payload[3]]);
    if reserved != 0 {
        return Err(WalError::InvalidCommitMarkerReserved(reserved));
    }
    Ok(CommitMarker {
        durable_through_sequence: read_u64(payload, 4),
        durable_record_count: read_u64(payload, 12),
        durable_payload_bytes: read_u64(payload, 20),
        batch_first_sequence: read_u64(payload, 28),
        batch_last_sequence: read_u64(payload, 36),
        batch_sha256: payload[44..76].try_into().map_err(|_| {
            WalError::InvalidCommitMarkerLength {
                actual: payload.len(),
            }
        })?,
    })
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

pub fn encode_terminal_wal_loss(loss: &TerminalWalLoss) -> Result<Vec<u8>, WalError> {
    validate_terminal_wal_loss(loss)?;
    let start_clock = loss.interval.start.clock.boot_id.as_bytes();
    let end_clock = loss.interval.end.clock.boot_id.as_bytes();
    let start_len =
        u16::try_from(start_clock.len()).map_err(|_| WalError::TerminalWalLossClockTooLong {
            actual: start_clock.len(),
        })?;
    let end_len =
        u16::try_from(end_clock.len()).map_err(|_| WalError::TerminalWalLossClockTooLong {
            actual: end_clock.len(),
        })?;
    let mut bytes =
        Vec::with_capacity(2 + start_clock.len() + 8 + 2 + end_clock.len() + 8 + 8 + 8 + 2);
    bytes.extend_from_slice(&start_len.to_le_bytes());
    bytes.extend_from_slice(start_clock);
    bytes.extend_from_slice(&loss.interval.start.nanoseconds.to_le_bytes());
    bytes.extend_from_slice(&end_len.to_le_bytes());
    bytes.extend_from_slice(end_clock);
    bytes.extend_from_slice(&loss.interval.end.nanoseconds.to_le_bytes());
    bytes.extend_from_slice(&loss.discarded_records.to_le_bytes());
    bytes.extend_from_slice(&loss.discarded_payload_bytes.to_le_bytes());
    bytes.push(loss.reason as u8);
    bytes.push(loss.ambiguity as u8);
    Ok(bytes)
}

pub fn decode_terminal_wal_loss(
    schema_version: u16,
    payload: &[u8],
) -> Result<TerminalWalLoss, WalError> {
    if schema_version != TERMINAL_WAL_LOSS_SCHEMA_VERSION {
        return Err(WalError::UnsupportedTerminalWalLossVersion(schema_version));
    }
    let mut offset = 0;
    let start_clock = decode_terminal_clock(payload, &mut offset)?;
    let start = MonotonicTimestamp {
        clock: start_clock,
        nanoseconds: decode_terminal_u64(payload, &mut offset)?,
    };
    let end_clock = decode_terminal_clock(payload, &mut offset)?;
    let end = MonotonicTimestamp {
        clock: end_clock,
        nanoseconds: decode_terminal_u64(payload, &mut offset)?,
    };
    let discarded_records = decode_terminal_u64(payload, &mut offset)?;
    let discarded_payload_bytes = decode_terminal_u64(payload, &mut offset)?;
    let reason = match decode_terminal_byte(payload, &mut offset)? {
        1 => TerminalWalLossReason::WalHardLimit,
        _ => return Err(WalError::InvalidTerminalWalLoss("unknown reason")),
    };
    let ambiguity = match decode_terminal_byte(payload, &mut offset)? {
        1 => TerminalWalLossAmbiguity::UnknownDownstreamEffects,
        _ => return Err(WalError::InvalidTerminalWalLoss("unknown ambiguity")),
    };
    if offset != payload.len() {
        return Err(WalError::InvalidTerminalWalLoss("trailing bytes"));
    }
    let loss = TerminalWalLoss {
        interval: TerminalWalLossInterval { start, end },
        discarded_records,
        discarded_payload_bytes,
        reason,
        ambiguity,
    };
    validate_terminal_wal_loss(&loss)?;
    Ok(loss)
}

fn validate_terminal_wal_loss(loss: &TerminalWalLoss) -> Result<(), WalError> {
    if loss.interval.start.clock != loss.interval.end.clock {
        return Err(WalError::InvalidTerminalWalLoss("interval clocks differ"));
    }
    if loss.interval.start.nanoseconds > loss.interval.end.nanoseconds {
        return Err(WalError::InvalidTerminalWalLoss("interval is reversed"));
    }
    if loss.discarded_records == 0 {
        return Err(WalError::InvalidTerminalWalLoss(
            "discarded records is zero",
        ));
    }
    Ok(())
}

fn decode_terminal_clock(payload: &[u8], offset: &mut usize) -> Result<ClockIdentity, WalError> {
    let length = usize::from(decode_terminal_u16(payload, offset)?);
    let bytes = decode_terminal_bytes(payload, offset, length)?;
    let boot_id = std::str::from_utf8(bytes)
        .map_err(|_| WalError::InvalidTerminalWalLoss("clock identity is not UTF-8"))?
        .to_owned();
    Ok(ClockIdentity { boot_id })
}

fn decode_terminal_u16(payload: &[u8], offset: &mut usize) -> Result<u16, WalError> {
    let bytes = decode_terminal_bytes(payload, offset, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn decode_terminal_u64(payload: &[u8], offset: &mut usize) -> Result<u64, WalError> {
    let bytes = decode_terminal_bytes(payload, offset, 8)?;
    Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
        WalError::InvalidTerminalWalLoss("invalid integer")
    })?))
}

fn decode_terminal_byte(payload: &[u8], offset: &mut usize) -> Result<u8, WalError> {
    Ok(decode_terminal_bytes(payload, offset, 1)?[0])
}

fn decode_terminal_bytes<'a>(
    payload: &'a [u8],
    offset: &mut usize,
    length: usize,
) -> Result<&'a [u8], WalError> {
    let end = offset
        .checked_add(length)
        .ok_or(WalError::InvalidTerminalWalLoss("length overflow"))?;
    let bytes = payload
        .get(*offset..end)
        .ok_or(WalError::InvalidTerminalWalLoss("truncated payload"))?;
    *offset = end;
    Ok(bytes)
}

pub fn batch_sha256(batch: &[WalRecordEnvelope]) -> Result<[u8; 32], WalError> {
    let mut hasher = Sha256::new();
    for envelope in batch {
        hasher.update(encode_envelope(envelope)?);
    }
    Ok(hasher.finalize().into())
}

pub fn commit_marker_envelope(
    recording_id: RecordingId,
    marker: &CommitMarker,
) -> Result<WalRecordEnvelope, WalError> {
    let sequence = marker
        .batch_last_sequence
        .checked_add(1)
        .ok_or(WalError::CommitArithmeticOverflow)?;
    Ok(WalRecordEnvelope::unplaced(
        recording_id,
        sequence,
        RecordKind::CommitMarker,
        COMMIT_MARKER_VERSION,
        0,
        encode_commit_marker(marker).to_vec(),
    ))
}

#[allow(clippy::too_many_lines)] // Keep fail-closed marker invariants in one auditable transition.
pub fn validate_commit_authority(
    previous: &CommitAuthority,
    batch: &[WalRecordEnvelope],
    marker_envelope: &WalRecordEnvelope,
) -> Result<CommitAuthority, WalError> {
    if marker_envelope.kind != RecordKind::CommitMarker
        || marker_envelope.schema_version != COMMIT_MARKER_VERSION
    {
        return Err(WalError::InvalidCommitMarkerEnvelope);
    }
    if batch.is_empty() {
        return Err(WalError::EmptyCommitBatch);
    }
    let marker = decode_commit_marker(&marker_envelope.payload)?;
    let marker_provenance =
        marker_envelope
            .provenance
            .as_ref()
            .ok_or(WalError::MissingProvenance {
                sequence: marker_envelope.sequence,
            })?;
    let expected_batch_first =
        if previous.segment_ordinal == Some(marker_provenance.segment_ordinal) {
            previous
                .marker_sequence
                .ok_or(WalError::CommitArithmeticOverflow)?
                .checked_add(1)
                .ok_or(WalError::CommitArithmeticOverflow)?
        } else {
            marker_provenance.segment_first_sequence
        };
    if marker.batch_first_sequence != expected_batch_first {
        return Err(WalError::CommitBoundaryMismatch {
            expected: expected_batch_first,
            actual: marker.batch_first_sequence,
        });
    }

    let mut expected_sequence = marker.batch_first_sequence;
    let mut batch_payload_bytes = 0_u64;
    for envelope in batch {
        if envelope.kind == RecordKind::CommitMarker {
            return Err(WalError::CommitBatchContainsMarker {
                sequence: envelope.sequence,
            });
        }
        let provenance = envelope
            .provenance
            .as_ref()
            .ok_or(WalError::MissingProvenance {
                sequence: envelope.sequence,
            })?;
        if envelope.recording_id != marker_envelope.recording_id {
            return Err(WalError::CommitRecordingIdMismatch);
        }
        if provenance.segment_ordinal != marker_provenance.segment_ordinal
            || provenance.segment_first_sequence != marker_provenance.segment_first_sequence
        {
            return Err(WalError::CommitSegmentMismatch);
        }
        if envelope.sequence != expected_sequence {
            return Err(WalError::CommitBoundaryMismatch {
                expected: expected_sequence,
                actual: envelope.sequence,
            });
        }
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or(WalError::CommitArithmeticOverflow)?;
        batch_payload_bytes = batch_payload_bytes
            .checked_add(
                u64::try_from(envelope.payload.len())
                    .map_err(|_| WalError::CommitArithmeticOverflow)?,
            )
            .ok_or(WalError::CommitArithmeticOverflow)?;
    }

    let batch_last_sequence = expected_sequence
        .checked_sub(1)
        .ok_or(WalError::CommitArithmeticOverflow)?;
    if marker.batch_last_sequence != batch_last_sequence {
        return Err(WalError::CommitBoundaryMismatch {
            expected: batch_last_sequence,
            actual: marker.batch_last_sequence,
        });
    }
    if marker_envelope.sequence != expected_sequence {
        return Err(WalError::CommitBoundaryMismatch {
            expected: expected_sequence,
            actual: marker_envelope.sequence,
        });
    }
    if marker.durable_through_sequence != batch_last_sequence {
        return Err(WalError::CommitDurableThroughMismatch {
            expected: batch_last_sequence,
            actual: marker.durable_through_sequence,
        });
    }
    let batch_record_count =
        u64::try_from(batch.len()).map_err(|_| WalError::CommitArithmeticOverflow)?;
    let durable_record_count = previous
        .durable_record_count
        .checked_add(batch_record_count)
        .ok_or(WalError::CommitArithmeticOverflow)?;
    if marker.durable_record_count != durable_record_count {
        return Err(WalError::CommitRecordCountMismatch {
            expected: durable_record_count,
            actual: marker.durable_record_count,
        });
    }
    let durable_payload_bytes = previous
        .durable_payload_bytes
        .checked_add(batch_payload_bytes)
        .ok_or(WalError::CommitArithmeticOverflow)?;
    if marker.durable_payload_bytes != durable_payload_bytes {
        return Err(WalError::CommitPayloadBytesMismatch {
            expected: durable_payload_bytes,
            actual: marker.durable_payload_bytes,
        });
    }
    if batch_sha256(batch)? != marker.batch_sha256 {
        return Err(WalError::CommitDigestMismatch);
    }

    Ok(CommitAuthority {
        marker_sequence: Some(marker_envelope.sequence),
        durable_through_sequence: Some(marker.durable_through_sequence),
        durable_record_count,
        durable_payload_bytes,
        segment_ordinal: Some(marker_provenance.segment_ordinal),
    })
}

pub struct WalReader<R> {
    reader: R,
    header: SegmentHeader,
    offset: u64,
    next_sequence: u64,
    max_record_bytes: usize,
}

impl<R: Read> WalReader<R> {
    pub fn new(
        mut reader: R,
        recording_id: RecordingId,
        segment_ordinal: u64,
        first_sequence: u64,
        max_record_bytes: usize,
    ) -> Result<Self, WalError> {
        let mut bytes = [0_u8; SEGMENT_HEADER_LEN];
        let read = read_up_to(&mut reader, &mut bytes)?;
        let header = decode_segment_header(&bytes[..read])?;
        header.validate_identity(recording_id, segment_ordinal, first_sequence)?;
        Ok(Self {
            reader,
            header,
            offset: SEGMENT_HEADER_LEN as u64,
            next_sequence: first_sequence,
            max_record_bytes,
        })
    }

    #[allow(clippy::too_many_lines)] // Fixed envelope validation stays linear and fail-closed.
    pub fn next_record(&mut self) -> Result<ReadOutcome, WalError> {
        let record_offset = self.offset;
        let mut header = [0_u8; ENVELOPE_HEADER_LEN];
        let header_read = read_up_to(&mut self.reader, &mut header)?;
        if header_read == 0 {
            return Ok(ReadOutcome::End);
        }
        if header_read < ENVELOPE_HEADER_LEN {
            return Ok(ReadOutcome::PartialTail {
                offset: record_offset,
            });
        }
        if header[..4] != ENVELOPE_MAGIC {
            return Err(WalError::InvalidEnvelopeMagic {
                offset: record_offset,
            });
        }
        let version = u16::from_le_bytes([header[4], header[5]]);
        let raw_kind = u16::from_le_bytes([header[6], header[7]]);
        if version != ENVELOPE_VERSION {
            return Err(WalError::UnsupportedEnvelopeVersion {
                version,
                kind: raw_kind,
            });
        }
        let kind = RecordKind::try_from(raw_kind)?;
        let flags = u16::from_le_bytes([header[8], header[9]]);
        let schema_version = u16::from_le_bytes([header[10], header[11]]);
        if schema_version != WAL_RECORD_SCHEMA_VERSION {
            return Err(WalError::UnsupportedRecordSchemaVersion {
                kind: raw_kind,
                version: schema_version,
            });
        }
        let header_len = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
        if header_len != ENVELOPE_HEADER_LEN_U32 {
            return Err(WalError::InvalidEnvelopeHeaderLength(header_len));
        }
        let payload_len = u32::from_le_bytes([header[16], header[17], header[18], header[19]]);
        let mut recording_id = [0_u8; 16];
        recording_id.copy_from_slice(&header[20..36]);
        let recording_id = RecordingId(uuid::Uuid::from_bytes(recording_id));
        if recording_id != self.header.recording_id {
            return Err(WalError::EnvelopeRecordingIdMismatch);
        }
        let sequence = u64::from_le_bytes([
            header[36], header[37], header[38], header[39], header[40], header[41], header[42],
            header[43],
        ]);
        if sequence != self.next_sequence {
            return Err(WalError::UnexpectedSequence {
                expected: self.next_sequence,
                actual: sequence,
            });
        }
        let next_sequence = sequence
            .checked_add(1)
            .ok_or(WalError::SequenceExhausted { sequence })?;
        let expected = u32::from_le_bytes([header[44], header[45], header[46], header[47]]);
        let payload_len = usize::try_from(payload_len).map_err(|_| WalError::RecordTooLarge {
            record_bytes: usize::MAX,
            max_record_bytes: self.max_record_bytes,
        })?;
        let record_bytes = ENVELOPE_HEADER_LEN.saturating_add(payload_len);
        if record_bytes > self.max_record_bytes {
            return Err(WalError::RecordTooLarge {
                record_bytes,
                max_record_bytes: self.max_record_bytes,
            });
        }
        let mut payload = vec![0_u8; payload_len];
        if read_up_to(&mut self.reader, &mut payload)? < payload.len() {
            return Ok(ReadOutcome::PartialTail {
                offset: record_offset,
            });
        }
        let actual = envelope_checksum(&header[..44], &payload);
        if actual != expected {
            return Err(WalError::EnvelopeChecksum {
                sequence,
                kind,
                expected,
                actual,
            });
        }
        self.offset += u64::try_from(record_bytes).unwrap_or(u64::MAX);
        self.next_sequence = next_sequence;
        Ok(ReadOutcome::Record(
            WalRecordEnvelope::unplaced(
                recording_id,
                sequence,
                kind,
                schema_version,
                flags,
                payload,
            )
            .with_provenance(WalRecordProvenance {
                segment_ordinal: self.header.segment_ordinal,
                segment_first_sequence: self.header.first_sequence,
                byte_offset: record_offset,
            }),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredSegment {
    pub path: PathBuf,
    pub header: SegmentHeader,
}

pub fn segment_file_name(first_sequence: u64) -> String {
    format!("{first_sequence:020}.chwal")
}

pub fn discover_segments(
    directory: impl AsRef<Path>,
    recording_id: RecordingId,
) -> Result<Vec<DiscoveredSegment>, WalError> {
    let mut discovered = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| WalError::InvalidSegmentFileName(name.to_string_lossy().into()))?;
        let has_tmp_extension = Path::new(&name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"));
        if name.starts_with('.') || has_tmp_extension || name.contains(".tmp-") {
            continue;
        }
        let digits = name
            .strip_suffix(".chwal")
            .filter(|digits| digits.len() == 20 && digits.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| WalError::InvalidSegmentFileName(name.clone()))?;
        let file_first_sequence = digits
            .parse::<u64>()
            .map_err(|_| WalError::InvalidSegmentFileName(name))?;
        let path = entry.path();
        let mut file = File::open(&path)?;
        let mut bytes = [0_u8; SEGMENT_HEADER_LEN];
        let read = read_up_to(&mut file, &mut bytes)?;
        let header = decode_segment_header(&bytes[..read])?;
        if header.recording_id != recording_id {
            return Err(WalError::SegmentIdentityMismatch("recording ID"));
        }
        discovered.push((file_first_sequence, DiscoveredSegment { path, header }));
    }
    discovered.sort_by_key(|(file_first_sequence, _)| *file_first_sequence);

    let mut previous_first_sequence = None;
    for (index, (_, segment)) in discovered.iter().enumerate() {
        let expected_ordinal = u64::try_from(index).unwrap_or(u64::MAX);
        if segment.header.segment_ordinal < expected_ordinal {
            return Err(WalError::DuplicateSegmentOrdinal(
                segment.header.segment_ordinal,
            ));
        }
        if segment.header.segment_ordinal > expected_ordinal {
            return Err(WalError::MissingSegmentOrdinal {
                expected: expected_ordinal,
                actual: segment.header.segment_ordinal,
            });
        }
        if let Some(previous) = previous_first_sequence {
            if segment.header.first_sequence == previous {
                return Err(WalError::DuplicateSegmentFirstSequence(previous));
            }
            if segment.header.first_sequence < previous {
                return Err(WalError::NonIncreasingSegmentFirstSequence {
                    previous,
                    current: segment.header.first_sequence,
                });
            }
        }
        previous_first_sequence = Some(segment.header.first_sequence);
    }
    for (file_first_sequence, segment) in &discovered {
        if segment.header.first_sequence != *file_first_sequence {
            return Err(WalError::SegmentFileNameMismatch {
                file_first_sequence: *file_first_sequence,
                header_first_sequence: segment.header.first_sequence,
            });
        }
    }

    Ok(discovered.into_iter().map(|(_, segment)| segment).collect())
}

pub struct RecordingLock {
    _file: File,
}

impl RecordingLock {
    pub fn acquire(wal_directory: impl AsRef<Path>) -> Result<Self, WalError> {
        #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
        return Err(WalError::UnsupportedSegmentPublication);

        #[cfg(any(target_os = "linux", target_vendor = "apple"))]
        {
            fs::create_dir_all(wal_directory.as_ref())?;
            fs::set_permissions(wal_directory.as_ref(), fs::Permissions::from_mode(0o700))?;
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true).mode(0o600);
            let file = options.open(wal_directory.as_ref().join("record.lock"))?;
            match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => Ok(Self { _file: file }),
                Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
                    Err(WalError::RecordingLockHeld)
                }
                Err(error) => Err(WalError::Io(io::Error::from_raw_os_error(
                    error.raw_os_error(),
                ))),
            }
        }
    }

    pub fn scan(
        &self,
        wal_directory: impl AsRef<Path>,
        recording_id: RecordingId,
        max_record_bytes: usize,
    ) -> Result<RecoveryScan, WalError> {
        scan_wal_unlocked(wal_directory.as_ref(), recording_id, max_record_bytes)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryPartialTail {
    pub path: PathBuf,
    pub byte_offset: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryScan {
    pub authority: CommitAuthority,
    /// Complete envelopes through final authoritative commit marker, marker included.
    pub committed: Vec<WalRecordEnvelope>,
    pub terminal_wal_losses: Vec<RecoveredTerminalWalLoss>,
    pub final_marker: Option<WalRecordEnvelope>,
    pub uncommitted: Vec<WalRecordEnvelope>,
    pub partial_tail: Option<RecoveryPartialTail>,
    pub authoritative_marker_count: usize,
}

pub fn scan_wal(
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    max_record_bytes: usize,
) -> Result<RecoveryScan, WalError> {
    let _lock = RecordingLock::acquire(&wal_directory)?;
    scan_wal_unlocked(wal_directory.as_ref(), recording_id, max_record_bytes)
}

/// Hashes exact verified WAL v1 segment bytes through final authoritative commit marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedSnapshot {
    pub recording_id: RecordingId,
    pub marker_sequence: u64,
    pub durable_through_sequence: u64,
    pub marker_segment_ordinal: u64,
    pub marker_byte_offset: u64,
    pub marker_digest: [u8; 32],
    pub input_digest: [u8; 32],
    pub segment_count: usize,
    pub committed_record_count: u64,
    pub committed_payload_bytes: u64,
}

/// Marker-capped WAL records for recorder-owned incremental ETL.
///
/// Snapshot read never acquires or mutates the writer lock. A mutable suffix
/// returns retryable uncertainty instead of exposing records to ETL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedRecordSnapshot {
    pub snapshot: CommittedSnapshot,
    pub envelopes: Vec<WalRecordEnvelope>,
}

/// Reads a marker-capped snapshot without taking the writer lock or mutating WAL.
/// Mutable suffixes are reported as retryable uncertainty.
pub fn read_committed_snapshot(
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    max_record_bytes: usize,
) -> Result<CommittedSnapshot, WalError> {
    let wal_directory = wal_directory.as_ref();
    let scan = scan_wal_unlocked(wal_directory, recording_id, max_record_bytes)?;
    committed_snapshot_from_scan(wal_directory, recording_id, &scan)
}

fn committed_snapshot_from_scan(
    wal_directory: &Path,
    recording_id: RecordingId,
    scan: &RecoveryScan,
) -> Result<CommittedSnapshot, WalError> {
    if scan.partial_tail.is_some() || !scan.uncommitted.is_empty() {
        return Err(WalError::RetryableSnapshotUncertainty);
    }
    let marker = scan
        .final_marker
        .as_ref()
        .ok_or(WalError::NoAuthoritativeCommit)?;
    let provenance = marker
        .provenance
        .as_ref()
        .ok_or(WalError::MissingProvenance {
            sequence: marker.sequence,
        })?;
    let marker_bytes = encode_envelope(marker)?;
    let marker_digest = Sha256::digest(marker_bytes).into();
    let input_digest = verified_snapshot_sha256(wal_directory, scan)?;
    let segment_count = usize::try_from(provenance.segment_ordinal)
        .ok()
        .and_then(|ordinal| ordinal.checked_add(1))
        .ok_or(WalError::CommitArithmeticOverflow)?;
    Ok(CommittedSnapshot {
        recording_id,
        marker_sequence: marker.sequence,
        durable_through_sequence: scan
            .authority
            .durable_through_sequence
            .ok_or(WalError::NoAuthoritativeCommit)?,
        marker_segment_ordinal: provenance.segment_ordinal,
        marker_byte_offset: provenance.byte_offset,
        marker_digest,
        input_digest,
        segment_count,
        committed_record_count: scan.authority.durable_record_count,
        committed_payload_bytes: scan.authority.durable_payload_bytes,
    })
}

/// Reads committed envelopes together with their verified snapshot authority.
pub fn read_committed_snapshot_with_records(
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    max_record_bytes: usize,
) -> Result<CommittedRecordSnapshot, WalError> {
    let wal_directory = wal_directory.as_ref();
    let scan = scan_wal_unlocked(wal_directory, recording_id, max_record_bytes)?;
    if scan.partial_tail.is_some() || !scan.uncommitted.is_empty() {
        return Err(WalError::RetryableSnapshotUncertainty);
    }
    let snapshot = committed_snapshot_from_scan(wal_directory, recording_id, &scan)?;
    Ok(CommittedRecordSnapshot {
        snapshot,
        envelopes: scan.committed,
    })
}

pub fn verified_snapshot_sha256(
    wal_directory: impl AsRef<Path>,
    scan: &RecoveryScan,
) -> Result<[u8; 32], WalError> {
    let marker = scan
        .final_marker
        .as_ref()
        .ok_or(WalError::NoAuthoritativeCommit)?;
    let provenance = marker
        .provenance
        .as_ref()
        .ok_or(WalError::MissingProvenance {
            sequence: marker.sequence,
        })?;
    let marker_end = provenance
        .byte_offset
        .checked_add(u64::try_from(COMMIT_MARKER_FRAME_LEN).unwrap_or(u64::MAX))
        .ok_or(WalError::CommitArithmeticOverflow)?;
    let segments = discover_segments(wal_directory.as_ref().join("segments"), marker.recording_id)?;
    let mut digest = Sha256::new();
    let mut found_marker = false;
    for segment in segments {
        if segment.header.segment_ordinal > provenance.segment_ordinal {
            break;
        }
        let bytes = fs::read(&segment.path)?;
        if segment.header.segment_ordinal == provenance.segment_ordinal {
            let end =
                usize::try_from(marker_end).map_err(|_| WalError::CommitArithmeticOverflow)?;
            if bytes.len() < end {
                return Err(WalError::CommitSegmentMismatch);
            }
            digest.update(&bytes[..end]);
            found_marker = true;
            break;
        }
        digest.update(bytes);
    }
    if !found_marker {
        return Err(WalError::CommitSegmentMismatch);
    }
    Ok(digest.finalize().into())
}

fn scan_wal_unlocked(
    wal_directory: &Path,
    recording_id: RecordingId,
    max_record_bytes: usize,
) -> Result<RecoveryScan, WalError> {
    let segments = discover_segments(wal_directory.join("segments"), recording_id)?;
    if segments.is_empty() {
        return Err(WalError::NoPublishedSegments);
    }
    let final_index = segments.len() - 1;
    let mut scan = RecoveryScan::default();
    let mut expected_next_segment_first = None;
    let mut uncommitted_suffix_started = false;

    for (index, segment) in segments.into_iter().enumerate() {
        if let Some(expected) = expected_next_segment_first
            && segment.header.first_sequence != expected
        {
            return Err(WalError::RecoverySegmentFirstSequenceMismatch {
                expected,
                actual: segment.header.first_sequence,
            });
        }
        let mut reader = WalReader::new(
            File::open(&segment.path)?,
            recording_id,
            segment.header.segment_ordinal,
            segment.header.first_sequence,
            max_record_bytes,
        )?;
        let mut pending = Vec::new();
        loop {
            match reader.next_record()? {
                ReadOutcome::Record(envelope) if envelope.kind == RecordKind::CommitMarker => {
                    if uncommitted_suffix_started {
                        return Err(WalError::CommitMarkerAfterUncommitted);
                    }
                    let authority =
                        validate_commit_authority(&scan.authority, &pending, &envelope)?;
                    for terminal in pending
                        .iter()
                        .filter(|record| record.kind == RecordKind::TerminalWalLoss)
                    {
                        scan.terminal_wal_losses.push(RecoveredTerminalWalLoss {
                            sequence: terminal.sequence,
                            loss: decode_terminal_wal_loss(
                                terminal.schema_version,
                                &terminal.payload,
                            )?,
                        });
                    }
                    scan.authority = authority;
                    scan.authoritative_marker_count += 1;
                    scan.committed.append(&mut pending);
                    scan.committed.push(envelope.clone());
                    scan.final_marker = Some(envelope);
                }
                ReadOutcome::Record(envelope) => {
                    if envelope.kind == RecordKind::TerminalWalLoss {
                        decode_terminal_wal_loss(envelope.schema_version, &envelope.payload)?;
                    }
                    pending.push(envelope);
                }
                ReadOutcome::End => break,
                ReadOutcome::PartialTail { offset } => {
                    if index != final_index {
                        return Err(WalError::PartialTailBeforeFinalSegment);
                    }
                    scan.partial_tail = Some(RecoveryPartialTail {
                        path: segment.path.clone(),
                        byte_offset: offset,
                    });
                    break;
                }
            }
        }
        if let Some(last) = pending.last() {
            expected_next_segment_first = Some(
                last.sequence
                    .checked_add(1)
                    .ok_or(WalError::CommitArithmeticOverflow)?,
            );
            uncommitted_suffix_started = true;
        } else {
            expected_next_segment_first = match scan.authority.marker_sequence {
                Some(sequence) => Some(
                    sequence
                        .checked_add(1)
                        .ok_or(WalError::CommitArithmeticOverflow)?,
                ),
                None => None,
            };
        }
        scan.uncommitted.extend(pending);
    }
    Ok(scan)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryRepair {
    pub repaired_bytes: u64,
    pub truncated_to: Option<u64>,
    pub scan: RecoveryScan,
}

pub fn repair_partial_tail(
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    max_record_bytes: usize,
) -> Result<RecoveryRepair, WalError> {
    let _lock = RecordingLock::acquire(&wal_directory)?;
    let scan = scan_wal_unlocked(wal_directory.as_ref(), recording_id, max_record_bytes)?;
    let Some(partial) = scan.partial_tail else {
        return Ok(RecoveryRepair {
            repaired_bytes: 0,
            truncated_to: None,
            scan,
        });
    };
    let original_len = fs::metadata(&partial.path)?.len();
    let file = OpenOptions::new().write(true).open(&partial.path)?;
    file.set_len(partial.byte_offset)?;
    file.sync_data()?;
    sync_directory(
        partial
            .path
            .parent()
            .ok_or_else(|| WalError::Io(io::Error::other("segment has no parent directory")))?,
    )?;
    let repaired_bytes = original_len.saturating_sub(partial.byte_offset);
    let scan = scan_wal_unlocked(wal_directory.as_ref(), recording_id, max_record_bytes)?;
    Ok(RecoveryRepair {
        repaired_bytes,
        truncated_to: Some(partial.byte_offset),
        scan,
    })
}

fn read_up_to(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buffer.len() {
        match reader.read(&mut buffer[total..]) {
            Ok(0) => break,
            Ok(count) => total += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(total)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegmentPublishFault {
    BeforeHeaderSync,
    BeforeRename,
    AfterRename,
}

pub struct SegmentedEnvelopeWriter {
    directory: PathBuf,
    recording_id: RecordingId,
    max_segment_bytes: u64,
    max_total_bytes: u64,
    physical_bytes: u64,
    file: Option<File>,
    segment_ordinal: Option<u64>,
    segment_first_sequence: u64,
    segment_bytes: u64,
    segment_started_millis: Option<u64>,
    last_sequence: Option<u64>,
    #[cfg(test)]
    fault: Option<SegmentPublishFault>,
    #[cfg(any(test, feature = "test-support"))]
    data_sync_count: usize,
    #[cfg(any(test, feature = "test-support"))]
    fail_next_data_sync: bool,
}

impl SegmentedEnvelopeWriter {
    pub fn create(
        directory: impl AsRef<Path>,
        recording_id: RecordingId,
        max_segment_bytes: u64,
    ) -> Result<Self, WalError> {
        Self::create_with_total_limit(
            directory,
            recording_id,
            max_segment_bytes,
            DEFAULT_MAX_WAL_BYTES,
        )
    }

    pub fn create_with_total_limit(
        directory: impl AsRef<Path>,
        recording_id: RecordingId,
        max_segment_bytes: u64,
        max_total_bytes: u64,
    ) -> Result<Self, WalError> {
        if !(MIN_SEGMENT_BYTES..=MAX_SEGMENT_BYTES).contains(&max_segment_bytes) {
            return Err(WalError::InvalidSegmentSizeRange);
        }
        if max_total_bytes < max_segment_bytes || max_total_bytes > DEFAULT_MAX_WAL_BYTES {
            return Err(WalError::InvalidTotalWalSize);
        }
        #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
        return Err(WalError::UnsupportedSegmentPublication);

        #[cfg(any(target_os = "linux", target_vendor = "apple"))]
        {
            fs::create_dir_all(directory.as_ref())?;
            fs::set_permissions(directory.as_ref(), fs::Permissions::from_mode(0o700))?;
            let physical_bytes = directory_physical_bytes(directory.as_ref())?;
            if physical_bytes > max_total_bytes {
                return Err(WalError::WalCapacityExceeded {
                    required: physical_bytes,
                    limit: max_total_bytes,
                });
            }
            Ok(Self {
                directory: directory.as_ref().to_path_buf(),
                recording_id,
                max_segment_bytes,
                max_total_bytes,
                physical_bytes,
                file: None,
                segment_ordinal: None,
                segment_first_sequence: 0,
                segment_bytes: 0,
                segment_started_millis: None,
                last_sequence: None,
                #[cfg(test)]
                fault: None,
                #[cfg(any(test, feature = "test-support"))]
                data_sync_count: 0,
                #[cfg(any(test, feature = "test-support"))]
                fail_next_data_sync: false,
            })
        }
    }

    fn reopen_existing(
        directory: impl AsRef<Path>,
        recording_id: RecordingId,
        max_segment_bytes: u64,
        max_total_bytes: u64,
        final_marker: &WalRecordEnvelope,
        now_millis: u64,
    ) -> Result<Self, WalError> {
        if !(MIN_SEGMENT_BYTES..=MAX_SEGMENT_BYTES).contains(&max_segment_bytes) {
            return Err(WalError::InvalidSegmentSizeRange);
        }
        if max_total_bytes < max_segment_bytes || max_total_bytes > DEFAULT_MAX_WAL_BYTES {
            return Err(WalError::InvalidTotalWalSize);
        }
        let provenance = final_marker
            .provenance
            .as_ref()
            .ok_or(WalError::MissingProvenance {
                sequence: final_marker.sequence,
            })?;
        let discovered = discover_segments(&directory, recording_id)?;
        let final_segment = discovered.last().ok_or(WalError::NoPublishedSegments)?;
        final_segment.header.validate_identity(
            recording_id,
            provenance.segment_ordinal,
            provenance.segment_first_sequence,
        )?;
        let segment_bytes = fs::metadata(&final_segment.path)?.len();
        if segment_bytes > max_segment_bytes {
            return Err(WalError::EnvelopeExceedsSegment {
                record_bytes: segment_bytes,
                segment_bytes: max_segment_bytes,
            });
        }
        let physical_bytes = directory_physical_bytes(directory.as_ref())?;
        if physical_bytes > max_total_bytes {
            return Err(WalError::WalCapacityExceeded {
                required: physical_bytes,
                limit: max_total_bytes,
            });
        }
        let mut options = OpenOptions::new();
        options.append(true).write(true).mode(0o600);
        Ok(Self {
            directory: directory.as_ref().to_path_buf(),
            recording_id,
            max_segment_bytes,
            max_total_bytes,
            physical_bytes,
            file: Some(options.open(&final_segment.path)?),
            segment_ordinal: Some(provenance.segment_ordinal),
            segment_first_sequence: provenance.segment_first_sequence,
            segment_bytes,
            segment_started_millis: Some(now_millis),
            last_sequence: Some(final_marker.sequence),
            #[cfg(test)]
            fault: None,
            #[cfg(any(test, feature = "test-support"))]
            data_sync_count: 0,
            #[cfg(any(test, feature = "test-support"))]
            fail_next_data_sync: false,
        })
    }

    #[cfg(test)]
    fn inject_publish_fault(&mut self, fault: SegmentPublishFault) {
        self.fault = Some(fault);
    }

    #[cfg(test)]
    fn fail_if_injected(&mut self, fault: SegmentPublishFault) -> Result<(), WalError> {
        if self.fault == Some(fault) {
            self.fault = None;
            return Err(WalError::Io(io::Error::other(
                "injected segment publication failure",
            )));
        }
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    fn inject_data_sync_failure(&mut self) {
        self.fail_next_data_sync = true;
    }

    #[cfg(any(test, feature = "test-support"))]
    fn data_sync_count(&self) -> usize {
        self.data_sync_count
    }

    fn would_rotate(&self, encoded_len: u64) -> bool {
        self.file.is_some()
            && self.segment_bytes.saturating_add(encoded_len) > self.max_segment_bytes
    }

    fn age_expired(&self, now_millis: u64, max_age_millis: u64) -> bool {
        self.file.is_some()
            && self.segment_bytes > SEGMENT_HEADER_LEN_U64
            && self
                .segment_started_millis
                .is_some_and(|started| now_millis.saturating_sub(started) >= max_age_millis)
    }

    fn ensure_total_capacity(&self, additional_bytes: u64) -> Result<(), WalError> {
        let required = self.physical_bytes.checked_add(additional_bytes).ok_or(
            WalError::WalCapacityExceeded {
                required: u64::MAX,
                limit: self.max_total_bytes,
            },
        )?;
        if required > self.max_total_bytes {
            return Err(WalError::WalCapacityExceeded {
                required,
                limit: self.max_total_bytes,
            });
        }
        Ok(())
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    fn rotate(&mut self, first_sequence: u64) -> Result<(), WalError> {
        self.rotate_at(first_sequence, 0)
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    fn rotate_at(&mut self, first_sequence: u64, now_millis: u64) -> Result<(), WalError> {
        self.ensure_total_capacity(SEGMENT_HEADER_LEN_U64)?;
        let segment_ordinal = match self.segment_ordinal {
            Some(ordinal) => ordinal
                .checked_add(1)
                .ok_or(WalError::SegmentOrdinalExhausted)?,
            None => 0,
        };
        let final_path = self.directory.join(segment_file_name(first_sequence));
        let temp_path = self.directory.join(format!(
            ".{}.tmp-{}",
            segment_file_name(first_sequence),
            uuid::Uuid::new_v4()
        ));
        let header = SegmentHeader {
            recording_id: self.recording_id,
            segment_ordinal,
            first_sequence,
            created_unix_nanos: unix_nanos_now(),
        };
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut temp = options.open(&temp_path)?;
        let mut published = false;
        let result = (|| {
            temp.write_all(&encode_segment_header(&header))?;
            #[cfg(test)]
            self.fail_if_injected(SegmentPublishFault::BeforeHeaderSync)?;
            temp.sync_all()?;
            #[cfg(test)]
            self.fail_if_injected(SegmentPublishFault::BeforeRename)?;
            renameat_with(CWD, &temp_path, CWD, &final_path, RenameFlags::NOREPLACE)
                .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))?;
            published = true;
            #[cfg(test)]
            self.fail_if_injected(SegmentPublishFault::AfterRename)?;
            sync_directory(&self.directory)
        })();
        if result.is_err() && !published {
            let _ = fs::remove_file(&temp_path);
        }
        result?;

        let mut options = OpenOptions::new();
        options.append(true).write(true).mode(0o600);
        self.file = Some(options.open(final_path)?);
        self.segment_ordinal = Some(segment_ordinal);
        self.segment_first_sequence = first_sequence;
        self.segment_bytes = SEGMENT_HEADER_LEN_U64;
        self.segment_started_millis = Some(now_millis);
        self.physical_bytes += SEGMENT_HEADER_LEN_U64;
        Ok(())
    }

    pub fn append(
        &mut self,
        envelope: &WalRecordEnvelope,
    ) -> Result<WalRecordProvenance, WalError> {
        envelope
            .sequence
            .checked_add(1)
            .ok_or(WalError::SequenceExhausted {
                sequence: envelope.sequence,
            })?;
        if envelope.recording_id != self.recording_id {
            return Err(WalError::WriterRecordingIdMismatch);
        }
        if envelope.provenance.is_some() {
            return Err(WalError::EnvelopeAlreadyPlaced);
        }
        if let Some(previous) = self.last_sequence {
            let expected = previous
                .checked_add(1)
                .ok_or(WalError::SequenceExhausted { sequence: previous })?;
            if envelope.sequence != expected {
                return Err(WalError::UnexpectedSequence {
                    expected,
                    actual: envelope.sequence,
                });
            }
        }
        let record_bytes = ENVELOPE_HEADER_LEN
            .checked_add(envelope.payload.len())
            .ok_or(WalError::RecordTooLarge {
                record_bytes: usize::MAX,
                max_record_bytes: DEFAULT_MAX_RECORD_BYTES,
            })?;
        if record_bytes > DEFAULT_MAX_RECORD_BYTES {
            return Err(WalError::RecordTooLarge {
                record_bytes,
                max_record_bytes: DEFAULT_MAX_RECORD_BYTES,
            });
        }
        let encoded = encode_envelope(envelope)?;
        let encoded_len = u64::try_from(encoded.len())
            .map_err(|_| WalError::PayloadTooLarge(envelope.payload.len()))?;
        if SEGMENT_HEADER_LEN_U64.saturating_add(encoded_len) > self.max_segment_bytes {
            return Err(WalError::EnvelopeExceedsSegment {
                record_bytes: encoded_len,
                segment_bytes: self.max_segment_bytes,
            });
        }
        let header_bytes = if self.file.is_none()
            || self.segment_bytes.saturating_add(encoded_len) > self.max_segment_bytes
        {
            SEGMENT_HEADER_LEN_U64
        } else {
            0
        };
        self.ensure_total_capacity(header_bytes.saturating_add(encoded_len))?;
        if self.file.is_none()
            || self.segment_bytes.saturating_add(encoded_len) > self.max_segment_bytes
        {
            #[cfg(any(target_os = "linux", target_vendor = "apple"))]
            self.rotate(envelope.sequence)?;
            #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
            return Err(WalError::UnsupportedSegmentPublication);
        }
        let byte_offset = self.segment_bytes;
        self.file
            .as_mut()
            .ok_or(WalError::NoActiveSegment)?
            .write_all(&encoded)?;
        self.segment_bytes += encoded_len;
        self.physical_bytes += encoded_len;
        self.last_sequence = Some(envelope.sequence);
        Ok(WalRecordProvenance {
            segment_ordinal: self.segment_ordinal.ok_or(WalError::NoActiveSegment)?,
            segment_first_sequence: self.segment_first_sequence,
            byte_offset,
        })
    }

    pub fn flush(&mut self) -> Result<(), WalError> {
        if let Some(file) = self.file.as_mut() {
            #[cfg(any(test, feature = "test-support"))]
            {
                self.data_sync_count += 1;
                if self.fail_next_data_sync {
                    self.fail_next_data_sync = false;
                    return Err(WalError::Io(io::Error::other("injected data sync failure")));
                }
            }
            file.sync_data()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableBatch {
    pub marker_sequence: u64,
    pub durable_through_sequence: u64,
    pub durable_record_count: u64,
    pub durable_payload_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupAppendResult {
    pub sequence: u64,
    pub durable_batches: Vec<DurableBatch>,
}

pub struct GroupCommitWalWriter {
    writer: SegmentedEnvelopeWriter,
    lock: Option<RecordingLock>,
    recording_id: RecordingId,
    next_sequence: u64,
    pending: Vec<WalRecordEnvelope>,
    pending_frame_bytes: u64,
    last_sync_millis: u64,
    max_segment_age_millis: u64,
    authority: CommitAuthority,
}

impl GroupCommitWalWriter {
    pub fn create(
        directory: impl AsRef<Path>,
        recording_id: RecordingId,
        max_segment_bytes: u64,
        first_sequence: u64,
        now_millis: u64,
    ) -> Result<Self, WalError> {
        Self::create_with_total_limit(
            directory,
            recording_id,
            max_segment_bytes,
            DEFAULT_MAX_WAL_BYTES,
            first_sequence,
            now_millis,
        )
    }

    pub fn create_with_total_limit(
        directory: impl AsRef<Path>,
        recording_id: RecordingId,
        max_segment_bytes: u64,
        max_total_bytes: u64,
        first_sequence: u64,
        now_millis: u64,
    ) -> Result<Self, WalError> {
        Self::create_with_total_limit_and_age(
            directory,
            recording_id,
            max_segment_bytes,
            max_total_bytes,
            first_sequence,
            now_millis,
            DEFAULT_SEGMENT_AGE_MILLIS,
        )
    }

    pub fn create_with_total_limit_and_age(
        directory: impl AsRef<Path>,
        recording_id: RecordingId,
        max_segment_bytes: u64,
        max_total_bytes: u64,
        first_sequence: u64,
        now_millis: u64,
        max_segment_age_millis: u64,
    ) -> Result<Self, WalError> {
        if max_segment_age_millis == 0 {
            return Err(WalError::InvalidSegmentAge);
        }
        if !(MIN_SEGMENT_BYTES..=MAX_SEGMENT_BYTES).contains(&max_segment_bytes) {
            return Err(WalError::InvalidSegmentSizeRange);
        }
        if max_total_bytes < max_segment_bytes || max_total_bytes > DEFAULT_MAX_WAL_BYTES {
            return Err(WalError::InvalidTotalWalSize);
        }
        let wal_directory = directory.as_ref();
        let lock = RecordingLock::acquire(wal_directory)?;
        Ok(Self {
            writer: SegmentedEnvelopeWriter::create_with_total_limit(
                wal_directory.join("segments"),
                recording_id,
                max_segment_bytes,
                max_total_bytes,
            )?,
            lock: Some(lock),
            recording_id,
            next_sequence: first_sequence,
            pending: Vec::new(),
            pending_frame_bytes: 0,
            last_sync_millis: now_millis,
            max_segment_age_millis,
            authority: CommitAuthority::default(),
        })
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn recording_id(&self) -> RecordingId {
        self.recording_id
    }

    pub fn authority(&self) -> &CommitAuthority {
        &self.authority
    }

    ///
    /// # Panics
    ///
    /// Panics only if internal rollover has dropped ownership unexpectedly.
    pub fn recording_lock(&self) -> &RecordingLock {
        self.lock
            .as_ref()
            .expect("group commit writer always owns recording lock")
    }

    /// Seal current epoch, release its lock, and acquire same-recording lock for next epoch.
    pub fn rollover(&mut self, now_millis: u64, first_sequence: u64) -> Result<(), WalError> {
        self.shutdown(now_millis)?;
        let wal_directory = self
            .writer
            .directory
            .parent()
            .ok_or_else(|| WalError::Io(io::Error::other("WAL directory has no parent")))?
            .to_path_buf();
        let max_segment_bytes = self.writer.max_segment_bytes;
        let max_total_bytes = self.writer.max_total_bytes;
        let max_segment_age_millis = self.max_segment_age_millis;
        self.lock.take();
        let lock = RecordingLock::acquire(&wal_directory)?;
        let segments_directory = wal_directory.join("segments");
        let mut writer = SegmentedEnvelopeWriter::create_with_total_limit(
            &segments_directory,
            self.recording_id,
            max_segment_bytes,
            max_total_bytes,
        )?;
        writer.segment_ordinal = discover_segments(&segments_directory, self.recording_id)?
            .into_iter()
            .map(|segment| segment.header.segment_ordinal)
            .max();
        self.writer = writer;
        self.lock = Some(lock);
        self.next_sequence = first_sequence;
        self.pending.clear();
        self.pending_frame_bytes = 0;
        self.last_sync_millis = now_millis;
        self.max_segment_age_millis = max_segment_age_millis;
        let previous_authority = self.authority.clone();
        self.authority = CommitAuthority {
            durable_record_count: previous_authority.durable_record_count,
            durable_payload_bytes: previous_authority.durable_payload_bytes,
            ..CommitAuthority::default()
        };
        Ok(())
    }

    pub fn set_max_segment_age_millis(
        &mut self,
        max_segment_age_millis: u64,
    ) -> Result<(), WalError> {
        if max_segment_age_millis == 0 {
            return Err(WalError::InvalidSegmentAge);
        }
        self.max_segment_age_millis = max_segment_age_millis;
        Ok(())
    }

    pub fn max_segment_age_millis(&self) -> u64 {
        self.max_segment_age_millis
    }

    pub fn persist_manifest(&self, manifest: &WalManifestV1) -> Result<PathBuf, WalError> {
        if manifest.recording_id != self.recording_id {
            return Err(ManifestError::Contradiction("recording_id").into());
        }
        if let Some(marker) = &manifest.authoritative_marker
            && self
                .authority
                .marker_sequence
                .is_none_or(|sequence| marker.sequence > sequence)
        {
            return Err(ManifestError::Contradiction("authoritative_marker").into());
        }
        let wal_directory = self
            .writer
            .directory
            .parent()
            .ok_or(ManifestError::Contradiction("wal_directory"))?;
        Ok(write_manifest_atomic(wal_directory, manifest)?)
    }

    pub fn append(
        &mut self,
        kind: RecordKind,
        schema_version: u16,
        flags: u16,
        payload: Vec<u8>,
        now_millis: u64,
    ) -> Result<GroupAppendResult, WalError> {
        if kind == RecordKind::CommitMarker {
            return Err(WalError::InvalidCommitMarkerEnvelope);
        }
        if kind == RecordKind::TerminalWalLoss {
            decode_terminal_wal_loss(schema_version, &payload)?;
        }
        let payload_len =
            u64::try_from(payload.len()).map_err(|_| WalError::PayloadTooLarge(payload.len()))?;
        let frame_len = u64::try_from(ENVELOPE_HEADER_LEN)
            .unwrap_or(u64::MAX)
            .checked_add(payload_len)
            .ok_or(WalError::CommitArithmeticOverflow)?;
        let max_record_bytes = u64::try_from(DEFAULT_MAX_RECORD_BYTES).unwrap_or(u64::MAX);
        if frame_len > max_record_bytes {
            return Err(WalError::RecordTooLarge {
                record_bytes: usize::try_from(frame_len).unwrap_or(usize::MAX),
                max_record_bytes: DEFAULT_MAX_RECORD_BYTES,
            });
        }
        let marker_len = u64::try_from(COMMIT_MARKER_FRAME_LEN).unwrap_or(u64::MAX);
        let pair_len = frame_len
            .checked_add(marker_len)
            .ok_or(WalError::CommitArithmeticOverflow)?;
        if SEGMENT_HEADER_LEN_U64.saturating_add(pair_len) > self.writer.max_segment_bytes {
            return Err(WalError::EnvelopeExceedsSegment {
                record_bytes: frame_len,
                segment_bytes: self.writer.max_segment_bytes,
            });
        }
        let rotate_before_data = self.writer.would_rotate(pair_len)
            || self
                .writer
                .age_expired(now_millis, self.max_segment_age_millis);
        let sequence_advance = if rotate_before_data && !self.pending.is_empty() {
            3
        } else {
            2
        };
        self.next_sequence
            .checked_add(sequence_advance)
            .ok_or(WalError::SequenceExhausted {
                sequence: self.next_sequence,
            })?;
        let old_marker_len = if rotate_before_data && !self.pending.is_empty() {
            marker_len
        } else {
            0
        };
        let header_len = if self.writer.file.is_none() || rotate_before_data {
            SEGMENT_HEADER_LEN_U64
        } else {
            0
        };
        self.writer.ensure_total_capacity(
            old_marker_len
                .checked_add(header_len)
                .and_then(|required| required.checked_add(pair_len))
                .ok_or(WalError::CommitArithmeticOverflow)?,
        )?;

        let mut durable_batches = Vec::new();
        if rotate_before_data
            && !self.pending.is_empty()
            && let Some(durable) = self.commit(now_millis)?
        {
            durable_batches.push(durable);
        }
        if rotate_before_data {
            #[cfg(any(target_os = "linux", target_vendor = "apple"))]
            self.writer.rotate_at(self.next_sequence, now_millis)?;
            #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
            return Err(WalError::UnsupportedSegmentPublication);
        }

        let sequence = self.next_sequence;
        let envelope = WalRecordEnvelope::unplaced(
            self.recording_id,
            sequence,
            kind,
            schema_version,
            flags,
            payload,
        );
        let provenance = self.writer.append(&envelope)?;
        self.pending.push(envelope.with_provenance(provenance));
        self.pending_frame_bytes = self
            .pending_frame_bytes
            .checked_add(frame_len)
            .ok_or(WalError::CommitArithmeticOverflow)?;
        self.next_sequence = sequence
            .checked_add(1)
            .ok_or(WalError::SequenceExhausted { sequence })?;

        if (self.pending_frame_bytes >= GROUP_COMMIT_BYTES
            || now_millis.saturating_sub(self.last_sync_millis) >= GROUP_COMMIT_INTERVAL_MILLIS)
            && let Some(durable) = self.commit(now_millis)?
        {
            durable_batches.push(durable);
        }
        Ok(GroupAppendResult {
            sequence,
            durable_batches,
        })
    }

    pub fn poll(&mut self, now_millis: u64) -> Result<Option<DurableBatch>, WalError> {
        let age_expired = self
            .writer
            .age_expired(now_millis, self.max_segment_age_millis);
        if age_expired {
            let durable = if self.pending.is_empty() {
                None
            } else {
                self.commit(now_millis)?
            };
            #[cfg(any(target_os = "linux", target_vendor = "apple"))]
            {
                self.writer.ensure_total_capacity(SEGMENT_HEADER_LEN_U64)?;
                self.writer.rotate_at(self.next_sequence, now_millis)?;
            }
            #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
            return Err(WalError::UnsupportedSegmentPublication);
            return Ok(durable);
        }
        if self.pending.is_empty()
            || now_millis.saturating_sub(self.last_sync_millis) < GROUP_COMMIT_INTERVAL_MILLIS
        {
            return Ok(None);
        }
        self.commit(now_millis)
    }

    pub fn flush(&mut self, now_millis: u64) -> Result<Option<DurableBatch>, WalError> {
        self.commit(now_millis)
    }

    pub fn shutdown(&mut self, now_millis: u64) -> Result<Option<DurableBatch>, WalError> {
        self.commit(now_millis)
    }

    fn commit(&mut self, now_millis: u64) -> Result<Option<DurableBatch>, WalError> {
        if self.pending.is_empty() {
            return Ok(None);
        }
        let marker = commit_marker_for_batch(&self.authority, &self.pending)?;
        let next_sequence =
            marker
                .batch_last_sequence
                .checked_add(2)
                .ok_or(WalError::SequenceExhausted {
                    sequence: marker.batch_last_sequence,
                })?;
        let marker_envelope = commit_marker_envelope(self.recording_id, &marker)?;
        let marker_len = u64::try_from(COMMIT_MARKER_FRAME_LEN).unwrap_or(u64::MAX);
        if self.writer.would_rotate(marker_len) {
            return Err(WalError::CommitMarkerWouldRotate);
        }
        let provenance = self.writer.append(&marker_envelope)?;
        let marker_envelope = marker_envelope.with_provenance(provenance);
        let authority =
            validate_commit_authority(&self.authority, &self.pending, &marker_envelope)?;
        self.writer.flush()?;

        self.next_sequence = next_sequence;
        self.pending.clear();
        self.pending_frame_bytes = 0;
        self.last_sync_millis = now_millis;
        self.authority = authority;
        Ok(Some(DurableBatch {
            marker_sequence: marker_envelope.sequence,
            durable_through_sequence: marker.durable_through_sequence,
            durable_record_count: marker.durable_record_count,
            durable_payload_bytes: marker.durable_payload_bytes,
        }))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn inject_data_sync_failure_for_test(&mut self) {
        self.writer.inject_data_sync_failure();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn data_sync_count_for_test(&self) -> usize {
        self.writer.data_sync_count()
    }

    #[cfg(test)]
    fn inject_data_sync_failure(&mut self) {
        self.inject_data_sync_failure_for_test();
    }

    #[cfg(test)]
    fn data_sync_count(&self) -> usize {
        self.data_sync_count_for_test()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryReopenPreview {
    pub scan_before: RecoveryScan,
    pub discarded_complete_records: u64,
    pub discarded_payload_bytes: u64,
    pub truncated_bytes: u64,
    pub truncated_to: Option<u64>,
    pub removed_segments: usize,
    pub next_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryReopenReport {
    pub scan_before: RecoveryScan,
    pub scan_after: RecoveryScan,
    pub discarded_complete_records: u64,
    pub discarded_payload_bytes: u64,
    pub truncated_bytes: u64,
    pub truncated_to: Option<u64>,
    pub removed_segments: usize,
    pub next_sequence: u64,
}

pub struct PreparedRecoveryReopen {
    lock: RecordingLock,
    wal_directory: PathBuf,
    recording_id: RecordingId,
    max_segment_bytes: u64,
    max_total_bytes: u64,
    now_millis: u64,
    marker_segment_path: PathBuf,
    truncate_to: Option<u64>,
    remove_paths: Vec<PathBuf>,
    preview: RecoveryReopenPreview,
}

impl PreparedRecoveryReopen {
    pub fn lock(&self) -> &RecordingLock {
        &self.lock
    }

    pub fn preview(&self) -> &RecoveryReopenPreview {
        &self.preview
    }

    pub fn apply(self) -> Result<(GroupCommitWalWriter, RecoveryReopenReport), WalError> {
        if let Some(truncate_to) = self.truncate_to {
            let file = OpenOptions::new()
                .write(true)
                .open(&self.marker_segment_path)?;
            file.set_len(truncate_to)?;
            file.sync_data()?;
        }
        for path in &self.remove_paths {
            fs::remove_file(path)?;
        }
        let segments_directory = self.wal_directory.join("segments");
        if self.truncate_to.is_some() || !self.remove_paths.is_empty() {
            sync_directory(&segments_directory)?;
        }

        let scan_after = self.lock.scan(
            &self.wal_directory,
            self.recording_id,
            DEFAULT_MAX_RECORD_BYTES,
        )?;
        if !scan_after.uncommitted.is_empty() || scan_after.partial_tail.is_some() {
            return Err(WalError::CommitBoundaryMismatch {
                expected: self.preview.next_sequence.saturating_sub(1),
                actual: scan_after
                    .uncommitted
                    .first()
                    .map_or(self.preview.next_sequence, |envelope| envelope.sequence),
            });
        }
        let retained_marker = scan_after
            .final_marker
            .as_ref()
            .ok_or(WalError::NoAuthoritativeCommit)?;
        let writer = SegmentedEnvelopeWriter::reopen_existing(
            &segments_directory,
            self.recording_id,
            self.max_segment_bytes,
            self.max_total_bytes,
            retained_marker,
            self.now_millis,
        )?;
        let group_writer = GroupCommitWalWriter {
            writer,
            lock: Some(self.lock),
            recording_id: self.recording_id,
            next_sequence: self.preview.next_sequence,
            pending: Vec::new(),
            pending_frame_bytes: 0,
            last_sync_millis: self.now_millis,
            max_segment_age_millis: DEFAULT_SEGMENT_AGE_MILLIS,
            authority: scan_after.authority.clone(),
        };
        let report = RecoveryReopenReport {
            scan_before: self.preview.scan_before,
            scan_after,
            discarded_complete_records: self.preview.discarded_complete_records,
            discarded_payload_bytes: self.preview.discarded_payload_bytes,
            truncated_bytes: self.preview.truncated_bytes,
            truncated_to: self.preview.truncated_to,
            removed_segments: self.preview.removed_segments,
            next_sequence: self.preview.next_sequence,
        };
        Ok((group_writer, report))
    }
}

pub fn prepare_group_commit_reopen(
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    max_segment_bytes: u64,
    max_total_bytes: u64,
    now_millis: u64,
) -> Result<PreparedRecoveryReopen, WalError> {
    let wal_directory = wal_directory.as_ref();
    let lock = RecordingLock::acquire(wal_directory)?;
    let scan = lock.scan(wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES)?;
    prepare_group_commit_reopen_from_scan(
        lock,
        wal_directory,
        recording_id,
        scan,
        max_segment_bytes,
        max_total_bytes,
        now_millis,
    )
}

#[allow(clippy::too_many_lines)] // All preconditions must pass before token permits mutation.
pub fn prepare_group_commit_reopen_from_scan(
    lock: RecordingLock,
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    scan_before: RecoveryScan,
    max_segment_bytes: u64,
    max_total_bytes: u64,
    now_millis: u64,
) -> Result<PreparedRecoveryReopen, WalError> {
    if !(MIN_SEGMENT_BYTES..=MAX_SEGMENT_BYTES).contains(&max_segment_bytes) {
        return Err(WalError::InvalidSegmentSizeRange);
    }
    if max_total_bytes < max_segment_bytes || max_total_bytes > DEFAULT_MAX_WAL_BYTES {
        return Err(WalError::InvalidTotalWalSize);
    }
    let wal_directory = wal_directory.as_ref();
    let final_marker = scan_before
        .final_marker
        .as_ref()
        .ok_or(WalError::NoAuthoritativeCommit)?;
    let marker_provenance =
        final_marker
            .provenance
            .as_ref()
            .ok_or(WalError::MissingProvenance {
                sequence: final_marker.sequence,
            })?;
    let next_sequence =
        final_marker
            .sequence
            .checked_add(1)
            .ok_or(WalError::SequenceExhausted {
                sequence: final_marker.sequence,
            })?;
    let segments_directory = wal_directory.join("segments");
    let discovered = discover_segments(&segments_directory, recording_id)?;
    let marker_segment = discovered
        .iter()
        .find(|segment| segment.header.segment_ordinal == marker_provenance.segment_ordinal)
        .ok_or(WalError::CommitSegmentMismatch)?;
    marker_segment.header.validate_identity(
        recording_id,
        marker_provenance.segment_ordinal,
        marker_provenance.segment_first_sequence,
    )?;
    let marker_segment_original_len = fs::metadata(&marker_segment.path)?.len();
    let mut marker_segment_retained_len = marker_segment_original_len;
    if let Some(first_uncommitted) = scan_before.uncommitted.first() {
        let provenance =
            first_uncommitted
                .provenance
                .as_ref()
                .ok_or(WalError::MissingProvenance {
                    sequence: first_uncommitted.sequence,
                })?;
        if provenance.segment_ordinal == marker_provenance.segment_ordinal {
            marker_segment_retained_len = provenance.byte_offset;
        } else if provenance.segment_ordinal < marker_provenance.segment_ordinal {
            return Err(WalError::CommitSegmentMismatch);
        }
    } else if let Some(partial) = &scan_before.partial_tail
        && partial.path == marker_segment.path
    {
        marker_segment_retained_len = partial.byte_offset;
    }
    if marker_segment_retained_len > max_segment_bytes {
        return Err(WalError::EnvelopeExceedsSegment {
            record_bytes: marker_segment_retained_len,
            segment_bytes: max_segment_bytes,
        });
    }
    let remove_paths: Vec<_> = discovered
        .iter()
        .filter(|segment| segment.header.segment_ordinal > marker_provenance.segment_ordinal)
        .map(|segment| segment.path.clone())
        .collect();
    let removed_segment_bytes = remove_paths.iter().try_fold(0_u64, |total, path| {
        total
            .checked_add(fs::metadata(path)?.len())
            .ok_or(WalError::CommitArithmeticOverflow)
    })?;
    let truncated_bytes = marker_segment_original_len
        .checked_sub(marker_segment_retained_len)
        .ok_or(WalError::CommitArithmeticOverflow)?;
    let projected_physical_bytes = directory_physical_bytes(&segments_directory)?
        .checked_sub(removed_segment_bytes)
        .and_then(|bytes| bytes.checked_sub(truncated_bytes))
        .ok_or(WalError::CommitArithmeticOverflow)?;
    if projected_physical_bytes > max_total_bytes {
        return Err(WalError::WalCapacityExceeded {
            required: projected_physical_bytes,
            limit: max_total_bytes,
        });
    }
    OpenOptions::new().append(true).open(&marker_segment.path)?;
    let discarded_complete_records = u64::try_from(scan_before.uncommitted.len())
        .map_err(|_| WalError::CommitArithmeticOverflow)?;
    let discarded_payload_bytes =
        scan_before
            .uncommitted
            .iter()
            .try_fold(0_u64, |total, envelope| {
                total
                    .checked_add(
                        u64::try_from(envelope.payload.len())
                            .map_err(|_| WalError::CommitArithmeticOverflow)?,
                    )
                    .ok_or(WalError::CommitArithmeticOverflow)
            })?;
    let preview = RecoveryReopenPreview {
        scan_before,
        discarded_complete_records,
        discarded_payload_bytes,
        truncated_bytes,
        truncated_to: (marker_segment_retained_len < marker_segment_original_len)
            .then_some(marker_segment_retained_len),
        removed_segments: remove_paths.len(),
        next_sequence,
    };
    Ok(PreparedRecoveryReopen {
        lock,
        wal_directory: wal_directory.to_path_buf(),
        recording_id,
        max_segment_bytes,
        max_total_bytes,
        now_millis,
        marker_segment_path: marker_segment.path.clone(),
        truncate_to: (marker_segment_retained_len < marker_segment_original_len)
            .then_some(marker_segment_retained_len),
        remove_paths,
        preview,
    })
}

fn commit_marker_for_batch(
    previous: &CommitAuthority,
    batch: &[WalRecordEnvelope],
) -> Result<CommitMarker, WalError> {
    let first = batch.first().ok_or(WalError::EmptyCommitBatch)?;
    let last = batch.last().ok_or(WalError::EmptyCommitBatch)?;
    let batch_record_count =
        u64::try_from(batch.len()).map_err(|_| WalError::CommitArithmeticOverflow)?;
    let batch_payload_bytes = batch.iter().try_fold(0_u64, |total, envelope| {
        total
            .checked_add(
                u64::try_from(envelope.payload.len())
                    .map_err(|_| WalError::CommitArithmeticOverflow)?,
            )
            .ok_or(WalError::CommitArithmeticOverflow)
    })?;
    Ok(CommitMarker {
        durable_through_sequence: last.sequence,
        durable_record_count: previous
            .durable_record_count
            .checked_add(batch_record_count)
            .ok_or(WalError::CommitArithmeticOverflow)?,
        durable_payload_bytes: previous
            .durable_payload_bytes
            .checked_add(batch_payload_bytes)
            .ok_or(WalError::CommitArithmeticOverflow)?,
        batch_first_sequence: first.sequence,
        batch_last_sequence: last.sequence,
        batch_sha256: batch_sha256(batch)?,
    })
}

fn unix_nanos_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX),
        Err(error) => -i64::try_from(error.duration().as_nanos()).unwrap_or(i64::MAX),
    }
}

fn directory_physical_bytes(path: &Path) -> Result<u64, WalError> {
    fs::read_dir(path)?.try_fold(0_u64, |total, entry| {
        let metadata = entry?.metadata()?;
        total
            .checked_add(if metadata.is_file() {
                metadata.len()
            } else {
                0
            })
            .ok_or(WalError::WalCapacityExceeded {
                required: u64::MAX,
                limit: DEFAULT_MAX_WAL_BYTES,
            })
    })
}

fn sync_directory(path: &Path) -> Result<(), WalError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn segment_header() -> SegmentHeader {
        SegmentHeader {
            recording_id: RecordingId::new(),
            segment_ordinal: 4,
            first_sequence: 7,
            created_unix_nanos: 123_456_789,
        }
    }

    #[test]
    fn segment_header_round_trip_and_identity_validation() {
        let header = segment_header();
        let bytes = encode_segment_header(&header);
        assert_eq!(&bytes[..4], b"CHS1");
        assert_eq!(
            u16::from_le_bytes(bytes[4..6].try_into().unwrap()),
            WAL_FORMAT_VERSION
        );
        assert_eq!(bytes.len(), SEGMENT_HEADER_LEN);
        assert_eq!(decode_segment_header(&bytes).unwrap(), header);
        header.validate_identity(header.recording_id, 4, 7).unwrap();
        assert!(matches!(
            header.validate_identity(RecordingId::new(), 4, 7),
            Err(WalError::SegmentIdentityMismatch("recording ID"))
        ));
        assert!(matches!(
            header.validate_identity(header.recording_id, 5, 7),
            Err(WalError::SegmentIdentityMismatch("segment ordinal"))
        ));
        assert!(matches!(
            header.validate_identity(header.recording_id, 4, 8),
            Err(WalError::SegmentIdentityMismatch("first sequence"))
        ));
    }

    #[test]
    fn segment_header_rejects_invalid_fields_and_truncation() {
        let encoded = encode_segment_header(&segment_header());

        for length in [0, SEGMENT_HEADER_LEN - 1] {
            assert!(matches!(
                decode_segment_header(&encoded[..length]),
                Err(WalError::TruncatedSegmentHeader { actual }) if actual == length
            ));
        }

        let mut invalid = encoded;
        invalid[0] ^= 0xff;
        assert!(matches!(
            decode_segment_header(&invalid),
            Err(WalError::InvalidSegmentMagic)
        ));

        let mut invalid = encoded;
        invalid[4..6].copy_from_slice(&(WAL_FORMAT_VERSION + 1).to_le_bytes());
        assert!(matches!(
            decode_segment_header(&invalid),
            Err(WalError::UnsupportedVersion(version)) if version == WAL_FORMAT_VERSION + 1
        ));

        let mut invalid = encoded;
        invalid[6..8].copy_from_slice(&(SEGMENT_HEADER_VERSION + 1).to_le_bytes());
        assert!(matches!(
            decode_segment_header(&invalid),
            Err(WalError::UnsupportedSegmentHeaderVersion(version))
                if version == SEGMENT_HEADER_VERSION + 1
        ));

        let mut invalid = encoded;
        invalid[8..12].copy_from_slice(&63_u32.to_le_bytes());
        assert!(matches!(
            decode_segment_header(&invalid),
            Err(WalError::InvalidSegmentHeaderLength(63))
        ));

        let mut invalid = encoded;
        invalid[52] = 1;
        assert!(matches!(
            decode_segment_header(&invalid),
            Err(WalError::NonzeroSegmentHeaderReserved)
        ));

        let mut invalid = encoded;
        invalid[44] ^= 0xff;
        assert!(matches!(
            decode_segment_header(&invalid),
            Err(WalError::SegmentHeaderChecksum { .. })
        ));
    }

    fn wal_segment(header: &SegmentHeader, envelopes: &[WalRecordEnvelope]) -> Vec<u8> {
        let mut bytes = encode_segment_header(header).to_vec();
        for envelope in envelopes {
            bytes.extend_from_slice(&encode_envelope(envelope).unwrap());
        }
        bytes
    }

    fn placed_envelope(
        recording_id: RecordingId,
        sequence: u64,
        kind: RecordKind,
        payload: &[u8],
        segment_ordinal: u64,
        segment_first_sequence: u64,
        byte_offset: u64,
    ) -> WalRecordEnvelope {
        WalRecordEnvelope::unplaced(recording_id, sequence, kind, 1, 0, payload.to_vec())
            .with_provenance(WalRecordProvenance {
                segment_ordinal,
                segment_first_sequence,
                byte_offset,
            })
    }

    fn marker_for(
        previous: &CommitAuthority,
        batch: &[WalRecordEnvelope],
        segment_ordinal: u64,
        segment_first_sequence: u64,
    ) -> WalRecordEnvelope {
        let batch_first_sequence = batch.first().unwrap().sequence;
        let batch_last_sequence = batch.last().unwrap().sequence;
        let batch_payload_bytes = batch
            .iter()
            .map(|envelope| u64::try_from(envelope.payload.len()).unwrap())
            .sum::<u64>();
        commit_marker_envelope(
            batch[0].recording_id,
            &CommitMarker {
                durable_through_sequence: batch_last_sequence,
                durable_record_count: previous.durable_record_count
                    + u64::try_from(batch.len()).unwrap(),
                durable_payload_bytes: previous.durable_payload_bytes + batch_payload_bytes,
                batch_first_sequence,
                batch_last_sequence,
                batch_sha256: batch_sha256(batch).unwrap(),
            },
        )
        .unwrap()
        .with_provenance(WalRecordProvenance {
            segment_ordinal,
            segment_first_sequence,
            byte_offset: 4_096,
        })
    }

    fn scan_commit_image(
        bytes: Vec<u8>,
        header: &SegmentHeader,
    ) -> Result<(CommitAuthority, usize, bool), WalError> {
        let mut reader = WalReader::new(
            Cursor::new(bytes),
            header.recording_id,
            header.segment_ordinal,
            header.first_sequence,
            DEFAULT_MAX_RECORD_BYTES,
        )?;
        let mut authority = CommitAuthority::default();
        let mut pending = Vec::new();
        loop {
            match reader.next_record()? {
                ReadOutcome::Record(envelope) if envelope.kind == RecordKind::CommitMarker => {
                    authority = validate_commit_authority(&authority, &pending, &envelope)?;
                    pending.clear();
                }
                ReadOutcome::Record(envelope) => pending.push(envelope),
                ReadOutcome::End => return Ok((authority, pending.len(), false)),
                ReadOutcome::PartialTail { .. } => {
                    return Ok((authority, pending.len(), true));
                }
            }
        }
    }

    fn discovery_directory() -> PathBuf {
        std::env::temp_dir().join(format!("chronicle-discovery-{}", uuid::Uuid::new_v4()))
    }

    fn write_discovery_segment(
        directory: &Path,
        file_first_sequence: u64,
        recording_id: RecordingId,
        segment_ordinal: u64,
        header_first_sequence: u64,
    ) {
        fs::create_dir_all(directory).unwrap();
        fs::write(
            directory.join(segment_file_name(file_first_sequence)),
            encode_segment_header(&SegmentHeader {
                recording_id,
                segment_ordinal,
                first_sequence: header_first_sequence,
                created_unix_nanos: 0,
            }),
        )
        .unwrap();
    }

    #[test]
    fn commit_marker_codec_is_fixed_76_byte_little_endian() {
        let marker = CommitMarker {
            durable_through_sequence: 5,
            durable_record_count: 6,
            durable_payload_bytes: 7,
            batch_first_sequence: 3,
            batch_last_sequence: 5,
            batch_sha256: [0xa5; 32],
        };
        let bytes = encode_commit_marker(&marker);
        assert_eq!(bytes.len(), COMMIT_MARKER_PAYLOAD_LEN);
        assert_eq!(&bytes[0..2], &COMMIT_MARKER_VERSION.to_le_bytes());
        assert_eq!(&bytes[2..4], &[0, 0]);
        assert_eq!(&bytes[4..12], &5_u64.to_le_bytes());
        assert_eq!(&bytes[12..20], &6_u64.to_le_bytes());
        assert_eq!(&bytes[20..28], &7_u64.to_le_bytes());
        assert_eq!(&bytes[28..36], &3_u64.to_le_bytes());
        assert_eq!(&bytes[36..44], &5_u64.to_le_bytes());
        assert_eq!(&bytes[44..76], &[0xa5; 32]);
        assert_eq!(decode_commit_marker(&bytes).unwrap(), marker);
        assert_eq!(
            encode_envelope(&commit_marker_envelope(RecordingId::new(), &marker).unwrap())
                .unwrap()
                .len(),
            COMMIT_MARKER_FRAME_LEN
        );

        for length in [COMMIT_MARKER_PAYLOAD_LEN - 1, COMMIT_MARKER_PAYLOAD_LEN + 1] {
            assert!(matches!(
                decode_commit_marker(&vec![0; length]),
                Err(WalError::InvalidCommitMarkerLength { actual }) if actual == length
            ));
        }
        let mut invalid = bytes;
        invalid[0..2].copy_from_slice(&(COMMIT_MARKER_VERSION + 1).to_le_bytes());
        assert!(matches!(
            decode_commit_marker(&invalid),
            Err(WalError::UnsupportedCommitMarkerVersion(version))
                if version == COMMIT_MARKER_VERSION + 1
        ));
        let mut invalid = bytes;
        invalid[2..4].copy_from_slice(&1_u16.to_le_bytes());
        assert!(matches!(
            decode_commit_marker(&invalid),
            Err(WalError::InvalidCommitMarkerReserved(1))
        ));
    }

    #[test]
    fn commit_authority_validates_multiple_batches_and_segment_rotation() {
        let recording_id = RecordingId::new();
        let first_batch = vec![
            placed_envelope(recording_id, 1, RecordKind::CaptureEvent, b"abc", 0, 1, 64),
            placed_envelope(recording_id, 2, RecordKind::LossWindow, b"de", 0, 1, 128),
        ];
        let first_marker = marker_for(&CommitAuthority::default(), &first_batch, 0, 1);
        let first =
            validate_commit_authority(&CommitAuthority::default(), &first_batch, &first_marker)
                .unwrap();
        assert_eq!(first.marker_sequence, Some(3));
        assert_eq!(first.durable_record_count, 2);
        assert_eq!(first.durable_payload_bytes, 5);

        let second_batch = vec![placed_envelope(
            recording_id,
            4,
            RecordKind::CaptureEvent,
            b"fghi",
            0,
            1,
            4_220,
        )];
        let second_marker = marker_for(&first, &second_batch, 0, 1);
        let second = validate_commit_authority(&first, &second_batch, &second_marker).unwrap();
        assert_eq!(second.marker_sequence, Some(5));
        assert_eq!(second.durable_record_count, 3);
        assert_eq!(second.durable_payload_bytes, 9);

        let rotated_batch = vec![placed_envelope(
            recording_id,
            6,
            RecordKind::LossWindow,
            b"j",
            1,
            6,
            64,
        )];
        let rotated_marker = marker_for(&second, &rotated_batch, 1, 6);
        let rotated = validate_commit_authority(&second, &rotated_batch, &rotated_marker).unwrap();
        assert_eq!(rotated.marker_sequence, Some(7));
        assert_eq!(rotated.segment_ordinal, Some(1));
        assert_eq!(rotated.durable_record_count, 4);
        assert_eq!(rotated.durable_payload_bytes, 10);
    }

    #[test]
    fn commit_authority_fails_closed_on_reference_identity_and_digest_errors() {
        let recording_id = RecordingId::new();
        let batch = vec![
            placed_envelope(recording_id, 1, RecordKind::CaptureEvent, b"a", 0, 1, 64),
            placed_envelope(recording_id, 2, RecordKind::LossWindow, b"b", 0, 1, 128),
        ];
        let marker = marker_for(&CommitAuthority::default(), &batch, 0, 1);

        assert!(matches!(
            validate_commit_authority(&CommitAuthority::default(), &[], &marker),
            Err(WalError::EmptyCommitBatch)
        ));
        assert!(matches!(
            validate_commit_authority(&CommitAuthority::default(), &batch[1..], &marker),
            Err(WalError::CommitBoundaryMismatch { .. })
        ));

        let mut invalid_batch = batch.clone();
        invalid_batch[1].sequence = 3;
        assert!(matches!(
            validate_commit_authority(&CommitAuthority::default(), &invalid_batch, &marker),
            Err(WalError::CommitBoundaryMismatch { .. })
        ));

        let mut invalid_batch = batch.clone();
        invalid_batch[1].recording_id = RecordingId::new();
        assert!(matches!(
            validate_commit_authority(&CommitAuthority::default(), &invalid_batch, &marker),
            Err(WalError::CommitRecordingIdMismatch)
        ));

        let mut invalid_batch = batch.clone();
        invalid_batch[1]
            .provenance
            .as_mut()
            .unwrap()
            .segment_ordinal = 1;
        assert!(matches!(
            validate_commit_authority(&CommitAuthority::default(), &invalid_batch, &marker),
            Err(WalError::CommitSegmentMismatch)
        ));

        let mut unplaced = batch.clone();
        unplaced[0].provenance = None;
        assert!(matches!(
            validate_commit_authority(&CommitAuthority::default(), &unplaced, &marker),
            Err(WalError::MissingProvenance { sequence: 1 })
        ));

        let mut nested_marker = batch.clone();
        nested_marker[0].kind = RecordKind::CommitMarker;
        assert!(matches!(
            validate_commit_authority(&CommitAuthority::default(), &nested_marker, &marker),
            Err(WalError::CommitBatchContainsMarker { sequence: 1 })
        ));

        let mut invalid_marker = marker.clone();
        let mut decoded = decode_commit_marker(&invalid_marker.payload).unwrap();
        decoded.batch_sha256[0] ^= 0xff;
        invalid_marker.payload = encode_commit_marker(&decoded).to_vec();
        assert!(matches!(
            validate_commit_authority(&CommitAuthority::default(), &batch, &invalid_marker),
            Err(WalError::CommitDigestMismatch)
        ));

        let mut invalid_marker = marker;
        let mut decoded = decode_commit_marker(&invalid_marker.payload).unwrap();
        decoded.durable_record_count += 1;
        invalid_marker.payload = encode_commit_marker(&decoded).to_vec();
        assert!(matches!(
            validate_commit_authority(&CommitAuthority::default(), &batch, &invalid_marker),
            Err(WalError::CommitRecordCountMismatch { .. })
        ));
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn recording_lock_is_exclusive_and_released_on_drop() {
        let directory = discovery_directory();
        let first = RecordingLock::acquire(&directory).unwrap();
        assert!(matches!(
            RecordingLock::acquire(&directory),
            Err(WalError::RecordingLockHeld)
        ));
        drop(first);
        assert!(RecordingLock::acquire(&directory).is_ok());
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn recovery_scan_uses_final_authoritative_marker_without_metadata() {
        let wal_directory = discovery_directory();
        let segments = wal_directory.join("segments");
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"committed".to_vec(), 0)
            .unwrap();
        assert_eq!(writer.flush(1).unwrap().unwrap().marker_sequence, 2);
        writer
            .append(
                RecordKind::CaptureEvent,
                1,
                0,
                b"also committed".to_vec(),
                2,
            )
            .unwrap();
        assert_eq!(writer.flush(3).unwrap().unwrap().marker_sequence, 4);
        drop(writer);

        let uncertain = WalRecordEnvelope::unplaced(
            recording_id,
            5,
            RecordKind::LossWindow,
            1,
            0,
            b"uncertain".to_vec(),
        );
        OpenOptions::new()
            .append(true)
            .open(segments.join(segment_file_name(1)))
            .unwrap()
            .write_all(&encode_envelope(&uncertain).unwrap())
            .unwrap();
        assert!(!wal_directory.join("recording.json").exists());

        let first = scan_wal(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        let second = scan_wal(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.authority.marker_sequence, Some(4));
        assert_eq!(first.authority.durable_through_sequence, Some(3));
        assert_eq!(first.authoritative_marker_count, 2);
        assert_eq!(
            first
                .committed
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert_eq!(first.uncommitted.len(), 1);
        assert_eq!(first.uncommitted[0].sequence, 5);
        assert_eq!(first.partial_tail, None);
        let marker = first.final_marker.as_ref().unwrap();
        let offset = marker.provenance.as_ref().unwrap().byte_offset;
        let bytes = fs::read(segments.join(segment_file_name(1))).unwrap();
        let end = usize::try_from(offset).unwrap() + COMMIT_MARKER_FRAME_LEN;
        let expected_snapshot: [u8; 32] = Sha256::digest(&bytes[..end]).into();
        assert_eq!(
            verified_snapshot_sha256(&wal_directory, &first).unwrap(),
            expected_snapshot
        );
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    #[allow(clippy::too_many_lines)] // Two corruption layouts share one setup contract.
    fn recovery_rejects_uncommitted_nonfinal_segment_and_cross_segment_gap() {
        let wal_directory = discovery_directory();
        let segments = wal_directory.join("segments");
        let recording_id = RecordingId::new();
        let first_header = SegmentHeader {
            recording_id,
            segment_ordinal: 0,
            first_sequence: 1,
            created_unix_nanos: 0,
        };
        let first_data = placed_envelope(
            recording_id,
            1,
            RecordKind::CaptureEvent,
            b"uncommitted",
            0,
            1,
            SEGMENT_HEADER_LEN_U64,
        );
        let second_header = SegmentHeader {
            recording_id,
            segment_ordinal: 1,
            first_sequence: 2,
            created_unix_nanos: 0,
        };
        let second_data = placed_envelope(
            recording_id,
            2,
            RecordKind::CaptureEvent,
            b"later",
            1,
            2,
            SEGMENT_HEADER_LEN_U64,
        );
        let second_marker = marker_for(
            &CommitAuthority::default(),
            std::slice::from_ref(&second_data),
            1,
            2,
        );
        fs::create_dir_all(&segments).unwrap();
        fs::write(
            segments.join(segment_file_name(1)),
            wal_segment(&first_header, &[first_data]),
        )
        .unwrap();
        fs::write(
            segments.join(segment_file_name(2)),
            wal_segment(&second_header, &[second_data, second_marker]),
        )
        .unwrap();
        assert!(matches!(
            scan_wal(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES),
            Err(WalError::CommitMarkerAfterUncommitted)
        ));
        fs::remove_dir_all(&wal_directory).unwrap();

        fs::create_dir_all(&segments).unwrap();
        let committed_data = placed_envelope(
            recording_id,
            1,
            RecordKind::CaptureEvent,
            b"committed",
            0,
            1,
            SEGMENT_HEADER_LEN_U64,
        );
        let committed_marker = marker_for(
            &CommitAuthority::default(),
            std::slice::from_ref(&committed_data),
            0,
            1,
        );
        let previous = validate_commit_authority(
            &CommitAuthority::default(),
            std::slice::from_ref(&committed_data),
            &committed_marker,
        )
        .unwrap();
        let gap_header = SegmentHeader {
            recording_id,
            segment_ordinal: 1,
            first_sequence: 4,
            created_unix_nanos: 0,
        };
        let gap_data = placed_envelope(
            recording_id,
            4,
            RecordKind::CaptureEvent,
            b"gap",
            1,
            4,
            SEGMENT_HEADER_LEN_U64,
        );
        let gap_marker = marker_for(&previous, std::slice::from_ref(&gap_data), 1, 4);
        fs::write(
            segments.join(segment_file_name(1)),
            wal_segment(&first_header, &[committed_data, committed_marker]),
        )
        .unwrap();
        fs::write(
            segments.join(segment_file_name(4)),
            wal_segment(&gap_header, &[gap_data, gap_marker]),
        )
        .unwrap();
        assert!(matches!(
            scan_wal(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES),
            Err(WalError::RecoverySegmentFirstSequenceMismatch {
                expected: 3,
                actual: 4
            })
        ));
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn repair_truncates_only_incomplete_final_frame_and_is_idempotent() {
        let wal_directory = discovery_directory();
        let segments = wal_directory.join("segments");
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"committed".to_vec(), 0)
            .unwrap();
        writer.flush(1).unwrap();
        drop(writer);

        let partial = encode_envelope(&WalRecordEnvelope::unplaced(
            recording_id,
            3,
            RecordKind::LossWindow,
            1,
            0,
            b"partial".to_vec(),
        ))
        .unwrap();
        let segment_path = segments.join(segment_file_name(1));
        let committed_len = fs::metadata(&segment_path).unwrap().len();
        OpenOptions::new()
            .append(true)
            .open(&segment_path)
            .unwrap()
            .write_all(&partial[..partial.len() / 2])
            .unwrap();

        let repaired =
            repair_partial_tail(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert_eq!(repaired.truncated_to, Some(committed_len));
        assert!(repaired.repaired_bytes > 0);
        assert_eq!(repaired.scan.authority.marker_sequence, Some(2));
        assert_eq!(repaired.scan.partial_tail, None);
        assert!(repaired.scan.uncommitted.is_empty());
        assert_eq!(fs::metadata(&segment_path).unwrap().len(), committed_len);

        let second =
            repair_partial_tail(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert_eq!(second.repaired_bytes, 0);
        assert_eq!(second.truncated_to, None);
        assert_eq!(second.scan, repaired.scan);

        let data = placed_envelope(
            recording_id,
            3,
            RecordKind::LossWindow,
            b"complete-data",
            0,
            1,
            committed_len,
        );
        let marker = marker_for(&second.scan.authority, std::slice::from_ref(&data), 0, 1);
        let data_bytes = encode_envelope(&data).unwrap();
        let marker_bytes = encode_envelope(&marker).unwrap();
        let mut file = OpenOptions::new().append(true).open(&segment_path).unwrap();
        file.write_all(&data_bytes).unwrap();
        file.write_all(&marker_bytes[..marker_bytes.len() / 2])
            .unwrap();
        drop(file);
        let repaired_marker =
            repair_partial_tail(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert_eq!(
            repaired_marker.truncated_to,
            Some(committed_len + u64::try_from(data_bytes.len()).unwrap())
        );
        assert_eq!(repaired_marker.scan.authority.marker_sequence, Some(2));
        assert_eq!(repaired_marker.scan.uncommitted.len(), 1);
        assert_eq!(repaired_marker.scan.uncommitted[0].sequence, 3);
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn repair_rejects_complete_corruption_and_truncated_header_without_mutation() {
        let wal_directory = discovery_directory();
        let segments = wal_directory.join("segments");
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"committed".to_vec(), 0)
            .unwrap();
        writer.flush(1).unwrap();
        drop(writer);
        let segment_path = segments.join(segment_file_name(1));
        let mut corrupted = fs::read(&segment_path).unwrap();
        corrupted[SEGMENT_HEADER_LEN + ENVELOPE_HEADER_LEN] ^= 0xff;
        fs::write(&segment_path, &corrupted).unwrap();
        assert!(matches!(
            repair_partial_tail(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES),
            Err(WalError::EnvelopeChecksum { .. })
        ));
        assert_eq!(fs::read(&segment_path).unwrap(), corrupted);
        fs::remove_dir_all(&wal_directory).unwrap();

        fs::create_dir_all(&segments).unwrap();
        let truncated_header = vec![0xa5; 10];
        fs::write(&segment_path, &truncated_header).unwrap();
        assert!(matches!(
            repair_partial_tail(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES),
            Err(WalError::TruncatedSegmentHeader { actual: 10 })
        ));
        assert_eq!(fs::read(&segment_path).unwrap(), truncated_header);
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn reopen_resumes_after_marker_and_rotates_without_overwrite() {
        let wal_directory = discovery_directory();
        let recording_id = RecordingId::new();
        let payload_len = usize::try_from(
            MIN_SEGMENT_BYTES
                - SEGMENT_HEADER_LEN_U64
                - u64::try_from(ENVELOPE_HEADER_LEN).unwrap()
                - u64::try_from(COMMIT_MARKER_FRAME_LEN).unwrap(),
        )
        .unwrap();
        let mut writer = GroupCommitWalWriter::create_with_total_limit(
            &wal_directory,
            recording_id,
            MIN_SEGMENT_BYTES,
            MIN_SEGMENT_BYTES * 2,
            1,
            0,
        )
        .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, vec![0; payload_len], 0)
            .unwrap();
        drop(writer);

        let prepared = prepare_group_commit_reopen(
            &wal_directory,
            recording_id,
            MIN_SEGMENT_BYTES,
            MIN_SEGMENT_BYTES * 2,
            1,
        )
        .unwrap();
        assert_eq!(prepared.preview().next_sequence, 3);
        let (mut reopened, report) = prepared.apply().unwrap();
        assert_eq!(report.next_sequence, 3);
        assert_eq!(report.discarded_complete_records, 0);
        assert_eq!(report.truncated_bytes, 0);
        assert_eq!(
            reopened
                .append(RecordKind::LossWindow, 1, 0, b"new".to_vec(), 1)
                .unwrap()
                .sequence,
            3
        );
        assert_eq!(reopened.shutdown(2).unwrap().unwrap().marker_sequence, 4);
        drop(reopened);

        let discovered = discover_segments(wal_directory.join("segments"), recording_id).unwrap();
        assert_eq!(discovered.len(), 2);
        let scan = scan_wal(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert_eq!(scan.authority.marker_sequence, Some(4));
        assert_eq!(scan.authority.durable_record_count, 2);
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn reopen_repairs_partial_tail_before_reusing_next_sequence() {
        let wal_directory = discovery_directory();
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"committed".to_vec(), 0)
            .unwrap();
        writer.flush(1).unwrap();
        drop(writer);
        let segment_path = wal_directory.join("segments").join(segment_file_name(1));
        let partial = encode_envelope(&WalRecordEnvelope::unplaced(
            recording_id,
            3,
            RecordKind::LossWindow,
            1,
            0,
            b"partial".to_vec(),
        ))
        .unwrap();
        OpenOptions::new()
            .append(true)
            .open(&segment_path)
            .unwrap()
            .write_all(&partial[..partial.len() - 1])
            .unwrap();

        let prepared = prepare_group_commit_reopen(
            &wal_directory,
            recording_id,
            MIN_SEGMENT_BYTES,
            MIN_SEGMENT_BYTES,
            2,
        )
        .unwrap();
        assert_eq!(prepared.preview().next_sequence, 3);
        assert!(prepared.preview().truncated_bytes > 0);
        let (mut reopened, report) = prepared.apply().unwrap();
        assert!(report.truncated_to.is_some());
        assert_eq!(
            reopened
                .append(RecordKind::LossWindow, 1, 0, b"replacement".to_vec(), 2)
                .unwrap()
                .sequence,
            3
        );
        reopened.shutdown(3).unwrap();
        drop(reopened);
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn reopen_prevalidates_smaller_limits_before_discarding_suffix() {
        let wal_directory = discovery_directory();
        let recording_id = RecordingId::new();
        let mut writer = GroupCommitWalWriter::create_with_total_limit(
            &wal_directory,
            recording_id,
            MIN_SEGMENT_BYTES * 2,
            MIN_SEGMENT_BYTES * 2,
            1,
            0,
        )
        .unwrap();
        writer
            .append(
                RecordKind::CaptureEvent,
                1,
                0,
                vec![0; DEFAULT_MAX_RECORD_BYTES - ENVELOPE_HEADER_LEN],
                0,
            )
            .unwrap();
        drop(writer);
        let segment_path = wal_directory.join("segments").join(segment_file_name(1));
        let uncertain = WalRecordEnvelope::unplaced(
            recording_id,
            3,
            RecordKind::LossWindow,
            1,
            0,
            b"uncertain".to_vec(),
        );
        OpenOptions::new()
            .append(true)
            .open(&segment_path)
            .unwrap()
            .write_all(&encode_envelope(&uncertain).unwrap())
            .unwrap();
        let before = fs::read(&segment_path).unwrap();

        assert!(matches!(
            prepare_group_commit_reopen(
                &wal_directory,
                recording_id,
                MIN_SEGMENT_BYTES,
                MIN_SEGMENT_BYTES,
                1,
            ),
            Err(WalError::EnvelopeExceedsSegment { .. })
        ));
        assert_eq!(fs::read(&segment_path).unwrap(), before);
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn reopen_discards_complete_suffix_and_removes_later_uncommitted_segments() {
        let wal_directory = discovery_directory();
        let segments = wal_directory.join("segments");
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"committed".to_vec(), 0)
            .unwrap();
        writer.flush(1).unwrap();
        drop(writer);

        let first_uncommitted = placed_envelope(
            recording_id,
            3,
            RecordKind::LossWindow,
            b"first",
            1,
            3,
            SEGMENT_HEADER_LEN_U64,
        );
        let second_uncommitted = placed_envelope(
            recording_id,
            4,
            RecordKind::CaptureEvent,
            b"second",
            2,
            4,
            SEGMENT_HEADER_LEN_U64,
        );
        fs::write(
            segments.join(segment_file_name(3)),
            wal_segment(
                &SegmentHeader {
                    recording_id,
                    segment_ordinal: 1,
                    first_sequence: 3,
                    created_unix_nanos: 0,
                },
                std::slice::from_ref(&first_uncommitted),
            ),
        )
        .unwrap();
        fs::write(
            segments.join(segment_file_name(4)),
            wal_segment(
                &SegmentHeader {
                    recording_id,
                    segment_ordinal: 2,
                    first_sequence: 4,
                    created_unix_nanos: 0,
                },
                std::slice::from_ref(&second_uncommitted),
            ),
        )
        .unwrap();
        let before = scan_wal(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert_eq!(before.uncommitted.len(), 2);

        let prepared = prepare_group_commit_reopen(
            &wal_directory,
            recording_id,
            MIN_SEGMENT_BYTES,
            DEFAULT_MAX_WAL_BYTES,
            2,
        )
        .unwrap();
        assert_eq!(prepared.preview().discarded_complete_records, 2);
        assert!(segments.join(segment_file_name(3)).exists());
        assert!(segments.join(segment_file_name(4)).exists());
        let (mut reopened, report) = prepared.apply().unwrap();
        assert_eq!(report.discarded_complete_records, 2);
        assert_eq!(report.discarded_payload_bytes, 11);
        assert_eq!(report.removed_segments, 2);
        assert_eq!(report.next_sequence, 3);
        assert!(report.scan_after.uncommitted.is_empty());
        assert_eq!(
            reopened
                .append(RecordKind::CaptureEvent, 1, 0, b"replacement".to_vec(), 2)
                .unwrap()
                .sequence,
            3
        );
        reopened.shutdown(3).unwrap();
        drop(reopened);
        assert_eq!(discover_segments(&segments, recording_id).unwrap().len(), 1);
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[test]
    fn durable_image_crash_matrix_never_infers_acknowledgement_delivery() {
        let header = SegmentHeader {
            recording_id: RecordingId::new(),
            segment_ordinal: 0,
            first_sequence: 1,
            created_unix_nanos: 0,
        };
        let batch = vec![placed_envelope(
            header.recording_id,
            1,
            RecordKind::CaptureEvent,
            b"data",
            0,
            1,
            SEGMENT_HEADER_LEN_U64,
        )];
        let marker = marker_for(&CommitAuthority::default(), &batch, 0, 1);
        let header_bytes = encode_segment_header(&header).to_vec();
        let data_bytes = encode_envelope(&batch[0]).unwrap();
        let marker_bytes = encode_envelope(&marker).unwrap();

        let mut before_marker = header_bytes.clone();
        before_marker.extend_from_slice(&data_bytes);
        let (authority, pending, partial) = scan_commit_image(before_marker, &header).unwrap();
        assert_eq!(authority, CommitAuthority::default());
        assert_eq!(pending, 1);
        assert!(!partial);

        let mut during_marker = header_bytes.clone();
        during_marker.extend_from_slice(&data_bytes);
        during_marker.extend_from_slice(&marker_bytes[..marker_bytes.len() / 2]);
        let (authority, pending, partial) = scan_commit_image(during_marker, &header).unwrap();
        assert_eq!(authority, CommitAuthority::default());
        assert_eq!(pending, 1);
        assert!(partial);

        // Before fdatasync, durable media still contains only its prior barrier image.
        let (authority, pending, partial) =
            scan_commit_image(header_bytes.clone(), &header).unwrap();
        assert_eq!(authority, CommitAuthority::default());
        assert_eq!(pending, 0);
        assert!(!partial);

        let mut after_sync_before_ack = header_bytes;
        after_sync_before_ack.extend_from_slice(&data_bytes);
        after_sync_before_ack.extend_from_slice(&marker_bytes);
        let acknowledgement_observed = false;
        let first_scan = scan_commit_image(after_sync_before_ack.clone(), &header).unwrap();
        let second_scan = scan_commit_image(after_sync_before_ack.clone(), &header).unwrap();
        assert_eq!(first_scan, second_scan);
        assert_eq!(first_scan.0.marker_sequence, Some(2));
        assert_eq!(first_scan.0.durable_through_sequence, Some(1));
        assert!(!acknowledgement_observed);

        let uncertain = placed_envelope(
            header.recording_id,
            3,
            RecordKind::LossWindow,
            b"uncertain",
            0,
            1,
            u64::try_from(after_sync_before_ack.len()).unwrap(),
        );
        after_sync_before_ack.extend_from_slice(&encode_envelope(&uncertain).unwrap());
        let (authority, pending, partial) =
            scan_commit_image(after_sync_before_ack, &header).unwrap();
        assert_eq!(authority.marker_sequence, Some(2));
        assert_eq!(pending, 1);
        assert!(!partial);
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn live_snapshot_is_marker_capped_and_never_repairs_suffix() {
        let directory = discovery_directory();
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"committed".to_vec(), 0)
            .unwrap();
        writer.shutdown(10).unwrap();
        let path = directory.join("segments").join(segment_file_name(1));
        let snapshot =
            read_committed_snapshot(&directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert_eq!(snapshot.marker_sequence, 2);
        assert_eq!(snapshot.committed_record_count, 1);
        let records = read_committed_snapshot_with_records(
            &directory,
            recording_id,
            DEFAULT_MAX_RECORD_BYTES,
        )
        .unwrap();
        assert_eq!(records.snapshot, snapshot);
        assert_eq!(records.envelopes.len(), 2);
        assert_eq!(
            records.envelopes.last().unwrap().sequence,
            snapshot.marker_sequence
        );
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"pending".to_vec(), 11)
            .unwrap();
        let after_append = fs::read(&path).unwrap();
        assert!(matches!(
            read_committed_snapshot(&directory, recording_id, DEFAULT_MAX_RECORD_BYTES),
            Err(WalError::RetryableSnapshotUncertainty)
        ));
        assert_eq!(fs::read(&path).unwrap(), after_append);
        drop(writer);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn group_writer_enforces_physical_total_cap_and_marker_reservation() {
        for total in [MIN_SEGMENT_BYTES - 1, DEFAULT_MAX_WAL_BYTES + 1] {
            assert!(matches!(
                GroupCommitWalWriter::create_with_total_limit(
                    discovery_directory(),
                    RecordingId::new(),
                    MIN_SEGMENT_BYTES,
                    total,
                    1,
                    0,
                ),
                Err(WalError::InvalidTotalWalSize)
            ));
        }

        let directory = discovery_directory();
        let segments = directory.join("segments");
        let recording_id = RecordingId::new();
        let payload_len = usize::try_from(
            MIN_SEGMENT_BYTES
                - SEGMENT_HEADER_LEN_U64
                - u64::try_from(ENVELOPE_HEADER_LEN).unwrap()
                - u64::try_from(COMMIT_MARKER_FRAME_LEN).unwrap(),
        )
        .unwrap();
        let mut writer = GroupCommitWalWriter::create_with_total_limit(
            &directory,
            recording_id,
            MIN_SEGMENT_BYTES,
            MIN_SEGMENT_BYTES,
            1,
            0,
        )
        .unwrap();
        let result = writer
            .append(RecordKind::CaptureEvent, 1, 0, vec![0; payload_len], 0)
            .unwrap();
        assert_eq!(result.durable_batches.len(), 1);
        assert_eq!(writer.writer.physical_bytes, MIN_SEGMENT_BYTES);
        assert_eq!(
            directory_physical_bytes(&segments).unwrap(),
            MIN_SEGMENT_BYTES
        );
        assert_eq!(writer.shutdown(1).unwrap(), None);
        drop(writer);
        fs::remove_dir_all(&directory).unwrap();

        fs::create_dir_all(&segments).unwrap();
        let temp = File::create(segments.join(".stale.tmp")).unwrap();
        temp.set_len(MIN_SEGMENT_BYTES - 100).unwrap();
        drop(temp);
        let mut writer = GroupCommitWalWriter::create_with_total_limit(
            &directory,
            recording_id,
            MIN_SEGMENT_BYTES,
            MIN_SEGMENT_BYTES,
            1,
            0,
        )
        .unwrap();
        assert!(matches!(
            writer.append(RecordKind::CaptureEvent, 1, 0, b"data".to_vec(), 0),
            Err(WalError::WalCapacityExceeded { .. })
        ));
        assert_eq!(
            directory_physical_bytes(&segments).unwrap(),
            MIN_SEGMENT_BYTES - 100
        );
        assert!(!segments.join(segment_file_name(1)).exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn segment_age_rotates_non_empty_segment_on_tick_without_empty_churn() {
        let directory = discovery_directory();
        let recording_id = RecordingId::new();
        let mut writer = GroupCommitWalWriter::create_with_total_limit_and_age(
            &directory,
            recording_id,
            MIN_SEGMENT_BYTES,
            DEFAULT_MAX_WAL_BYTES,
            1,
            0,
            100,
        )
        .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"data".to_vec(), 0)
            .unwrap();
        assert!(writer.poll(10).unwrap().is_some());
        let syncs = writer.data_sync_count();
        assert!(writer.poll(99).unwrap().is_none());
        assert!(writer.poll(100).unwrap().is_none());
        assert_eq!(writer.data_sync_count(), syncs);
        assert_eq!(
            discover_segments(directory.join("segments"), recording_id)
                .unwrap()
                .len(),
            2
        );
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"next".to_vec(), 101)
            .unwrap();
        assert_eq!(
            discover_segments(directory.join("segments"), recording_id)
                .unwrap()
                .len(),
            2
        );
        drop(writer);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn in_place_rollover_releases_lock_and_continues_sequence() {
        let directory = discovery_directory();
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"first".to_vec(), 0)
            .unwrap();
        writer.poll(10).unwrap();
        assert_eq!(writer.next_sequence(), 3);
        writer.rollover(20, 3).unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"second".to_vec(), 20)
            .unwrap();
        writer.poll(30).unwrap();
        assert_eq!(writer.next_sequence(), 5);
        drop(writer);
        assert_eq!(
            discover_segments(directory.join("segments"), recording_id)
                .unwrap()
                .len(),
            2
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn empty_segment_does_not_rotate_due_to_age() {
        let directory = discovery_directory();
        let mut writer = GroupCommitWalWriter::create_with_total_limit_and_age(
            &directory,
            RecordingId::new(),
            MIN_SEGMENT_BYTES,
            DEFAULT_MAX_WAL_BYTES,
            1,
            0,
            1,
        )
        .unwrap();
        assert!(writer.poll(u64::MAX).unwrap().is_none());
        assert!(
            fs::read_dir(directory.join("segments"))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(matches!(
            writer.set_max_segment_age_millis(0),
            Err(WalError::InvalidSegmentAge)
        ));
        drop(writer);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn group_commit_timer_orders_data_marker_sync_then_acknowledgement() {
        let directory = discovery_directory();
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        let appended = writer
            .append(RecordKind::CaptureEvent, 1, 0, b"data".to_vec(), 0)
            .unwrap();
        assert_eq!(appended.sequence, 1);
        assert!(appended.durable_batches.is_empty());
        assert_eq!(writer.data_sync_count(), 0);
        assert_eq!(writer.poll(9).unwrap(), None);
        let durable = writer.poll(10).unwrap().unwrap();
        assert_eq!(durable.marker_sequence, 2);
        assert_eq!(durable.durable_through_sequence, 1);
        assert_eq!(writer.data_sync_count(), 1);
        assert_eq!(writer.authority().marker_sequence, Some(2));
        assert_eq!(writer.next_sequence(), 3);
        let physical_before = directory_physical_bytes(&directory.join("segments")).unwrap();
        assert!(matches!(
            GroupCommitWalWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES, 3, 10),
            Err(WalError::RecordingLockHeld)
        ));
        assert_eq!(
            directory_physical_bytes(&directory.join("segments")).unwrap(),
            physical_before
        );
        drop(writer);

        let bytes = fs::read(directory.join("segments").join(segment_file_name(1))).unwrap();
        let mut reader = WalReader::new(
            Cursor::new(bytes),
            recording_id,
            0,
            1,
            DEFAULT_MAX_RECORD_BYTES,
        )
        .unwrap();
        assert!(matches!(
            reader.next_record().unwrap(),
            ReadOutcome::Record(WalRecordEnvelope {
                kind: RecordKind::CaptureEvent,
                sequence: 1,
                ..
            })
        ));
        assert!(matches!(
            reader.next_record().unwrap(),
            ReadOutcome::Record(WalRecordEnvelope {
                kind: RecordKind::CommitMarker,
                sequence: 2,
                ..
            })
        ));
        assert_eq!(reader.next_record().unwrap(), ReadOutcome::End);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn group_commit_threshold_flush_shutdown_and_empty_flush_sync_once_per_batch() {
        let directory = discovery_directory();
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        assert_eq!(writer.flush(0).unwrap(), None);
        assert_eq!(writer.data_sync_count(), 0);

        let payload_len = usize::try_from(GROUP_COMMIT_BYTES).unwrap() - ENVELOPE_HEADER_LEN;
        let threshold = writer
            .append(RecordKind::CaptureEvent, 1, 0, vec![0; payload_len], 1)
            .unwrap();
        assert_eq!(threshold.durable_batches.len(), 1);
        assert_eq!(threshold.durable_batches[0].marker_sequence, 2);
        assert_eq!(writer.data_sync_count(), 1);

        let next = writer
            .append(RecordKind::LossWindow, 1, 0, b"loss".to_vec(), 2)
            .unwrap();
        assert_eq!(next.sequence, 3);
        assert!(next.durable_batches.is_empty());
        assert_eq!(writer.shutdown(3).unwrap().unwrap().marker_sequence, 4);
        assert_eq!(writer.data_sync_count(), 2);
        assert_eq!(writer.flush(4).unwrap(), None);
        assert_eq!(writer.data_sync_count(), 2);
        drop(writer);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn group_commit_rotation_commits_pending_batch_without_extra_segment_sync() {
        let directory = discovery_directory();
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        let payload = vec![0; 3 * 1024 * 1024];
        let mut acknowledgements = Vec::new();
        for now in 0..6 {
            acknowledgements.extend(
                writer
                    .append(RecordKind::CaptureEvent, 1, 0, payload.clone(), now)
                    .unwrap()
                    .durable_batches,
            );
        }
        acknowledgements.extend(writer.shutdown(6).unwrap());
        assert_eq!(acknowledgements.len(), 4);
        assert_eq!(writer.data_sync_count(), 4);
        drop(writer);

        let discovered = discover_segments(directory.join("segments"), recording_id).unwrap();
        assert_eq!(discovered.len(), 2);
        assert_eq!(discovered[0].header.segment_ordinal, 0);
        assert_eq!(discovered[1].header.segment_ordinal, 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn failed_group_sync_never_advances_authority_or_returns_acknowledgement() {
        let directory = discovery_directory();
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"data".to_vec(), 0)
            .unwrap();
        writer.inject_data_sync_failure();
        assert!(writer.flush(1).is_err());
        assert_eq!(writer.data_sync_count(), 1);
        assert_eq!(writer.authority(), &CommitAuthority::default());
        drop(writer);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn discovers_wal_segments_in_numeric_order_and_ignores_temps() {
        let directory = discovery_directory();
        let recording_id = RecordingId::new();
        write_discovery_segment(&directory, 20, recording_id, 1, 20);
        write_discovery_segment(&directory, 10, recording_id, 0, 10);
        fs::write(
            directory.join(".00000000000000000030.chwal.tmp-1"),
            b"partial",
        )
        .unwrap();

        let discovered = discover_segments(&directory, recording_id).unwrap();
        assert_eq!(
            discovered
                .iter()
                .map(|segment| segment.header.first_sequence)
                .collect::<Vec<_>>(),
            vec![10, 20]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn discovery_rejects_missing_or_duplicate_ordinals() {
        let recording_id = RecordingId::new();
        let directory = discovery_directory();
        write_discovery_segment(&directory, 10, recording_id, 0, 10);
        write_discovery_segment(&directory, 20, recording_id, 2, 20);
        assert!(matches!(
            discover_segments(&directory, recording_id),
            Err(WalError::MissingSegmentOrdinal {
                expected: 1,
                actual: 2
            })
        ));
        fs::remove_dir_all(&directory).unwrap();

        write_discovery_segment(&directory, 10, recording_id, 0, 10);
        write_discovery_segment(&directory, 20, recording_id, 0, 20);
        assert!(matches!(
            discover_segments(&directory, recording_id),
            Err(WalError::DuplicateSegmentOrdinal(0))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn discovery_rejects_duplicate_nonincreasing_and_mismatched_first_sequences() {
        let recording_id = RecordingId::new();
        let directory = discovery_directory();
        write_discovery_segment(&directory, 10, recording_id, 0, 10);
        write_discovery_segment(&directory, 20, recording_id, 1, 10);
        assert!(matches!(
            discover_segments(&directory, recording_id),
            Err(WalError::DuplicateSegmentFirstSequence(10))
        ));
        fs::remove_dir_all(&directory).unwrap();

        write_discovery_segment(&directory, 10, recording_id, 0, 20);
        write_discovery_segment(&directory, 20, recording_id, 1, 10);
        assert!(matches!(
            discover_segments(&directory, recording_id),
            Err(WalError::NonIncreasingSegmentFirstSequence {
                previous: 20,
                current: 10
            })
        ));
        fs::remove_dir_all(&directory).unwrap();

        write_discovery_segment(&directory, 10, recording_id, 0, 11);
        assert!(matches!(
            discover_segments(&directory, recording_id),
            Err(WalError::SegmentFileNameMismatch {
                file_first_sequence: 10,
                header_first_sequence: 11
            })
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn segmented_writer_enforces_bounds_exact_boundary_and_private_modes() {
        use std::os::unix::fs::PermissionsExt;

        assert_eq!(DEFAULT_SEGMENT_BYTES, 256 * 1024 * 1024);
        for size in [MIN_SEGMENT_BYTES - 1, MAX_SEGMENT_BYTES + 1] {
            assert!(matches!(
                SegmentedEnvelopeWriter::create(discovery_directory(), RecordingId::new(), size),
                Err(WalError::InvalidSegmentSizeRange)
            ));
        }

        let directory = discovery_directory();
        let recording_id = RecordingId::new();
        let payload_len = usize::try_from(
            MIN_SEGMENT_BYTES
                - SEGMENT_HEADER_LEN_U64
                - u64::try_from(ENVELOPE_HEADER_LEN).unwrap(),
        )
        .unwrap();
        let envelope = WalRecordEnvelope::unplaced(
            recording_id,
            7,
            RecordKind::CaptureEvent,
            1,
            0,
            vec![0x5a; payload_len],
        );
        let mut writer =
            SegmentedEnvelopeWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES).unwrap();
        assert_eq!(
            writer.append(&envelope).unwrap(),
            WalRecordProvenance {
                segment_ordinal: 0,
                segment_first_sequence: 7,
                byte_offset: SEGMENT_HEADER_LEN_U64,
            }
        );
        writer.flush().unwrap();
        drop(writer);

        let segment = directory.join(segment_file_name(7));
        assert_eq!(segment.metadata().unwrap().len(), MIN_SEGMENT_BYTES);
        assert_eq!(
            directory.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            segment.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn segmented_writer_rotates_before_overflow_and_rejects_oversized_frame() {
        let directory = discovery_directory();
        let recording_id = RecordingId::new();
        let payload_len = usize::try_from(MIN_SEGMENT_BYTES / 2).unwrap();
        let mut writer =
            SegmentedEnvelopeWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES).unwrap();
        let first = WalRecordEnvelope::unplaced(
            recording_id,
            1,
            RecordKind::CaptureEvent,
            1,
            0,
            vec![1; payload_len],
        );
        let second = WalRecordEnvelope::unplaced(
            recording_id,
            2,
            RecordKind::LossWindow,
            1,
            0,
            vec![2; payload_len],
        );
        assert_eq!(writer.append(&first).unwrap().segment_ordinal, 0);
        assert_eq!(writer.append(&second).unwrap().segment_ordinal, 1);
        writer.flush().unwrap();
        drop(writer);
        assert_eq!(
            discover_segments(&directory, recording_id).unwrap().len(),
            2
        );
        fs::remove_dir_all(&directory).unwrap();

        let oversized_payload = usize::try_from(
            MIN_SEGMENT_BYTES
                - SEGMENT_HEADER_LEN_U64
                - u64::try_from(ENVELOPE_HEADER_LEN).unwrap()
                + 1,
        )
        .unwrap();
        let mut writer =
            SegmentedEnvelopeWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES).unwrap();
        let oversized = WalRecordEnvelope::unplaced(
            recording_id,
            1,
            RecordKind::CaptureEvent,
            1,
            0,
            vec![0; oversized_payload],
        );
        assert!(matches!(
            writer.append(&oversized),
            Err(WalError::EnvelopeExceedsSegment { .. })
        ));
        assert!(fs::read_dir(&directory).unwrap().next().is_none());
        fs::remove_dir_all(&directory).unwrap();

        let mut writer =
            SegmentedEnvelopeWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES * 2)
                .unwrap();
        let too_large = WalRecordEnvelope::unplaced(
            recording_id,
            1,
            RecordKind::CaptureEvent,
            1,
            0,
            vec![0; DEFAULT_MAX_RECORD_BYTES],
        );
        assert!(matches!(
            writer.append(&too_large),
            Err(WalError::RecordTooLarge { .. })
        ));
        assert!(fs::read_dir(&directory).unwrap().next().is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn segment_header_publication_faults_never_expose_partial_header() {
        for fault in [
            SegmentPublishFault::BeforeHeaderSync,
            SegmentPublishFault::BeforeRename,
            SegmentPublishFault::AfterRename,
        ] {
            let directory = discovery_directory();
            let recording_id = RecordingId::new();
            let mut writer =
                SegmentedEnvelopeWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES)
                    .unwrap();
            writer.inject_publish_fault(fault);
            let envelope = WalRecordEnvelope::unplaced(
                recording_id,
                1,
                RecordKind::CaptureEvent,
                1,
                0,
                b"payload".to_vec(),
            );
            assert!(writer.append(&envelope).is_err());
            let entries: Vec<_> = fs::read_dir(&directory)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            if fault == SegmentPublishFault::AfterRename {
                assert_eq!(entries, vec![segment_file_name(1)]);
                let bytes = fs::read(directory.join(segment_file_name(1))).unwrap();
                assert!(decode_segment_header(&bytes).is_ok());
            } else {
                assert!(entries.is_empty());
            }
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn envelope_round_trip_dispatches_all_kinds_with_derived_provenance() {
        let header = segment_header();
        let envelopes: Vec<_> = RecordKind::ALL
            .into_iter()
            .enumerate()
            .map(|(index, kind)| {
                WalRecordEnvelope::unplaced(
                    header.recording_id,
                    header.first_sequence + index as u64,
                    kind,
                    1,
                    u16::try_from(index).unwrap(),
                    vec![0, 0xff, kind as u8],
                )
            })
            .collect();
        let bytes = wal_segment(&header, &envelopes);
        let mut reader = WalReader::new(
            Cursor::new(bytes),
            header.recording_id,
            header.segment_ordinal,
            header.first_sequence,
            DEFAULT_MAX_RECORD_BYTES,
        )
        .unwrap();
        let mut offset = SEGMENT_HEADER_LEN as u64;
        for envelope in envelopes {
            let encoded_len = encode_envelope(&envelope).unwrap().len() as u64;
            let expected = envelope.with_provenance(WalRecordProvenance {
                segment_ordinal: header.segment_ordinal,
                segment_first_sequence: header.first_sequence,
                byte_offset: offset,
            });
            assert_eq!(reader.next_record().unwrap(), ReadOutcome::Record(expected));
            offset += encoded_len;
        }
        assert_eq!(reader.next_record().unwrap(), ReadOutcome::End);
    }

    #[test]
    fn reader_rejects_invalid_envelope_fields() {
        let header = segment_header();
        let envelope = WalRecordEnvelope::unplaced(
            header.recording_id,
            header.first_sequence,
            RecordKind::CaptureEvent,
            1,
            0,
            b"payload".to_vec(),
        );
        let encoded = wal_segment(&header, &[envelope]);
        let read_error = |bytes: Vec<u8>| {
            WalReader::new(
                Cursor::new(bytes),
                header.recording_id,
                header.segment_ordinal,
                header.first_sequence,
                DEFAULT_MAX_RECORD_BYTES,
            )
            .unwrap()
            .next_record()
            .unwrap_err()
        };
        let frame = SEGMENT_HEADER_LEN;

        let mut invalid = encoded.clone();
        invalid[frame] ^= 0xff;
        assert!(matches!(
            read_error(invalid),
            WalError::InvalidEnvelopeMagic { .. }
        ));

        let mut invalid = encoded.clone();
        invalid[frame + 4..frame + 6].copy_from_slice(&(ENVELOPE_VERSION + 1).to_le_bytes());
        assert!(matches!(
            read_error(invalid),
            WalError::UnsupportedEnvelopeVersion { version, .. }
                if version == ENVELOPE_VERSION + 1
        ));

        let mut invalid = encoded.clone();
        invalid[frame + 6..frame + 8].copy_from_slice(&5_u16.to_le_bytes());
        assert!(matches!(
            read_error(invalid),
            WalError::UnknownRecordKind(5)
        ));

        let mut invalid = encoded.clone();
        invalid[frame + 10..frame + 12]
            .copy_from_slice(&(WAL_RECORD_SCHEMA_VERSION + 1).to_le_bytes());
        assert!(matches!(
            read_error(invalid),
            WalError::UnsupportedRecordSchemaVersion { version, .. }
                if version == WAL_RECORD_SCHEMA_VERSION + 1
        ));
        assert!(matches!(
            encode_envelope(&WalRecordEnvelope::unplaced(
                header.recording_id,
                header.first_sequence,
                RecordKind::CaptureEvent,
                WAL_RECORD_SCHEMA_VERSION + 1,
                0,
                Vec::new(),
            )),
            Err(WalError::UnsupportedRecordSchemaVersion { .. })
        ));

        let mut invalid = encoded.clone();
        invalid[frame + 12..frame + 16].copy_from_slice(&47_u32.to_le_bytes());
        assert!(matches!(
            read_error(invalid),
            WalError::InvalidEnvelopeHeaderLength(47)
        ));

        let mut invalid = encoded.clone();
        invalid[frame + 20] ^= 0xff;
        assert!(matches!(
            read_error(invalid),
            WalError::EnvelopeRecordingIdMismatch
        ));

        let mut invalid = encoded.clone();
        invalid[frame + 36..frame + 44].copy_from_slice(&(header.first_sequence + 1).to_le_bytes());
        assert!(matches!(
            read_error(invalid),
            WalError::UnexpectedSequence { .. }
        ));

        let mut invalid = encoded;
        *invalid.last_mut().unwrap() ^= 0xff;
        assert!(matches!(
            read_error(invalid),
            WalError::EnvelopeChecksum { .. }
        ));
    }

    #[test]
    fn reader_checks_record_limit_before_payload_allocation() {
        let header = segment_header();
        let envelope = WalRecordEnvelope::unplaced(
            header.recording_id,
            header.first_sequence,
            RecordKind::CaptureEvent,
            1,
            0,
            Vec::new(),
        );
        let mut bytes = wal_segment(&header, &[envelope]);
        bytes[SEGMENT_HEADER_LEN + 16..SEGMENT_HEADER_LEN + 20]
            .copy_from_slice(&1_000_u32.to_le_bytes());
        let mut reader = WalReader::new(
            Cursor::new(bytes),
            header.recording_id,
            header.segment_ordinal,
            header.first_sequence,
            ENVELOPE_HEADER_LEN + 10,
        )
        .unwrap();
        assert!(matches!(
            reader.next_record(),
            Err(WalError::RecordTooLarge {
                record_bytes,
                max_record_bytes
            }) if record_bytes == ENVELOPE_HEADER_LEN + 1_000
                && max_record_bytes == ENVELOPE_HEADER_LEN + 10
        ));
    }

    fn terminal_loss(boot_id: &str, start: u64, end: u64, records: u64) -> TerminalWalLoss {
        let clock = ClockIdentity {
            boot_id: boot_id.into(),
        };
        TerminalWalLoss {
            interval: TerminalWalLossInterval {
                start: MonotonicTimestamp {
                    clock: clock.clone(),
                    nanoseconds: start,
                },
                end: MonotonicTimestamp {
                    clock,
                    nanoseconds: end,
                },
            },
            discarded_records: records,
            discarded_payload_bytes: 7,
            reason: TerminalWalLossReason::WalHardLimit,
            ambiguity: TerminalWalLossAmbiguity::UnknownDownstreamEffects,
        }
    }

    #[test]
    fn terminal_wal_loss_codec_round_trips_and_rejects_invalid_contracts() {
        let loss = terminal_loss("boot-a", 10, 20, 2);
        let encoded = encode_terminal_wal_loss(&loss).unwrap();
        assert_eq!(
            decode_terminal_wal_loss(TERMINAL_WAL_LOSS_SCHEMA_VERSION, &encoded).unwrap(),
            loss
        );
        assert!(matches!(
            decode_terminal_wal_loss(TERMINAL_WAL_LOSS_SCHEMA_VERSION + 1, &encoded),
            Err(WalError::UnsupportedTerminalWalLossVersion(_))
        ));
        assert!(matches!(
            encode_terminal_wal_loss(&terminal_loss("boot-a", 20, 10, 1)),
            Err(WalError::InvalidTerminalWalLoss("interval is reversed"))
        ));
        assert!(matches!(
            encode_terminal_wal_loss(&terminal_loss("boot-a", 10, 20, 0)),
            Err(WalError::InvalidTerminalWalLoss(
                "discarded records is zero"
            ))
        ));
        let mut malformed = encoded;
        let records_offset = 2 + "boot-a".len() + 8 + 2 + "boot-a".len() + 8;
        malformed[records_offset..records_offset + 8].copy_from_slice(&0_u64.to_le_bytes());
        assert!(matches!(
            decode_terminal_wal_loss(TERMINAL_WAL_LOSS_SCHEMA_VERSION, &malformed),
            Err(WalError::InvalidTerminalWalLoss(
                "discarded records is zero"
            ))
        ));
    }

    #[test]
    fn recovery_exposes_only_marker_committed_terminal_wal_loss() {
        let directory =
            std::env::temp_dir().join(format!("chronicle-terminal-loss-{}", uuid::Uuid::new_v4()));
        let recording_id = RecordingId::new();
        let mut writer = GroupCommitWalWriter::create_with_total_limit(
            &directory,
            recording_id,
            MIN_SEGMENT_BYTES,
            MIN_SEGMENT_BYTES,
            1,
            0,
        )
        .unwrap();
        let loss = terminal_loss("boot-a", 10, 20, 2);
        writer
            .append(
                RecordKind::TerminalWalLoss,
                TERMINAL_WAL_LOSS_SCHEMA_VERSION,
                0,
                encode_terminal_wal_loss(&loss).unwrap(),
                1,
            )
            .unwrap();
        writer.shutdown(1).unwrap();
        drop(writer);

        let scan = scan_wal(&directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert_eq!(scan.terminal_wal_losses.len(), 1);
        assert_eq!(scan.terminal_wal_losses[0].loss, loss);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn envelope_assigns_sequence_before_optional_placement_for_every_kind() {
        let recording_id = RecordingId::new();
        for (index, kind) in RecordKind::ALL.into_iter().enumerate() {
            let payload = vec![kind as u8];
            let sequence = index as u64 + 1;
            let envelope =
                WalRecordEnvelope::unplaced(recording_id, sequence, kind, 1, 0, payload.clone());
            assert_eq!(envelope.sequence, sequence);
            assert_eq!(envelope.payload, payload);
            assert_eq!(envelope.provenance, None);

            let provenance = WalRecordProvenance {
                segment_ordinal: 4,
                segment_first_sequence: 1,
                byte_offset: index as u64 * 64,
            };
            assert_eq!(
                envelope.with_provenance(provenance.clone()).provenance,
                Some(provenance)
            );
        }
    }

    #[test]
    fn record_kind_dispatch_is_exhaustive() {
        for kind in RecordKind::ALL {
            assert_eq!(RecordKind::try_from(kind as u16).unwrap(), kind);
        }
        assert!(matches!(
            RecordKind::try_from(0),
            Err(WalError::UnknownRecordKind(0))
        ));
        assert!(matches!(
            RecordKind::try_from(5),
            Err(WalError::UnknownRecordKind(5))
        ));
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn group_writer_rejects_sequence_exhaustion_before_any_frame_write() {
        let wal_directory = discovery_directory();
        let recording_id = RecordingId::new();
        let mut writer = GroupCommitWalWriter::create(
            &wal_directory,
            recording_id,
            MIN_SEGMENT_BYTES,
            u64::MAX - 1,
            0,
        )
        .unwrap();
        assert!(matches!(
            writer.append(RecordKind::CaptureEvent, 1, 0, b"data".to_vec(), 0),
            Err(WalError::SequenceExhausted { sequence }) if sequence == u64::MAX - 1
        ));
        assert!(
            fs::read_dir(wal_directory.join("segments"))
                .unwrap()
                .next()
                .is_none()
        );
        drop(writer);
        fs::remove_dir_all(wal_directory).unwrap();

        let wal_directory = discovery_directory();
        let mut writer = GroupCommitWalWriter::create(
            &wal_directory,
            recording_id,
            MIN_SEGMENT_BYTES,
            u64::MAX - 2,
            0,
        )
        .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"pending".to_vec(), 0)
            .unwrap();
        let segment_path = wal_directory
            .join("segments")
            .join(segment_file_name(u64::MAX - 2));
        let before = fs::read(&segment_path).unwrap();
        assert!(matches!(
            writer.append(RecordKind::CaptureEvent, 1, 0, b"rejected".to_vec(), 20),
            Err(WalError::SequenceExhausted { sequence }) if sequence == u64::MAX - 1
        ));
        assert_eq!(fs::read(&segment_path).unwrap(), before);
        drop(writer);
        let scan = scan_wal(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert!(scan.final_marker.is_none());
        assert_eq!(scan.uncommitted.len(), 1);
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn stale_segment_temp_does_not_block_publication() {
        let wal_directory = discovery_directory();
        let segments = wal_directory.join("segments");
        fs::create_dir_all(&segments).unwrap();
        let stale = segments.join(format!(".{}.tmp-stale", segment_file_name(1)));
        fs::write(&stale, b"stale").unwrap();
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"data".to_vec(), 0)
            .unwrap();
        writer.flush(1).unwrap();
        drop(writer);
        assert_eq!(discover_segments(&segments, recording_id).unwrap().len(), 1);
        assert!(stale.exists());
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn writer_hardens_directory_and_segment_permissions() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "chronicle-wal-permissions-{}-{unique}",
            std::process::id()
        ));
        let recording_id = RecordingId::new();
        let mut writer =
            SegmentedEnvelopeWriter::create(&directory, recording_id, MIN_SEGMENT_BYTES).unwrap();
        writer
            .append(&WalRecordEnvelope::unplaced(
                recording_id,
                7,
                RecordKind::CaptureEvent,
                WAL_RECORD_SCHEMA_VERSION,
                0,
                Vec::new(),
            ))
            .unwrap();
        writer.flush().unwrap();

        let directory_mode = fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
        let segment_mode = fs::metadata(directory.join(segment_file_name(7)))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory_mode, 0o700);
        assert_eq!(segment_mode, 0o600);
        fs::remove_dir_all(directory).unwrap();
    }
}
