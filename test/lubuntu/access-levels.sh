#!/bin/bash
set -u
export MSYS_NO_PATHCONV=1
deb=$1 server=$2 fp=$3
root=$(pwd -W 2>/dev/null || pwd)
. "$root/test/lubuntu/lublib.sh"
guest=$root/test/vm/guest
fails=0 total=0
check() { total=$((total + 1)); if [ "$2" = 1 ]; then echo "  PASS  $1"; else echo "  FAIL  $1"; fails=$((fails + 1)); fi; }
wait_for() {
    for _ in $(seq 120); do grep -qE "$2" "$guest/$1" 2>/dev/null && return 0; sleep 0.5; done
    echo "timed out waiting for $2 in $1"; exit 1
}
cmd() { for _ in $(seq 50); do printf '%s\r\n' "$1" >>"$guest/support.cmd" 2>/dev/null && return; sleep 0.1; done; }
clicks() { cat "$guest/clicks.out" 2>/dev/null | grep -c . ; }
clicks_reach() { for _ in $(seq 25); do [ "$(clicks)" -ge "$1" ] && break; sleep 0.2; done; [ "$(clicks)" = "$1" ] && echo 1 || echo 0; }

trap 'lub stop >/dev/null; win stop-tests.ps1' EXIT

(lub_put "$root/test/lubuntu/viewer.sh" /tmp/viewer.sh && lub_put "$deb" /tmp/rdr.deb) || { echo "upload failed"; exit 1; }
echo "installed: $(lub install /tmp/rdr.deb)"
lub server "$server" "$fp" >/dev/null

rm -f "$guest/screen.out" "$guest/support.out"
echo "$server $fp" >"$guest/support.args"
win start-tests.ps1
wait_for screen.out '^[0-9]+ [0-9]+'
read -r vw vh <"$guest/screen.out"; vh=${vh%$'\r'}
wait_for support.out '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"'
code=$(grep -oE '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' "$guest/support.out" | head -1 | cut -d'"' -f4)
echo "VM screen ${vw}x${vh}, code $code"

lub quick "$code" &
wait_for support.out '"support_request"'
cmd "y view"
wait_for support.out '"support_started"'
wait
sleep 5
hx=$((vw / 2 - 360)) hy=150
title=$(lub title); echo "  viewer title: $title"
lub click $hx $hy $vw $vh >/dev/null; sleep 2
check "Linux viewer, view only: the click is dropped" $([ "$(clicks)" = 0 ] && echo 1 || echo 0)
check "Linux viewer, view only: the title says so" $([[ $title == *"[View only]"* ]] && echo 1 || echo 0)

cmd "access full"; wait_for support.out '"access":"full"'; sleep 1
lub click $hx $hy $vw $vh >/dev/null
check "Linux viewer, full access: the click arrives" $(clicks_reach 1)
title=$(lub title)
check "Linux viewer, full access: no label in the title" $([[ $title != *"[View only]"* ]] && echo 1 || echo 0)
lub click $((vw / 2 + 150)) 17 $vw $vh >/dev/null
check "Linux viewer: a click on the banner spot reaches the window under it" $(clicks_reach 2)

cmd "access view"; wait_for support.out '"access":"view"'; sleep 1
lub click $hx $hy $vw $vh >/dev/null; sleep 2
check "Linux viewer, back to view only: the click is dropped" $([ "$(clicks)" = 2 ] && echo 1 || echo 0)
check "Linux viewer: the session is still up" $([ "$(lub running)" = yes ] && ! grep -q support_ended "$guest/support.out" && echo 1 || echo 0)

if [ $fails -gt 0 ]; then echo "FAIL: $fails of $total checks"; exit 1; fi
echo "PASS: all $total checks"
