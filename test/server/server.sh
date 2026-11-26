#!/usr/bin/env bash
set -euo pipefail
export MSYS_NO_PATHCONV=1
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
NAME=rider-test-server
IMAGE=rider-test-server:22.04
ADDR=192.168.56.1
DIST="$ROOT/target/dist-linux"
BIN="${RIDER_SERVER_BIN:-$DIST/rider-server}"

winpath() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else echo "$1"; fi; }

build() {
  mkdir -p "$DIST"
  docker run --rm \
    -v "$(winpath "$ROOT"):/src" -v rider-cargo:/root/.cargo -v rider-rustup:/root/.rustup \
    -v rider-target:/src/target -v "$(winpath "$DIST"):/out" -w /src ubuntu:22.04 bash -c '
      set -e
      apt-get update -qq && apt-get install -y -qq build-essential ca-certificates >/dev/null
      /root/.cargo/bin/cargo build -j 4 --release -p rider-server
      install -m755 target/release/rider-server /out/rider-server'
  echo "built $DIST/rider-server"
}

install_build() {
  [ -f "$BIN" ] || { echo "no Linux build ($BIN): run $0 build" >&2; exit 1; }
  docker exec "$NAME" mkdir -p /root/rider
  docker cp "$(winpath "$BIN")" "$NAME:/root/rider/rider-server"
  docker cp "$(winpath "$ROOT/deploy/install.sh")" "$NAME:/root/rider/install.sh"
  docker cp "$(winpath "$ROOT/deploy/rider-server.service")" "$NAME:/root/rider/rider-server.service"
  docker exec "$NAME" bash /root/rider/install.sh "$ADDR"
}

case "${1:-}" in
  build) build ;;
  up)
    docker build -q -t "$IMAGE" "$(winpath "$ROOT/test/server")" >/dev/null
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    docker run -d --name "$NAME" --hostname rider-test-server \
      --privileged --cgroupns=host -v /sys/fs/cgroup:/sys/fs/cgroup:rw --tmpfs /run --tmpfs /run/lock \
      -p "$ADDR:47900:47900/udp" -p "$ADDR:47901-47999:47901-47999/udp" \
      "$IMAGE" >/dev/null
    for _ in $(seq 30); do
      docker exec "$NAME" systemctl is-system-running 2>/dev/null | grep -qE "running|degraded" && break
      sleep 1
    done
    install_build
    ;;
  install) install_build ;;
  fp)
    docker exec "$NAME" journalctl -u rider-server --no-pager -o cat \
      | sed -n 's/^Server fingerprint: //p' | tail -1
    ;;
  sh) shift; docker exec "$NAME" "$@" ;;
  down) docker rm -f "$NAME" >/dev/null 2>&1 || true; echo "removed $NAME" ;;
  *)
    cat <<'EOF'
A local test rider-server, installed like a production one: Ubuntu 22.04
with systemd in Docker, the Linux build of this tree, deploy/install.sh,
the systemd unit from deploy/. Reachable at 192.168.56.1 on UDP 47900
and relay ports 47901-47999.

  test/server/server.sh build     Linux rider-server -> target/dist-linux/
  test/server/server.sh up        (re)create the container and install the build
  test/server/server.sh install   install the build again (upgrade in place)
  test/server/server.sh fp        the server's fingerprint
  test/server/server.sh sh CMD    run CMD in the container (e.g. journalctl ...)
  test/server/server.sh down      remove the container (and its state)

RIDER_SERVER_BIN=<file>: install that Linux binary instead.
EOF
    exit 1
    ;;
esac
