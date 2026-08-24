//! Record-level OASIS (SEMI P39) stream reader - floe indexer spike S1.
//!
//! Goals that shape the design (and why klayout could not give them to
//! us from Python):
//! - REPETITIONS STAY RECORDS: a 2-billion-member fill array is one
//!   record with member arithmetic, never an expansion.
//! - STREAMING: one forward pass, no in-memory layout database.
//! - The scan inventory (cells / per-layer record+member counts /
//!   texts / placements) is validated against klayout's stored-shape
//!   counts on the same files.
//!
//! CBLOCKs (DEFLATE) are inflated inline; a record never straddles a
//! CBLOCK boundary (spec guarantee), so the record loop simply runs
//! over the inflated bytes with the same modal state.

use std::collections::HashMap;
use std::io::Read;

pub mod doc;
pub mod write;

#[derive(Debug)]
pub enum OasisError {
    Io(std::io::Error),
    Format(String),
}

impl std::fmt::Display for OasisError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            OasisError::Io(e) => write!(f, "io: {}", e),
            OasisError::Format(s) => write!(f, "format: {}", s),
        }
    }
}

impl From<std::io::Error> for OasisError {
    fn from(e: std::io::Error) -> Self {
        OasisError::Io(e)
    }
}

pub(crate) type Result<T> = std::result::Result<T, OasisError>;

pub(crate) fn err<T>(pos: usize, msg: &str) -> Result<T> {
    Err(OasisError::Format(format!("@{}: {}", pos, msg)))
}

// ---------------------------------------------------------------- cursor

pub(crate) struct Cur<'a> {
    data: &'a [u8],
    pos: usize,
    /// byte offset of `data[0]` in the physical file (for messages)
    base: usize,
}

impl<'a> Cur<'a> {
    pub(crate) fn new(data: &'a [u8], base: usize) -> Self {
        Cur { data, pos: 0, base }
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub(crate) fn here(&self) -> usize {
        self.base + self.pos
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub(crate) fn byte(&mut self) -> Result<u8> {
        match self.data.get(self.pos) {
            Some(b) => {
                self.pos += 1;
                Ok(*b)
            }
            None => err(self.here(), "unexpected end of stream"),
        }
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return err(self.here(), "unexpected end of stream (bytes)");
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// base-128 varint, 7 bits per byte, LSB group first
    pub(crate) fn uint(&mut self) -> Result<u64> {
        let mut v: u64 = 0;
        let mut shift = 0u32;
        loop {
            let b = self.byte()?;
            if shift >= 63 && (b & 0x7f) > 1 {
                return err(self.here(), "unsigned integer overflow");
            }
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }

    /// sign-magnitude: LSB of the assembled varint is the sign
    pub(crate) fn sint(&mut self) -> Result<i64> {
        let u = self.uint()?;
        let mag = (u >> 1) as i64;
        Ok(if u & 1 == 1 { -mag } else { mag })
    }

    pub(crate) fn real(&mut self) -> Result<f64> {
        let t = self.uint()?;
        Ok(match t {
            0 => self.uint()? as f64,
            1 => -(self.uint()? as f64),
            2 => 1.0 / self.uint()? as f64,
            3 => -1.0 / self.uint()? as f64,
            4 => {
                let a = self.uint()? as f64;
                let b = self.uint()? as f64;
                a / b
            }
            5 => {
                let a = self.uint()? as f64;
                let b = self.uint()? as f64;
                -a / b
            }
            6 => f32::from_le_bytes(self.bytes(4)?.try_into().unwrap()) as f64,
            7 => f64::from_le_bytes(self.bytes(8)?.try_into().unwrap()),
            _ => return err(self.here(), "bad real type"),
        })
    }

    pub(crate) fn string(&mut self) -> Result<&'a [u8]> {
        let n = self.uint()? as usize;
        self.bytes(n)
    }

    /// g-delta: single-form octangular or two-form general
    pub(crate) fn g_delta(&mut self) -> Result<(i64, i64)> {
        let g = self.uint()?;
        if g & 1 == 0 {
            let dir = (g >> 1) & 7;
            let m = (g >> 4) as i64;
            Ok(match dir {
                0 => (m, 0),
                1 => (0, m),
                2 => (-m, 0),
                3 => (0, -m),
                4 => (m, m),
                5 => (-m, m),
                6 => (-m, -m),
                7 => (m, -m),
                _ => unreachable!(),
            })
        } else {
            let mag = (g >> 2) as i64;
            let x = if g & 2 == 2 { -mag } else { mag };
            let y = self.sint()?;
            Ok((x, y))
        }
    }
}

// ------------------------------------------------------------- repetition

/// Parsed enough to know the member count; kept as a record.
#[derive(Clone, Copy, Debug, Default)]
pub struct Repetition {
    pub members: u64,
}

fn read_repetition(
    c: &mut Cur,
    modal: &mut Option<Repetition>,
    st: &mut ScanStats,
) -> Result<Repetition> {
    let t = c.uint()?;
    *st.rep_types.entry(t).or_default() += 1;
    let rep = match t {
        0 => match modal {
            Some(r) => *r,
            None => return err(c.here(), "repetition reuse before first"),
        },
        1 => {
            let xd = c.uint()?;
            let yd = c.uint()?;
            c.uint()?; // x-space
            c.uint()?; // y-space
            Repetition { members: (xd + 2) * (yd + 2) }
        }
        2 => {
            let xd = c.uint()?;
            c.uint()?;
            Repetition { members: xd + 2 }
        }
        3 => {
            let yd = c.uint()?;
            c.uint()?;
            Repetition { members: yd + 2 }
        }
        4 | 5 => {
            let xd = c.uint()?;
            if t == 5 {
                c.uint()?; // grid
            }
            for _ in 0..=xd {
                c.uint()?;
            }
            Repetition { members: xd + 2 }
        }
        6 | 7 => {
            let yd = c.uint()?;
            if t == 7 {
                c.uint()?;
            }
            for _ in 0..=yd {
                c.uint()?;
            }
            Repetition { members: yd + 2 }
        }
        8 => {
            let nd = c.uint()?;
            let md = c.uint()?;
            c.g_delta()?;
            c.g_delta()?;
            Repetition { members: (nd + 2) * (md + 2) }
        }
        9 => {
            let d = c.uint()?;
            c.g_delta()?;
            Repetition { members: d + 2 }
        }
        10 | 11 => {
            let d = c.uint()?;
            if t == 11 {
                c.uint()?; // grid
            }
            for _ in 0..=d {
                c.g_delta()?;
            }
            Repetition { members: d + 2 }
        }
        _ => return err(c.here(), "bad repetition type"),
    };
    *modal = Some(rep);
    Ok(rep)
}

/// point-list: consume and return the delta count
fn read_point_list(c: &mut Cur) -> Result<u64> {
    let t = c.uint()?;
    let n = c.uint()?;
    match t {
        0 | 1 => {
            for _ in 0..n {
                c.sint()?; // 1-delta
            }
        }
        2 | 3 => {
            for _ in 0..n {
                c.uint()?; // 2-delta / 3-delta (dir bits + magnitude)
            }
        }
        4 | 5 => {
            for _ in 0..n {
                c.g_delta()?;
            }
        }
        _ => return err(c.here(), "bad point-list type"),
    }
    Ok(n)
}

fn read_interval(c: &mut Cur) -> Result<()> {
    match c.uint()? {
        0 => {}
        1 | 2 | 3 => {
            c.uint()?;
        }
        4 => {
            c.uint()?;
            c.uint()?;
        }
        _ => return err(c.here(), "bad interval type")?,
    }
    Ok(())
}

// ------------------------------------------------------------ modal state

#[derive(Default)]
struct Modal {
    repetition: Option<Repetition>,
    layer: Option<u64>,
    datatype: Option<u64>,
    textlayer: Option<u64>,
    texttype: Option<u64>,
    /// last property had a value list (needed only for stream sync of
    /// PROPERTY-repeat records, which carry no fields at all)
    _last_prop: bool,
}

// ------------------------------------------------------------- scan stats

#[derive(Default, Debug)]
pub struct LayerStat {
    pub records: u64,
    pub members: u64,
}

#[derive(Default, Debug)]
pub struct ScanStats {
    pub file_bytes: u64,
    pub cells: u64,
    pub cellnames: u64,
    pub placements: u64,
    pub placement_members: u64,
    pub cblocks: u64,
    pub cblock_bytes_inflated: u64,
    pub records: u64,
    /// record-id -> count (which OASIS constructs a file actually uses)
    pub record_ids: HashMap<u64, u64>,
    /// (layer, datatype) -> geometry record/member counts
    pub shapes: HashMap<(u64, u64), LayerStat>,
    /// (textlayer, texttype) -> text record/member counts
    pub texts: HashMap<(u64, u64), LayerStat>,
    /// repetition-type byte -> count (0 = modal reuse; 1-3/8-9 grids,
    /// 4-7/10-11 point lists) - tells whether a file's density is
    /// grid-encoded or explicit
    pub rep_types: HashMap<u64, u64>,
    pub unit: f64,
}

impl ScanStats {
    fn shape(&mut self, m: &Modal, c: &Cur, members: u64) -> Result<()> {
        let (l, d) = match (m.layer, m.datatype) {
            (Some(l), Some(d)) => (l, d),
            _ => return err(c.here(), "shape before layer/datatype modal"),
        };
        let e = self.shapes.entry((l, d)).or_default();
        e.records += 1;
        e.members += members;
        Ok(())
    }

    fn text(&mut self, m: &Modal, c: &Cur, members: u64) -> Result<()> {
        let (l, d) = match (m.textlayer, m.texttype) {
            (Some(l), Some(d)) => (l, d),
            _ => return err(c.here(), "text before textlayer/texttype modal"),
        };
        let e = self.texts.entry((l, d)).or_default();
        e.records += 1;
        e.members += members;
        Ok(())
    }
}

// ----------------------------------------------------------- record layer

fn rep_members(
    c: &mut Cur,
    modal: &mut Modal,
    st: &mut ScanStats,
    has_rep: bool,
) -> Result<u64> {
    if has_rep {
        Ok(read_repetition(c, &mut modal.repetition, st)?.members)
    } else {
        Ok(1)
    }
}

fn read_property_value(c: &mut Cur) -> Result<()> {
    let t = c.uint()?;
    match t {
        0..=5 => {
            // real forms sharing the type code
            match t {
                0 | 1 | 2 | 3 => {
                    c.uint()?;
                }
                4 | 5 => {
                    c.uint()?;
                    c.uint()?;
                }
                _ => unreachable!(),
            }
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
            c.uint()?; // propstring reference
        }
        _ => return err(c.here(), "bad property value type"),
    }
    Ok(())
}

#[derive(PartialEq)]
enum Rec {
    Normal,
    Cell,
    End,
}

/// Parse one record. `inflate=false` skips CBLOCK bodies without
/// decompressing (used by the parallel skim pass, which only needs
/// record boundaries - field consumption is modal-independent).
fn record(
    c: &mut Cur,
    modal: &mut Modal,
    st: &mut ScanStats,
    inflate: bool,
) -> Result<Rec> {
    let id = c.uint()?;
    if id == 2 {
        return Ok(Rec::End); // END: not counted (parallel workers
    }                        // never see it - keep both paths equal)
    st.records += 1;
    *st.record_ids.entry(id).or_default() += 1;
    match id {
        0 => {} // PAD
        1 => {
            // START: version, unit, offset-flag (+ table when flag==0)
            c.string()?;
            st.unit = c.real()?;
            if c.uint()? == 0 {
                for _ in 0..12 {
                    c.uint()?;
                }
            }
        }
        3 | 4 => {
            // CELLNAME
            c.string()?;
            if id == 4 {
                c.uint()?;
            }
            st.cellnames += 1;
        }
        5 | 6 => {
            c.string()?;
            if id == 6 {
                c.uint()?;
            }
        }
        7 | 8 | 9 | 10 => {
            c.string()?;
            if id == 8 || id == 10 {
                c.uint()?;
            }
        }
        11 | 12 => {
            // LAYERNAME: name + layer interval + datatype interval
            c.string()?;
            read_interval(c)?;
            read_interval(c)?;
        }
        13 => {
            c.uint()?;
            st.cells += 1;
            *modal = Modal::default();
            return Ok(Rec::Cell);
        }
        14 => {
            c.string()?;
            st.cells += 1;
            *modal = Modal::default();
            return Ok(Rec::Cell);
        }
        15 | 16 => {} // XYABSOLUTE / XYRELATIVE (coords consumed as-is)
        17 | 18 => {
            // PLACEMENT
            let info = c.byte()?;
            let (cbit, nbit) = (info & 0x80 != 0, info & 0x40 != 0);
            let (xbit, ybit) = (info & 0x20 != 0, info & 0x10 != 0);
            let rbit = info & 0x08 != 0;
            if cbit {
                if nbit {
                    c.uint()?;
                } else {
                    c.string()?;
                }
            }
            if id == 18 {
                if info & 0x04 != 0 {
                    c.real()?; // magnification
                }
                if info & 0x02 != 0 {
                    c.real()?; // angle
                }
            }
            if xbit {
                c.sint()?;
            }
            if ybit {
                c.sint()?;
            }
            let members = rep_members(c, modal, st, rbit)?;
            st.placements += 1;
            st.placement_members += members;
        }
        19 => {
            // TEXT: 0CNXYRTL
            let info = c.byte()?;
            let (cbit, nbit) = (info & 0x40 != 0, info & 0x20 != 0);
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (tbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if cbit {
                if nbit {
                    c.uint()?;
                } else {
                    c.string()?;
                }
            }
            if lbit {
                modal.textlayer = Some(c.uint()?);
            }
            if tbit {
                modal.texttype = Some(c.uint()?);
            }
            if xbit {
                c.sint()?;
            }
            if ybit {
                c.sint()?;
            }
            let members = rep_members(c, modal, st, rbit)?;
            st.text(modal, c, members)?;
        }
        20 => {
            // RECTANGLE: SWHXYRDL
            let info = c.byte()?;
            let sbit = info & 0x80 != 0;
            let (wbit, hbit) = (info & 0x40 != 0, info & 0x20 != 0);
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if wbit {
                c.uint()?;
            }
            if hbit {
                if sbit {
                    return err(c.here(), "square with H bit");
                }
                c.uint()?;
            }
            if xbit {
                c.sint()?;
            }
            if ybit {
                c.sint()?;
            }
            let members = rep_members(c, modal, st, rbit)?;
            st.shape(modal, c, members)?;
        }
        21 => {
            // POLYGON: 00PXYRDL
            let info = c.byte()?;
            let pbit = info & 0x20 != 0;
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if pbit {
                read_point_list(c)?;
            }
            if xbit {
                c.sint()?;
            }
            if ybit {
                c.sint()?;
            }
            let members = rep_members(c, modal, st, rbit)?;
            st.shape(modal, c, members)?;
        }
        22 => {
            // PATH: EWPXYRDL
            let info = c.byte()?;
            let ebit = info & 0x80 != 0;
            let wbit = info & 0x40 != 0;
            let pbit = info & 0x20 != 0;
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if wbit {
                c.uint()?;
            }
            if ebit {
                let scheme = c.uint()?;
                if (scheme >> 2) & 3 == 3 {
                    c.sint()?; // explicit start extension
                }
                if scheme & 3 == 3 {
                    c.sint()?; // explicit end extension
                }
            }
            if pbit {
                read_point_list(c)?;
            }
            if xbit {
                c.sint()?;
            }
            if ybit {
                c.sint()?;
            }
            let members = rep_members(c, modal, st, rbit)?;
            st.shape(modal, c, members)?;
        }
        23 | 24 | 25 => {
            // TRAPEZOID: 0WHXYRDL (+delta-a / delta-b)
            let info = c.byte()?;
            let (wbit, hbit) = (info & 0x40 != 0, info & 0x20 != 0);
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if wbit {
                c.uint()?;
            }
            if hbit {
                c.uint()?;
            }
            if id == 23 || id == 24 {
                c.sint()?; // delta-a
            }
            if id == 23 || id == 25 {
                c.sint()?; // delta-b
            }
            if xbit {
                c.sint()?;
            }
            if ybit {
                c.sint()?;
            }
            let members = rep_members(c, modal, st, rbit)?;
            st.shape(modal, c, members)?;
        }
        26 => {
            // CTRAPEZOID: TWHXYRDL
            let info = c.byte()?;
            let tbit = info & 0x80 != 0;
            let (wbit, hbit) = (info & 0x40 != 0, info & 0x20 != 0);
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if tbit {
                c.uint()?;
            }
            if wbit {
                c.uint()?;
            }
            if hbit {
                c.uint()?;
            }
            if xbit {
                c.sint()?;
            }
            if ybit {
                c.sint()?;
            }
            let members = rep_members(c, modal, st, rbit)?;
            st.shape(modal, c, members)?;
        }
        27 => {
            // CIRCLE: 00rXYRDL
            let info = c.byte()?;
            let radbit = info & 0x20 != 0;
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            if radbit {
                c.uint()?;
            }
            if xbit {
                c.sint()?;
            }
            if ybit {
                c.sint()?;
            }
            let members = rep_members(c, modal, st, rbit)?;
            st.shape(modal, c, members)?;
        }
        28 => {
            // PROPERTY: UUUUVCNS
            let info = c.byte()?;
            let uuuu = (info >> 4) & 0x0f;
            let vbit = info & 0x08 != 0;
            let cbit = info & 0x04 != 0;
            let nbit = info & 0x02 != 0;
            let sbit = info & 0x01 != 0;
            let _ = sbit;
            if cbit {
                if nbit {
                    c.uint()?;
                } else {
                    c.string()?;
                }
            }
            if !vbit {
                let n = if uuuu == 15 { c.uint()? } else { uuuu as u64 };
                for _ in 0..n {
                    read_property_value(c)?;
                }
            }
        }
        29 => {} // PROPERTY repeat-last
        30 | 31 => {
            // XNAME: attribute + string (+ref)
            c.uint()?;
            c.string()?;
            if id == 31 {
                c.uint()?;
            }
        }
        32 => {
            // XELEMENT: attribute + b-string
            c.uint()?;
            c.string()?;
        }
        33 => {
            // XGEOMETRY: 000XYRDL, attribute, b-string
            let info = c.byte()?;
            let (xbit, ybit) = (info & 0x10 != 0, info & 0x08 != 0);
            let rbit = info & 0x04 != 0;
            let (dbit, lbit) = (info & 0x02 != 0, info & 0x01 != 0);
            c.uint()?; // attribute
            if lbit {
                modal.layer = Some(c.uint()?);
            }
            if dbit {
                modal.datatype = Some(c.uint()?);
            }
            c.string()?;
            if xbit {
                c.sint()?;
            }
            if ybit {
                c.sint()?;
            }
            let members = rep_members(c, modal, st, rbit)?;
            st.shape(modal, c, members)?;
        }
        34 => {
            // CBLOCK: comp-type, uncomp-count, comp-count, bytes
            let ctype = c.uint()?;
            let un = c.uint()? as usize;
            let cn = c.uint()? as usize;
            let comp = c.bytes(cn)?;
            if ctype != 0 {
                return err(c.here(), "unknown CBLOCK compression");
            }
            if inflate {
                let mut out = Vec::with_capacity(un);
                flate2::read::DeflateDecoder::new(comp)
                    .read_to_end(&mut out)
                    .map_err(OasisError::Io)?;
                if out.len() != un {
                    return err(c.here(), "CBLOCK inflate size mismatch");
                }
                st.cblocks += 1;
                st.cblock_bytes_inflated += un as u64;
                let mut sub = Cur::new(&out, c.here());
                while !sub.at_end() {
                    if record(&mut sub, modal, st, true)? == Rec::End {
                        return err(sub.here(), "END inside CBLOCK");
                    }
                }
            }
        }
        _ => return err(c.here(), &format!("unknown record id {}", id)),
    }
    Ok(Rec::Normal)
}

const MAGIC: &[u8] = b"%SEMI-OASIS\r\n";

pub fn scan(data: &[u8]) -> Result<ScanStats> {
    if data.len() < MAGIC.len() || &data[..MAGIC.len()] != MAGIC {
        return err(0, "not an OASIS file (bad magic)");
    }
    let mut c = Cur::new(&data[MAGIC.len()..], MAGIC.len());
    let mut st = ScanStats {
        file_bytes: data.len() as u64,
        ..Default::default()
    };
    let mut modal = Modal::default();
    while !c.at_end() {
        if record(&mut c, &mut modal, &mut st, true)? == Rec::End {
            return Ok(st); // END reached
        }
    }
    err(c.here(), "stream ended without END record")
}

fn merge(into: &mut ScanStats, other: ScanStats) {
    into.cells += other.cells;
    into.cellnames += other.cellnames;
    into.placements += other.placements;
    into.placement_members += other.placement_members;
    into.cblocks += other.cblocks;
    into.cblock_bytes_inflated += other.cblock_bytes_inflated;
    into.records += other.records;
    for (k, v) in other.shapes {
        let e = into.shapes.entry(k).or_default();
        e.records += v.records;
        e.members += v.members;
    }
    for (k, v) in other.texts {
        let e = into.texts.entry(k).or_default();
        e.records += v.records;
        e.members += v.members;
    }
    for (k, v) in other.record_ids {
        *into.record_ids.entry(k).or_default() += v;
    }
    for (k, v) in other.rep_types {
        *into.rep_types.entry(k).or_default() += v;
    }
}

/// Skim pass shared by the parallel scan and the parallel doc parse:
/// CELL record offsets (CBLOCK bodies skipped, not inflated) plus,
/// per cut, how many IMPLICIT name-table records (CELLNAME id 3,
/// TEXTSTRING id 5) precede it - chunk parsers seed their implicit
/// refnum counters from that prefix so global numbering survives the
/// split. Assumes name tables sit outside CBLOCKs (klayout does).
/// Returns (head_end, [(cut_offset, cellname_base, textstring_base)],
/// end_offset).
pub(crate) fn cell_cuts(
    body: &[u8],
    file_off: usize,
) -> Result<(usize, Vec<(usize, u64, u64)>, usize)> {
    let mut c = Cur::new(body, file_off);
    let mut st = ScanStats::default(); // discarded
    let mut modal = Modal::default();
    let mut head_end = None;
    let mut cuts: Vec<(usize, u64, u64)> = Vec::new();
    let (mut n3, mut n5) = (0u64, 0u64);
    loop {
        if c.at_end() {
            return err(c.here(), "stream ended without END record");
        }
        let at = c.pos;
        let id = {
            let mut pc = Cur::new(&body[at..], 0);
            pc.uint()?
        };
        match record(&mut c, &mut modal, &mut st, false)? {
            Rec::Cell => {
                if head_end.is_none() {
                    head_end = Some(at);
                }
                cuts.push((at, n3, n5));
            }
            Rec::End => {
                let end_at = at;
                return Ok((head_end.unwrap_or(end_at), cuts, end_at));
            }
            Rec::Normal => {
                if id == 3 {
                    n3 += 1;
                } else if id == 5 {
                    n5 += 1;
                }
            }
        }
    }
}

/// Parallel scan: a cheap skim pass (CBLOCK bodies skipped, not
/// inflated) splits the stream at CELL boundaries - modal state resets
/// there, so each contiguous cell-group parses independently. Workers
/// then run the full parser (inflating) over their byte ranges. No
/// GIL, no shared state: the whole file's parse work spreads over
/// `jobs` OS threads.
pub fn scan_parallel(data: &[u8], jobs: usize) -> Result<ScanStats> {
    if jobs <= 1 {
        return scan(data);
    }
    if data.len() < MAGIC.len() || &data[..MAGIC.len()] != MAGIC {
        return err(0, "not an OASIS file (bad magic)");
    }
    let body = &data[MAGIC.len()..];
    // ---- skim: find CELL record offsets and the END offset ----------
    let mut c = Cur::new(body, MAGIC.len());
    let mut skim_stats = ScanStats::default(); // discarded (recounted)
    let mut modal = Modal::default();
    let mut head_end = None; // offset of first CELL record in `body`
    let mut cuts: Vec<usize> = Vec::new();
    let end_at;
    loop {
        if c.at_end() {
            return err(c.here(), "stream ended without END record");
        }
        let at = c.pos;
        match record(&mut c, &mut modal, &mut skim_stats, false)? {
            Rec::Cell => {
                if head_end.is_none() {
                    head_end = Some(at);
                }
                cuts.push(at);
            }
            Rec::End => {
                end_at = at;
                break;
            }
            Rec::Normal => {}
        }
    }
    let head_end = head_end.unwrap_or(end_at);
    // ---- head (tables before the first cell): parse inline ----------
    let mut st = ScanStats {
        file_bytes: data.len() as u64,
        ..Default::default()
    };
    {
        let mut hc = Cur::new(&body[..head_end], MAGIC.len());
        let mut hm = Modal::default();
        while !hc.at_end() {
            record(&mut hc, &mut hm, &mut st, true)?;
        }
    }
    // ---- cell groups over worker threads ----------------------------
    cuts.push(end_at);
    let n_units = cuts.len() - 1;
    let per = (n_units + jobs - 1) / jobs.max(1);
    let groups: Vec<(usize, usize)> = (0..n_units)
        .step_by(per.max(1))
        .map(|i| (cuts[i], cuts[(i + per).min(n_units)]))
        .collect();
    let results: Vec<Result<ScanStats>> = std::thread::scope(|s| {
        let handles: Vec<_> = groups
            .iter()
            .map(|&(a, b)| {
                s.spawn(move || {
                    let mut wc = Cur::new(&body[a..b], MAGIC.len() + a);
                    let mut wm = Modal::default();
                    let mut ws = ScanStats::default();
                    while !wc.at_end() {
                        if record(&mut wc, &mut wm, &mut ws, true)?
                            == Rec::End
                        {
                            return err(wc.here(), "END inside group");
                        }
                    }
                    Ok(ws)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for r in results {
        merge(&mut st, r?);
    }
    Ok(st)
}
