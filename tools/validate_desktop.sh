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
node desktop/ui/menu-action.test.cjs
node desktop/ui/ime-probe.test.cjs
node desktop/ui/recovery-probe.test.cjs
node desktop/ui/review-transport.test.cjs
node desktop/ui/session-loss.test.cjs
node desktop/ui/download-cancel-probe.test.cjs
FLOE_INDEX_BIN="$repo/rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$repo/rust/target/release/floe-renderd" \
    desktop/target/debug/floe2-desktop --smoke-test
FLOE_INDEX_BIN="$repo/rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$repo/rust/target/release/floe-renderd" \
    desktop/target/debug/floe2-desktop --smoke-test-recovery
FLOE_INDEX_BIN="$repo/rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$repo/rust/target/release/floe-renderd" \
    desktop/target/debug/floe2-desktop --smoke-test-review-recovery
FLOE_INDEX_BIN="$repo/rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$repo/rust/target/release/floe-renderd" \
    desktop/target/debug/floe2-desktop --smoke-test-storage-loss
FLOE_INDEX_BIN="$repo/rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$repo/rust/target/release/floe-renderd" \
    desktop/target/debug/floe2-desktop --smoke-test-cookie-loss
FLOE_INDEX_BIN="$repo/rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$repo/rust/target/release/floe-renderd" \
    desktop/target/debug/floe2-desktop --smoke-test-download-cancel
FLOE_INDEX_BIN="$repo/rust/target/release/floe-index" \
FLOE_RENDERD_BIN="$repo/rust/target/release/floe-renderd" \
    desktop/target/debug/floe2-desktop --smoke-test-download-publish
# A cleanup failure must reach the ordinary error exit, even after a successful
# session shutdown. Require both the real native QA verdict and the exact error
# route; an unrelated crash/nonzero status cannot pass this gate.
echo '== desktop cleanup-failure injection (expected error exit 1)'
if cleanup_output=$(FLOE_INDEX_BIN="$repo/rust/target/release/floe-index" \
    FLOE_RENDERD_BIN="$repo/rust/target/release/floe-renderd" \
    desktop/target/debug/floe2-desktop --smoke-test-download-cleanup-failure 2>&1); then
    printf '%s\n' "$cleanup_output"
    echo 'desktop: expected cleanup failure was silently reported as success' >&2
    exit 1
else
    cleanup_status=$?
fi
printf '%s\n' "$cleanup_output"
[ "$cleanup_status" -eq 1 ]
case "$cleanup_output" in *'DESKTOP DOWNLOAD CLEANUP FAILURE: OK'*) ;; *) exit 1 ;; esac
case "$cleanup_output" in *'floe2-desktop: Session ended, but private download temporary-file cleanup was not confirmed.'*) ;; *) exit 1 ;; esac
echo 'DESKTOP CLEANUP ERROR EXIT: OK (expected exit 1; no modal in isolated QA)'
