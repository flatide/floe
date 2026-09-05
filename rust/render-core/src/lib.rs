//! Deterministic CPU-rendering primitives for floe.
//!
//! The current M1 slice covers cache validation, hierarchy planning, decoded
//! page caching, and deterministic rectangle/polygon fill occupancy.

#![forbid(unsafe_code)]

mod cache;
mod cancel;
mod clip;
mod font;
mod page_cache;
mod page_index;
mod png;
mod query;
mod raster;
mod repetition;
mod request;
mod scene;
mod stats;
mod transform;

pub use cache::{
    Cache, CacheInfo, CacheLayer, DecodedPage, PagePayload, PlanSummary, PlannedLabels,
    PlannedView, RenderLabel,
};
pub use cancel::RenderCancellation;
pub use clip::ClipGeometry;
pub use font::{validate_font_px, DEFAULT_LABEL_FONT_PX, MAX_LABEL_FONT_PX, MIN_LABEL_FONT_PX};
pub use page_cache::DecodedPageCache;
pub use page_index::PageIndex;
pub use query::{
    pick_scene, pick_scene_cancellable, snap_scene, snap_scene_cancellable, ScenePick,
    ScenePickCandidate, SceneQueryLayer, SceneQueryRequest, SceneSnap, SceneSnapKind,
};
pub use raster::{
    render_geometry_occupancy, render_geometry_occupancy_cancellable, render_geometry_styled,
    render_geometry_styled_cancellable, render_geometry_styled_cancellable_reuse,
    render_geometry_styled_unbinned,
    render_geometry_styled_unbinned_cancellable, FrameReuse, GeometryRasterReport,
    GeometryRasterRequest, LayerFill,
    LayerStyle, RasterViewBox, RgbaFrame, StyledGeometryRasterRequest, DEFAULT_TILE_SIZE,
    MAX_TILE_SIZE,
};
pub use request::{PlanRequest, ViewBox, FULL_DEPTH};
pub use scene::FrameScene;
pub use stats::RenderStats;
