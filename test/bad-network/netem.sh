#!/bin/sh
# Make this Linux machine's network bad on purpose, to test and demo sessions
# on poor links. Run it on the controlling side (e.g. test/ubuntu-desktop) or
# on a Linux router between the two computers.
#
#   sudo ./netem.sh hotel        # pick a profile (below)
#   sudo ./netem.sh status       # show what is applied
#   sudo ./netem.sh off          # back to normal
#
# Interface: the default route's, or set IFACE=eth1.
# Only outgoing traffic is shaped; for both directions also run it on the
# other end, or on a router.
#
# Recording a demo: run the same session (scrolling a web page, dragging a
# window, playing a short video) once with `off` and once with a profile, and
# show the session's stats line next to it. For a side-by-side with another
# tool, apply the profile once and use both tools over the same link.
set -eu

IFACE=${IFACE:-$(ip route show default | awk '{print $5; exit}')}

case "${1:-}" in
  hotel)   NETEM="delay 80ms 30ms distribution normal loss 2% rate 3mbit" ;;     # busy shared Wi-Fi
  mobile)  NETEM="delay 120ms 40ms distribution normal loss 1% rate 2mbit" ;;    # 4G at the cell edge
  awful)   NETEM="delay 250ms 80ms distribution normal loss 5% rate 800kbit" ;;  # the worst link still worth using
  far)     NETEM="delay 150ms 10ms rate 10mbit" ;;                                # another continent, clean
  off)     tc qdisc del dev "$IFACE" root 2>/dev/null || true; echo "$IFACE: normal"; exit 0 ;;
  status)  tc qdisc show dev "$IFACE"; exit 0 ;;
  *)       echo "usage: sudo $0 hotel|mobile|awful|far|off|status" >&2; exit 2 ;;
esac

# shellcheck disable=SC2086 # NETEM is a list of tc arguments
tc qdisc replace dev "$IFACE" root netem $NETEM
echo "$IFACE: $1 ($NETEM)"
