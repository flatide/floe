//! Deterministic CPU-rendering primitives for floe.
//!
//! The current M1 slice covers cache validation, hierarchy planning, decoded
//! page caching, and deterministic rectangle/polygon fill occupancy.

#![forbid(unsafe_code)]

mod cache;
mod cancel;
mod page_cache;
mod png;
mod raster;
mod repetition;
mod request;
mod scene;
mod stats;
mod transform;

pub use cache::{Cache, CacheInfo, CacheLayer, DecodedPage, PagePayload, PlanSummary, PlannedView};
pub use cancel::RenderCancellation;
pub use page_cache::DecodedPageCache;
pub use raster::{
    render_geometry_occupancy, render_geometry_occupancy_cancellable, render_geometry_styled,
    render_geometry_styled_cancellable, GeometryRasterReport, GeometryRasterRequest, LayerFill,
    LayerStyle, RasterViewBox, RgbaFrame, StyledGeometryRasterRequest, DEFAULT_TILE_SIZE,
    MAX_TILE_SIZE,
};
pub use request::{PlanRequest, ViewBox, FULL_DEPTH};
pub use scene::FrameScene;
pub use stats::RenderStats;
