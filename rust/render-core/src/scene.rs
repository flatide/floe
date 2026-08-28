use floe_vfs::hier::{HierPlan, WsCell, WsKey};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{validate_font_px, Cache, DecodedPage, RenderLabel, DEFAULT_LABEL_FONT_PX};

/// Immutable geometry snapshot for one render/refinement round.
///
/// The hierarchy and repetitions remain in planner form. Only decoded page
/// references are indexed; no placement or shape is expanded here.
pub struct FrameScene {
    plan: Arc<HierPlan>,
    pages: BTreeMap<u32, Arc<DecodedPage>>,
    deferred_pages: Vec<u32>,
    cell_bounds: BTreeMap<WsKey, floe_ovm::BBox>,
    labels: Arc<[RenderLabel]>,
    label_font_px: f32,
    masks: SceneMasks,
}

/// Bottom-up subtree content masks over the plan hierarchy (F2R-03b
/// 2b). `layers` is the dense per-round table of layer indices any
/// decoded page or wash can paint; `bits`/`frames` hold one cumulative
/// subtree mask per working cell (row-aligned with `plan.wcells`).
/// Deferred pages contribute nothing on purpose: the raster walk skips
/// undecoded pages anyway, so a subtree whose pages are all deferred
/// prunes without changing a pixel — the next round rebuilds the scene
/// and the masks with it.
struct SceneMasks {
    layers: Vec<u32>,
    words: usize,
    bits: Vec<u64>,
    frames: Vec<bool>,
    /// FULL masks: every subtree answers "paints everything", so the
    /// walk never prunes. Used when the bit matrix would outgrow
    /// `MASK_BUDGET_BYTES` (review 2026-08-28: wcells × layer words
    /// is otherwise an unbounded allocation outside the decoded-page
    /// budget) and by the test oracle. Pixels are identical either
    /// way — masks only ever skip provably empty work.
    full: bool,
}

/// Cap on the subtree bit matrix. 16MiB covers a million working
/// cells at 128 decoded layers; a plan past that renders unpruned
/// rather than spiking memory the LRU budget never sees.
const MASK_BUDGET_BYTES: usize = 16 << 20;

impl SceneMasks {
    fn build(plan: &HierPlan, pages: &BTreeMap<u32, Arc<DecodedPage>>) -> Self {
        Self::build_bounded(plan, pages, MASK_BUDGET_BYTES)
    }

    fn full_masks() -> Self {
        Self {
            layers: Vec::new(),
            words: 0,
            bits: Vec::new(),
            frames: Vec::new(),
            full: true,
        }
    }

    fn build_bounded(
        plan: &HierPlan,
        pages: &BTreeMap<u32, Arc<DecodedPage>>,
        cap_bytes: usize,
    ) -> Self {
        let cells = &plan.wcells;
        let mut layers: Vec<u32> = pages.values().map(|page| page.layer_idx).collect();
        for cell in cells {
            layers.extend(cell.washes.iter().map(|&(layer_idx, _)| layer_idx));
        }
        layers.sort_unstable();
        layers.dedup();
        let words = layers.len().div_ceil(64).max(1);
        let Some(bit_words) = cells.len().checked_mul(words) else {
            return Self::full_masks();
        };
        let mask_bytes = bit_words
            .checked_mul(std::mem::size_of::<u64>())
            .and_then(|bytes| bytes.checked_add(cells.len()));
        match mask_bytes {
            Some(bytes) if bytes <= cap_bytes => {}
            _ => return Self::full_masks(),
        }
        let mut bits = vec![0u64; bit_words];
        let mut frames = vec![false; cells.len()];
        for (index, cell) in cells.iter().enumerate() {
            frames[index] = !cell.frames.is_empty();
            let row = index * words;
            for &page_id in &cell.pages {
                if let Some(page) = pages.get(&page_id) {
                    if let Ok(bit) = layers.binary_search(&page.layer_idx) {
                        bits[row + bit / 64] |= 1u64 << (bit % 64);
                    }
                }
            }
            for &(layer_idx, _) in &cell.washes {
                if let Ok(bit) = layers.binary_search(&layer_idx) {
                    bits[row + bit / 64] |= 1u64 << (bit % 64);
                }
            }
        }
        // Bottom-up union over instance edges: one memoized DFS,
        // O(cells + edges) row unions. A cycle back-edge floods the
        // visiting cell instead of erroring here: the walk must keep
        // descending so its own cycle validation stays reachable. A
        // child missing from wcells is skipped for the same reason —
        // the walk's missing-cell error fires before any pruning.
        let mut state = vec![0u8; cells.len()]; // 0 new, 1 open, 2 done
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for root in 0..cells.len() {
            if state[root] != 0 {
                continue;
            }
            state[root] = 1;
            stack.push((root, 0));
            while let Some(&(index, edge)) = stack.last() {
                let cell = &cells[index];
                if let Some(instance) = cell.insts.get(edge) {
                    stack.last_mut().expect("stack is non-empty").1 += 1;
                    let Ok(child) =
                        cells.binary_search_by_key(&instance.child, |cell| cell.key)
                    else {
                        continue;
                    };
                    match state[child] {
                        0 => {
                            state[child] = 1;
                            stack.push((child, 0));
                        }
                        1 => {
                            let row = index * words;
                            for word in &mut bits[row..row + words] {
                                *word = !0;
                            }
                            frames[index] = true;
                        }
                        _ => {
                            union_rows(&mut bits, words, index, child);
                            frames[index] |= frames[child];
                        }
                    }
                } else {
                    state[index] = 2;
                    stack.pop();
                    if let Some(&(parent, _)) = stack.last() {
                        union_rows(&mut bits, words, parent, index);
                        frames[parent] |= frames[index];
                    }
                }
            }
        }
        Self {
            layers,
            words,
            bits,
            frames,
            full: false,
        }
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        self.layers.len() * std::mem::size_of::<u32>()
            + self.bits.len() * std::mem::size_of::<u64>()
            + self.frames.len()
    }
}

fn union_rows(bits: &mut [u64], words: usize, dst: usize, src: usize) {
    if dst == src {
        return;
    }
    let (dst_row, src_row) = if dst < src {
        let (head, tail) = bits.split_at_mut(src * words);
        (&mut head[dst * words..(dst + 1) * words], &tail[..words])
    } else {
        let (head, tail) = bits.split_at_mut(dst * words);
        (&mut tail[..words], &head[src * words..(src + 1) * words])
    };
    for (dst_word, src_word) in dst_row.iter_mut().zip(src_row) {
        *dst_word |= *src_word;
    }
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
        Self::new_shared_with_labels(
            source,
            plan,
            decoded_pages,
            Arc::from([]),
            DEFAULT_LABEL_FONT_PX,
        )
    }

    /// Builds a refinement scene with one immutable request-scoped label plan.
    /// Every round and raster worker shares these rows; page decoding never
    /// causes the hierarchy/text planner to run again.
    pub fn new_shared_with_labels(
        source: &Cache,
        plan: Arc<HierPlan>,
        decoded_pages: Vec<Arc<DecodedPage>>,
        labels: Arc<[RenderLabel]>,
        label_font_px: f32,
    ) -> Result<Self, String> {
        validate_font_px(label_font_px)?;
        let mut cell_bounds = BTreeMap::new();
        for cell in &plan.wcells {
            cell_bounds.insert(cell.key, source.cell_bbox(cell.key.0)?);
        }
        Self::assemble_with_labels(plan, decoded_pages, cell_bounds, labels, label_font_px)
    }

    fn assemble_with_labels(
        plan: Arc<HierPlan>,
        decoded_pages: Vec<Arc<DecodedPage>>,
        cell_bounds: BTreeMap<WsKey, floe_ovm::BBox>,
        labels: Arc<[RenderLabel]>,
        label_font_px: f32,
    ) -> Result<Self, String> {
        validate_font_px(label_font_px)?;
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
        // Band range validation lives here so it cannot depend on which
        // subtrees the masked walk happens to visit (F2R-03b 2b).
        for cell in &plan.wcells {
            for (_, _, frame_band) in &cell.frames {
                if *frame_band > 3 {
                    return Err(format!(
                        "invalid plan: hierarchy frame band {} is outside 0..=3",
                        frame_band
                    ));
                }
            }
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
        let masks = SceneMasks::build(&plan, &pages);
        Ok(Self {
            plan,
            pages,
            deferred_pages,
            cell_bounds,
            labels,
            label_font_px,
            masks,
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

    pub fn labels(&self) -> &[RenderLabel] {
        &self.labels
    }

    pub fn label_font_px(&self) -> f32 {
        self.label_font_px
    }

    pub fn is_partial(&self) -> bool {
        !self.deferred_pages.is_empty()
    }

    /// Dense mask bit for a styled layer, or None when no decoded page
    /// or wash in this round can paint it (F2R-03b 2b).
    pub fn layer_mask_bit(&self, layer_idx: u32) -> Option<usize> {
        if self.masks.full {
            return Some(usize::MAX);
        }
        self.masks.layers.binary_search(&layer_idx).ok()
    }

    /// Whether the subtree rooted at `key` can paint the given dense
    /// layer bit this round. A cell missing from the plan answers true:
    /// pruning must never outrun the walk's own missing-cell errors.
    pub fn subtree_paints(&self, key: WsKey, bit: Option<usize>) -> bool {
        if self.masks.full {
            return true;
        }
        let Ok(index) = self
            .plan
            .wcells
            .binary_search_by_key(&key, |cell| cell.key)
        else {
            return true;
        };
        let Some(bit) = bit else {
            return false;
        };
        let row = index * self.masks.words;
        self.masks.bits[row + bit / 64] & (1u64 << (bit % 64)) != 0
    }

    /// Dense query words covering a set of styled layers, for the
    /// work-bin collection walk's combined gate (F2R-03b 2c). All
    /// zeros under FULL masks — `subtree_intersects` answers true
    /// there regardless.
    pub fn layer_query_words(&self, layers: &[u32]) -> Vec<u64> {
        let mut words = vec![0u64; self.masks.words];
        if self.masks.full {
            return words;
        }
        for &layer_idx in layers {
            if let Ok(bit) = self.masks.layers.binary_search(&layer_idx) {
                words[bit / 64] |= 1u64 << (bit % 64);
            }
        }
        words
    }

    /// Whether the subtree rooted at `key` can paint ANY queried layer
    /// or (when `want_frames`) holds hierarchy frames — the one-pass
    /// gate of the work-bin collection walk. Unknown cells answer true
    /// so the walk's own validation errors stay reachable.
    pub fn subtree_intersects(&self, key: WsKey, query: &[u64], want_frames: bool) -> bool {
        if self.masks.full {
            return true;
        }
        let Ok(index) = self
            .plan
            .wcells
            .binary_search_by_key(&key, |cell| cell.key)
        else {
            return true;
        };
        if want_frames && self.masks.frames[index] {
            return true;
        }
        let row = index * self.masks.words;
        self.masks.bits[row..row + self.masks.words]
            .iter()
            .zip(query)
            .any(|(word, wanted)| word & wanted != 0)
    }

    /// Whether the subtree rooted at `key` holds any hierarchy frame.
    pub fn subtree_has_frames(&self, key: WsKey) -> bool {
        if self.masks.full {
            return true;
        }
        match self
            .plan
            .wcells
            .binary_search_by_key(&key, |cell| cell.key)
        {
            Ok(index) => self.masks.frames[index],
            Err(_) => true,
        }
    }

    /// Heap footprint of the per-round subtree masks (telemetry only —
    /// the scene lives for one round, not in the decoded-page LRU).
    pub fn mask_bytes(&self) -> usize {
        self.masks.estimated_bytes()
    }

    /// Mask-off oracle: every subtree answers "paints everything", so a
    /// render equals the pre-2b unpruned walk exactly.
    #[cfg(test)]
    pub(crate) fn with_full_masks(mut self) -> Self {
        self.masks.full = true;
        self
    }

    #[cfg(test)]
    pub(crate) fn from_test_parts(
        plan: HierPlan,
        decoded_pages: Vec<Arc<DecodedPage>>,
        cell_bounds: BTreeMap<WsKey, floe_ovm::BBox>,
    ) -> Result<Self, String> {
        Self::assemble_with_labels(
            Arc::new(plan),
            decoded_pages,
            cell_bounds,
            Arc::from([]),
            DEFAULT_LABEL_FONT_PX,
        )
    }

    #[cfg(test)]
    pub(crate) fn from_test_parts_with_labels(
        plan: HierPlan,
        decoded_pages: Vec<Arc<DecodedPage>>,
        cell_bounds: BTreeMap<WsKey, floe_ovm::BBox>,
        labels: Arc<[RenderLabel]>,
        label_font_px: f32,
    ) -> Result<Self, String> {
        Self::assemble_with_labels(
            Arc::new(plan),
            decoded_pages,
            cell_bounds,
            labels,
            label_font_px,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_oasis::doc::{Cell, Doc, Rep};
    use floe_ovm::BBox;
    use floe_vfs::hier::{HierStats, WsCell, WsInst, REM_FULL};
    use std::collections::HashMap;

    fn page(page_id: u32) -> Arc<DecodedPage> {
        let doc = Doc {
            unit: 1000.0,
            cells: vec![Cell::default()],
            top: 0,
            layer_order: Vec::new(),
            norm_s: 0.0,
            layer_names: HashMap::new(),
            layer_aliases: HashMap::new(),
        };
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
            index: crate::PageIndex::build(&doc),
            doc,
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
        let scene = FrameScene::from_test_parts(plan(), vec![page(2)], bounds).unwrap();
        assert_eq!(scene.available_pages(), 1);
        assert_eq!(scene.deferred_pages(), &[4]);
        assert!(scene.is_partial());
        assert!(scene.cell(scene.top()).is_some());
        assert!(scene.page(2).is_some());
    }

    fn layer_page(page_id: u32, layer_idx: u32) -> Arc<DecodedPage> {
        let mut decoded = page(page_id);
        Arc::get_mut(&mut decoded).expect("fresh page").layer_idx = layer_idx;
        decoded
    }

    #[test]
    fn rejects_frame_band_outside_range() {
        let mut bad = plan();
        bad.wcells[0].frames.push((
            BBox {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 1,
            },
            Rep::One,
            4,
        ));
        let mut bounds = BTreeMap::new();
        bounds.insert(bad.top, page(2).bbox);
        let error = FrameScene::from_test_parts(bad, vec![page(2)], bounds)
            .err()
            .expect("band 4 must be rejected at assembly");
        assert!(error.contains("outside 0..=3"), "unexpected error: {error}");
    }

    #[test]
    fn masks_track_decoded_pages_washes_and_children() {
        let top = (0, REM_FULL);
        let child = (1, REM_FULL);
        let bbox = BBox {
            x0: 0,
            y0: 0,
            x1: 10,
            y1: 10,
        };
        let plan = HierPlan {
            top,
            wcells: vec![
                WsCell {
                    key: top,
                    pages: Vec::new(),
                    insts: vec![WsInst {
                        child,
                        x: 0,
                        y: 0,
                        rot: 0,
                        flip: false,
                        rep: Rep::One,
                    }],
                    frames: Vec::new(),
                    washes: vec![(5, bbox)],
                },
                WsCell {
                    key: child,
                    // page 2 decodes on layer 3; page 4 stays deferred, so
                    // its layer never reaches the mask table
                    pages: vec![2, 4],
                    insts: Vec::new(),
                    frames: vec![(bbox, Rep::One, 2)],
                    washes: Vec::new(),
                },
            ],
            pages: vec![2, 4],
            page_prio: vec![0, 1],
            stats: HierStats::default(),
        };
        let bounds = BTreeMap::from([(top, bbox), (child, bbox)]);
        let scene =
            FrameScene::from_test_parts(plan, vec![layer_page(2, 3)], bounds).unwrap();

        let decoded_bit = scene.layer_mask_bit(3);
        assert!(decoded_bit.is_some());
        assert!(scene.subtree_paints(top, decoded_bit), "child content unions up");
        assert!(scene.subtree_paints(child, decoded_bit));

        let wash_bit = scene.layer_mask_bit(5);
        assert!(wash_bit.is_some(), "washes count as content");
        assert!(scene.subtree_paints(top, wash_bit));
        assert!(!scene.subtree_paints(child, wash_bit), "wash is top's own");

        assert_eq!(scene.layer_mask_bit(9), None, "deferred pages add nothing");
        assert!(!scene.subtree_paints(child, None));
        assert!(
            scene.subtree_paints((99, REM_FULL), None),
            "unknown cells must never be pruned"
        );

        assert!(scene.subtree_has_frames(top), "child frames union up");
        assert!(scene.subtree_has_frames(child));
        assert!(scene.mask_bytes() > 0);

        // A bit matrix past the byte cap falls back to FULL masks:
        // nothing prunes, nothing allocates, pixels are unchanged.
        let mut scene = scene;
        let plan = Arc::clone(&scene.plan);
        let capped = SceneMasks::build_bounded(&plan, &scene.pages, 1);
        assert!(capped.full);
        assert!(capped.bits.is_empty());
        scene.masks = capped;
        assert_eq!(scene.mask_bytes(), 0);
        assert_eq!(scene.layer_mask_bit(9), Some(usize::MAX),
                   "full masks never declare a layer absent");
        assert!(scene.subtree_paints(child, None), "full masks never prune");
        assert!(scene.subtree_has_frames(top));
    }

    #[test]
    fn rejects_page_outside_plan() {
        let mut bounds = BTreeMap::new();
        bounds.insert(plan().top, page(2).bbox);
        let error = FrameScene::from_test_parts(plan(), vec![page(3)], bounds)
            .err()
            .expect("scene should reject foreign page");
        assert!(error.contains("outside the plan"));
    }
}
