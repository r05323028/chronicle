use chronicle_canonical::{
    CanonicalOperation, CanonicalOperationRef, CanonicalSession, CorrelationEvidence,
    CorrelationEvidenceKind, ExecutionContinuation, SourceConnectionGeneration, WalByteRange,
};
use chronicle_common::{Direction, EpochId, ProtocolId, RecordingId};
use chronicle_protocol::{ProtocolOperationBoundaryClaim, ProtocolRegistry};
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

    #[cfg(test)]
    pub(crate) const fn from_uuid(value: Uuid) -> Self {
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChronicleExecutionContext {
    generation: NativeExecutionContextGeneration,
    parent_generation: Option<NativeExecutionContextGeneration>,
}

impl ChronicleExecutionContext {
    pub(crate) fn root() -> Self {
        Self {
            generation: NativeExecutionContextGeneration::new(),
            parent_generation: None,
        }
    }

    pub const fn generation(self) -> NativeExecutionContextGeneration {
        self.generation
    }

    pub(crate) fn child(self) -> Self {
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
    #[error("native context still has pending handoffs")]
    ContextHasPendingHandoffs,
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
    /// Opaque protocol-local claim; trust is established only by ETL's
    /// registry-derived boundary index.
    pub protocol_operation_boundary: ProtocolOperationBoundaryClaim,
    pub source_range: WalByteRange,
}

impl NativeOperationBoundaryReceipt {
    pub fn validate(&self) -> Result<(), NativeSourceError> {
        if self.protocol_id != *self.protocol_operation_boundary.protocol() {
            return Err(NativeSourceError::InvalidReceipt(
                "protocol identity disagrees with protocol-owned boundary claim".into(),
            ));
        }
        if self.protocol_operation_boundary.direction() != self.direction {
            return Err(NativeSourceError::InvalidReceipt(
                "boundary claim direction disagrees with receipt direction".into(),
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
    pub(crate) boundary: NativeOperationBoundaryReceipt,
    pub(crate) context_generation: NativeExecutionContextGeneration,
}

impl NativeOperationAnchor {
    pub(crate) fn new(
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
    pub(crate) parent: NativeOperationAnchor,
    pub(crate) child: NativeOperationAnchor,
    pub(crate) relation: ExecutionContinuation,
    pub(crate) provenance: NativeObservationProvenance,
}

impl NativeExecutionHandoffObservation {
    pub(crate) fn new(
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundNativeExecutionHandoffFact {
    parent: CanonicalOperationRef,
    child: CanonicalOperationRef,
    relation: ExecutionContinuation,
    parent_anchor: NativeOperationAnchor,
    child_anchor: NativeOperationAnchor,
    provenance: NativeObservationProvenance,
}

impl BoundNativeExecutionHandoffFact {
    pub const fn parent(&self) -> CanonicalOperationRef {
        self.parent
    }

    pub const fn child(&self) -> CanonicalOperationRef {
        self.child
    }

    pub const fn relation(&self) -> ExecutionContinuation {
        self.relation
    }

    pub fn parent_anchor(&self) -> &NativeOperationAnchor {
        &self.parent_anchor
    }

    pub fn child_anchor(&self) -> &NativeOperationAnchor {
        &self.child_anchor
    }

    pub fn provenance(&self) -> &NativeObservationProvenance {
        &self.provenance
    }
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

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct NativeBindingOutput {
    pub facts: Vec<BoundNativeExecutionHandoffFact>,
    pub diagnostics: Vec<NativeBindingDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NativeAnchorBinding {
    Bound(CanonicalOperationRef),
    Unresolved(NativeBindingDiagnostic),
    Ambiguous(NativeBindingDiagnostic),
}

#[derive(Clone, Debug, Default)]
pub struct NativeBoundaryIndex {
    /// Claims are trusted here only because `from_sessions` populated them
    /// through the registered canonicalizer. The index is not caller-writable.
    boundaries: BTreeMap<CanonicalOperationRef, ProtocolOperationBoundaryClaim>,
    unavailable: BTreeSet<CanonicalOperationRef>,
}

impl NativeBoundaryIndex {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) fn insert_fixture(
        &mut self,
        reference: CanonicalOperationRef,
        claim: ProtocolOperationBoundaryClaim,
    ) {
        self.boundaries.insert(reference, claim);
        self.unavailable.remove(&reference);
    }

    pub fn from_sessions(sessions: &[CanonicalSession], registry: &ProtocolRegistry) -> Self {
        let mut index = Self::new();
        for session in sessions {
            let Some(recording_id) = session.source_provenance.recording_id else {
                continue;
            };
            for connection in &session.connections {
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
                    let identity = registry
                        .trusted_operation_boundary_identity(&connection.protocol, operation);
                    match identity {
                        Some(identity) => {
                            index.boundaries.insert(reference, identity.claim());
                            index.unavailable.remove(&reference);
                        }
                        None => {
                            index.unavailable.insert(reference);
                        }
                    }
                }
            }
        }
        index
    }

    pub(crate) fn boundary_for(
        &self,
        reference: &CanonicalOperationRef,
    ) -> Option<&ProtocolOperationBoundaryClaim> {
        self.boundaries.get(reference)
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

    /// Admit complete batches only. Capacity is checked before any queue
    /// mutation, so overflow leaves zero positive observations enqueued.
    pub fn push_batch(
        &mut self,
        observations: &[NativeExecutionHandoffObservation],
    ) -> Result<(), NativeSourceError> {
        if observations.len() > self.max_observations.saturating_sub(self.queue.len()) {
            return Err(NativeSourceError::SideChannelLimitExceeded {
                limit: self.max_observations,
            });
        }
        self.queue.extend(observations.iter().cloned());
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

#[derive(Clone, Debug)]
struct CompletedHandoff {
    context: ChronicleExecutionContext,
    child_receipt: NativeOperationBoundaryReceipt,
}

/// Bounded source-side assembly for explicit parent/child handoffs. It accepts
/// receipts in either order and never emits a partial anchor.
#[derive(Clone, Debug)]
pub struct NativeExecutionContextCarrier {
    limits: NativeSourceLimits,
    /// Live contexts only. Completed child contexts move to bounded tombstones.
    contexts: BTreeMap<NativeExecutionContextGeneration, ChronicleExecutionContext>,
    /// Receipts retained for live contexts that may create more children.
    parent_receipts: BTreeMap<NativeExecutionContextGeneration, NativeOperationBoundaryReceipt>,
    /// Handoffs awaiting one or both endpoint receipts or delivery acknowledgement.
    pending: BTreeMap<NativeExecutionContextGeneration, PendingHandoff>,
    /// Bounded idempotency/continuation window for completed handoffs.
    completed: BTreeMap<NativeExecutionContextGeneration, CompletedHandoff>,
    completed_order: VecDeque<NativeExecutionContextGeneration>,
    diagnostics: Vec<NativeObservationDiagnostic>,
}

impl NativeExecutionContextCarrier {
    pub fn new(limits: NativeSourceLimits) -> Self {
        Self {
            limits,
            contexts: BTreeMap::new(),
            parent_receipts: BTreeMap::new(),
            pending: BTreeMap::new(),
            completed: BTreeMap::new(),
            completed_order: VecDeque::new(),
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
        let needs_activation = match self.contexts.get(&parent.generation) {
            Some(active) if active == &parent => false,
            Some(_) => return Err(NativeSourceError::UnknownContext),
            None => {
                if self
                    .completed
                    .get(&parent.generation)
                    .is_none_or(|completed| completed.context != parent)
                {
                    return Err(NativeSourceError::UnknownContext);
                }
                true
            }
        };
        if self.pending.len() >= self.limits.max_pending_observations {
            return Err(NativeSourceError::PendingLimitExceeded {
                limit: self.limits.max_pending_observations,
            });
        }
        let projected_contexts = self.contexts.len() + usize::from(needs_activation) + 1;
        if projected_contexts > self.limits.max_contexts {
            return Err(NativeSourceError::ContextLimitExceeded {
                limit: self.limits.max_contexts,
            });
        }
        if needs_activation {
            let Some(completed) = self.completed.get(&parent.generation) else {
                return Err(NativeSourceError::UnknownContext);
            };
            let child_receipt = completed.child_receipt.clone();
            self.contexts.insert(parent.generation, parent);
            self.parent_receipts
                .insert(parent.generation, child_receipt);
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
        receipt.validate()?;
        if let Some(completed) = self.completed.get(&context.generation) {
            if completed.child_receipt != receipt {
                return Err(NativeSourceError::ConflictingReceipt);
            }
            return Ok(Vec::new());
        }
        self.ensure_context(context)?;
        let Some(pending) = self.pending.get_mut(&context.generation) else {
            return Err(NativeSourceError::UnknownContext);
        };
        if let Some(existing) = self.parent_receipts.get(&context.generation)
            && existing != &receipt
        {
            return Err(NativeSourceError::ConflictingReceipt);
        }
        if let Some(existing) = &pending.child_receipt
            && existing != &receipt
        {
            return Err(NativeSourceError::ConflictingReceipt);
        }
        pending.child_receipt = Some(receipt.clone());
        self.parent_receipts.insert(context.generation, receipt);
        self.complete_ready_for_child(context.generation)
    }

    pub fn expire_pending(
        &mut self,
        context_generation: NativeExecutionContextGeneration,
    ) -> Vec<NativeObservationDiagnostic> {
        let mut generations = vec![context_generation];
        let mut keys = BTreeSet::new();
        while let Some(parent_generation) = generations.pop() {
            for (key, pending) in &self.pending {
                let touches_generation = pending.parent_generation == parent_generation
                    || pending.child_generation == parent_generation;
                if touches_generation
                    && keys.insert(*key)
                    && pending.parent_generation == parent_generation
                {
                    generations.push(pending.child_generation);
                }
            }
        }

        let mut diagnostics = Vec::new();
        for key in keys {
            let Some(pending) = self.pending.remove(&key) else {
                continue;
            };
            diagnostics.push(Self::diagnostic(
                NativeObservationDiagnosticKind::PendingExpired,
                Some(pending.child_generation),
                "native handoff receipt did not complete within bounded source lifecycle",
            ));
            if pending.child_generation != context_generation
                && !self
                    .pending
                    .values()
                    .any(|item| item.parent_generation == pending.child_generation)
            {
                self.contexts.remove(&pending.child_generation);
                self.parent_receipts.remove(&pending.child_generation);
            }
        }
        if !self
            .pending
            .values()
            .any(|item| item.parent_generation == context_generation)
            && !self
                .pending
                .keys()
                .any(|generation| *generation == context_generation)
        {
            self.contexts.remove(&context_generation);
            self.parent_receipts.remove(&context_generation);
        }
        diagnostics
    }

    pub fn restart(&mut self) -> Vec<NativeObservationDiagnostic> {
        let mut diagnostics = std::mem::take(&mut self.diagnostics);
        let pending: BTreeSet<_> = self
            .pending
            .values()
            .map(|item| item.child_generation)
            .collect();
        for child_generation in pending {
            diagnostics.push(Self::diagnostic(
                NativeObservationDiagnosticKind::RestartLostPending,
                Some(child_generation),
                "native pending handoff was lost during source restart",
            ));
        }
        self.contexts.clear();
        self.parent_receipts.clear();
        self.pending.clear();
        self.completed.clear();
        self.completed_order.clear();
        diagnostics
    }

    pub fn record_side_channel_loss(
        &mut self,
        observation_count: usize,
        channel_limit: usize,
    ) -> NativeObservationDiagnostic {
        self.record_diagnostic(
            NativeObservationDiagnosticKind::SideChannelLoss,
            None,
            &format!(
                "native side-channel batch of {observation_count} observations exceeds capacity {channel_limit}"
            ),
        )
    }

    pub fn report_restart_loss(&self, observation_count: usize) -> NativeObservationDiagnostic {
        Self::diagnostic(
            NativeObservationDiagnosticKind::SideChannelLoss,
            None,
            &format!(
                "native side-channel lost {observation_count} queued observations during restart"
            ),
        )
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
        if self.completed.contains_key(&child_generation) {
            return Ok(Vec::new());
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
        Ok(vec![observation])
    }

    /// Commit observations only after their complete batch has been admitted to
    /// the side channel. A failed admission leaves every pending observation
    /// retryable and changes no positive-delivery state.
    pub fn acknowledge_delivered(
        &mut self,
        observations: &[NativeExecutionHandoffObservation],
    ) -> Result<(), NativeSourceError> {
        let mut generations = BTreeSet::new();
        for observation in observations {
            let child_generation = observation.child.context_generation;
            if !generations.insert(child_generation) {
                return Err(NativeSourceError::InvalidReceipt(
                    "duplicate native delivery acknowledgement".into(),
                ));
            }
            let expected = self
                .complete_ready_for_child(child_generation)?
                .pop()
                .ok_or_else(|| {
                    NativeSourceError::InvalidReceipt(
                        "native observation is not pending delivery".into(),
                    )
                })?;
            if expected != *observation {
                return Err(NativeSourceError::InvalidReceipt(
                    "native delivery acknowledgement does not match pending observation".into(),
                ));
            }
        }

        for observation in observations {
            let child_generation = observation.child.context_generation;
            let pending = self.pending.remove(&child_generation).ok_or_else(|| {
                NativeSourceError::InvalidReceipt(
                    "native pending handoff disappeared during acknowledgement".into(),
                )
            })?;
            let child_context = self
                .contexts
                .get(&child_generation)
                .copied()
                .ok_or(NativeSourceError::UnknownContext)?;
            let parent_generation = pending.parent_generation;
            let child_receipt = pending.child_receipt.ok_or_else(|| {
                NativeSourceError::InvalidReceipt(
                    "native completed handoff has no child receipt".into(),
                )
            })?;
            self.remember_completed(child_context, child_receipt);
            self.retire_completed_context_if_idle(child_generation);
            self.retire_completed_context_if_idle(parent_generation);
        }
        Ok(())
    }

    /// Retire a live context once it can no longer produce or receive a
    /// handoff. Completed children are retired automatically after delivery;
    /// roots and contexts with future children use this explicit lifecycle end.
    pub fn retire_context(
        &mut self,
        context: ChronicleExecutionContext,
    ) -> Result<(), NativeSourceError> {
        self.ensure_context(context)?;
        if self
            .pending
            .keys()
            .any(|generation| *generation == context.generation)
            || self
                .pending
                .values()
                .any(|item| item.parent_generation == context.generation)
        {
            return Err(NativeSourceError::ContextHasPendingHandoffs);
        }
        self.contexts.remove(&context.generation);
        self.parent_receipts.remove(&context.generation);
        Ok(())
    }

    pub fn active_context_count(&self) -> usize {
        self.contexts.len()
    }

    pub fn pending_observation_count(&self) -> usize {
        self.pending.len()
    }

    pub fn completed_tombstone_count(&self) -> usize {
        self.completed.len()
    }

    pub fn retained_parent_receipt_count(&self) -> usize {
        self.parent_receipts.len()
    }

    fn retire_completed_context_if_idle(&mut self, generation: NativeExecutionContextGeneration) {
        if self.completed.contains_key(&generation)
            && !self
                .pending
                .values()
                .any(|item| item.parent_generation == generation)
        {
            self.contexts.remove(&generation);
            self.parent_receipts.remove(&generation);
        }
    }

    fn remember_completed(
        &mut self,
        context: ChronicleExecutionContext,
        child_receipt: NativeOperationBoundaryReceipt,
    ) {
        let generation = context.generation;
        if !self.completed.contains_key(&generation) {
            self.completed_order.push_back(generation);
        }
        self.completed.insert(
            generation,
            CompletedHandoff {
                context,
                child_receipt,
            },
        );
        while self.completed.len() > self.limits.max_pending_observations {
            let Some(oldest) = self.completed_order.pop_front() else {
                break;
            };
            self.completed.remove(&oldest);
        }
    }

    fn diagnostic(
        kind: NativeObservationDiagnosticKind,
        context_generation: Option<NativeExecutionContextGeneration>,
        message: &str,
    ) -> NativeObservationDiagnostic {
        NativeObservationDiagnostic {
            kind,
            context_generation,
            message: message.into(),
        }
    }

    fn record_diagnostic(
        &mut self,
        kind: NativeObservationDiagnosticKind,
        context_generation: Option<NativeExecutionContextGeneration>,
        message: &str,
    ) -> NativeObservationDiagnostic {
        let diagnostic = Self::diagnostic(kind, context_generation, message);
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
    ProtocolOperationBoundaryClaim,
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

fn binding_diagnostic_key(
    diagnostic: &NativeBindingDiagnostic,
) -> (
    NativeBindingDiagnosticKind,
    Option<AnchorKey>,
    Vec<CanonicalOperationRef>,
    String,
) {
    (
        diagnostic.kind,
        diagnostic.anchor.as_ref().map(anchor_key),
        diagnostic.candidates.clone(),
        diagnostic.message.clone(),
    )
}

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
        let parent_binding = bind_native_anchor(sessions, boundary_index, &observation.parent);
        let child_binding = bind_native_anchor(sessions, boundary_index, &observation.child);
        let (parent, child) = match (parent_binding, child_binding) {
            (NativeAnchorBinding::Bound(parent), NativeAnchorBinding::Bound(child)) => {
                (parent, child)
            }
            (parent, child) => {
                for binding in [parent, child] {
                    if let NativeAnchorBinding::Unresolved(diagnostic)
                    | NativeAnchorBinding::Ambiguous(diagnostic) = binding
                    {
                        diagnostics.push(diagnostic);
                    }
                }
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
    diagnostics.sort_by_key(binding_diagnostic_key);
    NativeBindingOutput {
        facts: facts.into_values().collect(),
        diagnostics,
    }
}

fn exact_source_placement(
    operation: &CanonicalOperation,
    recording_id: RecordingId,
    source_epoch_id: EpochId,
    source_range: &WalByteRange,
) -> bool {
    let mut wal_matches = operation
        .provenance
        .wal_ranges
        .iter()
        .filter(|range| *range == source_range);
    let exact_wal_match = wal_matches.next().is_some() && wal_matches.next().is_none();
    if !exact_wal_match {
        return false;
    }

    let mut epoch_matches = operation
        .provenance
        .epoch_ranges
        .iter()
        .filter(|epoch_range| {
            epoch_range.parent_id == Some(recording_id)
                && epoch_range.wal_sequence_range.is_some_and(|(start, end)| {
                    start <= source_range.wal_sequence && source_range.wal_sequence <= end
                })
        });
    epoch_matches.next().is_some_and(|epoch_range| {
        epoch_range.epoch_id == source_epoch_id && epoch_matches.next().is_none()
    })
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
                let Some(identity) = boundary_index.boundary_for(&reference) else {
                    continue;
                };
                if identity != &anchor.boundary.protocol_operation_boundary
                    || identity.direction() != anchor.boundary.direction
                {
                    continue;
                }
                if operation.provenance.connection_generation.as_ref()
                    != Some(&anchor.boundary.source_generation)
                {
                    continue;
                }
                if !exact_source_placement(
                    operation,
                    anchor.boundary.recording_id,
                    anchor.boundary.source_epoch_id,
                    &anchor.boundary.source_range,
                ) {
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
pub(crate) fn native_evidence_by_operation(
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
        entries.sort_by_key(native_evidence_key);
    }
    evidence
}

fn native_evidence_key(
    item: &CorrelationEvidence,
) -> (
    CanonicalOperationRef,
    ExecutionContinuation,
    String,
    Option<String>,
) {
    let CorrelationEvidenceKind::NativeExecutionLineage { parent, relation } = &item.kind else {
        unreachable!("native evidence map contains only native lineage items");
    };
    (
        *parent,
        *relation,
        item.provenance.source.clone(),
        item.provenance.observation.clone(),
    )
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
            protocol_operation_boundary: ProtocolOperationBoundaryClaim::new(
                ProtocolId::new("http/1.1"),
                format!("request:{sequence}"),
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
        carrier.acknowledge_delivered(&observations).unwrap();
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
        let observations = carrier
            .record_child_receipt(child, receipt(child.generation, 2))
            .unwrap();
        assert_eq!(observations.len(), 1);
        carrier.acknowledge_delivered(&observations).unwrap();
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
        let observations = carrier
            .record_child_receipt(child, receipt(child.generation, 2))
            .unwrap();
        assert_eq!(observations.len(), 1);
        carrier.acknowledge_delivered(&observations).unwrap();
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
        carrier.acknowledge_delivered(&observations).unwrap();
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
        channel
            .push_batch(std::slice::from_ref(&observation))
            .unwrap();
        assert!(matches!(
            channel.push_batch(std::slice::from_ref(&observation)),
            Err(NativeSourceError::SideChannelLimitExceeded { limit: 1 })
        ));
    }

    fn ready_observations(count: usize) -> Vec<NativeExecutionHandoffObservation> {
        let limits = NativeSourceLimits::new(
            count.saturating_add(2),
            count.saturating_add(1),
            count.saturating_add(2),
        )
        .unwrap();
        let mut carrier = NativeExecutionContextCarrier::new(limits);
        let parent = carrier.start_root().unwrap();
        let mut children = Vec::new();
        for index in 0..count {
            let child = carrier.continue_from(parent).unwrap();
            carrier
                .record_child_receipt(child, receipt(child.generation, index as u64 + 2))
                .unwrap();
            children.push(child);
        }
        let observations = carrier
            .record_parent_receipt(parent, receipt(parent.generation, 1))
            .unwrap();
        assert_eq!(observations.len(), children.len());
        observations
    }

    #[test]
    fn batch_admits_exact_capacity() {
        let observations = ready_observations(3);
        let mut channel = BoundedNativeObservationChannel::new(3).unwrap();
        channel.push_batch(&observations).unwrap();
        assert_eq!(channel.len(), 3);
        assert_eq!(channel.drain(), observations);
    }

    #[test]
    fn batch_overflow_admits_no_observation() {
        let observations = ready_observations(3);
        let mut channel = BoundedNativeObservationChannel::new(2).unwrap();
        assert!(matches!(
            channel.push_batch(&observations),
            Err(NativeSourceError::SideChannelLimitExceeded { limit: 2 })
        ));
        assert!(channel.is_empty());
    }

    #[test]
    fn long_chain_retires_active_state_and_reuses_limits() {
        let limits = NativeSourceLimits::new(3, 2, 8).unwrap();
        let mut carrier = NativeExecutionContextCarrier::new(limits);
        let root = carrier.start_root().unwrap();
        let mut current = root;
        for index in 0..64_u64 {
            let child = carrier.continue_from(current).unwrap();
            carrier
                .record_child_receipt(child, receipt(child.generation, index + 2))
                .unwrap();
            let observations = carrier
                .record_parent_receipt(current, receipt(current.generation, index + 1))
                .unwrap();
            assert_eq!(observations.len(), 1);
            carrier.acknowledge_delivered(&observations).unwrap();
            assert!(carrier.active_context_count() <= limits.max_contexts);
            assert!(carrier.pending_observation_count() <= limits.max_pending_observations);
            assert!(carrier.completed_tombstone_count() <= limits.max_pending_observations);
            assert!(carrier.retained_parent_receipt_count() <= limits.max_contexts);
            current = child;
        }
        carrier.retire_context(root).unwrap();
        assert_eq!(carrier.active_context_count(), 0);
        assert_eq!(carrier.pending_observation_count(), 0);
    }

    #[test]
    fn expired_tombstones_reject_old_context_generation() {
        let limits = NativeSourceLimits::new(2, 2, 8).unwrap();
        let mut carrier = NativeExecutionContextCarrier::new(limits);
        let root = carrier.start_root().unwrap();
        carrier
            .record_parent_receipt(root, receipt(root.generation, 1))
            .unwrap();
        let mut children = Vec::new();
        for sequence in 2..=4 {
            let child = carrier.continue_from(root).unwrap();
            let observations = carrier
                .record_child_receipt(child, receipt(child.generation, sequence))
                .unwrap();
            carrier.acknowledge_delivered(&observations).unwrap();
            children.push(child);
        }
        assert_eq!(carrier.completed_tombstone_count(), 2);
        assert!(matches!(
            carrier.continue_from(children[0]),
            Err(NativeSourceError::UnknownContext)
        ));
        assert!(matches!(
            carrier.record_child_receipt(children[0], receipt(children[0].generation, 2)),
            Err(NativeSourceError::UnknownContext)
        ));
    }

    #[test]
    fn runtime_claim_carries_no_authority() {
        let mut value = receipt(NativeExecutionContextGeneration::default(), 1);
        value.protocol_operation_boundary = ProtocolOperationBoundaryClaim::new(
            ProtocolId::new("http/1.1"),
            "runtime-counter:7",
            Direction::ClientToServer,
        )
        .unwrap();
        assert!(value.validate().is_ok());
    }

    #[test]
    fn serialized_receipt_cannot_deserialize_trusted_authority() {
        let encoded =
            serde_json::to_string(&receipt(NativeExecutionContextGeneration::default(), 1))
                .unwrap();
        assert!(!encoded.contains("authority"));
        assert!(encoded.contains("protocol_operation_boundary"));
    }
}
