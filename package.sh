#!/usr/bin/env bash
# Build Redbull.app — a self-contained macOS menu-bar app bundle.
set -euo pipefail
cd "$(dirname "$0")"

APP="Redbull.app"
BIN="redbull"

echo "==> Building release binary"
cargo build --release

echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "target/release/$BIN" "$APP/Contents/MacOS/$BIN"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>            <string>Redbull</string>
    <key>CFBundleDisplayName</key>     <string>Redbull</string>
    <key>CFBundleIdentifier</key>      <string>com.redbull.stayawake</string>
    <key>CFBundleVersion</key>         <string>0.1.0</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>CFBundleExecutable</key>      <string>redbull</string>
    <key>CFBundlePackageType</key>     <string>APPL</string>
    <key>LSMinimumSystemVersion</key>  <string>10.13</string>
    <key>NSHighResolutionCapable</key> <true/>
    <!-- Menu-bar-only app: no Dock icon, no app menu. -->
    <key>LSUIElement</key>            <true/>
</dict>
</plist>
PLIST

echo "==> Ad-hoc code signing"
codesign --force --deep --sign - "$APP" 2>/dev/null || echo "   (codesign skipped)"

echo "==> Done: $(pwd)/$APP"
echo "    Run it:    open $APP"
echo "    Install:   cp -r $APP /Applications/"
