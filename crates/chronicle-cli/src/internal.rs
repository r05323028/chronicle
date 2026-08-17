use super::render::{EtlJson, RecordJson, render_json};
use super::{AppConfig, ApplicationError, Format, InternalCommand};
use std::path::PathBuf;

use chronicle_application::{
    RecorderConfigV1, RecorderLease, RecorderStatusV1, process_and_publish_recording_wal,
    process_and_publish_recording_wal_with_parent, record_fixture_file,
};

#[cfg(target_os = "linux")]
use super::render::ProductionRecordJson;
#[cfg(target_os = "linux")]
use super::signals::spawn_signal_watcher;
#[cfg(target_os = "linux")]
use chronicle_application::{
    CgroupSelector, ProductionSignalStop, load_recording_metadata, record_continuous_ebpf,
    recording_physical_wal_bytes,
};

#[allow(clippy::too_many_lines)]
pub(super) fn run(
    command: InternalCommand,
    format: Format,
    config: &AppConfig,
    recorder_config_path: Option<PathBuf>,
) -> Result<(String, i32), ApplicationError> {
    match command {
        InternalCommand::RecorderStatus { state_root } => {
            let metadata = chronicle_application::load_recorder_metadata(&state_root)
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            let owner_live = RecorderLease::state_is_owned(&state_root)?;
            let status = RecorderStatusV1::from_metadata(&metadata, owner_live)
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            let output = match format {
                Format::Human => status
                    .render_human()
                    .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?,
                Format::Json => status
                    .render_json()
                    .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?,
            };
            Ok((output, 0))
        }
        InternalCommand::Recorder => {
            let path = recorder_config_path.ok_or(ApplicationError::ProductionPreflight(
                "recorder requires --config FILE",
            ))?;
            let recorder_config = RecorderConfigV1::from_file(path)
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?
                .normalize()
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            #[cfg(target_os = "linux")]
            {
                let wal_dir = recorder_config.state_root.join("wal");
                let stop = ProductionSignalStop::default();
                spawn_signal_watcher(stop.clone(), wal_dir.clone());
                let result = record_continuous_ebpf(
                    CgroupSelector::Explicit(recorder_config.scope.cgroup_path.clone()),
                    recorder_config.scope.shared_scope_acknowledged,
                    &recorder_config,
                    &wal_dir,
                    &stop,
                )?;
                let metadata = load_recording_metadata(&wal_dir)?.ok_or(
                    ApplicationError::RecordingMetadataValidation(
                        "recording metadata missing after finalization".into(),
                    ),
                )?;
                let capture = metadata.capture.as_ref().ok_or(
                    ApplicationError::RecordingMetadataValidation(
                        "capture metadata missing after finalization".into(),
                    ),
                )?;
                let physical_wal_bytes = recording_physical_wal_bytes(&wal_dir)?;
                let output = match format {
                    Format::Human => format!(
                        "recording_id: {}\nstatus: {:?}\nshutdown_reason: {:?}\nphysical_wal_bytes: {}",
                        result.recording_id,
                        result.status,
                        result.shutdown_reason,
                        physical_wal_bytes
                    ),
                    Format::Json => render_json(&ProductionRecordJson {
                        version: 1,
                        recording_id: result.recording_id.to_string(),
                        status: result.status,
                        shutdown_reason: result.shutdown_reason,
                        last_valid_commit: result.last_valid_commit.as_ref(),
                        physical_wal_bytes,
                        selector: metadata.selector.as_ref(),
                        direct_tgid_count: capture.scope.direct_tgid_count,
                        descendant_cgroup_count: capture.scope.descendant_cgroup_count,
                        selected_subtree: capture.scope.selected_subtree,
                        shared_scope_acknowledged: capture.scope.shared_scope_acknowledged,
                        configured_bounds: &capture.configured_bounds,
                        effective_bounds: &capture.effective_bounds,
                        counters: &result.counters,
                    })?,
                };
                Ok((output, 0))
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = recorder_config;
                Err(ApplicationError::ProductionPreflight(
                    "live capture is unavailable on this platform",
                ))
            }
        }
        InternalCommand::Etl { wal_dir, output } => {
            let registry = chronicle_application::protocol_registry()?;
            let result = match chronicle_application::resolve_epoch_parent_context(&wal_dir)? {
                Some((parent_id, epoch_id, epoch_ordinal)) => {
                    process_and_publish_recording_wal_with_parent(
                        &wal_dir,
                        &output,
                        &registry,
                        parent_id,
                        epoch_id,
                        epoch_ordinal,
                    )?
                }
                None => process_and_publish_recording_wal(&wal_dir, &output, &registry)?,
            };
            let rendered = match format {
                Format::Human => format!(
                    "session_id: {}\noutput: {}\nalready_processed: {}\nignored_post_commit_records: {}",
                    result.session_id,
                    output.display(),
                    result.already_published,
                    result.ignored_post_commit_records,
                ),
                Format::Json => render_json(&EtlJson {
                    version: 1,
                    session_id: result.session_id.to_string(),
                    recording_id: result.checkpoint.recording_id.to_string(),
                    status: result.checkpoint.status,
                    shutdown_reason: result.checkpoint.shutdown_reason,
                    output,
                    already_processed: result.already_published,
                    ignored_post_commit_records: result.ignored_post_commit_records,
                    counters: &result.checkpoint.counters,
                    checkpoint: &result.checkpoint,
                })?,
            };
            Ok((rendered, 0))
        }
        InternalCommand::RecordFixture { input, root } => {
            let result = record_fixture_file(input, &root, config.wal.segment_size_bytes)?;
            let output = match format {
                Format::Human => format!(
                    "session_id: {}\nroot: {}",
                    result.session_id,
                    root.display()
                ),
                Format::Json => render_json(&RecordJson {
                    version: 1,
                    session_id: result.session_id.to_string(),
                    root,
                })?,
            };
            Ok((output, 0))
        }
        InternalCommand::Bootstrap { .. } => {
            unreachable!("bootstrap is handled synchronously before the runtime")
        }
    }
}
