/// Phase-separated counters shared by the CLI, daemon, and future GUI adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub plan_us: u64,
    pub page_read_us: u64,
    pub page_decode_us: u64,
    pub decode_workers_used: u16,
    pub scene_us: u64,
    pub raster_us: u64,
    pub png_us: u64,
    pub tiles: u32,
    pub workers_used: u16,
    pub primitives_tested: u64,
    pub primitives_drawn: u64,
    pub rep_members_tested: u64,
    pub rep_members_drawn: u64,
    pub decoded_cache_hit: u32,
    pub decoded_cache_miss: u32,
    pub decoded_cache_bytes: u64,
    pub cancelled: bool,
}
