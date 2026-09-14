#!/bin/sh
set -eu
verify_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
cd "$verify_root"
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c SHA256SUMS
elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c SHA256SUMS
else
    echo 'verification requires sha256sum or shasum' >&2
    exit 1
fi
