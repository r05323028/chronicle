//! Compile-time protocol capability interfaces and registry.

use chronicle_canonical::{CanonicalOperation, PayloadRef, ProtocolData};
use chronicle_common::{Direction, Endpoint, ProtocolId, Timestamp};
use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use thiserror::Error;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityStatus {
    Available,
    Planned,
    Research,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolCapabilities {
    pub detection: CapabilityStatus,
    pub decoding: CapabilityStatus,
    pub canonicalization: CapabilityStatus,
    pub replay: CapabilityStatus,
    pub verification: CapabilityStatus,
}

impl ProtocolCapabilities {
    pub const fn unavailable() -> Self {
        Self {
            detection: CapabilityStatus::Unavailable,
            decoding: CapabilityStatus::Unavailable,
            canonicalization: CapabilityStatus::Unavailable,
            replay: CapabilityStatus::Unavailable,
            verification: CapabilityStatus::Unavailable,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetectionResult {
    Confirmed { confidence: u8, evidence: String },
    Probable { confidence: u8, evidence: String },
    NeedMoreData { minimum_additional_bytes: usize },
    Rejected,
    Unknown,
}

impl DetectionResult {
    fn score(&self) -> Option<(u8, u8)> {
        match self {
            Self::Confirmed { confidence, .. } => Some((2, *confidence)),
            Self::Probable { confidence, .. } => Some((1, *confidence)),
            Self::NeedMoreData { .. } | Self::Rejected | Self::Unknown => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamChunk<'a> {
    pub direction: Direction,
    pub sequence: u64,
    pub timestamp: Option<Timestamp>,
    pub payload: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolStream<'a> {
    pub started_at: Option<Timestamp>,
    pub chunks: Vec<StreamChunk<'a>>,
    pub truncated: bool,
}

pub struct DetectionInput<'a> {
    pub stream: &'a ProtocolStream<'a>,
    pub override_protocol: Option<&'a ProtocolId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedFrame {
    pub direction: Direction,
    pub sequence: u64,
    pub payload: Vec<u8>,
    pub attributes: BTreeMap<String, String>,
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("malformed {protocol} traffic: {message}")]
    Malformed {
        protocol: ProtocolId,
        message: String,
    },
    #[error("protocol capability unavailable: {0}")]
    CapabilityUnavailable(&'static str),
    #[error("protocol registry already contains {0}")]
    Duplicate(ProtocolId),
    #[error("{category:?} transport error: {message}")]
    Transport {
        category: TransportErrorCategory,
        message: String,
    },
    #[error("protocol {0} is not registered")]
    NotRegistered(ProtocolId),
    #[error("protocol replay failed: {0}")]
    Replay(String),
}

pub trait ProtocolDetector: Send + Sync {
    fn protocol(&self) -> &ProtocolId;
    fn detect(&self, input: DetectionInput<'_>) -> DetectionResult;
}

pub trait ProtocolDecoder: Send {
    fn protocol(&self) -> &ProtocolId;
    fn push(&mut self, frame: DecodedFrame) -> Result<Vec<DecodedFrame>, ProtocolError>;
    fn finish(&mut self) -> Result<Vec<DecodedFrame>, ProtocolError>;
}

pub trait DecoderFactory: Send + Sync {
    fn protocol(&self) -> &ProtocolId;
    fn create(&self) -> Box<dyn ProtocolDecoder>;
}

pub trait ProtocolCanonicalizer: Send + Sync {
    fn protocol(&self) -> &ProtocolId;
    fn canonicalize(
        &self,
        stream: &ProtocolStream<'_>,
        frames: Vec<DecodedFrame>,
    ) -> Result<Vec<CanonicalOperation>, ProtocolError>;
}

#[derive(Clone)]
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub fn new(value: Vec<u8>) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Clone, Debug, Default)]
pub struct ReplayContext {
    pub credentials: BTreeMap<String, SecretBytes>,
    pub replacements: BTreeMap<String, String>,
    execution_target: Option<Endpoint>,
}

impl ReplayContext {
    /// Authorizes one explicit loopback target for this execution context.
    pub fn authorize_execution_for(&mut self, target: Endpoint) {
        self.execution_target = Some(target);
    }

    pub fn authorizes_execution_for(&self, target: &Endpoint) -> bool {
        self.execution_target.as_ref() == Some(target)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportErrorCategory {
    Refused,
    Timeout,
    Disconnect,
    Io,
    UnsupportedFraming,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedResponse {
    pub payload: Option<PayloadRef>,
    pub protocol_data: Option<ProtocolData>,
    pub attributes: BTreeMap<String, String>,
    pub error_category: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationStatus {
    Passed,
    Failed,
    Skipped,
    Inconclusive,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationResult {
    pub status: VerificationStatus,
    pub summary: String,
    pub details: BTreeMap<String, String>,
}

pub trait ReplayConnection: Send {
    fn execute<'a>(
        &'a mut self,
        operation: &'a CanonicalOperation,
    ) -> BoxFuture<'a, Result<ObservedResponse, ProtocolError>>;
}

pub trait ReplayAdapter: Send + Sync {
    fn protocol(&self) -> &ProtocolId;
    fn connect<'a>(
        &'a self,
        target: &'a Endpoint,
        context: &'a ReplayContext,
    ) -> BoxFuture<'a, Result<Box<dyn ReplayConnection>, ProtocolError>>;
}

pub trait Verifier: Send + Sync {
    fn protocol(&self) -> &ProtocolId;
    fn verify(
        &self,
        operation: &CanonicalOperation,
        observed: &ObservedResponse,
    ) -> VerificationResult;
}

pub struct ProtocolRegistration {
    pub id: ProtocolId,
    pub display_name: &'static str,
    pub capabilities: ProtocolCapabilities,
    pub detector: Option<Arc<dyn ProtocolDetector>>,
    pub decoder_factory: Option<Arc<dyn DecoderFactory>>,
    pub canonicalizer: Option<Arc<dyn ProtocolCanonicalizer>>,
    pub replay_adapter: Option<Arc<dyn ReplayAdapter>>,
    pub verifier: Option<Arc<dyn Verifier>>,
}

#[derive(Default)]
pub struct ProtocolRegistry {
    registrations: Vec<ProtocolRegistration>,
}

impl ProtocolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, registration: ProtocolRegistration) -> Result<(), ProtocolError> {
        if self
            .registrations
            .iter()
            .any(|existing| existing.id == registration.id)
        {
            return Err(ProtocolError::Duplicate(registration.id));
        }
        self.registrations.push(registration);
        Ok(())
    }

    pub fn get(&self, id: &ProtocolId) -> Option<&ProtocolRegistration> {
        self.registrations
            .iter()
            .find(|registration| &registration.id == id)
    }

    pub fn registrations(&self) -> &[ProtocolRegistration] {
        &self.registrations
    }

    pub fn detect(
        &self,
        stream: &ProtocolStream<'_>,
        override_protocol: Option<&ProtocolId>,
    ) -> Option<(&ProtocolRegistration, DetectionResult)> {
        if let Some(id) = override_protocol {
            return self.get(id).map(|registration| {
                (
                    registration,
                    DetectionResult::Confirmed {
                        confidence: 100,
                        evidence: "explicit user override".into(),
                    },
                )
            });
        }
        self.registrations
            .iter()
            .filter_map(|registration| {
                let detector = registration.detector.as_ref()?;
                let result = detector.detect(DetectionInput {
                    stream,
                    override_protocol: None,
                });
                result.score().map(|score| (registration, result, score))
            })
            .max_by_key(|(_, _, score)| *score)
            .map(|(registration, result, _)| (registration, result))
    }
}
