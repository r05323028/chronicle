## Purpose

Authorize the one additive domain surface the `correlation-resolver` capability requires: a resolver-generated `ScenarioRoot` correlation-evidence variant in Chronicle's non-frozen runtime correlation domain.

## ADDED Requirements

### Requirement: Correlation evidence may include resolver-generated ScenarioRoot records

Chronicle correlation evidence MAY include `ScenarioRoot { root: CanonicalOperationRef }` items. `ScenarioRoot` records Chronicle's explicit establishment of a `Known(Ingress)` operation as the root of its derived scenario; it is correlation-level membership evidence, non-temporal, and acceptable within a validated `Resolved` outcome. It SHALL NOT classify the operation's interaction role and SHALL NOT replace or mutate `InteractionRoleResolution`; role classification remains an independent dimension owned by role resolutions. Caller-supplied `ScenarioRoot` is not authorized as resolver input — input reservation and typed rejection belong to the `correlation-resolver` capability. The addition affects only the non-frozen runtime correlation-domain surface and SHALL NOT modify Capture Event v1, Canonical Session v1, WAL v1, public stable JSON, or any persisted contract.

#### Scenario: ScenarioRoot witnesses scenario-root membership

- **WHEN** a resolver establishes Scenario A around known-ingress operation A
- **THEN** A's `Resolved` resolution carries a `ScenarioRoot { root: A }` item as its non-temporal correlation witness
- **AND** existing foundation validation accepts the resolution without weakening

#### Scenario: ScenarioRoot does not classify role

- **WHEN** any graph contains `ScenarioRoot` items for established roots
- **THEN** each root's `InteractionRoleResolution` remains exactly as supplied, with its own evidence intact
- **AND** inspecting scenario membership never substitutes for role classification

#### Scenario: Caller-supplied ScenarioRoot is not resolver input

- **WHEN** caller-controlled input contains `ScenarioRoot` items in either evidence channel
- **THEN** the resolver rejects them per the `correlation-resolver` capability's input-reservation rules
- **AND** this domain variant remains valid exclusively in resolver output

#### Scenario: Frozen contracts remain untouched

- **WHEN** the variant ships
- **THEN** Capture Event v1, Canonical Session v1, WAL v1, public stable JSON, and persisted contracts contain no additions from it
