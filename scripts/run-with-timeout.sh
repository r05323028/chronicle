#!/usr/bin/env bash
# Run one command beneath a hard process-tree deadline.
set -euo pipefail

usage() {
	printf 'Usage: %s <seconds> <command> [arguments...]\n' "$0" >&2
}

if (($# < 2)); then
	usage
	exit 2
fi
TIMEOUT=$1
shift
GRACE=${CHRONICLE_TIMEOUT_GRACE_SECONDS:-5}
[[ $TIMEOUT =~ ^[1-9][0-9]*$ ]] || {
	printf 'ERROR: timeout must be a positive integer, got %q\n' "$TIMEOUT" >&2
	exit 2
}
[[ $GRACE =~ ^[0-9]+$ ]] || {
	printf 'ERROR: CHRONICLE_TIMEOUT_GRACE_SECONDS must be a non-negative integer, got %q\n' "$GRACE" >&2
	exit 2
}
command -v python3 >/dev/null 2>&1 || {
	printf 'ERROR: python3 is required by %s\n' "$0" >&2
	exit 2
}

exec python3 -c '
import ctypes
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

seconds = int(sys.argv[1])
grace = int(sys.argv[2])
command = sys.argv[3:]


def enable_subreaper():
    if not sys.platform.startswith("linux"):
        return
    try:
        # Linux prctl(PR_SET_CHILD_SUBREAPER): orphaned descendants reparent here.
        ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0)
    except (AttributeError, OSError):
        pass


def process_table():
    rows = []
    proc = Path("/proc")
    if proc.is_dir():
        for entry in proc.iterdir():
            if not entry.name.isdigit():
                continue
            try:
                text = (entry / "stat").read_text(encoding="utf-8")
                tail = text[text.rfind(")") + 2 :].split()
                rows.append((int(entry.name), int(tail[1])))
            except (OSError, ValueError, IndexError):
                pass
        return rows
    try:
        output = subprocess.check_output(
            ["ps", "-axo", "pid=,ppid="],
            text=True,
            stderr=subprocess.DEVNULL,
            timeout=2,
        )
        for line in output.splitlines():
            pid, ppid = line.split()
            rows.append((int(pid), int(ppid)))
    except (OSError, ValueError, subprocess.SubprocessError):
        pass
    return rows


def descendants(root, rows=None):
    children = {}
    for pid, ppid in process_table() if rows is None else rows:
        children.setdefault(ppid, []).append(pid)
    found = []
    pending = [root]
    while pending:
        direct = children.get(pending.pop(), [])
        found.extend(direct)
        pending.extend(direct)
    return found


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    stat = Path(f"/proc/{pid}/stat")
    if stat.is_file():
        try:
            return stat.read_text(encoding="utf-8").split(") ", 1)[1][0] != "Z"
        except (OSError, IndexError):
            pass
    return True


def refresh(proc, tracked):
    rows = process_table()
    tracked.update(descendants(proc.pid, rows))
    # Subreaper-adopted children are direct descendants of this wrapper.
    tracked.update(descendants(os.getpid(), rows))
    tracked.add(proc.pid)
    tracked.discard(os.getpid())


def reap_adopted():
    while True:
        try:
            pid, _ = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return
        if pid == 0:
            return


def signal_tree(proc, sig, tracked):
    refresh(proc, tracked)
    try:
        os.killpg(proc.pid, sig)
    except ProcessLookupError:
        pass
    for pid in reversed(sorted(tracked)):
        try:
            os.kill(pid, sig)
        except ProcessLookupError:
            pass


def terminate_tree(proc, first_signal, tracked):
    signal_tree(proc, first_signal, tracked)
    deadline = time.monotonic() + grace
    while time.monotonic() < deadline:
        refresh(proc, tracked)
        reap_adopted()
        survivors = [pid for pid in tracked if pid != proc.pid and alive(pid)]
        if proc.poll() is not None and not survivors:
            return
        time.sleep(0.05)
    signal_tree(proc, signal.SIGKILL, tracked)
    deadline = time.monotonic() + 1
    while time.monotonic() < deadline:
        reap_adopted()
        if not any(alive(pid) for pid in tracked):
            break
        time.sleep(0.02)
    try:
        proc.wait(timeout=1)
    except subprocess.TimeoutExpired:
        pass
    reap_adopted()


def evidence():
    path = os.environ.get("CHRONICLE_TIMEOUT_EVIDENCE_FILE")
    if not path:
        return
    value = {
        "timeout_layer": os.environ.get("CHRONICLE_TIMEOUT_LAYER", "command"),
        "name": os.environ.get("CHRONICLE_TIMEOUT_NAME", command[0]),
        "configured_timeout_seconds": seconds,
        "command": command,
        "exit_code": 124,
    }
    phase = os.environ.get("CHRONICLE_TIMEOUT_PHASE", "").strip()
    phase_file = os.environ.get("CHRONICLE_TIMEOUT_PHASE_FILE")
    if not phase and phase_file:
        try:
            phase = Path(phase_file).read_text(encoding="utf-8").strip()
        except OSError:
            pass
    if phase:
        value["phase"] = phase
    target = Path(path)
    diagnostics = []
    for configured in os.environ.get("CHRONICLE_TIMEOUT_DIAGNOSTICS", "").split(":"):
        if not configured:
            continue
        candidate = Path(configured)
        if not candidate.is_absolute():
            candidate = target.parent / candidate
        if candidate.exists():
            diagnostics.append(str(candidate.relative_to(target.parent)))
    if diagnostics:
        value["diagnostics"] = diagnostics
    try:
        target.parent.mkdir(parents=True, exist_ok=True)
        tmp = target.with_name(target.name + f".{os.getpid()}.tmp")
        tmp.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        os.replace(tmp, target)
    except OSError as exc:
        print(f"WARNING: could not write timeout evidence {target}: {exc}", file=sys.stderr)


enable_subreaper()
try:
    child = subprocess.Popen(command, start_new_session=True)
except (FileNotFoundError, PermissionError, OSError) as exc:
    print(f"ERROR: cannot execute {command[0]!r}: {exc}", file=sys.stderr)
    raise SystemExit(2)
tracked = {child.pid}


def relay(signum, _frame):
    for forwarded in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(forwarded, signal.SIG_IGN)
    terminate_tree(child, signum, tracked)
    raise SystemExit(128 + signum)


signal.signal(signal.SIGTERM, relay)
signal.signal(signal.SIGINT, relay)
signal.signal(signal.SIGHUP, relay)
deadline = time.monotonic() + seconds
refresh_interval = 0.25 if sys.platform.startswith("linux") else 5.0
next_refresh = 0.0
status = None
while time.monotonic() < deadline:
    now = time.monotonic()
    if now >= next_refresh:
        refresh(child, tracked)
        next_refresh = now + refresh_interval
    status = child.poll()
    if status is not None:
        break
    time.sleep(0.05)
if status is None:
    evidence()
    print(
        f"ERROR: command timed out after {seconds}s: "
        + " ".join(repr(part) for part in command),
        file=sys.stderr,
        flush=True,
    )
    terminate_tree(child, signal.SIGTERM, tracked)
    raise SystemExit(124)
refresh(child, tracked)
survivors = [pid for pid in tracked if pid != child.pid and alive(pid)]
if survivors:
    terminate_tree(child, signal.SIGTERM, tracked)
else:
    reap_adopted()
raise SystemExit(status if status >= 0 else 128 - status)
' "$TIMEOUT" "$GRACE" "$@"
