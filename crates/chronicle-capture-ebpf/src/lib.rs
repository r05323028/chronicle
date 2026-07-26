//! Linux eBPF capture adapter boundary.
//!
//! Aya runtime details and private ABI types stay in this crate. `CaptureAdapter`
//! remains the sole normalization boundary into `chronicle-capture` events.

#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
mod abi;
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
mod adapter;
mod error;
mod source;

pub use adapter::{CaptureAdapter, RecordingScopeConfig};
pub use error::EbpfCaptureError;
pub use source::EbpfCaptureSource;
