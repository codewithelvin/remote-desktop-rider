#!/usr/bin/env bash
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

mkdir -p "$DIR/usr/share/metainfo"
RAW="https://raw.githubusercontent.com/codewithelvin/remote-desktop-rider/main"
cat > "$DIR/usr/share/metainfo/$PKG.metainfo.xml" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<component type="desktop-application">
  <id>io.github.codewithelvin.RemoteDesktopRider</id>
  <name>Remote Desktop Rider</name>
  <summary>Control your own computers remotely, end-to-end encrypted</summary>
  <summary xml:lang="az">Kompüterlərinizi uzaqdan idarə edin, uçdan-uca şifrələnmiş</summary>
  <metadata_license>CC0-1.0</metadata_license>
  <project_license>MIT</project_license>
  <developer id="io.github.codewithelvin">
    <name>Elvin Huseynov</name>
  </developer>
  <description>
    <p>
      Remote Desktop Rider connects this computer to your Windows computers, at
      home or in the office, and shows their screen here with keyboard, mouse,
      clipboard and file transfer.
    </p>
    <ul>
      <li>Paired devices only: a PIN or a one-time code, checked on both sides</li>
      <li>End-to-end encrypted; your own rendezvous server, nothing in between can read it</li>
      <li>Works behind strict firewalls, falling back to HTTPS (port 443)</li>
      <li>Adapts quality to the connection, or choose 1080p, 720p or 480p</li>
      <li>English and Azerbaijani</li>
    </ul>
  </description>
  <launchable type="desktop-id">$PKG.desktop</launchable>
  <pkgname>$PKG</pkgname>
  <icon type="stock">$PKG</icon>
  <icon type="remote" width="256" height="256">$RAW/site/icon-256.png</icon>
  <categories>
    <category>Network</category>
    <category>RemoteAccess</category>
  </categories>
  <keywords>
    <keyword>remote</keyword>
    <keyword>desktop</keyword>
    <keyword>RDP</keyword>
    <keyword>support</keyword>
  </keywords>
  <url type="homepage">https://codewithelvin.github.io/remote-desktop-rider/</url>
  <url type="bugtracker">https://github.com/codewithelvin/remote-desktop-rider/issues</url>
  <url type="vcs-browser">https://github.com/codewithelvin/remote-desktop-rider</url>
  <screenshots>
    <screenshot type="default">
      <caption>A remote session</caption>
      <image>$RAW/docs/screenshots/session.jpg</image>
    </screenshot>
    <screenshot>
      <caption>The computers this device controls</caption>
      <image>$RAW/docs/screenshots/computers.png</image>
    </screenshot>
  </screenshots>
  <content_rating type="oars-1.1"/>
  <supports>
    <control>pointing</control>
    <control>keyboard</control>
  </supports>
  <releases>
    <release version="$VERSION" date="$(date -u +%Y-%m-%d)">
      <url>https://github.com/codewithelvin/remote-desktop-rider/releases/tag/v$VERSION</url>
    </release>
  </releases>
</component>
EOF

cat > "$DIR/DEBIAN/control" <<EOF
Package: $PKG
Version: $VERSION
Section: net
Priority: optional
Architecture: amd64
Maintainer: Elvin Huseynov <codewithelvin@users.noreply.github.com>
Depends: libc6 (>= 2.35), libgcc-s1, libstdc++6, libgl1, libegl1, libxkbcommon0, libxkbcommon-x11-0, libx11-6, libxcursor1, libxi6, libxrandr2, libwayland-client0, libwayland-egl1
Recommends: fonts-ubuntu | fonts-noto-core | fonts-dejavu-core
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
