# Endpoint Provenance Reconstruction

## Purpose

Define endpoint provenance from capture evidence through canonical reconstruction.

## Requirements

### Requirement: Socket evidence preserves endpoint provenance
`SocketEvidence` SHALL contain socket identity, network family, local IP and non-zero local port, remote IP and non-zero remote port, and socket role `active` or `passive`. Endpoint address families MUST match declared network family. Endpoint evidence SHALL remain capture-domain observation and MUST NOT contain application semantics. Because connect hooks precede TCP autobind, `SocketConnectObserved` SHALL use separate connect-intent evidence with remote endpoint and active role but no claimed local endpoint; complete `SocketEvidence` SHALL begin at establishment.

#### Scenario: Active socket evidence
- **WHEN** active connection evidence crosses capture boundary
- **THEN** socket evidence records process-local endpoint as local, peer endpoint as remote, and role as active

#### Scenario: Passive socket evidence
- **WHEN** accepted connection evidence crosses capture boundary
- **THEN** socket evidence records listening-process endpoint as local, peer endpoint as remote, and role as passive

#### Scenario: Invalid endpoint evidence
- **WHEN** family, address, port, or role is missing, malformed, zero, or inconsistent
- **THEN** adapter rejects observation with typed error
- **AND** no partial socket evidence event is emitted

### Requirement: Private ABI v1 carries socket endpoint evidence
Private eBPF ABI SHALL remain version 1. Connect-intent observations SHALL encode bounded remote address/port, network family, and active role without fabricating local endpoint. Established socket observations SHALL encode bounded local address/port, remote address/port, network family, and active/passive role. Producer and userspace decoder SHALL use same documented byte layouts and reject unknown role, family, size, byte order, or version.

#### Scenario: IPv4 ABI decode
- **WHEN** valid ABI v1 IPv4 active or passive socket item is decoded
- **THEN** exact local/remote IPv4 endpoints and role reach `RawKernelObservation`

#### Scenario: IPv6 ABI decode
- **WHEN** valid ABI v1 IPv6 active or passive socket item is decoded
- **THEN** exact local/remote IPv6 endpoints and role reach `RawKernelObservation`

#### Scenario: No ABI v2
- **WHEN** endpoint fields are added to producer and decoder
- **THEN** ABI discriminator remains 1
- **AND** no ABI v2 constant, layout, or dispatch path is introduced

### Requirement: Payload events reference cached socket evidence
Endpoint tuple and socket role SHALL be recorded on socket evidence and cached by `SocketIdentity`. `PayloadFragment` SHALL reference socket identity for endpoint correlation and MUST NOT duplicate local endpoint, remote endpoint, or socket role.

#### Scenario: Correlated payload
- **WHEN** payload follows complete established socket evidence for same identity
- **THEN** adapter emits payload fragment without endpoint tuple
- **AND** reconstruction resolves endpoint provenance from cache

#### Scenario: Connect intent before autobind
- **WHEN** active connect hook observes remote endpoint before local ephemeral port exists
- **THEN** adapter emits connect-intent evidence without local endpoint
- **AND** it does not admit that intent to complete socket-evidence cache until establishment

#### Scenario: Payload without socket evidence
- **WHEN** payload references identity absent from socket cache
- **THEN** adapter or reconstruction fails with typed missing-evidence result
- **AND** no endpoint is synthesized

#### Scenario: Conflicting socket evidence
- **WHEN** same socket identity is associated with different endpoint tuple or role
- **THEN** processing fails with typed conflicting-evidence result
- **AND** cache is not silently overwritten

### Requirement: Socket role determines canonical endpoint assignment
ETL SHALL assign canonical endpoints from cached socket evidence. Active role SHALL map local endpoint to client and remote endpoint to server. Passive role SHALL map remote endpoint to client and local endpoint to server. Ingress/egress direction MUST NOT determine client/server endpoint identity.

#### Scenario: Active assignment
- **WHEN** active socket carries local `10.0.0.2:40000` and remote `10.0.0.3:8080`
- **THEN** canonical client is `10.0.0.2:40000`
- **AND** canonical server is `10.0.0.3:8080`

#### Scenario: Passive assignment
- **WHEN** passive socket carries local `10.0.0.3:8080` and remote `10.0.0.2:40000`
- **THEN** canonical client is `10.0.0.2:40000`
- **AND** canonical server is `10.0.0.3:8080`

#### Scenario: Direction does not establish roles
- **WHEN** otherwise equivalent ingress/egress evidence is reconstructed once as active and once as passive
- **THEN** endpoint assignment follows role in each case
- **AND** ETL does not use traffic direction as role inference

### Requirement: Canonical output never invents unknown endpoints
Production reconstruction SHALL NOT emit `unknown:0` or another placeholder for missing endpoint provenance. Missing or contradictory socket evidence SHALL remain typed failure or explicit non-publication rather than fabricated canonical connection.

#### Scenario: Missing provenance at publication
- **WHEN** ETL cannot resolve socket identity to complete endpoint evidence
- **THEN** canonical connection is not published with placeholder endpoints
- **AND** caller receives typed provenance failure
