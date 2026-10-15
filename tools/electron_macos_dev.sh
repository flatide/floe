#!/bin/sh
# macOS shortcut for the Electron standalone from a checkout (the "development
# run" of docs/WEBUI_ELECTRON.ko.md): build the Rust workers, the floe2-web CLI
# and the Electron service in release mode, index a layout whose cache is
# missing, then open the sandboxed Electron window through
# tools/run_electron_dev.sh. Nothing is downloaded or installed: the pinned
# official Electron runtime (electron/runtime.json) must already be extracted.
# It is taken from $FLOE_ELECTRON_BIN when set, else from
# ~/Applications/floe-electron-<version>/Electron.app or
# /Applications/floe-electron-<version>/Electron.app.
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
usage() {
    cat <<'EOF'
usage: sh tools/electron_macos_dev.sh [OPTIONS] [SOURCE] [VIEW OPTIONS...]

  1. cargo build --release --offline --locked  (rust: floe-index, floe-renderd,
     floe-app = floe2-web; electron/service: floe-electron-service + download)
  2. floe2-web index SOURCE  when SOURCE is a layout without a cache next to it
  3. sh tools/run_electron_dev.sh view SOURCE VIEW OPTIONS...

OPTIONS (before SOURCE):
  --no-build       skip step 1 (use the release binaries already built)
  --build-only     steps 1 and 2 only; open no window
  --no-index       skip step 2; the window reports a missing cache itself
  --index-jobs N   workers for the automatic index (default 4)
  -h, --help       this text

A jobdeck (.jb) is never indexed automatically: index it yourself with
  rust/target/release/floe2-web index DECK.jb [--level 1,3] --jobs 4
or let the window ask. Without SOURCE the window opens on its folder picker
(give --root /abs/folder). Relative paths stay relative to this terminal.
Set FLOE_ELECTRON_BIN to use another extracted runtime. Rebuild after any UI
change: the UI is embedded in floe-electron-service and floe2-web.

examples:
  sh tools/electron_macos_dev.sh /abs/design.oas
  sh tools/electron_macos_dev.sh /abs/design.oas --goto 200,200,300 --multi
  sh tools/electron_macos_dev.sh /abs/deck.jb --level 1,3
  sh tools/electron_macos_dev.sh --build-only
  sh tools/electron_macos_dev.sh --no-build --root /abs/layouts
EOF
}
build=yes
run=yes
index=auto
index_jobs=4
while [ "$#" -gt 0 ]; do
    case "$1" in
        --no-build) build=no ;;
        --build-only) run=no ;;
        --no-index) index=no ;;
        --index-jobs)
            [ "$#" -ge 2 ] || { echo 'electron_macos_dev: --index-jobs needs N' >&2; exit 2; }
            index_jobs=$2
            shift
            ;;
        --index-jobs=*) index_jobs=${1#--index-jobs=} ;;
        -h|--help) usage; exit 0 ;;
        --) shift; break ;;
        *) break ;;
    esac
    shift
done
case "$index_jobs" in
    ''|*[!0-9]*|0) echo 'electron_macos_dev: --index-jobs must be a positive integer' >&2; exit 2 ;;
esac
[ "$(uname -s)" = Darwin ] || { echo 'electron_macos_dev: macOS only (Linux: see docs/WEBUI_TEEBOX_STANDALONE.ko.md)' >&2; exit 2; }
[ -z "${CARGO_BUILD_TARGET-}" ] || { echo 'electron_macos_dev: native build; unset CARGO_BUILD_TARGET' >&2; exit 2; }
if [ -n "${ELECTRON_RUN_AS_NODE-}${NODE_OPTIONS-}${NODE_PATH-}${NODE_EXTRA_CA_CERTS-}" ]; then
    echo 'electron_macos_dev: unset ELECTRON_RUN_AS_NODE, NODE_OPTIONS, NODE_PATH and NODE_EXTRA_CA_CERTS.' >&2
    exit 2
fi

# SOURCE is the first remaining word that is not an option; it stays in "$@".
source=
if [ "$#" -gt 0 ]; then
    case "$1" in
        -*) ;;
        *) source=$1 ;;
    esac
fi
if [ -n "$source" ] && [ ! -f "$source" ]; then
    echo "electron_macos_dev: SOURCE is not a readable file: $source" >&2
    exit 2
fi

# The pinned runtime, never downloaded here. The dev launcher re-checks the
# version and the sandbox at start.
version=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$repo/electron/runtime.json")
[ -n "$version" ] || { echo 'electron_macos_dev: electron/runtime.json has no version' >&2; exit 2; }
if [ "$run" = yes ] && [ "${FLOE_ELECTRON_BIN+x}" != x ]; then
    for app in "$HOME/Applications/floe-electron-$version/Electron.app" "/Applications/floe-electron-$version/Electron.app"; do
        if [ -x "$app/Contents/MacOS/Electron" ]; then
            FLOE_ELECTRON_BIN="$app/Contents/MacOS/Electron"
            export FLOE_ELECTRON_BIN
            break
        fi
    done
fi
if [ "$run" = yes ] && { [ -z "${FLOE_ELECTRON_BIN-}" ] || [ ! -x "$FLOE_ELECTRON_BIN" ]; }; then
    cat >&2 <<EOF
electron_macos_dev: no Electron $version runtime. Extract the official
electron-v$version-darwin-<arch>.zip (SHA-256 in electron/runtime.json) to
$HOME/Applications/floe-electron-$version/ or set FLOE_ELECTRON_BIN to its
Electron.app/Contents/MacOS/Electron.
EOF
    exit 2
fi
if [ -n "${FLOE_INDEX_BIN-}${FLOE_RENDERD_BIN-}${FLOE_ELECTRON_SERVICE_BIN-}${FLOE_ELECTRON_DOWNLOAD_BIN-}" ]; then
    echo 'electron_macos_dev: note - FLOE_INDEX_BIN/FLOE_RENDERD_BIN/FLOE_ELECTRON_SERVICE_BIN/FLOE_ELECTRON_DOWNLOAD_BIN overrides are in effect; the window uses those binaries, not this build.' >&2
fi

# 1. Build. Same toolchain selection as the other local scripts; vendored
#    crates only, so no network.
if [ "$build" = yes ]; then
    if command -v rustup >/dev/null 2>&1; then
        dev_rustc=$(rustup which rustc)
        dev_cargo=$(rustup which cargo)
    else
        dev_rustc=$(command -v rustc)
        dev_cargo=$(command -v cargo)
    fi
    echo "electron_macos_dev: building the Rust workers and floe2-web (rust/target/release)" >&2
    (cd "$repo/rust" && RUSTC="$dev_rustc" "$dev_cargo" build --release --offline --locked -p floe-index -p floe-renderd -p floe-app)
    echo "electron_macos_dev: building the Electron service (electron/service/target/release)" >&2
    (cd "$repo/electron/service" && RUSTC="$dev_rustc" "$dev_cargo" build --release --offline --locked)
fi
for bin in rust/target/release/floe-index rust/target/release/floe-renderd rust/target/release/floe2-web \
    electron/service/target/release/floe-electron-service electron/service/target/release/floe-electron-download; do
    [ -x "$repo/$bin" ] || { echo "electron_macos_dev: missing $bin; run without --no-build" >&2; exit 1; }
done

# 2. Index a layout whose cache (.<name>.ice next to it, or the legacy
#    <name>.floe; app-core cache::names) is missing. Explicit, once; never
#    --force from here.
if [ -n "$source" ] && [ "$index" = auto ]; then
    case "$source" in
        *.jb|*.JB) echo 'electron_macos_dev: jobdeck given; not indexed automatically (see --help)' >&2 ;;
        *)
            dir=$(dirname -- "$source")
            name=$(basename -- "$source")
            if [ ! -e "$dir/.$name.ice" ] && [ ! -e "$dir/$name.floe" ]; then
                echo "electron_macos_dev: no cache next to $source; indexing with --jobs $index_jobs" >&2
                "$repo/rust/target/release/floe2-web" index "$source" --jobs "$index_jobs"
            fi
            ;;
    esac
fi
[ "$run" = yes ] || { echo 'electron_macos_dev: build done; no window opened (--build-only)' >&2; exit 0; }

# 3. Open the window. run_electron_dev.sh keeps this terminal's cwd and the
#    quoted arguments; its default workers are this checkout's release build.
exec sh "$repo/tools/run_electron_dev.sh" view "$@"
