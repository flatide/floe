"""The product's identity: floe2 (its window title, messages and
single-instance socket - floe/instance.py).

The package holds floe2's GTK viewer alone since P3
(docs/SHARED_APP_LAYER.ko.md): the frozen floe shell it once shared the
modules with is the dev-only oracle's (tools/oracle/floe_oracle, whose
product.py keeps both names), and the field's stable floe is the
floe-legacy branch's. FLOE_PRODUCT no longer chooses anything here.
"""


def name():
    """The product name: floe2."""
    return "floe2"
