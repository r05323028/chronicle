#[cfg(target_os = "linux")]
use super::{PathBuf, ProductionSignalStop, mark_recording_forced_termination};

#[cfg(target_os = "linux")]
pub(super) fn spawn_signal_watcher(stop: ProductionSignalStop, wal_directory: PathBuf) {
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};

        let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
            return;
        };
        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            return;
        };
        loop {
            let interrupt_received = tokio::select! {
                _ = interrupt.recv() => true,
                _ = terminate.recv() => false,
            };
            let first = if interrupt_received {
                stop.request_interrupt()
            } else {
                stop.request_termination()
            };
            if !first {
                let _ = mark_recording_forced_termination(&wal_directory);
                std::process::exit(if interrupt_received { 130 } else { 143 });
            }
        }
    });
}

/// Signal watcher for command mode (no WAL marker: the recording directory is
/// internal to the orchestration; a second signal just exits).
#[cfg(target_os = "linux")]
pub(super) fn spawn_command_signal_watcher(stop: ProductionSignalStop) {
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
            return;
        };
        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            return;
        };
        loop {
            let interrupt_received = tokio::select! {
                _ = interrupt.recv() => true,
                _ = terminate.recv() => false,
            };
            if !if interrupt_received {
                stop.request_interrupt()
            } else {
                stop.request_termination()
            } {
                std::process::exit(if interrupt_received { 130 } else { 143 });
            }
        }
    });
}
