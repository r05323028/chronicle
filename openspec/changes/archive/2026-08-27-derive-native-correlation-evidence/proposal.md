## Why

PR #6 made correlation selection deterministic and conservative, but the existing native-evidence proposal skipped a required production boundary: runtime code cannot name a future `CanonicalOperationRef`. Current Chronicle code creates those references only after reconstruction, protocol canonicalization, ETL operation-ID normalization, session publication, and completion-owner assignment. A direct runtime-to-reference handoff would therefore be fixture-only or would smuggle guessed identity into causal selection.

This change remains named `derive-native-correlation-evidence` because it now includes the real observation-to-canonical binding pipeline. The first production source is explicitly opt-in: a Chronicle cooperative execution-context carrier plus a Chronicle transport/application boundary receipt. Passive recording remains conservative when that cooperation is absent.

## What Changes

- Define the complete pipeline: runtime execution → pre-canonical native handoff observation → generation-safe `NativeOperationAnchor` → exact anchor-to-canonical binding → bound handoff fact → `NativeExecutionLineage` → existing correlation resolver.
- Introduce separate semantic contracts for pre-canonical observation, operation anchor, binding result/diagnostic, bound handoff fact, and canonical correlation evidence. Runtime producers never construct future canonical references.
- Select a concrete first source: an opt-in Chronicle-native execution-context carrier in an application/runtime integration, paired with a transport boundary adapter that supplies exact protocol-local operation position and source/reconstruction lineage.
- Require the smallest new non-frozen boundary fact current code lacks: a generation-safe operation-boundary receipt connecting the cooperative context handoff to `SourceConnectionGeneration`, protocol-local operation position, recording/epoch placement, and exact source provenance. Current `SourceConnectionGeneration`, `DecodedFrame` provenance, protocol sequence, `CanonicalOperation.provenance`, and `CanonicalOperationRef` scopes remain authoritative where already available.
- Bind anchors against canonical sessions only by exact composite identity. Zero matches and multiple matches produce typed binding diagnostics and no positive relation. Binding ambiguity is never materialized as resolver causal ambiguity.
- Keep explicit `ExecutionContinuation` as the only first-slice native predicate. No PID/TID/task/socket/connection/stream equality, timing, proximity, protocol kind, role, processing order, WAL order, active-ingress fallback, or absence-of-contradiction rule is authorized.
- Feed bound native facts into existing ETL composition and the existing resolver only. Preserve Phase A1 sparse closure, Phase A2 outcome/witness derivation, and Phase B pre-filter → unique-parent → complete provisional graph → SCC → internal-edge removal.
- Keep native and trace evidence as additive positive support with no weights, priority, or negative voting. Genuine causal conflict remains `Ambiguous`; unresolved binding emits no evidence.
- Specify passive-only and cooperative-native capability boundaries, restart/loss behavior, bounded state properties, composition proof, and real native production E2E proof separately.
- Remove arbitrary normative fact-count constants. Require bounded state/work, typed overflow or bounded-loss diagnostics, no silent positive truncation, and no unbounded ancestry history; measured defaults remain implementation work.

## Capabilities

### New Capabilities

- `native-correlation-evidence`: captures explicit pre-canonical Chronicle-native handoffs, binds generation-safe anchors exactly to canonical operations, and derives bounded native evidence.

### Modified Capabilities

- `correlation-domain-model`: defines valid `NativeExecutionLineage` evidence and separates binding failures from graph-level causal ambiguity.
- `correlation-resolver`: admits bound native execution continuation into existing support, witness, parent, and SCC phases without a second selector.

## Impact

Expected implementation ownership remains within existing dependency direction: `chronicle-application` owns opt-in cooperative source wiring; `chronicle-etl` owns the non-frozen observation/anchor contract, exact binding, bound-fact normalization, and evidence derivation; `chronicle-canonical` owns evidence semantics and resolver predicates; `chronicle-session` and `chronicle-protocol` expose existing reconstruction/protocol-local facts but do not choose ownership; CLI remains unaware. No new crate or dependency edge is proposed.

The current passive recorder and eBPF adapter do not prove application execution causality and are unchanged. `chronicle record -- app` without cooperative observations and without trace evidence remains conservative, with non-root work generally `Uncorrelated`. Cooperative mode requires application/runtime integration and a Chronicle transport boundary receipt; it does not require OpenTelemetry or any provider SDK.

Capture Event v1, WAL v1, Canonical Session v1, Session Manifest, persisted checkpoints, public CLI JSON, replay-safety contracts, and the default `chronicle` dependency closure remain unchanged. Native observations are a separate non-frozen runtime/composition input in this change. Durable side-channel persistence and versioning are deferred; loss or restart without recoverable observations fails closed rather than fabricating relations. No `EventId`, scenario graph persistence, or provider type is introduced.
