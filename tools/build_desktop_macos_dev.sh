#!/bin/sh
# Local preview only: unsigned/unnotarized, not a distributable D3 package.
set -eu
cd -P "$(dirname "$0")/.."
profile=debug
case "$#:${1-}" in
    0:) ;;
    1:--release) profile=release ;;
    *) echo 'usage: sh tools/build_desktop_macos_dev.sh [--release]' >&2; exit 2 ;;
esac
[ "$(uname -s)" = Darwin ] || { echo 'macOS preview only' >&2; exit 2; }
[ -z "${CARGO_BUILD_TARGET-}" ] || { echo 'native macOS preview: unset CARGO_BUILD_TARGET' >&2; exit 2; }
repo=$PWD
if command -v rustup >/dev/null 2>&1; then
    desktop_rustc=$(rustup which rustc)
    desktop_cargo=$(rustup which cargo)
else
    desktop_rustc=$(command -v rustc)
    desktop_cargo=$(command -v cargo)
fi
export FLOE_PACKAGER_RUSTC="$desktop_rustc" FLOE_PACKAGER_CARGO="$desktop_cargo"
(cd rust && RUSTC="$desktop_rustc" "$desktop_cargo" build --release --offline --locked -p floe-index -p floe-renderd -p floe-web-packager --target-dir "$repo/rust/target")
mkdir -p "$repo/desktop/target"
preview=$(mktemp -d "$repo/desktop/target/macos-dev.XXXXXX")
app="$preview/Floe2.app"
mkdir -p "$app/Contents/MacOS"
rust/target/release/floe-web-packager "$repo" --desktop-notices "$app/Contents/Resources" > "$preview/notice-build.txt"
desktop_revision=$(sed -n 's/^source_revision=//p' "$preview/notice-build.txt")
desktop_notice=$(sed -n 's/^notice_index_sha1=//p' "$preview/notice-build.txt")
[ -n "$desktop_revision" ] && [ -n "$desktop_notice" ]
if [ "$profile" = release ]; then
    (cd desktop && RUSTC="$desktop_rustc" FLOE_SRC_REV="$desktop_revision" FLOE_NOTICE_INDEX_SHA1="$desktop_notice" "$desktop_cargo" build --release --offline --locked --target-dir "$repo/desktop/target")
else
    (cd desktop && RUSTC="$desktop_rustc" FLOE_SRC_REV="$desktop_revision" FLOE_NOTICE_INDEX_SHA1="$desktop_notice" "$desktop_cargo" build --offline --locked --target-dir "$repo/desktop/target")
fi
cp desktop/Info.plist "$app/Contents/Info.plist"
cp "desktop/target/$profile/floe2-desktop" rust/target/release/floe-index \
    rust/target/release/floe-renderd "$app/Contents/MacOS/"
cp desktop/NOTICES.md "$app/Contents/Resources/DESKTOP-NOTICES.md"
cp docs/WEBUI_DESKTOP.ko.md "$app/Contents/Resources/DEVELOPMENT-PREVIEW.ko.md"
plutil -lint "$app/Contents/Info.plist" >&2
"$app/Contents/MacOS/floe2-desktop" --check-notices >&2
rust/target/release/floe-web-packager "$repo" --desktop-receipt "$profile" "$app"
printf '%s\n' "$app"
