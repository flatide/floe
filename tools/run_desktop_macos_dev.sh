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
[ -x "$binary" ] || {
    echo "Build the $profile desktop preview first: sh tools/build_desktop_macos_dev.sh $build_arg" >&2
    exit 1
}
# Explicit overrides (including invalid/empty ones) stay authoritative.
FLOE_INDEX_BIN=${FLOE_INDEX_BIN-"$repo/rust/target/release/floe-index"}
FLOE_RENDERD_BIN=${FLOE_RENDERD_BIN-"$repo/rust/target/release/floe-renderd"}
export FLOE_INDEX_BIN FLOE_RENDERD_BIN
exec "$binary" "$@"
