//! Deterministic CPU-rendering primitives for floe.
//!
//! The current M1 slice covers cache validation, hierarchy planning, decoded
//! page caching, and deterministic rectangle/polygon fill occupancy.

#![forbid(unsafe_code)]

mod cache;
mod cancel;
mod cells;
mod clip;
mod deck;
mod font;
mod layer_decode;
mod page_cache;
mod page_index;
mod png;
mod query;
mod raster;
mod repetition;
mod request;
mod revision_lease;
mod scene;
mod stats;
mod summary;
mod transform;

pub use cache::{
    Cache, CacheInfo, CacheLayer, DecodePool, DecodedPage, PagePayload, PlanSummary, PlannedLabels,
    PlannedView, RenderLabel,
};
pub use cancel::RenderCancellation;
pub use cells::{
    children as cell_children, extent as cell_extent, find as cell_find, instances as cell_instances,
    CellChildren, CellExtent, ChildRow, DeckXf, FindResult, FindRow, HierError, HierHandle, Instances,
};
pub use clip::ClipGeometry;
pub use deck::{
    overlay, source_view, transform_bbox, Deck, DeckHierSource, DeckInfo, DeckLayer, DeckPlacement,
    DeckRenderReport, DeckRenderRequest, DeckSpec,
};
pub use font::{validate_font_px, DEFAULT_LABEL_FONT_PX, MAX_LABEL_FONT_PX, MIN_LABEL_FONT_PX};
pub use layer_decode::{LayerProbeReport, ProbeMode};
pub use page_cache::DecodedPageCache;
pub use page_index::PageIndex;
pub use query::{
    pick_scene, pick_scene_cancellable, snap_scene, snap_scene_cancellable, ScenePick,
    ScenePickCandidate, SceneQueryLayer, SceneQueryRequest, SceneSnap, SceneSnapKind,
};
pub use raster::{
    render_geometry_occupancy, render_geometry_occupancy_cancellable, render_geometry_styled,
    render_geometry_styled_cancellable, render_geometry_styled_cancellable_reuse,
    render_geometry_styled_cancellable_windowed, render_geometry_styled_unbinned,
    render_geometry_styled_unbinned_cancellable, DensityScenes, FrameReuse, GeometryRasterReport,
    GeometryRasterRequest, LayerFill, LayerRasterSession, LayerStyle, RasterViewBox, RgbaFrame,
    StyledGeometryRasterRequest, DEFAULT_TILE_SIZE, MAX_TILE_SIZE,
};
pub use request::{PlanRequest, ViewBox, FULL_DEPTH};
pub use scene::FrameScene;
pub use stats::{place_walks_wire, RenderStats, DENSITY_STACK_COUNTS, PLACE_WALK_OUTCOMES};
pub use summary::{
    cull_allowed as summary_cull_allowed, layout_allowed as summary_layout_allowed,
    level_for as summary_level_for, SummaryPlane, SummarySelection,
};

pub use floe_vfs::hier::FixedFit;
pub use floe_vfs::hier::HierPlan;
pub use floe_vfs::representatives::TreeOptions as RepresentativeOptions;
