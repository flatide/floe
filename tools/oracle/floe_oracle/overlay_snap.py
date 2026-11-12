"""The gates' reference for a snap on what the viewer draws over a frame - a
DRC error's shape (rust/app-core/src/view/overlay.rs, 0.12.454).

A snap is answered from the layout's geometry by the renderer: the nearest
vertex within the reach, else the nearest point of an edge. The segments of
a drawn shape are judged by the same rule around the same whole dbu, and the
two answers are one: a vertex before an edge, then the nearer; of two as
near, the layout's.
"""

WHOLE = 1e-6


def _whole(v):
    """A coordinate this near a whole dbu is that dbu (micrometres over the
    dbu are not whole: 0.7 / 0.001)."""
    v = float(v)
    w = float(round(v))
    return w if abs(v - w) <= WHOLE else v


def _on_grid(q, a, b):
    """An edge point is a whole dbu along its segment: a coordinate both
    ends share is the segment's own, a rounded one stays between the ends."""
    if a == b:
        return a
    return min(max(float(round(q)), min(a, b)), max(a, b))


def drawn(segments, x, y, r):
    """The drawn segments' own answer around the whole dbu (x, y) within r:
    ("vertex" | "edge", px, py) or None."""
    reach = float(r) * float(r)
    vertex = edge = None
    for seg in segments:
        x0, y0, x1, y1 = (_whole(v) for v in seg)
        for px, py in ((x0, y0), (x1, y1)):
            d = (px - x) ** 2 + (py - y) ** 2
            if d <= reach and (vertex is None or d < vertex[0]):
                vertex = (d, px, py)
        vx, vy = x1 - x0, y1 - y0
        length = vx * vx + vy * vy
        if length == 0:
            continue
        along = min(1.0, max(0.0, ((x - x0) * vx + (y - y0) * vy) / length))
        qx, qy = x0 + along * vx, y0 + along * vy
        d = (qx - x) ** 2 + (qy - y) ** 2
        if d <= reach and (edge is None or d < edge[0]):
            edge = (d, _on_grid(qx, x0, x1), _on_grid(qy, y0, y1))
    if vertex is not None:
        return ("vertex", vertex[1], vertex[2])
    if edge is not None:
        return ("edge", edge[1], edge[2])
    return None


def snap(segments, x, y, r, scene=None):
    """The snap's answer as the view channel says it - {"found", "x", "y",
    "snap"} and "overlay": True when it is on a drawn segment. `scene`: the
    layout's own answer in the same form (None or found False: nothing of
    the layout in reach, or the layout not asked)."""
    scene = scene if scene and scene.get("found") else None
    mine = drawn(segments, x, y, r) if segments else None
    if mine is not None and scene is not None:
        kind, px, py = mine
        if kind != scene["snap"]:
            better = kind == "vertex"
        else:
            better = ((px - x) ** 2 + (py - y) ** 2
                      < (scene["x"] - x) ** 2 + (scene["y"] - y) ** 2)
        if not better:
            mine = None
    if mine is not None:
        return {"found": True, "x": mine[1], "y": mine[2], "snap": mine[0],
                "overlay": True}
    if scene is not None:
        return {"found": True, "x": scene["x"], "y": scene["y"],
                "snap": scene["snap"]}
    return {"found": False}
