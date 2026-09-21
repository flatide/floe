#!/bin/sh
# Explicit local comparison gate. No runtime download, package install, or sandbox bypass.
set -eu
repo=$(CDPATH= cd -P "$(dirname "$0")/.." && pwd)
if [ -z "${FLOE_ELECTRON_BIN-}" ] || [ ! -x "$FLOE_ELECTRON_BIN" ]; then
    echo 'Set FLOE_ELECTRON_BIN to the checksum-verified pinned Electron executable.' >&2
    exit 2
fi
cd "$repo/electron/service"
cargo fmt --check
cargo test --offline --locked
cargo clippy --offline --locked --all-targets --no-deps -- -D warnings
cargo build --offline --locked
export FLOE_ELECTRON_SERVICE_BIN="$PWD/target/debug/floe-electron-service"
if [ "${FLOE_INDEX_BIN+x}" != x ]; then FLOE_INDEX_BIN="$repo/rust/target/release/floe-index"; fi
if [ "${FLOE_RENDERD_BIN+x}" != x ]; then FLOE_RENDERD_BIN="$repo/rust/target/release/floe-renderd"; fi
export FLOE_INDEX_BIN FLOE_RENDERD_BIN
node --test ../service-client.test.cjs ../service-lifecycle.test.cjs ../host.test.cjs ../layout-qa.test.cjs
cd "$repo"
sh tools/run_electron_dev.sh --smoke-test
echo 'ELECTRON E1 GATE: OK (not layout-performance or RHEL/ETX acceptance)'
