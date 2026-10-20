//! Cell-wise inventory of an OASIS stream: the streaming index build's
//! first pass (docs/STREAMING_INDEX_PLAN.ko.md §3, S1, 2026-10-19).
//!
//! One record walk over the whole file (the scan frame in lib.rs: the
//! `cell_cuts` skim, contiguous cell groups per thread, CBLOCKs
//! inflated) collects, without keeping a single record, what the
//! in-memory build's whole-Doc pre-passes derive from the parsed Doc
//! (rust/cli/src/prepass.rs `PrePass::from_doc`):
//! - per cell: its byte range and the implicit-name seeds a cell parser
//!   needs, its own bounding box with and without text anchors, layer
//!   and text-layer masks, its own member count, and one EDGE per
//!   distinct child in first-placement order carrying the placement
//!   count, the member sum and, per orientation (rot x flip), the union
//!   box of the placement offsets widened by their repetition extents;
//! - globally: the layer order (first appearance; LAYERNAME exact pairs
//!   count), layer names and aliases, per-layer record and member sums,
//!   the resolved cell table (CELL record order, placement-only targets
//!   appended in first-reference order, as doc.rs finish_inner does)
//!   and the top (the one cell nobody places).
//!
//! `finish` then folds the hierarchy exactly as build() folds the Doc:
//! topological order (the same DFS, following children in first-
//! placement order), rank, height, recursive members and masks, and
//! the recursive bounding boxes. For the boxes, axis-aligned boxes give
//! bbox(U_p (R(B) + o_p)) = R(B) (+) bbox{o_p}: the per-(edge,
//! orientation) offset box reproduces the Doc's placement-by-placement
//! fold (tiler/hier.rs cell_bboxes_opt) exactly, whatever the child's
//! box turns out to be. The parity gate is `floe-index inventory FILE
//! --check` (G1): every value here against the Doc pre-passes, cell by
//! cell.
//!
//! Known divergences from the Doc (invalid or odd streams only):
//! - a cell defined by two CELL records, one by reference whose
//!   CELLNAME comes later, merges here as the parallel Doc parse merges
//!   it; the sequential (`jobs 1`) Doc parse keeps two cells; likewise
//!   a range that re-opens one cell under a placeholder and under its
//!   name yields two pieces whose join follows the parallel parse's
//!   record order, not the sequential parse's interleaving;
//! - recursive members saturate per edge (members x child), the Doc per
//!   placement - equal unless a product exceeds u64;
//! - a CELL record inside a CBLOCK has no byte range of its own: it is
//!   counted in `cblock_defs`, its records belong to the enclosing
//!   visible cell's range (or the head), and `def_ranges` names that
//!   range;
//! - a CELLNAME or TEXTSTRING reference number defined twice is refused
//!   (the Doc resolves such a reference by record position - at the
//!   record when a definition precedes it, else from the table - which
//!   a per-range parser cannot reproduce; real writers never emit it).

use crate::doc::{resolved_layer_names, LayerNameRule};
use crate::geom::{
    box_grow, box_is_empty, orient_box, path_bbox, pts_box, Box4, EMPTY_BOX,
};
use crate::{err, Repetition, Result, ScanStats};
use std::collections::HashMap;

/// how a CELL record or a placement names a cell: by CELLNAME reference
/// number or by name (doc.rs PlaceTarget)
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Target {
    Ref(u64),
    Name(String),
}

/// a record's box widened by its repetition's extents, as the Doc's
/// recursive bbox and direct pass widen it (`x + ex.0.min(0)` ...)
pub(crate) fn rep_box(x0: i64, y0: i64, x1: i64, y1: i64, rep: &Repetition) -> Box4 {
    (
        x0 + rep.ex.0.min(0),
        y0 + rep.ey.0.min(0),
        x1 + rep.ex.1.max(0),
        y1 + rep.ey.1.max(0),
    )
}

/// `rep_box` of an anchor-relative point list placed at the anchor
pub(crate) fn rep_box_pts(
    pts: &[(i64, i64)],
    ax: i64,
    ay: i64,
    rep: &Repetition,
) -> Box4 {
    let b = pts_box(pts);
    rep_box(ax + b.0, ay + b.1, ax + b.2, ay + b.3, rep)
}

#[derive(Debug)]
struct WEdge {
    target: Target,
    placements: u64,
    members: u64,
    obox: [Box4; 8],
}

/// one CELL record's collection
#[derive(Debug)]
struct WCell {
    key: Target,
    start: u64,
    end: u64,
    in_cblock: bool,
    /// the visible range this CELL record lies in: its own start, or
    /// the enclosing visible cell's for a CELL inside a CBLOCK (None =
    /// the head)
    host_start: Option<u64>,
    n3: u64,
    n5: u64,
    dbox: Box4,
    dbox_notext: Box4,
    dmembers: u64,
    /// bits = this part's local layer indices
    mask: Vec<u64>,
    tmask: Vec<u64>,
    /// children in first-placement order
    edges: Vec<WEdge>,
    edge_of: HashMap<Target, usize>,
    /// (modal target generation, edge): consecutive placements of the
    /// modal target skip the hash lookup
    last_edge: Option<(u64, usize)>,
}

/// one contiguous parse's collection: the head (name tables before the
/// first cell) or one worker's cell group. Layers are indexed locally
/// in first-appearance order; `finish` merges the parts' orders in
/// file order, which is the Doc's global order.
#[derive(Debug, Default)]
pub struct Part {
    cells: Vec<WCell>,
    cur: Option<usize>,
    /// the last CELL record seen outside a CBLOCK (its range ends at
    /// the next one)
    open_visible: Option<usize>,
    layers: Vec<(u32, u32)>,
    lidx: HashMap<(u32, u32), u32>,
    lrecs: Vec<u64>,
    lmems: Vec<u64>,
    pub(crate) rules: Vec<LayerNameRule>,
    /// path spine scratch (absolute coordinates)
    spine: Vec<(i64, i64)>,
}

fn set_bit(v: &mut Vec<u64>, i: u32) {
    let w = (i / 64) as usize;
    if v.len() <= w {
        v.resize(w + 1, 0);
    }
    v[w] |= 1u64 << (i % 64);
}

impl Part {
    pub(crate) fn new() -> Part {
        Part::default()
    }

    pub(crate) fn open_cell(
        &mut self,
        key: Target,
        at: u64,
        in_cblock: bool,
        n3: u64,
        n5: u64,
    ) {
        if !in_cblock {
            if let Some(v) = self.open_visible {
                self.cells[v].end = at;
            }
        }
        let host_start = if in_cblock {
            self.open_visible.map(|v| self.cells[v].start)
        } else {
            Some(at)
        };
        let i = self.cells.len();
        self.cells.push(WCell {
            key,
            start: at,
            end: at,
            in_cblock,
            host_start,
            n3,
            n5,
            dbox: EMPTY_BOX,
            dbox_notext: EMPTY_BOX,
            dmembers: 0,
            mask: Vec::new(),
            tmask: Vec::new(),
            edges: Vec::new(),
            edge_of: HashMap::new(),
            last_edge: None,
        });
        self.cur = Some(i);
        if !in_cblock {
            self.open_visible = Some(i);
        }
    }

    /// after the last record of this parse: `end` = the END record's
    /// offset, or the next group's first CELL record
    pub(crate) fn close(&mut self, end: u64) {
        if let Some(v) = self.open_visible.take() {
            self.cells[v].end = end;
        }
        self.cur = None;
    }

    pub(crate) fn reg_layer(&mut self, l: u32, d: u32) -> u32 {
        if let Some(&i) = self.lidx.get(&(l, d)) {
            return i;
        }
        let i = self.layers.len() as u32;
        self.layers.push((l, d));
        self.lidx.insert((l, d), i);
        self.lrecs.push(0);
        self.lmems.push(0);
        i
    }

    pub(crate) fn shape(
        &mut self,
        pos: usize,
        li: u32,
        members: u64,
        bb: Box4,
    ) -> Result<()> {
        let ci = match self.cur {
            Some(i) => i,
            None => return err(pos, "shape outside cell"),
        };
        let cell = &mut self.cells[ci];
        box_grow(&mut cell.dbox, &bb);
        box_grow(&mut cell.dbox_notext, &bb);
        set_bit(&mut cell.mask, li);
        cell.dmembers = cell.dmembers.saturating_add(members);
        self.lrecs[li as usize] += 1;
        self.lmems[li as usize] += members;
        Ok(())
    }

    /// text anchors count into the box and both masks, not into members
    pub(crate) fn text(&mut self, pos: usize, li: u32, bb: Box4) -> Result<()> {
        let ci = match self.cur {
            Some(i) => i,
            None => return err(pos, "text outside cell"),
        };
        let cell = &mut self.cells[ci];
        box_grow(&mut cell.dbox, &bb);
        set_bit(&mut cell.mask, li);
        set_bit(&mut cell.tmask, li);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn place(
        &mut self,
        pos: usize,
        target: &Target,
        gen: u64,
        x: i64,
        y: i64,
        rot: u8,
        flip: bool,
        rep: &Repetition,
    ) -> Result<()> {
        let ci = match self.cur {
            Some(i) => i,
            None => return err(pos, "placement outside cell"),
        };
        let cell = &mut self.cells[ci];
        let ei = match cell.last_edge {
            Some((g, e)) if g == gen => e,
            _ => {
                let e = match cell.edge_of.get(target) {
                    Some(&e) => e,
                    None => {
                        cell.edges.push(WEdge {
                            target: target.clone(),
                            placements: 0,
                            members: 0,
                            obox: [EMPTY_BOX; 8],
                        });
                        cell.edge_of.insert(target.clone(), cell.edges.len() - 1);
                        cell.edges.len() - 1
                    }
                };
                cell.last_edge = Some((gen, e));
                e
            }
        };
        let e = &mut cell.edges[ei];
        e.placements += 1;
        e.members = e.members.saturating_add(rep.members);
        let o = (rot & 3) as usize | ((flip as usize) << 2);
        box_grow(&mut e.obox[o], &rep_box(x, y, x, y, rep));
        Ok(())
    }

    /// a path's box: the modal spine placed at the anchor, then
    /// geom::path_bbox - the build's own function
    pub(crate) fn path_box(
        &mut self,
        deltas: &[(i64, i64)],
        ax: i64,
        ay: i64,
        hw: i64,
        es: i64,
        ee: i64,
    ) -> Box4 {
        self.spine.clear();
        self.spine
            .extend(deltas.iter().map(|&(dx, dy)| (ax + dx, ay + dy)));
        path_bbox(&self.spine, hw, es, ee)
    }
}

// ------------------------------------------------------------- output

/// the bytes of one CELL record's definition: [start, end) in the file
/// and the implicit CELLNAME / TEXTSTRING counts before it (a cell
/// parser's refnum seeds, as `cell_cuts` reports them)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellRange {
    pub start: u64,
    pub end: u64,
    pub cellname_base: u64,
    pub textstring_base: u64,
}

/// one distinct child of a cell
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub child: usize,
    pub placements: u64,
    /// sum of the placements' repetition members (saturating)
    pub members: u64,
    /// per orientation `rot | flip << 2`: the union of the placements'
    /// anchors widened by their repetition extents (EMPTY_BOX: unused)
    pub obox: [Box4; 8],
}

#[derive(Clone, Debug)]
pub struct InvCell {
    pub name: String,
    /// CELL records outside CBLOCKs that define this cell (file order;
    /// one for every cell of a well-formed stream)
    pub ranges: Vec<CellRange>,
    /// CELL records inside CBLOCKs (no range of their own)
    pub cblock_defs: u32,
    /// start offsets, in file order, of the ranges (the head counts as
    /// `Inventory::head`) that hold a CELL record of this cell: once a
    /// reader has parsed them all, the cell's pieces are complete
    pub def_ranges: Vec<u64>,
    pub records: u64,
    pub shapes: u64,
    pub placements: u64,
    pub texts: u64,
    pub points: u64,
    pub doc_bytes: u64,
    pub doc_bytes_pow2: u64,
    /// own records' box, text anchors included (build: dbox) / excluded
    /// (occupancy, coverage); EMPTY_BOX when there are none
    pub dbox: Box4,
    pub dbox_notext: Box4,
    /// own shape members (texts and placements excluded)
    pub dmembers: u64,
    /// own layer mask / text-layer mask over `Inventory::layer_order`
    pub dmask: Vec<u8>,
    pub dtmask: Vec<u8>,
    /// children in first-placement order
    pub edges: Vec<Edge>,
}

pub struct Inventory {
    /// the scan's counters and per-CELL-record stats (file order)
    pub scan: ScanStats,
    /// cells in Doc index order
    pub cells: Vec<InvCell>,
    pub top: usize,
    /// the head: the bytes before the first visible CELL record (START,
    /// name tables, and any cell a CBLOCK there defines) - the first
    /// range a cell parser reads; seeds 0
    pub head: CellRange,
    /// CELLNAME reference -> name and TEXTSTRING reference -> string
    /// (implicit entries numbered by position, first entry wins): what
    /// the cell parser resolves names and texts through
    pub cellnames: HashMap<u64, String>,
    pub textstrings: HashMap<u64, String>,
    pub layer_order: Vec<(u32, u32)>,
    pub layer_names: HashMap<(u32, u32), String>,
    pub layer_aliases: HashMap<(u32, u32), Vec<String>>,
    /// per layer (index into layer_order): own shape records as the
    /// geometry parser materializes them, and their members
    pub lrecs: Vec<u64>,
    pub lmems: Vec<u64>,
    /// the fold: children-before-parents order, parents-before-children
    /// rank, height, recursive members, recursive layer / text masks,
    /// recursive boxes with and without texts
    pub topo: Vec<usize>,
    pub rank: Vec<u32>,
    pub height: Vec<u32>,
    pub rmembers: Vec<u64>,
    pub rmask: Vec<Vec<u8>>,
    pub rtmask: Vec<Vec<u8>>,
    pub rbb: Vec<Option<Box4>>,
    pub rbb_notext: Vec<Option<Box4>>,
}

impl Inventory {
    pub fn unit(&self) -> f64 {
        self.scan.unit
    }

    /// bytes per layer mask
    pub fn mask_bytes(&self) -> usize {
        self.layer_order.len().div_ceil(8).max(1)
    }

    /// cell name -> Doc index, for a cell range's parser
    /// (`doc::CellTables::new(&inv.cellnames, &inv.textstrings, &index)`)
    pub fn cell_index(&self) -> HashMap<String, usize> {
        self.cells
            .iter()
            .enumerate()
            .map(|(i, c)| (c.name.clone(), i))
            .collect()
    }
}

fn cell_index(
    names: &mut Vec<String>,
    by_name: &mut HashMap<String, usize>,
    name: &str,
) -> usize {
    if let Some(&i) = by_name.get(name) {
        return i;
    }
    let i = names.len();
    names.push(name.to_string());
    by_name.insert(name.to_string(), i);
    i
}

/// local bits -> global bits (ovm bitset layout: byte i/8, bit i%8)
fn remap_bits(local: &[u64], map: &[u32], out: &mut [u8]) {
    for (w, &word) in local.iter().enumerate() {
        let mut bits = word;
        while bits != 0 {
            let b = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let g = map[w * 64 + b] as usize;
            out[g / 8] |= 1 << (g % 8);
        }
    }
}

fn grow_opt(b: &mut Option<Box4>, add: Box4) {
    *b = Some(match *b {
        None => add,
        Some(o) => (
            o.0.min(add.0),
            o.1.min(add.1),
            o.2.max(add.2),
            o.3.max(add.3),
        ),
    });
}

/// children-before-parents order by the same iterative post-order DFS
/// as build()'s topo_order, following edges (distinct children in
/// first-placement order - the Doc's walk over the placements skips a
/// child already visited, so the recursion is the same). A back-edge
/// is the Doc's hard error.
fn topo_order(cells: &[InvCell]) -> Result<Vec<usize>> {
    let n = cells.len();
    let mut order = Vec::with_capacity(n);
    let mut state = vec![0u8; n]; // 0 unvisited, 1 on-stack, 2 done
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        state[start] = 1;
        stack.push((start, 0));
        while let Some(&(ci, k)) = stack.last() {
            if k < cells[ci].edges.len() {
                stack.last_mut().unwrap().1 = k + 1;
                let c = cells[ci].edges[k].child;
                if state[c] == 0 {
                    state[c] = 1;
                    stack.push((c, 0));
                } else if state[c] == 1 {
                    return err(
                        0,
                        &format!(
                            "cycle in hierarchy involving cell '{}' - \
                             invalid OASIS (placement graph must be \
                             acyclic); indexing refused",
                            cells[c].name
                        ),
                    );
                }
            } else {
                order.push(ci);
                state[ci] = 2;
                stack.pop();
            }
        }
    }
    Ok(order)
}

/// the parts of a finished scan -> the Inventory (name resolution, the
/// global layer order, cell merging, the top, the fold)
pub(crate) fn finish(mut st: ScanStats) -> Result<Inventory> {
    let head = match st.inv.take() {
        Some(p) => *p,
        None => return err(0, "inventory: scan ran without a collector"),
    };
    let mut parts: Vec<Part> = Vec::with_capacity(1 + st.inv_parts.len());
    parts.push(head);
    parts.append(&mut st.inv_parts);
    let refnames = std::mem::take(&mut st.cellname_table);
    if st.cellname_dups {
        return err(
            0,
            "duplicate CELLNAME reference number: the geometry parser              resolves it by record position, which a cell-wise build              cannot reproduce - refused",
        );
    }
    // the TEXTSTRING table (implicit entries numbered by position in
    // file order; a reference defined twice is refused for the same
    // reason as a CELLNAME); every reference a TEXT made must resolve
    // (doc.rs finish_inner)
    let mut textstrings: HashMap<u64, String> = HashMap::new();
    {
        let mut implicit = 0u64;
        for (r, s) in std::mem::take(&mut st.textstrings_seen) {
            let r = match r {
                Some(r) => r,
                None => {
                    implicit += 1;
                    implicit - 1
                }
            };
            if textstrings.contains_key(&r) {
                return err(
                    0,
                    &format!("duplicate TEXTSTRING reference number {} - refused", r),
                );
            }
            textstrings.insert(r, s);
        }
        if st.text_refs.iter().any(|r| !textstrings.contains_key(r)) {
            return err(0, "unresolved textstring ref");
        }
    }
    // the head ends where the first visible CELL record starts
    let head_end = parts
        .iter()
        .flat_map(|p| p.cells.iter())
        .filter(|wc| !wc.in_cblock)
        .map(|wc| wc.start)
        .min()
        .unwrap_or(st.end_at);

    // ---- layers: the parts' first-appearance orders merged in file
    // order (the Doc's head-then-chunks union), per-layer sums remapped
    let mut layer_order: Vec<(u32, u32)> = Vec::new();
    let mut gidx: HashMap<(u32, u32), u32> = HashMap::new();
    let mut maps: Vec<Vec<u32>> = Vec::with_capacity(parts.len());
    for part in &parts {
        let mut map = Vec::with_capacity(part.layers.len());
        for &ld in &part.layers {
            let g = match gidx.get(&ld) {
                Some(&g) => g,
                None => {
                    let g = layer_order.len() as u32;
                    layer_order.push(ld);
                    gidx.insert(ld, g);
                    g
                }
            };
            map.push(g);
        }
        maps.push(map);
    }
    let nl = layer_order.len();
    let bw = nl.div_ceil(8).max(1);
    let mut lrecs = vec![0u64; nl];
    let mut lmems = vec![0u64; nl];
    let mut rules: Vec<LayerNameRule> = Vec::new();
    for (part, map) in parts.iter_mut().zip(&maps) {
        for (i, &g) in map.iter().enumerate() {
            lrecs[g as usize] += part.lrecs[i];
            lmems[g as usize] += part.lmems[i];
        }
        rules.append(&mut part.rules);
    }
    let (layer_names, layer_aliases) = resolved_layer_names(&layer_order, &rules);

    // ---- cells: CELL records in file order name the cells (by-ref
    // records through the CELLNAME table, a missing entry keeps the
    // Doc's placeholder), then every placement target in file order
    // (an undefined target becomes an empty cell at the end)
    let n_w: usize = parts.iter().map(|p| p.cells.len()).sum();
    if n_w != st.cell_stats.len() {
        return err(0, "inventory: cell record count disagrees with the scan");
    }
    let mut names: Vec<String> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut wcell_ci: Vec<usize> = Vec::with_capacity(n_w);
    for part in &parts {
        for wc in &part.cells {
            let name = match &wc.key {
                Target::Name(n) => n.clone(),
                Target::Ref(r) => match refnames.get(r) {
                    Some(n) => n.clone(),
                    None => format!("\u{1}ref{}", r),
                },
            };
            wcell_ci.push(cell_index(&mut names, &mut by_name, &name));
        }
    }
    let mut edge_child: Vec<Vec<usize>> = Vec::with_capacity(n_w);
    for part in &parts {
        for wc in &part.cells {
            let mut v = Vec::with_capacity(wc.edges.len());
            for e in &wc.edges {
                let ci = match &e.target {
                    Target::Name(n) => cell_index(&mut names, &mut by_name, n),
                    Target::Ref(r) => match refnames.get(r) {
                        Some(n) => cell_index(&mut names, &mut by_name, n),
                        None => return err(0, "unresolved cellname ref"),
                    },
                };
                v.push(ci);
            }
            edge_child.push(v);
        }
    }
    let n = names.len();
    let mut cells: Vec<InvCell> = names
        .into_iter()
        .map(|name| InvCell {
            name,
            ranges: Vec::new(),
            cblock_defs: 0,
            def_ranges: Vec::new(),
            records: 0,
            shapes: 0,
            placements: 0,
            texts: 0,
            points: 0,
            doc_bytes: 0,
            doc_bytes_pow2: 0,
            dbox: EMPTY_BOX,
            dbox_notext: EMPTY_BOX,
            dmembers: 0,
            dmask: vec![0u8; bw],
            dtmask: vec![0u8; bw],
            edges: Vec::new(),
        })
        .collect();
    let mut edge_of: Vec<HashMap<usize, usize>> = vec![HashMap::new(); n];
    let mut wi = 0usize;
    for (pi, part) in parts.into_iter().enumerate() {
        for wc in part.cells {
            let ci = wcell_ci[wi];
            let cs = &st.cell_stats[wi];
            let children = &edge_child[wi];
            wi += 1;
            let cell = &mut cells[ci];
            if wc.in_cblock {
                cell.cblock_defs += 1;
            } else {
                cell.ranges.push(CellRange {
                    start: wc.start,
                    end: wc.end,
                    cellname_base: wc.n3,
                    textstring_base: wc.n5,
                });
            }
            let host = wc.host_start.unwrap_or(crate::MAGIC.len() as u64);
            if !cell.def_ranges.contains(&host) {
                cell.def_ranges.push(host);
            }
            cell.records += cs.records;
            cell.shapes += cs.shapes;
            cell.placements += cs.placements;
            cell.texts += cs.texts;
            cell.points += cs.points;
            cell.doc_bytes += cs.doc_bytes;
            cell.doc_bytes_pow2 += cs.doc_bytes_pow2;
            box_grow(&mut cell.dbox, &wc.dbox);
            box_grow(&mut cell.dbox_notext, &wc.dbox_notext);
            cell.dmembers = cell.dmembers.saturating_add(wc.dmembers);
            remap_bits(&wc.mask, &maps[pi], &mut cell.dmask);
            remap_bits(&wc.tmask, &maps[pi], &mut cell.dtmask);
            for (k, we) in wc.edges.into_iter().enumerate() {
                let child = children[k];
                match edge_of[ci].get(&child) {
                    Some(&e) => {
                        let e = &mut cell.edges[e];
                        e.placements += we.placements;
                        e.members = e.members.saturating_add(we.members);
                        for o in 0..8 {
                            box_grow(&mut e.obox[o], &we.obox[o]);
                        }
                    }
                    None => {
                        edge_of[ci].insert(child, cell.edges.len());
                        cell.edges.push(Edge {
                            child,
                            placements: we.placements,
                            members: we.members,
                            obox: we.obox,
                        });
                    }
                }
            }
        }
    }

    // ---- top = the cell nobody places (exactly one, as the Doc)
    let mut placed = vec![false; n];
    for c in &cells {
        for e in &c.edges {
            placed[e.child] = true;
        }
    }
    let tops: Vec<usize> = (0..n).filter(|&i| !placed[i]).collect();
    if tops.len() != 1 {
        return err(0, &format!("{} top cells (spike expects 1)", tops.len()));
    }
    let top = tops[0];

    // ---- the fold (build(): rank, height, rmembers, rmask, rtmask;
    // hier.rs cell_bboxes_opt: the recursive boxes), children first
    let topo = topo_order(&cells)?;
    let mut rank = vec![0u32; n];
    for (i, &ci) in topo.iter().rev().enumerate() {
        rank[ci] = match u32::try_from(i) {
            Ok(r) => r,
            Err(_) => return err(0, "limit exceeded: topo rank"),
        };
    }
    let mut height = vec![0u32; n];
    let mut rmembers = vec![0u64; n];
    let mut rmask: Vec<Vec<u8>> = vec![Vec::new(); n];
    let mut rtmask: Vec<Vec<u8>> = vec![Vec::new(); n];
    let mut rbb: Vec<Option<Box4>> = vec![None; n];
    let mut rbb_notext: Vec<Option<Box4>> = vec![None; n];
    for &ci in &topo {
        let c = &cells[ci];
        let mut hm = 0u32;
        let mut mm = c.dmembers;
        let mut mask = c.dmask.clone();
        let mut tmask = c.dtmask.clone();
        let mut bb = if box_is_empty(&c.dbox) { None } else { Some(c.dbox) };
        let mut bbn = if box_is_empty(&c.dbox_notext) {
            None
        } else {
            Some(c.dbox_notext)
        };
        for e in &c.edges {
            let ch = e.child;
            hm = hm.max(match height[ch].checked_add(1) {
                Some(h) => h,
                None => return err(0, "limit exceeded: hierarchy depth"),
            });
            mm = mm.saturating_add(rmembers[ch].saturating_mul(e.members));
            for (a, b) in mask.iter_mut().zip(&rmask[ch]) {
                *a |= *b;
            }
            for (a, b) in tmask.iter_mut().zip(&rtmask[ch]) {
                *a |= *b;
            }
            for (o, ob) in e.obox.iter().enumerate() {
                if box_is_empty(ob) {
                    continue;
                }
                let (rot, flip) = ((o & 3) as u8, o & 4 != 0);
                if let Some(cb) = rbb[ch] {
                    let r = orient_box(rot, flip, cb);
                    grow_opt(&mut bb, (r.0 + ob.0, r.1 + ob.1, r.2 + ob.2, r.3 + ob.3));
                }
                if let Some(cb) = rbb_notext[ch] {
                    let r = orient_box(rot, flip, cb);
                    grow_opt(&mut bbn, (r.0 + ob.0, r.1 + ob.1, r.2 + ob.2, r.3 + ob.3));
                }
            }
        }
        height[ci] = hm;
        rmembers[ci] = mm;
        rmask[ci] = mask;
        rtmask[ci] = tmask;
        rbb[ci] = bb;
        rbb_notext[ci] = bbn;
    }

    Ok(Inventory {
        scan: st,
        cells,
        top,
        head: CellRange {
            start: crate::MAGIC.len() as u64,
            end: head_end,
            cellname_base: 0,
            textstring_base: 0,
        },
        cellnames: refnames,
        textstrings,
        layer_order,
        layer_names,
        layer_aliases,
        lrecs,
        lmems,
        topo,
        rank,
        height,
        rmembers,
        rmask,
        rtmask,
        rbb,
        rbb_notext,
    })
}

// ------------------------------------------------------------ fixtures

/// Hand-encoded streams for the parity tests here and in rust/cli
/// (prepass.rs): what KLayout-written fixtures never contain.
#[doc(hidden)]
pub mod fixtures {
    use crate::write::W;
    use std::io::Write;

    const MAGIC: &[u8] = b"%SEMI-OASIS\r\n";

    fn start(w: &mut W) {
        w.out.extend_from_slice(MAGIC);
        w.uint(1);
        w.string(b"1.0");
        w.real_f64(1000.0);
        w.uint(0);
        for _ in 0..12 {
            w.uint(0);
        }
    }

    fn end(w: &mut W) {
        w.uint(2);
        w.uint(252);
        w.out.extend_from_slice(&[0u8; 252]);
        w.uint(0);
    }

    fn deflate(bytes: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::DeflateEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        );
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    /// CELLNAME table: "TOP" implicit 0, "LEAF" implicit 1, "MID"
    /// explicit 2, "LBL" explicit 3; TEXTSTRING "lbl" implicit 0
    fn tables(w: &mut W) {
        w.uint(3);
        w.string(b"TOP");
        w.uint(3);
        w.string(b"LEAF");
        w.uint(4);
        w.string(b"MID");
        w.uint(2);
        w.uint(4);
        w.string(b"LBL");
        w.uint(3);
        w.uint(5);
        w.string(b"lbl");
        // LAYERNAME: an exact pair already in use, an interval rule
        // (no registration), an exact pair nothing uses (registers)
        w.uint(11);
        w.string(b"METAL");
        w.uint(3);
        w.uint(1);
        w.uint(3);
        w.uint(0);
        w.uint(11);
        w.string(b"VIA");
        w.uint(4);
        w.uint(2);
        w.uint(3);
        w.uint(3);
        w.uint(0);
        w.uint(11);
        w.string(b"UNUSED");
        w.uint(3);
        w.uint(8);
        w.uint(3);
        w.uint(0);
    }

    /// TOP (CELL by ref 0): grids, four children in three orientations
    /// (one child undefined), a text, manhattan and diagonal paths,
    /// XYRELATIVE coordinates, a regular and a degenerate trapezoid,
    /// CTRAPEZOIDs with implied dimensions carried by the modal, a
    /// circle and a zero-radius circle, an explicit point-list
    /// repetition reused by the next record, a property
    fn top_cell(w: &mut W) {
        w.uint(13);
        w.uint(0);
        // RECT 1/0 at (0,0) 10x20, rep 1: 3x2 at dx 100 dy 200
        w.uint(20);
        w.byte(0x7f);
        w.uint(1);
        w.uint(0);
        w.uint(10);
        w.uint(20);
        w.sint(0);
        w.sint(0);
        w.uint(1);
        w.uint(1);
        w.uint(0);
        w.uint(100);
        w.uint(200);
        // PLACEMENT "MID" by name at (1000,500) rot 1, rep 2: 3 at dx 5000
        w.uint(17);
        w.byte(0x80 | 0x20 | 0x10 | 0x08 | (1 << 1));
        w.string(b"MID");
        w.sint(1000);
        w.sint(500);
        w.uint(2);
        w.uint(1);
        w.uint(5000);
        // PLACEMENT ref 2 (MID again) at (-300,700) rot 3 flip
        w.uint(17);
        w.byte(0x80 | 0x40 | 0x20 | 0x10 | (3 << 1) | 1);
        w.uint(2);
        w.sint(-300);
        w.sint(700);
        // PLACEMENT ref 1 (LEAF) at (20000, 0), then LEAF by name
        w.uint(17);
        w.byte(0x80 | 0x40 | 0x20 | 0x10);
        w.uint(1);
        w.sint(20000);
        w.sint(0);
        w.uint(17);
        w.byte(0x80 | 0x20 | 0x10);
        w.string(b"LEAF");
        w.sint(21000);
        w.sint(0);
        // PLACEMENT "GHOST" (never defined) at (5,5) rot 2
        w.uint(17);
        w.byte(0x80 | 0x20 | 0x10 | (2 << 1));
        w.string(b"GHOST");
        w.sint(5);
        w.sint(5);
        // TEXT "hello" on 5/0 at (-50,-60)
        w.uint(19);
        w.byte(0x40 | 0x10 | 0x08 | 0x02 | 0x01);
        w.string(b"hello");
        w.uint(5);
        w.uint(0);
        w.sint(-50);
        w.sint(-60);
        // PATH 2/0 hw 3, es 2 / ee 4 explicit, manhattan list (type 0:
        // +50 x, +30 y) at (100,100)
        w.uint(22);
        w.byte(0x80 | 0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(2);
        w.uint(0);
        w.uint(3);
        w.uint((3 << 2) | 3);
        w.sint(2);
        w.sint(4);
        w.uint(0);
        w.uint(2);
        w.sint(50);
        w.sint(30);
        w.sint(100);
        w.sint(100);
        // PATH diagonal (type 4 g-deltas) at (300,300), modal hw/es/ee
        w.uint(22);
        w.byte(0x20 | 0x10 | 0x08);
        w.uint(4);
        w.uint(2);
        w.g_delta(10, 10);
        w.g_delta(10, -10);
        w.sint(300);
        w.sint(300);
        // XYRELATIVE: RECT 1/0 20x20 (square) at (+10, -20) from (300,300)
        w.uint(16);
        w.uint(20);
        w.byte(0x80 | 0x40 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(1);
        w.uint(0);
        w.uint(20);
        w.sint(10);
        w.sint(-20);
        w.uint(15);
        // TRAPEZOID 23 horizontal 40x20 da 5 db -5 at (600,0) on 3/0
        w.uint(23);
        w.byte(0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(3);
        w.uint(0);
        w.uint(40);
        w.uint(20);
        w.sint(5);
        w.sint(-5);
        w.sint(600);
        w.sint(0);
        // degenerate TRAPEZOID 25 (h 0) on 7/0: registers the layer, no record
        w.uint(25);
        w.byte(0x20 | 0x10 | 0x02 | 0x01);
        w.uint(7);
        w.uint(0);
        w.uint(0);
        w.sint(0);
        w.sint(650);
        // CTRAPEZOID type 16 (triangle, w 30 -> implied h 30) at (700,0) on 3/0
        w.uint(26);
        w.byte(0x80 | 0x40 | 0x10 | 0x02 | 0x01);
        w.uint(3);
        w.uint(0);
        w.uint(16);
        w.uint(30);
        w.sint(700);
        // CTRAPEZOID type 0 w 50, H from the implied modal (30), y -> 100
        w.uint(26);
        w.byte(0x80 | 0x40 | 0x08);
        w.uint(0);
        w.uint(50);
        w.sint(100);
        // CIRCLE r 25 at (900,900) on 4/0; CIRCLE r 0 at (950,950) on 9/0
        w.uint(27);
        w.byte(0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(4);
        w.uint(0);
        w.uint(25);
        w.sint(900);
        w.sint(900);
        w.uint(27);
        w.byte(0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(9);
        w.uint(0);
        w.uint(0);
        w.sint(950);
        w.sint(950);
        // RECT 1/0 5x5 at (2000,0), rep 10: (0,0) (7,0) (7,9)
        w.uint(20);
        w.byte(0x80 | 0x40 | 0x10 | 0x08 | 0x04 | 0x02 | 0x01);
        w.uint(1);
        w.uint(0);
        w.uint(5);
        w.sint(2000);
        w.sint(0);
        w.uint(10);
        w.uint(1);
        w.g_delta(7, 0);
        w.g_delta(0, 9);
        // POLY 1/0 reusing that repetition: type 1 list (vertical first)
        w.uint(21);
        w.byte(0x20 | 0x10 | 0x08 | 0x04);
        w.uint(1);
        w.uint(3);
        w.sint(10);
        w.sint(20);
        w.sint(-10);
        w.sint(2100);
        w.sint(0);
        w.uint(0);
        // PROPERTY with two values, then a PAD
        w.uint(28);
        w.byte(0x20 | 0x04);
        w.string(b"P");
        w.uint(8);
        w.uint(7);
        w.uint(10);
        w.string(b"v");
        w.uint(0);
    }

    /// MID (CELL by name): a polygon, LEAF in two orientations with
    /// list and general-grid repetitions (one orientation twice), the
    /// text-only LBL
    fn mid_cell(w: &mut W) {
        w.uint(14);
        w.string(b"MID");
        // POLY 1/1 type 2 list at (0,0): E 10, N 10, W 10
        w.uint(21);
        w.byte(0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(1);
        w.uint(1);
        w.uint(2);
        w.uint(3);
        w.uint(10 << 2);
        w.uint((10 << 2) | 1);
        w.uint((10 << 2) | 2);
        w.sint(0);
        w.sint(0);
        // PLACEMENT ref 1 (LEAF) at (10,20) flip, rep 11 grid 2: (0,0) (6,2) (4,8)
        w.uint(17);
        w.byte(0x80 | 0x40 | 0x20 | 0x10 | 0x08 | 1);
        w.uint(1);
        w.sint(10);
        w.sint(20);
        w.uint(11);
        w.uint(1);
        w.uint(2);
        w.g_delta(3, 1);
        w.g_delta(-1, 3);
        // PLACEMENT ref 1 at (100,100) rot 1 flip, rep 8: 3 x 4 at (10,5) / (-3,7)
        w.uint(17);
        w.byte(0x80 | 0x40 | 0x20 | 0x10 | 0x08 | (1 << 1) | 1);
        w.uint(1);
        w.sint(100);
        w.sint(100);
        w.uint(8);
        w.uint(1);
        w.uint(2);
        w.g_delta(10, 5);
        w.g_delta(-3, 7);
        // PLACEMENT ref 1 at (-40,-40) flip again (same orientation as the first)
        w.uint(17);
        w.byte(0x80 | 0x40 | 0x20 | 0x10 | 1);
        w.uint(1);
        w.sint(-40);
        w.sint(-40);
        // PLACEMENT ref 3 (LBL) at (0,0)
        w.uint(17);
        w.byte(0x80 | 0x40 | 0x20 | 0x10);
        w.uint(3);
        w.sint(0);
        w.sint(0);
    }

    /// LEAF (CELL by ref 1): every remaining repetition type, the body
    /// in a CBLOCK when asked
    fn leaf_cell(w: &mut W, cblock: bool) {
        w.uint(13);
        w.uint(1);
        let mut b = W::new();
        // RECT 6/0 at (-5,-5) 10x10, rep 3: 4 at dy 11
        b.uint(20);
        b.byte(0x80 | 0x40 | 0x10 | 0x08 | 0x04 | 0x02 | 0x01);
        b.uint(6);
        b.uint(0);
        b.uint(10);
        b.sint(-5);
        b.sint(-5);
        b.uint(3);
        b.uint(2);
        b.uint(11);
        // RECT rep 9: 4 along (3,4)
        b.uint(20);
        b.byte(0x10 | 0x04);
        b.sint(50);
        b.uint(9);
        b.uint(2);
        b.g_delta(3, 4);
        // RECT rep 5: x list grid 3: 0, 3, 9
        b.uint(20);
        b.byte(0x10 | 0x04);
        b.sint(100);
        b.uint(5);
        b.uint(1);
        b.uint(3);
        b.uint(1);
        b.uint(2);
        // RECT rep 7: y list grid 2: 0, 4, 6
        b.uint(20);
        b.byte(0x10 | 0x04);
        b.sint(150);
        b.uint(7);
        b.uint(1);
        b.uint(2);
        b.uint(2);
        b.uint(1);
        // RECT rep 4: x list 0, 1, 3
        b.uint(20);
        b.byte(0x10 | 0x04);
        b.sint(200);
        b.uint(4);
        b.uint(1);
        b.uint(1);
        b.uint(2);
        // RECT rep 6: y list 0, 5, 5 (a duplicate member)
        b.uint(20);
        b.byte(0x10 | 0x04);
        b.sint(250);
        b.uint(6);
        b.uint(1);
        b.uint(5);
        b.uint(0);
        if cblock {
            let comp = deflate(&b.out);
            w.uint(34);
            w.uint(0);
            w.uint(b.out.len() as u64);
            w.uint(comp.len() as u64);
            w.out.extend_from_slice(&comp);
        } else {
            w.out.extend_from_slice(&b.out);
        }
    }

    /// LBL (CELL by ref 3): one text by TEXTSTRING reference on 5/1
    fn lbl_cell(w: &mut W) {
        w.uint(13);
        w.uint(3);
        w.uint(19);
        w.byte(0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(0);
        w.uint(5);
        w.uint(1);
        w.sint(7);
        w.sint(8);
    }

    /// LEAF again (CELL by ref 1): a second CELL record of a cell, after
    /// LBL - its one rectangle joins LEAF's records in file order
    fn leaf_again(w: &mut W) {
        w.uint(13);
        w.uint(1);
        w.uint(20);
        w.byte(0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(6);
        w.uint(0);
        w.uint(10);
        w.uint(10);
        w.sint(300);
        w.sint(300);
    }

    /// The adversarial hierarchy: TOP -> MID (x3 orientations) -> LEAF,
    /// LBL; TOP -> LEAF (by reference and by name), GHOST (undefined);
    /// LEAF defined by two CELL records. `tables_first` puts the name
    /// tables before the cells (seeds > 0 for every cell range), else
    /// after them (by-reference cells resolve at the end). `cblock`
    /// compresses LEAF's body.
    pub fn adversarial(tables_first: bool, cblock: bool) -> Vec<u8> {
        let mut w = W::new();
        start(&mut w);
        if tables_first {
            tables(&mut w);
        }
        top_cell(&mut w);
        mid_cell(&mut w);
        leaf_cell(&mut w, cblock);
        lbl_cell(&mut w);
        leaf_again(&mut w);
        if !tables_first {
            tables(&mut w);
        }
        end(&mut w);
        w.out
    }

    /// a cell re-opened by a CELL record inside a CBLOCK of its own
    /// range, with records after the CBLOCK: one range, one piece,
    /// three rectangles (the chunk parser folds the two CELL records)
    pub fn dup_in_range() -> Vec<u8> {
        let mut w = W::new();
        start(&mut w);
        let rect = |w: &mut W, x: i64| {
            w.uint(20);
            w.byte(0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
            w.uint(1);
            w.uint(0);
            w.uint(5);
            w.uint(5);
            w.sint(x);
            w.sint(0);
        };
        w.uint(14);
        w.string(b"A");
        rect(&mut w, 0);
        let mut b = W::new();
        b.uint(14);
        b.string(b"A");
        rect(&mut b, 100);
        let comp = deflate(&b.out);
        w.uint(34);
        w.uint(0);
        w.uint(b.out.len() as u64);
        w.uint(comp.len() as u64);
        w.out.extend_from_slice(&comp);
        rect(&mut w, 200);
        w.uint(14);
        w.string(b"TOP");
        w.uint(17);
        w.byte(0x80 | 0x20 | 0x10);
        w.string(b"A");
        w.sint(7);
        w.sint(7);
        end(&mut w);
        w.out
    }

    /// a top over two cells placing each other
    pub fn cyclic() -> Vec<u8> {
        let mut w = W::new();
        start(&mut w);
        for (a, b) in [("TOP", "A"), ("A", "B"), ("B", "A")] {
            w.uint(14);
            w.string(a.as_bytes());
            w.uint(17);
            w.byte(0x80 | 0x20 | 0x10);
            w.string(b.as_bytes());
            w.sint(1);
            w.sint(1);
        }
        end(&mut w);
        w.out
    }

    /// two unplaced cells
    pub fn two_tops() -> Vec<u8> {
        let mut w = W::new();
        start(&mut w);
        for name in ["A", "B"] {
            w.uint(14);
            w.string(name.as_bytes());
            w.uint(20);
            w.byte(0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
            w.uint(1);
            w.uint(0);
            w.uint(1);
            w.uint(1);
            w.sint(0);
            w.sint(0);
        }
        end(&mut w);
        w.out
    }

    /// a cell defined inside a CBLOCK ahead of the first visible CELL
    /// record (the skim does not inflate, so the parallel scan's head
    /// holds a whole cell), placed by the visible top
    pub fn head_cblock_cell() -> Vec<u8> {
        let mut w = W::new();
        start(&mut w);
        let mut b = W::new();
        b.uint(14);
        b.string(b"H");
        b.uint(20);
        b.byte(0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        b.uint(1);
        b.uint(0);
        b.uint(3);
        b.uint(4);
        b.sint(5);
        b.sint(6);
        b.uint(19);
        b.byte(0x40 | 0x10 | 0x08 | 0x02 | 0x01);
        b.string(b"t");
        b.uint(5);
        b.uint(0);
        b.sint(1);
        b.sint(1);
        let comp = deflate(&b.out);
        w.uint(34);
        w.uint(0);
        w.uint(b.out.len() as u64);
        w.uint(comp.len() as u64);
        w.out.extend_from_slice(&comp);
        w.uint(14);
        w.string(b"TOP");
        w.uint(17);
        w.byte(0x80 | 0x20 | 0x10);
        w.string(b"H");
        w.sint(100);
        w.sint(200);
        w.uint(20);
        w.byte(0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(2);
        w.uint(0);
        w.uint(30);
        w.uint(40);
        w.sint(-5);
        w.sint(-6);
        end(&mut w);
        w.out
    }

    /// TEXT streams doc.rs refuses: 0 = a non-UTF-8 TEXTSTRING, 1 = a
    /// non-UTF-8 inline string, 2 = a first TEXT without a string, 3 =
    /// a reference no TEXTSTRING defines; 4 = a TEXTSTRING reference
    /// number defined twice (doc.rs accepts it, the inventory refuses)
    pub fn text_refusal(kind: u8) -> Vec<u8> {
        let mut w = W::new();
        start(&mut w);
        if kind == 0 {
            w.uint(5);
            w.string(&[0xff, 0xfe]);
        }
        if kind == 4 {
            w.uint(6);
            w.string(b"first");
            w.uint(0);
            w.uint(6);
            w.string(b"second");
            w.uint(0);
        }
        w.uint(14);
        w.string(b"A");
        w.uint(20);
        w.byte(0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(1);
        w.uint(0);
        w.uint(1);
        w.uint(1);
        w.sint(0);
        w.sint(0);
        match kind {
            1 => {
                w.uint(19);
                w.byte(0x40 | 0x10 | 0x08 | 0x02 | 0x01);
                w.string(&[0xc3, 0x28]);
                w.uint(5);
                w.uint(0);
                w.sint(1);
                w.sint(2);
            }
            2 => {
                w.uint(19);
                w.byte(0x10 | 0x08 | 0x02 | 0x01);
                w.uint(5);
                w.uint(0);
                w.sint(1);
                w.sint(2);
            }
            3 => {
                w.uint(19);
                w.byte(0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
                w.uint(7);
                w.uint(5);
                w.uint(0);
                w.sint(1);
                w.sint(2);
            }
            _ => {}
        }
        end(&mut w);
        w.out
    }

    /// one cell with a record the geometry parser refuses
    pub fn with_record(id: u64) -> Vec<u8> {
        let mut w = W::new();
        start(&mut w);
        w.uint(14);
        w.string(b"A");
        match id {
            18 => {
                // placement of B by name with the M bit: C X Y M,
                // magnification 2 as a type-0 real (a whole number)
                w.uint(18);
                w.byte(0x80 | 0x20 | 0x10 | 0x04);
                w.string(b"B");
                w.uint(0);
                w.uint(2);
                w.sint(0);
                w.sint(0);
            }
            33 => {
                // XGEOMETRY attribute 1, 3 bytes, 1/0 at (0,0)
                w.uint(33);
                w.byte(0x10 | 0x08 | 0x02 | 0x01);
                w.uint(1);
                w.uint(1);
                w.uint(0);
                w.string(b"xyz");
                w.sint(0);
                w.sint(0);
            }
            _ => unreachable!(),
        }
        end(&mut w);
        w.out
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::doc::{parse_doc, Doc, Rep};
    use crate::geom::rep_extent;

    /// the Doc's own values, computed the way build()'s direct pass and
    /// hier.rs do (the cross-crate check against the real functions is
    /// rust/cli prepass.rs's test; this one pins the grammar sync)
    struct Reference {
        dbox: Vec<Box4>,
        dbox_notext: Vec<Box4>,
        dmembers: Vec<u64>,
        dmask: Vec<Vec<u8>>,
        dtmask: Vec<Vec<u8>>,
        edges: Vec<Vec<Edge>>,
        lrecs: Vec<u64>,
        lmems: Vec<u64>,
        rbb: Vec<Option<Box4>>,
        rbb_notext: Vec<Option<Box4>>,
    }

    fn rep_of(r: &Rep) -> Repetition {
        let (ex, ey) = rep_extent(r);
        Repetition { members: r.members(), ex, ey }
    }

    fn reference(doc: &Doc) -> Reference {
        let nl = doc.layer_order.len();
        let bw = nl.div_ceil(8).max(1);
        let lidx: HashMap<(u32, u32), usize> =
            doc.layer_order.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        let n = doc.cells.len();
        let mut r = Reference {
            dbox: vec![EMPTY_BOX; n],
            dbox_notext: vec![EMPTY_BOX; n],
            dmembers: vec![0; n],
            dmask: vec![vec![0u8; bw]; n],
            dtmask: vec![vec![0u8; bw]; n],
            edges: vec![Vec::new(); n],
            lrecs: vec![0; nl],
            lmems: vec![0; nl],
            rbb: vec![None; n],
            rbb_notext: vec![None; n],
        };
        for (ci, cell) in doc.cells.iter().enumerate() {
            let mut shape = |li: usize, m: u64, bb: Box4| {
                box_grow(&mut r.dbox[ci], &bb);
                box_grow(&mut r.dbox_notext[ci], &bb);
                r.dmask[ci][li / 8] |= 1 << (li % 8);
                r.dmembers[ci] += m;
                r.lrecs[li] += 1;
                r.lmems[li] += m;
            };
            for x in &cell.rects {
                let rp = rep_of(&x.rep);
                shape(lidx[&(x.layer, x.dt)], rp.members, rep_box(x.x, x.y, x.x + x.w, x.y + x.h, &rp));
            }
            for p in &cell.polys {
                let rp = rep_of(&p.rep);
                shape(lidx[&(p.layer, p.dt)], rp.members, rep_box_pts(&p.pts, 0, 0, &rp));
            }
            for p in &cell.paths {
                let rp = rep_of(&p.rep);
                let b = path_bbox(&p.pts, p.hw, p.es, p.ee);
                shape(lidx[&(p.layer, p.dt)], rp.members, rep_box(b.0, b.1, b.2, b.3, &rp));
            }
            for t in &cell.texts {
                let rp = rep_of(&t.rep);
                let li = lidx[&(t.layer, t.dt)];
                box_grow(&mut r.dbox[ci], &rep_box(t.x, t.y, t.x, t.y, &rp));
                r.dmask[ci][li / 8] |= 1 << (li % 8);
                r.dtmask[ci][li / 8] |= 1 << (li % 8);
            }
            for pl in &cell.places {
                let rp = rep_of(&pl.rep);
                let o = (pl.rot & 3) as usize | ((pl.flip as usize) << 2);
                let bb = rep_box(pl.x, pl.y, pl.x, pl.y, &rp);
                match r.edges[ci].iter_mut().find(|e| e.child == pl.cell) {
                    Some(e) => {
                        e.placements += 1;
                        e.members += rp.members;
                        box_grow(&mut e.obox[o], &bb);
                    }
                    None => {
                        let mut obox = [EMPTY_BOX; 8];
                        obox[o] = bb;
                        r.edges[ci].push(Edge { child: pl.cell, placements: 1, members: rp.members, obox });
                    }
                }
            }
        }
        // recursive boxes: memoized recursion over the placements
        fn walk(doc: &Doc, ci: usize, memo: &mut Vec<Option<Option<Box4>>>, own: &[Box4]) -> Option<Box4> {
            if let Some(b) = memo[ci] {
                return b;
            }
            let mut b = if box_is_empty(&own[ci]) { None } else { Some(own[ci]) };
            let places = doc.cells[ci].places.clone();
            for pl in &places {
                if let Some(cb) = walk(doc, pl.cell, memo, own) {
                    let rp = rep_of(&pl.rep);
                    let rb = orient_box(pl.rot, pl.flip, cb);
                    let ob = rep_box(pl.x, pl.y, pl.x, pl.y, &rp);
                    grow_opt(&mut b, (rb.0 + ob.0, rb.1 + ob.1, rb.2 + ob.2, rb.3 + ob.3));
                }
            }
            memo[ci] = Some(b);
            b
        }
        for texts in [true, false] {
            let own = if texts { r.dbox.clone() } else { r.dbox_notext.clone() };
            let mut memo = vec![None; n];
            for ci in 0..n {
                walk(doc, ci, &mut memo, &own);
            }
            let out: Vec<Option<Box4>> = memo.into_iter().map(|m| m.unwrap()).collect();
            if texts {
                r.rbb = out;
            } else {
                r.rbb_notext = out;
            }
        }
        r
    }

    fn check_against_doc(data: &[u8], jobs: usize) -> Inventory {
        let inv = crate::inventory(data, jobs).expect("inventory");
        let doc = parse_doc(data).expect("parse_doc");
        let names: Vec<&str> = doc.cells.iter().map(|c| c.name.as_str()).collect();
        let inames: Vec<&str> = inv.cells.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(inames, names, "cell table (jobs {})", jobs);
        assert_eq!(inv.top, doc.top);
        assert_eq!(inv.unit(), doc.unit);
        assert_eq!(inv.layer_order, doc.layer_order, "layer order");
        assert_eq!(inv.layer_names, doc.layer_names);
        assert_eq!(inv.layer_aliases, doc.layer_aliases);
        let r = reference(&doc);
        for ci in 0..doc.cells.len() {
            let c = &inv.cells[ci];
            let nm = &c.name;
            assert_eq!(c.dbox, r.dbox[ci], "dbox {}", nm);
            assert_eq!(c.dbox_notext, r.dbox_notext[ci], "dbox_notext {}", nm);
            assert_eq!(c.dmembers, r.dmembers[ci], "dmembers {}", nm);
            assert_eq!(c.dmask, r.dmask[ci], "dmask {}", nm);
            assert_eq!(c.dtmask, r.dtmask[ci], "dtmask {}", nm);
            assert_eq!(c.edges, r.edges[ci], "edges {}", nm);
            assert_eq!(inv.rbb[ci], r.rbb[ci], "rbb {}", nm);
            assert_eq!(inv.rbb_notext[ci], r.rbb_notext[ci], "rbb_notext {}", nm);
            assert_eq!(c.placements, doc.cells[ci].places.len() as u64, "placements {}", nm);
            assert_eq!(c.texts, doc.cells[ci].texts.len() as u64, "texts {}", nm);
        }
        assert_eq!(inv.lrecs, r.lrecs);
        assert_eq!(inv.lmems, r.lmems);
        inv
    }

    #[test]
    fn adversarial_matches_doc() {
        for (tables_first, cblock) in [(false, false), (true, false), (false, true), (true, true)] {
            let data = adversarial(tables_first, cblock);
            let inv = check_against_doc(&data, 1);
            // the cell table: CELL records, then the undefined target
            let names: Vec<&str> = inv.cells.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(names, ["TOP", "MID", "LEAF", "LBL", "GHOST"]);
            assert_eq!(inv.top, 0);
            // layers in first appearance: TOP's 1/0, 5/0 (text), 2/0
            // (paths), 3/0, 7/0 (the degenerate trapezoid still
            // registers), 4/0 (circle; the r=0 circle's 9/0 does not),
            // MID's 1/1, LEAF's 6/0, LBL's 5/1, then LAYERNAME's 8/0
            // (a LAYERNAME table before the cells registers its exact pairs
            // first - (1,0) and (8,0) - as the Doc does)
            let expect: &[(u32, u32)] = if tables_first {
                &[(1, 0), (8, 0), (5, 0), (2, 0), (3, 0), (7, 0), (4, 0), (1, 1), (6, 0), (5, 1)]
            } else {
                &[(1, 0), (5, 0), (2, 0), (3, 0), (7, 0), (4, 0), (1, 1), (6, 0), (5, 1), (8, 0)]
            };
            assert_eq!(inv.layer_order, expect);
            assert_eq!(inv.layer_names.get(&(1, 0)).map(|s| s.as_str()), Some("METAL"));
            assert_eq!(inv.layer_names.get(&(2, 0)).map(|s| s.as_str()), Some("VIA"));
            assert_eq!(inv.layer_names.get(&(3, 0)).map(|s| s.as_str()), Some("VIA"));
            assert_eq!(inv.layer_names.get(&(8, 0)).map(|s| s.as_str()), Some("UNUSED"));
            // 7/0 and 8/0 carry no record
            let li = |l: u32, d: u32| inv.layer_order.iter().position(|&k| k == (l, d)).unwrap();
            assert_eq!((inv.lrecs[li(7, 0)], inv.lrecs[li(8, 0)]), (0, 0));
            assert!(inv.lrecs[li(1, 0)] >= 3 && inv.lmems[li(1, 0)] > inv.lrecs[li(1, 0)]);
            // TOP's children in first-placement order: MID, LEAF, GHOST
            let top = &inv.cells[0];
            let kids: Vec<usize> = top.edges.iter().map(|e| e.child).collect();
            assert_eq!(kids, [1, 2, 4]);
            // MID: rot 1 (3 members along x) and rot 3 + flip
            let mid = &top.edges[0];
            assert_eq!((mid.placements, mid.members), (2, 4));
            assert_eq!(mid.obox[1], (1000, 500, 11000, 500));
            assert_eq!(mid.obox[3 | 4], (-300, 700, -300, 700));
            assert!(box_is_empty(&mid.obox[0]));
            // GHOST is empty: no box, height 0, no members
            assert_eq!(inv.rbb[4], None);
            assert_eq!((inv.height[4], inv.rmembers[4]), (0, 0));
            // LBL is text-only: a box with texts, none without
            assert!(inv.rbb[3].is_some() && inv.rbb_notext[3].is_none());
            assert_ne!(inv.rtmask[1], vec![0u8; inv.mask_bytes()]);
            // heights: TOP 2 (MID -> LEAF/LBL), MID 1, leaves 0
            assert_eq!(inv.height, [2, 1, 0, 0, 0]);
            // ranks: parents before children along every edge
            for (ci, c) in inv.cells.iter().enumerate() {
                for e in &c.edges {
                    assert!(inv.rank[ci] < inv.rank[e.child], "rank {} -> {}", ci, e.child);
                }
            }
            // ranges: one visible CELL record each (LEAF two), GHOST
            // none; seeds count the implicit CELLNAME (2) and TEXTSTRING
            // (1) records before the cell
            for ci in 0..4 {
                let c = &inv.cells[ci];
                assert_eq!(c.ranges.len(), if ci == 2 { 2 } else { 1 }, "{}", c.name);
                let starts: Vec<u64> = c.ranges.iter().map(|r| r.start).collect();
                assert_eq!(c.def_ranges, starts, "{}", c.name);
                let rg = &c.ranges[0];
                assert!(rg.start < rg.end);
                assert_eq!(data[rg.start as usize], if ci == 1 { 14 } else { 13 }, "{}", c.name);
                let seeds = if tables_first { (2, 1) } else { (0, 0) };
                assert_eq!((rg.cellname_base, rg.textstring_base), seeds);
            }
            assert!(inv.cells[4].ranges.is_empty());
            // consecutive ranges tile the cell section; LEAF's second
            // definition follows LBL
            assert_eq!(inv.cells[0].ranges[0].end, inv.cells[1].ranges[0].start);
            assert_eq!(inv.cells[2].ranges[0].end, inv.cells[3].ranges[0].start);
            assert_eq!(inv.cells[3].ranges[0].end, inv.cells[2].ranges[1].start);
            assert!(inv.cells[4].def_ranges.is_empty());
            if cblock {
                assert!(inv.scan.cblocks >= 1);
            }
            // the parallel scan, cells split over 2 and 3 workers,
            // reports the same inventory
            for jobs in [2, 3] {
                let par = check_against_doc(&data, jobs);
                assert_eq!(par.topo, inv.topo);
                assert_eq!(par.rank, inv.rank);
                assert_eq!(par.height, inv.height);
                assert_eq!(par.rmembers, inv.rmembers);
                assert_eq!(par.rmask, inv.rmask);
                assert_eq!(par.rtmask, inv.rtmask);
                assert_eq!(par.rbb, inv.rbb);
                assert_eq!(par.rbb_notext, inv.rbb_notext);
                for (a, b) in par.cells.iter().zip(&inv.cells) {
                    assert_eq!(a.ranges, b.ranges, "{}", a.name);
                    assert_eq!((a.records, a.doc_bytes), (b.records, b.doc_bytes));
                }
            }
        }
    }

    /// every range of the file through the cell parser (the head first,
    /// then the cells' ranges in file order), the pieces of a cell
    /// joined in file order and normalized on their own
    fn parse_all_cells(data: &[u8], inv: &Inventory, jobs: usize) -> Vec<crate::doc::Cell> {
        use crate::doc::{normalize_cells, parse_cell_range, Cell, CellTables};
        let cix = inv.cell_index();
        let tables = CellTables::new(&inv.cellnames, &inv.textstrings, &cix);
        let mut ranges: Vec<&CellRange> = vec![&inv.head];
        for c in &inv.cells {
            for r in &c.ranges {
                ranges.push(r);
            }
        }
        ranges.sort_by_key(|r| r.start);
        let n = inv.cells.len();
        let mut pieces: Vec<Vec<Cell>> = (0..n).map(|_| Vec::new()).collect();
        for r in ranges {
            for p in parse_cell_range(data, r, &tables).expect("parse range") {
                pieces[p.index].push(p.cell);
            }
        }
        pieces
            .into_iter()
            .enumerate()
            .map(|(ci, ps)| {
                let ic = &inv.cells[ci];
                // a defined cell has pieces, an undefined target none (a
                // range folds its CELL records of one cell into one piece,
                // so the count is not the record count)
                assert_eq!(ps.is_empty(), ic.def_ranges.is_empty(), "pieces of {}", ic.name);
                let mut cell = Cell { name: ic.name.clone(), ..Default::default() };
                for p in ps {
                    cell.rects.extend(p.rects);
                    cell.polys.extend(p.polys);
                    cell.paths.extend(p.paths);
                    cell.places.extend(p.places);
                    cell.texts.extend(p.texts);
                }
                normalize_cells(std::slice::from_mut(&mut cell), jobs);
                cell
            })
            .collect()
    }

    /// G2: the cell parser's cells equal the whole-Doc parse's, record by
    /// record, on every fixture (late and early tables, CBLOCK bodies, a
    /// cell defined inside the head's CBLOCK, by-reference and by-name
    /// placements, the undefined target, text references)
    #[test]
    fn cell_parser_matches_doc() {
        let mut fixtures: Vec<Vec<u8>> = Vec::new();
        for (tables_first, cblock) in [(false, false), (true, false), (false, true), (true, true)] {
            fixtures.push(adversarial(tables_first, cblock));
        }
        fixtures.push(head_cblock_cell());
        fixtures.push(dup_in_range());
        for data in &fixtures {
            let doc = parse_doc(data).expect("parse_doc");
            for jobs in [1, 2] {
                let inv = crate::inventory(data, jobs).expect("inventory");
                let cells = parse_all_cells(data, &inv, jobs);
                assert_eq!(cells.len(), doc.cells.len());
                for (ci, c) in cells.iter().enumerate() {
                    assert_eq!(c, &doc.cells[ci], "cell {} (jobs {})", ci, jobs);
                }
            }
        }
        // the head range of a tables-first stream holds the tables and no cell
        let data = adversarial(true, false);
        let inv = crate::inventory(&data, 1).expect("inventory");
        assert!(inv.head.end > inv.head.start);
        let cix = inv.cell_index();
        let tables = crate::doc::CellTables::new(&inv.cellnames, &inv.textstrings, &cix);
        assert!(crate::doc::parse_cell_range(&data, &inv.head, &tables).expect("head").is_empty());
        assert_eq!(inv.textstrings.get(&0).map(|s| s.as_str()), Some("lbl"));
        assert_eq!(inv.cellnames.get(&2).map(|s| s.as_str()), Some("MID"));
    }

    /// a cell defined inside the head's CBLOCK: the parallel scan's
    /// per-cell stats stay aligned with the inventory parts
    #[test]
    fn head_cblock_cell_keeps_stats_aligned() {
        let data = head_cblock_cell();
        let seq = check_against_doc(&data, 1);
        let par = check_against_doc(&data, 2);
        let names: Vec<&str> = seq.cells.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["H", "TOP"]);
        assert_eq!(seq.top, 1);
        for (a, b) in seq.cells.iter().zip(&par.cells) {
            assert_eq!(
                (a.records, a.shapes, a.placements, a.texts, a.doc_bytes),
                (b.records, b.shapes, b.placements, b.texts, b.doc_bytes),
                "{}",
                a.name
            );
        }
        assert_eq!((seq.cells[0].texts, seq.cells[1].placements), (1, 1));
        assert_eq!((seq.cells[0].cblock_defs, seq.cells[0].ranges.len()), (1, 0));
        assert_eq!(seq.cells[0].def_ranges, vec![seq.head.start]);
        assert_eq!(seq.cells[1].ranges.len(), 1);
        // a cell re-opened inside its own range's CBLOCK: one range holds
        // both CELL records
        let data = dup_in_range();
        let inv = check_against_doc(&data, 1);
        assert_eq!((inv.cells[0].ranges.len(), inv.cells[0].cblock_defs), (1, 1));
        assert_eq!(inv.cells[0].def_ranges, vec![inv.cells[0].ranges[0].start]);
        // three rectangles and the CBLOCK record (CELL records are not counted)
        assert_eq!((inv.cells[0].shapes, inv.cells[0].records), (3, 4));
    }

    #[test]
    fn refusals_match_the_geometry_parser() {
        // (fixture, whether parse_doc accepts it): the cyclic stream parses
        // (build() refuses it), a duplicate TEXTSTRING reference parses
        // (the inventory refuses it on purpose)
        for (data, doc_accepts) in [
            (cyclic(), true),
            (two_tops(), false),
            (with_record(18), false),
            (with_record(33), false),
            (text_refusal(0), false),
            (text_refusal(1), false),
            (text_refusal(2), false),
            (text_refusal(3), false),
            (text_refusal(4), true),
        ] {
            let inv = crate::inventory(&data, 1);
            let doc = parse_doc(&data);
            assert!(inv.is_err(), "inventory accepted what it must refuse");
            assert_eq!(doc.is_ok(), doc_accepts, "parse_doc on a refusal fixture");
        }
        let e = format!("{}", crate::inventory(&text_refusal(4), 1).err().unwrap());
        assert!(e.contains("duplicate TEXTSTRING reference"), "{}", e);
        let e = format!("{}", crate::inventory(&cyclic(), 1).err().unwrap());
        assert!(e.contains("cycle in hierarchy"), "{}", e);
        let e = format!("{}", crate::inventory(&two_tops(), 1).err().unwrap());
        assert!(e.contains("2 top cells"), "{}", e);
        let e = format!("{}", crate::inventory(&text_refusal(2), 1).err().unwrap());
        assert!(e.contains("text without string"), "{}", e);
        let e = format!("{}", crate::inventory(&text_refusal(3), 1).err().unwrap());
        assert!(e.contains("unresolved textstring ref"), "{}", e);
        // a RECTANGLE with S and H both set is spec-invalid, but doc.rs
        // reads H and takes h = w: the inventory agrees instead of refusing
        let mut w = crate::write::W::new();
        w.out.extend_from_slice(b"%SEMI-OASIS\r\n");
        w.uint(1);
        w.string(b"1.0");
        w.real_f64(1000.0);
        w.uint(0);
        for _ in 0..12 {
            w.uint(0);
        }
        w.uint(14);
        w.string(b"A");
        w.uint(20);
        w.byte(0x80 | 0x40 | 0x20 | 0x10 | 0x08 | 0x02 | 0x01);
        w.uint(1);
        w.uint(0);
        w.uint(10);
        w.uint(20);
        w.sint(0);
        w.sint(0);
        w.uint(2);
        w.uint(252);
        w.out.extend_from_slice(&[0u8; 252]);
        w.uint(0);
        let inv = check_against_doc(&w.out, 1);
        assert_eq!(inv.cells[0].dbox, (0, 0, 10, 10));
    }
}
