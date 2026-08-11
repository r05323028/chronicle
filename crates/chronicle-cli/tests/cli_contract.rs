#![allow(unsafe_code)] // pre_exec dup2 of the readiness pipe onto fd 3
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Output},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn command(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chronicle"))
        .args(arguments)
        .output()
        .expect("run chronicle")
}

fn output_text(output: &Output) -> (String, String) {
    (
        String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8"),
        String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8"),
    )
}

fn deprecation_warning(stderr: &str, invocation: &str) -> serde_json::Value {
    assert_eq!(stderr.lines().count(), 1, "one deprecation diagnostic");
    let warning: serde_json::Value = serde_json::from_str(stderr).expect("deprecation JSON");
    assert_eq!(warning["version"], 1);
    assert_eq!(warning["level"], "warning");
    assert_eq!(warning["code"], "deprecated_cli");
    assert_eq!(warning["invocation"], invocation);
    assert!(
        warning["replacement"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    );
    warning
}

fn record(root: &std::path::Path) -> String {
    record_fixture(
        root,
        std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        )),
    )
}

fn record_fixture(root: &std::path::Path, fixture: &std::path::Path) -> String {
    let fixture = fixture.to_str().expect("fixture path is UTF-8");
    let root = root.to_str().expect("test root is UTF-8");
    let output = command(&[
        "record", "--source", "fixture", "--input", fixture, "--root", root,
    ]);
    assert!(output.status.success());
    let (stdout, _) = output_text(&output);
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("session_id: "))
        .expect("record prints session ID")
        .into()
}

fn production_wal(directory: &std::path::Path) {
    production_wal_from_fixture(
        directory,
        std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        )),
    );
}

fn production_wal_from_fixture(directory: &std::path::Path, fixture: &std::path::Path) {
    chronicle_application::production_wal_from_fixture(directory, fixture)
        .expect("production fixture WAL");
}

fn publish_public_recording(root: &std::path::Path, fixture: &std::path::Path) -> (String, String) {
    let staged = root.join(format!("staged-wal-{}", uuid::Uuid::new_v4()));
    production_wal_from_fixture(&staged, fixture);
    let recording_id = chronicle_application::load_recording_metadata(&staged)
        .unwrap()
        .unwrap()
        .recording_id;
    let wal = root.join("recordings").join(recording_id.to_string());
    std::fs::create_dir_all(wal.parent().unwrap()).unwrap();
    std::fs::rename(staged, &wal).unwrap();
    let published = command(&[
        "--format",
        "json",
        "internal",
        "etl",
        "--wal-dir",
        wal.to_str().unwrap(),
        "--output",
        root.to_str().unwrap(),
    ]);
    assert_eq!(published.status.code(), Some(0));
    assert!(published.stderr.is_empty());
    let published: serde_json::Value = serde_json::from_slice(&published.stdout).unwrap();
    (
        recording_id.to_cli_string(),
        published["session_id"].as_str().unwrap().to_owned(),
    )
}

fn spawn_server(response: &'static [u8]) -> (String, Arc<Mutex<Vec<u8>>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    listener.set_nonblocking(true).expect("set nonblocking");
    let address = listener.local_addr().expect("server address");
    let request = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&request);
    let task = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).expect("set blocking stream");
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .expect("set read timeout");
                    let mut bytes = [0_u8; 4096];
                    let count = stream.read(&mut bytes).expect("read request");
                    observed
                        .lock()
                        .expect("request lock poisoned")
                        .extend_from_slice(&bytes[..count]);
                    stream.write_all(response).expect("write response");
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept test request: {error}"),
            }
        }
        panic!("test server timed out waiting for request");
    });
    (format!("http://{address}"), request, task)
}

fn assert_no_connection(listener: &TcpListener) {
    thread::sleep(Duration::from_millis(50));
    assert!(matches!(
        listener.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
}

fn replay_arguments<'a>(
    session_id: &'a str,
    root: &'a str,
    target: &'a str,
    execute: bool,
    allow_read: bool,
) -> Vec<&'a str> {
    let mut arguments = vec![
        "replay",
        session_id,
        "--root",
        root,
        "--target",
        target,
        "--allow-host",
        "127.0.0.1",
    ];
    if allow_read {
        arguments.push("--allow-read");
    }
    if execute {
        arguments.push("--execute");
    }
    arguments
}

#[test]
fn cli_record_inspect_replay_and_exit_contract() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-{}", uuid::Uuid::new_v4()));
    let session_id = record(&root);
    let root_text = root.to_str().expect("test root is UTF-8");

    let inspect = command(&[
        "--format",
        "json",
        "inspect",
        &session_id,
        "--root",
        root_text,
    ]);
    assert!(inspect.status.success());
    let (inspect_stdout, _) = output_text(&inspect);
    let inspected: serde_json::Value = serde_json::from_str(&inspect_stdout).expect("inspect JSON");
    assert_eq!(inspected["version"], 1);
    let connection = &inspected["connections"][0];
    assert_eq!(connection["client"]["host"], "192.0.2.10");
    assert_eq!(connection["client"]["port"], 41_000);
    assert_eq!(connection["server"]["host"], "192.0.2.20");
    assert_eq!(connection["server"]["port"], 8_080);
    let operation = &connection["operations"][0];
    assert_eq!(operation["method"], "GET");
    assert_eq!(operation["target"], "/hello");
    assert_eq!(operation["response_status"], "200");
    assert!(!inspect_stdout.contains("\"OK\""));

    let dry_run = command(&replay_arguments(
        &session_id,
        root_text,
        "http://127.0.0.1:9",
        false,
        false,
    ));
    assert_eq!(dry_run.status.code(), Some(0));
    assert!(output_text(&dry_run).0.contains("dry_run: true"));

    let json_dry_run = command(&[
        "--format",
        "json",
        "replay",
        &session_id,
        "--root",
        root_text,
        "--target",
        "http://127.0.0.1:9",
        "--allow-host",
        "127.0.0.1",
    ]);
    assert_eq!(json_dry_run.status.code(), Some(0));
    let (stdout, stderr) = output_text(&json_dry_run);
    deprecation_warning(&stderr, "replay_root");
    assert_eq!(stdout.matches('\n').count(), 1);
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("replay JSON");
    assert_eq!(report["version"], 1);
    assert_eq!(report["result"]["outcome"], "dry_run");

    let pass_response =
        b"HTTP/1.1 200 OK\r\nx-fixture: basic\r\ncontent-length: 2\r\nconnection: close\r\n\r\nOK";
    let (pass_target, pass_request, pass_task) = spawn_server(pass_response);
    let passed = command(&replay_arguments(
        &session_id,
        root_text,
        &pass_target,
        true,
        true,
    ));
    assert_eq!(passed.status.code(), Some(0));
    assert!(output_text(&passed).0.contains("verification=passed"));
    pass_task.join().expect("pass server completed");
    assert!(
        pass_request
            .lock()
            .expect("request lock poisoned")
            .starts_with(b"GET /hello HTTP/1.1")
    );

    let mismatch_response = b"HTTP/1.1 500 Mismatch\r\nx-fixture: mismatch\r\ncontent-length: 8\r\nconnection: close\r\n\r\nmismatch";
    let (mismatch_target, _, mismatch_task) = spawn_server(mismatch_response);
    let mismatch = command(&replay_arguments(
        &session_id,
        root_text,
        &mismatch_target,
        true,
        true,
    ));
    assert_eq!(mismatch.status.code(), Some(6));
    mismatch_task.join().expect("mismatch server completed");

    std::fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn denied_legacy_replay_emits_one_hint_error_and_no_result() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-denied-{}", uuid::Uuid::new_v4()));
    let session_id = record(&root);
    let root = root.to_str().unwrap();
    for format in [None, Some("json")] {
        let mut arguments = Vec::new();
        if let Some(format) = format {
            arguments.extend(["--format", format]);
        }
        arguments.extend([
            "replay",
            &session_id,
            "--root",
            root,
            "--target",
            "http://127.0.0.1:9",
            "--allow-host",
            "127.0.0.1",
            "--execute",
        ]);
        let denied = command(&arguments);
        assert_eq!(denied.status.code(), Some(4));
        let (stdout, stderr) = output_text(&denied);
        assert!(stdout.is_empty());
        assert!(stderr.contains("Hint: use"));
        assert!(!stderr.contains("deprecated_cli"));
        if format == Some("json") {
            assert_eq!(stderr.lines().count(), 1);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&stderr).unwrap()["code"],
                4
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn public_explicit_replay_requires_gates_and_never_contacts_recorded_target() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-target-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("create test root");
    let recorded_target = TcpListener::bind("127.0.0.1:0").expect("bind recorded target");
    recorded_target
        .set_nonblocking(true)
        .expect("set recorded target nonblocking");
    let recorded_port = recorded_target
        .local_addr()
        .expect("recorded address")
        .port();
    let fixture = root.join("recorded-target.json");
    let fixture_json = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/http/basic-session.json"
    ))
    .expect("read fixture")
    .replace(
        "\"host\": \"192.0.2.20\", \"port\": 8080",
        &format!("\"host\": \"127.0.0.1\", \"port\": {recorded_port}"),
    );
    std::fs::write(&fixture, fixture_json).expect("write fixture");
    let (recording_id, _) = publish_public_recording(&root, &fixture);
    let root_text = root.to_str().expect("root is UTF-8");

    let dry_run = command(&[
        "--data-dir",
        root_text,
        "replay",
        &recording_id,
        "--target",
        "http://127.0.0.1:9",
        "--allow-host",
        "127.0.0.1",
    ]);
    assert_eq!(dry_run.status.code(), Some(0));
    assert_no_connection(&recorded_target);

    let denied = command(&[
        "--data-dir",
        root_text,
        "replay",
        &recording_id,
        "--target",
        "http://127.0.0.1:9",
        "--allow-host",
        "127.0.0.1",
        "--execute",
    ]);
    assert_eq!(denied.status.code(), Some(4));
    assert_no_connection(&recorded_target);

    let response = b"HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:9/escape\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
    let (target, _, target_task) = spawn_server(response);
    let output = command(&[
        "--data-dir",
        root_text,
        "replay",
        &recording_id,
        "--target",
        &target,
        "--allow-host",
        "127.0.0.1",
        "--allow-read",
        "--execute",
    ]);
    assert_eq!(output.status.code(), Some(6));
    target_task.join().expect("target received replay");
    assert_no_connection(&recorded_target);
    std::fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn cli_json_replay_error_has_no_partial_stdout() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-json-{}", uuid::Uuid::new_v4()));
    let session_id = record(&root);
    let config = root.join("config.toml");
    std::fs::write(
        &config,
        "[replay]\nauthorization_env = \"CHRONICLE_TEST_MISSING_AUTH\"\n",
    )
    .expect("write config");
    let output = command(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "replay",
        &session_id,
        "--root",
        root.to_str().unwrap(),
        "--target",
        "http://127.0.0.1:9",
        "--allow-host",
        "127.0.0.1",
        "--allow-read",
        "--execute",
    ]);
    assert_eq!(output.status.code(), Some(3));
    let (stdout, stderr) = output_text(&output);
    assert!(stdout.is_empty(), "unexpected stdout: {stdout:?}");
    assert!(serde_json::from_str::<serde_json::Value>(&stderr).is_ok());
    std::fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn cli_dry_run_ignores_unneeded_missing_runtime_credential() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-dry-{}", uuid::Uuid::new_v4()));
    let session_id = record(&root);
    let config = root.join("config.toml");
    std::fs::write(
        &config,
        "[replay]\nauthorization_env = \"CHRONICLE_TEST_MISSING_AUTH\"\n",
    )
    .expect("write config");
    let output = command(&[
        "--config",
        config.to_str().unwrap(),
        "replay",
        &session_id,
        "--root",
        root.to_str().unwrap(),
        "--target",
        "http://127.0.0.1:9",
        "--allow-host",
        "127.0.0.1",
    ]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output_text(&output).0.contains("dry_run: true"));
    std::fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn cli_etl_publishes_repairs_checkpoint_and_rejects_corruption() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-etl-{}", uuid::Uuid::new_v4()));
    let wal = root.join("wal");
    let output = root.join("output");
    production_wal(&wal);
    let wal = wal.to_str().expect("WAL path is UTF-8");
    let output = output.to_str().expect("output path is UTF-8");

    let first = command(&[
        "--format",
        "json",
        "etl",
        "--wal-dir",
        wal,
        "--output",
        output,
    ]);
    assert!(first.status.success());
    let (stdout, stderr) = output_text(&first);
    deprecation_warning(&stderr, "etl");
    let first: serde_json::Value = serde_json::from_str(&stdout).expect("ETL JSON");
    assert_eq!(first["already_processed"], false);
    assert!(first["recording_id"].is_string());
    assert!(first["status"].is_string());
    assert!(first["shutdown_reason"].is_string());
    assert!(first["counters"]["committed"]["records"].is_u64());
    assert_eq!(first["checkpoint"]["version"], 1);
    assert!(first["checkpoint"]["wal_snapshot_sha256"].is_string());
    assert!(first["checkpoint"]["output_identity"].is_string());
    let session_id = first["session_id"].as_str().expect("session ID").to_owned();

    std::fs::remove_file(std::path::Path::new(wal).join("etl-checkpoint.json"))
        .expect("remove checkpoint");
    let same = command(&[
        "--format",
        "json",
        "internal",
        "etl",
        "--wal-dir",
        wal,
        "--output",
        output,
    ]);
    assert!(same.status.success());
    let (stdout, stderr) = output_text(&same);
    assert!(stderr.is_empty());
    let same: serde_json::Value = serde_json::from_str(&stdout).expect("ETL JSON");
    assert_eq!(same["session_id"], session_id);
    assert_eq!(same["already_processed"], true);
    assert!(
        std::path::Path::new(wal)
            .join("etl-checkpoint.json")
            .exists()
    );

    let second_output = root.join("second-output");
    let second_output = second_output.to_str().expect("second output path is UTF-8");
    let second = command(&[
        "--format",
        "json",
        "etl",
        "--wal-dir",
        wal,
        "--output",
        second_output,
    ]);
    assert!(second.status.success());
    let (stdout, stderr) = output_text(&second);
    deprecation_warning(&stderr, "etl");
    let second: serde_json::Value = serde_json::from_str(&stdout).expect("ETL JSON");
    assert_eq!(second["session_id"], session_id);
    assert_eq!(second["already_processed"], false);

    let segment = std::path::Path::new(wal).join("segments/00000000000000000001.chwal");
    let mut bytes = std::fs::read(&segment).expect("read segment");
    *bytes.last_mut().expect("segment content") ^= 1;
    std::fs::write(segment, bytes).expect("corrupt segment");
    let corrupt = command(&[
        "--format",
        "json",
        "etl",
        "--wal-dir",
        wal,
        "--output",
        output,
    ]);
    assert_eq!(corrupt.status.code(), Some(3));
    let (stdout, stderr) = output_text(&corrupt);
    assert!(stdout.is_empty());
    assert!(serde_json::from_str::<serde_json::Value>(&stderr).is_ok());

    std::fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn cli_reports_usage_and_data_errors_as_safe_json() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-errors-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("create test root");
    let malformed = root.join("malformed.json");
    std::fs::write(&malformed, b"{}").expect("write malformed fixture");

    let malformed_output = command(&[
        "record",
        "--source",
        "fixture",
        "--input",
        malformed.to_str().expect("fixture path is UTF-8"),
        "--root",
        root.to_str().expect("root is UTF-8"),
    ]);
    assert_eq!(malformed_output.status.code(), Some(3));

    let unknown = command(&[
        "--format",
        "json",
        "inspect",
        "00000000-0000-0000-0000-000000000000",
        "--root",
        root.to_str().expect("root is UTF-8"),
    ]);
    assert_eq!(unknown.status.code(), Some(3));
    let (unknown_stdout, unknown_stderr) = output_text(&unknown);
    assert!(unknown_stdout.is_empty());
    let error: serde_json::Value = serde_json::from_str(&unknown_stderr).expect("JSON error");
    assert_eq!(error["version"], 1);
    assert_eq!(error["code"], 3);

    let doctor = command(&["--format", "json", "doctor"]);
    assert_eq!(doctor.status.code(), Some(4));
    let (doctor_stdout, doctor_stderr) = output_text(&doctor);
    assert!(doctor_stderr.is_empty());
    assert_eq!(doctor_stdout.matches('\n').count(), 1);
    let doctor: serde_json::Value = serde_json::from_str(&doctor_stdout).unwrap();
    assert_eq!(doctor["version"], 1);
    assert_eq!(doctor["status"], "unsupported");
    assert!(doctor["probes"].is_array());
    assert!(doctor["probes"].as_array().unwrap().iter().all(|probe| {
        probe["remediation"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    }));
    assert!(doctor_stdout.contains("capture.platform"));
    assert!(!doctor_stdout.contains("distribution"));

    let etl = command(&["--format", "json", "etl"]);
    assert_eq!(etl.status.code(), Some(2));
    let (etl_stdout, etl_stderr) = output_text(&etl);
    assert!(etl_stdout.is_empty());
    assert!(!etl_stderr.is_empty());

    let usage = command(&["record"]);
    assert_eq!(usage.status.code(), Some(2));
    std::fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn doctor_config_failure_keeps_independent_probes_and_human_remediation() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-doctor-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let config = root.join("bad.toml");
    std::fs::write(&config, "not = [valid").unwrap();
    let json = command(&[
        "--format",
        "json",
        "--config",
        config.to_str().unwrap(),
        "--data-dir",
        root.to_str().unwrap(),
        "doctor",
    ]);
    let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let codes: Vec<_> = report["probes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|probe| probe["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"replay.configuration"));
    assert!(codes.contains(&"data_dir.access"));
    assert!(codes.contains(&"capture.platform"));

    let human = command(&["--data-dir", root.to_str().unwrap(), "doctor"]);
    assert!(output_text(&human).0.contains("remediation:"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_warnings_and_failure_hints_are_atomic_and_non_secret() {
    let secret = "super-secret-cli-value";
    let root =
        std::env::temp_dir().join(format!("chronicle-cli-{secret}-{}", uuid::Uuid::new_v4()));
    let fixture = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/http/basic-session.json"
    ));
    let success = command(&[
        "--format",
        "json",
        "record",
        "--source",
        "fixture",
        "--input",
        fixture.to_str().unwrap(),
        "--root",
        root.to_str().unwrap(),
    ]);
    assert_eq!(success.status.code(), Some(0));
    let (stdout, stderr) = output_text(&success);
    let result: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let session_id = result["session_id"].as_str().unwrap();
    let warning = deprecation_warning(&stderr, "record_source_fixture");
    assert!(!warning.to_string().contains(secret));

    let human = command(&["inspect", session_id, "--root", root.to_str().unwrap()]);
    assert_eq!(human.status.code(), Some(0));
    let (_, human_stderr) = output_text(&human);
    assert_eq!(human_stderr.lines().count(), 1);
    assert!(human_stderr.contains("deprecated CLI form 'inspect_root'"));
    assert!(!human_stderr.contains(secret));

    let invalid = command(&[
        "--format",
        "json",
        "record",
        "--source",
        "ebpf",
        "--wal-dir",
        secret,
    ]);
    assert_eq!(invalid.status.code(), Some(2));
    let (invalid_stdout, invalid_stderr) = output_text(&invalid);
    assert!(invalid_stdout.is_empty());
    assert_eq!(invalid_stderr.lines().count(), 1);
    let error: serde_json::Value = serde_json::from_str(&invalid_stderr).unwrap();
    assert_eq!(error["code"], 2);
    assert!(error["message"].as_str().unwrap().contains("Hint: use"));
    assert!(!invalid_stderr.contains("deprecated_cli"));
    assert!(!invalid_stderr.contains(secret));

    let failed = command(&[
        "--format",
        "json",
        "inspect",
        "00000000-0000-0000-0000-000000000000",
        "--root",
        root.join(secret).to_str().unwrap(),
    ]);
    assert_eq!(failed.status.code(), Some(3));
    let (failed_stdout, failed_stderr) = output_text(&failed);
    assert!(failed_stdout.is_empty());
    assert_eq!(failed_stderr.lines().count(), 1);
    let error: serde_json::Value = serde_json::from_str(&failed_stderr).unwrap();
    assert!(error["message"].as_str().unwrap().contains("Hint: use"));
    assert!(!failed_stderr.contains("deprecated_cli"));
    assert!(!failed_stderr.contains(secret));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_legacy_parse_error_has_safe_new_syntax_hint() {
    for arguments in [
        vec!["--format", "json", "record", "--source"],
        vec![
            "--format",
            "json",
            "--bogus",
            "super-secret",
            "etl",
            "--wal-dir",
            "wal",
            "--output",
            "output",
        ],
    ] {
        let output = command(&arguments);
        assert_eq!(output.status.code(), Some(2));
        let (stdout, stderr) = output_text(&output);
        assert!(stdout.is_empty());
        assert_eq!(stderr.lines().count(), 1);
        let error: serde_json::Value = serde_json::from_str(&stderr).unwrap();
        assert_eq!(error["code"], 2);
        assert!(error["message"].as_str().unwrap().contains("Hint: use"));
        assert!(!stderr.contains("deprecated_cli"));
        assert!(!stderr.contains("super-secret"));
    }
}

#[test]
fn empty_list_and_legacy_inspect_contract() {
    let empty_dir =
        std::env::temp_dir().join(format!("chronicle-cli-empty-{}", uuid::Uuid::new_v4()));
    let empty = command(&["--data-dir", empty_dir.to_str().unwrap(), "list"]);
    assert_eq!(empty.status.code(), Some(0));
    let (human_stdout, _) = output_text(&empty);
    assert!(human_stdout.contains("No recordings yet."));
    assert!(human_stdout.contains("chronicle record -- COMMAND..."));

    let empty_json = command(&[
        "--format",
        "json",
        "--data-dir",
        empty_dir.to_str().unwrap(),
        "list",
    ]);
    assert_eq!(empty_json.status.code(), Some(0));
    let (json_stdout, json_stderr) = output_text(&empty_json);
    assert!(json_stderr.is_empty());
    let value: serde_json::Value = serde_json::from_str(&json_stdout).expect("one atomic JSON");
    assert_eq!(value["version"], 1);
    assert_eq!(value["recordings"].as_array().map(Vec::len), Some(0));

    let root = std::env::temp_dir().join(format!("chronicle-cli-pop-{}", uuid::Uuid::new_v4()));
    let session_id = record(&root);
    let inspect = command(&[
        "--format",
        "json",
        "inspect",
        &session_id,
        "--root",
        root.to_str().unwrap(),
    ]);
    assert_eq!(inspect.status.code(), Some(0));
    assert!(serde_json::from_slice::<serde_json::Value>(&inspect.stdout).is_ok());
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(empty_dir).ok();
}

#[test]
fn public_inspect_resolves_recording_identity() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-public-{}", uuid::Uuid::new_v4()));
    let fixture = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/http/basic-session.json"
    ));
    let (recording_id, session_id) = publish_public_recording(&root, fixture);

    let listed = command(&[
        "--format",
        "json",
        "--data-dir",
        root.to_str().unwrap(),
        "list",
    ]);
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["recordings"][0]["recording_id"], recording_id);
    let latest = command(&[
        "--format",
        "json",
        "--data-dir",
        root.to_str().unwrap(),
        "inspect",
        "latest",
    ]);
    assert_eq!(latest.status.code(), Some(0));
    let latest: serde_json::Value = serde_json::from_slice(&latest.stdout).unwrap();
    assert_eq!(latest["version"], 1);
    assert_eq!(latest["recording_id"], recording_id);
    assert_eq!(latest["session_id"], session_id);
    assert_eq!(latest["sessions"], 1);
    assert!(latest["operations"].as_u64().is_some());

    let by_id = command(&[
        "--data-dir",
        root.to_str().unwrap(),
        "inspect",
        &recording_id,
    ]);
    assert_eq!(by_id.status.code(), Some(0));
    let (by_id_stdout, _) = output_text(&by_id);
    assert!(by_id_stdout.starts_with(&format!("recording_id: {recording_id}\n")));
    assert!(by_id_stdout.contains("replayability:"));
    assert!(by_id_stdout.contains("connection: http"));

    let missing = command(&[
        "--format",
        "json",
        "--data-dir",
        root.to_str().unwrap(),
        "inspect",
        "rec_00000000-0000-0000-0000-000000000000",
    ]);
    assert_eq!(missing.status.code(), Some(3));
    let (missing_stdout, missing_stderr) = output_text(&missing);
    assert!(missing_stdout.is_empty());
    let error: serde_json::Value = serde_json::from_str(&missing_stderr).unwrap();
    assert_eq!(error["code"], 3);
    assert!(error["message"].as_str().unwrap().contains("was not found"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn doctor_prospective_data_dir_is_non_mutating_and_actionable() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-doctor-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let prospective = root.join("prospective").join("chronicle");
    let doctor = command(&[
        "--format",
        "json",
        "--data-dir",
        prospective.to_str().unwrap(),
        "doctor",
    ]);
    let doctor: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let data_probe = doctor["probes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|probe| probe["code"] == "data_dir.access")
        .expect("data-dir probe");
    assert_eq!(data_probe["status"], "not_checked");
    assert!(
        data_probe["message"]
            .as_str()
            .unwrap()
            .contains("--data-dir")
    );
    assert!(!data_probe["remediation"].as_str().unwrap().is_empty());
    assert!(!prospective.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
#[test]
fn rootless_public_record_fails_before_target_on_unsupported_build() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-record-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let marker = root.join("target-started");
    let output = command(&[
        "--format",
        "json",
        "--data-dir",
        root.to_str().unwrap(),
        "record",
        "--",
        "/usr/bin/touch",
        marker.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(4));
    let (stdout, stderr) = output_text(&output);
    assert!(stdout.is_empty());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stderr).unwrap()["code"],
        4
    );
    assert!(!marker.exists());
    std::fs::remove_dir_all(root).ok();
}

#[cfg(unix)]
#[test]
fn public_list_storage_error_exits_three_without_stdout() {
    let root = std::env::temp_dir().join(format!("chronicle-cli-list-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let outside = root.with_extension("outside");
    std::fs::write(&outside, "{}").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("catalog.json")).unwrap();
    let output = command(&[
        "--format",
        "json",
        "--data-dir",
        root.to_str().unwrap(),
        "list",
    ]);
    assert_eq!(output.status.code(), Some(3));
    let (stdout, stderr) = output_text(&output);
    assert!(stdout.is_empty());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stderr).unwrap()["code"],
        3
    );
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_file(outside).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn rootless_command_replay_fails_before_target_without_access_separation() {
    let root =
        std::env::temp_dir().join(format!("chronicle-cli-rootless-{}", uuid::Uuid::new_v4()));
    let fixture = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/http/basic-session.json"
    ));
    let (recording_id, _) = publish_public_recording(&root, fixture);
    let marker = root.join("target-started");
    let output = command(&[
        "--format",
        "json",
        "--data-dir",
        root.to_str().unwrap(),
        "replay",
        &recording_id,
        "--",
        "/usr/bin/touch",
        marker.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(4));
    let (stdout, stderr) = output_text(&output);
    assert!(stdout.is_empty());
    let error: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(error["code"], 4);
    assert!(
        error["message"].as_str().is_some_and(|message| {
            message.contains("cgroup") || message.contains("target user")
        })
    );
    assert!(!marker.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[allow(clippy::too_many_lines)] // One process lifecycle test keeps fd setup and all failure edges visible.
fn bootstrap_blocks_until_ready_then_hardens_and_execs_target() {
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::process::CommandExt;

    // A pipe becomes the readiness channel: read end is dup2'd onto fd 3 in
    // the child; the parent writes the go byte after "attachment".
    let pipe = os_pipe().expect("pipe");
    let read_end = pipe.0;
    let write_end = pipe.1;

    let uid = std::fs::metadata("/proc/self").unwrap().uid();
    let gid = std::fs::metadata("/proc/self").unwrap().gid();
    let supplementary_groups_remain = std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Groups:"))
        .is_some_and(|groups| !groups.trim().is_empty());
    let can_clear_groups = rustix::process::geteuid().as_raw() == 0 || !supplementary_groups_remain;

    let mut command = Command::new(env!("CARGO_BIN_EXE_chronicle"));
    command.args([
        "internal",
        "bootstrap",
        "--uid",
        &uid.to_string(),
        "--gid",
        &gid.to_string(),
        "--",
        "/bin/sh",
        "-c",
        "exit 42",
    ]);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    unsafe {
        command.pre_exec(move || {
            // Place the readiness pipe read end on fixed fd 3 in the child.
            // When it already IS fd 3, only clear CLOEXEC so it survives the
            // bootstrap's exec of the chronicle binary. Both wrappers are
            // forgotten on every path so no descriptor drops in the child.
            use std::os::fd::FromRawFd;
            let source = std::fs::File::from_raw_fd(read_end);
            let mut target = std::os::fd::OwnedFd::from_raw_fd(3);
            let result = if read_end == 3 {
                rustix::io::fcntl_setfd(&target, rustix::io::FdFlags::empty())
            } else {
                rustix::io::dup2(&source, &mut target)
            };
            std::mem::forget(source);
            std::mem::forget(target);
            result.map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))
        });
    }
    let mut child = command.spawn().expect("spawn bootstrap");
    // The bootstrap must be blocked: give it a moment, then confirm it has
    // not exec'd (no exit yet) before releasing.
    std::thread::sleep(std::time::Duration::from_millis(200));
    if let Some(status) = child.try_wait().unwrap() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        panic!("bootstrap must block before readiness; exited {status:?} stderr: {stderr}");
    }
    // Release: capture attached, WAL durable.
    let mut write_end = unsafe { std::fs::File::from_raw_fd(write_end) };
    write_end.write_all(b"G").expect("write go byte");
    drop(write_end);
    let status = child.wait().expect("wait bootstrap");
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal = None;
    if can_clear_groups {
        // The target (sh -c 'exit 42') exit code 42 is reported factually.
        assert_eq!(
            status.code(),
            Some(42),
            "target exit code must be reported; signal={signal:?} stderr={stderr:?}"
        );
    } else {
        // Rootless host with inherited supplementary groups cannot satisfy the
        // hardening contract; bootstrap fails closed instead of execing.
        assert_eq!(status.code(), Some(3));
        assert!(stderr.contains("supplementary groups remain"));
    }

    // Failure path: without the go byte, EOF aborts the bootstrap with the
    // typed Chronicle failure code (never a synthesized 127).
    let pipe = os_pipe().expect("pipe");
    let read_end = pipe.0;
    let write_end = pipe.1;
    let mut command = Command::new(env!("CARGO_BIN_EXE_chronicle"));
    command.args([
        "internal",
        "bootstrap",
        "--uid",
        &uid.to_string(),
        "--gid",
        &gid.to_string(),
        "--",
        "/bin/true",
    ]);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    unsafe {
        command.pre_exec(move || {
            use std::os::fd::FromRawFd;
            let source = std::fs::File::from_raw_fd(read_end);
            let mut target = std::os::fd::OwnedFd::from_raw_fd(3);
            let result = if read_end == 3 {
                rustix::io::fcntl_setfd(&target, rustix::io::FdFlags::empty())
            } else {
                rustix::io::dup2(&source, &mut target)
            };
            std::mem::forget(source);
            std::mem::forget(target);
            result.map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))
        });
    }
    let mut child = command.spawn().expect("spawn bootstrap");
    // Close the write end without sending a go byte: EOF must abort. The raw
    // fd must be wrapped so dropping it actually closes the pipe.
    let write_end = unsafe { std::fs::File::from_raw_fd(write_end) };
    drop(write_end);
    let status = child.wait().expect("wait bootstrap");
    assert_eq!(
        status.code(),
        Some(3),
        "EOF before readiness is a typed failure"
    );

    // Hidden bootstrap credentials are not an authorization surface: direct
    // callers cannot request an identity different from inherited real IDs.
    let mismatched_uid = u32::from(uid == 0);
    let status = Command::new(env!("CARGO_BIN_EXE_chronicle"))
        // Closed/non-inherited fd 4 must be rejected before unsafe ownership.
        .env("CHRONICLE_BOOTSTRAP_STATUS_FD", "4")
        .args([
            "internal",
            "bootstrap",
            "--uid",
            &mismatched_uid.to_string(),
            "--gid",
            &gid.to_string(),
            "--",
            "/bin/true",
        ])
        .status()
        .expect("run mismatched bootstrap");
    assert_eq!(status.code(), Some(3));
}

#[cfg(target_os = "linux")]
fn os_pipe() -> Option<(std::os::fd::RawFd, std::os::fd::RawFd)> {
    use std::os::fd::AsRawFd;
    // CLOEXEC so the child's exec of the bootstrap closes every inherited end
    // except the dup2'd fd 3; otherwise the child would keep a write end open
    // and the EOF-abort path could never fire.
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).ok()?;
    // Leak the OwnedFds so the raw fds stay open for the child.
    let read = std::mem::ManuallyDrop::new(read).as_raw_fd();
    let write = std::mem::ManuallyDrop::new(write).as_raw_fd();
    Some((read, write))
}
