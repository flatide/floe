"""The viewer's perf line as Python wrote it (floe/gui.py `perf_status` and
`occ_note` until P4f, docs/SHARED_APP_LAYER.ko.md §7): the line field
engineers paste is a user contract, and the product's is the Rust port
(app-core view::perf, sent by floe2 gtk-service with each frame). Kept
here, dev-only, as gate perf_parity's reference - byte for byte - with
floe_oracle/rust_render.py's `_emit_frame` (the frame report).
"""

import os

from floe.gui import fmt_count


def occ_note(res, full=False):
    """Pass 2 drawn from the occupancy density (design.ovs; 2026-10-06, by
    default since 0.12.317 - FLOE_RUST_DENSITY_OCC=off the plans): its cell, the layers it held and those this
    frame made, as the status line says them - the log line (`full`) with
    what its cache holds (2026-10-07); "" when the plans drew it."""
    p2 = res.get("density_plan2") or {}
    if not p2.get("occ_layers"):
        return ""
    note = "%g um cells, %d layers" % (p2.get("occ_cell_nm", 0) / 1000.0, p2["occ_layers"])
    if p2.get("occ_made"):
        note += ", %d made" % p2["occ_made"]
    if full and p2.get("occ_cache_kb"):
        note += "; cache %.1f MB" % (p2["occ_cache_kb"] / 1024.0)
    return note


def perf_status(res, depth_note=""):
    """The perf line of a settled (or refining) frame: (full, brief). The
    full line goes to the terminal log and the lower bar's tooltip, every
    diagnostic in it; the brief one is what the lower bar shows - only what
    is checked frame by frame, so it fits without being cut off (user
    2026-10-01: "the log has it all; the bar should show only what is needed
    now, without an ellipsis")."""
    split = ""
    brief_split = ""
    if res.get("load_ms") is not None:
        ph = ""
        if res.get("phase_apply") is not None:
            # load = plan (rust) + delta (author/IPC)
            #        + apply (klayout parse + WC build)
            ph = " [%d plan+%d delta+%d apply]" % (
                res.get("phase_plan", 0),
                res.get("phase_delta", 0),
                res.get("phase_apply", 0))
        # the label plan is not part of load; shown only
        # when it is worth a look (2026-09-21)
        text = res.get("text_plan_ms", 0) or 0
        split = " = %d load%s%s + %d draw" % (
            res["load_ms"], ph,
            " + %d text" % text if text >= 100 else "",
            res["draw_ms"])
        # the budget fit decided for a new scale before its
        # plan (field 2026-10-05: it was part of `other`)
        probe = res.get("fit_probe_ms", 0) or 0
        if probe >= 100:
            split += " + %d fit probe" % round(probe)
        # renderd time no phase covers, and time spent
        # waiting behind earlier commands (queue + pipe)
        if res.get("other_ms", 0) > 200:
            split += " + %d other" % res["other_ms"]
        if res.get("wait_ms", 0) > 200:
            split += " + %d wait" % res["wait_ms"]
        # the bar: the same without the load's phases
        brief_split = split.replace(ph, "", 1) if ph else split
    cut = ""
    brief_cut = ""
    if res.get("cut_um"):
        cut = ", cut<%.3gum" % res["cut_um"]
        brief_cut = "cut<%.3gum" % res["cut_um"]
        # thin keep: each shape by its larger side (0.12.214,
        # the hairlines stay) or, under FLOE_RUST_SHAPE_CUT=min,
        # by its smaller side (0.12.173..0.12.213)
        culls = res.get("plan_culls") or {}
        if culls.get("shape_cut"):
            cut += " (larger side)" if culls.get("shape_cut_max") else " (min side)"
    fit = (res.get("plan_culls") or {})
    if (fit.get("fit_pct") or fit.get("fit_cull") or fit.get("fit_over")
            or fit.get("fit_thin")):
        # budget-fitted cut (0.12.162): the planner raised
        # the cut so the frame fits the decoded budget.
        # Shown HERE, next to the cut, because the bar is
        # ellipsized at its end and the long diagnostics
        # tail hid it (field 2026-09-18)
        factor = max(100, int(fit.get("fit_pct", 0) or 100)) / 100.0
        thin = int(fit.get("fit_thin", 0) or 0)
        full = int(fit.get("fit_full_pct", 0) or 0) / 100.0
        none = int(fit.get("fit_none_pct", 0) or 0) / 100.0
        if fit.get("fit_ranked"):
            # top plane first (pass 1, 2026-10-07: the drawing goes from
            # the top): the layers above the one the budget ends in
            # whole, that layer by its size classes as below, the
            # layers under it left out
            parts = []
            if factor > 1:
                parts.append("x%.3g" % factor)
            whole_n = int(fit.get("fit_layers_whole", 0) or 0)
            if whole_n:
                parts.append("top %d whole" % whole_n)
            edge = fit.get("fit_layer_edge")
            if edge:
                classes = []
                if thin:
                    classes.append("1/%d%s" % (1 << min(thin, 30), " below x%.3g" % full if full else ""))
                if none:
                    classes.append("none below x%.3g" % none)
                parts.append(edge + (" (%s)" % ", ".join(classes) if classes else ""))
            out_n = int(fit.get("fit_layers_out", 0) or 0)
            if out_n:
                parts.append("%d left out" % out_n)
            fitted = " %s to fit budget" % ", ".join(parts)
        elif thin or none:
            # budget-fitted density (0.12.169): size classes
            # largest first - complete from xF up, the class
            # the budget ends in about 1 in 2^k, nothing
            # under xG
            parts = []
            if factor > 1:
                parts.append("x%.3g" % factor)
            if thin:
                parts.append("1/%d%s" % (1 << min(thin, 30), " below x%.3g" % full if full else ""))
            if none:
                parts.append("none below x%.3g" % none)
            fitted = " %s to fit budget" % ", ".join(parts)
        else:
            fitted = " x%.3g to fit budget" % factor
        fitted += "%s%s%s" % (
            ", hairlines culled" if fit.get("fit_cull") else "",
            ", STILL OVER" if fit.get("fit_over") else "",
            # the fit remembered for this scale did not hold
            # this frame: decided anew, the picture may have
            # changed (SPEC-PLANNER 2026-09-27)
            " (refit)" if fit.get("fit_redecided") else "")
        # these layers' pages decode larger than the planner
        # estimates: fitted to the budget over that much
        # (2026-10-05; a frame that passed the budget is
        # planned anew, not failed)
        scale = int(fit.get("fit_scale", 0) or 0)
        if scale > 1000:
            fitted += ", pages x%.3g their estimate" % (scale / 1000.0)
        cut += fitted
        brief_cut += fitted
    drawn = ""
    if res.get("drawn") is not None:
        drawn = ", ~%s drawn" % fmt_count(res["drawn"])
    refin = ""
    if res.get("refining"):
        refin = ", refining %d" % res["refining"]
    text = ""
    if res.get("plan_ms") is not None:
        # "frontier", not "frames": the planner's
        # depth-cut record count. floe2 computes it
        # regardless of the frames toggle (the depth
        # frontier is plan-integral, +2.4ms measured),
        # so it stays non-zero with frames off - that
        # is not geometry being drawn (field question
        # 2026-09-02).
        text += ", plan %.1fms/%s frontier" % (
            res["plan_ms"],
            fmt_count(res.get("frame_rects", 0)))
        if res.get("fit_probe_ms"):
            # "walk": no hierarchy summary to go by (design.ovh,
            # `floe-index hier`) - every cell of the extent read
            text += ", fit probe %.1fms%s" % (
                res["fit_probe_ms"],
                " (walk)" if res.get("fit_probe_walk") else "")
    if res.get("text_plan_ms") is not None:
        text += ", text %.1fms/%s places" % (
            res["text_plan_ms"],
            fmt_count(res.get("text_place_records", 0)))
    if res.get("png_ms") is not None:
        text += ", %s %.1fms/pub %.1fms" % (
            "raw" if res.get("frame_format") == "raw"
            else "png",
            res["png_ms"], res.get("publish_ms", 0.0))
        # NNtiles is CUMULATIVE over the refinement
        # rounds (9 tiles x 5 rounds = 45), not a
        # thread count - raster threads are the Nj
        text += ", rust %dj %dtiles@%spx %sx%s" % (
            res.get("raster_jobs", 0),
            res.get("render_tiles", 0),
            res.get("tile_px", 0),
            res.get("frame_width", 0),
            res.get("frame_height", 0))
    # F2R diagnostics: refinement round count, decode
    # pool shape (sum/max vs wall exposes idle workers
    # and stragglers, idx = record-index build share),
    # slowest raster tile, and traversal visit/prune
    # counts for the 2c work-bin verdict.
    if res.get("rounds", 0) > 1:
        text += ", rounds %d" % res["rounds"]
    if res.get("decode_sum_ms"):
        text += ", dec sum %.0f/max %.0f/idx %.0fms" % (
            res["decode_sum_ms"],
            res.get("decode_max_ms", 0.0),
            res.get("index_ms", 0.0))
    if res.get("raster_tile_max_ms"):
        text += ", tile-max %.0fms" % (
            res["raster_tile_max_ms"])
    if res.get("tiles_reused"):
        # §F2R-16 pan reuse engaged for this frame
        text += ", pan-reuse %d tiles" % (
            res["tiles_reused"])
    if res.get("work_bin_items"):
        text += ", bin %s items" % fmt_count(
            res["work_bin_items"])
        # deferral causes: Nr = repetition edges past
        # the member-product gate, Ns = single
        # placements past the item budget (wNNN = the
        # heaviest such subtree weight) - names the
        # next 2c lever without a diagnostic build
        if res.get("work_bin_defer_rep") or \
                res.get("work_bin_defer_single"):
            text += " (defer %sr+%ss w%s)" % (
                fmt_count(res.get(
                    "work_bin_defer_rep", 0)),
                fmt_count(res.get(
                    "work_bin_defer_single", 0)),
                fmt_count(res.get(
                    "work_bin_defer_wmax", 0)))
    elif res.get("work_bin_overflow_items"):
        # bin hit its item cap and fell back to the
        # per-tile walk (pixels identical, slower)
        text += ", bin off(cap@%s)" % fmt_count(
            res["work_bin_overflow_items"])
    if res.get("member_paints"):
        # geometry member paints - the paint-vs-
        # traversal split for the F2R-03c judgment
        text += ", paints %s" % fmt_count(
            res["member_paints"])
    if res.get("cache_evicted"):
        # decoded-LRU churn: the working set no longer
        # fits FLOE_RUST_BUDGET_MB this session (§3.18)
        text += ", evict %s" % fmt_count(
            res["cache_evicted"])
    if res.get("retained_mb"):
        # §F2R-20: geometry frames renderd holds for
        # pan reuse (bounded by FLOE_RUST_RETAINED_MB)
        text += ", retained %dMB" % round(
            res["retained_mb"])
    if res.get("hier_cells_visited"):
        text += ", hier %s/%s pruned" % (
            fmt_count(res["hier_cells_visited"]),
            fmt_count(res.get("subtrees_pruned", 0)))
    if res.get("once_full_tiles") or res.get("once_items_skipped"):
        # F2R-28 write-once tiles: tiles that filled up (and the
        # passes they skipped), items skipped as fully covered
        text += ", once %s tiles/%s passes/%s items" % (
            fmt_count(res.get("once_full_tiles", 0)),
            fmt_count(res.get("once_passes_skipped", 0)),
            fmt_count(res.get("once_items_skipped", 0)))
    culls = res.get("plan_culls") or {}
    if any(culls.values()):
        # planner verdicts (field 2026-09-10): pages
        # culled by size/hairline, page-BVH nodes,
        # child-BVH nodes pruned, child cells omitted,
        # layer skips, washes, thin frames
        text += (", cut pages %s/pbvh %s/cbvh %s/cells %s"
                 ", layer %s, washed %s, thin %s"
                 % tuple(fmt_count(culls.get(k, 0)) for k in (
                     "pages_size", "page_bvh", "child_bvh",
                     "children_size", "layer", "washed",
                     "thin_frames")))
        if culls.get("thin_pages"):
            # all-thin pages the page hairline rule
            # would have dropped (2026-09-10): their
            # decode / raster cost is what the field
            # measurement of the lifted rule reads
            text += ", thin pages %s kept" % fmt_count(
                culls["thin_pages"])
        if culls.get("sub_cut_washes") or culls.get("sub_cut_sparse"):
            # sub-cut pages/nodes washed as footprints
            # and kept or expanded as sparse (2026-09-16)
            text += ", sub-cut washes %s/sparse %s" % (
                fmt_count(culls.get("sub_cut_washes", 0)),
                fmt_count(culls.get("sub_cut_sparse", 0)))
        if culls.get("sub_cut_boxes") or culls.get("sub_cut_box_over"):
            # sub-cut boxes (0.12.168): what the size cut
            # drops, kept as boxes under thin keep
            text += ", boxes %s%s%s%s" % (
                fmt_count(culls.get("sub_cut_boxes", 0)),
                " x%d coarser" % (1 << culls["sub_cut_box_level"])
                if culls.get("sub_cut_box_level") else "",
                " (+%s over)" % fmt_count(culls["sub_cut_box_over"])
                if culls.get("sub_cut_box_over") else "",
                " (%s unsure)" % fmt_count(culls["sub_cut_box_unsure"])
                if culls.get("sub_cut_box_unsure") else "")
        if culls.get("sub_cut_sparse_over") or culls.get("sub_cut_wash_over"):
            # dropped by the per-plan sub-cut budgets
            # (sparse ink / wash area): the frame is
            # showing less than the rules would
            text += ", sub-cut over %s/%s" % (
                fmt_count(culls.get("sub_cut_sparse_over", 0)),
                fmt_count(culls.get("sub_cut_wash_over", 0)))
        if (culls.get("rep_kept") or culls.get("rep_washed")
                or culls.get("rep_children")):
            # the page frontier (2026-09-17): cut pages
            # kept (drawn) and cut placements expanded
            # with thinned members - one in 4^k
            # (rep_washed stays 0: representatives are
            # never washed since the field's boxes)
            text += ", reps %s pages/%s children" % (
                fmt_count(culls.get("rep_kept", 0)),
                fmt_count(culls.get("rep_children", 0)))
            if culls.get("rep_level"):
                # the item budget's level: one cut item
                # in 2^L
                text += " L%d" % culls["rep_level"]
            if culls.get("rep_page_level"):
                # the decode budget thinned the pages
                # themselves (one in 2^P by index)
                text += " P%d" % culls["rep_page_level"]
    if culls.get("stored_rep_points") or culls.get("stored_rep_limited"):
        text += ", stored reps %s/tested %s%s" % (
            fmt_count(culls.get("stored_rep_points", 0)),
            fmt_count(culls.get("stored_rep_tested", 0)),
            " (capped)" if culls.get("stored_rep_limited") else "")
    summ = res.get("summary") or {}
    if summ.get("layers"):
        # occupancy summary (M2): these layers were
        # drawn from design.ovo, not their pages -
        # pick/snap do not see them in this view
        text += (", summary %d layers %s cells (level %d,"
                 " %g um; not pickable)" % (
                     summ["layers"], fmt_count(summ["cells"]),
                     summ["level"], summ["cell_um"]))
    elif summ.get("none") not in (None, "-", "policy",
                                  "exact"):
        # since 2026-09-18 the summary serves cull too,
        # so its absence is worth a word under either
        text += ", summary: none (%s)" % summ["none"]
    if res.get("labels_truncated"):
        text += ", labels partial"
    if res.get("over_budget_pages"):
        text += ", %d pages over budget (not drawn)" % (
            res["over_budget_pages"])
    if res.get("deck"):
        d = res["deck"]
        # raster/frame ms are SUMS over passes; "wall"
        # is the batches' real elapsed time, and the
        # pass parallelism (x tile workers) beside it
        # (review 2026-09-09 (5th))
        text += (", deck %d passes (%d frame, %d skipped, "
                 "%d scene reuses) "
                 "%d/%d pages, scene %d + frame sum %d + "
                 "composite %d ms, raster wall %d ms "
                 "%dp x %dt, %d batches, pass max %dMB, "
                 "batch max %dMB" % (
                     d["passes"], d["frame_passes"],
                     d["passes_skipped"],
                     d.get("scene_reuses", 0),
                     d["unique_pages"],
                     d["pages_summed"],
                     round(d["scene_us"] / 1000),
                     round(d["frame_raster_us"] / 1000),
                     round(d["composite_us"] / 1000),
                     round(d.get("raster_wall_us", 0)
                           / 1000),
                     d.get("pass_workers", 0),
                     res.get("workers", 0),
                     d.get("batches", 0),
                     round(d["pass_bytes_max"] / 1e6),
                     round(d.get("batch_bytes_max", 0)
                           / 1e6)))
        if d.get("streamed_passes"):
            text += ", %d streamed in %d slices" % (
                d["streamed_passes"], d["slices"])
        if d.get("wide_washes"):
            text += ", %s sub-cut washes" % fmt_count(
                d["wide_washes"])
        if d.get("summary_passes"):
            # passes drawn from their source's design.ovo
            # (M4); pick/snap do not see those layers
            text += ", summary %d passes %s cells (not pickable)" % (
                d["summary_passes"],
                fmt_count(d.get("summary_cells", 0)))
        if d.get("summary_none_passes"):
            text += ", %d passes without summary" % (
                d["summary_none_passes"])
    # tiles = plan total (resident pages included);
    # +new = pages actually shipped for this view
    # (cache misses, summed over its stream rounds)
    # the density stack (diagnostic FLOE_RUST_DENSITY_STACK=top,
    # CUT_DENSITY_DESIGN §10.10): the frame stacked its density.
    # First in the line - the bar, which showed this line until
    # 2026-10-01, is ellipsized at its end, and next to the cut
    # it fell off (field 2026-09-26)
    stack = ""
    if res.get("density_stack") is not None:
        # with the sub-cut dots: their block, the records'
        # floor pass 2 planned at, its plan time and the pages
        # it decoded (to compare FLOE_RUST_DENSITY_BLOCK_PX,
        # 2026-10-01, and FLOE_RUST_DENSITY_FLOOR_PX, 2026-09-30)
        parts = ["dots" if res.get("density_dots") is not None
                 else "top + empty"]
        # pass 1's shapes passed by, the density alone (diagnostic
        # FLOE_RUST_DENSITY_ONLY=on, 2026-10-04; renderd shares the env)
        if os.environ.get("FLOE_RUST_DENSITY_ONLY") == "on":
            parts.append("density only")
        # what pass 2 lit: the dots standing for the cells under the cut
        # and the shapes under the cut it draws from pages by their area -
        # both look like dots on screen, only the cells' count as dot items
        # (user 2026-10-01: "two draws and dots, yet dot items 0")
        lit = "lit %s px" % fmt_count(res["density_stack"].get("lit", 0))
        parts.append(lit)
        if res.get("density_block") is not None:
            parts.append("block %g px" % res["density_block"])
        if res.get("density_floor") is not None:
            parts.append("floor %.2g px" % res["density_floor"])
        # pass 2's reserve: the fixed one or what pass 1 left (2026-10-02)
        if (res.get("density_plan2") or {}).get("reserve_mb"):
            parts.append("reserve %s MB" % fmt_count(res["density_plan2"]["reserve_mb"]))
        # The backend reports the display mode; both use the covered-area
        # gain, while only the legacy mode varies pixel brightness.
        bright = (res.get("density_plan2") or {}).get("bright_milli")
        if bright:
            pattern = (res.get("density_plan2") or {}).get("pattern", 0)
            parts.append(("pattern, cover x%g" if pattern else "bright x%g") % (bright / 1000.0))
            # what a cell under the cut stands for (2026-10-05): the area its
            # shapes cover, or - an index without design.ovb or a hierarchy
            # summary - its whole box; nothing when the occupancy density drew
            # pass 2 (its frames reported `cells by box`, user 2026-10-06)
            if not occ_note(res):
                parts.append("cell cover" if (res.get("density_plan2") or {}).get("cell_cover") else "cells by box")
        # zoomed out past the fit view the dots thin (2026-10-04): their gain
        gain = (res.get("density_plan2") or {}).get("dot_gain_milli")
        if gain is not None and 0 < gain < 1000:
            parts.append("dots x%.2f" % (gain / 1000.0))
        # a dot block too sparse for the detail is left out (2026-10-04):
        # the dots it needs of its pixels, the blocks left out
        gate = (res.get("density_plan2") or {}).get("dot_gate_min") or 0
        if gate > 1:
            block = res.get("density_block") or 4
            gated = res["density_plan2"].get("dot_gated") or 0
            parts.append("gate %d/%d px" % (gate, round(block * block))
                         + (" (%s out)" % fmt_count(gated) if gated else ""))
        us = res.get("density_us") or {}
        # pass 2 drawn from the occupancy density instead of the plans
        # (2026-10-06; FLOE_RUST_DENSITY_OCC=off: the plans): the time it took, its
        # cell and layers in place of the plans' breakdown
        occ = occ_note(res)
        if us and occ:
            parts.append("pass 2 by occupancy %d ms (%s)" % (round(us.get("plan2_us", 0) / 1000), occ_note(res, full=True)))
        elif us:
            plan = "pass 2 plan %d ms" % round(
                us.get("plan2_us", 0) / 1000)
            # where it went (diagnostic, 2026-10-01): the
            # floor probes, the fitted plans and their passes,
            # the regions, the final plans' nodes
            p2 = res.get("density_plan2") or {}
            if p2:
                # where the cells' dot items came from (2026-10-02, the
                # field's `cell dots 87.2M`): the ones there are, a point
                # list's chunk counted at once one item, and the dot block
                # updates past the cells' grids
                # (the chunks' members right after them: 0.12.268 put them
                # last, after the array members - user 2026-10-02)
                # (a page's dots placed by its occupancy grid, design.ovb, say
                # so after the pages: whether the cache has one, 2026-10-02)
                # (the point-list chunks passed over in full blocks and those
                # read at a step right after the chunks, with their members:
                # 2026-10-03)
                held = {"by_list_chunks": "by_chunk_members", "full_chunks": "full_members",
                        "sampled_chunks": "sampled_members"}
                by = ", ".join("%s %s%s" % (name, fmt_count(p2[key]), " of %s members" % fmt_count(
                    p2.get(held[key], 0)) if key in held else " (%s by occupancy)" % fmt_count(
                    p2["occ_pages"]) if key == "by_pages" and p2.get("occ_pages") else "") for key, name in (
                    ("by_nodes", "nodes"), ("by_placements", "placements"), ("by_arrays", "arrays"),
                    ("by_list_members", "list members"), ("by_list_chunks", "list chunks"),
                    ("full_chunks", "list chunks in full blocks"), ("sampled_chunks", "list chunks sampled"),
                    ("by_array_members", "array members"), ("by_pages", "pages"),
                    # pages under the floor decoded, their occupancy cells too
                    # coarse on screen for dots (2026-10-03)
                    ("occ_decoded", "pages decoded under the floor")) if p2.get(key))
                if p2.get("map_updates"):
                    by += "; hash map %s" % fmt_count(p2["map_updates"])
                if p2.get("stages"):
                    by += "; %d layer stages" % p2["stages"]
                if p2.get("mask_tests") or p2.get("mask_fallbacks"):
                    by += "; mask %s/%s pruned, %s fallback" % (
                        fmt_count(p2.get("mask_pruned", 0)), fmt_count(p2.get("mask_tests", 0)),
                        fmt_count(p2.get("mask_fallbacks", 0)))
                by = by.lstrip("; ")
                plan += (" (probe %d ms x%d, fit %d ms x%d"
                         " passes on %d threads, %d regions%s,"
                         " nodes %s, page nodes %s, pages %s,"
                         " reads %s, cell dots %s%s)") % (
                    round(p2["probe_us"] / 1000),
                    p2["probes"],
                    round(p2["fit_us"] / 1000), p2["passes"],
                    max(1, p2.get("threads", 1)),
                    # the free pixels of the cells planned (2026-10-03)
                    p2["regions"], " (free top %s, others %s px)" % (
                        fmt_count(p2["free_top"]), fmt_count(p2["free_others"]))
                    if p2.get("free_top") or p2.get("free_others") else "",
                    fmt_count(p2["nodes"]),
                    fmt_count(p2["page_nodes"]),
                    fmt_count(p2["page_candidates"]),
                    fmt_count(p2.get("reads", 0)),
                    fmt_count(p2.get("items", 0)),
                    " [%s]" % by if by else "")
            parts.append(plan)
        pages = res.get("density_pages") or {}
        if pages:
            parts.append("%d pages" % pages.get("decoded", 0))
        # what pass 2's reserve kept out (2026-10-01): a floor probe past it,
        # a plan its fit thinned, pages its decode left out
        p2 = res.get("density_plan2") or {}
        over = [what for what, there in (
            ("floor probe", p2.get("probes_over")), ("thinned", p2.get("thinned")),
            ("%d pages left out" % pages.get("over_budget", 0), pages.get("over_budget")),
            # what the budget left out, drawn by its occupancy records
            # instead (2026-10-05)
            ("%d pages by occupancy instead" % p2.get("stood_in", 0), p2.get("stood_in"))) if there]
        if over:
            parts.append("pass 2 over budget: %s" % ", ".join(over))
        # pass 2's decode and raster wall (2026-10-03, the field's 449-layer
        # view took 35 s): the log line only
        if us.get("decode2_us") is not None:
            parts.append("pass 2 decode %d ms%s" % (round(us["decode2_us"] / 1000), ", raster %d ms" % round(us["raster2_us"] / 1000)
                                                     if us.get("raster2_us") is not None else ""))
        stack = " [density: %s]" % ", ".join(parts)
        # the bar: what pass 2 lit, its plan time and what it walked (nodes,
        # the placements it read, the cells' dot items it made); a floor
        # probe, a budget fit past one pass (with the floor it raised),
        # threads and decoded pages only when there are any; the block and
        # the regions stay in the log line
        brief = [] if res.get("density_dots") is not None else ["top + empty"]
        brief.append(lit)
        if us and occ:
            brief.append("pass 2 by occupancy %d ms (%s)" % (round(us.get("plan2_us", 0) / 1000), occ))
        elif us:
            plan = "pass 2 plan %d ms" % round(us.get("plan2_us", 0) / 1000)
            if p2:
                inner = []
                if p2.get("probes"):
                    inner.append("probe %d ms x%d" % (round(p2["probe_us"] / 1000), p2["probes"]))
                if p2.get("passes", 0) > 1:
                    inner.append("%d passes%s" % (
                        p2["passes"], ", floor %.2g px" % res["density_floor"]
                        if res.get("density_floor") is not None else ""))
                if p2.get("threads", 1) > 1:
                    inner.append("%d threads" % p2["threads"])
                inner.append("nodes %s, reads %s, cell dots %s" % (
                    fmt_count(p2["nodes"]), fmt_count(p2.get("reads", 0)),
                    fmt_count(p2.get("items", 0))))
                plan += " (%s)" % ", ".join(inner)
            brief.append(plan)
        if pages.get("decoded"):
            brief.append("%d pages decoded" % pages["decoded"])
        if over:
            brief.append("pass 2 over budget: %s" % ", ".join(over))
        brief_stack = "density: %s" % ", ".join(brief) if brief else "density"
    mode = "live%s (%d tiles, +%d new, %d ms" \
           "%s%s%s%s%s%s)" \
        % (stack, res["tiles"], res.get("new", 0) or 0,
           res["ms"], split,
           depth_note, cut, drawn,
           refin, text)
    # The lower bar: the frame's time and where it went, the density's
    # pass 2, the work bin (with the hierarchy walk it falls back to when
    # it is off), the cut and its budget fit, and what the picture lacks.
    # The depth sits in the bar above; the rest is in the log line.
    brief = ["%d ms%s" % (res["ms"], brief_split)]
    if res.get("deck"):
        brief.append("deck %d passes%s" % (
            res["deck"]["passes"],
            ", summary %d passes (not pickable)" % res["deck"]["summary_passes"]
            if res["deck"].get("summary_passes") else ""))
    if stack:
        brief.append(brief_stack)
    if res.get("work_bin_items"):
        brief.append("bin %s items" % fmt_count(res["work_bin_items"]))
    elif res.get("work_bin_overflow_items"):
        brief.append("bin off(cap@%s)%s" % (
            fmt_count(res["work_bin_overflow_items"]),
            ", hier %s/%s pruned" % (
                fmt_count(res["hier_cells_visited"]),
                fmt_count(res.get("subtrees_pruned", 0)))
            if res.get("hier_cells_visited") else ""))
    if brief_cut.strip():
        brief.append(brief_cut.strip())
    if res.get("over_budget_pages"):
        brief.append("%d pages over budget (not drawn)" % res["over_budget_pages"])
    if res.get("labels_truncated"):
        brief.append("labels partial")
    if res.get("cache_evicted"):
        brief.append("evict %s" % fmt_count(res["cache_evicted"]))
    if summ.get("layers"):
        brief.append("summary %d layers (not pickable)" % summ["layers"])
    return mode, " · ".join(brief)
