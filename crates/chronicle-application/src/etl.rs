//! Recovery, reconciliation, one-shot ETL publication, and checkpoint helpers.

use super::*;

pub fn decode_recording_metadata(bytes: &[u8]) -> Result<RecordingMetadata, ApplicationError> {
    let metadata: RecordingMetadata = serde_json::from_slice(bytes)
        .map_err(|error| ApplicationError::RecordingMetadataValidation(error.to_string()))?;
    validate_recording_metadata(&metadata)?;
    Ok(metadata)
}

pub fn load_recording_metadata(
    wal_directory: impl AsRef<Path>,
) -> Result<Option<RecordingMetadata>, ApplicationError> {
    match fs::read(wal_directory.as_ref().join("recording.json")) {
        Ok(bytes) => decode_recording_metadata(&bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn write_recording_metadata(
    wal_directory: impl AsRef<Path>,
    metadata: &RecordingMetadata,
) -> Result<(), ApplicationError> {
    write_recording_metadata_inner(wal_directory.as_ref(), metadata, None)
}

/// Best-effort persistence for a second signal before forced process exit.
pub fn mark_recording_forced_termination(
    wal_directory: impl AsRef<Path>,
) -> Result<(), ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let Some(mut metadata) = load_recording_metadata(wal_directory)? else {
        return Ok(());
    };
    if matches!(
        metadata.status,
        RecordingStatus::Starting | RecordingStatus::Recording
    ) {
        metadata.status = RecordingStatus::Aborted;
        metadata.shutdown_reason = Some(ShutdownReason::ForcedTermination);
        write_recording_metadata(wal_directory, &metadata)?;
    }
    Ok(())
}

impl RecordingIngestResult {
    /// Applies final ingest evidence before atomically persisting `recording.json`.
    pub fn persist_metadata(
        &self,
        wal_directory: impl AsRef<Path>,
        metadata: &mut RecordingMetadata,
    ) -> Result<(), ApplicationError> {
        metadata.status = self.status;
        metadata.shutdown_reason = Some(self.shutdown_reason);
        metadata.counters = self.counters.clone();
        metadata
            .terminal_wal_loss
            .clone_from(&self.terminal_wal_loss);
        write_recording_metadata(wal_directory, metadata)
    }
}

/// Drives one already-preflighted production source through bounded WAL persistence.
/// `requested_stop` supplies source completion or future signal handling; duration is enforced here.
/// `on_ready` fires once capture is attached and the WAL writer is durable, before the
/// poll loop: command-mode orchestration releases the bootstrap child there.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn record_production<S, Build, Now, Stop, Ready>(
    wal_directory: impl AsRef<Path>,
    metadata: &mut RecordingMetadata,
    capture_metadata: RecordingCaptureMetadata,
    bounds: ProductionRecordingBounds,
    build_source: Build,
    mut now_millis: Now,
    mut requested_stop: Stop,
    on_ready: Ready,
) -> Result<ProductionRecordingResult, ApplicationError>
where
    S: CaptureSource,
    Build: FnOnce() -> Result<S, ApplicationError>,
    Now: FnMut() -> u64,
    Stop: FnMut() -> Option<ShutdownReason>,
    Ready: FnOnce() -> Result<(), ApplicationError>,
{
    validate_production_recording_bounds(bounds)?;
    if metadata.status != RecordingStatus::Starting || metadata.shutdown_reason.is_some() {
        return Err(ApplicationError::RecordingMetadataValidation(
            "production recording must begin in starting state".into(),
        ));
    }

    let wal_directory = wal_directory.as_ref();
    write_recording_metadata(wal_directory, metadata)?;
    let mut source = match build_source() {
        Ok(source) => source,
        Err(error) => {
            persist_pre_attach_failure(
                wal_directory,
                metadata,
                capture_metadata,
                RecordingCaptureErrorCode::Attach,
            )?;
            return Err(error);
        }
    };
    if let Err(error) = source.start() {
        persist_pre_attach_failure(
            wal_directory,
            metadata,
            capture_metadata,
            RecordingCaptureErrorCode::Source,
        )?;
        return Err(error.into());
    }

    metadata.transition_to_recording(capture_metadata)?;
    write_recording_metadata(wal_directory, metadata)?;
    let started_at = now_millis();
    let writer = match GroupCommitWalWriter::create_with_total_limit(
        wal_directory,
        metadata.recording_id,
        bounds.segment_bytes,
        bounds.max_wal_bytes,
        1,
        started_at,
    ) {
        Ok(writer) => writer,
        Err(error) => {
            let _ = shutdown_source_without_wal(&mut source);
            return persist_live_failure(
                wal_directory,
                metadata,
                RecordingCaptureErrorCode::Source,
                ShutdownReason::WalFailure,
                error.into(),
            );
        }
    };
    let mut ingest = RecordingIngest::new(writer);
    // Capture is attached and the WAL writer is durable: release the
    // bootstrap/target now. A failure here (e.g. the readiness pipe died) is a
    // Chronicle launch failure, not a child exit.
    if let Err(error) = on_ready() {
        let _ = shutdown_source_without_wal(&mut source);
        return persist_live_failure(
            wal_directory,
            metadata,
            RecordingCaptureErrorCode::Attach,
            ShutdownReason::CaptureFailure,
            error,
        );
    }
    let mut capture_failed = false;
    let mut wal_failed = false;
    let mut requested_reason = loop {
        let now = now_millis();
        if let Some(reason) = requested_stop().or_else(|| {
            (now.saturating_sub(started_at) >= bounds.duration_seconds.saturating_mul(1_000))
                .then_some(ShutdownReason::DurationLimit)
        }) {
            break reason;
        }
        match source.poll() {
            Ok(Some(event)) => match persist_capture_event(&mut ingest, &event, now) {
                Ok(()) => {
                    if let Some(reason) = ingest.stop_reason() {
                        break reason;
                    }
                }
                Err(error) => match recording_failure_reason(&error) {
                    ShutdownReason::WalFailure => {
                        wal_failed = true;
                        break ShutdownReason::WalFailure;
                    }
                    ShutdownReason::CaptureFailure => {
                        capture_failed = true;
                        break ShutdownReason::CaptureFailure;
                    }
                    _ => unreachable!("recording failures have one of two terminal reasons"),
                },
            },
            Ok(None) => {}
            Err(error) => {
                eprintln!("chronicle: capture source error: {error}");
                capture_failed = true;
                break ShutdownReason::CaptureFailure;
            }
        }
    };

    let source_summary = match stop_and_finalize_source(&mut source, &mut ingest, &mut now_millis) {
        Ok(summary) => summary,
        Err(ApplicationError::Wal(_)) => {
            requested_reason = ShutdownReason::WalFailure;
            wal_failed = true;
            CaptureSourceSummary::default()
        }
        Err(_) => {
            requested_reason = ShutdownReason::CaptureFailure;
            capture_failed = true;
            CaptureSourceSummary::default()
        }
    };
    let mut result = match ingest.finish(requested_reason, now_millis()) {
        Ok(result) => result,
        Err(failure) => {
            wal_failed = true;
            failure.result
        }
    };
    if capture_failed {
        result.status = RecordingStatus::Failed;
        result.shutdown_reason = ShutdownReason::CaptureFailure;
        append_capture_error(metadata, RecordingCaptureErrorCode::Source);
    } else if wal_failed {
        result.status = RecordingStatus::Failed;
        result.shutdown_reason = ShutdownReason::WalFailure;
    }
    metadata.last_valid_commit = ingest.commit_boundary();
    result.persist_metadata(wal_directory, metadata)?;
    Ok(ProductionRecordingResult {
        recording_id: metadata.recording_id,
        status: result.status,
        shutdown_reason: result.shutdown_reason,
        last_valid_commit: metadata.last_valid_commit.clone(),
        counters: result.counters,
        terminal_wal_loss: result.terminal_wal_loss,
        source_summary,
    })
}

/// Counts every physical byte below the WAL segments directory.
pub fn recording_physical_wal_bytes(
    wal_directory: impl AsRef<Path>,
) -> Result<u64, ApplicationError> {
    fn count(path: &Path) -> Result<u64, ApplicationError> {
        fs::read_dir(path)?.try_fold(0_u64, |total, entry| {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let bytes = if metadata.is_dir() {
                count(&entry.path())?
            } else {
                metadata.len()
            };
            total.checked_add(bytes).ok_or_else(|| {
                ApplicationError::RecordingMetadataValidation(
                    "physical WAL size exceeds u64".into(),
                )
            })
        })
    }

    let segments = wal_directory.as_ref().join("segments");
    if !segments.is_dir() {
        return Ok(0);
    }
    count(&segments)
}

/// Builds the Linux eBPF source after `recording.json` reached `starting`.
/// PID baselines are rechecked after links attach; source construction drops links on failure.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub fn load_production_ebpf_source(
    selection: &CgroupSelection,
    pid_baseline: Option<&PidCgroupSelection>,
) -> Result<chronicle_capture_ebpf::EbpfCaptureSource, ApplicationError> {
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned();
    if boot_id.is_empty() {
        return Err(ApplicationError::ProductionPreflight(
            "boot clock identity unavailable",
        ));
    }
    let adapter = chronicle_capture_ebpf::CaptureAdapter::new(
        ClockIdentity { boot_id },
        chronicle_capture_ebpf::RecordingScopeConfig {
            identity: chronicle_capture::RecordingScopeIdentity {
                cgroup_id: selection.cgroup_id,
                canonical_path: selection.canonical_path.display().to_string(),
                namespace: None,
            },
            descendant_cgroup_ids: selection.descendant_cgroup_ids.clone(),
        },
        0,
    )?;
    let cgroup = File::open(&selection.canonical_path)?;
    chronicle_capture_ebpf::EbpfCaptureSource::load_embedded_with_post_attach(
        &cgroup,
        adapter,
        || {
            if let Some(baseline) = pid_baseline {
                baseline.revalidate().map_err(|_| {
                    chronicle_capture_ebpf::EbpfCaptureError::MissingIdentity(
                        "PID cgroup scope changed after attach",
                    )
                })?;
            }
            Ok(())
        },
    )
    .map_err(ApplicationError::from)
}

pub(crate) fn persist_pre_attach_failure(
    wal_directory: &Path,
    metadata: &mut RecordingMetadata,
    mut capture_metadata: RecordingCaptureMetadata,
    error: RecordingCaptureErrorCode,
) -> Result<(), ApplicationError> {
    if capture_metadata.errors.len() < MAX_RECORDING_CAPTURE_ERRORS {
        capture_metadata
            .errors
            .push(RecordingCaptureError { code: error });
    }
    metadata.capture = Some(capture_metadata);
    metadata.fail_start(ShutdownReason::CaptureFailure)?;
    write_recording_metadata(wal_directory, metadata)
}

pub(crate) fn persist_live_failure(
    wal_directory: &Path,
    metadata: &mut RecordingMetadata,
    error: RecordingCaptureErrorCode,
    reason: ShutdownReason,
    original: ApplicationError,
) -> Result<ProductionRecordingResult, ApplicationError> {
    if reason == ShutdownReason::CaptureFailure {
        append_capture_error(metadata, error);
    }
    metadata.finalize(RecordingStatus::Failed, reason)?;
    write_recording_metadata(wal_directory, metadata)?;
    Err(original)
}

pub(crate) fn recording_failure_reason(error: &ApplicationError) -> ShutdownReason {
    if matches!(error, ApplicationError::Wal(_)) {
        ShutdownReason::WalFailure
    } else {
        ShutdownReason::CaptureFailure
    }
}

fn append_capture_error(metadata: &mut RecordingMetadata, error: RecordingCaptureErrorCode) {
    if let Some(capture) = &mut metadata.capture
        && capture.errors.len() < MAX_RECORDING_CAPTURE_ERRORS
    {
        capture.errors.push(RecordingCaptureError { code: error });
    }
}

fn persist_capture_event(
    ingest: &mut RecordingIngest,
    event: &CaptureEvent,
    now_millis: u64,
) -> Result<(), ApplicationError> {
    let (kind, capture_timestamp, kernel_drops) = match &event.kind {
        CaptureEventKind::LossWindowObserved(window) => (
            RecordKind::LossWindow,
            Some(window.end.clone()),
            window.drop_delta,
        ),
        CaptureEventKind::SocketConnectObserved(intent) => (
            RecordKind::CaptureEvent,
            Some(intent.timestamp.clone()),
            None,
        ),
        CaptureEventKind::SocketConnected(evidence)
        | CaptureEventKind::SocketClosedObserved(evidence)
        | CaptureEventKind::SocketResetObserved(evidence) => (
            RecordKind::CaptureEvent,
            Some(evidence.timestamp.clone()),
            None,
        ),
        CaptureEventKind::SocketStateChangedObserved(observed) => (
            RecordKind::CaptureEvent,
            Some(observed.socket.timestamp.clone()),
            None,
        ),
        CaptureEventKind::PayloadFragment(fragment) => (
            RecordKind::CaptureEvent,
            Some(fragment.timestamp.clone()),
            None,
        ),
    };
    if let Some(drops) = kernel_drops {
        ingest.record_kernel_or_backend_drop(drops, 0);
    }
    let mut record = QueuedWalRecord {
        kind,
        schema_version: event.schema_version,
        flags: match &event.kind {
            CaptureEventKind::PayloadFragment(fragment) => u16::try_from(fragment.flags.0)
                .map_err(|_| ApplicationError::CaptureFlagsOutOfRange(fragment.flags.0))?,
            _ => 0,
        },
        payload: encode_event(event)?,
        capture_timestamp,
    };
    loop {
        match ingest.admit(record) {
            Ok(IngestAdmission::Accepted) => {
                ingest.drain(now_millis)?;
                return Ok(());
            }
            Ok(IngestAdmission::RejectedAfterStop) => return Ok(()),
            Err(backpressured) => {
                record = backpressured;
                ingest.drain(now_millis)?;
                if ingest.stop_reason().is_some() {
                    return Ok(());
                }
            }
        }
    }
}

fn stop_and_finalize_source<Now>(
    source: &mut impl CaptureSource,
    ingest: &mut RecordingIngest,
    now_millis: &mut Now,
) -> Result<CaptureSourceSummary, ApplicationError>
where
    Now: FnMut() -> u64,
{
    let deadline = now_millis().saturating_add(RECORDING_FINALIZATION_GRACE_MILLIS);
    let mut failure = source.request_shutdown().err().map(ApplicationError::from);
    loop {
        if now_millis() > deadline {
            failure.get_or_insert(CaptureError::Drain("finalization grace exceeded".into()).into());
            break;
        }
        match source.drain() {
            Ok(Some(event)) => {
                if let Err(error) = persist_capture_event(ingest, &event, now_millis()) {
                    failure.get_or_insert(error);
                }
            }
            Ok(None) => break,
            Err(error) => {
                failure.get_or_insert(error.into());
                break;
            }
        }
    }
    let summary = source.finalize();
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(summary?)
}

fn shutdown_source_without_wal(source: &mut impl CaptureSource) -> Result<(), CaptureError> {
    let mut failure = source.request_shutdown().err();
    loop {
        match source.drain() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(error) => {
                failure.get_or_insert(error);
                break;
            }
        }
    }
    let finalized = source.finalize();
    if let Some(error) = failure {
        return Err(error);
    }
    finalized.map(|_| ())
}

pub(crate) fn validate_recording_metadata(
    metadata: &RecordingMetadata,
) -> Result<(), ApplicationError> {
    if metadata.version != RECORDING_METADATA_SCHEMA_VERSION {
        return Err(ApplicationError::RecordingMetadataValidation(format!(
            "unsupported recording metadata version {}",
            metadata.version
        )));
    }
    let terminal = matches!(
        metadata.status,
        RecordingStatus::Completed | RecordingStatus::Failed | RecordingStatus::Aborted
    );
    if terminal != metadata.shutdown_reason.is_some() {
        return Err(ApplicationError::RecordingMetadataValidation(
            "terminal status and shutdown reason must be present together".into(),
        ));
    }
    if let Some(capture) = &metadata.capture {
        if capture.version != RECORDING_CAPTURE_METADATA_SCHEMA_VERSION {
            return Err(ApplicationError::RecordingMetadataValidation(format!(
                "unsupported recording capture metadata version {}",
                capture.version
            )));
        }
        if [
            &capture.build.chronicle_version,
            &capture.build.aya_version,
            &capture.build.aya_ebpf_version,
            &capture.build.ebpf_object_sha256,
            &capture.host.kernel_release,
            &capture.host.architecture,
            &capture.host.boot_id,
        ]
        .iter()
        .any(|value| value.is_empty())
        {
            return Err(ApplicationError::RecordingMetadataValidation(
                "recording capture identity fields must be non-empty".into(),
            ));
        }
        if !capture.scope.selected_subtree {
            return Err(ApplicationError::RecordingMetadataValidation(
                "recording capture scope must be selected subtree".into(),
            ));
        }
        if capture.capabilities.len() > MAX_RECORDING_CAPABILITIES
            || capture.errors.len() > MAX_RECORDING_CAPTURE_ERRORS
        {
            return Err(ApplicationError::RecordingMetadataValidation(
                "recording capture metadata exceeds bounded fields".into(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn write_recording_metadata_inner(
    wal_directory: &Path,
    metadata: &RecordingMetadata,
    fault: Option<RecordingMetadataWriteFault>,
) -> Result<(), ApplicationError> {
    validate_recording_metadata(metadata)?;
    write_private_atomic_json(wal_directory, "recording.json", metadata, fault)
}

pub(crate) fn write_private_atomic_json<T: Serialize + ?Sized>(
    directory: &Path,
    file_name: &str,
    value: &T,
    #[cfg_attr(not(test), allow(unused_variables))] fault: Option<RecordingMetadataWriteFault>,
) -> Result<(), ApplicationError> {
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    return Err(ApplicationError::RecordingMetadataValidation(
        "private atomic JSON persistence is unsupported on this platform".into(),
    ));

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        fs::create_dir_all(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let final_path = directory.join(file_name);
        let temp_path = directory.join(format!(".{file_name}.tmp-{}", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec(value)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options.open(&temp_path)?;
        let mut published = false;
        let result = (|| -> Result<(), ApplicationError> {
            file.write_all(&bytes)?;
            #[cfg(test)]
            if fault == Some(RecordingMetadataWriteFault::BeforeFileSync) {
                return Err(std::io::Error::other("injected atomic JSON failure").into());
            }
            file.sync_all()?;
            #[cfg(test)]
            if fault == Some(RecordingMetadataWriteFault::BeforeRename) {
                return Err(std::io::Error::other("injected atomic JSON failure").into());
            }
            fs::rename(&temp_path, &final_path)?;
            published = true;
            #[cfg(test)]
            if fault == Some(RecordingMetadataWriteFault::AfterRename) {
                return Err(std::io::Error::other("injected atomic JSON failure").into());
            }
            File::open(directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() && !published {
            let _ = fs::remove_file(temp_path);
        }
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingRecoveryReconciliation {
    pub metadata: RecordingMetadata,
    pub scan: RecoveryScan,
    pub metadata_updated: bool,
}

pub fn reconcile_recording_metadata(
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    expected_selector: Option<&RecordingSelectorIdentity>,
) -> Result<RecordingRecoveryReconciliation, ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let lock = RecordingLock::acquire(wal_directory)?;
    let scan = lock.scan(wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES)?;
    reconcile_recording_metadata_with_scan(
        &lock,
        wal_directory,
        recording_id,
        expected_selector,
        scan,
    )
}

pub fn reconcile_recording_metadata_with_scan(
    _lock: &RecordingLock,
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    expected_selector: Option<&RecordingSelectorIdentity>,
    scan: RecoveryScan,
) -> Result<RecordingRecoveryReconciliation, ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let original = load_recording_metadata(wal_directory)?;
    let mut metadata = original.clone().unwrap_or_else(|| RecordingMetadata {
        version: RECORDING_METADATA_SCHEMA_VERSION,
        recording_id,
        selector: expected_selector.cloned(),
        status: RecordingStatus::Aborted,
        shutdown_reason: Some(ShutdownReason::ProcessCrashRecovered),
        last_valid_commit: None,
        counters: RecordingCounters::default(),
        terminal_wal_loss: None,
        capture: None,
    });
    if metadata.recording_id != recording_id {
        return Err(ApplicationError::RecordingMetadataValidation(
            "recording ID does not match WAL segment identity".into(),
        ));
    }
    if let Some(expected) = expected_selector
        && metadata.selector.as_ref() != Some(expected)
    {
        return Err(ApplicationError::RecordingMetadataValidation(
            "recording selector does not match expected immutable selector".into(),
        ));
    }
    if matches!(
        metadata.status,
        RecordingStatus::Starting | RecordingStatus::Recording
    ) {
        metadata.status = RecordingStatus::Aborted;
        metadata.shutdown_reason = Some(ShutdownReason::ProcessCrashRecovered);
    }
    metadata.last_valid_commit = match (
        scan.authority.marker_sequence,
        scan.authority.durable_through_sequence,
        scan.authority.segment_ordinal,
    ) {
        (Some(marker_sequence), Some(durable_through_sequence), Some(segment_ordinal)) => {
            Some(RecordingCommitBoundary {
                marker_sequence,
                durable_through_sequence,
                durable_record_count: scan.authority.durable_record_count,
                durable_payload_bytes: scan.authority.durable_payload_bytes,
                segment_ordinal,
            })
        }
        _ => None,
    };
    metadata.counters.committed = RecordByteCount {
        records: scan.authority.durable_record_count,
        bytes: scan.authority.durable_payload_bytes,
    };
    metadata.counters.written_not_committed = RecordByteCount {
        records: u64::try_from(scan.uncommitted.len()).map_err(|_| {
            ApplicationError::RecordingMetadataValidation(
                "uncommitted record count exceeds metadata range".into(),
            )
        })?,
        bytes: scan.uncommitted.iter().try_fold(0_u64, |total, envelope| {
            total
                .checked_add(u64::try_from(envelope.payload.len()).map_err(|_| {
                    ApplicationError::RecordingMetadataValidation(
                        "uncommitted payload bytes exceed metadata range".into(),
                    )
                })?)
                .ok_or_else(|| {
                    ApplicationError::RecordingMetadataValidation(
                        "uncommitted payload bytes exceed metadata range".into(),
                    )
                })
        })?,
    };
    let metadata_updated = original.as_ref() != Some(&metadata);
    if metadata_updated {
        write_recording_metadata(wal_directory, &metadata)?;
    }
    Ok(RecordingRecoveryReconciliation {
        metadata,
        scan,
        metadata_updated,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryIssueCode {
    TailRepaired,
    PartialTail,
    FrameCrc,
    FrameVersion,
    HeaderCorruption,
    MarkerCrc,
    MarkerVersion,
    MarkerReference,
    MarkerDigest,
    Sequence,
    Identity,
    WalLimitLoss,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryTailRepairSummary {
    pub repaired_bytes: u64,
    pub truncated_to: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostMarkerUncertainty {
    pub records: u64,
    pub bytes: u64,
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub partial_tail_offset: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub version: u16,
    pub recording_id: RecordingId,
    pub status: RecordingStatus,
    pub shutdown_reason: Option<ShutdownReason>,
    pub repaired_tail: Option<RecoveryTailRepairSummary>,
    pub removed_segments: usize,
    pub last_valid_commit: Option<RecordingCommitBoundary>,
    pub committed: RecordByteCount,
    pub post_marker: PostMarkerUncertainty,
    pub wal_limit_loss: RecordByteCount,
    pub issue_codes: Vec<RecoveryIssueCode>,
}

pub fn recovery_issue_code(error: &WalError) -> RecoveryIssueCode {
    match error {
        WalError::EnvelopeChecksum {
            kind: RecordKind::CommitMarker,
            ..
        } => RecoveryIssueCode::MarkerCrc,
        WalError::EnvelopeChecksum { .. } | WalError::Checksum { .. } => {
            RecoveryIssueCode::FrameCrc
        }
        WalError::SegmentHeaderChecksum { .. }
        | WalError::InvalidSegmentMagic
        | WalError::InvalidSegmentHeaderLength(_)
        | WalError::NonzeroSegmentHeaderReserved
        | WalError::TruncatedSegmentHeader { .. }
        | WalError::UnsupportedSegmentHeaderVersion(_) => RecoveryIssueCode::HeaderCorruption,
        WalError::UnsupportedCommitMarkerVersion(_) => RecoveryIssueCode::MarkerVersion,
        WalError::UnsupportedEnvelopeVersion { kind, .. }
            if *kind == RecordKind::CommitMarker as u16 =>
        {
            RecoveryIssueCode::MarkerVersion
        }
        WalError::UnsupportedEnvelopeVersion { .. } | WalError::UnsupportedVersion(_) => {
            RecoveryIssueCode::FrameVersion
        }
        WalError::CommitDigestMismatch => RecoveryIssueCode::MarkerDigest,
        WalError::CommitBoundaryMismatch { .. }
        | WalError::CommitDurableThroughMismatch { .. }
        | WalError::CommitRecordCountMismatch { .. }
        | WalError::CommitPayloadBytesMismatch { .. }
        | WalError::CommitBatchContainsMarker { .. }
        | WalError::EmptyCommitBatch => RecoveryIssueCode::MarkerReference,
        WalError::UnexpectedSequence { .. }
        | WalError::OutOfOrder { .. }
        | WalError::RecoverySegmentFirstSequenceMismatch { .. } => RecoveryIssueCode::Sequence,
        WalError::CommitRecordingIdMismatch
        | WalError::CommitSegmentMismatch
        | WalError::SegmentIdentityMismatch(_)
        | WalError::EnvelopeRecordingIdMismatch => RecoveryIssueCode::Identity,
        _ => RecoveryIssueCode::Other,
    }
}

pub fn build_recovery_report(
    reconciliation: &RecordingRecoveryReconciliation,
    repair: Option<&RecoveryRepair>,
    additional_issues: impl IntoIterator<Item = RecoveryIssueCode>,
) -> Result<RecoveryReport, ApplicationError> {
    let uncommitted_bytes =
        reconciliation
            .scan
            .uncommitted
            .iter()
            .try_fold(0_u64, |total, envelope| {
                total
                    .checked_add(u64::try_from(envelope.payload.len()).map_err(|_| {
                        ApplicationError::RecordingMetadataValidation(
                            "post-marker payload bytes exceed report range".into(),
                        )
                    })?)
                    .ok_or_else(|| {
                        ApplicationError::RecordingMetadataValidation(
                            "post-marker payload bytes exceed report range".into(),
                        )
                    })
            })?;
    let mut issue_codes: BTreeSet<_> = additional_issues.into_iter().collect();
    if repair.is_some_and(|repair| repair.repaired_bytes > 0) {
        issue_codes.insert(RecoveryIssueCode::TailRepaired);
    }
    if reconciliation.scan.partial_tail.is_some() {
        issue_codes.insert(RecoveryIssueCode::PartialTail);
    }
    if reconciliation
        .metadata
        .counters
        .discarded_from_queue_due_to_wal_limit
        .records
        > 0
    {
        issue_codes.insert(RecoveryIssueCode::WalLimitLoss);
    }
    Ok(RecoveryReport {
        version: RECOVERY_REPORT_VERSION,
        recording_id: reconciliation.metadata.recording_id,
        status: reconciliation.metadata.status,
        shutdown_reason: reconciliation.metadata.shutdown_reason,
        repaired_tail: repair.map(|repair| RecoveryTailRepairSummary {
            repaired_bytes: repair.repaired_bytes,
            truncated_to: repair.truncated_to,
        }),
        removed_segments: 0,
        last_valid_commit: reconciliation.metadata.last_valid_commit.clone(),
        committed: reconciliation.metadata.counters.committed.clone(),
        post_marker: PostMarkerUncertainty {
            records: u64::try_from(reconciliation.scan.uncommitted.len()).map_err(|_| {
                ApplicationError::RecordingMetadataValidation(
                    "post-marker record count exceeds report range".into(),
                )
            })?,
            bytes: uncommitted_bytes,
            first_sequence: reconciliation
                .scan
                .uncommitted
                .first()
                .map(|envelope| envelope.sequence),
            last_sequence: reconciliation
                .scan
                .uncommitted
                .last()
                .map(|envelope| envelope.sequence),
            partial_tail_offset: reconciliation
                .scan
                .partial_tail
                .as_ref()
                .map(|partial| partial.byte_offset),
        },
        wal_limit_loss: reconciliation
            .metadata
            .counters
            .discarded_from_queue_due_to_wal_limit
            .clone(),
        issue_codes: issue_codes.into_iter().collect(),
    })
}

pub fn build_recovery_report_for_reopen(
    reconciliation: &RecordingRecoveryReconciliation,
    preview: &RecoveryReopenPreview,
    additional_issues: impl IntoIterator<Item = RecoveryIssueCode>,
) -> Result<RecoveryReport, ApplicationError> {
    let mut report = build_recovery_report(reconciliation, None, additional_issues)?;
    if preview.truncated_bytes > 0 || preview.removed_segments > 0 {
        report.repaired_tail = Some(RecoveryTailRepairSummary {
            repaired_bytes: preview.truncated_bytes,
            truncated_to: preview.truncated_to,
        });
        if !report
            .issue_codes
            .contains(&RecoveryIssueCode::TailRepaired)
        {
            report.issue_codes.push(RecoveryIssueCode::TailRepaired);
            report.issue_codes.sort_unstable();
        }
    }
    report.removed_segments = preview.removed_segments;
    Ok(report)
}

pub fn render_recovery_report_json(report: &RecoveryReport) -> Result<String, ApplicationError> {
    render_json(report)
}

pub fn persist_recovery_report(
    wal_directory: impl AsRef<Path>,
    report: &RecoveryReport,
) -> Result<(), ApplicationError> {
    if report.version != RECOVERY_REPORT_VERSION {
        return Err(ApplicationError::RecordingMetadataValidation(format!(
            "unsupported recovery report version {}",
            report.version
        )));
    }
    write_private_atomic_json(
        &wal_directory.as_ref().join("etl"),
        "recovery-report.json",
        report,
        None,
    )
}

pub fn decode_recovery_report(bytes: &[u8]) -> Result<RecoveryReport, ApplicationError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            ApplicationError::RecordingMetadataValidation(
                "recovery report version is missing or invalid".into(),
            )
        })?;
    if version != u64::from(RECOVERY_REPORT_VERSION) {
        return Err(ApplicationError::RecordingMetadataValidation(format!(
            "unsupported recovery report version {version}"
        )));
    }
    serde_json::from_value(value).map_err(ApplicationError::JsonSerialization)
}

pub struct RecoveredRecordingForAppend {
    pub writer: GroupCommitWalWriter,
    pub reconciliation: RecordingRecoveryReconciliation,
    pub recovery_report: RecoveryReport,
    pub reopen_report: RecoveryReopenReport,
}

#[allow(clippy::too_many_arguments)]
pub fn recover_recording_for_append(
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    expected_selector: Option<&RecordingSelectorIdentity>,
    max_segment_bytes: u64,
    max_total_bytes: u64,
    now_millis: u64,
) -> Result<RecoveredRecordingForAppend, ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let lock = RecordingLock::acquire(wal_directory)?;
    let scan = match lock.scan(wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES) {
        Ok(scan) => scan,
        Err(error) => {
            let failure_report = RecoveryReport {
                version: RECOVERY_REPORT_VERSION,
                recording_id,
                status: RecordingStatus::Failed,
                shutdown_reason: Some(ShutdownReason::WalFailure),
                repaired_tail: None,
                removed_segments: 0,
                last_valid_commit: None,
                committed: RecordByteCount::default(),
                post_marker: PostMarkerUncertainty::default(),
                wal_limit_loss: RecordByteCount::default(),
                issue_codes: vec![recovery_issue_code(&error)],
            };
            persist_recovery_report(wal_directory, &failure_report)?;
            return Err(error.into());
        }
    };
    let prepared = prepare_group_commit_reopen_from_scan(
        lock,
        wal_directory,
        recording_id,
        scan,
        max_segment_bytes,
        max_total_bytes,
        now_millis,
    )?;
    let before_reconciliation = reconcile_recording_metadata_with_scan(
        prepared.lock(),
        wal_directory,
        recording_id,
        expected_selector,
        prepared.preview().scan_before.clone(),
    )?;
    let recovery_report = build_recovery_report_for_reopen(
        &before_reconciliation,
        prepared.preview(),
        std::iter::empty(),
    )?;
    persist_recovery_report(wal_directory, &recovery_report)?;
    let (writer, reopen_report) = prepared.apply()?;
    let reconciliation = reconcile_recording_metadata_with_scan(
        writer.recording_lock(),
        wal_directory,
        recording_id,
        expected_selector,
        reopen_report.scan_after.clone(),
    )?;
    Ok(RecoveredRecordingForAppend {
        writer,
        reconciliation,
        recovery_report,
        reopen_report,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedWal {
    pub recording_id: RecordingId,
    pub first_sequence: Option<u64>,
    pub checkpoint: Option<WalCheckpoint>,
    pub record_count: usize,
}

pub fn write_capture_to_wal(
    source: &mut impl CaptureSource,
    directory: impl AsRef<Path>,
    max_segment_bytes: u64,
) -> Result<RecordedWal, ApplicationError> {
    let directory = directory.as_ref();
    if directory.exists() {
        return Err(ApplicationError::WalDestinationExists(
            directory.to_path_buf(),
        ));
    }

    let recording_id = RecordingId::new();
    let mut records = Vec::new();
    let mut encoded_bytes =
        u64::try_from(SEGMENT_HEADER_LEN + COMMIT_MARKER_FRAME_LEN).unwrap_or(u64::MAX);
    while let Some(event) = source.next_event()? {
        let flags = match &event.kind {
            CaptureEventKind::PayloadFragment(fragment) => u16::try_from(fragment.flags.0)
                .map_err(|_| ApplicationError::CaptureFlagsOutOfRange(fragment.flags.0))?,
            _ => 0,
        };
        let kind = if matches!(event.kind, CaptureEventKind::LossWindowObserved(_)) {
            RecordKind::LossWindow
        } else {
            RecordKind::CaptureEvent
        };
        let payload = encode_event(&event)?;
        let sequence = u64::try_from(records.len() + 1).unwrap_or(u64::MAX);
        encoded_bytes = encoded_bytes.saturating_add(
            u64::try_from(
                encode_envelope(&WalRecordEnvelope::unplaced(
                    recording_id,
                    sequence,
                    kind,
                    CAPTURE_EVENT_SCHEMA_VERSION,
                    flags,
                    payload.clone(),
                ))?
                .len(),
            )
            .unwrap_or(u64::MAX),
        );
        records.push((kind, flags, payload));
    }
    if encoded_bytes > max_segment_bytes {
        return Err(ApplicationError::FixtureWalTooLarge {
            bytes: encoded_bytes,
            limit: max_segment_bytes,
        });
    }

    let mut writer = GroupCommitWalWriter::create(
        directory,
        recording_id,
        max_segment_bytes.max(MIN_SEGMENT_BYTES),
        1,
        0,
    )?;
    for (kind, flags, payload) in records.iter().cloned() {
        writer.append(kind, CAPTURE_EVENT_SCHEMA_VERSION, flags, payload, 0)?;
    }
    writer.flush(0)?;
    drop(writer);
    let scan = chronicle_wal::scan_wal(directory, recording_id, DEFAULT_MAX_RECORD_BYTES)?;
    let checkpoint = if let Some(marker) = scan.final_marker.as_ref() {
        let encoded_len = u64::try_from(encode_envelope(marker)?.len()).unwrap_or(u64::MAX);
        marker.provenance.as_ref().map(|provenance| WalCheckpoint {
            segment_first_sequence: provenance.segment_first_sequence,
            byte_offset: provenance.byte_offset.saturating_add(encoded_len),
            next_sequence: marker.sequence.saturating_add(1),
        })
    } else {
        None
    };
    Ok(RecordedWal {
        recording_id,
        first_sequence: (!records.is_empty()).then_some(1),
        checkpoint,
        record_count: records.len(),
    })
}

pub fn process_fixture_wal(
    directory: impl AsRef<Path>,
    recording_id: RecordingId,
    registry: &ProtocolRegistry,
    session_id: chronicle_common::SessionId,
) -> Result<(EtlOutput, WalCheckpoint), ApplicationError> {
    let scan = chronicle_wal::scan_wal(directory, recording_id, DEFAULT_MAX_RECORD_BYTES)?;
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
    let checkpoint = WalCheckpoint {
        segment_first_sequence: provenance.segment_first_sequence,
        byte_offset: provenance.byte_offset
            + u64::try_from(encode_envelope(marker)?.len()).unwrap_or(u64::MAX),
        next_sequence: marker.sequence.saturating_add(1),
    };
    let output = EtlPipeline::new(SessionLimits::default()).process_envelopes(
        &scan.committed,
        registry,
        session_id,
    )?;
    Ok((output, checkpoint))
}

#[derive(Clone, Debug)]
pub struct RecordingEtlResult {
    pub output: EtlOutput,
    /// Complete frames beyond final marker are visible but never canonicalized.
    pub ignored_post_commit_records: u64,
    pub recording_id: RecordingId,
    pub status: RecordingStatus,
    pub shutdown_reason: Option<ShutdownReason>,
    pub counters: RecordingCounters,
    pub commit_boundary: RecordingCommitBoundary,
    pub commit_marker_byte_offset: u64,
    pub wal_snapshot_sha256: [u8; 32],
    pub recovery_sha256: [u8; 32],
}

/// ETL one finalized production recording while holding its exclusive WAL lock.
#[allow(clippy::too_many_lines)] // Validation and provenance share one recovery-authoritative boundary.
pub fn process_recording_wal(
    wal_directory: impl AsRef<Path>,
    registry: &ProtocolRegistry,
    session_id: chronicle_common::SessionId,
) -> Result<RecordingEtlResult, ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let lock = RecordingLock::acquire(wal_directory)?;
    let mut metadata = load_recording_metadata(wal_directory)?.ok_or_else(|| {
        ApplicationError::RecordingMetadataValidation("recording metadata is missing".into())
    })?;
    let mut scan = lock.scan(
        wal_directory,
        metadata.recording_id,
        DEFAULT_MAX_RECORD_BYTES,
    )?;
    if matches!(
        metadata.status,
        RecordingStatus::Starting | RecordingStatus::Recording
    ) {
        let reconciliation = reconcile_recording_metadata_with_scan(
            &lock,
            wal_directory,
            metadata.recording_id,
            None,
            scan,
        )?;
        let recovery_report = build_recovery_report(&reconciliation, None, std::iter::empty())?;
        persist_recovery_report(wal_directory, &recovery_report)?;
        metadata = reconciliation.metadata;
        scan = reconciliation.scan;
    }
    let source_status = match metadata.status {
        RecordingStatus::Completed => SourceStatus::Completed,
        RecordingStatus::Failed => SourceStatus::Failed,
        RecordingStatus::Aborted => SourceStatus::Aborted,
        RecordingStatus::Starting | RecordingStatus::Recording => unreachable!(),
    };
    let Some(boundary) = metadata.last_valid_commit.as_ref() else {
        return Err(ApplicationError::RecordingMetadataValidation(
            "finalized recording is missing commit boundary".into(),
        ));
    };
    if scan.authority.marker_sequence != Some(boundary.marker_sequence)
        || scan.authority.durable_through_sequence != Some(boundary.durable_through_sequence)
        || scan.authority.durable_record_count != boundary.durable_record_count
        || scan.authority.durable_payload_bytes != boundary.durable_payload_bytes
        || scan.authority.segment_ordinal != Some(boundary.segment_ordinal)
    {
        return Err(ApplicationError::RecordingMetadataValidation(
            "recording commit boundary does not match recovered WAL".into(),
        ));
    }

    let marker_provenance = scan
        .final_marker
        .as_ref()
        .and_then(|marker| marker.provenance.as_ref())
        .ok_or_else(|| {
            ApplicationError::RecordingMetadataValidation(
                "recovered commit marker is missing segment provenance".into(),
            )
        })?;
    let metadata_terminal_losses =
        metadata_terminal_wal_losses(metadata.terminal_wal_loss.as_ref());
    let wal_envelope_range = scan
        .committed
        .first()
        .zip(scan.committed.last())
        .map(|(first, last)| (first.sequence, last.sequence));
    let mut output = EtlPipeline::new(SessionLimits::default())
        .process_envelopes_with_terminal_losses(
            &scan.committed,
            &metadata_terminal_losses,
            registry,
            session_id,
        )?;
    let wal_snapshot_sha256 = verified_snapshot_sha256(wal_directory, &scan)?;
    let recovery_sha256 = recovery_sha256(&scan);
    assign_recording_ids_with_snapshot(&mut output.session, wal_snapshot_sha256)?;
    let connection_evidence: Vec<_> = output
        .session
        .connections
        .iter()
        .filter_map(|connection| {
            let mut sequences = connection
                .operations
                .iter()
                .map(|operation| operation.sequence);
            let first = sequences.next()?;
            let (first, last) = sequences.fold((first, first), |(first, last), sequence| {
                (first.min(sequence), last.max(sequence))
            });
            Some(ProvenanceEntry {
                kind: ProvenanceKind::Connection,
                sequence_range: Some((first, last)),
                reason: None,
            })
        })
        .collect();
    let mut source_evidence: Vec<_> = wal_envelope_range
        .map(|sequence_range| ProvenanceEntry {
            kind: ProvenanceKind::WalEnvelope,
            sequence_range: Some(sequence_range),
            reason: None,
        })
        .into_iter()
        .chain(
            output
                .evidence
                .commit_marker_sequences
                .iter()
                .map(|&sequence| ProvenanceEntry {
                    kind: ProvenanceKind::CommitMarker,
                    sequence_range: Some((sequence, sequence)),
                    reason: None,
                }),
        )
        .chain(connection_evidence)
        .chain(
            output
                .evidence
                .loss_windows
                .iter()
                .map(|loss| ProvenanceEntry {
                    kind: ProvenanceKind::RingLoss,
                    sequence_range: loss.sequence.map(|sequence| (sequence, sequence)),
                    reason: loss.value.ambiguity.reason.clone(),
                }),
        )
        .chain(
            output
                .evidence
                .terminal_wal_losses
                .iter()
                .map(|loss| ProvenanceEntry {
                    kind: ProvenanceKind::WalLimitLoss,
                    sequence_range: loss.sequence.map(|sequence| (sequence, sequence)),
                    reason: Some("wal_hard_limit".into()),
                }),
        )
        .collect();
    source_evidence.sort_by_key(|entry| entry.sequence_range);
    output.session.source_provenance = SourceProvenance {
        recording_id: Some(metadata.recording_id),
        status: source_status,
        reason: metadata
            .shutdown_reason
            .map(shutdown_reason_name)
            .map(str::to_owned),
        commit_marker: Some(CommitMarkerProvenance {
            segment_ordinal: boundary.segment_ordinal,
            byte_offset: marker_provenance.byte_offset,
            sequence: boundary.marker_sequence,
        }),
        wal_snapshot_sha256: Some(sha256_string(&wal_snapshot_sha256)),
        pipeline_version: Some(ETL_PIPELINE_VERSION.into()),
        evidence: source_evidence,
    };
    Ok(RecordingEtlResult {
        output,
        ignored_post_commit_records: u64::try_from(scan.uncommitted.len()).map_err(|_| {
            ApplicationError::RecordingMetadataValidation(
                "post-commit record count exceeds metadata range".into(),
            )
        })?,
        recording_id: metadata.recording_id,
        status: metadata.status,
        shutdown_reason: metadata.shutdown_reason,
        counters: metadata.counters,
        commit_boundary: boundary.clone(),
        commit_marker_byte_offset: marker_provenance.byte_offset,
        wal_snapshot_sha256,
        recovery_sha256,
    })
}

pub const RECORDING_ETL_CHECKPOINT_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingEtlCheckpoint {
    pub version: u16,
    pub recording_id: RecordingId,
    pub commit_marker_segment_ordinal: u64,
    pub commit_marker_byte_offset: u64,
    pub commit_marker_sequence: u64,
    pub wal_snapshot_sha256: String,
    pub recovery_sha256: String,
    pub wal_format_version: u16,
    pub pipeline_version: String,
    pub canonical_schema_version: u16,
    pub session_id: chronicle_common::SessionId,
    pub manifest_checksum: String,
    pub output_root: String,
    pub output_identity: String,
    pub status: RecordingStatus,
    pub shutdown_reason: Option<ShutdownReason>,
    pub counters: RecordingCounters,
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub(crate) fn has_published_wal(wal_directory: &Path) -> Result<bool, ApplicationError> {
    let segments = wal_directory.join("segments");
    if !segments.exists() {
        return Ok(false);
    }
    Ok(fs::read_dir(segments)
        .map_err(ApplicationError::Io)?
        .filter_map(Result::ok)
        .any(|entry| entry.path().extension().is_some_and(|ext| ext == "chwal")))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedRecordingResult {
    pub session_id: chronicle_common::SessionId,
    pub already_published: bool,
    pub ignored_post_commit_records: u64,
    pub checkpoint: RecordingEtlCheckpoint,
}

pub(crate) struct QuotaPublicationReservation<'a> {
    authority: &'a QuotaReservationAuthority,
    final_bytes: u64,
    manifest_bytes: u64,
    checkpoint_bytes: Option<u64>,
}

impl Drop for QuotaPublicationReservation<'_> {
    fn drop(&mut self) {
        let _ = self
            .authority
            .release(ReservationKind::FinalSession, self.final_bytes);
        let _ = self
            .authority
            .release(ReservationKind::Manifest, self.manifest_bytes);
        if let Some(bytes) = self.checkpoint_bytes {
            let _ = self.authority.release(ReservationKind::Checkpoint, bytes);
        }
    }
}

pub(crate) fn reserve_publication_peak(
    authority: &QuotaReservationAuthority,
    session_bytes: u64,
) -> Result<QuotaPublicationReservation<'_>, ApplicationError> {
    let final_bytes = session_bytes.saturating_mul(2);
    authority
        .reserve(ReservationKind::FinalSession, final_bytes)
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    if let Err(error) = authority.reserve(ReservationKind::Manifest, final_bytes) {
        let _ = authority.release(ReservationKind::FinalSession, final_bytes);
        return Err(ApplicationError::InvalidConfig(error.to_string()));
    }
    Ok(QuotaPublicationReservation {
        authority,
        final_bytes,
        manifest_bytes: final_bytes,
        checkpoint_bytes: None,
    })
}

impl QuotaPublicationReservation<'_> {
    pub(crate) fn reserve_checkpoint(&mut self, bytes: u64) -> Result<(), ApplicationError> {
        let reservation = bytes.saturating_mul(2);
        self.authority
            .reserve(ReservationKind::Checkpoint, reservation)
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
        self.checkpoint_bytes = Some(reservation);
        Ok(())
    }
}

/// Processes one finalized recording and atomically publishes its deterministic session.
pub fn process_and_publish_recording_wal(
    wal_directory: impl AsRef<Path>,
    root: impl AsRef<Path>,
    registry: &ProtocolRegistry,
) -> Result<PublishedRecordingResult, ApplicationError> {
    process_and_publish_recording_wal_inner(wal_directory, root, registry, None, false)
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub(crate) fn process_and_publish_recording_wal_owned(
    wal_directory: impl AsRef<Path>,
    root: impl AsRef<Path>,
    registry: &ProtocolRegistry,
    quota: &QuotaReservationAuthority,
) -> Result<PublishedRecordingResult, ApplicationError> {
    process_and_publish_recording_wal_inner(wal_directory, root, registry, Some(quota), true)
}

#[allow(clippy::too_many_lines)] // transaction keeps publication-before-checkpoint ordering inline
fn process_and_publish_recording_wal_inner(
    wal_directory: impl AsRef<Path>,
    root: impl AsRef<Path>,
    registry: &ProtocolRegistry,
    quota: Option<&QuotaReservationAuthority>,
    lease_owned: bool,
) -> Result<PublishedRecordingResult, ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let root = root.as_ref();
    if !lease_owned
        && let Some(state_root) = wal_directory.parent()
        && RecorderLease::state_is_owned(state_root)
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?
    {
        return Err(ApplicationError::InvalidConfig(
            "standalone ETL rejected while recorder owns state domain".into(),
        ));
    }
    let recording_id = load_recording_metadata(wal_directory)?
        .ok_or_else(|| {
            ApplicationError::RecordingMetadataValidation("recording metadata is missing".into())
        })?
        .recording_id;
    let processed = process_recording_wal(
        wal_directory,
        registry,
        chronicle_common::SessionId(recording_id.0),
    )?;
    if let Some(checkpoint) = load_recording_etl_checkpoint(wal_directory)?
        && checkpoint.recording_id == processed.recording_id
        && checkpoint.wal_snapshot_sha256 != sha256_string(&processed.wal_snapshot_sha256)
    {
        return Err(ApplicationError::CheckpointContradiction);
    }
    let issues = issue_summaries(&processed.output.issues);
    let replayability = replayability_reasons(&processed.output);
    let complete = session_complete(&processed.output);
    let expected = PublishSession {
        session: processed.output.session,
        checkpoint: None,
        issues: issues.into_iter().map(|issue| issue.code).collect(),
        replayability,
        complete,
    };
    let session_id = expected.session.id;
    let session_bytes = serde_json::to_vec(&expected.session)?.len() as u64;
    let mut quota_reservation = quota
        .map(|authority| reserve_publication_peak(authority, session_bytes))
        .transpose()?;
    let store = FilesystemSessionStore::new(root);
    // The output root may not exist yet; publication creates it, so create it
    // up front for the canonicalized checkpoint output_root field.
    fs::create_dir_all(root)?;
    // Serialize the recording-local checkpoint with the same serde bytes the
    // atomic writer used; the ETL-owned transaction writes them after
    // publication and verification so application never calls the concrete
    // session publisher or advances ETL checkpoints itself.
    let checkpoint = RecordingEtlCheckpoint {
        version: RECORDING_ETL_CHECKPOINT_VERSION,
        recording_id: processed.recording_id,
        commit_marker_segment_ordinal: processed.commit_boundary.segment_ordinal,
        commit_marker_byte_offset: processed.commit_marker_byte_offset,
        commit_marker_sequence: processed.commit_boundary.marker_sequence,
        wal_snapshot_sha256: sha256_string(&processed.wal_snapshot_sha256),
        recovery_sha256: sha256_string(&processed.recovery_sha256),
        wal_format_version: chronicle_wal::WAL_FORMAT_VERSION,
        pipeline_version: ETL_PIPELINE_VERSION.into(),
        canonical_schema_version: chronicle_canonical::CANONICAL_SCHEMA_VERSION,
        session_id,
        manifest_checksum: String::new(),
        output_root: fs::canonicalize(root)?.display().to_string(),
        output_identity: format!("sessions/{session_id}/manifest.json"),
        status: processed.status,
        shutdown_reason: processed.shutdown_reason,
        counters: processed.counters,
    };
    let outcome = chronicle_etl::publish_final_session(
        &store,
        &expected,
        // The manifest checksum is known only after publication and manifest
        // verification; build the persisted checkpoint bytes inside the
        // transaction so the real checksum lands on disk.
        |outcome| {
            let mut checkpoint = checkpoint.clone();
            checkpoint
                .manifest_checksum
                .clone_from(&outcome.manifest_checksum);
            serde_json::to_vec(&checkpoint).map_err(|error| {
                chronicle_etl::OneShotPublicationError::Checkpoint(error.to_string())
            })
        },
        &wal_directory.join("etl-checkpoint.json"),
        |bytes| {
            if let Some(reservation) = quota_reservation.as_mut() {
                reservation.reserve_checkpoint(bytes).map_err(|error| {
                    chronicle_etl::OneShotPublicationError::Reserve(error.to_string())
                })?;
            }
            Ok(())
        },
        |store, expected| {
            published_recording_matches(store, expected)
                .map_err(|error| chronicle_etl::OneShotPublicationError::Store(error.to_string()))
        },
    )
    .map_err(|error| match error {
        chronicle_etl::OneShotPublicationError::Mismatch { session_id } => {
            ApplicationError::PublishedRecordingMismatch { session_id }
        }
        other => ApplicationError::InvalidConfig(other.to_string()),
    })?;
    let checkpoint = RecordingEtlCheckpoint {
        manifest_checksum: outcome.manifest_checksum,
        ..checkpoint
    };
    drop(quota_reservation);
    Ok(PublishedRecordingResult {
        session_id: outcome.session_id,
        already_published: outcome.already_published,
        ignored_post_commit_records: processed.ignored_post_commit_records,
        checkpoint,
    })
}

pub(crate) fn load_recording_etl_checkpoint(
    wal_directory: &Path,
) -> Result<Option<RecordingEtlCheckpoint>, ApplicationError> {
    match fs::read(wal_directory.join("etl-checkpoint.json")) {
        Ok(bytes) => {
            let checkpoint: RecordingEtlCheckpoint = serde_json::from_slice(&bytes)?;
            if checkpoint.version != RECORDING_ETL_CHECKPOINT_VERSION {
                return Err(ApplicationError::RecordingMetadataValidation(format!(
                    "unsupported recording ETL checkpoint version {}",
                    checkpoint.version
                )));
            }
            Ok(Some(checkpoint))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn recovery_sha256(scan: &RecoveryScan) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"chronicle/recovery/v1\0");
    digest.update(
        scan.authority
            .marker_sequence
            .unwrap_or_default()
            .to_le_bytes(),
    );
    digest.update(
        scan.authority
            .durable_through_sequence
            .unwrap_or_default()
            .to_le_bytes(),
    );
    digest.update(scan.authority.durable_record_count.to_le_bytes());
    digest.update(scan.authority.durable_payload_bytes.to_le_bytes());
    digest.update(
        scan.authority
            .segment_ordinal
            .unwrap_or_default()
            .to_le_bytes(),
    );
    digest.update(
        u64::try_from(scan.committed.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    digest.update(
        u64::try_from(scan.uncommitted.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    digest.update([u8::from(scan.partial_tail.is_some())]);
    digest.finalize().into()
}

pub(crate) fn sha256_string(bytes: &[u8; 32]) -> String {
    let mut value = String::from("sha256:");
    for byte in bytes {
        write!(value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
}

fn published_recording_matches(
    store: &FilesystemSessionStore,
    expected: &PublishSession,
) -> Result<bool, StorageError> {
    let inspection = store.inspect_with_metadata(expected.session.id)?;
    let hydrated = store.hydrate(expected.session.id)?;
    Ok(hydrated == expected.session
        && inspection.checkpoint == expected.checkpoint
        && inspection.issues == expected.issues
        && inspection.replayability == expected.replayability
        && inspection.complete == expected.complete)
}

pub(crate) fn metadata_terminal_wal_losses(
    summary: Option<&TerminalWalLossSummary>,
) -> Vec<TerminalWalLoss> {
    summary
        .into_iter()
        .flat_map(|summary| &summary.entries)
        .filter(|entry| entry.persistence != TerminalWalLossPersistence::PersistedWal)
        .filter_map(|entry| {
            let (start, end) = (entry.start.clone()?, entry.end.clone()?);
            (start.clock == end.clock).then_some(TerminalWalLoss {
                interval: TerminalWalLossInterval { start, end },
                discarded_records: entry.discarded.records,
                discarded_payload_bytes: entry.discarded.bytes,
                reason: TerminalWalLossReason::WalHardLimit,
                ambiguity: TerminalWalLossAmbiguity::UnknownDownstreamEffects,
            })
        })
        .collect()
}

pub(crate) fn shutdown_reason_name(reason: ShutdownReason) -> &'static str {
    match reason {
        ShutdownReason::UserInterrupt => "user_interrupt",
        ShutdownReason::TerminationSignal => "termination_signal",
        ShutdownReason::SourceCompleted => "source_completed",
        ShutdownReason::DurationLimit => "duration_limit",
        ShutdownReason::WalSizeLimit => "wal_size_limit",
        ShutdownReason::CaptureFailure => "capture_failure",
        ShutdownReason::WalFailure => "wal_failure",
        ShutdownReason::ProcessCrashRecovered => "process_crash_recovered",
        ShutdownReason::ForcedTermination => "forced_termination",
    }
}
