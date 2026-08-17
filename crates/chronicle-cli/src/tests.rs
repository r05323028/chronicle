use super::*;
use crate::args::parse_duration;
use chronicle_application::{ReplayStatus, ReplayabilityStatus};

#[derive(Default)]
struct TestWriter {
    bytes: Vec<u8>,
    fail_write: bool,
    fail_flush: bool,
    flushes: usize,
}

impl Write for TestWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.fail_write {
            return Err(io::Error::other("injected write failure"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        if self.fail_flush {
            return Err(io::Error::other("injected flush failure"));
        }
        Ok(())
    }
}

#[test]
fn output_boundary_checks_write_and_flush() {
    let mut writer = TestWriter::default();
    write_output(&mut writer, "{\"ok\":true}").unwrap();
    assert_eq!(writer.bytes, b"{\"ok\":true}\n");
    assert_eq!(writer.flushes, 1);

    let mut write_failure = TestWriter {
        fail_write: true,
        ..TestWriter::default()
    };
    assert!(write_output(&mut write_failure, "{}").is_err());
    assert_eq!(write_failure.flushes, 0);

    let mut flush_failure = TestWriter {
        fail_flush: true,
        ..TestWriter::default()
    };
    assert!(write_output(&mut flush_failure, "{}").is_err());
    assert_eq!(flush_failure.bytes, b"{}\n");
    assert_eq!(flush_failure.flushes, 1);
}

#[test]
fn cli_parses_internal_recorder_status_command() {
    let cli = Cli::try_parse_from([
        "chronicle",
        "internal",
        "recorder-status",
        "--state-root",
        "state",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Command::Internal(InternalCommand::RecorderStatus { state_root })
            if state_root.as_path() == std::path::Path::new("state")
    ));
}

#[test]
fn cli_parses_internal_foreground_recorder_command() {
    let cli = Cli::try_parse_from([
        "chronicle",
        "internal",
        "recorder",
        "--config",
        "recorder.toml",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Command::Internal(InternalCommand::Recorder)
    ));
    assert_eq!(cli.config, Some(PathBuf::from("recorder.toml")));
}

#[test]
fn cli_parses_internal_record_fixture_and_etl() {
    let record = Cli::try_parse_from([
        "chronicle",
        "internal",
        "record-fixture",
        "--input",
        "fixture.json",
        "--root",
        "root",
    ])
    .unwrap();
    assert!(matches!(
        record.command,
        Command::Internal(InternalCommand::RecordFixture { .. })
    ));

    let etl = Cli::try_parse_from([
        "chronicle",
        "internal",
        "etl",
        "--wal-dir",
        "recording",
        "--output",
        "output",
    ])
    .unwrap();
    assert!(matches!(
        etl.command,
        Command::Internal(InternalCommand::Etl { wal_dir, output })
            if wal_dir.as_path() == std::path::Path::new("recording")
                && output.as_path() == std::path::Path::new("output")
    ));
}

#[test]
fn record_source_flag_is_rejected() {
    // The pre-0.1 record --source entrypoints were removed entirely;
    // clap rejects any --source form.
    assert!(
        Cli::try_parse_from([
            "chronicle",
            "record",
            "--source",
            "fixture",
            "--input",
            "fixture.json",
            "--root",
            "root",
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "chronicle",
            "record",
            "--source",
            "ebpf",
            "--cgroup",
            "/workload",
        ])
        .is_err()
    );
}
#[test]
fn production_record_help_hides_mechanics_and_shows_intent_options() {
    let mut command = Cli::command();
    let help = command
        .find_subcommand_mut("record")
        .expect("record subcommand")
        .render_long_help()
        .to_string();
    // Public record help shows intent options and hides legacy mechanics.
    assert!(help.contains("--duration"));
    assert!(help.contains("--name"));
    assert!(help.contains("--retry"));
    assert!(!help.contains("--source"));
    assert!(!help.contains("--wal-dir"));
    assert!(!help.contains("--segment-bytes"));
}

#[test]
fn production_record_arguments_reject_bad_selectors() {
    let mixed = Cli::try_parse_from([
        "chronicle",
        "record",
        "--pid",
        "42",
        "--cgroup",
        "/workload",
    ])
    .unwrap();
    assert!(validate_record_arguments(&mixed).is_err());

    let no_mode = Cli::try_parse_from(["chronicle", "record", "--name", "x"]).unwrap();
    assert!(validate_record_arguments(&no_mode).is_err());
}

#[test]
fn public_record_v1_human_and_json_are_stable() {
    let result = CommandRecordResult {
        recording_id: chronicle_application::RecordingId(
            uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
        ),
        name: None,
        status: RecordingStatus::Completed,
        shutdown_reason: Some(ShutdownReason::DurationLimit),
        child_exit: None,
        cleanup: CleanupOutcome::Clean,
        duration_ms: 62_000,
        operations: 3,
        counters: RecordingCounters::default(),
    };
    assert_eq!(
        render_command_record(&result, Format::Human).unwrap(),
        "Recording complete.\n  id: rec_00000000-0000-0000-0000-000000000001\n  duration: 1m 2s\n  operations: 3\n  dropped: 0\nTry:\n  chronicle inspect rec_00000000-0000-0000-0000-000000000001\n  chronicle replay rec_00000000-0000-0000-0000-000000000001 -- COMMAND..."
    );
    let json: serde_json::Value =
        serde_json::from_str(&render_command_record(&result, Format::Json).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(
        json["recording_id"],
        "rec_00000000-0000-0000-0000-000000000001"
    );
    assert!(json["name"].is_null());
    assert_eq!(json["status"], "completed");
    assert_eq!(json["shutdown_reason"], "duration_limit");
    assert_eq!(json["duration_ms"], 62_000);
    assert_eq!(json["operations"], 3);
    assert_eq!(json["dropped"], 0);
    assert!(json["child_exit"].is_null());
    assert_eq!(json["counters"]["committed"]["records"], 0);

    let mut with_child = result.clone();
    with_child.child_exit = Some(ChildExitResult::ExitCode { code: 42 });
    let child_json: serde_json::Value =
        serde_json::from_str(&render_command_record(&with_child, Format::Json).unwrap()).unwrap();
    assert_eq!(child_json["child_exit"]["kind"], "exit_code");
    assert_eq!(child_json["child_exit"]["code"], 42);
    assert!(
        render_command_record(&with_child, Format::Human)
            .unwrap()
            .contains("child: exit_code 42")
    );

    let mut lossy = result;
    lossy.name = Some("escaped\nname".into());
    lossy.counters.kernel_or_backend_dropped.records = 2;
    lossy.counters.rejected_due_to_quota.records = 3;
    let lossy_json = render_command_record(&lossy, Format::Json).unwrap();
    assert_eq!(lossy_json.lines().count(), 1);
    assert!(lossy_json.contains("escaped\\nname"));
    let lossy_json: serde_json::Value = serde_json::from_str(&lossy_json).unwrap();
    assert_eq!(lossy_json["dropped"], 5);
    assert_eq!(
        lossy_json["counters"]["kernel_or_backend_dropped"]["records"],
        2
    );
}

#[test]
fn public_replay_human_pass_and_fail_are_stable() {
    let result =
        |outcome, attempted, completed, failed| chronicle_application::ReplaySessionResult {
            session_id: "session".into(),
            replayability: ReplayabilityStatus::FullyReplayable,
            outcome,
            dry_run: outcome == ReplayStatus::DryRun,
            preflight_denied: outcome == ReplayStatus::StoppedPolicy,
            transport_failed: outcome == ReplayStatus::StoppedTransport,
            counts: chronicle_application::ReplayCounts {
                attempted,
                completed,
                failed,
                ..chronicle_application::ReplayCounts::default()
            },
            operations: Vec::new(),
        };
    let passed = result(ReplayStatus::Completed, 1, 1, 0);
    let passed_text = render_public_replay_human(&passed, &passed, None);
    assert!(passed_text.contains(
        "operations: attempted=1 completed=1 failed=0 unattempted=0 verification_failed=0"
    ));
    assert!(passed_text.ends_with("✓ passed\nReplay passed."));

    let mut failed = result(ReplayStatus::StoppedVerification, 1, 1, 0);
    failed.counts.verification_failed = 1;
    let failed_text = render_public_replay_human(&failed, &failed, None);
    assert!(failed_text.contains(
        "operations: attempted=1 completed=1 failed=0 unattempted=0 verification_failed=1"
    ));
    assert!(failed_text.ends_with("✗ failed\nReplay failed."));
    let orphaned = render_public_replay_human(
        &passed,
        &passed,
        Some(&CleanupOutcome::TimedOut { remaining: vec![7] }),
    );
    assert!(orphaned.ends_with("✗ failed\nReplay failed."));
}

#[test]
fn command_replay_cleanup_timeout_reports_orphan_evidence() {
    let cleanup = CleanupOutcome::TimedOut {
        remaining: vec![41, 42],
    };
    let json = serde_json::to_value(replay_cleanup_json(&cleanup)).unwrap();
    assert_eq!(json["status"], "timed_out");
    assert_eq!(json["possible_orphan_pids"], serde_json::json!([41, 42]));
    assert!(replay_cleanup_human(&cleanup).contains("possible orphan pids"));
}

#[cfg(test)]
mod new_surface_tests {
    use super::*;

    #[test]
    fn duration_parser_accepts_optional_uncapped_deadlines() {
        assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_mins(5));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_hours(1));
        assert_eq!(parse_duration("24h").unwrap(), Duration::from_hours(24));
        assert_eq!(
            parse_duration("1500ms").unwrap(),
            Duration::from_millis(1500)
        );
        assert!(parse_duration("0").is_err());
        assert!(parse_duration("1w").is_err());
        assert!(parse_duration("bogus").is_err());
    }

    #[test]
    fn retry_conflicts_with_capture_options_are_usage_errors() {
        let conflicts = [
            vec!["--retry", "rec_1", "--name", "x"],
            vec!["--retry", "rec_1", "--duration", "5m"],
            vec!["--retry", "rec_1", "--pid", "42"],
            vec!["--retry", "rec_1", "--cgroup", "/w"],
            vec!["--retry", "rec_1", "--", "echo", "hi"],
        ];
        for extra in conflicts {
            let mut argv = vec!["chronicle", "record"];
            argv.extend(extra.clone());
            let parsed = Cli::try_parse_from(argv).expect("parses");
            assert!(validate_record_arguments(&parsed).is_err(), "{extra:?}");
        }
    }

    #[test]
    fn record_requires_a_mode_and_selectors_are_mutually_exclusive() {
        let bare = Cli::try_parse_from(["chronicle", "record"]).unwrap();
        assert!(validate_record_arguments(&bare).is_err());

        let both_selectors =
            Cli::try_parse_from(["chronicle", "record", "--pid", "1", "--cgroup", "/w"]).unwrap();
        assert!(validate_record_arguments(&both_selectors).is_err());

        let command_mode =
            Cli::try_parse_from(["chronicle", "record", "--", "echo", "hi"]).unwrap();
        assert!(validate_record_arguments(&command_mode).is_ok());
    }

    #[test]
    fn replay_target_and_command_modes_conflict_in_clap() {
        let error = Cli::try_parse_from([
            "chronicle",
            "replay",
            "latest",
            "--target",
            "http://127.0.0.1:8080",
            "--",
            "/bin/true",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn replay_help_shows_both_modes_and_write_stays_explicit() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("replay")
            .expect("replay subcommand")
            .render_long_help()
            .to_string();
        assert!(help.contains("--allow-write"));
        assert!(help.contains("--target"));
        assert!(help.contains("command mode"));
        assert!(help.contains("COMMAND"));
    }

    #[test]
    fn public_help_lists_exactly_five_commands() {
        let help = Cli::command().render_long_help().to_string();
        for name in ["record", "replay", "list", "inspect", "doctor"] {
            assert!(help.contains(name), "missing {name}");
        }
        assert!(help.contains("Quick start:"));
        assert!(help.contains("chronicle record -- COMMAND..."));
        assert!(help.contains("chronicle inspect latest"));
        assert!(!help.contains("HTTP fixtures"));
        // Legacy mechanics are hidden from public help.
        assert!(!help.contains("recorder-status"));
        assert!(!help.contains("etl"));
        assert!(!help.contains("--wal-dir"));
    }
}
