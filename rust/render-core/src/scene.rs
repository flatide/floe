use floe_vfs::hier::{HierPlan, WsCell, WsKey};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{Cache, DecodedPage};

/// Immutable geometry snapshot for one render/refinement round.
///
/// The hierarchy and repetitions remain in planner form. Only decoded page
/// references are indexed; no placement or shape is expanded here.
pub struct FrameScene {
    plan: Arc<HierPlan>,
    pages: BTreeMap<u32, Arc<DecodedPage>>,
    deferred_pages: Vec<u32>,
    cell_bounds: BTreeMap<WsKey, floe_ovm::BBox>,
}

impl FrameScene {
    pub fn new(
        source: &Cache,
        plan: HierPlan,
        decoded_pages: Vec<Arc<DecodedPage>>,
    ) -> Result<Self, String> {
        Self::new_shared(source, Arc::new(plan), decoded_pages)
    }

    /// Builds a refinement-round scene while sharing the immutable hierarchy
    /// plan with the other rounds of the same generation.
    pub fn new_shared(
        source: &Cache,
        plan: Arc<HierPlan>,
        decoded_pages: Vec<Arc<DecodedPage>>,
    ) -> Result<Self, String> {
        let mut cell_bounds = BTreeMap::new();
        for cell in &plan.wcells {
            cell_bounds.insert(cell.key, source.cell_bbox(cell.key.0)?);
        }
        Self::assemble(plan, decoded_pages, cell_bounds)
    }

    fn assemble(
        plan: Arc<HierPlan>,
        decoded_pages: Vec<Arc<DecodedPage>>,
        cell_bounds: BTreeMap<WsKey, floe_ovm::BBox>,
    ) -> Result<Self, String> {
        if plan.pages.len() != plan.page_prio.len() {
            return Err(format!(
                "invalid plan: {} pages but {} priorities",
                plan.pages.len(),
                plan.page_prio.len()
            ));
        }
        if !plan.pages.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err("invalid plan: pages are not sorted unique".to_string());
        }
        if !plan.wcells.windows(2).all(|pair| pair[0].key < pair[1].key) {
            return Err("invalid plan: working cells are not sorted unique".to_string());
        }
        if plan
            .wcells
            .binary_search_by_key(&plan.top, |cell| cell.key)
            .is_err()
        {
            return Err(format!("invalid plan: top {:?} is missing", plan.top));
        }

        let mut pages = BTreeMap::new();
        for page in decoded_pages {
            if plan.pages.binary_search(&page.page_id).is_err() {
                return Err(format!("decoded page {} is outside the plan", page.page_id));
            }
            if pages.insert(page.page_id, page).is_some() {
                return Err("duplicate decoded page in scene".to_string());
            }
        }
        let deferred_pages = plan
            .pages
            .iter()
            .copied()
            .filter(|page_id| !pages.contains_key(page_id))
            .collect();
        for cell in &plan.wcells {
            if !cell_bounds.contains_key(&cell.key) {
                return Err(format!(
                    "invalid scene: bbox for working cell {:?} is missing",
                    cell.key
                ));
            }
        }
        Ok(Self {
            plan,
            pages,
            deferred_pages,
            cell_bounds,
        })
    }

    pub fn top(&self) -> WsKey {
        self.plan.top
    }

    pub fn plan(&self) -> &HierPlan {
        self.plan.as_ref()
    }

    pub fn cell(&self, key: WsKey) -> Option<&WsCell> {
        self.plan
            .wcells
            .binary_search_by_key(&key, |cell| cell.key)
            .ok()
            .map(|index| &self.plan.wcells[index])
    }

    pub fn page(&self, page_id: u32) -> Option<&Arc<DecodedPage>> {
        self.pages.get(&page_id)
    }

    pub fn cell_bbox(&self, key: WsKey) -> Option<floe_ovm::BBox> {
        self.cell_bounds.get(&key).copied()
    }

    pub fn available_pages(&self) -> usize {
        self.pages.len()
    }

    pub fn deferred_pages(&self) -> &[u32] {
        &self.deferred_pages
    }

    pub fn is_partial(&self) -> bool {
        !self.deferred_pages.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn from_test_parts(
        plan: HierPlan,
        decoded_pages: Vec<Arc<DecodedPage>>,
        cell_bounds: BTreeMap<WsKey, floe_ovm::BBox>,
    ) -> Result<Self, String> {
        Self::assemble(Arc::new(plan), decoded_pages, cell_bounds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_oasis::doc::{Cell, Doc};
    use floe_ovm::BBox;
    use floe_vfs::hier::{HierStats, WsCell, REM_FULL};
    use std::collections::HashMap;

    fn page(page_id: u32) -> Arc<DecodedPage> {
        Arc::new(DecodedPage {
            page_id,
            layer_idx: 0,
            bbox: BBox {
                x0: 0,
                y0: 0,
                x1: 10,
                y1: 10,
            },
            encoded_bytes: 1,
            records: 0,
            members: 0,
            doc: Doc {
                unit: 1000.0,
                cells: vec![Cell::default()],
                top: 0,
                layer_order: Vec::new(),
                norm_s: 0.0,
                layer_names: HashMap::new(),
                layer_aliases: HashMap::new(),
            },
        })
    }

    fn plan() -> HierPlan {
        let top = (0, REM_FULL);
        HierPlan {
            top,
            wcells: vec![WsCell {
                key: top,
                pages: vec![2, 4],
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
            }],
            pages: vec![2, 4],
            page_prio: vec![0, 1],
            stats: HierStats::default(),
        }
    }

    #[test]
    fn tracks_available_and_deferred_pages() {
        let mut bounds = BTreeMap::new();
        bounds.insert(plan().top, page(2).bbox);
        let scene = FrameScene::assemble(Arc::new(plan()), vec![page(2)], bounds).unwrap();
        assert_eq!(scene.available_pages(), 1);
        assert_eq!(scene.deferred_pages(), &[4]);
        assert!(scene.is_partial());
        assert!(scene.cell(scene.top()).is_some());
        assert!(scene.page(2).is_some());
    }

    #[test]
    fn rejects_page_outside_plan() {
        let mut bounds = BTreeMap::new();
        bounds.insert(plan().top, page(2).bbox);
        let error = FrameScene::assemble(Arc::new(plan()), vec![page(3)], bounds)
            .err()
            .expect("scene should reject foreign page");
        assert!(error.contains("outside the plan"));
    }
}
