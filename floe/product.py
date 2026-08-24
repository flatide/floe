"""Small product boundary shared by the ``floe`` and ``floe2`` shells.

The implementation and Rust workspace deliberately stay canonical in this
repository.  ``floe2`` selects its product identity before importing the
shared modules, which gives it a separate window/socket identity without
copying renderer, cache, or GUI code.
"""

import os


_PRODUCT_ENV = "FLOE_PRODUCT"
_PRODUCTS = ("floe", "floe2")


def name():
    """Return the active product name.

    ``FLOE_PRODUCT`` is an internal hand-off used by the ``floe2`` package and
    inherited render children.  Treat an invalid value as a configuration
    error instead of silently joining the other product's instance socket.
    """
    product = os.environ.get(_PRODUCT_ENV, "floe").strip().lower() or "floe"
    if product not in _PRODUCTS:
        raise RuntimeError(
            "%s must be floe or floe2, got %r" % (_PRODUCT_ENV, product))
    return product


def default_renderer():
    """Stable floe uses KLayout; the floe2 product is Rust-only."""
    return "rust" if name() == "floe2" else "klayout"
