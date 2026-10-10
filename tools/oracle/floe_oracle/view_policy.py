"""KLayout-free policy shared by the legacy and Rust viewers.

Keep cache-derived limits and reserved layer selection here so importing the
native GUI for the Rust renderer does not load ``klayout.db`` indirectly.
"""


MAX_LIVE_TILES = 32
EVICT_ABOVE = 128


def frame_layer(meta):
    """Return the runtime hierarchy-frame layer.

    The rule must match ``floe_vfs::frame_layer`` exactly.  It depends only on
    cache metadata; no layout database is needed.
    """
    used = {int(layer["layer"]) for layer in meta.get("layers", [])}
    value = max(min(max(used) + 1, 0xFFFFFFFF) if used else 1, 1)
    while value in used:
        value -= 1
    return value, 0


def live_caps(meta):
    """Return live-page and eviction caps scaled by average tile size."""
    try:
        grid = meta["grid"]
        average = meta["src"]["size"] / max(
            1, grid["nx"] * grid["ny"])
        factor = max(1, min(8, round(6e6 / max(1.0, average))))
    except (KeyError, TypeError):
        factor = 1
    return MAX_LIVE_TILES * factor, EVICT_ABOVE * factor
