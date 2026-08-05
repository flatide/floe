"""Read design.ovc (coverage bitplanes) and sample it for a view.

Coverage is a per-layer 8-bit density mipmap over the die (built by
floe-vfs::coverage). The viewer tints it with the LIVE palette and
fills it into the blank areas of the klayout frame, so a fill field
the cut drops still shows as a density block (Calibre-style) instead
of blanking. Display-only; pick/snap/measure use exact pages.
"""

import struct

import numpy as np

_HDR = "<8sId4qIIII"  # magic, ver, dbu, die x4, res_x, res_y, nlv, nl

# density -> visible brightness gamma. Fill areal density is ~1-6%;
# gamma 0.35 lifts 1% to ~19% and 6% to ~35% brightness so sparse
# fill shows as a readable density block instead of near-black.
COV_GAMMA = 0.35


class Coverage:
    def __init__(self, path):
        d = open(path, "rb").read()
        (magic, ver, self.dbu, x0, y0, x1, y1, self.res_x,
         self.res_y, self.n_levels, n_layers) = struct.unpack_from(
             _HDR, d, 0)
        assert magic == b"FLOEOVC1" and ver == 1, "bad ovc"
        self.die = (x0, y0, x1, y1)
        o = struct.calcsize(_HDR)
        layer_keys = []
        for _ in range(n_layers):
            l, dt = struct.unpack_from("<2I", d, o)
            o += 8
            layer_keys.append((l, dt))
        n_entries = struct.unpack_from("<I", d, o)[0]
        o += 4
        body_off = struct.unpack_from("<Q", d, o)[0]
        o += 8
        # layer key -> {level: (w, h, np.uint8[h,w])}
        self.layers = {}
        for _ in range(n_entries):
            lv = d[o]
            o += 1
            li = struct.unpack_from("<I", d, o)[0]
            o += 4
            w, h = struct.unpack_from("<2H", d, o)
            o += 4
            off = struct.unpack_from("<Q", d, o)[0]
            o += 8
            ln = struct.unpack_from("<I", d, o)[0]
            o += 4
            arr = np.frombuffer(d, np.uint8, ln,
                                body_off + off).reshape(h, w)
            self.layers.setdefault(layer_keys[li], {})[lv] = (
                w, h, arr)
        # finest texel size in dbu (level 0)
        self.tex0 = ((x1 - x0) / max(1, self.res_x),
                     (y1 - y0) / max(1, self.res_y))

    def _pick_level(self, planes, tgt_dbu):
        """coarsest level whose texel is still <= the target world
        size per screen pixel (so one texel ~ one pixel or finer)."""
        best = None
        for lv, (w, h, _a) in planes.items():
            tex = (self.die[2] - self.die[0]) / max(1, w)
            if tex <= tgt_dbu or best is None:
                if best is None or tex > best[1]:
                    best = (lv, tex)
        return None if best is None else best[0]

    def view_rgb(self, x0, y0, x1, y1, w, h, visible, colors):
        """Tinted coverage RGB [h,w,3] for the view (dbu bbox), plus a
        bool 'any coverage'. visible: set of (layer,dt) or None (all).
        colors: (layer,dt) -> '#rrggbb'."""
        if x1 <= x0 or y1 <= y0:
            return None, False
        acc = np.zeros((h, w, 3), np.uint16)
        tgt = max((x1 - x0) / w, (y1 - y0) / h)
        # per-output-pixel world coords -> texel index (y flipped:
        # screen row 0 = top = y1, plane row 0 = die_y0 = bottom)
        wx = x0 + (np.arange(w) + 0.5) * (x1 - x0) / w
        wy = y1 - (np.arange(h) + 0.5) * (y1 - y0) / h
        any_cov = False
        for key, planes in self.layers.items():
            if visible is not None and key not in visible:
                continue
            hexcol = colors.get(key)
            if not hexcol:
                continue
            lv = self._pick_level(planes, tgt)
            if lv is None:
                continue
            pw, ph, arr = planes[lv]
            texw = (self.die[2] - self.die[0]) / max(1, pw)
            texh = (self.die[3] - self.die[1]) / max(1, ph)
            cx = np.floor((wx - self.die[0]) / texw).astype(np.int64)
            cy = np.floor((wy - self.die[1]) / texh).astype(np.int64)
            inx = (cx >= 0) & (cx < pw)
            iny = (cy >= 0) & (cy < ph)
            if not inx.any() or not iny.any():
                continue
            cx = np.clip(cx, 0, pw - 1)
            cy = np.clip(cy, 0, ph - 1)
            dens = arr[np.ix_(cy, cx)].astype(np.float32)  # 0..255
            dens[~iny, :] = 0
            dens[:, ~inx] = 0
            if not dens.any():
                continue
            # perceptual boost: real fill is areally sparse (a field
            # of sub-um atoms is only ~1-6% covered), so raw density
            # renders near-black. Gamma-lift so any real coverage
            # reads as a visible density block (Calibre shows presence,
            # not true areal fraction), while empty texels stay 0.
            dens = (np.power(dens / 255.0, COV_GAMMA) * 255.0
                    ).astype(np.uint16)
            c = int(hexcol.lstrip("#"), 16)
            r, g, b = (c >> 16) & 255, (c >> 8) & 255, c & 255
            # tint by density; max-accumulate (painter/screen union)
            np.maximum(acc[..., 0], (dens * r) // 255, out=acc[..., 0])
            np.maximum(acc[..., 1], (dens * g) // 255, out=acc[..., 1])
            np.maximum(acc[..., 2], (dens * b) // 255, out=acc[..., 2])
            any_cov = True
        return (acc.astype(np.uint8) if any_cov else None), any_cov


def composite(frame_png_path, cov_rgb, black=24):
    """Fill the empty pixels of the klayout frame with the coverage
    RGB; returns PNG bytes. cov_rgb: uint8 [h,w,3] or None.

    "Empty" is neighborhood-aware: design fills are a 50% speckle
    (render.py), so half the pixels INSIDE every drawn polygon are
    near-black. A per-pixel darkness test would repaint them with
    density tint - the exact "only fills black pixels" contract
    violation. A pixel counts as background only when no lit pixel
    sits in its 3x3 neighborhood; speckle interiors always have a
    lit 4-neighbor, true background never does (drawn frames just
    grow a 1px halo that stays unfilled - invisible)."""
    from PIL import Image
    import io
    im = Image.open(frame_png_path).convert("RGB")
    fr = np.asarray(im)
    if cov_rgb is None or cov_rgb.shape[:2] != fr.shape[:2]:
        buf = io.BytesIO()
        im.save(buf, "PNG")
        return buf.getvalue()
    lit = fr.max(axis=2) > black
    near = lit.copy()  # separable 3x3 dilation, numpy only
    near[1:, :] |= lit[:-1, :]
    near[:-1, :] |= lit[1:, :]
    grown = near.copy()
    grown[:, 1:] |= near[:, :-1]
    grown[:, :-1] |= near[:, 1:]
    mask = ~grown
    out = fr.copy()
    out[mask] = cov_rgb[mask]
    buf = io.BytesIO()
    Image.fromarray(out, "RGB").save(buf, "PNG")
    return buf.getvalue()
