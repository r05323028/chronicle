//! Linux eBPF capture adapter boundary.
//!
//! Aya runtime details and private ABI types stay in this crate. `CaptureAdapter`
//! remains the sole normalization boundary into `chronicle-capture` events.

#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
mod abi;
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
mod adapter;
mod error;
mod preflight;
mod source;

pub use adapter::{CaptureAdapter, RecordingScopeConfig};
pub use error::EbpfCaptureError;
pub use preflight::{EbpfPreflight, PreflightCheck, probe_embedded};
pub use source::{EbpfCaptureSource, embedded_object_sha256};
