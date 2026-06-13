#!/usr/bin/env bash
# Build a universal (Apple Silicon + Intel) Redbull.app and package release
# artifacts (.zip and .dmg) into dist/.
set -euo pipefail
cd "$(dirname "$0")"

BIN="redbull"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
ARM_TARGET="aarch64-apple-darwin"
X86_TARGET="x86_64-apple-darwin"
UNIVERSAL="target/universal/$BIN"
DIST="dist"

echo "==> Ensuring rust targets are installed"
rustup target add "$ARM_TARGET" "$X86_TARGET" >/dev/null

echo "==> Building $ARM_TARGET"
cargo build --release --target "$ARM_TARGET"
echo "==> Building $X86_TARGET"
cargo build --release --target "$X86_TARGET"

echo "==> Creating universal binary with lipo"
mkdir -p "$(dirname "$UNIVERSAL")"
lipo -create -output "$UNIVERSAL" \
    "target/$ARM_TARGET/release/$BIN" \
    "target/$X86_TARGET/release/$BIN"
lipo -archs "$UNIVERSAL"

echo "==> Assembling universal Redbull.app"
REDBULL_BIN="$UNIVERSAL" ./package.sh

echo "==> Packaging release artifacts into $DIST/"
rm -rf "$DIST" && mkdir -p "$DIST"
ZIP="$DIST/Redbull-$VERSION-universal.zip"
DMG="$DIST/Redbull-$VERSION-universal.dmg"

# .zip (use ditto to preserve bundle structure / resource forks)
ditto -c -k --sequesterRsrc --keepParent Redbull.app "$ZIP"

# .dmg with an Applications symlink for drag-to-install
STAGE="$(mktemp -d)"
cp -R Redbull.app "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "Redbull" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"

echo
echo "==> Release artifacts (v$VERSION):"
( cd "$DIST" && shasum -a 256 ./* )
ls -lh "$DIST" | sed 's/^/    /'
