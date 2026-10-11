"""floe_oracle - the frozen floe shell (KLayout) and the Python reference
implementations, kept for the gates alone (docs/SHARED_APP_LAYER.ko.md P3).

Not a product: floe2 is the Rust command line (rust/floe2) and the GTK
viewer (floe/), and neither imports this package. The gates hold the
shared Rust to these modules' answers (the cache, the jobdeck plan, the
DRC review, the capture batch, the index locks, svrf) and render with
their KLayout worker as the pixel oracle. The field's stable floe is the
floe-legacy branch's.

`python -m floe_oracle CMD` (with tools/oracle and the checkout on
PYTHONPATH) is the frozen `python -m floe CMD` - index (--legacy .tiles
too), info, render, clip, probe, profile, drc, svrf, jobdeck; the viewer
is the product's (`floe2 view`).
"""

# one version for the checkout: the product's
from floe import RENDERD_VERSION, __version__  # noqa: F401
