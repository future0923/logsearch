#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="log-search"
VERSION="${VERSION:-0.1.0}"
TARGET="${TARGET:-x86_64-unknown-linux-gnu}"
DIST_DIR="$ROOT_DIR/dist"
RELEASE_DIR="$DIST_DIR/$APP_NAME-$VERSION-linux-x64"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required" >&2
  exit 1
fi

if ! command -v npm >/dev/null 2>&1; then
  echo "npm is required" >&2
  exit 1
fi

rm -rf "$RELEASE_DIR"
mkdir -p "$RELEASE_DIR/frontend" "$RELEASE_DIR/data"

echo "Building frontend..."
(cd "$ROOT_DIR/frontend" && npm ci && npm run build)

echo "Building backend for $TARGET..."
(cd "$ROOT_DIR/backend" && cargo build --release --target "$TARGET")

cp "$ROOT_DIR/backend/target/$TARGET/release/backend" "$RELEASE_DIR/$APP_NAME"
cp -R "$ROOT_DIR/frontend/dist/." "$RELEASE_DIR/frontend/"
cp "$ROOT_DIR/config.example.toml" "$RELEASE_DIR/config.toml"
cp "$ROOT_DIR/packaging/start.sh" "$RELEASE_DIR/start.sh"
cp "$ROOT_DIR/packaging/stop.sh" "$RELEASE_DIR/stop.sh"
cp "$ROOT_DIR/packaging/status.sh" "$RELEASE_DIR/status.sh"
cp "$ROOT_DIR/packaging/upgrade.sh" "$RELEASE_DIR/upgrade.sh"
cp "$ROOT_DIR/packaging/README.txt" "$RELEASE_DIR/README.txt"
cp "$ROOT_DIR/packaging/log-search.service" "$RELEASE_DIR/log-search.service"
chmod +x "$RELEASE_DIR/$APP_NAME" "$RELEASE_DIR/start.sh" "$RELEASE_DIR/stop.sh" "$RELEASE_DIR/status.sh" "$RELEASE_DIR/upgrade.sh"

tar -C "$DIST_DIR" -czf "$RELEASE_DIR.tar.gz" "$(basename "$RELEASE_DIR")"

echo
echo "Release created:"
echo "  $RELEASE_DIR"
echo "  $RELEASE_DIR.tar.gz"
