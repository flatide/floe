#!/bin/sh
# Build the floe-index binaries for the closed-network Linux hosts.
#
# Two artifacts:
#   dist/floe-index-linux-gnu     - glibc DYNAMIC build (Linux build
#       machines only). ~40% faster on parallel indexing: glibc's
#       AVX memcpy and per-thread malloc arenas beat musl's portable
#       code (MAIN09: 60s vs 97s). Use this when the target host's
#       glibc is same-or-newer than the build machine's - the usual
#       case when build and prod are the same distro family.
#   dist/floe-index-linux-x86_64  - fully static musl ELF: no glibc
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
if [ "$(uname -s)" = "Linux" ]; then
    cargo build --release
    cp target/release/floe-index dist/floe-index-linux-gnu
    (cd dist && sha256sum floe-index-linux-gnu \
        > floe-index-linux-gnu.sha256 2>/dev/null \
        || shasum -a 256 floe-index-linux-gnu \
        > floe-index-linux-gnu.sha256)
fi
cargo build --release --target x86_64-unknown-linux-musl
cp target/x86_64-unknown-linux-musl/release/floe-index \
   dist/floe-index-linux-x86_64
(cd dist && sha256sum floe-index-linux-x86_64 \
    > floe-index-linux-x86_64.sha256 2>/dev/null \
    || shasum -a 256 floe-index-linux-x86_64 \
    > floe-index-linux-x86_64.sha256)
ls -lh dist/floe-index-linux-* | grep -v sha256
cat dist/*.sha256
