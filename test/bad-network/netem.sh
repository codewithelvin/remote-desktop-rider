#!/bin/sh
set -eu

IFACE=${IFACE:-$(ip route show default | awk '{print $5; exit}')}

case "${1:-}" in
  hotel)   NETEM="delay 80ms 30ms distribution normal loss 2% rate 3mbit" ;;
  mobile)  NETEM="delay 120ms 40ms distribution normal loss 1% rate 2mbit" ;;
  awful)   NETEM="delay 250ms 80ms distribution normal loss 5% rate 800kbit" ;;
  far)     NETEM="delay 150ms 10ms rate 10mbit" ;;
  off)     tc qdisc del dev "$IFACE" root 2>/dev/null || true; echo "$IFACE: normal"; exit 0 ;;
  status)  tc qdisc show dev "$IFACE"; exit 0 ;;
  *)       echo "usage: sudo $0 hotel|mobile|awful|far|off|status" >&2; exit 2 ;;
esac

tc qdisc replace dev "$IFACE" root netem $NETEM
echo "$IFACE: $1 ($NETEM)"
