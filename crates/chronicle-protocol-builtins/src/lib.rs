//! Statically linked protocol registrations.
//!
//! Only `fake` is functional. Real protocol modules declare honest target status.

use chronicle_common::ProtocolId;
use chronicle_protocol::{
    CapabilityStatus, ProtocolCapabilities, ProtocolError, ProtocolRegistration, ProtocolRegistry,
};

fn scaffold_registration(
    id: &str,
    display_name: &'static str,
    capabilities: ProtocolCapabilities,
) -> ProtocolRegistration {
    ProtocolRegistration {
        id: ProtocolId::new(id),
        display_name,
        capabilities,
        detector: None,
        decoder_factory: None,
        canonicalizer: None,
        replay_adapter: None,
        verifier: None,
    }
}

const PLANNED: ProtocolCapabilities = ProtocolCapabilities {
    detection: CapabilityStatus::Planned,
    decoding: CapabilityStatus::Planned,
    canonicalization: CapabilityStatus::Planned,
    replay: CapabilityStatus::Planned,
    verification: CapabilityStatus::Planned,
};

pub mod http {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("http/1.1", "HTTP/1.1", PLANNED)
    }
}

pub mod postgres {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("postgres", "PostgreSQL", PLANNED)
    }
}

pub mod mysql_family {
    //! Shared MySQL/MariaDB framing boundary.
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("mysql-family", "MySQL family", PLANNED)
    }
}

pub mod mysql {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("mysql", "MySQL", PLANNED)
    }
}

pub mod mariadb {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("mariadb", "MariaDB", PLANNED)
    }
}

pub mod oracle {
    //! Oracle Net/TNS semantics remain research-only; opaque preservation is required.
    use super::{
        CapabilityStatus, ProtocolCapabilities, ProtocolRegistration, scaffold_registration,
    };
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration(
            "oracle",
            "Oracle Net",
            ProtocolCapabilities {
                detection: CapabilityStatus::Research,
                decoding: CapabilityStatus::Research,
                canonicalization: CapabilityStatus::Planned,
                replay: CapabilityStatus::Research,
                verification: CapabilityStatus::Research,
            },
        )
    }
}

pub mod mongodb {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("mongodb", "MongoDB", PLANNED)
    }
}

pub mod kafka {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("kafka", "Kafka", PLANNED)
    }
}

pub mod nats {
    use super::{PLANNED, ProtocolRegistration, scaffold_registration};
    pub fn registration() -> ProtocolRegistration {
        scaffold_registration("nats", "NATS", PLANNED)
    }
}

pub mod fake {
    use chronicle_canonical::{
        Attributes, CanonicalOperation, OperationEffect, OperationKind, PayloadRef, ProtocolData,
        RelativeTimeNanos,
    };
    use chronicle_common::{Direction, Endpoint, OperationId, ProtocolId};
    use chronicle_protocol::{
        BoxFuture, CapabilityStatus, DecodedFrame, DecoderFactory, DetectionInput, DetectionResult,
        ObservedResponse, ProtocolCanonicalizer, ProtocolCapabilities, ProtocolDecoder,
        ProtocolDetector, ProtocolError, ProtocolRegistration, ProtocolStream, ReplayAdapter,
        ReplayConnection, ReplayContext, VerificationResult, VerificationStatus, Verifier,
    };
    use std::collections::BTreeMap;
    use std::sync::Arc;

    struct FakeDetector {
        id: ProtocolId,
    }

    impl ProtocolDetector for FakeDetector {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn detect(&self, input: DetectionInput<'_>) -> DetectionResult {
            if input
                .stream
                .chunks
                .iter()
                .any(|chunk| chunk.payload.starts_with(b"FAKE "))
            {
                DetectionResult::Confirmed {
                    confidence: 100,
                    evidence: "FAKE prefix".into(),
                }
            } else {
                DetectionResult::Unknown
            }
        }
    }

    struct FakeDecoderFactory {
        id: ProtocolId,
    }

    impl DecoderFactory for FakeDecoderFactory {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn create(&self) -> Box<dyn ProtocolDecoder> {
            Box::new(FakeDecoder {
                id: self.id.clone(),
            })
        }
    }

    struct FakeDecoder {
        id: ProtocolId,
    }

    impl ProtocolDecoder for FakeDecoder {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn push(&mut self, frame: DecodedFrame) -> Result<Vec<DecodedFrame>, ProtocolError> {
            Ok(vec![frame])
        }

        fn finish(&mut self) -> Result<Vec<DecodedFrame>, ProtocolError> {
            Ok(Vec::new())
        }
    }

    struct FakeCanonicalizer {
        id: ProtocolId,
    }

    impl FakeCanonicalizer {
        fn operation(
            request: DecodedFrame,
            response: Option<DecodedFrame>,
            truncated: bool,
        ) -> CanonicalOperation {
            let incomplete = response.is_none();
            CanonicalOperation {
                id: OperationId::new(),
                sequence: request.sequence,
                started_at_offset: RelativeTimeNanos(request.sequence),
                completed_at_offset: response
                    .as_ref()
                    .map(|frame| RelativeTimeNanos(frame.sequence)),
                kind: OperationKind::Request,
                effect: OperationEffect::Read,
                request: PayloadRef::Inline {
                    content_type: Some("application/x-chronicle-fake".into()),
                    bytes: request.payload.clone(),
                },
                recorded_response: response.map(|frame| PayloadRef::Inline {
                    content_type: Some("application/x-chronicle-fake".into()),
                    bytes: frame.payload,
                }),
                attributes: Attributes::new(),
                protocol_data: ProtocolData {
                    schema_version: 1,
                    media_type: Some("application/x-chronicle-fake".into()),
                    bytes: request.payload,
                },
                incomplete,
                truncated,
                redactions: Vec::new(),
            }
        }
    }

    impl ProtocolCanonicalizer for FakeCanonicalizer {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn canonicalize(
            &self,
            stream: &ProtocolStream<'_>,
            frames: Vec<DecodedFrame>,
        ) -> Result<Vec<CanonicalOperation>, ProtocolError> {
            let mut operations = Vec::new();
            let mut pending = None;
            for frame in frames {
                match frame.direction {
                    Direction::ClientToServer => {
                        if let Some(request) = pending.replace(frame) {
                            operations.push(Self::operation(request, None, stream.truncated));
                        }
                    }
                    Direction::ServerToClient => {
                        if let Some(request) = pending.take() {
                            operations.push(Self::operation(
                                request,
                                Some(frame),
                                stream.truncated,
                            ));
                        }
                    }
                }
            }
            if let Some(request) = pending {
                operations.push(Self::operation(request, None, stream.truncated));
            }
            Ok(operations)
        }
    }

    struct FakeReplayAdapter {
        id: ProtocolId,
    }

    impl ReplayAdapter for FakeReplayAdapter {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn connect<'a>(
            &'a self,
            _target: &'a Endpoint,
            _context: &'a ReplayContext,
        ) -> BoxFuture<'a, Result<Box<dyn ReplayConnection>, ProtocolError>> {
            Box::pin(async { Ok(Box::new(FakeReplayConnection) as Box<dyn ReplayConnection>) })
        }
    }

    struct FakeReplayConnection;

    impl ReplayConnection for FakeReplayConnection {
        fn execute<'a>(
            &'a mut self,
            operation: &'a CanonicalOperation,
        ) -> BoxFuture<'a, Result<ObservedResponse, ProtocolError>> {
            Box::pin(async move {
                Ok(ObservedResponse {
                    payload: operation.recorded_response.clone(),
                    attributes: BTreeMap::new(),
                    error_category: None,
                })
            })
        }
    }

    struct FakeVerifier {
        id: ProtocolId,
    }

    impl Verifier for FakeVerifier {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn verify(
            &self,
            operation: &CanonicalOperation,
            observed: &ObservedResponse,
        ) -> VerificationResult {
            let passed = operation.recorded_response == observed.payload;
            VerificationResult {
                status: if passed {
                    VerificationStatus::Passed
                } else {
                    VerificationStatus::Failed
                },
                summary: if passed {
                    "fake response matched".into()
                } else {
                    "fake response differed".into()
                },
                details: BTreeMap::new(),
            }
        }
    }

    pub fn registration() -> ProtocolRegistration {
        let id = ProtocolId::new("fake");
        ProtocolRegistration {
            id: id.clone(),
            display_name: "Chronicle fake protocol",
            capabilities: ProtocolCapabilities {
                detection: CapabilityStatus::Available,
                decoding: CapabilityStatus::Available,
                canonicalization: CapabilityStatus::Available,
                replay: CapabilityStatus::Available,
                verification: CapabilityStatus::Available,
            },
            detector: Some(Arc::new(FakeDetector { id: id.clone() })),
            decoder_factory: Some(Arc::new(FakeDecoderFactory { id: id.clone() })),
            canonicalizer: Some(Arc::new(FakeCanonicalizer { id: id.clone() })),
            replay_adapter: Some(Arc::new(FakeReplayAdapter { id: id.clone() })),
            verifier: Some(Arc::new(FakeVerifier { id })),
        }
    }
}

pub fn registry() -> Result<ProtocolRegistry, ProtocolError> {
    let mut registry = ProtocolRegistry::new();
    for registration in [
        fake::registration(),
        http::registration(),
        postgres::registration(),
        mysql_family::registration(),
        mysql::registration(),
        mariadb::registration(),
        oracle::registration(),
        mongodb::registration(),
        kafka::registration(),
        nats::registration(),
    ] {
        registry.register(registration)?;
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_common::Direction;
    use chronicle_protocol::{ProtocolStream, StreamChunk};

    fn stream(payload: &[u8]) -> ProtocolStream<'_> {
        ProtocolStream {
            chunks: vec![StreamChunk {
                direction: Direction::ClientToServer,
                sequence: 1,
                payload,
            }],
            truncated: false,
        }
    }

    #[test]
    fn fake_detects_and_unknown_remains_unknown() {
        let registry = registry().unwrap();
        let (registration, _) = registry.detect(&stream(b"FAKE request"), None).unwrap();
        assert_eq!(registration.id.as_str(), "fake");
        assert!(registry.detect(&stream(b"opaque"), None).is_none());
    }
}
