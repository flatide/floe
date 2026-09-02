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
    pub primitives_tested: u64,
    pub primitives_drawn: u64,
    pub rep_members_tested: u64,
    pub rep_members_drawn: u64,
    /// Hierarchy walk entries across all tiles and paint planes
    /// (geometry and frame-band walks; F2R-03b 2b gate metric).
    pub hier_cells_visited: u64,
    /// Instance edges skipped by the subtree content masks.
    pub subtrees_pruned: u64,
    pub decoded_cache_hit: u32,
    pub decoded_cache_miss: u32,
    pub decoded_cache_bytes: u64,
    pub cancelled: bool,
}
