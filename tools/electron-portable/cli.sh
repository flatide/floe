#!/bin/sh
# The matched Rust CLI: indexing/inspection without starting Electron.
set -eu
cli_bundle=$(CDPATH= cd -P "$(dirname "$0")" && pwd)
if [ "${FLOE_INDEX_BIN+x}${FLOE_RENDERD_BIN+x}" != '' ]; then
    echo 'floe2-cli: unset FLOE_INDEX_BIN and FLOE_RENDERD_BIN; use the matched bundle.' >&2
    exit 2
fi
for cli_file in floe2-web floe-index floe-renderd; do
    [ -f "$cli_bundle/rust/target/release/$cli_file" ] && [ -x "$cli_bundle/rust/target/release/$cli_file" ] || {
        echo 'floe2-cli: incomplete/non-executable bundle; no PATH fallback.' >&2
        exit 2
    }
done
FLOE_INDEX_BIN="$cli_bundle/rust/target/release/floe-index"
FLOE_RENDERD_BIN="$cli_bundle/rust/target/release/floe-renderd"
export FLOE_INDEX_BIN FLOE_RENDERD_BIN
exec "$cli_bundle/rust/target/release/floe2-web" "$@"
