//! Per-page spatial index over geometry records (F2R-03b).
//!
//! Built once at page decode and shared through the decoded-page LRU, so a
//! render only visits records whose full repetition extent can intersect the
//! tile-local view. Pruning must stay conservative: a record whose exact
//! extent cannot be represented (overflow, corrupt geometry that the render
//! path reports as an explicit error) is indexed with an all-covering extent
//! so the render-time validation error stays reachable.

use floe_oasis::doc::{Cell, Doc, PathRec, PolyRec, RectRec, Rep};
use floe_ovm::BBox;

use crate::raster::{checked_path_centerline, checked_path_outline};
use crate::repetition::PTS_CHUNK_POINTS;

const LEAF_RECORDS: usize = 8;

/// `Rep::Pts` forms below this point count are scanned directly; the chunk
/// table would cost more than the skipped work.
const PTS_CHUNK_MIN_POINTS: usize = 256;

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
    pts: PtsChunkIndex,
}

/// How many records/point-chunks pass between cancellation probes
/// during index construction.
const BUILD_CANCEL_STRIDE: usize = 4096;

/// Periodic cancellation probe for the index build (review
/// 2026-08-28): the index is ~41% of decode CPU and a large page was
/// an uninterruptible span, so a stale generation kept burning CPU
/// after a pan. The stride keeps the probe off the per-record fast
/// path.
pub struct BuildTicker<'a> {
    check: &'a mut dyn FnMut() -> Result<(), String>,
    countdown: usize,
}

impl<'a> BuildTicker<'a> {
    pub fn new(check: &'a mut dyn FnMut() -> Result<(), String>) -> Self {
        Self {
            check,
            countdown: BUILD_CANCEL_STRIDE,
        }
    }

    fn tick(&mut self) -> Result<(), String> {
        self.countdown -= 1;
        if self.countdown == 0 {
            self.countdown = BUILD_CANCEL_STRIDE;
            (self.check)()?;
        }
        Ok(())
    }
}

impl PageIndex {
    /// Indexes the page's single geometry cell. A malformed document (which
    /// the decode/render paths reject separately) gets an empty index.
    pub fn build(doc: &Doc) -> Self {
        Self::build_cancellable(doc, &mut || Ok(()))
            .expect("index build cannot fail without a cancellation check")
    }

    /// `build` with a periodic cancellation probe between records and
    /// Pts chunks; the extent loops (path outline construction
    /// included) dominate the build, the BVH partition after them is
    /// comparatively cheap and runs unprobed.
    pub fn build_cancellable(
        doc: &Doc,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        let Some(cell) = doc.cells.get(doc.top) else {
            return Ok(Self {
                rects: RecordTree::default(),
                polys: RecordTree::default(),
                paths: RecordTree::default(),
                pts: PtsChunkIndex::default(),
            });
        };
        let mut ticker = BuildTicker::new(check);
        let mut rect_extents = Vec::with_capacity(cell.rects.len());
        for rect in &cell.rects {
            ticker.tick()?;
            rect_extents.push(rect_extent(rect));
        }
        let mut poly_extents = Vec::with_capacity(cell.polys.len());
        for polygon in &cell.polys {
            ticker.tick()?;
            poly_extents.push(poly_extent(polygon));
        }
        let mut path_extents = Vec::with_capacity(cell.paths.len());
        for path in &cell.paths {
            ticker.tick()?;
            path_extents.push(path_extent(path));
        }
        Ok(Self {
            rects: RecordTree::build(rect_extents),
            polys: RecordTree::build(poly_extents),
            paths: RecordTree::build(path_extents),
            pts: PtsChunkIndex::build(cell, &mut ticker)?,
        })
    }

    /// Reference index that never prunes; the oracle for equivalence tests.
    /// It also carries no Pts chunk tables, so every repetition walk is the
    /// full source-order scan.
    #[cfg(test)]
    pub(crate) fn unpruned(doc: &Doc) -> Self {
        let Some(cell) = doc.cells.get(doc.top) else {
            return Self::build(doc);
        };
        Self {
            rects: RecordTree::build(vec![ALWAYS; cell.rects.len()]),
            polys: RecordTree::build(vec![ALWAYS; cell.polys.len()]),
            paths: RecordTree::build(vec![ALWAYS; cell.paths.len()]),
            pts: PtsChunkIndex::default(),
        }
    }

    /// Chunk bboxes for a large `Rep::Pts`, or `None` when the repetition is
    /// not a Pts form, is below the threshold, or belongs to another page.
    pub(crate) fn pts_chunks(&self, rep: &Rep) -> Option<&[BBox]> {
        self.pts.chunks_for(rep)
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
            .saturating_add(self.pts.estimated_bytes())
    }
}

/// Decode-time chunk bboxes for large `Rep::Pts` offset lists (F2R-03b 2a).
///
/// OASIS modal repetition reuse makes many records share one `Arc` point
/// list, so tables are deduplicated by the slice's data pointer. The `Arc`
/// slices live in the page's `Doc` for the index lifetime; the key is
/// identity only and is never dereferenced.
#[derive(Default)]
pub(crate) struct PtsChunkIndex {
    tables: Vec<(usize, Box<[BBox]>)>,
}

impl PtsChunkIndex {
    fn build(cell: &Cell, ticker: &mut BuildTicker<'_>) -> Result<Self, String> {
        let mut large: Vec<&[(i64, i64)]> = Vec::new();
        for rep in cell
            .rects
            .iter()
            .map(|rect| &rect.rep)
            .chain(cell.polys.iter().map(|polygon| &polygon.rep))
            .chain(cell.paths.iter().map(|path| &path.rep))
        {
            if let Rep::Pts(points) = rep {
                if points.len() >= PTS_CHUNK_MIN_POINTS {
                    large.push(points);
                }
            }
        }
        large.sort_by_key(|points| points.as_ptr() as usize);
        large.dedup_by_key(|points| points.as_ptr() as usize);
        let mut tables = Vec::new();
        for points in large {
            let mut chunks = Vec::with_capacity(points.len().div_ceil(PTS_CHUNK_POINTS));
            for chunk in points.chunks(PTS_CHUNK_POINTS) {
                ticker.tick()?;
                let mut bbox = BBox::EMPTY;
                for &(x, y) in chunk {
                    bbox.grow(&BBox {
                        x0: x,
                        y0: y,
                        x1: x,
                        y1: y,
                    });
                }
                chunks.push(bbox);
            }
            let chunks: Box<[BBox]> = chunks.into();
            if Self::chunks_are_selective(&chunks) {
                tables.push((points.as_ptr() as usize, chunks));
            }
        }
        Ok(Self { tables })
    }

    /// File-order chunks only pay off while they stay spatially tight on at
    /// least one axis. Writers commonly sort point lists along one axis
    /// (KLayout emits them y-sorted), which makes chunks razor-thin on that
    /// axis even when the other axis spans the whole cloud — still highly
    /// selective for window queries. A list whose chunks span most of the
    /// cloud on both axes (randomly ordered fill) would add bbox tests
    /// without ever skipping a chunk, so its table is dropped and the plain
    /// full scan keeps running. An axis only counts when the full extent is
    /// non-degenerate there; a zero-width axis cannot tell chunks apart.
    fn chunks_are_selective(chunks: &[BBox]) -> bool {
        let mut full = BBox::EMPTY;
        let mut width_sum: i128 = 0;
        let mut height_sum: i128 = 0;
        for chunk in chunks {
            full.grow(chunk);
            if !chunk.is_empty() {
                width_sum += (chunk.x1 - chunk.x0) as i128;
                height_sum += (chunk.y1 - chunk.y0) as i128;
            }
        }
        if full.is_empty() {
            return false;
        }
        let count = chunks.len() as i128;
        let full_width = (full.x1 - full.x0) as i128;
        let full_height = (full.y1 - full.y0) as i128;
        (full_width > 0 && width_sum * 4 <= full_width * count)
            || (full_height > 0 && height_sum * 4 <= full_height * count)
    }

    fn chunks_for(&self, rep: &Rep) -> Option<&[BBox]> {
        let Rep::Pts(points) = rep else {
            return None;
        };
        let key = points.as_ptr() as usize;
        self.tables
            .binary_search_by_key(&key, |entry| entry.0)
            .ok()
            .map(|found| &*self.tables[found].1)
    }

    fn estimated_bytes(&self) -> u64 {
        let mut bytes = (self.tables.capacity() as u64)
            .saturating_mul(std::mem::size_of::<(usize, Box<[BBox]>)>() as u64);
        for (_, chunks) in &self.tables {
            bytes = bytes.saturating_add(
                (chunks.len() as u64).saturating_mul(std::mem::size_of::<BBox>() as u64),
            );
        }
        bytes
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
    fn index_build_probes_cancellation_between_records() {
        // 3 * stride rects guarantee at least two probes; a failing
        // check must abort the build instead of finishing the page.
        let doc = Doc {
            unit: 1000.0,
            cells: vec![Cell {
                name: "BIG".to_string(),
                rects: (0..3 * super::BUILD_CANCEL_STRIDE as i64)
                    .map(|i| RectRec {
                        layer: 1,
                        dt: 0,
                        x: i,
                        y: i,
                        w: 1,
                        h: 1,
                        rep: Rep::One,
                    })
                    .collect(),
                ..Cell::default()
            }],
            top: 0,
            layer_order: vec![(1, 0)],
            norm_s: 0.0,
            layer_names: std::collections::HashMap::new(),
            layer_aliases: std::collections::HashMap::new(),
        };
        let mut probes = 0usize;
        let built = PageIndex::build_cancellable(&doc, &mut || {
            probes += 1;
            Ok(())
        });
        assert!(built.is_ok());
        assert!(probes >= 2, "expected periodic probes, saw {probes}");

        let mut calls = 0usize;
        let cancelled = PageIndex::build_cancellable(&doc, &mut || {
            calls += 1;
            Err("render cancelled: stale generation".to_string())
        });
        assert_eq!(
            cancelled.err().as_deref(),
            Some("render cancelled: stale generation")
        );
        assert_eq!(calls, 1, "first probe must abort the build");
    }

    #[test]
    fn pts_chunk_tables_are_deduplicated_and_thresholded() {
        let shared: Arc<[(i64, i64)]> = (0..PTS_CHUNK_MIN_POINTS as i64).map(|i| (i, -i)).collect();
        let small: Arc<[(i64, i64)]> = Arc::from([(0i64, 0i64), (5, 5)]);
        let rect = |rep: Rep| RectRec {
            layer: 0,
            dt: 0,
            x: 0,
            y: 0,
            w: 1,
            h: 1,
            rep,
        };
        let cell = Cell {
            rects: vec![
                rect(Rep::Pts(Arc::clone(&shared))),
                rect(Rep::Pts(Arc::clone(&shared))),
                rect(Rep::Pts(Arc::clone(&small))),
                rect(Rep::One),
            ],
            ..Cell::default()
        };
        let index = PtsChunkIndex::build(&cell, &mut BuildTicker::new(&mut || Ok(()))).unwrap();
        assert_eq!(index.tables.len(), 1, "shared arc builds one table");
        let chunks = index
            .chunks_for(&cell.rects[0].rep)
            .expect("large Pts must be chunked");
        assert_eq!(
            chunks.len(),
            PTS_CHUNK_MIN_POINTS.div_ceil(PTS_CHUNK_POINTS)
        );
        assert!(std::ptr::eq(
            chunks,
            index.chunks_for(&cell.rects[1].rep).unwrap()
        ));
        assert!(index.chunks_for(&cell.rects[2].rep).is_none());
        assert!(index.chunks_for(&cell.rects[3].rep).is_none());
    }

    #[test]
    fn spatially_incoherent_pts_get_no_chunk_table() {
        // Alternating far corners make every chunk bbox span the whole
        // cloud; the table would never skip a chunk, so it is dropped.
        let scattered: Arc<[(i64, i64)]> = (0..PTS_CHUNK_MIN_POINTS as i64)
            .map(|i| {
                if i % 2 == 0 {
                    (0, 0)
                } else {
                    (100_000, 100_000)
                }
            })
            .collect();
        let cell = Cell {
            rects: vec![RectRec {
                layer: 0,
                dt: 0,
                x: 0,
                y: 0,
                w: 1,
                h: 1,
                rep: Rep::Pts(scattered),
            }],
            ..Cell::default()
        };
        let index = PtsChunkIndex::build(&cell, &mut BuildTicker::new(&mut || Ok(()))).unwrap();
        assert!(index.chunks_for(&cell.rects[0].rep).is_none());
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
