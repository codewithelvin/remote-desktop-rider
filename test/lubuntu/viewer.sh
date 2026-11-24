#!/bin/bash
# Inside the Lubuntu VM: drive the Linux viewer on the desktop (:0) for tests.
#   viewer.sh install <deb>               install a package
#   viewer.sh server <addr> <hex>         set the rendezvous server
#   viewer.sh quick <code>                start a one-time session (background)
#   viewer.sh title                       the session window's title
#   viewer.sh click <hx> <hy> <hw> <hh>   click where host pixel (hx, hy) of a hw x hh screen shows
#   viewer.sh running                     "yes" while the viewer runs
#   viewer.sh stop
set -e
export DISPLAY=:0 RIDER_LANG=en
log=/tmp/rider-viewer.log
win() { xdotool search --sync --onlyvisible --classname rider-client 2>/dev/null | head -1 || true; }
case "$1" in
install) sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q --allow-downgrades "$2" >/dev/null && dpkg -l remote-desktop-rider | tail -1 ;;
server) rider-client set-server "$2" "$3" ;;
quick)
    pkill -x rider-client || true
    nohup rider-client quick "$2" >"$log" 2>&1 &
    for _ in $(seq 60); do w=$(xdotool search --onlyvisible --name ' fps ' 2>/dev/null | head -1 || true); [ -n "$w" ] && break; sleep 0.5; done
    [ -n "$w" ] || { echo "no window"; cat "$log"; exit 1; }
    xdotool windowsize "$w" 1000 700 windowmove "$w" 60 60
    echo "window $w" ;;
title) xdotool getwindowname "$(xdotool search --onlyvisible --name ' fps ' | head -1)" ;;
click)
    w=$(xdotool search --onlyvisible --name ' fps ' | head -1)
    eval "$(xdotool getwindowgeometry --shell "$w")"   # X Y WIDTH HEIGHT of the client area
    read -r cx cy < <(awk -v hx="$2" -v hy="$3" -v hw="$4" -v hh="$5" -v cw="$WIDTH" -v ch="$HEIGHT" 'BEGIN {
        s = cw / hw; if (ch / hh < s) s = ch / hh
        printf "%d %d\n", (cw - hw * s) / 2 + hx * s, (ch - hh * s) / 2 + hy * s }')
    xdotool mousemove $((X + cx)) $((Y + cy)) sleep 0.2 click 1
    echo "clicked $((X + cx)),$((Y + cy))" ;;
running) pgrep -x rider-client >/dev/null && echo yes || echo no ;;
stop) pkill -x rider-client || true ;;
esac
