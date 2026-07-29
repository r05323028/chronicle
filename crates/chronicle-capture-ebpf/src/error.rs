use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EbpfCaptureError {
    #[error("eBPF capture requires Linux")]
    UnsupportedPlatform,
    #[error("eBPF capture requires chronicle-capture-ebpf `linux-ebpf` feature")]
    FeatureDisabled,
    #[error("unsupported eBPF capability: {0}")]
    UnsupportedCapability(&'static str),
    #[error("eBPF attach failed for {hook}: {reason}")]
    Attach {
        hook: &'static str,
        reason: &'static str,
    },
    #[error("eBPF verifier rejected {program}: {reason}")]
    Verifier {
        program: &'static str,
        reason: &'static str,
    },
    #[error("eBPF ABI decode failed at {context}: {reason}")]
    Decode {
        context: &'static str,
        reason: &'static str,
    },
    #[error("eBPF payload is invalid at {context}: {reason}")]
    InvalidPayload {
        context: &'static str,
        reason: &'static str,
    },
    #[error("eBPF observation lacks required identity: {0}")]
    MissingIdentity(&'static str),
    #[error("eBPF socket evidence is invalid: {0}")]
    InvalidSocketEvidence(String),
    #[error("eBPF socket evidence conflicts with cached identity")]
    ConflictingSocketEvidence,
    #[error("eBPF ring-loss evidence is incomplete: {0}")]
    RingLoss(&'static str),
    #[error("eBPF cleanup failed: {0}")]
    Cleanup(&'static str),
}
