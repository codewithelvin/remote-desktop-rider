#!/bin/sh
set -eu

INSIDE=10.77.0.0/24
OUTSIDE=192.168.56.10
SERVER_NS=srv
SERVER_GW=172.31.0.1
SERVER_IP=172.31.0.2
DROPIN=/etc/systemd/system/rider-server.service.d/nat-lab.conf
OUTSIDE_DEV=$(ip -o -4 addr show | awk -v ip="$OUTSIDE" '$4 ~ "^"ip"/" {print $2}')
DELAY_MS=${2:-0}

server_in_namespace() {
  if ! ip netns list | grep -qw "$SERVER_NS"; then
    ip netns add "$SERVER_NS"
    ip link add vsrv0 type veth peer name vsrv1
    ip link set vsrv1 netns "$SERVER_NS"
    ip addr add "$SERVER_GW/24" dev vsrv0
    ip link set vsrv0 up
    ip netns exec "$SERVER_NS" ip addr add "$SERVER_IP/24" dev vsrv1
    ip netns exec "$SERVER_NS" ip link set vsrv1 up
    ip netns exec "$SERVER_NS" ip link set lo up
    ip netns exec "$SERVER_NS" ip route add default via "$SERVER_GW"
  fi
  if [ ! -f "$DROPIN" ]; then
    mkdir -p "$(dirname "$DROPIN")"
    cat > "$DROPIN" <<EOF
[Service]
NetworkNamespacePath=/run/netns/$SERVER_NS
ExecStart=
ExecStart=/usr/local/bin/rider-server --state-dir /var/lib/rider-server run --listen 0.0.0.0:47900 --relay-ports 47901-47999 --public-ip $OUTSIDE
EOF
    systemctl daemon-reload
    systemctl restart rider-server
  fi
}

server_on_router() {
  if [ -f "$DROPIN" ]; then
    rm -f "$DROPIN"
    systemctl daemon-reload
    systemctl restart rider-server
  fi
  if ip netns list | grep -qw "$SERVER_NS"; then
    ip link del vsrv0 2>/dev/null || true
    ip netns del "$SERVER_NS"
  fi
}

home_router_firewall() {
  iptables -N RDR_LAB 2>/dev/null || iptables -F RDR_LAB
  iptables -C INPUT -j RDR_LAB 2>/dev/null || iptables -I INPUT -j RDR_LAB
  iptables -A RDR_LAB -i "$OUTSIDE_DEV" -p udp -m conntrack --ctstate NEW -j DROP
}

clear_nat() {
  iptables -F RDR_LAB 2>/dev/null || true
  iptables -t nat -F PREROUTING
  iptables -t nat -F POSTROUTING
  iptables -t nat -F INPUT
  conntrack -F >/dev/null 2>&1 || true
}

nat_rules() {
  iptables -t nat -A PREROUTING -d "$OUTSIDE" -p udp --dport 47900:47999 -j DNAT --to-destination "$SERVER_IP"
  iptables -t nat -A POSTROUTING -s "$INSIDE" -j SNAT --to-source "$OUTSIDE" "$@"
}

distance() {
  tc qdisc del dev "$OUTSIDE_DEV" root 2>/dev/null || true
  if [ "$1" -gt 0 ]; then
    tc qdisc add dev "$OUTSIDE_DEV" root netem delay "${1}ms"
  fi
}

command -v conntrack >/dev/null || { apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq conntrack >/dev/null 2>&1; }
clear_nat
distance 0

case "${1:-}" in
  cone) sysctl -qw net.ipv4.ip_forward=1; server_in_namespace; nat_rules; home_router_firewall; distance "$DELAY_MS" ;;
  symmetric) sysctl -qw net.ipv4.ip_forward=1; server_in_namespace; nat_rules --random-fully; home_router_firewall; distance "$DELAY_MS" ;;
  off) sysctl -qw net.ipv4.ip_forward=0; server_on_router ;;
  *) echo "usage: sudo $0 cone|symmetric|off [delay-ms]" >&2; exit 2 ;;
esac
echo "nat: $1, ${DELAY_MS} ms added toward the viewer"
