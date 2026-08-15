mod support;

use chronicle_application::{
    ReplayConfig, ReplayRequest, ReplayTiming, inspect_session, record_fixture, replay_session,
};
use chronicle_capture::FixtureCaptureSource;
use chronicle_wal::segment_file_name;
use support::{HttpTestServer, ResponseMode};

fn fixture() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/http/basic-session.json"
    ))
    .expect("read fixture")
}

fn options(target: String) -> ReplayRequest {
    ReplayRequest {
        target: Some(target),
        allow_hosts: vec!["127.0.0.1".into()],
        execute: true,
        allow_reads: true,
        allow_writes: false,
        timing: ReplayTiming::Asap,
    }
}

#[tokio::test]
async fn fixture_wal_http_artifact_replay_and_verification_vertical_slice() {
    let root =
        std::env::temp_dir().join(format!("chronicle-http-vertical-{}", uuid::Uuid::new_v4()));
    let fixture_path = root.join("fixture.json");
    std::fs::create_dir_all(&root).expect("create test root");
    std::fs::write(&fixture_path, fixture()).expect("write fixture copy");
    let mut source =
        FixtureCaptureSource::from_json(&std::fs::read(&fixture_path).expect("read fixture copy"))
            .expect("parse fixture");
    let recorded = record_fixture(&mut source, &root, 1024 * 1024).expect("record fixture");

    let segment = root
        .join("wal")
        .join(recorded.session_id.to_string())
        .join("segments")
        .join(segment_file_name(1));
    assert!(segment.is_file());
    assert_eq!(recorded.checkpoint.next_sequence, 6);

    std::fs::remove_file(fixture_path).expect("remove fixture copy");
    std::fs::remove_dir_all(root.join("wal")).expect("remove WAL");
    let inspected = inspect_session(&root, recorded.session_id).expect("inspect persisted session");
    assert!(inspected.replayable);

    let pass_server = HttpTestServer::spawn(ResponseMode::Pass).await;
    let passed = replay_session(
        &root,
        &recorded.session_id.to_string(),
        &ReplayConfig::default(),
        &options(pass_server.origin()),
    )
    .await
    .expect("replay against matching server");
    assert_eq!(passed.operations[0].verification, "passed");
    assert_eq!(pass_server.requests().len(), 1);

    let mismatch_server = HttpTestServer::spawn(ResponseMode::Mismatch).await;
    let mismatched = replay_session(
        &root,
        &recorded.session_id.to_string(),
        &ReplayConfig::default(),
        &options(mismatch_server.origin()),
    )
    .await
    .expect("replay against mismatch server");
    assert_eq!(mismatched.operations[0].verification, "failed");
    assert_eq!(mismatch_server.requests().len(), 1);

    std::fs::remove_dir_all(root).expect("remove test root");
}
