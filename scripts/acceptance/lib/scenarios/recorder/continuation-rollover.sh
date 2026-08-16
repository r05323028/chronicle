#!/usr/bin/env bash
# Central-dispatch scenario: continuation-rollover.
scenario_recorder_continuation_rollover() {
    phase continuation_rollover 'verify live multi-epoch continuation handoff and bounded lineage'
    printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
    python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/continuation-workload.json"
    v2_checkpoint_ready() {
        find "$STATE_ROOT/wal" -type f -name 'incremental-etl-checkpoint-v2.json' -print -quit | grep -q .
    }
    wait_until 30 0.2 'successor V2 checkpoint after continuation workload' v2_checkpoint_ready
    local proof="$ARTIFACT_ROOT/continuation-rollover-proof.json"
    python3 - "$STATE_ROOT/wal" "$STORE_ROOT" "$proof" <<'PY'
import json
import sys
from pathlib import Path

wal_root, store_root, proof_path = map(Path, sys.argv[1:])
epoch_dirs = [wal_root]
epoch_dirs.extend(sorted((wal_root / "epochs").glob("*") if (wal_root / "epochs").is_dir() else []))
epoch_dirs = [path for path in epoch_dirs if (path / "recording.json").is_file()]
if len(epoch_dirs) < 2:
    raise SystemExit(f"expected at least two epoch directories, found {len(epoch_dirs)}")

metadata = {path: json.loads((path / "recording.json").read_text()) for path in epoch_dirs}
by_recording_id = {value["recording_id"]: path for path, value in metadata.items()}
continuations = []
for predecessor in epoch_dirs:
    output_path = predecessor / "continuation-out.json"
    if not output_path.is_file():
        continue
    output = json.loads(output_path.read_text())
    successor = by_recording_id.get(output["successor_epoch_id"])
    if successor is None:
        raise SystemExit("continuation successor is not present in authoritative epoch set")
    input_path = successor / "continuation-in.json"
    if not input_path.is_file():
        raise SystemExit("continuation-out has no successor continuation-in")
    if json.loads(input_path.read_text()) != output:
        raise SystemExit("continuation-in/out bytes disagree")
    if output["predecessor_epoch_id"] != metadata[predecessor]["recording_id"]:
        raise SystemExit("continuation predecessor identity mismatch")
    state = output.get("state", [])
    if not isinstance(state, list) or len(state) > 64 * 1024 * 1024:
        raise SystemExit("continuation state exceeds bounded representation")
    if output.get("checksum", "") == "" or len(output["checksum"]) != 64:
        raise SystemExit("continuation checksum missing")
    continuations.append({
        "predecessor": str(predecessor),
        "successor": str(successor),
        "predecessor_epoch_id": output["predecessor_epoch_id"],
        "successor_epoch_id": output["successor_epoch_id"],
        "state_bytes": len(state),
        "checksum": output["checksum"],
    })
if len(continuations) < 3:
    raise SystemExit("fewer than three continuation edges were published")

v2 = []
for epoch in epoch_dirs:
    path = epoch / "incremental-etl-checkpoint-v2.json"
    if not path.is_file():
        continue
    checkpoint = json.loads(path.read_text())
    if checkpoint.get("version") != 2:
        raise SystemExit("unsupported incremental checkpoint version")
    if checkpoint.get("epoch_id") != metadata[epoch]["recording_id"]:
        raise SystemExit("V2 checkpoint epoch identity mismatch")
    dependency = checkpoint.get("continuation", {})
    if dependency.get("state") not in {"ready", "unavailable", "consumed", "failed"}:
        raise SystemExit("V2 checkpoint has invalid continuation state")
    if len(checkpoint.get("decoder", {}).get("state", [])) > 64 * 1024 * 1024:
        raise SystemExit("V2 decoder state exceeds bound")
    v2.append({"epoch": str(epoch), "state": dependency.get("state"), "outputs": len(checkpoint.get("outputs", []))})
if not v2:
    raise SystemExit("no parent-aware V2 checkpoint was published")
ready_v2 = [item for item in v2 if item["state"] == "ready"]
if len(ready_v2) < 2:
    raise SystemExit("fewer than two independent ready V2 dependencies were published")

store_keys = []
for path in store_root.rglob("*") if store_root.is_dir() else []:
    if path.is_file() and "continuation" in path.name:
        store_keys.append(str(path.relative_to(store_root)))
proof = {
    "version": 1,
    "epoch_count": len(epoch_dirs),
    "continuation_count": len(continuations),
    "v2_checkpoint_count": len(v2),
    "continuations": continuations,
    "v2_checkpoints": v2,
    "store_continuation_files": sorted(store_keys),
    "bounded_state_bytes": max(item["state_bytes"] for item in continuations),
}
Path(proof_path).write_text(json.dumps(proof, indent=2, sort_keys=True) + "\n")
PY
    set_check continuation_lineage passed
    set_check continuation_v2_checkpoint passed
    set_check continuation_bounded_state passed

    local invalid_wal="$ARTIFACT_ROOT/invalid-continuation-wal"
    local invalid_store="$ARTIFACT_ROOT/invalid-continuation-store"
    local source_epoch
    source_epoch=$(
        python3 - "$STATE_ROOT/wal" <<'PY'
import sys
from pathlib import Path
root = Path(sys.argv[1])
for path in sorted((root / "epochs").glob("*")):
    if not (path / "recording.json").is_file() or not (path / "continuation-in.json").is_file():
        continue
    if any((path / "segments").glob("*.chwal")):
        print(path)
        break
PY
    )
    test -n "$source_epoch"
    cp -a "$source_epoch" "$invalid_wal"
    python3 - "$invalid_wal/continuation-in.json" <<'PY'
import json
import sys
from pathlib import Path
path = Path(sys.argv[1])
value = json.loads(path.read_text())
value["checksum"] = "0" * 64
path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
PY
    set +e
    "$CHRONICLE" --format json internal etl --wal-dir "$invalid_wal" --output "$invalid_store" \
        >"$ARTIFACT_ROOT/invalid-continuation.json" 2>"$ARTIFACT_ROOT/invalid-continuation.log"
    local invalid_status=$?
    set -e
    if ((invalid_status == 0)) || ! grep -Eqi 'continuation|checksum' "$ARTIFACT_ROOT/invalid-continuation.json" "$ARTIFACT_ROOT/invalid-continuation.log"; then
        cat "$ARTIFACT_ROOT/invalid-continuation.json" "$ARTIFACT_ROOT/invalid-continuation.log" >&2
        exit 1
    fi
    set_check continuation_invalid_fail_closed passed
}
