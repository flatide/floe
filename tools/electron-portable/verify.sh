#!/bin/sh
set -eu
bundle=$(CDPATH= cd -P "$(dirname "$0")" && pwd)
exec "$bundle/bin/floe-bundle-check" "$bundle" --electron-verify "$bundle"
