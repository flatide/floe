//! Minimal OASIS writer for band files (spike S2): one flat cell per
//! file, rectangles and polygons with repetitions preserved. Field
//! emission is explicit except layer/datatype modality (records are
//! sorted by layer pair). No CBLOCK yet - correctness first; klayout
//! reads the plain stream fine and compression is a later knob.

use crate::doc::{PolyRec, RectRec, Rep, TextRec};
use crate::Result;

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
        let mut modal: Option<(u32, u32)> = None;
        let mut rects = cell.rects.to_vec();
        let mut polys = cell.polys.to_vec();
        rects.sort_by_key(|r| (r.layer, r.dt));
        polys.sort_by_key(|p| (p.layer, p.dt));
        for r in &rects {
            w.uint(20);
            let same = modal == Some((r.layer, r.dt));
            let has_rep = !matches!(r.rep, Rep::One);
            let mut info: u8 = 0x40 | 0x20 | 0x10 | 0x08;
            if !same {
                info |= 0x03;
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
        for p in &polys {
            w.uint(21);
            let same = modal == Some((p.layer, p.dt));
            let has_rep = !matches!(p.rep, Rep::One);
            let mut info: u8 = 0x20 | 0x10 | 0x08;
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
        for t in cell.texts {
            // TEXT id 19, info 0CNXYRTL: C=1 explicit string (N=0
            // inline a-string), X, Y, T=texttype, L=textlayer;
            // no modality - label volume is skeleton-bounded
            w.uint(19);
            let has_rep = !matches!(t.rep, Rep::One);
            let mut info: u8 = 0x40 | 0x10 | 0x08 | 0x02 | 0x01;
            if has_rep {
                info |= 0x04;
            }
            w.byte(info);
            w.string(t.s.as_bytes());
            w.uint(t.layer as u64);
            w.uint(t.dt as u64);
            w.sint(t.x);
            w.sint(t.y);
            if has_rep {
                w.rep(&t.rep);
            }
        }
        for (tname, x, y, rot, flip, rep) in &cell.places {
            // PLACEMENT id 17, info CNXYRAAF: C=1 (explicit ref),
            // N=0 (by name), X, Y, rot in AA, flip in F
            w.uint(17);
            let has_rep = !matches!(rep, Rep::One);
            let mut info: u8 = 0x80 | 0x20 | 0x10;
            if has_rep {
                info |= 0x08;
            }
            info |= (rot & 3) << 1;
            if *flip {
                info |= 0x01;
            }
            w.byte(info);
            w.string(tname.as_bytes());
            w.sint(*x);
            w.sint(*y);
            if has_rep {
                w.rep(rep);
            }
        }
    }
    w.uint(2);
    w.uint(252);
    w.out.extend_from_slice(&[0u8; 252]);
    w.uint(0);
    Ok(w.out)
}
