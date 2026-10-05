use floe_ovm::BBox;

/// Full hierarchy depth sentinel used by `floe-vfs`.
pub const FULL_DEPTH: u32 = u32::MAX;

/// Closed viewport in layout database units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewBox {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl ViewBox {
    pub fn new(x0: i64, y0: i64, x1: i64, y1: i64) -> Result<Self, String> {
        let view = Self { x0, y0, x1, y1 };
        view.validate()?;
        Ok(view)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.x0 > self.x1 || self.y0 > self.y1 {
            return Err(format!(
                "invalid view: expected x0<=x1 and y0<=y1, got {},{},{},{}",
                self.x0, self.y0, self.x1, self.y1
            ));
        }
        Ok(())
    }

    pub fn as_bbox(self) -> BBox {
        BBox {
            x0: self.x0,
            y0: self.y0,
            x1: self.x1,
            y1: self.y1,
        }
    }
}

/// Renderer-facing hierarchy-plan request.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanRequest {
    pub view: ViewBox,
    pub cut_dbu: i64,
    pub visible_layers: Option<Vec<String>>,
    pub depth: u32,
    pub px_per_dbu: f64,
    /// Exact requests disable planner LOD/wash and all size culling.
    pub exact: bool,
    /// Jobdeck wide-view policy: what the size cut drops keeps its
    /// on-screen existence as a footprint wash (floe_vfs::ViewReq).
    pub sub_cut_wash: bool,
    /// The page frontier (floe_vfs::ViewReq::page_reps): what the cut
    /// drops is thinned to representatives instead of vanishing.
    pub page_reps: bool,
    /// The decoded-generation budget the page frontier's page decode
    /// must fit under (floe_vfs::ViewReq::decode_budget; 0 = unknown).
    pub decode_budget: u64,
    /// The page hairline policy (floe_vfs::ViewReq::page_hairline):
    /// true culls all-thin pages (plain layout performance policy),
    /// false keeps them (mask / jobdeck policy).
    pub page_hairline: bool,
    /// Cache layer indices drawn from the occupancy summary (M2):
    /// their pages are not planned (floe_vfs::ViewReq::page_skip).
    pub summary_layers: Vec<u32>,
    /// Prune subtrees that hold only summarized layers (no frames
    /// wanted): floe_vfs::ViewReq::prune_skipped.
    pub prune_summary: bool,
    /// Sub-cut boxes (floe_vfs::ViewReq::sub_cut_box).
    pub sub_cut_box: bool,
    /// The per-shape cut (floe_vfs::ViewReq::shape_cut).
    pub shape_cut: bool,
    /// The hairline-keeping cut (floe_vfs::ViewReq::shape_cut_max).
    pub shape_cut_max: bool,
    /// The M7-C page wash (floe_vfs::ViewReq::page_wash).
    pub page_wash: bool,
    /// The M7 LOD swap (floe_vfs::ViewReq::lod_swap).
    pub lod_swap: bool,
    /// The frame draws hierarchy frames (floe_vfs::ViewReq::frames).
    pub frames: bool,
    /// The regions of `view` the plan is for (floe_vfs HierOpts::regions;
    /// empty = the whole view): the density stack's pass 2 plans the space
    /// the originals left (CUT_DENSITY_DESIGN §10.10).
    pub regions: Vec<ViewBox>,
    /// Visible layers by cache layer index, in place of `visible_layers`
    /// when Some (the density stack's pass 2: the top plane's layer alone,
    /// then the others).
    pub visible_indices: Option<Vec<u32>>,
    /// A budget fit decided before, to apply as it is (floe_vfs::hier::
    /// FixedFit; renderd keeps one per scale so the viewport frame and its
    /// margin thin alike).
    pub fixed_fit: Option<floe_vfs::hier::FixedFit>,
    /// The cell the plan starts from, in its own coordinates
    /// (floe_vfs::ViewReq::root; the viewer's view root, SPEC-VIEWER
    /// §8c); None = the top cell.
    pub root: Option<u32>,
    /// The density stack's sub-cut dots (floe_vfs::hier::HierOpts::
    /// sub_cut_dots, pass 2 only): `cut_dbu` is the cells' cut - a cell
    /// under it is a dot item, never walked into or decoded - and pages and
    /// records take `cut_dbu` times this share. None: one cut for all.
    pub sub_cut_dots: Option<f64>,
    /// The sub-cut dots' one walk (floe_vfs::hier::HierOpts::dot_records):
    /// pages at `cut_dbu`, records at `cut_dbu` times this share, a page all
    /// under the cut a dot item - no probe, no budget fit. None: as
    /// `sub_cut_dots` says.
    pub dot_records: Option<f64>,
    /// > 0: a probe of whether the plan fits (floe_vfs::hier::HierOpts::
    /// probe_limit): planned as asked, abandoned once its pages pass this
    /// many decoded bytes (stats.fit_over). 0: a plan.
    pub probe_limit: u64,
    /// Pages the frame holds decoded already, sorted
    /// (floe_vfs::hier::HierOpts::free_pages): the density stack's pass 1,
    /// whose pages cost its pass 2's budget nothing.
    pub free_pages: Option<std::sync::Arc<[u32]>>,
    /// An empty plan keeps its top working cell, empty: the frame is an
    /// empty picture (renderd, render-cli; user 2026-10-04: a layer the file
    /// names and no cell holds, alone on, drew `invalid plan: top is missing`
    /// - the routing chip's BOUNDARY 100/0). False: an empty plan has no
    /// working cell - a jobdeck source's, whose empty plan is a skipped pass.
    pub empty_top: bool,
    /// The density stack's brightness (floe_vfs HierOpts::dot_bright): Some(g)
    /// with GeometryRasterRequest::density_bright - pass 2's dots count the
    /// area they cover. None: the dots as lit pixels.
    pub dot_bright: Option<f64>,
    /// The occupancy first (floe_vfs HierOpts::dot_occ_first): Some(share)
    /// with `sub_cut_dots` at 1 - a page under the cells' cut is spread by a
    /// fine enough occupancy grid whatever its shapes' size; one without
    /// such a grid whose largest shape reaches `share` of the cut is decoded.
    /// None: the pages cut where `sub_cut_dots` says.
    pub dot_occ_first: Option<f64>,
}

impl PlanRequest {
    pub fn validate(&self) -> Result<(), String> {
        self.view.validate()?;
        for region in &self.regions {
            region.validate()?;
        }
        if self.cut_dbu < 0 {
            return Err(format!("invalid cut_dbu: {}", self.cut_dbu));
        }
        if !self.px_per_dbu.is_finite() || self.px_per_dbu < 0.0 {
            return Err(format!("invalid px_per_dbu: {}", self.px_per_dbu));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_rejects_reversed_axes() {
        assert!(ViewBox::new(2, 0, 1, 1).is_err());
        assert!(ViewBox::new(0, 2, 1, 1).is_err());
    }

    #[test]
    fn request_rejects_non_finite_scale() {
        let req = PlanRequest {
            view: ViewBox::new(0, 0, 1, 1).unwrap(),
            cut_dbu: 0,
            visible_layers: None,
            depth: FULL_DEPTH,
            px_per_dbu: f64::NAN,
            exact: true,
            sub_cut_wash: false,
            page_reps: false,
            decode_budget: 0,
            page_hairline: true,
            summary_layers: Vec::new(),
            prune_summary: false,
            sub_cut_box: false,
            shape_cut: false,
            shape_cut_max: false,
            frames: true,
            page_wash: true,
            lod_swap: true,
            regions: Vec::new(),
            visible_indices: None,
            fixed_fit: None,
            root: None,
            sub_cut_dots: None,
            dot_records: None,
            probe_limit: 0,
            free_pages: None,
            empty_top: true,
            dot_bright: None,
            dot_occ_first: None,
        };
        assert!(req.validate().is_err());
    }
}
