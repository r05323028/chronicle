use chronicle_canonical::SourceConnectionGeneration;
use chronicle_common::{EpochId, RecordingId};
use chronicle_etl::{
    BoundedNativeObservationChannel, ChronicleExecutionContext, NativeExecutionContextCarrier,
    NativeExecutionContextGeneration, NativeExecutionHandoffObservation,
    NativeObservationDiagnostic, NativeOperationBoundaryReceipt, NativeSourceError,
    NativeSourceLimits,
};
use chronicle_protocol_builtins::http::HttpRequestBoundaryObservation;

/// HTTP transport boundary adapter. It accepts a protocol-decoder-derived
/// observation plus capture placement before canonical operations exist; the
/// registered HTTP canonicalizer later establishes trusted identity during ETL
/// binding.
#[derive(Clone, Debug)]
pub struct NativeHttpBoundaryAdapter {
    recording_id: RecordingId,
    source_epoch_id: EpochId,
    source_generation: SourceConnectionGeneration,
}

impl NativeHttpBoundaryAdapter {
    pub fn new(
        recording_id: RecordingId,
        source_epoch_id: EpochId,
        source_generation: SourceConnectionGeneration,
    ) -> Self {
        Self {
            recording_id,
            source_epoch_id,
            source_generation,
        }
    }

    pub fn request_boundary(
        &self,
        observation: &HttpRequestBoundaryObservation,
    ) -> Result<NativeOperationBoundaryReceipt, NativeSourceError> {
        let claim = observation.protocol_operation_boundary_claim();
        let protocol_id = claim.protocol().clone();
        let direction = claim.direction();
        let receipt = NativeOperationBoundaryReceipt {
            recording_id: self.recording_id,
            source_epoch_id: self.source_epoch_id,
            source_generation: self.source_generation.clone(),
            protocol_id,
            direction,
            protocol_operation_boundary: claim,
            source_range: observation.source_range().clone(),
        };
        receipt.validate()?;
        Ok(receipt)
    }
}

/// Application-owned opt-in wiring for Chronicle-native execution handoffs.
/// It carries no provider context, scenario identity, or canonical operation ref.
pub struct CooperativeNativeSource {
    carrier: NativeExecutionContextCarrier,
    channel: BoundedNativeObservationChannel,
}

pub struct NativeSourceDelivery {
    pub observations: Vec<NativeExecutionHandoffObservation>,
    pub diagnostics: Vec<NativeObservationDiagnostic>,
}

impl CooperativeNativeSource {
    pub fn new(limits: NativeSourceLimits, channel_capacity: usize) -> Option<Self> {
        Some(Self {
            carrier: NativeExecutionContextCarrier::new(limits),
            channel: BoundedNativeObservationChannel::new(channel_capacity)?,
        })
    }

    pub fn start_root(&mut self) -> Result<ChronicleExecutionContext, NativeSourceError> {
        self.carrier.start_root()
    }

    pub fn continue_from(
        &mut self,
        parent: ChronicleExecutionContext,
    ) -> Result<ChronicleExecutionContext, NativeSourceError> {
        self.carrier.continue_from(parent)
    }

    pub fn retire_context(
        &mut self,
        context: ChronicleExecutionContext,
    ) -> Result<(), NativeSourceError> {
        self.carrier.retire_context(context)
    }

    pub fn record_parent_boundary(
        &mut self,
        context: ChronicleExecutionContext,
        receipt: NativeOperationBoundaryReceipt,
    ) -> Result<(), NativeSourceError> {
        let observations = self.carrier.record_parent_receipt(context, receipt)?;
        self.deliver(&observations)
    }

    pub fn record_child_boundary(
        &mut self,
        context: ChronicleExecutionContext,
        receipt: NativeOperationBoundaryReceipt,
    ) -> Result<(), NativeSourceError> {
        let observations = self.carrier.record_child_receipt(context, receipt)?;
        self.deliver(&observations)
    }

    pub fn expire(&mut self, generation: NativeExecutionContextGeneration) -> NativeSourceDelivery {
        NativeSourceDelivery {
            observations: Vec::new(),
            diagnostics: self.carrier.expire_pending(generation),
        }
    }

    pub fn restart(&mut self) -> NativeSourceDelivery {
        let lost_observations = self.channel.drain().len();
        NativeSourceDelivery {
            observations: Vec::new(),
            diagnostics: self
                .carrier
                .restart_with_side_channel_loss(lost_observations),
        }
    }

    pub fn drain(&mut self) -> NativeSourceDelivery {
        NativeSourceDelivery {
            observations: self.channel.drain(),
            diagnostics: self.carrier.take_diagnostics(),
        }
    }

    fn deliver(
        &mut self,
        observations: &[NativeExecutionHandoffObservation],
    ) -> Result<(), NativeSourceError> {
        if observations.is_empty() {
            return Ok(());
        }
        match self.channel.push_batch(observations) {
            Ok(()) => self.carrier.acknowledge_delivered(observations),
            Err(error @ NativeSourceError::SideChannelLimitExceeded { limit }) => {
                self.carrier
                    .record_side_channel_loss(observations.len(), limit);
                Err(error)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_canonical::{SourceConnectionGeneration, WalByteRange};
    use chronicle_common::{Direction, EpochId, ProtocolId, RecordingId};
    use chronicle_etl::NativeObservationDiagnosticKind;
    use chronicle_protocol::{DecodedFrame, ProtocolDecoder, ProtocolOperationBoundaryClaim};

    fn receipt(sequence: u64) -> NativeOperationBoundaryReceipt {
        NativeOperationBoundaryReceipt {
            recording_id: RecordingId::from_uuid(uuid::Uuid::from_u128(1)),
            source_epoch_id: EpochId::from_uuid(uuid::Uuid::from_u128(2)),
            source_generation: SourceConnectionGeneration::Fixture,
            protocol_id: ProtocolId::new("http/1.1"),
            direction: Direction::ClientToServer,
            protocol_operation_boundary: ProtocolOperationBoundaryClaim::new(
                ProtocolId::new("http/1.1"),
                format!("http-request-sequence:{sequence}"),
                Direction::ClientToServer,
            )
            .unwrap(),
            source_range: WalByteRange {
                segment_ordinal: 0,
                segment_first_sequence: 0,
                frame_byte_offset: sequence,
                wal_sequence: sequence,
                direction: Direction::ClientToServer,
                payload_byte_offset: 0,
                payload_byte_length: 1,
                gap_before: false,
            },
        }
    }

    #[test]
    fn cooperative_source_delivers_explicit_handoff_only_after_both_boundaries() {
        let limits = NativeSourceLimits::new(8, 8, 8).unwrap();
        let mut source = CooperativeNativeSource::new(limits, 8).unwrap();
        let parent = source.start_root().unwrap();
        let child = source.continue_from(parent).unwrap();
        source.record_child_boundary(child, receipt(2)).unwrap();
        assert!(source.drain().observations.is_empty());
        source.record_parent_boundary(parent, receipt(1)).unwrap();
        let delivery = source.drain();
        assert_eq!(delivery.observations.len(), 1);
        assert!(delivery.diagnostics.is_empty());
    }

    #[test]
    fn multi_child_batch_overflow_is_atomic_and_retryable() {
        let limits = NativeSourceLimits::new(8, 8, 8).unwrap();
        let mut source = CooperativeNativeSource::new(limits, 2).unwrap();

        // Keep one unrelated observation queued so the next two-child batch
        // cannot fit. This observation must be the only one drained after the
        // failed admission.
        let prefill_parent = source.start_root().unwrap();
        let prefill_child = source.continue_from(prefill_parent).unwrap();
        source
            .record_child_boundary(prefill_child, receipt(2))
            .unwrap();
        source
            .record_parent_boundary(prefill_parent, receipt(1))
            .unwrap();

        let parent = source.start_root().unwrap();
        let child_a = source.continue_from(parent).unwrap();
        let child_b = source.continue_from(parent).unwrap();
        source.record_child_boundary(child_a, receipt(4)).unwrap();
        source.record_child_boundary(child_b, receipt(5)).unwrap();
        let error = source
            .record_parent_boundary(parent, receipt(3))
            .unwrap_err();
        assert_eq!(
            error,
            NativeSourceError::SideChannelLimitExceeded { limit: 2 }
        );
        let failed = source.drain();
        assert_eq!(failed.observations.len(), 1);
        assert_eq!(
            failed.diagnostics,
            vec![NativeObservationDiagnostic {
                kind: NativeObservationDiagnosticKind::SideChannelLoss,
                context_generation: None,
                message: "native side-channel batch of 2 observations exceeds capacity 2".into(),
            }]
        );

        // Receipt state stayed pending, so retry after draining admits both
        // children together.
        source.record_parent_boundary(parent, receipt(3)).unwrap();
        let retried = source.drain();
        assert_eq!(retried.observations.len(), 2);
        assert!(retried.diagnostics.is_empty());
    }

    #[test]
    fn retired_roots_release_active_context_capacity() {
        let limits = NativeSourceLimits::new(1, 1, 1).unwrap();
        let mut source = CooperativeNativeSource::new(limits, 1).unwrap();
        for _ in 0..64 {
            let root = source.start_root().unwrap();
            source.retire_context(root).unwrap();
        }
        assert!(source.start_root().is_ok());
    }

    #[test]
    fn restart_discards_queued_observations_with_typed_loss() {
        let limits = NativeSourceLimits::new(4, 4, 4).unwrap();
        let mut source = CooperativeNativeSource::new(limits, 4).unwrap();
        let parent = source.start_root().unwrap();
        let child = source.continue_from(parent).unwrap();
        source.record_child_boundary(child, receipt(2)).unwrap();
        source.record_parent_boundary(parent, receipt(1)).unwrap();

        let restart = source.restart();
        assert!(restart.observations.is_empty());
        assert_eq!(
            restart.diagnostics,
            vec![NativeObservationDiagnostic {
                kind: NativeObservationDiagnosticKind::SideChannelLoss,
                context_generation: None,
                message: "native side-channel lost 1 queued observations during restart".into(),
            }]
        );
        assert!(source.drain().observations.is_empty());
        assert!(source.drain().diagnostics.is_empty());
    }

    fn request_observation(sequence: u64) -> HttpRequestBoundaryObservation {
        let mut decoder = chronicle_protocol_builtins::http::Decoder::new();
        let frames = decoder
            .push(DecodedFrame {
                direction: Direction::ClientToServer,
                sequence,
                payload: b"GET / HTTP/1.1\r\nHost: example\r\n\r\n".to_vec(),
                attributes: std::collections::BTreeMap::new(),
                connection_generation: Some(SourceConnectionGeneration::Fixture),
                provenance: vec![WalByteRange {
                    segment_ordinal: 0,
                    segment_first_sequence: 0,
                    frame_byte_offset: sequence,
                    wal_sequence: sequence,
                    direction: Direction::ClientToServer,
                    payload_byte_offset: 0,
                    payload_byte_length: 1,
                    gap_before: false,
                }],
                missing_payload_provenance: false,
            })
            .unwrap();
        HttpRequestBoundaryObservation::from_decoded_request(&frames[0]).unwrap()
    }

    #[test]
    fn http_boundary_adapter_requires_protocol_observation() {
        let adapter = NativeHttpBoundaryAdapter::new(
            RecordingId::from_uuid(uuid::Uuid::from_u128(10)),
            EpochId::from_uuid(uuid::Uuid::from_u128(11)),
            SourceConnectionGeneration::Fixture,
        );
        let observation = request_observation(4);
        let boundary = adapter.request_boundary(&observation).unwrap();
        assert_eq!(
            boundary.protocol_operation_boundary.value(),
            "http-request-sequence:4"
        );
        assert_eq!(
            boundary.protocol_operation_boundary.direction(),
            Direction::ClientToServer
        );
    }

    #[test]
    fn non_http_or_missing_provenance_cannot_be_boundary_observation() {
        let frame = DecodedFrame {
            direction: Direction::ClientToServer,
            sequence: 4,
            payload: b"application-counter:4".to_vec(),
            attributes: std::collections::BTreeMap::new(),
            connection_generation: None,
            provenance: Vec::new(),
            missing_payload_provenance: false,
        };
        assert!(HttpRequestBoundaryObservation::from_decoded_request(&frame).is_none());
    }
}
