"""floe - fast viewer/clipper for large OASIS files backed by a spatial tile cache."""

# floe app/display version - bumped on EVERY push (the About dialog,
# --version and the portable bundle name read this), 2026-08-30.
__version__ = "0.12.13"

# Expected version of the bundled Rust binaries. floe-renderd reports
# its CARGO_PKG_VERSION in the ready handshake and the adapter refuses a
# mismatch (rust_render.py), so this MUST equal the built binaries; keep
# it == rust/{cli,renderd,render-cli}/Cargo.toml. Bumped ONLY on pushes
# that rebuild the Rust binaries - that decoupling lets a Python-only
# push advance __version__ without tripping the renderd version guard
# (no rebuild needed on the deploy host).
RENDERD_VERSION = "0.12.12"
