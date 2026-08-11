## Context

Chronicle currently has a sound acyclic crate graph and clear runtime flow, but no complete enforceable policy. Existing `docs/architecture.md` describes a high-level pipeline, archived eBPF OpenSpec work protects one adapter direction, and `scripts/validate.sh fast` checks source ownership. None enumerates every crate owner or rejects a new valid-Cargo but architecturally forbidden edge.

Review used current working-tree source because active `user-intent-cli` work is already moving command record/replay orchestration. The checkout also contains unrelated user changes; implementation of this architecture change must preserve and rebase around them rather than overwrite them.

### Current dependency graph

Root `Cargo.toml` admits `crates/*`, yielding 13 Chronicle workspace crates. `ebpf/` and `ebpf-feasibility/` are intentionally separate workspaces and have no Chronicle path dependencies. `cargo metadata --format-version 1 --no-deps` plus manifest review shows no cycle, no workspace build dependency, 43 normal path-dependency declarations, and 4 dev declarations.

Current normal edges, grouped by dependent crate:

```text
chronicle-common -> {}
chronicle-canonical -> {chronicle-common}
chronicle-capture -> {chronicle-common}
chronicle-capture-ebpf -> {chronicle-capture, chronicle-common}
chronicle-wal -> {chronicle-capture, chronicle-common}
chronicle-session -> {chronicle-capture, chronicle-common, chronicle-wal}
chronicle-protocol -> {chronicle-canonical, chronicle-common, chronicle-session}
chronicle-protocol-builtins -> {chronicle-canonical, chronicle-common, chronicle-protocol}
chronicle-storage -> {chronicle-canonical, chronicle-common}
chronicle-replay -> {chronicle-canonical, chronicle-common, chronicle-protocol}
chronicle-etl -> {
  chronicle-canonical,
  chronicle-capture,
  chronicle-common,
  chronicle-protocol,
  chronicle-session,
  chronicle-storage,
  chronicle-wal
}
chronicle-application -> {
  chronicle-canonical,
  chronicle-capture,
  chronicle-capture-ebpf [optional; cfg(target_os = "linux")],
  chronicle-common,
  chronicle-etl,
  chronicle-protocol,
  chronicle-protocol-builtins,
  chronicle-replay,
  chronicle-session,
  chronicle-storage,
  chronicle-wal
}
chronicle-cli -> {
  chronicle-application,
  chronicle-common,
  chronicle-protocol,
  chronicle-protocol-builtins,
  chronicle-replay
}
```

Current dev-only declarations:

```text
chronicle-application -> chronicle-wal [test-support feature; also a normal dependency]
chronicle-cli -> chronicle-capture
chronicle-cli -> chronicle-wal
chronicle-etl -> chronicle-protocol-builtins
```

### Current ownership and coupling

Healthy boundaries:

- `chronicle-common` is a dependency leaf.
- `chronicle-canonical` depends only on common primitives.
- eBPF raw ABI and Aya handles remain private to `chronicle-capture-ebpf`; normalized `CaptureEvent` is the outward boundary.
- protocol built-ins depend on the protocol SPI, never reverse.
- storage depends only on canonical/common and hides filesystem mechanics inside storage implementations.
- replay depends on canonical/common/protocol only.
- ETL already owns extraction/transformation plus incremental publication helpers and correctly depends on storage.
- application is the composition root; CLI already routes WAL and ETL work through application services.

Boundaries needing correction or clarification:

- one-shot final session publication and recording-local checkpoint sequencing still occur directly in application through `FilesystemSessionStore`, while incremental publication helpers live in ETL. Target ETL ownership is therefore incomplete even though the dependency direction is healthy.
- `chronicle-session` imports concrete `chronicle_wal::TerminalWalLoss`, so reconstruction cannot be tested or reused independently from WAL implementation.
- CLI normal dependencies/imports expose replay options/outcomes and protocol errors directly; exit mapping and rendering therefore cross the application seam.
- CLI dev dependencies build WAL/capture fixtures directly, weakening the stated “application boundary only” rule.
- application root is large and re-exports low-level recorder, quota, transition, scope, and persistence primitives alongside use-case APIs.
- WAL public API spans durability, manifest, retention, and test fault controls. This change documents/narrows ownership where needed but does not split the crate or redesign retention authority.
- storage exposes three related API families (`FilesystemSessionStore`, `RecordingStore`, generic metadata/artifact traits). This change documents their ownership; it does not create another abstraction.
- legacy names such as WAL provenance remain in serialized checkpoint/canonical contracts. They must not be renamed if doing so changes persisted formats.

Boundaries that must not change:

- WAL commit markers remain recovery authority; manifests remain descriptive/rebuildable.
- ETL keeps storage publication and checkpoint-ordering responsibilities.
- canonical/WAL/checkpoint/CLI contracts remain byte- and behavior-compatible.
- Linux eBPF remains optional and platform-gated.
- replay safety remains default-deny and independent from capture/WAL.
- no new crate or backend appears.

## Goals / Non-Goals

**Goals:**

- Give every Chronicle crate one primary responsibility and explicit forbidden knowledge.
- Define allowed normal/dev/build workspace edges, including optional and target-specific declarations.
- Reject unclassified or forbidden workspace edges in fast/release CI validation.
- Remove concrete WAL types from session reconstruction.
- Make application use-case ownership visible and public exports intentional.
- Make CLI depend only on application for Chronicle APIs.
- Put durable agent guidance in `AGENTS.md` and detailed rationale in architecture docs.
- Preserve current product and persisted behavior.

**Non-Goals:**

- New crate splits, generic architecture frameworks, generated docs, or third-party dependency tools.
- Transform-only ETL or moving load/publication out of ETL.
- WAL, canonical, checkpoint, recording, storage, CLI output, or protocol schema changes.
- Refactoring every broad public API solely for aesthetic purity.
- Privileged Linux acceptance for a portable manifest-graph rule.

## Decisions

### 1. Layering is an allowlist, not a numeric “lower layer may depend on anything below” rule

`validation/architecture.toml` will list allowed Chronicle dependencies for each source crate and dependency kind. Unlisted workspace edges are forbidden. Critical forbidden relationships are also named for clear diagnostics: no dependency on CLI; no protocol-core dependency on built-ins; no session dependency on WAL; no CLI Chronicle dependency except application; no common upward dependency.

Target normal allowlist:

| Source | Allowed Chronicle normal dependencies |
| --- | --- |
| `chronicle-common` | none |
| `chronicle-canonical` | `chronicle-common` |
| `chronicle-capture` | `chronicle-common` |
| `chronicle-capture-ebpf` | `chronicle-capture`, `chronicle-common` |
| `chronicle-wal` | `chronicle-capture`, `chronicle-common` |
| `chronicle-session` | `chronicle-capture`, `chronicle-common` |
| `chronicle-protocol` | `chronicle-canonical`, `chronicle-common`, `chronicle-session` |
| `chronicle-protocol-builtins` | `chronicle-canonical`, `chronicle-common`, `chronicle-protocol` |
| `chronicle-storage` | `chronicle-canonical`, `chronicle-common` |
| `chronicle-replay` | `chronicle-canonical`, `chronicle-common`, `chronicle-protocol` |
| `chronicle-etl` | `chronicle-canonical`, `chronicle-capture`, `chronicle-common`, `chronicle-protocol`, `chronicle-session`, `chronicle-storage`, `chronicle-wal` |
| `chronicle-application` | every non-CLI Chronicle crate needed for composition, including optional target-gated `chronicle-capture-ebpf` |
| `chronicle-cli` | `chronicle-application` only |

Dev/build rules are separate so tests cannot silently bypass architecture. `chronicle-etl -> chronicle-protocol-builtins` remains allowed for protocol integration tests. `chronicle-application -> chronicle-wal` remains allowed because application already owns WAL composition and uses the test-support feature. CLI capture/WAL dev edges are migrated behind application-owned test helpers or application integration fixtures, leaving CLI with application as its sole Chronicle dependency in every dependency kind. No workspace build edge is initially allowed.

Rationale: explicit pairs express actual ownership better than a coarse layer number because ETL legitimately spans extraction through storage load, protocol SPI intentionally spans canonicalization/replay adapters, and application intentionally composes all lower capabilities.

Alternatives considered:

- Snapshot all current edges: rejected because it would bless known session and CLI leakage.
- Enforce only forbidden pairs: rejected because unknown future edges would pass until someone remembered to ban them.
- Enforce numeric layers: rejected because valid cross-cutting ETL/application edges become exceptions that obscure policy.

### 2. `AGENTS.md` is normative guidance; TOML is executable mirror

Add concise `AGENTS.md` section naming responsibilities, direction, and forbidden patterns. Add detailed `docs/architecture/crate-boundaries.md` with current/target graph, rationale, and examples. `validation/architecture.toml` mirrors allowed/forbidden edges for automation. Changes to one require review/update of all three.

Rationale: agents read `AGENTS.md`; maintainers need detailed docs; CI needs structured data. Generating one from another adds tooling and reduces readability without proven need.

Alternative: make TOML sole source and generate prose. Rejected as unnecessary machinery and contrary to mandatory `AGENTS.md` source-of-truth requirement.

### 3. Reuse existing Python validation harness and Cargo metadata

Extend existing standard-library validation tooling with an architecture command, or add one small adjacent standard-library helper if keeping `scripts/validation.py` focused is clearer. It will:

1. run bounded `cargo metadata --format-version 1 --no-deps` from workspace root;
2. identify workspace path dependencies by package identity, not string prefix;
3. preserve dependency kind, optional flag, rename/package identity, and target condition;
4. compare all normal/dev/build declarations against policy regardless of host target;
5. reject missing policy entries, unknown workspace members, duplicate/conflicting policy, forbidden/unclassified edges, dependencies on CLI, and cycles;
6. print deterministic source/target/kind diagnostics and exit nonzero on violations.

Allowed entries need not exist: deleting an unnecessary dependency is safe and must not require first editing policy. Policy lists maximum allowed coupling, not required coupling.

Run check in `validate.sh fast` and `release`; targeted mode reaches it through relevant portable/build-tooling selection. Existing CI already runs fast validation, so no new job or privileged scenario is needed. Add standard-library fixture tests beside existing layered-validation tests.

Alternatives considered:

- `cargo-deny`, `cargo-hakari`, or another dependency: rejected; this is a small graph check and Chronicle already has a Python/TOML validation harness.
- Parse only host-resolved `cargo tree`: rejected because optional/target-specific edges can disappear on macOS.
- Parse TOML without Cargo metadata: rejected as sole authority because package renames/workspace identity are easier and safer through Cargo metadata. Manifest parsing may supplement tests/config validation, not replace Cargo's graph model.

### 4. Convert WAL terminal-loss wire type before session reconstruction

Keep `TerminalWalLoss` and its codec in `chronicle-wal` so WAL bytes and recovery API remain compatible. Introduce or reuse a transport-neutral persistence-loss evidence type at the evidence boundary, preferably `chronicle-capture` because it already owns clock, interval, ambiguity, and loss evidence concepts. ETL converts recovered WAL terminal-loss records into that neutral type before calling `chronicle-session`. Session then depends only on capture/common and accepts neutral loss evidence.

Do not rename serialized checkpoint/canonical fields merely to remove “wal” terminology. Concrete type dependency and implementation knowledge are the defect; persisted provenance names are compatibility constraints until an explicit format change.

Alternative: move `TerminalWalLoss` itself into common/capture. Rejected because that moves WAL wire ownership instead of adapting it and risks format/API churn.

### 5. Organize application by use case without new abstraction layers

Use existing modules and the smallest moves needed to make ownership obvious. Target conceptual groups:

```text
application
├── record      # one-shot/selector/command recording and recovery retry
├── recorder    # continuous lifecycle, lease, quota, rollover, status
├── etl         # application entrypoints around complete ETL workflows
├── replay      # explicit and command replay workflows
├── inspect     # catalog/session inspection
└── doctor      # diagnostic orchestration
```

Exact directory nesting is optional. Success means root `lib.rs` no longer contains unrelated large implementations and public exports are grouped/curated. Existing concrete functions are preferred over a new service trait/factory. Remove the mostly unimplemented `ChronicleApplication` facade if no real consumer requires it; do not complete it merely to preserve an unused abstraction.

Public low-level APIs remain only when another crate, integration test, or documented extension seam needs them. Internal module moves must preserve behavior and serialized types.

Alternative: split application into six crates. Explicitly rejected by scope and maintenance cost.

### 6. Application owns CLI-facing request, result, rendering, and exit taxonomy

CLI keeps Clap structs, command selection, process runtime/signal wiring required by the binary, checked stdout/stderr writes, and final process exit. Application exposes bounded request/result/error APIs sufficient for CLI to avoid importing protocol, built-in, replay, WAL, capture, or common crates.

Where CLI currently constructs `LoopbackReplayOptions`, matches `ReplayOutcome`, or inspects nested `ProtocolError`, add the smallest application-owned constructor/request DTO and stable application outcome/error classification. Prefer methods on existing application result/error types over duplicate wrapper enums. Rendering can remain application-owned where already present; CLI writes the completed string atomically.

CLI tests must exercise behavior through binary/application seams. Fixture construction that requires WAL/capture moves to application test support or prepares artifacts through application services. This is test organization only, not product behavior.

Alternative: keep direct lower-layer imports because CLI is “only an adapter.” Rejected: Cargo edges permit business logic and type taxonomy to leak later, and acceptance explicitly requires application-only dependency.

### 7. Preserve ETL as complete Extract-Transform-Load

Architecture docs and policy explicitly allow ETL to depend on WAL, capture, session, protocol, canonical, and storage. Current ownership is split: `chronicle-etl::publication` owns incremental immutable-artifact publication/checkpoint helpers, while application directly publishes one-shot final sessions through `FilesystemSessionStore` and writes the recording-local checkpoint.

Target consolidates the existing one-shot publication-before-checkpoint transaction behind an ETL-owned API. Application retains use-case invocation, domain-lock/quota policy, input/output selection, and presentation, but it does not call the concrete session publisher or advance ETL checkpoints itself. Migration MUST preserve application-supplied quota reservations, deterministic IDs, no-replace verification, byte-compatible final session/checkpoint artifacts, and exact publication-before-checkpoint ordering.

Alternative: document the current split as complete ETL. Rejected because direct application publication contradicts the requested complete Extract-Transform-Load owner. Alternative: move all ETL publication into application to make ETL transform-only. Rejected by explicit scope and existing crash-safety ownership.

## Risks / Trade-offs

- **[Risk] Large application moves collide with active `user-intent-cli` edits.** → Implement after rebasing current work; move one use case at a time; preserve public behavior with focused tests; never overwrite unrelated dirty files.
- **[Risk] Application-owned CLI DTOs duplicate lower-layer models.** → Add only adapter-facing fields/methods required by CLI; reuse existing application types and conversions; avoid generic facade/framework.
- **[Risk] Validator misses optional/target-specific edges on macOS.** → Test target/optional metadata explicitly and inspect Cargo metadata declarations, not only resolved build graph.
- **[Risk] Dev dependencies become an escape hatch.** → Validate normal, dev, and build kinds separately; CLI restriction applies to all Chronicle dependency kinds.
- **[Risk] Policy and prose drift.** → Require same-change updates to `AGENTS.md`, architecture doc, and TOML; add deterministic policy validation and review checklist. Avoid generator until drift is observed.
- **[Risk] Session neutralization accidentally changes checkpoint or WAL bytes.** → Keep WAL wire structs/codecs and serialized legacy fields stable; conversion occurs before session input; add byte-equivalence and reconstruction equivalence tests.
- **[Risk] Moving one-shot Load sequencing drops application quota/domain safeguards.** → Pass required reservation/authority inputs into the ETL-owned transaction, retain current fault matrix, and compare final session/checkpoint bytes and ordering before removing direct application calls.
- **[Risk] Broad WAL/storage surfaces remain imperfect.** → Document ownership now; narrow only bypasses needed by actual boundary work. Do not expand scope into speculative API cleanup.
- **[Trade-off] Explicit allowlist requires edits when a legitimate dependency is added.** → Intentional review gate; error points to source, target, kind, and policy file.

## Migration Plan

1. Add `docs/architecture/crate-boundaries.md`, `AGENTS.md` rules, and `validation/architecture.toml` describing target graph; validator may initially report known session/CLI violations in a focused migration mode only on the implementation branch, never merged with exemptions that bless them.
2. Add validator implementation/tests, but delay mandatory fast/release wiring until known forbidden edges are removed.
3. Introduce neutral persistence-loss evidence and conversion at ETL extraction boundary; remove `chronicle-wal` from session manifest; run session/ETL/WAL tests and verify persisted bytes unchanged.
4. Consolidate existing application-owned one-shot final publication/checkpoint sequencing behind an ETL-owned API, preserving application quota/domain policy and exact artifact/order behavior.
5. Refactor remaining application use cases in small moves, preserving behavior and public entrypoints needed by active CLI work.
6. Add application-owned CLI seam; remove CLI lower-layer normal/dev dependencies and imports; run CLI contract tests.
7. Wire architecture check into fast/release validation, then run OpenSpec strict validation, architecture check, `validate.sh fast`, and focused crate tests. Privileged acceptance is not required unless implementation changes production runtime behavior beyond this design; if it does, stop and open/extend scope rather than claiming architecture-only completion.
8. Update graphify after code-file modifications as required by repository guidance.

Rollback is source-only: revert validator wiring, module moves, dependency changes, and neutral conversion together. No data migration or artifact rollback exists because persisted formats do not change.

## Acceptance Criteria

Check only from explicit implementation evidence:

- [ ] Every crate has one documented primary responsibility.
- [ ] Dependency direction is documented.
- [ ] Architecture validation exists.
- [ ] Forbidden dependency edges fail validation.
- [ ] CLI only depends on application boundary for Chronicle APIs.
- [ ] Application responsibilities are separated internally.
- [ ] Session does not depend on WAL-specific implementation types.
- [ ] `AGENTS.md` contains architecture rules.
- [ ] Existing tests continue passing.
- [ ] No user-visible behavior changes.

## Open Questions

None blocking. Exact application file nesting may follow active `user-intent-cli` work; conceptual ownership and dependency rules are normative, directory aesthetics are not.
