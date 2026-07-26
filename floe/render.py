"""Headless PNG rendering via klayout.lay (no display required)."""

import klayout.db as db

try:
    import klayout.lay as klay
except ImportError:  # pragma: no cover
    klay = None

_VIEW_CONFIG = {
    "background-color": "#000000",
    "grid-visible": "false",
    "text-visible": "false",
    "cell-box-visible": "false",
}


def _require_lay():
    if klay is None:
        raise RuntimeError("klayout.lay not available - PNG rendering "
                           "requires the klayout pip package with lay module")


class Renderer:
    """Wraps a LayoutView bound to one Layout object.

    The layout may grow after construction (e.g. mosaic tiles loaded on
    demand); call refresh() afterwards so new cells/layers are picked up.
    """

    def __init__(self, layout, top_cell, colors=None, hier_offset=0,
                 show_texts=False, hollow=()):
        """hier_offset: artificial hierarchy levels above the design top
        (the tile mosaic adds 2: FLOE_MOSAIC -> TILE_r_c -> design cells);
        user-facing depth values are shifted by this amount.
        show_texts: draw text shapes (skeleton view: cell names, labels).
        hollow: (layer, datatype) keys drawn as outlines only."""
        _require_lay()
        self.lv = klay.LayoutView()
        for k, v in _VIEW_CONFIG.items():
            try:
                self.lv.set_config(k, v)
            except Exception:
                pass
        if show_texts:
            try:
                self.lv.set_config("text-visible", "true")
            except Exception:
                pass
        self.layout = layout
        self.top = top_cell
        self.colors = colors or {}  # (layer, datatype) -> "#rrggbb"
        self.hollow = set(hollow)
        self.hier_offset = hier_offset
        self.lv.show_layout(layout, False)
        self.refresh()

    def refresh(self):
        cv = self.lv.cellview(0)
        cv.cell = self.top
        self.lv.add_missing_layers()
        for lp in self.lv.each_layer():
            key = (lp.source_layer, lp.source_datatype)
            hexcol = self.colors.get(key)
            if hexcol:
                col = int(hexcol.lstrip("#"), 16)
                lp.fill_color = col
                lp.frame_color = col
            if key in self.hollow:
                lp.dither_pattern = 1  # hollow: outline only
        self.lv.max_hier()

    def set_visible(self, visible):
        """visible: None (all) or iterable of (layer, datatype)."""
        vis = None if visible is None else set(visible)
        for lp in self.lv.each_layer():
            lp.visible = (vis is None
                          or (lp.source_layer, lp.source_datatype) in vis)

    def render_png(self, out_path, x0, y0, x1, y1, w, h, visible=None,
                   depth=None):
        """Render bbox given in dbu to a PNG file of w x h pixels.

        depth: Calibre-style hierarchy depth. None = full hierarchy;
        0 = design-top shapes only, N = expand N levels below the design
        top. Cells beyond the limit are drawn as outline frames with their
        cell name.
        """
        self.set_visible(visible)
        if depth is None:
            self._config("cell-box-visible", "false")
            self.lv.max_hier()
        else:
            self._config("cell-box-visible", "true")
            self.lv.min_hier_levels = 0
            self.lv.max_hier_levels = max(0, depth) + self.hier_offset
        dbu = self.layout.dbu
        self.lv.zoom_box(db.DBox(x0 * dbu, y0 * dbu, x1 * dbu, y1 * dbu))
        self.lv.save_image(out_path, w, h)
        return out_path

    def _config(self, k, v):
        try:
            self.lv.set_config(k, v)
        except Exception:
            pass
