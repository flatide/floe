//! Hierarchy summary (design.ovh) - the cell tree's index
//! (docs/SPEC-FORMATS.ko.md "design.ovh", docs/SPEC-VIEWER.ko.md §8c).
//!
//! The viewer's cell tree needs, per cell, its DISTINCT children with
//! the number of placed members and where those placements lie, its
//! parents, and how many instances of it the top cell holds. design.ovm
//! stores placements as one record per OASIS PLACEMENT in BVH leaf
//! order (grouped by parent, not by child): listing the children of a
//! cell means reading its whole placement range - 82 M records (5.3 GB)
//! on the 1/10 MAIN01 stand-in, ~800 M on MAIN01 itself - and finding a
//! cell's parents means reading them all. Neither can run when a tree
//! node opens, so both are summarized once, into this file, next to the
//! cache: by the indexer at the end of `floe-index vfs`, or later by
//! `floe-index hier <cache>` (`floe2 index --hier-only`) for a cache
//! built before it existed. A small cache (HIER_INLINE_PLACES records or
//! fewer) is summarized in memory by the daemon instead.
//!
//! File (little-endian), version 1:
//!   header 64 B: magic "FLOEOVH1", version u32 @8, n_cells u32 @12,
//!   n_places u64 @16, src_size u64 @24, src_mtime u64 @32, top u32 @40,
//!   reserved u32 @44, n_edges u64 @48, reserved u64 @56;
//!   kids_start: (n_cells + 1) x u64 - edge index range of each parent;
//!   edges: n_edges x EDGE_LEN - child u32, reserved u32, members u64
//!     (the placed members of that child in this parent, every
//!     repetition expanded, u64-saturating), extent 4 x i64 (the union
//!     of the child's recursive bbox under every placement of it in the
//!     parent, parent coordinates; EMPTY for a child without shapes);
//!     the edges of one parent ascend by child;
//!   parents_start: (n_cells + 1) x u64 - parent index range of each child;
//!   parents: n_edges x u32 - the parents of each child, ascending;
//!   insts: n_cells x u64 - how many instances of each cell the top cell
//!     holds (the top itself: 1; a cell not placed under the top: 0),
//!     every repetition expanded along every path, u64-saturating.
//! The identity (n_cells, n_places, src_size, src_mtime, top) must match
//! the cache's design.ovm; a file that fails any check reads as "no
//! summary" and the cell tree says how to build it.

use floe_ovm::{BBox, Ovm, PlaceHead};
use std::collections::HashMap;

pub const MAGIC: &[u8; 8] = b"FLOEOVH1";
pub const VERSION: u32 = 1;
pub const HEADER_LEN: usize = 64;
pub const EDGE_LEN: usize = 48;
/// A cache with at most this many placement records is summarized in
/// memory by the daemon when design.ovh is absent (256 MB of records:
/// a few hundred ms); above it the file is required.
pub const HIER_INLINE_PLACES: u64 = 4_000_000;

/// One parent -> child edge of the summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub child: u32,
    pub members: u64,
    /// union of the child's recursive bbox under every placement of it
    /// in the parent (parent coordinates); EMPTY when the child has no
    /// shapes
    pub extent: BBox,
}

fn g32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn g64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn gi64(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn gbox(b: &[u8], o: usize) -> BBox {
    BBox {
        x0: gi64(b, o),
        y0: gi64(b, o + 8),
        x1: gi64(b, o + 16),
        y1: gi64(b, o + 24),
    }
}
fn p32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn p64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn pi64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn pbox(out: &mut Vec<u8>, b: &BBox) {
    pi64(out, b.x0);
    pi64(out, b.y0);
    pi64(out, b.x1);
    pi64(out, b.y1);
}

/// The placed members of one record: 1, na x nb, or the pts count.
pub fn record_members(head: &PlaceHead) -> u64 {
    match head.kind {
        0 => 1,
        1 => (head.na as u64).saturating_mul(head.nb as u64),
        _ => head.na as u64,
    }
}

/// The bbox of every member of one record in the parent's coordinates:
/// the child's recursive bbox under the placement transform, grown by
/// the repetition's offsets (a Grid's four corner offsets, a Pts pool
/// entry's extent). EMPTY when the child has no shapes.
pub fn record_extent(ovm: &Ovm, pli: u64, head: &PlaceHead, child_rbbox: &BBox) -> BBox {
    if child_rbbox.is_empty() {
        return BBox::EMPTY;
    }
    let xf = floe_tiler::Xf::place(head.x, head.y, head.rot, head.flip);
    let mut base = BBox::EMPTY;
    for (x, y) in [
        (child_rbbox.x0, child_rbbox.y0),
        (child_rbbox.x0, child_rbbox.y1),
        (child_rbbox.x1, child_rbbox.y0),
        (child_rbbox.x1, child_rbbox.y1),
    ] {
        let (tx, ty) = xf.apply(x, y);
        base.grow(&BBox { x0: tx, y0: ty, x1: tx, y1: ty });
    }
    let (ox, oy) = match head.kind {
        0 => ((0, 0), (0, 0)),
        1 => {
            let a = head.na as i64 - 1;
            let b = head.nb as i64 - 1;
            let corners = [
                (0i64, 0i64),
                (a * head.va.0, a * head.va.1),
                (b * head.vb.0, b * head.vb.1),
                (a * head.va.0 + b * head.vb.0, a * head.va.1 + b * head.vb.1),
            ];
            let xs = corners.iter().map(|c| c.0);
            let ys = corners.iter().map(|c| c.1);
            (
                (xs.clone().min().unwrap(), xs.max().unwrap()),
                (ys.clone().min().unwrap(), ys.max().unwrap()),
            )
        }
        _ => match ovm.pts_ref(pli) {
            Some(pts) => {
                let e = pts.extent();
                ((e.x0.min(0), e.x1.max(0)), (e.y0.min(0), e.y1.max(0)))
            }
            None => ((0, 0), (0, 0)),
        },
    };
    BBox {
        x0: base.x0.saturating_add(ox.0),
        y0: base.y0.saturating_add(oy.0),
        x1: base.x1.saturating_add(ox.1),
        y1: base.y1.saturating_add(oy.1),
    }
}

/// Build statistics for the log line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildStats {
    pub cells: u32,
    pub records: u64,
    pub edges: u64,
    /// cells not placed under the top (insts 0, the top excluded)
    pub unplaced: u32,
}

/// Summarize a cache's hierarchy: one sequential pass over its
/// placement records. Returns the file bytes and the statistics.
pub fn build(ovm: &Ovm) -> (Vec<u8>, BuildStats) {
    let n = ovm.n_cells as usize;
    let mut kids_start: Vec<u64> = Vec::with_capacity(n + 1);
    // (child, members, extent) per parent, children ascending
    let mut edges: Vec<(u32, u64, BBox)> = Vec::new();
    let mut per_parent: HashMap<u32, (u64, BBox)> = HashMap::new();
    let mut rbboxes: Vec<BBox> = Vec::with_capacity(n);
    for ci in 0..ovm.n_cells {
        rbboxes.push(ovm.cell_rbbox(ci));
    }
    let mut records = 0u64;
    for ci in 0..ovm.n_cells {
        kids_start.push(edges.len() as u64);
        let (start, count) = ovm.cell_places(ci);
        per_parent.clear();
        for k in 0..count as u64 {
            let pli = start as u64 + k;
            let head = ovm.place_head(pli);
            records += 1;
            let child_rbbox = rbboxes.get(head.child as usize).copied().unwrap_or(BBox::EMPTY);
            let extent = record_extent(ovm, pli, &head, &child_rbbox);
            let entry = per_parent.entry(head.child).or_insert((0, BBox::EMPTY));
            entry.0 = entry.0.saturating_add(record_members(&head));
            entry.1.grow(&extent);
        }
        let mut kids: Vec<(u32, u64, BBox)> = per_parent.iter().map(|(c, (m, e))| (*c, *m, *e)).collect();
        kids.sort_unstable_by_key(|k| k.0);
        edges.extend(kids);
    }
    kids_start.push(edges.len() as u64);
    // parents (CSR over the same edges, by child)
    let mut parent_count = vec![0u64; n + 1];
    for (parent, &start) in kids_start.iter().enumerate().take(n) {
        let end = kids_start[parent + 1];
        for e in &edges[start as usize..end as usize] {
            if (e.0 as usize) < n {
                parent_count[e.0 as usize + 1] += 1;
            }
        }
    }
    let mut parents_start = vec![0u64; n + 1];
    for i in 0..n {
        parents_start[i + 1] = parents_start[i] + parent_count[i + 1];
    }
    let mut parents = vec![u32::MAX; edges.len()];
    let mut fill = parents_start.clone();
    for parent in 0..n {
        let (start, end) = (kids_start[parent] as usize, kids_start[parent + 1] as usize);
        for e in &edges[start..end] {
            let c = e.0 as usize;
            if c < n {
                parents[fill[c] as usize] = parent as u32;
                fill[c] += 1;
            }
        }
    }
    // parents of a child ascend: the edges are visited by ascending parent
    // instances under the top: topological order by topo_rank (the
    // indexer guarantees a parent's rank below its children's)
    let mut order: Vec<u32> = (0..ovm.n_cells).collect();
    let mut ranks: Vec<u32> = Vec::with_capacity(n);
    for ci in 0..ovm.n_cells {
        ranks.push(ovm.cell(ci).topo_rank);
    }
    order.sort_unstable_by_key(|&ci| (ranks[ci as usize], ci));
    let mut insts = vec![0u64; n];
    if (ovm.top as usize) < n {
        insts[ovm.top as usize] = 1;
    }
    for &parent in &order {
        let mine = insts[parent as usize];
        if mine == 0 {
            continue;
        }
        let (start, end) = (kids_start[parent as usize] as usize, kids_start[parent as usize + 1] as usize);
        for e in &edges[start..end] {
            if (e.0 as usize) < n {
                let add = mine.saturating_mul(e.1);
                insts[e.0 as usize] = insts[e.0 as usize].saturating_add(add);
            }
        }
    }
    let unplaced = (0..n).filter(|&ci| insts[ci] == 0 && ci != ovm.top as usize).count() as u32;
    // encode
    let mut out: Vec<u8> = Vec::with_capacity(
        HEADER_LEN + (n + 1) * 16 + edges.len() * (EDGE_LEN + 4) + n * 8,
    );
    out.extend_from_slice(MAGIC);
    p32(&mut out, VERSION);
    p32(&mut out, ovm.n_cells);
    p64(&mut out, ovm.n_places);
    p64(&mut out, ovm.src_size);
    p64(&mut out, ovm.src_mtime);
    p32(&mut out, ovm.top);
    p32(&mut out, 0);
    p64(&mut out, edges.len() as u64);
    p64(&mut out, 0);
    debug_assert_eq!(out.len(), HEADER_LEN);
    for &s in &kids_start {
        p64(&mut out, s);
    }
    for (child, members, extent) in &edges {
        p32(&mut out, *child);
        p32(&mut out, 0);
        p64(&mut out, *members);
        pbox(&mut out, extent);
    }
    for &s in &parents_start {
        p64(&mut out, s);
    }
    for &p in &parents {
        p32(&mut out, p);
    }
    for &i in &insts {
        p64(&mut out, i);
    }
    (
        out,
        BuildStats {
            cells: ovm.n_cells,
            records,
            edges: edges.len() as u64,
            unplaced,
        },
    )
}

/// Build and publish `<dir>/design.ovh` by tmp + rename. Returns the
/// statistics and the file size.
pub fn write(ovm: &Ovm, dir: &str) -> Result<(BuildStats, u64), String> {
    use std::io::Write as _;
    let (bytes, stats) = build(ovm);
    let tmp = format!("{}/design.ovh.tmp", dir);
    let path = format!("{}/design.ovh", dir);
    let result = (|| -> Result<(), String> {
        let mut file = std::fs::File::create(&tmp).map_err(|e| format!("{}: {}", tmp, e))?;
        file.write_all(&bytes).map_err(|e| format!("{}: {}", tmp, e))?;
        file.sync_all().map_err(|e| format!("{}: {}", tmp, e))?;
        drop(file);
        std::fs::rename(&tmp, &path).map_err(|e| format!("{} -> {}: {}", tmp, path, e))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|()| (stats, bytes.len() as u64))
}

/// A hierarchy summary, read-only mapped or held in memory.
pub struct HierSummary {
    data: floe_ovm::Backing,
    pub n_cells: u32,
    pub n_places: u64,
    pub src_size: u64,
    pub src_mtime: u64,
    pub top: u32,
    pub n_edges: u64,
    kids_off: usize,
    edges_off: usize,
    parents_start_off: usize,
    parents_off: usize,
    insts_off: usize,
}

impl std::fmt::Debug for HierSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "HierSummary(cells={} edges={} top={})",
            self.n_cells, self.n_edges, self.top
        )
    }
}

impl HierSummary {
    pub fn open(path: &str) -> Result<HierSummary, String> {
        let data = floe_ovm::map_file(path)?;
        HierSummary::parse(data).map_err(|e| format!("{}: {}", path, e))
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<HierSummary, String> {
        HierSummary::parse(floe_ovm::Backing::Vec(bytes))
    }

    fn parse(data: floe_ovm::Backing) -> Result<HierSummary, String> {
        let b: &[u8] = &data;
        if b.len() < HEADER_LEN {
            return Err(format!("truncated hierarchy summary ({} bytes)", b.len()));
        }
        if &b[..8] != MAGIC {
            return Err("not a hierarchy summary (bad magic)".to_string());
        }
        let version = g32(b, 8);
        if version != VERSION {
            return Err(format!("hierarchy summary version {} (this build reads {})", version, VERSION));
        }
        let n_cells = g32(b, 12);
        let n_places = g64(b, 16);
        let src_size = g64(b, 24);
        let src_mtime = g64(b, 32);
        let top = g32(b, 40);
        let n_edges = g64(b, 48);
        let n = n_cells as usize;
        let ne = usize::try_from(n_edges).map_err(|_| "edge count overflows".to_string())?;
        let kids_off = HEADER_LEN;
        let edges_off = kids_off + (n + 1) * 8;
        let parents_start_off = edges_off + ne * EDGE_LEN;
        let parents_off = parents_start_off + (n + 1) * 8;
        let insts_off = parents_off + ne * 4;
        let total = insts_off + n * 8;
        if b.len() != total {
            return Err(format!(
                "hierarchy summary is {} bytes, {} expected for {} cells and {} edges",
                b.len(),
                total,
                n_cells,
                n_edges
            ));
        }
        let s = HierSummary {
            data,
            n_cells,
            n_places,
            src_size,
            src_mtime,
            top,
            n_edges,
            kids_off,
            edges_off,
            parents_start_off,
            parents_off,
            insts_off,
        };
        // the ranges must be monotone and end at n_edges; every edge
        // endpoint must be a cell
        let mut prev = 0u64;
        for ci in 0..=n {
            let v = s.kids_range_raw(ci);
            if v < prev || v > n_edges {
                return Err(format!("corrupt kids_start at {}", ci));
            }
            prev = v;
        }
        if prev != n_edges {
            return Err("kids_start does not end at n_edges".to_string());
        }
        prev = 0;
        for ci in 0..=n {
            let v = s.parents_range_raw(ci);
            if v < prev || v > n_edges {
                return Err(format!("corrupt parents_start at {}", ci));
            }
            prev = v;
        }
        if prev != n_edges {
            return Err("parents_start does not end at n_edges".to_string());
        }
        for e in 0..ne {
            if s.edge(e).child >= n_cells {
                return Err(format!("edge {} names cell {} of {}", e, s.edge(e).child, n_cells));
            }
            let p = g32(&s.data, s.parents_off + e * 4);
            if p >= n_cells {
                return Err(format!("parent entry {} names cell {} of {}", e, p, n_cells));
            }
        }
        if n > 0 && top >= n_cells {
            return Err(format!("top {} of {} cells", top, n_cells));
        }
        Ok(s)
    }

    /// The summary belongs to this cache: same cell and record counts,
    /// same source bytes, same top.
    pub fn validate_against(&self, ovm: &Ovm) -> Result<(), String> {
        if self.src_size != ovm.src_size || self.src_mtime != ovm.src_mtime {
            return Err(format!(
                "hierarchy summary was built for source {}/{}, cache has {}/{}",
                self.src_size, self.src_mtime, ovm.src_size, ovm.src_mtime
            ));
        }
        if self.n_cells != ovm.n_cells || self.n_places != ovm.n_places {
            return Err(format!(
                "hierarchy summary has {} cells / {} placements, cache {} / {}",
                self.n_cells, self.n_places, ovm.n_cells, ovm.n_places
            ));
        }
        if self.top != ovm.top {
            return Err(format!("hierarchy summary top {}, cache top {}", self.top, ovm.top));
        }
        Ok(())
    }

    fn kids_range_raw(&self, i: usize) -> u64 {
        g64(&self.data, self.kids_off + i * 8)
    }
    fn parents_range_raw(&self, i: usize) -> u64 {
        g64(&self.data, self.parents_start_off + i * 8)
    }

    fn edge(&self, e: usize) -> Edge {
        let o = self.edges_off + e * EDGE_LEN;
        Edge {
            child: g32(&self.data, o),
            members: g64(&self.data, o + 8),
            extent: gbox(&self.data, o + 16),
        }
    }

    /// Edge index range of a parent's children.
    fn kids_range(&self, ci: u32) -> (usize, usize) {
        if ci >= self.n_cells {
            return (0, 0);
        }
        (
            self.kids_range_raw(ci as usize) as usize,
            self.kids_range_raw(ci as usize + 1) as usize,
        )
    }

    /// The distinct children of a cell, ascending by child index.
    pub fn children(&self, ci: u32) -> impl Iterator<Item = Edge> + '_ {
        let (lo, hi) = self.kids_range(ci);
        (lo..hi).map(move |e| self.edge(e))
    }

    pub fn child_count(&self, ci: u32) -> usize {
        let (lo, hi) = self.kids_range(ci);
        hi - lo
    }

    pub fn has_children(&self, ci: u32) -> bool {
        self.child_count(ci) > 0
    }

    /// The edge parent -> child, if the parent places the child.
    pub fn edge_to(&self, parent: u32, child: u32) -> Option<Edge> {
        let (lo, hi) = self.kids_range(parent);
        let mut a = lo;
        let mut b = hi;
        while a < b {
            let mid = a + (b - a) / 2;
            let e = self.edge(mid);
            match e.child.cmp(&child) {
                std::cmp::Ordering::Less => a = mid + 1,
                std::cmp::Ordering::Greater => b = mid,
                std::cmp::Ordering::Equal => return Some(e),
            }
        }
        None
    }

    /// The distinct parents of a cell, ascending.
    pub fn parents(&self, ci: u32) -> impl Iterator<Item = u32> + '_ {
        let (lo, hi) = if ci >= self.n_cells {
            (0, 0)
        } else {
            (
                self.parents_range_raw(ci as usize) as usize,
                self.parents_range_raw(ci as usize + 1) as usize,
            )
        };
        (lo..hi).map(move |e| g32(&self.data, self.parents_off + e * 4))
    }

    pub fn parent_count(&self, ci: u32) -> usize {
        if ci >= self.n_cells {
            return 0;
        }
        (self.parents_range_raw(ci as usize + 1) - self.parents_range_raw(ci as usize)) as usize
    }

    /// Instances of the cell under the top (1 for the top itself, 0 for
    /// a cell the top does not reach).
    pub fn insts(&self, ci: u32) -> u64 {
        if ci >= self.n_cells {
            return 0;
        }
        g64(&self.data, self.insts_off + ci as usize * 8)
    }

    /// The cells whose subtree holds `ci` (ci included): a flag per
    /// cell. Walks the parents; a cell the top does not reach still
    /// lists its parents.
    pub fn ancestors(&self, ci: u32) -> Vec<bool> {
        let mut flags = vec![false; self.n_cells as usize];
        if ci >= self.n_cells {
            return flags;
        }
        let mut stack = vec![ci];
        flags[ci as usize] = true;
        while let Some(c) = stack.pop() {
            for p in self.parents(c) {
                if !flags[p as usize] {
                    flags[p as usize] = true;
                    stack.push(p);
                }
            }
        }
        flags
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }
}

#[doc(hidden)]
pub mod fixture {
    //! A small index from (name, placements) rows, for this module's tests
    //! and the render-core cell queries' tests (public so another crate's
    //! tests can build one; never used by a product path).
    use floe_oasis::doc::Rep;
    use floe_ovm::{BBox, Builder, Ovm, PBVH_NONE};

    /// `places`: (child, x, y, rot, flip, rep); children must come first
    /// (a child's index is below its parents'). `shape`: the cell's own
    /// shapes bbox (None = no shapes of its own).
    pub struct FCell {
        pub name: &'static str,
        pub shape: Option<BBox>,
        pub places: Vec<(usize, i64, i64, u8, bool, Rep)>,
    }

    pub fn place_bbox(rbb: &BBox, x: i64, y: i64, rot: u8, flip: bool, rep: &Rep) -> BBox {
        if rbb.is_empty() {
            return BBox::EMPTY;
        }
        let xf = floe_tiler::Xf::place(x, y, rot, flip);
        let mut base = BBox::EMPTY;
        for (px, py) in [(rbb.x0, rbb.y0), (rbb.x0, rbb.y1), (rbb.x1, rbb.y0), (rbb.x1, rbb.y1)] {
            let (tx, ty) = xf.apply(px, py);
            base.grow(&BBox { x0: tx, y0: ty, x1: tx, y1: ty });
        }
        let (ex, ey) = match rep {
            Rep::One => ((0, 0), (0, 0)),
            Rep::Grid { na, nb, va, vb } => {
                let a = *na as i64 - 1;
                let b = *nb as i64 - 1;
                let xs = [0, a * va.0, b * vb.0, a * va.0 + b * vb.0];
                let ys = [0, a * va.1, b * vb.1, a * va.1 + b * vb.1];
                (
                    (*xs.iter().min().unwrap(), *xs.iter().max().unwrap()),
                    (*ys.iter().min().unwrap(), *ys.iter().max().unwrap()),
                )
            }
            Rep::Pts(p) => {
                let xs = p.iter().map(|q| q.0);
                let ys = p.iter().map(|q| q.1);
                (
                    (xs.clone().min().unwrap_or(0).min(0), xs.max().unwrap_or(0).max(0)),
                    (ys.clone().min().unwrap_or(0).min(0), ys.max().unwrap_or(0).max(0)),
                )
            }
        };
        BBox {
            x0: base.x0 + ex.0,
            y0: base.y0 + ey.0,
            x1: base.x1 + ex.1,
            y1: base.y1 + ey.1,
        }
    }

    pub fn build(cells: &[FCell], top: usize) -> Ovm {
        let n = cells.len();
        let mut height = vec![0u32; n];
        let mut rbb = vec![BBox::EMPTY; n];
        for ci in 0..n {
            let mut b = BBox::EMPTY;
            if let Some(s) = &cells[ci].shape {
                b.grow(s);
            }
            for (c, x, y, rot, flip, rep) in &cells[ci].places {
                assert!(*c < ci, "fixture must be children-first");
                height[ci] = height[ci].max(height[*c] + 1);
                b.grow(&place_bbox(&rbb[*c], *x, *y, *rot, *flip, rep));
            }
            rbb[ci] = b;
        }
        let mut b = Builder::new(1000.0, 77, 88, 1);
        b.top = top as u32;
        b.layer(1, 0, "L1", 0, 0);
        let mask = b.bitset(&[1]);
        for ci in 0..n {
            let place_base = b.n_places() as u32;
            let mut items = BBox::EMPTY;
            for (c, x, y, rot, flip, rep) in &cells[ci].places {
                b.place(*c as u32, *x, *y, *rot, *flip, rep);
                items.grow(&place_bbox(&rbb[*c], *x, *y, *rot, *flip, rep));
            }
            let (bvh_start, bvh_count) = if cells[ci].places.is_empty() {
                (0, 0)
            } else {
                assert!(cells[ci].places.len() <= 8, "one-leaf bvh cap");
                (
                    b.bvh_node(&items, place_base, cells[ci].places.len() as u16, true, u32::MAX, u32::MAX),
                    1,
                )
            };
            let page_start = b.n_pages();
            if let Some(s) = &cells[ci].shape {
                b.page(ci as u32, 0, 0, s, 0, 0, 0, 1, 1, 1, 1, floe_ovm::LOD_EXACT, floe_ovm::LOD_PAGE_NONE);
            }
            let page_count = b.n_pages() - page_start;
            let (pr_start, pr_count) = if page_count > 0 {
                (b.prange(0, page_start, page_count, PBVH_NONE), 1u32)
            } else {
                (b.n_pranges(), 0)
            };
            b.cell(
                cells[ci].name,
                height[ci],
                (n - 1 - ci) as u32,
                &rbb[ci],
                &rbb[ci],
                place_base,
                cells[ci].places.len() as u32,
                page_start,
                page_count,
                bvh_start,
                bvh_count,
                pr_start,
                pr_count,
                mask,
                mask,
                1,
                0,
                0,
                mask,
            );
        }
        Ovm::from_bytes(b.finish(0, 0)).unwrap()
    }

    pub fn shape(x0: i64, y0: i64, x1: i64, y1: i64) -> Option<BBox> {
        Some(BBox { x0, y0, x1, y1 })
    }

    /// LEAF (0..10 x 0..4) placed 3 times in MID (once directly, a 2x3
    /// grid), MID twice in TOP (one rotated), LEAF once more in TOP as a
    /// pts repetition, an ORPHAN nobody places, and an EMPTY cell without
    /// shapes placed in TOP.
    pub fn sample() -> Ovm {
        build(
            &[
                FCell { name: "LEAF", shape: shape(0, 0, 10, 4), places: vec![] },
                FCell { name: "EMPTY", shape: None, places: vec![] },
                FCell {
                    name: "MID",
                    shape: shape(0, 0, 100, 100),
                    places: vec![
                        (0, 5, 5, 0, false, Rep::One),
                        (0, 20, 50, 0, false, Rep::Grid { na: 2, nb: 3, va: (30, 0), vb: (0, 10) }),
                    ],
                },
                FCell { name: "ORPHAN", shape: shape(0, 0, 1, 1), places: vec![] },
                FCell {
                    name: "TOP",
                    shape: shape(0, 0, 1000, 1000),
                    places: vec![
                        (2, 0, 0, 0, false, Rep::One),
                        (2, 500, 100, 1, false, Rep::One),
                        (0, 800, 800, 0, true, Rep::Pts(vec![(0, 0), (50, 0), (0, 50)].into())),
                        (1, 10, 10, 0, false, Rep::One),
                    ],
                },
            ],
            4,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::sample;
    use super::*;

    fn summary(ovm: &Ovm) -> HierSummary {
        let (bytes, _) = build(ovm);
        HierSummary::from_bytes(bytes).unwrap()
    }

    #[test]
    fn children_carry_members_and_extents_and_parents_mirror_them() {
        let ovm = sample();
        let (bytes, stats) = build(&ovm);
        assert_eq!(stats, BuildStats { cells: 5, records: 6, edges: 4, unplaced: 1 });
        let s = HierSummary::from_bytes(bytes).unwrap();
        s.validate_against(&ovm).unwrap();
        // TOP (4): EMPTY (1) x1, MID (2) x2, LEAF (0) x3 - ascending by child
        let kids: Vec<(u32, u64)> = s.children(4).map(|e| (e.child, e.members)).collect();
        assert_eq!(kids, vec![(0, 3), (1, 1), (2, 2)]);
        // MID (2): LEAF x 1 + 2*3
        let kids: Vec<(u32, u64)> = s.children(2).map(|e| (e.child, e.members)).collect();
        assert_eq!(kids, vec![(0, 7)]);
        assert!(!s.has_children(0));
        assert!(!s.has_children(3));
        // the LEAF grid in MID: 2 columns 30 apart, 3 rows 10 apart, from (20, 50)
        let grid = s.edge_to(2, 0).unwrap().extent;
        assert_eq!(grid, BBox { x0: 5, y0: 5, x1: 60, y1: 74 });
        // MID in TOP: identity at 0,0 and a 90-degree turn at (500, 100):
        // x' = 500 - y, y' = 100 + x -> x in [400, 500], y in [100, 200]
        assert_eq!(s.edge_to(4, 2).unwrap().extent, BBox { x0: 0, y0: 0, x1: 500, y1: 200 });
        // the flipped LEAF pts placement in TOP: flip mirrors y (y -> -y),
        // offsets reach 50 to the right and up
        assert_eq!(s.edge_to(4, 0).unwrap().extent, BBox { x0: 800, y0: 796, x1: 860, y1: 850 });
        // EMPTY has no shapes: extent EMPTY
        assert!(s.edge_to(4, 1).unwrap().extent.is_empty());
        assert!(s.edge_to(4, 3).is_none());
        // parents
        assert_eq!(s.parents(0).collect::<Vec<_>>(), vec![2, 4]);
        assert_eq!(s.parents(2).collect::<Vec<_>>(), vec![4]);
        assert_eq!(s.parents(4).count(), 0);
        assert_eq!(s.parents(3).count(), 0);
        // instances under TOP: LEAF = 2 MIDs x 7 + 3 = 17
        assert_eq!(s.insts(4), 1);
        assert_eq!(s.insts(2), 2);
        assert_eq!(s.insts(0), 17);
        assert_eq!(s.insts(1), 1);
        assert_eq!(s.insts(3), 0);
        // ancestors of LEAF: MID and TOP (and itself)
        assert_eq!(s.ancestors(0), vec![true, false, true, false, true]);
        assert_eq!(s.ancestors(3), vec![false, false, false, true, false]);
    }

    #[test]
    fn the_file_is_bound_to_its_cache_and_refuses_damage() {
        let ovm = sample();
        let s = summary(&ovm);
        let other = super::fixture::build(
            &[super::fixture::FCell { name: "ONLY", shape: super::fixture::shape(0, 0, 1, 1), places: vec![] }],
            0,
        );
        assert!(s.validate_against(&other).is_err());
        let bytes = s.bytes().to_vec();
        assert!(HierSummary::from_bytes(bytes[..bytes.len() - 1].to_vec()).is_err());
        let mut bad = bytes.clone();
        bad[8] = 9; // version
        assert!(HierSummary::from_bytes(bad).is_err());
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(HierSummary::from_bytes(bad).is_err());
        // an edge naming a cell outside the table
        let mut bad = bytes.clone();
        let edges_off = HEADER_LEN + (5 + 1) * 8;
        bad[edges_off..edges_off + 4].copy_from_slice(&99u32.to_le_bytes());
        assert!(HierSummary::from_bytes(bad).is_err());
    }

    #[test]
    fn write_publishes_by_rename_and_reads_back() {
        let ovm = sample();
        let dir = std::env::temp_dir().join(format!("floe-hiersum-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir_s = dir.to_str().unwrap();
        let (stats, size) = write(&ovm, dir_s).unwrap();
        assert_eq!(stats.edges, 4);
        let path = dir.join("design.ovh");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), size);
        assert!(!dir.join("design.ovh.tmp").exists());
        let s = HierSummary::open(path.to_str().unwrap()).unwrap();
        s.validate_against(&ovm).unwrap();
        assert_eq!(s.insts(0), 17);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn members_saturate_instead_of_wrapping() {
        let head = PlaceHead {
            child: 0,
            rot: 0,
            flip: false,
            kind: 1,
            x: 0,
            y: 0,
            na: u32::MAX,
            nb: u32::MAX,
            va: (1, 0),
            vb: (0, 1),
        };
        assert_eq!(record_members(&head), u32::MAX as u64 * u32::MAX as u64);
        let one = PlaceHead { kind: 0, ..head };
        assert_eq!(record_members(&one), 1);
        let pts = PlaceHead { kind: 2, na: 7, ..head };
        assert_eq!(record_members(&pts), 7);
    }
}
