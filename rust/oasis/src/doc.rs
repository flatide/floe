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
#[derive(Clone, Debug)]
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

/// point list -> vertex deltas from the anchor (implicit closing
/// vertex for the manhattan types included)
fn read_points(c: &mut Cur) -> Result<Vec<(i64, i64)>> {
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
            if x != 0 || y != 0 {
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
                    m.poly_pts = Some(read_points(c)?);
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
            22 => return err(c.here(), "PATH: out of spike scope"),
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

const MAGIC: &[u8] = b"%SEMI-OASIS\r\n";

pub fn parse_doc(data: &[u8]) -> Result<Doc> {
    if data.len() < MAGIC.len() || &data[..MAGIC.len()] != MAGIC {
        return err(0, "not an OASIS file");
    }
    let mut c = Cur::new(&data[MAGIC.len()..], MAGIC.len());
    let mut m = Modal::default();
    let mut b = Builder {
        cells: Vec::new(),
        by_name: HashMap::new(),
        refnames: HashMap::new(),
        implicit_ref: 0,
        textstrings: HashMap::new(),
        implicit_tref: 0,
        cur: None,
        pending: Vec::new(),
        pending_texts: Vec::new(),
        layer_order: Vec::new(),
        layer_seen: HashMap::new(),
        layer_names: HashMap::new(),
        unit: 0.0,
    };
    parse_records(&mut c, &mut m, &mut b, 0)?;
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
    })
}
