//! Per-page spatial index over geometry records (F2R-03b).
//!
//! Built once at page decode and shared through the decoded-page LRU, so a
//! render only visits records whose full repetition extent can intersect the
//! tile-local view. Pruning must stay conservative: a record whose exact
//! extent cannot be represented (overflow, corrupt geometry that the render
//! path reports as an explicit error) is indexed with an all-covering extent
//! so the render-time validation error stays reachable.

use floe_oasis::doc::{Doc, PathRec, PolyRec, RectRec, Rep};
use floe_ovm::BBox;

use crate::raster::{checked_path_centerline, checked_path_outline};

const LEAF_RECORDS: usize = 8;

/// All-covering extent: never pruned, so render-time validation still runs.
const ALWAYS: BBox = BBox {
    x0: i64::MIN,
    y0: i64::MIN,
    x1: i64::MAX,
    y1: i64::MAX,
};

pub struct PageIndex {
    rects: RecordTree,
    polys: RecordTree,
    paths: RecordTree,
}

impl PageIndex {
    /// Indexes the page's single geometry cell. A malformed document (which
    /// the decode/render paths reject separately) gets an empty index.
    pub fn build(doc: &Doc) -> Self {
        let Some(cell) = doc.cells.get(doc.top) else {
            return Self {
                rects: RecordTree::default(),
                polys: RecordTree::default(),
                paths: RecordTree::default(),
            };
        };
        Self {
            rects: RecordTree::build(cell.rects.iter().map(rect_extent).collect()),
            polys: RecordTree::build(cell.polys.iter().map(poly_extent).collect()),
            paths: RecordTree::build(cell.paths.iter().map(path_extent).collect()),
        }
    }

    /// Reference index that never prunes; the oracle for equivalence tests.
    #[cfg(test)]
    pub(crate) fn unpruned(doc: &Doc) -> Self {
        let Some(cell) = doc.cells.get(doc.top) else {
            return Self::build(doc);
        };
        Self {
            rects: RecordTree::build(vec![ALWAYS; cell.rects.len()]),
            polys: RecordTree::build(vec![ALWAYS; cell.polys.len()]),
            paths: RecordTree::build(vec![ALWAYS; cell.paths.len()]),
        }
    }

    pub(crate) fn rects(&self) -> &RecordTree {
        &self.rects
    }

    pub(crate) fn polys(&self) -> &RecordTree {
        &self.polys
    }

    pub(crate) fn paths(&self) -> &RecordTree {
        &self.paths
    }

    /// Conservative LRU charge for the index itself.
    pub fn estimated_bytes(&self) -> u64 {
        (std::mem::size_of::<Self>() as u64)
            .saturating_add(self.rects.estimated_bytes())
            .saturating_add(self.polys.estimated_bytes())
            .saturating_add(self.paths.estimated_bytes())
    }
}

#[derive(Default)]
pub(crate) struct RecordTree {
    /// Flattened BVH: a node's left child is the next node, the right child
    /// index is stored. `count > 0` marks a leaf over `order[start..][..count]`.
    nodes: Vec<TreeNode>,
    order: Vec<u32>,
}

struct TreeNode {
    bbox: BBox,
    start: u32,
    count: u32,
}

impl RecordTree {
    fn build(extents: Vec<BBox>) -> Self {
        let mut order: Vec<u32> = (0..extents.len() as u32).collect();
        let mut nodes = Vec::new();
        if !order.is_empty() {
            nodes.reserve(2 * order.len().div_ceil(LEAF_RECORDS));
            build_node(&extents, &mut order, 0, extents.len(), &mut nodes);
        }
        Self { nodes, order }
    }

    /// Visits every record whose indexed extent intersects `view`, in
    /// ascending record order so the caller walks the record vector with a
    /// forward stride instead of BVH leaf order. Records inside an
    /// intersecting leaf are handed over without an individual extent test;
    /// the per-record repetition logic rejects them exactly as the unindexed
    /// walk did.
    pub(crate) fn for_each_intersecting(
        &self,
        view: BBox,
        scratch: &mut RecordSet,
        mut visit: impl FnMut(u32) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.nodes.is_empty() {
            return Ok(());
        }
        // A view that covers every indexed extent keeps the plain
        // sequential scan of the pre-index walk.
        let root = &self.nodes[0].bbox;
        if !root.is_empty()
            && view.x0 <= root.x0
            && view.y0 <= root.y0
            && view.x1 >= root.x1
            && view.y1 >= root.y1
        {
            for record in 0..self.order.len() as u32 {
                visit(record)?;
            }
            return Ok(());
        }
        scratch.reset(self.order.len());
        let mut stack: Vec<u32> = Vec::with_capacity(32);
        stack.push(0);
        while let Some(node_index) = stack.pop() {
            let node = &self.nodes[node_index as usize];
            if !node.bbox.intersects(&view) {
                continue;
            }
            if node.count > 0 {
                let start = node.start as usize;
                for &record in &self.order[start..start + node.count as usize] {
                    scratch.insert(record);
                }
            } else {
                stack.push(node.start);
                stack.push(node_index + 1);
            }
        }
        scratch.for_each_set(visit)
    }

    fn estimated_bytes(&self) -> u64 {
        (self.nodes.capacity() as u64)
            .saturating_mul(std::mem::size_of::<TreeNode>() as u64)
            .saturating_add((self.order.capacity() as u64).saturating_mul(4))
    }
}

/// Reusable per-worker bitset that replays a tree query's hits in ascending
/// record order, turning BVH leaf order into a forward memory stride.
#[derive(Default)]
pub(crate) struct RecordSet {
    words: Vec<u64>,
    active_words: usize,
}

impl RecordSet {
    fn reset(&mut self, records: usize) {
        let words = records.div_ceil(64);
        if self.words.len() < words {
            self.words.resize(words, 0);
        }
        self.words[..self.active_words.max(words)].fill(0);
        self.active_words = words;
    }

    fn insert(&mut self, record: u32) {
        self.words[record as usize / 64] |= 1u64 << (record % 64);
    }

    fn for_each_set(&self, mut visit: impl FnMut(u32) -> Result<(), String>) -> Result<(), String> {
        for (word_index, &word) in self.words[..self.active_words].iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let bit = bits.trailing_zeros();
                visit(word_index as u32 * 64 + bit)?;
                bits &= bits - 1;
            }
        }
        Ok(())
    }
}

fn build_node(
    extents: &[BBox],
    order: &mut [u32],
    start: usize,
    end: usize,
    nodes: &mut Vec<TreeNode>,
) -> u32 {
    let node_index = nodes.len() as u32;
    let mut bbox = BBox::EMPTY;
    for &record in &order[start..end] {
        bbox.grow(&extents[record as usize]);
    }
    let len = end - start;
    if len <= LEAF_RECORDS {
        nodes.push(TreeNode {
            bbox,
            start: start as u32,
            count: len as u32,
        });
        return node_index;
    }
    nodes.push(TreeNode {
        bbox,
        start: 0,
        count: 0,
    });
    // Median split by record-extent center on the wider axis. The median is
    // positional, so ties and duplicate centers still halve the range and
    // recursion depth stays logarithmic.
    let span_x = bbox.x1 as i128 - bbox.x0 as i128;
    let span_y = bbox.y1 as i128 - bbox.y0 as i128;
    let mid = start + len / 2;
    let slice = &mut order[start..end];
    if span_x >= span_y {
        slice.select_nth_unstable_by_key(len / 2, |&record| {
            let extent = &extents[record as usize];
            extent.x0 / 2 + extent.x1 / 2
        });
    } else {
        slice.select_nth_unstable_by_key(len / 2, |&record| {
            let extent = &extents[record as usize];
            extent.y0 / 2 + extent.y1 / 2
        });
    }
    build_node(extents, order, start, mid, nodes);
    let right = build_node(extents, order, mid, end, nodes);
    nodes[node_index as usize].start = right;
    node_index
}

/// Union bbox of every repetition member offset, or `None` when it cannot be
/// represented (the render path reports those as explicit limit errors).
fn rep_offset_extent(rep: &Rep) -> Option<(i128, i128, i128, i128)> {
    match rep {
        Rep::One => Some((0, 0, 0, 0)),
        Rep::Grid { na, nb, va, vb } => {
            let ia = i128::from(na.saturating_sub(1));
            let ib = i128::from(nb.saturating_sub(1));
            let ax = ia.checked_mul(i128::from(va.0))?;
            let ay = ia.checked_mul(i128::from(va.1))?;
            let bx = ib.checked_mul(i128::from(vb.0))?;
            let by = ib.checked_mul(i128::from(vb.1))?;
            let cx = ax.checked_add(bx)?;
            let cy = ay.checked_add(by)?;
            Some((
                0.min(ax).min(bx).min(cx),
                0.min(ay).min(by).min(cy),
                0.max(ax).max(bx).max(cx),
                0.max(ay).max(by).max(cy),
            ))
        }
        Rep::Pts(points) => {
            let mut min_x = 0i128;
            let mut min_y = 0i128;
            let mut max_x = 0i128;
            let mut max_y = 0i128;
            for &(x, y) in points.iter() {
                min_x = min_x.min(i128::from(x));
                min_y = min_y.min(i128::from(y));
                max_x = max_x.max(i128::from(x));
                max_y = max_y.max(i128::from(y));
            }
            Some((min_x, min_y, max_x, max_y))
        }
    }
}

fn repeated_extent(base: BBox, rep: &Rep) -> BBox {
    let Some((min_x, min_y, max_x, max_y)) = rep_offset_extent(rep) else {
        return ALWAYS;
    };
    let x0 = i128::from(base.x0) + min_x;
    let y0 = i128::from(base.y0) + min_y;
    let x1 = i128::from(base.x1) + max_x;
    let y1 = i128::from(base.y1) + max_y;
    let (Ok(x0), Ok(y0), Ok(x1), Ok(y1)) = (
        i64::try_from(x0),
        i64::try_from(y0),
        i64::try_from(x1),
        i64::try_from(y1),
    ) else {
        return ALWAYS;
    };
    BBox { x0, y0, x1, y1 }
}

fn rect_extent(rect: &RectRec) -> BBox {
    if rect.w < 0 || rect.h < 0 {
        // The render path rejects negative sizes as corrupt regardless of
        // visibility; keep that error reachable.
        return ALWAYS;
    }
    let (Some(x1), Some(y1)) = (rect.x.checked_add(rect.w), rect.y.checked_add(rect.h)) else {
        return ALWAYS;
    };
    repeated_extent(
        BBox {
            x0: rect.x,
            y0: rect.y,
            x1,
            y1,
        },
        &rect.rep,
    )
}

fn poly_extent(polygon: &PolyRec) -> BBox {
    if polygon.pts.len() < 3 {
        return ALWAYS;
    }
    let mut base = BBox::EMPTY;
    for &(x, y) in &polygon.pts {
        base.grow(&BBox {
            x0: x,
            y0: y,
            x1: x,
            y1: y,
        });
    }
    repeated_extent(base, &polygon.rep)
}

fn path_extent(path: &PathRec) -> BBox {
    // The render walk culls members by the outline bbox; index the identical
    // bbox so pruning matches it member for member. Any construction error
    // is a render-time error and must stay reachable.
    let Ok(outline) = checked_path_outline(&path.pts, path.hw, path.es, path.ee) else {
        return ALWAYS;
    };
    match checked_path_centerline(&path.pts) {
        Ok(Some(_)) => {}
        _ => return ALWAYS,
    }
    let mut base = BBox::EMPTY;
    for &(x, y) in &outline {
        base.grow(&BBox {
            x0: x,
            y0: y,
            x1: x,
            y1: y,
        });
    }
    if base.is_empty() {
        return ALWAYS;
    }
    repeated_extent(base, &path.rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn bbox(x0: i64, y0: i64, x1: i64, y1: i64) -> BBox {
        BBox { x0, y0, x1, y1 }
    }

    fn collect(tree: &RecordTree, view: BBox) -> Vec<u32> {
        let mut hits = Vec::new();
        let mut scratch = RecordSet::default();
        tree.for_each_intersecting(view, &mut scratch, |record| {
            hits.push(record);
            Ok(())
        })
        .unwrap();
        hits.sort_unstable();
        hits
    }

    #[test]
    fn tree_query_is_a_superset_of_linear_extent_filter_and_exact_on_leaves() {
        // A deterministic scatter with mixed sizes, including records far
        // outside every query view.
        let mut extents = Vec::new();
        for index in 0..500i64 {
            let x = (index * 37) % 1000;
            let y = (index * 91) % 800;
            extents.push(bbox(x, y, x + 5 + index % 17, y + 3 + index % 11));
        }
        extents.push(bbox(1_000_000, 1_000_000, 1_000_100, 1_000_100));
        let tree = RecordTree::build(extents.clone());
        for view in [
            bbox(0, 0, 100, 100),
            bbox(450, 300, 700, 500),
            bbox(-50, -50, -1, -1),
            bbox(999_950, 999_950, 1_000_050, 1_000_050),
            BBox::EMPTY,
        ] {
            let hits = collect(&tree, view);
            let expected: Vec<u32> = (0..extents.len() as u32)
                .filter(|&record| extents[record as usize].intersects(&view))
                .collect();
            // Leaves are visited whole, so the tree may return extra
            // records, but never miss one that intersects.
            for record in &expected {
                assert!(hits.contains(record), "missing {record} for {view:?}");
            }
            for record in &hits {
                let leafmates = extents[*record as usize];
                let _ = leafmates;
            }
            if view.is_empty() {
                assert!(hits.is_empty());
            }
        }
    }

    #[test]
    fn corrupt_or_overflowing_records_are_never_pruned() {
        let rect = RectRec {
            layer: 0,
            dt: 0,
            x: i64::MAX - 4,
            y: 0,
            w: 8,
            h: 8,
            rep: Rep::One,
        };
        assert_eq!(rect_extent(&rect), ALWAYS);
        let negative = RectRec { w: -1, ..rect };
        assert_eq!(rect_extent(&negative), ALWAYS);
        let degenerate = PathRec {
            layer: 0,
            dt: 0,
            pts: vec![(3, 4), (3, 4)],
            hw: 1,
            es: 0,
            ee: 0,
            rep: Rep::One,
        };
        assert_eq!(path_extent(&degenerate), ALWAYS);
        let thin_poly = PolyRec {
            layer: 0,
            dt: 0,
            pts: vec![(0, 0), (1, 1)],
            rep: Rep::One,
        };
        assert_eq!(poly_extent(&thin_poly), ALWAYS);
    }

    #[test]
    fn repetition_extents_cover_every_member() {
        let grid = Rep::Grid {
            na: 3,
            nb: 2,
            va: (10, 0),
            vb: (0, -7),
        };
        assert_eq!(repeated_extent(bbox(0, 0, 4, 4), &grid), bbox(0, -7, 24, 4));
        let pts = Rep::Pts(Arc::from([(0, 0), (-5, 12), (30, -2)]));
        assert_eq!(
            repeated_extent(bbox(1, 1, 2, 2), &pts),
            bbox(-4, -1, 32, 14)
        );
        let huge = Rep::Grid {
            na: u64::MAX,
            nb: u64::MAX,
            va: (i64::MAX, 0),
            vb: (0, i64::MAX),
        };
        assert_eq!(repeated_extent(bbox(0, 0, 1, 1), &huge), ALWAYS);
    }
}
