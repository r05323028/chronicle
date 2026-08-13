//! Rootless ETL publication contract (migration task 3.3).
//!
//! Portable replacement for ETL assertions currently duplicated inside
//! privileged acceptance scenarios: publish, rerun idempotence,
//! one-shot equivalence across output roots, and corruption fail-closed.
//! No root, eBPF, systemd, or Multipass required.

use chronicle_application::{
    process_and_publish_recording_wal, production_wal_from_fixture, protocol_registry,
};
use std::path::{Path, PathBuf};

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/http/basic-session.json")
}

fn fixture_wal(root: &Path) -> PathBuf {
    let wal = root.join("wal");
    production_wal_from_fixture(&wal, fixture_path()).expect("fixture WAL with recording metadata");
    wal
}

fn temp_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "chronicle-etl-contract-{tag}-{}",
        uuid::Uuid::new_v4()
    ))
}

fn first_segment(wal: &Path) -> PathBuf {
    let segments = wal.join("segments");
    let mut names: Vec<PathBuf> = std::fs::read_dir(&segments)
        .expect("segments directory")
        .map(|entry| entry.expect("read segment entry").path())
        .collect();
    names.sort();
    names.into_iter().next().expect("at least one WAL segment")
}

#[test]
fn etl_publishes_and_rerun_is_idempotent() {
    let root = temp_root("idempotent");
    let wal = fixture_wal(&root);
    let registry = protocol_registry().expect("protocol registry");
    let output = root.join("out");

    let first = process_and_publish_recording_wal(&wal, &output, &registry).expect("first publish");
    assert!(!first.already_published);
    let session_dir = output.join("sessions").join(first.session_id.to_string());
    assert!(
        session_dir.join("session.json").is_file(),
        "canonical session published"
    );
    assert!(
        session_dir.join("manifest.json").is_file(),
        "session manifest published"
    );

    let second =
        process_and_publish_recording_wal(&wal, &output, &registry).expect("rerun idempotent");
    assert_eq!(second.session_id, first.session_id);
    assert!(
        second.already_published,
        "rerun must be reported already-published"
    );

    std::fs::remove_dir_all(&root).expect("cleanup temp root");
}

#[test]
fn etl_equivalence_across_output_roots() {
    let root = temp_root("equivalence");
    let wal = fixture_wal(&root);
    let registry = protocol_registry().expect("protocol registry");

    let a = process_and_publish_recording_wal(&wal, root.join("a"), &registry).expect("output a");
    let b = process_and_publish_recording_wal(&wal, root.join("b"), &registry).expect("output b");
    assert_eq!(
        a.session_id, b.session_id,
        "one-shot equivalence across output roots"
    );

    std::fs::remove_dir_all(&root).expect("cleanup temp root");
}

#[test]
fn etl_rejects_corrupted_segment_fail_closed() {
    let root = temp_root("corruption");
    let wal = fixture_wal(&root);
    let registry = protocol_registry().expect("protocol registry");
    let output = root.join("out");
    process_and_publish_recording_wal(&wal, &output, &registry).expect("baseline publish");

    let segment = first_segment(&wal);
    let mut bytes = std::fs::read(&segment).expect("read segment");
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&segment, bytes).expect("corrupt segment byte");

    let error = process_and_publish_recording_wal(&wal, &output, &registry)
        .expect_err("corrupted WAL must fail closed");
    let message = error.to_string().to_lowercase();
    assert!(
        message.contains("checksum")
            || message.contains("corrupt")
            || message.contains("integrity")
            || message.contains("verify"),
        "unexpected corruption error: {error}"
    );

    std::fs::remove_dir_all(&root).expect("cleanup temp root");
}
#[test]
fn etl_repairs_missing_checkpoint() {
    let root = temp_root("repair");
    let wal = fixture_wal(&root);
    let registry = protocol_registry().expect("protocol registry");
    let output = root.join("out");

    let first = process_and_publish_recording_wal(&wal, &output, &registry).expect("first publish");
    let checkpoint = wal.join("etl-checkpoint.json");
    assert!(checkpoint.is_file(), "checkpoint written after publish");
    std::fs::remove_file(&checkpoint).expect("remove checkpoint");

    let second =
        process_and_publish_recording_wal(&wal, &output, &registry).expect("repair republish");
    assert_eq!(second.session_id, first.session_id);
    assert!(second.already_published, "republish after lost checkpoint");
    assert!(checkpoint.is_file(), "checkpoint regenerated");

    std::fs::remove_dir_all(&root).expect("cleanup temp root");
}
