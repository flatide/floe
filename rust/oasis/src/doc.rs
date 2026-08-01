//! Geometry-carrying OASIS parser (spike S2): parses a file into a
//! record-level document - rectangles, polygons and placements with
//! their repetitions intact. Scope is the record inventory measured
//! on the target assets (RECTANGLE / POLYGON / PLACEMENT id 17 /
//! TEXT / tables); PATH, TRAPEZOID, CTRAPEZOID, CIRCLE, XGEOMETRY
//! and magnified/arbitrary-angle placements raise a clear error so
//! the production version grows deliberately, not silently.

use crate::{err, Cur, OasisError, Result};
use std::collections::HashMap;
use std::io::Read;

// ------------------------------------------------------------- records

/// Repetition, normalized to two forms that cover all 12 OASIS types
/// without expansion beyond what the file itself spells out.
#[derive(Clone, Debug, PartialEq)]
pub enum Rep {
    One,
    /// na x nb members at va/vb steps (axis vectors, dbu)
    Grid {
        na: u64,
        nb: u64,
        va: (i64, i64),
        vb: (i64, i64),
    },
    /// explicit member offsets, (0,0) first (spelled out in the file)
    Pts(Vec<(i64, i64)>),
}

impl Rep {
    pub fn members(&self) -> u64 {
        match self {
            Rep::One => 1,
            Rep::Grid { na, nb, .. } => na * nb,
            Rep::Pts(v) => v.len() as u64,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RectRec {
    pub layer: u32,
    pub dt: u32,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub rep: Rep,
}

#[derive(Clone, Debug)]
pub struct PolyRec {
    pub layer: u32,
    pub dt: u32,
    /// absolute vertices in cell coordinates (first = anchor)
    pub pts: Vec<(i64, i64)>,
    pub rep: Rep,
}

#[derive(Clone, Debug)]
pub struct PlaceRec {
    /// resolved cell index into Doc::cells
    pub cell: usize,
    pub x: i64,
    pub y: i64,
    /// 0..3 = 0/90/180/270 degrees CCW
    pub rot: u8,
    pub flip: bool, // mirror about x axis (applied before rotation)
    pub rep: Rep,
}

#[derive(Clone, Debug)]
pub struct PathRec {
    pub layer: u32,
    pub dt: u32,
    /// absolute spine vertices in cell coordinates
    pub pts: Vec<(i64, i64)>,
    /// half-width
    pub hw: i64,
    /// start/end extensions (resolved values, may be negative)
    pub es: i64,
    pub ee: i64,
    pub rep: Rep,
}

#[derive(Clone, Debug)]
pub struct TextRec {
    pub layer: u32,
    pub dt: u32,
    pub x: i64,
    pub y: i64,
    pub rep: Rep,
    pub s: String,
}

#[derive(Default)]
pub struct Cell {
    pub name: String,
    pub rects: Vec<RectRec>,
    pub polys: Vec<PolyRec>,
    pub paths: Vec<PathRec>,
    pub places: Vec<PlaceRec>,
    /// kept whole (tiles stay text-free; the skeleton/sidecar builds
    /// draw from here) - text points also count into the layout bbox
    pub texts: Vec<TextRec>,
}

pub struct Doc {
    pub unit: f64, // grid steps per micron (dbu = 1/unit um)
    pub cells: Vec<Cell>,
    pub top: usize,
    /// (layer, datatype) pairs in file-appearance order (LAYERNAME
    /// tables count as an appearance) - klayout's layer_indexes()
    /// enumerates in this order, and the meta layer list / palette
    /// assignment must match it
    pub layer_order: Vec<(u32, u32)>,
    /// seconds spent in the grid-normalize pass (parse-phase
    /// attribution for the CLI log)
    pub norm_s: f64,
    /// names from LAYERNAME records with exact-value intervals
    pub layer_names: HashMap<(u32, u32), String>,
}

// ------------------------------------------------------------ modal set

#[derive(Default)]
struct Modal {
    rep: Option<Rep>,
    layer: Option<u64>,
    datatype: Option<u64>,
    textlayer: Option<u64>,
    texttype: Option<u64>,
    // geometry/placement/text coordinates + xy-relative mode
    relative: bool,
    geo_x: i64,
    geo_y: i64,
    pl_x: i64,
    pl_y: i64,
    tx_x: i64,
    tx_y: i64,
    geo_w: Option<i64>,
    geo_h: Option<i64>,
    poly_pts: Option<Vec<(i64, i64)>>, // deltas from anchor
    path_pts: Option<Vec<(i64, i64)>>, // separate modal per spec
    path_hw: Option<i64>,
    path_es: Option<i64>,
    path_ee: Option<i64>,
    place_cell: Option<PlaceTarget>,
    text_string: Option<TextTarget>,
}

#[derive(Clone)]
enum PlaceTarget {
    Ref(u64),
    Name(String),
}

#[derive(Clone)]
enum TextTarget {
    Ref(u64),
    Str(String),
}

fn read_rep(c: &mut Cur, modal: &mut Option<Rep>) -> Result<Rep> {
    let t = c.uint()?;
    let rep = match t {
        0 => match modal {
            Some(r) => r.clone(),
            None => return err(c.here(), "repetition reuse before first"),
        },
        1 => {
            let xd = c.uint()?;
            let yd = c.uint()?;
            let dx = c.uint()? as i64;
            let dy = c.uint()? as i64;
            Rep::Grid { na: xd + 2, nb: yd + 2, va: (dx, 0), vb: (0, dy) }
        }
        2 => {
            let xd = c.uint()?;
            let dx = c.uint()? as i64;
            Rep::Grid { na: xd + 2, nb: 1, va: (dx, 0), vb: (0, 0) }
        }
        3 => {
            let yd = c.uint()?;
            let dy = c.uint()? as i64;
            Rep::Grid { na: yd + 2, nb: 1, va: (0, dy), vb: (0, 0) }
        }
        4 | 5 => {
            let xd = c.uint()?;
            let g = if t == 5 { c.uint()? as i64 } else { 1 };
            let mut pts = Vec::with_capacity(xd as usize + 2);
            let mut x = 0i64;
            pts.push((0, 0));
            for _ in 0..=xd {
                x += c.uint()? as i64 * g;
                pts.push((x, 0));
            }
            Rep::Pts(pts)
        }
        6 | 7 => {
            let yd = c.uint()?;
            let g = if t == 7 { c.uint()? as i64 } else { 1 };
            let mut pts = Vec::with_capacity(yd as usize + 2);
            let mut y = 0i64;
            pts.push((0, 0));
            for _ in 0..=yd {
                y += c.uint()? as i64 * g;
                pts.push((0, y));
            }
            Rep::Pts(pts)
        }
        8 => {
            let nd = c.uint()?;
            let md = c.uint()?;
            let va = c.g_delta()?;
            let vb = c.g_delta()?;
            Rep::Grid { na: nd + 2, nb: md + 2, va, vb }
        }
        9 => {
            let d = c.uint()?;
            let va = c.g_delta()?;
            Rep::Grid { na: d + 2, nb: 1, va, vb: (0, 0) }
        }
        10 | 11 => {
            let d = c.uint()?;
            let g = if t == 11 { c.uint()? as i64 } else { 1 };
            let mut pts = Vec::with_capacity(d as usize + 2);
            let (mut x, mut y) = (0i64, 0i64);
            pts.push((0, 0));
            for _ in 0..=d {
                let (dx, dy) = c.g_delta()?;
                x += dx * g;
                y += dy * g;
                pts.push((x, y));
            }
            Rep::Pts(pts)
        }
        _ => return err(c.here(), "bad repetition type"),
    };
    *modal = Some(rep.clone());
    Ok(rep)
}

/// point list -> vertex deltas from the anchor. `closed`: polygon
/// semantics (implicit closing vertex for the manhattan types);
/// path point lists are open chains.
fn read_points(c: &mut Cur, closed: bool) -> Result<Vec<(i64, i64)>> {
    let t = c.uint()?;
    let n = c.uint()? as usize;
    let mut pts: Vec<(i64, i64)> = Vec::with_capacity(n + 2);
    pts.push((0, 0));
    let (mut x, mut y) = (0i64, 0i64);
    match t {
        0 | 1 => {
            let mut horiz = t == 0;
            for _ in 0..n {
                let d = c.sint()?;
                if horiz {
                    x += d;
                } else {
                    y += d;
                }
                horiz = !horiz;
                pts.push((x, y));
            }
            // close manhattan-ly: one implicit vertex on the pending axis
            if closed && (x != 0 || y != 0) {
                let u = if horiz { (0, y) } else { (x, 0) };
                if u != (x, y) && u != (0, 0) {
                    pts.push(u);
                }
            }
        }
        2 => {
            for _ in 0..n {
                let g = c.uint()?;
                let m = (g >> 2) as i64;
                let (dx, dy) = match g & 3 {
                    0 => (m, 0),
                    1 => (0, m),
                    2 => (-m, 0),
                    _ => (0, -m),
                };
                x += dx;
                y += dy;
                pts.push((x, y));
            }
        }
        3 => {
            for _ in 0..n {
                let g = c.uint()?;
                let m = (g >> 3) as i64;
                let (dx, dy) = match g & 7 {
                    0 => (m, 0),
                    1 => (0, m),
                    2 => (-m, 0),
                    3 => (0, -m),
                    4 => (m, m),
                    5 => (-m, m),
                    6 => (-m, -m),
                    _ => (m, -m),
                };
                x += dx;
                y += dy;
                pts.push((x, y));
            }
        }
        4 => {
            for _ in 0..n {
                let (dx, dy) = c.g_delta()?;
                x += dx;
                y += dy;
                pts.push((x, y));
            }
        }
        5 => {
            // double-delta: each g-delta adds to the RUNNING delta
            let (mut rx, mut ry) = (0i64, 0i64);
            for _ in 0..n {
                let (dx, dy) = c.g_delta()?;
                rx += dx;
                ry += dy;
                x += rx;
                y += ry;
                pts.push((x, y));
            }
        }
        _ => return err(c.here(), "bad point-list type"),
    }
    Ok(pts)
}

fn coord(c: &mut Cur, modal_v: &mut i64, relative: bool) -> Result<()> {
    let v = c.sint()?;
    *modal_v = if relative { *modal_v + v } else { v };
    Ok(())
}

// --------------------------------------------------------------- parser

struct Builder {
    cells: Vec<Cell>,
    by_name: HashMap<String, usize>,
    refnames: HashMap<u64, String>,
    implicit_ref: u64,
    textstrings: HashMap<u64, String>,
    implicit_tref: u64,
    cur: Option<usize>,
    // placements recorded with unresolved targets, fixed up at the end
    pending: Vec<(usize, usize, PlaceTarget)>,
    // texts whose string is a TEXTSTRING refnum (table may come later)
    pending_texts: Vec<(usize, usize, u64)>,
    layer_order: Vec<(u32, u32)>,
    layer_seen: HashMap<(u32, u32), ()>,
    layer_names: HashMap<(u32, u32), String>,
    unit: f64,
}

impl Builder {
    fn cell_index(&mut self, name: &str) -> usize {
        if let Some(&i) = self.by_name.get(name) {
            return i;
        }
        let i = self.cells.len();
        self.cells.push(Cell { name: name.to_string(), ..Cell::default() });
        self.by_name.insert(name.to_string(), i);
        i
    }

    fn reg_layer(&mut self, l: u32, d: u32) {
        if self.layer_seen.insert((l, d), ()).is_none() {
            self.layer_order.push((l, d));
        }
    }
}

fn utf8(c: &Cur, b: &[u8]) -> Result<String> {
    String::from_utf8(b.to_vec())
        .map_err(|_| OasisError::Format(format!("@{}: non-utf8 name", c.here())))
}

fn parse_records(
    c: &mut Cur,
    m: &mut Modal,
    b: &mut Builder,
    depth: u32,
) -> Result<bool> {
    while !c.at_end() {
        let id = c.uint()?;
        match id {
            0 => {}
            1 => {
                c.string()?;
                b.unit = c.real()?;
                if c.uint()? == 0 {
                    for _ in 0..12 {
                        c.uint()?;
                    }
                }
            }
            2 => {
                if depth > 0 {
                    return err(c.here(), "END inside CBLOCK");
                }
                return Ok(false);
            }
            3 | 4 => {
                let sb = c.string()?.to_vec();
                let name = utf8(c, &sb)?;
                let r = if id == 4 { c.uint()? } else {
                    let r = b.implicit_ref;
                    b.implicit_ref += 1;
                    r
                };
                b.refnames.insert(r, name);
            }
            5 | 6 => {
                let sb = c.string()?.to_vec();
                let s = utf8(c, &sb)?;
                let r = if id == 6 { c.uint()? } else {
                    let r = b.implicit_tref;
                    b.implicit_tref += 1;
                    r
                };
                b.textstrings.insert(r, s);
            }
            7 | 8 | 9 | 10 => {
                c.string()?;
                if id == 8 || id == 10 {
                    c.uint()?;
                }
            }
            11 | 12 => {
                // LAYERNAME: register exact (layer, datatype) pairs in
                // appearance order + remember the name (klayout's
                // layer table starts with these)
                let sb = c.string()?.to_vec();
                let name = utf8(c, &sb)?;
                let mut exact: [Option<u64>; 2] = [None, None];
                for e in &mut exact {
                    match c.uint()? {
                        0 => {}
                        1 | 2 => {
                            c.uint()?;
                        }
                        3 => *e = Some(c.uint()?),
                        4 => {
                            let a = c.uint()?;
                            let bb = c.uint()?;
                            if a == bb {
                                *e = Some(a);
                            }
                        }
                        _ => return err(c.here(), "bad interval"),
                    }
                }
                if let (Some(l), Some(d)) = (exact[0], exact[1]) {
                    let key = (l as u32, d as u32);
                    b.reg_layer(key.0, key.1);
                    b.layer_names.entry(key).or_insert(name);
                }
            }
            13 | 14 => {
                let name = if id == 13 {
                    let r = c.uint()?;
                    match b.refnames.get(&r) {
                        Some(n) => n.clone(),
                        // name table may come later in the stream:
                        // use a placeholder key tied to the refnum
                        None => format!("\u{1}ref{}", r),
                    }
                } else {
                    {
                        let sb = c.string()?.to_vec();
                        utf8(c, &sb)?
                    }
                };
                b.cur = Some(b.cell_index(&name));
                *m = Modal::default();
            }
            15 => m.relative = false,
            16 => m.relative = true,
            17 => {
                let info = c.byte()?;
                let cur = match b.cur {
                    Some(i) => i,
                    None => return err(c.here(), "placement outside cell"),
                };
                if info & 0x80 != 0 {
                    m.place_cell = Some(if info & 0x40 != 0 {
                        PlaceTarget::Ref(c.uint()?)
                    } else {
                        PlaceTarget::Name({
                            let sb = c.string()?.to_vec();
                            utf8(c, &sb)?
                        })
                    });
                }
                if info & 0x20 != 0 {
                    coord(c, &mut m.pl_x, m.relative)?;
                }
                if info & 0x10 != 0 {
                    coord(c, &mut m.pl_y, m.relative)?;
                }
                let rep = if info & 0x08 != 0 {
                    read_rep(c, &mut m.rep)?
                } else {
                    Rep::One
                };
                let rot = (info >> 1) & 3;
                let flip = info & 0x01 != 0;
                let tgt = match &m.place_cell {
                    Some(t) => t.clone(),
                    None => return err(c.here(), "placement without cell"),
                };
                let slot = b.cells[cur].places.len();
                b.cells[cur].places.push(PlaceRec {
                    cell: usize::MAX,
                    x: m.pl_x,
                    y: m.pl_y,
                    rot,
                    flip,
                    rep,
                });
                b.pending.push((cur, slot, tgt));
            }
            18 => return err(c.here(), "magnified/angled placement: \
                                        out of spike scope"),
            19 => {
                let info = c.byte()?;
                let cur = match b.cur {
                    Some(i) => i,
                    None => return err(c.here(), "text outside cell"),
                };
                if info & 0x40 != 0 {
                    m.text_string = Some(if info & 0x20 != 0 {
                        TextTarget::Ref(c.uint()?)
                    } else {
                        TextTarget::Str({
                            let sb = c.string()?.to_vec();
                            utf8(c, &sb)?
                        })
                    });
                }
                if info & 0x01 != 0 {
                    m.textlayer = Some(c.uint()?);
                }
                if info & 0x02 != 0 {
                    m.texttype = Some(c.uint()?);
                }
                if info & 0x10 != 0 {
                    coord(c, &mut m.tx_x, m.relative)?;
                }
                if info & 0x08 != 0 {
                    coord(c, &mut m.tx_y, m.relative)?;
                }
                let rep = if info & 0x04 != 0 {
                    read_rep(c, &mut m.rep)?
                } else {
                    Rep::One
                };
                let (l, d) = match (m.textlayer, m.texttype) {
                    (Some(l), Some(d)) => (l as u32, d as u32),
                    _ => return err(c.here(), "text before layer modal"),
                };
                b.reg_layer(l, d);
                let s = match &m.text_string {
                    Some(TextTarget::Str(s)) => s.clone(),
                    Some(TextTarget::Ref(r)) => {
                        match b.textstrings.get(r) {
                            Some(s) => s.clone(),
                            None => {
                                // string table may come later
                                b.pending_texts.push((
                                    cur,
                                    b.cells[cur].texts.len(),
                                    *r,
                                ));
                                String::new()
                            }
                        }
                    }
                    None => return err(c.here(), "text without string"),
                };
                b.cells[cur].texts.push(TextRec {
                    layer: l,
                    dt: d,
                    x: m.tx_x,
                    y: m.tx_y,
                    rep,
                    s,
                });
            }
            20 => {
                let info = c.byte()?;
                let cur = match b.cur {
                    Some(i) => i,
                    None => return err(c.here(), "shape outside cell"),
                };
                if info & 0x01 != 0 {
                    m.layer = Some(c.uint()?);
                }
                if info & 0x02 != 0 {
                    m.datatype = Some(c.uint()?);
                }
                if info & 0x40 != 0 {
                    m.geo_w = Some(c.uint()? as i64);
                }
                if info & 0x20 != 0 {
                    m.geo_h = Some(c.uint()? as i64);
                }
                let w = m.geo_w
                    .ok_or_else(|| OasisError::Format("no width".into()))?;
                let h = if info & 0x80 != 0 {
                    m.geo_h = Some(w);
                    w
                } else {
                    m.geo_h
                        .ok_or_else(|| OasisError::Format("no height".into()))?
                };
                if info & 0x10 != 0 {
                    coord(c, &mut m.geo_x, m.relative)?;
                }
                if info & 0x08 != 0 {
                    coord(c, &mut m.geo_y, m.relative)?;
                }
                let rep = if info & 0x04 != 0 {
                    read_rep(c, &mut m.rep)?
                } else {
                    Rep::One
                };
                let (l, d) = match (m.layer, m.datatype) {
                    (Some(l), Some(d)) => (l as u32, d as u32),
                    _ => return err(c.here(), "rect before layer modal"),
                };
                b.reg_layer(l, d);
                b.cells[cur].rects.push(RectRec {
                    layer: l,
                    dt: d,
                    x: m.geo_x,
                    y: m.geo_y,
                    w,
                    h,
                    rep,
                });
            }
            21 => {
                let info = c.byte()?;
                let cur = match b.cur {
                    Some(i) => i,
                    None => return err(c.here(), "shape outside cell"),
                };
                if info & 0x01 != 0 {
                    m.layer = Some(c.uint()?);
                }
                if info & 0x02 != 0 {
                    m.datatype = Some(c.uint()?);
                }
                if info & 0x20 != 0 {
                    m.poly_pts = Some(read_points(c, true)?);
                }
                if info & 0x10 != 0 {
                    coord(c, &mut m.geo_x, m.relative)?;
                }
                if info & 0x08 != 0 {
                    coord(c, &mut m.geo_y, m.relative)?;
                }
                let rep = if info & 0x04 != 0 {
                    read_rep(c, &mut m.rep)?
                } else {
                    Rep::One
                };
                let (l, d) = match (m.layer, m.datatype) {
                    (Some(l), Some(d)) => (l as u32, d as u32),
                    _ => return err(c.here(), "poly before layer modal"),
                };
                b.reg_layer(l, d);
                let deltas = match &m.poly_pts {
                    Some(p) => p,
                    None => return err(c.here(), "poly without points"),
                };
                let pts = deltas
                    .iter()
                    .map(|(dx, dy)| (m.geo_x + dx, m.geo_y + dy))
                    .collect();
                b.cells[cur].polys.push(PolyRec { layer: l, dt: d, pts, rep });
            }
            22 => {
                // PATH: EWPXYRDL
                let info = c.byte()?;
                let cur = match b.cur {
                    Some(i) => i,
                    None => return err(c.here(), "shape outside cell"),
                };
                if info & 0x01 != 0 {
                    m.layer = Some(c.uint()?);
                }
                if info & 0x02 != 0 {
                    m.datatype = Some(c.uint()?);
                }
                if info & 0x40 != 0 {
                    m.path_hw = Some(c.uint()? as i64);
                }
                let hw = m.path_hw
                    .ok_or_else(|| OasisError::Format("no halfwidth".into()))?;
                if info & 0x80 != 0 {
                    let scheme = c.uint()?;
                    m.path_es = Some(match (scheme >> 2) & 3 {
                        0 => m.path_es.ok_or_else(|| OasisError::Format(
                            "start ext reuse before first".into()))?,
                        1 => 0,
                        2 => hw,
                        _ => c.sint()?,
                    });
                    m.path_ee = Some(match scheme & 3 {
                        0 => m.path_ee.ok_or_else(|| OasisError::Format(
                            "end ext reuse before first".into()))?,
                        1 => 0,
                        2 => hw,
                        _ => c.sint()?,
                    });
                }
                let es = m.path_es.ok_or_else(
                    || OasisError::Format("no start extension".into()))?;
                let ee = m.path_ee.ok_or_else(
                    || OasisError::Format("no end extension".into()))?;
                if info & 0x20 != 0 {
                    m.path_pts = Some(read_points(c, false)?);
                }
                if info & 0x10 != 0 {
                    coord(c, &mut m.geo_x, m.relative)?;
                }
                if info & 0x08 != 0 {
                    coord(c, &mut m.geo_y, m.relative)?;
                }
                let rep = if info & 0x04 != 0 {
                    read_rep(c, &mut m.rep)?
                } else {
                    Rep::One
                };
                let (l, d) = match (m.layer, m.datatype) {
                    (Some(l), Some(d)) => (l as u32, d as u32),
                    _ => return err(c.here(), "path before layer modal"),
                };
                b.reg_layer(l, d);
                let deltas = match &m.path_pts {
                    Some(p) => p,
                    None => return err(c.here(), "path without points"),
                };
                let pts = deltas
                    .iter()
                    .map(|(dx, dy)| (m.geo_x + dx, m.geo_y + dy))
                    .collect();
                b.cells[cur].paths.push(PathRec {
                    layer: l,
                    dt: d,
                    pts,
                    hw,
                    es,
                    ee,
                    rep,
                });
            }
            23..=26 => return err(c.here(), "TRAPEZOID: out of spike scope"),
            27 => return err(c.here(), "CIRCLE: out of spike scope"),
            28 => {
                let info = c.byte()?;
                if info & 0x04 != 0 {
                    if info & 0x02 != 0 {
                        c.uint()?;
                    } else {
                        c.string()?;
                    }
                }
                if info & 0x08 == 0 {
                    let n = if info >> 4 == 15 {
                        c.uint()?
                    } else {
                        (info >> 4) as u64
                    };
                    for _ in 0..n {
                        let t = c.uint()?;
                        match t {
                            0..=3 => {
                                c.uint()?;
                            }
                            4 | 5 => {
                                c.uint()?;
                                c.uint()?;
                            }
                            6 => {
                                c.bytes(4)?;
                            }
                            7 => {
                                c.bytes(8)?;
                            }
                            8 => {
                                c.uint()?;
                            }
                            9 => {
                                c.sint()?;
                            }
                            10 | 11 | 12 => {
                                c.string()?;
                            }
                            13 | 14 | 15 => {
                                c.uint()?;
                            }
                            _ => return err(c.here(), "bad prop value"),
                        }
                    }
                }
            }
            29 => {}
            30 | 31 => {
                c.uint()?;
                c.string()?;
                if id == 31 {
                    c.uint()?;
                }
            }
            32 => {
                c.uint()?;
                c.string()?;
            }
            33 => return err(c.here(), "XGEOMETRY: out of spike scope"),
            34 => {
                let ctype = c.uint()?;
                let un = c.uint()? as usize;
                let cn = c.uint()? as usize;
                let comp = c.bytes(cn)?;
                if ctype != 0 {
                    return err(c.here(), "unknown CBLOCK compression");
                }
                let mut out = Vec::with_capacity(un);
                flate2::read::DeflateDecoder::new(comp)
                    .read_to_end(&mut out)
                    .map_err(OasisError::Io)?;
                if out.len() != un {
                    return err(c.here(), "CBLOCK size mismatch");
                }
                let mut sub = Cur::new(&out, c.here());
                parse_records(&mut sub, m, b, depth + 1)?;
            }
            _ => return err(c.here(), &format!("unknown record {}", id)),
        }
    }
    if depth == 0 {
        return err(c.here(), "stream ended without END");
    }
    Ok(true)
}

/// Constant-pitch runs of a sorted int slice: (start, pitch, count).
/// Unlike the python original this PARTITIONS (no shared endpoints -
/// duplicated members would inflate the validated member counts).
fn const_pitch_runs(vals: &[i64]) -> Vec<(i64, i64, usize)> {
    let n = vals.len();
    let mut runs = Vec::new();
    let mut i = 0usize;
    while i < n {
        if i == n - 1 {
            runs.push((vals[i], 0, 1));
            break;
        }
        let pitch = vals[i + 1] - vals[i];
        if pitch == 0 {
            runs.push((vals[i], 0, 1));
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < n - 1 && vals[j + 1] - vals[j] == pitch {
            j += 1;
        }
        runs.push((vals[i], pitch, j - i + 1));
        i = j + 1;
    }
    runs
}

/// Decompose a point set into maximal regular sub-grids + leftovers
/// (the python indexer's _find_grids, made exact). Real fill/via
/// point lists are grids WITH HOLES - an exact whole-set matcher
/// never fires on them, and a 9.8 GB chip kept its 140 GB of
/// explicit offsets that way. Column signature grouping: per x, the
/// constant-pitch y-runs; columns sharing (y0, pitch, count) at a
/// constant x-pitch fuse into arrays.
/// Returns None when nothing usefully gridded.
#[allow(clippy::type_complexity)]
fn find_grids(
    pts: &[(i64, i64)],
) -> Option<(Vec<(i64, i64, i64, u64, i64, u64)>, Vec<(i64, i64)>)> {
    let mut v = pts.to_vec();
    v.sort_unstable();
    // ((y0, pitch, cnt), x) collected flat and sorted replaces the
    // per-signature hash map of x-vectors: the pass runs over every
    // stored point list of a chip, so per-column allocations dominated
    // (real 150 MB chip: this whole pass tripled the parse phase)
    let mut sigs: Vec<((i64, i64, usize), i64)> = Vec::new();
    let mut leftovers: Vec<(i64, i64)> = Vec::new();
    let mut ys: Vec<i64> = Vec::new();
    let mut i = 0usize;
    while i < v.len() {
        let x = v[i].0;
        let mut j = i;
        while j < v.len() && v[j].0 == x {
            j += 1;
        }
        ys.clear();
        ys.extend(v[i..j].iter().map(|p| p.1));
        for (y0, pitch, cnt) in const_pitch_runs(&ys) {
            if cnt == 1 {
                leftovers.push((x, y0));
            } else {
                sigs.push(((y0, pitch, cnt), x));
            }
        }
        i = j;
    }
    if sigs.is_empty() {
        return None; // no runnable column anywhere
    }
    sigs.sort_unstable();
    let mut arrays: Vec<(i64, i64, i64, u64, i64, u64)> = Vec::new();
    let mut xs: Vec<i64> = Vec::new();
    let mut k = 0usize;
    while k < sigs.len() {
        let (y0, dy, ny) = sigs[k].0;
        xs.clear();
        let mut m = k;
        while m < sigs.len() && sigs[m].0 == sigs[k].0 {
            xs.push(sigs[m].1);
            m += 1;
        }
        k = m;
        for (x0, dx, nx) in const_pitch_runs(&xs) {
            // tiny fragments cost more as records than as points
            if nx * ny < 8 {
                for a in 0..nx as i64 {
                    for b in 0..ny as i64 {
                        leftovers.push((x0 + a * dx, y0 + b * dy));
                    }
                }
            } else {
                arrays.push((x0, y0, dx, nx as u64, dy, ny as u64));
            }
        }
    }
    if arrays.is_empty() {
        return None;
    }
    leftovers.sort_unstable();
    Some((arrays, leftovers))
}

fn grid_rep(dx: i64, nx: u64, dy: i64, ny: u64) -> Rep {
    if nx == 1 {
        // degenerate column: 1-D vertical grid (zero pitch on a used
        // axis would divide by zero in the tiler's splitter)
        Rep::Grid { na: ny, nb: 1, va: (0, dy), vb: (0, 0) }
    } else {
        Rep::Grid { na: nx, nb: ny, va: (dx, 0), vb: (0, dy) }
    }
}

type GridDecomp =
    Option<(Vec<(i64, i64, i64, u64, i64, u64)>, Vec<(i64, i64)>)>;

/// Split records with big point-list repetitions into gridded parts
/// plus a leftover point list. Coverage and member counts are
/// invariant; the tiler then splits the grids ARITHMETICALLY per
/// tile and only the true leftovers stay explicit. `res` feeds the
/// precomputed find_grids result for every candidate rep in walk
/// order (rects, polys, paths, texts, places).
fn normalize_cell(
    cell: &mut Cell,
    res: &mut impl Iterator<Item = GridDecomp>,
) {
    macro_rules! norm {
        ($vec:expr, $shift:expr) => {{
            let old = std::mem::take(&mut $vec);
            for rec in old {
                let decomp = match &rec.rep {
                    Rep::Pts(p) if p.len() >= 16 => {
                        res.next().expect("normalize task list out of sync")
                    }
                    _ => None,
                };
                match decomp {
                    None => $vec.push(rec),
                    Some((arrays, leftovers)) => {
                        for (x0, y0, dx, nx, dy, ny) in arrays {
                            let mut nr = rec.clone();
                            $shift(&mut nr, x0, y0);
                            nr.rep = grid_rep(dx, nx, dy, ny);
                            $vec.push(nr);
                        }
                        match leftovers.len() {
                            0 => {}
                            1 => {
                                let mut nr = rec.clone();
                                $shift(
                                    &mut nr,
                                    leftovers[0].0,
                                    leftovers[0].1,
                                );
                                nr.rep = Rep::One;
                                $vec.push(nr);
                            }
                            _ => {
                                let (bx, by) = leftovers[0];
                                let mut nr = rec.clone();
                                $shift(&mut nr, bx, by);
                                nr.rep = Rep::Pts(
                                    leftovers
                                        .iter()
                                        .map(|&(x, y)| (x - bx, y - by))
                                        .collect(),
                                );
                                $vec.push(nr);
                            }
                        }
                    }
                }
            }
        }};
    }
    norm!(cell.rects, |r: &mut RectRec, dx: i64, dy: i64| {
        r.x += dx;
        r.y += dy;
    });
    norm!(cell.polys, |r: &mut PolyRec, dx: i64, dy: i64| {
        for p in &mut r.pts {
            p.0 += dx;
            p.1 += dy;
        }
    });
    norm!(cell.paths, |r: &mut PathRec, dx: i64, dy: i64| {
        for p in &mut r.pts {
            p.0 += dx;
            p.1 += dy;
        }
    });
    norm!(cell.texts, |r: &mut TextRec, dx: i64, dy: i64| {
        r.x += dx;
        r.y += dy;
    });
    norm!(cell.places, |r: &mut PlaceRec, dx: i64, dy: i64| {
        r.x += dx;
        r.y += dy;
    });
}

/// Candidate reps of one cell in normalize_cell's walk order.
fn pts_candidates(cell: &Cell) -> impl Iterator<Item = &Vec<(i64, i64)>> {
    cell.rects
        .iter()
        .map(|r| &r.rep)
        .chain(cell.polys.iter().map(|r| &r.rep))
        .chain(cell.paths.iter().map(|r| &r.rep))
        .chain(cell.texts.iter().map(|r| &r.rep))
        .chain(cell.places.iter().map(|r| &r.rep))
        .filter_map(|rep| match rep {
            Rep::Pts(p) if p.len() >= 16 => Some(p),
            _ => None,
        })
}

fn normalize_reps(doc: &mut Doc, jobs: usize) {
    // find_grids is a pure function of the point list and the work
    // clusters in a handful of fill cells, so cell-chunk scheduling
    // starved most threads (real 150 MB chip: parse 24s -> 68s at 12
    // jobs). Phase 1 evaluates every candidate rep off a flat task
    // list at RECORD granularity; phase 2 rebuilds the record
    // vectors sequentially in walk order - output is byte-identical
    // to the sequential build at any thread count.
    let cand: Vec<&Vec<(i64, i64)>> =
        doc.cells.iter().flat_map(pts_candidates).collect();
    let mut results: Vec<std::sync::OnceLock<GridDecomp>> = Vec::new();
    results.resize_with(cand.len(), std::sync::OnceLock::new);
    if jobs <= 1 || cand.len() < 2 {
        for (p, r) in cand.iter().zip(results.iter_mut()) {
            let _ = r.set(find_grids(p));
        }
    } else {
        // per-record atomic pull: a handful of giant reps parallelize
        // as well as millions of small ones
        let ctr = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..jobs {
                s.spawn(|| loop {
                    let i = ctr.fetch_add(
                        1,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    if i >= cand.len() {
                        break;
                    }
                    let _ = results[i].set(find_grids(cand[i]));
                });
            }
        });
    }
    drop(cand);
    let mut res = results
        .into_iter()
        .map(|c| c.into_inner().expect("normalize slot unset"));
    for cell in &mut doc.cells {
        normalize_cell(cell, &mut res);
    }
    debug_assert!(res.next().is_none());
}

const MAGIC: &[u8] = b"%SEMI-OASIS\r\n";

fn new_builder(implicit_ref: u64, implicit_tref: u64) -> Builder {
    Builder {
        cells: Vec::new(),
        by_name: HashMap::new(),
        refnames: HashMap::new(),
        implicit_ref,
        textstrings: HashMap::new(),
        implicit_tref,
        cur: None,
        pending: Vec::new(),
        pending_texts: Vec::new(),
        layer_order: Vec::new(),
        layer_seen: HashMap::new(),
        layer_names: HashMap::new(),
        unit: 0.0,
    }
}

/// placeholder "\u{1}ref{n}" -> table name once the global tables are
/// assembled (unresolvable ones stay for the finish pass to report)
fn resolve_ref_name(refnames: &HashMap<u64, String>, name: String) -> String {
    name.strip_prefix('\u{1}')
        .and_then(|s| s.strip_prefix("ref"))
        .and_then(|s| s.parse::<u64>().ok())
        .and_then(|r| refnames.get(&r).cloned())
        .unwrap_or(name)
}

/// Fold one chunk builder's CELLS into the global builder (tables
/// must already be union-merged so placeholder names resolve here -
/// otherwise a by-ref cell in one chunk and its by-name definition in
/// another would materialize twice).
fn merge_cells(g: &mut Builder, o: Builder) {
    let mut map = Vec::with_capacity(o.cells.len());
    let mut place_base = Vec::with_capacity(o.cells.len());
    let mut text_base = Vec::with_capacity(o.cells.len());
    for cell in o.cells {
        let name = resolve_ref_name(&g.refnames, cell.name);
        let gi = g.cell_index(&name);
        map.push(gi);
        place_base.push(g.cells[gi].places.len());
        text_base.push(g.cells[gi].texts.len());
        g.cells[gi].rects.extend(cell.rects);
        g.cells[gi].polys.extend(cell.polys);
        g.cells[gi].paths.extend(cell.paths);
        g.cells[gi].places.extend(cell.places);
        g.cells[gi].texts.extend(cell.texts);
    }
    for (ci, slot, tgt) in o.pending {
        g.pending.push((map[ci], place_base[ci] + slot, tgt));
    }
    for (ci, slot, r) in o.pending_texts {
        g.pending_texts.push((map[ci], text_base[ci] + slot, r));
    }
}

pub fn parse_doc(data: &[u8]) -> Result<Doc> {
    parse_doc_parallel(data, 1)
}

/// Doc parse over `jobs` threads: the cell_cuts skim splits the
/// stream at CELL boundaries (modal state resets there), workers
/// parse contiguous cell groups into private builders, and the merge
/// unions the name tables first so cross-chunk by-ref cells land in
/// the same Cell entry. Output is byte-identical to the sequential
/// parse: chunk results merge in file order.
pub fn parse_doc_parallel(data: &[u8], jobs: usize) -> Result<Doc> {
    if data.len() < MAGIC.len() || &data[..MAGIC.len()] != MAGIC {
        return err(0, "not an OASIS file");
    }
    let body = &data[MAGIC.len()..];
    if jobs <= 1 {
        let mut c = Cur::new(body, MAGIC.len());
        let mut m = Modal::default();
        let mut b = new_builder(0, 0);
        parse_records(&mut c, &mut m, &mut b, 0)?;
        return finish(b, 1);
    }
    let (head_end, cuts, end_at) = crate::cell_cuts(body, MAGIC.len())?;
    let mut b = new_builder(0, 0);
    {
        let mut hc = Cur::new(&body[..head_end], MAGIC.len());
        let mut hm = Modal::default();
        parse_records(&mut hc, &mut hm, &mut b, 1)?;
    }
    let n_units = cuts.len();
    if n_units == 0 {
        return finish(b, jobs);
    }
    let per = n_units.div_ceil(jobs).max(1);
    let groups: Vec<((usize, u64, u64), usize)> = (0..n_units)
        .step_by(per)
        .map(|i| {
            let end = if i + per < n_units {
                cuts[i + per].0
            } else {
                end_at
            };
            (cuts[i], end)
        })
        .collect();
    let chunks: Vec<Result<Builder>> = std::thread::scope(|s| {
        let handles: Vec<_> = groups
            .iter()
            .map(|&((start, n3, n5), end)| {
                s.spawn(move || {
                    let mut c = Cur::new(
                        &body[start..end],
                        MAGIC.len() + start,
                    );
                    let mut m = Modal::default();
                    let mut cb = new_builder(n3, n5);
                    // depth 1: chunk streams end without END records
                    parse_records(&mut c, &mut m, &mut cb, 1)?;
                    Ok(cb)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("parse worker panicked"))
            .collect()
    });
    let mut builders = Vec::with_capacity(chunks.len());
    for r in chunks {
        builders.push(r?);
    }
    // tables first (any chunk may hold them), then cells in file order
    for cb in &mut builders {
        for (r, n) in std::mem::take(&mut cb.refnames) {
            b.refnames.entry(r).or_insert(n);
        }
        for (r, s) in std::mem::take(&mut cb.textstrings) {
            b.textstrings.entry(r).or_insert(s);
        }
        for &(l, d) in std::mem::take(&mut cb.layer_order).iter() {
            b.reg_layer(l, d);
        }
        for (k, v) in std::mem::take(&mut cb.layer_names) {
            b.layer_names.entry(k).or_insert(v);
        }
    }
    for cb in builders {
        merge_cells(&mut b, cb);
    }
    finish(b, jobs)
}

fn finish(b: Builder, jobs: usize) -> Result<Doc> {
    let mut doc = finish_inner(b)?;
    let t = std::time::Instant::now();
    normalize_reps(&mut doc, jobs);
    doc.norm_s = t.elapsed().as_secs_f64();
    Ok(doc)
}

fn finish_inner(mut b: Builder) -> Result<Doc> {
    // late name tables: rename placeholder cells created from refnums
    let ref_cells: Vec<(usize, String)> = b
        .cells
        .iter()
        .enumerate()
        .filter_map(|(i, cell)| {
            cell.name
                .strip_prefix('\u{1}')
                .and_then(|s| s.strip_prefix("ref"))
                .and_then(|s| s.parse::<u64>().ok())
                .and_then(|r| b.refnames.get(&r).map(|n| (i, n.clone())))
        })
        .collect();
    for (i, name) in ref_cells {
        b.by_name.remove(&b.cells[i].name);
        b.cells[i].name = name.clone();
        b.by_name.insert(name, i);
    }
    // late TEXTSTRING table: fill in referenced strings
    for (cell, slot, r) in std::mem::take(&mut b.pending_texts) {
        match b.textstrings.get(&r) {
            Some(s) => b.cells[cell].texts[slot].s = s.clone(),
            None => return err(0, "unresolved textstring ref"),
        }
    }
    // resolve placements
    let pending = std::mem::take(&mut b.pending);
    for (cell, slot, tgt) in pending {
        let idx = match tgt {
            PlaceTarget::Name(n) => b.cell_index(&n),
            PlaceTarget::Ref(r) => {
                let n = match b.refnames.get(&r) {
                    Some(n) => n.clone(),
                    None => return err(0, "unresolved cellname ref"),
                };
                b.cell_index(&n)
            }
        };
        b.cells[cell].places[slot].cell = idx;
    }
    // top = the cell nobody places (largest content wins ties later;
    // spike expects exactly one)
    let mut placed = vec![false; b.cells.len()];
    for cell in &b.cells {
        for p in &cell.places {
            placed[p.cell] = true;
        }
    }
    let tops: Vec<usize> = (0..b.cells.len()).filter(|&i| !placed[i]).collect();
    if tops.len() != 1 {
        return err(0, &format!("{} top cells (spike expects 1)", tops.len()));
    }
    Ok(Doc {
        unit: b.unit,
        cells: b.cells,
        top: tops[0],
        layer_order: b.layer_order,
        layer_names: b.layer_names,
        norm_s: 0.0,
    })
}
