#!/bin/sh
# A terminal launcher, not a renamed/signed Finder application.
set -eu
bundle=$(CDPATH= cd -P "$(dirname "$0")" && pwd)
if [ -n "${ELECTRON_RUN_AS_NODE-}${NODE_OPTIONS-}${NODE_PATH-}" ]; then
    echo 'floe2 Electron: unset ELECTRON_RUN_AS_NODE, NODE_OPTIONS and NODE_PATH.' >&2
    exit 2
fi
if [ "${FLOE_ELECTRON_BIN+x}" = x ]; then
    case "$FLOE_ELECTRON_BIN" in /*) ;; *) echo 'FLOE_ELECTRON_BIN must be an absolute executable path.' >&2; exit 2 ;; esac
    runtime=$FLOE_ELECTRON_BIN
else
    runtime="$bundle/runtime/@RUNTIME@"
fi
test -f "$runtime" && test -x "$runtime" || { echo 'Electron executable unavailable; no fallback.' >&2; exit 2; }
# Do not cd: relative sources remain relative to the invoking terminal.
# Explicit service/index/renderd overrides retain the host's fail-closed policy.
exec "$runtime" "$bundle/electron" "$@"
