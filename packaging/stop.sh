#!/usr/bin/env bash
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PID_FILE="$APP_DIR/run/log-search.pid"

if [ ! -f "$PID_FILE" ]; then
  echo "Log Search is not running."
  exit 0
fi

PID="$(cat "$PID_FILE")"
if [ -z "$PID" ] || ! kill -0 "$PID" 2>/dev/null; then
  rm -f "$PID_FILE"
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
