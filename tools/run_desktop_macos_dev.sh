#!/bin/sh
# Run the native binary directly so all relative CLI paths keep the caller's
# working directory. LaunchServices/open cannot preserve that directory.
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
profile=debug
build_arg=
if [ "${1-}" = --release ]; then
    profile=release
    build_arg=--release
    shift
fi
[ "$(uname -s)" = Darwin ] || { echo 'macOS preview only' >&2; exit 2; }
binary="$repo/desktop/target/$profile/floe2-desktop"
workers="$repo/rust/target/release"
receipt="$repo/desktop/target/macos-preview-$profile.path"
bad_receipt() {
    echo "Invalid/stale $profile preview receipt; rebuild: sh tools/build_desktop_macos_dev.sh $build_arg" >&2
    exit 1
}
if [ -e "$receipt" ] || [ -L "$receipt" ]; then
    [ ! -L "$receipt" ] && [ -f "$receipt" ] && [ -r "$receipt" ] || bad_receipt
    # Read one literal path, not shell input. An invalid receipt never silently
    # falls back to a stale binary that lacks the matching packaged notices.
    app=$(head -c 8192 "$receipt")
    [ "$(wc -c < "$receipt")" -lt 8192 ] || bad_receipt
    case "$app" in
        "$repo"/desktop/target/macos-dev.*/Floe2.app) ;;
        *) bad_receipt ;;
    esac
    suffix=${app#"$repo/desktop/target/macos-dev."}
    suffix=${suffix%/Floe2.app}
    case "$suffix" in ''|*[!a-zA-Z0-9]*) bad_receipt ;; esac
    binary="$app/Contents/MacOS/floe2-desktop"
    workers="$app/Contents/MacOS"
    [ -x "$binary" ] && [ -f "$app/Contents/Resources/NOTICE-INDEX.json" ] || bad_receipt
fi
[ -x "$binary" ] || {
    echo "Build the $profile desktop preview first: sh tools/build_desktop_macos_dev.sh $build_arg" >&2
    exit 1
}
# Explicit overrides (including invalid/empty ones) stay authoritative.
FLOE_INDEX_BIN=${FLOE_INDEX_BIN-"$workers/floe-index"}
FLOE_RENDERD_BIN=${FLOE_RENDERD_BIN-"$workers/floe-renderd"}
export FLOE_INDEX_BIN FLOE_RENDERD_BIN
exec "$binary" "$@"
