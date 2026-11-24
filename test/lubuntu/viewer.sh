#!/bin/bash
# Inside the Lubuntu VM: drive the Linux viewer on the desktop (:0) for tests.
#   viewer.sh install <deb>               install a package
#   viewer.sh server <addr> <hex>         set the rendezvous server
#   viewer.sh quick <code>                start a one-time session (background)
#   viewer.sh title                       the session window's title
#   viewer.sh wait-title <text>           the title once it contains the text
#   viewer.sh click <hx> <hy> <hw> <hh>   click where host pixel (hx, hy) of a hw x hh screen shows
#   viewer.sh running                     "yes" while the viewer runs
#   viewer.sh chat                        open the chat (Ctrl+Alt+Shift+C)
#   viewer.sh say <text>                  type a chat message and press Enter
#   viewer.sh files                       files received in this session (name size)
#   viewer.sh shot <file>                 a picture of the screen (PNG)
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
wait-title)   # the title once it contains the text (up to 15 s), else the last one
    shift
    want="$*" t=""
    # The window by its process, read again each time; a failed read while
    # the title changes must not end the loop (set -e).
    for _ in $(seq 50); do
        w=$(xdotool search --onlyvisible --pid "$(pgrep -x rider-client | head -1)" 2>/dev/null | tail -1 || true)
        [ -n "$w" ] && t=$(xdotool getwindowname "$w" 2>/dev/null || true)
        [[ $t == *"$want"* ]] && break
        sleep 0.3
    done
    [ -n "$t" ] || t="(no window: $(xdotool search --onlyvisible --class rider 2>/dev/null | tr '\n' ' '))"
    echo "$t" ;;
click)
    w=$(xdotool search --onlyvisible --name ' fps ' | head -1)
    eval "$(xdotool getwindowgeometry --shell "$w")"   # X Y WIDTH HEIGHT of the client area
    read -r cx cy < <(awk -v hx="$2" -v hy="$3" -v hw="$4" -v hh="$5" -v cw="$WIDTH" -v ch="$HEIGHT" 'BEGIN {
        s = cw / hw; if (ch / hh < s) s = ch / hh
        printf "%d %d\n", (cw - hw * s) / 2 + hx * s, (ch - hh * s) / 2 + hy * s }')
    xdotool mousemove $((X + cx)) $((Y + cy)) sleep 0.2 click 1
    echo "clicked $((X + cx)),$((Y + cy))" ;;
running) pgrep -x rider-client >/dev/null && echo yes || echo no ;;
chat)
    w=$(xdotool search --onlyvisible --name ' fps ' | head -1)
    xdotool windowactivate --sync "$w" key --clearmodifiers ctrl+alt+shift+c ;;
say)
    w=$(xdotool search --onlyvisible --name ' fps ' | head -1)
    shift
    xdotool windowactivate --sync "$w" type --delay 40 "$*"
    xdotool key Return ;;
files) find "${TMPDIR:-/tmp}/RemoteDesktopRider/sessions" -type f ! -name .lock -printf '%f %s\n' 2>/dev/null || true ;;
shot) import -window root "$2" ;;   # ImageMagick
stop) pkill -x rider-client || true ;;
esac
