#!/usr/bin/env bash
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_FILE="${1:-${CONFIG_FILE:-config.toml}}"
PID_FILE="$APP_DIR/run/log-search.pid"
LOG_FILE="$APP_DIR/logs/log-search.log"

mkdir -p "$APP_DIR/run" "$APP_DIR/logs"
cd "$APP_DIR"

if [ ! -x "$APP_DIR/log-search" ]; then
  echo "log-search binary not found or not executable: $APP_DIR/log-search" >&2
  exit 1
fi

if [ ! -f "$CONFIG_FILE" ]; then
  echo "config file not found: $CONFIG_FILE" >&2
  exit 1
fi

if [ -f "$PID_FILE" ]; then
  PID="$(cat "$PID_FILE")"
  if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    echo "Log Search is already running."
    echo "PID: $PID"
    echo "Log: $LOG_FILE"
    exit 0
  fi
  rm -f "$PID_FILE"
fi

nohup ./log-search --config "$CONFIG_FILE" --static-dir frontend >> "$LOG_FILE" 2>&1 &
PID="$!"
echo "$PID" > "$PID_FILE"

echo "Log Search started."
echo "PID: $PID"
echo "Log: $LOG_FILE"
echo "Open: http://127.0.0.1:12457"
