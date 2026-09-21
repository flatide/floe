#!/bin/sh
# Development comparison only; reuse the installed, offline Rust packager.
set -eu
electron_repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
exec sh "$electron_repo/tools/make_web_portable.sh" --electron "$@"
