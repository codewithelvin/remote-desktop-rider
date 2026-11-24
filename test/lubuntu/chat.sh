#!/bin/bash
# Session chat with the Linux viewer: the Lubuntu VM connects with a
# one-time code to a host on this PC (driven through its JSON lines, like
# the app). Run from the repo root in Git Bash on the PC, with only the
# Lubuntu VM up:
#
#   test/lubuntu/chat.sh <deb> <server> <fingerprint-hex>
#
# Typing happens in the VM, and only while the session is View only, so even
# a chat panel that failed to open could not act on this PC. Files from the
# viewer need Full access and typing in a file dialog: test/vm/chat.ps1
# covers that direction.
set -u
export MSYS_NO_PATHCONV=1   # Git Bash would turn /tmp/... into Windows paths
deb=$1 server=$2 fp=$3
root=$(pwd -W 2>/dev/null || pwd)
. "$root/test/lubuntu/lublib.sh"   # lub, lub_ssh, lub_put, win
work=$(mktemp -d)
cmds=$work/host.cmd out=$work/host.out
fails=0 total=0
check() { total=$((total + 1)); if [ "$2" = 1 ]; then echo "  PASS  $1"; else echo "  FAIL  $1"; fails=$((fails + 1)); fi; }
send() { printf '%s\n' "$1" >>"$cmds"; }
wait_for() { # pattern [seconds]
    for _ in $(seq $((${2:-30} * 2))); do grep -qE "$1" "$out" 2>/dev/null && return 0; sleep 0.5; done
    return 1
}
yes_no() { "$@" && echo 1 || echo 0; }

# The host on this PC, as the app runs it, with settings in a scratch folder.
: >"$cmds"
(tail -f "$cmds" | ProgramData="$(cygpath -w "$work")" APPDATA="$(cygpath -w "$work")" RIDER_LANG=en \
    "$root/target/release/rider-host.exe" --json support "$server" "$fp" >"$out" 2>"$work/host.err") &
trap 'send quit; sleep 1; lub stop >/dev/null; pkill -f "tail -f $cmds" 2>/dev/null; rm -rf "$work"' EXIT

(lub_put "$root/test/lubuntu/viewer.sh" /tmp/viewer.sh && lub_put "$deb" /tmp/rdr.deb) || { echo "upload failed"; exit 1; }
echo "installed: $(lub install /tmp/rdr.deb)"
lub server "$server" "$fp" >/dev/null
wait_for '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' 30 || { echo "no code from the host"; cat "$work/host.err"; exit 1; }
code=$(grep -oE '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' "$out" | head -1 | cut -d'"' -f4)
echo "code $code"

lub quick "$code" & quick=$!
wait_for '"support_request"' 60 || { echo "no request"; exit 1; }
send "y view"
wait_for '"support_started"' 30
wait "$quick"   # only the viewer start, not the host
sleep 3

# This PC -> Linux, with the chat still closed: the title shows it.
(sleep 2; send 'chat "Salam from the host"') &
title=$(lub wait-title Salam from the host)
case $title in *"Salam from the host"*) ok=1 ;; *) ok=0 ;; esac
check "a message from this PC shows on the Linux viewer ('${title%% — *}')" $ok

# Linux -> this PC, typed in the VM's chat (View only: stray keys are dropped here).
lub chat >/dev/null; sleep 1
lub say "Hello from Linux" >/dev/null
check "a message typed in the Linux viewer's chat arrives" $(yes_no wait_for '"event":"chat","text":"Hello from Linux"' 15)
lub shot /tmp/rider-chat.png >/dev/null
lub_ssh "cat /tmp/rider-chat.png" >"$root/test/lubuntu/chat-shot.png" 2>/dev/null || true

# A file from this PC (Full access for it; nothing is typed meanwhile).
send "access full"; wait_for '"access":"full"' 10
src=$work/from-host.txt
printf 'host file %s' "$RANDOM$RANDOM" >"$src"
send "send-file \"$(cygpath -w "$src" | sed 's/\\/\\\\/g')\""
wait_for '"event":"file_done"' 30
sleep 1
files=$(lub files)
check "a file from this PC arrives in the Linux viewer's session folder" $(yes_no grep -q "^from-host.txt $(stat -c %s "$src")\$" <<<"$files")
send "access view"; wait_for '"access":"view"' 10

# The end: both sides delete what they received.
send quit
for _ in $(seq 30); do [ "$(lub running)" = no ] && break; sleep 1; done
check "the Linux viewer closed when the session ended" $(yes_no [ "$(lub running)" = no ])
check "the Linux viewer's received file is deleted" $(yes_no [ -z "$(lub files)" ])

if [ $fails -gt 0 ]; then echo "FAIL: $fails of $total checks"; exit 1; fi
echo "PASS: all $total checks"
