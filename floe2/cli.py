"""Rust-only command-line entry point for floe2."""

import os


def main(argv=None):
    backend = os.environ.get("FLOE_RENDERER", "rust").strip().lower() or "rust"
    if backend != "rust":
        raise SystemExit(
            "floe2 is Rust-only; FLOE_RENDERER=%r is not supported" % backend)
    os.environ["FLOE_RENDERER"] = "rust"

    # Import after fixing the product/backend contract.  This keeps direct
    # ``python -m floe2`` startup KLayout-free and gives argparse the right
    # product name without duplicating the shared command implementation.
    from floe.cli import main as shared_main
    return shared_main(argv, prog="floe2", rust_only=True)
