#!/usr/bin/env bash
# Install or upgrade rider-server on a systemd Linux host (run as root).
#
#   sudo bash install.sh [PUBLIC_IP]
#
# Expects `rider-server` (Linux x86_64 binary) and `rider-server.service` next
# to this script. PUBLIC_IP is the address peers use for relay ports; give it
# when the machine sits behind NAT, or to be explicit. Safe to run again: the
# server key in /var/lib/rider-server is kept, so its fingerprint never changes.
set -euo pipefail
cd "$(dirname "$0")"
public_ip="${1:-}"

[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo)" >&2; exit 1; }
[ -x rider-server ] || chmod +x rider-server
./rider-server --version >/dev/null

install -m755 rider-server /usr/local/bin/rider-server
unit=/etc/systemd/system/rider-server.service
install -m644 rider-server.service "$unit"
if [ -n "$public_ip" ]; then
  sed -i "s|--relay-ports 47901-47999$|--relay-ports 47901-47999 --public-ip $public_ip|" "$unit"
fi

systemctl daemon-reload
systemctl enable rider-server >/dev/null 2>&1
systemctl restart rider-server

if command -v ufw >/dev/null && ufw status | grep -q "Status: active"; then
  ufw allow 47900/udp comment 'rider-server rendezvous' >/dev/null
  ufw allow 47901:47999/udp comment 'rider-server relay' >/dev/null
  echo "ufw: opened UDP 47900 and 47901-47999"
else
  echo "ufw not active: make sure UDP 47900 and 47901-47999 are open in any other firewall"
fi

sleep 2
systemctl is-active --quiet rider-server || { journalctl -u rider-server -n 20 --no-pager; exit 1; }
echo "rider-server $(/usr/local/bin/rider-server --version | awk '{print $2}') is running."
journalctl -u rider-server -n 20 --no-pager -o cat | grep -E "fingerprint|rendezvous on" | tail -2
