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
use std::sync::Arc;

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
    /// Shared because OASIS modal repetition reuse is common in fill data.
    /// Cloning a record must not clone an arbitrarily large offset vector.
    Pts(Arc<[(i64, i64)]>),
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
    /// display name selected as the last applicable non-empty LAYERNAME
    pub layer_names: HashMap<(u32, u32), String>,
    /// all non-empty LAYERNAME aliases applying to each concrete pair,
    /// in source appearance order; layer_names is the last alias
    pub layer_aliases: HashMap<(u32, u32), Vec<String>>,
}

// ------------------------------------------------------------ modal set

/// first-quadrant cosine table for the CIRCLE 64-gon:
/// cos(k*pi/32), k = 0..=16. LITERALS, not runtime cos() - libm
/// may differ by an ulp across platforms and the built page bytes
/// must not depend on the build host.
const COS64: [f64; 17] = [
    1.0,
    0.9951847266721969,
    0.9807852804032304,
    0.9569403357322088,
    0.9238795325112867,
    0.881921264348355,
    0.8314696123025452,
    0.773010453362737,
    0.7071067811865476,
    0.6343932841636455,
    0.5555702330196022,
    0.4713967368259976,
    0.3826834323650898,
    0.29028467725446233,
    0.19509032201612825,
    0.0980171403295606,
    0.0,
];

/// CIRCLE materialization: inscribed 64-gon, vertex 0 on +x, CCW.
/// The four axis vertices are exact, so the polygon bbox equals
/// the circle's. Consecutive duplicates collapse (a radius under
/// ~6 dbu rounds neighbors together) - the count stays >= 4 for
/// any r >= 1.
fn circle64(x: i64, y: i64, r: i64) -> Vec<(i64, i64)> {
    let rf = r as f64;
    let mut pts: Vec<(i64, i64)> = Vec::with_capacity(64);
    for k in 0..64usize {
        let (q, i) = (k / 16, k % 16);
        let (c, s) = (COS64[i], COS64[16 - i]);
        let (cx, cy) = match q {
            0 => (c, s),
            1 => (-s, c),
            2 => (-c, -s),
            _ => (s, -c),
        };
        let p = (
            x + (rf * cx).round() as i64,
            y + (rf * cy).round() as i64,
        );
        if pts.last() != Some(&p) {
            pts.push(p);
        }
    }
    while pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    pts
}

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
    circle_r: Option<i64>,
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
            let deltas = bounded_encoded_count(c, xd, 1, "repetition x offsets")?;
            let mut pts = Vec::with_capacity(checked_capacity(c, deltas, 1)?);
            let mut x = 0i64;
            pts.push((0, 0));
            for _ in 0..deltas {
                x += c.uint()? as i64 * g;
                pts.push((x, 0));
            }
            Rep::Pts(pts.into())
        }
        6 | 7 => {
            let yd = c.uint()?;
            let g = if t == 7 { c.uint()? as i64 } else { 1 };
            let deltas = bounded_encoded_count(c, yd, 1, "repetition y offsets")?;
            let mut pts = Vec::with_capacity(checked_capacity(c, deltas, 1)?);
            let mut y = 0i64;
            pts.push((0, 0));
            for _ in 0..deltas {
                y += c.uint()? as i64 * g;
                pts.push((0, y));
            }
            Rep::Pts(pts.into())
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
            let deltas = bounded_encoded_count(c, d, 1, "repetition point offsets")?;
            let mut pts = Vec::with_capacity(checked_capacity(c, deltas, 1)?);
            let (mut x, mut y) = (0i64, 0i64);
            pts.push((0, 0));
            for _ in 0..deltas {
                let (dx, dy) = c.g_delta()?;
                x += dx * g;
                y += dy * g;
                pts.push((x, y));
            }
            Rep::Pts(pts.into())
        }
        _ => return err(c.here(), "bad repetition type"),
    };
    *modal = Some(rep.clone());
    Ok(rep)
}

fn bounded_encoded_count(
    c: &Cur<'_>,
    declared: u64,
    encoded_extra: u64,
    field: &str,
) -> Result<usize> {
    let encoded = match declared.checked_add(encoded_extra) {
        Some(value) => value,
        None => return err(c.here(), &format!("{} count overflow", field)),
    };
    let encoded: usize = match encoded.try_into() {
        Ok(value) => value,
        Err(_) => return err(c.here(), &format!("{} count exceeds platform limit", field)),
    };
    if encoded > c.remaining() {
        return err(
            c.here(),
            &format!(
                "{} count {} exceeds {} remaining bytes",
                field,
                encoded,
                c.remaining()
            ),
        );
    }
    Ok(encoded)
}

fn checked_capacity(c: &Cur<'_>, encoded: usize, extra: usize) -> Result<usize> {
    match encoded.checked_add(extra) {
        Some(value) => Ok(value),
        None => err(c.here(), "point-list capacity overflow"),
    }
}

/// point list -> vertex deltas from the anchor. `closed`: polygon
/// semantics (implicit closing vertex for the manhattan types);
/// path point lists are open chains.
fn read_points(c: &mut Cur, closed: bool) -> Result<Vec<(i64, i64)>> {
    let t = c.uint()?;
    let declared = c.uint()?;
    let n = bounded_encoded_count(c, declared, 0, "point-list entries")?;
    let mut pts: Vec<(i64, i64)> = Vec::with_capacity(checked_capacity(c, n, 2)?);
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
    layer_name_rules: Vec<LayerNameRule>,
    unit: f64,
}

#[derive(Clone)]
struct NameInterval {
    lo: u64,
    hi: u64,
}

impl NameInterval {
    fn exact(&self) -> Option<u32> {
        if self.lo == self.hi {
            u32::try_from(self.lo).ok()
        } else {
            None
        }
    }

    fn contains(&self, value: u32) -> bool {
        self.lo <= value as u64 && value as u64 <= self.hi
    }
}

#[derive(Clone)]
struct LayerNameRule {
    name: String,
    layer: NameInterval,
    datatype: NameInterval,
}

fn name_interval(c: &mut Cur) -> Result<NameInterval> {
    let interval = match c.uint()? {
        0 => NameInterval { lo: 0, hi: u64::MAX },
        1 => NameInterval { lo: 0, hi: c.uint()? },
        2 => NameInterval { lo: c.uint()?, hi: u64::MAX },
        3 => {
            let v = c.uint()?;
            NameInterval { lo: v, hi: v }
        }
        4 => {
            let lo = c.uint()?;
            let hi = c.uint()?;
            if lo > hi {
                return err(c.here(), "reversed interval");
            }
            NameInterval { lo, hi }
        }
        _ => return err(c.here(), "bad interval"),
    };
    Ok(interval)
}

fn resolved_layer_names(
    order: &[(u32, u32)],
    rules: &[LayerNameRule],
) -> (
    HashMap<(u32, u32), String>,
    HashMap<(u32, u32), Vec<String>>,
) {
    let mut names = HashMap::new();
    let mut aliases = HashMap::new();
    for &(layer, datatype) in order {
        let mut matched = Vec::new();
        for rule in rules {
            if !rule.name.is_empty()
                && rule.layer.contains(layer)
                && rule.datatype.contains(datatype)
                && !matched.contains(&rule.name)
            {
                matched.push(rule.name.clone());
            }
        }
        if let Some(name) = matched.last() {
            names.insert((layer, datatype), name.clone());
            aliases.insert((layer, datatype), matched);
        }
    }
    (names, aliases)
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
                // LAYERNAME: exact pairs participate in layer appearance
                // order. Keep every interval rule until all concrete pairs
                // are known; KLayout exposes overlapping aliases as
                // "aaa;bbb", while Floe displays the last alias.
                let sb = c.string()?.to_vec();
                let name = utf8(c, &sb)?;
                let layer = name_interval(c)?;
                let datatype = name_interval(c)?;
                if let (Some(l), Some(d)) =
                    (layer.exact(), datatype.exact())
                {
                    b.reg_layer(l, d);
                }
                b.layer_name_rules.push(LayerNameRule {
                    name, layer, datatype,
                });
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
            27 => {
                // CIRCLE (00rXYRDL) - materialized as an inscribed
                // 64-gon PolyRec so it rides the existing polygon
                // page path unchanged (no OVM/OVP format bump).
                let info = c.byte()?;
                let cur = match b.cur {
                    Some(i) => i,
                    None => {
                        return err(c.here(), "shape outside cell")
                    }
                };
                if info & 0x01 != 0 {
                    m.layer = Some(c.uint()?);
                }
                if info & 0x02 != 0 {
                    m.datatype = Some(c.uint()?);
                }
                if info & 0x20 != 0 {
                    let r = c.uint()?;
                    m.circle_r = Some(match i64::try_from(r) {
                        Ok(v) => v,
                        Err(_) => {
                            return err(
                                c.here(),
                                "circle radius out of range",
                            )
                        }
                    });
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
                    _ => {
                        return err(
                            c.here(),
                            "circle before layer modal",
                        )
                    }
                };
                let r = match m.circle_r {
                    Some(r) => r,
                    None => {
                        return err(
                            c.here(),
                            "circle without radius",
                        )
                    }
                };
                if r > 0 {
                    b.reg_layer(l, d);
                    b.cells[cur].polys.push(PolyRec {
                        layer: l,
                        dt: d,
                        pts: circle64(m.geo_x, m.geo_y, r),
                        rep,
                    });
                }
                // r == 0 defines no geometry; a 1-point "polygon"
                // would not survive the page re-encode - skip
            }
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

/// Keep normalization results for only a small consecutive cell window while
/// still exposing enough record-level tasks to the worker pool. A one-cell
/// window starves designs with one expensive repetition per cell; an all-chip
/// window caused the production parse peak.
const NORMALIZE_CELL_BATCH_MAX: usize = 8;

/// Split records with big point-list repetitions into gridded parts
/// plus a leftover point list. Coverage and member counts are
/// invariant; the tiler then splits the grids ARITHMETICALLY per
/// tile and only the true leftovers stay explicit. `res` feeds the
/// precomputed find_grids result for every candidate rep in walk
/// order (rects, polys, paths, texts, places).
fn normalize_cell(
    cell: &mut Cell,
    res: &mut impl Iterator<Item = Arc<GridDecomp>>,
) {
    macro_rules! norm {
        ($vec:expr, $shift:expr) => {{
            let old = std::mem::take(&mut $vec);
            for rec in old {
                let decomp = if matches!(
                    &rec.rep,
                    Rep::Pts(p) if p.len() >= 16
                ) {
                    res.next().expect("normalize task list out of sync")
                } else {
                    $vec.push(rec);
                    continue;
                };
                match decomp.as_ref() {
                    None => $vec.push(rec),
                    Some((arrays, leftovers)) => {
                        for &(x0, y0, dx, nx, dy, ny) in arrays {
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
                                        .collect::<Vec<_>>()
                                        .into(),
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
fn pts_candidates(
    cell: &Cell,
) -> impl Iterator<Item = &Arc<[(i64, i64)]>> {
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
    // Evaluate at record granularity for load balance, but retain results for
    // only one bounded cell window. The old chip-wide task/result arrays kept
    // every grid and leftover list next to the still-unmodified Doc until the
    // final apply pass. Cell order and candidate walk order are unchanged, so
    // output stays byte-identical at every worker count.
    for cells in doc.cells.chunks_mut(NORMALIZE_CELL_BATCH_MAX) {
        // Modal repetition reuse gives many records the same Arc. Normalize
        // that point set once, then share the immutable decomposition across
        // all records that reference it. This prevents N workers from sorting
        // N copies of the same die-wide fill repetition simultaneously.
        let mut unique: Vec<&[(i64, i64)]> = Vec::new();
        let mut unique_by_ptr: HashMap<(usize, usize), usize> =
            HashMap::new();
        let mut candidate_unique_by_cell: Vec<Vec<usize>> =
            Vec::with_capacity(cells.len());
        for cell in cells.iter() {
            let mut candidate_unique = Vec::new();
            for p in pts_candidates(cell) {
                let key = (p.as_ptr() as usize, p.len());
                let ui = match unique_by_ptr.get(&key) {
                    Some(&ui) => ui,
                    None => {
                        let ui = unique.len();
                        unique.push(p.as_ref());
                        unique_by_ptr.insert(key, ui);
                        ui
                    }
                };
                candidate_unique.push(ui);
            }
            candidate_unique_by_cell.push(candidate_unique);
        }
        if unique.is_empty() {
            continue;
        }
        let mut results: Vec<std::sync::OnceLock<GridDecomp>> = Vec::new();
        results.resize_with(unique.len(), std::sync::OnceLock::new);
        if jobs <= 1 || unique.len() < 2 {
            for (p, r) in unique.iter().zip(results.iter_mut()) {
                let _ = r.set(find_grids(p));
            }
        } else {
            let ctr = std::sync::atomic::AtomicUsize::new(0);
            std::thread::scope(|s| {
                for _ in 0..jobs.min(unique.len()) {
                    let unique = &unique;
                    let results = &results;
                    let ctr = &ctr;
                    s.spawn(move || loop {
                        let i = ctr.fetch_add(
                            1,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        if i >= unique.len() {
                            return;
                        }
                        let _ = results[i].set(find_grids(unique[i]));
                    });
                }
            });
        }
        drop(unique);
        let results: Vec<Arc<GridDecomp>> = results
            .into_iter()
            .map(|c| {
                Arc::new(c.into_inner().expect("normalize slot unset"))
            })
            .collect();
        for (cell, candidate_unique) in
            cells.iter_mut().zip(candidate_unique_by_cell)
        {
            let mut res = candidate_unique
                .into_iter()
                .map(|ui| Arc::clone(&results[ui]));
            normalize_cell(cell, &mut res);
            debug_assert!(res.next().is_none());
        }
    }
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
        layer_name_rules: Vec::new(),
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

pub fn parse_doc_parallel(data: &[u8], jobs: usize) -> Result<Doc> {
    parse_doc_parallel_phased(data, jobs, |_| {})
}

/// Parse with a hook between syntax materialization and repetition
/// normalization. The indexer uses it for an exact RSS boundary; keeping the
/// hook in the parser avoids labeling normalize scratch as syntax-parse RAM.
pub fn parse_doc_parallel_phased(
    data: &[u8],
    jobs: usize,
    syntax_done: impl FnOnce(&Doc),
) -> Result<Doc> {
    let mut doc = parse_doc_syntax(data, jobs)?;
    syntax_done(&doc);
    let t = std::time::Instant::now();
    normalize_reps(&mut doc, jobs);
    doc.norm_s = t.elapsed().as_secs_f64();
    Ok(doc)
}

/// Doc parse over `jobs` threads: the cell_cuts skim splits the
/// stream at CELL boundaries (modal state resets there), workers
/// parse contiguous cell groups into private builders, and the merge
/// unions the name tables first so cross-chunk by-ref cells land in
/// the same Cell entry. Output is byte-identical to the sequential
/// parse: chunk results merge in file order.
fn parse_doc_syntax(data: &[u8], jobs: usize) -> Result<Doc> {
    if data.len() < MAGIC.len() || &data[..MAGIC.len()] != MAGIC {
        return err(0, "not an OASIS file");
    }
    let body = &data[MAGIC.len()..];
    if jobs <= 1 {
        let mut c = Cur::new(body, MAGIC.len());
        let mut m = Modal::default();
        let mut b = new_builder(0, 0);
        parse_records(&mut c, &mut m, &mut b, 0)?;
        return finish_inner(b);
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
        return finish_inner(b);
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
        b.layer_name_rules.append(&mut cb.layer_name_rules);
    }
    for cb in builders {
        merge_cells(&mut b, cb);
    }
    finish_inner(b)
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
    let (layer_names, layer_aliases) =
        resolved_layer_names(&b.layer_order, &b.layer_name_rules);
    Ok(Doc {
        unit: b.unit,
        cells: b.cells,
        top: tops[0],
        layer_order: b.layer_order,
        layer_names,
        layer_aliases,
        norm_s: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_aliases_apply_intervals_and_last_alias_wins() {
        let interval = |lo, hi| NameInterval { lo, hi };
        let rules = vec![
            LayerNameRule {
                name: "aaa".into(),
                layer: interval(15, 15),
                datatype: interval(192, 192),
            },
            LayerNameRule {
                name: "bbb".into(),
                layer: interval(15, 15),
                datatype: interval(190, 200),
            },
            // Repeated matching rules do not create repeated aliases.
            LayerNameRule {
                name: "bbb".into(),
                layer: interval(15, 15),
                datatype: interval(192, 192),
            },
            LayerNameRule {
                name: "ccc".into(),
                layer: interval(16, 16),
                datatype: interval(192, 192),
            },
        ];
        let (names, aliases) = resolved_layer_names(
            &[(15, 192), (15, 0), (16, 192)],
            &rules,
        );
        assert_eq!(aliases[&(15, 192)], ["aaa", "bbb"]);
        assert_eq!(names[&(15, 192)], "bbb");
        assert!(!aliases.contains_key(&(15, 0)));
        assert_eq!(aliases[&(16, 192)], ["ccc"]);
    }

    /// CIRCLE (record 27) parses into an inscribed 64-gon PolyRec:
    /// exact axis vertices (bbox == circle bbox), modal reuse for
    /// layer/datatype/radius, repetition pass-through, and r == 0
    /// producing no geometry. Byte-level - klayout's python API
    /// cannot author CIRCLE records directly.
    #[test]
    fn circle_records_become_64gon_polys() {
        use crate::write::W;
        let mut w = W::new();
        w.out.extend_from_slice(b"%SEMI-OASIS\r\n");
        w.uint(1); // START
        w.string(b"1.0");
        w.real_f64(1000.0);
        w.uint(0);
        for _ in 0..12 {
            w.uint(0);
        }
        w.uint(14); // CELL by name
        w.string(b"TOP");
        w.uint(27); // CIRCLE: L D R X Y (0x3B)
        w.byte(0x3B);
        w.uint(7); // layer
        w.uint(1); // datatype
        w.uint(100); // radius
        w.sint(1000); // x
        w.sint(-500); // y
        w.uint(27); // CIRCLE: X + repetition (0x14), modals reused
        w.byte(0x14);
        w.sint(5000); // x
        w.uint(2); // rep type 2: horizontal, 3 columns
        w.uint(1); // dimension - 2
        w.uint(400); // x-space
        w.uint(27); // CIRCLE: R only (0x20) - r = 0, no geometry
        w.byte(0x20);
        w.uint(0);
        w.uint(2); // END
        let doc = parse_doc(&w.out).expect("circle fixture parses");
        let top = &doc.cells[doc.top];
        assert_eq!(top.polys.len(), 2, "r=0 must not emit");
        let p = &top.polys[0];
        assert_eq!((p.layer, p.dt), (7, 1));
        assert_eq!(p.pts.len(), 64, "r=100: all vertices distinct");
        let xs: Vec<i64> = p.pts.iter().map(|q| q.0).collect();
        let ys: Vec<i64> = p.pts.iter().map(|q| q.1).collect();
        assert_eq!(
            (
                *xs.iter().min().unwrap(),
                *xs.iter().max().unwrap(),
                *ys.iter().min().unwrap(),
                *ys.iter().max().unwrap()
            ),
            (900, 1100, -600, -400),
            "bbox must equal the circle bbox exactly"
        );
        assert_eq!(p.pts[0], (1100, -500), "vertex 0 on +x");
        assert_eq!(p.pts[16], (1000, -400), "vertex 16 on +y");
        // every vertex within half a dbu of the true circle
        for &(vx, vy) in &p.pts {
            let (dx, dy) = (vx - 1000, vy + 500);
            let d2 = dx * dx + dy * dy;
            assert!(
                (99 * 99..=100 * 100 + 100).contains(&d2),
                "vertex off circle: ({}, {}) d2={}",
                vx,
                vy,
                d2
            );
        }
        // second circle: modal radius/layer reused, x moved, rep
        let p2 = &top.polys[1];
        assert_eq!((p2.layer, p2.dt), (7, 1));
        assert_eq!(p2.pts[0], (5100, -500));
        assert_eq!(p2.rep.members(), 3);
        // tiny radius: consecutive duplicates collapse but the
        // polygon stays closed and >= 4 points
        let tiny = circle64(0, 0, 1);
        assert!(tiny.len() >= 4 && tiny.len() < 64);
        assert_eq!(tiny[0], (1, 0));
    }

    /// Byte-level LAYERNAME fixture: klayout only ever writes exact
    /// (type 3) intervals, so no suite asset exercises the type
    /// 0/1/2/4 decoders in name_interval(). A mis-consumed operand
    /// there would desync the record stream and misparse the REST of
    /// the file - hence the rects placed AFTER the rule block.
    #[test]
    fn layername_interval_records_parse_and_resolve() {
        use crate::write::W;
        let mut w = W::new();
        w.out.extend_from_slice(b"%SEMI-OASIS\r\n");
        w.uint(1); // START
        w.string(b"1.0");
        w.real_f64(1000.0);
        w.uint(0);
        for _ in 0..12 {
            w.uint(0);
        }
        // every interval type once (name, layer-interval, dt-interval)
        w.uint(11); // "all": type 0 x type 0 - matches every pair
        w.string(b"all");
        w.uint(0);
        w.uint(0);
        w.uint(11); // "low": layer 0..=100 (type 1), dt exactly 0
        w.string(b"low");
        w.uint(1);
        w.uint(100);
        w.uint(3);
        w.uint(0);
        w.uint(11); // "high": layer 200.. (type 2), any dt
        w.string(b"high");
        w.uint(2);
        w.uint(200);
        w.uint(0);
        w.uint(11); // "band": layer 10..=20 x dt 0..=5 (type 4)
        w.string(b"band");
        w.uint(4);
        w.uint(10);
        w.uint(20);
        w.uint(4);
        w.uint(0);
        w.uint(5);
        w.uint(11); // "pin": exact 15/0 (type 3) - registers the pair
        w.string(b"pin");
        w.uint(3);
        w.uint(15);
        w.uint(3);
        w.uint(0);
        w.uint(11); // "eq4": type 4 with lo == hi is exact too
        w.string(b"eq4");
        w.uint(4);
        w.uint(33);
        w.uint(33);
        w.uint(4);
        w.uint(0);
        w.uint(0);
        // geometry AFTER the rules: the desync guard
        w.uint(14); // CELL by name
        w.string(b"TOP");
        w.uint(20); // RECTANGLE on 15/0: L D W H X Y
        w.byte(0x7B);
        w.uint(15);
        w.uint(0);
        w.uint(4);
        w.uint(5);
        w.sint(1);
        w.sint(2);
        w.uint(20); // RECTANGLE on 250/7
        w.byte(0x7B);
        w.uint(250);
        w.uint(7);
        w.uint(6);
        w.uint(7);
        w.sint(3);
        w.sint(4);
        w.uint(2); // END
        let doc = parse_doc(&w.out).expect("fixture parses");
        // the stream survived every interval operand
        let top = &doc.cells[doc.top];
        assert_eq!(top.rects.len(), 2);
        assert_eq!(
            (top.rects[1].layer, top.rects[1].dt, top.rects[1].x),
            (250, 7, 3)
        );
        // 15/0: every applicable alias in source order, last is
        // the display name
        assert_eq!(
            doc.layer_aliases[&(15, 0)],
            ["all", "low", "band", "pin"]
        );
        assert_eq!(doc.layer_names[&(15, 0)], "pin");
        // 250/7: the lower-bounded rule applies, the bounded ones
        // do not
        assert_eq!(doc.layer_aliases[&(250, 7)], ["all", "high"]);
        assert_eq!(doc.layer_names[&(250, 7)], "high");
        // exact rules REGISTER their pair (type 3 and equal type 4
        // alike), interval-only rules never create pairs
        assert!(doc.layer_order.contains(&(15, 0)));
        assert!(doc.layer_order.contains(&(33, 0)));
        assert_eq!(doc.layer_aliases[&(33, 0)], ["all", "low", "eq4"]);
        assert!(!doc
            .layer_order
            .iter()
            .any(|&(l, _)| l == 100 || l == 200 || l == 10));
    }

    #[test]
    fn modal_pts_reuse_shares_the_offset_storage() {
        // type 4, xd=0, one delta=7 -> [(0,0), (7,0)]
        let mut modal = None;
        let first = read_rep(&mut Cur::new(&[4, 0, 7], 0), &mut modal)
            .expect("first repetition");
        // type 0 reuses the modal value. This must be an Arc clone, not a
        // point-by-point allocation proportional to the repetition size.
        let reused = read_rep(&mut Cur::new(&[0], 0), &mut modal)
            .expect("modal repetition");
        match (&first, &reused) {
            (Rep::Pts(a), Rep::Pts(b)) => {
                assert!(Arc::ptr_eq(a, b));
                assert_eq!(a.as_ref(), &[(0, 0), (7, 0)]);
            }
            _ => panic!("expected explicit point repetitions"),
        }
    }

    #[test]
    fn oversized_point_repetitions_fail_before_allocation() {
        use crate::write::W;

        for repetition_type in [4, 5, 6, 7, 10, 11] {
            let mut w = W::new();
            w.uint(repetition_type);
            w.uint(1 << 40);
            if matches!(repetition_type, 5 | 7 | 11) {
                w.uint(1);
            }
            let error = read_rep(&mut Cur::new(&w.out, 0), &mut None).unwrap_err();
            assert!(
                error.to_string().contains("exceeds 0 remaining bytes"),
                "type {repetition_type}: {error}"
            );
        }
    }
}
