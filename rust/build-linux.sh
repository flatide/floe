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
#   GNU_GLIBC=2.28 sh rust/build-linux.sh   # the glibc variant for RHEL 8+
#                                            # hosts from any build machine
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
BINS="floe2 floe-index floe-renderd floe-render-cli path-inventory"
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
# GNU_GLIBC=2.28: the glibc variant is linked against that glibc whatever
# the build machine's (cargo-zigbuild + zig take that glibc's symbols only;
# any build OS) - it runs on hosts with that glibc or newer (RHEL 8 = 2.28).
# Unset, the glibc variant is the Linux build machine's own (its glibc floor).
if [ -n "${GNU_GLIBC:-}" ]; then
    command -v cargo-zigbuild >/dev/null || {
        echo "GNU_GLIBC needs cargo-zigbuild: cargo install cargo-zigbuild"; exit 1; }
    command -v zig >/dev/null || python3 -c "import ziglang" 2>/dev/null || {
        echo "GNU_GLIBC needs zig: pip install ziglang (or zig on PATH)"; exit 1; }
    if command -v rustup >/dev/null 2>&1; then
        rustup target add x86_64-unknown-linux-gnu >/dev/null
    fi
    cargo zigbuild --release --target "x86_64-unknown-linux-gnu.$GNU_GLIBC"
    copy_binaries linux-gnu target/x86_64-unknown-linux-gnu/release
elif [ "$(uname -s)" = "Linux" ]; then
    cargo build --release
    copy_binaries linux-gnu target/release
fi
cargo build --release --target x86_64-unknown-linux-musl
copy_binaries linux-x86_64 target/x86_64-unknown-linux-musl/release
ls -lh dist/*-linux-* | grep -v sha256
cat dist/*.sha256
