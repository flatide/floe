//! Minimal OASIS writer for band files (spike S2): one flat cell per
//! file, rectangles and polygons with repetitions preserved. Field
//! emission is explicit except layer/datatype modality (records are
//! sorted by layer pair). No CBLOCK yet - correctness first; klayout
//! reads the plain stream fine and compression is a later knob.

use crate::doc::{PathRec, PolyRec, RectRec, Rep, TextRec};
use crate::{OasisError, Result};

pub struct W {
    pub out: Vec<u8>,
}

impl W {
    pub fn new() -> Self {
        W { out: Vec::new() }
    }

    pub fn byte(&mut self, b: u8) {
        self.out.push(b);
    }

    pub fn uint(&mut self, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.out.push(b);
                return;
            }
            self.out.push(b | 0x80);
        }
    }

    pub fn sint(&mut self, v: i64) {
        let u = if v < 0 {
            (((-v) as u64) << 1) | 1
        } else {
            (v as u64) << 1
        };
        self.uint(u);
    }

    pub fn real_f64(&mut self, v: f64) {
        self.uint(7);
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    pub fn string(&mut self, s: &[u8]) {
        self.uint(s.len() as u64);
        self.out.extend_from_slice(s);
    }

    pub fn g_delta(&mut self, x: i64, y: i64) {
        // always the general 2-form: (|x|<<2 | sign<<1 | 1), then y
        let g = ((x.unsigned_abs()) << 2) | if x < 0 { 2 } else { 0 } | 1;
        self.uint(g);
        self.sint(y);
    }

    fn rep(&mut self, rep: &Rep) {
        match rep {
            Rep::One => unreachable!("One carries no repetition field"),
            Rep::Grid { na, nb, va, vb } => {
                if *nb > 1 && *na > 1 && va.1 == 0 && vb.0 == 0
                    && va.0 > 0 && vb.1 > 0
                {
                    self.uint(1);
                    self.uint(na - 2);
                    self.uint(nb - 2);
                    self.uint(va.0 as u64);
                    self.uint(vb.1 as u64);
                } else if *nb == 1 && va.1 == 0 && va.0 > 0 {
                    self.uint(2);
                    self.uint(na - 2);
                    self.uint(va.0 as u64);
                } else if *nb == 1 && va.0 == 0 && va.1 > 0 {
                    self.uint(3);
                    self.uint(na - 2);
                    self.uint(va.1 as u64);
                } else if *nb == 1 {
                    self.uint(9);
                    self.uint(na - 2);
                    self.g_delta(va.0, va.1);
                } else {
                    self.uint(8);
                    self.uint(na - 2);
                    self.uint(nb - 2);
                    self.g_delta(va.0, va.1);
                    self.g_delta(vb.0, vb.1);
                }
            }
            Rep::Pts(pts) => {
                // type 10: dimension = members-2, successive g-deltas
                self.uint(10);
                self.uint(pts.len() as u64 - 2);
                let mut prev = (0i64, 0i64);
                for &(x, y) in &pts[1..] {
                    self.g_delta(x - prev.0, y - prev.1);
                    prev = (x, y);
                }
            }
        }
    }
}

const MAGIC: &[u8] = b"%SEMI-OASIS\r\n";

/// Serialize one flat cell of band content to OASIS bytes.
pub fn write_cell(
    cell_name: &str,
    unit: f64,
    rects: &mut [RectRec],
    polys: &mut [PolyRec],
) -> Result<Vec<u8>> {
    let mut w = W::new();
    w.out.extend_from_slice(MAGIC);
    w.uint(1); // START
    w.string(b"1.0");
    w.real_f64(unit);
    w.uint(0); // offset table here...
    for _ in 0..12 {
        w.uint(0); // ...and empty (no strict-mode tables)
    }
    w.uint(14); // CELL by name
    w.string(cell_name.as_bytes());
    rects.sort_by_key(|r| (r.layer, r.dt));
    polys.sort_by_key(|p| (p.layer, p.dt));
    let mut modal: Option<(u32, u32)> = None;
    for r in rects.iter() {
        w.uint(20);
        let same = modal == Some((r.layer, r.dt));
        let has_rep = !matches!(r.rep, Rep::One);
        let mut info: u8 = 0x40 | 0x20 | 0x10 | 0x08; // W H X Y
        if !same {
            info |= 0x03; // L D
        }
        if has_rep {
            info |= 0x04;
        }
        w.byte(info);
        if !same {
            w.uint(r.layer as u64);
            w.uint(r.dt as u64);
            modal = Some((r.layer, r.dt));
        }
        w.uint(r.w as u64);
        w.uint(r.h as u64);
        w.sint(r.x);
        w.sint(r.y);
        if has_rep {
            w.rep(&r.rep);
        }
    }
    for p in polys.iter() {
        w.uint(21);
        let same = modal == Some((p.layer, p.dt));
        let has_rep = !matches!(p.rep, Rep::One);
        let mut info: u8 = 0x20 | 0x10 | 0x08; // P X Y
        if !same {
            info |= 0x03;
        }
        if has_rep {
            info |= 0x04;
        }
        w.byte(info);
        if !same {
            w.uint(p.layer as u64);
            w.uint(p.dt as u64);
            modal = Some((p.layer, p.dt));
        }
        // point list type 4 (g-deltas), anchor = first vertex
        let (ax, ay) = p.pts[0];
        w.uint(4);
        w.uint(p.pts.len() as u64 - 1);
        let mut prev = (ax, ay);
        for &(x, y) in &p.pts[1..] {
            w.g_delta(x - prev.0, y - prev.1);
            prev = (x, y);
        }
        w.sint(ax);
        w.sint(ay);
        if has_rep {
            w.rep(&p.rep);
        }
    }
    // END: fixed 256-byte record (id + padding a-string + scheme 0)
    w.uint(2);
    w.uint(252);
    w.out.extend_from_slice(&[0u8; 252]);
    w.uint(0);
    Ok(w.out)
}

// ------------------------------------------------------------ tree files

/// One cell of a hierarchical band file.
pub struct WCell<'a> {
    pub name: String,
    pub rects: &'a [RectRec],
    pub polys: &'a [PolyRec],
    pub paths: &'a [PathRec],
    /// labels (skeleton only - band tiles stay text-free)
    pub texts: &'a [TextRec],
    /// (target cell name, x, y, rot, flip, rep)
    pub places: Vec<(&'a str, i64, i64, u8, bool, &'a Rep)>,
}

/// Serialize a multi-cell band file (hierarchy preserved). Cell
/// bodies are emitted in the given order; OASIS placements reference
/// cells by name, so definition order is free.
pub fn write_tree(cells: &[WCell], unit: f64) -> Result<Vec<u8>> {
    let mut w = W::new();
    w.out.extend_from_slice(MAGIC);
    w.uint(1);
    w.string(b"1.0");
    w.real_f64(unit);
    w.uint(0);
    for _ in 0..12 {
        w.uint(0);
    }
    for cell in cells {
        w.uint(14); // CELL by name
        w.string(cell.name.as_bytes());
        // body into a scratch buffer, then a CBLOCK when it pays.
        // Emission is MODAL like klayout's writer: XYRELATIVE with
        // records sorted by (layer, dt, y, x) so coordinates become
        // tiny deltas, W/H/layer reused via modal variables, and
        // identical repetitions re-referenced as type 0. A 9.8 GB
        // chip holds ~8.5e9 small grid records; explicit emission
        // cost ~16 B/record after deflate against the source's
        // ~1.2 B/record and produced a 175 GB cache.
        let mut b = W::new();
        b.uint(16); // XYRELATIVE
        let mut m_ld: Option<(u32, u32)> = None;
        let mut m_w: Option<i64> = None;
        let mut m_h: Option<i64> = None;
        let (mut m_x, mut m_y) = (0i64, 0i64); // geometry modal
        let mut m_rep: Option<&Rep> = None;
        let mut m_hw: Option<i64> = None;
        let mut ridx: Vec<usize> = (0..cell.rects.len()).collect();
        ridx.sort_by_key(|&i| {
            let r = &cell.rects[i];
            (r.layer, r.dt, r.y, r.x)
        });
        let mut pidx: Vec<usize> = (0..cell.polys.len()).collect();
        pidx.sort_by_key(|&i| {
            let p = &cell.polys[i];
            (p.layer, p.dt, p.pts[0].1, p.pts[0].0)
        });
        for &ri in &ridx {
            let r = &cell.rects[ri];
            b.uint(20);
            let same_ld = m_ld == Some((r.layer, r.dt));
            let sq = r.w == r.h;
            let wbit = m_w != Some(r.w);
            let hbit = !sq && m_h != Some(r.h);
            let (dx, dy) = (r.x - m_x, r.y - m_y);
            let has_rep = !matches!(r.rep, Rep::One);
            let reuse = has_rep && m_rep == Some(&r.rep);
            let mut info: u8 = 0;
            if sq {
                info |= 0x80;
            }
            if wbit {
                info |= 0x40;
            }
            if hbit {
                info |= 0x20;
            }
            if dx != 0 {
                info |= 0x10;
            }
            if dy != 0 {
                info |= 0x08;
            }
            if has_rep {
                info |= 0x04;
            }
            if !same_ld {
                info |= 0x03;
            }
            b.byte(info);
            if !same_ld {
                b.uint(r.layer as u64);
                b.uint(r.dt as u64);
                m_ld = Some((r.layer, r.dt));
            }
            if wbit {
                b.uint(r.w as u64);
            }
            if hbit {
                b.uint(r.h as u64);
            }
            m_w = Some(r.w);
            m_h = Some(r.h);
            if dx != 0 {
                b.sint(dx);
            }
            if dy != 0 {
                b.sint(dy);
            }
            m_x = r.x;
            m_y = r.y;
            if has_rep {
                if reuse {
                    b.uint(0);
                } else {
                    b.rep(&r.rep);
                    m_rep = Some(&r.rep);
                }
            }
        }
        for &pi in &pidx {
            let p = &cell.polys[pi];
            b.uint(21);
            let same_ld = m_ld == Some((p.layer, p.dt));
            let (ax, ay) = p.pts[0];
            let (dx, dy) = (ax - m_x, ay - m_y);
            let has_rep = !matches!(p.rep, Rep::One);
            let reuse = has_rep && m_rep == Some(&p.rep);
            let mut info: u8 = 0x20;
            if dx != 0 {
                info |= 0x10;
            }
            if dy != 0 {
                info |= 0x08;
            }
            if has_rep {
                info |= 0x04;
            }
            if !same_ld {
                info |= 0x03;
            }
            b.byte(info);
            if !same_ld {
                b.uint(p.layer as u64);
                b.uint(p.dt as u64);
                m_ld = Some((p.layer, p.dt));
            }
            b.uint(4); // point list: g-deltas
            b.uint(p.pts.len() as u64 - 1);
            let mut prev = (ax, ay);
            for &(x, y) in &p.pts[1..] {
                b.g_delta(x - prev.0, y - prev.1);
                prev = (x, y);
            }
            if dx != 0 {
                b.sint(dx);
            }
            if dy != 0 {
                b.sint(dy);
            }
            m_x = ax;
            m_y = ay;
            if has_rep {
                if reuse {
                    b.uint(0);
                } else {
                    b.rep(&p.rep);
                    m_rep = Some(&p.rep);
                }
            }
        }
        for pa in cell.paths {
            // PATH id 22 - extension scheme kept explicit (3/3),
            // halfwidth and coordinates modal
            b.uint(22);
            let same_ld = m_ld == Some((pa.layer, pa.dt));
            let (ax, ay) = pa.pts[0];
            let (dx, dy) = (ax - m_x, ay - m_y);
            let wbit = m_hw != Some(pa.hw);
            let has_rep = !matches!(pa.rep, Rep::One);
            let reuse = has_rep && m_rep == Some(&pa.rep);
            let mut info: u8 = 0x80 | 0x20;
            if wbit {
                info |= 0x40;
            }
            if dx != 0 {
                info |= 0x10;
            }
            if dy != 0 {
                info |= 0x08;
            }
            if has_rep {
                info |= 0x04;
            }
            if !same_ld {
                info |= 0x03;
            }
            b.byte(info);
            if !same_ld {
                b.uint(pa.layer as u64);
                b.uint(pa.dt as u64);
                m_ld = Some((pa.layer, pa.dt));
            }
            if wbit {
                b.uint(pa.hw as u64);
                m_hw = Some(pa.hw);
            }
            b.uint(0b1111); // start & end: explicit values follow
            b.sint(pa.es);
            b.sint(pa.ee);
            b.uint(4);
            b.uint(pa.pts.len() as u64 - 1);
            let mut prev = (ax, ay);
            for &(x, y) in &pa.pts[1..] {
                b.g_delta(x - prev.0, y - prev.1);
                prev = (x, y);
            }
            if dx != 0 {
                b.sint(dx);
            }
            if dy != 0 {
                b.sint(dy);
            }
            m_x = ax;
            m_y = ay;
            if has_rep {
                if reuse {
                    b.uint(0);
                } else {
                    b.rep(&pa.rep);
                    m_rep = Some(&pa.rep);
                }
            }
        }
        let (mut t_x, mut t_y) = (0i64, 0i64); // text modal
        let mut t_ld: Option<(u32, u32)> = None;
        for t in cell.texts {
            // TEXT id 19: explicit inline string, modal layer/coords
            b.uint(19);
            let same_ld = t_ld == Some((t.layer, t.dt));
            let (dx, dy) = (t.x - t_x, t.y - t_y);
            let has_rep = !matches!(t.rep, Rep::One);
            let reuse = has_rep && m_rep == Some(&t.rep);
            let mut info: u8 = 0x40;
            if dx != 0 {
                info |= 0x10;
            }
            if dy != 0 {
                info |= 0x08;
            }
            if has_rep {
                info |= 0x04;
            }
            if !same_ld {
                info |= 0x03;
            }
            b.byte(info);
            b.string(t.s.as_bytes());
            if !same_ld {
                b.uint(t.layer as u64);
                b.uint(t.dt as u64);
                t_ld = Some((t.layer, t.dt));
            }
            if dx != 0 {
                b.sint(dx);
            }
            if dy != 0 {
                b.sint(dy);
            }
            t_x = t.x;
            t_y = t.y;
            if has_rep {
                if reuse {
                    b.uint(0);
                } else {
                    b.rep(&t.rep);
                    m_rep = Some(&t.rep);
                }
            }
        }
        let (mut p_x, mut p_y) = (0i64, 0i64); // placement modal
        for (tname, x, y, rot, flip, rep) in &cell.places {
            // PLACEMENT id 17, info CNXYRAAF: C=1 (explicit ref),
            // N=0 (by name), modal coords, modal repetition
            b.uint(17);
            let (dx, dy) = (*x - p_x, *y - p_y);
            let has_rep = !matches!(rep, Rep::One);
            let reuse = has_rep && m_rep == Some(*rep);
            let mut info: u8 = 0x80;
            if dx != 0 {
                info |= 0x20;
            }
            if dy != 0 {
                info |= 0x10;
            }
            if has_rep {
                info |= 0x08;
            }
            info |= (rot & 3) << 1;
            if *flip {
                info |= 0x01;
            }
            b.byte(info);
            b.string(tname.as_bytes());
            if dx != 0 {
                b.sint(dx);
            }
            if dy != 0 {
                b.sint(dy);
            }
            p_x = *x;
            p_y = *y;
            if has_rep {
                if reuse {
                    b.uint(0);
                } else {
                    b.rep(rep);
                    m_rep = Some(rep);
                }
            }
        }
        if b.out.len() >= 64 {
            let mut enc = flate2::write::DeflateEncoder::new(
                Vec::new(),
                flate2::Compression::new(1),
            );
            std::io::Write::write_all(&mut enc, &b.out)
                .map_err(OasisError::Io)?;
            let comp = enc.finish().map_err(OasisError::Io)?;
            w.uint(34); // CBLOCK, comp-type 0 (DEFLATE)
            w.uint(0);
            w.uint(b.out.len() as u64);
            w.uint(comp.len() as u64);
            w.out.extend_from_slice(&comp);
        } else {
            w.out.extend_from_slice(&b.out);
        }
    }
    w.uint(2);
    w.uint(252);
    w.out.extend_from_slice(&[0u8; 252]);
    w.uint(0);
    Ok(w.out)
}
