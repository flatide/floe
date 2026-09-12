#!/bin/sh
# Read-only M0 inventory. No browser launch, profile writes or network probes.
set -eu

firefox_bin=
explicit_firefox=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --firefox)
            if [ "$#" -lt 2 ] || [ -z "$2" ]; then
                echo 'audit_webui_env: --firefox needs an executable path' >&2
                exit 2
            fi
            firefox_bin=$2
            explicit_firefox=1
            shift 2
            ;;
        -h|--help)
            echo 'Usage: sh tools/audit_webui_env.sh [--firefox /path/to/firefox]'
            echo 'Print basic versions only; no GUI/profile/network/performance test.'
            exit 0
            ;;
        *)
            echo 'audit_webui_env: unknown option (use --help)' >&2
            exit 2
            ;;
    esac
done

if [ "$explicit_firefox" -eq 1 ]; then
    # A path, not a shell command; never eval user-supplied text.
    case "$firefox_bin" in
        /*|./*|../*) ;;
        *)
            echo 'audit_webui_env: --firefox must be a path, not a command' >&2
            exit 2
            ;;
    esac
    if [ ! -f "$firefox_bin" ] || [ ! -x "$firefox_bin" ]; then
        echo 'audit_webui_env: --firefox is not an executable file' >&2
        exit 2
    fi
else
    firefox_bin=$(command -v firefox 2>/dev/null || :)
    if [ -z "$firefox_bin" ] && [ -x /Applications/Firefox.app/Contents/MacOS/firefox ]; then
        firefox_bin=/Applications/Firefox.app/Contents/MacOS/firefox
    fi
fi

echo 'audit_schema=1'
echo "os=$(uname -s)"
echo "arch=$(uname -m)"
echo "libc=$(getconf GNU_LIBC_VERSION 2>/dev/null || echo not_detected)"
if [ -n "${DISPLAY:-}" ]; then
    echo 'display=set'
else
    echo 'display=unset'
fi
if [ -n "${WAYLAND_DISPLAY:-}" ]; then
    echo 'wayland_display=set'
else
    echo 'wayland_display=unset'
fi

if [ -n "$firefox_bin" ]; then
    # Keep only the version banner. Do not print stderr paths or env values.
    if firefox_version=$("$firefox_bin" --version 2>/dev/null); then
        printf 'firefox=%s\n' "$(printf '%s\n' "$firefox_version" | sed -n '1p')"
    else
        echo 'firefox=version_command_failed'
    fi
else
    echo 'firefox=not_found'
fi
if command -v rustc >/dev/null 2>&1; then
    rust_version=$(rustc --version 2>/dev/null || echo version_command_failed)
    printf 'rustc=%s\n' "$rust_version"
else
    echo 'rustc=not_found'
fi

echo 'browser_features=not_measured'
echo 'profile_isolation=not_measured'
echo 'etx_input_and_latency=not_measured'
echo 'portal_and_remote_access=not_measured'
echo 'result=inventory_only'
