//! OVR1: bounded, display-only native point samples, independent of OVP pages.
//!
//! Count logical members bottom-up, then resolve only sampled member ranks
//! by streaming them down the hierarchy (see `build`). A trillion-member Grid
//! costs one count and at most the sample budget, never a trillion-member
//! walk, and a billion placement records cost a scan, never a directory
//! entry each. Groups preserve (layer, relative depth), so a depth-zero view
//! cannot accidentally display descendants. These are approximate existence
//! samples, NOT occupancy or query geometry.
use floe_oasis::doc::{Doc, PlaceRec, Rep};
use floe_ovm::{BBox, Backing, Ovm};
use std::collections::{BTreeMap, HashMap};

mod tree;
pub use tree::{Options as TreeOptions, Stream as TreeStream, write as write_tree};

pub const DEFAULT_POINTS: usize = 262_144;
pub const MAX_POINTS: usize = 4_194_304;
pub const FRAME_POINTS: usize = 262_144;
const CHUNK: usize = 128;
const MAX_GROUPS: usize = 65_536;
const MAX_DIRECTORY_GROUPS: usize = 67_108_864; // 24-byte (key, count) each: 1.5 GiB, excluding the Doc
const MAGIC: &[u8; 8] = b"FLOEOVR1";
/// OVR2 step 1 (docs/OVR2_DESIGN.ko.md): the same samples as 64-byte shapes
const MAGIC2: &[u8; 8] = b"FLOEOVR2";
type Key = (u32, u32, u32); // layer, datatype, relative depth

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i64,
    pub y: i64,
    pub max_dim: u64,
    pub min_dim: u64,
    pub rank: u32,
}
impl Point {
    fn bbox(self) -> BBox {
        BBox {
            x0: self.x,
            y0: self.y,
            x1: self.x,
            y1: self.y,
        }
    }
}
/// OVR2 (docs/OVR2_DESIGN.ko.md section 4): the sampled shape itself in
/// top coordinates, so a sub-cut hairline keeps its long axis on screen
/// (4, 3, 2, 1 px as the view widens) instead of collapsing to a dot.
/// Rect: the transformed corners (x0 <= x1, y0 <= y1). Segment: one real
/// boundary edge of a polygon or of a path's outline (the longest
/// non-degenerate one, the first on a tie; PRIM_PARTIAL says the shape has
/// more). Point: a shape with no usable edge (counted as a fallback).
/// `gate_dim` is the cut test of the ORIGINAL shape - min(max_dim, 2 *
/// min_dim) of its bbox, the plain thin:cull rule - never of the picked
/// edge. `rank` is the draw order, as in Point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prim {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
    pub gate_dim: u64,
    pub thickness: u64,
    pub kind: u8,
    pub flags: u8,
    pub rank: u32,
}
pub const PRIM_RECT: u8 = 0;
pub const PRIM_SEGMENT: u8 = 1;
pub const PRIM_POINT: u8 = 2;
pub const PRIM_PARTIAL: u8 = 1;
/// Runtime-only proxy flag: a union of subpixel hairlines stays solid even
/// when the union is wide enough to enter the ordinary rectangle fill path.
pub const PRIM_MERGED_SOLID: u8 = 2;
impl Prim {
    pub fn bbox(&self) -> BBox {
        BBox {
            x0: self.x0.min(self.x1),
            y0: self.y0.min(self.y1),
            x1: self.x0.max(self.x1),
            y1: self.y0.max(self.y1),
        }
    }
}
/// a record's primitive in cell coordinates, before the repetition offset
#[derive(Clone, Copy)]
struct Local {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
    kind: u8,
    flags: u8,
}
#[derive(Debug)]
pub struct Group {
    pub key: Key,
    pub members: u64,
    pub points: Vec<Point>,
    /// the same samples as shapes, in the same order (empty unless built
    /// with `build_with(.., true)`)
    pub prims: Vec<Prim>,
}
pub struct Built {
    pub groups: Vec<Group>,
    /// (layer, datatype, depth) groups over all reachable cells - the size of
    /// the count directory, the only structure proportional to the hierarchy
    /// (24 bytes each, MAX_DIRECTORY_GROUPS). Nothing is kept per placement.
    pub directory: usize,
    /// peak number of sample requests in flight during the resolve pass
    /// (64 bytes each; bounded by the sample count, not by the layout)
    pub peak_requests: usize,
    /// shapes that had no usable edge and were stored as a point (OVR2)
    pub point_fallbacks: u64,
}
/// what the resolve pass fills
struct Out {
    points: Vec<Vec<Option<Point>>>,
    prims: Option<Vec<Vec<Option<Prim>>>>,
    locals: HashMap<(usize, u8, u32), Local>,
    point_fallbacks: u64,
}
/// per cell: (layer, datatype, relative depth) -> logical members, sorted by key
type Counts = Vec<(Key, u64)>;

/// A sample on its way down the hierarchy. `group` (final top group) and
/// `sample` (draw order = the file's Point.rank) never change; `key` and
/// `rank` are relative to the cell holding the request; (m, tx, ty) is the
/// accumulated cell-to-top map p_top = m * p + t.
#[derive(Clone, Copy)]
struct Req {
    rank: u64,
    tx: i128,
    ty: i128,
    key: Key,
    group: u32,
    sample: u32,
    m: [i8; 4],
}
const IDENTITY: [i8; 4] = [1, 0, 0, 1];

// p_parent = R_rot(F_flip(p)) + (pl.x + off.x, pl.y + off.y), composed on the
// right of the accumulated map (M . T): the same flip, rotate, translate order
// and i128 arithmetic as applying the placement chain bottom-up.
fn place_xf(m: [i8; 4], tx: i128, ty: i128, pl: &PlaceRec, off: (i128, i128)) -> ([i8; 4], i128, i128) {
    let f: i8 = if pl.flip { -1 } else { 1 };
    let (c, s): (i8, i8) = match pl.rot & 3 {
        0 => (1, 0),
        1 => (0, 1),
        2 => (-1, 0),
        _ => (0, -1),
    };
    let t = [c, -s * f, s, c * f];
    let nm = [
        m[0] * t[0] + m[1] * t[2],
        m[0] * t[1] + m[1] * t[3],
        m[2] * t[0] + m[3] * t[2],
        m[2] * t[1] + m[3] * t[3],
    ];
    let (px, py) = (pl.x as i128 + off.0, pl.y as i128 + off.1);
    (
        nm,
        m[0] as i128 * px + m[1] as i128 * py + tx,
        m[2] as i128 * px + m[3] as i128 * py + ty,
    )
}
fn apply_xf(m: [i8; 4], tx: i128, ty: i128, x: i128, y: i128) -> (i128, i128) {
    (
        m[0] as i128 * x + m[1] as i128 * y + tx,
        m[2] as i128 * x + m[3] as i128 * y + ty,
    )
}

fn add_count(own: &mut BTreeMap<Key, u64>, key: Key, n: u64) -> Result<(), String> {
    if n == 0 {
        return Ok(());
    }
    let e = own.entry(key).or_insert(0);
    *e = e
        .checked_add(n)
        .ok_or("representatives: recursive member count exceeds u64")?;
    Ok(())
}


fn members(rep: &Rep) -> Result<u64, String> {
    match rep {
        Rep::One => Ok(1),
        Rep::Grid { na, nb, .. } => na
            .checked_mul(*nb)
            .ok_or_else(|| "representatives: repetition count exceeds u64".into()),
        Rep::Pts(pts) => Ok(pts.len() as u64),
    }
}
fn offset(rep: &Rep, rank: u64) -> (i128, i128) {
    match rep {
        Rep::One => (0, 0),
        Rep::Grid { na, va, vb, .. } => {
            let (a, b) = ((rank % na) as i128, (rank / na) as i128);
            (
                a * va.0 as i128 + b * vb.0 as i128,
                a * va.1 as i128 + b * vb.1 as i128,
            )
        }
        Rep::Pts(pts) => {
            let p = pts[rank as usize];
            (p.0 as i128, p.1 as i128)
        }
    }
}
// Iterative postorder: no call-stack dependence on source hierarchy depth.
fn postorder(doc: &Doc) -> Result<Vec<usize>, String> {
    if doc.top >= doc.cells.len() {
        return Err("representatives: invalid top".into());
    }
    let mut states = vec![0u8; doc.cells.len()];
    let mut stack = vec![(doc.top, 0usize)];
    let mut order = Vec::new();
    states[doc.top] = 1;
    while let Some((ci, next)) = stack.last_mut() {
        if *next == doc.cells[*ci].places.len() {
            states[*ci] = 2;
            order.push(*ci);
            stack.pop();
        } else {
            let child = doc.cells[*ci].places[*next].cell;
            *next += 1;
            match states.get(child) {
                Some(0) => {
                    states[child] = 1;
                    stack.push((child, 0));
                }
                Some(1) => return Err("representatives: cyclic cell hierarchy".into()),
                Some(2) => (),
                _ => return Err("representatives: invalid child".into()),
            }
        }
    }
    Ok(order)
}

// A permutation of the low-bit domain (each multiplication is odd and each
// xor-shift is invertible). Unlike a simple rank stride it does not select
// just one column of a power-of-two Grid. Rejection handles non-power sizes.
fn permute(mut x: u64, mask: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15) & mask;
    x ^= x >> 17;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9) & mask;
    x ^= x >> 11;
    x = x.wrapping_mul(0x94d049bb133111eb) & mask;
    (x ^ (x >> 23)) & mask
}

// A per-group cap and a global cap, with proportional water filling after a
// small floor. This never lets one dense layer erase every sparse layer.
fn quotas(counts: &[u64], cap: usize, total_cap: usize) -> Result<Vec<usize>, String> {
    if counts.len() > total_cap {
        return Err("representatives: too many groups for point budget".into());
    }
    let want: Vec<usize> = counts.iter().map(|&n| n.min(cap as u64) as usize).collect();
    if want.iter().sum::<usize>() <= total_cap {
        return Ok(want);
    }
    let floor = (total_cap / counts.len()).min(64);
    let mut out: Vec<usize> = want.iter().map(|&n| n.min(floor)).collect();
    let mut left = total_cap - out.iter().sum::<usize>();
    while left > 0 {
        let weight: u128 = counts
            .iter()
            .zip(&out)
            .zip(&want)
            .filter(|((_, a), b)| a < b)
            .map(|((&n, _), _)| n as u128)
            .sum();
        if weight == 0 {
            break;
        }
        let round = left;
        for i in 0..out.len() {
            if out[i] < want[i] {
                let n = ((round as u128 * counts[i] as u128 / weight) as usize)
                    .max(1)
                    .min(want[i] - out[i])
                    .min(left);
                out[i] += n;
                left -= n;
            }
        }
    }
    Ok(out)
}

/// Two passes over the parsed Doc, neither of which stores anything per
/// placement record (the 0.12.154 member directory did, and MAIN01's
/// placements alone exceeded its 2 GiB).
///
/// 1. count (bottom-up): logical members per (layer, datatype, depth) group
///    per cell. Placements are aggregated per distinct child first - counts
///    commute - so the cost is records + sum(distinct children x child
///    groups), and the memory is the directory (cells x groups).
/// 2. resolve (top-down, reverse postorder): the sample ranks drawn at top
///    become requests that move down the hierarchy. A cell is scanned once
///    for all requests it received from all its parents; one traversal of
///    its records in the count order (rects, polygons, paths, placements)
///    advances a cursor per requested group, forwards the ranks that fall
///    into a placement to the child (rank / n = instance, rank % n = inner
///    rank, transform composed), and resolves the ones that fall on a shape.
///    Requests are moved, and a scanned cell's buffer is dropped.
///
/// Cost: count = O(records + sum distinct children x child groups); resolve
/// = O(records of the scanned cells + sum over scanned cells of distinct
/// children x child groups + samples x depth + sorting). Memory: directory +
/// samples in flight (64 bytes each, peak logged) + points + the Doc.
pub fn build(doc: &Doc, per_group: usize, progress: Option<fn(&str)>) -> Result<Built, String> {
    build_with(doc, per_group, progress, false)
}

/// `shapes`: also resolve every sample to its Prim (OVR2). The samples,
/// their order and the points are the same either way.
pub fn build_with(doc: &Doc, per_group: usize, progress: Option<fn(&str)>, shapes: bool) -> Result<Built, String> {
    if per_group == 0 || per_group > MAX_POINTS {
        return Err(format!(
            "representatives: points must be in 1..={MAX_POINTS}"
        ));
    }
    let log = |s: &str| {
        if let Some(f) = progress {
            f(s)
        }
    };
    let order = postorder(doc)?;
    let started = std::time::Instant::now();
    let mut counts: Vec<Counts> = vec![Vec::new(); doc.cells.len()];
    let mut directory = 0usize;
    let mut heartbeat = std::time::Instant::now();
    for &ci in &order {
        let cell = &doc.cells[ci];
        let mut own = BTreeMap::new();
        for r in &cell.rects {
            add_count(&mut own, (r.layer, r.dt, 0), members(&r.rep)?)?;
        }
        for p in cell.polys.iter().filter(|p| !p.pts.is_empty()) {
            add_count(&mut own, (p.layer, p.dt, 0), members(&p.rep)?)?;
        }
        for p in cell.paths.iter().filter(|p| !p.pts.is_empty()) {
            add_count(&mut own, (p.layer, p.dt, 0), members(&p.rep)?)?;
        }
        let mut per_child: HashMap<usize, u64> = HashMap::new();
        for pl in &cell.places {
            let n = members(&pl.rep)?;
            let e = per_child.entry(pl.cell).or_insert(0);
            *e = e
                .checked_add(n)
                .ok_or("representatives: recursive member count exceeds u64")?;
        }
        for (&child, &n) in &per_child {
            for &((l, d, depth), m) in &counts[child] {
                let depth = depth
                    .checked_add(1)
                    .filter(|&d| d <= 4096)
                    .ok_or("representatives: hierarchy depth exceeds 4096")?;
                add_count(
                    &mut own,
                    (l, d, depth),
                    n.checked_mul(m)
                        .ok_or("representatives: recursive member count exceeds u64")?,
                )?;
            }
        }
        directory += own.len();
        if directory > MAX_DIRECTORY_GROUPS {
            return Err(format!(
                "representatives: group directory exceeds {MAX_DIRECTORY_GROUPS} groups"
            ));
        }
        counts[ci] = own.into_iter().collect();
        if heartbeat.elapsed().as_secs() >= 10 {
            log(&format!("count cell={ci} directory={directory}"));
            heartbeat = std::time::Instant::now();
        }
    }
    log(&format!(
        "count directory={directory} groups ({:.0} MiB) in {:.1}s",
        directory as f64 * 24.0 / 1_048_576.0,
        started.elapsed().as_secs_f64()
    ));
    let root = &counts[doc.top];
    if root.len() > MAX_GROUPS {
        return Err("representatives: top group limit exceeded".into());
    }
    let totals: Vec<u64> = root.iter().map(|&(_, m)| m).collect();
    let budgets = quotas(&totals, per_group, MAX_POINTS)?;
    // the sample ranks of every top group, drawn as before (permutation,
    // rejection); each becomes a request that starts at top
    let mut pending: Vec<Vec<Req>> = (0..doc.cells.len()).map(|_| Vec::new()).collect();
    let mut out = Out {
        points: Vec::with_capacity(root.len()),
        prims: if shapes { Some(Vec::with_capacity(root.len())) } else { None },
        locals: HashMap::new(),
        point_fallbacks: 0,
    };
    for (gi, (&(key, members), count)) in root.iter().zip(budgets).enumerate() {
        let mask = members
            .checked_next_power_of_two()
            .unwrap_or(0)
            .wrapping_sub(1);
        let mut candidate = 0u64;
        let mut drawn = 0usize;
        while drawn < count {
            let rank = permute(candidate, mask);
            candidate += 1;
            if candidate > (count as u64).saturating_mul(64).saturating_add(128) {
                return Err("representatives: sampling work limit exceeded".into());
            }
            if rank >= members {
                continue;
            }
            pending[doc.top].push(Req {
                rank,
                tx: 0,
                ty: 0,
                key,
                group: gi as u32,
                sample: drawn as u32,
                m: IDENTITY,
            });
            drawn += 1;
        }
        out.points.push(vec![None; count]);
        if let Some(prims) = out.prims.as_mut() {
            prims.push(vec![None; count]);
        }
    }
    let started = std::time::Instant::now();
    let mut in_flight = pending[doc.top].len();
    let mut peak = in_flight;
    let mut anchors = HashMap::new();
    let mut scanned = 0usize;
    let mut heartbeat = std::time::Instant::now();
    for &ci in order.iter().rev() {
        let reqs = std::mem::take(&mut pending[ci]);
        if reqs.is_empty() {
            continue;
        }
        in_flight -= reqs.len();
        in_flight += resolve_cell(doc, &counts, ci, reqs, &mut pending, &mut out, &mut anchors)?;
        peak = peak.max(in_flight);
        scanned += 1;
        if heartbeat.elapsed().as_secs() >= 10 {
            log(&format!("resolve cells={scanned} in flight={in_flight}"));
            heartbeat = std::time::Instant::now();
        }
    }
    if in_flight != 0 {
        return Err("representatives: samples left unresolved (internal)".into());
    }
    log(&format!(
        "resolve cells={scanned} peak requests={peak} ({:.0} MiB) in {:.1}s",
        peak as f64 * 64.0 / 1_048_576.0,
        started.elapsed().as_secs_f64()
    ));
    let Out { points, prims, point_fallbacks, .. } = out;
    let mut prims = prims.map(|p| p.into_iter());
    let mut result = Vec::with_capacity(root.len());
    for (&(key, members), pts) in root.iter().zip(points) {
        let mut done = Vec::with_capacity(pts.len());
        for p in pts {
            done.push(p.ok_or("representatives: sample not resolved (internal)")?);
        }
        let mut shaped = Vec::new();
        if let Some(it) = prims.as_mut() {
            for p in it.next().ok_or("representatives: shape group missing (internal)")? {
                shaped.push(p.ok_or("representatives: sample shape not resolved (internal)")?);
            }
        }
        log(&format!(
            "{}/{} depth={} members={} points={}",
            key.0,
            key.1,
            key.2,
            members,
            done.len()
        ));
        result.push(Group {
            key,
            members,
            points: done,
            prims: shaped,
        });
    }
    Ok(Built {
        groups: result,
        directory,
        peak_requests: peak,
        point_fallbacks,
    })
}

/// requests of one group inside one cell: reqs[next..end] in rank order,
/// `pos` = members of the group counted so far in the scan
struct GroupCursor {
    next: usize,
    end: usize,
    pos: u64,
}

/// One scan of cell `ci` for all its requests. Returns the number forwarded
/// to children (pushed onto `pending`).
fn resolve_cell(
    doc: &Doc,
    counts: &[Counts],
    ci: usize,
    mut reqs: Vec<Req>,
    pending: &mut [Vec<Req>],
    out: &mut Out,
    anchors: &mut HashMap<(usize, u8, u32), Point>,
) -> Result<usize, String> {
    reqs.sort_unstable_by_key(|r| (r.key, r.rank, r.sample));
    let mut cursors: Vec<GroupCursor> = Vec::new();
    let mut key_of: HashMap<Key, usize> = HashMap::new();
    let mut i = 0;
    while i < reqs.len() {
        let key = reqs[i].key;
        let mut j = i;
        while j < reqs.len() && reqs[j].key == key {
            j += 1;
        }
        key_of.insert(key, cursors.len());
        cursors.push(GroupCursor { next: i, end: j, pos: 0 });
        i = j;
    }
    let cell = &doc.cells[ci];
    // own shapes, in the count order
    for (ri, r) in cell.rects.iter().enumerate() {
        if let Some(&c) = key_of.get(&(r.layer, r.dt, 0)) {
            hit_shape(doc, ci, 0, ri, &r.rep, &mut cursors[c], &reqs, out, anchors)?;
        }
    }
    for (ri, p) in cell.polys.iter().enumerate().filter(|(_, p)| !p.pts.is_empty()) {
        if let Some(&c) = key_of.get(&(p.layer, p.dt, 0)) {
            hit_shape(doc, ci, 1, ri, &p.rep, &mut cursors[c], &reqs, out, anchors)?;
        }
    }
    for (ri, p) in cell.paths.iter().enumerate().filter(|(_, p)| !p.pts.is_empty()) {
        if let Some(&c) = key_of.get(&(p.layer, p.dt, 0)) {
            hit_shape(doc, ci, 2, ri, &p.rep, &mut cursors[c], &reqs, out, anchors)?;
        }
    }
    // placements in record order; per distinct child, the child groups that
    // have requests in this cell (child members, cursor, child key) - so a
    // placement whose child has none costs one lookup
    let mut forwarded = 0usize;
    let mut child_active: HashMap<usize, Vec<(u64, usize, Key)>> = HashMap::new();
    for pl in &cell.places {
        let acts = child_active.entry(pl.cell).or_insert_with(|| {
            counts[pl.cell]
                .iter()
                .filter_map(|&((l, d, dep), m)| {
                    key_of.get(&(l, d, dep + 1)).map(|&c| (m, c, (l, d, dep)))
                })
                .collect()
        });
        if acts.is_empty() {
            continue;
        }
        let n_pl = members(&pl.rep)?;
        for &(n_child, c, child_key) in acts.iter() {
            let span = n_pl
                .checked_mul(n_child)
                .ok_or("representatives: recursive member count exceeds u64")?;
            let cur = &mut cursors[c];
            let limit = cur
                .pos
                .checked_add(span)
                .ok_or("representatives: recursive member count exceeds u64")?;
            while cur.next < cur.end && reqs[cur.next].rank < limit {
                let req = reqs[cur.next];
                let r = req.rank - cur.pos;
                let (m, tx, ty) = place_xf(req.m, req.tx, req.ty, pl, offset(&pl.rep, r / n_child));
                pending[pl.cell].push(Req {
                    rank: r % n_child,
                    tx,
                    ty,
                    key: child_key,
                    m,
                    ..req
                });
                forwarded += 1;
                cur.next += 1;
            }
            cur.pos = limit;
        }
    }
    if cursors.iter().any(|c| c.next != c.end) {
        return Err("representatives: sample rank beyond the counted members (internal)".into());
    }
    Ok(forwarded)
}

/// A shape record of `n` members in the scan: the requests whose rank falls
/// into it become points (anchor + repetition offset, then the cell-to-top map).
#[allow(clippy::too_many_arguments)]
fn hit_shape(
    doc: &Doc,
    ci: usize,
    kind: u8,
    ri: usize,
    rep: &Rep,
    cur: &mut GroupCursor,
    reqs: &[Req],
    out: &mut Out,
    anchors: &mut HashMap<(usize, u8, u32), Point>,
) -> Result<(), String> {
    let n = members(rep)?;
    let limit = cur
        .pos
        .checked_add(n)
        .ok_or("representatives: recursive member count exceeds u64")?;
    while cur.next < cur.end && reqs[cur.next].rank < limit {
        let req = reqs[cur.next];
        let base = anchor(doc, ci, kind, ri, anchors)?;
        let off = offset(rep, req.rank - cur.pos);
        let (x, y) = apply_xf(req.m, req.tx, req.ty, base.x as i128 + off.0, base.y as i128 + off.1);
        out.points[req.group as usize][req.sample as usize] = Some(Point {
            x: x
                .try_into()
                .map_err(|_| "representatives: x coordinate overflow")?,
            y: y
                .try_into()
                .map_err(|_| "representatives: y coordinate overflow")?,
            max_dim: base.max_dim,
            min_dim: base.min_dim,
            rank: req.sample,
        });
        if out.prims.is_some() {
            let local = local_prim(doc, ci, kind, ri, out)?;
            let a = apply_xf(req.m, req.tx, req.ty, local.x0 as i128 + off.0, local.y0 as i128 + off.1);
            let z = apply_xf(req.m, req.tx, req.ty, local.x1 as i128 + off.0, local.y1 as i128 + off.1);
            let coord = |v: i128| -> Result<i64, String> {
                v.try_into().map_err(|_| "representatives: shape coordinate overflow".to_string())
            };
            let (ax, ay, zx, zy) = (coord(a.0)?, coord(a.1)?, coord(z.0)?, coord(z.1)?);
            // a rect keeps x0 <= x1, y0 <= y1 under any rotation or mirror
            let (x0, y0, x1, y1) = if local.kind == PRIM_RECT {
                (ax.min(zx), ay.min(zy), ax.max(zx), ay.max(zy))
            } else {
                (ax, ay, zx, zy)
            };
            let prims = out.prims.as_mut().expect("shapes requested");
            prims[req.group as usize][req.sample as usize] = Some(Prim {
                x0,
                y0,
                x1,
                y1,
                gate_dim: base.max_dim.min(base.min_dim.saturating_mul(2)),
                thickness: 0,
                kind: local.kind,
                flags: local.flags,
                rank: req.sample,
            });
        }
        cur.next += 1;
    }
    cur.pos = limit;
    Ok(())
}

/// The record's primitive in cell coordinates, cached per record: a rect
/// as itself; a polygon as its longest non-degenerate boundary edge; a path
/// as the longest edge of the outline the raster paints (path_outline_any).
/// A shape without such an edge degrades to a point on it and is counted.
fn local_prim(doc: &Doc, ci: usize, kind: u8, ri: usize, out: &mut Out) -> Result<Local, String> {
    let record: u32 = ri
        .try_into()
        .map_err(|_| "representatives: record index exceeds u32")?;
    if let Some(cached) = out.locals.get(&(ci, kind, record)) {
        return Ok(*cached);
    }
    fn longest_edge(pts: &[(i64, i64)]) -> Option<((i64, i64), (i64, i64))> {
        let mut best: Option<(u128, (i64, i64), (i64, i64))> = None;
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            let (dx, dy) = ((b.0 as i128 - a.0 as i128), (b.1 as i128 - a.1 as i128));
            let len = (dx * dx + dy * dy) as u128;
            if len > 0 && best.is_none_or(|(l, _, _)| len > l) {
                best = Some((len, a, b));
            }
        }
        best.map(|(_, a, b)| (a, b))
    }
    let cell = &doc.cells[ci];
    let mut fallback = false;
    let local = match kind {
        0 => {
            let r = &cell.rects[ri];
            let x1 = r.x.checked_add(r.w).ok_or("representatives: rect x overflow")?;
            let y1 = r.y.checked_add(r.h).ok_or("representatives: rect y overflow")?;
            Local { x0: r.x.min(x1), y0: r.y.min(y1), x1: r.x.max(x1), y1: r.y.max(y1), kind: PRIM_RECT, flags: 0 }
        }
        _ => {
            let outline;
            let pts: &[(i64, i64)] = if kind == 1 {
                &cell.polys[ri].pts
            } else {
                let p = &cell.paths[ri];
                outline = floe_tiler::path_outline_any(&p.pts, p.hw, p.es, p.ee).unwrap_or_default();
                &outline
            };
            match longest_edge(pts) {
                Some((a, b)) => Local { x0: a.0, y0: a.1, x1: b.0, y1: b.1, kind: PRIM_SEGMENT, flags: PRIM_PARTIAL },
                None => {
                    fallback = true;
                    let first = if kind == 1 { cell.polys[ri].pts[0] } else { cell.paths[ri].pts[0] };
                    Local { x0: first.0, y0: first.1, x1: first.0, y1: first.1, kind: PRIM_POINT, flags: PRIM_PARTIAL }
                }
            }
        }
    };
    if fallback {
        out.point_fallbacks += 1;
    }
    out.locals.insert((ci, kind, record), local);
    Ok(local)
}

/// The record's anchor (rect centre, polygon first vertex, path first spine
/// point) and dimensions in cell coordinates, cached per record.
fn anchor(
    doc: &Doc,
    ci: usize,
    kind: u8,
    ri: usize,
    anchors: &mut HashMap<(usize, u8, u32), Point>,
) -> Result<Point, String> {
    let record: u32 = ri
        .try_into()
        .map_err(|_| "representatives: record index exceeds u32")?;
    if let Some(cached) = anchors.get(&(ci, kind, record)) {
        return Ok(*cached);
    }
    let cell = &doc.cells[ci];
    let (x, y, w, h) = match kind {
        0 => {
            let r = &cell.rects[ri];
            (
                r.x.checked_add(r.w / 2)
                    .ok_or("representatives: rect x overflow")?,
                r.y.checked_add(r.h / 2)
                    .ok_or("representatives: rect y overflow")?,
                r.w.unsigned_abs(),
                r.h.unsigned_abs(),
            )
        }
        _ => {
            let pts = if kind == 1 {
                &cell.polys[ri].pts
            } else {
                &cell.paths[ri].pts
            };
            let mut b = BBox::EMPTY;
            for &(x, y) in pts {
                b.grow(&BBox {
                    x0: x,
                    y0: y,
                    x1: x,
                    y1: y,
                });
            }
            let extra = if kind == 2 {
                cell.paths[ri].hw.unsigned_abs().saturating_mul(2)
            } else {
                0
            };
            // A polygon vertex or path spine point lies on actual geometry;
            // a concave polygon's bbox centre need not lie inside it.
            (
                pts[0].0,
                pts[0].1,
                b.x1.abs_diff(b.x0).saturating_add(extra),
                b.y1.abs_diff(b.y0).saturating_add(extra),
            )
        }
    };
    let p = Point {
        x,
        y,
        max_dim: w.max(h),
        min_dim: w.min(h),
        rank: 0,
    };
    anchors.insert((ci, kind, record), p);
    Ok(p)
}

fn put32(out: &mut Vec<u8>, n: u32) {
    out.extend(n.to_le_bytes());
}
fn put64(out: &mut Vec<u8>, n: u64) {
    out.extend(n.to_le_bytes());
}
fn put_bbox(out: &mut Vec<u8>, b: BBox) {
    for n in [b.x0, b.y0, b.x1, b.y1] {
        put64(out, n as u64);
    }
}
fn morton(p: &Point, b: BBox) -> u64 {
    let axis = |v: i64, lo: i64, hi: i64| -> u64 {
        ((v as i128 - lo as i128) * 65535 / (hi as i128 - lo as i128).max(1)) as u64
    };
    let (x, y) = (axis(p.x, b.x0, b.x1), axis(p.y, b.y0, b.y1));
    (0..16).fold(0, |m, i| {
        m | ((x >> i & 1) << (2 * i)) | ((y >> i & 1) << (2 * i + 1))
    })
}
pub fn encode(built: &mut Built, ovm: &Ovm) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(MAGIC);
    put32(&mut out, 1);
    put32(&mut out, crc32fast::hash(&ovm.data));
    put64(&mut out, ovm.src_size);
    put64(&mut out, ovm.src_mtime);
    put64(&mut out, ovm.unit.to_bits());
    put32(&mut out, ovm.top);
    put32(&mut out, built.groups.len() as u32);
    for group in &mut built.groups {
        let mut bounds = BBox::EMPTY;
        for &p in &group.points {
            bounds.grow(&p.bbox());
        }
        group
            .points
            .sort_unstable_by_key(|p| (morton(p, bounds), p.rank));
        for n in [
            group.key.0,
            group.key.1,
            group.key.2,
            group.points.len() as u32,
        ] {
            put32(&mut out, n);
        }
        put64(&mut out, group.members);
        put32(&mut out, group.points.len().div_ceil(CHUNK) as u32);
        for chunk in group.points.chunks(CHUNK) {
            let mut b = BBox::EMPTY;
            for &p in chunk {
                b.grow(&p.bbox());
            }
            put_bbox(&mut out, b);
            put32(&mut out, chunk.len() as u32);
            put32(&mut out, chunk.iter().map(|p| p.rank).min().unwrap());
            for p in chunk {
                put64(&mut out, p.x as u64);
                put64(&mut out, p.y as u64);
                put64(&mut out, p.max_dim);
                put64(&mut out, p.min_dim);
                put32(&mut out, p.rank);
                put32(&mut out, 0);
            }
        }
    }
    let crc = crc32fast::hash(&out);
    put32(&mut out, crc);
    out
}

fn morton_xy(x: i64, y: i64, b: BBox) -> u64 {
    morton(&Point { x, y, max_dim: 0, min_dim: 0, rank: 0 }, b)
}

/// OVR2 step 1: the header and group table of OVR1 (version 2), chunks of
/// up to 128 shapes in Morton order of their centres, each chunk's bbox the
/// union of its shapes' EXTENTS (a long hairline whose centre is off screen
/// is still found), 64-byte records: uid (group << 32 | rank) u64, x0 y0 x1
/// y1 i64, gate_dim u64, thickness u64, kind u8, flags u8, 6 zero bytes.
/// Requires a `build_with(.., true)` result.
pub fn encode_v2(built: &mut Built, ovm: &Ovm) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    out.extend(MAGIC2);
    put32(&mut out, 2);
    put32(&mut out, crc32fast::hash(&ovm.data));
    put64(&mut out, ovm.src_size);
    put64(&mut out, ovm.src_mtime);
    put64(&mut out, ovm.unit.to_bits());
    put32(&mut out, ovm.top);
    put32(&mut out, built.groups.len() as u32);
    for (gi, group) in built.groups.iter_mut().enumerate() {
        if group.prims.len() != group.points.len() {
            return Err("representatives: OVR2 needs the shapes of every sample (build_with)".into());
        }
        let centre = |p: &Prim| {
            (
                ((p.x0 as i128 + p.x1 as i128) / 2) as i64,
                ((p.y0 as i128 + p.y1 as i128) / 2) as i64,
            )
        };
        let mut bounds = BBox::EMPTY;
        for p in &group.prims {
            let (x, y) = centre(p);
            bounds.grow(&BBox { x0: x, y0: y, x1: x, y1: y });
        }
        group.prims.sort_unstable_by_key(|p| {
            let (x, y) = centre(p);
            (morton_xy(x, y, bounds), p.rank)
        });
        for n in [group.key.0, group.key.1, group.key.2, group.prims.len() as u32] {
            put32(&mut out, n);
        }
        put64(&mut out, group.members);
        put32(&mut out, group.prims.len().div_ceil(CHUNK) as u32);
        for chunk in group.prims.chunks(CHUNK) {
            let mut b = BBox::EMPTY;
            for p in chunk {
                b.grow(&p.bbox());
            }
            put_bbox(&mut out, b);
            put32(&mut out, chunk.len() as u32);
            put32(&mut out, chunk.iter().map(|p| p.rank).min().unwrap());
            for p in chunk {
                put64(&mut out, ((gi as u64) << 32) | p.rank as u64);
                for v in [p.x0, p.y0, p.x1, p.y1] {
                    put64(&mut out, v as u64);
                }
                put64(&mut out, p.gate_dim);
                put64(&mut out, p.thickness);
                out.extend([p.kind, p.flags, 0, 0, 0, 0, 0, 0]);
            }
        }
    }
    let crc = crc32fast::hash(&out);
    put32(&mut out, crc);
    Ok(out)
}

struct ChunkRef {
    bbox: BBox,
    offset: usize,
    len: usize,
    min_rank: u32,
}
struct GroupRef {
    layer: u32,
    depth: u32,
    count: usize,
    bounds: BBox,
    chunks: Vec<ChunkRef>,
}
pub struct File {
    data: Backing,
    groups: Vec<GroupRef>,
    /// 1 = points, 2 = flat shapes, 3 = shapes with a premerged spatial tree.
    version: u32,
    tree: Option<tree::Tree>,
}
#[derive(Clone, Default, Debug)]
pub struct QueryStats {
    pub points: u64,
    pub tested: u64,
    pub chunks: u64,
    pub limited: bool,
    pub nodes: u64,
    pub proxy_nodes: u64,
    pub bytes: u64,
    pub pixels: u64,
}
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}
impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let b = self
            .data
            .get(self.pos..self.pos + N)
            .ok_or("representatives: truncated file")?;
        self.pos += N;
        Ok(b.try_into().unwrap())
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn bbox(&mut self) -> Result<BBox, String> {
        Ok(BBox {
            x0: self.u64()? as i64,
            y0: self.u64()? as i64,
            x1: self.u64()? as i64,
            y1: self.u64()? as i64,
        })
    }
    fn point(&mut self) -> Result<Point, String> {
        let p = Point {
            x: self.u64()? as i64,
            y: self.u64()? as i64,
            max_dim: self.u64()?,
            min_dim: self.u64()?,
            rank: self.u32()?,
        };
        if self.u32()? != 0 {
            return Err("representatives: unknown point flags".into());
        }
        Ok(p)
    }
    /// an OVR2 record and the group index of its uid
    fn prim(&mut self) -> Result<(Prim, u32), String> {
        let uid = self.u64()?;
        let p = Prim {
            x0: self.u64()? as i64,
            y0: self.u64()? as i64,
            x1: self.u64()? as i64,
            y1: self.u64()? as i64,
            gate_dim: self.u64()?,
            thickness: self.u64()?,
            kind: 0,
            flags: 0,
            rank: uid as u32,
        };
        let tail: [u8; 8] = self.take()?;
        if tail[0] > PRIM_POINT || tail[1] & !PRIM_PARTIAL != 0 || tail[2..] != [0; 6] {
            return Err("representatives: unknown shape kind or flags".into());
        }
        Ok((Prim { kind: tail[0], flags: tail[1], ..p }, (uid >> 32) as u32))
    }
}
impl File {
    pub fn open(dir: &str, ovm: &Ovm) -> Result<Self, String> {
        Self::from_backing(floe_ovm::map_file(&format!("{dir}/design.ovr"))?, ovm)
    }
    pub fn from_backing(data: Backing, ovm: &Ovm) -> Result<Self, String> {
        if data.len() >= 12 && &data[..8] == MAGIC2
            && u32::from_le_bytes(data[8..12].try_into().unwrap()) == 3 {
            let tree = tree::Tree::parse(&data, ovm)?;
            return Ok(Self { data, groups: Vec::new(), version: 3, tree: Some(tree) });
        }
        // OVR1: 40-byte points (192 MiB); OVR2: 64-byte shapes (384 MiB)
        let version = if data.len() >= 8 && &data[..8] == MAGIC2 { 2u32 } else { 1 };
        let cap = if version == 2 { 384 } else { 192 } * 1024 * 1024;
        if data.len() < 52 || data.len() > cap {
            return Err("representatives: invalid file size".into());
        }
        let end = data.len() - 4;
        if crc32fast::hash(&data[..end]) != u32::from_le_bytes(data[end..].try_into().unwrap()) {
            return Err("representatives: checksum mismatch".into());
        }
        let mut c = Cursor {
            data: &data[..end],
            pos: 0,
        };
        if &c.take::<8>()? != if version == 2 { MAGIC2 } else { MAGIC } || c.u32()? != version {
            return Err("representatives: unsupported format".into());
        }
        if c.u32()? != crc32fast::hash(&ovm.data)
            || c.u64()? != ovm.src_size
            || c.u64()? != ovm.src_mtime
            || c.u64()? != ovm.unit.to_bits()
            || c.u32()? != ovm.top
        {
            return Err("representatives: cache identity mismatch; rebuild design.ovr".into());
        }
        let n = c.u32()? as usize;
        if n > MAX_GROUPS {
            return Err("representatives: group limit exceeded".into());
        }
        let layer_map: BTreeMap<_, _> = (0..ovm.n_layers)
            .map(|i| {
                let l = ovm.layer(i);
                ((l.layer, l.dt), i)
            })
            .collect();
        let mut groups = Vec::with_capacity(n);
        let mut total = 0usize;
        let mut previous = None;
        for _ in 0..n {
            let key = (c.u32()?, c.u32()?, c.u32()?);
            if previous.is_some_and(|p| p >= key) {
                return Err("representatives: unsorted/duplicate groups".into());
            }
            previous = Some(key);
            let layer = *layer_map
                .get(&(key.0, key.1))
                .ok_or("representatives: unknown layer")?;
            let count = c.u32()? as usize;
            let members = c.u64()?;
            let nchunks = c.u32()? as usize;
            total += count;
            if count == 0
                || count as u64 > members
                || total > MAX_POINTS
                || key.2 > 4096
                || nchunks != count.div_ceil(CHUNK)
            {
                return Err("representatives: invalid group counts".into());
            }
            let mut chunks = Vec::with_capacity(nchunks);
            let mut seen = vec![false; count];
            let mut parsed = 0;
            for _ in 0..nchunks {
                let bbox = c.bbox()?;
                let len = c.u32()? as usize;
                let min_rank = c.u32()?;
                if len == 0 || len > CHUNK || parsed + len > count || bbox.is_empty() {
                    return Err("representatives: invalid chunk".into());
                }
                let offset = c.pos;
                let mut actual_min = u32::MAX;
                for _ in 0..len {
                    let rank = if version == 2 {
                        let (p, group) = c.prim()?;
                        let b = p.bbox();
                        if group as usize != groups.len()
                            || (p.kind == PRIM_RECT && (p.x0 > p.x1 || p.y0 > p.y1))
                            || b.x0 < bbox.x0
                            || b.y0 < bbox.y0
                            || b.x1 > bbox.x1
                            || b.y1 > bbox.y1
                        {
                            return Err("representatives: invalid shape".into());
                        }
                        p.rank
                    } else {
                        let p = c.point()?;
                        if p.min_dim > p.max_dim || !bbox.intersects(&p.bbox()) {
                            return Err("representatives: invalid point".into());
                        }
                        p.rank
                    };
                    if rank as usize >= count || seen[rank as usize] {
                        return Err("representatives: invalid point".into());
                    }
                    seen[rank as usize] = true;
                    actual_min = actual_min.min(rank);
                }
                if actual_min != min_rank {
                    return Err("representatives: invalid rank directory".into());
                }
                parsed += len;
                chunks.push(ChunkRef {
                    bbox,
                    offset,
                    len,
                    min_rank,
                });
            }
            if parsed != count {
                return Err("representatives: incomplete group".into());
            }
            let mut bounds = BBox::EMPTY;
            for chunk in &chunks {
                bounds.grow(&chunk.bbox);
            }
            groups.push(GroupRef {
                layer,
                depth: key.2,
                count,
                bounds,
                chunks,
            });
        }
        if c.pos != end {
            return Err("representatives: trailing data".into());
        }
        Ok(Self { data, groups, version, tree: None })
    }

    /// No source page reads, hierarchy expansion, or repetition enumeration.
    /// The rank mask is global (not viewport-relative), so panning/margin
    /// reuse selects the same world points. Zoom thinning uses nested masks.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The visible groups of a request and each one's nested rank shift.
    fn selection(&self, req: &crate::ViewReq, stats: &mut QueryStats) -> Vec<(&GroupRef, u32)> {
        let bit = |bits: &[u8], li: u32| {
            bits.get(li as usize / 8)
                .is_some_and(|b| b & (1 << (li % 8)) != 0)
        };
        let groups: Vec<_> = self
            .groups
            .iter()
            .filter(|g| {
                g.depth <= req.depth && bit(&req.vis, g.layer) && !bit(&req.page_skip, g.layer)
            })
            .collect();
        // Screen density, not feature thickness: a sparse pitch must not
        // erase a layer merely because its lines happen to be nanometres wide.
        // One representative per ~4 screen pixels; each octave changes the
        // nested rank modulus by four. World bounds make this pan invariant.
        // Global cap is derived from STORED counts, not the current zoom.
        // Otherwise a zoom-out could relax the global cap and resurrect points
        // in a group whose own density level has not changed yet.
        let mut base_shift = 0u32;
        while groups
            .iter()
            .map(|g| (g.count as u64).div_ceil(1u64 << base_shift))
            .sum::<u64>()
            > FRAME_POINTS as u64
        {
            base_shift += 2;
        }
        let shifts: Vec<u32> = groups
            .iter()
            .map(|g| {
                let w =
                    ((g.bounds.x1 as i128 - g.bounds.x0 as i128) as f64 * req.px_per_dbu).max(1.0);
                let h =
                    ((g.bounds.y1 as i128 - g.bounds.y0 as i128) as f64 * req.px_per_dbu).max(1.0);
                let target = (w * h / 4.0).max(1.0);
                let density_shift =
                    (((g.count as f64 / target).log2().max(0.0) / 2.0).ceil() as u32 * 2).min(32);
                stats.limited |= base_shift > density_shift;
                density_shift.max(base_shift)
            })
            .collect();
        groups.into_iter().zip(shifts).collect()
    }

    /// OVR2: the shapes of a request. The same nested rank thinning and
    /// frame cap as the points (step 1 changes what a sample looks like,
    /// not how many are drawn); a shape is taken by its EXTENT meeting the
    /// view and by the original shape's gate_dim being under the cut.
    pub fn query_prims(&self, req: &crate::ViewReq) -> (Vec<(u32, Prim)>, QueryStats) {
        let mut stats = QueryStats::default();
        if self.version != 2 || req.cut_dbu <= 0 || req.px_per_dbu <= 0.0 {
            return (Vec::new(), stats);
        }
        let mut out = Vec::new();
        for (g, shift) in self.selection(req, &mut stats) {
            for chunk in &g.chunks {
                stats.chunks += 1;
                if !chunk.bbox.intersects(&req.view) || (shift >= 32 && chunk.min_rank != 0) {
                    continue;
                }
                let mut c = Cursor { data: &self.data, pos: chunk.offset };
                for _ in 0..chunk.len {
                    let (p, _) = c.prim().expect("validated OVR shape");
                    stats.tested += 1;
                    if (p.rank as u64) & ((1u64 << shift) - 1) != 0
                        || p.gate_dim >= req.cut_dbu as u64
                        || !req.view.intersects(&p.bbox())
                    {
                        continue;
                    }
                    out.push((g.layer, p));
                }
            }
        }
        stats.points = out.len() as u64;
        (out, stats)
    }

    pub fn query(&self, req: &crate::ViewReq) -> (Vec<(u32, BBox)>, QueryStats) {
        let mut stats = QueryStats::default();
        if self.version != 1 || req.cut_dbu <= 0 || req.px_per_dbu <= 0.0 {
            return (Vec::new(), stats);
        }
        let mut out = Vec::new();
        for (g, shift) in self.selection(req, &mut stats) {
            for chunk in &g.chunks {
                stats.chunks += 1;
                if !chunk.bbox.intersects(&req.view) || (shift >= 32 && chunk.min_rank != 0) {
                    continue;
                }
                let mut c = Cursor {
                    data: &self.data,
                    pos: chunk.offset,
                };
                for _ in 0..chunk.len {
                    let p = c.point().expect("validated OVR point");
                    stats.tested += 1;
                    if (p.rank as u64) & ((1u64 << shift) - 1) != 0
                        || !req.view.intersects(&p.bbox())
                    {
                        continue;
                    }
                    let threshold = if req.page_hairline {
                        p.max_dim.min(p.min_dim.saturating_mul(2))
                    } else {
                        p.max_dim
                    };
                    if threshold >= req.cut_dbu as u64 {
                        continue;
                    }
                    out.push((g.layer, p.bbox()));
                }
            }
        }
        stats.points = out.len() as u64;
        (out, stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_oasis::doc::{Cell, PathRec, PolyRec, RectRec};
    use std::collections::BTreeSet;
    use std::sync::Arc;

    fn doc(cells: Vec<Cell>, top: usize) -> Doc {
        Doc {
            unit: 1000.0,
            cells,
            top,
            layer_order: vec![(1, 0)],
            norm_s: 0.0,
            layer_names: HashMap::new(),
            layer_aliases: HashMap::new(),
        }
    }
    fn rect(rep: Rep) -> RectRec {
        RectRec {
            layer: 1,
            dt: 0,
            x: 0,
            y: 0,
            w: 2,
            h: 2,
            rep,
        }
    }
    fn ovm() -> Ovm {
        let mut b = floe_ovm::Builder::new(1000.0, 123, 456, 1);
        b.layer(1, 0, "L", 0, 0);
        let mask = b.bitset(&[0]);
        b.cell(
            "TOP",
            0,
            0,
            &BBox::EMPTY,
            &BBox::EMPTY,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            mask,
            mask,
            0,
            0,
            0,
            mask,
        );
        Ovm::from_bytes(b.finish(0, 0)).unwrap()
    }
    fn request(cut: i64, depth: u32) -> crate::ViewReq {
        crate::ViewReq {
            view: BBox {
                x0: -1_000_000_000,
                y0: -1_000_000_000,
                x1: 1_000_000_000,
                y1: 1_000_000_000,
            },
            cut_dbu: cut,
            depth,
            vis: vec![1],
            px_per_dbu: 0.25,
            sub_cut_wash: false,
            page_reps: false,
            decode_budget: 0,
            page_hairline: true,
            page_skip: vec![],
            prune_skipped: false,
            sub_cut_box: false,
            shape_cut: false,
            frames: true,
        }
    }
    fn load(built: &mut Built) -> File {
        let ovm = ovm();
        File::from_backing(Backing::Vec(encode(built, &ovm)), &ovm).unwrap()
    }

    #[test]
    fn trillion_member_grid_is_bounded_and_spatially_distributed() {
        let cell = Cell {
            rects: vec![rect(Rep::Grid {
                na: 1_000_000,
                nb: 1_000_000,
                va: (8, 0),
                vb: (0, 8),
            })],
            ..Cell::default()
        };
        let source = doc(vec![cell], 0);
        let built = build(&source, 4096, None).unwrap();
        assert_eq!(built.directory, 1);
        let g = &built.groups[0];
        assert_eq!(g.members, 1_000_000_000_000);
        assert_eq!(g.points.len(), 4096);
        assert_eq!(
            g.points
                .iter()
                .map(|p| (p.x, p.y))
                .collect::<BTreeSet<_>>()
                .len(),
            4096
        );
        let bins: BTreeSet<_> = g
            .points
            .iter()
            .map(|p| {
                assert_eq!((p.x - 1) % 8, 0);
                assert_eq!((p.y - 1) % 8, 0);
                (p.x / 1_000_000, p.y / 1_000_000)
            })
            .collect();
        assert_eq!(
            bins.len(),
            64,
            "samples must cover the whole array, not one corner/stripe"
        );
    }

    #[test]
    fn nested_placements_preserve_rotation_mirror_offsets_and_depth() {
        let child = Cell {
            rects: vec![rect(Rep::Pts(Arc::from([(0, 0), (10, 20)])))],
            ..Cell::default()
        };
        let parent = Cell {
            places: vec![PlaceRec {
                cell: 0,
                x: 100,
                y: 200,
                rot: 1,
                flip: true,
                rep: Rep::Grid {
                    na: 2,
                    nb: 1,
                    va: (40, 0),
                    vb: (0, 0),
                },
            }],
            ..Cell::default()
        };
        let top = Cell {
            rects: vec![rect(Rep::One)],
            places: vec![PlaceRec {
                cell: 1,
                x: -10,
                y: 30,
                rot: 2,
                flip: false,
                rep: Rep::One,
            }],
            ..Cell::default()
        };
        let source = doc(vec![child, parent, top], 2);
        let mut built = build(&source, 100, None).unwrap();
        let g = built.groups.iter().find(|g| g.key.2 == 2).unwrap();
        let actual: BTreeSet<_> = g.points.iter().map(|p| (p.x, p.y)).collect();
        let expected: BTreeSet<_> = [(-111, -171), (-131, -181), (-151, -171), (-171, -181)]
            .into_iter()
            .collect();
        assert_eq!(actual, expected);
        let file = load(&mut built);
        assert_eq!(file.query(&request(3, 0)).0.len(), 1);
        assert_eq!(file.query(&request(3, 1)).0.len(), 1);
        assert_eq!(file.query(&request(3, 2)).0.len(), 5);
        assert_eq!(file.query(&request(3, u32::MAX)).0.len(), 5);
    }

    #[test]
    fn masks_cut_visibility_and_zoom_have_nested_representatives() {
        let source = doc(
            vec![Cell {
                rects: vec![rect(Rep::Grid {
                    na: 64,
                    nb: 64,
                    va: (10, 0),
                    vb: (0, 10),
                })],
                ..Cell::default()
            }],
            0,
        );
        let mut built = build(&source, 4096, None).unwrap();
        let file = load(&mut built);
        let set = |cut| {
            let mut req = request(cut, 0);
            req.px_per_dbu = 0.75 / cut as f64;
            file.query(&req)
                .0
                .into_iter()
                .map(|(_, b)| (b.x0, b.y0))
                .collect::<BTreeSet<_>>()
        };
        let (near, mid, far) = (set(3), set(6), set(12));
        assert_eq!((near.len(), mid.len(), far.len()), (4096, 1024, 256));
        assert!(far.is_subset(&mid) && mid.is_subset(&near));
        let mut req = request(3, 0);
        req.page_skip = vec![1];
        assert!(file.query(&req).0.is_empty());
        req.page_skip.clear();
        req.vis = vec![0];
        assert!(file.query(&req).0.is_empty());
        assert!(file.query(&request(0, 0)).0.is_empty());
        assert!(file.query(&request(2, 0)).0.is_empty());
        req.vis = vec![1];
        req.view = BBox {
            x0: 10,
            y0: 10,
            x1: 30,
            y1: 30,
        };
        let (visible, stats) = file.query(&req);
        assert_eq!(visible.len(), 4);
        assert!(stats.tested < 4096);
    }

    #[test]
    fn rejects_corrupt_truncated_or_foreign_sidecars() {
        let source = doc(
            vec![Cell {
                rects: vec![rect(Rep::One)],
                ..Cell::default()
            }],
            0,
        );
        let mut built = build(&source, 10, None).unwrap();
        let mut target = ovm();
        let bytes = encode(&mut built, &target);
        for end in [0, 47, bytes.len() - 1] {
            assert!(File::from_backing(Backing::Vec(bytes[..end].to_vec()), &target).is_err());
        }
        let mut bad = bytes.clone();
        bad[60] ^= 1;
        assert!(File::from_backing(Backing::Vec(bad), &target).is_err());
        target.src_size += 1;
        assert!(File::from_backing(Backing::Vec(bytes), &target).is_err());
    }

    #[test]
    fn bounded_quota_does_not_starve_sparse_groups() {
        let q = quotas(&[1_000_000_000, 3, 1_000_000_000], DEFAULT_POINTS, 1000).unwrap();
        assert_eq!(q.iter().sum::<usize>(), 1000);
        assert_eq!(q[1], 3);
        assert!(q[0] > 490 && q[2] > 490);
        assert!(quotas(&[], 1, 1).unwrap().is_empty());
    }

    #[test]
    fn frame_cap_cannot_resurrect_points_when_zooming_out() {
        let n = FRAME_POINTS / 2 + 1;
        let mut built = Built {
            directory: 0,
            peak_requests: 0,
            point_fallbacks: 0,
            groups: (0..2)
                .map(|depth| {
                    let scale = if depth == 0 { 1 } else { 100 };
                    Group {
                        key: (1, 0, depth),
                        members: n as u64,
                        points: (0..n)
                            .map(|rank| Point {
                                x: (rank % 512) as i64 * scale + depth as i64 * 100_000,
                                y: (rank / 512) as i64 * scale,
                                max_dim: 1,
                                min_dim: 1,
                                rank: rank as u32,
                            })
                            .collect(),
                        prims: Vec::new(),
                    }
                })
                .collect(),
        };
        let file = load(&mut built);
        let mut previous = None;
        for scale in [4.0, 2.0, 1.0, 0.5] {
            let mut req = request(1000, u32::MAX);
            req.px_per_dbu = scale;
            let (points, _) = file.query(&req);
            assert!(points.len() <= FRAME_POINTS);
            let points: BTreeSet<_> = points.into_iter().map(|(_, b)| (b.x0, b.y0)).collect();
            if let Some(old) = previous {
                assert!(points.is_subset(&old));
            }
            previous = Some(points);
        }
    }

    #[test]
    fn rejects_cycles_and_count_overflow() {
        let cyclic = doc(
            vec![Cell {
                places: vec![PlaceRec {
                    cell: 0,
                    x: 0,
                    y: 0,
                    rot: 0,
                    flip: false,
                    rep: Rep::One,
                }],
                ..Cell::default()
            }],
            0,
        );
        assert!(build(&cyclic, 1, None).err().unwrap().contains("cyclic"));
        let huge = doc(
            vec![Cell {
                rects: vec![rect(Rep::Grid {
                    na: u64::MAX,
                    nb: 2,
                    va: (1, 0),
                    vb: (0, 1),
                })],
                ..Cell::default()
            }],
            0,
        );
        assert!(build(&huge, 1, None).is_err());
    }

    #[test]
    fn serialization_is_deterministic_and_sampling_prefix_is_stable() {
        let source = doc(
            vec![Cell {
                rects: vec![rect(Rep::Grid {
                    na: 1000,
                    nb: 2000,
                    va: (10, 0),
                    vb: (0, 10),
                })],
                ..Cell::default()
            }],
            0,
        );
        let mut a = build(&source, 1024, None).unwrap();
        let mut b = build(&source, 2048, None).unwrap();
        assert_eq!(a.groups[0].points, b.groups[0].points[..1024]);
        let ovm = ovm();
        let first = encode(&mut a, &ovm);
        assert_eq!(first, encode(&mut a, &ovm));
        assert!(File::from_backing(Backing::Vec(encode(&mut b, &ovm)), &ovm).is_ok());
    }

    /// The 0.12.154 builder (materialized member directory, one entry per
    /// placement record x child group): the reference the streaming
    /// resolve must match byte for byte.
    mod legacy {
        use super::super::*;
        const MAX_INDEX_ENTRIES: usize = 134_217_728;
        #[derive(Clone, Copy)]
        struct Entry {
            end: u64,
            record: u32,
            kind: u8, // rect, polygon, path, placement
        }
        #[derive(Default)]
        struct Run {
            entries: Vec<Entry>,
            members: u64,
        }
        type CellIndex = BTreeMap<Key, Run>;

        fn add(
            index: &mut CellIndex,
            key: Key,
            n: u64,
            record: usize,
            kind: u8,
            entries: &mut usize,
        ) -> Result<(), String> {
            if n == 0 {
                return Ok(());
            }
            *entries += 1;
            if *entries > MAX_INDEX_ENTRIES {
                return Err("representatives: member directory exceeds 2 GiB limit".into());
            }
            let run = index.entry(key).or_default();
            run.members = run
                .members
                .checked_add(n)
                .ok_or("representatives: recursive member count exceeds u64")?;
            run.entries.push(Entry {
                end: run.members,
                record: record
                    .try_into()
                    .map_err(|_| "representatives: record index exceeds u32")?,
                kind,
            });
            Ok(())
        }

        pub fn build(doc: &Doc, per_group: usize, progress: Option<fn(&str)>) -> Result<Vec<Group>, String> {
            if per_group == 0 || per_group > MAX_POINTS {
                return Err(format!(
                    "representatives: points must be in 1..={MAX_POINTS}"
                ));
            }
            let order = postorder(doc)?;
            let mut index: Vec<CellIndex> = (0..doc.cells.len()).map(|_| BTreeMap::new()).collect();
            let mut entries = 0;
            let mut groups = 0usize;
            let mut heartbeat = std::time::Instant::now();
            for &ci in &order {
                let cell = &doc.cells[ci];
                let mut own = CellIndex::new();
                for (i, r) in cell.rects.iter().enumerate() {
                    add(
                        &mut own,
                        (r.layer, r.dt, 0),
                        members(&r.rep)?,
                        i,
                        0,
                        &mut entries,
                    )?;
                }
                for (i, p) in cell
                    .polys
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| !p.pts.is_empty())
                {
                    add(
                        &mut own,
                        (p.layer, p.dt, 0),
                        members(&p.rep)?,
                        i,
                        1,
                        &mut entries,
                    )?;
                }
                for (i, p) in cell
                    .paths
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| !p.pts.is_empty())
                {
                    add(
                        &mut own,
                        (p.layer, p.dt, 0),
                        members(&p.rep)?,
                        i,
                        2,
                        &mut entries,
                    )?;
                }
                for (i, pl) in cell.places.iter().enumerate() {
                    let n = members(&pl.rep)?;
                    for (&(l, d, depth), run) in &index[pl.cell] {
                        let depth = depth
                            .checked_add(1)
                            .filter(|&d| d <= 4096)
                            .ok_or("representatives: hierarchy depth exceeds 4096")?;
                        add(
                            &mut own,
                            (l, d, depth),
                            n.checked_mul(run.members)
                                .ok_or("representatives: recursive member count exceeds u64")?,
                            i,
                            3,
                            &mut entries,
                        )?;
                    }
                }
                groups += own.len();
                if groups > 4_194_304 {
                    return Err("representatives: cell/group directory limit exceeded".into());
                }
                index[ci] = own;
                if heartbeat.elapsed().as_secs() >= 10 {
                    if let Some(log) = progress {
                        log(&format!("count entries={entries} cell={ci}"));
                    }
                    heartbeat = std::time::Instant::now();
                }
            }
            let root = &index[doc.top];
            if root.len() > MAX_GROUPS {
                return Err("representatives: top group limit exceeded".into());
            }
            let counts: Vec<_> = root.values().map(|r| r.members).collect();
            let budgets = quotas(&counts, per_group, MAX_POINTS)?;
            let mut result = Vec::with_capacity(root.len());
            let mut anchors = HashMap::new();
            for ((&key, run), count) in root.iter().zip(budgets) {
                let mut points = Vec::with_capacity(count);
                let mask = run
                    .members
                    .checked_next_power_of_two()
                    .unwrap_or(0)
                    .wrapping_sub(1);
                let mut candidate = 0u64;
                while points.len() < count {
                    let rank = permute(candidate, mask);
                    candidate += 1;
                    if candidate > (count as u64).saturating_mul(64).saturating_add(128) {
                        return Err("representatives: sampling work limit exceeded".into());
                    }
                    if rank >= run.members {
                        continue;
                    }
                    let mut p = resolve(doc, &index, key, rank, &mut anchors)?;
                    p.rank = points.len() as u32;
                    points.push(p);
                }
                if let Some(log) = progress {
                    log(&format!(
                        "{}/{} depth={} members={} points={}",
                        key.0,
                        key.1,
                        key.2,
                        run.members,
                        points.len()
                    ));
                }
                result.push(Group {
                    key,
                    members: run.members,
                    points,
                    prims: Vec::new(),
                });
            }
            let _ = entries;
            Ok(result)
        }

        fn resolve(
            doc: &Doc,
            index: &[CellIndex],
            mut key: Key,
            mut rank: u64,
            anchors: &mut HashMap<(usize, u8, u32), Point>,
        ) -> Result<Point, String> {
            let mut ci = doc.top;
            let mut transforms = Vec::new();
            let mut point;
            let rep_offset;
            loop {
                let run = &index[ci][&key];
                let ei = run.entries.partition_point(|e| e.end <= rank);
                let entry = run.entries[ei];
                rank -= if ei == 0 { 0 } else { run.entries[ei - 1].end };
                let cell = &doc.cells[ci];
                let ri = entry.record as usize;
                if entry.kind == 3 {
                    let pl = &cell.places[ri];
                    key.2 -= 1;
                    let n = index[pl.cell][&key].members;
                    let off = offset(&pl.rep, rank / n);
                    transforms.push((pl, off));
                    rank %= n;
                    ci = pl.cell;
                } else {
                    let rep = match entry.kind {
                        0 => &cell.rects[ri].rep,
                        1 => &cell.polys[ri].rep,
                        _ => &cell.paths[ri].rep,
                    };
                    rep_offset = offset(rep, rank);
                    let anchor_key = (ci, entry.kind, entry.record);
                    point = if let Some(cached) = anchors.get(&anchor_key) {
                        *cached
                    } else {
                        let calculated = (|| -> Result<Point, String> {
                            let (x, y, w, h) = match entry.kind {
                                0 => {
                                    let r = &cell.rects[ri];
                                    (
                                        r.x.checked_add(r.w / 2)
                                            .ok_or("representatives: rect x overflow")?,
                                        r.y.checked_add(r.h / 2)
                                            .ok_or("representatives: rect y overflow")?,
                                        r.w.unsigned_abs(),
                                        r.h.unsigned_abs(),
                                    )
                                }
                                _ => {
                                    let pts = if entry.kind == 1 {
                                        &cell.polys[ri].pts
                                    } else {
                                        &cell.paths[ri].pts
                                    };
                                    let mut b = BBox::EMPTY;
                                    for &(x, y) in pts {
                                        b.grow(&BBox {
                                            x0: x,
                                            y0: y,
                                            x1: x,
                                            y1: y,
                                        });
                                    }
                                    let extra = if entry.kind == 2 {
                                        cell.paths[ri].hw.unsigned_abs().saturating_mul(2)
                                    } else {
                                        0
                                    };
                                    // A polygon vertex or path spine point lies on actual geometry;
                                    // a concave polygon's bbox centre need not lie inside it.
                                    (
                                        pts[0].0,
                                        pts[0].1,
                                        b.x1.abs_diff(b.x0).saturating_add(extra),
                                        b.y1.abs_diff(b.y0).saturating_add(extra),
                                    )
                                }
                            };
                            Ok(Point {
                                x,
                                y,
                                max_dim: w.max(h),
                                min_dim: w.min(h),
                                rank: 0,
                            })
                        })()?;
                        anchors.insert(anchor_key, calculated);
                        calculated
                    };
                    break;
                }
            }
            let (mut x, mut y) = (
                point.x as i128 + rep_offset.0,
                point.y as i128 + rep_offset.1,
            );
            for (pl, off) in transforms.into_iter().rev() {
                if pl.flip {
                    y = -y;
                }
                (x, y) = match pl.rot & 3 {
                    0 => (x, y),
                    1 => (-y, x),
                    2 => (-x, -y),
                    _ => (y, -x),
                };
                x += pl.x as i128 + off.0;
                y += pl.y as i128 + off.1;
            }
            point.x = x
                .try_into()
                .map_err(|_| "representatives: x coordinate overflow")?;
            point.y = y
                .try_into()
                .map_err(|_| "representatives: y coordinate overflow")?;
            Ok(point)
        }

    }

    #[test]
    fn streaming_resolve_matches_the_materialized_directory_byte_for_byte() {
        // shared leaves under several parents with every transform, Grid and
        // Pts repetitions on shapes and placements, polygons and paths, and
        // groups both fully and partially sampled
        let leaf = Cell {
            rects: vec![
                rect(Rep::Pts(Arc::from([(0, 0), (10, 20), (30, 5)]))),
                RectRec { layer: 2, dt: 0, x: 5, y: 5, w: 4, h: 6,
                          rep: Rep::Grid { na: 3, nb: 2, va: (10, 0), vb: (0, 12) } },
            ],
            polys: vec![PolyRec { layer: 1, dt: 0, pts: vec![(1, 1), (9, 1), (9, 5), (1, 5)], rep: Rep::One },
                        PolyRec { layer: 3, dt: 1, pts: vec![(0, 0), (4, 0), (4, 4)],
                                  rep: Rep::Grid { na: 2, nb: 2, va: (20, 0), vb: (0, 20) } }],
            paths: vec![PathRec { layer: 1, dt: 0, pts: vec![(0, 0), (50, 0), (50, 30)], hw: 3, es: 0, ee: 0,
                                  rep: Rep::Pts(Arc::from([(0, 0), (0, 100)])) }],
            ..Cell::default()
        };
        let mid = Cell {
            rects: vec![rect(Rep::Grid { na: 40, nb: 40, va: (8, 0), vb: (0, 8) })],
            places: vec![
                PlaceRec { cell: 0, x: 100, y: 200, rot: 1, flip: true,
                           rep: Rep::Grid { na: 2, nb: 1, va: (40, 0), vb: (0, 0) } },
                PlaceRec { cell: 0, x: -7, y: 3, rot: 3, flip: false,
                           rep: Rep::Pts(Arc::from([(0, 0), (500, 0), (0, 500)])) },
            ],
            ..Cell::default()
        };
        let top = Cell {
            rects: vec![rect(Rep::One), RectRec { layer: 2, dt: 0, x: -3, y: -3, w: 6, h: 6, rep: Rep::One }],
            places: vec![
                PlaceRec { cell: 1, x: -10, y: 30, rot: 2, flip: true, rep: Rep::One },
                PlaceRec { cell: 1, x: 1000, y: 0, rot: 0, flip: false,
                           rep: Rep::Grid { na: 2, nb: 2, va: (2000, 0), vb: (0, 2000) } },
                PlaceRec { cell: 0, x: 7, y: 7, rot: 1, flip: false, rep: Rep::One },
                PlaceRec { cell: 1, x: 5, y: -5, rot: 3, flip: true, rep: Rep::One },
            ],
            ..Cell::default()
        };
        let source = doc(vec![leaf, mid, top], 2);
        for per_group in [3usize, 100, 5000] {
            let mut ours = build(&source, per_group, None).unwrap();
            let mut theirs = legacy::build(&source, per_group, None).unwrap();
            assert_eq!(ours.groups.len(), theirs.len());
            for (a, b) in ours.groups.iter().zip(&theirs) {
                assert_eq!((a.key, a.members), (b.key, b.members));
                assert_eq!(a.points, b.points, "group {:?} at {} per group", a.key, per_group);
            }
            let ovm = ovm();
            let mut legacy_built = Built { groups: std::mem::take(&mut theirs), directory: 0, peak_requests: 0, point_fallbacks: 0 };
            assert_eq!(encode(&mut ours, &ovm), encode(&mut legacy_built, &ovm));
            assert!(ours.directory >= 3 && ours.peak_requests > 0);
        }
    }

    fn load_v2(built: &mut Built) -> File {
        let ovm = ovm();
        File::from_backing(Backing::Vec(encode_v2(built, &ovm).unwrap()), &ovm).unwrap()
    }

    #[test]
    fn ovr2_shapes_keep_their_extent_under_every_transform() {
        // a 2 x 40 hairline rect, a polygon whose longest edge is its 30-long
        // base, a path whose outline's longest edge runs along its 50-long
        // spine, and a degenerate polygon that can only be a point
        let leaf = Cell {
            rects: vec![RectRec { layer: 1, dt: 0, x: 0, y: 0, w: 2, h: 40, rep: Rep::One }],
            polys: vec![
                PolyRec { layer: 2, dt: 0, pts: vec![(0, 0), (30, 0), (30, 10), (0, 10)], rep: Rep::One },
                PolyRec { layer: 4, dt: 0, pts: vec![(7, 7), (7, 7), (7, 7)], rep: Rep::One },
            ],
            paths: vec![PathRec { layer: 3, dt: 0, pts: vec![(0, 0), (50, 0)], hw: 1, es: 0, ee: 0, rep: Rep::One }],
            ..Cell::default()
        };
        let top = Cell {
            places: vec![
                PlaceRec { cell: 0, x: 1000, y: 2000, rot: 1, flip: false, rep: Rep::One },
                PlaceRec { cell: 0, x: -500, y: 300, rot: 0, flip: true, rep: Rep::One },
            ],
            ..Cell::default()
        };
        let mut source = doc(vec![leaf, top], 1);
        source.layer_order = vec![(1, 0), (2, 0), (3, 0), (4, 0)];
        let built = build_with(&source, 100, None, true).unwrap();
        assert_eq!(built.point_fallbacks, 1);
        let shapes = |layer: u32| -> Vec<Prim> {
            let g = built.groups.iter().find(|g| g.key.0 == layer).unwrap();
            assert_eq!(g.prims.len(), g.points.len());
            let mut v = g.prims.clone();
            v.sort_by_key(|p| p.bbox().x0);
            v
        };
        // the rect under flip (y -> -y) then translate, and under rot 90:
        // its long axis turns from vertical to horizontal
        let rects = shapes(1);
        assert_eq!((rects[0].kind, rects[0].bbox()), (PRIM_RECT, BBox { x0: -500, y0: 260, x1: -498, y1: 300 }));
        assert_eq!(rects[1].bbox(), BBox { x0: 960, y0: 2000, x1: 1000, y1: 2002 });
        assert!(rects.iter().all(|p| p.gate_dim == 4 && p.x0 <= p.x1 && p.y0 <= p.y1));
        // the polygon's base edge, 30 long, marked partial; gate from its bbox
        let polys = shapes(2);
        assert!(polys.iter().all(|p| p.kind == PRIM_SEGMENT && p.flags == PRIM_PARTIAL && p.gate_dim == 20));
        let len = |p: &Prim| (p.x1 - p.x0).abs().max((p.y1 - p.y0).abs());
        assert!(polys.iter().all(|p| len(p) == 30));
        // the path: an outline edge as long as the spine; gate = 2 * its width
        let paths = shapes(3);
        assert!(paths.iter().all(|p| p.kind == PRIM_SEGMENT && len(p) == 50 && p.gate_dim == 4));
        assert!(shapes(4).iter().all(|p| p.kind == PRIM_POINT && p.x0 == p.x1 && p.y0 == p.y1));
    }

    #[test]
    fn ovr2_files_round_trip_and_a_long_shape_is_found_by_its_extent() {
        let long = Cell {
            rects: vec![RectRec { layer: 1, dt: 0, x: 0, y: 0, w: 100_000, h: 2, rep: Rep::One }, rect(Rep::One)],
            ..Cell::default()
        };
        let source = doc(vec![long], 0);
        let mut built = build_with(&source, 10, None, true).unwrap();
        let v1 = load(&mut built);
        let file = load_v2(&mut built);
        assert_eq!((v1.version(), file.version()), (1, 2));
        // a view far from the long rect's centre (50_000, 1) still meets its extent
        let mut req = request(1000, 0);
        req.view = BBox { x0: 90_000, y0: -10, x1: 90_100, y1: 10 };
        let (shapes, stats) = file.query_prims(&req);
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].1.bbox(), BBox { x0: 0, y0: 0, x1: 100_000, y1: 2 });
        assert_eq!(stats.points, 1);
        // the point file has nothing there, and each file answers only its own query
        assert!(v1.query(&req).0.is_empty());
        assert!(file.query(&req).0.is_empty() && v1.query_prims(&req).0.is_empty());
        // the cut gates by the ORIGINAL shape: 2 * min_dim = 4 for the long rect
        req.cut_dbu = 4;
        assert!(file.query_prims(&req).0.is_empty());
        // a shapes file needs the shapes; corruption and a foreign cache are refused
        let ovm = ovm();
        assert!(encode_v2(&mut build(&source, 10, None).unwrap(), &ovm).is_err());
        let bytes = encode_v2(&mut built, &ovm).unwrap();
        let mut bad = bytes.clone();
        let at = bad.len() - 12; // inside the last record's kind/flags/pad bytes
        bad[at] ^= 0x40;
        assert!(File::from_backing(Backing::Vec(bad), &ovm).is_err());
        assert!(File::from_backing(Backing::Vec(bytes[..bytes.len() - 1].to_vec()), &ovm).is_err());
    }

    #[test]
    fn ovr2_thins_by_the_same_nested_ranks_as_the_points() {
        let source = doc(
            vec![Cell {
                rects: vec![rect(Rep::Grid { na: 64, nb: 64, va: (10, 0), vb: (0, 10) })],
                ..Cell::default()
            }],
            0,
        );
        let mut built = build_with(&source, 4096, None, true).unwrap();
        let (v1, v2) = (load(&mut built), load_v2(&mut built));
        for cut in [3i64, 6, 12] {
            let mut req = request(cut, 0);
            req.px_per_dbu = 0.75 / cut as f64;
            let points: BTreeSet<_> = v1.query(&req).0.into_iter().map(|(_, b)| (b.x0, b.y0)).collect();
            let shapes: BTreeSet<_> = v2
                .query_prims(&req)
                .0
                .into_iter()
                .map(|(_, p)| ((p.x0 + p.x1) / 2, (p.y0 + p.y1) / 2))
                .collect();
            assert_eq!(points, shapes, "cut {cut}: the same samples, as shapes");
        }
    }
}
