#!/usr/bin/env bash
set -euo pipefail
export MSYS_NO_PATHCONV=1
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
NAME=rider-test-server
IMAGE=rider-test-server:22.04
ADDR=192.168.56.1
DIST="$ROOT/target/dist-linux"
BIN="${RIDER_SERVER_BIN:-$DIST/rider-server}"
TUNNEL_NAME=rider-test.local
CERT="$ROOT/test/vm/guest/rider-test-cert.pem"

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

https_tunnel() {
  docker exec "$NAME" bash -c "
    set -e
    command -v nginx >/dev/null || { apt-get update -qq && apt-get install -y -qq nginx openssl >/dev/null; }
    mkdir -p /etc/nginx/rider
    [ -f /etc/nginx/rider/cert.pem ] || openssl req -x509 -newkey rsa:2048 -nodes -days 30 \
      -keyout /etc/nginx/rider/key.pem -out /etc/nginx/rider/cert.pem \
      -subj /CN=$TUNNEL_NAME -addext subjectAltName=DNS:$TUNNEL_NAME 2>/dev/null
    rm -f /etc/nginx/sites-enabled/default
    systemctl enable --now nginx >/dev/null 2>&1"
  docker exec -i "$NAME" tee /etc/nginx/sites-enabled/rider-tunnel >/dev/null <<EOF
server {
    listen 443 ssl;
    server_name $TUNNEL_NAME;
    ssl_certificate /etc/nginx/rider/cert.pem;
    ssl_certificate_key /etc/nginx/rider/key.pem;
    location /rider-tunnel {
        proxy_pass http://127.0.0.1:47900;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_read_timeout 1h;
        proxy_send_timeout 1h;
        proxy_buffering off;
    }
    location / {
        return 404;
    }
}
EOF
  docker exec "$NAME" bash -c "nginx -t 2>/dev/null && systemctl reload nginx"
  docker cp "$NAME:/etc/nginx/rider/cert.pem" "$(winpath "$CERT")"
  echo "HTTPS tunnel on $ADDR:443 as $TUNNEL_NAME; certificate in $CERT"
}

case "${1:-}" in
  build) build ;;
  up)
    docker build -q -t "$IMAGE" "$(winpath "$ROOT/test/server")" >/dev/null
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    docker run -d --name "$NAME" --hostname rider-test-server \
      --privileged --cgroupns=host -v /sys/fs/cgroup:/sys/fs/cgroup:rw --tmpfs /run --tmpfs /run/lock \
      -p "$ADDR:47900:47900/udp" -p "$ADDR:47901-47999:47901-47999/udp" -p "$ADDR:443:443/tcp" \
      "$IMAGE" >/dev/null
    for _ in $(seq 30); do
      docker exec "$NAME" systemctl is-system-running 2>/dev/null | grep -qE "running|degraded" && break
      sleep 1
    done
    install_build
    https_tunnel
    ;;
  install) install_build ;;
  tunnel) https_tunnel ;;
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
and relay ports 47901-47999, and the HTTPS tunnel (nginx, self-signed
certificate) on TCP 443 as rider-test.local.

  test/server/server.sh build     Linux rider-server -> target/dist-linux/
  test/server/server.sh up        (re)create the container and install the build
  test/server/server.sh install   install the build again (upgrade in place)
  test/server/server.sh tunnel    set up the HTTPS tunnel again (nginx on 443)
  test/server/server.sh fp        the server's fingerprint
  test/server/server.sh sh CMD    run CMD in the container (e.g. journalctl ...)
  test/server/server.sh down      remove the container (and its state)

RIDER_SERVER_BIN=<file>: install that Linux binary instead.
EOF
    exit 1
    ;;
esac
