"""Antialiased circle sprites for DRC error markers, independent of GTK."""

from functools import lru_cache

import numpy as np


@lru_cache(maxsize=96)
def circle_rgba(radius, color, secondary=None, solid=False):
    """Return a filled circle sprite as tightly packed RGBA bytes.

    The sprite is ``(2 * radius + 1)`` pixels square. Colors use
    ``0xRRGGBBAA``; an optional secondary color occupies its right half.
    A translucent center preserves layout visibility, while the roughly
    two-pixel outline and one-pixel antialiased edge keep the circle legible.
    ``solid`` makes error markers opaque inside.
    """
    radius = int(radius)
    if radius < 1:
        raise ValueError("circle radius must be positive")
    size = radius * 2 + 1
    yy, xx = np.ogrid[-radius:radius + 1, -radius:radius + 1]
    distance = np.sqrt(xx * xx + yy * yy)
    coverage = np.clip(radius + 0.5 - distance, 0.0, 1.0)
    stroke = min(2.0, radius / 2.0)
    outline = distance >= radius - stroke
    alpha = (255.0 if solid else 72.0 + 136.0 * outline) * coverage

    def channels(rgba):
        rgba = int(rgba)
        return ((rgba >> 24) & 255, (rgba >> 16) & 255,
                (rgba >> 8) & 255, rgba & 255)

    pixels = np.empty((size, size, 4), dtype=np.uint8)
    pixels[:] = channels(color)
    if secondary is not None:
        pixels[:, radius:] = channels(secondary)
    pixels[:, :, 3] = np.rint(alpha * pixels[:, :, 3] / 255.0).astype(np.uint8)
    pixels[coverage == 0] = 0
    return pixels.tobytes()
