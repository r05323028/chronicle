# WAL Format

## Current v1 record

All integers are little-endian.

```text
magic[4] = CHWL
version:u16 = 1
kind:u16
flags:u16
reserved:u16
payload_len:u32
sequence:u64
crc32c:u32
payload[payload_len]
```

CRC32C covers bytes from `version` through `sequence`, followed by payload. It excludes magic and checksum field. Current record kind 1 is a capture-event envelope. Current envelope encoding is versioned JSON for scaffold testing; WAL framing is encoding-independent and JSON is not claimed as final production wire format.

Segment names are `segment-{first_sequence:020}.wal`. Writer rotates before an append that would exceed configured size (except a single oversized record), syncs previous segment, enforces consecutive sequence numbers, rejects `u64::MAX` because checkpoint format cannot represent its successor, and never depends on remote storage. On Unix, WAL directories are forced to mode `0700` and new segment files to `0600`; non-Unix deployments must enforce equivalent ACLs.

Reader outcomes distinguish complete record, clean end, and partial tail. Partial header or payload returns `PartialTail` at record start without advancing checkpoint. Reader rejects unexpected sequence numbers and records above its explicit allocation limit before allocating payload memory. `WalReader::new` uses a safe 16 MiB record limit; deployments may set a different bound with `with_max_record_bytes`. A complete record with bad CRC is corruption and returns a typed error.

Checkpoint model records segment first sequence, byte offset after last valid record, and next expected sequence.

## Recovery boundary

Current reader safely identifies partial tails; it does not mutate or truncate them. Current writer creates new segments and does not resume an existing directory. Therefore crash restart repair is not implemented yet.

## Planned MVP

- scan and validate segments at startup;
- quarantine/report corruption;
- truncate only verified partial final record after explicit recovery policy;
- persist reader checkpoints atomically;
- idempotent ETL commits before checkpoint advance;
- retention and archival after all consumers acknowledge segments;
- disk limit policy with visible capture backpressure/failure.

WAL replication and embedded databases are out of scope.
