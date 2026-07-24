//! Linux eBPF adapter boundary.
//!
//! Aya programs and hook-specific semantics are intentionally deferred. Future socket
//! read/write hooks must emit ordered byte chunks and document where truncation occurs.

use chronicle_capture::{CaptureError, CaptureEvent, CaptureSource};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EbpfCaptureError {
    #[error("eBPF capture is not implemented in this scaffold")]
    NotImplemented,
    #[error("eBPF capture requires Linux")]
    UnsupportedPlatform,
}

pub struct EbpfCaptureSource;

impl EbpfCaptureSource {
    pub fn load() -> Result<Self, EbpfCaptureError> {
        if cfg!(target_os = "linux") {
            Err(EbpfCaptureError::NotImplemented)
        } else {
            Err(EbpfCaptureError::UnsupportedPlatform)
        }
    }
}

impl CaptureSource for EbpfCaptureSource {
    fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        Err(CaptureError::Source(
            EbpfCaptureError::NotImplemented.to_string(),
        ))
    }
}
