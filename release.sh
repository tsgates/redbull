#!/usr/bin/env bash
# Build a size-optimized Apple Silicon (arm64) Redbull.app and package release
# artifacts (.zip and .dmg) into dist/.
#
# Uses the nightly toolchain to recompile std for size (-Z build-std) with the
# immediate-abort panic strategy, which strips panic/unwind machinery. This
# takes the binary from ~347K (stable) to ~104K. Requires the nightly toolchain.
set -euo pipefail
cd "$(dirname "$0")"

BIN="redbull"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
TARGET="aarch64-apple-darwin"
DIST="dist"

echo "==> Ensuring nightly toolchain + rust-src"
rustup toolchain install nightly --profile minimal >/dev/null 2>&1 || true
rustup component add rust-src --toolchain nightly >/dev/null

echo "==> Building $TARGET (build-std, immediate-abort)"
RUSTFLAGS="-Zunstable-options -Cpanic=immediate-abort" \
    cargo +nightly build --release \
    -Z build-std=std,panic_abort \
    --target "$TARGET"
BIN_PATH="target/$TARGET/release/$BIN"
lipo -archs "$BIN_PATH"

echo "==> Assembling Redbull.app"
REDBULL_BIN="$BIN_PATH" ./package.sh

echo "==> Packaging release artifacts into $DIST/"
rm -rf "$DIST" && mkdir -p "$DIST"
ZIP="$DIST/Redbull-$VERSION-arm64.zip"
DMG="$DIST/Redbull-$VERSION-arm64.dmg"

# .zip (use ditto to preserve bundle structure / resource forks)
ditto -c -k --sequesterRsrc --keepParent Redbull.app "$ZIP"

# .dmg with an Applications symlink for drag-to-install
STAGE="$(mktemp -d)"
cp -R Redbull.app "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "Redbull" -srcfolder "$STAGE" -ov -format UDZO "$DMG"
rm -rf "$STAGE"

echo
echo "==> Release artifacts (v$VERSION, arm64):"
( cd "$DIST" && shasum -a 256 ./* )
ls -lh "$DIST" | sed 's/^/    /'
