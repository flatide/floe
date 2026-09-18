#!/bin/sh
# Read-only embedded WebView inventory: no browser, sudo, package install,
# network connection, DISPLAY contents, hostname, source/cache path or token.
set -eu
[ "$#" -le 1 ] || { echo 'audit_desktop_env: too many arguments' >&2; exit 2; }
case "${1:-}" in
    '') ;;
    -h|--help)
        echo 'Usage: sh tools/audit_desktop_env.sh'
        echo 'Read-only OS/WebKitGTK inventory; does not prove ETX compatibility.'
        exit 0 ;;
    *) echo 'audit_desktop_env: unexpected argument (use --help)' >&2; exit 2 ;;
esac

echo 'desktop_audit_schema=1'
echo "os=$(uname -s)"
echo "arch=$(uname -m)"
echo "libc=$(getconf GNU_LIBC_VERSION 2>/dev/null || echo not_detected)"
if [ -r /etc/os-release ]; then
    # Parse values as data, never source an environment file as shell code.
    sed -n -e 's/^ID=\([A-Za-z0-9_.-]*\)$/distribution=\1/p' \
        -e 's/^ID="\([A-Za-z0-9_.-]*\)"$/distribution=\1/p' \
        -e 's/^VERSION_ID=\([0-9.]*\)$/distribution_version=\1/p' \
        -e 's/^VERSION_ID="\([0-9.]*\)"$/distribution_version=\1/p' /etc/os-release
fi
[ -z "${DISPLAY:-}" ] && echo 'display=unset' || echo 'display=set'
[ -z "${WAYLAND_DISPLAY:-}" ] && echo 'wayland_display=unset' || echo 'wayland_display=set'

for package in gtk3 webkit2gtk3 webkit2gtk4.1 libsoup libsoup3; do
    if command -v rpm >/dev/null 2>&1; then
        if version=$(rpm -q --qf '%{VERSION}-%{RELEASE}.%{ARCH}\n' "$package" 2>/dev/null); then
            printf 'rpm_%s=%s\n' "$package" "$version"
        else
            printf 'rpm_%s=not_installed\n' "$package"
        fi
    else
        printf 'rpm_%s=unavailable\n' "$package"
    fi
done
for api in gtk+-3.0 webkit2gtk-4.0 webkit2gtk-4.1 libsoup-2.4 libsoup-3.0; do
    if command -v pkg-config >/dev/null 2>&1; then
        if version=$(pkg-config --modversion "$api" 2>/dev/null); then
            printf 'devel_%s=%s\n' "$api" "$version"
        else
            printf 'devel_%s=not_detected\n' "$api"
        fi
    else
        printf 'devel_%s=unavailable\n' "$api"
    fi
done
echo 'devel_note=missing_pkg_config_metadata_does_not_prove_missing_runtime'
echo 'webkit_abi_note=4.0_and_4.1_are_not_interchangeable'
echo 'embedded_startup=not_measured'
echo 'etx_pixels_input_latency=not_measured'
echo 'result=inventory_only'
