#!/bin/bash
set -u
export MSYS_NO_PATHCONV=1
server=$1 fp=$2 viewer=${3:-/Users/rider/rdr/target/quick/rider-client}
root=$(pwd -W 2>/dev/null || pwd)
vncdo=${VNCDO:-$(command -v vncdo || echo "$APPDATA/Python/Python313/Scripts/vncdo.exe")}
work=$(mktemp -d)
cmds=$work/host.cmd out=$work/host.out
fails=0 total=0
check() { total=$((total + 1)); if [ "$2" = 1 ]; then echo "  PASS  $1"; else echo "  FAIL  $1"; fails=$((fails + 1)); fi; }
mac() { ssh -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -p 50922 rider@127.0.0.1 "$@"; }
vnc() { timeout 60 "$vncdo" -s 127.0.0.1::5999 "$@"; }
send() { printf '%s\n' "$1" >>"$cmds"; }
wait_for() {
    for _ in $(seq $((${2:-30} * 2))); do grep -qE "$1" "$out" 2>/dev/null && return 0; sleep 0.5; done
    return 1
}
mac_files() { mac 'find "$(getconf DARWIN_USER_TEMP_DIR)RemoteDesktopRider/sessions" -type f ! -name .lock 2>/dev/null | while read -r f; do echo "$(basename "$f") $(stat -f %z "$f")"; done'; }

: >"$cmds"
(tail -f "$cmds" | ProgramData="$(cygpath -w "$work")" APPDATA="$(cygpath -w "$work")" RIDER_LANG=en \
    "$root/target/release/rider-host.exe" --json support "$server" "$fp" >"$out" 2>"$work/host.err") &
trap 'send quit; sleep 2; mac "pkill -x rider-client" 2>/dev/null; pkill -f "tail -f $cmds" 2>/dev/null; rm -rf "$work"' EXIT

wait_for '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' 30 || { echo "no code from the host"; cat "$work/host.err"; exit 1; }
code=$(grep -oE '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' "$out" | head -1 | cut -d'"' -f4)
echo "code $code"

mac "pkill -x rider-client; $viewer set-server $server $fp >/dev/null
  printf '#!/bin/bash\nRIDER_LANG=en exec $viewer quick $code >/tmp/rider-viewer.log 2>&1\n' >/tmp/rider-viewer.command
  chmod +x /tmp/rider-viewer.command && open /tmp/rider-viewer.command" >/dev/null
wait_for '"support_request"' 90 || { echo "no request from the Mac"; mac 'cat /tmp/rider-viewer.log'; exit 1; }
send "y view"
wait_for '"support_started"' 30
check "the Mac's viewer connected with the code" $(wait_for '"support_started"' 1 && echo 1 || echo 0)
for _ in $(seq 60); do mac 'grep -q "^Connected to" /tmp/rider-viewer.log' && break; sleep 1; done
sleep 3
front() { mac 'lsappinfo info -only name "$(lsappinfo front)"' 2>/dev/null | grep -q rider-client; }
for _ in $(seq 15); do front && break; vnc move 960 500 click 1 >/dev/null 2>&1; sleep 1; done
front || echo "  (the session window did not come to the front)"

send 'chat "Salam from the host"'
sleep 2
vnc key ctrl-alt-shift-c pause 4 capture "$root/test/macos/chat-open.png" type "hello from mac" key enter >/dev/null 2>&1
check "a message typed in the Mac viewer's chat arrives" $(wait_for '"event":"chat","text":"hello from mac"' 20 && echo 1 || echo 0)

send "access full"; wait_for '"access":"full"' 10
src=$work/from-host.txt
printf 'host file %s' "$RANDOM$RANDOM" >"$src"
send "send-file \"$(cygpath -w "$src" | sed 's/\\/\\\\/g')\""
wait_for '"event":"file_done"' 30
sleep 2
files=$(mac_files)
check "a file from this PC arrives in the Mac viewer's session folder" $(grep -q "^from-host.txt $(stat -c %s "$src")\$" <<<"$files" && echo 1 || echo 0)
send "access view"; wait_for '"access":"view"' 10
sleep 2
vnc capture "$root/test/macos/chat-shot.png" >/dev/null 2>&1

send quit
for _ in $(seq 30); do mac 'pgrep -x rider-client >/dev/null' || break; sleep 1; done
check "the Mac viewer closed when the session ended" $(mac 'pgrep -x rider-client >/dev/null' && echo 0 || echo 1)
check "the Mac viewer's received file is deleted" $([ -z "$(mac_files)" ] && echo 1 || echo 0)

if [ $fails -gt 0 ]; then echo "FAIL: $fails of $total checks"; exit 1; fi
echo "PASS: all $total checks"
