#!/usr/bin/env bash
# Build size-optimized Redbull.app packages for both macOS architectures
# (Apple Silicon arm64 + Intel x86_64) and write .dmg/.zip per arch to dist/.
#
# Uses the nightly toolchain to recompile std for size (-Z build-std) with the
# immediate-abort panic strategy, which strips panic/unwind machinery (~104K
# per binary). Requires the nightly toolchain.
set -euo pipefail
cd "$(dirname "$0")"

BIN="redbull"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
DIST="dist"
RUSTFLAGS_OPT="-Zunstable-options -Cpanic=immediate-abort"

echo "==> Ensuring nightly toolchain + rust-src"
rustup toolchain install nightly --profile minimal >/dev/null 2>&1 || true
rustup component add rust-src --toolchain nightly >/dev/null

rm -rf "$DIST" && mkdir -p "$DIST"

build_arch () {
    local target="$1" label="$2"
    echo "==> Building $target (build-std, immediate-abort)"
    RUSTFLAGS="$RUSTFLAGS_OPT" cargo +nightly build --release \
        -Z build-std=std,panic_abort --target "$target"
    local bin="target/$target/release/$BIN"
    lipo -archs "$bin"

    echo "==> Packaging $label"
    REDBULL_BIN="$bin" ./package.sh >/dev/null
    ditto -c -k --sequesterRsrc --keepParent Redbull.app "$DIST/Redbull-$VERSION-$label.zip"
    local stage; stage="$(mktemp -d)"
    cp -R Redbull.app "$stage/"
    ln -s /Applications "$stage/Applications"
    hdiutil create -quiet -volname "Redbull" -srcfolder "$stage" -ov -format UDZO \
        "$DIST/Redbull-$VERSION-$label.dmg"
    rm -rf "$stage"
}

# Intel first, Apple Silicon last so the leftover Redbull.app is native arm64.
build_arch x86_64-apple-darwin x86_64
build_arch aarch64-apple-darwin arm64

echo
echo "==> Release artifacts (v$VERSION):"
( cd "$DIST" && shasum -a 256 ./* )
ls -lh "$DIST" | sed 's/^/    /'
