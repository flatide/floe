#!/bin/sh
# Separate from the Python/GTK portable builder. Only installed toolchains;
# do not ask rustup to install targets, fetch crates, or launch a browser.
set -eu
web_repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
if command -v rustup >/dev/null 2>&1; then
    web_rustc=$(rustup which rustc)
    web_cargo=$(rustup which cargo)
else
    web_rustc=$(command -v rustc)
    web_cargo=$(command -v cargo)
fi
web_host=$("$web_rustc" -vV | sed -n 's/^host: //p')
test -n "$web_host"
web_build_dir="$web_repo/rust/target/web-packager"
(
    cd "$web_repo/rust"
    RUSTC="$web_rustc" "$web_cargo" build --offline --locked -p floe-web-packager \
        --target "$web_host" --target-dir "$web_build_dir" --jobs 4
)
export FLOE_PACKAGER_RUSTC="$web_rustc" FLOE_PACKAGER_CARGO="$web_cargo"
exec "$web_build_dir/$web_host/debug/floe-web-packager" "$web_repo" "$@"
