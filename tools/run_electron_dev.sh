#!/bin/sh
# Comparison host only. Preserve caller cwd, quoted arguments, and explicit overrides.
set -eu
repo=$(CDPATH= cd -P "$(dirname "$0")/.." && pwd)
case "$(uname -s)" in Darwin|Linux) ;; *) echo 'Electron comparison: macOS/Linux only' >&2; exit 2 ;; esac
if [ -z "${FLOE_ELECTRON_BIN-}" ] || [ ! -x "$FLOE_ELECTRON_BIN" ]; then
    echo 'Set FLOE_ELECTRON_BIN to the checksum-verified pinned Electron executable.' >&2
    exit 2
fi
exec "$FLOE_ELECTRON_BIN" "$repo/electron" "$@"
