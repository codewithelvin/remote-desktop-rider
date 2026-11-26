#!/bin/bash
set -u
export MSYS_NO_PATHCONV=1
deb=$1 server=$2 fp=$3
root=$(pwd -W 2>/dev/null || pwd)
. "$root/test/lubuntu/lublib.sh"
guest=$root/test/vm/guest
work=$(mktemp -d)
fails=0 total=0
check() { total=$((total + 1)); if [ "$2" = 1 ]; then echo "  PASS  $1"; else echo "  FAIL  $1"; fails=$((fails + 1)); fi; }
wait_for() {
    for _ in $(seq 120); do grep -qE "$2" "$guest/$1" 2>/dev/null && return 0; sleep 0.5; done
    echo "timed out waiting for $2 in $1"; exit 1
}
cmd() { for _ in $(seq 50); do printf '%s\r\n' "$1" >>"$guest/support.cmd" 2>/dev/null && return; sleep 0.1; done; }
lub_get() { timeout 120 scp -F "$lub_cfg" -o LogLevel=ERROR -q "default:$1" "$2"; }
png() {
    powershell -NoProfile -Command "Add-Type -AssemblyName System.Drawing; $1" | tr -d '\r'
}
make_png() {
    png "\$b = New-Object Drawing.Bitmap 200, 150; for (\$y = 0; \$y -lt 150; \$y++) { for (\$x = 0; \$x -lt 200; \$x++) { \$b.SetPixel(\$x, \$y, [Drawing.Color]::FromArgb(255, (\$x * 9) % 256, (\$y * 11) % 256, (\$x * \$y) % 256)) } }; \$b.Save('$1', [Drawing.Imaging.ImageFormat]::Png)"
}
diff_png() {
    png "\$a = [Drawing.Bitmap]::FromFile('$1'); \$b = [Drawing.Bitmap]::FromFile('$2'); if (\$a.Width -ne \$b.Width -or \$a.Height -ne \$b.Height) { 'size ' + \$a.Width + 'x' + \$a.Height + ' vs ' + \$b.Width + 'x' + \$b.Height; exit }; \$d = 0; for (\$y = 0; \$y -lt \$a.Height; \$y++) { for (\$x = 0; \$x -lt \$a.Width; \$x++) { if ((\$a.GetPixel(\$x, \$y).ToArgb() -band 0xFFFFFF) -ne (\$b.GetPixel(\$x, \$y).ToArgb() -band 0xFFFFFF)) { \$d++ } } }; \$d"
}
guest_clip() {
    rm -f "$guest/clip-image.out"
    win "clip-start.ps1 $1"
    for _ in $(seq 60); do [ -s "$guest/clip-image.out" ] && break; sleep 0.5; done
    tr -d '\r' <"$guest/clip-image.out" 2>/dev/null | head -1
}

trap 'lub stop >/dev/null; win stop-tests.ps1; rm -rf "$work"' EXIT

(lub_put "$root/test/lubuntu/viewer.sh" /tmp/viewer.sh && lub_put "$deb" /tmp/rdr.deb) || { echo "upload failed"; exit 1; }
echo "installed: $(lub install /tmp/rdr.deb)"
if [ "$(lub has-xclip)" != yes ]; then
    LUB_TIMEOUT=300 lub_ssh "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q xclip >/dev/null" || { echo "cannot install xclip"; exit 1; }
fi
lub server "$server" "$fp" >/dev/null

rm -f "$guest/screen.out" "$guest/support.out" "$guest/clip-sent.png" "$guest/clip-got.png"
echo "$server $fp" >"$guest/support.args"
win start-tests.ps1
wait_for support.out '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"'
code=$(grep -oE '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' "$guest/support.out" | head -1 | cut -d'"' -f4)
echo "support code $code"
lub clip-text "before the session" >/dev/null
lub quick "$code" &
wait_for support.out '"support_request"'
cmd "y full"
wait_for support.out '"support_started"'
wait
sleep 3

guest_clip clear >/dev/null
text=""
for _ in $(seq 20); do text=$(lub_ssh "DISPLAY=:0 xclip -selection clipboard -o" 2>/dev/null | tr -d '\r'); [ "$text" = "rider clipboard test" ] && break; sleep 0.5; done
check "text copied in the VM arrives on Linux ('$text')" $([ "$text" = "rider clipboard test" ] && echo 1 || echo 0)

echo "  VM: $(guest_clip set)"
got=$(lub clip-get /tmp/from-vm.png)
lub_get /tmp/from-vm.png "$work/from-vm.png"
d=$( [ -s "$work/from-vm.png" ] && diff_png "$(cygpath -w "$guest/clip-sent.png" 2>/dev/null || echo "$guest/clip-sent.png")" "$(cygpath -w "$work/from-vm.png" 2>/dev/null || echo "$work/from-vm.png")" || echo missing)
check "an image copied in the VM arrives on Linux, pixel for pixel ($got, $d differing)" $([ "$d" = 0 ] && echo 1 || echo 0)

src=$(cygpath -w "$work/from-linux.png" 2>/dev/null || echo "$work/from-linux.png")
make_png "$src"
lub_put "$work/from-linux.png" /tmp/from-linux.png
lub clip-set /tmp/from-linux.png >/dev/null
r=$(guest_clip get)
d=$( [ -s "$guest/clip-got.png" ] && diff_png "$src" "$(cygpath -w "$guest/clip-got.png" 2>/dev/null || echo "$guest/clip-got.png")" || echo missing)
check "an image copied on Linux arrives in the VM, pixel for pixel ($r, $d differing)" $([ "$d" = 0 ] && echo 1 || echo 0)

cmd "access view"; wait_for support.out '"access":"view"'; sleep 1
lub clip-text "view only" >/dev/null
guest_clip set >/dev/null
sleep 6
targets=$(lub_ssh "DISPLAY=:0 xclip -selection clipboard -t TARGETS -o" 2>/dev/null | tr -d '\r' | tr '\n' ' ')
check "view only: an image copied in the VM does not arrive" $([[ $targets != *image/png* ]] && echo 1 || echo 0)
check "the session is still up" $([ "$(lub running)" = yes ] && ! grep -q support_ended "$guest/support.out" && echo 1 || echo 0)

if [ $fails -gt 0 ]; then echo "FAIL: $fails of $total checks"; exit 1; fi
echo "PASS: all $total checks"
