#!/usr/bin/env bash
# Build "Remote Desktop Rider.app" (Apple Silicon + Intel) and a .dmg.
#
#   packaging/macos/build.sh 0.3.0      ->  target/dist/RemoteDesktopRider-0.3.0-macos.dmg
#
# Runs on macOS with Xcode command-line tools and rustup. The app is signed
# ad hoc (no Apple Developer ID): on first launch macOS asks to confirm it
# (System Settings > Privacy & Security > Open Anyway).
set -euo pipefail

VERSION="${1:?usage: build.sh <version>}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

NAME="Remote Desktop Rider"
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
export MACOSX_DEPLOYMENT_TARGET=11.0

for t in "${TARGETS[@]}"; do
  rustup target add "$t" >/dev/null
  cargo build --release --target "$t" -p rider-app -p rider-client
done

OUT="target/dist"
APP="$OUT/$NAME.app"
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# One universal binary per program.
for bin in RemoteDesktopRider rider-client; do
  lipo -create -output "$APP/Contents/MacOS/$bin" \
    "target/aarch64-apple-darwin/release/$bin" "target/x86_64-apple-darwin/release/$bin"
done

# Icon: every size iconutil wants, drawn by rider_core::icon.
ICONSET="$OUT/AppIcon.iconset"
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  cargo run -q --release -p rider-core --example icon-png -- "$s" "$ICONSET/icon_${s}x${s}.png"
  cargo run -q --release -p rider-core --example icon-png -- "$((s * 2))" "$ICONSET/icon_${s}x${s}@2x.png"
done
iconutil -c icns -o "$APP/Contents/Resources/AppIcon.icns" "$ICONSET"
rm -rf "$ICONSET"

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIdentifier</key><string>io.github.codewithelvin.remote-desktop-rider</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleExecutable</key><string>RemoteDesktopRider</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleLocalizations</key><array><string>en</string><string>az</string></array>
  <key>LSMinimumSystemVersion</key><string>$MACOSX_DEPLOYMENT_TARGET</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>© Elvin Huseynov</string>
</dict>
</plist>
EOF

# Ad-hoc signature: required to run on Apple Silicon; no developer account.
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

# The disk image: the app plus a link to Applications to drag it onto.
STAGE="$OUT/dmg"
rm -rf "$STAGE" && mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
DMG="$OUT/RemoteDesktopRider-$VERSION-macos.dmg"
rm -f "$DMG"
hdiutil create -volname "$NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
echo "$DMG"
