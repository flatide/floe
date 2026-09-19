#!/bin/sh
# Local preview only: unsigned/unnotarized, not a distributable D3 package.
set -eu
cd "$(dirname "$0")/.."
profile=debug
case "$#:${1-}" in
    0:) ;;
    1:--release) profile=release ;;
    *) echo 'usage: sh tools/build_desktop_macos_dev.sh [--release]' >&2; exit 2 ;;
esac
[ "$(uname -s)" = Darwin ] || { echo 'macOS preview only' >&2; exit 2; }
repo=$PWD
(cd rust && cargo build --release --offline --locked -p floe-index -p floe-renderd)
if [ "$profile" = release ]; then
    (cd desktop && cargo build --release --offline --locked)
else
    (cd desktop && cargo build --offline --locked)
fi
preview=$(mktemp -d "$repo/desktop/target/macos-dev.XXXXXX")
app="$preview/Floe2.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp desktop/Info.plist "$app/Contents/Info.plist"
cp "desktop/target/$profile/floe2-desktop" rust/target/release/floe-index \
    rust/target/release/floe-renderd "$app/Contents/MacOS/"
cp desktop/NOTICES.md "$app/Contents/Resources/DESKTOP-NOTICES.md"
cp docs/WEBUI_DESKTOP.ko.md "$app/Contents/Resources/DEVELOPMENT-PREVIEW.ko.md"
plutil -lint "$app/Contents/Info.plist" >&2
printf '%s\n' "$app"
