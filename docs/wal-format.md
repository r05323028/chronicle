# WAL Format

Chronicle has one mutable MVP WAL format: v1. Readers reject every other declared format, envelope, and record-schema version. No legacy reader or format dispatch remains.

All integers are little-endian. Recording WAL lives under `segments/`; segment files are named `{first_sequence:020}.chwal`.

## Segment header

Each segment starts with fixed 64-byte header:

```text
0..4    magic = CHS1
4..6    format_version:u16 = 1
6..8    header_version:u16 = 1
8..12   header_len:u32 = 64
12..28  recording_id:uuid bytes
28..36  segment_ordinal:u64
36..44  first_sequence:u64
44..52  created_unix_nanos:i64
52..60  reserved = zero
60..64  crc32c:u32
```

Header CRC32C covers bytes `0..60`. Reader validates magic, versions, length, identity, reserved bytes, checksum, ordinal ordering, and first-sequence continuity.

## Record envelope

Each record uses 48-byte header followed by bounded payload:

```text
0..4    magic = CHE1
4..6    envelope_version:u16 = 1
6..8    kind:u16
8..10   flags:u16
10..12  record_schema_version:u16 = 1
12..16  header_len:u32 = 48
16..20  payload_len:u32
20..36  recording_id:uuid bytes
36..44  sequence:u64
44..48  crc32c:u32
48..    payload[payload_len]
```

Envelope CRC32C covers bytes `0..44` plus payload. Active kinds are capture event, loss window, commit marker, and terminal WAL loss. Writer and reader reject non-v1 record schema, unknown kinds, wrong recording identity, sequence gaps, oversized records, checksum failure, and malformed kind payloads.

## Commit authority

`GroupCommitWalWriter` batches data envelopes, appends fixed 76-byte commit-marker payload, syncs data and marker, then acknowledges durability. Marker includes cumulative durable sequence/count/bytes plus batch sequence bounds and SHA-256 over exact placed envelopes. Complete records after final valid marker remain visible as uncommitted suffix and never enter ETL.

Writer rotates before data plus reserved commit marker would exceed configured segment size. Segment size is bounded from 16 MiB through 4 GiB; total WAL size is independently bounded. Unix directories/files use `0700`/`0600`.

## Recovery

Recovery scans numeric segments under exclusive recording lock, validates every header/envelope and commit chain, and returns:

- authoritative committed envelopes through final valid marker;
- uncommitted complete suffix;
- terminal WAL-loss evidence;
- optional incomplete tail only on final segment.

Repair truncates only verified incomplete final frame. Complete corruption fails closed. Reopen resumes after final authoritative marker, removes uncommitted later segments when policy permits, preserves sequence continuity, and never infers acknowledgement delivery from bytes alone.

Fixture and production recording use same segmented, group-committed v1 path. Filesystem session manifest stores resulting checkpoint; no single-file or `CHWL` compatibility path exists.
