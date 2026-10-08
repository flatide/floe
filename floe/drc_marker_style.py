"""Screen-space styling for aggregate DRC markers, independent of GTK."""

from functools import lru_cache

import numpy as np


def marker_cell_px(width, height):
    """Initial screen grouping pitch; the worker coarsens dense views."""
    return max(32.0, min(64.0, min(width, height) / 16.0))


def aggregate_radius(count, width, height):
    """Return a bounded pixel radius for an aggregate's population.

    Count bands are 2--9, 10--49, 50--99, 100--999, then decades.
    Their base diameters are 8, 10, 12, 14, 16, ... pixels. Scaling
    with the viewport's shorter dimension keeps these readable on both
    small windows and large displays without growing with layout zoom.
    """
    count = max(2, int(count))
    if count < 10:
        band = 0
    elif count < 50:
        band = 1
    elif count < 100:
        band = 2
    else:
        band, threshold = 3, 1000
        # Even at the minimum viewport scale, band 20 reaches the cap.
        # Bound this loop independently of the input integer's size.
        while count >= threshold and band < 20:
            band += 1
            threshold *= 10
    scale = max(0.75, min(1.5, min(width, height) / 800.0))
    return max(2, int(round(min(18.0, (4 + band) * scale))))


@lru_cache(maxsize=96)
def circle_rgba(radius, color, secondary=None):
    """Return a filled circle sprite as tightly packed RGBA bytes.

    The sprite is ``(2 * radius + 1)`` pixels square. Colors use
    ``0xRRGGBBAA``; an optional secondary color occupies its right half.
    A translucent center preserves layout visibility, while the roughly
    two-pixel outline and one-pixel antialiased edge make the count-sized
    boundary legible. Aggregate radii are bounded by ``aggregate_radius``.
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
    alpha = (72.0 + 136.0 * outline) * coverage

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
