//! OVR1: bounded, display-only native point samples, independent of OVP pages.
//!
//! Count logical members bottom-up, then resolve only sampled member ranks.
//! A trillion-member Grid therefore costs one count and at most the sample
//! budget, never a trillion-member walk. Groups preserve (layer, relative
//! depth), so a depth-zero view cannot accidentally display descendants.
//! These are approximate existence samples, NOT occupancy or query geometry.
use floe_oasis::doc::{Doc, Rep};
use floe_ovm::{BBox, Backing, Ovm};
use std::collections::{BTreeMap, HashMap};

pub const DEFAULT_POINTS: usize = 262_144;
pub const MAX_POINTS: usize = 4_194_304;
pub const FRAME_POINTS: usize = 262_144;
const CHUNK: usize = 128;
const MAX_GROUPS: usize = 65_536;
const MAX_INDEX_ENTRIES: usize = 134_217_728; // 2 GiB, excluding the source Doc
const MAGIC: &[u8; 8] = b"FLOEOVR1";
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
#[derive(Debug)]
pub struct Group {
    pub key: Key,
    pub members: u64,
    pub points: Vec<Point>,
}
pub struct Built {
    pub groups: Vec<Group>,
    pub entries: usize,
}
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

pub fn build(doc: &Doc, per_group: usize, progress: Option<fn(&str)>) -> Result<Built, String> {
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
        });
    }
    Ok(Built {
        groups: result,
        entries,
    })
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
}
#[derive(Default, Debug)]
pub struct QueryStats {
    pub points: u64,
    pub tested: u64,
    pub chunks: u64,
    pub limited: bool,
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
}
impl File {
    pub fn open(dir: &str, ovm: &Ovm) -> Result<Self, String> {
        Self::from_backing(floe_ovm::map_file(&format!("{dir}/design.ovr"))?, ovm)
    }
    pub fn from_backing(data: Backing, ovm: &Ovm) -> Result<Self, String> {
        if data.len() < 52 || data.len() > 192 * 1024 * 1024 {
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
        if &c.take::<8>()? != MAGIC || c.u32()? != 1 {
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
                    let p = c.point()?;
                    if p.rank as usize >= count
                        || seen[p.rank as usize]
                        || p.min_dim > p.max_dim
                        || !bbox.intersects(&p.bbox())
                    {
                        return Err("representatives: invalid point".into());
                    }
                    seen[p.rank as usize] = true;
                    actual_min = actual_min.min(p.rank);
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
        Ok(Self { data, groups })
    }

    /// No source page reads, hierarchy expansion, or repetition enumeration.
    /// The rank mask is global (not viewport-relative), so panning/margin
    /// reuse selects the same world points. Zoom thinning uses nested masks.
    pub fn query(&self, req: &crate::ViewReq) -> (Vec<(u32, BBox)>, QueryStats) {
        let mut stats = QueryStats::default();
        if req.cut_dbu <= 0 || req.px_per_dbu <= 0.0 {
            return (Vec::new(), stats);
        }
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
        let mut out = Vec::new();
        for (g, shift) in groups.into_iter().zip(shifts) {
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
    use floe_oasis::doc::{Cell, PlaceRec, RectRec};
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
        assert_eq!(built.entries, 1);
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
            entries: 0,
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
}
