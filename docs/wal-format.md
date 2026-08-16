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

## Commit authority and group commit

`GroupCommitWalWriter` batches non-marker envelopes, appends one fixed 76-byte `CommitMarker`, flushes, and calls one `fdatasync` on the current segment. Triggers are 4 MiB unsynced data, 10 ms, rotation, explicit flush, or shutdown. Runtime durability acknowledgement is sent/recorded only after sync succeeds. There is no external watermark file, second metadata-watermark cycle, per-record sync, or user-facing durability mode.

The marker carries cumulative durable sequence/count/bytes, batch bounds, and SHA-256 over exact placed framed bytes. Recovery authority requires complete marker/frame CRCs, contiguous same-segment references, exact boundaries, cumulative totals, recording identity, versions, and digest. A valid marker proves persisted WAL durability only; recovery never reconstructs whether a caller observed its acknowledgement. Complete records after the final valid marker remain visible as uncommitted suffix and never enter ETL.

Writer reserves a complete data frame plus final marker in current and total physical capacity before writing. Rotation also reserves the next header and temporary publication peak. Segment size is bounded from 16 MiB through 4 GiB; total bytes under one epoch's `segments/` directory default to and never exceed 4 GiB. A parent recording may own many such epochs; this does not change WAL v1 bytes or marker authority. Unix directories/files use `0700`/`0600`. Queue-limit discard is recorder admission loss, not kernel loss, and is represented by typed terminal WAL-loss evidence or metadata-only summary.

## Recovery

Recovery locks the recording, scans numeric segments, validates every header/envelope and marker chain, and returns:

- authoritative committed envelopes through final valid marker;
- uncommitted complete suffix and written-not-durable uncertainty;
- terminal WAL-loss evidence;
- optional incomplete tail only on final segment.

Repair truncates only a verified incomplete final frame (including a partial final marker), syncs the file and directory, and records a warning. Complete corruption, invalid references/digests, sequence gaps, identity mismatches, and unsupported versions fail closed. Reopen resumes after final authoritative marker, removes only reported uncommitted suffixes, preserves sequence continuity, and never infers acknowledgement delivery from bytes alone.

Fixture and production recording use same segmented, group-committed v1 path. ETL reads only the recovered committed boundary; the filesystem manifest/checkpoint stores provenance and digest, not an external durability watermark. No single-file or `CHWL` compatibility path exists.
