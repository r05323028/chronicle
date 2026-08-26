use chronicle_etl::{
    BoundedNativeObservationChannel, ChronicleExecutionContext, NativeExecutionContextCarrier,
    NativeExecutionContextGeneration, NativeExecutionHandoffObservation,
    NativeObservationDiagnostic, NativeOperationBoundaryReceipt, NativeSourceError,
    NativeSourceLimits,
};

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

    pub fn record_parent_boundary(
        &mut self,
        context: ChronicleExecutionContext,
        receipt: NativeOperationBoundaryReceipt,
    ) -> Result<(), NativeSourceError> {
        let observations = self.carrier.record_parent_receipt(context, receipt)?;
        self.deliver(observations)
    }

    pub fn record_child_boundary(
        &mut self,
        context: ChronicleExecutionContext,
        receipt: NativeOperationBoundaryReceipt,
    ) -> Result<(), NativeSourceError> {
        let observations = self.carrier.record_child_receipt(context, receipt)?;
        self.deliver(observations)
    }

    pub fn expire(&mut self, generation: NativeExecutionContextGeneration) -> NativeSourceDelivery {
        NativeSourceDelivery {
            observations: Vec::new(),
            diagnostics: self.carrier.expire_pending(generation),
        }
    }

    pub fn restart(&mut self) -> NativeSourceDelivery {
        NativeSourceDelivery {
            observations: Vec::new(),
            diagnostics: self.carrier.restart(),
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
        observations: Vec<NativeExecutionHandoffObservation>,
    ) -> Result<(), NativeSourceError> {
        for observation in observations {
            self.channel.push(observation)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_canonical::{SourceConnectionGeneration, WalByteRange};
    use chronicle_common::{Direction, EpochId, ProtocolId, RecordingId};
    use chronicle_protocol::ProtocolOperationBoundaryIdentity;

    fn receipt(sequence: u64) -> NativeOperationBoundaryReceipt {
        NativeOperationBoundaryReceipt {
            recording_id: RecordingId::from_uuid(uuid::Uuid::from_u128(1)),
            source_epoch_id: EpochId::from_uuid(uuid::Uuid::from_u128(2)),
            source_generation: SourceConnectionGeneration::Fixture,
            protocol_id: ProtocolId::new("http/1.1"),
            direction: Direction::ClientToServer,
            protocol_operation_boundary: ProtocolOperationBoundaryIdentity::from_canonicalizer(
                ProtocolId::new("http/1.1"),
                format!("http-request-sequence:{sequence}"),
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
}
