#!/usr/bin/env bash
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PID_FILE="$APP_DIR/run/log-search.pid"
APP_BIN="$APP_DIR/log-search"
APP_BIN_REAL="$(cd "$APP_DIR" && pwd -P)/log-search"
APP_DIR_REAL="$(cd "$APP_DIR" && pwd -P)"

find_running_pid() {
  if [ -f "$PID_FILE" ]; then
    PID="$(cat "$PID_FILE")"
    if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
      return 0
    fi
    rm -f "$PID_FILE"
  fi

  if [ -d /proc ]; then
    for CANDIDATE in $(pgrep -x log-search 2>/dev/null || true); do
      CWD="$(readlink "/proc/$CANDIDATE/cwd" 2>/dev/null || true)"
      if [ "$CWD" = "$APP_DIR_REAL" ]; then
        PID="$CANDIDATE"
        return 0
      fi
    done
  fi

  PID="$(pgrep -f "($APP_BIN|$APP_BIN_REAL)( |$)" 2>/dev/null | head -n 1 || true)"
  if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    return 0
  fi

  PID=""
  return 1
}

if ! find_running_pid; then
  echo "Log Search is not running."
  exit 0
fi

kill "$PID"

for _ in $(seq 1 20); do
  if ! kill -0 "$PID" 2>/dev/null; then
    rm -f "$PID_FILE"
    echo "Log Search stopped."
    exit 0
  fi
  sleep 0.2
done

echo "Log Search did not stop in time. Sending SIGKILL..."
kill -9 "$PID" 2>/dev/null || true
rm -f "$PID_FILE"
echo "Log Search stopped."
