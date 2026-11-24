# Shared by the test/lubuntu scripts (source it after setting $root): talk to
# the Lubuntu VM with plain ssh/scp. `vagrant ssh` starts Ruby on every call
# (3-5 s each); the connection settings are read from Vagrant once instead.
lub_cfg=$(mktemp)
(cd "$root/test/lubuntu" && vagrant ssh-config 2>/dev/null) >"$lub_cfg" ||
    { echo "the Lubuntu VM is not up (cd test/lubuntu && vagrant up)"; exit 1; }
lub_ssh() { timeout "${LUB_TIMEOUT:-60}" ssh -F "$lub_cfg" -o LogLevel=ERROR default "$@"; }
lub_put() { timeout 300 scp -F "$lub_cfg" -o LogLevel=ERROR -q "$1" "default:$2"; }
# lub <viewer.sh command...>: drive the viewer on the VM's desktop.
lub() { lub_ssh "bash /tmp/viewer.sh $*" 2>/dev/null | tr -d '\r'; }
# win <guest script>: run a test/vm/guest script in the Windows VM (its agent).
win() { powershell -NoProfile -Command ". '$root/test/vm/vmlib.ps1'; Guest '$1'" >/dev/null 2>&1; }
