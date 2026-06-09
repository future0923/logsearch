#!/usr/bin/env bash
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_FILE="${1:-${CONFIG_FILE:-config.toml}}"
PID_FILE="$APP_DIR/run/log-search.pid"
LOG_FILE="$APP_DIR/logs/log-search.log"

server_addr() {
  awk '
    /^\[server\][[:space:]]*$/ { in_server = 1; next }
    /^\[/ { in_server = 0 }
    in_server && /^[[:space:]]*addr[[:space:]]*=/ {
      value = $0
      sub(/^[^=]*=[[:space:]]*/, "", value)
      gsub(/"/, "", value)
      gsub(/[[:space:]]/, "", value)
      print value
      exit
    }
  ' "$CONFIG_FILE"
}

print_open_hint() {
  local addr="$1"
  local host port

  if [ -z "$addr" ]; then
    addr="0.0.0.0:12457"
  fi

  host="${addr%:*}"
  port="${addr##*:}"

  echo "Listening: $addr"
  if [ "$host" = "0.0.0.0" ]; then
    echo "Open locally: http://127.0.0.1:$port"
    echo "Open from LAN: http://<server-ip>:$port"
  else
    echo "Open: http://$host:$port"
  fi
}

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
print_open_hint "$(server_addr)"
