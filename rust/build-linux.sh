#!/bin/sh
# Build the floe-index binaries for the closed-network Linux hosts.
#
# Each runtime binary is emitted in two variants:
#   dist/<binary>-linux-gnu     - glibc DYNAMIC build (Linux build
#       machines only). ~40% faster on parallel indexing: glibc's
#       AVX memcpy and per-thread malloc arenas beat musl's portable
#       code (MAIN09: 60s vs 97s). Use this when the target host's
#       glibc is same-or-newer than the build machine's - the usual
#       case when build and prod are the same distro family.
#   dist/<binary>-linux-x86_64  - fully static musl ELF: no glibc
#       coupling, runs on ANY x86_64 Linux. The portability fallback.
#
#   sh rust/build-linux.sh
#
# Carry a binary in, `chmod +x`, verify with the checksum, smoke:
#   ./floe-index-linux-gnu scan some.oas 16
set -e
cd "$(dirname "$0")"
PATH="$HOME/.cargo/bin:$PATH"
# rustup manages the musl std on online machines; an offline
# standalone-tarball toolchain (BUILD.md) has it preinstalled
if command -v rustup >/dev/null 2>&1; then
    rustup target add x86_64-unknown-linux-musl >/dev/null
fi
mkdir -p dist
BINS="floe-index floe-renderd floe-render-cli path-inventory"
copy_binaries() {
    suffix=$1
    source_dir=$2
    for binary in $BINS; do
        output="dist/$binary-$suffix"
        cp "$source_dir/$binary" "$output"
        (cd dist && sha256sum "$binary-$suffix" \
            > "$binary-$suffix.sha256" 2>/dev/null \
            || shasum -a 256 "$binary-$suffix" \
            > "$binary-$suffix.sha256")
    done
}
if [ "$(uname -s)" = "Linux" ]; then
    cargo build --release
    copy_binaries linux-gnu target/release
fi
cargo build --release --target x86_64-unknown-linux-musl
copy_binaries linux-x86_64 target/x86_64-unknown-linux-musl/release
ls -lh dist/*-linux-* | grep -v sha256
cat dist/*.sha256
