//! The desktop viewer's frame report and perf line (step P4b of moving the GTK
//! render loop onto the shared `ViewController`, docs/SHARED_APP_LAYER.ko.md §7).
//!
//! [`FrameReport`] is floe_oracle/rust_render.py's `_emit_frame` ported: it adds a
//! generation's refinement rounds - renderd's `frame` lines, which the
//! controller hands over as `floe_worker_client::Frame::fields` - into the
//! result the viewer reads, a JSON object with the Python dict's keys, value
//! types and values. [`perf_status`], [`fmt_count`], [`occ_note`] and
//! [`load_note`] are the viewer's Python ones (floe_oracle/perf_line.py since
//! P4f - floe/gui.py's before -, floe/gui.py `fmt_count` and
//! `Viewer._load_note`), over that object. The perf line is what
//! field engineers paste, a user contract: these give Python's strings byte for
//! byte, Python's arithmetic and formatting included - `round()` half to even,
//! int / int division correctly rounded, `%d` truncating a float, `%g`, `str()`
//! of a float (its shortest repr), `max` keeping the first of equals.
//! tools/validate_perf_parity.py (gate `perf_parity`) holds both to the Python
//! on synthetic results, on real renders recorded through the GTK adapter
//! (FLOE_RUST_RECORD) and on fuzzed frame lines.
//!
//! Where it differs, renderd never goes: a non-finite float becomes JSON null
//! (`density_floor=nan`), a result integer past u64 a float, a wire integer
//! past 2^96 or in other than ASCII digits is not read (Python's `int()` takes
//! any size and any Unicode digit), and what Python's `perf_status` refuses
//! (a TypeError on a value of the wrong type, `1 << N` past 4300 digits) still
//! gives a line. An earlier round's Python result also changes its
//! `place_walks` when later rounds come (the lists are the generation's); the
//! Rust result is a copy, the Python one as it was emitted.
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// `density_stack=` (`lit/top/lower/covered/claimed`, CUT_DENSITY_DESIGN §10.10).
pub const DENSITY_STACK_COUNTS: [&str; 5] = ["lit", "top", "lower", "covered", "claimed"];
/// `density_pages=` (pass 2's pages).
pub const DENSITY_PAGE_COUNTS: [&str; 4] = ["planned", "in_hand", "decoded", "over_budget"];
/// `density_us=` (pass 2's times).
pub const DENSITY_TIMES: [&str; 6] = [
    "plan2_us",
    "scene2_us",
    "collect_us",
    "regions_us",
    "decode2_us",
    "raster2_us",
];
/// `density_bin=`.
pub const DENSITY_BIN: [&str; 3] = ["items", "deferred", "overflow"];
/// `density_dots=` (the sub-cut dots: items planned, items past the cap).
pub const DENSITY_DOTS: [&str; 2] = ["items", "over"];
/// `density_plan2=` (pass 2's plans; floe_oracle/rust_render.py DENSITY_PLAN2 says
/// what each is). Older renderers send the first 46, 44, 40 or 39.
pub const DENSITY_PLAN2: [&str; 48] = [
    "probe_us",
    "fit_us",
    "probes",
    "passes",
    "regions",
    "nodes",
    "page_nodes",
    "page_candidates",
    "threads",
    "reads",
    "items",
    "probes_over",
    "thinned",
    "by_nodes",
    "by_placements",
    "by_arrays",
    "by_list_members",
    "by_list_chunks",
    "by_chunk_members",
    "by_array_members",
    "by_pages",
    "map_updates",
    "reserve_mb",
    "occ_pages",
    "occ_decoded",
    "full_chunks",
    "full_members",
    "sampled_chunks",
    "sampled_members",
    "free_top",
    "free_others",
    "dot_gain_milli",
    "dot_gated",
    "dot_gate_min",
    "bright_milli",
    "stood_in",
    "cell_cover",
    "cover_cells",
    "node_sampled",
    "pattern",
    "mask_tests",
    "mask_pruned",
    "mask_fallbacks",
    "stages",
    "occ_layers",
    "occ_cell_nm",
    "occ_made",
    "occ_cache_kb",
];
const DENSITY_PLAN2_SIZES: [usize; 5] = [48, 46, 44, 40, 39];

// ---- the frame report ---------------------------------------------------

/// What `_emit_frame` reads of the viewer's render job (`state["job"]`).
#[derive(Clone, Debug, PartialEq)]
pub struct PerfJob {
    /// The job's `bbox` as the viewer gave it; the result's `bbox` is this
    /// value (a list of four numbers).
    pub bbox: Value,
    /// `int(job["w"])`, `int(job["h"])`: the frame's pixels.
    pub w: i64,
    pub h: i64,
    /// `job.get("scope", "live")`.
    pub scope: Value,
    /// `bool(job.get("bg"))`: a margin frame.
    pub bg: bool,
    /// `float(job.get("cut_px") or 0.0)`: the size cut asked for, px.
    pub cut_px: f64,
}

impl PerfJob {
    /// A live viewport frame of `bbox` (dbu) at `w` x `h` px, no cut.
    pub fn new(bbox: [f64; 4], w: u32, h: u32) -> Self {
        Self {
            bbox: Value::Array(bbox.iter().map(|&v| float_value(v)).collect()),
            w: i64::from(w),
            h: i64::from(h),
            scope: Value::from("live"),
            bg: false,
            cut_px: 0.0,
        }
    }

    /// The GTK adapter's job dict (a recorded one: FLOE_RUST_RECORD), read as
    /// `_emit_frame` reads it.
    pub fn from_python_job(job: &Value) -> Self {
        let j = Py::of(job);
        Self {
            bbox: job.get("bbox").cloned().unwrap_or(Value::Null),
            w: saturate_i64(int_of(j.get("w"))),
            h: saturate_i64(int_of(j.get("h"))),
            scope: job
                .get("scope")
                .cloned()
                .unwrap_or_else(|| Value::from("live")),
            bg: j.get("bg").truthy(),
            cut_px: float_of(j.get("cut_px").or(Py::Float(0.0))),
        }
    }
}

/// What `_emit_frame` reads of the adapter itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerfAdapter {
    /// The raster threads each frame is asked for (`jobs=`, FLOE_RUST_RASTER_JOBS).
    pub raster_jobs: i64,
    /// The file top's height renderd's `opened` reported (`max_depth=`);
    /// None from a renderd without it.
    pub max_depth: Option<i64>,
    /// The cache's dbu (um); the result's `cut_um` needs it.
    pub dbu: Option<f64>,
}

/// What the client measured of one round.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PerfTiming {
    /// Reading the frame's file, us (`_emit_frame`'s `adapter_read_us`).
    pub adapter_read_us: i64,
    /// From the render's submission to this round's emission, ms.
    pub elapsed_ms: f64,
}

/// Whether a round ends its generation: a probe answers once, a frame says
/// `final=1`.
pub fn is_final(fields: &BTreeMap<String, String>, probe: bool) -> bool {
    probe || wire_int(fields, "final", 0) != 0
}

#[derive(Clone, Debug, Default)]
struct Sums {
    new: i128,
    read_us: i128,
    decode_us: i128,
    scene_us: i128,
    draw_us: i128,
    png_us: i128,
    plan_us: i128,
    fit_probe_us: i128,
    fit_probe_walk: i128,
    publish_write_us: i128,
    publish_sync_us: i128,
    publish_rename_us: i128,
    adapter_read_us: i128,
    cache_hit: i128,
    cache_evicted: i128,
    render_tiles: i128,
    frame_cache_hit: i128,
    retained_bytes: i128,
    resident_bytes: i128,
    decode_workers: i128,
    rounds: i128,
    decode_sum_us: i128,
    decode_max_us: i128,
    index_us: i128,
    raster_tile_max_us: i128,
    tiles_reused: i128,
    mask_bytes: i128,
    bin_items: i128,
    bin_overflow: i128,
    defer_rep: i128,
    defer_single: i128,
    defer_wmax: i128,
    member_paints: i128,
    rep_tested: i128,
    rep_drawn: i128,
    hier_cells: i128,
    subtree_prunes: i128,
    once_tiles: i128,
    once_passes: i128,
    once_items: i128,
    /// {outcome: [walks, members]} in first-seen order
    place_walks: Vec<(String, [i128; 2])>,
    density_stack: Option<Value>,
    density_pages: Option<Value>,
    density_us: Option<Value>,
    density_bin: Option<Value>,
    density_dots: Option<Value>,
    density_plan2: Option<Value>,
    density_floor: Option<f64>,
    density_block: Option<f64>,
    queue_us: i128,
    wall_us: i128,
    text_plan_us: i128,
}

/// One generation's rounds added up (`_submit_render`'s state, `_emit_frame`'s
/// sums): a new report per submitted render, one [`FrameReport::round`] per
/// frame line of it, dropped after the [`is_final`] one.
#[derive(Clone, Debug)]
pub struct FrameReport {
    job: PerfJob,
    s: Sums,
}

impl FrameReport {
    pub fn new(job: PerfJob) -> Self {
        Self {
            job,
            s: Sums::default(),
        }
    }

    pub fn job(&self) -> &PerfJob {
        &self.job
    }

    /// [`FrameReport::round`] of a frame the worker client delivered (its
    /// `fields` lack the line's `png` path, which the report never reads).
    pub fn frame(
        &mut self,
        frame: &floe_worker_client::Frame,
        adapter: &PerfAdapter,
        timing: PerfTiming,
    ) -> Value {
        self.round(&frame.fields.0, false, adapter, timing)
    }

    /// One round (`fields`: the `frame` / `probe_frame` line's key=value
    /// tokens): the result `_emit_frame` puts on the queue for it, the pixel
    /// bytes left out.
    pub fn round(
        &mut self,
        fields: &BTreeMap<String, String>,
        probe: bool,
        adapter: &PerfAdapter,
        timing: PerfTiming,
    ) -> Value {
        let wi = |name: &str| wire_int(fields, name, 0);
        let generation = wire_int(fields, "gen", -1);
        let frame_format = fields.get("format").map_or("png", String::as_str);
        let adapter_read_us = i128::from(timing.adapter_read_us);
        let fin = is_final(fields, probe);
        let refining = if fin { 0 } else { 1.max(wi("deferred")) };
        let s = &mut self.s;
        s.plan_us = wi("plan_us");
        s.fit_probe_us = wi("fit_probe_us");
        s.fit_probe_walk = wi("fit_probe_walk");
        s.read_us += wi("read_us");
        s.decode_us += wi("decode_us");
        s.scene_us += wi("scene_us");
        s.draw_us += wi("raster_us");
        s.png_us += wi("png_us");
        s.publish_write_us += wi("publish_write_us");
        s.publish_sync_us += wi("publish_sync_us");
        s.publish_rename_us += wi("publish_rename_us");
        s.adapter_read_us += adapter_read_us;
        s.rounds += 1;
        s.decode_sum_us += wi("decode_sum_us");
        s.decode_max_us = s.decode_max_us.max(wi("decode_max_us"));
        s.index_us += wi("index_us");
        s.raster_tile_max_us = s.raster_tile_max_us.max(wi("raster_tile_max_us"));
        s.tiles_reused += wi("tiles_reused");
        s.mask_bytes = s.mask_bytes.max(wi("mask_bytes"));
        s.bin_items += wi("bin_items");
        s.bin_overflow = s.bin_overflow.max(wi("bin_overflow"));
        s.defer_rep += wi("bin_defer_rep");
        s.defer_single += wi("bin_defer_single");
        s.defer_wmax = s.defer_wmax.max(wi("bin_defer_wmax"));
        s.member_paints +=
            wi("rect_paints") + wi("polygon_paints") + wi("path_paints") + wi("frame_paints");
        s.rep_tested += wi("rep_tested");
        s.rep_drawn += wi("rep_drawn");
        s.hier_cells += wi("hier_cells");
        s.subtree_prunes += wi("subtree_prunes");
        s.once_tiles += wi("once_tiles");
        s.once_passes += wi("once_passes");
        s.once_items += wi("once_items");
        add_place_walks(
            &mut s.place_walks,
            fields.get("place_walks").map_or("-", String::as_str),
        );
        let wire = |name: &str| fields.get(name).map_or("-", String::as_str);
        s.density_stack = wire_counts(wire("density_stack"), &DENSITY_STACK_COUNTS);
        s.density_pages = wire_counts(wire("density_pages"), &DENSITY_PAGE_COUNTS);
        s.density_us = wire_counts(wire("density_us"), &DENSITY_TIMES);
        s.density_bin = wire_counts(wire("density_bin"), &DENSITY_BIN);
        s.density_dots = wire_counts(wire("density_dots"), &DENSITY_DOTS);
        s.density_plan2 = density_plan2(wire("density_plan2"));
        s.density_floor = parse_float(wire("density_floor"));
        s.density_block = parse_float(wire("density_block"));
        s.new += wi("cache_miss");
        s.cache_hit += wi("cache_hit");
        s.cache_evicted += wi("cache_evict");
        s.retained_bytes = wi("retained_bytes");
        s.frame_cache_hit += wi("frame_cache_hit");
        s.render_tiles += wi("tiles");
        s.resident_bytes = s.resident_bytes.max(wi("resident_bytes"));
        s.decode_workers = s.decode_workers.max(wi("decode_workers"));
        let deferred = wi("deferred");
        s.queue_us = wi("queue_us");
        s.wall_us = wi("wall_us");
        s.text_plan_us = wi("text_plan_us");
        let elapsed_ms = timing.elapsed_ms;
        let wall_ms = s.wall_us as f64 / 1000.0;
        let (mut wait_ms, mut other_ms) = (0i128, 0i128);
        if wall_ms > 0.0 {
            wait_ms = 0.max(round_f(
                elapsed_ms - wall_ms - s.adapter_read_us as f64 / 1000.0,
            ));
            if !probe {
                let phases_us = s.plan_us
                    + s.text_plan_us
                    + s.fit_probe_us
                    + s.read_us
                    + s.decode_us
                    + s.scene_us
                    + s.draw_us
                    + s.png_us
                    + s.publish_write_us
                    + s.publish_sync_us
                    + s.publish_rename_us;
                other_ms = 0.max(round_f(wall_ms - phases_us as f64 / 1000.0));
            }
        }
        let ms = |us: i128| float_value(us as f64 / 1000.0);
        let mib = |bytes: i128| float_value(bytes as f64 / (1024.0 * 1024.0));
        let rdiv = |us: i128| int_value(round_f(int_truediv(us, 1000)));
        let job = &self.job;
        let mut o = Map::new();
        let mut put = |k: &str, v: Value| {
            o.insert(k.to_string(), v);
        };
        put(
            "kind",
            Value::from(if probe { "probe_frame" } else { "frame" }),
        );
        put("frame_format", Value::from(frame_format));
        put("bbox", job.bbox.clone());
        put("gen", int_value(generation));
        put(
            "tiles",
            int_value(wire_int(fields, "plan_pages", wi("pages") + deferred)),
        );
        put("new", int_value(s.new));
        put("scope", job.scope.clone());
        put("bg", Value::Bool(job.bg));
        put(
            "load_ms",
            rdiv(s.plan_us + s.read_us + s.decode_us + s.scene_us),
        );
        put("phase_plan", rdiv(s.plan_us));
        put("phase_delta", rdiv(s.read_us));
        put("phase_apply", rdiv(s.decode_us + s.scene_us));
        put("draw_ms", rdiv(s.draw_us));
        put("read_ms", ms(s.read_us));
        put("decode_ms", ms(s.decode_us));
        put("scene_ms", ms(s.scene_us));
        put("raster_ms", ms(s.draw_us));
        put("png_ms", ms(s.png_us));
        put("publish_write_ms", ms(s.publish_write_us));
        put("publish_sync_ms", ms(s.publish_sync_us));
        put("publish_rename_ms", ms(s.publish_rename_us));
        put(
            "publish_ms",
            ms(s.publish_write_us + s.publish_sync_us + s.publish_rename_us),
        );
        put("adapter_read_ms", ms(s.adapter_read_us));
        put("cache_hit", int_value(s.cache_hit));
        put("cache_miss", int_value(s.new));
        put("cache_evicted", int_value(s.cache_evicted));
        put("frame_cache_hit", int_value(s.frame_cache_hit));
        put("retained_mb", mib(s.retained_bytes));
        put("resident_mb", mib(s.resident_bytes));
        put("decode_workers", int_value(s.decode_workers));
        put("workers", int_value(wi("workers")));
        put("raster_jobs", Value::from(adapter.raster_jobs));
        put("render_tiles", int_value(s.render_tiles));
        put("tile_px", int_value(wi("tile_px")));
        put("frame_width", Value::from(job.w));
        put("frame_height", Value::from(job.h));
        put("wait_ms", int_value(wait_ms));
        put("queue_ms", ms(s.queue_us));
        put("wall_ms", float_value(wall_ms));
        put("other_ms", int_value(other_ms));
        put("ms", int_value(round_f(elapsed_ms)));
        put("plan_ms", ms(s.plan_us));
        put("fit_probe_ms", ms(s.fit_probe_us));
        put("fit_probe_walk", Value::Bool(s.fit_probe_walk != 0));
        put("wc_cells", int_value(wi("wc_cells")));
        put("inst_edges", int_value(wi("inst_edges")));
        put("frame_rects", int_value(wi("frame_rects")));
        let mut culls = Map::new();
        for (key, name) in PLAN_CULLS {
            let v = if *key == "fit_layer_edge" {
                match fields.get(*name) {
                    None => Value::Null,
                    Some(v) if v == "-" => Value::Null,
                    Some(v) => Value::from(v.as_str()),
                }
            } else {
                int_value(wi(name))
            };
            culls.insert(key.to_string(), v);
        }
        put("plan_culls", Value::Object(culls));
        let mut summary = Map::new();
        for (key, name) in [
            ("layers", "summary_layers"),
            ("cells", "summary_cells"),
            ("pixels", "summary_pixels"),
            ("level", "summary_level"),
        ] {
            summary.insert(key.into(), int_value(wi(name)));
        }
        summary.insert(
            "cell_um".into(),
            float_value(
                fields
                    .get("summary_cell_um")
                    .map_or(Some(0.0), |v| parse_float(v))
                    .unwrap_or(0.0),
            ),
        );
        summary.insert(
            "none".into(),
            Value::from(fields.get("summary_none").map_or("-", String::as_str)),
        );
        summary.insert("pages_skipped".into(), int_value(wi("summary_pages")));
        put("summary", Value::Object(summary));
        put("text_plan_ms", ms(wi("text_plan_us")));
        put("text_place_records", int_value(wi("text_place_records")));
        put("labels", int_value(wi("labels")));
        put("label_tile_paints", int_value(wi("label_tile_paints")));
        put("label_pixel_paints", int_value(wi("label_pixel_paints")));
        put("rounds", int_value(s.rounds));
        put("decode_sum_ms", ms(s.decode_sum_us));
        put("decode_max_ms", ms(s.decode_max_us));
        put("index_ms", ms(s.index_us));
        put("raster_tile_max_ms", ms(s.raster_tile_max_us));
        put("tiles_reused", int_value(s.tiles_reused));
        put("mask_mb", mib(s.mask_bytes));
        put("work_bin_items", int_value(s.bin_items));
        put("work_bin_overflow_items", int_value(s.bin_overflow));
        put("work_bin_defer_rep", int_value(s.defer_rep));
        put("work_bin_defer_single", int_value(s.defer_single));
        put("work_bin_defer_wmax", int_value(s.defer_wmax));
        put("member_paints", int_value(s.member_paints));
        put("rep_members_tested", int_value(s.rep_tested));
        put("rep_members_drawn", int_value(s.rep_drawn));
        put("hier_cells_visited", int_value(s.hier_cells));
        put("subtrees_pruned", int_value(s.subtree_prunes));
        put("once_full_tiles", int_value(s.once_tiles));
        put("once_passes_skipped", int_value(s.once_passes));
        put("once_items_skipped", int_value(s.once_items));
        put(
            "place_walks",
            Value::Object(
                s.place_walks
                    .iter()
                    .map(|(k, [w, m])| (k.clone(), Value::from(vec![int_value(*w), int_value(*m)])))
                    .collect(),
            ),
        );
        let opt = |v: &Option<Value>| v.clone().unwrap_or(Value::Null);
        put("density_stack", opt(&s.density_stack));
        put("density_pages", opt(&s.density_pages));
        put("density_us", opt(&s.density_us));
        put("density_bin", opt(&s.density_bin));
        put("density_dots", opt(&s.density_dots));
        put(
            "density_floor",
            s.density_floor.map_or(Value::Null, float_value),
        );
        put(
            "density_block",
            s.density_block.map_or(Value::Null, float_value),
        );
        put("density_plan2", opt(&s.density_plan2));
        if refining != 0 {
            put("refining", int_value(refining));
            if wi("density_round") != 0 {
                put("density_round", Value::Bool(true));
            }
        } else if deferred != 0 {
            put("over_budget_pages", int_value(deferred));
        }
        if fields.contains_key("passes") {
            let mut deck = Map::new();
            for (key, name) in DECK {
                deck.insert(key.to_string(), int_value(wi(name)));
            }
            deck.insert("over_budget_pages".into(), int_value(deferred));
            put("deck", Value::Object(deck));
        }
        if let Some(depth) = adapter.max_depth {
            put("max_depth", Value::from(depth));
        }
        if wi("labels_truncated") != 0 {
            put("labels_truncated", Value::Bool(true));
        }
        let cut_px = if job.cut_px > 0.0 { job.cut_px } else { 0.0 };
        if cut_px != 0.0 {
            let b = |i: usize| job.bbox.get(i).map_or(f64::NAN, |v| float_of(Py::of(v)));
            let span_dbu = b(2) - b(0);
            let scaled = span_dbu * adapter.dbu.unwrap_or(f64::NAN);
            let px_per_um = job.w as f64 / if scaled > 1e-12 { scaled } else { 1e-12 };
            put("cut_um", float_value(round_digits(cut_px / px_per_um, 3)));
        }
        if probe {
            let mut p = Map::new();
            p.insert(
                "mode".into(),
                Value::from(fields.get("mode").map_or("", String::as_str)),
            );
            for name in PROBE_COUNTS {
                p.insert(name.to_string(), int_value(wi(name)));
            }
            for (key, name) in PROBE_TIMES {
                p.insert(key.to_string(), ms(wi(name)));
            }
            for name in PROBE_TAIL {
                p.insert(name.to_string(), int_value(wi(name)));
            }
            put("probe", Value::Object(p));
        }
        Value::Object(o)
    }
}

/// `plan_culls`: (result key, wire field); `fit_layer_edge` is a string.
const PLAN_CULLS: &[(&str, &str)] = &[
    ("pages_size", "cull_pages"),
    ("page_bvh", "cull_pbvh"),
    ("child_bvh", "cull_cbvh"),
    ("children_size", "cull_children"),
    ("layer", "cull_layer"),
    ("washed", "washed"),
    ("lod_swapped", "lod_swapped"),
    ("thin_frames", "thin_frames"),
    ("thin_pages", "thin_pages"),
    ("sub_cut_washes", "sub_cut_washes"),
    ("sub_cut_sparse", "sub_cut_sparse"),
    ("sub_cut_sparse_over", "sub_cut_sparse_over"),
    ("sub_cut_wash_over", "sub_cut_wash_over"),
    ("rep_kept", "rep_kept"),
    ("rep_washed", "rep_washed"),
    ("rep_children", "rep_children"),
    ("rep_page_level", "rep_page_level"),
    ("rep_level", "rep_level"),
    ("fit_pct", "fit_pct"),
    ("fit_cull", "fit_cull"),
    ("fit_over", "fit_over"),
    ("fit_thin", "fit_thin"),
    ("fit_full_pct", "fit_full_pct"),
    ("fit_none_pct", "fit_none_pct"),
    ("fit_fixed", "fit_fixed"),
    ("fit_redecided", "fit_redecided"),
    ("fit_ranked", "fit_ranked"),
    ("fit_layers_whole", "fit_layers_whole"),
    ("fit_layer_edge", "fit_layer_edge"),
    ("fit_layers_out", "fit_layers_out"),
    ("fit_scale", "fit_scale"),
    ("fit_refits", "fit_refits"),
    ("sub_cut_boxes", "sub_cut_boxes"),
    ("sub_cut_box_over", "sub_cut_box_over"),
    ("sub_cut_box_level", "sub_cut_box_level"),
    ("sub_cut_box_unsure", "sub_cut_box_unsure"),
    ("shape_cut", "shape_cut"),
    ("shape_cut_max", "shape_cut_max"),
    ("stored_rep_points", "stored_rep_points"),
    ("stored_rep_tested", "stored_rep_tested"),
    ("stored_rep_limited", "stored_rep_limited"),
    ("stored_rep_nodes", "stored_rep_nodes"),
    ("stored_rep_proxies", "stored_rep_proxies"),
    ("stored_rep_bytes", "stored_rep_bytes"),
    ("stored_rep_pixels", "stored_rep_pixels"),
    ("stored_rep_spans", "stored_rep_spans"),
    ("stored_rep_painted_pixels", "stored_rep_painted_pixels"),
];

/// A jobdeck composite's counters (the frame line has `passes=`).
const DECK: &[(&str, &str)] = &[
    ("passes", "passes"),
    ("passes_skipped", "passes_skipped"),
    ("frame_passes", "frame_passes"),
    ("unique_pages", "unique_pages"),
    ("pages_summed", "pages"),
    ("pass_bytes_max", "pass_bytes_max"),
    ("scene_us", "scene_us"),
    ("frame_raster_us", "frame_raster_us"),
    ("raster_us", "raster_us"),
    ("composite_us", "composite_us"),
    ("scene_reuses", "scene_reuses"),
    ("raster_wall_us", "raster_wall_us"),
    ("pass_workers", "pass_workers"),
    ("batches", "batches"),
    ("batch_bytes_max", "batch_bytes_max"),
    ("streamed_passes", "streamed_passes"),
    ("slices", "slices"),
    ("wide_washes", "wide_washes"),
    ("summary_passes", "summary_passes"),
    ("summary_none_passes", "summary_none_passes"),
    ("summary_cells", "summary_cells"),
];

/// A probe's own report (docs/LAYER_DECODE_PROBE_PLAN.ko.md §8), in order:
/// counts, then times (ms of the wire's us), then the tail counts.
const PROBE_COUNTS: &[&str] = &[
    "block",
    "planned_pages",
    "selected_pages",
    "requested_pages",
    "decoded_pages",
    "cache_hits",
    "cache_misses",
    "skipped_pages",
    "skipped_bytes",
    "decoded_bytes",
    "demand_candidates",
    "demand_out_of_view",
    "demand_occluded",
    "demand_unsure",
    "passes",
    "blocks",
    "layer_passes",
];
const PROBE_TIMES: &[(&str, &str)] = &[
    ("decode_ms", "decode_us"),
    ("read_ms", "read_us"),
    ("decode_sum_ms", "decode_sum_us"),
    ("demand_ms", "demand_us"),
    ("pool_ms", "pool_us"),
    ("scene_ms", "scene_us"),
    ("prepare_ms", "prepare_us"),
    ("paint_ms", "paint_us"),
    ("total_ms", "total_us"),
    ("raster_ms", "raster_us"),
];
const PROBE_TAIL: &[&str] = &["once_tiles", "once_passes", "once_items", "bin_items"];

// ---- the wire's values, read as the Python adapter reads them ---------

/// `_wire_int`: `int(fields.get(name, default))`, the default when it is not
/// an integer.
fn wire_int(fields: &BTreeMap<String, String>, name: &str, default: i128) -> i128 {
    fields
        .get(name)
        .map_or(default, |v| parse_int(v).unwrap_or(default))
}

/// `_wire_counts`: a `/`-separated value as {name: int}; None for `-`, a
/// value that is not integers or a count mismatch.
fn wire_counts(value: &str, names: &[&str]) -> Option<Value> {
    wire_list(value, names.len()).map(|counts| {
        Value::Object(
            names
                .iter()
                .zip(counts)
                .map(|(n, c)| (n.to_string(), int_value(c)))
                .collect(),
        )
    })
}

fn wire_list(value: &str, len: usize) -> Option<Vec<i128>> {
    if value.is_empty() || value == "-" {
        return None;
    }
    let counts = value
        .split('/')
        .map(parse_int)
        .collect::<Option<Vec<_>>>()?;
    (counts.len() == len).then_some(counts)
}

/// `_density_plan2`: pass 2's diagnostics, the fields an older renderer
/// omits 0.
fn density_plan2(value: &str) -> Option<Value> {
    for size in DENSITY_PLAN2_SIZES {
        if let Some(counts) = wire_list(value, size) {
            let mut out = Map::new();
            for (i, name) in DENSITY_PLAN2.iter().enumerate() {
                out.insert(
                    name.to_string(),
                    int_value(counts.get(i).copied().unwrap_or(0)),
                );
            }
            return Some(Value::Object(out));
        }
    }
    None
}

/// `_add_place_walks`: `<outcome>:<walks>/<members>`, comma-separated, `-`
/// for none; a malformed part is passed over.
fn add_place_walks(total: &mut Vec<(String, [i128; 2])>, value: &str) {
    if value.is_empty() || value == "-" {
        return;
    }
    for part in value.split(',') {
        let Some((name, counts)) = part.split_once(':') else {
            continue;
        };
        let Some((walks, members)) = counts.split_once('/') else {
            continue;
        };
        let (Some(walks), Some(members)) = (parse_int(walks), parse_int(members)) else {
            continue;
        };
        match total.iter_mut().find(|(n, _)| n == name) {
            Some((_, entry)) => {
                entry[0] += walks;
                entry[1] += members;
            }
            None => total.push((name.to_string(), [walks, members])),
        }
    }
}

/// Python's whitespace for `str.strip()` / `int()` / `float()`.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Python's `_` rule for numbers: only between two digits; the text without
/// them.
fn without_underscores(s: &str) -> Option<String> {
    if !s.contains('_') {
        return Some(s.to_string());
    }
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'_'
            && !(i > 0 && i + 1 < b.len() && b[i - 1].is_ascii_digit() && b[i + 1].is_ascii_digit())
        {
            return None;
        }
    }
    Some(s.replace('_', ""))
}

/// Python's `int(text)` (base 10, ASCII digits).
fn parse_int(text: &str) -> Option<i128> {
    let t = without_underscores(text.trim_matches(py_space))?;
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, &t[..]),
    };
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut v: i128 = 0;
    for c in digits.bytes() {
        v = v.checked_mul(10)?.checked_add(i128::from(c - b'0'))?;
    }
    // past 2^96 not read (Python's is unbounded; past 2^64 no JSON number
    // holds it anyway), so the generation's sums cannot overflow
    (v < 1 << 96).then_some(if neg { -v } else { v })
}

/// Python's `float(text)`.
fn parse_float(text: &str) -> Option<f64> {
    let t = without_underscores(text.trim_matches(py_space))?;
    let body = t.trim_start_matches(['+', '-']);
    if t.len() - body.len() > 1 || body.is_empty() {
        return None;
    }
    let lower = body.to_ascii_lowercase();
    if !(lower == "inf" || lower == "infinity" || lower == "nan")
        && !body
            .bytes()
            .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return None;
    }
    t.parse::<f64>().ok()
}

// ---- Python values and arithmetic -------------------------------------

/// A JSON value as Python sees it in the result dict.
#[derive(Clone, Copy, Debug)]
enum Py<'a> {
    None,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(&'a str),
    List(&'a [Value]),
    Dict(&'a Map<String, Value>),
}

impl<'a> Py<'a> {
    fn of(v: &'a Value) -> Self {
        match v {
            Value::Null => Py::None,
            Value::Bool(b) => Py::Bool(*b),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Py::Int(i128::from(i))
                } else if let Some(u) = n.as_u64() {
                    Py::Int(i128::from(u))
                } else {
                    Py::Float(n.as_f64().unwrap_or(f64::NAN))
                }
            }
            Value::String(s) => Py::Str(s),
            Value::Array(a) => Py::List(a),
            Value::Object(o) => Py::Dict(o),
        }
    }
    fn truthy(self) -> bool {
        match self {
            Py::None => false,
            Py::Bool(b) => b,
            Py::Int(i) => i != 0,
            Py::Float(x) => x != 0.0,
            Py::Str(s) => !s.is_empty(),
            Py::List(a) => !a.is_empty(),
            Py::Dict(o) => !o.is_empty(),
        }
    }
    fn is_none(self) -> bool {
        matches!(self, Py::None)
    }
    /// `d.get(key)` (None for a value that is not a dict, as `(x or {})`).
    fn get(self, key: &str) -> Py<'a> {
        match self {
            Py::Dict(o) => o.get(key).map_or(Py::None, Py::of),
            _ => Py::None,
        }
    }
    /// `d.get(key, default)`.
    fn get_or(self, key: &str, default: Py<'a>) -> Py<'a> {
        match self {
            Py::Dict(o) => o.get(key).map_or(default, Py::of),
            _ => default,
        }
    }
    /// `self or other`.
    fn or(self, other: Py<'a>) -> Py<'a> {
        if self.truthy() {
            self
        } else {
            other
        }
    }
    fn num(self) -> Option<Num> {
        match self {
            Py::Bool(b) => Some(Num::Int(i128::from(b))),
            Py::Int(i) => Some(Num::Int(i)),
            Py::Float(x) => Some(Num::Float(x)),
            _ => None,
        }
    }
    fn eq_str(self, s: &str) -> bool {
        matches!(self, Py::Str(v) if v == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Num {
    Int(i128),
    Float(f64),
}

impl Num {
    fn f(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(x) => x,
        }
    }
}

const TWO_127: f64 = 1.7014118346046923e38;

/// Python's exact comparison of numbers (an int against a float too).
fn num_cmp(a: Num, b: Num) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    fn int_float(i: i128, f: f64) -> Option<Ordering> {
        if f.is_nan() {
            return None;
        }
        let t = f.trunc();
        if t >= TWO_127 {
            return Some(Ordering::Less);
        }
        if t < -TWO_127 {
            return Some(Ordering::Greater);
        }
        match i.cmp(&(t as i128)) {
            Ordering::Equal => (0.0).partial_cmp(&(f - t)),
            o => Some(o),
        }
    }
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => Some(x.cmp(&y)),
        (Num::Float(x), Num::Float(y)) => x.partial_cmp(&y),
        (Num::Int(i), Num::Float(f)) => int_float(i, f),
        (Num::Float(f), Num::Int(i)) => int_float(i, f).map(Ordering::reverse),
    }
}

/// `a > b` (False where Python would raise: not numbers).
fn gt(a: Py, b: Num) -> bool {
    a.num()
        .and_then(|a| num_cmp(a, b))
        .is_some_and(|o| o.is_gt())
}

fn ge(a: Py, b: Num) -> bool {
    a.num()
        .and_then(|a| num_cmp(a, b))
        .is_some_and(|o| o.is_ge())
}

fn lt(a: Py, b: Num) -> bool {
    a.num()
        .and_then(|a| num_cmp(a, b))
        .is_some_and(|o| o.is_lt())
}

/// `max(a, b)`: the first unless the second is greater.
fn max2<'a>(a: Py<'a>, b: Py<'a>) -> Py<'a> {
    match (a.num(), b.num()) {
        (Some(x), Some(y)) if num_cmp(y, x).is_some_and(|o| o.is_gt()) => b,
        _ => a,
    }
}

/// `float(x)` of a number (0 for what Python would refuse).
fn float_of(p: Py) -> f64 {
    match p {
        Py::Str(s) => parse_float(s).unwrap_or(0.0),
        _ => p.num().map_or(0.0, Num::f),
    }
}

/// `int(x)`: a float truncated, a string read (0 for what Python would refuse).
fn int_of(p: Py) -> i128 {
    match p {
        Py::Str(s) => parse_int(s).unwrap_or(0),
        Py::Float(x) => trunc_i128(x),
        _ => match p.num() {
            Some(Num::Int(i)) => i,
            _ => 0,
        },
    }
}

fn trunc_i128(x: f64) -> i128 {
    // saturating past i128; NaN 0
    x.trunc() as i128
}

/// `round(x)` of a float: half to even.
fn round_f(x: f64) -> i128 {
    x.round_ties_even() as i128
}

/// `round(x)` of a number.
fn round_n(n: Num) -> i128 {
    match n {
        Num::Int(i) => i,
        Num::Float(x) => round_f(x),
    }
}

/// `round(x, digits)` of a float: the nearest float to x correctly rounded
/// (half to even) to `digits` decimals.
fn round_digits(x: f64, digits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.digits$}").parse().unwrap_or(x)
}

/// Python's int / int: the exact quotient correctly rounded.
fn int_truediv(a: i128, b: i128) -> f64 {
    const EXACT: u128 = 1 << 53;
    if b == 0 {
        return f64::NAN;
    }
    let (n, d) = (a.unsigned_abs(), b.unsigned_abs());
    let neg = (a < 0) != (b < 0);
    // both exact as floats: IEEE division rounds the exact quotient (a
    // divisor past 64 bits, never one here, is left to it too)
    let q = if (n <= EXACT && d <= EXACT) || d >= 1 << 64 {
        n as f64 / d as f64
    } else {
        // a quotient of 55 bits or more, the remainder as a sticky bit: the
        // integer's conversion then rounds as the exact quotient does
        let bits = |v: u128| 128 - v.leading_zeros() as i32;
        let shift = (55 + bits(d) - bits(n)).max(0);
        let scaled = n << shift;
        let (q, r) = (scaled / d, scaled % d);
        let q = q | u128::from(r != 0);
        q as f64 * 2f64.powi(-shift)
    };
    if neg {
        -q
    } else {
        q
    }
}

/// `a / b` of two numbers.
fn truediv(a: Num, b: Num) -> f64 {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => int_truediv(x, y),
        _ => a.f() / b.f(),
    }
}

/// `round(x / divisor)` of a number from the dict.
fn round_div(p: Py, divisor: Num) -> i128 {
    round_f(truediv(p.num().unwrap_or(Num::Int(0)), divisor))
}

// ---- Python formatting -------------------------------------------------

/// Decimal digits of an integer past i128 (a float's, a large shift's).
fn big_decimal(mut limbs: Vec<u32>) -> String {
    // little-endian base 1e9 limbs
    while limbs.len() > 1 && *limbs.last().unwrap() == 0 {
        limbs.pop();
    }
    let mut out = limbs.last().unwrap().to_string();
    for limb in limbs.iter().rev().skip(1) {
        out.push_str(&format!("{limb:09}"));
    }
    out
}

fn big_shifted(mantissa: u64, shift: u32) -> String {
    let mut limbs = vec![
        (mantissa % 1_000_000_000) as u32,
        ((mantissa / 1_000_000_000) % 1_000_000_000) as u32,
        (mantissa / 1_000_000_000_000_000_000) as u32,
    ];
    for _ in 0..shift {
        let mut carry = 0u64;
        for limb in limbs.iter_mut() {
            let v = u64::from(*limb) * 2 + carry;
            *limb = (v % 1_000_000_000) as u32;
            carry = v / 1_000_000_000;
        }
        if carry > 0 {
            limbs.push(carry as u32);
        }
    }
    big_decimal(limbs)
}

/// `str(int(x))` of a float.
fn float_int_string(x: f64) -> String {
    let t = x.trunc();
    if !t.is_finite() {
        return "0".into();
    }
    if t.abs() < TWO_127 {
        return (t as i128).to_string();
    }
    let bits = t.abs().to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32 - 1075;
    let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
    let digits = big_shifted(mantissa, exp as u32);
    if t < 0.0 {
        format!("-{digits}")
    } else {
        digits
    }
}

/// The largest N whose 2^N Python prints (4300 digits, its default
/// int_max_str_digits).
const PY_INT_DIGITS_SHIFT: i128 = 14_284;

/// `%d` of `1 << n`.
fn pow2_string(n: i128) -> String {
    match n {
        n if n < 0 => "0".into(),
        n if n < 126 => (1i128 << n).to_string(),
        n if n <= PY_INT_DIGITS_SHIFT => big_shifted(1, n as u32),
        // Python refuses to print an int of more than 4300 digits (its line
        // fails); never a renderd value
        n => format!("2**{n}"),
    }
}

/// `%d` of a number (a float truncated, a bool 0/1).
fn fmt_d(p: Py) -> String {
    match p {
        Py::Float(x) => float_int_string(x),
        _ => int_of(p).to_string(),
    }
}

/// `%.Nf`.
fn fmt_f(x: f64, prec: usize) -> String {
    if x.is_nan() {
        "nan".into()
    } else if x.is_infinite() {
        (if x > 0.0 { "inf" } else { "-inf" }).into()
    } else {
        format!("{x:.prec$}")
    }
}

/// The digits and the decimal point's place (value = 0.DIGITS x 10^decpt)
/// of Rust's exponent form (`1.2345e3`).
fn digits_of(exp_form: &str) -> (bool, String, i32) {
    let (mantissa, exp) = exp_form.split_once('e').unwrap_or((exp_form, "0"));
    let (neg, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    (neg, digits, exp.parse::<i32>().unwrap_or(0) + 1)
}

fn exp_suffix(e: i32) -> String {
    format!("e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
}

/// `%.Ng`.
fn fmt_g(x: f64, prec: usize) -> String {
    let prec = prec.max(1);
    if !x.is_finite() {
        return fmt_f(x, 0);
    }
    if x == 0.0 {
        return (if x.is_sign_negative() { "-0" } else { "0" }).into();
    }
    let (neg, digits, decpt) = digits_of(&format!("{:.*e}", prec - 1, x));
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let mut out = String::from(if neg { "-" } else { "" });
    if decpt <= -4 || decpt > prec as i32 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push_str(&exp_suffix(decpt - 1));
    } else if decpt <= 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat((-decpt) as usize));
        out.push_str(digits);
    } else if decpt as usize >= digits.len() {
        out.push_str(digits);
        out.push_str(&"0".repeat(decpt as usize - digits.len()));
    } else {
        out.push_str(&digits[..decpt as usize]);
        out.push('.');
        out.push_str(&digits[decpt as usize..]);
    }
    out
}

/// `repr(x)` / `str(x)` of a float: the shortest digits that read back.
fn repr_f64(x: f64) -> String {
    if !x.is_finite() {
        return fmt_f(x, 0);
    }
    if x == 0.0 {
        return (if x.is_sign_negative() { "-0.0" } else { "0.0" }).into();
    }
    let (neg, digits, decpt) = digits_of(&format!("{x:e}"));
    let mut out = String::from(if neg { "-" } else { "" });
    if decpt <= -4 || decpt > 16 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push_str(&exp_suffix(decpt - 1));
    } else if decpt <= 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat((-decpt) as usize));
        out.push_str(&digits);
    } else if decpt as usize >= digits.len() {
        out.push_str(&digits);
        out.push_str(&"0".repeat(decpt as usize - digits.len()));
        out.push_str(".0");
    } else {
        out.push_str(&digits[..decpt as usize]);
        out.push('.');
        out.push_str(&digits[decpt as usize..]);
    }
    out
}

/// `%s` / `str(x)`.
fn fmt_s(p: Py) -> String {
    match p {
        Py::None => "None".into(),
        Py::Bool(b) => (if b { "True" } else { "False" }).into(),
        Py::Int(i) => i.to_string(),
        Py::Float(x) => repr_f64(x),
        Py::Str(s) => s.into(),
        Py::List(a) => format!(
            "[{}]",
            a.iter()
                .map(|v| py_repr(Py::of(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Py::Dict(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", py_repr(Py::Str(k)), py_repr(Py::of(v))))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn py_repr(p: Py) -> String {
    match p {
        Py::Str(s) => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        _ => fmt_s(p),
    }
}

/// `float(x)` formatted `%.Nf`.
fn ff(p: Py, prec: usize) -> String {
    fmt_f(float_of(p), prec)
}

/// `float(x)` formatted `%.Ng`.
fn fg(p: Py, prec: usize) -> String {
    fmt_g(float_of(p), prec)
}

fn int_value(i: i128) -> Value {
    if let Ok(v) = i64::try_from(i) {
        Value::from(v)
    } else if let Ok(v) = u64::try_from(i) {
        Value::from(v)
    } else {
        float_value(i as f64)
    }
}

fn float_value(x: f64) -> Value {
    serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number)
}

fn saturate_i64(i: i128) -> i64 {
    i64::try_from(i).unwrap_or(if i < 0 { i64::MIN } else { i64::MAX })
}

// ---- the perf line -----------------------------------------------------

const I0: Py<'static> = Py::Int(0);

/// floe/gui.py `fmt_count`: a human count, 950 / 12k / 3.4M / 1.2G.
pub fn fmt_count(n: &Value) -> String {
    count(Py::of(n))
}

fn count(n: Py) -> String {
    let num = n.num().unwrap_or(Num::Int(0));
    let at_least = |limit: f64| num_cmp(num, Num::Float(limit)).is_some_and(|o| o.is_ge());
    if at_least(1e9) {
        format!("{}G", fmt_f(truediv(num, Num::Float(1e9)), 1))
    } else if at_least(1e6) {
        format!("{}M", fmt_f(truediv(num, Num::Float(1e6)), 1))
    } else if at_least(10e3) {
        format!("{}k", fmt_f(truediv(num, Num::Float(1e3)), 0))
    } else {
        match num {
            Num::Float(x) => float_int_string(x),
            Num::Int(i) => i.to_string(),
        }
    }
}

/// floe/gui.py `occ_note`: pass 2 drawn from the occupancy density - its
/// cell, the layers it held and those this frame made (`full`: and what its
/// cache holds); "" when the plans drew it.
pub fn occ_note(res: &Value, full: bool) -> String {
    occ(Py::of(res), full)
}

fn occ(res: Py, full: bool) -> String {
    let p2 = res.get("density_plan2");
    if !p2.get("occ_layers").truthy() {
        return String::new();
    }
    let mut note = format!(
        "{} um cells, {} layers",
        fmt_g(
            truediv(
                p2.get_or("occ_cell_nm", I0).num().unwrap_or(Num::Int(0)),
                Num::Float(1000.0)
            ),
            6
        ),
        fmt_d(p2.get("occ_layers"))
    );
    if p2.get("occ_made").truthy() {
        note += &format!(", {} made", fmt_d(p2.get("occ_made")));
    }
    if full && p2.get("occ_cache_kb").truthy() {
        note += &format!(
            "; cache {} MB",
            fmt_f(
                truediv(
                    p2.get("occ_cache_kb").num().unwrap_or(Num::Int(0)),
                    Num::Float(1024.0)
                ),
                1
            )
        );
    }
    note
}

/// The viewer's load clock (floe/gui.py `Viewer._load_note`), monotonic
/// seconds: the file chosen, the cache and layer panel ready, the render
/// service open (renderd's own cache open, ms, when it said).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadMarks {
    pub t0: f64,
    pub cache: f64,
    pub service: f64,
    pub renderd_open_ms: Option<f64>,
}

/// The first settled frame after a load says how long the load took: (the
/// log line's note, the bar's); empty without marks or for a refining round.
/// The caller forgets the marks once it is said.
pub fn load_note(marks: Option<&LoadMarks>, res: &Value, now: f64) -> (String, String) {
    let Some(m) = marks else {
        return Default::default();
    };
    if Py::of(res).get("refining").truthy() {
        return Default::default();
    }
    let mut service = format!("service {} s", fmt_f(m.service - m.cache, 1));
    if let Some(opened) = m.renderd_open_ms {
        service += &format!(" [renderd open {} s]", fmt_f(opened / 1000.0, 1));
    }
    (
        format!(
            "loaded in {} s (cache {} s + {} + first frame {} s) · ",
            fmt_f(now - m.t0, 1),
            fmt_f(m.cache - m.t0, 1),
            service,
            fmt_f(now - m.service, 2)
        ),
        format!("loaded in {} s · ", fmt_f(now - m.t0, 1)),
    )
}

/// The viewer's `perf_status` (floe_oracle/perf_line.py): the perf line of a
/// settled (or refining) frame,
/// (the log line and tooltip with every diagnostic, the lower bar's brief
/// one). FLOE_RUST_DENSITY_ONLY=on (renderd's diagnostic) adds `density only`.
pub fn perf_status(res: &Value, depth_note: &str) -> (String, String) {
    let density_only = std::env::var("FLOE_RUST_DENSITY_ONLY").is_ok_and(|v| v == "on");
    perf_status_with(res, depth_note, density_only)
}

/// [`perf_status`] with the environment's FLOE_RUST_DENSITY_ONLY given.
pub fn perf_status_with(res: &Value, depth_note: &str, density_only: bool) -> (String, String) {
    let res = Py::of(res);
    let k = Num::Int(1000);
    let mut split = String::new();
    let mut brief_split = String::new();
    if !res.get("load_ms").is_none() {
        let mut ph = String::new();
        if !res.get("phase_apply").is_none() {
            ph = format!(
                " [{} plan+{} delta+{} apply]",
                fmt_d(res.get_or("phase_plan", I0)),
                fmt_d(res.get_or("phase_delta", I0)),
                fmt_d(res.get_or("phase_apply", I0))
            );
        }
        let text = res.get_or("text_plan_ms", I0).or(I0);
        split = format!(
            " = {} load{}{} + {} draw",
            fmt_d(res.get("load_ms")),
            ph,
            if ge(text, Num::Int(100)) {
                format!(" + {} text", fmt_d(text))
            } else {
                String::new()
            },
            fmt_d(res.get("draw_ms"))
        );
        let probe = res.get_or("fit_probe_ms", I0).or(I0);
        if ge(probe, Num::Int(100)) {
            split += &format!(
                " + {} fit probe",
                round_n(probe.num().unwrap_or(Num::Int(0)))
            );
        }
        if gt(res.get_or("other_ms", I0), Num::Int(200)) {
            split += &format!(" + {} other", fmt_d(res.get("other_ms")));
        }
        if gt(res.get_or("wait_ms", I0), Num::Int(200)) {
            split += &format!(" + {} wait", fmt_d(res.get("wait_ms")));
        }
        brief_split = if ph.is_empty() {
            split.clone()
        } else {
            split.replacen(&ph, "", 1)
        };
    }
    let mut cut = String::new();
    let mut brief_cut = String::new();
    if res.get("cut_um").truthy() {
        cut = format!(", cut<{}um", fg(res.get("cut_um"), 3));
        brief_cut = format!("cut<{}um", fg(res.get("cut_um"), 3));
        let culls = res.get("plan_culls");
        if culls.get("shape_cut").truthy() {
            cut += if culls.get("shape_cut_max").truthy() {
                " (larger side)"
            } else {
                " (min side)"
            };
        }
    }
    let fit = res.get("plan_culls");
    if fit.get("fit_pct").truthy()
        || fit.get("fit_cull").truthy()
        || fit.get("fit_over").truthy()
        || fit.get("fit_thin").truthy()
    {
        let pct = int_of(fit.get_or("fit_pct", I0).or(Py::Int(100)));
        let factor = 100i128.max(pct) as f64 / 100.0;
        let thin = int_of(fit.get_or("fit_thin", I0).or(I0));
        let full = int_of(fit.get_or("fit_full_pct", I0).or(I0)) as f64 / 100.0;
        let none = int_of(fit.get_or("fit_none_pct", I0).or(I0)) as f64 / 100.0;
        let thin_class = || {
            format!(
                "1/{}{}",
                pow2_string(thin.min(30)),
                if full != 0.0 {
                    format!(" below x{}", fmt_g(full, 3))
                } else {
                    String::new()
                }
            )
        };
        let mut fitted = if fit.get("fit_ranked").truthy() {
            let mut parts = Vec::new();
            if factor > 1.0 {
                parts.push(format!("x{}", fmt_g(factor, 3)));
            }
            let whole_n = int_of(fit.get_or("fit_layers_whole", I0).or(I0));
            if whole_n != 0 {
                parts.push(format!("top {whole_n} whole"));
            }
            let edge = fit.get("fit_layer_edge");
            if edge.truthy() {
                let mut classes = Vec::new();
                if thin != 0 {
                    classes.push(thin_class());
                }
                if none != 0.0 {
                    classes.push(format!("none below x{}", fmt_g(none, 3)));
                }
                parts.push(format!(
                    "{}{}",
                    fmt_s(edge),
                    if classes.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", classes.join(", "))
                    }
                ));
            }
            let out_n = int_of(fit.get_or("fit_layers_out", I0).or(I0));
            if out_n != 0 {
                parts.push(format!("{out_n} left out"));
            }
            format!(" {} to fit budget", parts.join(", "))
        } else if thin != 0 || none != 0.0 {
            let mut parts = Vec::new();
            if factor > 1.0 {
                parts.push(format!("x{}", fmt_g(factor, 3)));
            }
            if thin != 0 {
                parts.push(thin_class());
            }
            if none != 0.0 {
                parts.push(format!("none below x{}", fmt_g(none, 3)));
            }
            format!(" {} to fit budget", parts.join(", "))
        } else {
            format!(" x{} to fit budget", fmt_g(factor, 3))
        };
        if fit.get("fit_cull").truthy() {
            fitted += ", hairlines culled";
        }
        if fit.get("fit_over").truthy() {
            fitted += ", STILL OVER";
        }
        if fit.get("fit_redecided").truthy() {
            fitted += " (refit)";
        }
        let scale = int_of(fit.get_or("fit_scale", I0).or(I0));
        if scale > 1000 {
            fitted += &format!(
                ", pages x{} their estimate",
                fmt_g(scale as f64 / 1000.0, 3)
            );
        }
        cut += &fitted;
        brief_cut += &fitted;
    }
    let mut drawn = String::new();
    if !res.get("drawn").is_none() {
        drawn = format!(", ~{} drawn", count(res.get("drawn")));
    }
    let mut refin = String::new();
    if res.get("refining").truthy() {
        refin = format!(", refining {}", fmt_d(res.get("refining")));
    }
    let mut text = String::new();
    if !res.get("plan_ms").is_none() {
        text += &format!(
            ", plan {}ms/{} frontier",
            ff(res.get("plan_ms"), 1),
            count(res.get_or("frame_rects", I0))
        );
        if res.get("fit_probe_ms").truthy() {
            text += &format!(
                ", fit probe {}ms{}",
                ff(res.get("fit_probe_ms"), 1),
                if res.get("fit_probe_walk").truthy() {
                    " (walk)"
                } else {
                    ""
                }
            );
        }
    }
    if !res.get("text_plan_ms").is_none() {
        text += &format!(
            ", text {}ms/{} places",
            ff(res.get("text_plan_ms"), 1),
            count(res.get_or("text_place_records", I0))
        );
    }
    if !res.get("png_ms").is_none() {
        text += &format!(
            ", {} {}ms/pub {}ms",
            if res.get("frame_format").eq_str("raw") {
                "raw"
            } else {
                "png"
            },
            ff(res.get("png_ms"), 1),
            ff(res.get_or("publish_ms", Py::Float(0.0)), 1)
        );
        text += &format!(
            ", rust {}j {}tiles@{}px {}x{}",
            fmt_d(res.get_or("raster_jobs", I0)),
            fmt_d(res.get_or("render_tiles", I0)),
            fmt_s(res.get_or("tile_px", I0)),
            fmt_s(res.get_or("frame_width", I0)),
            fmt_s(res.get_or("frame_height", I0))
        );
    }
    if gt(res.get_or("rounds", I0), Num::Int(1)) {
        text += &format!(", rounds {}", fmt_d(res.get("rounds")));
    }
    if res.get("decode_sum_ms").truthy() {
        text += &format!(
            ", dec sum {}/max {}/idx {}ms",
            ff(res.get("decode_sum_ms"), 0),
            ff(res.get_or("decode_max_ms", Py::Float(0.0)), 0),
            ff(res.get_or("index_ms", Py::Float(0.0)), 0)
        );
    }
    if res.get("raster_tile_max_ms").truthy() {
        text += &format!(", tile-max {}ms", ff(res.get("raster_tile_max_ms"), 0));
    }
    if res.get("tiles_reused").truthy() {
        text += &format!(", pan-reuse {} tiles", fmt_d(res.get("tiles_reused")));
    }
    if res.get("work_bin_items").truthy() {
        text += &format!(", bin {} items", count(res.get("work_bin_items")));
        if res.get("work_bin_defer_rep").truthy() || res.get("work_bin_defer_single").truthy() {
            text += &format!(
                " (defer {}r+{}s w{})",
                count(res.get_or("work_bin_defer_rep", I0)),
                count(res.get_or("work_bin_defer_single", I0)),
                count(res.get_or("work_bin_defer_wmax", I0))
            );
        }
    } else if res.get("work_bin_overflow_items").truthy() {
        text += &format!(
            ", bin off(cap@{})",
            count(res.get("work_bin_overflow_items"))
        );
    }
    if res.get("member_paints").truthy() {
        text += &format!(", paints {}", count(res.get("member_paints")));
    }
    if res.get("cache_evicted").truthy() {
        text += &format!(", evict {}", count(res.get("cache_evicted")));
    }
    if res.get("retained_mb").truthy() {
        text += &format!(
            ", retained {}MB",
            round_n(res.get("retained_mb").num().unwrap_or(Num::Int(0)))
        );
    }
    if res.get("hier_cells_visited").truthy() {
        text += &format!(
            ", hier {}/{} pruned",
            count(res.get("hier_cells_visited")),
            count(res.get_or("subtrees_pruned", I0))
        );
    }
    if res.get("once_full_tiles").truthy() || res.get("once_items_skipped").truthy() {
        text += &format!(
            ", once {} tiles/{} passes/{} items",
            count(res.get_or("once_full_tiles", I0)),
            count(res.get_or("once_passes_skipped", I0)),
            count(res.get_or("once_items_skipped", I0))
        );
    }
    let culls = res.get("plan_culls");
    let any_cull = match culls {
        Py::Dict(o) => o.values().any(|v| Py::of(v).truthy()),
        _ => false,
    };
    let c = |key: &str| count(culls.get_or(key, I0));
    if any_cull {
        text += &format!(
            ", cut pages {}/pbvh {}/cbvh {}/cells {}, layer {}, washed {}, thin {}",
            c("pages_size"),
            c("page_bvh"),
            c("child_bvh"),
            c("children_size"),
            c("layer"),
            c("washed"),
            c("thin_frames")
        );
        if culls.get("thin_pages").truthy() {
            text += &format!(", thin pages {} kept", count(culls.get("thin_pages")));
        }
        if culls.get("sub_cut_washes").truthy() || culls.get("sub_cut_sparse").truthy() {
            text += &format!(
                ", sub-cut washes {}/sparse {}",
                c("sub_cut_washes"),
                c("sub_cut_sparse")
            );
        }
        if culls.get("sub_cut_boxes").truthy() || culls.get("sub_cut_box_over").truthy() {
            text += &format!(
                ", boxes {}{}{}{}",
                c("sub_cut_boxes"),
                if culls.get("sub_cut_box_level").truthy() {
                    format!(
                        " x{} coarser",
                        pow2_string(int_of(culls.get("sub_cut_box_level")))
                    )
                } else {
                    String::new()
                },
                if culls.get("sub_cut_box_over").truthy() {
                    format!(" (+{} over)", count(culls.get("sub_cut_box_over")))
                } else {
                    String::new()
                },
                if culls.get("sub_cut_box_unsure").truthy() {
                    format!(" ({} unsure)", count(culls.get("sub_cut_box_unsure")))
                } else {
                    String::new()
                }
            );
        }
        if culls.get("sub_cut_sparse_over").truthy() || culls.get("sub_cut_wash_over").truthy() {
            text += &format!(
                ", sub-cut over {}/{}",
                c("sub_cut_sparse_over"),
                c("sub_cut_wash_over")
            );
        }
        if culls.get("rep_kept").truthy()
            || culls.get("rep_washed").truthy()
            || culls.get("rep_children").truthy()
        {
            text += &format!(
                ", reps {} pages/{} children",
                c("rep_kept"),
                c("rep_children")
            );
            if culls.get("rep_level").truthy() {
                text += &format!(" L{}", fmt_d(culls.get("rep_level")));
            }
            if culls.get("rep_page_level").truthy() {
                text += &format!(" P{}", fmt_d(culls.get("rep_page_level")));
            }
        }
    }
    if culls.get("stored_rep_points").truthy() || culls.get("stored_rep_limited").truthy() {
        text += &format!(
            ", stored reps {}/tested {}{}",
            c("stored_rep_points"),
            c("stored_rep_tested"),
            if culls.get("stored_rep_limited").truthy() {
                " (capped)"
            } else {
                ""
            }
        );
    }
    let summ = res.get("summary");
    if summ.get("layers").truthy() {
        text += &format!(
            ", summary {} layers {} cells (level {}, {} um; not pickable)",
            fmt_d(summ.get("layers")),
            count(summ.get("cells")),
            fmt_d(summ.get("level")),
            fg(summ.get("cell_um"), 6)
        );
    } else {
        let none = summ.get("none");
        let listed = none.is_none() || ["-", "policy", "exact"].iter().any(|s| none.eq_str(s));
        if !listed {
            text += &format!(", summary: none ({})", fmt_s(none));
        }
    }
    if res.get("labels_truncated").truthy() {
        text += ", labels partial";
    }
    if res.get("over_budget_pages").truthy() {
        text += &format!(
            ", {} pages over budget (not drawn)",
            fmt_d(res.get("over_budget_pages"))
        );
    }
    let deck = res.get("deck");
    if deck.truthy() {
        let d = deck;
        let mb = Num::Float(1e6);
        text += &format!(
            ", deck {} passes ({} frame, {} skipped, {} scene reuses) {}/{} pages, scene {} + frame sum {} + composite {} ms, raster wall {} ms {}p x {}t, {} batches, pass max {}MB, batch max {}MB",
            fmt_d(d.get("passes")),
            fmt_d(d.get("frame_passes")),
            fmt_d(d.get("passes_skipped")),
            fmt_d(d.get_or("scene_reuses", I0)),
            fmt_d(d.get("unique_pages")),
            fmt_d(d.get("pages_summed")),
            round_div(d.get("scene_us"), k),
            round_div(d.get("frame_raster_us"), k),
            round_div(d.get("composite_us"), k),
            round_div(d.get_or("raster_wall_us", I0), k),
            fmt_d(d.get_or("pass_workers", I0)),
            fmt_d(res.get_or("workers", I0)),
            fmt_d(d.get_or("batches", I0)),
            round_div(d.get("pass_bytes_max"), mb),
            round_div(d.get_or("batch_bytes_max", I0), mb)
        );
        if d.get("streamed_passes").truthy() {
            text += &format!(
                ", {} streamed in {} slices",
                fmt_d(d.get("streamed_passes")),
                fmt_d(d.get("slices"))
            );
        }
        if d.get("wide_washes").truthy() {
            text += &format!(", {} sub-cut washes", count(d.get("wide_washes")));
        }
        if d.get("summary_passes").truthy() {
            text += &format!(
                ", summary {} passes {} cells (not pickable)",
                fmt_d(d.get("summary_passes")),
                count(d.get_or("summary_cells", I0))
            );
        }
        if d.get("summary_none_passes").truthy() {
            text += &format!(
                ", {} passes without summary",
                fmt_d(d.get("summary_none_passes"))
            );
        }
    }
    let mut stack = String::new();
    let mut brief_stack = String::new();
    if !res.get("density_stack").is_none() {
        let p2 = res.get("density_plan2");
        let mut parts = vec![if res.get("density_dots").is_none() {
            "top + empty".to_string()
        } else {
            "dots".to_string()
        }];
        if density_only {
            parts.push("density only".into());
        }
        let lit = format!(
            "lit {} px",
            count(res.get("density_stack").get_or("lit", I0))
        );
        parts.push(lit.clone());
        if !res.get("density_block").is_none() {
            parts.push(format!("block {} px", fg(res.get("density_block"), 6)));
        }
        if !res.get("density_floor").is_none() {
            parts.push(format!("floor {} px", fg(res.get("density_floor"), 2)));
        }
        if p2.get("reserve_mb").truthy() {
            parts.push(format!("reserve {} MB", count(p2.get("reserve_mb"))));
        }
        let bright = p2.get("bright_milli");
        if bright.truthy() {
            let pattern = p2.get_or("pattern", I0);
            let gain = fmt_g(
                truediv(bright.num().unwrap_or(Num::Int(0)), Num::Float(1000.0)),
                6,
            );
            parts.push(if pattern.truthy() {
                format!("pattern, cover x{gain}")
            } else {
                format!("bright x{gain}")
            });
            if occ(res, false).is_empty() {
                parts.push(
                    if p2.get("cell_cover").truthy() {
                        "cell cover"
                    } else {
                        "cells by box"
                    }
                    .into(),
                );
            }
        }
        let gain = p2.get("dot_gain_milli");
        if !gain.is_none()
            && lt(Py::Int(0), gain.num().unwrap_or(Num::Int(0)))
            && lt(gain, Num::Int(1000))
        {
            parts.push(format!(
                "dots x{}",
                fmt_f(
                    truediv(gain.num().unwrap_or(Num::Int(0)), Num::Float(1000.0)),
                    2
                )
            ));
        }
        let gate = p2.get("dot_gate_min").or(I0);
        if gt(gate, Num::Int(1)) {
            let block = res.get("density_block").or(Py::Int(4));
            let gated = p2.get("dot_gated").or(I0);
            let square = match block.num().unwrap_or(Num::Int(4)) {
                Num::Int(b) => b.saturating_mul(b),
                Num::Float(b) => round_f(b * b),
            };
            parts.push(format!(
                "gate {}/{} px{}",
                fmt_d(gate),
                square,
                if gated.truthy() {
                    format!(" ({} out)", count(gated))
                } else {
                    String::new()
                }
            ));
        }
        let us = res.get("density_us");
        let occ_brief = occ(res, false);
        let plan2_ms = || round_div(us.get_or("plan2_us", I0), k);
        if us.truthy() && !occ_brief.is_empty() {
            parts.push(format!(
                "pass 2 by occupancy {} ms ({})",
                plan2_ms(),
                occ(res, true)
            ));
        } else if us.truthy() {
            let mut plan = format!("pass 2 plan {} ms", plan2_ms());
            if p2.truthy() {
                let held = |key: &str| match key {
                    "by_list_chunks" => Some("by_chunk_members"),
                    "full_chunks" => Some("full_members"),
                    "sampled_chunks" => Some("sampled_members"),
                    _ => None,
                };
                let mut by = [
                    ("by_nodes", "nodes"),
                    ("by_placements", "placements"),
                    ("by_arrays", "arrays"),
                    ("by_list_members", "list members"),
                    ("by_list_chunks", "list chunks"),
                    ("full_chunks", "list chunks in full blocks"),
                    ("sampled_chunks", "list chunks sampled"),
                    ("by_array_members", "array members"),
                    ("by_pages", "pages"),
                    ("occ_decoded", "pages decoded under the floor"),
                ]
                .iter()
                .filter(|(key, _)| p2.get(key).truthy())
                .map(|(key, name)| {
                    let tail = if let Some(members) = held(key) {
                        format!(" of {} members", count(p2.get_or(members, I0)))
                    } else if *key == "by_pages" && p2.get("occ_pages").truthy() {
                        format!(" ({} by occupancy)", count(p2.get("occ_pages")))
                    } else {
                        String::new()
                    };
                    format!("{} {}{}", name, count(p2.get(key)), tail)
                })
                .collect::<Vec<_>>()
                .join(", ");
                if p2.get("map_updates").truthy() {
                    by += &format!("; hash map {}", count(p2.get("map_updates")));
                }
                if p2.get("stages").truthy() {
                    by += &format!("; {} layer stages", fmt_d(p2.get("stages")));
                }
                if p2.get("mask_tests").truthy() || p2.get("mask_fallbacks").truthy() {
                    by += &format!(
                        "; mask {}/{} pruned, {} fallback",
                        count(p2.get_or("mask_pruned", I0)),
                        count(p2.get_or("mask_tests", I0)),
                        count(p2.get_or("mask_fallbacks", I0))
                    );
                }
                let by = by.trim_start_matches([';', ' ']);
                plan += &format!(
                    " (probe {} ms x{}, fit {} ms x{} passes on {} threads, {} regions{}, nodes {}, page nodes {}, pages {}, reads {}, cell dots {}{})",
                    round_div(p2.get("probe_us"), k),
                    fmt_d(p2.get("probes")),
                    round_div(p2.get("fit_us"), k),
                    fmt_d(p2.get("passes")),
                    fmt_d(max2(Py::Int(1), p2.get_or("threads", Py::Int(1)))),
                    fmt_d(p2.get("regions")),
                    if p2.get("free_top").truthy() || p2.get("free_others").truthy() {
                        format!(
                            " (free top {}, others {} px)",
                            count(p2.get("free_top")),
                            count(p2.get("free_others"))
                        )
                    } else {
                        String::new()
                    },
                    count(p2.get("nodes")),
                    count(p2.get("page_nodes")),
                    count(p2.get("page_candidates")),
                    count(p2.get_or("reads", I0)),
                    count(p2.get_or("items", I0)),
                    if by.is_empty() {
                        String::new()
                    } else {
                        format!(" [{by}]")
                    }
                );
            }
            parts.push(plan);
        }
        let pages = res.get("density_pages");
        if pages.truthy() {
            parts.push(format!("{} pages", fmt_d(pages.get_or("decoded", I0))));
        }
        let mut over = Vec::new();
        if p2.get("probes_over").truthy() {
            over.push("floor probe".to_string());
        }
        if p2.get("thinned").truthy() {
            over.push("thinned".to_string());
        }
        if pages.get("over_budget").truthy() {
            over.push(format!(
                "{} pages left out",
                fmt_d(pages.get_or("over_budget", I0))
            ));
        }
        if p2.get("stood_in").truthy() {
            over.push(format!(
                "{} pages by occupancy instead",
                fmt_d(p2.get_or("stood_in", I0))
            ));
        }
        if !over.is_empty() {
            parts.push(format!("pass 2 over budget: {}", over.join(", ")));
        }
        if !us.get("decode2_us").is_none() {
            parts.push(format!(
                "pass 2 decode {} ms{}",
                round_div(us.get("decode2_us"), k),
                if us.get("raster2_us").is_none() {
                    String::new()
                } else {
                    format!(", raster {} ms", round_div(us.get("raster2_us"), k))
                }
            ));
        }
        stack = format!(" [density: {}]", parts.join(", "));
        let mut brief: Vec<String> = if res.get("density_dots").is_none() {
            vec!["top + empty".into()]
        } else {
            Vec::new()
        };
        brief.push(lit);
        if us.truthy() && !occ_brief.is_empty() {
            brief.push(format!(
                "pass 2 by occupancy {} ms ({})",
                plan2_ms(),
                occ_brief
            ));
        } else if us.truthy() {
            let mut plan = format!("pass 2 plan {} ms", plan2_ms());
            if p2.truthy() {
                let mut inner = Vec::new();
                if p2.get("probes").truthy() {
                    inner.push(format!(
                        "probe {} ms x{}",
                        round_div(p2.get("probe_us"), k),
                        fmt_d(p2.get("probes"))
                    ));
                }
                if gt(p2.get_or("passes", I0), Num::Int(1)) {
                    inner.push(format!(
                        "{} passes{}",
                        fmt_d(p2.get("passes")),
                        if res.get("density_floor").is_none() {
                            String::new()
                        } else {
                            format!(", floor {} px", fg(res.get("density_floor"), 2))
                        }
                    ));
                }
                if gt(p2.get_or("threads", Py::Int(1)), Num::Int(1)) {
                    inner.push(format!("{} threads", fmt_d(p2.get("threads"))));
                }
                inner.push(format!(
                    "nodes {}, reads {}, cell dots {}",
                    count(p2.get("nodes")),
                    count(p2.get_or("reads", I0)),
                    count(p2.get_or("items", I0))
                ));
                plan += &format!(" ({})", inner.join(", "));
            }
            brief.push(plan);
        }
        if pages.get("decoded").truthy() {
            brief.push(format!("{} pages decoded", fmt_d(pages.get("decoded"))));
        }
        if !over.is_empty() {
            brief.push(format!("pass 2 over budget: {}", over.join(", ")));
        }
        brief_stack = if brief.is_empty() {
            "density".into()
        } else {
            format!("density: {}", brief.join(", "))
        };
    }
    let mode = format!(
        "live{} ({} tiles, +{} new, {} ms{}{}{}{}{}{})",
        stack,
        fmt_d(res.get("tiles")),
        fmt_d(res.get_or("new", I0).or(I0)),
        fmt_d(res.get("ms")),
        split,
        depth_note,
        cut,
        drawn,
        refin,
        text
    );
    let mut brief = vec![format!("{} ms{}", fmt_d(res.get("ms")), brief_split)];
    if deck.truthy() {
        brief.push(format!(
            "deck {} passes{}",
            fmt_d(deck.get("passes")),
            if deck.get("summary_passes").truthy() {
                format!(
                    ", summary {} passes (not pickable)",
                    fmt_d(deck.get("summary_passes"))
                )
            } else {
                String::new()
            }
        ));
    }
    if !stack.is_empty() {
        brief.push(brief_stack);
    }
    if res.get("work_bin_items").truthy() {
        brief.push(format!("bin {} items", count(res.get("work_bin_items"))));
    } else if res.get("work_bin_overflow_items").truthy() {
        brief.push(format!(
            "bin off(cap@{}){}",
            count(res.get("work_bin_overflow_items")),
            if res.get("hier_cells_visited").truthy() {
                format!(
                    ", hier {}/{} pruned",
                    count(res.get("hier_cells_visited")),
                    count(res.get_or("subtrees_pruned", I0))
                )
            } else {
                String::new()
            }
        ));
    }
    let brief_cut = brief_cut.trim_matches(py_space);
    if !brief_cut.is_empty() {
        brief.push(brief_cut.to_string());
    }
    if res.get("over_budget_pages").truthy() {
        brief.push(format!(
            "{} pages over budget (not drawn)",
            fmt_d(res.get("over_budget_pages"))
        ));
    }
    if res.get("labels_truncated").truthy() {
        brief.push("labels partial".into());
    }
    if res.get("cache_evicted").truthy() {
        brief.push(format!("evict {}", count(res.get("cache_evicted"))));
    }
    if summ.get("layers").truthy() {
        brief.push(format!(
            "summary {} layers (not pickable)",
            fmt_d(summ.get("layers"))
        ));
    }
    (mode, brief.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_print_as_python_repr() {
        for (x, s) in [
            (1e-5, "1e-05"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (0.1, "0.1"),
            (1.0, "1.0"),
            (0.0001, "0.0001"),
            (-0.0, "-0.0"),
            (123456789012345680.0, "1.2345678901234568e+17"),
            (2.5e-300, "2.5e-300"),
            (f64::INFINITY, "inf"),
        ] {
            assert_eq!(repr_f64(x), s, "{x}");
        }
    }

    #[test]
    fn percent_g_is_python_s() {
        for (x, p, s) in [
            (0.0001234, 6, "0.0001234"),
            (1e-5, 6, "1e-05"),
            (100.0, 6, "100"),
            (1234567.0, 3, "1.23e+06"),
            (1000.0, 3, "1e+03"),
            (100.0, 3, "100"),
            (2.5, 1, "2"),
            (0.000125, 2, "0.00013"),
            (-0.0, 6, "-0"),
            (1.5, 6, "1.5"),
            (999.5, 3, "1e+03"),
            (123456789.0, 6, "1.23457e+08"),
        ] {
            assert_eq!(fmt_g(x, p), s, "%.{p}g of {x}");
        }
    }

    #[test]
    fn rounding_is_half_to_even() {
        // Python: round(2.5) == 2, round(3.5) == 4, round(0.0625, 3) == 0.062
        assert_eq!(round_f(2.5), 2);
        assert_eq!(round_f(3.5), 4);
        assert_eq!(round_f(-0.5), 0);
        assert_eq!(round_digits(0.0625, 3), 0.062);
        assert_eq!(round_digits(2.0005, 3), 2.001);
        assert_eq!(round_digits(1.0005, 3), 1.0);
        // load_ms = round(us / 1000): 2500 us -> 2, 3500 us -> 4
        let mut fields = BTreeMap::new();
        fields.insert("gen".to_string(), "1".to_string());
        fields.insert("final".to_string(), "1".to_string());
        fields.insert("plan_us".to_string(), "2500".to_string());
        fields.insert("raster_us".to_string(), "3500".to_string());
        let mut report = FrameReport::new(PerfJob::new([0.0, 0.0, 10.0, 10.0], 10, 10));
        let res = report.round(
            &fields,
            false,
            &PerfAdapter::default(),
            PerfTiming {
                adapter_read_us: 0,
                elapsed_ms: 12.5,
            },
        );
        assert_eq!(res["load_ms"], json!(2));
        assert_eq!(res["draw_ms"], json!(4));
        assert_eq!(res["ms"], json!(12));
        assert_eq!(res["plan_ms"], json!(2.5));
    }

    #[test]
    fn int_division_is_correctly_rounded() {
        // Python's values of (2**62+3)/7, (-(2**60)-7)/1000, (10**17+500)/1000
        assert_eq!(int_truediv((1 << 62) + 3, 7), 6.588122883467697e+17);
        assert_eq!(int_truediv(-(1 << 60) - 7, 1000), -1152921504606847.0);
        assert_eq!(
            int_truediv(100_000_000_000_000_500, 1000),
            100000000000000.5
        );
        assert_eq!(int_truediv(7, 2), 3.5);
    }

    #[test]
    fn counts_and_wire_values_read_as_python() {
        for (n, s) in [
            (json!(9999), "9999"),
            (json!(9999.9), "9999"),
            (json!(10000), "10k"),
            (json!(12500), "12k"),
            (json!(13500), "14k"),
            (json!(999999), "1000k"),
            (json!(1250000), "1.2M"),
            (json!(1e9), "1.0G"),
            (json!(true), "1"),
            (json!(-20000), "-20000"),
        ] {
            assert_eq!(fmt_count(&n), s, "{n}");
        }
        assert_eq!(parse_int("1_000"), Some(1000));
        assert_eq!(parse_int("+7"), Some(7));
        assert_eq!(parse_int("1__0"), None);
        assert_eq!(parse_int("-"), None);
        assert_eq!(parse_float("1_0.5"), Some(10.5));
        assert_eq!(parse_float("-"), None);
        assert_eq!(pow2_string(130), "1361129467683753853853498429727072845824");
        assert_eq!(float_int_string(1e30), "1000000000000000019884624838656");
    }

    #[test]
    fn a_frame_line_reads_as_the_status_line_says() {
        let res = json!({
            "tiles": 12, "new": 3, "ms": 250, "load_ms": 120, "phase_plan": 10,
            "phase_delta": 20, "phase_apply": 90, "draw_ms": 80, "other_ms": 201,
            "cut_um": 0.25, "plan_culls": {"fit_thin": 2, "fit_full_pct": 250},
            "refining": 1,
        });
        let (full, brief) = perf_status_with(&res, ", depth 3", false);
        assert_eq!(
            full,
            "live (12 tiles, +3 new, 250 ms = 120 load [10 plan+20 delta+90 apply] + 80 draw \
             + 201 other, depth 3, cut<0.25um 1/4 below x2.5 to fit budget, refining 1, \
             cut pages 0/pbvh 0/cbvh 0/cells 0, layer 0, washed 0, thin 0)"
        );
        assert_eq!(
            brief,
            "250 ms = 120 load + 80 draw + 201 other · cut<0.25um 1/4 below x2.5 to fit budget"
        );
    }
}
