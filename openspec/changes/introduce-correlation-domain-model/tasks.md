## 1. Identity and canonical domain

- [ ] 1.1 Add neutral Chronicle-owned `EventId` and `ScenarioId` primitives in `chronicle-common`; verify `OperationId` remains the sole interaction identity and no duplicate `InteractionId` is introduced.
- [ ] 1.2 Add the correlation domain module under `chronicle-canonical` with `Scenario`, scenario-interaction membership, directed causal edges, `CorrelationEvidence`, `CorrelationResolution`, and categorical `CorrelationConfidence` values defined by `specs/correlation-domain-model/spec.md`.
- [ ] 1.3 Add domain validation for unique event/interaction/scenario references, root-ingress validity, selected-parent cardinality, self-edge rejection, acyclic selected causal edges, and candidate/evidence reference integrity.
- [ ] 1.4 Keep operation completeness, replayability, session identity, recording/epoch provenance, and scenario ownership as separate fields/decisions; a scenario assignment MUST NOT make incomplete or lost operations replayable.

## 2. Evidence and resolution semantics

- [ ] 2.1 Implement the Chronicle-owned tagged evidence vocabulary for trace, protocol ownership, execution/task lineage, process, thread, connection/socket generation, protocol stream, temporal/lifetime, and namespaced custom evidence, including bounded source/provenance metadata.
- [ ] 2.2 Represent trace identifiers as provider-neutral opaque values with completeness/provenance; add no OpenTelemetry, W3C, B3, Datadog, AWS X-Ray, or other tracing SDK type to any core/domain API.
- [ ] 2.3 Implement resolved (`Exact`, `Strong`, `Inferred`), `Ambiguous` with candidate-specific evidence, and `Uncorrelated` outcomes without numeric-score-only explanations or forced scenario membership.
- [ ] 2.4 Add model-level checks proving temporal overlap alone cannot produce a resolved assignment and that PID, TID, socket, connection, stream, task-worker, and timestamp values remain evidence only.

## 3. Pipeline and contract integration

- [ ] 3.1 Define the canonical/ETL seam where future correlation consumes protocol-produced `CanonicalOperation` values and evidence while `chronicle-capture`, `chronicle-session`, `chronicle-protocol`, `chronicle-wal`, `chronicle-storage`, and `chronicle-application` retain their documented ownership.
- [ ] 3.2 Preserve protocol-local request/response correlation in protocol canonicalizers; do not make capture, socket reconstruction, WAL order, or storage publication responsible for scenario ownership.
- [ ] 3.3 Define the 0.2 persisted integration point and reader/writer policy before adding event/scenario fields to Capture Event or Canonical Session artifacts; do not mutate 0.1 v1 artifacts or add implicit compatibility fallbacks.
- [ ] 3.4 Preserve unresolved evidence through ETL/publication and ensure post-response background work is not automatically attached to a completed ingress scenario.

## 4. Domain and pipeline tests

- [ ] 4.1 Add canonical unit tests for stable event/interaction/scenario identity, identity separation, causal-edge validation, explainable evidence retention, and operation completeness/replayability independence.
- [ ] 4.2 Add an integration fixture covering concurrent `POST /checkout`, `GET /profile`, and `POST /login` roots with interleaved PostgreSQL, HTTP, and Redis interactions; assert independent expected Scenario A/B/C memberships and no cross-scenario parent edge.
- [ ] 4.3 Add tests for overlapping ingress lifetimes in one PID, reused TIDs/thread-pool workers, async execution migration across runtime workers, and absence of any single-thread/request assumption.
- [ ] 4.4 Add tests for connection-pool reuse and multiplexed streams, including multiple logical interactions on one socket/connection and independent HTTP/2-style stream ownership.
- [ ] 4.5 Add tests for complete, partial, and absent trace context; prove no-trace scenarios remain representable and missing trace parents are never synthesized.
- [ ] 4.6 Add tests for exact/strong/inferred resolution, conflicting evidence, ambiguous A/B candidates with candidate-specific evidence, and uncorrelated interactions that remain preserved without synthetic ownership.
- [ ] 4.7 Keep each test at the lowest conclusive functional layer required by `validation/test-architecture/README.md`; use unit tests for value invariants and integration tests only for cross-crate ETL/domain flow.

## 5. Existing architecture validation

- [ ] 5.1 Extend `validation/architecture.toml` and the existing `scripts/validation.py architecture` command with the smallest direct external-dependency denylist needed to reject tracing SDK/provider packages from core/domain processing crates (`chronicle-common`, `chronicle-canonical`, `chronicle-capture`, `chronicle-session`, `chronicle-protocol`, and `chronicle-etl`); keep adapter-owned outer crates separate and do not create a parallel checker.
- [ ] 5.2 Add standard-library temporary-workspace tests beside existing architecture validation tests proving forbidden tracing dependencies fail, allowed existing CLI logging remains outside the domain rule, and all normal/dev/build Chronicle edges still obey the current allowlist.
- [ ] 5.3 Mechanically verify the final dependency graph has no OpenTelemetry or provider-specific tracing package in the listed core/domain processing crates, no new Chronicle crate, no reverse adapter edge, and no change to the existing `chronicle-canonical -> chronicle-common` boundary.
- [ ] 5.4 Run the existing architecture check through `./scripts/validate.sh fast` or its changed-path equivalent and retain diagnostics as implementation evidence.

## 6. Durable documentation

- [ ] 6.1 Update `docs/architecture/crate-boundaries.md` with canonical correlation ownership, common identity ownership, capture/session/protocol/ETL responsibilities, and the adapter-inward dependency direction without changing the allowed Chronicle graph unnecessarily.
- [ ] 6.2 Update `docs/architecture/overview.md` and `docs/canonical-model.md` with the distinction between protocol request/response correlation and scenario correlation, the conservative ingress-response boundary, and unresolved outcomes.
- [ ] 6.3 Update `AGENTS.md` with only concise durable invariants: Chronicle owns correlation; trace is optional enrichment; temporal overlap is insufficient; infrastructure identities are evidence; ambiguity remains ambiguity.
- [ ] 6.4 Review user-facing documentation impact. This foundation changes no CLI or website behavior; if later implementation adds user-facing scenario commands or canonical English pages, update corresponding `zh-tw`/`ja` pages and run `cd website && npm run verify:localization`.

## 7. Validation and acceptance

- [ ] 7.1 Validate the change artifacts with `openspec validate --all --strict --no-interactive`.
- [ ] 7.2 Run `cargo fmt --check`, warnings-denied Clippy, targeted workspace tests, OpenSpec validation, and architecture/tooling checks through the repository's bounded validation entrypoint after implementation; no build-only result counts as completion.
- [ ] 7.3 Check the final implementation diff for accidental OpenTelemetry/tracing SDK requirements, unsafe PID/TID/socket/connection/stream/time ownership assumptions, missing concurrent-ingress coverage, missing ambiguous/uncorrelated outcomes, and unrelated replay/export/test-generation scope.
- [ ] 7.4 Run `graphify update .` after any production code modification in the implementation change; this planning-only change modifies no production source and therefore must not trigger a graph update.
- [ ] 7.5 Confirm implementation changes only the approved domain/docs/validation surfaces, preserve WAL/capture/canonical/replay compatibility rules, and report any architecture conflict before completion.
