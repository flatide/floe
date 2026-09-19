//! V4 hierarchy-preserving planner (rust/VFS_HIER.md par.2).
//!
//! ONE topo-rank min-heap sweep over the .ovm v2 metadata propagates
//! clip regions (localview, up to K boxes) per WsKey = (cell,
//! remaining-depth) and emits a pruned COPY of the source hierarchy:
//! working-set cells holding page selections, child instance edges
//! (arrays preserved as arrays, nested arrays intact - zero
//! expansion), and frame rects (+rep) at explicit depth boundaries.
//! Size-cut children are omitted until layer-aware proxies exist. The
//! flat plan/descend path stays alongside for A/B until M5.
//!
//! Correctness rule everywhere: over-inclusion is cost, omission is
//! a bug. Every conservative fallback (K-box merge, skew index bbox,
//! pts chunk-coarse, i128 saturation) only ever ADDS members.

use crate::{xf_bbox, ViewReq};
use floe_oasis::doc::Rep;
use floe_ovm::{bit_test, masks_intersect, BBox, Ovm, PBVH_NONE};
use floe_tiler::Xf;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};

/// remaining-depth sentinel: no depth truncation below this WC
pub const REM_FULL: u32 = u32::MAX;
/// A sub-cut wash must be able to stand for at least this share of
/// its footprint's pixels (members as one-pixel hairlines), else it
/// is withheld (Hier::wash_worth, field 2026-09-15). The bar is low
/// on purpose: a wash overstating a sparse array 100-fold is the
/// documented wide-view trade (a 1 um lattice on a 10 um pitch at
/// 2.5 um/px covers 6%), while a page of a few fiducial marks whose
/// bbox spans the mask (10^-5) must not become a block.
pub const WASH_MIN_COVERAGE: f64 = 1.0 / 256.0;
/// The coverage a HAIRLINE-cut page or node must reach for a wash
/// under the cull policy (2026-09-16, user: a wide view of a design
/// layout should show presence without the occupancy summary, whose
/// build takes an hour on the 9.8 GB chip): a line counts its length
/// in pixels, so three long lines in an otherwise empty page are
/// 3-4 % and stay exact, dense routing washes. FLOE_RUST_WASH_HAIR_
/// COVERAGE overrides (diagnostic).
pub const WASH_MIN_COVERAGE_HAIR: f64 = 1.0 / 8.0;

/// Representatives (the page frontier, user design 2026-09-17,
/// staged after the review of the same day; the level is a screen
/// DENSITY since the field of the same day): what the cut drops is
/// thinned to one item in 2^L instead of vanishing, L per item from
/// its ink against its screen area (density_level): the smallest L
/// with ink / 2^L <= rep_density x area, ink = members x max(1, min
/// side px) x max(1, long side px) - an upper bound on what its
/// members can paint - and area its box on screen. A dense array or
/// hairline field thins to the density's dot pattern, a sparse page
/// draws whole; zooming out shrinks the area four times per octave
/// with the members clamped at a pixel, so L climbs two and a quarter
/// survive (the user's "one in four"), and the survivors are nested
/// the way the frontier's lattice representatives are (rev 45):
/// S(L+1) is a subset of S(L) - what a wider view shows was shown at
/// every closer view, nothing pops in. Per item, so a pan changes
/// nothing (a global budget tried before let dense arrays fill their
/// pixels while sparse regions went empty, and moved with every pan).
/// The thinning is applied ONCE along the path: a page's level is
/// handed (less the decode budget's page level) to the raster, where a
/// record's members absorb min(., log2 members) and its index the
/// rest; a cut child-BVH subtree within the density's dot pitch
/// (1 / sqrt(density) px) is one dot at its centre and is not walked
/// (rep_node_dot), a wider one is walked to its leaves, where a cut
/// placement takes its footprint's level - members absorb min(L,
/// log2 members), the placement's index the rest - and a kept member
/// is a dot of the child's box (rep_dots). Levels are capped so 2^L
/// fits a u64.
pub const REP_LEVELS_MAX: u32 = 40;

/// levels below the cut: floor(log2((threshold / measure)^2)), 0 at or
/// above the cut, exact in integers
pub fn rep_level(measure: u64, threshold: u64) -> u32 {
    if threshold == 0 {
        return 0;
    }
    let m2 = (measure.max(1) as u128) * (measure.max(1) as u128);
    let t2 = (threshold as u128) * (threshold as u128);
    let mut l = 0u32;
    while (m2 << (l + 1)) <= t2 && l < REP_LEVELS_MAX {
        l += 1;
    }
    l
}

/// whether the item of `index` (within its run) is a representative
/// `level` levels below the cut: one in 2^level
pub fn rep_keeps(index: u64, level: u32) -> bool {
    level == 0 || index % (1u64 << level.min(REP_LEVELS_MAX)) == 0
}

/// whether a run [lo, hi) of indices (relative to `base`) holds a
/// representative at `level`; true when the run is unknown (hi <= lo)
fn rep_in_run(lo: u32, hi: u32, base: u32, level: u32) -> bool {
    if hi <= lo || level == 0 {
        return true;
    }
    let m = 1u64 << level.min(REP_LEVELS_MAX);
    let first = lo.saturating_sub(base) as u64;
    let last = hi.saturating_sub(base) as u64;
    let next = first.div_ceil(m) * m;
    next < last
}

/// the levels a repetition of `members` absorbs of `level`
pub fn member_levels(members: u64, level: u32) -> u32 {
    level.min(members.max(1).ilog2())
}

/// a grid thinned by `level` member levels: strides 2^ja x 2^jb with
/// ja + jb = level, balanced across the axes (an isotropic stipple),
/// each at most the axis' own log2, the axis that runs out of members
/// handing its remainder to the other - so one member in 2^level, a
/// 1-D array thinning along its length; member 0 stays, and the
/// survivors of level + 1 are among those of level
pub fn thin_grid(na: u64, nb: u64, va: (i64, i64), vb: (i64, i64), level: u32) -> (u64, u64, (i64, i64), (i64, i64)) {
    let la = na.max(1).ilog2();
    let lb = nb.max(1).ilog2();
    let ja = level.div_ceil(2).min(la);
    let jb = (level - ja).min(lb);
    let ja = (level - jb).min(la);
    let (sa, sb) = (1u64 << ja, 1u64 << jb);
    (
        na.div_ceil(sa),
        nb.div_ceil(sb),
        (va.0.saturating_mul(sa as i64), va.1.saturating_mul(sa as i64)),
        (vb.0.saturating_mul(sb as i64), vb.1.saturating_mul(sb as i64)),
    )
}

/// a points repetition thinned by `level` member levels: every
/// 2^level-th slot, slot 0 first (nested in level)
pub fn thin_pts(pts: &[(i64, i64)], level: u32) -> std::sync::Arc<[(i64, i64)]> {
    let step = 1usize << level.min(REP_LEVELS_MAX);
    pts.iter().step_by(step).copied().collect()
}

/// the verdict on a cut page under the sub-cut rules or as a representative
#[derive(Clone, Copy, PartialEq, Debug)]
enum SubCut {
    /// sparse: kept, its members drawn as hairline pixels
    Keep,
    /// dense: its footprint washed in the layer colour
    Wash,
    /// dropped (not washable, or beyond a per-plan budget)
    Drop,
}

fn hair_wash_coverage() -> f64 {
    static COVERAGE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *COVERAGE.get_or_init(|| {
        std::env::var("FLOE_RUST_WASH_HAIR_COVERAGE")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| *v > 0.0 && *v <= 1.0)
            .unwrap_or(WASH_MIN_COVERAGE_HAIR)
    })
}

/// Per-plan sub-cut budgets (HierOpts::sub_cut_sparse_px /
/// sub_cut_wash_px), screen pixels: about four screens of hairline
/// ink and sixteen screens of block fill on a 4 Mpx view - each a few
/// tens of ms of raster - so what the sub-cut rules add to a frame is
/// bounded whatever the chip holds. Provisional (2026-09-16) until the
/// field's perf line of the slow frame sets them.
pub const SUB_CUT_SPARSE_PX: f64 = 16.0e6;
pub const SUB_CUT_WASH_PX: f64 = 64.0e6;

fn env_mpx(name: &str, default_px: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| v * 1.0e6)
        .unwrap_or(default_px)
}

fn sub_cut_sparse_px() -> f64 {
    static PX: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *PX.get_or_init(|| env_mpx("FLOE_RUST_SUB_CUT_SPARSE_MPX", SUB_CUT_SPARSE_PX))
}

fn sub_cut_wash_px() -> f64 {
    static PX: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *PX.get_or_init(|| env_mpx("FLOE_RUST_SUB_CUT_WASH_MPX", SUB_CUT_WASH_PX))
}

/// Sub-cut boxes (ViewReq::sub_cut_box; field 2026-09-18/19: the keep
/// picture is Calibre-like except that what the size cut drops VANISHES -
/// on the synthetic MAIN01 one via layer is an empty screen from the fit
/// view to x4, where Calibre keeps every shape at a minimum size). What
/// the size cut drops stays as a BOX drawn from index metadata alone - no
/// page is decoded:
///   * a size-cut page, page-BVH node, child-BVH node or child placement
///     footprint at most `sub_cut_box_px` on screen in both axes is one
///     box on its layers, and nothing below it is visited;
///   * an array of sub-cut cells wider than a box is drawn member by
///     member, each as its own bbox; members run together along an axis only
///     where they really touch on screen (pitch <= member size, or under a
///     pixel). Review 2026-09-19: running together everything closer than a
///     box turned 0.5 px members at a 3 px pitch into one 89 x 89 px fill.
///     More than SUB_CUT_BOX_ARRAY_MAX members in view are strided from
///     index 0 (real positions, lower density; sub_cut_box_strided);
///   * a wider size-cut node is walked (the cut used to prune it whole),
///     so the walk - and the box count - is bound by the screen, about one
///     box per sub_cut_box_px^2 pixels of covered area per hierarchy level;
///   * a box is drawn on a layer only when something of that layer is
///     REALLY there within the requested depth (review 2026-09-19: a box
///     from the recursive layer mask showed shapes below a depth limit, and
///     a node mask sampled from 8 placements lost a layer held by one
///     placement in 32). A page's layer is exact. A child's layers are the
///     layers of the cells within the depth it is shown at (`cell_bits`:
///     the recursive mask at full depth, the own-shapes mask at the depth
///     boundary, else a memoized walk of its placements). A child-BVH node
///     takes the union over ALL placements below it, stopping early once
///     every visible layer the cell can hold is found (`sub_cut_box_reads`
///     counts the placements read);
///   * a box is ONE rect, on the topmost (in paint order) of the layers it
///     stands for - see box_layers;
///   * only with at most `sub_cut_box_layers` (16) layers visible: the boxes
///     are for the view that would otherwise be sparse. Measured on the
///     chip-geometry synthetic MAIN01 1/10 (2026-09-19, keep, detail high,
///     boxes off -> on): 16 layers light 183 K -> 896 K px at the fit view
///     for 0.04 -> 0.36 s; with 32 layers the screen is already 90 % lit
///     from x2 on, and with 128 or all 449 every pixel is lit from x4 on
///     while the frame goes 0.8 -> 4.1 s and 2.5 -> 5.9 s - the boxes of a
///     top layer are painted first and buy nothing. Beyond
///     `sub_cut_box_max` box rects the pass is planned a level coarser.
///     FLOE_RUST_SUB_CUT_BOX_LAYERS overrides (diagnostic; up to
///     LAYER_SET_MAX).
/// A box is as honest as its size: within sub_cut_box_px of something
/// real. Exact requests, probes, deck passes and the diagnostic sub-cut
/// wash / page representatives never take boxes.
pub const SUB_CUT_BOX_PX: f64 = 4.0;

/// A set of visible layers by PAINT RANK (bit k = the k-th visible layer in
/// ascending (layer, datatype) order - the order the viewer paints them,
/// bottom to top), up to LAYER_SET_MAX of them.
pub const LAYER_SET_MAX: usize = 512;

/// Every operation takes `n`, the words in use (the visible layers / 64,
/// rounded up): with a handful of layers visible - the common case - a set
/// is one word, and these run once per placement read.
#[derive(Clone, Copy)]
struct LayerSet([u64; LAYER_SET_MAX / 64]);

impl LayerSet {
    const EMPTY: LayerSet = LayerSet([0; LAYER_SET_MAX / 64]);

    #[inline]
    fn is_empty(&self, n: usize) -> bool {
        self.0[..n].iter().all(|word| *word == 0)
    }

    #[inline]
    fn same(&self, other: &LayerSet, n: usize) -> bool {
        self.0[..n] == other.0[..n]
    }

    #[inline]
    fn insert(&mut self, rank: usize) {
        self.0[rank / 64] |= 1 << (rank % 64);
    }

    #[inline]
    fn union(&mut self, other: &LayerSet, n: usize) {
        for (word, more) in self.0[..n].iter_mut().zip(other.0[..n].iter()) {
            *word |= *more;
        }
    }

    /// the topmost layer of the set
    #[inline]
    fn top(&self, n: usize) -> Option<usize> {
        self.0[..n].iter().enumerate().rev().find(|(_, word)| **word != 0).map(|(at, word)| at * 64 + 63 - word.leading_zeros() as usize)
    }
}

/// HierOpts::sub_cut_box_layers default; FLOE_RUST_SUB_CUT_BOX_LAYERS overrides (diagnostic).
fn sub_cut_box_layers() -> u32 {
    static N: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("FLOE_RUST_SUB_CUT_BOX_LAYERS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(SUB_CUT_BOX_LAYERS)
    })
}

/// HierOpts::sub_cut_box_px default; FLOE_RUST_SUB_CUT_BOX_PX overrides (diagnostic).
fn sub_cut_box_px() -> f64 {
    static PX: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *PX.get_or_init(|| {
        std::env::var("FLOE_RUST_SUB_CUT_BOX_PX")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(SUB_CUT_BOX_PX)
    })
}
pub const SUB_CUT_BOX_MAX: u64 = 2_000_000;
/// A plan that would pass sub_cut_box_max is planned again one LEVEL
/// coarser - boxes twice as large, arrays every second member - up to this
/// level: the whole view gets coarser evenly, where dropping the boxes past
/// the cap left the cells walked last with nothing (sub_cut_box_level).
pub const SUB_CUT_BOX_LEVELS: u32 = 3;
/// placements one pass may read to find the layers really present (about
/// 7 ns each); past it a node keeps what it found so far - true positives -
/// and the rest of its layers are unknown (sub_cut_box_unsure)
pub const SUB_CUT_BOX_READS: u64 = 64_000_000;
pub const SUB_CUT_BOX_LAYERS: u32 = 16;
/// member boxes one array placement may emit per layer before it is strided
pub const SUB_CUT_BOX_ARRAY_MAX: u64 = 1 << 18;

/// HierOpts::rep_decode_bytes default: 256 MiB
pub const REP_DECODE_BYTES: u64 = 256 << 20;

/// HierOpts::rep_density default: a quarter of the pixels of a dense
/// field - Calibre's dot pattern at a wide view
pub const REP_DENSITY: f64 = 0.25;

fn rep_density() -> f64 {
    static D: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *D.get_or_init(|| {
        std::env::var("FLOE_RUST_REP_DENSITY")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(REP_DENSITY)
    })
}

/// the smallest level with `items / 2^L <= budget` (0 when within)
pub fn level_for(items: u64, budget: u64) -> u32 {
    if budget == 0 || items <= budget {
        return 0;
    }
    let over = items.div_ceil(budget);
    (u64::BITS - (over - 1).leading_zeros()).min(REP_LEVELS_MAX)
}

/// Budget-fitted cut (field 2026-09-18: `thin keep` + detail high is the
/// picture closest to Calibre - denser, even - but a wide view, many layers
/// or a deep depth ended in "decoded generation budget exceeded"). The
/// planner estimates the decoded memory of the pages it selects and, when a
/// request would not fit its generation budget, plans at the FINEST coarser
/// cut that fits - the density is lowered, never a page dropped at random.
/// The candidate cuts (fit_rungs) are the requested cut x 2^(k/4), k = 1..24
/// (x64), plus the standard detail cuts (DETAIL_CUTS_PX) above it: a request
/// at detail high can therefore never end coarser than detail medium would
/// plan as asked (field 2026-09-18, second report: with half-octave rungs
/// high skipped from 2.83 px to 4 px where medium's 3 px fitted, and was
/// the FASTER and coarser of the two). The rung is found by bisection - a
/// pass that goes over is abandoned at once, a pass that fits is complete -
/// so a fitted plan costs about log2(26) passes. A keep request for which
/// no rung fits (long hairlines are never shed by the size cut) culls its
/// hairline pages and searches the same rungs again; if nothing fits at
/// all, the complete plan of the last rung is returned flagged `fit_over`
/// and the render reports the budget as before. Deterministic in the
/// request; exact requests (cut 0), deck passes and probes (decode_budget
/// 0) are never touched; FLOE_RUST_FIT_BUDGET=off is the kill switch.
///
/// The estimate (page_memory): a decoded page is its records as structs plus
/// the page index - measured 2026-09-18 on the synthetic MAIN01 at 173 B per
/// rect record - so 4096 + 192 per record; stored bytes beyond 12 per record
/// are point lists (polygon vertices, Pts offsets) and cost about six times
/// their stored size in memory. About a tenth above the rect measurement;
/// the post-decode check stays as the safety net for anything it misses.
///
/// Budget-fitted DENSITY (2026-09-19, field on the synthetic MAIN01: thin
/// keep showed nothing for five zoom steps from the fit view). Raising the
/// cut sheds whole size classes: where a view's shapes are one class, the
/// finest cut that fits selects NOTHING. A plan over its budget is now the
/// longest PREFIX that fits of its pages in one fixed priority order:
///   * size class first, largest first - the class of a page is the octave
///     of the largest cut that still selects it (absolute dbu, so a page's
///     class never moves with the view);
///   * within a class the van der Corput order of (sequence number in the
///     (cell, layer) run + cell + layer): the bit-reversed value ascending,
///     so any prefix of a class is an even sample of it, a longer prefix a
///     superset, and single-page runs of different cells and layers thin
///     like long runs do;
///   * the page index last.
/// The classes above the one the prefix ends in are complete, that class
/// is sampled, the classes below it are gone - the detail goes from the
/// finest class up, and the class that does not fit is thinned instead of
/// dropped (the empty screen). The order does not depend on the view or
/// the budget and the prefix is strict (it ends at the first page that
/// does not fit), so at a given cut NARROWING THE VIEW OR RAISING THE
/// BUDGET NEVER REMOVES A PAGE THAT STAYS IN VIEW. (Review 2026-09-19 of
/// 0.12.166: a 2^k sample topped up with the largest pages broke that -
/// a wide view kept pages {0, 1, 8}, the narrower one {0, 4, 8}.) Zooming
/// in lowers the cut and brings finer classes in, which all rank after
/// the pages already shown.
/// The passes only find the pages to rank: the requested cut first, then
/// the powers of two above it (whole classes); a pass is abandoned past
/// FIT_OVERSHOOT budgets, and when a completed pass leaves room the class
/// below it is planned to the end, because the prefix ends there.
/// FLOE_RUST_FIT_THIN=off restores the ladder above.
pub const FIT_OVERSHOOT: u64 = 8;
/// the cut may double this many times (x64, the ladder's reach)
pub const FIT_OCTAVES_MAX: u32 = 6;
pub const FIT_PAGE_FIXED: u64 = 4096;
pub const FIT_RECORD_BYTES: u64 = 192;
pub const FIT_RECORD_STORED: u64 = 12;
pub const FIT_POINT_FACTOR: u64 = 6;
/// quarter-octave rungs up to x64
pub const FIT_RUNGS: u32 = 24;
/// the viewer's detail levels (floe/service.py DETAIL_PX) as rungs
pub const DETAIL_CUTS_PX: [f64; 3] = [1.0, 3.0, 5.0];

pub fn page_memory(records: u32, stored: u32) -> u64 {
    let plain = records as u64 * FIT_RECORD_STORED;
    FIT_PAGE_FIXED + records as u64 * FIT_RECORD_BYTES + (stored as u64).saturating_sub(plain) * FIT_POINT_FACTOR
}

/// The cuts a budget-fitted plan may use, ascending, all above the request's.
pub fn fit_rungs(cut_dbu: i64, px_per_dbu: f64) -> Vec<i64> {
    let mut rungs: Vec<i64> = (1..=FIT_RUNGS)
        .map(|k| ((cut_dbu as f64) * 2f64.powf(k as f64 / 4.0)).round().min(i64::MAX as f64) as i64)
        .collect();
    if px_per_dbu > 0.0 && px_per_dbu.is_finite() {
        rungs.extend(DETAIL_CUTS_PX.iter().map(|px| (px / px_per_dbu).round() as i64));
    }
    rungs.retain(|&c| c > cut_dbu);
    rungs.sort_unstable();
    rungs.dedup();
    rungs
}

fn fit_thin_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FLOE_RUST_FIT_THIN").as_deref() != Ok("off"))
}

fn fit_budget_enabled() -> bool {
    static B: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *B.get_or_init(|| std::env::var("FLOE_RUST_FIT_BUDGET").as_deref() != Ok("off"))
}

fn rep_decode_bytes() -> u64 {
    static B: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *B.get_or_init(|| {
        std::env::var("FLOE_RUST_REP_DECODE_MB")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .map(|v| (v * 1048576.0) as u64)
            .unwrap_or(REP_DECODE_BYTES)
    })
}

/// A sub-cut page-BVH node whose extent is at most this many screen
/// pixels on both sides washes whole, as it always did (a block that
/// small overstates nothing anyone can see); a wider node walks on
/// so its pages decide by their own member coverage. Keeps the plan
/// from descending every sub-cut node of a wide deck view (field
/// 2026-09-15: the walk budget is per pass, hundreds of passes).
pub const WASH_WIDE_NODE_PX: f64 = 16.0;
/// Calibre-style frame size bands, judged on the MIN screen side
/// (however long the other side is, a short side demotes the box):
///   >= FRAME_WHITE_PX   white outline   (band 0, dt+0)
///   >= FRAME_GRAY_PX    gray  outline   (band 1, dt+1)
///   >= FRAME_FILL_PX    gray  fill      (band 2, dt+2)
///   else                gray  dotted    (band 3, dt+3) - the dotted
///     outline degrades to a few dots at ~4px and a single pixel
///     below, approximating Calibre's 4-dot / 1px marks.
/// Block names have no fill/dots form; they use band 0/1 only
/// (white above FRAME_WHITE_PX, gray below).
pub const FRAME_WHITE_PX: f64 = 25.0;
pub const FRAME_GRAY_PX: f64 = 9.0;
pub const FRAME_FILL_PX: f64 = 5.0;

/// classify a drawn frame box (WC-local rect) into a size band by
/// its min screen side. No screen scale (px_per_dbu <= 0, e.g.
/// probes/legacy) => band 0 (white outline), the pre-tone default.
pub fn frame_band(rect: &BBox, px_per_dbu: f64) -> u8 {
    if px_per_dbu <= 0.0 {
        return 0;
    }
    let mn = (rect.x1 - rect.x0)
        .max(0)
        .min((rect.y1 - rect.y0).max(0)) as f64
        * px_per_dbu;
    if mn >= FRAME_WHITE_PX {
        0
    } else if mn >= FRAME_GRAY_PX {
        1
    } else if mn >= FRAME_FILL_PX {
        2
    } else {
        3
    }
}

/// (cell index, remaining depth) - the working-set cell identity.
/// Full-depth views collapse to one key per cell (r >= height folds
/// to REM_FULL, par.2.5), finite depth grows at most depth+1
/// variants per cell.
pub type WsKey = (u32, u32);

#[derive(Clone, Debug)]
pub struct HierOpts {
    /// localview boxes kept per WsKey before least-waste merging
    pub k_boxes: usize,
    /// pts reps at or below this emit a full (rebased) rep - above
    /// it the visible subset is selected by chunk scan (par.2.3)
    pub pts_full_rep: u32,
    /// per-request cap on per-offset visibility tests; exhausted
    /// scans degrade to whole-chunk inclusion (never omission)
    pub pts_enum_budget: u64,
    /// plan-wide cap on emitted frame rects (flat parity)
    pub frame_cap: usize,
    /// M7 LOD density gate: swap a page for its merged variant
    /// when members > lod_k * on-screen px^2 of its visible part.
    /// 0.0 disables (and req.px_per_dbu == 0.0 always disables -
    /// probes are exact by construction).
    pub lod_k: f64,
    /// M7-C: pages spanning at most this many screen px in BOTH
    /// axes collapse to one layer rect (their bbox). Baked LOD
    /// variants cannot serve this zoom band: on small pages the
    /// members are LARGE relative to the 128-grid and pass through
    /// verbatim (sample9 fit view: 9216 swaps still shipped 8M
    /// records). 0.0 disables; px_per_dbu == 0.0 always disables.
    pub wash_px: f64,
    /// hairline cut factor (rev 41): everything the size cut
    /// touches also culls when the MIN side is under
    /// hairline * cut_dbu, however long the other side is - at a
    /// wide view a sub-pixel-thin wire is a 1px stroke that only
    /// builds walls. Pages use the v6 max_min field ("every record
    /// is thin"), folds/frames/names use the box min side.
    /// 0.0 disables; cut 0 disables naturally.
    /// Rev 45: FRAMES no longer take this cull when thin_lattice_um
    /// is on - boundary boxes follow the Calibre lattice ladder
    /// below instead.
    pub hairline: f64,
    /// rev 45 (Calibre alignment, frames only): a boundary box whose
    /// MIN side is under the cut no longer vanishes - deterministic
    /// representatives on a layout-fixed lattice of this pitch (um)
    /// survive, banded like any frame (gray fill / dotted), until
    /// BOTH sides are under the cut. The lattice is anchored to the
    /// owning cell's coordinates, so the surviving set is identical
    /// at every zoom (observed Calibre behavior). 0.0 restores the
    /// rev 41 hairline cull for frames.
    pub thin_lattice_um: f64,
    /// screen px of one lattice pitch below which the per-bin
    /// representative set demotes from the interval bounds (2 in a
    /// row, 4 corners in a 2D array) to a single one - the observed
    /// Calibre 2 -> 1 -> 0 ladder. px_per_dbu == 0 never demotes.
    pub thin_demote_px: f64,
    /// Sub-cut wash (ViewReq::sub_cut_wash): child-BVH nodes the size
    /// cut prunes are still WALKED to their placements (exact
    /// per-child layer masks) while this many nodes remain in the
    /// plan's budget; beyond it a pruned node washes as one coarse
    /// box on the owning cell's visible layers. Bounds the wide-view
    /// walk the rev 43 prune exists for (184M placements).
    pub sub_cut_walk_budget: u64,
    /// Per-plan budgets on what the sub-cut rules may ADD to a frame,
    /// both in screen pixels (field 2026-09-16: a 150 MB chip at
    /// thin:cull detail medium drew over 6 s at mid zoom, and
    /// FLOE_RUST_SUB_CUT_WASH=off restored the earlier speed).
    /// `sub_cut_sparse_px`: the ink estimate of sparse pages kept and
    /// sparse placements expanded (members x member px, each member
    /// at least one pixel - the wash_worth measure); beyond it a
    /// sparse item is DROPPED as the cull always did, never washed (a
    /// sparse footprint wash is the false block of 2026-09-15).
    /// `sub_cut_wash_px`: the visible footprint area of the washes,
    /// one per layer; beyond it a sub-cut item is dropped. Spent in
    /// walk order, so a plan stays deterministic. 0 drops every
    /// sparse item / every wash (A/B in the field). Defaults
    /// SUB_CUT_SPARSE_PX / SUB_CUT_WASH_PX; FLOE_RUST_SUB_CUT_SPARSE_MPX
    /// / FLOE_RUST_SUB_CUT_WASH_MPX override in Mpx (diagnostic).
    pub sub_cut_sparse_px: f64,
    pub sub_cut_wash_px: f64,
    /// sub-cut boxes (see SUB_CUT_BOX_PX): the largest box in screen px,
    /// the box rects a plan may emit, the visible layers it still boxes for
    pub sub_cut_box_px: f64,
    pub sub_cut_box_max: u64,
    pub sub_cut_box_layers: u32,
    pub sub_cut_box_reads: u64,
    /// the level of this pass (plan_hier_as_asked raises it, see SUB_CUT_BOX_LEVELS)
    pub sub_cut_box_level: u32,
    /// The page frontier's decode budget per plan, decoded bytes of
    /// the cut pages kept as representatives (a page is about 1 MiB
    /// decoded; the render cache holds 1 GiB): beyond it the plan is
    /// redone with the pages thinned one in 2^Lp by index (Lp the
    /// smallest that fits), the records inside taking the rest of
    /// their level. 0 = no budget. FLOE_RUST_REP_DECODE_MB overrides
    /// (diagnostic).
    pub rep_decode_bytes: u64,
    /// The page frontier's screen density: the ink a cut item's
    /// survivors may paint per pixel of its box on screen (0.25 = a
    /// dot pattern of a quarter of the pixels in a dense field). Sets
    /// every item's level (density_level). 0 = no thinning.
    /// FLOE_RUST_REP_DENSITY overrides (diagnostic).
    pub rep_density: f64,
    /// fit the plan to ViewReq::decode_budget by raising the cut (see
    /// FIT_SHIFTS_MAX); false = plan as asked (FLOE_RUST_FIT_BUDGET=off)
    pub fit_budget: bool,
    /// a plan over its budget lowers the density before the detail
    /// (FLOE_RUST_FIT_THIN=off: the cut ladder)
    pub fit_thin: bool,
    /// Field diagnosis (2026-09-10): record one ExplainRow per page,
    /// page-BVH node, child placement / child-BVH node and frame the
    /// walk judged INSIDE the view - kept, culled by size, hairline,
    /// washed, LOD-swapped, folded, omitted - so a vanished region
    /// can be traced to the rule that dropped it. Off by default
    /// (`floe-index plan --explain 1`).
    pub explain: bool,
}

/// One verdict of the walk (HierOpts::explain): what was judged
/// (`kind`: top / page / pbvh / cbvh / child / frame), the verdict,
/// the owning cell, the layer for pages, the record id (page id, BVH
/// node, placement index), the box in cell-local dbu, its size
/// metrics (page max_w / max_h / max_min, a box's w / h / min side)
/// and the member count for pages.
#[derive(Clone, Debug, PartialEq)]
pub struct ExplainRow {
    pub kind: &'static str,
    pub verdict: &'static str,
    pub cell: u32,
    pub layer_idx: Option<u32>,
    pub id: u64,
    pub bbox: BBox,
    pub w: u64,
    pub h: u64,
    pub min: u64,
    pub members: u64,
}

impl Default for HierOpts {
    fn default() -> HierOpts {
        HierOpts {
            k_boxes: 4,
            pts_full_rep: 8192,
            pts_enum_budget: 200_000,
            frame_cap: 200_000,
            lod_k: 4.0,
            wash_px: 2.0,
            hairline: 0.5,
            thin_lattice_um: 7.0,
            thin_demote_px: 14.0,
            sub_cut_walk_budget: 200_000,
            sub_cut_sparse_px: sub_cut_sparse_px(),
            sub_cut_wash_px: sub_cut_wash_px(),
            sub_cut_box_px: sub_cut_box_px(),
            sub_cut_box_max: SUB_CUT_BOX_MAX,
            sub_cut_box_layers: sub_cut_box_layers(),
            sub_cut_box_reads: SUB_CUT_BOX_READS,
            sub_cut_box_level: 0,
            rep_decode_bytes: rep_decode_bytes(),
            rep_density: rep_density(),
            fit_budget: fit_budget_enabled(),
            fit_thin: fit_thin_enabled(),
            explain: false,
        }
    }
}

/// one child edge of a working-set cell, in WC-local coordinates.
/// rep is emission-ready: One / full Grid / REBASED pts subset
/// (first offset (0,0) - the pool is Morton-ordered, so even a full
/// pts emission rebases onto its first slot).
#[derive(Clone, Debug, PartialEq)]
pub struct WsInst {
    pub child: WsKey,
    pub x: i64,
    pub y: i64,
    pub rot: u8,
    pub flip: bool,
    pub rep: Rep,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WsCell {
    pub key: WsKey,
    /// page directory indexes, sorted unique
    pub pages: Vec<u32>,
    /// per page (parallel to `pages`): the page frontier's levels its
    /// records are thinned by in the raster (0 = every record)
    pub page_levels: Vec<u8>,
    pub insts: Vec<WsInst>,
    /// depth-boundary child outlines: offset-0 member bbox in
    /// WC-local coords + the placement rep (arrays outline per
    /// member) + the size BAND (0..3, see frame_band; authored on
    /// frame_layer dt+band)
    pub frames: Vec<(BBox, Rep, u8)>,
    /// M7-C wash degrade: pages whose WHOLE screen image fits in
    /// wash_px x wash_px collapse to one bbox rect on their own
    /// layer (li, page bbox) - at that size any subset of the
    /// page's members paints the same pixel blob, and shipping
    /// geometry only builds a hairline wall no dither can thin
    pub washes: Vec<(u32, BBox)>,
    /// representative shapes of design.ovr (OVR2) in this cell's frame -
    /// the top cell only; drawn as the shapes they are, never as washes
    pub reps: Vec<(u32, crate::representatives::Prim)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HierStats {
    /// pages of summarized layers left unselected (ViewReq::page_skip)
    pub summary_pages: u64,
    pub wc_cells: u64,
    pub wc_variants: u64,
    pub inst_edges: u64,
    pub frame_rects: u64,
    pub visited_bvh: u64,
    pub cull_layer: u64,
    pub cull_size: u64,
    pub cull_page_size: u64,
    /// sub-cut washes emitted (pages, page-BVH nodes, child
    /// placements) and the coarse child-BVH node washes among them
    pub sub_cut_washes: u64,
    pub sub_cut_coarse: u64,
    /// sub-cut pages kept and child placements expanded instead of
    /// washed: the footprint is wider than the cut and its members,
    /// even as one-pixel hairlines, could not cover WASH_MIN_COVERAGE
    /// (1/256) of it - too few to justify a wash, cheap to draw, and
    /// Calibre shows them at every zoom (field 2026-09-15: two 140 x
    /// 4 um marks 137 mm apart washed as one 137 x 54 mm block)
    pub sub_cut_sparse: u64,
    /// sub-cut items the per-plan budgets dropped (field 2026-09-16, a
    /// 6 s mid-zoom draw): sparse pages / placements beyond
    /// HierOpts::sub_cut_sparse_px, washes beyond sub_cut_wash_px
    pub sub_cut_sparse_over: u64,
    pub sub_cut_wash_over: u64,
    /// sub-cut boxes (ViewReq::sub_cut_box): box rects emitted, the BVH
    /// nodes among their sources, boxes dropped beyond sub_cut_box_max,
    /// placements read to find the layers really present under a node or
    /// within a depth limit, arrays strided past SUB_CUT_BOX_ARRAY_MAX
    pub sub_cut_boxes: u64,
    pub sub_cut_box_nodes: u64,
    pub sub_cut_box_over: u64,
    pub sub_cut_box_reads: u64,
    pub sub_cut_box_strided: u64,
    /// node boxes whose scan ran out of sub_cut_box_reads (layers beyond the
    /// ones found are unknown), and the level the boxes were planned at
    pub sub_cut_box_unsure: u64,
    pub sub_cut_box_level: u32,
    /// representatives (the page frontier, ViewReq::page_reps): cut
    /// pages kept (sparse, drawn as pixels) / washed (dense), cut
    /// placements washed or expanded, BVH subtrees pruned because no
    /// index of theirs is a representative's
    pub rep_pages_kept: u64,
    pub rep_pages_washed: u64,
    pub rep_children: u64,
    pub rep_pruned: u64,
    /// decoded bytes of the representative pages kept, the page level
    /// (one page in 2^Lp by index) the decode budget forced, and
    /// whether the plan was redone for it
    pub rep_decode_bytes: u64,
    pub rep_page_level: u32,
    pub rep_replans: u32,
    /// the members of the cut items kept as representatives, the
    /// highest level any of them took, and the decoded bytes of every
    /// selected page (the decode budget is measured against what the
    /// generation may still hold)
    pub rep_items: u64,
    pub rep_level: u32,
    pub page_bytes: u64,
    /// budget-fitted cut: the estimated decoded memory of the selected
    /// pages (page_memory), the cut the plan was fitted to as a percentage
    /// of the requested one (0 = planned as asked), whether a keep request
    /// fell back to the hairline cull, the passes planned, and whether even
    /// the last rung was over (the render then reports the budget as before)
    pub fit_bytes: u64,
    pub fit_pct: u32,
    pub fit_cull: u32,
    pub fit_passes: u32,
    pub fit_over: bool,
    /// budget-fitted density: the class the prefix ends in keeps about one
    /// page in 2^fit_thin (0 = no class is sampled); the complete classes
    /// start at fit_full_pct percent of the requested cut (0 = none is
    /// complete); everything under fit_none_pct percent of it is gone (0 =
    /// no class was dropped whole)
    pub fit_thin: u32,
    pub fit_full_pct: u32,
    pub fit_none_pct: u32,
    /// dots emitted for representative cut placements (kept members
    /// in view, before the layer fan-out) and for cut child-BVH
    /// subtrees within the dot pitch (one each)
    pub rep_dots: u64,
    pub rep_node_dots: u64,
    /// pages selected whose every record is thin (max_min < hairline
    /// x cut): what the page hairline rule would have dropped
    pub thin_pages_kept: u64,
    pub culled_page_layer_roots: u64,
    pub culled_page_bvh_bbox: u64,
    pub culled_page_bvh_cut: u64,
    pub visited_page_bvh: u64,
    pub page_candidates: u64,
    pub pts_enumerated: u64,
    pub pts_fallback: u64,
    pub pts_offsets_scanned: u64,
    pub pts_selected: u64,
    pub pts_offsets_emitted: u64,
    pub pts_bytes_emitted: u64,
    pub grid_fallback_full: u64,
    pub kbox_merges: u64,
    /// pages swapped for their LOD variant by the density gate
    pub lod_swapped: u64,
    /// pages collapsed to a single layer-colored bbox rect (M7-C)
    pub washed_pages: u64,
    /// instance-BVH subtrees pruned by the v7 size annotations
    /// (every placement under them would have been size/hairline
    /// culled - rev 43)
    pub culled_bvh_size: u64,
    /// instance-BVH subtrees pruned because no cell placed below them
    /// holds a visible layer (the v8 node layer masks)
    pub culled_bvh_layer: u64,
    /// boundary records that entered the rev 45 thin-frame lattice
    /// path (min side under the cut, lattice representatives kept)
    pub thin_frames: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HierPlan {
    pub top: WsKey,
    /// sorted by key
    pub wcells: Vec<WsCell>,
    /// union of wcell pages, sorted unique
    pub pages: Vec<u32>,
    /// streaming priority aligned with `pages`: squared distance
    /// from the page bbox to the nearest localview-box CENTER of a
    /// WC that selected it, in that WC's LOCAL frame (0 = the
    /// screen-center ray hits the page; min across shared uses).
    /// Page bboxes are cell-local, so only a local-frame metric
    /// orders "center first" correctly under placement/rotation.
    pub page_prio: Vec<u64>,
    pub stats: HierStats,
    /// HierOpts::explain rows (empty otherwise).
    pub explain: Vec<ExplainRow>,
}

// ------------------------------------------------- K-box localview

fn area2(b: &BBox) -> i128 {
    if b.is_empty() {
        return 0;
    }
    (b.x1 - b.x0) as i128 * (b.y1 - b.y0) as i128
}

fn contains(a: &BBox, b: &BBox) -> bool {
    a.x0 <= b.x0 && a.y0 <= b.y0 && b.x1 <= a.x1 && b.y1 <= a.y1
}

/// K-box clip-region accumulator. Boxes are queried SEPARATELY (a
/// single union bbox would cover the void between two far-apart
/// appearances of a cell); over K the least-waste pair merges, ties
/// resolved by insertion order - same request, same plan, always.
#[derive(Clone, Debug, Default)]
struct KBox {
    boxes: Vec<BBox>,
}

impl KBox {
    fn add(&mut self, nb: BBox, k: usize, merges: &mut u64) {
        if nb.is_empty() {
            return;
        }
        for b in &self.boxes {
            if contains(b, &nb) {
                return;
            }
        }
        self.boxes.retain(|b| !contains(&nb, b));
        self.boxes.push(nb);
        while self.boxes.len() > k.max(1) {
            let (mut bi, mut bj, mut bw) = (0usize, 1usize, i128::MAX);
            for i in 0..self.boxes.len() {
                for j in (i + 1)..self.boxes.len() {
                    let mut u = self.boxes[i];
                    u.grow(&self.boxes[j]);
                    let w = area2(&u)
                        - area2(&self.boxes[i])
                        - area2(&self.boxes[j]);
                    if w < bw {
                        (bi, bj, bw) = (i, j, w);
                    }
                }
            }
            let b2 = self.boxes.remove(bj);
            self.boxes[bi].grow(&b2);
            *merges += 1;
        }
    }
}

// ------------------------------------------------ grid closed form

fn div_floor_i128(a: i128, b: i128) -> i128 {
    let q = a / b;
    let r = a % b;
    if r != 0 && ((r < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

fn div_ceil_i128(a: i128, b: i128) -> i128 {
    let q = a / b;
    let r = a % b;
    if r != 0 && ((r < 0) == (b < 0)) {
        q + 1
    } else {
        q
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridVis {
    Empty,
    /// inclusive index rectangle, clamped to [0,na) x [0,nb)
    Range { i0: i64, i1: i64, j0: i64, j1: i64 },
}

/// visible index rectangle of a grid rep against offset-region R
/// (member o = i*va + j*vb visible iff o in R). Exact for det != 0
/// (Cramer corners + floor/ceil) and for the writer's 1-D normal
/// forms; a zero-vector 2-D form prunes its nonzero axis exactly,
/// while other degenerate 2-D forms use a conservative full range.
/// Over-inclusion is cost, omission is a bug (par.2.3). All math
/// is i128: i64 products cannot overflow it.
pub fn grid_ranges(
    na: i64,
    nb: i64,
    va: (i64, i64),
    vb: (i64, i64),
    r: &BBox,
) -> GridVis {
    if r.is_empty() || na <= 0 || nb <= 0 {
        return GridVis::Empty;
    }
    let clamp = |i0: i128, i1: i128, j0: i128, j1: i128| -> GridVis {
        let i0 = i0.max(0);
        let i1 = i1.min(na as i128 - 1);
        let j0 = j0.max(0);
        let j1 = j1.min(nb as i128 - 1);
        if i0 > i1 || j0 > j1 {
            GridVis::Empty
        } else {
            GridVis::Range {
                i0: i0 as i64,
                i1: i1 as i64,
                j0: j0 as i64,
                j1: j1 as i64,
            }
        }
    };
    let det = va.0 as i128 * vb.1 as i128 - va.1 as i128 * vb.0 as i128;
    if det != 0 {
        let (mut i0, mut i1) = (i128::MAX, i128::MIN);
        let (mut j0, mut j1) = (i128::MAX, i128::MIN);
        for &(rx, ry) in &[
            (r.x0, r.y0),
            (r.x0, r.y1),
            (r.x1, r.y0),
            (r.x1, r.y1),
        ] {
            let ni =
                rx as i128 * vb.1 as i128 - ry as i128 * vb.0 as i128;
            let nj =
                va.0 as i128 * ry as i128 - va.1 as i128 * rx as i128;
            i0 = i0.min(div_floor_i128(ni, det));
            i1 = i1.max(div_ceil_i128(ni, det));
            j0 = j0.min(div_floor_i128(nj, det));
            j1 = j1.max(div_ceil_i128(nj, det));
        }
        return clamp(i0, i1, j0, j1);
    }
    // det == 0: the writer's 1-D normal forms are exact
    if nb == 1 {
        return match axis1d(na, va, r) {
            Some((a, b)) => clamp(a, b, 0, 0),
            None => GridVis::Empty,
        };
    }
    if na == 1 {
        return match axis1d(nb, vb, r) {
            Some((a, b)) => clamp(0, 0, a, b),
            None => GridVis::Empty,
        };
    }
    // A used zero vector is legal in field OASIS and means duplicate members,
    // not an unbounded array.  Keep the duplicate axis in full while pruning
    // the other axis exactly.  The renderer separately caps the resulting
    // visit count, so a corrupt enormous duplicate grid cannot hang it.
    if vb == (0, 0) {
        return match axis1d(na, va, r) {
            Some((a, b)) => clamp(a, b, 0, nb as i128 - 1),
            None => GridVis::Empty,
        };
    }
    if va == (0, 0) {
        return match axis1d(nb, vb, r) {
            Some((a, b)) => clamp(0, na as i128 - 1, a, b),
            None => GridVis::Empty,
        };
    }
    // Other collinear 2-D grids retain the conservative full range.
    clamp(0, na as i128 - 1, 0, nb as i128 - 1)
}

/// t in [0,n) with t*v inside r on both axes (v may be negative or
/// zero per component)
fn axis1d(n: i64, v: (i64, i64), r: &BBox) -> Option<(i128, i128)> {
    let mut lo = 0i128;
    let mut hi = n as i128 - 1;
    for (vc, a, b) in [(v.0, r.x0, r.x1), (v.1, r.y0, r.y1)] {
        if vc == 0 {
            if !(a <= 0 && 0 <= b) {
                return None;
            }
        } else if vc > 0 {
            lo = lo.max(div_ceil_i128(a as i128, vc as i128));
            hi = hi.min(div_floor_i128(b as i128, vc as i128));
        } else {
            lo = lo.max(div_ceil_i128(b as i128, vc as i128));
            hi = hi.min(div_floor_i128(a as i128, vc as i128));
        }
        if lo > hi {
            return None;
        }
    }
    Some((lo, hi))
}

/// offset bbox of the 4 index-rectangle corners (i128, saturated to
/// i64 - saturation only widens, which is safe)
fn grid_ovis(
    i0: i64,
    i1: i64,
    j0: i64,
    j1: i64,
    va: (i64, i64),
    vb: (i64, i64),
) -> BBox {
    let sat = |v: i128| {
        v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    };
    let mut b = BBox::EMPTY;
    for &(i, j) in &[(i0, j0), (i0, j1), (i1, j0), (i1, j1)] {
        let ox = sat(i as i128 * va.0 as i128 + j as i128 * vb.0 as i128);
        let oy = sat(i as i128 * va.1 as i128 + j as i128 * vb.1 as i128);
        b.grow(&BBox { x0: ox, y0: oy, x1: ox, y1: oy });
    }
    b
}

/// b (+) (-o): the region of offsets/anchors whose translate of `o`
/// still meets `b` (Minkowski with the negated box; saturating -
/// widening is safe)
fn minkowski_neg(b: &BBox, o: &BBox) -> BBox {
    if b.is_empty() || o.is_empty() {
        return BBox::EMPTY;
    }
    BBox {
        x0: b.x0.saturating_sub(o.x1),
        y0: b.y0.saturating_sub(o.y1),
        x1: b.x1.saturating_sub(o.x0),
        y1: b.y1.saturating_sub(o.y0),
    }
}

// ------------------------------------------------------ the sweep

/// `vis` with the summarized layers removed (ViewReq::page_skip).
pub fn vis_minus_skip(req: &ViewReq) -> Vec<u8> {
    if req.page_skip.is_empty() {
        return req.vis.clone();
    }
    req.vis
        .iter()
        .zip(req.page_skip.iter().chain(std::iter::repeat(&0u8)))
        .map(|(v, s)| v & !s)
        .collect()
}

/// The subtree mask of a request: `vis` minus `page_skip` when it
/// prunes summarized subtrees, else `vis`.
pub fn walk_vis(req: &ViewReq) -> Vec<u8> {
    if req.prune_skipped {
        vis_minus_skip(req)
    } else {
        req.vis.clone()
    }
}

pub fn plan_hier(v: &Ovm, req: &ViewReq, opts: &HierOpts) -> HierPlan {
    if !opts.fit_budget || req.decode_budget == 0 || req.cut_dbu <= 0 {
        return plan_hier_as_asked(v, req, opts, 0);
    }
    if opts.fit_thin {
        return plan_hier_thinned(v, req, opts);
    }
    let mut passes = 1u32;
    let asked = plan_hier_as_asked(v, req, opts, req.decode_budget);
    if !asked.stats.fit_over {
        let mut plan = asked;
        plan.stats.fit_passes = passes;
        return plan;
    }
    let rungs = fit_rungs(req.cut_dbu, req.px_per_dbu);
    // the finest rung that fits, keep first and then (a keep request only)
    // with the hairlines culled; bytes never grow with the cut, so bisect
    let ladders: &[bool] = if req.page_hairline { &[true] } else { &[false, true] };
    let mut attempt = req.clone();
    for &hairline in ladders {
        attempt.page_hairline = hairline;
        let (mut lo, mut hi) = (0usize, rungs.len());
        // the best plan so far and the cut it was planned at
        let mut best: Option<(HierPlan, i64)> = None;
        if hairline != req.page_hairline {
            // the requested cut itself, now with the hairline cull
            attempt.cut_dbu = req.cut_dbu;
            passes += 1;
            let plan = plan_hier_as_asked(v, &attempt, opts, req.decode_budget);
            if !plan.stats.fit_over {
                (best, hi) = (Some((plan, req.cut_dbu)), 0);
            }
        }
        while lo < hi {
            let mid = (lo + hi) / 2;
            attempt.cut_dbu = rungs[mid];
            passes += 1;
            let plan = plan_hier_as_asked(v, &attempt, opts, req.decode_budget);
            if plan.stats.fit_over {
                lo = mid + 1;
            } else {
                (best, hi) = (Some((plan, rungs[mid])), mid);
            }
        }
        if let Some((mut plan, cut)) = best {
            plan.stats.fit_pct = ((cut as f64 / req.cut_dbu as f64) * 100.0).round().max(100.0) as u32;
            plan.stats.fit_cull = (hairline && !req.page_hairline) as u32;
            plan.stats.fit_passes = passes;
            return plan;
        }
    }
    // nothing fits: the complete plan of the last rung, flagged, so the
    // render reports the budget exactly as it used to
    attempt.cut_dbu = rungs.last().copied().unwrap_or(req.cut_dbu);
    let mut plan = plan_hier_as_asked(v, &attempt, opts, 0);
    plan.stats.fit_over = true;
    plan.stats.fit_pct = ((attempt.cut_dbu as f64 / req.cut_dbu as f64) * 100.0).round().max(100.0) as u32;
    plan.stats.fit_cull = (!req.page_hairline) as u32;
    plan.stats.fit_passes = passes + 1;
    plan
}

/// Budget-fitted density (see FIT_OVERSHOOT).
fn plan_hier_thinned(v: &Ovm, req: &ViewReq, opts: &HierOpts) -> HierPlan {
    let limit = req.decode_budget.saturating_mul(FIT_OVERSHOOT);
    // the requested cut, then the powers of two above it: whole classes
    let mut cuts = vec![req.cut_dbu];
    let first = 1i64 << (64 - (req.cut_dbu as u64).leading_zeros()).min(62);
    cuts.extend((0..FIT_OCTAVES_MAX).map(|k| first.saturating_mul(1i64 << k)));
    let mut attempt = req.clone();
    let mut passes = 0u32;
    for at in 0..cuts.len() {
        attempt.cut_dbu = cuts[at];
        passes += 1;
        let mut plan = plan_hier_as_asked(v, &attempt, opts, limit);
        if plan.stats.fit_over {
            continue;
        }
        let bytes = unique_page_memory(v, &plan);
        if at == 0 && bytes <= req.decode_budget {
            plan.stats.fit_passes = passes;
            return plan;
        }
        let mut used = at;
        if at > 0 && bytes < req.decode_budget {
            // the classes of this cut fit with room to spare: the prefix ends
            // in the class below, which the pass before abandoned - plan it
            // to the end
            used = at - 1;
            attempt.cut_dbu = cuts[used];
            passes += 1;
            plan = plan_hier_as_asked(v, &attempt, opts, 0);
        }
        let hairline = if attempt.page_hairline { opts.hairline } else { 0.0 };
        if !thin_to_budget(v, &mut plan, hairline, req.cut_dbu, req.decode_budget) {
            break;
        }
        plan.stats.fit_pct = ((cuts[used] as f64 / req.cut_dbu as f64) * 100.0).round().max(100.0) as u32;
        if used > 0 {
            // the raised cut dropped what is under it
            plan.stats.fit_none_pct = plan.stats.fit_none_pct.max(plan.stats.fit_pct);
        }
        plan.stats.fit_passes = passes;
        return plan;
    }
    // not even the first page fits: the complete plan of the last cut,
    // flagged, so the render reports the budget exactly as it used to
    let mut plan = plan_hier_as_asked(v, &attempt, opts, 0);
    plan.stats.fit_over = true;
    plan.stats.fit_pct = ((attempt.cut_dbu as f64 / req.cut_dbu as f64) * 100.0).round().max(100.0) as u32;
    plan.stats.fit_passes = passes + 1;
    plan
}

/// The estimated decoded memory of a plan's pages, each counted once.
fn unique_page_memory(v: &Ovm, plan: &HierPlan) -> u64 {
    plan.pages.iter().map(|&pi| { let p = v.page(pi); page_memory(p.records, p.usize_) }).sum()
}

/// The largest cut that still selects the page: the size cut looks at the
/// longer side, the hairline cull (factor `hairline`, 0 = keep) at the
/// shorter one. Its octave is the page's size class.
pub fn fit_key(p: &floe_ovm::PageV, hairline: f64) -> u64 {
    let long = p.max_w.max(p.max_h);
    if hairline > 0.0 {
        long.min((p.max_min as f64 / hairline) as u64)
    } else {
        long
    }
}

/// The fixed priority of a page (see FIT_OVERSHOOT): ascending = first.
pub fn fit_priority(p: &floe_ovm::PageV, hairline: f64, page: u32) -> (std::cmp::Reverse<u32>, u32, u32) {
    let class = 63 - fit_key(p, hairline).max(1).leading_zeros();
    let phase = p.seq.wrapping_add(p.cell).wrapping_add(p.layer_idx);
    (std::cmp::Reverse(class), phase.reverse_bits(), page)
}

/// Keeps of a complete plan the longest prefix in `fit_priority` order that
/// `budget` holds. False when not even the first page fits.
fn thin_to_budget(v: &Ovm, plan: &mut HierPlan, hairline: f64, asked_cut: i64, budget: u64) -> bool {
    let metas: Vec<floe_ovm::PageV> = plan.pages.iter().map(|&pi| v.page(pi)).collect();
    let mem: Vec<u64> = metas.iter().map(|p| page_memory(p.records, p.usize_)).collect();
    let total: u64 = mem.iter().sum();
    // the pass summed per working cell; the generation holds a page once
    plan.stats.fit_bytes = total;
    if total <= budget {
        return true;
    }
    let prio: Vec<_> = metas.iter().zip(&plan.pages).map(|(p, &pi)| fit_priority(p, hairline, pi)).collect();
    let mut order: Vec<usize> = (0..metas.len()).collect();
    order.sort_by_key(|&i| prio[i]);
    let mut keep = vec![false; metas.len()];
    let (mut bytes, mut kept) = (0u64, 0usize);
    for &i in &order {
        if bytes + mem[i] > budget {
            break;
        }
        bytes += mem[i];
        keep[i] = true;
        kept += 1;
    }
    if kept == 0 {
        return false;
    }
    // the class the prefix ends in, what it keeps of it, and whether
    // anything lies below it
    let class_of = |i: usize| prio[i].0 .0;
    let edge = class_of(order[kept]);
    let in_edge = order.iter().filter(|&&i| class_of(i) == edge).count() as u64;
    let kept_edge = order[..kept].iter().filter(|&&i| class_of(i) == edge).count() as u64;
    let below = order.iter().any(|&i| class_of(i) < edge);
    let pct = |class: u32| (((1u64 << class.min(62)) as f64 / asked_cut.max(1) as f64) * 100.0).round().clamp(100.0, u32::MAX as f64) as u32;
    plan.stats.fit_thin = if kept_edge == 0 { 0 } else { (in_edge.div_ceil(kept_edge) as f64).log2().ceil().max(1.0) as u32 };
    plan.stats.fit_full_pct = order[..kept].iter().map(|&i| class_of(i)).filter(|&class| class > edge).min().map(pct).unwrap_or(0);
    plan.stats.fit_none_pct = if kept_edge == 0 {
        pct(edge + 1)
    } else if below {
        pct(edge)
    } else {
        0
    };
    let dropped: HashSet<u32> = plan.pages.iter().zip(&keep).filter(|(_, k)| !**k).map(|(&pi, _)| pi).collect();
    let mut page_bytes = 0u64;
    for cell in &mut plan.wcells {
        if !cell.page_levels.is_empty() {
            let mut levels = std::mem::take(&mut cell.page_levels).into_iter();
            let pages = &cell.pages;
            cell.page_levels = pages
                .iter()
                .filter_map(|pi| {
                    let level = levels.next().unwrap_or(0);
                    (!dropped.contains(pi)).then_some(level)
                })
                .collect();
        }
        cell.pages.retain(|pi| !dropped.contains(pi));
        page_bytes += cell.pages.iter().map(|&pi| v.page(pi).usize_ as u64).sum::<u64>();
    }
    let mut at = 0usize;
    plan.page_prio.retain(|_| {
        at += 1;
        keep[at - 1]
    });
    let mut at = 0usize;
    plan.pages.retain(|_| {
        at += 1;
        keep[at - 1]
    });
    plan.stats.page_bytes = page_bytes;
    plan.stats.fit_bytes = bytes;
    true
}

fn plan_hier_as_asked(v: &Ovm, req: &ViewReq, opts: &HierOpts, fit_limit: u64) -> HierPlan {
    let reps = req.page_reps && !req.sub_cut_wash && req.cut_dbu > 0;
    let mut plan = plan_hier_pass(v, req, opts, 0, fit_limit);
    // sub-cut boxes past their cap: the same pass one level coarser
    let mut level = opts.sub_cut_box_level;
    while plan.stats.sub_cut_box_over > 0 && !plan.stats.fit_over && level < SUB_CUT_BOX_LEVELS {
        level += 1;
        plan = plan_hier_pass(v, req, &HierOpts { sub_cut_box_level: level, ..opts.clone() }, 0, fit_limit);
    }
    plan.stats.sub_cut_box_level = level;
    if !reps || opts.rep_decode_bytes == 0 {
        return plan;
    }
    // the decode budget of the representative pages: the option, and
    // never more than half of what the generation's budget still
    // holds after the pages the plan selects anyway (field
    // 2026-09-17: the generation budget tripped with the option alone)
    let normal = plan.stats.page_bytes.saturating_sub(plan.stats.rep_decode_bytes);
    let mut budget = opts.rep_decode_bytes;
    if req.decode_budget > 0 {
        budget = budget.min(req.decode_budget.saturating_sub(normal) / 2).max(1);
    }
    if plan.stats.rep_decode_bytes > budget {
        // redo the plan with the pages thinned one in 2^Lp by index,
        // Lp the smallest level that fits (their records take the rest
        // of their level in the raster). Deterministic in the view.
        let page_level = level_for(plan.stats.rep_decode_bytes, budget);
        let mut again = plan_hier_pass(v, req, opts, page_level, fit_limit);
        again.stats.rep_replans = 1;
        plan = again;
    }
    plan
}

fn plan_hier_pass(v: &Ovm, req: &ViewReq, opts: &HierOpts, page_level: u32, fit_limit: u64) -> HierPlan {
    let structural_frontier = opts.frame_cap != 0;
    let mut h = Hier {
        v,
        req,
        opts,
        cut: req.cut_dbu.max(0) as u64,
        lv: HashMap::new(),
        heap: BinaryHeap::new(),
        out: BTreeMap::new(),
        pages_all: BTreeSet::new(),
        prio: HashMap::new(),
        st: HierStats::default(),
        pts_budget: opts.pts_enum_budget,
        px_per_dbu: req.px_per_dbu,
        lod_k: opts.lod_k,
        wash_px: opts.wash_px,
        explain: Vec::new(),
        explain_on: opts.explain,
        sub_cut_wash: req.sub_cut_wash && req.cut_dbu > 0,
        boxm: false,
        box_px: opts.sub_cut_box_px * (1u32 << opts.sub_cut_box_level.min(8)) as f64,
        box_stride: 1i64 << opts.sub_cut_box_level.min(8),
        reads_left: opts.sub_cut_box_reads,
        vis_layers: Vec::new(),
        cell_bits_memo: HashMap::new(),
        node_bits_memo: HashMap::new(),
        mask_bits_memo: HashMap::new(),
        vis_rank: HashMap::new(),
        set_words: 1,
        boxes_left: opts.sub_cut_box_max,
        reps: req.page_reps && !req.sub_cut_wash && req.cut_dbu > 0,
        rep_page_level: page_level,
        dot_lattice: HashSet::new(),
        fit_limit,
        page_levels: HashMap::new(),
        wash_walk_budget: opts.sub_cut_walk_budget,
        sparse_px_left: opts.sub_cut_sparse_px,
        wash_px_left: opts.sub_cut_wash_px,
        wash_nodes: HashSet::new(),
        sparse_edges: HashSet::new(),
        hair: (req.cut_dbu.max(0) as f64 * opts.hairline) as u64,
        page_hair: if req.page_hairline {
            (req.cut_dbu.max(0) as f64 * opts.hairline) as u64
        } else {
            0
        },
        walk_vis: walk_vis(req),
        wash_vis: vis_minus_skip(req),
        thin_dbu: if opts.thin_lattice_um > 0.0 {
            (opts.thin_lattice_um * v.unit).max(1.0) as u64
        } else {
            0
        },
        thin_demote: req.px_per_dbu > 0.0
            && opts.thin_lattice_um * v.unit * req.px_per_dbu
                < opts.thin_demote_px,
        thin_bins: HashSet::new(),
        frames_total: 0,
    };
    if req.sub_cut_box
        && req.cut_dbu > 0
        && req.px_per_dbu > 0.0
        && !req.sub_cut_wash
        && !req.page_reps
        && opts.sub_cut_box_px > 0.0
    {
        // the layers a box may take: visible and not drawn by a summary
        let layers: Vec<u32> = h
            .wash_vis
            .iter()
            .enumerate()
            .flat_map(|(at, &byte)| (0..8u32).filter(move |bit| byte & (1 << bit) != 0).map(move |bit| at as u32 * 8 + bit))
            // a request may set the padding bits of its last byte ("everything")
            .filter(|&idx| idx < v.n_layers)
            .collect();
        if !layers.is_empty() && layers.len() <= (opts.sub_cut_box_layers as usize).min(LAYER_SET_MAX) {
            // paint order: the viewer paints ascending (layer, datatype), the
            // index numbers layers by first appearance in the file
            let mut layers: Vec<(u32, u32, u32)> = layers.iter().map(|&idx| { let l = v.layer(idx); (l.layer, l.dt, idx) }).collect();
            layers.sort();
            h.boxm = true;
            h.vis_layers = layers.iter().map(|&(_, _, idx)| idx).collect();
            h.vis_rank = h.vis_layers.iter().enumerate().map(|(rank, &idx)| (idx, rank)).collect();
            h.set_words = h.vis_layers.len().div_ceil(64).max(1);
        }
    }
    let top_ci = v.top;
    let r0 = h.norm_r(
        top_ci,
        if req.depth == u32::MAX { REM_FULL } else { req.depth },
    );
    if v.n_cells > 0 {
        let tc = v.cell(top_ci);
        let w = (tc.rbbox.x1 - tc.rbbox.x0).max(0) as u64;
        let hh = (tc.rbbox.y1 - tc.rbbox.y0).max(0) as u64;
        if h.explain_on {
            let sub_cut = r0 == REM_FULL && !h.sub_cut_wash && w < h.cut && hh < h.cut;
            let rb = tc.rbbox;
            h.note("top", if sub_cut { "cull_size" } else { "keep" }, top_ci, None, 0, rb, w, hh, w.min(hh), 0);
        }
        // A finite, non-folded depth has a structural frontier even
        // when no design layer is selected. Full depth (including a
        // finite depth folded past the cell height) remains geometry-only.
        if ((r0 != REM_FULL && structural_frontier)
            || masks_intersect(v.bitset(tc.lmask_rec), &h.walk_vis))
            && !tc.rbbox.is_empty()
            && (r0 != REM_FULL || h.sub_cut_wash || !(w < h.cut && hh < h.cut))
        {
            let seed = req.view.intersect(&tc.rbbox);
            h.contribute((top_ci, r0), seed);
        }
    }
    while let Some(Reverse((_, ci, r))) = h.heap.pop() {
        h.expand(ci, r);
        if h.fit_limit > 0 && h.st.fit_bytes > h.fit_limit {
            // over the generation budget: this pass is abandoned and
            // plan_hier plans again with a coarser cut
            h.st.fit_over = true;
            break;
        }
    }
    let mut st = h.st;
    st.rep_page_level = page_level;
    st.wc_cells = h.out.len() as u64;
    st.wc_variants =
        h.out.keys().filter(|&&(_, r)| r != REM_FULL).count() as u64;
    let pages: Vec<u32> = h.pages_all.into_iter().collect();
    let page_prio: Vec<u64> = pages
        .iter()
        .map(|pi| {
            h.prio
                .get(pi)
                .map(|&d| d.min(u64::MAX as u128) as u64)
                .unwrap_or(u64::MAX)
        })
        .collect();
    HierPlan {
        top: (top_ci, r0),
        wcells: h.out.into_values().collect(),
        pages,
        page_prio,
        stats: st,
        explain: std::mem::take(&mut h.explain),
    }
}

/// Rev 46: world-space frame boxes for the MINIMAP - the exact set
/// a fit view at this plan's depth/cut draws, expanded from the WS
/// tree and thinned by a spatial round-robin keep. (The baked meta
/// frontier walked the raw hierarchy with DFS-order scan budgets
/// and left regional holes on 184M-placement chips; planning at
/// the canvas fit scale reuses the whole rev 41/43/45 ladder and
/// matches the main view by construction.) Deterministic: WS map
/// order, in-order rep enumeration, strict-greater replacement.
pub fn frontier_boxes(
    v: &Ovm,
    plan: &HierPlan,
    keep: usize,
) -> (Vec<([i64; 4], u8)>, bool) {
    const GRID: i128 = 64;
    /// per grid cell candidates kept while streaming
    const PER_CELL: usize = 4;
    /// expansion work guard (frame members + instance pushes).
    /// The second return value reports whether it ran dry: the
    /// walk stops SILENTLY at the budget, so on monster chips
    /// the DFS prefix bias can leave regional holes - callers
    /// log it (review 2026-08-18; the lazy-expansion redesign
    /// waits for an office-scale measurement).
    const BUDGET: u64 = 8_000_000;
    if v.n_cells == 0 || plan.wcells.is_empty() {
        return (Vec::new(), false);
    }
    let die = v.cell(v.top).rbbox;
    let (dw, dh) = (
        ((die.x1 - die.x0).max(1)) as i128,
        ((die.y1 - die.y0).max(1)) as i128,
    );
    let by_key: HashMap<WsKey, &WsCell> =
        plan.wcells.iter().map(|w| (w.key, w)).collect();
    let mut cells: BTreeMap<
        (i64, i64),
        Vec<(i128, [i64; 4], u8)>,
    > = BTreeMap::new();
    let mut budget = BUDGET;
    let mut stack: Vec<(WsKey, Xf)> =
        vec![(plan.top, Xf::identity())];
    while let Some((key, xf)) = stack.pop() {
        let wc = match by_key.get(&key) {
            Some(wc) => *wc,
            None => continue,
        };
        for (rect, rep, band) in &wc.frames {
            each_rep_offset(rep, &mut budget, &mut |ox, oy| {
                let m = BBox {
                    x0: rect.x0.saturating_add(ox),
                    y0: rect.y0.saturating_add(oy),
                    x1: rect.x1.saturating_add(ox),
                    y1: rect.y1.saturating_add(oy),
                };
                let wb = xf_bbox(&xf, &m);
                let bx = [wb.x0, wb.y0, wb.x1, wb.y1];
                let area = (wb.x1 - wb.x0).max(0) as i128
                    * (wb.y1 - wb.y0).max(0) as i128;
                let cx = (wb.x0 as i128 + wb.x1 as i128) / 2
                    - die.x0 as i128;
                let cy = (wb.y0 as i128 + wb.y1 as i128) / 2
                    - die.y0 as i128;
                let g = (
                    (cx * GRID / dw).clamp(0, GRID - 1) as i64,
                    (cy * GRID / dh).clamp(0, GRID - 1) as i64,
                );
                let cell = cells.entry(g).or_default();
                if cell.len() < PER_CELL {
                    cell.push((area, bx, *band));
                } else {
                    // replace the smallest strictly-smaller entry
                    // (deterministic: ties keep the earlier box)
                    let mut mi = 0;
                    for (i, it) in cell.iter().enumerate() {
                        if (it.0, it.1) < (cell[mi].0, cell[mi].1)
                        {
                            mi = i;
                        }
                    }
                    if area > cell[mi].0 {
                        cell[mi] = (area, bx, *band);
                    }
                }
            });
        }
        for inst in &wc.insts {
            each_rep_offset(
                &inst.rep,
                &mut budget,
                &mut |ox, oy| {
                    let t = Xf::place(
                        inst.x.saturating_add(ox),
                        inst.y.saturating_add(oy),
                        inst.rot,
                        inst.flip,
                    );
                    stack.push((inst.child, xf.compose(&t)));
                },
            );
        }
    }
    // biggest-first round-robin across grid cells (same fairness as
    // the final keep of the retired bake)
    for c in cells.values_mut() {
        c.sort_by_key(|&(a, bx, _)| (Reverse(a), bx));
    }
    let mut kept: Vec<(i128, [i64; 4], u8)> = Vec::new();
    let mut round = 0usize;
    'rr: loop {
        let mut added = false;
        for c in cells.values() {
            if let Some(&it) = c.get(round) {
                kept.push(it);
                added = true;
                if kept.len() >= keep {
                    break 'rr;
                }
            }
        }
        if !added {
            break;
        }
        round += 1;
    }
    kept.sort_by_key(|&(a, bx, _)| (Reverse(a), bx));
    (
        kept.into_iter().map(|(_, bx, band)| (bx, band)).collect(),
        budget == 0,
    )
}

/// in-order rep member offsets under a shared work budget
fn each_rep_offset(
    rep: &Rep,
    budget: &mut u64,
    f: &mut impl FnMut(i64, i64),
) {
    match rep {
        Rep::One => {
            if *budget > 0 {
                *budget -= 1;
                f(0, 0);
            }
        }
        Rep::Grid { na, nb, va, vb } => {
            'o: for j in 0..*nb as i64 {
                for i in 0..*na as i64 {
                    if *budget == 0 {
                        break 'o;
                    }
                    *budget -= 1;
                    f(
                        i.saturating_mul(va.0)
                            .saturating_add(j.saturating_mul(vb.0)),
                        i.saturating_mul(va.1)
                            .saturating_add(j.saturating_mul(vb.1)),
                    );
                }
            }
        }
        Rep::Pts(p) => {
            for &(x, y) in p.iter() {
                if *budget == 0 {
                    break;
                }
                *budget -= 1;
                f(x, y);
            }
        }
    }
}

/// squared distance from the center of `b` to the box `p`
/// (0 when the center lies inside)
fn dist2_center_to_box(b: &BBox, p: &BBox) -> u128 {
    let cx = (b.x0 as i128 + b.x1 as i128) / 2;
    let cy = (b.y0 as i128 + b.y1 as i128) / 2;
    let dx = (p.x0 as i128 - cx).max(cx - p.x1 as i128).max(0);
    let dy = (p.y0 as i128 - cy).max(cy - p.y1 as i128).max(0);
    (dx * dx + dy * dy) as u128
}

struct Hier<'a> {
    v: &'a Ovm,
    req: &'a ViewReq,
    opts: &'a HierOpts,
    cut: u64,
    lv: HashMap<WsKey, KBox>,
    /// (rank, ci, r) min-heap: every parent has a strictly smaller
    /// rank, so a key's localview is FINAL when it pops - one
    /// expansion per key, no fixpoint (par.2.3)
    heap: BinaryHeap<Reverse<(u32, u32, u32)>>,
    out: BTreeMap<WsKey, WsCell>,
    pages_all: BTreeSet<u32>,
    /// page -> min squared center distance (streaming priority)
    prio: HashMap<u32, u128>,
    st: HierStats,
    pts_budget: u64,
    frames_total: usize,
    px_per_dbu: f64,
    lod_k: f64,
    wash_px: f64,
    /// hairline threshold in dbu (hairline * cut_dbu)
    hair: u64,
    /// the same threshold for PAGES, 0 unless ViewReq::page_hairline
    page_hair: u64,
    /// `vis` for subtree decisions: minus `page_skip` when the request
    /// prunes summarized subtrees (M3), else `vis` itself
    walk_vis: Vec<u8>,
    /// `vis` minus `page_skip` always: a summarized layer never gets a
    /// sub-cut wash (its summary already says where it is)
    wash_vis: Vec<u8>,
    /// rev 45 thin-frame lattice pitch in dbu (0 = lattice off,
    /// frames fall back to the rev 41 hairline cull)
    thin_dbu: u64,
    /// one lattice pitch is under thin_demote_px on screen: keep a
    /// single representative per bin instead of the interval bounds
    thin_demote: bool,
    /// HierOpts::explain: the rows, and whether to record them
    explain: Vec<ExplainRow>,
    explain_on: bool,
    /// ViewReq::sub_cut_wash and its remaining walk budget
    sub_cut_wash: bool,
    wash_walk_budget: u64,
    /// ViewReq::sub_cut_box in force (see plan_hier_pass), the largest box
    /// in screen px, the visible layers a box may take and the box rects left
    boxm: bool,
    box_px: f64,
    /// arrays keep every box_stride-th member (the pass level)
    box_stride: i64,
    reads_left: u64,
    vis_layers: Vec<u32>,
    /// cell_bits / node_bits of this pass: (cell | node, remaining depth) ->
    /// which of `vis_layers` (bit k = vis_layers[k]) are really there
    cell_bits_memo: HashMap<(u32, u32), LayerSet>,
    node_bits_memo: HashMap<(u32, u32), LayerSet>,
    /// layer bitset index -> its visible layers, and layer index -> paint rank
    mask_bits_memo: HashMap<u32, LayerSet>,
    vis_rank: HashMap<u32, usize>,
    /// LayerSet words in use
    set_words: usize,
    boxes_left: u64,
    /// ViewReq::page_reps in force (cut on, sub-cut wash off)
    reps: bool,
    /// the page level of this pass (one representative page in 2^Lp)
    rep_page_level: u32,
    /// the dot-pitch lattice cells (cell frame) that already hold a
    /// node dot in the cell being walked
    dot_lattice: HashSet<(i64, i64)>,
    /// stop the pass once the selected pages' estimated memory passes this (0 = never)
    fit_limit: u64,
    /// the levels a kept representative page hands to the raster
    page_levels: HashMap<u32, u8>,
    /// remaining per-plan sub-cut budgets (HierOpts::sub_cut_sparse_px
    /// / sub_cut_wash_px), screen px
    sparse_px_left: f64,
    wash_px_left: f64,
    /// child-BVH nodes already washed coarsely (per expand)
    wash_nodes: HashSet<u32>,
    /// placements expanded because they were sparse (no wash could
    /// stand for them): place_edge lets them through the cell cut
    sparse_edges: HashSet<u64>,
    /// (child ci, bin x, bin y) bins already owning a representative
    /// - One/Pts thin frames dedupe here; cleared per expand (bins
    /// are WC-local coordinates)
    thin_bins: HashSet<(u32, i64, i64)>,
}

impl<'a> Hier<'a> {
    fn norm_r(&self, ci: u32, r: u32) -> u32 {
        if r == REM_FULL {
            return REM_FULL;
        }
        if self.v.n_cells == 0 {
            return r;
        }
        // r >= height: truncation is a no-op, fold to the sentinel
        // variant (par.2.5) - leaves always land on REM_FULL
        if r >= self.v.cell(ci).height {
            REM_FULL
        } else {
            r
        }
    }

    fn contribute(&mut self, key: WsKey, b: BBox) {
        if b.is_empty() {
            return;
        }
        let e = self.lv.entry(key).or_default();
        let first = e.boxes.is_empty();
        e.add(b, self.opts.k_boxes, &mut self.st.kbox_merges);
        if first {
            self.heap.push(Reverse((
                self.v.cell(key.0).topo_rank,
                key.0,
                key.1,
            )));
        }
    }

    fn expand(&mut self, ci: u32, r: u32) {
        let key = (ci, r);
        let boxes = self.lv.get(&key).expect("lv seeded").boxes.clone();
        let cell = self.v.cell(ci);
        let mut wc = WsCell {
            key,
            pages: Vec::new(),
            page_levels: Vec::new(),
            insts: Vec::new(),
            frames: Vec::new(),
            washes: Vec::new(),
            reps: Vec::new(),
        };
        // ---- own pages: (cell,layer) runs, layer roots skip whole,
        // per-box queries dedup into one sorted set
        let mut psel: BTreeSet<u32> = BTreeSet::new();
        for pri in cell.prange_start
            ..cell.prange_start + cell.prange_count
        {
            let pr = self.v.prange(pri);
            if !bit_test(&self.req.vis, pr.layer_idx as usize) {
                self.st.culled_page_layer_roots += 1;
                continue;
            }
            // an occupancy summary draws this layer (M2): its pages
            // are neither selected nor decoded; the walk goes on for
            // frames and the other layers
            if !self.req.page_skip.is_empty()
                && bit_test(&self.req.page_skip, pr.layer_idx as usize)
            {
                self.st.summary_pages += pr.page_count as u64;
                if self.explain_on {
                    for pi in pr.page_lo..pr.page_lo + pr.page_count {
                        let p = self.v.page(pi);
                        if boxes.iter().any(|b| p.bbox.intersects(b)) {
                            self.note_page("summary", ci, &p, pi);
                        }
                    }
                }
                continue;
            }
            if pr.pbvh_root == PBVH_NONE {
                for pi in pr.page_lo..pr.page_lo + pr.page_count {
                    self.st.page_candidates += 1;
                    let p = self.v.page(pi);
                    let size_cut = p.max_w < self.cut && p.max_h < self.cut;
                    if size_cut || p.max_min < self.page_hair {
                        let in_view = boxes.iter().any(|b| p.bbox.intersects(b));
                        // washable under the sub-cut rules (a size cut
                        // always, a hairline cut under cull with the
                        // stricter coverage; diagnostic) or as a
                        // representative (the page frontier: every cut
                        // page in view, one in 2^Lp by index under the
                        // decode budget, its records thinned by the
                        // rest of its level in the raster; a cell
                        // reached through a representative placement
                        // draws every page whole)
                        let rep = in_view
                            && self.reps
                            && rep_keeps((pi - pr.page_lo) as u64, self.rep_page_level);
                        let washable = in_view && (self.sub_cut_wash || rep);
                        if size_cut && in_view && self.box_page(&p, pi, &mut wc.washes, ci) {
                            self.st.cull_page_size += 1;
                            continue;
                        }
                        match self.sub_cut_verdict(&p, &boxes[..], size_cut, washable, rep) {
                            SubCut::Keep => {
                                // sparse: too few members for a wash to
                                // stand for them and cheap to draw - keep
                                // the page; its members render as
                                // hairline pixels, as Calibre shows them
                                if rep {
                                    let l = self.density_level(p.members, p.max_min, p.max_w.max(p.max_h), &p.bbox);
                                    self.st.rep_items = self.st.rep_items.saturating_add(p.members);
                                    self.st.rep_level = self.st.rep_level.max(l);
                                    let level = l.saturating_sub(self.rep_page_level);
                                    self.page_levels.insert(pi, level.min(255) as u8);
                                    self.st.rep_decode_bytes += p.usize_ as u64;
                                    self.st.rep_pages_kept += 1;
                                    self.note_page("rep_keep", ci, &p, pi);
                                } else {
                                    self.st.sub_cut_sparse += 1;
                                    self.note_page("keep_sparse", ci, &p, pi);
                                }
                                psel.insert(pi);
                            }
                            SubCut::Wash => {
                                self.st.cull_page_size += 1;
                                if rep {
                                    self.st.rep_pages_washed += 1;
                                    self.note_page("rep_wash", ci, &p, pi);
                                } else {
                                    self.st.sub_cut_washes += 1;
                                    self.note_page(if size_cut { "cull_size" } else { "cull_hair" }, ci, &p, pi);
                                }
                                wc.washes.push((p.layer_idx, p.bbox));
                            }
                            SubCut::Drop => {
                                self.st.cull_page_size += 1;
                                if in_view {
                                    self.note_page(if size_cut { "cull_size" } else { "cull_hair" }, ci, &p, pi);
                                }
                            }
                        }
                        continue;
                    }
                    if boxes.iter().any(|b| p.bbox.intersects(b)) {
                        psel.insert(pi);
                    }
                }
            } else {
                for b in &boxes {
                    self.walk_pbvh(
                        pr.pbvh_root,
                        b,
                        &mut psel,
                        ci,
                        pr.layer_idx,
                        pr.page_lo,
                        &mut wc.washes,
                    );
                }
            }
        }
        // M7 density gate, two conditions on WHOLE-page metrics
        // (review finding: comparing whole-page members against
        // the px^2 of a partial intersection made LOD fire MORE as
        // you zoomed IN - the exact inversion of the contract):
        //  (a) fidelity precondition: one LOD grid cell must be at
        //      or below ONE SCREEN PIXEL in both axes, i.e. the
        //      page spans <= LOD_GRID px - merged cells can never
        //      appear as visible blocks;
        //  (b) worth: members must outnumber the page's own screen
        //      pixels lod_k-fold. Zooming in grows the page's
        //      on-screen size, fails (a), and exact returns.
        let mut sel: BTreeSet<u32> = BTreeSet::new();
        for &pi in &psel {
            let p = self.v.page(pi);
            // M7-C wash: a page whose whole image is at most
            // wash_px in both axes is ONE blob on screen - ship
            // its bbox as a single rect on its own layer instead
            // of geometry (baked variants keep small pages'
            // relatively-large members verbatim and cannot thin
            // them; strokes bypass the speckle, so exact geometry
            // saturates into a solid wall)
            // (never a representative page: the page frontier draws it
            // thinned to the density - a bbox rect here was the solid
            // 2 x 2 px square the field saw as boxes at the fit view)
            if self.wash_px > 0.0 && self.px_per_dbu > 0.0 && !self.page_levels.contains_key(&pi) {
                let pw = (p.bbox.x1 - p.bbox.x0).max(0) as f64
                    * self.px_per_dbu;
                let ph = (p.bbox.y1 - p.bbox.y0).max(0) as f64
                    * self.px_per_dbu;
                if pw <= self.wash_px && ph <= self.wash_px {
                    wc.washes.push((p.layer_idx, p.bbox));
                    self.st.washed_pages += 1;
                    self.note_page("wash", ci, &p, pi);
                    continue;
                }
            }
            let mut eff = pi;
            if self.lod_k > 0.0
                && self.px_per_dbu > 0.0
                && p.lod_page != floe_ovm::LOD_PAGE_NONE
            {
                let (ew, eh) = (
                    (p.bbox.x1 - p.bbox.x0).max(0),
                    (p.bbox.y1 - p.bbox.y0).max(0),
                );
                let g = floe_ovm::LOD_GRID;
                // fidelity is judged on the REAL emitted cell size:
                // a merged rect is never thinner than 1 DBU, so on
                // tiny pages ceil(extent/grid) collapses to 1 DBU
                // and px_per_dbu > 1 must keep exact (review
                // finding - the page<=128px form missed this)
                let cw = ((ew + g - 1) / g).max(1) as f64
                    * self.px_per_dbu;
                let ch = ((eh + g - 1) / g).max(1) as f64
                    * self.px_per_dbu;
                let (pw, ph) = (
                    ew as f64 * self.px_per_dbu,
                    eh as f64 * self.px_per_dbu,
                );
                if cw <= 1.0
                    && ch <= 1.0
                    && p.members as f64
                        > self.lod_k * (pw * ph).max(1.0)
                {
                    eff = p.lod_page;
                    self.st.lod_swapped += 1;
                }
            }
            // a thin page (every record under the hairline threshold)
            // that the page hairline rule would have dropped
            let thin = self.hair > 0 && p.max_min < self.hair;
            if thin {
                self.st.thin_pages_kept += 1;
            }
            self.note_page(
                match (eff == pi, thin) {
                    (true, false) => "exact",
                    (true, true) => "exact_thin",
                    (false, false) => "lod",
                    (false, true) => "lod_thin",
                },
                ci,
                &p,
                pi,
            );
            sel.insert(eff);
        }
        for &pi in &sel {
            let pb = self.v.page(pi).bbox;
            let d = boxes
                .iter()
                .map(|b| dist2_center_to_box(b, &pb))
                .min()
                .unwrap_or(u128::MAX);
            self.prio
                .entry(pi)
                .and_modify(|e| *e = (*e).min(d))
                .or_insert(d);
        }
        self.pages_all.extend(sel.iter().copied());
        wc.pages = sel.into_iter().collect();
        wc.page_levels = wc.pages.iter().map(|p| self.page_levels.get(p).copied().unwrap_or(0)).collect();
        for &p in &wc.pages {
            let page = self.v.page(p);
            self.st.page_bytes = self.st.page_bytes.saturating_add(page.usize_ as u64);
            self.st.fit_bytes = self.st.fit_bytes.saturating_add(page_memory(page.records, page.usize_));
        }
        // ---- children (r = 0: depth exhausted - children render
        // as outline frames; own pages above carry the geometry)
        if cell.bvh_count != 0 {
            // Inline triage during the walk (field fix, 9.8G class:
            // 184M placements). Collecting every visible instance
            // into a set BEFORE cut classification made a wide view
            // O(visible instances) in MEMORY - tens of millions of
            // entries - before a single frame emitted. Below-cut
            // children now resolve inline with no-alloc accessors;
            // only ABOVE-cut edges (few, bounded by real content)
            // are gathered for the multi-box contribution pass.
            let cut = self.cut;
            // the visible layers this cell can hold at all: a node scan for
            // the sub-cut boxes stops once it has found them
            let box_upper = if self.boxm { self.vis_bits(self.v.cell_lmask_rec(ci)) } else { LayerSet::EMPTY };
            // rev 45: at the r==0 boundary with the thin lattice on,
            // hairline-thin subtrees must still be WALKED - their
            // boxes are no longer culled but sampled. The both-dims
            // prune (max_dim, the fit-view dust killer) stays; only
            // the max_min term is depth-gated.
            let hair_prune = if r == 0 && self.thin_dbu > 0 {
                0
            } else {
                self.hair
            };
            self.thin_bins.clear();
            self.wash_nodes.clear();
            self.sparse_edges.clear();
            self.dot_lattice.clear();
            // the page frontier's run for this cell's placements (the
            // child-BVH root's box) and the per-node heaviest-member
            // table it prunes with
            let mut edges: BTreeSet<u64> = BTreeSet::new();
            let mut framed: HashSet<u64> = HashSet::new();
            for b in &boxes {
                let mut stack = vec![cell.bvh_start];
                while let Some(ni) = stack.pop() {
                    let node = self.v.bvh(ni);
                    self.st.visited_bvh += 1;
                    // v8 layer masks: no cell placed below holds a visible
                    // layer - what the per-placement cull_layer test would
                    // find out one placement at a time. Hierarchy frames
                    // are drawn whatever the layers, so only where none
                    // can come (full depth, or frames off).
                    if node.lmask_rec != floe_ovm::LMASK_UNKNOWN
                        && (r == REM_FULL || self.opts.frame_cap == 0)
                        && !masks_intersect(self.v.bitset(node.lmask_rec), &self.walk_vis)
                    {
                        self.st.culled_bvh_layer += 1;
                        continue;
                    }
                    // rev 43: v7 size annotations - a subtree whose
                    // every child cell is under the cut (or
                    // hairline-thin) prunes wholesale; the fit-view
                    // walk stops paying O(boundary placements) for
                    // boxes the cut was always going to drop (150M
                    // field case: 4.5s plan for 1023 boxes)
                    if (node.max_dim as u64) < cut
                        || (node.max_min as u64) < hair_prune
                    {
                        self.st.culled_bvh_size += 1;
                        if self.explain_on && node.bbox.intersects(b) {
                            let nb = node.bbox;
                            let (md, mm) = (node.max_dim as u64, node.max_min as u64);
                            self.note("cbvh", "prune_size", ci, None, ni as u64, nb, md, md, mm, node.count as u64);
                        }
                        // the page frontier: a cut subtree no wider on
                        // screen than the density's dot pitch is ONE
                        // dot - a pixel at its centre on this cell's
                        // visible layers (rep_node_dot); nothing below
                        // it is visited. A wider one is walked to its
                        // children; its leaves' placements take their
                        // own footprint's level (place_rep). The walk
                        // costs the nodes wider than the pitch in view -
                        // bounded by the screen, never by the placements
                        // below (field 2026-09-17: descending every cut
                        // subtree took over 80 s at full depth).
                        // sub-cut boxes: a size-cut node no wider than
                        // a box is one box; a wider one is walked so
                        // its placements decide by their footprints
                        let box_descend = self.boxm
                            && r != 0
                            && (node.max_dim as u64) < cut
                            && node.bbox.intersects(b)
                            && {
                                if self.box_small(&node.bbox) {
                                    self.box_node(&mut wc, ni, &node.bbox, r, box_upper, (node.lmask_rec, node.lmask_direct));
                                    false
                                } else {
                                    true
                                }
                            };
                        let rep_descend = box_descend || !self.sub_cut_wash
                            && self.reps
                            && r != 0
                            && node.bbox.intersects(b)
                            && {
                                if self.within_dot_pitch(&node.bbox) {
                                    self.rep_node_dot(&mut wc, ni, &node.bbox);
                                    false
                                } else {
                                    true
                                }
                            };
                        if !rep_descend {
                        if !self.sub_cut_wash || !node.bbox.intersects(b) {
                            continue;
                        }
                        // sub-cut wash (jobdeck wide view): walk the
                        // pruned subtree to its placements while the
                        // budget lasts - each washes its footprint on
                        // the layers its child really holds - and
                        // beyond it wash the node's whole extent on
                        // this cell's visible layers (coarse). At
                        // r == 0 the children are outlines only
                        // (depth exhausted): no wash either way.
                        if r == 0 {
                            continue;
                        }
                        if self.wash_walk_budget > 0 {
                            self.wash_walk_budget -= 1;
                        } else {
                            if self.wash_nodes.insert(ni) {
                                let fp = node.bbox;
                                let mask = self.v.cell_lmask_rec(ci);
                                if self.wash_layers(&mut wc, mask, fp, std::slice::from_ref(b), true) {
                                    self.st.sub_cut_coarse += 1;
                                }
                            }
                            continue;
                        }
                        }
                    }
                    if !node.bbox.intersects(b) {
                        continue;
                    }
                    if !node.leaf {
                        for k in 0..node.count as u32 {
                            stack.push(node.first + k);
                        }
                        continue;
                    }
                    for k in 0..node.count as u64 {
                        let pli = node.first as u64 + k;
                        if edges.contains(&pli)
                            || framed.contains(&pli)
                        {
                            continue;
                        }
                        let h = self.v.place_head(pli);
                        let rb = self.v.cell_rbbox(h.child);
                        if rb.is_empty() {
                            continue;
                        }
                        let cw = (rb.x1 - rb.x0).max(0) as u64;
                        let chh = (rb.y1 - rb.y0).max(0) as u64;
                        // r == 0: depth is exhausted - every child
                        // renders as a structural outline,
                        // independent of design-layer visibility
                        // (review finding: a pure-hierarchy top
                        // opened at the depth-0 default as a black
                        // screen; "top geometry + outlines" now
                        // means what it says). Rev 39: the box
                        // itself takes the size cut like geometry
                        // (Calibre size-cuts its cell boxes), on
                        // the MEMBER box - sub-cut boxes vanish
                        // instead of merging; the gate lives in
                        // frame_depth_boundary.
                        if r == 0 {
                            if self.frames_total
                                < self.opts.frame_cap
                            {
                                framed.insert(pli);
                                self.frame_depth_boundary(
                                    &mut wc, pli, &h, &rb, &boxes,
                                );
                            }
                            continue;
                        }
                        // A finite remaining depth walks structure to
                        // the requested frontier - but a sub-cut
                        // subtree FOLDS HERE: none of its pages can
                        // pass the cut at any depth, and its own
                        // frontier frames would be sub-pixel, so
                        // descending is pure plan/delta/parse cost
                        // (150M field case: 4,055,904 edges vs 663
                        // for the identical drawable page set). One
                        // outline now (frames on) or nothing.
                        if r != REM_FULL {
                            let size_cut = cw < cut && chh < cut;
                            if size_cut || cw.min(chh) < self.hair {
                                // rev 33: the fold is SILENT. A
                                // fold box tracked the cut - it
                                // appeared and vanished with zoom
                                // and could never carry a name.
                                // Calibre's box set is a pure
                                // function of depth and so is ours
                                // now: the r==0 boundary is the
                                // only frame source. The descent
                                // stays culled (rev 31's point);
                                // only the proxy box is gone,
                                // matching the depth-full omission
                                // rule.
                                // the page frontier: a representative
                                // placement is EXPANDED with its members
                                // thinned (one in 4^k over members and
                                // placement index, place_rep), never
                                // washed as its footprint
                                if !self.sub_cut_wash && self.reps {
                                    if let Some(lm) = self.place_rep(pli, &h, &rb, cell.place_start) {
                                        self.st.rep_children += 1;
                                        self.note_child("rep_dots", pli, &h, &rb, &boxes);
                                        self.rep_dots(&mut wc, pli, &h, &rb, &boxes, lm);
                                        self.st.cull_size += 1;
                                        framed.insert(pli);
                                        continue;
                                    }
                                }
                                if self.sub_cut_wash
                                    && !self.wash_sub_cut_child(
                                        &mut wc, pli, &h, &rb, &boxes, self.hair_cut(size_cut),
                                    )
                                {
                                    // sparse (no wash could stand for
                                    // it): walk it - few members, and
                                    // Calibre shows them at every zoom
                                    self.st.sub_cut_sparse += 1;
                                    self.note_child("expand_sparse", pli, &h, &rb, &boxes);
                                    self.sparse_edges.insert(pli);
                                    edges.insert(pli);
                                    continue;
                                }
                                if self.boxm && size_cut {
                                    self.box_child(&mut wc, pli, &h, &rb, &boxes, r);
                                }
                                self.st.cull_size += 1;
                                framed.insert(pli);
                                self.note_child("fold_size", pli, &h, &rb, &boxes);
                                continue;
                            }
                            if self.opts.frame_cap != 0
                                || masks_intersect(
                                    self.v.bitset(
                                        self.v.cell_lmask_rec(h.child),
                                    ),
                                    &self.walk_vis,
                                )
                            {
                                // rev 37 (final contract): a box
                                // lives ONLY at its own depth
                                // boundary, exactly like its name -
                                // an expanded child never carries
                                // an outline, whether its structure
                                // bottoms out here or not (the rev
                                // 34/36 persistence experiments are
                                // withdrawn: a depth-1 box must not
                                // survive to depth 2, field
                                // decision).
                                edges.insert(pli);
                                self.note_child("expand", pli, &h, &rb, &boxes);
                            } else {
                                self.st.cull_layer += 1;
                                self.note_child("cull_layer", pli, &h, &rb, &boxes);
                            }
                            continue;
                        }
                        if !masks_intersect(
                            self.v.bitset(
                                self.v.cell_lmask_rec(h.child),
                            ),
                            &self.walk_vis,
                        ) {
                            self.st.cull_layer += 1;
                            self.note_child("cull_layer", pli, &h, &rb, &boxes);
                            continue;
                        }
                        // A size cut is a detail omission, not a
                        // hierarchy-depth boundary. cell_rbbox is an
                        // all-layer recursive box; drawing it as a
                        // visible-layer proxy produced unrelated,
                        // die-spanning gray lines and repeated-array
                        // stripes. Until a per-(cell,layer) proxy is
                        // available, omit the below-cut child instead
                        // of displaying false geometry.
                        let size_cut = cw < cut && chh < cut;
                        if size_cut || cw.min(chh) < self.hair {
                            if !self.sub_cut_wash && self.reps {
                                if let Some(lm) = self.place_rep(pli, &h, &rb, cell.place_start) {
                                    self.st.rep_children += 1;
                                    self.note_child("rep_dots", pli, &h, &rb, &boxes);
                                    self.rep_dots(&mut wc, pli, &h, &rb, &boxes, lm);
                                    self.st.cull_size += 1;
                                    continue;
                                }
                            }
                            if self.sub_cut_wash
                                && !self.wash_sub_cut_child(
                                    &mut wc, pli, &h, &rb, &boxes, self.hair_cut(size_cut),
                                )
                            {
                                // sparse (no wash could stand for it):
                                // expand - few members, and Calibre
                                // shows them at every zoom
                                self.st.sub_cut_sparse += 1;
                                self.note_child("expand_sparse", pli, &h, &rb, &boxes);
                                self.sparse_edges.insert(pli);
                                edges.insert(pli);
                                continue;
                            }
                            if self.boxm && size_cut {
                                self.box_child(&mut wc, pli, &h, &rb, &boxes, r);
                            }
                            self.st.cull_size += 1;
                            self.note_child(
                                if cw < cut && chh < cut { "omit_size" } else { "omit_hair" },
                                pli, &h, &rb, &boxes,
                            );
                            continue;
                        }
                        self.note_child("expand", pli, &h, &rb, &boxes);
                        edges.insert(pli);
                    }
                }
            }
            for pli in edges {
                self.place_edge(&mut wc, r, pli, &boxes);
            }
        }
        self.out.insert(key, wc);
    }

    /// Sub-cut wash of one child placement the size cut dropped: its
    /// whole footprint (the repetition extent, one rect) on every
    /// visible layer of the child's recursive layer mask, when the
    /// footprint meets a view box.
    /// At most box_px on screen in both axes: one box.
    fn box_small(&self, fp: &BBox) -> bool {
        let ppd = self.px_per_dbu;
        let fw = (fp.x1 - fp.x0).max(0) as f64 * ppd;
        let fh = (fp.y1 - fp.y0).max(0) as f64 * ppd;
        fw <= self.box_px && fh <= self.box_px
    }

    fn take_box(&mut self, n: u64) -> bool {
        if n <= self.boxes_left {
            self.boxes_left -= n;
            self.st.sub_cut_boxes += n;
            true
        } else {
            self.st.sub_cut_box_over += 1;
            false
        }
    }

    /// A size-cut page in view under the sub-cut boxes: true when it stays
    /// as its bbox on its own layer (never decoded).
    fn box_page(&mut self, p: &floe_ovm::PageV, pi: u32, washes: &mut Vec<(u32, BBox)>, ci: u32) -> bool {
        if !self.boxm || !self.box_small(&p.bbox) || !self.take_box(1) {
            return false;
        }
        washes.push((p.layer_idx, p.bbox));
        self.note_page("box", ci, p, pi);
        true
    }

    /// which of `vis_layers` a layer bitset holds (bit k = vis_layers[k]),
    /// memoized by bitset index
    fn vis_bits(&mut self, mask: u32) -> LayerSet {
        if self.vis_layers.len() <= 16 {
            // a few layers: testing their bits beats a memo lookup (this runs
            // once per placement read)
            let bits = self.v.bitset(mask);
            let mut out = LayerSet::EMPTY;
            for (rank, &layer) in self.vis_layers.iter().enumerate() {
                if bits.get((layer / 8) as usize).is_some_and(|byte| byte & (1 << (layer % 8)) != 0) {
                    out.0[0] |= 1 << rank;
                }
            }
            return out;
        }
        if let Some(known) = self.mask_bits_memo.get(&mask) {
            return *known;
        }
        let mut out = LayerSet::EMPTY;
        for (at, (&byte, &vis)) in self.v.bitset(mask).iter().zip(self.wash_vis.iter()).enumerate() {
            let mut both = byte & vis;
            while both != 0 {
                let layer = at as u32 * 8 + both.trailing_zeros();
                if let Some(&rank) = self.vis_rank.get(&layer) {
                    out.insert(rank);
                }
                both &= both - 1;
            }
        }
        self.mask_bits_memo.insert(mask, out);
        out
    }

    /// the remaining depth a child of a cell at `r` is shown with
    fn child_rem(&self, child: u32, r: u32) -> u32 {
        if r == REM_FULL || r == 0 {
            return r;
        }
        if r - 1 >= self.v.cell_height(child) {
            REM_FULL
        } else {
            r - 1
        }
    }

    /// The visible layers cell `ci` really draws with `rem` levels left
    /// below it: the recursive mask at full depth, its own shapes at the
    /// depth boundary, else its own shapes and what its children draw one
    /// level down (a walk of its placements, memoized per pass, stopped
    /// once everything the recursive mask allows is found).
    fn cell_bits(&mut self, ci: u32, rem: u32) -> LayerSet {
        let v = self.v;
        let all = self.vis_bits(v.cell_lmask_rec(ci));
        let n = self.set_words;
        if rem == REM_FULL || all.is_empty(n) || rem >= v.cell_height(ci) {
            return all;
        }
        let own = self.vis_bits(v.cell_lmask_direct(ci));
        if rem == 0 || own.same(&all, n) {
            return own;
        }
        if let Some(&known) = self.cell_bits_memo.get(&(ci, rem)) {
            return known;
        }
        let (start, count) = v.cell_places(ci);
        let mut found = own;
        for pli in start as u64..start as u64 + count as u64 {
            if self.reads_left == 0 {
                // unknown beyond here: what was found is there, the rest is
                // not claimed (and not memoized as complete)
                self.st.sub_cut_box_unsure += 1;
                return found;
            }
            self.reads_left -= 1;
            self.st.sub_cut_box_reads += 1;
            let child = v.place_child(pli);
            let below = self.cell_bits(child, self.child_rem(child, rem));
            found.union(&below, n);
            if found.same(&all, n) {
                break;
            }
        }
        self.cell_bits_memo.insert((ci, rem), found);
        found
    }

    /// `fp` as ONE box on the topmost of the visible layers in `found`. The
    /// boxes of the layers under it would cover the same pixels and be
    /// overwritten: every paint is opaque, and the default fill (the
    /// speckle) has one phase for all layers. (With per-layer stipples the
    /// lower layers would show through the top one's dark pixels - inside a
    /// box of at most a few pixels, behind its solid outline.) One rect per
    /// box instead of one per layer is what lets the boxes work with any
    /// number of layers visible: 32 layers took 1.9 M rects, the plan cap.
    fn box_layers(&mut self, wc: &mut WsCell, found: LayerSet, fp: BBox) -> bool {
        let Some(top) = found.top(self.set_words) else {
            return false;
        };
        if !self.take_box(1) {
            return false;
        }
        wc.washes.push((self.vis_layers[top], fp));
        true
    }

    /// A size-cut child-BVH node no wider than a box, in a cell shown with
    /// `r` levels left: one box on the layers the placements below it
    /// really draw; nothing below is visited for geometry. The node's v8
    /// layer masks answer without a read at full depth (the recursive
    /// union) and one level above the depth boundary (the own-shapes
    /// union: a child with nothing below it is its own shapes); between
    /// the two they bound the answer, and only when the bounds differ - or
    /// the index carries no masks - are the placements asked, ALL of them,
    /// up to the moment every layer in `upper` is found.
    fn box_node(&mut self, wc: &mut WsCell, ni: u32, fp: &BBox, r: u32, upper: LayerSet, masks: (u32, u32)) {
        let known = if masks.0 == floe_ovm::LMASK_UNKNOWN || masks.1 == floe_ovm::LMASK_UNKNOWN {
            None
        } else {
            let (most, least) = (self.vis_bits(masks.0), self.vis_bits(masks.1));
            if r == REM_FULL || most.same(&least, self.set_words) {
                Some((most, most))
            } else if r == 1 {
                Some((least, least))
            } else {
                Some((least, most))
            }
        };
        let (least, upper) = match known {
            Some((least, most)) if least.same(&most, self.set_words) => {
                if self.box_layers(wc, most, *fp) {
                    self.st.sub_cut_box_nodes += 1;
                }
                return;
            }
            Some((least, most)) => (least, most),
            None => (LayerSet::EMPTY, upper),
        };
        let found = match self.node_bits_memo.get(&(ni, r)) {
            Some(&known) => known,
            None => {
                let (lo, hi) = self.cbvh_places(ni);
                let v = self.v;
                let mut found = least;
                for pli in lo as u64..hi as u64 {
                    if self.reads_left == 0 {
                        self.st.sub_cut_box_unsure += 1;
                        break;
                    }
                    self.reads_left -= 1;
                    self.st.sub_cut_box_reads += 1;
                    let child = v.place_child(pli);
                    let below = self.cell_bits(child, self.child_rem(child, r));
                    found.union(&below, self.set_words);
                    if found.same(&upper, self.set_words) {
                        break;
                    }
                }
                self.node_bits_memo.insert((ni, r), found);
                found
            }
        };
        if self.box_layers(wc, found, *fp) {
            self.st.sub_cut_box_nodes += 1;
        }
    }

    /// A size-cut child placement under the sub-cut boxes, in a cell shown
    /// with `r` levels left. A single child is its bbox (under the cut). An
    /// array no wider than a box is one box. A wider axis-aligned grid is
    /// drawn member by member, each member its own bbox; along an axis where
    /// the members really touch on screen (pitch <= member size, or under a
    /// pixel) they run together; a point list is drawn point by point. A
    /// skewed grid wider than a box is dropped as before.
    fn box_child(&mut self, wc: &mut WsCell, pli: u64, h: &floe_ovm::PlaceHead, rb: &BBox, boxes: &[BBox], r: u32) {
        let t0 = Xf::place(h.x, h.y, h.rot, h.flip);
        let b0 = xf_bbox(&t0, rb);
        let fp = match h.kind {
            0 => b0,
            1 => grow_by_offsets(&b0, &grid_ovis(0, h.na as i64 - 1, 0, h.nb as i64 - 1, h.va, h.vb)),
            _ => match self.v.pts_ref(pli) {
                Some(pr) => grow_by_offsets(&b0, &pr.extent()),
                None => b0,
            },
        };
        if fp.is_empty() || !boxes.iter().any(|b| fp.intersects(b)) {
            return;
        }
        let found = self.cell_bits(h.child, self.child_rem(h.child, r));
        if found.is_empty(self.set_words) {
            return;
        }
        if h.kind == 0 || self.box_small(&fp) {
            self.box_layers(wc, found, fp);
            return;
        }
        let mut view = BBox::EMPTY;
        for b in boxes {
            view.grow(b);
        }
        if h.kind == 2 {
            // a point list: the members in view, chunk by chunk
            let Some(pr) = self.v.pts_ref(pli) else {
                return;
            };
            let region = minkowski_neg(&view, &b0);
            let stride = self.box_stride;
            let mut members = 0u64;
            for k in 0..pr.n_chunks {
                if !pr.chunk_bbox(k).intersects(&region) {
                    continue;
                }
                let (lo, hi) = pr.chunk_range(k);
                for slot in (lo..hi).filter(|slot| *slot as i64 % stride == 0) {
                    let (ox, oy) = pr.pt(slot);
                    let at = pt_box(ox, oy);
                    if !at.intersects(&region) {
                        continue;
                    }
                    members += 1;
                    if members > SUB_CUT_BOX_ARRAY_MAX {
                        self.st.sub_cut_box_over += 1;
                        return;
                    }
                    if !self.box_layers(wc, found, grow_by_offsets(&b0, &at)) {
                        return;
                    }
                }
            }
            return;
        }
        let (na, nb, va, vb) = (h.na as i64, h.nb as i64, h.va, h.vb);
        // axis-aligned grids only: a along x and b along y, or the other way round
        // (a skewed grid wider than a box is dropped as before)
        let aligned = (va.1 == 0 && vb.0 == 0) || (va.0 == 0 && vb.1 == 0);
        if h.kind != 1 || !aligned {
            return;
        }
        let GridVis::Range { i0, i1, j0, j1 } = grid_ranges(na, nb, va, vb, &minkowski_neg(&view, &b0)) else {
            return;
        };
        let ppd = self.px_per_dbu;
        let (mw, mh) = ((b0.x1 - b0.x0).max(0) as f64 * ppd, (b0.y1 - b0.y0).max(0) as f64 * ppd);
        // members touch on screen along an axis: the pitch is no more than
        // the member's size there, or than the pixel a smaller member lights
        let touch = |count: i64, step: (i64, i64)| {
            let (pitch, size) = if step.1 == 0 { (step.0.unsigned_abs() as f64 * ppd, mw) } else { (step.1.unsigned_abs() as f64 * ppd, mh) };
            count == 1 || pitch <= size.max(1.0)
        };
        let (run_a, run_b) = (touch(na, va), touch(nb, vb));
        let (count_a, count_b) = (if run_a { 1 } else { (i1 - i0 + 1) as u64 }, if run_b { 1 } else { (j1 - j0 + 1) as u64 });
        // too many members in view: every s-th of index 0, s, 2s, ... (real
        // positions at a lower density, stable under a pan)
        let mut stride = self.box_stride;
        while (count_a.div_ceil(if run_a { 1 } else { stride as u64 })) * (count_b.div_ceil(if run_b { 1 } else { stride as u64 })) > SUB_CUT_BOX_ARRAY_MAX {
            stride += 1;
        }
        if stride > self.box_stride {
            self.st.sub_cut_box_strided += 1;
        }
        let groups = |lo: i64, hi: i64, run: bool| -> Vec<(i64, i64)> {
            if run {
                vec![(lo, hi)]
            } else {
                let first = (lo + stride - 1) / stride * stride;
                (first..=hi).step_by(stride as usize).map(|at| (at, at)).collect()
            }
        };
        let (group_a, group_b) = (groups(i0, i1, run_a), groups(j0, j1, run_b));
        let rects = (group_a.len() * group_b.len()) as u64;
        if rects > self.boxes_left {
            self.st.sub_cut_box_over += 1;
            return;
        }
        for &(ia, ib) in &group_a {
            for &(ja, jb) in &group_b {
                let member = grow_by_offsets(&b0, &grid_ovis(ia, ib, ja, jb, va, vb));
                if !self.box_layers(wc, found, member) {
                    return;
                }
            }
        }
    }

    /// The footprint is wider than WASH_WIDE_NODE_PX on a side.
    fn wash_wide(&self, fp: &BBox) -> bool {
        let ppd = self.px_per_dbu;
        if !(ppd > 0.0) {
            return false;
        }
        let fw = (fp.x1 - fp.x0).max(0) as f64 * ppd;
        let fh = (fp.y1 - fp.y0).max(0) as f64 * ppd;
        fw > WASH_WIDE_NODE_PX || fh > WASH_WIDE_NODE_PX
    }

    /// The footprint is one screen blob: both sides at most the cut.
    fn wash_blob(&self, fp: &BBox) -> bool {
        let ppd = self.px_per_dbu;
        if !(ppd > 0.0) {
            return true;
        }
        let fw = (fp.x1 - fp.x0).max(0) as f64 * ppd;
        let fh = (fp.y1 - fp.y0).max(0) as f64 * ppd;
        let cut_px = self.cut as f64 * ppd;
        fw <= cut_px && fh <= cut_px
    }

    /// Whether a sub-cut wash may stand in for what the hairline
    /// render of `members` records of at most `w` x `h` dbu inside
    /// `fp` would show: the footprint is one screen blob, or the
    /// members - each at least one pixel, as a hairline is - could
    /// cover at least WASH_MIN_COVERAGE of the footprint's pixels.
    /// Field 2026-09-15 (level 4 at depth 0): a page holding two
    /// 140 x 4 um marks 137 mm apart has a 137 x 54 mm page bbox and
    /// was washed as one block of that size in the layer colour from
    /// the zoom where the marks went under the cut. A sparse page
    /// is now KEPT (its members draw as hairline pixels, as Calibre
    /// shows them at every zoom - user 2026-09-15) and a sparse
    /// placement expanded, the cost being bounded by the very
    /// sparseness that ruled the wash out.
    /// `hair`: the item is hairline-cut under the cull policy, where
    /// the stricter WASH_MIN_COVERAGE_HAIR applies; `rep`: the item is
    /// a representative (page frontier), which always takes the 1/8
    /// rule - a sparse representative is cheap to draw, and a wash is
    /// only worth its overstatement when the page is really dense.
    /// The ink estimate is members x (min side) x (long side): every
    /// member's area is at most its min side times its long side, so
    /// this bounds the real ink from above without the max_w x max_h
    /// overshoot that made an L of two hairlines (1000 x 1 and
    /// 1 x 1000) look 200 % dense (review 2026-09-17).
    #[allow(clippy::too_many_arguments)]
    fn wash_worth(&self, fp: &BBox, members: u64, w: u64, h: u64, min_side: u64, hair: bool, rep: bool) -> bool {
        if self.wash_blob(fp) {
            return true;
        }
        let ppd = self.px_per_dbu;
        let fw = ((fp.x1 - fp.x0).max(0) as f64 * ppd).max(1.0);
        let fh = ((fp.y1 - fp.y0).max(0) as f64 * ppd).max(1.0);
        let long = (w.max(h) as f64 * ppd).max(1.0);
        let short = (min_side.min(w.max(h)) as f64 * ppd).max(1.0);
        let min = if hair || rep { hair_wash_coverage() } else { WASH_MIN_COVERAGE };
        (members as f64) * short * long >= min * fw * fh
    }

    /// Screen px of `fp` inside the view boxes - the raster cost of a
    /// wash there (0 when px_per_dbu is 0: a probe costs no raster).
    fn visible_px(&self, fp: &BBox, boxes: &[BBox]) -> f64 {
        let ppd = self.px_per_dbu;
        if !(ppd > 0.0) {
            return 0.0;
        }
        let mut px = 0.0;
        for b in boxes {
            let (x0, x1) = (fp.x0.max(b.x0), fp.x1.min(b.x1));
            let (y0, y1) = (fp.y0.max(b.y0), fp.y1.min(b.y1));
            if x1 > x0 && y1 > y0 {
                px += ((x1 - x0) as f64 * ppd).max(1.0) * ((y1 - y0) as f64 * ppd).max(1.0);
            }
        }
        px
    }

    /// Takes a sparse item's ink estimate (members x member px, each
    /// member at least one pixel, as wash_worth measures it) from the
    /// per-plan sparse budget; false when it does not fit - the item
    /// is dropped as the cull always did (sub_cut_sparse_over).
    fn take_sparse(&mut self, members: u64, w: u64, h: u64) -> bool {
        let ppd = self.px_per_dbu;
        let ink = if ppd > 0.0 {
            members as f64 * (w as f64 * ppd).max(1.0) * (h as f64 * ppd).max(1.0)
        } else {
            members as f64
        };
        if ink <= self.sparse_px_left {
            self.sparse_px_left -= ink;
            true
        } else {
            self.st.sub_cut_sparse_over += 1;
            false
        }
    }

    /// Takes the visible area of a wash (`layers` rects of `fp`) from
    /// the per-plan wash budget; false when it does not fit - the item
    /// is dropped as the cull always did (sub_cut_wash_over).
    fn take_wash(&mut self, fp: &BBox, boxes: &[BBox], layers: u64) -> bool {
        let px = self.visible_px(fp, boxes) * layers as f64;
        if px <= self.wash_px_left {
            self.wash_px_left -= px;
            true
        } else {
            self.st.sub_cut_wash_over += 1;
            false
        }
    }

    /// A cut page in view: the sub-cut verdict when `washable` (the
    /// sub-cut rules, or the page is a representative) - sparse pages
    /// are kept and drawn as pixels, dense ones washed, either within
    /// the per-plan budgets - else Drop
    fn sub_cut_verdict(&mut self, p: &floe_ovm::PageV, boxes: &[BBox], size_cut: bool, washable: bool, rep: bool) -> SubCut {
        if !washable {
            return SubCut::Drop;
        }
        if rep {
            // a representative is DRAWN, never washed (field
            // 2026-09-17: washes left one box at the fit view and
            // boxes at the next zooms where lines were wanted); its
            // cost is what the page cost one octave closer, and the
            // octave thinning bounds how many there are
            return SubCut::Keep;
        }
        let hair = self.hair_cut(size_cut);
        if !self.wash_worth(&p.bbox, p.members, p.max_w, p.max_h, p.max_min, hair, false) {
            // a sparse page beyond the budget is dropped, never
            // washed (its footprint is a false block)
            return if self.take_sparse(p.members, p.max_w, p.max_h) { SubCut::Keep } else { SubCut::Drop };
        }
        if self.take_wash(&p.bbox, boxes, 1) {
            SubCut::Wash
        } else {
            SubCut::Drop
        }
    }

    /// the pages of a page-BVH subtree, [lo, hi): pages are laid out in
    /// tree order (finish_layer's leaf-order permute), so the first
    /// leaf's first page and the last leaf's end bound the subtree
    fn pbvh_pages(&self, ni: u32) -> (u32, u32) {
        let mut a = ni;
        let lo = loop {
            let n = self.v.pbvh(a);
            if n.leaf || n.count == 0 {
                break n.first;
            }
            a = n.first;
        };
        let mut z = ni;
        let hi = loop {
            let n = self.v.pbvh(z);
            if n.leaf || n.count == 0 {
                break n.first + n.count as u32;
            }
            z = n.first + n.count as u32 - 1;
        };
        (lo, hi)
    }

    /// the placements of a child-BVH subtree, [lo, hi)
    fn cbvh_places(&self, ni: u32) -> (u32, u32) {
        let mut a = ni;
        let lo = loop {
            let n = self.v.bvh(a);
            if n.leaf || n.count == 0 {
                break n.first;
            }
            a = n.first;
        };
        let mut z = ni;
        let hi = loop {
            let n = self.v.bvh(z);
            if n.leaf || n.count == 0 {
                break n.first + n.count as u32;
            }
            z = n.first + n.count as u32 - 1;
        };
        (lo, hi)
    }

    /// Whether a cut placement is a representative, and the member
    /// levels its repetition is thinned by: L levels below its cut keep
    /// one item in 2^L. An array's members absorb min(L, log2 members)
    /// of them - one member in 2^lm, by balanced strides (thin_grid) or
    /// every 2^lm-th point - and the placement's index within its cell
    /// the remaining L - lm; a plain placement (one member) takes them
    /// all by index. Both factors are powers of two growing with L, so
    /// the survivors stay nested; and never the array's footprint as one
    /// wash (field 2026-09-17: that was the one box left at the fit
    /// view) - each survivor is a dot of its own box (rep_dots).
    fn place_rep(&mut self, pli: u64, h: &floe_ovm::PlaceHead, rb: &BBox, place_start: u32) -> Option<u32> {
        let members = self.place_members(pli, h);
        let (cw, chh) = ((rb.x1 - rb.x0).max(0) as u64, (rb.y1 - rb.y0).max(0) as u64);
        let fp = self.place_footprint(pli, h, rb);
        let l = self.density_level(members, cw.min(chh), cw.max(chh), &fp);
        let lm = member_levels(members, l);
        if rep_keeps(pli.saturating_sub(place_start as u64), l - lm) {
            self.st.rep_items = self.st.rep_items.saturating_add(members);
            self.st.rep_level = self.st.rep_level.max(l);
            Some(lm)
        } else {
            None
        }
    }

    /// the density's dot pitch in screen px (1 / sqrt(density)), and
    /// whether a box on screen is within it on both axes
    fn within_dot_pitch(&self, bx: &BBox) -> bool {
        let ppd = self.px_per_dbu;
        let d = self.opts.rep_density;
        if !(ppd > 0.0) || !(d > 0.0) {
            return false;
        }
        let pitch = (1.0 / d.sqrt()).max(1.0);
        let w = (bx.x1 - bx.x0).max(0) as f64 * ppd;
        let h = (bx.y1 - bx.y0).max(0) as f64 * ppd;
        w <= pitch && h <= pitch
    }

    /// one dot for a cut child-BVH subtree: a pixel at the node's
    /// centre on the visible recursive layers of its first placement's
    /// child (the node holds no layer mask of its own; the first child
    /// is its representative), at most one dot per cell of the dot
    /// pitch's lattice in the cell's frame (sibling nodes within the
    /// pitch would otherwise pile up: the field's block came out
    /// three-quarters lit)
    fn rep_node_dot(&mut self, wc: &mut WsCell, ni: u32, bx: &BBox) {
        let ppd = self.px_per_dbu;
        let side = if ppd > 0.0 { (1.0 / ppd).ceil().max(1.0) as i64 } else { 1 };
        let pitch = (1.0 / self.opts.rep_density.max(1e-9).sqrt()).max(1.0);
        let pitch_dbu = if ppd > 0.0 { (pitch / ppd).ceil().max(1.0) as i64 } else { 1 };
        let cx = bx.x0 / 2 + bx.x1 / 2;
        let cy = bx.y0 / 2 + bx.y1 / 2;
        if !self.dot_lattice.insert((cx.div_euclid(pitch_dbu), cy.div_euclid(pitch_dbu))) {
            return;
        }
        let dot = BBox {
            x0: cx - side / 2,
            y0: cy - side / 2,
            x1: cx - side / 2 + side,
            y1: cy - side / 2 + side,
        };
        let v = self.v;
        let mut a = ni;
        let first = loop {
            let n = v.bvh(a);
            if n.leaf || n.count == 0 {
                break n.first as u64;
            }
            a = n.first;
        };
        let child = v.place_head(first).child;
        let bits = v.bitset(v.cell_lmask_rec(child));
        for (byte_index, (&m, &vis)) in bits.iter().zip(self.wash_vis.iter()).enumerate() {
            let both = m & vis;
            for bit in 0..8 {
                if both & (1 << bit) != 0 {
                    wc.washes.push(((byte_index * 8 + bit) as u32, dot));
                }
            }
        }
        self.st.rep_node_dots += 1;
        self.st.rep_children += 1;
    }

    /// the page frontier's level of a cut item: the smallest L with
    /// ink / 2^L <= rep_density x area, ink = members x max(1, min side
    /// px) x max(1, long side px) (every member paints at most that),
    /// area the item's box on screen, at least a pixel each way
    fn density_level(&self, members: u64, min_side: u64, long_side: u64, bx: &BBox) -> u32 {
        let ppd = self.px_per_dbu;
        let d = self.opts.rep_density;
        if !(ppd > 0.0) || !(d > 0.0) || members == 0 {
            return 0;
        }
        let w = (min_side as f64 * ppd).max(1.0);
        let l = (long_side as f64 * ppd).max(1.0);
        let ink = members as f64 * w * l;
        let aw = ((bx.x1 - bx.x0).max(0) as f64 * ppd).max(1.0);
        let ah = ((bx.y1 - bx.y0).max(0) as f64 * ppd).max(1.0);
        let target = d * aw * ah;
        if ink <= target {
            return 0;
        }
        ((ink / target).log2().ceil().max(0.0) as u32).min(REP_LEVELS_MAX)
    }

    /// a placement's whole footprint (the repetition extent of the
    /// child's box) in the parent's frame
    fn place_footprint(&self, pli: u64, h: &floe_ovm::PlaceHead, rb: &BBox) -> BBox {
        let t0 = Xf::place(h.x, h.y, h.rot, h.flip);
        let b0 = xf_bbox(&t0, rb);
        match h.kind {
            0 => b0,
            1 => grow_by_offsets(
                &b0,
                &grid_ovis(0, h.na as i64 - 1, 0, h.nb as i64 - 1, h.va, h.vb),
            ),
            _ => match self.v.pts_ref(pli) {
                Some(pr) => grow_by_offsets(&b0, &pr.extent()),
                None => b0,
            },
        }
    }

    /// a placement's repetition member count
    fn place_members(&self, pli: u64, h: &floe_ovm::PlaceHead) -> u64 {
        match h.kind {
            0 => 1,
            1 => (h.na as u64).saturating_mul(h.nb as u64),
            _ => self.v.pts_ref(pli).map(|p| p.count as u64).unwrap_or(1),
        }
    }

    /// A representative cut placement drawn as DOTS: one rect of the
    /// child's box per kept member (member 0 and every 2^lm-th, the
    /// grid by thin_grid's balanced strides) on every visible layer
    /// of the child's recursive mask - the box is at most the cut on
    /// screen (or a hairline strip), so it is the instance's picture
    /// at this zoom, costs no page decode and no walk into the child
    /// (field 2026-09-17: drawing the child whole decoded every page
    /// of every kept instance for one pixel each, and filled blocks
    /// into boxes). Nothing off the view is emitted.
    fn rep_dots(&mut self, wc: &mut WsCell, pli: u64, h: &floe_ovm::PlaceHead, rb: &BBox, boxes: &[BBox], lm: u32) -> u64 {
        let v = self.v;
        let bits = v.bitset(v.cell_lmask_rec(h.child));
        let mut layers: Vec<u32> = Vec::new();
        for (byte_index, (&m, &vis)) in bits.iter().zip(self.wash_vis.iter()).enumerate() {
            let both = m & vis;
            for bit in 0..8 {
                if both & (1 << bit) != 0 {
                    layers.push((byte_index * 8 + bit) as u32);
                }
            }
        }
        if layers.is_empty() {
            return 0;
        }
        let t0 = Xf::place(h.x, h.y, h.rot, h.flip);
        let b0 = xf_bbox(&t0, rb);
        let mut dots = 0u64;
        // at most one dot per cell of the dot pitch's lattice (cell
        // frame), whatever emits it: sparse repetitions scattered over
        // the same area - an OASIS writer folds scattered instances of
        // one cell into point sets - are each sparse by their own
        // footprint yet pile up together (the field's block came out
        // three-quarters lit)
        let ppd = self.px_per_dbu;
        let pitch = (1.0 / self.opts.rep_density.max(1e-9).sqrt()).max(1.0);
        let pitch_dbu = if ppd > 0.0 { (pitch / ppd).ceil().max(1.0) as i64 } else { 1 };
        let lattice = &mut self.dot_lattice;
        let mut emit = |wc: &mut WsCell, dx: i64, dy: i64| {
            let mb = BBox {
                x0: b0.x0.saturating_add(dx),
                y0: b0.y0.saturating_add(dy),
                x1: b0.x1.saturating_add(dx),
                y1: b0.y1.saturating_add(dy),
            };
            if !boxes.iter().any(|b| mb.intersects(b)) {
                return;
            }
            let (cx, cy) = (mb.x0 / 2 + mb.x1 / 2, mb.y0 / 2 + mb.y1 / 2);
            if !lattice.insert((cx.div_euclid(pitch_dbu), cy.div_euclid(pitch_dbu))) {
                return;
            }
            for &l in &layers {
                wc.washes.push((l, mb));
            }
            dots += 1;
        };
        match h.kind {
            0 => emit(wc, 0, 0),
            1 => {
                let (na, nb, va, vb) = thin_grid(h.na as u64, h.nb as u64, h.va, h.vb, lm);
                for j in 0..nb as i64 {
                    for i in 0..na as i64 {
                        emit(wc, i * va.0 + j * vb.0, i * va.1 + j * vb.1);
                    }
                }
            }
            _ => {
                if let Some(pr) = v.pts_ref(pli) {
                    let step = 1u32 << lm.min(31);
                    let mut s = 0u32;
                    while s < pr.count {
                        let (dx, dy) = pr.pt(s);
                        emit(wc, dx, dy);
                        s = s.saturating_add(step);
                        if step == 0 {
                            break;
                        }
                    }
                }
            }
        }
        self.st.rep_dots = self.st.rep_dots.saturating_add(dots);
        dots
    }

    /// Whether a culled item is a hairline cut under the cull policy
    /// (a size cut, or any cut under keep where page_hair is 0, takes
    /// the dense-array coverage rule).
    fn hair_cut(&self, size_cut: bool) -> bool {
        !size_cut && self.page_hair > 0
    }

    /// Returns false when the placement is sparse (no wash could
    /// stand for it): the caller expands it instead of dropping it.
    fn wash_sub_cut_child(
        &mut self,
        wc: &mut WsCell,
        pli: u64,
        h: &floe_ovm::PlaceHead,
        rb: &BBox,
        boxes: &[BBox],
        hair: bool,
    ) -> bool {
        let t0 = Xf::place(h.x, h.y, h.rot, h.flip);
        let b0 = xf_bbox(&t0, rb);
        let (fp, members) = match h.kind {
            0 => (b0, 1u64),
            1 => (
                grow_by_offsets(
                    &b0,
                    &grid_ovis(0, h.na as i64 - 1, 0, h.nb as i64 - 1, h.va, h.vb),
                ),
                (h.na as u64).saturating_mul(h.nb as u64),
            ),
            _ => match self.v.pts_ref(pli) {
                Some(pr) => (grow_by_offsets(&b0, &pr.extent()), pr.count as u64),
                None => (b0, 1u64),
            },
        };
        if fp.is_empty() || !boxes.iter().any(|b| fp.intersects(b)) {
            return true;
        }
        let bw = (b0.x1 - b0.x0).max(0) as u64;
        let bh = (b0.y1 - b0.y0).max(0) as u64;
        if !self.wash_worth(&fp, members, bw, bh, bw.min(bh), hair, false) {
            // sparse: expanded while the sparse budget lasts; beyond
            // it dropped (the caller's cull), never washed
            return !self.take_sparse(members, bw, bh);
        }
        let mask = self.v.cell_lmask_rec(h.child);
        self.wash_layers(wc, mask, fp, boxes, true);
        true
    }

    /// One wash rect `fp` per visible layer in bitset `mask`, when the
    /// wash budget covers them (false: dropped, sub_cut_wash_over).
    fn wash_layers(&mut self, wc: &mut WsCell, mask: u32, fp: BBox, boxes: &[BBox], count: bool) -> bool {
        let v = self.v;
        let bits = v.bitset(mask);
        let mut layers: Vec<u32> = Vec::new();
        for (byte_index, (&m, &vis)) in bits.iter().zip(self.wash_vis.iter()).enumerate() {
            let both = m & vis;
            if both == 0 {
                continue;
            }
            for bit in 0..8 {
                if both & (1 << bit) != 0 {
                    layers.push((byte_index * 8 + bit) as u32);
                }
            }
        }
        if layers.is_empty() {
            return true;
        }
        if !self.take_wash(&fp, boxes, layers.len() as u64) {
            return false;
        }
        for layer in layers {
            wc.washes.push((layer, fp));
            if count {
                self.st.sub_cut_washes += 1;
            }
        }
        true
    }

    /// One explain row (HierOpts::explain); a no-op otherwise.
    #[allow(clippy::too_many_arguments)]
    fn note(
        &mut self,
        kind: &'static str,
        verdict: &'static str,
        cell: u32,
        layer_idx: Option<u32>,
        id: u64,
        bbox: BBox,
        w: u64,
        h: u64,
        min: u64,
        members: u64,
    ) {
        if self.explain_on {
            self.explain.push(ExplainRow {
                kind,
                verdict,
                cell,
                layer_idx,
                id,
                bbox,
                w,
                h,
                min,
                members,
            });
        }
    }

    fn note_page(&mut self, verdict: &'static str, cell: u32, p: &floe_ovm::PageV, pi: u32) {
        if self.explain_on {
            self.note("page", verdict, cell, Some(p.layer_idx), pi as u64, p.bbox, p.max_w, p.max_h, p.max_min, p.members);
        }
    }

    /// A child placement's verdict, when its (first member's) box
    /// meets a view box.
    fn note_child(
        &mut self,
        verdict: &'static str,
        pli: u64,
        h: &floe_ovm::PlaceHead,
        rb: &BBox,
        boxes: &[BBox],
    ) {
        if !self.explain_on {
            return;
        }
        let wb = xf_bbox(&Xf::place(h.x, h.y, h.rot, h.flip), rb);
        if !boxes.iter().any(|b| wb.intersects(b)) {
            return;
        }
        let cw = (rb.x1 - rb.x0).max(0) as u64;
        let chh = (rb.y1 - rb.y0).max(0) as u64;
        let members = match h.kind {
            1 => h.na as u64 * h.nb as u64,
            0 => 1,
            _ => self.v.pts_ref(pli).map(|pr| pr.count as u64).unwrap_or(1),
        };
        self.note("child", verdict, h.child, None, pli, wb, cw, chh, cw.min(chh), members);
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn walk_pbvh(
        &mut self,
        root: u32,
        b: &BBox,
        psel: &mut BTreeSet<u32>,
        cell: u32,
        layer_idx: u32,
        page_lo: u32,
        washes: &mut Vec<(u32, BBox)>,
    ) {
        let mut stack = vec![root];
        while let Some(ni) = stack.pop() {
            let n = self.v.pbvh(ni);
            self.st.visited_page_bvh += 1;
            if n.max_w < self.cut && n.max_h < self.cut {
                self.st.culled_page_bvh_cut += 1;
                if self.explain_on && n.bbox.intersects(b) {
                    let nb = n.bbox;
                    let (mw, mh) = (n.max_w, n.max_h);
                    self.note("pbvh", "cull_size", cell, Some(layer_idx), ni as u64, nb, mw, mh, mw.min(mh), n.count as u64);
                }
                // sub-cut wash: a node that is one screen blob, or
                // at most WASH_WIDE_NODE_PX on both sides, washes
                // whole (a page BVH is per (cell, layer), so the
                // layer is exact); a wider node walks on while the
                // budget lasts so its pages decide by their own
                // member coverage (wash_worth / keep_sparse), and
                // beyond the budget washes its extent coarsely
                if self.boxm && n.bbox.intersects(b) {
                    // sub-cut boxes: a node no wider than a box is one
                    // (a page BVH is per (cell, layer): the layer is
                    // exact); a wider one walks on to its pages
                    if self.box_small(&n.bbox) {
                        if self.take_box(1) {
                            washes.push((layer_idx, n.bbox));
                            self.st.sub_cut_box_nodes += 1;
                        }
                        continue;
                    }
                } else if self.sub_cut_wash && n.bbox.intersects(b) {
                    if self.wash_blob(&n.bbox) || !self.wash_wide(&n.bbox) {
                        if self.take_wash(&n.bbox, std::slice::from_ref(b), 1) {
                            washes.push((layer_idx, n.bbox));
                            self.st.sub_cut_washes += 1;
                        }
                        continue;
                    }
                    if self.wash_walk_budget == 0 {
                        if self.take_wash(&n.bbox, std::slice::from_ref(b), 1) {
                            washes.push((layer_idx, n.bbox));
                            self.st.sub_cut_washes += 1;
                            self.st.sub_cut_coarse += 1;
                        }
                        continue;
                    }
                    self.wash_walk_budget -= 1;
                } else if self.reps && n.bbox.intersects(b) && {
                    // the page frontier: a representative page may live
                    // below (an index that is a multiple of 2^Lp within
                    // the run, Lp the pass' page level): descend; a
                    // subtree without one is pruned
                    let (lo, hi) = self.pbvh_pages(ni);
                    rep_in_run(lo, hi, page_lo, self.rep_page_level)
                } {
                    // descend
                } else {
                    if self.reps && n.bbox.intersects(b) {
                        self.st.rep_pruned += 1;
                    }
                    continue;
                }
            } else if self.reps
                && self.rep_page_level > 0
                && self.page_hair > 0
                && n.max_w.min(n.max_h) < self.page_hair
                && n.bbox.intersects(b)
            {
                // every page below is hairline-cut (a page's max_min is
                // at most the smaller of the node's max_w / max_h, so
                // this catches the uniformly oriented routing runs; a
                // node mixing both orientations walks on to its leaves
                // - the index carries no max_min per node): prune the
                // subtree when its run holds no representative page
                let (lo, hi) = self.pbvh_pages(ni);
                if !rep_in_run(lo, hi, page_lo, self.rep_page_level) {
                    self.st.rep_pruned += 1;
                    continue;
                }
            }
            if !n.bbox.intersects(b) {
                self.st.culled_page_bvh_bbox += 1;
                continue;
            }
            if n.leaf {
                for pi in n.first..n.first + n.count as u32 {
                    self.st.page_candidates += 1;
                    let p = self.v.page(pi);
                    let size_cut = p.max_w < self.cut && p.max_h < self.cut;
                    if size_cut || p.max_min < self.page_hair {
                        let in_view = p.bbox.intersects(b);
                        // see the linear page loop
                        let rep = in_view
                            && self.reps
                            && rep_keeps((pi - page_lo) as u64, self.rep_page_level);
                        let washable = in_view && (self.sub_cut_wash || rep);
                        if size_cut && in_view && self.box_page(&p, pi, washes, cell) {
                            self.st.cull_page_size += 1;
                            continue;
                        }
                        match self.sub_cut_verdict(&p, std::slice::from_ref(b), size_cut, washable, rep) {
                            SubCut::Keep => {
                                if rep {
                                    let l = self.density_level(p.members, p.max_min, p.max_w.max(p.max_h), &p.bbox);
                                    self.st.rep_items = self.st.rep_items.saturating_add(p.members);
                                    self.st.rep_level = self.st.rep_level.max(l);
                                    let level = l.saturating_sub(self.rep_page_level);
                                    self.page_levels.insert(pi, level.min(255) as u8);
                                    self.st.rep_decode_bytes += p.usize_ as u64;
                                    self.st.rep_pages_kept += 1;
                                    self.note_page("rep_keep", cell, &p, pi);
                                } else {
                                    self.st.sub_cut_sparse += 1;
                                    self.note_page("keep_sparse", cell, &p, pi);
                                }
                                psel.insert(pi);
                            }
                            SubCut::Wash => {
                                self.st.cull_page_size += 1;
                                if rep {
                                    self.st.rep_pages_washed += 1;
                                    self.note_page("rep_wash", cell, &p, pi);
                                } else {
                                    self.st.sub_cut_washes += 1;
                                    self.note_page(if size_cut { "cull_size" } else { "cull_hair" }, cell, &p, pi);
                                }
                                washes.push((layer_idx, p.bbox));
                            }
                            SubCut::Drop => {
                                self.st.cull_page_size += 1;
                                if in_view {
                                    self.note_page(if size_cut { "cull_size" } else { "cull_hair" }, cell, &p, pi);
                                }
                            }
                        }
                        continue;
                    }
                    if p.bbox.intersects(b) {
                        psel.insert(pi);
                    }
                }
            } else {
                for k in 0..n.count as u32 {
                    stack.push(n.first + k);
                }
            }
        }
    }

    /// Explicit hierarchy-depth boundary -> outline frame. Per-member
    /// outlines (rect+rep) are the only form (rev 39: the ~2px pitch
    /// fuse and the pts density fuse are gone - a step function of the
    /// screen scale made whole regions of arrays pop between "gray
    /// member dust" and "big white footprint" on a hair of zoom).
    /// Instead the size cut applies to the MEMBER box itself: boxes
    /// under the cut in BOTH dimensions vanish exactly like
    /// geometry, uniformly for the whole rep, and the r>0 fold
    /// above already uses the same cell-box test. Rev 45: a box
    /// under the cut in ONE dimension (thin) degrades to lattice
    /// representatives instead of vanishing - see
    /// frame_thin_lattice. The one degradation left is the
    /// huge-pts count guard:
    /// materializing O(count) offsets per plan is a memory hazard, so
    /// such reps fall back to one footprint box. Footprint visibility
    /// test uses the repetition extent (zero-copy for pts).
    fn frame_depth_boundary(
        &mut self,
        wc: &mut WsCell,
        pli: u64,
        h: &floe_ovm::PlaceHead,
        rb: &BBox,
        boxes: &[BBox],
    ) {
        let t0 = Xf::place(h.x, h.y, h.rot, h.flip);
        let b0 = xf_bbox(&t0, rb);
        // rev 39: the cut gates the drawn member box (all members of
        // a rep share these dims, so one test culls the whole record
        // before any offset work)
        let bw = (b0.x1 - b0.x0).max(0) as u64;
        let bh = (b0.y1 - b0.y0).max(0) as u64;
        let in_view = self.explain_on && boxes.iter().any(|b| b0.intersects(b));
        if bw < self.cut && bh < self.cut {
            self.st.cull_size += 1;
            if in_view {
                self.note("frame", "cull_size", h.child, None, pli, b0, bw, bh, bw.min(bh), 0);
            }
            return;
        }
        if bw.min(bh) < self.cut {
            // rev 45 (Calibre): a thin box - min side under the cut,
            // long side at or above it - no longer vanishes. The
            // layout-fixed lattice keeps deterministic
            // representatives (banded gray fill / dots by the
            // existing ladder) until BOTH sides go under the cut.
            if self.thin_dbu > 0 {
                self.st.thin_frames += 1;
                if in_view {
                    self.note("frame", "thin_lattice", h.child, None, pli, b0, bw, bh, bw.min(bh), 0);
                }
                self.frame_thin_lattice(wc, pli, h, &b0, boxes);
                return;
            }
            // lattice off: the rev 41 hairline cull
            if bw.min(bh) < self.hair {
                self.st.cull_size += 1;
                if in_view {
                    self.note("frame", "cull_hair", h.child, None, pli, b0, bw, bh, bw.min(bh), 0);
                }
                return;
            }
        }
        if in_view {
            self.note("frame", "keep", h.child, None, pli, b0, bw, bh, bw.min(bh), 0);
        }
        let (rect, rep, fp) = match h.kind {
            0 => (b0, Rep::One, b0),
            1 => {
                let ov = grid_ovis(
                    0,
                    h.na as i64 - 1,
                    0,
                    h.nb as i64 - 1,
                    h.va,
                    h.vb,
                );
                let fp = grow_by_offsets(&b0, &ov);
                let rep = Rep::Grid {
                    na: h.na as u64,
                    nb: h.nb as u64,
                    va: h.va,
                    vb: h.vb,
                };
                (b0, rep, fp)
            }
            _ => {
                let pr = self.v.pts_ref(pli).expect("pts kind");
                let ext = pr.extent();
                let fp = grow_by_offsets(&b0, &ext);
                if pr.count > self.opts.pts_full_rep {
                    (fp, Rep::One, fp)
                } else {
                    let mut pts =
                        Vec::with_capacity(pr.count as usize);
                    for s in 0..pr.count {
                        pts.push(pr.pt(s));
                    }
                    (b0, Rep::Pts(pts.into()), fp)
                }
            }
        };
        self.push_frame(wc, rect, rep, &fp, boxes);
    }

    /// Rev 45 thin-frame sampling: representatives on a 2D lattice
    /// of thin_dbu pitch, anchored to the owning cell's coordinates
    /// (zoom-invariant survivor set - the observed Calibre
    /// behavior). Grids thin in closed form per axis: stride
    /// k = ceil(pitch_lattice / pitch_axis), keeping the interval
    /// bound offsets {0, k-1} (2 per bin in a row, 4 corners in a
    /// 2D array) until one lattice pitch is under thin_demote_px on
    /// screen, then only offset 0 (the 2 -> 1 -> 0 ladder). One/Pts
    /// members dedupe through thin_bins keyed by (child, bin) -
    /// first record wins, deterministic in placement order.
    fn frame_thin_lattice(
        &mut self,
        wc: &mut WsCell,
        pli: u64,
        h: &floe_ovm::PlaceHead,
        b0: &BBox,
        boxes: &[BBox],
    ) {
        let t = self.thin_dbu as i64;
        let shift = |b: &BBox, dx: i64, dy: i64| BBox {
            x0: b.x0.saturating_add(dx),
            y0: b.y0.saturating_add(dy),
            x1: b.x1.saturating_add(dx),
            y1: b.y1.saturating_add(dy),
        };
        match h.kind {
            0 => {
                let key = (
                    h.child,
                    b0.x0.div_euclid(t),
                    b0.y0.div_euclid(t),
                );
                if self.thin_bins.insert(key) {
                    self.push_frame(wc, *b0, Rep::One, b0, boxes);
                }
            }
            1 => {
                let (na, nb) = (h.na as i64, h.nb as i64);
                let stride = |v: (i64, i64)| {
                    let p = v.0.abs().max(v.1.abs());
                    if p <= 0 { 1 } else { ((t + p - 1) / p).max(1) }
                };
                let (ka, kb) = (stride(h.va), stride(h.vb));
                let demote = self.thin_demote;
                let offs = |k: i64| {
                    if k > 1 && !demote {
                        vec![0i64, k - 1]
                    } else {
                        vec![0i64]
                    }
                };
                let (oas, obs) = (offs(ka), offs(kb));
                // members of the axis sub-lattice i = o (mod k)
                let cnt = |o: i64, n: i64, k: i64| {
                    if o >= n { 0 } else { (n - o + k - 1) / k }
                };
                let sat = |v: i128| {
                    v.clamp(i64::MIN as i128, i64::MAX as i128)
                        as i64
                };
                for &oa in &oas {
                    for &ob in &obs {
                        let (ca, cb) =
                            (cnt(oa, na, ka), cnt(ob, nb, kb));
                        if ca == 0 || cb == 0 {
                            continue;
                        }
                        let dx = sat(oa as i128 * h.va.0 as i128
                            + ob as i128 * h.vb.0 as i128);
                        let dy = sat(oa as i128 * h.va.1 as i128
                            + ob as i128 * h.vb.1 as i128);
                        let base = shift(b0, dx, dy);
                        let sva = (
                            h.va.0.saturating_mul(ka),
                            h.va.1.saturating_mul(ka),
                        );
                        let svb = (
                            h.vb.0.saturating_mul(kb),
                            h.vb.1.saturating_mul(kb),
                        );
                        let rep = if ca == 1 && cb == 1 {
                            Rep::One
                        } else {
                            Rep::Grid {
                                na: ca as u64,
                                nb: cb as u64,
                                va: sva,
                                vb: svb,
                            }
                        };
                        let ov = grid_ovis(
                            0,
                            ca - 1,
                            0,
                            cb - 1,
                            sva,
                            svb,
                        );
                        let fp = grow_by_offsets(&base, &ov);
                        self.push_frame(wc, base, rep, &fp, boxes);
                    }
                }
            }
            _ => {
                let pr = self.v.pts_ref(pli).expect("pts kind");
                let ext = pr.extent();
                let fp = grow_by_offsets(b0, &ext);
                if pr.count > self.opts.pts_full_rep {
                    // count guard unchanged: one footprint box (a
                    // memory guard, not a visual heuristic)
                    self.push_frame(wc, fp, Rep::One, &fp, boxes);
                    return;
                }
                let mut kept: Vec<(i64, i64)> = Vec::new();
                for s in 0..pr.count {
                    let (ox, oy) = pr.pt(s);
                    let key = (
                        h.child,
                        b0.x0.saturating_add(ox).div_euclid(t),
                        b0.y0.saturating_add(oy).div_euclid(t),
                    );
                    if self.thin_bins.insert(key) {
                        kept.push((ox, oy));
                    }
                }
                if kept.is_empty() {
                    return;
                }
                // rebase on the first kept member ((0,0)-first)
                let (rx, ry) = kept[0];
                let base = shift(b0, rx, ry);
                let rep = if kept.len() == 1 {
                    Rep::One
                } else {
                    Rep::Pts(
                        kept.iter()
                            .map(|&(x, y)| (x - rx, y - ry))
                            .collect::<Vec<_>>()
                            .into(),
                    )
                };
                self.push_frame(wc, base, rep, &fp, boxes);
            }
        }
    }

    fn push_frame(
        &mut self,
        wc: &mut WsCell,
        rect: BBox,
        rep: Rep,
        fp: &BBox,
        boxes: &[BBox],
    ) {
        if boxes.iter().any(|b| fp.intersects(b)) {
            // band from the box actually drawn (member box for
            // per-member reps, footprint for fused ones)
            let band = frame_band(&rect, self.px_per_dbu);
            wc.frames.push((rect, rep, band));
            self.frames_total += 1;
            self.st.frame_rects += 1;
        }
    }

    fn place_edge(
        &mut self,
        wc: &mut WsCell,
        r: u32,
        pli: u64,
        boxes: &[BBox],
    ) {
        let h = self.v.place_head(pli);
        let child = self.v.cell(h.child);
        let structural = r != REM_FULL;
        if !structural
            && !masks_intersect(
                self.v.bitset(child.lmask_rec),
                &self.walk_vis,
            )
        {
            self.st.cull_layer += 1;
            return;
        }
        if child.rbbox.is_empty() {
            return;
        }
        let t0 = Xf::place(h.x, h.y, h.rot, h.flip);
        let b0 = xf_bbox(&t0, &child.rbbox);
        // cell-level cut (par.2.4): local dims, swap-invariant under
        // quarter turns, so above/below-cut is a property of the
        // CELL, never of the member. (The BVH walk triages most
        // below-cut placements inline before this fn is reached.)
        let cw = (child.rbbox.x1 - child.rbbox.x0).max(0) as u64;
        let ch = (child.rbbox.y1 - child.rbbox.y0).max(0) as u64;
        // a sparse placement the wide policy expanded instead of
        // washing (expand_sparse) passes the cut here on purpose
        if !structural
            && !self.sparse_edges.contains(&pli)
            && ((cw < self.cut && ch < self.cut)
                || cw.min(ch) < self.hair)
        {
            self.st.cull_size += 1;
            return;
        }
        let child_r = if r == REM_FULL {
            REM_FULL
        } else {
            self.norm_r(h.child, r - 1)
        };
        let ckey = (h.child, child_r);
        match h.kind {
            0 => {
                let mut vis = false;
                for b in boxes {
                    if !b0.intersects(b) {
                        continue;
                    }
                    let c = xf_bbox(&t0.invert(), b)
                        .intersect(&child.rbbox);
                    if !c.is_empty() {
                        vis = true;
                        self.contribute(ckey, c);
                    }
                }
                if vis {
                    wc.insts.push(WsInst {
                        child: ckey,
                        x: h.x,
                        y: h.y,
                        rot: h.rot,
                        flip: h.flip,
                        rep: Rep::One,
                    });
                    self.st.inst_edges += 1;
                }
            }
            1 => {
                let mut vis = false;
                for b in boxes {
                    let rbox = minkowski_neg(b, &b0);
                    let (i0, i1, j0, j1) = match grid_ranges(
                        h.na as i64,
                        h.nb as i64,
                        h.va,
                        h.vb,
                        &rbox,
                    ) {
                        GridVis::Empty => continue,
                        GridVis::Range { i0, i1, j0, j1 } => {
                            (i0, i1, j0, j1)
                        }
                    };
                    if h.na > 1
                        && h.nb > 1
                        && grid_degenerate(h.va, h.vb)
                    {
                        self.st.grid_fallback_full += 1;
                    }
                    // O_vis = bbox of the visible-index corners;
                    // localview(child) gains T0^-1(b (+) -O_vis)
                    // clipped to rbbox (par.2.3 step 3)
                    let ovis = grid_ovis(i0, i1, j0, j1, h.va, h.vb);
                    let shifted = minkowski_neg(b, &ovis);
                    let c = xf_bbox(&t0.invert(), &shifted)
                        .intersect(&child.rbbox);
                    if !c.is_empty() {
                        vis = true;
                        self.contribute(ckey, c);
                    }
                }
                if vis {
                    // ONE CellInstArray, full na x nb: off-view
                    // members are klayout's clip problem; nesting
                    // stays nested (zero expansion)
                    wc.insts.push(WsInst {
                        child: ckey,
                        x: h.x,
                        y: h.y,
                        rot: h.rot,
                        flip: h.flip,
                        rep: Rep::Grid {
                            na: h.na as u64,
                            nb: h.nb as u64,
                            va: h.va,
                            vb: h.vb,
                        },
                    });
                    self.st.inst_edges += 1;
                }
            }
            _ => {
                let pr = self.v.pts_ref(pli).expect("pts kind");
                let count = pr.count;
                let ext = pr.extent();
                self.st.pts_enumerated += 1;
                // one scan feeds BOTH the emission selection (slot
                // identity, par.2.3) and per-box O_vis - the ladder:
                // exact scan (small) / chunk-pruned scan / whole-
                // chunk inclusion on a dry budget / extent. Every
                // rung only ever over-includes.
                let mut sel: BTreeSet<u32> = BTreeSet::new();
                let mut contribs: Vec<BBox> = Vec::new();
                for b in boxes {
                    let rbox = minkowski_neg(b, &b0);
                    if !ext.intersects(&rbox) {
                        continue; // extent is the FIRST-PASS reject
                    }
                    let mut ovis = BBox::EMPTY;
                    if count <= self.opts.pts_full_rep {
                        // small rep: ALWAYS exact-scan, never against
                        // the budget - 8192xK point tests are
                        // microseconds, and the old budget-dry
                        // fallback (O_vis = extent) let ONE drained
                        // request balloon every later child localview
                        // to its whole cell: the view=cwb pathology
                        // reborn through pts (9.8G field case, depth
                        // 9 at a 0.1um view: 29k pages / 2.3G
                        // members). The enum budget bounds the BIG
                        // scans below.
                        self.st.pts_offsets_scanned += count as u64;
                        for s in 0..count {
                            let (ox, oy) = pr.pt(s);
                            if rbox.contains_pt(ox, oy) {
                                sel.insert(s);
                                ovis.grow(&pt_box(ox, oy));
                            }
                        }
                    } else {
                        for k in 0..pr.n_chunks {
                            let cb = pr.chunk_bbox(k);
                            if !cb.intersects(&rbox) {
                                continue;
                            }
                            let (lo, hi) = pr.chunk_range(k);
                            let n = (hi - lo) as u64;
                            if self.pts_budget >= n {
                                self.pts_budget -= n;
                                self.st.pts_offsets_scanned += n;
                                for s in lo..hi {
                                    let (ox, oy) = pr.pt(s);
                                    if rbox.contains_pt(ox, oy) {
                                        sel.insert(s);
                                        ovis.grow(&pt_box(ox, oy));
                                    }
                                }
                            } else {
                                // dry budget: include the WHOLE
                                // chunk (bounded over-inclusion,
                                // zero omission)
                                self.st.pts_fallback += 1;
                                for s in lo..hi {
                                    sel.insert(s);
                                }
                                ovis.grow(&cb);
                            }
                        }
                    }
                    if !ovis.is_empty() {
                        let shifted = minkowski_neg(b, &ovis);
                        let c = xf_bbox(&t0.invert(), &shifted)
                            .intersect(&child.rbbox);
                        if !c.is_empty() {
                            contribs.push(c);
                        }
                    }
                }
                if contribs.is_empty() {
                    return;
                }
                for c in contribs {
                    self.contribute(ckey, c);
                }
                // emission: small -> ALL slots (full rep); big ->
                // selected subset. Both REBASE (par.2.3): the pool
                // is Morton-ordered, so even "full" re-anchors on
                // its first slot.
                let emit: Vec<u32> = if count <= self.opts.pts_full_rep
                {
                    (0..count).collect()
                } else {
                    sel.into_iter().collect()
                };
                if emit.is_empty() {
                    return;
                }
                self.st.pts_selected += emit.len() as u64;
                let sub = |a: i64, b: i64| -> i64 {
                    i64::try_from(a as i128 - b as i128)
                        .expect("pts rebase overflow")
                };
                let (dx, dy, rep) = if emit.len() == 1 {
                    let (ox, oy) = pr.pt(emit[0]);
                    (ox, oy, Rep::One)
                } else {
                    let (ox0, oy0) = pr.pt(emit[0]);
                    let mut pts = Vec::with_capacity(emit.len());
                    pts.push((0i64, 0i64));
                    for &s in &emit[1..] {
                        let (ox, oy) = pr.pt(s);
                        pts.push((sub(ox, ox0), sub(oy, oy0)));
                    }
                    (ox0, oy0, Rep::Pts(pts.into()))
                };
                self.st.pts_offsets_emitted += emit.len() as u64;
                self.st.pts_bytes_emitted += 16 * emit.len() as u64;
                wc.insts.push(WsInst {
                    child: ckey,
                    x: h.x + dx,
                    y: h.y + dy,
                    rot: h.rot,
                    flip: h.flip,
                    rep,
                });
                self.st.inst_edges += 1;
            }
        }
    }
}

fn pt_box(x: i64, y: i64) -> BBox {
    BBox { x0: x, y0: y, x1: x, y1: y }
}

fn grid_degenerate(va: (i64, i64), vb: (i64, i64)) -> bool {
    va.0 as i128 * vb.1 as i128 - va.1 as i128 * vb.0 as i128 == 0
}

/// widen a base box by an offset-extent box (member footprints)
fn grow_by_offsets(base: &BBox, off: &BBox) -> BBox {
    if base.is_empty() || off.is_empty() {
        return BBox::EMPTY;
    }
    BBox {
        x0: base.x0.saturating_add(off.x0),
        y0: base.y0.saturating_add(off.y0),
        x1: base.x1.saturating_add(off.x1),
        y1: base.y1.saturating_add(off.y1),
    }
}

/// hierarchy-frontier outline layer (drawn hollow by the viewer).
/// Runtime frame/outline layer, dt 0: one past the highest DESIGN
/// layer number, stepping DOWN to the nearest unused number when
/// that saturates (u32::MAX is a legal design layer - review
/// round 3), so the result never collides with real content for
/// any layer set. The viewer derives the same value from meta's
/// layer list with the IDENTICAL rule.
pub fn frame_layer(v: &Ovm) -> (u32, u32) {
    let used: HashSet<u32> =
        (0..v.n_layers).map(|li| v.layer(li).layer).collect();
    let mut fl = used
        .iter()
        .max()
        .map_or(1, |m| m.saturating_add(1))
        .max(1);
    while used.contains(&fl) {
        fl -= 1; // only reachable when max == u32::MAX; the used
                 // set is finite, so an unused number exists below
    }
    (fl, 0)
}

/// gen-ephemeral working-set cell name `W<gen>_<r>_<ci>`, r = "F"
/// for the full-depth sentinel (par.3.1): digits/fixed chars only -
/// design names travel once via the names= table, never in cell
/// names (whitespace/unicode hygiene).
pub fn ws_name(gen: u64, key: WsKey) -> String {
    if key.1 == REM_FULL {
        format!("W{}_F_{}", gen, key.0)
    } else {
        format!("W{}_{}_{}", gen, key.1, key.0)
    }
}

impl crate::Vfs {
    /// V4 hierarchy-preserving plan (default knobs). The page
    /// hairline policy is the request's (ViewReq::page_hairline);
    /// FLOE_RUST_PAGE_HAIRLINE=cull|keep overrides it for diagnosis.
    pub fn plan_hier(&self, req: &ViewReq) -> HierPlan {
        match std::env::var("FLOE_RUST_PAGE_HAIRLINE").as_deref() {
            Ok("cull") | Ok("keep") => {
                let mut req = req.clone();
                req.page_hairline = std::env::var("FLOE_RUST_PAGE_HAIRLINE").as_deref() == Ok("cull");
                plan_hier(&self.ovm, &req, &HierOpts::default())
            }
            _ => plan_hier(&self.ovm, req, &HierOpts::default()),
        }
    }

    /// hier delta (par.3.2): ONE OASIS = new pages spliced verbatim
    /// + authored working-set cells (identity page instances, child
    /// CellInstArray edges, frame rects on FRAME_LAYER). Resident
    /// pages are referenced by name with no definition in the file
    /// (M0-verified klayout name binding). The gen's top WC is the
    /// file's single top, so the delta stays parse_doc-able.
    pub fn delta_hier(
        &self,
        plan: &HierPlan,
        new_pages: &[u32],
        gen: u64,
        avail: Option<&std::collections::HashSet<u32>>,
    ) -> Result<Vec<u8>, String> {
        use floe_oasis::doc::RectRec;
        use floe_oasis::write::{
            splice_tree, tree_body, write_tree, WCell,
        };
        let payloads = self.read_page_payloads(new_pages)?;
        let (frame_fl, frame_fd) = frame_layer(&self.ovm);
        // owned storage first - WCell borrows names/rects/reps
        struct Pre {
            name: String,
            rects: Vec<RectRec>,
            places: Vec<(String, i64, i64, u8, bool, Rep)>,
        }
        let pre: Vec<Pre> = plan
            .wcells
            .iter()
            .map(|w| {
                let rects: Vec<RectRec> = w
                    .frames
                    .iter()
                    .map(|(b, rep, band)| RectRec {
                        layer: frame_fl,
                        // dt+band selects the viewer draw style:
                        // 0 white outline, 1 gray outline, 2 gray
                        // fill, 3 gray dotted
                        dt: frame_fd + *band as u32,
                        x: b.x0,
                        y: b.y0,
                        w: (b.x1 - b.x0).max(1),
                        h: (b.y1 - b.y0).max(1),
                        rep: rep.clone(),
                    })
                    .chain(w.washes.iter().map(|(li, b)| {
                        // M7-C: sub-wash_px page -> one rect on the
                        // page's own design layer (normal fill, so
                        // the viewer's speckle thins it like any
                        // geometry - the Calibre wash texture)
                        let lv = self.ovm.layer(*li);
                        RectRec {
                            layer: lv.layer,
                            dt: lv.dt,
                            x: b.x0,
                            y: b.y0,
                            w: (b.x1 - b.x0).max(1),
                            h: (b.y1 - b.y0).max(1),
                            rep: Rep::One,
                        }
                    }))
                    .collect();
                let mut places =
                    Vec::with_capacity(w.pages.len() + w.insts.len());
                for &pi in &w.pages {
                    // streaming (par.5 M3.5): a partial delta must
                    // only reference MATERIALIZED pages - a name
                    // with no definition anywhere would make
                    // klayout mint an empty ghost cell that nothing
                    // ever evicts (review finding)
                    if let Some(a) = avail {
                        if !a.contains(&pi) {
                            continue;
                        }
                    }
                    places.push((
                        self.page_name(pi),
                        0,
                        0,
                        0u8,
                        false,
                        Rep::One,
                    ));
                }
                for i in &w.insts {
                    places.push((
                        ws_name(gen, i.child),
                        i.x,
                        i.y,
                        i.rot,
                        i.flip,
                        i.rep.clone(),
                    ));
                }
                Pre { name: ws_name(gen, w.key), rects, places }
            })
            .collect();
        let wcells: Vec<WCell> = pre
            .iter()
            .map(|p| WCell {
                name: p.name.clone(),
                rects: &p.rects,
                polys: &[],
                paths: &[],
                texts: &[],
                places: p
                    .places
                    .iter()
                    .map(|(n, x, y, r, f, rep)| {
                        (n.as_str(), *x, *y, *r, *f, rep)
                    })
                    .collect(),
            })
            .collect();
        let mut bodies: Vec<&[u8]> =
            payloads.iter().map(|p| tree_body(p)).collect();
        let authored;
        if !wcells.is_empty() {
            authored = write_tree(&wcells, self.ovm.unit)
                .map_err(|e| e.to_string())?;
            bodies.push(tree_body(&authored));
        }
        Ok(splice_tree(self.ovm.unit, &bodies))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_ovm::Builder;
    use floe_tiler::hier::rep_extent;

    fn bx(x0: i64, y0: i64, x1: i64, y1: i64) -> BBox {
        BBox { x0, y0, x1, y1 }
    }

    fn rq_px(
        view: BBox,
        cut: i64,
        depth: u32,
        px_per_dbu: f64,
    ) -> ViewReq {
        ViewReq {
            view,
            cut_dbu: cut,
            vis: vec![0xff],
            depth,
            px_per_dbu,
            sub_cut_wash: false,
            page_reps: false,
            decode_budget: 0,
                    page_hairline: false,
                    page_skip: Vec::new(),
                    prune_skipped: false,
                    sub_cut_box: false,
        }
    }

    /// M7 density gate: a dense page swaps for its LOD twin only
    /// when members outnumber its on-screen pixels by k; probes
    /// (px_per_dbu = 0) and zoom-ins stay exact
    #[test]
    fn lod_density_gate_swaps_and_reverts() {
        let mut b = Builder::new(1000.0, 0, 0, 1);
        b.top = 0;
        b.layer(1, 0, "L1", 1, 1_000_000);
        let m = b.bitset(&[1]);
        let pb = bx(0, 0, 100_000, 100_000);
        b.page(
            0, 0, 0, &pb, 0, 0, 0, 1, 1_000_000, 50, 50,
            floe_ovm::LOD_EXACT, 1,
        );
        b.page(
            0, 0, 0, &pb, 0, 0, 0, 1, 4_000, 1000, 1000,
            floe_ovm::LOD_MERGED, floe_ovm::LOD_PAGE_NONE,
        );
        let pr = b.prange(0, 0, 1, PBVH_NONE);
        b.cell(
            "T", 0, 0, &pb, &pb, 0, 0, 0, 2, 0, 0, pr, 1, m, m,
            1_000_000, 0, 0, m,
        );
        let ovm = Ovm::from_bytes(b.finish(0, 0)).unwrap();
        let o = HierOpts::default();
        // zoomed OUT: page is ~100 px on screen -> 10^4 px^2,
        // members 10^6 > 4*10^4 -> LOD
        let p1 = plan_hier(
            &ovm,
            &rq_px(pb, 0, u32::MAX, 0.001),
            &o,
        );
        assert_eq!(p1.pages, vec![1], "{:?}", p1.pages);
        assert_eq!(p1.stats.lod_swapped, 1);
        // zoomed IN: 10^10 px^2 -> exact
        let p2 =
            plan_hier(&ovm, &rq_px(pb, 0, u32::MAX, 1.0), &o);
        assert_eq!(p2.pages, vec![0], "{:?}", p2.pages);
        // deep zoom into a CORNER SLICE of the dense page: the old
        // gate compared whole-page members against the tiny
        // intersection's pixels and swapped to LOD exactly when
        // zoomed in (review finding) - grid cells would be ~780 px
        // blocks. Whole-page + fidelity precondition keeps exact.
        let p2b = plan_hier(
            &ovm,
            &rq_px(bx(0, 0, 100, 100), 0, u32::MAX, 1.0),
            &o,
        );
        assert_eq!(p2b.pages, vec![0], "{:?}", p2b.pages);
        assert_eq!(p2b.stats.lod_swapped, 0);
        // probe (px 0): exact by construction
        let p3 =
            plan_hier(&ovm, &rq_px(pb, 0, u32::MAX, 0.0), &o);
        assert_eq!(p3.pages, vec![0]);
        // tiny-DBU dense page at px_per_dbu > 1: the emitted
        // cells are 1 DBU = several px - fidelity gate keeps exact
        // even though the page spans < 128 px (review finding)
        {
            let mut b = Builder::new(1000.0, 0, 0, 1);
            b.top = 0;
            b.layer(1, 0, "L1", 1, 1_000_000);
            let m = b.bitset(&[1]);
            let tb = bx(0, 0, 20, 20);
            b.page(
                0, 0, 0, &tb, 0, 0, 0, 1, 1_000_000, 1, 1,
                floe_ovm::LOD_EXACT, 1,
            );
            b.page(
                0, 0, 0, &tb, 0, 0, 0, 1, 100, 20, 20,
                floe_ovm::LOD_MERGED, floe_ovm::LOD_PAGE_NONE,
            );
            let pr = b.prange(0, 0, 1, PBVH_NONE);
            b.cell(
                "S", 0, 0, &tb, &tb, 0, 0, 0, 2, 0, 0, pr, 1, m,
                m, 1_000_000, 0, 0, m,
            );
            let ovm2 = Ovm::from_bytes(b.finish(0, 0)).unwrap();
            let ps = plan_hier(
                &ovm2,
                &rq_px(tb, 0, u32::MAX, 4.0),
                &HierOpts::default(),
            );
            assert_eq!(ps.pages, vec![0], "{:?}", ps.pages);
            assert_eq!(ps.stats.lod_swapped, 0);
        }
        // kill switch (lod_k 0): exact
        let mut off = HierOpts::default();
        off.lod_k = 0.0;
        let p4 = plan_hier(
            &ovm,
            &rq_px(pb, 0, u32::MAX, 0.001),
            &off,
        );
        assert_eq!(p4.pages, vec![0]);
    }

    /// frame layer stays collision-free even when u32::MAX is a
    /// real design layer (review round 3)
    #[test]
    fn frame_layer_never_collides() {
        let mut b = Builder::new(1000.0, 0, 0, 3);
        b.top = 0;
        b.layer(5, 0, "A", 0, 0);
        b.layer(u32::MAX, 0, "B", 0, 0);
        b.layer(u32::MAX - 1, 0, "C", 0, 0);
        let m = b.bitset(&[1]);
        let pb = bx(0, 0, 10, 10);
        b.cell("T", 0, 0, &pb, &pb, 0, 0, 0, 0, 0, 0, 0, 0, m, m, 0,
               0, 0, m);
        let ovm = Ovm::from_bytes(b.finish(0, 0)).unwrap();
        let (fl, _) = frame_layer(&ovm);
        for li in 0..ovm.n_layers {
            assert_ne!(fl, ovm.layer(li).layer);
        }
    }

    /// depth 0 on a pure-hierarchy top must NOT be empty: every
    /// direct child renders as an outline frame (review finding -
    /// the depth-0 open default was a black screen when the top
    /// had no geometry of its own)
    #[test]
    fn depth_zero_emits_child_frames() {
        let v = fixture(
            &[
                FCell {
                    name: "CHILD",
                    pages: vec![(bx(0, 0, 1000, 1000), 500, 500)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![],
                    places: vec![
                        (0, 0, 0, 0, false, Rep::One),
                        (0, 5000, 0, 0, false, Rep::One),
                    ],
                },
            ],
            1,
        );
        let req = rq(bx(-100, -100, 10_000, 10_000), 0, 0);
        let plan = plan_hier(&v, &req, &HierOpts::default());
        assert!(plan.pages.is_empty(), "{:?}", plan.pages);
        assert!(
            plan.stats.frame_rects >= 2,
            "no frames at depth 0: {}",
            plan.stats.frame_rects
        );
        let mut off = HierOpts::default();
        off.frame_cap = 0;
        let no_frames = plan_hier(&v, &req, &off);
        assert_eq!(no_frames.stats.frame_rects, 0);
        assert!(no_frames.wcells.iter().all(|w| w.frames.is_empty()));
        let mut empty_req = req.clone();
        empty_req.vis.fill(0);
        let empty = plan_hier(&v, &empty_req, &off);
        assert!(empty.wcells.is_empty() && empty.pages.is_empty());
    }

    /// Size cut is an omission contract. An all-layer recursive bbox
    /// is not a truthful proxy for the selected layer, so full-depth
    /// planning must not emit FRAME_LAYER geometry for it.
    #[test]
    fn below_cut_child_emits_no_false_frame() {
        let v = fixture(
            &[
                FCell {
                    name: "SMALL",
                    pages: vec![(bx(0, 0, 5, 5), 5, 5)],
                    places: vec![],
                },
                FCell {
                    name: "TOP",
                    pages: vec![(bx(0, 0, 4000, 100), 4000, 100)],
                    places: vec![(0, 100, 0, 0, false, Rep::One)],
                },
            ],
            1,
        );
        let plan = plan_hier(
            &v,
            &rq(bx(0, 0, 4000, 100), 10, u32::MAX),
            &HierOpts::default(),
        );
        assert_eq!(plan.stats.frame_rects, 0);
        assert!(!plan.wcells.iter().any(|w| w.key.0 == 0));
        assert!(plan.pages.iter().all(|&pi| v.page(pi).cell != 0));
    }

    /// A depth-boundary outline is structural and deliberately does
    /// not belong to the visible-layer selection. Design pages remain
    /// filtered while the same frontier survives layers=none.
    #[test]
    fn depth_frame_ignores_visible_layer_mask() {
        let mut b = Builder::new(1000.0, 0, 0, 2);
        b.top = 1;
        b.layer(1, 0, "CHILD_ONLY", 0, 0);
        b.layer(2, 0, "TOP_ONLY", 0, 0);
        let m0 = b.bitset(&[0b01]);
        let m1 = b.bitset(&[0b10]);
        let both = b.bitset(&[0b11]);

        let child_bb = bx(0, 0, 100, 100);
        b.page(
            0, 0, 0, &child_bb, 0, 0, 0, 1, 1, 100, 100,
            floe_ovm::LOD_EXACT, floe_ovm::LOD_PAGE_NONE,
        );
        let child_pr = b.prange(0, 0, 1, PBVH_NONE);
        b.cell(
            "CHILD", 0, 1, &child_bb, &child_bb, 0, 0, 0, 1, 0, 0,
            child_pr, 1, m0, m0, 1, 0, 0, m0,
        );

        let place_start = b.n_places() as u32;
        b.place(0, 1000, 0, 0, false, &Rep::One);
        let placed = bx(1000, 0, 1100, 100);
        let bvh =
            b.bvh_node(&placed, place_start, 1, true, 100, 100);
        let top_own = bx(0, 0, 10, 10);
        b.page(
            1, 1, 0, &top_own, 0, 0, 0, 1, 1, 10, 10,
            floe_ovm::LOD_EXACT, floe_ovm::LOD_PAGE_NONE,
        );
        let top_pr = b.prange(1, 1, 1, PBVH_NONE);
        let top_bb = bx(0, 0, 1100, 100);
        b.cell(
            "TOP", 1, 0, &top_own, &top_bb, place_start, 1, 1, 1,
            bvh, 1, top_pr, 1, m1, both, 2, 0, 0, both,
        );
        let v = Ovm::from_bytes(b.finish(0, 0)).unwrap();
        let req = ViewReq {
            view: bx(0, 0, 1200, 200),
            cut_dbu: 0,
            vis: vec![0b10],
            depth: 0,
            px_per_dbu: 0.0,
                    sub_cut_wash: false,
                    page_reps: false,
                    decode_budget: 0,
                    page_hairline: false,
                    page_skip: Vec::new(),
                    prune_skipped: false,
                    sub_cut_box: false,
        };
        let plan = plan_hier(&v, &req, &HierOpts::default());
        assert_eq!(plan.pages, vec![1]);
        assert_eq!(plan.stats.frame_rects, 1);

        let none = ViewReq {
            vis: vec![0],
            ..req
        };
        let structural = plan_hier(&v, &none, &HierOpts::default());
        assert!(structural.pages.is_empty());
        assert_eq!(structural.stats.frame_rects, 1);
    }

    /// Each finite depth exposes only its own frontier. Once the
    /// requested depth reaches the top height, normalization folds to
    /// full and the structural overlay disappears.
    #[test]
    fn depth_frontiers_are_distinct_non_cumulative() {
        let v = fixture(
            &[
                FCell {
                    name: "LEAF",
                    pages: vec![(bx(0, 0, 20, 20), 20, 20)],
                    places: vec![],
                },
                FCell {
                    name: "MID",
                    pages: vec![],
                    places: vec![(0, 100, 0, 0, false, Rep::One)],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![(1, 1000, 0, 0, false, Rep::One)],
                },
            ],
            2,
        );
        let view = bx(0, 0, 2000, 200);
        let p0 = plan_hier(&v, &rq(view, 0, 0), &HierOpts::default());
        assert_eq!(p0.stats.frame_rects, 1);
        assert!(p0.wcells.iter().any(|w| w.key == (2, 0)
            && w.frames.len() == 1));
        assert!(!p0.wcells.iter().any(|w| w.key.0 == 1));

        let p1 = plan_hier(&v, &rq(view, 0, 1), &HierOpts::default());
        assert_eq!(p1.stats.frame_rects, 1);
        assert!(p1.wcells.iter().any(|w| w.key == (1, 0)
            && w.frames.len() == 1));
        assert!(p1.wcells.iter().any(|w| w.key.0 == 2
            && w.frames.is_empty()));

        let p2 = plan_hier(&v, &rq(view, 0, 2), &HierOpts::default());
        assert_eq!(p2.top, (2, REM_FULL));
        assert_eq!(p2.stats.frame_rects, 0);
    }

    /// A sub-cut subtree FOLDS at a finite depth (rev 31): no edge,
    /// no descent - its pages can never pass the cut and its deeper
    /// frontier would be sub-pixel. The 150M field case put 4.06M
    /// such edges in one delta while the drawable page set was
    /// identical to depth full. Rev 33: the fold is also SILENT
    /// (no proxy box) - frames are a pure function of depth, so
    /// frames on and off now agree everywhere below the boundary.
    #[test]
    fn finite_depth_folds_sub_cut_subtrees() {
        let v = fixture(
            &[
                FCell {
                    name: "TINY",
                    pages: vec![(bx(0, 0, 8, 8), 8, 8)],
                    places: vec![],
                },
                FCell {
                    name: "SMALL", // 8x8 child -> ~9x9 subtree
                    pages: vec![],
                    places: vec![(0, 1, 1, 0, false, Rep::One)],
                },
                FCell {
                    name: "BIG",
                    pages: vec![(bx(0, 0, 900, 900), 900, 900)],
                    places: vec![],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![
                        (1, 0, 0, 0, false, Rep::One),
                        (2, 1000, 0, 0, false, Rep::One),
                    ],
                },
            ],
            3,
        );
        // cut 50: SMALL's whole subtree is sub-cut, BIG is not.
        // depth 1 (< height 2, so it does NOT fold to full) would
        // previously make SMALL an edge and frame TINY below it.
        let req = rq(bx(-10, -10, 2000, 1000), 50, 1);
        let plan = plan_hier(&v, &req, &HierOpts::default());
        // BIG's page ships; TINY's page is cut-culled everywhere
        assert_eq!(plan.pages.len(), 1);
        assert_eq!(v.page(plan.pages[0]).cell, 2);
        // the sub-cut subtree folded SILENTLY (rev 33) and expanded
        // BIG carries no outline (rev 37: boxes live only at their
        // own depth boundary); neither SMALL nor TINY got a
        // working-set cell
        assert_eq!(plan.stats.frame_rects, 0);
        let top = plan.wcells.iter().find(|w| w.key.0 == 3).unwrap();
        assert!(top.frames.is_empty());
        assert!(!plan.wcells.iter().any(|w| w.key.0 <= 1));
        // the only edge is TOP -> BIG
        assert_eq!(plan.stats.inst_edges, 1);
        // frames off: the fold is silent (no frame, still no edge)
        let mut off = HierOpts::default();
        off.frame_cap = 0;
        let p2 = plan_hier(&v, &req, &off);
        assert_eq!(p2.stats.frame_rects, 0);
        assert_eq!(p2.stats.inst_edges, 1);
        assert_eq!(p2.pages, plan.pages);
    }

    /// M7-C: a page whose whole screen image fits wash_px in both
    /// axes collapses to one rect on its own layer (any member
    /// subset paints the same blob; exact geometry saturates into
    /// a stroke wall). Zooming in, wash_px 0, px 0 (probe), and a
    /// masked layer all keep/restore the exact behavior.
    #[test]
    fn sub_pixel_pages_wash_to_layer_rects() {
        let v = fixture(
            &[FCell {
                name: "T",
                pages: vec![(bx(0, 0, 1000, 1000), 500, 500)],
                places: vec![],
            }],
            0,
        );
        let view = bx(-10, -10, 1100, 1100);
        // 1000 dbu * 0.001 px/dbu = 1px <= wash_px 2
        let p = plan_hier(
            &v,
            &rq_px(view, 0, 0, 0.001),
            &HierOpts::default(),
        );
        assert_eq!(p.stats.washed_pages, 1);
        assert!(p.pages.is_empty());
        let t = p.wcells.iter().find(|w| w.key.0 == 0).unwrap();
        assert_eq!(t.washes, vec![(0u32, bx(0, 0, 1000, 1000))]);
        assert!(t.pages.is_empty());
        // zoomed in (100px): geometry returns
        let pz = plan_hier(
            &v,
            &rq_px(view, 0, 0, 0.1),
            &HierOpts::default(),
        );
        assert_eq!(pz.stats.washed_pages, 0);
        assert_eq!(pz.pages.len(), 1);
        // kill switch: wash_px 0 ships geometry even sub-pixel
        let mut off = HierOpts::default();
        off.wash_px = 0.0;
        let pk = plan_hier(&v, &rq_px(view, 0, 0, 0.001), &off);
        assert_eq!(pk.stats.washed_pages, 0);
        assert_eq!(pk.pages.len(), 1);
        // no screen scale (probe parity): exact
        let pn =
            plan_hier(&v, &rq(view, 0, 0), &HierOpts::default());
        assert_eq!(pn.stats.washed_pages, 0);
        assert_eq!(pn.pages.len(), 1);
        // layer masked off: neither geometry nor wash
        let mut mreq = rq_px(view, 0, 0, 0.001);
        mreq.vis = vec![0];
        let pv = plan_hier(&v, &mreq, &HierOpts::default());
        assert_eq!(pv.stats.washed_pages, 0);
        assert!(pv.wcells.iter().all(|w| w.washes.is_empty()));
    }

    /// Rev 41 hairline cut: everything the size cut touches also
    /// culls when the MIN side is under hairline * cut - pages via
    /// the v6 max_min field, folds and frames via the box min side.
    /// A sub-hair wire is a 1px stroke however long it is; at wide
    /// views it only builds walls the speckle cannot thin.
    #[test]
    fn a_plan_over_its_decode_budget_is_planned_at_the_finest_cut_that_fits() {
        // field 2026-09-18: keep + detail high is the closest picture to
        // Calibre, but wide views ended in "decoded generation budget
        // exceeded". Size classes in one cell: 8 squares of 100, 4 of 145,
        // 4 of 400, 2 of 1600, a 6400 x 10 hairline (and later a giant).
        let mut pages = Vec::new();
        for i in 0..8 {
            pages.push((bx(i * 200, 0, i * 200 + 100, 100), 100, 100));
        }
        for i in 0..4 {
            pages.push((bx(i * 300, 500, i * 300 + 145, 645), 145, 145));
        }
        for i in 0..4 {
            pages.push((bx(i * 800, 1000, i * 800 + 400, 1400), 400, 400));
        }
        for i in 0..2 {
            pages.push((bx(i * 3200, 3000, i * 3200 + 1600, 4600), 1600, 1600));
        }
        pages.push((bx(0, 6000, 6400, 6010), 6400, 10));
        let small = fixture(&[FCell { name: "TOP", pages: pages.clone(), places: vec![] }], 0);
        let view = bx(-10, -10, 20_000_000, 20_000_000);
        let per = page_memory(1, 0);
        // cut 50 dbu = 1 px (detail high): medium's 3 px is 150 dbu, low's 5 px 250
        let ask = |cut: i64, budget: u64, hairline: bool| {
            let mut r = rq(view, cut, u32::MAX);
            r.px_per_dbu = 0.02;
            r.decode_budget = budget;
            r.page_hairline = hairline;
            r
        };
        // the cut ladder (FLOE_RUST_FIT_THIN=off); the density fit has its own test
        let fitted = |v: &Ovm, r: &ViewReq| plan_hier(v, r, &HierOpts { fit_thin: false, ..HierOpts::default() });
        let as_asked = |v: &Ovm, r: &ViewReq| plan_hier(v, r, &HierOpts { fit_budget: false, ..HierOpts::default() });
        // the oracle: the first rung (ascending) whose plain plan fits
        let oracle = |v: &Ovm, r: &ViewReq| -> Option<(i64, usize)> {
            std::iter::once(r.cut_dbu).chain(fit_rungs(r.cut_dbu, r.px_per_dbu)).find_map(|cut| {
                let mut at = r.clone();
                at.cut_dbu = cut;
                let plan = as_asked(v, &at);
                (plan.stats.fit_bytes <= r.decode_budget).then_some((cut, plan.pages.len()))
            })
        };
        let rungs = fit_rungs(50, 0.02);
        assert!(rungs.windows(2).all(|w| w[0] < w[1]) && rungs[0] > 50);
        assert!(rungs.contains(&150) && rungs.contains(&250), "the detail cuts are rungs: {rungs:?}");
        // no budget, a roomy one, the kill switch, an exact request: as asked
        for r in [ask(50, 0, false), ask(50, 19 * per, false), ask(0, per, false)] {
            let plan = fitted(&small, &r);
            assert_eq!((plan.stats.fit_pct, plan.stats.fit_cull, plan.stats.fit_over), (0, 0, false));
            assert_eq!(plan.pages, as_asked(&small, &r).pages);
        }
        assert_eq!(fitted(&small, &ask(50, 19 * per, false)).stats.fit_passes, 1);
        // every budget lands on the finest rung that fits, pages included
        for pages_allowed in [11u64, 10, 7, 6, 3, 2, 1] {
            let r = ask(50, pages_allowed * per, false);
            let plan = fitted(&small, &r);
            let (cut, n) = oracle(&small, &r).expect("a keep rung fits");
            assert_eq!(
                (plan.pages.len(), plan.stats.fit_pct, plan.stats.fit_cull, plan.stats.fit_over),
                (n, if cut == 50 { 0 } else { ((cut as f64 / 50.0) * 100.0).round() as u32 }, 0, false),
                "{pages_allowed} pages allowed"
            );
            assert!(plan.stats.fit_bytes <= r.decode_budget && plan.stats.fit_passes <= 7, "{:?}", plan.stats.fit_passes);
        }
        // detail high never ends coarser than medium would plan as asked: the
        // 145s go at medium's 150 dbu, which the quarter-octave rungs (141, 168)
        // alone would have stepped over
        let seven = fitted(&small, &ask(50, 7 * per, false));
        assert_eq!((seven.pages.len(), seven.stats.fit_pct), (7, 300));
        assert_eq!(seven.pages, as_asked(&small, &ask(150, 0, false)).pages);
        // less than a page: keep cannot shed the long hairline by size, so
        // the hairlines are culled and the rungs are searched again
        let none = fitted(&small, &ask(50, 1, false));
        assert_eq!((none.pages.len(), none.stats.fit_cull, none.stats.fit_over), (0, 1, false));
        let cull = fitted(&small, &ask(50, 1, true));
        assert_eq!((cull.pages.len(), cull.stats.fit_cull, cull.stats.fit_pct), (0, 0, none.stats.fit_pct));
        // nothing fits a giant: the complete plan of the last rung, flagged
        pages.push((bx(0, 0, 10_000_000, 10_000_000), 10_000_000, 10_000_000));
        let giant = fixture(&[FCell { name: "TOP", pages, places: vec![] }], 0);
        let over = fitted(&giant, &ask(50, 1, false));
        assert_eq!((over.pages.len(), over.stats.fit_cull, over.stats.fit_over, over.stats.fit_pct), (1, 1, true, 6400));
    }

    #[test]
    fn a_plan_over_its_decode_budget_keeps_its_cut_and_lowers_the_density() {
        // field 2026-09-19 (synthetic MAIN01, thin keep): five zoom steps of
        // empty screen - the view's shapes are ONE size class, so the finest
        // cut that fitted selected nothing. Sixteen 200-squares (4 px: above
        // the 2 px page wash; class 7 = 128..255 dbu) and two of 1600 (class
        // 10) in one (cell, layer) run; cut 50.
        let mut pages = Vec::new();
        for i in 0..16 {
            pages.push((bx(i * 400, 0, i * 400 + 200, 200), 200, 200));
        }
        for i in 0..2 {
            pages.push((bx(i * 3200, 3000, i * 3200 + 1600, 4600), 1600, 1600));
        }
        let chip = fixture(&[FCell { name: "TOP", pages: pages.clone(), places: vec![] }], 0);
        let view = bx(-10, -10, 20_000_000, 20_000_000);
        let per = page_memory(1, 0);
        let ask = |budget: u64| {
            let mut r = rq(view, 50, u32::MAX);
            r.px_per_dbu = 0.02;
            r.decode_budget = budget;
            r
        };
        let fitted = |v: &Ovm, r: &ViewReq| plan_hier(v, r, &HierOpts::default());
        let ladder = |v: &Ovm, r: &ViewReq| plan_hier(v, r, &HierOpts { fit_thin: false, ..HierOpts::default() });
        let fit = |p: &HierPlan| (p.stats.fit_pct, p.stats.fit_thin, p.stats.fit_full_pct, p.stats.fit_none_pct, p.stats.fit_passes);
        // a plan that fits is the plan as asked, in one pass
        let roomy = fitted(&chip, &ask(18 * per));
        assert_eq!((roomy.pages.len(), fit(&roomy)), (18, (0, 0, 0, 0, 1)));
        // six pages: the ladder sheds the whole class of 200s ...
        let six = ask(6 * per);
        assert_eq!(ladder(&chip, &six).pages, vec![16, 17]);
        // ... the density fit keeps the 1600s and an even quarter of the 200s
        // (van der Corput: 0, 8, 4, 12, 2, ...), in one pass
        let plan = fitted(&chip, &six);
        assert_eq!(plan.pages, vec![0, 4, 8, 12, 16, 17]);
        assert_eq!((fit(&plan), plan.stats.fit_over, plan.stats.fit_bytes), ((100, 2, 2048, 0, 1), false, 6 * per));
        assert_eq!(plan.wcells[0].pages, plan.pages, "the working cell is thinned with the plan");
        assert_eq!(plan.page_prio.len(), plan.pages.len());
        // every smaller budget keeps a subset, every larger one a superset
        let mut last: Option<Vec<u32>> = None;
        for budget in 1..=18u64 {
            let now = fitted(&chip, &ask(budget * per + per / 2)).pages;
            assert_eq!(now.len() as u64, budget);
            if let Some(before) = &last {
                assert!(before.iter().all(|p| now.contains(p)), "budget {budget}: {before:?} -> {now:?}");
            }
            last = Some(now);
        }
        assert_eq!(fitted(&chip, &ask(3 * per)).pages, vec![0, 16, 17]);
        // two pages: eighteen are more than FIT_OVERSHOOT budgets, so the passes
        // at 50, 64 and 128 are abandoned and the one at 256 - the class of the
        // 1600s - fits exactly
        let two = fitted(&chip, &ask(2 * per));
        assert_eq!((two.pages.clone(), fit(&two)), (vec![16, 17], (512, 0, 0, 512, 4)));
        // a page and a half: one of the two
        let one = fitted(&chip, &ask(per + per / 2));
        assert_eq!((one.pages.clone(), fit(&one), one.stats.fit_over), (vec![16], (512, 1, 0, 512, 4), false));
        // the class below: forty 200s and one 1600 against four pages. The
        // passes up to 128 are over FIT_OVERSHOOT budgets, the one at 256 holds
        // one page and leaves room - the prefix ends in the class below, so the
        // cut of 128 is planned to the end: three 200s stay with the 1600,
        // where the ladder shows the 1600 alone
        let mut cliff = Vec::new();
        for i in 0..40 {
            cliff.push((bx(i * 400, 0, i * 400 + 200, 200), 200, 200));
        }
        cliff.push((bx(0, 3000, 1600, 4600), 1600, 1600));
        let cliff = fixture(&[FCell { name: "TOP", pages: cliff, places: vec![] }], 0);
        let four = fitted(&cliff, &ask(4 * per));
        assert_eq!((four.pages.clone(), fit(&four)), (vec![0, 16, 32, 40], (256, 4, 2048, 256, 5)));
        assert_eq!(ladder(&cliff, &ask(4 * per)).pages, vec![40]);
        // single-page runs (small cells): every other CELL stays, by cell index
        let cells: Vec<FCell> = (0..8)
            .map(|_| FCell { name: "LEAF", pages: vec![(bx(0, 0, 200, 200), 200, 200)], places: vec![] })
            .chain(std::iter::once(FCell {
                name: "TOP",
                pages: vec![],
                places: (0..8).map(|i| (i, i as i64 * 1000, 0, 0, false, Rep::One)).collect(),
            }))
            .collect();
        let leaves = fixture(&cells, 8);
        let half = fitted(&leaves, &ask(4 * per));
        assert_eq!((half.pages.len(), half.stats.fit_thin), (4, 1));
        assert!(half.pages.iter().all(|&p| leaves.page(p).cell % 2 == 0));
        assert!(half.wcells.iter().all(|w| w.pages.iter().all(|p| half.pages.contains(p))));
        // not even the first page fits: the complete plan, flagged, as before
        pages.push((bx(0, 0, 10_000_000, 10_000_000), 10_000_000, 10_000_000));
        let giant = fixture(&[FCell { name: "TOP", pages, places: vec![] }], 0);
        let over = fitted(&giant, &ask(1));
        assert_eq!((over.pages.len(), over.stats.fit_over), (1, true));
    }

    #[test]
    fn narrowing_the_view_never_removes_a_budget_fitted_page_that_stays_in_view() {
        // review 2026-09-19 of 0.12.166: the 2^k sample topped up with the
        // largest pages kept {0, 1, 8} in a wide view and {0, 4, 8} in a
        // narrower one - page 1 vanished on the way in, still in view. Three
        // size classes in a row along x; the views are prefixes of the row.
        let mut pages = Vec::new();
        for i in 0..48i64 {
            let side = [200, 200, 200, 450, 200, 1600][(i % 6) as usize];
            pages.push((bx(i * 2000, 0, i * 2000 + side, side), side as u64, side as u64));
        }
        let chip = fixture(&[FCell { name: "TOP", pages, places: vec![] }], 0);
        let per = page_memory(1, 0);
        let ask = |x1: i64, budget: u64| {
            let mut r = rq(bx(-10, -10, x1, 5000), 50, u32::MAX);
            r.px_per_dbu = 0.02;
            r.decode_budget = budget;
            r
        };
        let in_view = |x1: i64| plan_hier(&chip, &ask(x1, 0), &HierOpts::default()).pages;
        let widths = [96_000i64, 61_000, 40_000, 23_000, 9_000];
        for budget in [2u64, 3, 5, 8, 13, 21] {
            let mut wide: Option<Vec<u32>> = None;
            for &x1 in &widths {
                let plan = plan_hier(&chip, &ask(x1, budget * per), &HierOpts::default());
                assert!(!plan.stats.fit_over && plan.stats.fit_bytes <= budget * per);
                let visible = in_view(x1);
                if let Some(before) = &wide {
                    let lost: Vec<u32> = before.iter().copied().filter(|p| visible.contains(p) && !plan.pages.contains(p)).collect();
                    assert!(lost.is_empty(), "budget {budget}, view to {x1}: {lost:?} left the plan but not the view");
                }
                wide = Some(plan.pages);
            }
        }
        // and the largest class is never the one to go
        let plan = plan_hier(&chip, &ask(96_000, 10 * per), &HierOpts::default());
        assert!((0..48u32).filter(|p| p % 6 == 5).all(|p| plan.pages.contains(&p)), "{:?}", plan.pages);
    }

    #[test]
    fn what_the_size_cut_drops_stays_as_a_box_under_the_sub_cut_boxes() {
        // field 2026-09-18/19: the keep picture is Calibre-like except that
        // what the size cut drops vanishes (one via layer of the synthetic
        // MAIN01: an empty screen from the fit view to x4). 0.02 px/dbu: the
        // cut of 100 dbu is 2 px, a box is at most 4 px = 200 dbu, a LEAF is
        // 60 dbu = 1.2 px.
        let leaf = FCell { name: "LEAF", pages: vec![(bx(0, 0, 60, 60), 60, 60)], places: vec![] };
        let top = FCell {
            name: "TOP",
            pages: vec![
                (bx(0, 0, 5000, 5000), 5000, 5000),   // drawn as ever
                (bx(6000, 0, 6100, 100), 60, 60),     // size-cut, 2 px: a box
                (bx(8000, 0, 9000, 1000), 60, 60),    // size-cut, 20 px: vanishes as before
            ],
            places: vec![
                (0, 0, 7000, 0, false, Rep::One),
                // 50 members that abut (pitch = size): one run
                (0, 0, 8000, 0, false, Rep::Grid { na: 50, nb: 1, va: (60, 0), vb: (0, 0) }),
                // 5 x 2 members 20 px apart: ten boxes
                (0, 0, 9000, 0, false, Rep::Grid { na: 5, nb: 2, va: (1000, 0), vb: (0, 1000) }),
                // review 2026-09-19: 1.2 px members at a 3 px pitch, 30 x 30. Closer
                // than a box, but they do not touch: 900 member boxes, not one 89 px fill
                (0, 0, 12_000, 0, false, Rep::Grid { na: 30, nb: 30, va: (150, 0), vb: (0, 150) }),
            ],
        };
        let chip = fixture(&[leaf, top], 1);
        let view = bx(-10, -10, 20_000, 20_000);
        let ask = |boxes: bool, depth: u32| {
            let mut r = rq(view, 100, depth);
            r.px_per_dbu = 0.02;
            r.sub_cut_box = boxes;
            r.vis = vec![1];        // the fixture's one layer (the cap counts visible layers)
            r
        };
        let plain = plan_hier(&chip, &ask(false, u32::MAX), &HierOpts::default());
        assert!(plain.wcells.iter().all(|w| w.washes.is_empty()) && plain.stats.sub_cut_boxes == 0);
        let plan = plan_hier(&chip, &ask(true, u32::MAX), &HierOpts::default());
        // nothing more is decoded, nothing more is expanded
        assert_eq!((plan.pages.clone(), plan.wcells.len()), (plain.pages.clone(), plain.wcells.len()));
        let washes = &plan.wcells.iter().find(|w| w.key.0 == 1).unwrap().washes;
        let has = |b: BBox| washes.iter().filter(|(layer, wash)| *layer == 0 && *wash == b).count();
        assert_eq!(has(bx(6000, 0, 6100, 100)), 1, "the small size-cut page: {washes:?}");
        assert_eq!(has(bx(8000, 0, 9000, 1000)), 0, "a wide size-cut page is no box");
        assert_eq!(has(bx(0, 7000, 60, 7060)), 1, "the single child");
        assert_eq!(has(bx(0, 8000, 49 * 60 + 60, 8060)), 1, "members that abut are one run");
        for (i, j) in [(0, 0), (4, 0), (0, 1), (4, 1)] {
            assert_eq!(has(bx(i * 1000, 9000 + j * 1000, i * 1000 + 60, 9060 + j * 1000)), 1, "sparse member {i},{j}");
        }
        for (i, j) in [(0, 0), (29, 0), (13, 7), (29, 29)] {
            assert_eq!(has(bx(i * 150, 12_000 + j * 150, i * 150 + 60, 12_060 + j * 150)), 1, "member {i},{j} of the 3 px array");
        }
        assert!(washes.iter().all(|(_, b)| b.x1 - b.x0 <= 3000 && b.y1 - b.y0 <= 200), "no box beyond the run and the 4 px: {washes:?}");
        assert_eq!((washes.len(), plan.stats.sub_cut_boxes, plan.stats.sub_cut_box_over, plan.stats.sub_cut_box_level), (913, 913, 0, 0));
        // the same request, the same boxes; a narrow view boxes only what it sees
        assert_eq!(&plan_hier(&chip, &ask(true, u32::MAX), &HierOpts::default()).wcells, &plan.wcells);
        let mut narrow = ask(true, u32::MAX);
        narrow.view = bx(3900, 8900, 4200, 9200);
        let seen = plan_hier(&chip, &narrow, &HierOpts::default());
        let seen = &seen.wcells.iter().find(|w| w.key.0 == 1).unwrap().washes;
        // (the planner's local view is wider than the request by its margin, so a
        // neighbour or two come along; the far members do not)
        assert!(seen.iter().any(|(_, b)| *b == bx(4000, 9000, 4060, 9060)), "{seen:?}");
        assert!(seen.len() < 10 && seen.iter().all(|(_, b)| b.x0 >= 3000 && b.y0 >= 8000), "{seen:?}");
        // depth 0: children are outlines only, the page box stays
        let flat = plan_hier(&chip, &ask(true, 0), &HierOpts::default());
        assert_eq!(flat.stats.sub_cut_boxes, 1);
        // more layers visible than the cap, a hidden layer, an exact request: none
        let none = plan_hier(&chip, &ask(true, u32::MAX), &HierOpts { sub_cut_box_layers: 0, ..HierOpts::default() });
        assert_eq!(none.stats.sub_cut_boxes, 0);
        let mut hidden = ask(true, u32::MAX);
        hidden.vis = vec![0];
        assert_eq!(plan_hier(&chip, &hidden, &HierOpts::default()).stats.sub_cut_boxes, 0);
        let mut exact = ask(true, u32::MAX);
        exact.cut_dbu = 0;
        assert_eq!(plan_hier(&chip, &exact, &HierOpts::default()).stats.sub_cut_boxes, 0);
        // past the cap the whole plan goes one level coarser - boxes of 8 px,
        // every second member - instead of leaving the cells walked last empty
        let coarser = plan_hier(&chip, &ask(true, u32::MAX), &HierOpts { sub_cut_box_max: 400, ..HierOpts::default() });
        assert_eq!((coarser.stats.sub_cut_box_level, coarser.stats.sub_cut_box_over), (1, 0));
        let washes = &coarser.wcells.iter().find(|w| w.key.0 == 1).unwrap().washes;
        assert_eq!(washes.iter().filter(|(_, b)| b.y0 >= 12_000).count(), 15 * 15, "every second member of the 30 x 30");
        // a cluster of children no wider than a box is ONE box, and nothing below it is visited
        let cluster = fixture(
            &[
                FCell { name: "LEAF", pages: vec![(bx(0, 0, 60, 60), 60, 60)], places: vec![] },
                FCell {
                    name: "TOP",
                    pages: vec![(bx(0, 0, 5000, 5000), 5000, 5000)],
                    places: vec![(0, 9000, 9000, 0, false, Rep::One), (0, 9070, 9000, 0, false, Rep::One), (0, 9000, 9070, 0, false, Rep::One)],
                },
            ],
            1,
        );
        let one = plan_hier(&cluster, &ask(true, u32::MAX), &HierOpts::default());
        assert_eq!((one.stats.sub_cut_boxes, one.stats.sub_cut_box_nodes), (1, 1));
        let washes = &one.wcells.iter().find(|w| w.key.0 == 1).unwrap().washes;
        assert_eq!(washes.as_slice(), &[(0, bx(9000, 9000, 9130, 9130))]);
    }

    #[test]
    fn a_sub_cut_box_shows_only_what_the_requested_depth_holds() {
        // review 2026-09-19: the box took the child's RECURSIVE layer mask, so a
        // depth-1 view showed boxes for shapes that live at depth 2. MID holds
        // no shape of its own, only LEAFs; DEEP only a MID.
        let cells = [
            FCell { name: "LEAF", pages: vec![(bx(0, 0, 30, 30), 30, 30)], places: vec![] },
            FCell { name: "MID", pages: vec![], places: vec![(0, 0, 0, 0, false, Rep::One), (0, 40, 0, 0, false, Rep::One)] },
            FCell { name: "DEEP", pages: vec![], places: vec![(1, 0, 0, 0, false, Rep::One)] },
            FCell {
                name: "TOP",
                pages: vec![(bx(0, 0, 5000, 5000), 5000, 5000)],
                places: vec![
                    (1, 6000, 0, 0, false, Rep::One),       // MID: shapes one level below it
                    (2, 8000, 0, 0, false, Rep::One),       // DEEP: shapes two levels below it
                    (0, 10_000, 0, 0, false, Rep::One),     // LEAF: its own shapes
                ],
            },
            // the same three kinds within one 4 px node (the fixture's BVH is one
            // leaf per cell, so the cluster gets a cell of its own)
            FCell {
                name: "CLUSTER",
                pages: vec![(bx(0, 0, 5000, 5000), 5000, 5000)],
                places: vec![(1, 12_000, 0, 0, false, Rep::One), (2, 12_080, 0, 0, false, Rep::One), (2, 12_000, 80, 0, false, Rep::One)],
            },
        ];
        let boxes_of = |top: usize, depth: u32, masks: bool| {
            let chip = fixture_with(&cells[..=top], top, masks);
            let mut r = rq(bx(-10, -10, 20_000, 20_000), 100, depth);
            r.px_per_dbu = 0.02;
            r.sub_cut_box = true;
            r.vis = vec![1];
            let plan = plan_hier(&chip, &r, &HierOpts::default());
            let mut out: Vec<i64> = plan.wcells.iter().flat_map(|w| w.washes.iter().map(|(_, b)| b.x0)).collect();
            out.sort();
            (out, plan.stats.sub_cut_box_nodes, plan.stats.sub_cut_box_reads)
        };
        // the v8 node masks and the placement reads give the same answer
        let boxes_at = |top: usize, depth: u32| {
            let (with, without) = (boxes_of(top, depth, true), boxes_of(top, depth, false));
            assert_eq!((&with.0, with.1), (&without.0, without.1), "top {top} depth {depth}");
            (with.0, with.1)
        };
        // depth 1 draws TOP and its children's OWN shapes: the LEAF alone
        assert_eq!(boxes_at(3, 1).0, vec![10_000]);
        // depth 2 reaches the LEAFs inside MID, not DEEP's
        assert_eq!(boxes_at(3, 2).0, vec![6000, 10_000]);
        // depth 3 and full depth: all of them
        assert_eq!(boxes_at(3, 3).0, vec![6000, 8000, 10_000]);
        assert_eq!(boxes_at(3, u32::MAX), boxes_at(3, 3));
        // the node box asks the same question of every placement below it
        assert_eq!(boxes_at(4, 1), (vec![], 0));
        assert_eq!(boxes_at(4, 2), (vec![12_000], 1));
        assert_eq!(boxes_at(4, u32::MAX), (vec![12_000], 1));
        // at full depth and one level above the depth boundary the masks answer
        // without reading a placement; in between they only bound the answer
        assert_eq!((boxes_of(4, u32::MAX, true).2, boxes_of(4, 1, true).2), (0, 0));
        assert!(boxes_of(4, 2, true).2 > 0 && boxes_of(4, u32::MAX, false).2 > 0);
    }

    #[test]
    fn a_sub_cut_box_is_one_rect_on_the_topmost_layer_it_stands_for() {
        // 0.12.171: one rect per box, not one per layer - the boxes of the
        // layers underneath cover the same pixels and are overwritten. A
        // cluster of a LEAF on L1/0 and a LEAF on L2/0 within 4 px.
        let cells = [
            FCell { name: "LEAF", pages: vec![(bx(0, 0, 60, 60), 60, 60)], places: vec![] },
            FCell { name: "LEAF@2", pages: vec![(bx(0, 0, 60, 60), 60, 60)], places: vec![] },
            FCell {
                name: "TOP",
                pages: vec![(bx(0, 0, 5000, 5000), 5000, 5000)],
                places: vec![(0, 9000, 9000, 0, false, Rep::One), (1, 9070, 9000, 0, false, Rep::One), (0, 9000, 9070, 0, false, Rep::One)],
            },
        ];
        let boxes = |vis: u8, masks: bool| {
            let chip = fixture_with(&cells, 2, masks);
            let mut r = rq(bx(-10, -10, 20_000, 20_000), 100, u32::MAX);
            r.px_per_dbu = 0.02;
            r.sub_cut_box = true;
            r.vis = vec![vis];
            let plan = plan_hier(&chip, &r, &HierOpts::default());
            (plan.wcells.iter().flat_map(|w| w.washes.clone()).collect::<Vec<_>>(), plan.stats.sub_cut_boxes)
        };
        let node = bx(9000, 9000, 9130, 9130);
        for masks in [true, false] {
            // both visible: one rect, on L2/0 (painted over L1/0)
            assert_eq!(boxes(0b11, masks), (vec![(1, node)], 1), "masks {masks}");
            // "everything visible" sets the padding bits of the byte too
            assert_eq!(boxes(0xff, masks), (vec![(1, node)], 1));
            // one visible: that layer
            assert_eq!(boxes(0b01, masks), (vec![(0, node)], 1));
            assert_eq!(boxes(0b10, masks), (vec![(1, node)], 1));
        }
    }

    #[test]
    fn a_subtree_without_a_visible_layer_is_pruned_by_its_node_mask() {
        // v8: the per-placement cull_layer test, asked of the node
        let cells = [
            FCell { name: "LEAF", pages: vec![(bx(0, 0, 500, 500), 500, 500)], places: vec![] },
            // TOP's own page is on the second layer, the LEAFs' on the first
            FCell {
                name: "TOP@2",
                pages: vec![(bx(0, 2000, 5000, 7000), 5000, 5000)],
                places: (0..6).map(|i| (0, i as i64 * 1000, 0, 0, false, Rep::One)).collect(),
            },
        ];
        let plan_of = |masks: bool, vis: u8, depth: u32, frames: bool| {
            let chip = fixture_with(&cells, 1, masks);
            let mut r = rq(bx(-10, -10, 20_000, 20_000), 100, depth);
            r.px_per_dbu = 0.02;
            r.vis = vec![vis];
            let opts = HierOpts { frame_cap: if frames { 200_000 } else { 0 }, ..HierOpts::default() };
            plan_hier(&chip, &r, &opts)
        };
        // the LEAFs' layer hidden: one node test instead of six placements
        let (with, without) = (plan_of(true, 0b10, u32::MAX, true), plan_of(false, 0b10, u32::MAX, true));
        assert_eq!((with.stats.culled_bvh_layer, with.stats.cull_layer), (1, 0));
        assert_eq!((without.stats.culled_bvh_layer, without.stats.cull_layer), (0, 6));
        assert_eq!((&with.pages, &with.wcells), (&without.pages, &without.wcells));
        assert_eq!(with.pages.len(), 1, "TOP's own page");
        // visible: nothing is pruned, the same plan
        let (with, without) = (plan_of(true, 0b11, u32::MAX, true), plan_of(false, 0b11, u32::MAX, true));
        assert_eq!((with.stats.culled_bvh_layer, &with.pages, &with.wcells), (0, &without.pages, &without.wcells));
        // a depth boundary draws hierarchy frames whatever the layers: no prune
        // while frames may come, the same frames either way
        let (with, without) = (plan_of(true, 0b10, 0, true), plan_of(false, 0b10, 0, true));
        assert_eq!(with.stats.culled_bvh_layer, 0);
        assert!(with.stats.frame_rects > 0 && with.wcells == without.wcells);
        assert_eq!(plan_of(true, 0b10, 0, false).stats.culled_bvh_layer, 1);
    }

    #[test]
    fn explain_rows_name_the_rule_that_dropped_a_region() {
        // field diagnosis 2026-09-10: which rule dropped what, in
        // the view - a hairline page (cull_hair), a fat page kept
        // exact, a sub-cut child cell omitted at full depth
        let v = fixture(
            &[
                FCell {
                    name: "TINY",
                    pages: vec![(bx(0, 0, 50, 50), 50, 50)],
                    places: vec![],
                },
                FCell {
                    name: "MIX",
                    pages: vec![
                        (bx(0, 0, 4000, 100), 4000, 100),
                        (bx(0, 300, 500, 800), 500, 500),
                    ],
                    places: vec![],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![
                        (0, 0, 0, 0, false, Rep::One),
                        (1, 6000, 0, 0, false, Rep::One),
                    ],
                },
            ],
            2,
        );
        let view = bx(-10, -10, 11_000, 1000);
        let mut o = HierOpts::default();
        o.explain = true;
        let mut cull_req = rq(view, 300, u32::MAX);
        cull_req.page_hairline = true;
        let p = plan_hier(&v, &cull_req, &o);
        let name = |ci: u32| v.cell(ci).name.clone();
        let rows: Vec<(String, &str, &str)> = p
            .explain
            .iter()
            .map(|r| (name(r.cell), r.kind, r.verdict))
            .collect();
        assert!(rows.contains(&("MIX".to_string(), "page", "cull_hair")), "{:?}", rows);
        assert!(rows.contains(&("MIX".to_string(), "page", "exact")), "{:?}", rows);
        // the default keeps the thin page and says so
        let mut od = HierOpts::default();
        od.explain = true;
        let pd = plan_hier(&v, &rq(view, 300, u32::MAX), &od);
        let rows_d: Vec<(String, &str, &str)> = pd
            .explain
            .iter()
            .map(|r| (name(r.cell), r.kind, r.verdict))
            .collect();
        assert!(rows_d.contains(&("MIX".to_string(), "page", "exact_thin")), "{:?}", rows_d);
        assert!(!rows_d.iter().any(|r| r.2 == "cull_hair"));
        assert!(rows.contains(&("TINY".to_string(), "child", "omit_size")), "{:?}", rows);
        assert!(rows.contains(&("MIX".to_string(), "child", "expand")), "{:?}", rows);
        assert!(rows.iter().any(|r| r.1 == "top" && r.2 == "keep"));
        let hair = p.explain.iter().find(|r| r.verdict == "cull_hair").unwrap();
        assert_eq!((hair.w, hair.h, hair.min), (4000, 100, 100));
        // off by default: no rows
        let q = plan_hier(&v, &rq(view, 300, u32::MAX), &HierOpts::default());
        assert!(q.explain.is_empty());
    }

    #[test]
    fn hairline_min_side_cut() {
        let v = fixture(
            &[
                FCell {
                    name: "FAT",
                    pages: vec![(bx(0, 0, 500, 500), 500, 500)],
                    places: vec![],
                },
                FCell {
                    name: "MIX", // fat rbbox, one hairline page
                    pages: vec![
                        (bx(0, 0, 4000, 100), 4000, 100),
                        (bx(0, 300, 500, 800), 500, 500),
                    ],
                    places: vec![],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![
                        (0, 0, 0, 0, false, Rep::One),
                        (1, 6000, 0, 0, false, Rep::One),
                    ],
                },
            ],
            2,
        );
        let view = bx(-10, -10, 11_000, 1000);
        // cut 300 -> hair 150: MIX's hairline page (max_min 100)
        // culled by the v6 field even though its long side is far
        // above the cut AND the cell rbbox is fat; MIX's fat page
        // and FAT survive - WITH the page hairline rule on (the
        // rev 41 behaviour, FLOE_RUST_PAGE_HAIRLINE=cull)
        let mut cull_req = rq(view, 300, u32::MAX);
        cull_req.page_hairline = true;
        let p = plan_hier(&v, &cull_req, &HierOpts::default());
        assert_eq!(p.pages.len(), 2);
        assert!(p.pages.iter().all(
            |&pi| v.page(pi).max_min >= 150
        ));
        assert!(p.stats.cull_page_size >= 1);
        assert_eq!(p.stats.thin_pages_kept, 0);
        // the default since 2026-09-10 (field: 81-124 nm lines up to
        // 119 um long vanished with their pages): the thin page is
        // KEPT and counted; the raster draws its lines as 1 px
        // hairlines of their full length
        let pd = plan_hier(&v, &rq(view, 300, u32::MAX), &HierOpts::default());
        assert_eq!(pd.pages.len(), 3);
        assert_eq!(pd.stats.thin_pages_kept, 1);
        assert_eq!(pd.stats.cull_page_size, 0);
        // hairline 0 restores the pure both-dims rule
        let mut off = HierOpts::default();
        off.hairline = 0.0;
        let p0 = plan_hier(&v, &cull_req, &off);
        assert_eq!(p0.pages.len(), 3);
        // boundary: hair == max_min exactly (cut 200 -> hair 100)
        // is NOT below - the thin page stays
        let mut pb_req = rq(view, 200, u32::MAX);
        pb_req.page_hairline = true;
        let pb = plan_hier(&v, &pb_req, &HierOpts::default());
        assert_eq!(pb.pages.len(), 3);
        // fold + frame follow: THIN placed one level down folds at
        // r>0 (min side 100 < 150) and gets no boundary box at r==0
        let v2 = fixture(
            &[
                FCell {
                    name: "THINLEAF",
                    pages: vec![(bx(0, 0, 4000, 100), 4000, 100)],
                    places: vec![],
                },
                FCell {
                    name: "MID",
                    pages: vec![],
                    places: vec![(0, 0, 0, 0, false, Rep::One)],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![(1, 0, 0, 0, false, Rep::One)],
                },
            ],
            2,
        );
        let p2 = plan_hier(
            &v2,
            &rq(view, 300, 2),
            &HierOpts::default(),
        );
        assert!(p2.pages.is_empty());
        // rev 43: the v7 node annotation prunes the uniformly-thin
        // subtree before the per-place fold even runs
        assert!(
            p2.stats.cull_size + p2.stats.culled_bvh_size >= 1
        );
        assert!(p2.stats.culled_bvh_size >= 1);
        let p3 = plan_hier(
            &v2,
            &rq(view, 300, 1),
            &HierOpts::default(),
        );
        assert_eq!(p3.stats.frame_rects, 0);
    }

    /// Rev 45 (Calibre alignment): a boundary box whose MIN side is
    /// under the cut no longer vanishes - deterministic
    /// representatives on a layout-fixed lattice survive (interval
    /// bound offsets, 2 per bin in a row, demoting to 1 when a
    /// lattice pitch is small on screen) until BOTH sides are under
    /// the cut. Grids thin in closed form: stride
    /// ceil(lattice/pitch) sub-grids, no member enumeration.
    #[test]
    fn thin_frames_sample_on_lattice() {
        let v = fixture(
            &[
                FCell {
                    name: "THIN", // 300 x 100 member box
                    pages: vec![(bx(0, 0, 300, 100), 300, 100)],
                    places: vec![],
                },
                FCell {
                    name: "TOP", // row of 100 at 200 DBU pitch
                    pages: vec![],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 100,
                            nb: 1,
                            va: (200, 0),
                            vb: (0, 0),
                        },
                    )],
                },
            ],
            1,
        );
        let view = bx(-10, -10, 30_000, 1_000);
        // cut 300: min side 100 < 300 <= max side 300 -> thin band.
        // px 0.01: one 7000-DBU lattice pitch = 70px >= demote 14 ->
        // stride-35 sub-grids at the bound offsets {0, 34}
        let p = plan_hier(
            &v,
            &rq_px(view, 300, 0, 0.01),
            &HierOpts::default(),
        );
        assert_eq!(p.stats.thin_frames, 1);
        let top =
            p.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(top.frames.len(), 2);
        let members: u64 =
            top.frames.iter().map(|(_, r, _)| r.members()).sum();
        // ceil(100/35) = 3 at offset 0 (i = 0,35,70) + 2 at
        // offset 34 (i = 34,69)
        assert_eq!(members, 5);
        for (b, rep, band) in &top.frames {
            match rep {
                Rep::Grid { va, .. } => {
                    assert_eq!(*va, (7000, 0))
                }
                r => panic!("expected grid, got {:?}", r),
            }
            assert_eq!(b.y0, 0);
            assert_eq!(*band, 3, "1px min side -> dotted");
        }
        let mut xs: Vec<i64> =
            top.frames.iter().map(|(b, _, _)| b.x0).collect();
        xs.sort();
        assert_eq!(xs, vec![0, 6800]); // offsets 0 and 34 * 200
        // one lattice pitch under thin_demote_px on screen (7px):
        // a single representative per bin (the observed 2 -> 1
        // ladder)
        let pd = plan_hier(
            &v,
            &rq_px(view, 300, 0, 0.001),
            &HierOpts::default(),
        );
        let td =
            pd.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(td.frames.len(), 1);
        assert_eq!(td.frames[0].1.members(), 3);
        // cut 0: no thin band, the full rep as before
        let p0 = plan_hier(
            &v,
            &rq_px(view, 0, 0, 0.01),
            &HierOpts::default(),
        );
        let t0 =
            p0.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(t0.frames.len(), 1);
        assert_eq!(t0.frames[0].1.members(), 100);
        // lattice off restores the rev 41 hairline cull
        // (min 100 < hair 150)
        let mut off = HierOpts::default();
        off.thin_lattice_um = 0.0;
        let pf = plan_hier(&v, &rq_px(view, 300, 0, 0.01), &off);
        assert_eq!(pf.stats.frame_rects, 0);
        assert_eq!(pf.stats.thin_frames, 0);
        // BOTH sides under the cut: gone entirely, lattice or not
        let pb = plan_hier(
            &v,
            &rq_px(view, 500, 0, 0.01),
            &HierOpts::default(),
        );
        assert_eq!(pb.stats.frame_rects, 0);
        assert_eq!(pb.stats.thin_frames, 0);
    }

    /// Rev 46: minimap frontier = the plan's frame set expanded to
    /// world space through the WS instance tree (xf composition +
    /// rep members on both the frame and the instance edge), with
    /// a deterministic spatial keep.
    #[test]
    fn frontier_boxes_expand_ws_world_space() {
        let v = fixture(
            &[
                FCell {
                    name: "THIN",
                    pages: vec![(bx(0, 0, 300, 100), 300, 100)],
                    places: vec![],
                },
                FCell {
                    name: "MID",
                    pages: vec![],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 3,
                            nb: 1,
                            va: (1000, 0),
                            vb: (0, 0),
                        },
                    )],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![
                        (1, 10_000, 0, 0, false, Rep::One),
                        (1, 0, 20_000, 0, false, Rep::One),
                    ],
                },
            ],
            2,
        );
        let view = bx(-10, -10, 40_000, 40_000);
        let p = plan_hier(
            &v,
            &rq(view, 0, 1),
            &HierOpts::default(),
        );
        let (fb, trunc) = frontier_boxes(&v, &p, 6000);
        assert!(!trunc, "tiny fixture must not hit the budget");
        assert_eq!(fb.len(), 6);
        let mut got: Vec<(i64, i64)> =
            fb.iter().map(|(b, _)| (b[0], b[1])).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                (0, 20_000),
                (1_000, 20_000),
                (2_000, 20_000),
                (10_000, 0),
                (11_000, 0),
                (12_000, 0),
            ]
        );
        // keep cap trims via the deterministic round-robin
        assert_eq!(frontier_boxes(&v, &p, 4).0.len(), 4);
        // determinism: same plan, same bytes
        assert_eq!(fb, frontier_boxes(&v, &p, 6000).0);
    }

    /// Rev 45 One/Pts thin frames: one deterministic representative
    /// per (child, lattice bin) - the first record in placement
    /// order wins; distinct children own distinct slots. Pts
    /// subsets rebase on their first kept member ((0,0)-first
    /// emission convention).
    #[test]
    fn thin_singles_and_pts_dedupe_per_bin() {
        let v = fixture(
            &[
                FCell {
                    name: "A",
                    pages: vec![(bx(0, 0, 300, 100), 300, 100)],
                    places: vec![],
                },
                FCell {
                    name: "B",
                    pages: vec![(bx(0, 0, 300, 100), 300, 100)],
                    places: vec![],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![
                        (0, 0, 0, 0, false, Rep::One),
                        // same 7000-DBU bin as x = 0: deduped
                        (0, 500, 0, 0, false, Rep::One),
                        (0, 1_000, 0, 0, false, Rep::One),
                        // next bin: its own representative
                        (0, 8_000, 0, 0, false, Rep::One),
                        // other child: own slot in bin 0
                        (1, 100, 0, 0, false, Rep::One),
                    ],
                },
            ],
            2,
        );
        let view = bx(-10, -10, 30_000, 1_000);
        let p = plan_hier(
            &v,
            &rq_px(view, 300, 0, 0.01),
            &HierOpts::default(),
        );
        assert_eq!(p.stats.thin_frames, 5);
        let top =
            p.wcells.iter().find(|w| w.key.0 == 2).unwrap();
        assert_eq!(top.frames.len(), 3);
        let mut xs: Vec<i64> =
            top.frames.iter().map(|(b, _, _)| b.x0).collect();
        xs.sort();
        assert_eq!(xs, vec![0, 100, 8_000]);
        // pts rep: per-bin representative, rebased on the first
        // kept member
        let v2 = fixture(
            &[
                FCell {
                    name: "A",
                    pages: vec![(bx(0, 0, 300, 100), 300, 100)],
                    places: vec![],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Pts(
                            vec![(0, 0), (100, 0), (7_500, 0)]
                                .into(),
                        ),
                    )],
                },
            ],
            1,
        );
        let p2 = plan_hier(
            &v2,
            &rq_px(view, 300, 0, 0.01),
            &HierOpts::default(),
        );
        let t2 =
            p2.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(t2.frames.len(), 1);
        let (b, rep, _) = &t2.frames[0];
        assert_eq!(b.x0, 0);
        match rep {
            Rep::Pts(pts) => {
                assert_eq!(&pts[..], &[(0, 0), (7_500, 0)])
            }
            r => panic!("expected pts, got {:?}", r),
        }
    }

    /// Rev 37 (final): a box lives ONLY at its own depth boundary,
    /// exactly like its name. A box first shown at depth 1 must be
    /// GONE at depth 2 - no bottomed-leaf persistence in expanded
    /// regions, no collapse-root boxes (the rev 34/36 experiments).
    #[test]
    fn boxes_live_only_at_their_depth_boundary() {
        let v = fixture(
            &[
                FCell {
                    name: "LEAF",
                    pages: vec![(bx(0, 0, 80, 80), 80, 80)],
                    places: vec![],
                },
                FCell {
                    name: "B", // height 1
                    pages: vec![],
                    places: vec![(0, 0, 0, 0, false, Rep::One)],
                },
                FCell {
                    name: "SLEAF",
                    pages: vec![(bx(0, 0, 80, 80), 80, 80)],
                    places: vec![],
                },
                FCell {
                    name: "S", // height 1: fully inside depth 2
                    pages: vec![],
                    places: vec![(2, 0, 0, 0, false, Rep::One)],
                },
                FCell {
                    name: "A", // height 2
                    pages: vec![],
                    places: vec![(1, 0, 0, 0, false, Rep::One)],
                },
                FCell {
                    name: "TOP", // height 3
                    pages: vec![],
                    places: vec![
                        (4, 0, 0, 0, false, Rep::One),
                        (3, 5000, 0, 0, false, Rep::One),
                    ],
                },
            ],
            5,
        );
        let view = bx(-10, -10, 7000, 1000);
        // depth 1 boundary: B and SLEAF boxed (their parents hit
        // r==0), nothing else
        let p1 =
            plan_hier(&v, &rq(view, 0, 1), &HierOpts::default());
        assert_eq!(p1.stats.frame_rects, 2);
        // depth 2: the boundary moved down - LEAF is boxed, and
        // SLEAF's depth-1 box is GONE (S is now fully expanded)
        let p2 =
            plan_hier(&v, &rq(view, 0, 2), &HierOpts::default());
        assert_eq!(p2.stats.frame_rects, 1);
        let b_wc = p2
            .wcells
            .iter()
            .find(|w| w.key == (1, 0))
            .unwrap();
        assert_eq!(b_wc.frames.len(), 1);
        assert!(p2
            .wcells
            .iter()
            .filter(|w| w.key.0 == 3 || w.key.0 == 5)
            .all(|w| w.frames.is_empty()));
        // layers off: identical box sets (structural frontier)
        for (d, want) in [(1u32, 2u64), (2, 1)] {
            let req = ViewReq {
                vis: vec![0],
                ..rq(view, 0, d)
            };
            let p = plan_hier(&v, &req, &HierOpts::default());
            assert_eq!(p.stats.frame_rects, want, "depth {}", d);
        }
    }

    /// Rev 35: Calibre tone split - white only when BOTH drawn
    /// dimensions reach FRAME_WHITE_PX on screen (boundary
    /// inclusive); no screen scale = legacy all-white. The fixed
    /// sizes assume 20px < FRAME_WHITE_PX <= 90px.
    #[test]
    fn frames_split_into_size_bands() {
        // px 0.1: DBU/10 = screen px. Sizes pick one box per band
        // and the two boundary cases (>= is inclusive).
        let v = fixture(
            &[
                FCell {
                    name: "WHITE", // 300 -> 30px: band 0
                    pages: vec![(bx(0, 0, 300, 300), 300, 300)],
                    places: vec![],
                },
                FCell {
                    name: "W_AT", // 250 -> exactly 25px: band 0
                    pages: vec![(bx(0, 0, 250, 250), 250, 250)],
                    places: vec![],
                },
                FCell {
                    name: "GRAY", // 150 -> 15px: band 1 outline
                    pages: vec![(bx(0, 0, 150, 150), 150, 150)],
                    places: vec![],
                },
                FCell {
                    name: "GTHIN", // 4000x150 -> 400x15px: band 1
                    pages: vec![(bx(0, 0, 4000, 150), 4000, 150)],
                    places: vec![],
                },
                FCell {
                    name: "FILL", // 70 -> 7px: band 2 fill
                    pages: vec![(bx(0, 0, 70, 70), 70, 70)],
                    places: vec![],
                },
                FCell {
                    name: "F_AT", // 50 -> exactly 5px: band 2
                    pages: vec![(bx(0, 0, 50, 50), 50, 50)],
                    places: vec![],
                },
                FCell {
                    name: "DOTS", // 40 -> 4px: band 3 dotted
                    pages: vec![(bx(0, 0, 40, 40), 40, 40)],
                    places: vec![],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![
                        (0, 0, 0, 0, false, Rep::One),
                        (1, 500, 0, 0, false, Rep::One),
                        (2, 1000, 0, 0, false, Rep::One),
                        (3, 1500, 0, 0, false, Rep::One),
                        (4, 8000, 0, 0, false, Rep::One),
                        (5, 8500, 0, 0, false, Rep::One),
                        (6, 9000, 0, 0, false, Rep::One),
                    ],
                },
            ],
            7,
        );
        let view = bx(-10, -10, 20000, 1000);
        let plan = plan_hier(
            &v,
            &rq_px(view, 0, 0, 0.1),
            &HierOpts::default(),
        );
        let top =
            plan.wcells.iter().find(|w| w.key.0 == 7).unwrap();
        assert_eq!(top.frames.len(), 7);
        let band = |w: i64| {
            top.frames
                .iter()
                .find(|(b, _, _)| b.x1 - b.x0 == w)
                .unwrap()
                .2
        };
        assert_eq!(band(300), 0, "30px -> white outline");
        assert_eq!(band(250), 0, "exactly 25px -> white");
        assert_eq!(band(150), 1, "15px -> gray outline");
        assert_eq!(band(4000), 1, "long/thin (15px min) -> gray");
        assert_eq!(band(70), 2, "7px -> gray fill");
        assert_eq!(band(50), 2, "exactly 5px -> gray fill");
        assert_eq!(band(40), 3, "4px -> gray dotted");
        // no screen scale: everything stays band 0 (legacy plans)
        let p0 =
            plan_hier(&v, &rq(view, 0, 0), &HierOpts::default());
        let t0 =
            p0.wcells.iter().find(|w| w.key.0 == 7).unwrap();
        assert!(t0.frames.iter().all(|f| f.2 == 0));
    }

    /// Rev 34: r==0 depth-boundary boxes take the size cut exactly
    /// like geometry (Calibre size-cuts its cell boxes) - sub-cut
    /// boundary children are dropped instead of drawn as dust.
    #[test]
    fn boundary_frames_take_the_size_cut() {
        let v = fixture(
            &[
                FCell {
                    name: "TINY",
                    pages: vec![(bx(0, 0, 8, 8), 8, 8)],
                    places: vec![],
                },
                FCell {
                    name: "BIG",
                    pages: vec![(bx(0, 0, 900, 900), 900, 900)],
                    places: vec![],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![
                        (0, 0, 0, 0, false, Rep::One),
                        (1, 1000, 0, 0, false, Rep::One),
                    ],
                },
            ],
            2,
        );
        let view = bx(-10, -10, 2000, 1000);
        let plan =
            plan_hier(&v, &rq(view, 50, 0), &HierOpts::default());
        // only BIG's boundary box survives the size cut
        assert_eq!(plan.stats.frame_rects, 1);
        let top =
            plan.wcells.iter().find(|w| w.key.0 == 2).unwrap();
        assert_eq!(top.frames.len(), 1);
        // cut 0 draws both boundary boxes as before
        let all =
            plan_hier(&v, &rq(view, 0, 0), &HierOpts::default());
        assert_eq!(all.stats.frame_rects, 2);
    }

    fn rq(view: BBox, cut: i64, depth: u32) -> ViewReq {
        ViewReq {
            view,
            cut_dbu: cut,
            vis: vec![0xff],
            depth,
            px_per_dbu: 0.0,
            sub_cut_wash: false,
            page_reps: false,
            decode_budget: 0,
                    page_hairline: false,
                    page_skip: Vec::new(),
                    prune_skipped: false,
                    sub_cut_box: false,
        }
    }

    // ---- fixture: cells listed CHILDREN-FIRST (index order is a
    // topo order, so rank = n-1-ci gives parent < child), heights
    // and recursive bboxes computed here, one layer (L1/0), one
    // linear prange per paged cell, one-leaf instance BVH.
    struct FCell {
        name: &'static str,
        pages: Vec<(BBox, u64, u64)>,
        places: Vec<(usize, i64, i64, u8, bool, Rep)>,
    }

    fn place_bbox(
        rbb: &BBox,
        x: i64,
        y: i64,
        rot: u8,
        flip: bool,
        rep: &Rep,
    ) -> BBox {
        let xf = Xf::place(x, y, rot, flip);
        let base = xf_bbox(&xf, rbb);
        let (ex, ey) = rep_extent(rep);
        BBox {
            x0: base.x0 + ex.0.min(0),
            y0: base.y0 + ey.0.min(0),
            x1: base.x1 + ex.1.max(0),
            y1: base.y1 + ey.1.max(0),
        }
    }

    fn fixture(cells: &[FCell], top: usize) -> Ovm {
        fixture_with(cells, top, true)
    }

    /// `masks` false: an index without the v8 node layer masks (the planner
    /// reads placements instead)
    fn fixture_with(cells: &[FCell], top: usize, masks: bool) -> Ovm {
        let n = cells.len();
        let mut height = vec![0u32; n];
        let mut rbb = vec![BBox::EMPTY; n];
        // the layer of a cell's pages: L1/0 (index 0), or L2/0 (index 1) for a
        // cell whose name ends in "@2"; the recursive layer mask of its subtree
        let layer_of = |ci: usize| u32::from(cells[ci].name.ends_with("@2"));
        let mut shapes = vec![0u8; n];
        for ci in 0..n {
            let own = if cells[ci].pages.is_empty() { 0 } else { 1u8 << layer_of(ci) };
            shapes[ci] = cells[ci].places.iter().fold(own, |mask, (c, ..)| mask | shapes[*c]);
            let mut b = BBox::EMPTY;
            for (pb, _, _) in &cells[ci].pages {
                b.grow(pb);
            }
            for (c, x, y, rot, flip, rep) in &cells[ci].places {
                assert!(*c < ci, "fixture must be children-first");
                height[ci] = height[ci].max(height[*c] + 1);
                b.grow(&place_bbox(&rbb[*c], *x, *y, *rot, *flip, rep));
            }
            rbb[ci] = b;
        }
        let mut b = Builder::new(1000.0, 0, 0, 1);
        b.top = top as u32;
        b.layer(1, 0, "L1", 0, 0);
        let m1 = b.bitset(&[1]);
        if (0..n).any(|ci| layer_of(ci) == 1) {
            b.layer(2, 0, "L2", 0, 0);
        }
        for ci in 0..n {
            assert!(cells[ci].places.len() <= 8, "one-leaf bvh cap");
            let place_base = b.n_places() as u32;
            let mut items = BBox::EMPTY;
            for (c, x, y, rot, flip, rep) in &cells[ci].places {
                b.place(*c as u32, *x, *y, *rot, *flip, rep);
                items.grow(&place_bbox(
                    &rbb[*c], *x, *y, *rot, *flip, rep,
                ));
            }
            let (bvh_start, bvh_count) =
                if cells[ci].places.is_empty() {
                    (0, 0)
                } else {
                    // v7 size annotations: real values, like the
                    // production build (max over places of the
                    // child rbbox max/min side)
                    let (mut md, mut mn) = (0u32, 0u32);
                    for (c, ..) in &cells[ci].places {
                        let rb = &rbb[*c];
                        let w = (rb.x1 - rb.x0).max(0);
                        let h = (rb.y1 - rb.y0).max(0);
                        let sat = |v: i64| {
                            u32::try_from(v).unwrap_or(u32::MAX)
                        };
                        md = md.max(sat(w.max(h)));
                        mn = mn.max(sat(w.min(h)));
                    }
                    (
                        b.bvh_node(
                            &items,
                            place_base,
                            cells[ci].places.len() as u16,
                            true,
                            md,
                            mn,
                        ),
                        1,
                    )
                };
            let page_start = b.n_pages();
            for (k, (pb, mw, mh)) in
                cells[ci].pages.iter().enumerate()
            {
                b.page(
                    ci as u32, layer_of(ci), k as u32, pb, 0, 0, 0, 1, 1, *mw,
                    *mh, floe_ovm::LOD_EXACT,
                    floe_ovm::LOD_PAGE_NONE,
                );
            }
            let page_count = b.n_pages() - page_start;
            let (pr_start, pr_count) = if page_count > 0 {
                (
                    b.prange(layer_of(ci), page_start, page_count, PBVH_NONE),
                    1u32,
                )
            } else {
                (b.n_pranges(), 0)
            };
            let mask_own = b.bitset(&[if cells[ci].pages.is_empty() { 0 } else { 1u8 << layer_of(ci) }]);
            let mask_rec = b.bitset(&[shapes[ci]]);
            b.cell(
                cells[ci].name,
                height[ci],
                (n - 1 - ci) as u32,
                &rbb[ci],
                &rbb[ci],
                place_base,
                cells[ci].places.len() as u32,
                page_start,
                page_count,
                bvh_start,
                bvh_count,
                pr_start,
                pr_count,
                mask_own,
                mask_rec,
                1,
                0,
                0,
                m1,
            );
        }
        // like the indexer: v8 subtree layer masks on the BVH nodes (every
        // node: the fixture's cells hold at most eight placements)
        if masks {
            b.annotate_bvh_masks_min(2, 1);
        }
        Ovm::from_bytes(b.finish(0, 0)).unwrap()
    }

    // ---- brute reference: every rep member expanded, exact
    // per-path local view, same cut/layer/depth predicates - the
    // correctness oracle. hier must NEVER select fewer pages. The
    // page predicate follows HierOpts::page_hairline like the
    // planner (review 2026-09-11: the oracle still culled thin pages
    // after the default changed, so a planner that dropped one would
    // have passed); `brute` is the default policy, `brute_with` any.
    fn brute(v: &Ovm, req: &ViewReq) -> BTreeSet<u32> {
        brute_with(v, req, &HierOpts::default())
    }

    fn brute_with(v: &Ovm, req: &ViewReq, opts: &HierOpts) -> BTreeSet<u32> {
        #[allow(clippy::too_many_arguments)]
        fn walk(
            v: &Ovm,
            ci: u32,
            xf: &Xf,
            r: u32,
            req: &ViewReq,
            cut: u64,
            hair: u64,
            page_hair: u64,
            out: &mut BTreeSet<u32>,
        ) {
            let cell = v.cell(ci);
            if !masks_intersect(v.bitset(cell.lmask_rec), &walk_vis(req)) {
                return;
            }
            if cell.rbbox.is_empty() {
                return;
            }
            let w = (cell.rbbox.x1 - cell.rbbox.x0).max(0) as u64;
            let h = (cell.rbbox.y1 - cell.rbbox.y0).max(0) as u64;
            if (w < cut && h < cut) || w.min(h) < hair {
                return;
            }
            let wb = xf_bbox(xf, &cell.rbbox);
            if !wb.intersects(&req.view) {
                return;
            }
            let inv = xf.invert();
            let lview = xf_bbox(&inv, &req.view);
            for pi in
                cell.page_start..cell.page_start + cell.page_count
            {
                let p = v.page(pi);
                if !bit_test(&req.vis, p.layer_idx as usize) {
                    continue;
                }
                if !req.page_skip.is_empty()
                    && bit_test(&req.page_skip, p.layer_idx as usize)
                {
                    continue;
                }
                if (p.max_w < cut && p.max_h < cut)
                    || p.max_min < page_hair
                {
                    continue;
                }
                if p.bbox.intersects(&lview) {
                    out.insert(pi);
                }
            }
            if r == 0 {
                return;
            }
            for pli in cell.place_start
                ..cell.place_start + cell.place_count
            {
                let pl = v.place(pli as u64);
                let offs: Vec<(i64, i64)> = match &pl.rep {
                    Rep::One => vec![(0, 0)],
                    Rep::Grid { na, nb, va, vb } => {
                        let mut o = Vec::new();
                        for i in 0..*na as i64 {
                            for j in 0..*nb as i64 {
                                o.push((
                                    i * va.0 + j * vb.0,
                                    i * va.1 + j * vb.1,
                                ));
                            }
                        }
                        o
                    }
                    Rep::Pts(p) => p.to_vec(),
                };
                let cr =
                    if r == REM_FULL { REM_FULL } else { r - 1 };
                for (ox, oy) in offs {
                    let m = xf.compose(&Xf::place(
                        pl.x + ox,
                        pl.y + oy,
                        pl.rot,
                        pl.flip,
                    ));
                    walk(v, pl.child, &m, cr, req, cut, hair, page_hair, out);
                }
            }
        }
        let mut out = BTreeSet::new();
        let r0 = if req.depth == u32::MAX {
            REM_FULL
        } else {
            req.depth
        };
        let cut = req.cut_dbu.max(0) as u64;
        // the planner's rounding of both thresholds; the page policy
        // is the request's, like the planner
        let hair = (cut as f64 * opts.hairline) as u64;
        let page_hair = if req.page_hairline { hair } else { 0 };
        walk(
            v,
            v.top,
            &Xf::identity(),
            r0,
            req,
            cut,
            hair,
            page_hair,
            &mut out,
        );
        out
    }

    fn hier_pages(p: &HierPlan) -> BTreeSet<u32> {
        p.pages.iter().copied().collect()
    }

    fn check(v: &Ovm, req: &ViewReq, expect_equal: bool) -> HierPlan {
        let plan = plan_hier(v, req, &HierOpts::default());
        let b = brute(v, req);
        let h = hier_pages(&plan);
        assert!(
            h.is_superset(&b),
            "MISS: brute {:?} not within hier {:?}",
            b,
            h
        );
        if expect_equal {
            assert_eq!(h, b, "hier over-included");
        }
        plan
    }

    // -------------------------------------------------- pure grid

    #[test]
    fn grid_ranges_no_miss_and_near_tight() {
        let vas = [(7, 0), (0, 7), (7, 3), (-5, 2), (4, 4), (0, 0)];
        let vbs = [(0, 9), (9, 0), (2, -6), (8, 8), (0, 0)];
        let rs = [
            bx(-10, -10, 10, 10),
            bx(0, 0, 0, 0),
            bx(5, 3, 40, 29),
            bx(-40, -33, -5, -6),
            bx(13, -25, 57, -2),
        ];
        for &va in &vas {
            for &vb in &vbs {
                for &(na, nb) in
                    &[(1i64, 1i64), (2, 1), (1, 3), (4, 3), (5, 5)]
                {
                    for r in &rs {
                        let g = grid_ranges(na, nb, va, vb, r);
                        let mut exact = Vec::new();
                        for i in 0..na {
                            for j in 0..nb {
                                let ox = i * va.0 + j * vb.0;
                                let oy = i * va.1 + j * vb.1;
                                if r.contains_pt(ox, oy) {
                                    exact.push((i, j));
                                }
                            }
                        }
                        match g {
                            GridVis::Empty => assert!(
                                exact.is_empty(),
                                "MISS va{:?} vb{:?} n{}x{} r{:?}: {:?}",
                                va, vb, na, nb, r, exact
                            ),
                            GridVis::Range { i0, i1, j0, j1 } => {
                                for &(i, j) in &exact {
                                    assert!(
                                        i0 <= i && i <= i1
                                            && j0 <= j && j <= j1,
                                        "member ({},{}) outside \
                                         [{},{}]x[{},{}] va{:?} \
                                         vb{:?} r{:?}",
                                        i, j, i0, i1, j0, j1, va,
                                        vb, r
                                    );
                                }
                                // tightness claims only where the
                                // method is tight: axis-aligned
                                // invertible (+-1 from floor/ceil)
                                // and the 1-D interval forms; skew
                                // corner bboxes may be wider (still
                                // conservative, checked above)
                                let det = va.0 as i128
                                    * vb.1 as i128
                                    - va.1 as i128 * vb.0 as i128;
                                let axis = det != 0
                                    && va.1 == 0
                                    && vb.0 == 0;
                                let oned = det == 0
                                    && (na == 1 || nb == 1);
                                if !exact.is_empty() && (axis || oned)
                                {
                                    let ti0 = exact
                                        .iter()
                                        .map(|e| e.0)
                                        .min()
                                        .unwrap();
                                    let ti1 = exact
                                        .iter()
                                        .map(|e| e.0)
                                        .max()
                                        .unwrap();
                                    assert!(
                                        i0 >= ti0 - 1 && i1 <= ti1 + 1,
                                        "index slack va{:?} vb{:?}",
                                        va, vb
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ----------------------------------------------- plan fixtures

    fn nested_grid() -> Ovm {
        fixture(
            &[
                FCell {
                    name: "LEAF",
                    pages: vec![(bx(0, 0, 100, 100), 100, 100)],
                    places: vec![],
                },
                FCell {
                    name: "MID",
                    pages: vec![],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 3,
                            nb: 3,
                            va: (200, 0),
                            vb: (0, 200),
                        },
                    )],
                },
                FCell {
                    name: "TOP",
                    pages: vec![(bx(0, 0, 2000, 2000), 50, 50)],
                    places: vec![(
                        1,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 2,
                            nb: 2,
                            va: (1000, 0),
                            vb: (0, 1000),
                        },
                    )],
                },
            ],
            2,
        )
    }

    #[test]
    fn nested_grid_narrow_equality_and_shape() {
        let v = nested_grid();
        // inside member (1,1) of MID(0,0) - single leaf reached
        let plan =
            check(&v, &rq(bx(210, 210, 290, 290), 0, u32::MAX), true);
        // arrays stay arrays: ONE grid edge per level, no expansion
        let top = plan
            .wcells
            .iter()
            .find(|w| w.key == (2, REM_FULL))
            .unwrap();
        assert_eq!(top.insts.len(), 1);
        assert!(matches!(
            top.insts[0].rep,
            Rep::Grid { na: 2, nb: 2, .. }
        ));
        let mid = plan
            .wcells
            .iter()
            .find(|w| w.key == (1, REM_FULL))
            .unwrap();
        assert_eq!(mid.insts.len(), 1);
        assert!(matches!(
            mid.insts[0].rep,
            Rep::Grid { na: 3, nb: 3, .. }
        ));
        assert_eq!(plan.stats.inst_edges, 2);
        // member straddle
        check(&v, &rq(bx(150, 50, 450, 150), 0, u32::MAX), true);
        // whole die
        check(&v, &rq(bx(-10, -10, 2400, 2400), 0, u32::MAX), true);
        // page cut: TOP page (max 50) drops, leaf pages stay
        let plan =
            check(&v, &rq(bx(210, 210, 290, 290), 60, u32::MAX), true);
        let b = brute(&v, &rq(bx(210, 210, 290, 290), 60, u32::MAX));
        assert!(!b.iter().any(|&pi| v.page(pi).cell == 2));
        assert_eq!(hier_pages(&plan), b);
        // off-die view: empty plan
        let plan = plan_hier(
            &v,
            &rq(bx(90000, 90000, 90010, 90010), 0, u32::MAX),
            &HierOpts::default(),
        );
        assert!(plan.wcells.is_empty() && plan.pages.is_empty());
    }

    #[test]
    fn rot_flip_equality() {
        // asymmetric leaf under every quarter-turn/flip
        let mut places = Vec::new();
        for k in 0..8u8 {
            places.push((
                0usize,
                (k as i64) * 300,
                100,
                k % 4,
                k >= 4,
                Rep::One,
            ));
        }
        let v = fixture(
            &[
                FCell {
                    name: "L",
                    pages: vec![(bx(0, 0, 40, 10), 40, 10)],
                    places: vec![],
                },
                FCell { name: "T", pages: vec![], places },
            ],
            1,
        );
        for k in 0..8i64 {
            check(
                &v,
                &rq(
                    bx(k * 300 - 50, 40, k * 300 + 50, 70),
                    0,
                    u32::MAX,
                ),
                true,
            );
        }
        check(&v, &rq(bx(-100, -100, 2500, 300), 0, u32::MAX), true);
    }

    #[test]
    fn shared_child_kbox() {
        // one cell instanced far apart; leaf has 4 corner pages so a
        // merged single-box localview would over-select
        let leaf_pages = vec![
            (bx(0, 0, 50, 50), 50, 50),
            (bx(150, 0, 200, 50), 50, 50),
            (bx(0, 150, 50, 200), 50, 50),
            (bx(150, 150, 200, 200), 50, 50),
        ];
        let v = fixture(
            &[
                FCell { name: "L", pages: leaf_pages, places: vec![] },
                FCell {
                    name: "T",
                    pages: vec![],
                    places: vec![
                        (0, 0, 0, 0, false, Rep::One),
                        (0, 100_000, 0, 0, false, Rep::One),
                    ],
                },
            ],
            1,
        );
        // view covers corner page 0 of copy A and corner page 3 of
        // copy B: exact per-box queries keep it to two pages
        let view = bx(-10, -10, 100_180, 190);
        let req = ViewReq {
            view,
            cut_dbu: 0,
            vis: vec![0xff],
            depth: u32::MAX,
            px_per_dbu: 0.0,
                    sub_cut_wash: false,
                    page_reps: false,
                    decode_budget: 0,
                    page_hairline: false,
                    page_skip: Vec::new(),
                    prune_skipped: false,
                    sub_cut_box: false,
        };
        // brute equality needs the corner windows, not the whole
        // spanning box - use two-box behavior via narrow checks
        let plan4 = plan_hier(&v, &req, &HierOpts::default());
        let b = brute(&v, &req);
        assert!(hier_pages(&plan4).is_superset(&b));
        // K=1 forces a single merged box: still no miss, possibly
        // more pages
        let mut o1 = HierOpts::default();
        o1.k_boxes = 1;
        let plan1 = plan_hier(&v, &req, &o1);
        assert!(hier_pages(&plan1).is_superset(&hier_pages(&plan4)));
        // narrow window inside copy A corner 0 only: strict equality
        check(&v, &rq(bx(5, 5, 45, 45), 0, u32::MAX), true);
    }

    // ------------------------------------------------------- pts

    fn pts_small() -> (Ovm, Vec<(i64, i64)>) {
        let offs = vec![
            (0, 0),
            (5_000, 0),
            (10_000, 3_000),
            (20_000, 20_000),
            (20_000, 20_000), // duplicate member (legal multiset)
        ];
        let v = fixture(
            &[
                FCell {
                    name: "CHIP",
                    pages: vec![(bx(0, 0, 10, 10), 10, 10)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![],
                    places: vec![(
                        0,
                        100,
                        200,
                        0,
                        false,
                        Rep::Pts(offs.clone().into()),
                    )],
                },
            ],
            1,
        );
        (v, offs)
    }

    /// world member anchors implied by the emitted inst edges to
    /// `child` (multiset, top-level fixture: parent frame == world)
    fn emitted_anchors(plan: &HierPlan, child_ci: u32) -> Vec<(i64, i64)> {
        let mut out = Vec::new();
        for w in &plan.wcells {
            for i in &w.insts {
                if i.child.0 != child_ci {
                    continue;
                }
                match &i.rep {
                    Rep::One => out.push((i.x, i.y)),
                    Rep::Pts(p) => {
                        for &(ox, oy) in p.iter() {
                            out.push((i.x + ox, i.y + oy));
                        }
                    }
                    Rep::Grid { na, nb, va, vb } => {
                        for a in 0..*na as i64 {
                            for c in 0..*nb as i64 {
                                out.push((
                                    i.x + a * va.0 + c * vb.0,
                                    i.y + a * va.1 + c * vb.1,
                                ));
                            }
                        }
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }

    #[test]
    fn pts_small_full_rep_rebased() {
        let (v, offs) = pts_small();
        // window over the third member only
        let plan = check(
            &v,
            &rq(bx(10_090, 3_190, 10_120, 3_220), 0, u32::MAX),
            true,
        );
        // small path emits the FULL member set, rebased: anchors
        // must equal base + every original offset (duplicate kept)
        let mut want: Vec<(i64, i64)> = offs
            .iter()
            .map(|&(x, y)| (100 + x, 200 + y))
            .collect();
        want.sort_unstable();
        assert_eq!(emitted_anchors(&plan, 0), want);
        // rebased rep anchors at (0,0)
        let t = plan.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        match &t.insts[0].rep {
            Rep::Pts(p) => assert_eq!(p[0], (0, 0)),
            r => panic!("expected pts, got {:?}", r),
        }
    }

    fn pts_big() -> (Ovm, Vec<(i64, i64)>) {
        // three far clusters, 3000 offsets each (> PTS_FULL_REP)
        let mut offs = Vec::new();
        for (cx, cy) in
            [(0i64, 0i64), (500_000, 0), (1_000_000, 500_000)]
        {
            for i in 0..3000i64 {
                offs.push((
                    cx + (i * 37) % 2000,
                    cy + (i * 91) % 2000,
                ));
            }
        }
        let v = fixture(
            &[
                FCell {
                    name: "CHIP",
                    pages: vec![(bx(0, 0, 10, 10), 10, 10)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Pts(offs.clone().into()),
                    )],
                },
            ],
            1,
        );
        (v, offs)
    }

    #[test]
    fn pts_small_ignores_budget_no_extent_cascade() {
        // MID is a big cell with a page FAR from the view; TOP
        // places MID via a small pts rep whose extent spans the
        // whole die. With a drained budget the old fallback set
        // O_vis = extent, ballooning MID's localview to its whole
        // rbbox and pulling the far page in - the view=cwb
        // pathology through pts (9.8G depth-9 field case). Small
        // reps now always exact-scan: the far page must stay out.
        let v = fixture(
            &[
                FCell {
                    name: "MID",
                    pages: vec![
                        (bx(0, 0, 100, 100), 100, 100),
                        (bx(900_000, 0, 900_100, 100), 100, 100),
                    ],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Pts(vec![
                            (0, 0),
                            (500_000, 0),
                            (1_000_000, 0),
                        ].into()),
                    )],
                },
            ],
            1,
        );
        let mut o = HierOpts::default();
        o.pts_enum_budget = 0; // drained from the start
        let req = rq(bx(-10, -10, 150, 150), 0, u32::MAX);
        let plan = plan_hier(&v, &req, &o);
        // only the near page of the offset-(0,0) member is visible
        assert_eq!(plan.pages, vec![0], "{:?}", plan.pages);
        assert!(hier_pages(&plan).is_superset(&brute(&v, &req)));
    }

    #[test]
    fn pts_big_subset_exact_and_budget_fallback() {
        let (v, offs) = pts_big();
        // window over cluster B only
        let view = bx(499_000, -1_000, 503_000, 3_000);
        let req = rq(view, 0, u32::MAX);
        let plan = check(&v, &req, true);
        assert!(plan.stats.pts_offsets_scanned > 0);
        assert_eq!(plan.stats.pts_fallback, 0);
        // emitted anchors == exactly the offsets whose chip bbox
        // (10x10 at offset) touches the window
        let mut want: Vec<(i64, i64)> = offs
            .iter()
            .copied()
            .filter(|&(x, y)| {
                x + 10 >= view.x0
                    && x <= view.x1
                    && y + 10 >= view.y0
                    && y <= view.y1
            })
            .collect();
        want.sort_unstable();
        assert_eq!(emitted_anchors(&plan, 0), want);
        // dry budget: whole-chunk fallback may over-include but
        // must never miss
        let mut o = HierOpts::default();
        o.pts_enum_budget = 64;
        let plan2 = plan_hier(&v, &req, &o);
        assert!(plan2.stats.pts_fallback > 0);
        let got = emitted_anchors(&plan2, 0);
        let gs: BTreeSet<(i64, i64)> = got.iter().copied().collect();
        for w in &want {
            assert!(gs.contains(w), "budget fallback dropped {:?}", w);
        }
        assert!(hier_pages(&plan2).is_superset(&brute(&v, &req)));
    }

    // ------------------------------------------------ depth/frames

    #[test]
    fn depth_variants_per_path() {
        // TOP places A (which places B) and B directly at another
        // spot; finite depth truncates PER PATH (par.2.5)
        let v = fixture(
            &[
                FCell {
                    name: "LEAF",
                    pages: vec![(bx(0, 0, 20, 20), 20, 20)],
                    places: vec![],
                },
                FCell {
                    name: "B",
                    pages: vec![(bx(0, 0, 200, 200), 200, 200)],
                    places: vec![(0, 50, 50, 0, false, Rep::One)],
                },
                FCell {
                    name: "A",
                    pages: vec![],
                    places: vec![(1, 0, 0, 0, false, Rep::One)],
                },
                FCell {
                    name: "TOP",
                    pages: vec![],
                    places: vec![
                        (2, 0, 0, 0, false, Rep::One),
                        (1, 10_000, 0, 0, false, Rep::One),
                    ],
                },
            ],
            3,
        );
        let view = bx(-100, -100, 11_000, 400);
        for d in [1u32, 2, 3, u32::MAX] {
            check(&v, &rq(view, 0, d), true);
        }
        // d=2: B via A has r=0 (pages only), direct B has r=1 which
        // folds to FULL (height(B)=1) - both variants must exist
        let plan =
            plan_hier(&v, &rq(view, 0, 2), &HierOpts::default());
        let keys: Vec<WsKey> =
            plan.wcells.iter().map(|w| w.key).collect();
        assert!(keys.contains(&(1, 0)), "{:?}", keys);
        assert!(keys.contains(&(1, REM_FULL)), "{:?}", keys);
        assert!(plan.stats.wc_variants >= 1);
        // the truncated variant must NOT reach LEAF; the full one
        // must
        let b0 = plan
            .wcells
            .iter()
            .find(|w| w.key == (1, 0))
            .unwrap();
        assert!(b0.insts.is_empty());
        let bf = plan
            .wcells
            .iter()
            .find(|w| w.key == (1, REM_FULL))
            .unwrap();
        assert_eq!(bf.insts.len(), 1);
    }

    #[test]
    fn depth_boundary_frames_keep_rep() {
        let v = fixture(
            &[
                FCell {
                    name: "S",
                    pages: vec![(bx(0, 0, 5, 5), 5, 5)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![(bx(0, 0, 4000, 100), 4000, 100)],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 4,
                            nb: 1,
                            va: (1000, 0),
                            vb: (0, 0),
                        },
                    )],
                },
            ],
            1,
        );
        let plan = check(&v, &rq(bx(0, 0, 4000, 100), 3, 0), true);
        let t = plan.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(t.frames.len(), 1);
        let (fb, frep, _) = &t.frames[0];
        assert_eq!(*fb, bx(0, 0, 5, 5));
        assert!(matches!(frep, Rep::Grid { na: 4, .. }));
        assert!(t.insts.is_empty());
        // no S working-set cell, no S pages
        assert!(!plan.wcells.iter().any(|w| w.key.0 == 0));
        assert!(plan.pages.iter().all(|&pi| v.page(pi).cell != 0));
        // rev 39: the cut gates the MEMBER box - 5x5 under cut 10
        // culls the whole rep, footprint size notwithstanding
        let pc = plan_hier(
            &v,
            &rq(bx(0, 0, 4000, 100), 10, 0),
            &HierOpts::default(),
        );
        assert_eq!(pc.stats.frame_rects, 0);
        assert!(
            pc.stats.cull_size + pc.stats.culled_bvh_size > 0
        );
        // window over member 3 of the array only: frame still comes
        // (footprint test is per whole-rep extent)
        let plan2 = plan_hier(
            &v,
            &rq(bx(2_900, 0, 3_100, 90), 3, 0),
            &HierOpts::default(),
        );
        let t2 =
            plan2.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(t2.frames.len(), 1);
        let plan3 = plan_hier(
            &v,
            &rq(bx(4_500, 0, 4_800, 90), 3, 0),
            &HierOpts::default(),
        );
        if let Some(t3) =
            plan3.wcells.iter().find(|w| w.key.0 == 1)
        {
            assert_eq!(t3.frames.len(), 0);
        }
    }

    #[test]
    fn dense_frames_stay_per_member_and_take_the_cut() {
        // rev 39: no fusing, whatever the pitch. The old ~2px fuse
        // was a step function of the screen scale - whole regions of
        // same-pitch arrays popped between gray member dust and big
        // white footprint boxes on a 0.00001um view change. Sub-cut
        // members now simply vanish, like geometry.
        let v = fixture(
            &[
                FCell {
                    name: "S",
                    pages: vec![(bx(0, 0, 5, 5), 5, 5)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![(bx(0, 0, 4000, 100), 4000, 100)],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 200,
                            nb: 1,
                            va: (15, 0),
                            vb: (0, 0),
                        },
                    )],
                },
            ],
            1,
        );
        // sub-2px pitch (15 DBU at 0.1 px/DBU) with cut below the
        // member size: the rep survives AS a rep, tone from the
        // 0.5px member box (gray)
        let plan = check(
            &v,
            &rq_px(bx(0, 0, 4000, 100), 3, 0, 0.1),
            true,
        );
        let t = plan.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(t.frames.len(), 1);
        let (fb, frep, band) = &t.frames[0];
        assert!(matches!(frep, Rep::Grid { na: 200, .. }), "{:?}", frep);
        assert_eq!(*fb, bx(0, 0, 5, 5));
        // 5 DBU member at 0.1 px = 0.5px min side -> band 3 (dotted)
        assert_eq!(*band, 3, "0.5px member box -> dotted band");
        // cut above the member size culls the whole array outright -
        // no fused footprint box reappears
        let pc = plan_hier(
            &v,
            &rq_px(bx(0, 0, 4000, 100), 10, 0, 0.1),
            &HierOpts::default(),
        );
        assert_eq!(pc.stats.frame_rects, 0);
        assert!(
            pc.stats.cull_size + pc.stats.culled_bvh_size > 0
        );
    }

    #[test]
    fn depth_boundary_huge_sparse_pts_frame_fuses() {
        // the ONE degradation rev 39 keeps: per-member frames for a
        // huge pts list would re-materialize O(count) offsets every
        // plan (memory guard, not a visual heuristic) - the count
        // cap degrades it to one footprint box. Sub-cut members
        // still cull the whole rep before the guard is consulted.
        let n = 9000usize;
        let offs: Vec<(i64, i64)> = (0..n as i64)
            .map(|i| (i % 100 * 1_000_000, i / 100 * 1_000_000))
            .collect();
        let v = fixture(
            &[
                FCell {
                    name: "S",
                    pages: vec![(bx(0, 0, 5, 5), 5, 5)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![(
                        bx(0, 0, 99_000_010, 100),
                        99_000_010,
                        100,
                    )],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Pts(offs.into()),
                    )],
                },
            ],
            1,
        );
        let plan = plan_hier(
            &v,
            &rq(bx(0, 0, 99_000_010, 200), 3, 0),
            &HierOpts::default(),
        );
        let t = plan.wcells.iter().find(|w| w.key.0 == 1).unwrap();
        assert_eq!(t.frames.len(), 1);
        assert!(matches!(t.frames[0].1, Rep::One));
        // ONLY the count cap explains the degrade
        // (9000 > pts_full_rep 8192)
        assert!(9000 > HierOpts::default().pts_full_rep);
        // member 5x5 under cut 10: the whole rep culls, no
        // footprint fallback resurrects it
        let pc = plan_hier(
            &v,
            &rq(bx(0, 0, 99_000_010, 200), 10, 0),
            &HierOpts::default(),
        );
        assert_eq!(pc.stats.frame_rects, 0);
    }

    /// Review 2026-09-11: the page hairline option through the page-
    /// BVH leaf path (walk_pbvh) and the linear run, on and off, each
    /// EQUAL to the oracle under the same option - a planner that
    /// dropped a thin page with the option off would now fail here.
    #[test]
    fn page_hairline_option_on_the_pbvh_leaf_path_matches_the_oracle() {
        // 20 pages in a row, every other one thin (90 x 5: min side 5)
        let mk = |with_tree: bool| -> Ovm {
            let mut b = Builder::new(1000.0, 0, 0, 1);
            b.top = 0;
            b.layer(1, 0, "L1", 0, 0);
            let m1 = b.bitset(&[1]);
            for k in 0..20u32 {
                let x = k as i64 * 100;
                let h = if k % 2 == 1 { 5 } else { 50 };
                b.page(
                    0,
                    0,
                    k,
                    &bx(x, 0, x + 90, h),
                    0,
                    0,
                    0,
                    1,
                    1,
                    90,
                    h as u64,
                    floe_ovm::LOD_EXACT,
                    floe_ovm::LOD_PAGE_NONE,
                );
            }
            let root = if with_tree {
                let l0 = b.pbvh_node(&bx(0, 0, 990, 50), 0, 10, true, 90, 50);
                let _l1 =
                    b.pbvh_node(&bx(1000, 0, 1990, 50), 10, 10, true, 90, 50);
                b.pbvh_node(&bx(0, 0, 1990, 50), l0, 2, false, 90, 50)
            } else {
                PBVH_NONE
            };
            let pr = b.prange(0, 0, 20, root);
            b.cell(
                "T",
                0,
                0,
                &bx(0, 0, 1990, 50),
                &bx(0, 0, 1990, 50),
                0,
                0,
                0,
                20,
                0,
                0,
                pr,
                1,
                m1,
                m1,
                1,
                0,
                0,
                m1,
            );
            Ovm::from_bytes(b.finish(0, 0)).unwrap()
        };
        let lin = mk(false);
        let tree = mk(true);
        // cut 20 -> hair 10: the thin pages' min side 5 is under it,
        // their long side 90 is not (never a size cull)
        let req = rq(bx(0, 0, 1990, 50), 20, u32::MAX);
        for page_hairline in [false, true] {
            let mut o = HierOpts::default();
            o.explain = true;
            let mut req = req.clone();
            req.page_hairline = page_hairline;
            let pl = plan_hier(&lin, &req, &o);
            let pt = plan_hier(&tree, &req, &o);
            assert!(pt.stats.visited_page_bvh > 0, "the tree path was walked");
            let oracle = brute_with(&lin, &req, &o);
            assert_eq!(hier_pages(&pl), oracle, "linear, page_hairline={page_hairline}");
            assert_eq!(hier_pages(&pt), oracle, "pbvh leaf, page_hairline={page_hairline}");
            let thin: BTreeSet<u32> = (0..20u32).filter(|k| k % 2 == 1).collect();
            if page_hairline {
                assert_eq!(oracle.len(), 10);
                assert!(oracle.is_disjoint(&thin));
                assert_eq!(pt.stats.cull_page_size, 10);
                assert_eq!(pt.stats.thin_pages_kept, 0);
                assert_eq!(pt.explain.iter().filter(|r| r.verdict == "cull_hair").count(), 10);
            } else {
                assert_eq!(oracle.len(), 20);
                assert!(oracle.is_superset(&thin));
                assert_eq!(pt.stats.cull_page_size, 0);
                assert_eq!(pt.stats.thin_pages_kept, 10);
                assert_eq!(pt.explain.iter().filter(|r| r.verdict == "exact_thin").count(), 10);
            }
        }
        // the request decides: a plain-policy request culls, a mask-
        // policy request keeps - in the oracle as in the planner
        assert_eq!(brute(&lin, &req).len(), 20);
        let mut cull = req.clone();
        cull.page_hairline = true;
        assert_eq!(brute(&lin, &cull).len(), 10);
    }

    #[test]
    fn a_thinned_grid_keeps_one_member_in_two_per_level_and_nests() {
        // 64 x 64: level 2 -> 32 x 32 on a doubled pitch; level 6 -> 8 x 8
        assert_eq!(thin_grid(64, 64, (10, 0), (0, 10), 2), (32, 32, (20, 0), (0, 20)));
        assert_eq!(thin_grid(64, 64, (10, 0), (0, 10), 6), (8, 8, (80, 0), (0, 80)));
        // an odd level puts the extra stride on the first axis
        assert_eq!(thin_grid(64, 64, (10, 0), (0, 10), 1), (32, 64, (20, 0), (0, 10)));
        // a 1-D array thins along its length: one in 2^level
        assert_eq!(thin_grid(1024, 1, (5, 0), (0, 0), 4), (64, 1, (80, 0), (0, 0)));
        // strides never exceed the axis: 4 x 1024 at level 6 -> 1 x 64
        assert_eq!(thin_grid(4, 1024, (1, 0), (0, 1), 6), (1, 64, (4, 0), (0, 16)));
        // nested: the survivors of level + 1 sit on level's lattice
        for l in 0..8u32 {
            let (_, _, va1, vb1) = thin_grid(256, 256, (1, 0), (0, 1), l);
            let (_, _, va2, vb2) = thin_grid(256, 256, (1, 0), (0, 1), l + 1);
            assert_eq!(va2.0 % va1.0, 0);
            assert_eq!(vb2.1 % vb1.1, 0);
        }
        // points: every 2^level-th slot, slot 0 first
        let pts: Vec<(i64, i64)> = (0..100).map(|k| (k, 2 * k)).collect();
        assert_eq!(thin_pts(&pts, 0).len(), 100);
        assert_eq!(thin_pts(&pts, 3).iter().map(|p| p.0).collect::<Vec<_>>(), vec![0, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 96]);
        assert_eq!(member_levels(1, 5), 0);
        assert_eq!(member_levels(1000, 5), 5);
        assert_eq!(member_levels(1000, 12), 9);
    }

    #[test]
    fn representatives_thin_by_level_and_nest() {
        // rep_level: two levels per octave below the threshold
        assert_eq!(rep_level(1000, 1000), 0);
        assert_eq!(rep_level(708, 1000), 0);
        assert_eq!(rep_level(707, 1000), 1);
        assert_eq!(rep_level(500, 1000), 2);
        assert_eq!(rep_level(250, 1000), 4);
        assert_eq!(rep_level(1, 1000), 19);
        assert_eq!(rep_level(0, 1 << 40), REP_LEVELS_MAX);
        assert_eq!(rep_level(5, 0), 0);
        // rep_keeps: one in 2^level, nested, index 0 always
        for l in 0..=12u32 {
            assert!(rep_keeps(0, l));
            let kept: Vec<u64> = (0..4096u64).filter(|&i| rep_keeps(i, l)).collect();
            assert_eq!(kept.len(), 4096 >> l, "level {}", l);
            for &i in &kept {
                assert!(rep_keeps(i, l.saturating_sub(1)), "nested: {} at level {}", i, l);
            }
        }
        // rep_in_run: a run without a representative prunes, an
        // unknown run descends
        assert!(rep_in_run(0, 1, 0, 6));
        assert!(!rep_in_run(1, 64, 0, 6));
        assert!(rep_in_run(1, 65, 0, 6));
        assert!(rep_in_run(100, 200, 100, 10));
        assert!(!rep_in_run(101, 200, 100, 10));
        assert!(rep_in_run(5, 5, 0, 4));
    }

    #[test]
    fn pbvh_equals_linear() {
        // 20 pages in a row; one ovm linear, one with a hand-built
        // 3-node page BVH - identical selections, tree stats move
        let mk = |with_tree: bool| -> Ovm {
            let mut b = Builder::new(1000.0, 0, 0, 1);
            b.top = 0;
            b.layer(1, 0, "L1", 0, 0);
            let m1 = b.bitset(&[1]);
            for k in 0..20u32 {
                let x = k as i64 * 100;
                b.page(
                    0,
                    0,
                    k,
                    &bx(x, 0, x + 90, 50),
                    0,
                    0,
                    0,
                    1,
                    1,
                    90,
                    50,
                    floe_ovm::LOD_EXACT,
                    floe_ovm::LOD_PAGE_NONE,
                );
            }
            let root = if with_tree {
                let l0 = b.pbvh_node(
                    &bx(0, 0, 990, 50),
                    0,
                    10,
                    true,
                    90,
                    50,
                );
                let _l1 = b.pbvh_node(
                    &bx(1000, 0, 1990, 50),
                    10,
                    10,
                    true,
                    90,
                    50,
                );
                b.pbvh_node(&bx(0, 0, 1990, 50), l0, 2, false, 90, 50)
            } else {
                PBVH_NONE
            };
            let pr = b.prange(0, 0, 20, root);
            b.cell(
                "T",
                0,
                0,
                &bx(0, 0, 1990, 50),
                &bx(0, 0, 1990, 50),
                0,
                0,
                0,
                20,
                0,
                0,
                pr,
                1,
                m1,
                m1,
                1,
                0,
                0,
                m1,
            );
            Ovm::from_bytes(b.finish(0, 0)).unwrap()
        };
        let lin = mk(false);
        let tree = mk(true);
        for view in [
            bx(120, 0, 380, 50),
            bx(0, 0, 1990, 50),
            bx(1500, 10, 1620, 40),
        ] {
            let req = rq(view, 0, u32::MAX);
            let a = plan_hier(&lin, &req, &HierOpts::default());
            let c = plan_hier(&tree, &req, &HierOpts::default());
            assert_eq!(a.pages, c.pages);
            assert!(c.stats.visited_page_bvh > 0);
        }
        // narrow view culls the far subtree by bbox
        let req = rq(bx(120, 0, 380, 50), 0, u32::MAX);
        let c = plan_hier(&tree, &req, &HierOpts::default());
        assert!(c.stats.culled_page_bvh_bbox > 0);
        // page-level cut prunes whole subtrees pre-leaf
        let req = rq(bx(0, 0, 1990, 50), 200, u32::MAX);
        let c = plan_hier(&tree, &req, &HierOpts::default());
        assert!(c.stats.culled_page_bvh_cut > 0);
        assert!(c.pages.is_empty());
    }

    // ---------------------------------------------- delta (par.3.2)

    #[test]
    fn page_prio_is_local_frame() {
        // CHILD sits at (100k,100k): its page bbox is CELL-LOCAL
        // (0..100), so a world-center metric would call it "far"
        // even when it fills the screen center (review finding).
        // The planner must price it through the WC's LOCAL view.
        let v = fixture(
            &[
                FCell {
                    name: "C",
                    pages: vec![(bx(0, 0, 100, 100), 100, 100)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![(bx(0, 0, 80, 80), 80, 80)],
                    places: vec![(
                        0,
                        100_000,
                        100_000,
                        0,
                        false,
                        Rep::One,
                    )],
                },
            ],
            1,
        );
        // wide view covering both pages; the child's LOCAL view box
        // covers its whole page (center inside -> prio 0) while the
        // top's own page sits ~50k off the top-frame view center
        let req =
            rq(bx(-10, -10, 100_140, 100_140), 0, u32::MAX);
        let plan = plan_hier(&v, &req, &HierOpts::default());
        assert_eq!(plan.pages, vec![0, 1]);
        // child page: the local view box covers it -> center inside
        // -> prio 0; top's own page is far from the view center
        assert_eq!(plan.page_prio[0], 0, "{:?}", plan.page_prio);
        assert!(plan.page_prio[1] > 0, "{:?}", plan.page_prio);
        // narrow view exactly on the child: still prio 0
        let req2 =
            rq(bx(99_990, 99_990, 100_110, 100_110), 0, u32::MAX);
        let plan2 = plan_hier(&v, &req2, &HierOpts::default());
        assert_eq!(plan2.pages, vec![0]);
        assert_eq!(plan2.page_prio, vec![0]);
    }

    #[test]
    fn delta_hier_roundtrip_full() {
        use floe_oasis::doc::{parse_doc, RectRec};
        use floe_oasis::write::{write_tree, WCell};
        let mk_page = |name: &str, w: i64| {
            let r = RectRec {
                layer: 1,
                dt: 0,
                x: 0,
                y: 0,
                w,
                h: 20,
                rep: Rep::One,
            };
            write_tree(
                &[WCell {
                    name: name.into(),
                    rects: std::slice::from_ref(&r),
                    polys: &[],
                    paths: &[],
                    texts: &[],
                    places: vec![],
                }],
                1000.0,
            )
            .unwrap()
        };
        let p0 = mk_page("P0_0_0", 100);
        let p1 = mk_page("P1_0_0", 500);
        let mut ovp = p0.clone();
        ovp.extend_from_slice(&p1);
        let dir = std::env::temp_dir()
            .join(format!("floe_m2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ovp_path = dir.join("design.ovp");
        std::fs::write(&ovp_path, &ovp).unwrap();
        // matching metadata: LEAF(page P0_0_0) under TOP(page
        // P1_0_0) via a 2x1 grid at (0,100)
        let mut b = Builder::new(1000.0, 0, 0, 1);
        b.top = 1;
        b.layer(1, 0, "L1", 0, 0);
        let m = b.bitset(&[1]);
        let leafbb = bx(0, 0, 100, 20);
        b.page(
            0,
            0,
            0,
            &leafbb,
            0,
            p0.len() as u64,
            p0.len() as u64,
            1,
            1,
            100,
            20,
            floe_ovm::LOD_EXACT,
            floe_ovm::LOD_PAGE_NONE,
        );
        b.page(
            1,
            0,
            0,
            &bx(0, 0, 500, 20),
            p0.len() as u64,
            p1.len() as u64,
            p1.len() as u64,
            1,
            1,
            500,
            20,
            floe_ovm::LOD_EXACT,
            floe_ovm::LOD_PAGE_NONE,
        );
        let pl = b.place(
            0,
            0,
            100,
            0,
            false,
            &Rep::Grid { na: 2, nb: 1, va: (200, 0), vb: (0, 0) },
        );
        let n0 = b.bvh_node(
            &bx(0, 100, 300, 120),
            pl as u32,
            1,
            true,
            u32::MAX,
            u32::MAX,
        );
        let pr0 = b.prange(0, 0, 1, PBVH_NONE);
        b.cell(
            "LEAF", 0, 1, &leafbb, &leafbb, 0, 0, 0, 1, 0, 0, pr0,
            1, m, m, 1, 0, 0, m,
        );
        let pr1 = b.prange(0, 1, 1, PBVH_NONE);
        b.cell(
            "TOPC",
            1,
            0,
            &bx(0, 0, 500, 120),
            &bx(0, 0, 500, 120),
            0,
            1,
            1,
            1,
            n0,
            1,
            pr1,
            1,
            m,
            m,
            1,
            0,
            0,
            m,
        );
        let ovm = Ovm::from_bytes(b.finish(ovp.len() as u64, 0)).unwrap();
        let vfs = crate::Vfs {
            ovm,
            ovp_path: ovp_path.to_string_lossy().into_owned(),
            ovt: floe_ovm::Backing::Vec(Vec::new()),
        };
        let req = rq(bx(-10, -10, 600, 200), 0, u32::MAX);
        let plan =
            plan_hier(&vfs.ovm, &req, &HierOpts::default());
        assert_eq!(plan.pages, vec![0, 1]);
        let delta =
            vfs.delta_hier(&plan, &plan.pages, 3, None).unwrap();
        // deterministic emission
        assert_eq!(
            delta,
            vfs.delta_hier(&plan, &plan.pages, 3, None).unwrap()
        );
        let doc = parse_doc(&delta).unwrap();
        // single-top OASIS: the gen's top WC
        assert_eq!(doc.cells[doc.top].name, "W3_F_1");
        let mut names: Vec<&str> =
            doc.cells.iter().map(|c| c.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec!["P0_0_0", "P1_0_0", "W3_F_0", "W3_F_1"]
        );
        let topc = &doc.cells[doc.top];
        assert_eq!(topc.places.len(), 2);
        // identity page instance first, then the child WC edge with
        // the array preserved
        assert_eq!(doc.cells[topc.places[0].cell].name, "P1_0_0");
        assert!(matches!(topc.places[0].rep, Rep::One));
        let wleaf = &doc.cells[topc.places[1].cell];
        assert_eq!(wleaf.name, "W3_F_0");
        assert!(matches!(
            topc.places[1].rep,
            Rep::Grid { na: 2, nb: 1, .. }
        ));
        assert_eq!((topc.places[1].x, topc.places[1].y), (0, 100));
        assert_eq!(wleaf.places.len(), 1);
        assert_eq!(
            doc.cells[wleaf.places[0].cell].name,
            "P0_0_0"
        );
        // incremental delta: no page bodies, resident refs stay
        // undefined (parse_doc materializes them as empty cells,
        // klayout binds them by name - M0)
        let inc = vfs.delta_hier(&plan, &[], 4, None).unwrap();
        let doc2 = parse_doc(&inc).unwrap();
        assert_eq!(doc2.cells[doc2.top].name, "W4_F_1");
        let ghost = doc2
            .cells
            .iter()
            .find(|c| c.name == "P0_0_0")
            .unwrap();
        assert!(ghost.rects.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delta_hier_frames_and_pts_authored() {
        use floe_oasis::doc::parse_doc;
        // frames: explicit depth-boundary grid child -> WC rect on
        // the runtime frame layer with the rep preserved
        // (authored-only delta, new_pages = [])
        let v = fixture(
            &[
                FCell {
                    name: "S",
                    pages: vec![(bx(0, 0, 5, 5), 5, 5)],
                    places: vec![],
                },
                FCell {
                    name: "T",
                    pages: vec![(bx(0, 0, 4000, 100), 4000, 100)],
                    places: vec![(
                        0,
                        0,
                        0,
                        0,
                        false,
                        Rep::Grid {
                            na: 4,
                            nb: 1,
                            va: (1000, 0),
                            vb: (0, 0),
                        },
                    )],
                },
            ],
            1,
        );
        let plan = plan_hier(
            &v,
            // cut 3: under the 5x5 member box (rev 39 member cut)
            &rq(bx(0, 0, 4000, 100), 3, 0),
            &HierOpts::default(),
        );
        let vfs = crate::Vfs {
            ovm: v,
            ovp_path: String::new(),
            ovt: floe_ovm::Backing::Vec(Vec::new()),
        };
        let delta = vfs.delta_hier(&plan, &[], 9, None).unwrap();
        let doc = parse_doc(&delta).unwrap();
        let top = &doc.cells[doc.top];
        assert_eq!(top.name, "W9_0_1");
        assert_eq!(top.rects.len(), 1);
        let fr = &top.rects[0];
        assert_eq!(
            (fr.layer, fr.dt),
            frame_layer(&vfs.ovm),
            "frame rect must ride the runtime frame layer"
        );
        assert!(matches!(fr.rep, Rep::Grid { na: 4, .. }));
        // referenced page is a ghost (undefined) cell here
        assert!(doc
            .cells
            .iter()
            .any(|c| c.name == "P1_0_0" && c.rects.is_empty()));
        // M7-C wash: the same fixture at a sub-wash_px scale
        // authors ONE rect on the page's own layer (1/0) instead
        // of referencing the page
        let wplan = plan_hier(
            &vfs.ovm,
            &rq_px(bx(0, 0, 4000, 100), 0, 0, 0.0004),
            &HierOpts::default(),
        );
        let wdelta = vfs.delta_hier(&wplan, &[], 11, None).unwrap();
        let wdoc = parse_doc(&wdelta).unwrap();
        let wtop = &wdoc.cells[wdoc.top];
        assert!(wplan.stats.washed_pages > 0);
        assert!(wplan.pages.is_empty());
        assert!(wtop
            .rects
            .iter()
            .any(|r| (r.layer, r.dt) == (1, 0)));

        // pts: rebased subset survives the writer/parser round trip
        let (v2, offs) = pts_small();
        let plan2 = plan_hier(
            &v2,
            &rq(bx(10_090, 3_190, 10_120, 3_220), 0, u32::MAX),
            &HierOpts::default(),
        );
        let vfs2 = crate::Vfs {
            ovm: v2,
            ovp_path: String::new(),
            ovt: floe_ovm::Backing::Vec(Vec::new()),
        };
        let delta2 = vfs2.delta_hier(&plan2, &[], 11, None).unwrap();
        let doc2 = parse_doc(&delta2).unwrap();
        let t2 = &doc2.cells[doc2.top];
        let pl = t2
            .places
            .iter()
            .find(|p| doc2.cells[p.cell].name.starts_with("W11_"))
            .unwrap();
        let mut got: Vec<(i64, i64)> = match &pl.rep {
            Rep::Pts(p) => {
                assert_eq!(p[0], (0, 0), "rebased anchor");
                p.iter().map(|&(x, y)| (pl.x + x, pl.y + y)).collect()
            }
            r => panic!("expected pts, got {:?}", r),
        };
        got.sort_unstable();
        let mut want: Vec<(i64, i64)> = offs
            .iter()
            .map(|&(x, y)| (100 + x, 200 + y))
            .collect();
        want.sort_unstable();
        assert_eq!(got, want);
    }

    #[test]
    fn deterministic_plans() {
        let (v, _) = pts_big();
        let req = rq(bx(-1_000, -1_000, 1_100_000, 600_000), 0, u32::MAX);
        let a = plan_hier(&v, &req, &HierOpts::default());
        let b = plan_hier(&v, &req, &HierOpts::default());
        assert_eq!(a, b);
        // K overflow: 6 spread copies of one leaf force merges
        let mut places = Vec::new();
        for k in 0..6i64 {
            places.push((
                0usize,
                k * 40_000,
                (k % 2) * 25_000,
                0u8,
                false,
                Rep::One,
            ));
        }
        let v2 = fixture(
            &[
                FCell {
                    name: "L",
                    pages: vec![(bx(0, 0, 100, 100), 100, 100)],
                    places: vec![],
                },
                FCell { name: "T", pages: vec![], places },
            ],
            1,
        );
        let req2 =
            rq(bx(-500, -500, 250_000, 30_000), 0, u32::MAX);
        let p1 = plan_hier(&v2, &req2, &HierOpts::default());
        let p2 = plan_hier(&v2, &req2, &HierOpts::default());
        assert_eq!(p1, p2);
        assert!(hier_pages(&p1).is_superset(&brute(&v2, &req2)));
        // direct K-merge determinism: 6 disjoint boxes through K=4
        // (identical planner contributions dedup by containment, so
        // the merge path is exercised at the accumulator level)
        let boxes = [
            bx(0, 0, 10, 10),
            bx(1000, 0, 1010, 10),
            bx(0, 1000, 10, 1010),
            bx(1000, 1000, 1010, 1010),
            bx(500, 500, 510, 510),
            bx(2000, 2000, 2010, 2010),
        ];
        let (mut m1, mut m2) = (0u64, 0u64);
        let mut k1 = KBox::default();
        let mut k2 = KBox::default();
        for b in boxes {
            k1.add(b, 4, &mut m1);
        }
        for b in boxes {
            k2.add(b, 4, &mut m2);
        }
        assert!(m1 > 0);
        assert_eq!(m1, m2);
        assert_eq!(k1.boxes, k2.boxes);
        assert!(k1.boxes.len() <= 4);
        // no box lost: every input is inside some kept box
        for b in boxes {
            assert!(k1.boxes.iter().any(|k| contains(k, &b)));
        }
    }
}
