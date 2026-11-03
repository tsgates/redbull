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

# Signing + notarization (see package.sh). With a Developer ID certificate the
# app and .dmg are signed; with a notarytool keychain profile they are also
# notarized and stapled, so they open without Gatekeeper warnings. Create the
# profile once with:
#     xcrun notarytool store-credentials redbull --apple-id <id> --team-id <team>
IDENTITY="${SIGN_IDENTITY-$(security find-identity -v -p codesigning 2>/dev/null \
    | awk -F'"' '/Developer ID Application/ {print $2; exit}')}"
export SIGN_IDENTITY="${IDENTITY:--}"
NOTARY_PROFILE="${NOTARY_PROFILE:-redbull}"
NOTARIZE=0
if [ "$SIGN_IDENTITY" = "-" ]; then
    echo "==> No Developer ID certificate: building ad-hoc signed packages"
elif xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1; then
    echo "==> Signing as $SIGN_IDENTITY; notarizing with profile '$NOTARY_PROFILE'"
    NOTARIZE=1
else
    echo "==> Signing as $SIGN_IDENTITY; NOT notarizing (no notarytool profile '$NOTARY_PROFILE')"
fi

notarize () {
    local file="$1" out id
    echo "==> Notarizing $(basename "$file") (this takes a few minutes)"
    out="$(xcrun notarytool submit "$file" --keychain-profile "$NOTARY_PROFILE" \
        --wait --output-format json)"
    id="$(sed -n 's/.*"id" *: *"\([^"]*\)".*/\1/p' <<<"$out" | head -1)"
    if ! grep -q '"status" *: *"Accepted"' <<<"$out"; then
        echo "$out" >&2
        [ -n "$id" ] && xcrun notarytool log "$id" --keychain-profile "$NOTARY_PROFILE" >&2
        echo "notarization failed" >&2
        exit 1
    fi
}

build_arch () {
    local target="$1" label="$2"
    echo "==> Building $target (build-std, immediate-abort)"
    RUSTFLAGS="$RUSTFLAGS_OPT" cargo +nightly build --release \
        -Z build-std=std,panic_abort --target "$target"
    local bin="target/$target/release/$BIN"
    lipo -archs "$bin"

    echo "==> Packaging $label"
    REDBULL_BIN="$bin" ./package.sh >/dev/null
    local dmg="$DIST/Redbull-$VERSION-$label.dmg"
    local stage; stage="$(mktemp -d)"
    cp -R Redbull.app "$stage/"
    ln -s /Applications "$stage/Applications"
    hdiutil create -quiet -volname "Redbull" -srcfolder "$stage" -ov -format UDZO "$dmg"
    rm -rf "$stage"
    if [ "$SIGN_IDENTITY" != "-" ]; then
        codesign --force --timestamp --sign "$SIGN_IDENTITY" "$dmg"
    fi
    if [ "$NOTARIZE" = 1 ]; then
        # One submission covers the app inside, so both can be stapled.
        notarize "$dmg"
        xcrun stapler staple -q "$dmg"
        xcrun stapler staple -q Redbull.app
        spctl --assess --type open --context context:primary-signature -v "$dmg"
        spctl --assess --type execute -v Redbull.app
    fi
    ditto -c -k --sequesterRsrc --keepParent Redbull.app "$DIST/Redbull-$VERSION-$label.zip"
}

# Intel first, Apple Silicon last so the leftover Redbull.app is native arm64.
build_arch x86_64-apple-darwin x86_64
build_arch aarch64-apple-darwin arm64

echo
echo "==> Release artifacts (v$VERSION):"
( cd "$DIST" && shasum -a 256 ./* )
ls -lh "$DIST" | sed 's/^/    /'
