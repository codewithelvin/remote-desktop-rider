#!/usr/bin/env bash
# Build the Debian/Ubuntu package (x86_64) of the app and the session viewer.
#
#   packaging/linux/build-deb.sh 0.3.0   ->  target/dist/remote-desktop-rider_0.3.0_amd64.deb
#
# Build on the oldest supported Ubuntu (22.04) so the package installs on
# Debian 12+, Ubuntu 22.04+ and their derivatives.
set -euo pipefail

VERSION="${1:?usage: build-deb.sh <version>}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

PKG=remote-desktop-rider
cargo build --release -p rider-app -p rider-client

OUT="target/dist"
DIR="$OUT/deb"
rm -rf "$DIR"
LIB="$DIR/usr/lib/$PKG"
mkdir -p "$LIB" "$DIR/usr/bin" "$DIR/usr/share/applications" "$DIR/DEBIAN"
install -m 755 target/release/RemoteDesktopRider "$LIB/RemoteDesktopRider"
install -m 755 target/release/rider-client "$LIB/rider-client"
strip "$LIB/RemoteDesktopRider" "$LIB/rider-client"
# The app starts rider-client from its own folder; the links are for terminals.
ln -s "../lib/$PKG/RemoteDesktopRider" "$DIR/usr/bin/$PKG"
ln -s "../lib/$PKG/rider-client" "$DIR/usr/bin/rider-client"

for s in 32 48 64 128 256 512; do
  d="$DIR/usr/share/icons/hicolor/${s}x${s}/apps"
  mkdir -p "$d"
  cargo run -q --release -p rider-core --example icon-png -- "$s" "$d/$PKG.png"
done

cat > "$DIR/usr/share/applications/$PKG.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Remote Desktop Rider
GenericName=Remote desktop
GenericName[az]=Uzaq masaüstü
Comment=Control your computers remotely, end-to-end encrypted
Comment[az]=Kompüterlərinizi uzaqdan idarə edin, uçdan-uca şifrələnmiş
Exec=/usr/lib/$PKG/RemoteDesktopRider
Icon=$PKG
Terminal=false
Categories=Network;RemoteAccess;
StartupWMClass=remote-desktop-rider
EOF

# The window system, OpenGL and the keyboard are loaded at run time.
cat > "$DIR/DEBIAN/control" <<EOF
Package: $PKG
Version: $VERSION
Section: net
Priority: optional
Architecture: amd64
Maintainer: Elvin Huseynov <codewithelvin@users.noreply.github.com>
Depends: libc6 (>= 2.35), libgcc-s1, libstdc++6, libgl1, libegl1, libxkbcommon0, libxkbcommon-x11-0, libx11-6, libxcursor1, libxi6, libxrandr2, libwayland-client0, libwayland-egl1
Homepage: https://codewithelvin.github.io/remote-desktop-rider/
Description: Remote desktop for your own computers
 Control Windows computers from this one: paired devices with a PIN or a
 one-time code, end-to-end encrypted, through your own rendezvous server.
 English and Azerbaijani.
EOF
size=$(du -sk "$DIR/usr" | cut -f1)
echo "Installed-Size: $size" >> "$DIR/DEBIAN/control"

DEB="$OUT/${PKG}_${VERSION}_amd64.deb"
rm -f "$DEB"
dpkg-deb --root-owner-group --build "$DIR" "$DEB" >/dev/null
rm -rf "$DIR"
echo "$DEB"
