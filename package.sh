#!/usr/bin/env bash
# Build Redbull.app — a self-contained macOS menu-bar app bundle.
#
# By default it builds a native release binary. To bundle a prebuilt binary
# instead (e.g. a universal binary from release.sh), set REDBULL_BIN:
#
#     REDBULL_BIN=path/to/redbull ./package.sh
set -euo pipefail
cd "$(dirname "$0")"

APP="Redbull.app"
BIN="redbull"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"

if [ -n "${REDBULL_BIN:-}" ]; then
    echo "==> Using prebuilt binary: $REDBULL_BIN"
    SRC_BIN="$REDBULL_BIN"
else
    echo "==> Building release binary (native)"
    cargo build --release
    SRC_BIN="target/release/$BIN"
fi

echo "==> Generating app icon"
# Regenerate AppIcon.icns from the bolt artwork if it's missing.
if [ ! -f assets/AppIcon.icns ]; then
    ( cd assets
      rustc -O gen_icon.rs -o /tmp/redbull_gen_icon && /tmp/redbull_gen_icon
      rm -rf Redbull.iconset && mkdir Redbull.iconset
      for spec in "16 icon_16x16" "32 icon_16x16@2x" "32 icon_32x32" "64 icon_32x32@2x" \
                  "128 icon_128x128" "256 icon_128x128@2x" "256 icon_256x256" \
                  "512 icon_256x256@2x" "512 icon_512x512" "1024 icon_512x512@2x"; do
          set -- $spec
          sips -z "$1" "$1" icon_1024.png --out "Redbull.iconset/$2.png" >/dev/null
      done
      iconutil -c icns Redbull.iconset -o AppIcon.icns
      rm -rf Redbull.iconset )
fi

echo "==> Assembling $APP (v$VERSION)"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$SRC_BIN" "$APP/Contents/MacOS/$BIN"
chmod +x "$APP/Contents/MacOS/$BIN"
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>            <string>Redbull</string>
    <key>CFBundleDisplayName</key>     <string>Redbull</string>
    <key>CFBundleIdentifier</key>      <string>com.redbull.stayawake</string>
    <key>CFBundleVersion</key>         <string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleExecutable</key>      <string>redbull</string>
    <key>CFBundleIconFile</key>        <string>AppIcon</string>
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

echo "==> Done: $(pwd)/$APP  ($(lipo -archs "$APP/Contents/MacOS/$BIN" 2>/dev/null || echo native))"
echo "    Run it:    open $APP"
echo "    Install:   cp -r $APP /Applications/"
