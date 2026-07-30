#!/bin/sh
# Cross-build the floe-index binary for the closed-network Linux
# hosts, from any machine with rustup (macOS/Linux/aarch64 alike).
#
# The output is a fully static musl ELF: no glibc version coupling,
# runs on any x86_64 Linux. No C toolchain is needed - the dependency
# tree is pure Rust and rustc's bundled rust-lld does the linking
# (see .cargo/config.toml).
#
#   sh rust/build-linux.sh
#   -> rust/dist/floe-index-linux-x86_64  (+ .sha256)
#
# Carry the binary in, `chmod +x`, verify with the checksum, smoke:
#   ./floe-index-linux-x86_64 scan some.oas 16
set -e
cd "$(dirname "$0")"
PATH="$HOME/.cargo/bin:$PATH"
rustup target add x86_64-unknown-linux-musl >/dev/null
cargo build --release --target x86_64-unknown-linux-musl
mkdir -p dist
cp target/x86_64-unknown-linux-musl/release/floe-index \
   dist/floe-index-linux-x86_64
(cd dist && shasum -a 256 floe-index-linux-x86_64 \
    > floe-index-linux-x86_64.sha256)
ls -lh dist/floe-index-linux-x86_64
cat dist/floe-index-linux-x86_64.sha256
