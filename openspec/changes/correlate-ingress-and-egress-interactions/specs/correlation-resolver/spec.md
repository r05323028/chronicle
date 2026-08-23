## Purpose

Define Chronicle's deterministic, provider-neutral, ambiguity-safe resolver that selects scenario ownership for canonical operations from Chronicle-owned role resolutions and `CorrelationEvidence`, producing valid `CorrelationGraph` output. The resolver computes what the `correlation-domain-model` capability validates; foundation validation semantics remain authoritative for resolver output.

## ADDED Requirements

### Requirement: Resolver is deterministic and provider-neutral

Chronicle SHALL provide a resolver that consumes recording-scoped canonical operation references, supplied `InteractionRoleResolution` values, and Chronicle-owned `CorrelationEvidence`, and SHALL produce a populated `CorrelationGraph` satisfying all `correlation-domain-model` invariants without modification of that validator. Given identical inputs, results SHALL be identical — including derived `ScenarioId`s and representation ordering — across process restarts, ETL retries, ETL replay, input iteration order, map/hash iteration order, worker scheduling, and epoch publication boundaries. The resolver SHALL NOT depend on or expose OpenTelemetry, W3C, B3, Datadog, AWS X-Ray, Jaeger, Zipkin, or other tracing SDK types, and its behavior SHALL NOT change when trace providers are absent.

#### Scenario: Restart and retry stability

- **WHEN** the resolver runs twice over identical canonical inputs in separate invocations simulating restart, ETL retry, and replay
- **THEN** both outputs are identical including every derived `ScenarioId`
- **AND** no output depends on wall-clock time, randomness, or processing counters

#### Scenario: Input order independence

- **WHEN** the same canonical inputs are presented in different iteration orders, including orders that would differ under hash-map traversal
- **THEN** semantic correlation outcomes, memberships, edges, and identifiers are unchanged
- **AND** any representation reordering is a deterministic canonical sort, not an outcome change

#### Scenario: Epoch publication boundary does not change identity

- **WHEN** the same logical scenario is resolved from inputs published in different epoch groupings
- **THEN** the scenario keeps one derived identity rooted at the same root operation reference
- **AND** epoch rollover neither splits nor renames the scenario

#### Scenario: No provider SDK enters resolution

- **WHEN** trace-shaped evidence exists only as Chronicle-owned opaque provider-labelled `CorrelationEvidence`
- **THEN** the resolver resolves it like any other evidence
- **AND** no provider SDK type appears in the resolver API, and removing all provider adapters changes nothing except evidence availability

### Requirement: Candidates come only from known ingress roots

The resolver SHALL construct exactly one scenario candidate per admitted operation whose supplied role resolution is `Known(Ingress)`. Operations with `Known(Egress)`, `Unknown`, or `Ambiguous` role resolutions SHALL never own or root a scenario candidate. Scenario candidates SHALL exist as graph entities even when nothing resolves into them. The resolver SHALL derive each `ScenarioId` deterministically from stable scope — the recording identity plus the root operation's full scoped reference — and SHALL NOT use randomness, timestamps, session/epoch identity, or processing order in the derivation.

#### Scenario: One candidate per known ingress

- **WHEN** two admitted operations have role resolution `Known(Ingress)` and others do not
- **THEN** the resolver creates exactly two scenario candidates rooted at those references
- **AND** each candidate's identity derives from its root reference within the recording scope

#### Scenario: Egress never becomes a candidate owner

- **WHEN** an operation has role resolution `Known(Egress)` regardless of its evidence strength
- **THEN** it can resolve only as a member of an existing scenario candidate
- **AND** it never roots, owns, or creates a scenario

#### Scenario: Unknown or ambiguous role never becomes a root

- **WHEN** an operation's supplied role resolution is `Unknown` or `Ambiguous` while its correlation evidence strongly suggests ingress causality
- **THEN** the resolver creates no scenario rooted at that operation
- **AND** it preserves the supplied role resolution verbatim instead of promoting it

### Requirement: Evidence tiers constrain resolution power

The resolver SHALL classify evidence into normative tiers before use:

- **Decisive causal-link evidence** — a trace relationship whose declared parent context resolves exactly to one candidate operation's trace identity — MAY establish ownership alone.
- **Narrowing evidence** — temporal disjointness on comparable timelines, exact execution-task lineage filtering, and generation-disjointness proving co-execution impossible — MAY eliminate impossible candidates but SHALL never select among survivors.
- **Supporting/contextual evidence** — connection/socket/stream identity, protocol ownership, corroboration matches — SHALL neither eliminate nor select alone; it MAY corroborate a survivor supported by another tier.
- **Never-independent evidence** — temporal overlap, bare PID/TID equality, wire direction, socket role, shared connection identity, and processing order — SHALL never contribute to selecting an owner, alone or combined exclusively with other never-independent items.

Shared connection or stream identity SHALL NOT be treated as narrowing because reuse is normal. Custom namespaced evidence SHALL be supporting/contextual by default; promoting it requires a specification change. Conflicting decisive links to different owners SHALL produce ambiguity, not first-processed-wins selection.

#### Scenario: Decisive trace link establishes ownership

- **WHEN** one egress carries a trace relationship whose parent context resolves exactly to Ingress A's trace identity and to no other viable owner
- **THEN** the egress resolves to Scenario A with `Exact` confidence
- **AND** the resolution cites the decisive evidence item

#### Scenario: Narrowing eliminates but never selects

- **WHEN** execution-task lineage excludes all viable owners except one, and no decisive item exists
- **THEN** the resolver may treat remaining candidates as narrowed
- **AND** final selection still requires non-temporal support from at least two independent dimensions before emitting `Strong`

#### Scenario: Supporting evidence cannot carry a resolution

- **WHEN** an egress shares a connection, stream, socket role, and wire direction with Ingress A's traffic but holds no decisive or narrowing causal item
- **THEN** the egress does not resolve to Scenario A on those items
- **AND** they remain retained as supporting evidence on whatever outcome is emitted

#### Scenario: Temporal overlap selects nothing

- **WHEN** an egress overlaps multiple ingress lifetimes and temporal overlap is the only relationship to each
- **THEN** no overlap count, proximity, recency, duration, or ordering heuristic contributes to selection
- **AND** the outcome is determined solely by other tiers, if any

#### Scenario: Temporal disjointness eliminates impossibilities

- **WHEN** an egress lifetime is provably disjoint from Ingress B's lifetime on comparable timelines while Ingress A's timeline supports it
- **THEN** Ingress B may be eliminated as an impossible candidate
- **AND** elimination alone does not resolve the egress; remaining-tier rules decide

#### Scenario: Incomparable timelines fail safe

- **WHEN** temporal comparison across sessions lacks comparable offsets
- **THEN** the resolver treats the temporal item as non-eliminating rather than guessing comparability

#### Scenario: Conflicting decisive evidence yields ambiguity

- **WHEN** one decisive evidence source supports Scenario A and another supports Scenario B for the same egress
- **THEN** the result is `Ambiguous { A, B }` with each candidate retaining its own supporting evidence
- **AND** the result never depends on which evidence was processed first

### Requirement: Outcomes are resolved, ambiguous, or uncorrelated

For each admitted non-root operation, the resolver SHALL emit exactly one outcome after eliminating evidence narrows the viable-owner set: exactly one sufficiently supported owner SHALL yield `Resolved`; multiple genuinely viable owners SHALL yield `Ambiguous { candidates }` with candidate-specific evidence preserved; zero viable owners SHALL yield `Uncorrelated` with available evidence retained. The resolver SHALL NOT force an unowned egress into any scenario and SHALL NOT emit the `Inferred` confidence value; externally supplied graphs retain that freedom.

#### Scenario: One ingress owns multiple egress interactions

- **WHEN** Ingress A holds decisive or sufficient evidence linking HTTP egress X and PostgreSQL egress Y to it, with no competing viable owner
- **THEN** X and Y both resolve to Scenario A
- **AND** Scenario A contains Ingress A as root with both egress members

#### Scenario: Concurrent ingresses resolve independently

- **WHEN** Ingress A and Ingress B overlap in time while evidence uniquely connects egress X to A and egress Y to B
- **THEN** the resolver produces Scenario A {A, X} and Scenario B {B, Y}
- **AND** neither scenario absorbs the other's member despite lifetime overlap

#### Scenario: Genuine ambiguity is preserved

- **WHEN** both Scenario A and Scenario B remain viable owners for one egress after all eliminations
- **THEN** the result is `Ambiguous { A, B }` with per-candidate evidence
- **AND** arbitrary ordering, timing proximity, member counts, or evidence volume never break the tie

#### Scenario: Temporal-only evidence stays unresolved

- **WHEN** an egress overlaps one or more ingress lifetimes and holds no stronger tier of evidence
- **THEN** the outcome is not `Resolved`
- **AND** the egress remains ambiguous or uncorrelated with its temporal evidence retained

#### Scenario: No valid parent remains uncorrelated

- **WHEN** no active or historical scenario candidate survives elimination for an egress
- **THEN** the result is `Uncorrelated` with retained evidence
- **AND** no synthetic owner, root, or membership entry is created

### Requirement: Concurrent ingress is handled without timing heuristics

When multiple ingress lifetimes execute concurrently — including nested and partially overlapping starts — the resolver SHALL evaluate each operation's ownership independently against all viable owners using evidence tiers only. The resolver SHALL contain no rule that prefers the oldest, newest, closest-starting, longest-running, or currently active ingress, and no rule keyed to arrival or processing sequence.

#### Scenario: Three overlapping ingresses with sparse egresses

- **WHEN** Ingress A, B, and C lifetimes mutually overlap and egresses X and Y carry unique evidence for A and B respectively
- **THEN** X resolves into Scenario A and Y into Scenario B regardless of start order, duration, or overlap degree
- **AND** C owns no member merely by being open or idle during X or Y

#### Scenario: Timing proximity is inert

- **WHEN** the temporally closest ingress to an egress differs from the evidence-linked owner
- **THEN** the evidence-linked owner wins
- **AND** no observable behavior distinguishes proximity weighting because none exists

#### Scenario: Partial-overlap ambiguity

- **WHEN** one egress remains evidence-viable under two overlapping ingresses while a second egress resolves uniquely
- **THEN** the first egress emits `Ambiguous` naming exactly those viable scenarios
- **AND** the second resolves normally in the same run

### Requirement: Role resolution is preserved verbatim

The resolver SHALL copy supplied `InteractionRoleResolution` values into the output graph unchanged. Correlation outcomes, selected edges, and scenario membership SHALL NOT rewrite an `Unknown` or `Ambiguous` role into a known role, promote timing-derived classification to ingress, or let an operation's scenario membership alter its role evidence. Operations with unknown or ambiguous roles MAY participate as scenario members through ordinary evidence-based resolution but SHALL never become roots.

#### Scenario: Unknown-role member keeps unknown role

- **WHEN** an operation with role resolution `Unknown` carries decisive causal evidence linking it to Scenario A
- **THEN** it may appear as a Scenario A member with a `Resolved` correlation outcome
- **AND** its role resolution remains `Unknown` with original evidence intact

#### Scenario: Ambiguous-role member is never promoted

- **WHEN** an operation with `Ambiguous { Ingress, Egress }` role resolution joins Scenario A via strong evidence
- **THEN** its role candidates and their evidence remain inspectable and unchanged
- **AND** it never qualifies as Scenario A's root

#### Scenario: Correlation never fabricates ingress

- **WHEN** a scenario needs a root and only non-root-eligible operations relate to it
- **THEN** the resolver leaves them without a scenario rather than inventing a root

### Requirement: Selected causal edges follow construction invariants

The resolver SHALL emit selected edges only between operations resolved into the same scenario, with parent chains derived from resolution structure: root first, then chained members. Edges SHALL satisfy every foundation invariant — full scoped endpoints, acyclic, no self-edge, at most one selected parent per child, root never a child — and cross-session or cross-epoch edges SHALL remain valid when both endpoints carry correct full references. Chained structure (ingress causes egress, egress causes further egress) SHALL attach descendants along the chain indicated by evidence rather than forcing every member into a direct child of the root. Ambiguous candidate relationships SHALL never become selected edges. Resolution rounds for chaining SHALL be bounded and internally ordered deterministically.

#### Scenario: Chained egress forms a grandchild edge

- **WHEN** an ingress causes an HTTP egress and that egress causes a database egress, with evidence indicating the database egress belongs under the HTTP egress
- **THEN** the graph contains ingress -> HTTP edge and HTTP -> database edge within one scenario
- **AND** scenario ownership semantics are unchanged: all three belong to the same single scenario

#### Scenario: Cross-session edge stays valid

- **WHEN** a parent resolved in epoch N's canonical session connects to a child resolved in epoch N+1's session
- **THEN** the selected edge carries both full scoped references and validates
- **AND** session difference alone neither invalidates nor rewrites the edge

#### Scenario: Tree invariant violations are impossible by construction

- **WHEN** the resolver completes any input set
- **THEN** the emitted graph contains no self-edge, cycle, multi-parent child, or child root
- **AND** existing graph validation accepts the edge set unchanged

#### Scenario: Ambiguous candidates never become edges

- **WHEN** an operation ends ambiguous between Scenarios A and B
- **THEN** no selected edge references that operation in either scenario
- **AND** its candidate relationships remain only in resolution evidence

### Requirement: Connection reuse preserves operation independence

Multiple logical operations sharing one physical connection SHALL remain independent canonical operations and independent correlation subjects throughout resolution. Connection identity, socket identity, and stream identity SHALL remain evidence only and SHALL never merge operations, transfer one operation's resolution to another, or become scenario identity.

#### Scenario: Shared connection does not merge scenarios

- **WHEN** two egress exchanges reuse one database connection while belonging to evidence-distinct scenarios
- **THEN** both remain separate members of their respective scenarios
- **AND** shared connection identity neither merges them nor reassigns either

#### Scenario: Connection identity never becomes scenario identity

- **WHEN** any resolution completes over connection-heavy traffic
- **THEN** every scenario identity derives from its root operation reference
- **AND** no connection, socket, stream, PID, TID, or port equals a scenario identity

### Requirement: Cross-epoch scenarios resolve through scoped references

The resolver SHALL support scenarios whose root and children are published in different finalized epoch sessions, resolving every operation through its full `CanonicalOperationRef`. Epoch rollover SHALL NOT split, rename, or duplicate a scenario, and terminal operations SHALL participate under their completion-owner scope. Identical `OperationId` values under different session scopes SHALL remain distinct resolution subjects.

#### Scenario: Root and child span epochs

- **WHEN** an ingress is owned by epoch N's session and a related egress by epoch N+1's session, with evidence linking them
- **THEN** both resolve into one scenario whose root reference targets epoch N's session
- **AND** the selected edge crosses sessions with full scoped endpoints

#### Scenario: Terminal scope participates

- **WHEN** an operation begins in epoch N but its canonical completion is owned by epoch N+1
- **THEN** the resolver admits it under the completion-owner reference
- **AND** predecessor continuation ranges remain provenance, never a second subject

#### Scenario: Same operation ID in two sessions stays distinct

- **WHEN** equal `OperationId` values exist under different session scopes in one recording
- **THEN** the resolver treats them as distinct subjects with independent outcomes
- **AND** resolving one never affects the other

### Requirement: Resolution works identically with and without tracing

Trace context SHALL be treated as high-quality decisive evidence when present, and the resolver SHALL produce complete, valid results from Chronicle-native evidence alone when no trace context exists. Execution/task lineage, connection/socket generation, protocol stream relationships, protocol ownership, and bounded custom evidence SHALL suffice to resolve scenarios without any tracing SDK installed. The absence of trace providers SHALL NOT degrade capture, WAL, ETL, canonicalization, storage, replay, or CLI behavior.

#### Scenario: Trace-assisted resolution

- **WHEN** provider-neutral trace evidence uniquely establishes ownership for concurrent traffic
- **THEN** the resolver uses it like any decisive evidence
- **AND** no provider SDK type or adapter participates in resolution

#### Scenario: Native-only resolution

- **WHEN** no trace context exists anywhere in the recording and native lineage/connection/protocol evidence uniquely determines ownership
- **THEN** scenarios resolve with the same outcome shapes and determinism guarantees as trace-assisted runs
- **AND** missing trace parents are never synthesized from infrastructure identifiers or timestamps

### Requirement: ETL composes the resolver without owning correlation semantics

`chronicle-etl` MAY convert published canonical sessions of one recording into resolver inputs and obtain the resulting `CorrelationGraph`, but SHALL contain no selection, scoring, or ownership logic. The default publication and checkpoint path SHALL NOT invoke the resolver, keeping capture-path overhead zero unless explicitly requested. No new crate, dependency edge, or architecture-policy entry is introduced by the resolver.

#### Scenario: ETL helper delegates all decisions

- **WHEN** the ETL composition helper converts sessions and invokes the resolver
- **THEN** every outcome equals direct resolver invocation over equivalent inputs
- **AND** the helper contributes ordering normalization only

#### Scenario: Publication path stays unchanged

- **WHEN** a recording publishes canonical sessions without requesting correlation
- **THEN** publication, checkpoints, and stored artifacts are byte-identical to pre-change behavior

#### Scenario: Architecture policy untouched

- **WHEN** architecture validation runs after implementation
- **THEN** the crate membership, allowlist edges, semantic boundaries, and external-dependency guards pass without edits
- **AND** no standalone correlation crate exists

### Requirement: Resolver remains runtime-only and compatibility-safe

The resolver SHALL operate in memory over non-v1 domain values and SHALL NOT persist correlation results, add fields to Capture Event v1 / Canonical Session v1 / WAL v1, introduce `EventId`, or add reader defaults or compatibility fallbacks. Persisted correlation artifacts SHALL require a dedicated follow-up change selecting a separately versioned sidecar/new artifact or an explicit versioned contract with migration policy. Invalid resolver inputs SHALL fail closed with typed errors; unverifiable references SHALL fail closed rather than resolve leniently; ordinary evidence situations SHALL always yield one of the three outcomes per admitted operation rather than an error.

#### Scenario: Frozen contracts unchanged

- **WHEN** the resolver ships
- **THEN** Capture Event v1, Canonical Session v1, WAL v1, public stable JSON, and CLI contracts contain no correlation additions
- **AND** no persisted artifact records resolver output

#### Scenario: Invalid input fails closed

- **WHEN** resolver inputs contain duplicate references, mismatched recording scope, or role resolutions failing foundation validation
- **THEN** the resolver rejects the input set with a typed error
- **AND** it neither drops the offending operation silently nor guesses a substitute classification

#### Scenario: Persistence deferred explicitly

- **WHEN** durable correlation artifacts are required later
- **THEN** a dedicated change such as `persist-correlation-scenario-artifacts` chooses the versioned sidecar/new-artifact or migration design
- **AND** this resolver change authorizes neither
