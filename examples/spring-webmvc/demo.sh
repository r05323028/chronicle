#!/usr/bin/env bash
# Chronicle x Spring WebMVC demo: record real traffic, then replay it.
# Usage: ./demo.sh build|record|replay|clean
#
# record/replay need a Linux host with chronicle live capture. Run the script
# with sudo when chronicle has no ambient capabilities: the script then uses
# the separated supervisor (effective root + non-root real IDs) exactly like
# the privileged acceptance scenarios.
set -euo pipefail
cd "$(dirname "$0")"

PORT="${PORT:-18080}"
JAR="target/webmvc.jar"
DATA_DIR="$PWD/target/chronicle-data"
CHRONICLE_BIN="$(command -v chronicle)"

# Effective-root runs keep the real IDs at the invoking (non-root) user so the
# supervised cgroup keeps root-only membership-control separation.
SUPERVISOR=""
if [ "$(id -u)" = "0" ]; then
  TARGET_UID=$(stat -c '%u' "$(cd "$(dirname "$0")" && pwd)/demo.sh")
  TARGET_GID=$(stat -c '%g' "$(cd "$(dirname "$0")" && pwd)/demo.sh")
  if [ "$TARGET_UID" = "0" ]; then
    echo "cannot supervise a root-owned checkout; run the demo from a user-owned path" >&2
    exit 1
  fi
  SUPERVISOR=1
fi

chronicle() {
  # Separated supervisor: effective root + real IDs at the target user, so the
  # supervised cgroup keeps root-only membership-control separation (same
  # shape as scripts/acceptance/separated-supervisor.py).
  if [ -n "$SUPERVISOR" ]; then
    exec sudo python3 -c 'import ctypes, os, sys
u = int(sys.argv[1]); g = int(sys.argv[2])
libc = ctypes.CDLL(None, use_errno=True)
libc.setresgid(g, 0, 0)
libc.setresuid(u, 0, 0)
os.execv(sys.argv[3], sys.argv[3:])' "$TARGET_UID" "$TARGET_GID" "$CHRONICLE_BIN" --data-dir "$DATA_DIR" "$@"
  else
    exec "$CHRONICLE_BIN" --data-dir "$DATA_DIR" "$@"
  fi
}

case "${1:-}" in
  build)
    mvn -q package
    ;;
  record)
    # Replay refuses the exact recorded destination and command mode infers a
    # loopback listener on the recorded port, so traffic must reach the app on
    # a NON-loopback address (the host's primary IP) while the replay copy
    # later binds loopback.
    HOST_IP=$(hostname -I 2>/dev/null | awk '{print $1}')
    if [ -z "$HOST_IP" ] || [ "$HOST_IP" = "127.0.0.1" ]; then
      echo "no non-loopback host address; recorded traffic must be non-loopback" >&2
      exit 1
    fi
    chronicle record --name webmvc-demo -- java -jar "$JAR" --server.port="$PORT" &
    rec=$!
    trap 'kill -TERM "$rec" 2>/dev/null || true' INT TERM
    for _ in $(seq 1 60); do
      curl -fsS "http://127.0.0.1:$PORT/hello" >/dev/null 2>&1 && break
      sleep 0.5
    done
    curl -fsS "http://$HOST_IP:$PORT/hello"; echo
    curl -fsS -X POST -H 'Content-Type: text/plain' --data 'echo-me' "http://$HOST_IP:$PORT/echo"; echo
    kill -TERM "$rec"
    wait "$rec"
    chronicle inspect latest
    ;;
  replay)
    # Spring binds 0.0.0.0 by default; the replay copy must expose a loopback
    # listener on the recorded port for command-mode listener discovery.
    chronicle replay latest -- java -jar "$JAR" --server.address=127.0.0.1 --server.port="$PORT" --allow-write
    ;;
  clean)
    mvn -q clean
    rm -rf "$DATA_DIR"
    ;;
  *)
    sed -n '2,6p' "$0"
    exit 1
    ;;
esac
