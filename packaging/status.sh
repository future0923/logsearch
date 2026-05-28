#!/usr/bin/env bash
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PID_FILE="$APP_DIR/run/log-search.pid"
LOG_FILE="$APP_DIR/logs/log-search.log"

if [ ! -f "$PID_FILE" ]; then
  echo "Log Search is not running."
  exit 0
fi

PID="$(cat "$PID_FILE")"
if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
  echo "Log Search is running."
  echo "PID: $PID"
  echo "Log: $LOG_FILE"
  exit 0
fi

rm -f "$PID_FILE"
echo "Log Search is not running."
