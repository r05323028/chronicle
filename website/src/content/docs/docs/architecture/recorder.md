---
title: Recorder
description: The recording lifecycle around capture and WAL ownership.
---

A recording lifecycle owns one capture scope, one WAL domain, and one finalization path. Public users normally start with command mode:

```bash
chronicle record --name checkout -- ./my-app
```

## Command-mode lifecycle

1. Resolve and lock the public data directory.
2. Prepare the recording identity and bounded WAL domain.
3. Attach the capture source before starting a supervised command.
4. Admit normalized events into a bounded queue.
5. Group-commit evidence to WAL and make loss visible.
6. Stop on process exit, signal, an optional whole-recording deadline, or fatal capture/storage failure; epoch and physical WAL limits trigger rollover instead of ordinary termination.
7. Recover the authoritative WAL prefix.
8. Run ETL and publish one immutable canonical session per finalized epoch, retaining parent/epoch provenance.
9. Update the advisory catalog only after canonical publication.

A finalization failure does not require recapturing when the recording is recoverable:

```bash
chronicle record --retry checkout
```

## Continuous recorder

Command, PID, cgroup, and daemon modes share one continuous coordinator. It owns one filesystem domain, bounded epoch rotation, incremental ETL/continuation resume, liveness/health metadata, and shutdown cleanup; capture may continue while predecessor ETL lags.

This is not an always-on distributed capture service. Recorder state, WAL, manifests, checkpoints, and catalog facts remain local and bounded. Consult the repository's [recorder runbook](https://github.com/r05323028/chronicle/blob/main/docs/operations/recorder-runbook.md) before operating that advanced path. The current local deployment co-locates the recorder runtime with incremental ETL; that is a topology choice, not logical ownership — ETL remains an independent boundary (see [ETL](../etl/)). One `.chronicle-domain.lock` protects the local filesystem coordination domain; it is not the architectural ownership mechanism between Recorder and ETL.

## Stop and recovery

The first termination signal drains and finalizes within configured bounds. A forced termination or unsafe successor-capacity failure remains visible in recording metadata and WAL-loss evidence; an epoch threshold alone requests rollover. Recovery repairs only a verified incomplete final tail; it does not hide complete corruption or invent acknowledgement history.
