"""The frozen floe shell's product identity (dev-only oracle, P3).

``FLOE_PRODUCT`` says which shell runs: ``floe`` (the frozen KLayout
shell, `python -m floe_oracle`, the KLayout renderer by default) or
``floe2`` (a gate that drives these modules as floe2 did - the Rust
renderer). The product's own identity is floe/product.py's, floe2 alone.
"""

import os


_PRODUCT_ENV = "FLOE_PRODUCT"
_PRODUCTS = ("floe", "floe2")


def name():
    """Return the active product name; an invalid value is a configuration
    error."""
    product = os.environ.get(_PRODUCT_ENV, "floe").strip().lower() or "floe"
    if product not in _PRODUCTS:
        raise RuntimeError(
            "%s must be floe or floe2, got %r" % (_PRODUCT_ENV, product))
    return product


def default_renderer():
    """The frozen floe renders with KLayout; floe2 with Rust alone."""
    return "rust" if name() == "floe2" else "klayout"
