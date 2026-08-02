//! Per-layer coverage bitplanes (design.ovc) - the Calibre-style
//! density overview, done efficiently (rust/VFS.md V3b).
//!
//! Coverage is a world-space, per-layer, 8-bit density map at a
//! coarse mipmap. It is DISPLAY-ONLY: the viewer tints it with the
//! live palette so a fill field that the cut drops still shows as a
//! density block instead of blanking. It never flattens the design:
//!   * per-cell recursive covered area per layer is a scalar
//!     computed bottom-up (like recursive member counts);
//!   * a top-down splat descends only until a cell/array projects to
//!     ~a texel, then adds its recursive area as uniform density -
//!     so a dense array folds to one density region, O(records +
//!     texels), no member expansion.

use floe_ovm::BBox;
use floe_oasis::doc::{Doc, Rep};
use floe_tiler::hier::{cell_bboxes, rep_extent};
use floe_tiler::Xf;

pub const MAGIC: &[u8; 8] = b"FLOEOVC1";
pub const VERSION: u32 = 1;
/// finest mip: texels across the longer die axis
pub const BASE_RES: u32 = 512;
/// stop descending when a node projects below this many finest texels
const TEXEL_CUTOFF: f64 = 1.5;

fn poly_area(pts: &[(i64, i64)]) -> f64 {
    // shoelace, absolute
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0i128;
    for i in 0..n {
        let (x1, y1) = pts[i];
        let (x2, y2) = pts[(i + 1) % n];
        a += x1 as i128 * y2 as i128 - x2 as i128 * y1 as i128;
    }
    (a.unsigned_abs() as f64) / 2.0
}

fn path_area(pts: &[(i64, i64)], hw: i64) -> f64 {
    // spine length * full width (extensions ignored: coarse density)
    let mut len = 0.0;
    for w in pts.windows(2) {
        let dx = (w[1].0 - w[0].0) as f64;
        let dy = (w[1].1 - w[0].1) as f64;
        len += (dx * dx + dy * dy).sqrt();
    }
    len * (2 * hw) as f64
}

/// recursive covered area per (cell, layer index), members included,
/// bottom-up over the cell DAG (fixpoint - child order not assumed).
fn recursive_area(
    doc: &Doc,
    nl: usize,
    lidx: &std::collections::HashMap<(u32, u32), usize>,
) -> Vec<Vec<f64>> {
    let n = doc.cells.len();
    let mut direct = vec![vec![0f64; nl]; n];
    for (ci, cell) in doc.cells.iter().enumerate() {
        for r in &cell.rects {
            let li = lidx[&(r.layer, r.dt)];
            direct[ci][li] +=
                (r.w * r.h) as f64 * r.rep.members() as f64;
        }
        for p in &cell.polys {
            let li = lidx[&(p.layer, p.dt)];
            direct[ci][li] +=
                poly_area(&p.pts) * p.rep.members() as f64;
        }
        for pa in &cell.paths {
            let li = lidx[&(pa.layer, pa.dt)];
            direct[ci][li] +=
                path_area(&pa.pts, pa.hw) * pa.rep.members() as f64;
        }
    }
    let mut area = direct.clone();
    loop {
        let mut changed = false;
        for ci in 0..n {
            let mut a = direct[ci].clone();
            for pl in &doc.cells[ci].places {
                let m = pl.rep.members() as f64;
                for l in 0..nl {
                    a[l] += m * area[pl.cell][l];
                }
            }
            if a != area[ci] {
                area[ci] = a;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    area
}

pub struct Coverage {
    pub res_x: u32,
    pub res_y: u32,
    pub n_layers: usize,
    pub die: BBox,
    /// finest-level accumulators [layer][y*res_x + x], density 0..~1+
    planes: Vec<Vec<f32>>,
}

impl Coverage {
    /// world -> finest texel scale
    fn build(
        doc: &Doc,
        layer_order: &[(u32, u32)],
        jobs: usize,
    ) -> Coverage {
        let nl = layer_order.len();
        let lidx: std::collections::HashMap<(u32, u32), usize> =
            layer_order
                .iter()
                .enumerate()
                .map(|(i, &k)| (k, i))
                .collect();
        let bboxes = cell_bboxes(doc);
        let die = match bboxes[doc.top] {
            Some((x0, y0, x1, y1)) => BBox { x0, y0, x1, y1 },
            None => BBox { x0: 0, y0: 0, x1: 1, y1: 1 },
        };
        let dw = (die.x1 - die.x0).max(1) as f64;
        let dh = (die.y1 - die.y0).max(1) as f64;
        let (res_x, res_y) = if dw >= dh {
            (BASE_RES, ((BASE_RES as f64 * dh / dw).ceil() as u32).max(1))
        } else {
            (((BASE_RES as f64 * dw / dh).ceil() as u32).max(1), BASE_RES)
        };
        let texw = dw / res_x as f64;
        let texh = dh / res_y as f64;
        let area = recursive_area(doc, nl, &lidx);
        let mut planes: Vec<Vec<f32>> = Vec::new();
        planes.resize_with(nl, Vec::new);
        let mut cov = Coverage {
            res_x,
            res_y,
            n_layers: nl,
            die,
            planes,
        };
        let ctx = SplatCtx {
            doc,
            lidx: &lidx,
            bboxes: &bboxes,
            area: &area,
            texw,
            texh,
        };
        // the top cell's direct records (usually few) on the main
        // thread, then its placements (the bulk - blocks, fill) split
        // across workers, each into its own accumulator, then summed.
        // Independent subtrees -> deterministic sum, order-invariant.
        cov.splat_direct(&ctx, doc.top, &Xf::identity());
        let places = &doc.cells[doc.top].places;
        let total = places.len();
        let done = std::sync::atomic::AtomicUsize::new(0);
        let id = Xf::identity();
        let nthreads = jobs.max(1).min(8);
        if nthreads <= 1 || total < 2 {
            for pl in places {
                cov.splat_place(&ctx, pl, &id);
            }
        } else {
            let t0 = std::time::Instant::now();
            let next = std::sync::atomic::AtomicUsize::new(0);
            let parts: Vec<Coverage> = std::thread::scope(|s| {
                // heartbeat: coverage splat can run for minutes on a
                // big chip; report progress so it never looks hung
                {
                    let done = &done;
                    s.spawn(move || {
                        use std::sync::atomic::Ordering::Relaxed;
                        let mut last = t0;
                        loop {
                            std::thread::sleep(
                                std::time::Duration::from_millis(200),
                            );
                            if done.load(Relaxed) >= total {
                                return;
                            }
                            if last.elapsed().as_secs_f64() >= 5.0 {
                                last = std::time::Instant::now();
                                eprintln!(
                                    "[vfs] coverage {}/{} subtrees \
                                     ({}s)",
                                    done.load(Relaxed),
                                    total,
                                    t0.elapsed().as_secs()
                                );
                            }
                        }
                    });
                }
                let handles: Vec<_> = (0..nthreads)
                    .map(|_| {
                        let ctx = &ctx;
                        let next = &next;
                        let done = &done;
                        let base = cov.empty_like();
                        s.spawn(move || {
                            use std::sync::atomic::Ordering::Relaxed;
                            let mut w = base;
                            loop {
                                let i = next.fetch_add(1, Relaxed);
                                if i >= total {
                                    break;
                                }
                                w.splat_place(
                                    ctx, &places[i], &id,
                                );
                                done.fetch_add(1, Relaxed);
                            }
                            w
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("coverage worker"))
                    .collect()
            });
            for p in &parts {
                cov.merge(p);
            }
        }
        cov
    }

    /// add `density` over the world rect [wx0,wy0,wx1,wy1] on layer
    fn splat_rect(
        &mut self,
        layer: usize,
        w: &BBox,
        density: f32,
        texw: f64,
        texh: f64,
    ) {
        if density <= 0.0 || w.is_empty() {
            return;
        }
        let cx0 = ((w.x0 - self.die.x0) as f64 / texw)
            .floor()
            .max(0.0) as u32;
        let cy0 = ((w.y0 - self.die.y0) as f64 / texh)
            .floor()
            .max(0.0) as u32;
        let cx1 = (((w.x1 - self.die.x0) as f64 / texw).ceil()
            as i64)
            .clamp(0, self.res_x as i64) as u32;
        let cy1 = (((w.y1 - self.die.y0) as f64 / texh).ceil()
            as i64)
            .clamp(0, self.res_y as i64) as u32;
        if cx1 <= cx0 || cy1 <= cy0 {
            return;
        }
        let rx = self.res_x;
        let plane = &mut self.planes[layer];
        if plane.is_empty() {
            plane.resize((self.res_x * self.res_y) as usize, 0.0);
        }
        for cy in cy0..cy1 {
            let row = (cy * rx) as usize;
            for cx in cx0..cx1 {
                plane[row + cx as usize] += density;
            }
        }
    }
}

struct SplatCtx<'a> {
    doc: &'a Doc,
    lidx: &'a std::collections::HashMap<(u32, u32), usize>,
    bboxes: &'a [Option<(i64, i64, i64, i64)>],
    area: &'a [Vec<f64>],
    texw: f64,
    texh: f64,
}

impl Coverage {
    fn world_bbox(xf: &Xf, b: (i64, i64, i64, i64)) -> BBox {
        let a = xf.apply(b.0, b.1);
        let c = xf.apply(b.2, b.3);
        BBox {
            x0: a.0.min(c.0),
            y0: a.1.min(c.1),
            x1: a.0.max(c.0),
            y1: a.1.max(c.1),
        }
    }

    fn splat_uniform_cell(
        &mut self,
        ctx: &SplatCtx,
        ci: usize,
        wb: &BBox,
        mult: f64,
    ) {
        // distribute each layer's recursive area uniformly over wb
        let a = (wb.x1 - wb.x0).max(1) as f64
            * (wb.y1 - wb.y0).max(1) as f64;
        let (texw, texh) = (ctx.texw, ctx.texh);
        for l in 0..self.n_layers {
            let area = ctx.area[ci][l] * mult;
            if area > 0.0 {
                self.splat_rect(l, wb, (area / a) as f32, texw, texh);
            }
        }
    }

    fn splat_cell(&mut self, ctx: &SplatCtx, ci: usize, xf: &Xf) {
        self.splat_direct(ctx, ci, xf);
        let places = ctx.doc.cells[ci].places.clone();
        for pl in &places {
            self.splat_place(ctx, pl, xf);
        }
    }

    /// direct (own-layer) records of a cell into their world bboxes
    fn splat_direct(&mut self, ctx: &SplatCtx, ci: usize, xf: &Xf) {
        let cell = &ctx.doc.cells[ci];
        let (texw, texh) = (ctx.texw, ctx.texh);
        for r in &cell.rects {
            let l = ctx.lidx[&(r.layer, r.dt)];
            let (ex, ey) = rep_extent(&r.rep);
            let lb = (
                r.x + ex.0.min(0),
                r.y + ey.0.min(0),
                r.x + r.w + ex.1.max(0),
                r.y + r.h + ey.1.max(0),
            );
            let wb = Coverage::world_bbox(xf, lb);
            let a = (wb.x1 - wb.x0).max(1) as f64
                * (wb.y1 - wb.y0).max(1) as f64;
            let area = (r.w * r.h) as f64 * r.rep.members() as f64;
            self.splat_rect(l, &wb, (area / a) as f32, texw, texh);
        }
        for p in &cell.polys {
            let l = ctx.lidx[&(p.layer, p.dt)];
            let mut lb = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
            for &(x, y) in &p.pts {
                lb.0 = lb.0.min(x);
                lb.1 = lb.1.min(y);
                lb.2 = lb.2.max(x);
                lb.3 = lb.3.max(y);
            }
            let (ex, ey) = rep_extent(&p.rep);
            let lb = (lb.0 + ex.0.min(0), lb.1 + ey.0.min(0),
                      lb.2 + ex.1.max(0), lb.3 + ey.1.max(0));
            let wb = Coverage::world_bbox(xf, lb);
            let a = (wb.x1 - wb.x0).max(1) as f64
                * (wb.y1 - wb.y0).max(1) as f64;
            let area = poly_area(&p.pts) * p.rep.members() as f64;
            self.splat_rect(l, &wb, (area / a) as f32, texw, texh);
        }
        for pa in &cell.paths {
            let l = ctx.lidx[&(pa.layer, pa.dt)];
            let b4 = floe_tiler::path_bbox(&pa.pts, pa.hw, pa.es, pa.ee);
            let (ex, ey) = rep_extent(&pa.rep);
            let lb = (b4.0 + ex.0.min(0), b4.1 + ey.0.min(0),
                      b4.2 + ex.1.max(0), b4.3 + ey.1.max(0));
            let wb = Coverage::world_bbox(xf, lb);
            let a = (wb.x1 - wb.x0).max(1) as f64
                * (wb.y1 - wb.y0).max(1) as f64;
            let area = path_area(&pa.pts, pa.hw)
                * pa.rep.members() as f64;
            self.splat_rect(l, &wb, (area / a) as f32, texw, texh);
        }
    }

    /// one placement: descend (One, big enough) or fold to uniform
    /// density (array, or a subtree below texel granularity)
    fn splat_place(
        &mut self,
        ctx: &SplatCtx,
        pl: &floe_oasis::doc::PlaceRec,
        xf: &Xf,
    ) {
        let (texw, texh) = (ctx.texw, ctx.texh);
        let cb = match ctx.bboxes[pl.cell] {
            Some(b) => b,
            None => return,
        };
        match &pl.rep {
            Rep::One => {
                let base =
                    xf.compose(&Xf::place(pl.x, pl.y, pl.rot, pl.flip));
                let wb = Coverage::world_bbox(&base, cb);
                if self.small(&wb, texw, texh) {
                    self.splat_uniform_cell(ctx, pl.cell, &wb, 1.0);
                } else {
                    self.splat_cell(ctx, pl.cell, &base);
                }
            }
            rep => {
                // whole array footprint in world; fold to uniform
                // density = members * recursive area / footprint
                let base = Xf::place(pl.x, pl.y, pl.rot, pl.flip);
                let cw =
                    Coverage::world_bbox(&xf.compose(&base), cb);
                let (rx, ry) = rep_extent(rep);
                let wb = grow_rep(xf, &cw, &rx, &ry);
                let members = rep.members() as f64;
                self.splat_uniform_cell(ctx, pl.cell, &wb, members);
            }
        }
    }

    /// empty clone (same grid, no accumulated density) for a worker
    fn empty_like(&self) -> Coverage {
        let mut planes: Vec<Vec<f32>> = Vec::new();
        planes.resize_with(self.n_layers, Vec::new);
        Coverage {
            res_x: self.res_x,
            res_y: self.res_y,
            n_layers: self.n_layers,
            die: self.die,
            planes,
        }
    }

    /// sum another worker's planes into this one
    fn merge(&mut self, other: &Coverage) {
        for l in 0..self.n_layers {
            if other.planes[l].is_empty() {
                continue;
            }
            if self.planes[l].is_empty() {
                self.planes[l] =
                    vec![0.0; (self.res_x * self.res_y) as usize];
            }
            for (a, b) in
                self.planes[l].iter_mut().zip(&other.planes[l])
            {
                *a += *b;
            }
        }
    }

    fn small(&self, wb: &BBox, texw: f64, texh: f64) -> bool {
        (wb.x1 - wb.x0) as f64 <= TEXEL_CUTOFF * texw
            && (wb.y1 - wb.y0) as f64 <= TEXEL_CUTOFF * texh
    }
}

/// grow a base world bbox by a repetition's world-space offset extent
fn grow_rep(
    xf: &Xf,
    base: &BBox,
    rx: &(i64, i64),
    ry: &(i64, i64),
) -> BBox {
    let mut wx0 = 0i64;
    let mut wx1 = 0i64;
    let mut wy0 = 0i64;
    let mut wy1 = 0i64;
    for &(ox, oy) in
        &[(rx.0, ry.0), (rx.1, ry.0), (rx.0, ry.1), (rx.1, ry.1)]
    {
        let (dx, dy) = xf.apply_vec(ox, oy);
        wx0 = wx0.min(dx);
        wx1 = wx1.max(dx);
        wy0 = wy0.min(dy);
        wy1 = wy1.max(dy);
    }
    BBox {
        x0: base.x0 + wx0,
        y0: base.y0 + wy0,
        x1: base.x1 + wx1,
        y1: base.y1 + wy1,
    }
}

// -------------------------------------------------------- serialize

/// pack to design.ovc bytes: finest planes downsampled into a mip
/// pyramid, only non-empty (layer, level) planes stored (8-bit,
/// density clamped to 1.0 -> 255).
pub fn write_ovc(
    doc: &Doc,
    layer_order: &[(u32, u32)],
    jobs: usize,
) -> Vec<u8> {
    let cov = Coverage::build(doc, layer_order, jobs);
    let nl = cov.n_layers;
    // build mip levels (halving) down to <=8 on the longer axis
    let mut levels: Vec<(u32, u32, Vec<Vec<u8>>)> = Vec::new();
    let mut cur_rx = cov.res_x;
    let mut cur_ry = cov.res_y;
    // level 0 quantized
    let quant = |plane: &Vec<f32>, rx: u32, ry: u32| -> Vec<u8> {
        if plane.is_empty() {
            return Vec::new();
        }
        let mut out = vec![0u8; (rx * ry) as usize];
        for i in 0..out.len() {
            let v = (plane[i] * 255.0).round();
            out[i] = v.clamp(0.0, 255.0) as u8;
        }
        out
    };
    let mut cur: Vec<Vec<f32>> = cov.planes;
    loop {
        let planes8: Vec<Vec<u8>> =
            cur.iter().map(|p| quant(p, cur_rx, cur_ry)).collect();
        levels.push((cur_rx, cur_ry, planes8));
        if cur_rx <= 8 && cur_ry <= 8 {
            break;
        }
        let nrx = (cur_rx / 2).max(1);
        let nry = (cur_ry / 2).max(1);
        let mut nxt: Vec<Vec<f32>> = Vec::new();
        nxt.resize_with(nl, Vec::new);
        for l in 0..nl {
            if cur[l].is_empty() {
                continue;
            }
            let mut dn = vec![0f32; (nrx * nry) as usize];
            for y in 0..nry {
                for x in 0..nrx {
                    // average of the 2x2 block (mean density)
                    let mut s = 0f32;
                    let mut c = 0f32;
                    for dy in 0..2u32 {
                        for dx in 0..2u32 {
                            let sx = x * 2 + dx;
                            let sy = y * 2 + dy;
                            if sx < cur_rx && sy < cur_ry {
                                s += cur[l]
                                    [(sy * cur_rx + sx) as usize];
                                c += 1.0;
                            }
                        }
                    }
                    dn[(y * nrx + x) as usize] =
                        if c > 0.0 { s / c } else { 0.0 };
                }
            }
            nxt[l] = dn;
        }
        cur = nxt;
        cur_rx = nrx;
        cur_ry = nry;
    }

    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put32(&mut out, VERSION);
    out.extend_from_slice(&doc.unit.to_le_bytes());
    for v in [cov.die.x0, cov.die.y0, cov.die.x1, cov.die.y1] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    put32(&mut out, cov.res_x);
    put32(&mut out, cov.res_y);
    put32(&mut out, levels.len() as u32);
    put32(&mut out, nl as u32);
    // layer table
    for &(l, d) in layer_order {
        put32(&mut out, l);
        put32(&mut out, d);
    }
    // directory: for each non-empty (level, layer): level u8, layer
    // u32, w u16, h u16, off u64, len u32
    let mut dir = Vec::new();
    let mut body = Vec::new();
    for (lv, (rx, ry, planes8)) in levels.iter().enumerate() {
        for (l, p) in planes8.iter().enumerate() {
            if p.is_empty() || p.iter().all(|&b| b == 0) {
                continue;
            }
            let off = body.len() as u64;
            body.extend_from_slice(p);
            dir.push(lv as u8);
            put32(&mut dir, l as u32);
            put16(&mut dir, *rx as u16);
            put16(&mut dir, *ry as u16);
            dir.extend_from_slice(&off.to_le_bytes());
            put32(&mut dir, p.len() as u32);
        }
    }
    let n_entries = dir.len() / 21; // 1+4+2+2+8+4
    put32(&mut out, n_entries as u32);
    let body_off = out.len() as u64 + dir.len() as u64 + 8;
    out.extend_from_slice(&body_off.to_le_bytes());
    out.extend_from_slice(&dir);
    out.extend_from_slice(&body);
    out
}

fn put16(o: &mut Vec<u8>, v: u16) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn put32(o: &mut Vec<u8>, v: u32) {
    o.extend_from_slice(&v.to_le_bytes());
}
