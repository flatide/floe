//! OVR2 revision 3: a disk-resident spatial tree and bounded, resumable queries.
//! Leaves retain the stage-1 primitives. Each node has at most eight rectangle
//! proxies with a conservative accumulated L-infinity Hausdorff error. Geometry
//! is never inferred from a node's bounds at render time.
use super::*;
use std::collections::VecDeque;
use std::io::{Seek, SeekFrom, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

const HEADER: usize = 64;
const GROUP: usize = 32;
const NODE: usize = 112;
const FANOUT: usize = 8;
const PROXIES: usize = 8;
const CAP: usize = 512 * 1024 * 1024;
const RECTANGLES: u32 = 1;

#[derive(Clone, Debug)]
pub struct Options {
    pub direct: bool,
    pub epsilon_px: f64,
    /// Maximum device scale when the viewport is anisotropic. The planner's
    /// minimum scale still controls cut and the conservative viewport halo.
    pub max_px_per_dbu: Option<f64>,
    pub halo_px: f64,
    /// None is the unstyled solid/one-pixel renderer.
    pub solid_layers: Option<Vec<u32>>,
    pub hairline_layers: Option<Vec<u32>>,
    pub nodes_per_batch: u64,
    pub candidates_per_batch: u64,
    pub bytes_per_batch: u64,
    pub output_per_batch: usize,
    pub pixels_per_batch: u64,
    pub micros_per_batch: u64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            direct: false,
            epsilon_px: 0.5,
            max_px_per_dbu: None,
            halo_px: 1.,
            solid_layers: None,
            hairline_layers: None,
            nodes_per_batch: 32768,
            candidates_per_batch: 262144,
            bytes_per_batch: 16 << 20,
            output_per_batch: 65536,
            pixels_per_batch: 16 << 20,
            micros_per_batch: 10000,
        }
    }
}

#[derive(Clone)]
struct Meta {
    bbox: BBox,
    error: u64,
    gate_min: u64,
    gate_max: u64,
    minor: u64,
    first: u32,
    children: u32,
    count: u32,
    flags: u32,
    leaf_off: u64,
    proxy_off: u64,
    proxies: u32,
    leaf_crc: u32,
    proxy_crc: u32,
}
struct BuildNode {
    meta: Meta,
    leaf_start: usize,
    proxy: Vec<Prim>,
}
struct Root {
    layer: u32,
    depth: u32,
    root: usize,
    count: u32,
}
struct NodeRef {
    meta: Meta,
    leaf_ok: AtomicBool,
    proxy_ok: AtomicBool,
}
pub(super) struct Tree {
    roots: Vec<Root>,
    nodes: Vec<NodeRef>,
}

fn contains(a: BBox, b: BBox) -> bool {
    a.x0 <= b.x0 && a.y0 <= b.y0 && a.x1 >= b.x1 && a.y1 >= b.y1
}
fn ceil_half(v: i128) -> u64 {
    ((v.max(0) + 1) / 2).min(u64::MAX as i128) as u64
}
fn distance_to_rect(outer: BBox, inner: BBox) -> u64 {
    // A conservative bound for every point of outer to inner.
    [
        inner.x0 as i128 - outer.x0 as i128,
        outer.x1 as i128 - inner.x1 as i128,
        inner.y0 as i128 - outer.y0 as i128,
        outer.y1 as i128 - inner.y1 as i128,
    ]
    .into_iter()
    .max()
    .unwrap()
    .max(0)
    .min(u64::MAX as i128) as u64
}
fn merge(a: Prim, b: Prim) -> (Prim, u64) {
    let mut bounds = a.bbox();
    bounds.grow(&b.bbox());
    let delta = if a.x0 == b.x0 && a.x1 == b.x1 {
        ceil_half((a.y0.max(b.y0) as i128) - a.y1.min(b.y1) as i128)
    } else if a.y0 == b.y0 && a.y1 == b.y1 {
        ceil_half((a.x0.max(b.x0) as i128) - a.x1.min(b.x1) as i128)
    } else {
        distance_to_rect(bounds, a.bbox()).min(distance_to_rect(bounds, b.bbox()))
    };
    (
        Prim {
            x0: bounds.x0,
            y0: bounds.y0,
            x1: bounds.x1,
            y1: bounds.y1,
            rank: a.rank.min(b.rank),
            gate_dim: a.gate_dim.max(b.gate_dim),
            ..a
        },
        delta,
    )
}
fn summarize(input: &[Prim], inherited: u64) -> (Vec<Prim>, u64) {
    if input.is_empty() || input.iter().any(|p| p.kind != PRIM_RECT) {
        return (vec![], 0);
    }
    // Adjacent candidates only: bounded O(CHUNK^2) local work, never an
    // all-pairs union of a layer. Morton children keep candidates local.
    let mut work: Vec<(Prim, u64)> = input.iter().copied().map(|p| (p, inherited)).collect();
    work.sort_unstable_by_key(|(p, _)| (p.x0, p.x1, p.y0, p.y1, p.rank));
    while work.len() > 1 {
        let best = work
            .windows(2)
            .enumerate()
            .map(|(i, pair)| {
                let (p, delta) = merge(pair[0].0, pair[1].0);
                (pair[0].1.max(pair[1].1).saturating_add(delta), i, p, delta)
            })
            .min_by_key(|(error, i, _, _)| (*error, *i))
            .unwrap();
        if work.len() <= PROXIES && best.3 != 0 {
            break;
        }
        work[best.1] = (best.2, best.0);
        work.remove(best.1 + 1);
    }
    let error = work.iter().map(|p| p.1).max().unwrap_or(0);
    (work.into_iter().map(|p| p.0).collect(), error)
}
fn node(input: &[Prim], leaf_start: usize) -> BuildNode {
    let mut bbox = BBox::EMPTY;
    for p in input {
        bbox.grow(&p.bbox());
    }
    let (proxy, error) = summarize(input, 0);
    BuildNode {
        meta: Meta {
            bbox,
            error,
            gate_min: input.iter().map(|p| p.gate_dim).min().unwrap(),
            gate_max: input.iter().map(|p| p.gate_dim).max().unwrap(),
            minor: input
                .iter()
                .map(|p| {
                    let b = p.bbox();
                    (b.x1 as i128 - b.x0 as i128).min(b.y1 as i128 - b.y0 as i128) as u64
                })
                .max()
                .unwrap(),
            first: 0,
            children: 0,
            count: input.len() as u32,
            flags: if input.iter().all(|p| p.kind == PRIM_RECT) {
                RECTANGLES
            } else {
                0
            },
            leaf_off: 0,
            proxy_off: 0,
            proxies: proxy.len() as u32,
            leaf_crc: 0,
            proxy_crc: 0,
        },
        leaf_start,
        proxy,
    }
}
fn build_nodes(prims: &[Prim]) -> Vec<BuildNode> {
    let mut nodes: Vec<_> = prims
        .chunks(CHUNK)
        .enumerate()
        .map(|(i, ps)| node(ps, i * CHUNK))
        .collect();
    let (mut start, mut end) = (0, nodes.len());
    while end - start > 1 {
        let next = nodes.len();
        for first in (start..end).step_by(FANOUT) {
            let last = (first + FANOUT).min(end);
            let mut meta = nodes[first].meta.clone();
            meta.first = first as u32;
            meta.children = (last - first) as u32;
            meta.count = 0;
            let mut input = Vec::with_capacity(FANOUT * PROXIES);
            let mut complete = true;
            for child in &nodes[first..last] {
                meta.bbox.grow(&child.meta.bbox);
                meta.gate_min = meta.gate_min.min(child.meta.gate_min);
                meta.gate_max = meta.gate_max.max(child.meta.gate_max);
                meta.minor = meta.minor.max(child.meta.minor);
                meta.error = meta.error.max(child.meta.error);
                meta.flags &= child.meta.flags;
                complete &= !child.proxy.is_empty();
                input.extend_from_slice(&child.proxy);
            }
            let (proxy, error) = if complete {
                summarize(&input, meta.error)
            } else {
                (vec![], meta.error)
            };
            meta.error = error;
            meta.proxies = proxy.len() as u32;
            nodes.push(BuildNode {
                meta,
                leaf_start: 0,
                proxy,
            });
        }
        start = next;
        end = nodes.len();
    }
    nodes
}
fn node_count(count: usize) -> usize {
    let mut n = count.div_ceil(CHUNK);
    let mut total = n;
    while n > 1 {
        n = n.div_ceil(FANOUT);
        total += n;
    }
    total
}
fn prim_bytes(ps: &[Prim], group: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(ps.len() * 64);
    for p in ps {
        put64(&mut out, ((group as u64) << 32) | p.rank as u64);
        for v in [p.x0, p.y0, p.x1, p.y1] {
            put64(&mut out, v as u64);
        }
        put64(&mut out, p.gate_dim);
        put64(&mut out, p.thickness);
        out.extend([p.kind, p.flags, 0, 0, 0, 0, 0, 0]);
    }
    out
}
fn put_meta(out: &mut Vec<u8>, m: &Meta) {
    put_bbox(out, m.bbox);
    for v in [m.error, m.gate_min, m.gate_max, m.minor] {
        put64(out, v);
    }
    for v in [m.first, m.children, m.count, m.flags] {
        put32(out, v);
    }
    put64(out, m.leaf_off);
    put64(out, m.proxy_off);
    for v in [m.proxies, m.leaf_crc, m.proxy_crc, 0] {
        put32(out, v);
    }
}
fn read_meta(c: &mut Cursor<'_>) -> Result<Meta, String> {
    let m = Meta {
        bbox: c.bbox()?,
        error: c.u64()?,
        gate_min: c.u64()?,
        gate_max: c.u64()?,
        minor: c.u64()?,
        first: c.u32()?,
        children: c.u32()?,
        count: c.u32()?,
        flags: c.u32()?,
        leaf_off: c.u64()?,
        proxy_off: c.u64()?,
        proxies: c.u32()?,
        leaf_crc: c.u32()?,
        proxy_crc: c.u32()?,
    };
    if c.u32()? != 0 {
        return Err("representatives: unknown tree flags".into());
    }
    Ok(m)
}

/// Streaming writer: only one group's tree and small block buffers are extra
/// to the resolved samples. No second file-sized Vec or bitmap pyramid.
pub fn write<W: Write + Seek>(out: &mut W, built: &mut Built, ovm: &Ovm) -> Result<u64, String> {
    let nodes: usize = built.groups.iter().map(|g| node_count(g.prims.len())).sum();
    if built.groups.len() > MAX_GROUPS
        || built.groups.iter().map(|g| g.prims.len()).sum::<usize>() > MAX_POINTS
    {
        return Err("representatives: tree sample/group limit exceeded".into());
    }
    let meta_end = HEADER + GROUP * built.groups.len() + NODE * nodes + 4;
    let io = |e: std::io::Error| e.to_string();
    out.write_all(&vec![0; meta_end]).map_err(io)?;
    let mut groups = Vec::new();
    let mut metas = Vec::new();
    let mut offset = meta_end as u64;
    for (gi, group) in built.groups.iter_mut().enumerate() {
        if group.prims.is_empty() || group.prims.len() != group.points.len() {
            return Err("representatives: tree requires every sample's shape".into());
        }
        let mut bounds = BBox::EMPTY;
        for p in &group.prims {
            bounds.grow(&p.bbox());
        }
        group.prims.sort_unstable_by_key(|p| {
            let (x, y) = (
                ((p.x0 as i128 + p.x1 as i128) / 2) as i64,
                ((p.y0 as i128 + p.y1 as i128) / 2) as i64,
            );
            // Broad cut bands avoid needless descent across a cut boundary.
            (
                64 - p.gate_dim.leading_zeros(),
                morton_xy(x, y, bounds),
                p.rank,
            )
        });
        let mut tree = build_nodes(&group.prims);
        let base = metas.len();
        for n in [
            group.key.0,
            group.key.1,
            group.key.2,
            (base + tree.len() - 1) as u32,
            group.prims.len() as u32,
            0,
        ] {
            put32(&mut groups, n);
        }
        put64(&mut groups, group.members);
        for n in &mut tree {
            if n.meta.children != 0 {
                n.meta.first += base as u32;
            } else {
                let bytes = prim_bytes(
                    &group.prims[n.leaf_start..n.leaf_start + n.meta.count as usize],
                    gi as u32,
                );
                n.meta.leaf_off = offset;
                n.meta.leaf_crc = crc32fast::hash(&bytes);
                offset += bytes.len() as u64;
                if offset > CAP as u64 {
                    return Err("representatives: tree exceeds 512 MiB".into());
                }
                out.write_all(&bytes).map_err(io)?;
            }
            if !n.proxy.is_empty() {
                let bytes = prim_bytes(&n.proxy, gi as u32);
                n.meta.proxy_off = offset;
                n.meta.proxy_crc = crc32fast::hash(&bytes);
                offset += bytes.len() as u64;
                if offset > CAP as u64 {
                    return Err("representatives: tree exceeds 512 MiB".into());
                }
                out.write_all(&bytes).map_err(io)?;
            }
            metas.push(n.meta.clone());
        }
    }
    let mut meta = Vec::with_capacity(meta_end);
    meta.extend(MAGIC2);
    put32(&mut meta, 3);
    put32(&mut meta, crc32fast::hash(&ovm.data));
    for v in [ovm.src_size, ovm.src_mtime, ovm.unit.to_bits()] {
        put64(&mut meta, v);
    }
    for v in [ovm.top, built.groups.len() as u32, nodes as u32, 0] {
        put32(&mut meta, v);
    }
    put64(&mut meta, meta_end as u64);
    meta.extend(groups);
    for m in &metas {
        put_meta(&mut meta, m);
    }
    let crc = crc32fast::hash(&meta);
    put32(&mut meta, crc);
    debug_assert_eq!(meta.len(), meta_end);
    out.seek(SeekFrom::Start(0)).map_err(io)?;
    out.write_all(&meta).map_err(io)?;
    out.seek(SeekFrom::Start(offset)).map_err(io)?;
    Ok(offset)
}

impl Tree {
    pub(super) fn parse(data: &[u8], ovm: &Ovm) -> Result<Self, String> {
        let bad = || "representatives: invalid tree directory".to_string();
        if data.len() < HEADER + 4 || data.len() > CAP {
            return Err(bad());
        }
        let mut c = Cursor { data, pos: 0 };
        if &c.take::<8>()? != MAGIC2 || c.u32()? != 3 {
            return Err(bad());
        }
        if c.u32()? != crc32fast::hash(&ovm.data)
            || c.u64()? != ovm.src_size
            || c.u64()? != ovm.src_mtime
            || c.u64()? != ovm.unit.to_bits()
            || c.u32()? != ovm.top
        {
            return Err("representatives: cache identity mismatch; rebuild design.ovr".into());
        }
        let ng = c.u32()? as usize;
        let nn = c.u32()? as usize;
        if c.u32()? != 0 {
            return Err(bad());
        }
        let end = c.u64()? as usize;
        if ng > MAX_GROUPS
            || nn > MAX_POINTS * 2
            || end != HEADER + GROUP * ng + NODE * nn + 4
            || end > data.len()
        {
            return Err(bad());
        }
        if crc32fast::hash(&data[..end - 4])
            != u32::from_le_bytes(data[end - 4..end].try_into().unwrap())
        {
            return Err("representatives: tree directory checksum mismatch".into());
        }
        let layer_map: BTreeMap<_, _> = (0..ovm.n_layers)
            .map(|i| {
                let l = ovm.layer(i);
                ((l.layer, l.dt), i)
            })
            .collect();
        let mut roots = Vec::with_capacity(ng);
        let mut previous = None;
        let mut total = 0usize;
        for _ in 0..ng {
            let key = (c.u32()?, c.u32()?, c.u32()?);
            let root = c.u32()? as usize;
            let count = c.u32()?;
            if c.u32()? != 0 {
                return Err(bad());
            }
            let members = c.u64()?;
            total += count as usize;
            if previous.is_some_and(|p| p >= key)
                || key.2 > 4096
                || count == 0
                || count as u64 > members
                || total > MAX_POINTS
                || root >= nn
            {
                return Err(bad());
            }
            previous = Some(key);
            roots.push(Root {
                layer: *layer_map.get(&(key.0, key.1)).ok_or_else(bad)?,
                depth: key.2,
                root,
                count,
            });
        }
        let mut nodes: Vec<NodeRef> = Vec::with_capacity(nn);
        let mut incoming = vec![0u8; nn];
        let mut offset = end as u64;
        for ni in 0..nn {
            let m = read_meta(&mut c)?;
            if m.bbox.is_empty()
                || m.gate_min > m.gate_max
                || m.flags & !RECTANGLES != 0
                || m.proxies as usize > PROXIES
                || (m.proxies > 0 && m.flags != RECTANGLES)
            {
                return Err(bad());
            }
            if m.children == 0 {
                if m.first != 0 || m.count == 0 || m.count as usize > CHUNK || m.leaf_off != offset
                {
                    return Err(bad());
                }
                offset += m.count as u64 * 64;
            } else {
                if m.count != 0
                    || m.leaf_off != 0
                    || m.leaf_crc != 0
                    || m.children as usize > FANOUT
                    || m.first as usize + m.children as usize > ni
                {
                    return Err(bad());
                }
                for ci in m.first as usize..(m.first + m.children) as usize {
                    let child = &nodes[ci].meta;
                    if incoming[ci] != 0
                        || !contains(m.bbox, child.bbox)
                        || m.gate_min > child.gate_min
                        || m.gate_max < child.gate_max
                        || m.minor < child.minor
                        || m.error < child.error
                        || m.flags & child.flags != m.flags
                    {
                        return Err(bad());
                    }
                    incoming[ci] = 1;
                }
            }
            if m.proxies > 0 {
                if m.proxy_off != offset {
                    return Err(bad());
                }
                offset += m.proxies as u64 * 64;
            } else if m.proxy_off != 0 || m.proxy_crc != 0 {
                return Err(bad());
            }
            if offset > data.len() as u64 {
                return Err(bad());
            }
            nodes.push(NodeRef {
                meta: m,
                leaf_ok: AtomicBool::new(false),
                proxy_ok: AtomicBool::new(false),
            });
        }
        // Contiguous, independently rooted groups; no alias, cycle or orphan.
        let mut start = 0;
        for root in &roots {
            if root.root < start || incoming[root.root] != 0 {
                return Err(bad());
            }
            let mut count = 0;
            for i in start..=root.root {
                let m = &nodes[i].meta;
                count += m.count as usize;
                if (i < root.root && incoming[i] != 1)
                    || (m.children > 0 && (m.first as usize) < start)
                {
                    return Err(bad());
                }
            }
            if count != root.count as usize {
                return Err(bad());
            }
            start = root.root + 1;
        }
        if start != nn || offset != data.len() as u64 {
            return Err(bad());
        }
        Ok(Self { roots, nodes })
    }
    fn block(&self, data: &[u8], ni: usize, gi: usize, proxy: bool) -> Result<Vec<Prim>, String> {
        let node = &self.nodes[ni];
        let m = &node.meta;
        let (off, n, crc, valid) = if proxy {
            (m.proxy_off, m.proxies, m.proxy_crc, &node.proxy_ok)
        } else {
            (m.leaf_off, m.count, m.leaf_crc, &node.leaf_ok)
        };
        let bytes = &data[off as usize..off as usize + n as usize * 64];
        if !valid.load(Ordering::Acquire) && crc32fast::hash(bytes) != crc {
            return Err("representatives: tree block checksum mismatch".into());
        }
        let mut c = Cursor {
            data: bytes,
            pos: 0,
        };
        let mut prims = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let (p, group) = c.prim()?;
            if group as usize != gi
                || p.rank >= self.roots[gi].count
                || !contains(m.bbox, p.bbox())
                || (p.kind == PRIM_RECT && (p.x0 > p.x1 || p.y0 > p.y1))
                || (proxy && p.kind != PRIM_RECT)
                || (m.flags == RECTANGLES && p.kind != PRIM_RECT)
                || p.gate_dim < m.gate_min
                || p.gate_dim > m.gate_max
            {
                return Err("representatives: invalid tree primitive".into());
            }
            prims.push(p);
        }
        valid.store(true, Ordering::Release);
        Ok(prims)
    }
}

/// Resumes after a work/IO/paint budget without dropping any region. Every
/// non-final batch is explicitly a refinement; no silent first-N truncation.
pub struct Stream {
    file: Arc<File>,
    view: BBox,
    cut: u64,
    ppd: f64,
    options: Options,
    queue: VecDeque<(usize, usize)>,
    pending: VecDeque<(u32, Prim)>,
    pub stats: QueryStats,
}
impl Stream {
    pub fn new(file: Arc<File>, req: &crate::ViewReq, options: Options) -> Self {
        let bit = |bits: &[u8], l: u32| {
            bits.get(l as usize / 8)
                .is_some_and(|b| b & (1 << (l % 8)) != 0)
        };
        let tree = file.tree.as_ref().expect("tree stream requires revision 3");
        let queue = if req.cut_dbu <= 0 || !req.px_per_dbu.is_finite() || req.px_per_dbu <= 0. {
            VecDeque::new()
        } else {
            tree.roots
                .iter()
                .enumerate()
                .filter(|(_, g)| {
                    g.depth <= req.depth && bit(&req.vis, g.layer) && !bit(&req.page_skip, g.layer)
                })
                .map(|(i, g)| (i, g.root))
                .collect()
        };
        // Same stroke halo for nodes, proxies and leaves, including long
        // off-centre lines and strokes just outside the geometric viewport.
        let halo = if req.px_per_dbu > 0. {
            (options.halo_px.max(1.) / req.px_per_dbu)
                .ceil()
                .min(i64::MAX as f64) as i64
        } else {
            0
        };
        let view = BBox {
            x0: req.view.x0.saturating_sub(halo),
            y0: req.view.y0.saturating_sub(halo),
            x1: req.view.x1.saturating_add(halo),
            y1: req.view.y1.saturating_add(halo),
        };
        let ppd = options
            .max_px_per_dbu
            .filter(|p| p.is_finite())
            .unwrap_or(req.px_per_dbu)
            .max(req.px_per_dbu);
        Self {
            file,
            view,
            cut: req.cut_dbu.max(0) as u64,
            ppd,
            options,
            queue,
            pending: VecDeque::new(),
            stats: QueryStats::default(),
        }
    }
    pub fn is_done(&self) -> bool {
        self.queue.is_empty() && self.pending.is_empty()
    }
    pub fn next(&mut self, cancelled: impl Fn() -> bool) -> Result<Vec<(u32, Prim)>, String> {
        let start = self.stats.clone();
        let started = std::time::Instant::now();
        let mut iterations = 0u64;
        let mut out = Vec::new();
        while !self.is_done() {
            if cancelled() {
                return Err("render cancelled".into());
            }
            if self.stats.nodes - start.nodes >= self.options.nodes_per_batch.max(1)
                || self.stats.tested - start.tested >= self.options.candidates_per_batch.max(1)
                || self.stats.bytes - start.bytes >= self.options.bytes_per_batch.max(8192)
                || out.len() >= self.options.output_per_batch.max(1)
                || self.stats.pixels - start.pixels >= self.options.pixels_per_batch.max(1)
                || (iterations > 0
                    && iterations % 128 == 0
                    && started.elapsed().as_micros()
                        >= self.options.micros_per_batch.max(1) as u128)
            {
                self.stats.limited = true;
                break;
            }
            iterations += 1;
            if let Some((layer, p)) = self.pending.pop_front() {
                let b = p.bbox();
                let w = (b.x1.min(self.view.x1) as i128 - b.x0.max(self.view.x0) as i128).max(0)
                    as f64
                    * self.ppd;
                let h = (b.y1.min(self.view.y1) as i128 - b.y0.max(self.view.y0) as i128).max(0)
                    as f64
                    * self.ppd;
                // Conservative clipped paint estimate (area for rects, length
                // for edges); charged even if another shape overlaps it.
                let pixels = if p.kind == PRIM_SEGMENT {
                    w + h + 2.
                } else {
                    (w + 2.) * (h + 2.)
                };
                self.stats.pixels = self.stats.pixels.saturating_add(pixels.ceil() as u64);
                self.stats.points += 1;
                out.push((layer, p));
                continue;
            }
            let (gi, ni) = self.queue.pop_front().unwrap();
            self.stats.nodes += 1;
            let tree = self.file.tree.as_ref().unwrap();
            let m = &tree.nodes[ni].meta;
            let layer = tree.roots[gi].layer;
            if !m.bbox.intersects(&self.view) || m.gate_min >= self.cut {
                continue;
            }
            let hairline = (m.minor as f64) * self.ppd < 1.;
            let allows = |ls: &Option<Vec<u32>>| ls.as_ref().is_none_or(|ls| ls.contains(&layer));
            let proxy = !self.options.direct
                && m.proxies > 0
                && m.gate_max < self.cut
                && m.error as f64 * self.ppd <= self.options.epsilon_px
                && (allows(&self.options.solid_layers)
                    || (hairline && allows(&self.options.hairline_layers)));
            if !proxy && m.children > 0 {
                for child in m.first..m.first + m.children {
                    self.queue.push_back((gi, child as usize));
                }
                continue;
            }
            self.stats.bytes += if proxy {
                m.proxies as u64 * 64
            } else {
                m.count as u64 * 64
            };
            self.stats.chunks += 1;
            if proxy {
                self.stats.proxy_nodes += 1;
            }
            for mut p in tree.block(&self.file.data, ni, gi, proxy)? {
                self.stats.tested += 1;
                if p.gate_dim >= self.cut || !p.bbox().intersects(&self.view) {
                    continue;
                }
                if proxy && hairline {
                    p.flags |= PRIM_MERGED_SOLID;
                }
                self.pending.push_back((layer, p));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rect(x: i64, y: i64, w: i64, h: i64, rank: u32) -> Prim {
        Prim {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
            gate_dim: (w.min(h) as u64) * 2,
            thickness: 0,
            kind: PRIM_RECT,
            flags: 0,
            rank,
        }
    }
    fn ovm() -> Ovm {
        let mut b = floe_ovm::Builder::new(1000., 123, 456, 1);
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
    fn encoded(prims: Vec<Prim>) -> (Vec<u8>, Ovm) {
        encoded_groups(vec![(0, prims)])
    }
    fn encoded_groups(groups: Vec<(u32, Vec<Prim>)>) -> (Vec<u8>, Ovm) {
        let mut built = Built {
            groups: groups
                .into_iter()
                .map(|(depth, prims)| Group {
                    key: (1, 0, depth),
                    members: prims.len() as u64,
                    points: prims
                        .iter()
                        .map(|p| Point {
                            x: p.x0,
                            y: p.y0,
                            max_dim: 1,
                            min_dim: 1,
                            rank: p.rank,
                        })
                        .collect(),
                    prims,
                })
                .collect(),
            directory: 1,
            peak_requests: 0,
            point_fallbacks: 0,
        };
        let ovm = ovm();
        let mut out = std::io::Cursor::new(Vec::new());
        write(&mut out, &mut built, &ovm).unwrap();
        (out.into_inner(), ovm)
    }
    fn request(ppd: f64) -> crate::ViewReq {
        crate::ViewReq {
            view: BBox {
                x0: -100,
                y0: -100,
                x1: 1_000_000,
                y1: 1_000_000,
            },
            cut_dbu: 100,
            vis: vec![1],
            depth: 0,
            px_per_dbu: ppd,
            sub_cut_wash: false,
            page_reps: false,
            decode_budget: 0,
            page_hairline: true,
            page_skip: vec![],
            prune_skipped: false,
            sub_cut_box: false,
            shape_cut: false,
            frames: true,
            page_wash: true,
        }
    }
    fn stream(prims: Vec<Prim>, req: &crate::ViewReq, options: Options) -> Stream {
        let (bytes, ovm) = encoded(prims);
        let f = Arc::new(File::from_backing(Backing::Vec(bytes), &ovm).unwrap());
        Stream::new(f, req, options)
    }
    fn collect(s: &mut Stream) -> Vec<(u32, Prim)> {
        let mut out = Vec::new();
        while !s.is_done() {
            out.extend(s.next(|| false).unwrap());
        }
        out
    }
    #[test]
    fn dense_parallel_lines_merge_and_refine_without_global_rank_thinning() {
        let prims: Vec<_> = (0..8192)
            .map(|i| rect(i * 4, 0, 2, 10000, i as u32))
            .collect();
        let mut far = stream(prims.clone(), &request(0.001), Options::default());
        assert!(collect(&mut far).len() <= 8);
        assert_eq!(far.stats.nodes, 1);
        assert!(far.stats.bytes <= 512 && far.stats.tested <= 8);
        let mut near = stream(prims, &request(1.), Options::default());
        assert_eq!(collect(&mut near).len(), 8192);
        assert!(near.stats.nodes > 1);
    }
    #[test]
    fn separated_clusters_do_not_fill_the_gap_and_long_extents_are_queried() {
        let prims: Vec<_> = (0..256)
            .map(|i| rect(if i < 128 { 0 } else { 100000 }, 0, 2, 10000, i))
            .collect();
        let mut req = request(0.001);
        req.view = BBox {
            x0: 20000,
            y0: 100,
            x1: 21000,
            y1: 200,
        };
        assert!(collect(&mut stream(prims.clone(), &req, Options::default())).is_empty());
        req.view = BBox {
            x0: 0,
            y0: 9000,
            x1: 100,
            y1: 9100,
        };
        let mut s = stream(prims, &req, Options::default());
        assert_eq!(collect(&mut s).len(), 1);
    }
    #[test]
    fn resumable_batches_equal_direct_query_and_cancel_without_dropping_tail() {
        let prims: Vec<_> = (0..1000)
            .map(|i| rect(i * 40, 0, 2, 100, i as u32))
            .collect();
        let req = request(0.01);
        let mut expected = stream(
            prims.clone(),
            &req,
            Options {
                direct: true,
                ..Options::default()
            },
        );
        let mut s = stream(
            prims,
            &req,
            Options {
                direct: true,
                output_per_batch: 3,
                nodes_per_batch: 2,
                ..Options::default()
            },
        );
        assert!(s.next(|| true).unwrap_err().contains("cancelled"));
        assert_eq!(collect(&mut s), collect(&mut expected));
        assert!(s.stats.limited);
        assert_eq!(s.stats.points, 1000);
    }
    #[test]
    fn cut_skip_depth_style_and_roi_prune_before_leaf_reads() {
        let prims: Vec<_> = (0..1024)
            .map(|i| rect(i * 4, 0, if i % 2 == 0 { 1 } else { 2 }, 10000, i as u32))
            .collect();
        let mut req = request(0.001);
        req.cut_dbu = 3;
        let mut s = stream(
            prims.clone(),
            &req,
            Options {
                direct: true,
                ..Options::default()
            },
        );
        assert_eq!(collect(&mut s).len(), 512);
        req.page_skip = vec![1];
        let mut s = stream(prims.clone(), &req, Options::default());
        assert!(collect(&mut s).is_empty());
        assert_eq!(s.stats.bytes, 0);
        req.page_skip.clear();
        req.cut_dbu = 100;
        let mut s = stream(
            prims.clone(),
            &req,
            Options {
                solid_layers: Some(vec![]),
                hairline_layers: Some(vec![]),
                ..Options::default()
            },
        );
        assert_eq!(collect(&mut s).len(), 1024);
        assert_eq!(s.stats.proxy_nodes, 0);
        req.view = BBox {
            x0: 900000,
            y0: 900000,
            x1: 901000,
            y1: 901000,
        };
        let mut s = stream(prims, &req, Options::default());
        assert!(collect(&mut s).is_empty());
        assert_eq!(s.stats.nodes, 1);
        assert_eq!(s.stats.bytes, 0);
    }
    #[test]
    fn block_corruption_is_lazy_and_directory_corruption_is_rejected() {
        let (bytes, ovm) = encoded(
            (0..256)
                .map(|i| rect(i * 4, 0, 2, 1000, i as u32))
                .collect(),
        );
        let mut corrupt = bytes.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        let f = Arc::new(File::from_backing(Backing::Vec(corrupt), &ovm).unwrap());
        let mut s = Stream::new(f, &request(0.0001), Options::default());
        assert!(s.next(|| false).unwrap_err().contains("checksum"));
        let mut corrupt = bytes.clone();
        corrupt[HEADER + GROUP] ^= 1;
        assert!(File::from_backing(Backing::Vec(corrupt), &ovm).is_err());
        assert!(File::from_backing(Backing::Vec(bytes[..last].to_vec()), &ovm).is_err());
    }
    #[test]
    fn depth_visibility_non_rectangles_and_small_roi_keep_only_local_samples() {
        let mut edge = rect(5, 5, 10, 10, 0);
        edge.kind = PRIM_SEGMENT;
        let (bytes, ovm) =
            encoded_groups(vec![(0, vec![edge]), (2, vec![rect(10, 10, 1, 100, 0)])]);
        let f = Arc::new(File::from_backing(Backing::Vec(bytes), &ovm).unwrap());
        let mut req = request(0.001);
        let mut s = Stream::new(Arc::clone(&f), &req, Options::default());
        assert_eq!(collect(&mut s), vec![(0, edge)]);
        req.depth = 2;
        let mut s = Stream::new(Arc::clone(&f), &req, Options::default());
        assert_eq!(collect(&mut s).len(), 2);
        req.vis.clear();
        let mut s = Stream::new(f, &req, Options::default());
        assert!(collect(&mut s).is_empty());
        assert_eq!(s.stats.nodes, 0);

        req = request(1.);
        req.view = BBox {
            x0: 16000,
            y0: 0,
            x1: 16100,
            y1: 20,
        };
        let mut s = stream(
            (0..8192).map(|i| rect(i * 4, 0, 1, 10, i as u32)).collect(),
            &req,
            Options::default(),
        );
        let local = collect(&mut s);
        assert!((24..=30).contains(&local.len()));
        assert!(s.stats.nodes < 64 && s.stats.tested <= 256, "{:?}", s.stats);
    }
    #[test]
    fn anisotropic_scale_and_wide_outline_halo_are_conservative() {
        let req = request(0.001);
        let mut s = stream(
            (0..1024)
                .map(|i| rect(i * 4, 0, 1, 100, i as u32))
                .collect(),
            &req,
            Options {
                max_px_per_dbu: Some(1.),
                ..Options::default()
            },
        );
        assert_eq!(collect(&mut s).len(), 1024);
        let mut req = request(1.);
        req.view = BBox {
            x0: 0,
            y0: 0,
            x1: 100,
            y1: 100,
        };
        let prims = vec![rect(104, 10, 1, 20, 0)];
        assert!(collect(&mut stream(prims.clone(), &req, Options::default())).is_empty());
        assert_eq!(
            collect(&mut stream(
                prims,
                &req,
                Options {
                    halo_px: 5.,
                    ..Options::default()
                }
            ))
            .len(),
            1
        );
    }
    #[test]
    fn conservative_merge_error_bounds_both_sets_on_a_small_geometry_oracle() {
        let prims: Vec<_> = (0..32)
            .map(|i| rect(i * 3, (i % 3) * 2, 1 + (i % 2), 8 + (i % 5), i as u32))
            .collect();
        let (proxies, error) = summarize(&prims, 0);
        let distance = |x: i64, y: i64, ps: &[Prim]| {
            ps.iter()
                .map(|p| (p.x0 - x).max(x - p.x1).max(p.y0 - y).max(y - p.y1).max(0) as u64)
                .min()
                .unwrap()
        };
        for x in -1..100 {
            for y in -1..20 {
                if distance(x, y, &prims) == 0 {
                    assert!(distance(x, y, &proxies) <= error);
                }
                if distance(x, y, &proxies) == 0 {
                    assert!(distance(x, y, &prims) <= error);
                }
            }
        }
    }
}
