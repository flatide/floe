/// Phase-separated counters shared by the CLI, daemon, and future GUI adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub plan_us: u64,
    pub page_read_us: u64,
    pub page_decode_us: u64,
    /// Sum of per-page decode wall times. Compared against
    /// `page_decode_us * decode_workers_used` this exposes worker idle
    /// time; equal values mean the pool ran fully busy.
    pub page_decode_sum_us: u64,
    /// Slowest single page decode (straggler detection).
    pub page_decode_max_us: u64,
    /// Portion of `page_decode_sum_us` spent building record indexes
    /// (the F2R-03b lazy-index decision input).
    pub page_index_us: u64,
    pub decode_workers_used: u16,
    pub scene_us: u64,
    pub raster_us: u64,
    /// Slowest single image tile (tail imbalance across raster workers).
    pub raster_tile_max_us: u64,
    /// Tiles served from the shifted previous geometry frame instead
    /// of rastering (§F2R-16 pan reuse).
    pub tiles_reused: u32,
    /// Items collected by the 2c work bin (0 = walk fallback or
    /// occupancy mode).
    pub work_bin_items: u64,
    /// Items reached when the bin hit its cap and fell back to the
    /// per-tile walk (0 = no overflow).
    pub work_bin_overflow_items: u64,
    /// Deferral edges by cause (§3.17 diagnosis): repetition edges vs
    /// single placements that measured past the trial item budget,
    /// with the largest static weight among them.
    pub work_bin_defer_rep: u64,
    pub work_bin_defer_single: u64,
    pub work_bin_defer_weight_max: u64,
    pub png_us: u64,
    pub tiles: u32,
    pub workers_used: u16,
    pub representative_spans: u64,
    pub representative_pixels: u64,
    pub primitives_tested: u64,
    pub primitives_drawn: u64,
    pub rep_members_tested: u64,
    pub rep_members_drawn: u64,
    /// Hierarchy walk entries across all tiles and paint planes
    /// (geometry and frame-band walks; F2R-03b 2b gate metric).
    pub hier_cells_visited: u64,
    /// F2R-28 write-once tiles: tiles whose pass sequence ended early
    /// because every pixel was written, the passes (planes and frame
    /// bands) they skipped, and the items (cell visits, pages, instance
    /// members, washes, point chunks) skipped because their device box
    /// held no open pixel.
    pub once_full_tiles: u32,
    pub once_passes_skipped: u64,
    pub once_items_skipped: u64,
    /// Instance edges skipped by the subtree content masks.
    pub subtrees_pruned: u64,
    pub decoded_cache_hit: u32,
    pub decoded_cache_miss: u32,
    /// Pages evicted from the decoded LRU while serving this load -
    /// nonzero means the working set is churning past the budget
    /// (§3.18 long-session diagnosis).
    pub decoded_cache_evicted: u32,
    pub decoded_cache_bytes: u64,
    pub cancelled: bool,
    /// Placement survivor walks by outcome (CUT_DENSITY_DESIGN §10.8, the
    /// placement lattice): (walks planned, visible members) per outcome of
    /// PLACE_WALK_OUTCOMES, one-dimensional arrays at [k], two-dimensional at
    /// [PLACE_WALK_OUTCOMES.len() + k]. Counted per walk: the work bin's
    /// collection once a frame, the tile walks and mini walks once a tile.
    pub place_walks: [(u64, u64); 32],
}

/// The outcomes of a placement array's survivor walk, in RenderStats::
/// place_walks order: walked, or why the members were all visited.
pub const PLACE_WALK_OUTCOMES: [&str; 16] = [
    "walked",
    "not_leaf",
    "no_range",
    "page_level",
    "undecoded",
    "non_rim",
    "array_record",
    "path",
    "shapes",
    "prep_work",
    "not_subpixel",
    "no_shapes",
    "no_axis",
    "axis_mismatch",
    "cost",
    "cursor_cap",
];

/// RenderStats::place_walks as a wire value: `<outcome><1|2>:<walks>/<members>`
/// for every outcome seen, comma-separated; `-` for none.
pub fn place_walks_wire(walks: &[(u64, u64); 32]) -> String {
    let n = PLACE_WALK_OUTCOMES.len();
    let parts: Vec<String> = walks
        .iter()
        .enumerate()
        .filter(|(_, (count, _))| *count > 0)
        .map(|(k, (count, members))| format!("{}{}:{}/{}", PLACE_WALK_OUTCOMES[k % n], 1 + k / n, count, members))
        .collect();
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(",")
    }
}
