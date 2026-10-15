#!/bin/sh
# Local standalone entry point. DISPLAY is a window destination, not an identity.
set -eu
case "${1-}" in
    -h|--help)
        echo 'Usage: floe2-teebox view [SOURCE ...] [VIEW OPTIONS]'
        echo '       floe2-teebox --check'
        echo 'Linux x64 + DISPLAY; runs the bundled Electron and local Rust backend.'
        echo 'No login, remote service, implicit indexing or security-setting changes.'
        echo 'Use floe2-cli index SOURCE for explicit OASIS/jobdeck indexing.'
        exit 0 ;;
    --check) [ "$#" -eq 1 ] || { echo 'floe2-teebox: --check takes no arguments.' >&2; exit 2; } ;;
    view) ;;
    *) echo 'floe2-teebox: use view [SOURCE ...] [OPTIONS] or --check.' >&2; exit 2 ;;
esac
teebox_bundle=$(CDPATH= cd -P "$(dirname "$0")" && pwd)
[ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ] || {
    echo 'floe2-teebox: Linux x64 only; on macOS use floe2-electron.' >&2; exit 2;
}
teebox_uid=$(id -u)
case "$teebox_uid" in ''|*[!0-9]*|0)
    echo 'floe2-teebox: requires a non-root OS user.' >&2; exit 2 ;;
esac
[ -n "${DISPLAY-}" ] || { echo 'floe2-teebox: DISPLAY is required; inherit the user ETX/X11 session.' >&2; exit 2; }
# A fixed deployment must not silently run another installation's workers.
if [ "${FLOE_ELECTRON_BIN+x}${FLOE_ELECTRON_SERVICE_BIN+x}${FLOE_ELECTRON_DOWNLOAD_BIN+x}${FLOE_INDEX_BIN+x}${FLOE_RENDERD_BIN+x}" != '' ]; then
    echo 'floe2-teebox: unset Electron/service/download/index/renderd binary overrides; use the complete bundle.' >&2
    exit 2
fi
if [ -n "${ELECTRON_RUN_AS_NODE-}${NODE_OPTIONS-}${NODE_PATH-}${NODE_EXTRA_CA_CERTS-}" ]; then
    echo 'floe2-teebox: unset ELECTRON_RUN_AS_NODE, NODE_OPTIONS, NODE_PATH and NODE_EXTRA_CA_CERTS.' >&2
    exit 2
fi
for teebox_file in floe2-electron floe2-cli runtime/electron \
    rust/target/release/floe2-web rust/target/release/floe-index rust/target/release/floe-renderd \
    electron/service/target/release/floe-electron-service \
    electron/service/target/release/floe-electron-download; do
    [ -f "$teebox_bundle/$teebox_file" ] && [ -x "$teebox_bundle/$teebox_file" ] || {
        echo 'floe2-teebox: incomplete/non-executable Linux bundle; rebuild or re-extract into a new directory.' >&2
        exit 2
    }
done
if [ "$1" = --check ]; then
    # Explicit check only: verifier executes trusted bundled Rust, ldd resolves
    # trusted Chromium objects. Neither launches a GUI or reads a design.
    sh "$teebox_bundle/verify.sh"
    # The browser CLI also reports optional Firefox discovery in its JSON.
    # It is irrelevant to this host; use its native success exit, not that text.
    if ! "$teebox_bundle/floe2-cli" selfcheck --adjacent >/dev/null; then
        echo 'floe2-teebox: native selfcheck failed; run floe2-cli selfcheck --adjacent for details.' >&2
        exit 2
    fi
    command -v ldd >/dev/null 2>&1 || { echo 'floe2-teebox: ldd unavailable; dependency check incomplete.' >&2; exit 2; }
    for teebox_file in "$teebox_bundle/runtime/electron" "$teebox_bundle/runtime/"*.so "$teebox_bundle/runtime/"*.so.*; do
        [ -f "$teebox_file" ] || continue
        if ! teebox_deps=$(LC_ALL=C ldd "$teebox_file" 2>&1); then
            echo 'floe2-teebox: Chromium library check failed; ask the administrator to inspect ldd.' >&2
            exit 2
        fi
        case "$teebox_deps" in *'not found'*)
            echo 'floe2-teebox: missing Chromium system libraries; ask the administrator to inspect ldd.' >&2
            exit 2 ;;
        esac
    done
    echo 'TEEBOX STANDALONE: bundle/native/library checks passed; DISPLAY is set.'
    echo 'X11 access, sandbox execution, dynamic optional libraries and RHEL/ETX usability remain untested.'
    exit 0
fi
# Force X11 only for this ETX entry point. Do not overwrite DISPLAY/XAUTHORITY,
# change cwd, detach, create an identity, or add flags that disable window reuse.
FLOE_ELECTRON_X11=1
FLOE_ELECTRON_BIN="$teebox_bundle/runtime/electron"
FLOE_ELECTRON_SERVICE_BIN="$teebox_bundle/electron/service/target/release/floe-electron-service"
FLOE_ELECTRON_DOWNLOAD_BIN="$teebox_bundle/electron/service/target/release/floe-electron-download"
FLOE_INDEX_BIN="$teebox_bundle/rust/target/release/floe-index"
FLOE_RENDERD_BIN="$teebox_bundle/rust/target/release/floe-renderd"
export FLOE_ELECTRON_X11 FLOE_ELECTRON_BIN FLOE_ELECTRON_SERVICE_BIN FLOE_ELECTRON_DOWNLOAD_BIN FLOE_INDEX_BIN FLOE_RENDERD_BIN
exec "$teebox_bundle/floe2-electron" "$@"
