#!/usr/bin/env bash
set -euo pipefail

GITHUB_OWNER="${LOG_SEARCH_GITHUB_OWNER:-future0923}"
GITHUB_REPO="${LOG_SEARCH_GITHUB_REPO:-logsearch}"
GITEE_OWNER="${LOG_SEARCH_GITEE_OWNER:-future94}"
GITEE_REPO="${LOG_SEARCH_GITEE_REPO:-logsearch}"
MIRROR="${LOG_SEARCH_MIRROR:-auto}"
TARGET_VERSION="${1:-latest}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CURRENT_DIR="$(pwd -P)"
APP_DIR="${LOG_SEARCH_APP_DIR:-}"
SYSTEMD_DIR="${LOG_SEARCH_SYSTEMD_DIR:-/etc/systemd/system}"
WORK_DIR=""
PREVIOUS_MODE="script"
PREVIOUS_CONFIG_FILE=""
PREVIOUS_STATIC_DIR=""

cleanup() {
  if [ -n "$WORK_DIR" ]; then
    rm -rf "$WORK_DIR"
  fi
}
trap cleanup EXIT

die() {
  echo "upgrade failed: $*" >&2
  exit 1
}

need_command() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

resolve_app_dir() {
  if [ -n "$APP_DIR" ]; then
    cd "$APP_DIR" && pwd -P
    return
  fi

  if [ -f "$CURRENT_DIR/config.toml" ] || [ -d "$CURRENT_DIR/data" ] || [ -x "$CURRENT_DIR/log-search" ]; then
    echo "$CURRENT_DIR"
    return
  fi

  echo "$SCRIPT_DIR"
}

cmd_arg_value() {
  local cmdline="$1"
  local option="$2"
  printf '%s\n' "$cmdline" | sed -n "s/.*$option[[:space:]]\\{1,\\}\\([^[:space:]]\\{1,\\}\\).*/\\1/p" | head -n 1
}

read_pid_cmdline() {
  local pid="$1"

  if [ -r "/proc/$pid/cmdline" ]; then
    tr '\0' ' ' < "/proc/$pid/cmdline"
    return
  fi

  ps -p "$pid" -o command= 2>/dev/null || true
}

find_running_pid() {
  local pid candidate cwd

  if [ -f "$APP_DIR/run/log-search.pid" ]; then
    pid="$(cat "$APP_DIR/run/log-search.pid")"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
      echo "$pid"
      return
    fi
  fi

  if [ -d /proc ]; then
    for candidate in $(pgrep -x log-search 2>/dev/null || true); do
      cwd="$(readlink "/proc/$candidate/cwd" 2>/dev/null || true)"
      if [ "$cwd" = "$APP_DIR" ]; then
        echo "$candidate"
        return
      fi
    done
  fi

  candidate="$(pgrep -f "$APP_DIR/log-search" 2>/dev/null | head -n 1 || true)"
  if [ -n "$candidate" ] && kill -0 "$candidate" 2>/dev/null; then
    echo "$candidate"
  fi
}

normalize_system() {
  case "${LOG_SEARCH_SYSTEM:-$(uname -s)}" in
    Linux|linux) echo "linux" ;;
    Darwin|darwin) echo "darwin" ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT|windows) echo "windows" ;;
    *) die "unsupported system: ${LOG_SEARCH_SYSTEM:-$(uname -s)}" ;;
  esac
}

normalize_arch() {
  case "${LOG_SEARCH_ARCH:-$(uname -m)}" in
    x86_64|amd64) echo "amd64" ;;
    aarch64|arm64) echo "arm64" ;;
    *) die "unsupported architecture: ${LOG_SEARCH_ARCH:-$(uname -m)}" ;;
  esac
}

download_to() {
  local url="$1"
  local target="$2"

  case "$url" in
    file://*)
      cp "${url#file://}" "$target"
      return 0
      ;;
  esac

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$target"
    return $?
  fi

  if command -v wget >/dev/null 2>&1; then
    wget -qO "$target" "$url"
    return $?
  fi

  die "curl or wget is required"
}

read_url() {
  local url="$1"
  local target="$WORK_DIR/response.txt"
  download_to "$url" "$target"
  cat "$target"
}

json_tag_name() {
  sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1
}

resolve_latest_version() {
  local latest_url="${LOG_SEARCH_LATEST_URL:-}"
  local tag=""

  if [ -n "$latest_url" ]; then
    tag="$(read_url "$latest_url" | json_tag_name || true)"
  fi

  if [ -z "$tag" ] && { [ "$MIRROR" = "auto" ] || [ "$MIRROR" = "gitee" ]; }; then
    tag="$(read_url "https://gitee.com/api/v5/repos/$GITEE_OWNER/$GITEE_REPO/releases/latest" | json_tag_name || true)"
  fi

  if [ -z "$tag" ] && { [ "$MIRROR" = "auto" ] || [ "$MIRROR" = "github" ]; }; then
    tag="$(read_url "https://api.github.com/repos/$GITHUB_OWNER/$GITHUB_REPO/releases/latest" | json_tag_name || true)"
  fi

  [ -n "$tag" ] || die "could not resolve latest release version"
  echo "$tag"
}

asset_name_for() {
  local version_without_v="$1"
  local system="$2"
  local arch="$3"
  local extension="tar.gz"

  if [ "$system" = "windows" ]; then
    extension="zip"
  fi

  echo "log-search_${version_without_v}_${system}_${arch}.${extension}"
}

download_release() {
  local tag="$1"
  local asset="$2"
  local target="$3"
  local version_without_v="${tag#v}"

  if [ -n "${LOG_SEARCH_UPGRADE_BASE_URL:-}" ]; then
    download_to "${LOG_SEARCH_UPGRADE_BASE_URL%/}/$version_without_v/$asset" "$target"
    return
  fi

  if { [ "$MIRROR" = "auto" ] || [ "$MIRROR" = "gitee" ]; }; then
    if download_to "https://gitee.com/$GITEE_OWNER/$GITEE_REPO/releases/download/$tag/$asset" "$target"; then
      return
    fi
  fi

  if { [ "$MIRROR" = "auto" ] || [ "$MIRROR" = "github" ]; }; then
    if download_to "https://github.com/$GITHUB_OWNER/$GITHUB_REPO/releases/download/$tag/$asset" "$target"; then
      return
    fi
  fi

  die "could not download release asset: $asset"
}

extract_release() {
  local archive="$1"
  local target_dir="$2"

  mkdir -p "$target_dir"
  case "$archive" in
    *.tar.gz)
      need_command tar
      tar -C "$target_dir" -xzf "$archive"
      ;;
    *.zip)
      need_command unzip
      unzip -q "$archive" -d "$target_dir"
      ;;
    *)
      die "unsupported archive: $archive"
      ;;
  esac
}

find_release_dir() {
  local extract_dir="$1"
  local found
  found="$(find "$extract_dir" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
  [ -n "$found" ] || die "release archive did not contain a directory"
  echo "$found"
}

validate_release_dir() {
  local release_dir="$1"
  [ -x "$release_dir/log-search" ] || die "new release missing executable log-search"
  [ -d "$release_dir/frontend" ] || die "new release missing frontend directory"
  [ -f "$release_dir/config.toml" ] || die "new release missing config.toml"
}

is_systemd_install() {
  command -v systemctl >/dev/null 2>&1 || return 1
  if [ "${LOG_SEARCH_ASSUME_ROOT:-0}" != "1" ] && [ "$(id -u)" -ne 0 ]; then
    return 1
  fi
  [ -f "$SYSTEMD_DIR/log-search.service" ]
}

is_systemd_active() {
  is_systemd_install || return 1
  systemctl is-active --quiet log-search >/dev/null 2>&1
}

systemd_config_file() {
  local service_file="$SYSTEMD_DIR/log-search.service"
  local exec_start config

  if [ -f "$service_file" ]; then
    exec_start="$(sed -n 's/^ExecStart=//p' "$service_file" | head -n 1)"
    config="$(printf '%s\n' "$exec_start" | sed -n 's/.*--config[[:space:]]\{1,\}\([^[:space:]]\{1,\}\).*/\1/p')"
    if [ -n "$config" ]; then
      echo "$config"
      return
    fi
  fi

  echo "$APP_DIR/config.toml"
}

systemd_static_dir() {
  local service_file="$SYSTEMD_DIR/log-search.service"
  local exec_start static_dir

  if [ -f "$service_file" ]; then
    exec_start="$(sed -n 's/^ExecStart=//p' "$service_file" | head -n 1)"
    static_dir="$(cmd_arg_value "$exec_start" "--static-dir")"
    if [ -n "$static_dir" ]; then
      echo "$static_dir"
      return
    fi
  fi

  echo "$APP_DIR/frontend"
}

capture_runtime_state() {
  local pid cmdline config static_dir

  if [ -n "${LOG_SEARCH_PREVIOUS_CMDLINE:-}" ]; then
    PREVIOUS_MODE="${LOG_SEARCH_PREVIOUS_MODE:-script}"
    cmdline="$LOG_SEARCH_PREVIOUS_CMDLINE"
  elif is_systemd_active; then
    PREVIOUS_MODE="systemd"
    PREVIOUS_CONFIG_FILE="$(systemd_config_file)"
    PREVIOUS_STATIC_DIR="$(systemd_static_dir)"
    echo "Detected previous run mode: systemd"
    echo "Detected config: $PREVIOUS_CONFIG_FILE"
    return
  else
    PREVIOUS_MODE="script"
    pid="$(find_running_pid || true)"
    if [ -n "$pid" ]; then
      cmdline="$(read_pid_cmdline "$pid")"
    else
      cmdline=""
    fi
  fi

  config="$(cmd_arg_value "$cmdline" "--config")"
  static_dir="$(cmd_arg_value "$cmdline" "--static-dir")"
  PREVIOUS_CONFIG_FILE="${config:-$APP_DIR/config.toml}"
  PREVIOUS_STATIC_DIR="${static_dir:-frontend}"

  echo "Detected previous run mode: $PREVIOUS_MODE"
  echo "Detected config: $PREVIOUS_CONFIG_FILE"
}

write_systemd_service() {
  local config_file static_dir
  config_file="${PREVIOUS_CONFIG_FILE:-$APP_DIR/config.toml}"
  static_dir="${PREVIOUS_STATIC_DIR:-$APP_DIR/frontend}"
  if [ "$static_dir" = "frontend" ]; then
    static_dir="$APP_DIR/frontend"
  fi
  mkdir -p "$SYSTEMD_DIR"
  cat > "$SYSTEMD_DIR/log-search.service" <<EOF
[Unit]
Description=Log Search
After=network.target

[Service]
Type=simple
WorkingDirectory=$APP_DIR
ExecStart=$APP_DIR/log-search --config $config_file --static-dir $static_dir
Restart=on-failure
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF
}

stop_current() {
  if [ -x "$APP_DIR/stop.sh" ]; then
    "$APP_DIR/stop.sh" || true
  fi

  if is_systemd_install; then
    systemctl stop log-search || true
  fi
}

start_current() {
  if [ "$PREVIOUS_MODE" = "systemd" ]; then
    write_systemd_service
    systemctl daemon-reload
    systemctl start log-search
    systemctl status log-search --no-pager
    return
  fi

  if [ -x "$APP_DIR/start.sh" ]; then
    (cd "$APP_DIR" && CONFIG_FILE="${PREVIOUS_CONFIG_FILE:-config.toml}" ./start.sh)
  else
    echo "start.sh not found; upgrade copied files but service was not started." >&2
  fi
}

backup_current() {
  local backup_dir="$APP_DIR/backups/$(date +%Y%m%d-%H%M%S)"
  mkdir -p "$backup_dir"

  for path in config.toml log-search frontend start.sh stop.sh status.sh upgrade.sh README.txt log-search.service; do
    if [ -e "$APP_DIR/$path" ]; then
      cp -a "$APP_DIR/$path" "$backup_dir/"
    fi
  done

  echo "$backup_dir"
}

copy_release_without_config() {
  local release_dir="$1"
  local entry name

  for entry in "$release_dir"/* "$release_dir"/.[!.]* "$release_dir"/..?*; do
    [ -e "$entry" ] || continue
    name="$(basename "$entry")"
    case "$name" in
      config.toml|data|logs|run|backups) continue ;;
    esac
    cp -a "$entry" "$APP_DIR/"
  done
}

sync_release() {
  local release_dir="$1"
  local current_upgrade="$WORK_DIR/current-upgrade.sh"

  if [ -f "$APP_DIR/upgrade.sh" ]; then
    cp "$APP_DIR/upgrade.sh" "$current_upgrade"
  fi

  if command -v rsync >/dev/null 2>&1; then
    rsync -a --delete \
      --exclude 'config.toml' \
      --exclude 'data/' \
      --exclude 'logs/' \
      --exclude 'run/' \
      --exclude 'backups/' \
      "$release_dir/" "$APP_DIR/"
  else
    find "$APP_DIR" -mindepth 1 -maxdepth 1 \
      ! -name config.toml \
      ! -name data \
      ! -name logs \
      ! -name run \
      ! -name backups \
      ! -name upgrade.sh \
      -exec rm -rf {} +
    copy_release_without_config "$release_dir"
  fi

  if [ ! -f "$APP_DIR/upgrade.sh" ] && [ -f "$current_upgrade" ]; then
    cp "$current_upgrade" "$APP_DIR/upgrade.sh"
  fi

  cp "$release_dir/config.toml" "$APP_DIR/config.toml.new"
  chmod +x "$APP_DIR/log-search" 2>/dev/null || true
  chmod +x "$APP_DIR/start.sh" "$APP_DIR/stop.sh" "$APP_DIR/status.sh" "$APP_DIR/upgrade.sh" 2>/dev/null || true
}

main() {
  local system arch tag version_without_v asset archive extract_dir release_dir backup_dir

  APP_DIR="$(resolve_app_dir)"
  system="$(normalize_system)"
  arch="$(normalize_arch)"
  [ "$system" != "windows" ] || die "online in-place upgrade is not supported on Windows yet"

  WORK_DIR="$(mktemp -d)"

  tag="$TARGET_VERSION"
  if [ "$tag" = "latest" ]; then
    tag="$(resolve_latest_version)"
  fi
  version_without_v="${tag#v}"
  asset="$(asset_name_for "$version_without_v" "$system" "$arch")"
  archive="$WORK_DIR/$asset"
  extract_dir="$WORK_DIR/extracted"

  echo "Log Search upgrade"
  echo "App dir: $APP_DIR"
  echo "Version: $tag"
  echo "Asset: $asset"

  capture_runtime_state
  download_release "$tag" "$asset" "$archive"
  extract_release "$archive" "$extract_dir"
  release_dir="$(find_release_dir "$extract_dir")"
  validate_release_dir "$release_dir"

  backup_dir="$(backup_current)"
  echo "Backup: $backup_dir"

  stop_current
  sync_release "$release_dir"
  start_current

  echo "Upgrade finished."
  echo "User config kept: $APP_DIR/config.toml"
  echo "New sample config: $APP_DIR/config.toml.new"
}

main "$@"
