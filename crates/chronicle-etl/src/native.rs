use chronicle_canonical::{
    CanonicalOperationRef, CanonicalSession, CorrelationEvidence, CorrelationEvidenceKind,
    ExecutionContinuation, SourceConnectionGeneration, WalByteRange,
};
use chronicle_common::{Direction, EpochId, ProtocolId, RecordingId};
use chronicle_protocol::{ProtocolOperationBoundaryIdentity, ProtocolRegistry};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use thiserror::Error;
use uuid::Uuid;

/// Opaque Chronicle-owned execution-context generation. It is provenance and
/// reuse protection, never a causal predicate by itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NativeExecutionContextGeneration(Uuid);

impl NativeExecutionContextGeneration {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for NativeExecutionContextGeneration {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal provider-neutral Chronicle context. No final canonical reference
/// or scenario identity is available at runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ChronicleExecutionContext {
    pub generation: NativeExecutionContextGeneration,
    pub parent_generation: Option<NativeExecutionContextGeneration>,
}

impl ChronicleExecutionContext {
    pub fn root() -> Self {
        Self {
            generation: NativeExecutionContextGeneration::new(),
            parent_generation: None,
        }
    }

    #[must_use]
    pub fn child(self) -> Self {
        Self {
            generation: NativeExecutionContextGeneration::new(),
            parent_generation: Some(self.generation),
        }
    }
}

pub type NativeExecutionContext = ChronicleExecutionContext;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeSourceLimits {
    pub max_contexts: usize,
    pub max_pending_observations: usize,
    pub max_diagnostics: usize,
}

impl NativeSourceLimits {
    pub const fn new(
        max_contexts: usize,
        max_pending_observations: usize,
        max_diagnostics: usize,
    ) -> Option<Self> {
        if max_contexts == 0 || max_pending_observations == 0 || max_diagnostics == 0 {
            None
        } else {
            Some(Self {
                max_contexts,
                max_pending_observations,
                max_diagnostics,
            })
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum NativeSourceError {
    #[error("native context is not active")]
    UnknownContext,
    #[error("native context limit {limit} exceeded")]
    ContextLimitExceeded { limit: usize },
    #[error("native pending observation limit {limit} exceeded")]
    PendingLimitExceeded { limit: usize },
    #[error("native diagnostic limit {limit} exceeded")]
    DiagnosticLimitExceeded { limit: usize },
    #[error("native side-channel limit {limit} exceeded")]
    SideChannelLimitExceeded { limit: usize },
    #[error("native boundary receipt conflict")]
    ConflictingReceipt,
    #[error("invalid native boundary receipt: {0}")]
    InvalidReceipt(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeObservationDiagnosticKind {
    PendingExpired,
    RestartLostPending,
    SideChannelLoss,
    UnsupportedProtocolBoundary,
    AnchorBindingUnresolved,
    AnchorBindingAmbiguous,
    InvalidBoundHandoff,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeObservationDiagnostic {
    pub kind: NativeObservationDiagnosticKind,
    pub context_generation: Option<NativeExecutionContextGeneration>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeOperationBoundaryReceipt {
    pub recording_id: RecordingId,
    pub source_epoch_id: EpochId,
    pub source_generation: SourceConnectionGeneration,
    pub protocol_id: ProtocolId,
    pub direction: Direction,
    pub protocol_operation_boundary: ProtocolOperationBoundaryIdentity,
    pub source_range: WalByteRange,
}

impl NativeOperationBoundaryReceipt {
    pub fn validate(&self) -> Result<(), NativeSourceError> {
        if self.protocol_id != self.protocol_operation_boundary.protocol {
            return Err(NativeSourceError::InvalidReceipt(
                "protocol identity disagrees with protocol-owned boundary identity".into(),
            ));
        }
        if !self.protocol_operation_boundary.is_authoritative() {
            return Err(NativeSourceError::InvalidReceipt(
                "runtime-supplied operation boundary is not binding authority".into(),
            ));
        }
        if self.source_range.direction != self.direction {
            return Err(NativeSourceError::InvalidReceipt(
                "source range direction disagrees with receipt direction".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeOperationAnchor {
    pub boundary: NativeOperationBoundaryReceipt,
    pub context_generation: NativeExecutionContextGeneration,
}

impl NativeOperationAnchor {
    pub fn new(
        boundary: NativeOperationBoundaryReceipt,
        context_generation: NativeExecutionContextGeneration,
    ) -> Result<Self, NativeSourceError> {
        boundary.validate()?;
        Ok(Self {
            boundary,
            context_generation,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NativeObservationProvenance {
    pub source: String,
    pub parent_context_generation: NativeExecutionContextGeneration,
    pub child_context_generation: NativeExecutionContextGeneration,
}

impl Default for NativeObservationProvenance {
    fn default() -> Self {
        Self {
            source: "chronicle-native-execution-context".into(),
            parent_context_generation: NativeExecutionContextGeneration::default(),
            child_context_generation: NativeExecutionContextGeneration::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeExecutionHandoffObservation {
    pub parent: NativeOperationAnchor,
    pub child: NativeOperationAnchor,
    pub relation: ExecutionContinuation,
    pub provenance: NativeObservationProvenance,
}

impl NativeExecutionHandoffObservation {
    pub fn new(
        parent: NativeOperationAnchor,
        child: NativeOperationAnchor,
        provenance: NativeObservationProvenance,
    ) -> Result<Self, NativeSourceError> {
        if parent.context_generation == child.context_generation {
            return Err(NativeSourceError::InvalidReceipt(
                "native handoff cannot point from a context generation to itself".into(),
            ));
        }
        if provenance.parent_context_generation != parent.context_generation
            || provenance.child_context_generation != child.context_generation
        {
            return Err(NativeSourceError::InvalidReceipt(
                "native handoff provenance does not match endpoint generations".into(),
            ));
        }
        Ok(Self {
            parent,
            child,
            relation: ExecutionContinuation,
            provenance,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundNativeExecutionHandoffFact {
    pub parent: CanonicalOperationRef,
    pub child: CanonicalOperationRef,
    pub relation: ExecutionContinuation,
    pub parent_anchor: NativeOperationAnchor,
    pub child_anchor: NativeOperationAnchor,
    pub provenance: NativeObservationProvenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NativeBindingDiagnosticKind {
    AnchorBindingUnresolved,
    AnchorBindingAmbiguous,
    InvalidBoundHandoff,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeBindingDiagnostic {
    pub kind: NativeBindingDiagnosticKind,
    pub anchor: Option<NativeOperationAnchor>,
    pub candidates: Vec<CanonicalOperationRef>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NativeBindingOutput {
    pub facts: Vec<BoundNativeExecutionHandoffFact>,
    pub diagnostics: Vec<NativeBindingDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeAnchorBinding {
    Bound(CanonicalOperationRef),
    Unresolved(NativeBindingDiagnostic),
    Ambiguous(NativeBindingDiagnostic),
}

#[derive(Clone, Debug, Default)]
pub struct NativeBoundaryIndex {
    boundaries: BTreeMap<CanonicalOperationRef, ProtocolOperationBoundaryIdentity>,
    unavailable: BTreeSet<CanonicalOperationRef>,
}

impl NativeBoundaryIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        reference: CanonicalOperationRef,
        identity: ProtocolOperationBoundaryIdentity,
    ) {
        if identity.is_authoritative() {
            self.boundaries.insert(reference, identity);
            self.unavailable.remove(&reference);
        } else {
            self.boundaries.remove(&reference);
            self.unavailable.insert(reference);
        }
    }

    pub fn from_sessions(sessions: &[CanonicalSession], registry: &ProtocolRegistry) -> Self {
        let mut index = Self::new();
        for session in sessions {
            let Some(recording_id) = session.source_provenance.recording_id else {
                continue;
            };
            for connection in &session.connections {
                let canonicalizer = registry
                    .get(&connection.protocol)
                    .and_then(|registration| registration.canonicalizer.as_ref());
                for operation in &connection.operations {
                    let Some(owner_epoch_id) = operation.provenance.completion_owner_epoch else {
                        continue;
                    };
                    let reference = CanonicalOperationRef::new(
                        recording_id,
                        owner_epoch_id,
                        session.id,
                        operation.id,
                    );
                    let identity = canonicalizer
                        .and_then(|canonicalizer| {
                            canonicalizer.operation_boundary_identity(operation)
                        })
                        .filter(|identity| {
                            identity.protocol == connection.protocol && identity.is_authoritative()
                        });
                    match identity {
                        Some(identity) => index.insert(reference, identity),
                        None => {
                            index.unavailable.insert(reference);
                        }
                    }
                }
            }
        }
        index
    }

    pub fn boundary_for(
        &self,
        reference: &CanonicalOperationRef,
    ) -> Option<&ProtocolOperationBoundaryIdentity> {
        self.boundaries.get(reference)
    }

    pub fn is_unavailable(&self, reference: &CanonicalOperationRef) -> bool {
        self.unavailable.contains(reference)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedNativeObservationChannel {
    max_observations: usize,
    queue: VecDeque<NativeExecutionHandoffObservation>,
}

impl BoundedNativeObservationChannel {
    pub const fn new(max_observations: usize) -> Option<Self> {
        if max_observations == 0 {
            None
        } else {
            Some(Self {
                max_observations,
                queue: VecDeque::new(),
            })
        }
    }

    pub fn push(
        &mut self,
        observation: NativeExecutionHandoffObservation,
    ) -> Result<(), NativeSourceError> {
        if self.queue.len() >= self.max_observations {
            return Err(NativeSourceError::SideChannelLimitExceeded {
                limit: self.max_observations,
            });
        }
        self.queue.push_back(observation);
        Ok(())
    }

    pub fn drain(&mut self) -> Vec<NativeExecutionHandoffObservation> {
        self.queue.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

#[derive(Clone, Debug)]
struct PendingHandoff {
    parent_generation: NativeExecutionContextGeneration,
    child_generation: NativeExecutionContextGeneration,
    child_receipt: Option<NativeOperationBoundaryReceipt>,
}

/// Bounded source-side assembly for explicit parent/child handoffs. It accepts
/// receipts in either order and never emits a partial anchor.
#[derive(Clone, Debug)]
pub struct NativeExecutionContextCarrier {
    limits: NativeSourceLimits,
    contexts: BTreeMap<NativeExecutionContextGeneration, ChronicleExecutionContext>,
    parent_receipts: BTreeMap<NativeExecutionContextGeneration, NativeOperationBoundaryReceipt>,
    pending: BTreeMap<NativeExecutionContextGeneration, PendingHandoff>,
    completed: BTreeSet<NativeExecutionContextGeneration>,
    diagnostics: Vec<NativeObservationDiagnostic>,
}

impl NativeExecutionContextCarrier {
    pub fn new(limits: NativeSourceLimits) -> Self {
        Self {
            limits,
            contexts: BTreeMap::new(),
            parent_receipts: BTreeMap::new(),
            pending: BTreeMap::new(),
            completed: BTreeSet::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn start_root(&mut self) -> Result<ChronicleExecutionContext, NativeSourceError> {
        if self.contexts.len() >= self.limits.max_contexts {
            return Err(NativeSourceError::ContextLimitExceeded {
                limit: self.limits.max_contexts,
            });
        }
        let context = ChronicleExecutionContext::root();
        self.contexts.insert(context.generation, context);
        Ok(context)
    }

    pub fn continue_from(
        &mut self,
        parent: ChronicleExecutionContext,
    ) -> Result<ChronicleExecutionContext, NativeSourceError> {
        if !self.contexts.contains_key(&parent.generation) {
            return Err(NativeSourceError::UnknownContext);
        }
        if self.contexts.len() >= self.limits.max_contexts {
            return Err(NativeSourceError::ContextLimitExceeded {
                limit: self.limits.max_contexts,
            });
        }
        if self.pending.len() >= self.limits.max_pending_observations {
            return Err(NativeSourceError::PendingLimitExceeded {
                limit: self.limits.max_pending_observations,
            });
        }
        let child = parent.child();
        self.contexts.insert(child.generation, child);
        self.pending.insert(
            child.generation,
            PendingHandoff {
                parent_generation: parent.generation,
                child_generation: child.generation,
                child_receipt: None,
            },
        );
        Ok(child)
    }

    pub fn record_parent_receipt(
        &mut self,
        context: ChronicleExecutionContext,
        receipt: NativeOperationBoundaryReceipt,
    ) -> Result<Vec<NativeExecutionHandoffObservation>, NativeSourceError> {
        self.ensure_context(context)?;
        receipt.validate()?;
        if let Some(existing) = self.parent_receipts.get(&context.generation)
            && existing != &receipt
        {
            return Err(NativeSourceError::ConflictingReceipt);
        }
        self.parent_receipts.insert(context.generation, receipt);
        self.complete_ready_for_parent(context.generation)
    }

    pub fn record_child_receipt(
        &mut self,
        context: ChronicleExecutionContext,
        receipt: NativeOperationBoundaryReceipt,
    ) -> Result<Vec<NativeExecutionHandoffObservation>, NativeSourceError> {
        self.ensure_context(context)?;
        receipt.validate()?;
        let Some(pending) = self.pending.get_mut(&context.generation) else {
            if self.completed.contains(&context.generation) {
                return Ok(Vec::new());
            }
            return Err(NativeSourceError::UnknownContext);
        };
        if let Some(existing) = &pending.child_receipt
            && existing != &receipt
        {
            return Err(NativeSourceError::ConflictingReceipt);
        }
        pending.child_receipt = Some(receipt);
        self.complete_ready_for_child(context.generation)
    }

    pub fn expire_pending(
        &mut self,
        context_generation: NativeExecutionContextGeneration,
    ) -> Vec<NativeObservationDiagnostic> {
        let keys: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| {
                pending.parent_generation == context_generation
                    || pending.child_generation == context_generation
            })
            .map(|(key, _)| *key)
            .collect();
        let mut diagnostics = Vec::new();
        for key in keys {
            self.pending.remove(&key);
            diagnostics.push(self.record_diagnostic(
                NativeObservationDiagnosticKind::PendingExpired,
                Some(context_generation),
                "native handoff receipt did not complete within bounded source lifecycle",
            ));
        }
        self.diagnostics.clear();
        diagnostics
    }

    pub fn restart(&mut self) -> Vec<NativeObservationDiagnostic> {
        let mut diagnostics = Vec::new();
        let pending: Vec<_> = self
            .pending
            .values()
            .map(|item| item.child_generation)
            .collect();
        for child_generation in pending {
            diagnostics.push(self.record_diagnostic(
                NativeObservationDiagnosticKind::RestartLostPending,
                Some(child_generation),
                "native pending handoff was lost during source restart",
            ));
        }
        self.contexts.clear();
        self.parent_receipts.clear();
        self.pending.clear();
        self.completed.clear();
        self.diagnostics.clear();
        diagnostics
    }

    pub fn take_diagnostics(&mut self) -> Vec<NativeObservationDiagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    fn ensure_context(&self, context: ChronicleExecutionContext) -> Result<(), NativeSourceError> {
        if self.contexts.get(&context.generation) == Some(&context) {
            Ok(())
        } else {
            Err(NativeSourceError::UnknownContext)
        }
    }

    fn complete_ready_for_parent(
        &mut self,
        parent_generation: NativeExecutionContextGeneration,
    ) -> Result<Vec<NativeExecutionHandoffObservation>, NativeSourceError> {
        let children: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.parent_generation == parent_generation)
            .map(|(child, _)| *child)
            .collect();
        let mut observations = Vec::new();
        for child in children {
            observations.extend(self.complete_ready_for_child(child)?);
        }
        Ok(observations)
    }

    fn complete_ready_for_child(
        &mut self,
        child_generation: NativeExecutionContextGeneration,
    ) -> Result<Vec<NativeExecutionHandoffObservation>, NativeSourceError> {
        let Some(pending) = self.pending.get(&child_generation) else {
            return Ok(Vec::new());
        };
        let Some(parent_receipt) = self.parent_receipts.get(&pending.parent_generation) else {
            return Ok(Vec::new());
        };
        let Some(child_receipt) = pending.child_receipt.clone() else {
            return Ok(Vec::new());
        };
        if self.completed.contains(&child_generation) {
            self.pending.remove(&child_generation);
            return Ok(Vec::new());
        }
        if self.completed.len() >= self.limits.max_pending_observations {
            return Err(NativeSourceError::PendingLimitExceeded {
                limit: self.limits.max_pending_observations,
            });
        }
        let parent_context = self
            .contexts
            .get(&pending.parent_generation)
            .copied()
            .ok_or(NativeSourceError::UnknownContext)?;
        let child_context = self
            .contexts
            .get(&pending.child_generation)
            .copied()
            .ok_or(NativeSourceError::UnknownContext)?;
        let parent = NativeOperationAnchor::new(parent_receipt.clone(), parent_context.generation)?;
        let child = NativeOperationAnchor::new(child_receipt, child_context.generation)?;
        let provenance = NativeObservationProvenance {
            source: "chronicle-native-execution-context".into(),
            parent_context_generation: parent_context.generation,
            child_context_generation: child_context.generation,
        };
        let observation = NativeExecutionHandoffObservation::new(parent, child, provenance)?;
        self.pending.remove(&child_generation);
        self.completed.insert(child_generation);
        Ok(vec![observation])
    }

    fn record_diagnostic(
        &mut self,
        kind: NativeObservationDiagnosticKind,
        context_generation: Option<NativeExecutionContextGeneration>,
        message: &str,
    ) -> NativeObservationDiagnostic {
        let diagnostic = NativeObservationDiagnostic {
            kind,
            context_generation,
            message: message.into(),
        };
        if self.diagnostics.len() < self.limits.max_diagnostics {
            self.diagnostics.push(diagnostic.clone());
        }
        diagnostic
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SourceRangeKey(u64, u64, u64, u64, u8, u64, u64, u64, bool);

fn source_range_key(range: &WalByteRange) -> SourceRangeKey {
    SourceRangeKey(
        range.segment_ordinal,
        range.segment_first_sequence,
        range.frame_byte_offset,
        range.wal_sequence,
        match range.direction {
            Direction::ClientToServer => 0,
            Direction::ServerToClient => 1,
        },
        range.payload_byte_offset,
        range.payload_byte_length,
        0,
        range.gap_before,
    )
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AnchorKey(
    RecordingId,
    EpochId,
    SourceConnectionGeneration,
    ProtocolId,
    u8,
    ProtocolOperationBoundaryIdentity,
    SourceRangeKey,
    NativeExecutionContextGeneration,
);

fn anchor_key(anchor: &NativeOperationAnchor) -> AnchorKey {
    AnchorKey(
        anchor.boundary.recording_id,
        anchor.boundary.source_epoch_id,
        anchor.boundary.source_generation.clone(),
        anchor.boundary.protocol_id.clone(),
        match anchor.boundary.direction {
            Direction::ClientToServer => 0,
            Direction::ServerToClient => 1,
        },
        anchor.boundary.protocol_operation_boundary.clone(),
        source_range_key(&anchor.boundary.source_range),
        anchor.context_generation,
    )
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ObservationKey(AnchorKey, AnchorKey, NativeObservationProvenance);

/// Bind complete pre-canonical observations against canonical sessions. The
/// candidate set is exact; no ordering, timing, or infrastructure fallback is
/// available.
pub fn bind_native_execution_observations(
    sessions: &[CanonicalSession],
    boundary_index: &NativeBoundaryIndex,
    observations: &[NativeExecutionHandoffObservation],
) -> NativeBindingOutput {
    let mut facts: BTreeMap<ObservationKey, BoundNativeExecutionHandoffFact> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for observation in observations {
        let parent = bind_native_anchor(sessions, boundary_index, &observation.parent);
        let child = bind_native_anchor(sessions, boundary_index, &observation.child);
        let (parent, child) = match (parent, child) {
            (NativeAnchorBinding::Bound(parent), NativeAnchorBinding::Bound(child)) => {
                (parent, child)
            }
            (
                NativeAnchorBinding::Unresolved(diagnostic)
                | NativeAnchorBinding::Ambiguous(diagnostic),
                _,
            )
            | (
                _,
                NativeAnchorBinding::Unresolved(diagnostic)
                | NativeAnchorBinding::Ambiguous(diagnostic),
            ) => {
                diagnostics.push(diagnostic);
                continue;
            }
        };
        if parent.recording_id != child.recording_id || parent == child {
            diagnostics.push(NativeBindingDiagnostic {
                kind: NativeBindingDiagnosticKind::InvalidBoundHandoff,
                anchor: None,
                candidates: vec![parent, child],
                message: "native handoff endpoints must be distinct and share recording scope"
                    .into(),
            });
            continue;
        }
        let fact = BoundNativeExecutionHandoffFact {
            parent,
            child,
            relation: observation.relation,
            parent_anchor: observation.parent.clone(),
            child_anchor: observation.child.clone(),
            provenance: observation.provenance.clone(),
        };
        facts
            .entry(ObservationKey(
                anchor_key(&fact.parent_anchor),
                anchor_key(&fact.child_anchor),
                fact.provenance.clone(),
            ))
            .or_insert(fact);
    }
    diagnostics.sort_by_key(|diagnostic| {
        (
            diagnostic.kind,
            diagnostic.candidates.clone(),
            diagnostic.message.clone(),
        )
    });
    NativeBindingOutput {
        facts: facts.into_values().collect(),
        diagnostics,
    }
}

fn bind_native_anchor(
    sessions: &[CanonicalSession],
    boundary_index: &NativeBoundaryIndex,
    anchor: &NativeOperationAnchor,
) -> NativeAnchorBinding {
    if let Err(error) = anchor.boundary.validate() {
        return NativeAnchorBinding::Unresolved(NativeBindingDiagnostic {
            kind: NativeBindingDiagnosticKind::AnchorBindingUnresolved,
            anchor: Some(anchor.clone()),
            candidates: Vec::new(),
            message: error.to_string(),
        });
    }
    let mut candidates = Vec::new();
    for session in sessions {
        if session.source_provenance.recording_id != Some(anchor.boundary.recording_id) {
            continue;
        }
        for connection in &session.connections {
            if connection.protocol != anchor.boundary.protocol_id {
                continue;
            }
            for operation in &connection.operations {
                let Some(owner_epoch_id) = operation.provenance.completion_owner_epoch else {
                    continue;
                };
                let reference = CanonicalOperationRef::new(
                    anchor.boundary.recording_id,
                    owner_epoch_id,
                    session.id,
                    operation.id,
                );
                if boundary_index.boundary_for(&reference)
                    != Some(&anchor.boundary.protocol_operation_boundary)
                {
                    continue;
                }
                if operation.provenance.connection_generation.as_ref()
                    != Some(&anchor.boundary.source_generation)
                {
                    continue;
                }
                if !operation
                    .provenance
                    .epoch_ranges
                    .iter()
                    .any(|range| range.epoch_id == anchor.boundary.source_epoch_id)
                {
                    continue;
                }
                if !operation
                    .provenance
                    .wal_ranges
                    .iter()
                    .any(|range| range == &anchor.boundary.source_range)
                {
                    continue;
                }
                if reference.resolve_in_session(session).is_ok() {
                    candidates.push(reference);
                }
            }
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.as_slice() {
        [reference] => NativeAnchorBinding::Bound(*reference),
        [] => NativeAnchorBinding::Unresolved(NativeBindingDiagnostic {
            kind: NativeBindingDiagnosticKind::AnchorBindingUnresolved,
            anchor: Some(anchor.clone()),
            candidates,
            message: "no canonical operation matches complete native anchor".into(),
        }),
        _ => NativeAnchorBinding::Ambiguous(NativeBindingDiagnostic {
            kind: NativeBindingDiagnosticKind::AnchorBindingAmbiguous,
            anchor: Some(anchor.clone()),
            candidates,
            message: "multiple canonical operations match complete native anchor".into(),
        }),
    }
}

/// Convert bound facts into child-side correlation evidence. This function does
/// not select scenarios, assign confidence, or construct graph edges.
pub fn native_evidence_by_operation(
    facts: &[BoundNativeExecutionHandoffFact],
) -> BTreeMap<CanonicalOperationRef, Vec<CorrelationEvidence>> {
    let mut evidence: BTreeMap<CanonicalOperationRef, Vec<CorrelationEvidence>> = BTreeMap::new();
    for fact in facts {
        let item = CorrelationEvidence::with_provenance(
            CorrelationEvidenceKind::NativeExecutionLineage {
                parent: fact.parent,
                relation: fact.relation,
            },
            chronicle_canonical::EvidenceProvenance {
                source: fact.provenance.source.clone(),
                observation: Some(format!(
                    "{}:{}",
                    fact.provenance.parent_context_generation.as_uuid(),
                    fact.provenance.child_context_generation.as_uuid()
                )),
            },
        );
        let entries = evidence.entry(fact.child).or_default();
        if !entries.contains(&item) {
            entries.push(item);
        }
    }
    for entries in evidence.values_mut() {
        entries.sort_by_key(|item| format!("{item:?}"));
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_canonical::WalByteRange;

    fn limits() -> NativeSourceLimits {
        NativeSourceLimits::new(8, 8, 8).unwrap()
    }

    fn receipt(
        context: NativeExecutionContextGeneration,
        sequence: u64,
    ) -> NativeOperationBoundaryReceipt {
        let _ = context;
        NativeOperationBoundaryReceipt {
            recording_id: RecordingId::from_uuid(Uuid::from_u128(1)),
            source_epoch_id: EpochId::from_uuid(Uuid::from_u128(2)),
            source_generation: SourceConnectionGeneration::Fixture,
            protocol_id: ProtocolId::new("http/1.1"),
            direction: Direction::ClientToServer,
            protocol_operation_boundary: ProtocolOperationBoundaryIdentity::from_canonicalizer(
                ProtocolId::new("http/1.1"),
                format!("request:{sequence}"),
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
    fn child_before_parent_completes_once_after_both_receipts() {
        let mut carrier = NativeExecutionContextCarrier::new(limits());
        let parent = carrier.start_root().unwrap();
        let child = carrier.continue_from(parent).unwrap();
        assert!(
            carrier
                .record_child_receipt(child, receipt(child.generation, 2))
                .unwrap()
                .is_empty()
        );
        let observations = carrier
            .record_parent_receipt(parent, receipt(parent.generation, 1))
            .unwrap();
        assert_eq!(observations.len(), 1);
        assert!(
            carrier
                .record_child_receipt(child, receipt(child.generation, 2))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn parent_before_child_completes_when_child_receipt_arrives_later() {
        let mut carrier = NativeExecutionContextCarrier::new(limits());
        let parent = carrier.start_root().unwrap();
        let child = carrier.continue_from(parent).unwrap();
        assert!(
            carrier
                .record_parent_receipt(parent, receipt(parent.generation, 1))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            carrier
                .record_child_receipt(child, receipt(child.generation, 2))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn incomplete_parent_expires_without_positive_relation() {
        let mut carrier = NativeExecutionContextCarrier::new(limits());
        let parent = carrier.start_root().unwrap();
        let child = carrier.continue_from(parent).unwrap();
        carrier
            .record_child_receipt(child, receipt(child.generation, 2))
            .unwrap();
        let diagnostics = carrier.expire_pending(parent.generation);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].kind,
            NativeObservationDiagnosticKind::PendingExpired
        );
    }

    #[test]
    fn incomplete_child_expires_without_positive_relation() {
        let mut carrier = NativeExecutionContextCarrier::new(limits());
        let parent = carrier.start_root().unwrap();
        let child = carrier.continue_from(parent).unwrap();
        carrier
            .record_parent_receipt(parent, receipt(parent.generation, 1))
            .unwrap();
        let diagnostics = carrier.expire_pending(child.generation);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].kind,
            NativeObservationDiagnosticKind::PendingExpired
        );
    }

    #[test]
    fn restart_discards_pending_state_fail_closed() {
        let mut carrier = NativeExecutionContextCarrier::new(limits());
        let parent = carrier.start_root().unwrap();
        let child = carrier.continue_from(parent).unwrap();
        carrier
            .record_child_receipt(child, receipt(child.generation, 2))
            .unwrap();
        let delivery = carrier.restart();
        assert_eq!(delivery.len(), 1);
        assert_eq!(
            delivery[0].kind,
            NativeObservationDiagnosticKind::RestartLostPending
        );
    }

    #[test]
    fn duplicate_completion_is_idempotent() {
        let mut carrier = NativeExecutionContextCarrier::new(limits());
        let parent = carrier.start_root().unwrap();
        let child = carrier.continue_from(parent).unwrap();
        carrier
            .record_parent_receipt(parent, receipt(parent.generation, 1))
            .unwrap();
        assert_eq!(
            carrier
                .record_child_receipt(child, receipt(child.generation, 2))
                .unwrap()
                .len(),
            1
        );
        assert!(
            carrier
                .record_parent_receipt(parent, receipt(parent.generation, 1))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn multiple_pending_children_remain_independent() {
        let mut carrier = NativeExecutionContextCarrier::new(limits());
        let parent = carrier.start_root().unwrap();
        let child_a = carrier.continue_from(parent).unwrap();
        let child_b = carrier.continue_from(parent).unwrap();
        carrier
            .record_child_receipt(child_a, receipt(child_a.generation, 2))
            .unwrap();
        carrier
            .record_child_receipt(child_b, receipt(child_b.generation, 3))
            .unwrap();
        let observations = carrier
            .record_parent_receipt(parent, receipt(parent.generation, 1))
            .unwrap();
        assert_eq!(observations.len(), 2);
        assert_ne!(
            observations[0].child.context_generation,
            observations[1].child.context_generation
        );
    }

    #[test]
    fn side_channel_overflow_fails_closed() {
        let mut carrier = NativeExecutionContextCarrier::new(limits());
        let parent = carrier.start_root().unwrap();
        let child = carrier.continue_from(parent).unwrap();
        carrier
            .record_child_receipt(child, receipt(child.generation, 2))
            .unwrap();
        let observation = carrier
            .record_parent_receipt(parent, receipt(parent.generation, 1))
            .unwrap()
            .pop()
            .unwrap();
        let mut channel = BoundedNativeObservationChannel::new(1).unwrap();
        channel.push(observation.clone()).unwrap();
        assert!(matches!(
            channel.push(observation),
            Err(NativeSourceError::SideChannelLimitExceeded { limit: 1 })
        ));
    }

    #[test]
    fn runtime_supplied_boundary_is_rejected() {
        let mut value = receipt(NativeExecutionContextGeneration::default(), 1);
        value.protocol_operation_boundary = ProtocolOperationBoundaryIdentity::new(
            ProtocolId::new("http/1.1"),
            "runtime-counter:7",
        )
        .unwrap();
        assert!(value.validate().is_err());
    }
}
