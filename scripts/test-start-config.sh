#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

cp "$ROOT_DIR/packaging/start.sh" "$TMP_DIR/start.sh"
chmod +x "$TMP_DIR/start.sh"
mkdir -p "$TMP_DIR/frontend"

cat > "$TMP_DIR/log-search" <<'FAKE_BINARY'
#!/usr/bin/env bash
printf '%s\n' "$@" > args.txt
FAKE_BINARY
chmod +x "$TMP_DIR/log-search"

assert_args() {
  local expected_config="$1"
  local expected_output="$2"
  shift
  shift

  rm -f "$TMP_DIR/args.txt" "$TMP_DIR/run/log-search.pid"
  local output
  output="$("$@")"

  for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -f "$TMP_DIR/args.txt" ] && break
    sleep 0.1
  done

  local expected
  expected="$(printf '%s\n' --config "$expected_config" --static-dir frontend)"

  if [ ! -f "$TMP_DIR/args.txt" ]; then
    echo "expected fake binary to record arguments" >&2
    exit 1
  fi

  if [ "$(cat "$TMP_DIR/args.txt")" != "$expected" ]; then
    echo "unexpected arguments" >&2
    echo "expected:" >&2
    printf '%s\n' "$expected" >&2
    echo "actual:" >&2
    cat "$TMP_DIR/args.txt" >&2
    exit 1
  fi

  if ! printf '%s\n' "$output" | grep -F "$expected_output" >/dev/null; then
    echo "expected start output to contain: $expected_output" >&2
    echo "actual output:" >&2
    printf '%s\n' "$output" >&2
    exit 1
  fi
}

cat > "$TMP_DIR/config.toml" <<'EOF'
[server]
addr = "127.0.0.1:12457"
EOF
assert_args "config.toml" "Open: http://127.0.0.1:12457" "$TMP_DIR/start.sh"

cat > "$TMP_DIR/env-config.toml" <<'EOF'
[server]
addr = "0.0.0.0:12457"
EOF
assert_args "env-config.toml" "Listening: 0.0.0.0:12457" env CONFIG_FILE=env-config.toml "$TMP_DIR/start.sh"

cat > "$TMP_DIR/arg-config.toml" <<'EOF'
[server]
addr = "192.168.0.10:12457"
EOF
assert_args "arg-config.toml" "Open: http://192.168.0.10:12457" env CONFIG_FILE=env-config.toml "$TMP_DIR/start.sh" arg-config.toml

echo "start.sh config override tests passed."
