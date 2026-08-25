## Purpose

Authorize the one additive domain surface the `correlation-resolver` capability requires: a resolver-generated `ScenarioRoot` correlation-evidence variant in Chronicle's non-frozen runtime correlation domain, together with its placement-consistency rule once the value exists inside a `CorrelationGraph`.

## ADDED Requirements

### Requirement: Correlation evidence may include resolver-generated ScenarioRoot records valid only in root-establishment placement

Chronicle correlation evidence MAY include `ScenarioRoot { root: CanonicalOperationRef }` items. `ScenarioRoot` records Chronicle's explicit establishment of a `Known(Ingress)` operation as the root of its derived scenario; it is correlation-level membership evidence, non-temporal, and acceptable within a validated `Resolved` outcome. It SHALL NOT classify the operation's interaction role and SHALL NOT replace or mutate `InteractionRoleResolution`; role classification remains an independent dimension owned by role resolutions.

Responsibilities stay separate: the RESOLVER owns creation (input reservation forbidding caller-supplied items in both channels is owned by the `correlation-resolver` capability), while the CORRELATION DOMAIN owns consistency validation once the value exists inside a graph. Because `ScenarioRoot` has placement-dependent semantics, generic evidence validity alone is insufficient: a graph containing `ScenarioRoot { root: R }` is consistent ONLY when all of the following hold:

1. the item appears in the `Resolved` correlation outcome of operation R;
2. that outcome selects Scenario S;
3. Scenario S.root equals R;
4. R's preserved role resolution is `Known(Ingress)`.

Domain/graph validation SHALL reject semantically invalid placements, including: a `ScenarioRoot { root: B }` inside the resolution of a different operation A; a `ScenarioRoot` on an ordinary non-root member; a `ScenarioRoot { root: A }` inside a resolution selecting a scenario whose root is not A; a `ScenarioRoot` inside `Uncorrelated` evidence; a `ScenarioRoot` as ambiguous-candidate evidence; and a `ScenarioRoot` inside `SelectedCausalEdge.evidence`. This is a consistency rule for the new variant only: existing evidence validation is not redesigned, and externally supplied graphs are not rejected beyond semantically invalid `ScenarioRoot` placement.

The addition affects only the non-frozen runtime correlation-domain surface and SHALL NOT modify Capture Event v1, Canonical Session v1, WAL v1, public stable JSON, or any persisted contract.

#### Scenario: ScenarioRoot witnesses scenario-root membership

- **WHEN** a resolver establishes Scenario A around known-ingress operation A
- **THEN** A's `Resolved` resolution carries a `ScenarioRoot { root: A }` item as its non-temporal correlation witness
- **AND** existing foundation validation accepts the resolution without weakening

#### Scenario: Valid root placement passes full validation

- **WHEN** Scenario S.root is A, role[A] is `Known(Ingress)`, and resolution[A] is `Resolved { scenario: S, confidence: Exact, evidence: [ScenarioRoot { root: A }] }`
- **THEN** graph validation succeeds under `validate_against_sessions(supplied_sessions)`
- **AND** the placement satisfies every condition of this requirement

#### Scenario: ScenarioRoot does not classify role

- **WHEN** any graph contains `ScenarioRoot` items for established roots
- **THEN** each root's `InteractionRoleResolution` remains exactly as supplied, with its own evidence intact
- **AND** inspecting scenario membership never substitutes for role classification

#### Scenario: Wrong-operation placement fails validation

- **WHEN** resolution[A] is Resolved but contains `ScenarioRoot { root: B }` where A and B differ
- **THEN** graph validation rejects the graph

#### Scenario: Non-root member placement fails validation

- **WHEN** Scenario S.root is A while ordinary member B's resolution contains `ScenarioRoot { root: B }`
- **THEN** graph validation rejects the graph because the item does not sit in the root's own resolution

#### Scenario: Wrong-scenario placement fails validation

- **WHEN** `ScenarioRoot { root: A }` appears in a resolution selecting Scenario T whose root is not A
- **THEN** graph validation rejects the graph

#### Scenario: Uncorrelated, ambiguous-candidate, and edge placements fail validation

- **WHEN** a `ScenarioRoot` item appears inside `Uncorrelated` evidence, inside any `Ambiguous.candidates[*].evidence`, or inside `SelectedCausalEdge.evidence`
- **THEN** graph validation rejects the graph in every case
- **AND** `ScenarioRoot` can never serve as generic relationship or parent evidence

#### Scenario: Caller-supplied ScenarioRoot is not resolver input

- **WHEN** caller-controlled input contains `ScenarioRoot` items in either evidence channel
- **THEN** the resolver rejects them per the `correlation-resolver` capability's input-reservation rules
- **AND** caller input cannot supply this variant to the resolver
- **AND** within `CorrelationGraph` values, this variant is valid only in the root-establishment placement defined above

#### Scenario: Frozen contracts remain untouched

- **WHEN** the variant ships
- **THEN** Capture Event v1, Canonical Session v1, WAL v1, public stable JSON, and persisted contracts contain no additions from it
