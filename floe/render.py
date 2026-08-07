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
    # Headless save_image must never replace a label by its one-pixel
    # anchor. Labels are request-capped, so real glyph rendering is
    # bounded and is the only representation we want.
    "text-lazy-rendering": "false",
    "text-point-mode": "false",
    "cell-box-visible": "false",
}

# Calibre speckle: a 50% one-physical-pixel checkerboard shared by every
# design layer. KLayout offsets a stipple by paint-plane parity, even when
# every plane names the same custom pattern. Assigning these inverse forms
# alternately cancels that offset and leaves the SAME holes in every layer.
_DESIGN_SPECKLE_STIPPLES = ("*.\n.*", ".*\n*.")


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
                 show_texts=False, hollow=(), speckle=True, above=()):
        """hier_offset: artificial hierarchy levels above the design top
        (the tile mosaic adds 2: FLOE_MOSAIC -> TILE_r_c -> design cells);
        user-facing depth values are shifted by this amount.
        show_texts: draw text shapes (skeleton view: cell names, labels).
        hollow: (layer, datatype) keys drawn as outlines only.
        above: the subset of hollow keys painted OVER the design
        geometry instead of under it (white frames stay readable in
        dense fill; gray dust stays buried).
        speckle: Calibre-style 50%% fill (the live viewer); False keeps
        solid fills - headless exports (`floe render`) are archival
        artifacts, and the speckle restyle is a viewer behavior."""
        _require_lay()
        self.lv = klay.LayoutView()
        for k, v in _VIEW_CONFIG.items():
            try:
                self.lv.set_config(k, v)
            except Exception:
                pass
        self._text_visible = bool(show_texts)
        if show_texts:
            try:
                self.lv.set_config("text-visible", "true")
                # LayoutView's lazy text path substitutes a one-pixel
                # anchor in headless save_image output. Live labels are
                # capped, so render their real glyphs instead.
                self.lv.set_config("text-lazy-rendering", "false")
            except Exception:
                pass
        self.layout = layout
        self.top = top_cell
        self.colors = colors or {}  # (layer, datatype) -> "#rrggbb"
        # Earlier entries paint later, i.e. hollow[0] is the topmost
        # outline plane (white frames above gray ones).
        self._hollow_order = tuple(hollow)
        self.hollow = set(hollow)
        self._above = set(above) & self.hollow
        self.hier_offset = hier_offset
        self.speckle = bool(speckle)
        self.lv.show_layout(layout, False)
        self._design_speckle_patterns = tuple(
            self.lv.add_stipple("floe-calibre-speckle-%d" % i, pattern)
            for i, pattern in enumerate(_DESIGN_SPECKLE_STIPPLES))
        self.refresh()

    def refresh(self):
        cv = self.lv.cellview(0)
        cv.cell = self.top
        self.lv.add_missing_layers()
        self._place_hollow_underlays_first()
        # Use the actual property-list position AFTER moving hollow
        # underlays. KLayout's internal dither offset follows this paint
        # plane, and remains stable when a layer is made invisible.
        for paint_plane, lp in enumerate(self.lv.each_layer()):
            key = (lp.source_layer, lp.source_datatype)
            hexcol = self.colors.get(key)
            if hexcol:
                col = int(hexcol.lstrip("#"), 16)
                lp.fill_color = col
                lp.frame_color = col
            if key in self.hollow:
                lp.dither_pattern = 1  # hollow: outline only
                lp.transparent = False
                lp.width = 1
            elif self.speckle:
                # Non-alpha, single-pass Calibre speckle. Filled pixels use
                # the upper layer's color; common empty pixels preserve a
                # lower polygon's continuous frame as a dotted trace.
                lp.dither_pattern = self._design_speckle_patterns[
                    paint_plane & 1]
                lp.transparent = False
                lp.width = 1
            else:
                lp.dither_pattern = 0  # solid (headless exports)
                lp.transparent = False
                lp.width = 1
        self.lv.max_hier()

    def _place_hollow_underlays_first(self):
        """Order structural outline planes around design geometry.

        LayoutView.add_missing_layers sorts new properties by source
        layer number, regardless of the order in which layers were created
        in Layout. KLayout paints later property entries over earlier
        ones; the wanted order is [gray underlays, design geometry,
        `above` overlays]: gray frame dust stays buried under fill,
        white frames (the `above` set) stay readable on top of it.
        Within each hollow group the REVERSE of the `hollow` argument
        order applies (its first entry topmost). Rebuild the flat
        property list only when it deviates.
        """
        nodes = list(self.lv.each_layer())
        prio = {key: i for i, key in enumerate(self._hollow_order)}

        def key(node):
            return (node.source_layer, node.source_datatype)

        def by_prio(group):
            return sorted(group, key=lambda n: -prio[key(n)])

        under = by_prio(n for n in nodes
                        if key(n) in self.hollow
                        and key(n) not in self._above)
        over = by_prio(n for n in nodes if key(n) in self._above)
        design = [n for n in nodes if key(n) not in self.hollow]
        want = under + design + over
        if [key(n) for n in want] == [key(n) for n in nodes]:
            return

        # Duplicate before clear_layers(): node refs belong to the list
        # that is about to be destroyed. Preserve the relative ordering
        # within the design group.
        ordered = [node.dup() for node in want]
        self.lv.clear_layers()
        for node in ordered:
            self.lv.insert_layer(self.lv.end_layers(), node)

    def set_visible(self, visible):
        """visible: None (all) or iterable of (layer, datatype)."""
        vis = None if visible is None else set(visible)
        for lp in self.lv.each_layer():
            lp.visible = (vis is None
                          or (lp.source_layer, lp.source_datatype) in vis)

    def set_text_visible(self, on):
        """Toggle text drawing (Calibre-style zoom gating of labels:
        an overview must not paint every label into a wall; texts
        appear only once zoomed in). Idempotent."""
        on = bool(on)
        if on == self._text_visible:
            return
        self._text_visible = on
        try:
            self.lv.set_config("text-visible", "true" if on else "false")
            if on:
                self.lv.set_config("text-lazy-rendering", "false")
        except Exception:
            pass

    def set_cell_visibility(self, show, hide):
        """Cell indexes to show/hide; a hidden cell's whole subtree is
        skipped at draw time (this is how size bands beyond the current
        zoom's cut are excluded without touching the layout)."""
        for ci in hide:
            self.lv.hide_cell(ci, 0)
        for ci in show:
            self.lv.show_cell(ci, 0)

    _abstract = False

    def set_abstract(self, on, width=10.0):
        """klayout abstract mode: cells whose on-screen image is below
        `width` px draw as empty frames instead of content - a lossy
        navigation mode (measured 25x on a heavy wide view), toggled
        live from the GUI ('a' key), so it is per-render job state."""
        on = bool(on)
        if on == self._abstract:
            return
        self._abstract = on
        self._config("abstract-mode-width", "%g" % width)
        self._config("abstract-mode-enabled", "true" if on else "false")

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
