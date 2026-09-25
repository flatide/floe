//! Deterministic CPU-rendering primitives for floe.
//!
//! The current M1 slice covers cache validation, hierarchy planning, decoded
//! page caching, and deterministic rectangle/polygon fill occupancy.

#![forbid(unsafe_code)]

mod cache;
mod cancel;
mod clip;
mod deck;
mod font;
mod page_cache;
mod page_index;
mod png;
mod query;
mod layer_decode;
mod raster;
mod repetition;
mod request;
mod scene;
mod stats;
mod summary;
mod transform;

pub use cache::{
    Cache, CacheInfo, CacheLayer, DecodePool, DecodedPage, PagePayload, PlanSummary, PlannedLabels,
    PlannedView, RenderLabel,
};
pub use cancel::RenderCancellation;
pub use clip::ClipGeometry;
pub use deck::{
    overlay, source_view, transform_bbox, Deck, DeckInfo, DeckLayer, DeckPlacement,
    DeckRenderReport, DeckRenderRequest, DeckSpec,
};
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
    render_geometry_styled_cancellable_windowed,
    render_geometry_styled_unbinned,
    render_geometry_styled_unbinned_cancellable, FrameReuse, GeometryRasterReport,
    GeometryRasterRequest, LayerFill, LayerRasterSession,
    LayerStyle, RasterViewBox, RgbaFrame, StyledGeometryRasterRequest, DEFAULT_TILE_SIZE,
    MAX_TILE_SIZE,
};
pub use layer_decode::{LayerProbeReport, ProbeMode};
pub use request::{PlanRequest, ViewBox, FULL_DEPTH};
pub use scene::FrameScene;
pub use summary::{cull_allowed as summary_cull_allowed, layout_allowed as summary_layout_allowed, level_for as summary_level_for, SummaryPlane, SummarySelection};
pub use stats::{place_walks_wire, RenderStats, PLACE_WALK_OUTCOMES};

pub use floe_vfs::hier::HierPlan;
pub use floe_vfs::representatives::TreeOptions as RepresentativeOptions;
