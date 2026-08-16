# Continuous recorder runbook

## Preflight

1. Create dedicated `chronicle` user and state directory.
2. Keep recorder state/store outside captured cgroup subtree.
3. Validate config and placement:

```sh
chronicle --format json --data-dir /var/lib/chronicle doctor
```

1. Confirm supported kernel, cgroup v2, BTF, required capabilities, and free space.

## Start and readiness

```sh
systemctl start chronicle-recorder
chronicle --format json internal recorder-status --state-root /var/lib/chronicle
systemctl status chronicle-recorder --no-pager
```

Require `liveness=true`, `capture_readiness="ready"`, and `health="healthy"`. `processing_readiness="unknown"` means capture may run while ETL recovers.

## Stop and restart

```sh
systemctl stop chronicle-recorder
systemctl restart chronicle-recorder
journalctl -u chronicle-recorder -b --no-pager
```

First SIGINT/SIGTERM drains. A second signal forces termination and leaves crash-recovery metadata. Omitted `--duration` means no whole-recording deadline; epoch and segment bounds still force safe rollover. Never delete WAL or state files manually while lease is held.

## ETL lag and disk pressure

Check status counters and quota/free bytes. Stop new admission when minimum-free reserve is threatened. Resolve eligible cleanup only after checkpoint and finalized-session integrity verification. Lag never authorizes cleanup. A successor may capture while predecessor ETL waits for verified continuation; this affects processing readiness, not capture admission.

## Corruption and recovery

Preserve contradictory manifest, checkpoint, or digest artifacts in place; they remain non-authoritative. Preserve WAL commit markers and tombstones. Restart recorder; inspect status remediation and recovery evidence before resuming capture.

## Evidence export

Retain machine-readable status, recovery reports, manifests, checkpoints, tombstones, command logs, SHA-256 manifests, acceptance fingerprint, source commit/tree provenance, and kernel/architecture/capability data under runtime-only `target/validation-evidence/acceptance/<profile>/<fingerprint>/<run-id>/`. Redact payloads and credentials. Require clean, known current source that remains unchanged for release-grade evidence.

## Upgrade and rollback

Stop recorder, retain state/store/WAL, deploy exact artifact, run doctor, then start. Roll back binary/config only after schema and lineage validation. Do not mix state from different configuration digests.

## Ownership

- Platform owner: kernel, cgroup v2, BTF, capabilities, systemd, disk.
- Application owner: config, leases, lifecycle, WAL, ETL, retention, quota.
- Chronicle operator: readiness, lag, recovery, evidence, upgrade/rollback.
