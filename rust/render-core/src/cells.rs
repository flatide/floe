//! Cell tree queries (docs/SPEC-VIEWER.ko.md §8c): a cell's children
//! with their placed member counts, cell-name search, the extent of a
//! cell's instances under the top and the instances' boxes inside a
//! view - answered from the hierarchy summary design.ovh
//! (floe_vfs::hiersum) and, for the boxes, the index's placement BVH.
//!
//! The summary is opened on first use and re-checked by size and mtime
//! whenever a query asks (the viewer may build it, through
//! `floe-index hier`, while the daemon is up); a cache small enough
//! (HIER_INLINE_PLACES records or fewer) is summarized in memory when
//! the file is absent. Without a summary every query answers
//! `HierError::NoSummary`, and the viewer offers to build it.

use crate::transform::OrthoTransform;
use floe_ovm::{BBox, Ovm, PlaceHead};
use floe_vfs::hiersum::{HierSummary, HIER_INLINE_PLACES};
use floe_vfs::Vfs;
use std::sync::{Arc, Mutex, OnceLock};

/// Why a query could not be answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HierError {
    /// design.ovh is absent (or does not belong to this cache) and the
    /// cache is too large to summarize in memory: build it with
    /// `floe-index hier <cache>` (`floe2 index --hier-only <src>`)
    NoSummary(String),
    Other(String),
}

impl HierError {
    pub fn message(&self) -> &str {
        match self {
            HierError::NoSummary(m) | HierError::Other(m) => m,
        }
    }
}

impl From<String> for HierError {
    fn from(message: String) -> Self {
        HierError::Other(message)
    }
}

#[derive(Default)]
struct HierSlot {
    stat: Option<(u64, u64)>,
    summary: Option<Arc<HierSummary>>,
    error: Option<String>,
}

/// One cache's hierarchy: the index and its summary, shareable with the
/// daemon's query thread (the render worker owns the cache itself).
pub struct HierHandle {
    /// the index (crate::light::LightLink: a light cache's image grows as
    /// cells are visited; the walks here fill what they expand, as the plans do)
    link: crate::light::LightLink,
    dir: String,
    slot: Mutex<HierSlot>,
    names: OnceLock<Arc<Vec<String>>>,
}

/// The number of placement records a cache may hold for the daemon to
/// summarize it in memory when design.ovh is absent
/// (FLOE_RUST_HIER_INLINE_PLACES, diagnostic; default HIER_INLINE_PLACES).
fn inline_places_limit() -> u64 {
    static LIMIT: OnceLock<u64> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var("FLOE_RUST_HIER_INLINE_PLACES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(HIER_INLINE_PLACES)
    })
}

impl HierHandle {
    pub fn new(vfs: Arc<Vfs>, dir: String) -> Self {
        Self::with_link(crate::light::LightLink::full(vfs), dir)
    }

    pub(crate) fn with_link(link: crate::light::LightLink, dir: String) -> Self {
        Self {
            link,
            dir,
            slot: Mutex::new(HierSlot::default()),
            names: OnceLock::new(),
        }
    }

    /// The index as it is now (a light cache's grows; a query takes one and
    /// reads it to the end).
    pub fn vfs(&self) -> Arc<Vfs> {
        self.link.vfs()
    }

    pub fn unit(&self) -> f64 {
        self.link.vfs().ovm.unit
    }

    pub fn dir(&self) -> &str {
        &self.dir
    }

    /// Every cell's name, read once.
    pub fn names(&self) -> Arc<Vec<String>> {
        Arc::clone(self.names.get_or_init(|| {
            let vfs = self.link.vfs();
            let ovm = &vfs.ovm;
            Arc::new((0..ovm.n_cells).map(|ci| ovm.cell(ci).name).collect())
        }))
    }

    /// The summary: the file next to the cache when it is there and
    /// belongs to it, else an in-memory build for a small cache.
    pub fn summary(&self) -> Result<Arc<HierSummary>, HierError> {
        // a light cache's: from the inventory's edges, made at open (the
        // image's placements are the visited cells' alone; no file is read)
        if let Some(light) = &self.link.light {
            return Ok(Arc::clone(&light.summary));
        }
        let path = format!("{}/design.ovh", self.dir);
        let stat = std::fs::metadata(&path).ok().map(|m| {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() * 1_000_000_000 + d.subsec_nanos() as u64)
                .unwrap_or(0);
            (m.len(), mtime)
        });
        let mut slot = match self.slot.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        let vfs = self.link.vfs();
        let ovm = &vfs.ovm;
        // an in-memory summary (stat None) stands until a file appears
        if slot.summary.is_some() && (slot.stat == stat || slot.stat.is_none() && stat.is_none()) {
            return Ok(Arc::clone(slot.summary.as_ref().unwrap()));
        }
        if slot.stat == stat && slot.error.is_some() {
            return Err(HierError::NoSummary(slot.error.clone().unwrap()));
        }
        slot.stat = stat;
        slot.summary = None;
        slot.error = None;
        match stat {
            Some(_) => match HierSummary::open(&path) {
                Ok(file) => match file.validate_against(ovm) {
                    Ok(()) => slot.summary = Some(Arc::new(file)),
                    Err(e) => slot.error = Some(e),
                },
                Err(e) => slot.error = Some(e),
            },
            None => {
                if ovm.n_places <= inline_places_limit() {
                    let (bytes, _) = floe_vfs::hiersum::build(ovm);
                    match HierSummary::from_bytes(bytes) {
                        Ok(s) => slot.summary = Some(Arc::new(s)),
                        Err(e) => slot.error = Some(e),
                    }
                } else {
                    slot.error = Some(format!(
                        "no hierarchy summary ({}): the cache holds {} placement records; build it with floe-index hier",
                        path, ovm.n_places
                    ));
                }
            }
        }
        if let Some(e) = &slot.error {
            if stat.is_some() {
                eprintln!("[render-core] hierarchy summary {}: none ({})", path, e);
            }
        }
        match &slot.summary {
            Some(s) => Ok(Arc::clone(s)),
            None => Err(HierError::NoSummary(slot.error.clone().unwrap_or_default())),
        }
    }
}

/// One child row of a cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildRow {
    pub cell: u32,
    pub name: String,
    /// placed members of the child in the parent (repetitions expanded)
    pub members: u64,
    /// the child places nothing itself
    pub leaf: bool,
}

/// A cell and its children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellChildren {
    pub cell: u32,
    pub name: String,
    /// instances of the cell under the top (0 = not placed under it)
    pub insts: u64,
    pub height: u32,
    /// the cell's recursive bbox (cell coordinates)
    pub rbbox: BBox,
    /// distinct children, by name (case-insensitively), at most `cap`
    pub rows: Vec<ChildRow>,
    /// distinct children in all
    pub total: usize,
}

fn name_key(name: &str) -> String {
    name.to_lowercase()
}

/// A cell's children, by name. `cell` None = the top cell.
pub fn children(h: &HierHandle, cell: Option<u32>, cap: usize) -> Result<CellChildren, HierError> {
    let s = h.summary()?;
    let vfs = h.vfs();
    let ovm = &vfs.ovm;
    let ci = cell.unwrap_or(ovm.top);
    if ci >= ovm.n_cells {
        return Err(HierError::Other(format!("cell index {} out of range 0..{}", ci, ovm.n_cells)));
    }
    let names = h.names();
    let record = ovm.cell(ci);
    let mut rows: Vec<ChildRow> = s
        .children(ci)
        .map(|e| ChildRow {
            cell: e.child,
            name: names.get(e.child as usize).cloned().unwrap_or_default(),
            members: e.members,
            leaf: !s.has_children(e.child),
        })
        .collect();
    let total = rows.len();
    rows.sort_by_cached_key(|r| (name_key(&r.name), r.name.clone(), r.cell));
    rows.truncate(cap);
    Ok(CellChildren {
        cell: ci,
        name: record.name,
        insts: s.insts(ci),
        height: record.height,
        rbbox: record.rbbox,
        rows,
        total,
    })
}

/// One name-search match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindRow {
    pub cell: u32,
    pub name: String,
    pub insts: u64,
}

/// The matches of a name search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindResult {
    /// matches in all
    pub total: usize,
    /// by name (case-insensitively), at most `limit`
    pub rows: Vec<FindRow>,
}

/// `*` any run, `?` one character; both sides already lower-cased.
fn glob_match(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if let Some((sp, st)) = star {
            p = sp + 1;
            t = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

/// Whether a (lower-cased) name matches a (lower-cased) pattern: a
/// pattern with `*` or `?` is a whole-name glob, any other a substring;
/// the empty pattern matches every name.
pub fn name_matches(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    if pattern.contains('*') || pattern.contains('?') {
        let p: Vec<char> = pattern.chars().collect();
        let t: Vec<char> = name.chars().collect();
        glob_match(&p, &t)
    } else {
        name.contains(pattern)
    }
}

/// Cells whose name matches (case-insensitively), by name.
pub fn find(h: &HierHandle, pattern: &str, limit: usize) -> Result<FindResult, HierError> {
    let s = h.summary()?;
    let names = h.names();
    let pattern = pattern.to_lowercase();
    let mut rows: Vec<FindRow> = names
        .iter()
        .enumerate()
        .filter(|(_, name)| name_matches(&pattern, &name.to_lowercase()))
        .map(|(ci, name)| FindRow {
            cell: ci as u32,
            name: name.clone(),
            insts: s.insts(ci as u32),
        })
        .collect();
    let total = rows.len();
    rows.sort_by_cached_key(|r| (name_key(&r.name), r.name.clone(), r.cell));
    rows.truncate(limit);
    Ok(FindResult { total, rows })
}

/// Where a cell's instances lie under the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellExtent {
    pub insts: u64,
    /// the union of the cell's instances' boxes (top coordinates); None
    /// when the top does not reach the cell or it has no shapes
    pub bbox: Option<BBox>,
    /// the box is the union of the top-level placements holding the cell
    /// (the summary keeps one extent per parent -> child edge, so an
    /// instance deeper than one level is located by its container)
    pub approx: bool,
}

/// The root a query walks from: the given cell, else the top.
fn root_of(ovm: &Ovm, root: Option<u32>) -> Result<u32, HierError> {
    match root {
        Some(r) if r >= ovm.n_cells => Err(HierError::Other(format!(
            "root index {} out of range 0..{}",
            r, ovm.n_cells
        ))),
        Some(r) => Ok(r),
        None => Ok(ovm.top),
    }
}

/// Instances of `cell` under `root`: the summary's count when the root
/// is the top, else a product sum over the cells between them (the
/// ancestors of `cell`, in topological order, from the root).
fn insts_under(s: &HierSummary, ovm: &Ovm, root: u32, cell: u32, anc: &[bool]) -> u64 {
    if root == ovm.top {
        return s.insts(cell);
    }
    let mut inset: Vec<(u32, u32)> = anc
        .iter()
        .enumerate()
        .filter(|(_, &f)| f)
        .map(|(ci, _)| (ovm.cell(ci as u32).topo_rank, ci as u32))
        .collect();
    inset.sort_unstable();
    let mut counts: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    counts.insert(root, 1);
    for (_, ci) in inset {
        let Some(&n) = counts.get(&ci) else {
            continue;
        };
        if n == 0 {
            continue;
        }
        for e in s.children(ci) {
            if anc.get(e.child as usize).copied().unwrap_or(false) {
                let add = n.saturating_mul(e.members);
                let slot = counts.entry(e.child).or_insert(0);
                *slot = slot.saturating_add(add);
            }
        }
    }
    counts.get(&cell).copied().unwrap_or(0)
}

/// The extent of a cell's instances under `root` (None = the top), from
/// the summary's edges out of the root: exact for a cell the root places
/// directly, else the containers' extent. Root coordinates.
pub fn extent(h: &HierHandle, root: Option<u32>, cell: u32) -> Result<CellExtent, HierError> {
    let s = h.summary()?;
    let vfs = h.vfs();
    let ovm = &vfs.ovm;
    let root = root_of(ovm, root)?;
    if cell >= ovm.n_cells {
        return Err(HierError::Other(format!("cell index {} out of range 0..{}", cell, ovm.n_cells)));
    }
    if cell == root {
        let b = ovm.cell_rbbox(cell);
        return Ok(CellExtent {
            insts: 1,
            bbox: (!b.is_empty()).then_some(b),
            approx: false,
        });
    }
    let anc = s.ancestors(cell);
    if !anc.get(root as usize).copied().unwrap_or(false) {
        return Ok(CellExtent { insts: 0, bbox: None, approx: false });
    }
    let mut bbox = BBox::EMPTY;
    let mut approx = false;
    for e in s.children(root) {
        if anc[e.child as usize] {
            bbox.grow(&e.extent);
            if e.child != cell {
                approx = true;
            }
        }
    }
    Ok(CellExtent {
        insts: insts_under(&s, ovm, root, cell, &anc),
        bbox: (!bbox.is_empty()).then_some(bbox),
        approx,
    })
}

/// The instances of a cell inside a view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instances {
    /// instance boxes (top coordinates) meeting the view, at most `cap`
    pub boxes: Vec<BBox>,
    /// the cap or the walk budget stopped the walk: more instances may
    /// meet the view
    pub more: bool,
    /// placement records and members the walk looked at
    pub visited: u64,
}

struct Walk<'a> {
    ovm: &'a Ovm,
    summary: &'a HierSummary,
    inset: &'a [bool],
    target: u32,
    cap: usize,
    budget: u64,
    visited: u64,
    out: Vec<BBox>,
    more: bool,
    /// per walked cell, where its instances of the target lie (the
    /// union of its edges' extents towards the cells holding the
    /// target; cell coordinates) - the view is cut to it before the
    /// cell's BVH is walked, so a whole-chip view of a cell placed in
    /// six blocks visits those blocks' leaves and no other
    reach: std::collections::HashMap<u32, BBox>,
    /// each instance's box goes to `out` (the in-view walk), or only into
    /// `union` (`info`'s span; no cap)
    keep_boxes: bool,
    union: BBox,
    /// target instances met
    found: u64,
    /// the first `xf_cap` instances' transforms (top coordinates)
    xf_cap: usize,
    xfs: Vec<OrthoTransform>,
}

impl<'a> Walk<'a> {
    fn new(ovm: &'a Ovm, summary: &'a HierSummary, inset: &'a [bool], target: u32, cap: usize, budget: u64) -> Self {
        Walk {
            ovm,
            summary,
            inset,
            target,
            cap,
            budget,
            visited: 0,
            out: Vec::new(),
            more: false,
            reach: std::collections::HashMap::new(),
            keep_boxes: true,
            union: BBox::EMPTY,
            found: 0,
            xf_cap: 0,
            xfs: Vec::new(),
        }
    }
}

fn floor_div(a: i128, b: i128) -> i128 {
    let q = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

fn ceil_div(a: i128, b: i128) -> i128 {
    -floor_div(-a, b)
}

/// The index range [lo, hi] of members `k` (0..n) whose box
/// `child_box + k * v` meets the view along an axis-aligned vector
/// `v`; the whole range when `v` is diagonal or zero.
fn grid_range(child_box: &BBox, view: &BBox, v: (i64, i64), n: u32) -> (i64, i64) {
    let n = n as i64;
    if n == 0 {
        return (0, -1);
    }
    let (lo, hi, step) = if v.1 == 0 && v.0 != 0 {
        (view.x0 as i128 - child_box.x1 as i128, view.x1 as i128 - child_box.x0 as i128, v.0 as i128)
    } else if v.0 == 0 && v.1 != 0 {
        (view.y0 as i128 - child_box.y1 as i128, view.y1 as i128 - child_box.y0 as i128, v.1 as i128)
    } else {
        return (0, n - 1);
    };
    // k * step in [lo, hi]
    let (a, b) = if step > 0 {
        (ceil_div(lo, step), floor_div(hi, step))
    } else {
        (ceil_div(hi, step), floor_div(lo, step))
    };
    let a = a.max(0).min(n as i128) as i64;
    let b = b.min(n as i128 - 1).max(-1) as i64;
    (a, b)
}

fn shifted(b: &BBox, ox: i64, oy: i64) -> BBox {
    BBox {
        x0: b.x0.saturating_add(ox),
        y0: b.y0.saturating_add(oy),
        x1: b.x1.saturating_add(ox),
        y1: b.y1.saturating_add(oy),
    }
}

impl Walk<'_> {
    fn spend(&mut self, n: u64) -> bool {
        self.visited = self.visited.saturating_add(n);
        if self.visited > self.budget {
            self.more = true;
            return false;
        }
        true
    }

    /// One member of a record: emit it or walk into it.
    fn member(
        &mut self,
        head: &PlaceHead,
        child_box: &BBox,
        ox: i64,
        oy: i64,
        outer: &OrthoTransform,
        local_view: &BBox,
    ) -> Result<bool, String> {
        if !self.spend(1) {
            return Ok(false);
        }
        let mbox = shifted(child_box, ox, oy);
        if !mbox.intersects(local_view) {
            return Ok(true);
        }
        if head.child == self.target {
            let placed = outer.apply_bbox(mbox)?;
            self.found += 1;
            if self.xfs.len() < self.xf_cap {
                let xf = OrthoTransform::place(head.x.saturating_add(ox), head.y.saturating_add(oy), head.rot, head.flip)?;
                self.xfs.push(outer.compose(&xf)?);
            }
            if !self.keep_boxes {
                self.union.grow(&placed);
                return Ok(true);
            }
            self.out.push(placed);
            if self.out.len() >= self.cap {
                self.more = true;
                return Ok(false);
            }
            return Ok(true);
        }
        let member_xf = OrthoTransform::place(
            head.x.saturating_add(ox),
            head.y.saturating_add(oy),
            head.rot,
            head.flip,
        )?;
        let inner_outer = outer.compose(&member_xf)?;
        let inner_view = member_xf.invert()?.apply_bbox(*local_view)?;
        self.walk(head.child, &inner_outer, &inner_view)
    }

    fn record(&mut self, pli: u64, outer: &OrthoTransform, local_view: &BBox) -> Result<bool, String> {
        if !self.spend(1) {
            return Ok(false);
        }
        let head = self.ovm.place_head(pli);
        if (head.child as usize) >= self.inset.len() || !self.inset[head.child as usize] {
            return Ok(true);
        }
        let child_rbbox = self.ovm.cell_rbbox(head.child);
        if child_rbbox.is_empty() {
            return Ok(true);
        }
        let child_box = OrthoTransform::place(head.x, head.y, head.rot, head.flip)?.apply_bbox(child_rbbox)?;
        match head.kind {
            0 => self.member(&head, &child_box, 0, 0, outer, local_view),
            1 => {
                let (ka, kb) = grid_range(&child_box, local_view, head.va, head.na);
                let (la, lb) = grid_range(&child_box, local_view, head.vb, head.nb);
                for k in ka..=kb {
                    for l in la..=lb {
                        let ox = k.saturating_mul(head.va.0).saturating_add(l.saturating_mul(head.vb.0));
                        let oy = k.saturating_mul(head.va.1).saturating_add(l.saturating_mul(head.vb.1));
                        if !self.member(&head, &child_box, ox, oy, outer, local_view)? {
                            return Ok(false);
                        }
                    }
                }
                Ok(true)
            }
            _ => {
                let Some(pts) = self.ovm.pts_ref(pli) else {
                    return Ok(true);
                };
                for chunk in 0..pts.n_chunks {
                    let cb = pts.chunk_bbox(chunk);
                    let reach = BBox {
                        x0: child_box.x0.saturating_add(cb.x0),
                        y0: child_box.y0.saturating_add(cb.y0),
                        x1: child_box.x1.saturating_add(cb.x1),
                        y1: child_box.y1.saturating_add(cb.y1),
                    };
                    if !self.spend(1) {
                        return Ok(false);
                    }
                    if !reach.intersects(local_view) {
                        continue;
                    }
                    let (lo, hi) = pts.chunk_range(chunk);
                    for slot in lo..hi {
                        let (ox, oy) = pts.pt(slot);
                        if !self.member(&head, &child_box, ox, oy, outer, local_view)? {
                            return Ok(false);
                        }
                    }
                }
                Ok(true)
            }
        }
    }

    /// Where `cell`'s instances of the target lie, in its coordinates.
    fn reach_of(&mut self, cell: u32) -> BBox {
        if let Some(b) = self.reach.get(&cell) {
            return *b;
        }
        let mut b = BBox::EMPTY;
        for e in self.summary.children(cell) {
            if self.inset.get(e.child as usize).copied().unwrap_or(false) {
                b.grow(&e.extent);
            }
        }
        self.reach.insert(cell, b);
        b
    }

    /// The placements of `cell` meeting `local_view` (cell coordinates);
    /// `outer` maps cell coordinates to the top's. Returns false when
    /// the walk stopped (cap or budget).
    fn walk(&mut self, cell: u32, outer: &OrthoTransform, local_view: &BBox) -> Result<bool, String> {
        let (start, count) = self.ovm.cell_places(cell);
        if count == 0 {
            return Ok(true);
        }
        let local_view = &local_view.intersect(&self.reach_of(cell));
        if local_view.is_empty() {
            return Ok(true);
        }
        let (bvh_start, bvh_count) = self.ovm.cell_bvh(cell);
        if bvh_count == 0 {
            for k in 0..count as u64 {
                if !self.record(start as u64 + k, outer, local_view)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        let mut stack = vec![bvh_start];
        while let Some(ni) = stack.pop() {
            if !self.spend(1) {
                return Ok(false);
            }
            let node = self.ovm.bvh(ni);
            if !node.bbox.intersects(local_view) {
                continue;
            }
            if !node.leaf {
                for k in 0..node.count as u32 {
                    stack.push(node.first + k);
                }
                continue;
            }
            for k in 0..node.count as u64 {
                if !self.record(node.first as u64 + k, outer, local_view)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

/// The instances of `cell` whose box meets `view` (root coordinates;
/// root None = the top): a walk from the root through the cells whose
/// subtree holds it, pruned by the placement BVH, at most `cap` boxes and
/// `budget` visits.
pub fn instances(
    h: &HierHandle,
    root: Option<u32>,
    cell: u32,
    view: BBox,
    cap: usize,
    budget: u64,
) -> Result<Instances, HierError> {
    let s = h.summary()?;
    let vfs = h.vfs();
    let ovm = &vfs.ovm;
    let root = root_of(ovm, root)?;
    if cell >= ovm.n_cells {
        return Err(HierError::Other(format!("cell index {} out of range 0..{}", cell, ovm.n_cells)));
    }
    if view.is_empty() || cap == 0 {
        return Ok(Instances { boxes: Vec::new(), more: false, visited: 0 });
    }
    if cell == root {
        let b = ovm.cell_rbbox(cell);
        let boxes = if !b.is_empty() && b.intersects(&view) { vec![b] } else { Vec::new() };
        return Ok(Instances { boxes, more: false, visited: 1 });
    }
    let inset = s.ancestors(cell);
    if !inset.get(root as usize).copied().unwrap_or(false) {
        return Ok(Instances { boxes: Vec::new(), more: false, visited: 0 });
    }
    // (a light cache: the walk marks the cells whose placements the image
    // lacks and runs again once they are in - crate::light::LightLink::run)
    let (boxes, more, visited) = h.link.run(|vfs| {
        let mut walk = Walk::new(&vfs.ovm, &s, &inset, cell, cap, budget);
        walk.walk(root, &OrthoTransform::identity(), &view)?;
        Ok((walk.out, walk.more, walk.visited))
    })?;
    Ok(Instances {
        boxes,
        more,
        visited,
    })
}

/// What the status line says of a cell (SPEC-VIEWER §8c; user 2026-10-15:
/// "셀 트리에서 셀을 선택하면 셀의 관련정보(좌표등)을 상태줄에 표시해줘").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellInfo {
    pub cell: u32,
    pub name: String,
    /// levels below the cell
    pub height: u32,
    /// distinct children
    pub children: usize,
    /// the cell's recursive bbox in its own coordinates; None = no shapes
    pub rbbox: Option<BBox>,
    /// the cell is the root the query walked from (the top or a view root)
    pub is_root: bool,
    /// instances under the root
    pub insts: u64,
    /// where they lie (root coordinates): the union of their boxes when a
    /// walk met every one of them within its budget, else CellExtent's box
    /// and `approx`
    pub span: Option<BBox>,
    pub approx: bool,
    /// the instance, when the root holds the cell once
    pub placement: Option<CellPlacement>,
}

/// One instance of a cell, in the root's coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellPlacement {
    /// the cell's origin
    pub x: i64,
    pub y: i64,
    /// quarter turns counterclockwise
    pub rot: u8,
    /// mirrored about the x axis before the rotation (GDS/OASIS order)
    pub flip: bool,
    /// the instance's box
    pub bbox: BBox,
}

/// A view that holds every coordinate a walk can reach: each cell cuts it
/// to where its instances of the target lie before anything is transformed.
const EVERYWHERE: BBox = BBox {
    x0: i64::MIN / 4,
    y0: i64::MIN / 4,
    x1: i64::MAX / 4,
    y1: i64::MAX / 4,
};

/// A cell's facts under `root` (None = the top): its own extent and
/// children, how many instances the root holds and where - the summary's
/// extent for a cell the root places directly, else a walk of at most
/// `budget` visits counts them all for the exact span (past it, the
/// summary's extent stands); for one instance the walk gives its placement.
/// A cell without shapes has no box to walk to: its count and no place.
pub fn info(h: &HierHandle, root: Option<u32>, cell: u32, budget: u64) -> Result<CellInfo, HierError> {
    let s = h.summary()?;
    let vfs = h.vfs();
    let ovm = &vfs.ovm;
    let root_ci = root_of(ovm, root)?;
    if cell >= ovm.n_cells {
        return Err(HierError::Other(format!("cell index {} out of range 0..{}", cell, ovm.n_cells)));
    }
    let record = ovm.cell(cell);
    let rbbox = ovm.cell_rbbox(cell);
    let extent = extent(h, root, cell)?;
    let mut out = CellInfo {
        cell,
        name: record.name,
        height: record.height,
        children: s.children(cell).count(),
        rbbox: (!rbbox.is_empty()).then_some(rbbox),
        is_root: cell == root_ci,
        insts: extent.insts,
        span: extent.bbox,
        approx: extent.approx,
        placement: None,
    };
    // a cell the root places directly (and only there) has its exact extent
    // in the summary already, the union of its records'; the walk is for a
    // cell deeper down and for the one instance's placement
    if out.is_root || out.insts == 0 || rbbox.is_empty() || (!out.approx && out.insts > 1) {
        return Ok(out);
    }
    let inset = s.ancestors(cell);
    let (more, found, union, xfs) = h.link.run(|vfs| {
        let mut walk = Walk::new(&vfs.ovm, &s, &inset, cell, usize::MAX, budget);
        walk.keep_boxes = false;
        walk.xf_cap = 1;
        walk.walk(root_ci, &OrthoTransform::identity(), &EVERYWHERE)?;
        Ok((walk.more, walk.found, walk.union, walk.xfs))
    })?;
    if more || found != out.insts || union.is_empty() {
        return Ok(out);
    }
    out.span = Some(union);
    out.approx = false;
    if out.insts == 1 {
        if let Some(xf) = xfs.first() {
            let (x, y, rot, flip) = xf.parts();
            out.placement = Some(CellPlacement { x, y, rot, flip, bbox: union });
        }
    }
    Ok(out)
}

/// A jobdeck placement of a source: `p -> scale * p + (dx, dy)`, deck dbu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckXf {
    pub scale: f64,
    pub dx: f64,
    pub dy: f64,
}

impl DeckXf {
    pub const IDENTITY: DeckXf = DeckXf { scale: 1.0, dx: 0.0, dy: 0.0 };

    /// A source box in deck coordinates.
    pub fn apply(&self, b: &BBox) -> [f64; 4] {
        [
            self.scale * b.x0 as f64 + self.dx,
            self.scale * b.y0 as f64 + self.dy,
            self.scale * b.x1 as f64 + self.dx,
            self.scale * b.y1 as f64 + self.dy,
        ]
    }

    /// A deck view in source coordinates (grown to whole dbu); None
    /// when the placement is degenerate.
    pub fn source_view(&self, view: [f64; 4]) -> Option<BBox> {
        if !(self.scale > 0.0) || !self.scale.is_finite() {
            return None;
        }
        let f = |v: f64, d: f64| (v - d) / self.scale;
        let clamp = |v: f64| v.max(i64::MIN as f64 / 4.0).min(i64::MAX as f64 / 4.0);
        Some(BBox {
            x0: clamp(f(view[0], self.dx)).floor() as i64,
            y0: clamp(f(view[1], self.dy)).floor() as i64,
            x1: clamp(f(view[2], self.dx)).ceil() as i64,
            y1: clamp(f(view[3], self.dy)).ceil() as i64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_oasis::doc::Rep;
    use floe_vfs::hiersum::fixture::{build, sample, shape, FCell};

    fn handle(ovm: Ovm) -> HierHandle {
        let vfs = Vfs::from_ovm_for_tests(ovm);
        HierHandle::new(Arc::new(vfs), "/nonexistent/floe-cells-test".to_string())
    }

    fn b(x0: i64, y0: i64, x1: i64, y1: i64) -> BBox {
        BBox { x0, y0, x1, y1 }
    }

    #[test]
    fn children_are_named_counted_and_sorted() {
        let h = handle(sample());
        let top = children(&h, None, 100).unwrap();
        assert_eq!(top.name, "TOP");
        assert_eq!(top.insts, 1);
        assert_eq!(top.total, 3);
        let rows: Vec<(&str, u64, bool)> = top.rows.iter().map(|r| (r.name.as_str(), r.members, r.leaf)).collect();
        assert_eq!(rows, vec![("EMPTY", 1, true), ("LEAF", 3, true), ("MID", 2, false)]);
        let mid = children(&h, Some(2), 100).unwrap();
        assert_eq!(mid.insts, 2);
        assert_eq!(mid.rows.len(), 1);
        assert_eq!((mid.rows[0].name.as_str(), mid.rows[0].members), ("LEAF", 7));
        // the cap keeps the count of all children
        let capped = children(&h, None, 1).unwrap();
        assert_eq!(capped.rows.len(), 1);
        assert_eq!(capped.total, 3);
        assert!(children(&h, Some(99), 1).is_err());
    }

    #[test]
    fn find_is_a_substring_or_a_glob_case_insensitively() {
        let h = handle(sample());
        let all = find(&h, "", 100).unwrap();
        assert_eq!(all.total, 5);
        assert_eq!(all.rows[0].name, "EMPTY");
        let sub = find(&h, "ea", 100).unwrap();
        assert_eq!(sub.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["LEAF"]);
        assert_eq!(sub.rows[0].insts, 17);
        let glob = find(&h, "*I*", 100).unwrap();
        assert_eq!(glob.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["MID"]);
        let q = find(&h, "l?af", 100).unwrap();
        assert_eq!(q.total, 1);
        assert_eq!(find(&h, "leaf*", 100).unwrap().total, 1);
        assert_eq!(find(&h, "eaf*", 100).unwrap().total, 0);
        let limited = find(&h, "", 2).unwrap();
        assert_eq!((limited.total, limited.rows.len()), (5, 2));
        assert!(name_matches("a*c", "abc"));
        assert!(name_matches("a*c", "ac"));
        assert!(!name_matches("a*c", "acb"));
        assert!(name_matches("*", ""));
        assert!(name_matches("a**b?", "axxbz"));
    }

    #[test]
    fn extent_is_exact_for_direct_children_and_the_container_for_deeper_ones() {
        let h = handle(sample());
        // TOP itself
        let top = extent(&h, None, 4).unwrap();
        assert_eq!(top, CellExtent { insts: 1, bbox: Some(b(0, 0, 1000, 1000)), approx: false });
        // MID: placed directly in TOP, exact
        let mid = extent(&h, None, 2).unwrap();
        assert_eq!(mid, CellExtent { insts: 2, bbox: Some(b(0, 0, 500, 200)), approx: false });
        // LEAF: in TOP directly (pts at 800,800) and inside both MIDs -> the
        // MIDs' extent joins in, approximate
        let leaf = extent(&h, None, 0).unwrap();
        assert_eq!(leaf, CellExtent { insts: 17, bbox: Some(b(0, 0, 860, 850)), approx: true });
        // ORPHAN: not under the top
        assert_eq!(extent(&h, None, 3).unwrap(), CellExtent { insts: 0, bbox: None, approx: false });
        // EMPTY: placed but without shapes
        assert_eq!(extent(&h, None, 1).unwrap(), CellExtent { insts: 1, bbox: None, approx: false });
    }

    #[test]
    fn instances_walk_every_path_with_rotation_flip_grid_and_pts() {
        let h = handle(sample());
        let all = instances(&h, None, 0, b(-1000, -1000, 5000, 5000), 100, 1_000_000).unwrap();
        assert_eq!(all.boxes.len(), 17);
        assert!(!all.more);
        let mut boxes = all.boxes.clone();
        boxes.sort_by_key(|k| (k.x0, k.y0));
        // MID at identity: LEAF at (5,5) and the 2x3 grid from (20,50)
        assert!(boxes.contains(&b(5, 5, 15, 9)));
        assert!(boxes.contains(&b(20, 50, 30, 54)));
        assert!(boxes.contains(&b(50, 70, 60, 74)));
        // MID turned 90 degrees at (500,100): x' = 500 - y, y' = 100 + x;
        // LEAF (5,5)-(15,9) -> x' in [491, 495], y' in [105, 115]
        assert!(boxes.contains(&b(491, 105, 495, 115)));
        // the flipped pts placement in TOP: (800,800) mirrored in y, then
        // offsets (50,0) and (0,50)
        assert!(boxes.contains(&b(800, 796, 810, 800)));
        assert!(boxes.contains(&b(850, 796, 860, 800)));
        assert!(boxes.contains(&b(800, 846, 810, 850)));
        // a view around the rotated MID only
        let part = instances(&h, None, 0, b(400, 100, 500, 200), 100, 1_000_000).unwrap();
        assert_eq!(part.boxes.len(), 7);
        assert!(part.boxes.iter().all(|k| k.x0 >= 400 && k.x1 <= 500));
        // MID's own instances: two boxes
        let mids = instances(&h, None, 2, b(0, 0, 1000, 1000), 100, 1_000_000).unwrap();
        let mut mb = mids.boxes.clone();
        mb.sort_by_key(|k| k.x0);
        assert_eq!(mb, vec![b(0, 0, 100, 100), b(400, 100, 500, 200)]);
        // the cap stops the walk and says so
        let capped = instances(&h, None, 0, b(-1000, -1000, 5000, 5000), 3, 1_000_000).unwrap();
        assert_eq!(capped.boxes.len(), 3);
        assert!(capped.more);
        // the budget too
        let starved = instances(&h, None, 0, b(-1000, -1000, 5000, 5000), 100, 2).unwrap();
        assert!(starved.more);
        assert!(starved.boxes.len() < 17);
        // an unplaced cell has no instances; the top is itself
        assert!(instances(&h, None, 3, b(0, 0, 1000, 1000), 10, 100).unwrap().boxes.is_empty());
        assert_eq!(instances(&h, None, 4, b(0, 0, 10, 10), 10, 100).unwrap().boxes, vec![b(0, 0, 1000, 1000)]);
        // a view that misses everything
        assert!(instances(&h, None, 0, b(2000, 2000, 3000, 3000), 10, 100).unwrap().boxes.is_empty());
    }

    #[test]
    fn a_view_root_walks_and_counts_from_that_cell_in_its_coordinates() {
        let h = handle(sample());
        // MID (2) as the root: LEAF is its direct child - exact extent,
        // 7 instances, boxes in MID's coordinates
        let leaf = extent(&h, Some(2), 0).unwrap();
        assert_eq!(leaf, CellExtent { insts: 7, bbox: Some(b(5, 5, 60, 74)), approx: false });
        let got = instances(&h, Some(2), 0, b(-100, -100, 1000, 1000), 100, 1_000_000).unwrap();
        assert_eq!(got.boxes.len(), 7);
        assert!(got.boxes.contains(&b(5, 5, 15, 9)));
        assert!(got.boxes.contains(&b(50, 70, 60, 74)));
        assert!(got.boxes.iter().all(|k| k.x1 <= 100 && k.y1 <= 100));
        // the root itself, and a cell above the root
        assert_eq!(extent(&h, Some(2), 2).unwrap(), CellExtent { insts: 1, bbox: Some(b(0, 0, 100, 100)), approx: false });
        assert_eq!(instances(&h, Some(2), 2, b(0, 0, 10, 10), 10, 100).unwrap().boxes, vec![b(0, 0, 100, 100)]);
        assert_eq!(extent(&h, Some(2), 4).unwrap(), CellExtent { insts: 0, bbox: None, approx: false });
        assert!(instances(&h, Some(2), 4, b(0, 0, 1000, 1000), 10, 100).unwrap().boxes.is_empty());
        // the top as an explicit root is the default
        assert_eq!(extent(&h, Some(4), 0).unwrap(), extent(&h, None, 0).unwrap());
        assert_eq!(
            instances(&h, Some(4), 0, b(-1000, -1000, 5000, 5000), 100, 1_000_000).unwrap().boxes.len(),
            17
        );
        // a deeper root: count through two levels - a fixture where the
        // root holds the container twice
        let ovm = build(
            &[
                FCell { name: "X", shape: shape(0, 0, 1, 1), places: vec![] },
                FCell {
                    name: "C",
                    shape: None,
                    places: vec![(0, 0, 0, 0, false, Rep::Grid { na: 3, nb: 1, va: (10, 0), vb: (0, 10) })],
                },
                FCell {
                    name: "R",
                    shape: None,
                    places: vec![(1, 0, 0, 0, false, Rep::One), (1, 100, 0, 0, false, Rep::One)],
                },
                FCell { name: "T", shape: None, places: vec![(2, 0, 0, 0, false, Rep::Grid { na: 5, nb: 1, va: (1000, 0), vb: (0, 1) })] },
            ],
            3,
        );
        let h = handle(ovm);
        assert_eq!(extent(&h, Some(2), 0).unwrap().insts, 6);
        assert!(extent(&h, Some(2), 0).unwrap().approx);
        assert_eq!(extent(&h, None, 0).unwrap().insts, 30);
        assert!(root_of(&h.vfs().ovm, Some(99)).is_err());
    }

    #[test]
    fn info_says_where_the_root_holds_a_cell_and_how_it_is_placed() {
        let h = handle(sample());
        // the top: the root itself, its own box
        let top = info(&h, None, 4, 1_000_000).unwrap();
        assert_eq!((top.name.as_str(), top.is_root, top.insts, top.children, top.height), ("TOP", true, 1, 3, 2));
        assert_eq!((top.span, top.approx, top.placement), (Some(b(0, 0, 1000, 1000)), false, None));
        // MID: twice, directly - the exact span, no one placement
        let mid = info(&h, None, 2, 1_000_000).unwrap();
        assert_eq!((mid.insts, mid.span, mid.approx, mid.placement), (2, Some(b(0, 0, 500, 200)), false, None));
        assert_eq!((mid.rbbox, mid.children, mid.height, mid.is_root), (Some(b(0, 0, 100, 100)), 1, 1, false));
        // LEAF: 17 instances through both MIDs and the pts repetition - the
        // walk's union, where the summary had only the MIDs' extent
        let leaf = info(&h, None, 0, 1_000_000).unwrap();
        assert_eq!(extent(&h, None, 0).unwrap().bbox, Some(b(0, 0, 860, 850)));
        assert_eq!((leaf.insts, leaf.span, leaf.approx), (17, Some(b(5, 5, 860, 850)), false));
        assert_eq!((leaf.children, leaf.height, leaf.rbbox), (0, 0, Some(b(0, 0, 10, 4))));
        // past the budget the summary's extent stands, approximate
        let starved = info(&h, None, 0, 2).unwrap();
        assert_eq!((starved.insts, starved.span, starved.approx), (17, Some(b(0, 0, 860, 850)), true));
        // a shapeless cell: counted, nowhere to walk to; an unplaced one
        let empty = info(&h, None, 1, 1_000_000).unwrap();
        assert_eq!((empty.insts, empty.rbbox, empty.span, empty.placement), (1, None, None, None));
        let orphan = info(&h, None, 3, 1_000_000).unwrap();
        assert_eq!((orphan.insts, orphan.span, orphan.placement), (0, None, None));
        // under MID as the view root: LEAF seven times in MID's coordinates
        let under = info(&h, Some(2), 0, 1_000_000).unwrap();
        assert_eq!((under.insts, under.span, under.approx), (7, Some(b(5, 5, 60, 74)), false));
        assert!(info(&h, None, 99, 10).is_err());
        // one instance two levels down: its origin and orientation are the
        // composed placement's - C turns X 90 degrees after a mirror at
        // (5, 7), T turns C 180 degrees at (100, 200)
        let ovm = build(
            &[
                FCell { name: "X", shape: shape(0, 0, 10, 4), places: vec![] },
                FCell { name: "C", shape: shape(0, 0, 50, 50), places: vec![(0, 5, 7, 1, true, Rep::One)] },
                FCell { name: "T", shape: shape(0, 0, 1000, 1000), places: vec![(1, 100, 200, 2, false, Rep::One)] },
            ],
            2,
        );
        let h = handle(ovm);
        let x = info(&h, None, 0, 1_000_000).unwrap();
        assert_eq!((x.insts, x.approx, x.span), (1, false, Some(b(91, 183, 95, 193))));
        assert_eq!(x.placement, Some(CellPlacement { x: 95, y: 193, rot: 3, flip: true, bbox: b(91, 183, 95, 193) }));
        // C as the root: X's placement in C
        let in_c = info(&h, Some(1), 0, 1_000_000).unwrap();
        assert_eq!(in_c.placement, Some(CellPlacement { x: 5, y: 7, rot: 1, flip: true, bbox: b(5, 7, 9, 17) }));
        // C itself under T: placed once, turned
        let c = info(&h, None, 1, 1_000_000).unwrap();
        assert_eq!(c.placement, Some(CellPlacement { x: 100, y: 200, rot: 2, flip: false, bbox: b(50, 150, 100, 200) }));
    }

    #[test]
    fn grid_ranges_prune_axis_arrays_exactly() {
        // members at x = 0, 30, 60, ... (box 10 wide): a view over x in
        // [55, 95] meets members 2 and 3
        let cb = b(0, 0, 10, 4);
        assert_eq!(grid_range(&cb, &b(55, 0, 95, 4), (30, 0), 10), (2, 3));
        assert_eq!(grid_range(&cb, &b(11, 0, 29, 4), (30, 0), 10), (1, 0)); // none
        assert_eq!(grid_range(&cb, &b(-100, 0, 1000, 4), (30, 0), 4), (0, 3));
        // a negative step: members at x = 0, -30, -60
        assert_eq!(grid_range(&cb, &b(-65, 0, -25, 4), (-30, 0), 10), (1, 2));
        // y axis
        assert_eq!(grid_range(&cb, &b(0, 21, 10, 21), (0, 10), 5), (2, 2));
        // a diagonal vector: everything
        assert_eq!(grid_range(&cb, &b(0, 0, 1, 1), (3, 3), 7), (0, 6));
        // the walk agrees with the brute count on a wide array
        let ovm = build(
            &[
                FCell { name: "A", shape: shape(0, 0, 10, 4), places: vec![] },
                FCell {
                    name: "T",
                    shape: None,
                    places: vec![(0, 0, 0, 0, false, Rep::Grid { na: 40, nb: 30, va: (30, 0), vb: (0, 10) })],
                },
            ],
            1,
        );
        let h = handle(ovm);
        let view = b(100, 100, 400, 150);
        let got = instances(&h, None, 0, view, 10_000, 1_000_000).unwrap();
        let mut brute = 0;
        for k in 0..40 {
            for l in 0..30 {
                let m = b(k * 30, l * 10, k * 30 + 10, l * 10 + 4);
                if m.intersects(&view) {
                    brute += 1;
                }
            }
        }
        assert_eq!(got.boxes.len(), brute);
        assert!(got.visited < 40 * 30);
    }

    #[test]
    fn a_whole_chip_view_visits_only_the_blocks_holding_the_cell() {
        // a wide top: 8 blocks side by side, the target sits in the last
        // one only; a top-wide view must not look into the other seven
        let mut cells = vec![
            FCell { name: "X", shape: shape(0, 0, 2, 2), places: vec![] },
            FCell { name: "FILL", shape: shape(0, 0, 100, 100), places: vec![] },
        ];
        for i in 0..8 {
            let places = if i == 7 {
                vec![(0, 10, 10, 0, false, Rep::One)]
            } else {
                vec![(1, 0, 0, 0, false, Rep::Grid { na: 8, nb: 8, va: (100, 0), vb: (0, 100) })]
            };
            cells.push(FCell { name: if i == 7 { "BLOCK_X" } else { "BLOCK" }, shape: shape(0, 0, 800, 800), places });
        }
        let top_places = (0..8).map(|i| (2 + i, i as i64 * 1000, 0, 0, false, Rep::One)).collect();
        cells.push(FCell { name: "T", shape: None, places: top_places });
        let n = cells.len();
        let h = handle(build(&cells, n - 1));
        let got = instances(&h, None, 0, b(0, 0, 8000, 800), 100, 1_000_000).unwrap();
        assert_eq!(got.boxes, vec![b(7010, 10, 7012, 12)]);
        // the top's 8 records and the one block's leaf: nowhere near the
        // 7 x 64 fill members
        assert!(got.visited < 40, "{}", got.visited);
    }

    #[test]
    fn deck_transforms_round_trip_a_view() {
        let xf = DeckXf { scale: 8.0, dx: 1000.0, dy: -500.0 };
        assert_eq!(xf.apply(&b(1, 2, 3, 4)), [1008.0, -484.0, 1024.0, -468.0]);
        assert_eq!(xf.source_view([1008.0, -484.0, 1024.0, -468.0]), Some(b(1, 2, 3, 4)));
        assert_eq!(xf.source_view([1009.0, -484.0, 1023.0, -468.0]), Some(b(1, 2, 3, 4)));
        assert_eq!(DeckXf { scale: 0.0, dx: 0.0, dy: 0.0 }.source_view([0.0, 0.0, 1.0, 1.0]), None);
        assert_eq!(DeckXf::IDENTITY.apply(&b(1, 2, 3, 4)), [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn a_missing_summary_is_reported_as_such_and_a_small_cache_builds_inline() {
        // the sample is small: an inline summary answers
        let h = handle(sample());
        assert!(h.summary().is_ok());
        // a file that does not belong to the cache: refused, no inline fallback
        let dir = std::env::temp_dir().join(format!("floe-cells-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let other = build(&[FCell { name: "ONLY", shape: shape(0, 0, 1, 1), places: vec![] }], 0);
        floe_vfs::hiersum::write(&other, dir.to_str().unwrap()).unwrap();
        let h = HierHandle::new(Arc::new(Vfs::from_ovm_for_tests(sample())), dir.to_str().unwrap().to_string());
        match h.summary() {
            Err(HierError::NoSummary(m)) => assert!(m.contains("hierarchy summary has 1 cells"), "{m}"),
            other => panic!("{other:?}"),
        }
        // the right file appearing later is picked up
        floe_vfs::hiersum::write(&sample(), dir.to_str().unwrap()).unwrap();
        assert!(h.summary().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
