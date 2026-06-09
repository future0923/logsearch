#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="log-search"
VERSION="${VERSION:-0.1.0}"
RUST_VERSION="${RUST_VERSION:-1.88.0}"
DIST_DIR="$ROOT_DIR/dist"
RELEASE_NAME="$APP_NAME-$VERSION-centos7-x64"
RELEASE_DIR="$DIST_DIR/$RELEASE_NAME"
BUILDER_IMAGE="${BUILDER_IMAGE:-$APP_NAME-centos7-builder:rust-$RUST_VERSION}"
CARGO_CACHE_VOLUME="${CARGO_CACHE_VOLUME:-${APP_NAME}-centos7-cargo-cache}"
TARGET_CACHE_VOLUME="${TARGET_CACHE_VOLUME:-${APP_NAME}-centos7-target-cache}"
REBUILD_BUILDER="${REBUILD_BUILDER:-0}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required" >&2
  exit 1
fi

if ! command -v npm >/dev/null 2>&1; then
  echo "npm is required to build the frontend on macOS" >&2
  exit 1
fi

echo "Building frontend on host..."
(cd "$ROOT_DIR/frontend" && npm ci && npm run build)

if [ "$REBUILD_BUILDER" = "1" ] || ! docker image inspect "$BUILDER_IMAGE" >/dev/null 2>&1; then
  echo "Building Docker builder image: $BUILDER_IMAGE"
  docker build \
    --build-arg RUST_VERSION="$RUST_VERSION" \
    -t "$BUILDER_IMAGE" \
    -f - "$ROOT_DIR" <<'DOCKERFILE'
FROM centos:7

ARG RUST_VERSION=1.88.0
ENV PATH=/root/.cargo/bin:$PATH

RUN set -euo pipefail; \
    sed -i s/mirror.centos.org/vault.centos.org/g /etc/yum.repos.d/*.repo; \
    sed -i s/^#.*baseurl=http/baseurl=http/g /etc/yum.repos.d/*.repo; \
    sed -i s/^mirrorlist=http/#mirrorlist=http/g /etc/yum.repos.d/*.repo; \
    yum install -y curl gcc gcc-c++ make perl openssl-devel pkgconfig; \
    yum clean all; \
    curl -sSf https://rsproxy.cn/rustup-init.sh -o /tmp/rustup-init.sh; \
    sh /tmp/rustup-init.sh -y --default-toolchain "$RUST_VERSION"; \
    rm -f /tmp/rustup-init.sh; \
    mkdir -p /root/.cargo; \
    cat > /root/.cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy]
registry = "https://rsproxy.cn/crates.io-index"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"

[net]
git-fetch-with-cli = true
EOF
DOCKERFILE
else
  echo "Using cached Docker builder image: $BUILDER_IMAGE"
fi

echo "Building backend in CentOS 7 container..."
echo "Cargo cache volume: $CARGO_CACHE_VOLUME"
echo "Target cache volume: $TARGET_CACHE_VOLUME"
docker volume create "$CARGO_CACHE_VOLUME" >/dev/null
docker volume create "$TARGET_CACHE_VOLUME" >/dev/null

docker run --rm \
  -v "$ROOT_DIR":/work \
  -v "$CARGO_CACHE_VOLUME":/root/.cargo/registry \
  -v "$TARGET_CACHE_VOLUME":/cargo-target \
  -e CARGO_TARGET_DIR=/cargo-target \
  -w /work/backend \
  "$BUILDER_IMAGE" \
  bash -lc '
set -euo pipefail
cargo build --release
mkdir -p /work/backend/target/centos7-release
cp /cargo-target/release/backend /work/backend/target/centos7-release/backend
'

rm -rf "$RELEASE_DIR"
mkdir -p "$RELEASE_DIR/frontend" "$RELEASE_DIR/data"

cp "$ROOT_DIR/backend/target/centos7-release/backend" "$RELEASE_DIR/$APP_NAME"
cp -R "$ROOT_DIR/frontend/dist/." "$RELEASE_DIR/frontend/"
cp "$ROOT_DIR/config.example.toml" "$RELEASE_DIR/config.toml"
cp "$ROOT_DIR/packaging/start.sh" "$RELEASE_DIR/start.sh"
cp "$ROOT_DIR/packaging/stop.sh" "$RELEASE_DIR/stop.sh"
cp "$ROOT_DIR/packaging/status.sh" "$RELEASE_DIR/status.sh"
cp "$ROOT_DIR/packaging/upgrade.sh" "$RELEASE_DIR/upgrade.sh"
cp "$ROOT_DIR/packaging/README.txt" "$RELEASE_DIR/README.txt"
cp "$ROOT_DIR/packaging/log-search.service" "$RELEASE_DIR/log-search.service"
chmod +x "$RELEASE_DIR/$APP_NAME" "$RELEASE_DIR/start.sh" "$RELEASE_DIR/stop.sh" "$RELEASE_DIR/status.sh" "$RELEASE_DIR/upgrade.sh"

tar -C "$DIST_DIR" -czf "$RELEASE_DIR.tar.gz" "$RELEASE_NAME"

echo
echo "CentOS 7 release created:"
echo "  $RELEASE_DIR"
echo "  $RELEASE_DIR.tar.gz"
echo
echo "Docker caches:"
echo "  Builder image: $BUILDER_IMAGE"
echo "  Cargo cache:   $CARGO_CACHE_VOLUME"
echo "  Target cache:  $TARGET_CACHE_VOLUME"
echo "Set REBUILD_BUILDER=1 to rebuild the Docker builder image."
