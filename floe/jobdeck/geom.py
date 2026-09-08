"""dbu choice and integer placement - free of the renderer and of KLayout
so placement can be checked against hand calculations.

Formula (docs/JOBDECK.ko.md):
    ratio = ad / source_dbu      mag = sf * ratio
    dx    = jx - cx * ratio      dy  = jy - cy * ratio

floe2 keeps i64 coordinates, so the int32 coarsening the KLayout reference
tool needs never applies here; the deck grid (`choose_dbu`) still exists
because the residual statistics are the fastest check that the grid is
right (all zero = the placement offsets are exact multiples).
"""

from __future__ import annotations

from dataclasses import dataclass

COORD_MAX = 2 ** 62          # i64 with headroom (the renderer's coordinates)


@dataclass
class Placement:
    """One entry of one CHIP at one ROWS position, on the deck grid."""
    chip: str
    idx: int
    row: int
    tc: str
    ly: int
    dt: int
    mag: float
    dx_um: float
    dy_um: float
    ix: int          # dx on the deck dbu grid
    iy: int
    rx_um: float     # residual: dx_um - ix*dbu
    ry_um: float
    bbox_um: tuple
    jx: float        # the ROWS position (x, y), exact from the deck
    jy: float


def choose_dbu(source_dbus, ads, extent_um=None, safety: int = 2,
               coord_max: int = COORD_MAX):
    """dbu = min(source dbus, AD values) / safety, coarsened (doubled) only
    if the extent would not fit `coord_max`. AD belongs in the candidate set
    because jx/jy sit on the AD grid; the safety divisor exists because cx
    is a bbox midpoint and can carry a half unit."""
    cands = [d for d in source_dbus if d and d > 0] + \
            [a for a in ads if a and a > 0]
    if not cands:
        raise ValueError("no dbu candidates (no sources and no AD values)")
    base = min(cands)
    dbu = base / safety
    limit = coord_max * 0.9
    doublings = 0
    if extent_um:
        while extent_um / dbu > limit:
            dbu *= 2
            doublings += 1
            if doublings > 64:
                raise OverflowError(
                    "extent %g um cannot be held in the coordinate range"
                    % extent_um)
    return dbu, {
        "candidates": sorted(set(cands)),
        "base": base,
        "safety": safety,
        "preferred_dbu": base / safety,
        "dbu": dbu,
        "extent_um": extent_um,
        "coord_at_extent": None if not extent_um else int(extent_um / dbu),
        "coord_limit": int(limit),
        "doublings": doublings,
        "why": ("min(source dbu, AD) / safety; AD included because ROWS "
                "positions sit on the AD grid"
                + ("; coarsened %dx to fit the coordinate range"
                   % doublings if doublings else "")),
    }


def to_grid(um: float, dbu: float, coord_max: int = COORD_MAX):
    """(integer dbu value, residual in um)."""
    i = int(round(um / dbu))
    if abs(i) > coord_max:
        raise OverflowError("%g um at dbu %g overflows the coordinate range"
                            % (um, dbu))
    return i, um - i * dbu


def entry_pairs(entry, cross: bool = True) -> list:
    """(LY, DT) pairs an entry contributes. Whether multi-value LY and DT
    cross-product or zip is UNCONFIRMED (no real case seen); cross is
    the default and the flag is kept until one is."""
    ly = entry.ly or []
    dt = entry.dt or [0]
    if cross:
        return [(l, d) for l in ly for d in dt]
    return [(l, dt[i] if i < len(dt) else dt[-1]) for i, l in enumerate(ly)]


def layer_table(deck, cross: bool = True, by_chip: bool = False) -> list:
    """(idx, ly, dt) -> sequential deck layer; (chip, idx, ly, dt) with
    `by_chip` (per-CHIP colouring). Never computed arithmetically from ly
    (ly is user-assigned with no firm bound). Sorted for reproducibility;
    with `by_chip` the CHIP order is the deck's."""
    chip_pos: dict = {}
    for c in deck.chips:
        chip_pos.setdefault(c.id, len(chip_pos))
    seen = set()
    for c in deck.chips:
        for e in c.entries:
            for (l, d) in entry_pairs(e, cross):
                key = (chip_pos[c.id], c.id) if by_chip else (0, "")
                seen.add((key, e.idx, l, d))
    return [{"out": n, "chip": key[1], "idx": i, "ly": l, "dt": d,
             "title": deck.title(i)}
            for n, (key, i, l, d) in enumerate(sorted(seen))]


def out_of(table, deck) -> dict:
    """{(chip, idx, ly, dt): out} for every CHIP, whichever way the table
    was grouped."""
    per_chip = any(r["chip"] for r in table)
    if per_chip:
        return {(r["chip"], r["idx"], r["ly"], r["dt"]): r["out"]
                for r in table}
    shared = {(r["idx"], r["ly"], r["dt"]): r["out"] for r in table}
    return {(c.id, i, l, d): o for c in deck.chips
            for (i, l, d), o in shared.items()}


MISSING_RAISE = "raise"     # a TC without a dbu is a hard error
MISSING_SKIP = "skip"       # its entries are left out and listed

SKIP_MISSING = "missing"            # TC file not found
SKIP_UNREADABLE = "unreadable"      # header broken / reader raised
SKIP_EMPTY_LAYER = "empty_layer"    # (LY, DT) read fine but holds nothing
SKIP_UNKNOWN_FORMAT = "unknown_format"  # no OASIS/GDS header
SKIP_NOT_INDEXED = "not_indexed"    # floe2: no <src>.floe cache yet
SKIP_REASONS = (SKIP_MISSING, SKIP_UNREADABLE, SKIP_EMPTY_LAYER,
                SKIP_UNKNOWN_FORMAT, SKIP_NOT_INDEXED)


def skip_record(chip, idx, tc, line, rows, reason, detail, stage,
                anchors, ly=None, dt=None) -> dict:
    """One ledger record: what the deck asked for that is NOT placed."""
    rec = {"chip": chip, "idx": idx, "tc": tc, "line": line, "rows": rows,
           "reason": reason, "detail": detail, "stage": stage,
           "anchors": [[float(x), float(y)] for x, y in anchors]}
    if ly is not None:
        rec["ly"] = ly
        rec["dt"] = dt
    return rec


def skip_counts(skipped) -> dict:
    out: dict = {}
    for r in skipped:
        out[r["reason"]] = out.get(r["reason"], 0) + 1
    return {k: out[k] for k in SKIP_REASONS if k in out} | \
        {k: v for k, v in out.items() if k not in SKIP_REASONS}


def plan(deck, source_dbu: dict, safety: int = 2, cross: bool = True,
         ids=None, missing: str = MISSING_RAISE, by_chip: bool = False,
         bad: dict | None = None):
    """Every placement plus the statistics a report needs.

    `ids` restricts the returned placements to those identifiers (None =
    all); the dbu choice and the extent still come from the WHOLE deck so
    a selection never moves the grid. `missing` says what to do with an
    entry whose TC has no dbu: raise, or skip it. `bad` is
    {tc: (status, error[, stage])} for the sources without a dbu.

    Two lists come out of an entry that cannot be placed: stats["skipped"]
    holds entries INSIDE the selection (what the image would lack);
    stats["deck_issues"] holds the same records for entries outside it
    (information, never an error for this selection).
    """
    bad = bad or {}
    table = layer_table(deck, cross, by_chip)
    out_lookup = out_of(table, deck)
    want = None if ids is None else {int(i) for i in ids}

    raw = []
    skipped = []
    deck_issues = []
    seen_skip = set()
    for c in deck.chips:
        for row, (jy, jx) in enumerate(c.rows):
            for e in c.entries:
                sdbu = source_dbu.get(e.tc)
                if sdbu is None:
                    entry = tuple(bad.get(e.tc, (SKIP_MISSING, "")))
                    status, err = entry[0], entry[1]
                    stage = entry[2] if len(entry) > 2 else "probe"
                    key = (c.id, e.idx)
                    selected = want is None or e.idx in want
                    if status in SKIP_REASONS:
                        reason = status
                    else:
                        reason = SKIP_MISSING
                    if selected and missing != MISSING_SKIP \
                            and reason in (SKIP_MISSING, SKIP_UNREADABLE):
                        raise KeyError("no source dbu for %r" % e.tc)
                    if key not in seen_skip:
                        seen_skip.add(key)
                        rec = skip_record(
                            c.id, e.idx, e.tc, e.lineno, len(c.rows),
                            reason, err or "source has no dbu", stage,
                            [(x, y) for (y, x) in c.rows])
                        (skipped if selected else deck_issues).append(rec)
                    continue
                mag, dx, dy = e.placement(jy, jx, sdbu)
                bbox = (dx + mag * e.bx, dy + mag * e.by,
                        dx + mag * e.ux, dy + mag * e.uy)
                for (l, d) in entry_pairs(e, cross):
                    raw.append((c.id, e.idx, row, e.tc, l, d, mag, dx, dy,
                                bbox, jx, jy))

    extent = max([abs(v) for r in raw for v in r[9]]
                 + [abs(r[7]) for r in raw] + [abs(r[8]) for r in raw]) \
        if raw else None
    ads = [e.ad for c in deck.chips for e in c.entries]
    dbu, why = choose_dbu(source_dbu.values(), ads, extent, safety)

    total = len(raw)
    if want is not None:
        raw = [r for r in raw if r[1] in want]

    placements = []
    for (cid, idx, row, tc, l, d, mag, dx, dy, bbox, jx, jy) in raw:
        ix, rx = to_grid(dx, dbu)
        iy, ry = to_grid(dy, dbu)
        placements.append(Placement(
            chip=cid, idx=idx, row=row, tc=tc, ly=l, dt=d, mag=mag,
            dx_um=dx, dy_um=dy, ix=ix, iy=iy, rx_um=rx, ry_um=ry,
            bbox_um=bbox, jx=jx, jy=jy))

    res = [abs(p.rx_um) for p in placements] + \
          [abs(p.ry_um) for p in placements]
    xs = [v for p in placements for v in (p.bbox_um[0], p.bbox_um[2])]
    ys = [v for p in placements for v in (p.bbox_um[1], p.bbox_um[3])]
    stats = {
        "dbu": dbu,
        "dbu_choice": why,
        "instances": len(placements),
        "instances_total": total,
        "selection": None if want is None else sorted(want),
        "skipped": skipped,
        "deck_issues": deck_issues,
        "deck_issue_counts": skip_counts(deck_issues),
        "skip_counts": skip_counts(skipped),
        "residual_max_um": max(res) if res else 0.0,
        "residual_nonzero": sum(1 for r in res if r != 0.0),
        "residual_over_half_dbu": sum(1 for r in res if r > dbu / 2),
        "mags": sorted({p.mag for p in placements}),
        "bbox_um": (min(xs), min(ys), max(xs), max(ys)) if xs else None,
        "layer_table": table,
        "layer_table_by_chip": by_chip,
        "out_of": out_lookup,
    }
    return placements, stats
