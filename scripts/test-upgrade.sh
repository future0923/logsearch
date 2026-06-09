#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT_DIR/packaging/upgrade.sh"
TMP_DIR="$(mktemp -d)"

cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_file() {
  [ -f "$1" ] || fail "expected file: $1"
}

assert_dir() {
  [ -d "$1" ] || fail "expected directory: $1"
}

assert_not_exists() {
  [ ! -e "$1" ] || fail "expected missing path: $1"
}

assert_content() {
  local path="$1"
  local expected="$2"
  local actual
  actual="$(cat "$path")"
  [ "$actual" = "$expected" ] || fail "expected $path to contain '$expected', got '$actual'"
}

make_release() {
  local version="$1"
  local include_upgrade="${2:-1}"
  local root="$TMP_DIR/releases/$version"
  local release_dir="$root/log-search_${version}_linux_amd64"
  mkdir -p "$release_dir/frontend" "$root"
  printf 'binary-%s' "$version" > "$release_dir/log-search"
  printf 'frontend-%s' "$version" > "$release_dir/frontend/index.html"
  printf 'config-%s' "$version" > "$release_dir/config.toml"
  printf 'readme-%s' "$version" > "$release_dir/README.txt"
  cat > "$release_dir/start.sh" <<EOF
#!/usr/bin/env bash
echo start-$version >> "\${LOG_SEARCH_LIFECYCLE_LOG:-lifecycle.log}"
echo "config=\${CONFIG_FILE:-}" >> "\${LOG_SEARCH_LIFECYCLE_LOG:-lifecycle.log}"
EOF
  cat > "$release_dir/stop.sh" <<EOF
#!/usr/bin/env bash
echo stop-$version >> "\${LOG_SEARCH_LIFECYCLE_LOG:-lifecycle.log}"
EOF
  cat > "$release_dir/status.sh" <<EOF
#!/usr/bin/env bash
echo status-$version
EOF
  if [ "$include_upgrade" = "1" ]; then
    printf 'upgrade-%s' "$version" > "$release_dir/upgrade.sh"
  fi
  printf 'service-%s' "$version" > "$release_dir/log-search.service"
  chmod +x "$release_dir/log-search" "$release_dir/start.sh" "$release_dir/stop.sh" "$release_dir/status.sh"
  if [ -f "$release_dir/upgrade.sh" ]; then
    chmod +x "$release_dir/upgrade.sh"
  fi
  tar -C "$root" -czf "$root/log-search_${version}_linux_amd64.tar.gz" "log-search_${version}_linux_amd64"
}

make_app() {
  local app="$1"
  mkdir -p "$app/frontend" "$app/data" "$app/logs" "$app/run" "$app/backups" "$app/old-only"
  printf 'old-binary' > "$app/log-search"
  printf 'old-frontend' > "$app/frontend/index.html"
  printf 'user-config' > "$app/config.toml"
  printf 'user-index' > "$app/data/index.bin"
  printf 'user-log' > "$app/logs/log-search.log"
  printf '123' > "$app/run/log-search.pid"
  printf 'old-backup' > "$app/backups/keep.txt"
  printf 'remove-me' > "$app/old-only/file.txt"
  cat > "$app/start.sh" <<'EOF'
#!/usr/bin/env bash
echo start >> "${LOG_SEARCH_LIFECYCLE_LOG:-lifecycle.log}"
echo "config=${CONFIG_FILE:-}" >> "${LOG_SEARCH_LIFECYCLE_LOG:-lifecycle.log}"
EOF
  cat > "$app/stop.sh" <<'EOF'
#!/usr/bin/env bash
echo stop >> "${LOG_SEARCH_LIFECYCLE_LOG:-lifecycle.log}"
EOF
  chmod +x "$app/start.sh" "$app/stop.sh"
}

make_release "0.2.0"
make_release "0.3.0"
make_release "0.4.0" 0

LATEST_FILE="$TMP_DIR/latest.json"
printf '{"tag_name":"v0.3.0"}' > "$LATEST_FILE"

APP_LATEST="$TMP_DIR/app-latest"
make_app "$APP_LATEST"
(
  cd "$APP_LATEST"
  LOG_SEARCH_UPGRADE_BASE_URL="file://$TMP_DIR/releases" \
  LOG_SEARCH_LATEST_URL="file://$LATEST_FILE" \
  LOG_SEARCH_SYSTEM=linux \
  LOG_SEARCH_ARCH=amd64 \
  "$SCRIPT"
)

assert_content "$APP_LATEST/log-search" "binary-0.3.0"
assert_content "$APP_LATEST/frontend/index.html" "frontend-0.3.0"
assert_content "$APP_LATEST/config.toml" "user-config"
assert_content "$APP_LATEST/config.toml.new" "config-0.3.0"
assert_content "$APP_LATEST/data/index.bin" "user-index"
assert_content "$APP_LATEST/logs/log-search.log" "user-log"
assert_content "$APP_LATEST/backups/keep.txt" "old-backup"
assert_not_exists "$APP_LATEST/old-only"
assert_file "$APP_LATEST/upgrade.sh"
assert_dir "$APP_LATEST/backups"

APP_PINNED="$TMP_DIR/app-pinned"
make_app "$APP_PINNED"
(
  cd "$APP_PINNED"
  LOG_SEARCH_UPGRADE_BASE_URL="file://$TMP_DIR/releases" \
  LOG_SEARCH_SYSTEM=linux \
  LOG_SEARCH_ARCH=amd64 \
  "$SCRIPT" v0.2.0
)

assert_content "$APP_PINNED/log-search" "binary-0.2.0"
assert_content "$APP_PINNED/config.toml" "user-config"
assert_content "$APP_PINNED/config.toml.new" "config-0.2.0"

APP_LEGACY_RELEASE="$TMP_DIR/app-legacy-release"
make_app "$APP_LEGACY_RELEASE"
cp "$SCRIPT" "$APP_LEGACY_RELEASE/upgrade.sh"
chmod +x "$APP_LEGACY_RELEASE/upgrade.sh"
(
  cd "$APP_LEGACY_RELEASE"
  LOG_SEARCH_UPGRADE_BASE_URL="file://$TMP_DIR/releases" \
  LOG_SEARCH_SYSTEM=linux \
  LOG_SEARCH_ARCH=amd64 \
  ./upgrade.sh v0.4.0
)

assert_file "$APP_LEGACY_RELEASE/upgrade.sh"

APP_FALLBACK="$TMP_DIR/app-fallback"
make_app "$APP_FALLBACK"
(
  cd "$APP_FALLBACK"
  LOG_SEARCH_UPGRADE_BASE_URL="file://$TMP_DIR/releases" \
  LOG_SEARCH_SYSTEM=linux \
  LOG_SEARCH_ARCH=amd64 \
  PATH="/usr/bin:/bin" \
  "$SCRIPT" v0.2.0
)

assert_content "$APP_FALLBACK/log-search" "binary-0.2.0"
assert_content "$APP_FALLBACK/config.toml" "user-config"
assert_content "$APP_FALLBACK/data/index.bin" "user-index"
assert_content "$APP_FALLBACK/logs/log-search.log" "user-log"
assert_content "$APP_FALLBACK/backups/keep.txt" "old-backup"
assert_not_exists "$APP_FALLBACK/old-only"

APP_SYSTEMD="$TMP_DIR/app-systemd"
SYSTEMD_DIR="$TMP_DIR/systemd"
FAKE_BIN="$TMP_DIR/fake-bin"
EXTERNAL_CONFIG="$TMP_DIR/external-config.toml"
mkdir -p "$SYSTEMD_DIR" "$FAKE_BIN"
make_app "$APP_SYSTEMD"
printf 'external-config' > "$EXTERNAL_CONFIG"
APP_SYSTEMD_REAL="$(cd "$APP_SYSTEMD" && pwd -P)"
cat > "$APP_SYSTEMD/log-search.service" <<EOF
[Unit]
Description=Log Search

[Service]
WorkingDirectory=$APP_SYSTEMD_REAL
ExecStart=$APP_SYSTEMD_REAL/log-search --config $EXTERNAL_CONFIG --static-dir $APP_SYSTEMD_REAL/frontend
EOF
cp "$APP_SYSTEMD/log-search.service" "$SYSTEMD_DIR/log-search.service"
cat > "$FAKE_BIN/systemctl" <<'EOF'
#!/usr/bin/env bash
if [ "$1" = "is-active" ]; then
  [ "${LOG_SEARCH_FAKE_SYSTEMD_ACTIVE:-0}" = "1" ]
  exit
fi
echo "$*" >> "$LOG_SEARCH_FAKE_SYSTEMCTL_LOG"
exit 0
EOF
chmod +x "$FAKE_BIN/systemctl"
(
  cd "$APP_SYSTEMD"
  LOG_SEARCH_UPGRADE_BASE_URL="file://$TMP_DIR/releases" \
  LOG_SEARCH_SYSTEM=linux \
  LOG_SEARCH_ARCH=amd64 \
  LOG_SEARCH_ASSUME_ROOT=1 \
  LOG_SEARCH_FAKE_SYSTEMD_ACTIVE=1 \
  LOG_SEARCH_SYSTEMD_DIR="$SYSTEMD_DIR" \
  LOG_SEARCH_FAKE_SYSTEMCTL_LOG="$TMP_DIR/systemctl.log" \
  LOG_SEARCH_LIFECYCLE_LOG="$TMP_DIR/systemd-lifecycle.log" \
  PATH="$FAKE_BIN:/usr/bin:/bin" \
  "$SCRIPT" v0.2.0
)

assert_file "$SYSTEMD_DIR/log-search.service"
grep -F "WorkingDirectory=$APP_SYSTEMD_REAL" "$SYSTEMD_DIR/log-search.service" >/dev/null || fail "systemd WorkingDirectory was not rewritten"
grep -F "ExecStart=$APP_SYSTEMD_REAL/log-search --config $EXTERNAL_CONFIG --static-dir $APP_SYSTEMD_REAL/frontend" "$SYSTEMD_DIR/log-search.service" >/dev/null || fail "systemd ExecStart did not preserve existing config path"
grep -F "stop log-search" "$TMP_DIR/systemctl.log" >/dev/null || fail "systemd stop was not called"
grep -F "start log-search" "$TMP_DIR/systemctl.log" >/dev/null || fail "systemd start was not called"
grep -F "stop" "$TMP_DIR/systemd-lifecycle.log" >/dev/null || fail "legacy stop.sh was not called before systemd start"

APP_PARENT_CONFIG_ROOT="$TMP_DIR/parent-config-root"
APP_PARENT="$APP_PARENT_CONFIG_ROOT/log-search_0.1.0_linux_amd64"
PARENT_SYSTEMD_DIR="$TMP_DIR/parent-systemd"
mkdir -p "$PARENT_SYSTEMD_DIR"
make_app "$APP_PARENT"
APP_PARENT_REAL="$(cd "$APP_PARENT" && pwd -P)"
PARENT_CONFIG="$APP_PARENT_CONFIG_ROOT/config.toml"
printf 'parent-config' > "$PARENT_CONFIG"
cat > "$PARENT_SYSTEMD_DIR/log-search.service" <<EOF
[Unit]
Description=Log Search

[Service]
WorkingDirectory=$APP_PARENT_REAL
ExecStart=$APP_PARENT_REAL/log-search --config $APP_PARENT_REAL/config.toml --static-dir $APP_PARENT_REAL/frontend
EOF
(
  cd "$APP_PARENT"
  LOG_SEARCH_UPGRADE_BASE_URL="file://$TMP_DIR/releases" \
  LOG_SEARCH_SYSTEM=linux \
  LOG_SEARCH_ARCH=amd64 \
  LOG_SEARCH_ASSUME_ROOT=1 \
  LOG_SEARCH_PREVIOUS_MODE=script \
  LOG_SEARCH_PREVIOUS_CMDLINE="$APP_PARENT_REAL/log-search --config $PARENT_CONFIG --static-dir frontend" \
  LOG_SEARCH_SYSTEMD_DIR="$PARENT_SYSTEMD_DIR" \
  LOG_SEARCH_FAKE_SYSTEMCTL_LOG="$TMP_DIR/parent-systemctl.log" \
  LOG_SEARCH_LIFECYCLE_LOG="$TMP_DIR/parent-lifecycle.log" \
  PATH="$FAKE_BIN:/usr/bin:/bin" \
  "$SCRIPT" v0.2.0
)

grep -F "start" "$TMP_DIR/parent-lifecycle.log" >/dev/null || fail "script run mode was not preserved"
grep -F "config=$PARENT_CONFIG" "$TMP_DIR/parent-lifecycle.log" >/dev/null || fail "script run mode did not preserve previous config"
grep -F "start log-search" "$TMP_DIR/parent-systemctl.log" >/dev/null && fail "systemd was started even though previous mode was script"

echo "upgrade.sh tests passed."
