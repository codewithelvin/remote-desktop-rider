lub_cfg=$(mktemp)
(cd "$root/test/lubuntu" && vagrant ssh-config 2>/dev/null) >"$lub_cfg" ||
    { echo "the Lubuntu VM is not up (cd test/lubuntu && vagrant up)"; exit 1; }
lub_ssh() { timeout "${LUB_TIMEOUT:-60}" ssh -F "$lub_cfg" -o LogLevel=ERROR default "$@"; }
lub_put() { timeout 300 scp -F "$lub_cfg" -o LogLevel=ERROR -q "$1" "default:$2"; }
lub() { lub_ssh "bash /tmp/viewer.sh $*" 2>/dev/null | tr -d '\r'; }
win() { powershell -NoProfile -Command ". '$root/test/vm/vmlib.ps1'; Guest '$1'" >/dev/null 2>&1; }
