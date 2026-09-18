#!/bin/sh
# Explicit native GUI gate, not silently run by the headless/Linux battery.
set -eu
cd "$(dirname "$0")/.."
case "$(uname -s)" in
    Darwin) ;;
    *) echo 'desktop: macOS host only; RHEL 8/ETX acceptance remains pending' >&2; exit 2 ;;
esac
[ "$#" -eq 0 ] || { echo 'usage: sh tools/validate_desktop.sh' >&2; exit 2; }
repo=$PWD
(cd rust && cargo build --release --offline --locked -p floe-index -p floe-renderd)
(cd desktop && cargo fmt -- --check && cargo test --offline --locked && cargo build --offline --locked)
node rust/web/ui/session-exit.test.cjs
FLOE_INDEX_BIN="$repo/rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$repo/rust/target/release/floe-renderd" \
    desktop/target/debug/floe2-desktop --smoke-test
