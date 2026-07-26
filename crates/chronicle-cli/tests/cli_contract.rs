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

fn record(root: &std::path::Path) -> String {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/http/basic-session.json"
    );
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
    let operation = &inspected["connections"][0]["operations"][0];
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

    let denied = command(&replay_arguments(
        &session_id,
        root_text,
        "http://127.0.0.1:9",
        true,
        false,
    ));
    assert_eq!(denied.status.code(), Some(4));

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
    assert_eq!(error["code"], 3);

    let doctor = command(&["--format", "json", "doctor"]);
    assert_eq!(doctor.status.code(), Some(0));
    let (doctor_stdout, doctor_stderr) = output_text(&doctor);
    assert!(doctor_stderr.is_empty());
    assert_eq!(doctor_stdout.matches('\n').count(), 1);
    assert!(serde_json::from_str::<serde_json::Value>(&doctor_stdout).is_ok());

    let etl = command(&["--format", "json", "etl"]);
    assert_eq!(etl.status.code(), Some(3));
    let (etl_stdout, etl_stderr) = output_text(&etl);
    assert!(etl_stdout.is_empty());
    assert_eq!(etl_stderr.matches('\n').count(), 1);
    assert!(serde_json::from_str::<serde_json::Value>(&etl_stderr).is_ok());

    let usage = command(&["record"]);
    assert_eq!(usage.status.code(), Some(2));
    std::fs::remove_dir_all(root).expect("remove test root");
}
