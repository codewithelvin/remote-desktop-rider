#!/bin/bash
set -u
export MSYS_NO_PATHCONV=1
container=${MAC_CONTAINER:-rider-macos}
vncdo=${VNCDO:-$(command -v vncdo || echo "$APPDATA/Python/Python313/Scripts/vncdo.exe")}
mac() { ssh -o BatchMode=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR \
    -o ConnectTimeout=5 -p 50922 rider@127.0.0.1 "$@"; }
up_ssh() { mac true 2>/dev/null; }

case "${1:-}" in
up)
    up_ssh && { echo "already up"; exit 0; }
    docker start "$container" >/dev/null || exit 1
    start=$(date +%s)
    for _ in $(seq 120); do
        if up_ssh; then
            for _ in $(seq 60); do mac 'who | grep -q console' && break; sleep 2; done
            mac 'who | grep -q console' || { echo "up, but nobody is logged in on screen (auto-login: MAC_PASS=... $0 setup)"; exit 1; }
            echo "up in $(( $(date +%s) - start )) s"; exit 0
        fi
        timeout 10 "$vncdo" -s 127.0.0.1::5999 key enter >/dev/null 2>&1
        sleep 5
    done
    echo "macOS did not come up in 10 minutes (look at VNC 127.0.0.1:5999)"; exit 1 ;;
build)
    tar --exclude=./target --exclude=./.git --exclude='*.png' --exclude='*.out' -czf - . |
        mac 'mkdir -p ~/rdr && tar xzf - -C ~/rdr 2>/dev/null' || exit 1
    mac 'cd ~/rdr && $HOME/.cargo/bin/cargo build --profile quick -p rider-client 2>&1 | grep -E "^error|Finished" -A5' ;;
down)
    up_ssh && mac 'sudo -n /sbin/shutdown -h now' 2>/dev/null
    for _ in $(seq 60); do docker exec "$container" pgrep -f qemu-system >/dev/null 2>&1 || break; sleep 2; done
    docker stop "$container" >/dev/null && echo "stopped" ;;
ssh) shift; mac "$@" ;;
setup)
    pass=${MAC_PASS:?set MAC_PASS to the Mac account password}
    mac 'cat > /tmp/kcpassword.py' <"$(dirname "$0")/kcpassword.py"
    mac "echo '$pass' | sudo -S -p '' true &&
        sudo -n defaults write /Library/Preferences/com.apple.loginwindow autoLoginUser rider &&
        echo '$pass' | sudo -n /usr/bin/python3 /tmp/kcpassword.py && rm /tmp/kcpassword.py
        echo 'rider ALL=(root) NOPASSWD: /sbin/shutdown' | sudo -n tee /etc/sudoers.d/rider-shutdown >/dev/null
        sudo -n chmod 440 /etc/sudoers.d/rider-shutdown && sudo -n visudo -cf /etc/sudoers.d/rider-shutdown"
    mac "defaults write NSGlobalDomain NSAutomaticWindowAnimationsEnabled -bool false
        defaults write NSGlobalDomain NSWindowResizeTime -float 0.001
        defaults write NSGlobalDomain NSScrollAnimationEnabled -bool false
        defaults write com.apple.dock mineffect -string scale
        defaults write com.apple.dock launchanim -bool false
        defaults write com.apple.dock expose-animation-duration -float 0
        defaults write com.apple.finder DisableAllAnimations -bool true
        defaults write com.apple.universalaccess reduceMotion -bool true
        defaults write com.apple.universalaccess reduceTransparency -bool true
        killall Dock Finder 2>/dev/null
        echo '$pass' | sudo -S -p '' mdutil -a -i off >/dev/null
        sudo -n defaults write /Library/Preferences/com.apple.SoftwareUpdate AutomaticCheckEnabled -bool false
        sudo -n defaults write /Library/Preferences/com.apple.SoftwareUpdate AutomaticDownload -bool false
        sudo -n pmset -a sleep 0 displaysleep 0
        echo 'set up: auto-login, shutdown without password, animations off'" ;;
*)
    cat <<'EOF'
The macOS test VM (Docker-OSX container rider-macos: SSH on
127.0.0.1:50922, VNC on 127.0.0.1:5999). Run from the repo root in Git Bash.

  test/macos/mac.sh up       start it; through the boot picker; wait for SSH
  test/macos/mac.sh build    copy this source over and build the viewer there
  test/macos/mac.sh down     shut macOS down cleanly and stop the container
  test/macos/mac.sh ssh ...  run a command on the Mac
  MAC_PASS=... test/macos/mac.sh setup   once: log in automatically at
                             start, allow shutdown without a password
EOF
    exit 1
    ;;
esac
