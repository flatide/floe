"""Deck -> placement plan + report: the M1 deliverable.

`plan_deck` ties parser, catalog, geometry and colour together without
touching the renderer. Its output (placements with mag/dx/dy on the deck
grid, a colour per placement, the skip ledger) is exactly what the M2
composite scene will consume, so the numbers are gated here first.
"""

from __future__ import annotations

import json
import os
from dataclasses import asdict

from .color import ColorScheme, MODE_IDENTIFIER, normalize_mode, order_text
from .geom import MISSING_RAISE, plan
from .parser import parse_jobdeck
from .sources import SourceCatalog


def plan_deck(deck_path: str, sources_dir=None, ids=None,
              mode: str = MODE_IDENTIFIER, missing: str = MISSING_RAISE,
              scheme: ColorScheme | None = None, cross: bool = True,
              safety: int = 2, strict: bool = True, load_ids=None):
    """Parse, probe, place and colour one deck.

    Returns (deck, catalog, placements, stats, scheme, colormap).
    `sources_dir` defaults to the deck's directory (TC paths are
    relative to it). `ids` is the ANALYSIS selection (`floe2 jobdeck
    --level`): it restricts the placements and, in chip mode, splices
    the selected levels into the colour order (section 2). `load_ids`
    is the LOAD selection (the viewer, `floe2 view/index/render
    --level`): it restricts the placements, the rows and the sources
    probed in full, and moves NO colour - review 2026-09-10 (9th)
    P2-4: a CHIP loaded with one level kept its full-deck colour
    only in the level view. The grid never moves with either: the
    whole deck's extent and every source's dbu (header only for the
    sources a load selection leaves out) decide it.
    """
    mode = normalize_mode(mode)
    deck = parse_jobdeck(deck_path, strict=strict)
    if sources_dir is None:
        sources_dir = os.path.dirname(os.path.abspath(deck_path)) or "."
    catalog = SourceCatalog(sources_dir)
    if load_ids is not None:
        load_ids = sorted({int(i) for i in load_ids}) or None
    catalog.probe_all(deck.sources(load_ids))
    dbus = dict(catalog.dbus())
    if load_ids is not None:
        # the grid contract: every source's dbu, from the header alone
        # for the sources the load leaves out (no cache-state lookup,
        # not registered as probed)
        for tc in deck.sources():
            if tc not in catalog.infos:
                dbu = catalog.header_dbu(tc)
                if dbu:
                    dbus[tc] = dbu
    if scheme is None:
        scheme = ColorScheme(mode=mode, cross_ly_dt=cross)
    else:
        scheme.mode = mode
    by_chip = scheme.mode == "chip"
    placements, stats = plan(deck, dbus, safety=safety,
                             cross=scheme.cross_ly_dt,
                             ids=load_ids if load_ids is not None else ids,
                             missing=missing, by_chip=by_chip,
                             bad=catalog.bad())
    colormap = scheme.build(deck, ids)
    stats["colors"] = {
        "mode": scheme.mode,
        "order": scheme.order_table(deck, ids),
        "map": {_ckey(k): v for k, v in colormap.items()},
    }
    stats["sources"] = catalog.report()
    return deck, catalog, placements, stats, scheme, colormap


def _ckey(k) -> str:
    if isinstance(k, tuple):
        return "%d/%d" % k
    return str(k)


def report_dict(deck, placements, stats) -> dict:
    """JSON-ready report: the deck's own report, the plan statistics, and
    every placement (mag/dx/dy in um and on the deck grid)."""
    st = dict(stats)
    st.pop("out_of", None)
    return {
        "deck": deck.report(),
        "plan": st,
        "placements": [asdict(p) for p in placements],
    }


def write_report(path: str, deck, placements, stats) -> None:
    with open(path, "w") as fh:
        json.dump(report_dict(deck, placements, stats), fh, indent=1,
                  default=_json_default)


def _json_default(o):
    if isinstance(o, (set, frozenset)):
        return sorted(o)
    if isinstance(o, tuple):
        return list(o)
    return str(o)


def deck_summary(deck, catalog, placements, stats, scheme) -> list[str]:
    """Human-readable summary lines (the `floe2 jobdeck` output)."""
    src = stats["sources"]
    cov = deck.coverage()
    lines = [
        "deck      : %s%s" % (deck.path,
                              "  (%s)" % deck.jb_name if deck.jb_name else ""),
        "chips     : %d  levels %s  (%d complete, %d partial)"
        % (len(deck.chips), deck.levels(), cov["complete"],
           cov["partial"]),
        "sources   : %d probed, %d ok, %d indexed (.floe)  dir %s"
        % (src["probed"], src["ok"], src["indexed"], catalog.dir),
        "instances : %d placed%s" % (
            stats["instances"],
            "" if stats["selection"] is None else
            " of %d (selection %s)" % (stats["instances_total"],
                                        stats["selection"])),
        "grid      : dbu %g um (%s)" % (stats["dbu"], stats["dbu_choice"]["why"]),
        "residual  : max %g um, %d non-zero, %d over half a dbu"
        % (stats["residual_max_um"], stats["residual_nonzero"],
           stats["residual_over_half_dbu"]),
        "mag       : %s" % stats["mags"],
        "bbox um   : %s" % (
            "none" if stats["bbox_um"] is None else
            "%.4f %.4f %.4f %.4f" % tuple(stats["bbox_um"])),
        "view      : %s -> %s" % (
            {"level": "level view (by mask level)",
             "chip": "chip view (by CHIP block)",
             "layer": "source layer view (LY/DT)"}.get(scheme.mode,
                                                        scheme.mode),
            order_text(stats["colors"]["order"])),
    ]
    for info in src["files"]:
        if info["status"] != "ok":
            lines.append("source    : %s %s (%s)" % (
                info["tc"], info["status"].upper(), info["error"]))
    for rec in stats["skipped"]:
        lines.append("skipped   : CHIP %s $%d %s: %s (%s, %d row(s))" % (
            rec["chip"], rec["idx"], rec["tc"], rec["reason"],
            rec["detail"], rec["rows"]))
    for rec in stats["deck_issues"]:
        lines.append("outside   : CHIP %s $%d %s: %s (not selected)" % (
            rec["chip"], rec["idx"], rec["tc"], rec["reason"]))
    for ln, msg in deck.warnings:
        lines.append("warning   : line %d: %s" % (ln, msg))
    ex = deck.extras()
    if ex:
        lines.append("unknown $ : %s" % ", ".join(
            "%s (%d)" % (k, len(v)) for k, v in sorted(ex.items())))
    if deck.unknown:
        lines.append("unparsed  : %d line(s), first: %d: %s" % (
            len(deck.unknown), deck.unknown[0][0], deck.unknown[0][1]))
    return lines


def placement_lines(placements, scheme, colormap) -> list[str]:
    out = ["%-8s %-4s %-4s %-24s %-8s %-9s %16s %16s %8s" % (
        "chip", "idx", "row", "tc", "ly/dt", "mag", "dx_um", "dy_um",
        "colour")]
    for p in placements:
        out.append("%-8s $%-3d %-4d %-24s %-8s %-9g %16.4f %16.4f %8s" % (
            p.chip, p.idx, p.row, p.tc, "%d/%d" % (p.ly, p.dt), p.mag,
            p.dx_um, p.dy_um, scheme.color_of(colormap, p)))
    return out
