//! The viewer's cell tree over renderd's hier thread (docs/RUST_RENDERER.md
//! `cell_*`): typed requests, bounded answers, hex names decoded here. These
//! never touch the render generation or the pick/snap scene bookkeeping.
use crate::{protocol::MAX_LINE_BYTES, query::unhex, Error, Fields, Result};
use std::fmt::Write;

/// renderd's own caps (CELLS_CHILD_CAP, CELL_FIND_CAP, CELL_INSTS_CAP): an
/// answer above them is a protocol fault, not a bigger allocation.
pub const CELLS_CHILD_CAP: usize = 20_000;
pub const CELL_FIND_CAP: usize = 5_000;
pub const CELL_INSTS_CAP: usize = 4_096;
/// Sources one answer may list: a deck's spec entries, bounded by the spec.
pub const CELL_SOURCES_CAP: usize = 65_536;
/// A search pattern is hex-encoded on the wire; the line limit bounds it
/// anyway, this keeps a hostile pattern from being the whole command.
pub const CELL_PATTERN_BYTES: usize = 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum CellRequest {
    /// `cell_sources`: one source for a layout, the deck's sources in spec order.
    Sources,
    /// `cells`: a cell's distinct children; `cell` None = the source's top.
    Children { source: usize, cell: Option<u32> },
    /// `cell_find`: substring or whole-name glob; `source` None = every source.
    Find {
        source: Option<usize>,
        pattern: String,
        limit: usize,
    },
    /// `cell_bbox`: recursive extent under `root` (None = the top).
    Bbox {
        source: usize,
        cell: u32,
        root: Option<u32>,
    },
    /// `cell_insts`: instance boxes meeting `view`, at most `cap`.
    Insts {
        source: usize,
        cell: u32,
        view: [f64; 4],
        cap: usize,
        root: Option<u32>,
    },
}
impl CellRequest {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Sources => "cell_sources",
            Self::Children { .. } => "cells",
            Self::Find { .. } => "cell_find",
            Self::Bbox { .. } => "cell_bbox",
            Self::Insts { .. } => "cell_insts",
        }
    }
    pub fn root(&self) -> Option<u32> {
        match self {
            Self::Bbox { root, .. } | Self::Insts { root, .. } => *root,
            _ => None,
        }
    }
    pub(crate) fn command(&self, sequence: u64) -> Result<String> {
        if sequence == 0 || sequence > i64::MAX as u64 {
            return Err(Error::input("invalid cell query sequence"));
        }
        let source_index = |s: usize| {
            i64::try_from(s)
                .map_err(|_| Error::input("invalid cell source index"))
                .map(|_| s)
        };
        let mut command = match self {
            Self::Sources => format!("cell_sources seq={sequence}"),
            Self::Children { source, cell } => {
                let mut c = format!("cells seq={sequence} src={}", source_index(*source)?);
                if let Some(cell) = cell {
                    write!(c, " cell={cell}").unwrap();
                }
                c
            }
            Self::Find {
                source,
                pattern,
                limit,
            } => {
                if pattern.len() > CELL_PATTERN_BYTES || !(1..=CELL_FIND_CAP).contains(limit) {
                    return Err(Error::input("cell find pattern/limit out of bounds"));
                }
                let src = match source {
                    Some(s) => source_index(*s)? as i64,
                    None => -1,
                };
                let mut c = format!("cell_find seq={sequence} src={src} limit={limit}");
                if !pattern.is_empty() {
                    c.push_str(" pat_hex=");
                    for b in pattern.bytes() {
                        write!(c, "{b:02x}").unwrap();
                    }
                }
                c
            }
            Self::Bbox { source, cell, root } => {
                let mut c = format!(
                    "cell_bbox seq={sequence} src={} cell={cell}",
                    source_index(*source)?
                );
                if let Some(root) = root {
                    write!(c, " root={root}").unwrap();
                }
                c
            }
            Self::Insts {
                source,
                cell,
                view,
                cap,
                root,
            } => {
                let [x0, y0, x1, y1] = *view;
                if !view.iter().all(|v| v.is_finite()) || x0 >= x1 || y0 >= y1 {
                    return Err(Error::input("invalid cell instance view"));
                }
                if !(1..=CELL_INSTS_CAP).contains(cap) {
                    return Err(Error::input("cell instance cap out of bounds"));
                }
                let mut c = format!(
                    "cell_insts seq={sequence} src={} cell={cell} view={x0},{y0},{x1},{y1} cap={cap}",
                    source_index(*source)?
                );
                if let Some(root) = root {
                    write!(c, " root={root}").unwrap();
                }
                c
            }
        };
        if command.len() > MAX_LINE_BYTES {
            return Err(Error::input("cell command exceeds limit"));
        }
        command.shrink_to_fit();
        Ok(command)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellSource {
    pub source: usize,
    pub placements: u64,
    /// The source's path as the daemon knows it (a deck's spec entry).
    pub path: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CellChild {
    pub cell: u32,
    pub members: u64,
    pub leaf: bool,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CellMatch {
    pub source: usize,
    pub cell: u32,
    pub insts: u64,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq)]
pub enum CellReply {
    Sources(Vec<CellSource>),
    Children {
        source: usize,
        cell: u32,
        name: String,
        /// Instances under the top (or the root the daemon walked from).
        insts: u64,
        height: u64,
        /// The source's DBU in micrometres (a deck: the deck unit).
        unit: f64,
        /// Recursive extent; None = the cell holds no shapes.
        bbox: Option<[f64; 4]>,
        /// Distinct children; `children` holds at most CELLS_CHILD_CAP of them.
        total: u64,
        children: Vec<CellChild>,
    },
    Find {
        total: u64,
        matches: Vec<CellMatch>,
    },
    Bbox {
        source: usize,
        cell: u32,
        insts: u64,
        /// The extent of the top-level placements holding the cell, not the
        /// cell's own instances.
        approx: bool,
        bbox: Option<[f64; 4]>,
    },
    Insts {
        source: usize,
        cell: u32,
        more: bool,
        visited: u64,
        boxes: Vec<[f64; 4]>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellFailureCode {
    /// No hierarchy summary (design.ovh): `floe-index hier <cache>` builds it.
    NoHier,
    /// A newer `cell_insts` was queued behind this one.
    Superseded,
    /// The daemon has no open cache.
    State,
    /// The query itself failed (an unknown cell/source, a read error).
    Query,
    /// The answer exceeded the reply line limit; the client dropped it.
    Oversize,
}
impl CellFailureCode {
    pub fn wire(self) -> &'static str {
        match self {
            Self::NoHier => "nohier",
            Self::Superseded => "superseded",
            Self::State => "state",
            Self::Query => "query",
            Self::Oversize => "oversize",
        }
    }
    fn parse(value: &str) -> Result<Self> {
        match value {
            "nohier" => Ok(Self::NoHier),
            "superseded" => Ok(Self::Superseded),
            "state" => Ok(Self::State),
            "query" => Ok(Self::Query),
            _ => Err(Error::protocol("invalid cell failure code")),
        }
    }
}
/// A daemon-side refusal: the query was well formed, the answer is "no".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellFailure {
    pub code: CellFailureCode,
    /// Native diagnostic; a transport maps the code, not this text.
    pub message: String,
}
pub type CellOutcome = std::result::Result<CellReply, CellFailure>;

pub(crate) fn is_cell_kind(kind: &str) -> bool {
    matches!(
        kind,
        "cell_sources" | "cells" | "cell_find" | "cell_bbox" | "cell_insts"
    )
}

fn float(s: &str) -> Result<f64> {
    let v: f64 = s
        .parse()
        .map_err(|_| Error::protocol("invalid cell coordinate"))?;
    if !v.is_finite() {
        return Err(Error::protocol("invalid cell coordinate"));
    }
    Ok(v)
}
fn f64_box(s: &str) -> Result<[f64; 4]> {
    let parts = s.split(',').map(float).collect::<Result<Vec<_>>>()?;
    let b: [f64; 4] = parts
        .try_into()
        .map_err(|_| Error::protocol("invalid cell bbox"))?;
    if b[0] > b[2] || b[1] > b[3] {
        return Err(Error::protocol("invalid cell bbox"));
    }
    Ok(b)
}
fn optional_box(s: &str) -> Result<Option<[f64; 4]>> {
    if s == "-" {
        Ok(None)
    } else {
        f64_box(s).map(Some)
    }
}
fn index(s: &str) -> Result<usize> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::protocol("invalid cell index"));
    }
    let n: u64 = s
        .parse()
        .map_err(|_| Error::protocol("invalid cell index"))?;
    if n > i64::MAX as u64 || n.to_string() != s {
        return Err(Error::protocol("invalid cell index"));
    }
    usize::try_from(n).map_err(|_| Error::protocol("invalid cell index"))
}
fn cell_index(s: &str) -> Result<u32> {
    u32::try_from(index(s)?).map_err(|_| Error::protocol("invalid cell index"))
}
fn counter(s: &str) -> Result<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::protocol("invalid cell counter"));
    }
    s.parse()
        .map_err(|_| Error::protocol("invalid cell counter"))
}
/// `-` is the empty list; the count field must agree with the rows.
fn rows(value: &str, separator: char, n: u64, cap: usize) -> Result<Vec<&str>> {
    let out: Vec<&str> = if value == "-" {
        Vec::new()
    } else {
        value.split(separator).collect()
    };
    if out.len() > cap || out.len() as u64 != n {
        return Err(Error::protocol("cell row count mismatch"));
    }
    Ok(out)
}
fn columns<const N: usize>(row: &str) -> Result<[&str; N]> {
    let parts: Vec<&str> = row.splitn(N, ':').collect();
    parts
        .try_into()
        .map_err(|_| Error::protocol("invalid cell row"))
}

/// A well-formed answer for `request`, which the daemon refused or granted.
pub(crate) fn parse_reply(kind: &str, f: &Fields, request: &CellRequest) -> Result<CellOutcome> {
    if kind != request.kind() {
        return Err(Error::protocol("cell reply kind mismatch"));
    }
    if !f.flag("found")? {
        return Ok(Err(CellFailure {
            code: CellFailureCode::parse(f.required("code")?)?,
            message: unhex(f.required("err_hex")?)?,
        }));
    }
    let source_field = |expected: usize| -> Result<usize> {
        let src = index(f.required("src")?)?;
        if src != expected {
            return Err(Error::protocol("cell reply source mismatch"));
        }
        Ok(src)
    };
    let reply = match request {
        CellRequest::Sources => {
            let n = counter(f.required("n")?)?;
            let mut sources = Vec::new();
            for (i, row) in rows(f.required("sources")?, ',', n, CELL_SOURCES_CAP)?
                .into_iter()
                .enumerate()
            {
                let [src, placements, path_hex] = columns::<3>(row)?;
                if index(src)? != i {
                    return Err(Error::protocol("cell sources out of order"));
                }
                sources.push(CellSource {
                    source: i,
                    placements: counter(placements)?,
                    path: unhex(path_hex)?,
                });
            }
            CellReply::Sources(sources)
        }
        CellRequest::Children { source, cell } => {
            let src = source_field(*source)?;
            let answered = cell_index(f.required("cell")?)?;
            if cell.is_some_and(|c| c != answered) {
                return Err(Error::protocol("cell reply cell mismatch"));
            }
            let unit = float(f.required("unit")?)?;
            if unit <= 0. {
                return Err(Error::protocol("invalid cell unit"));
            }
            let n = counter(f.required("n")?)?;
            let total = counter(f.required("total")?)?;
            if n > total {
                return Err(Error::protocol("cell child count exceeds total"));
            }
            let mut children = Vec::new();
            for row in rows(f.required("children")?, ',', n, CELLS_CHILD_CAP)? {
                let [ci, members, leaf, name_hex] = columns::<4>(row)?;
                children.push(CellChild {
                    cell: cell_index(ci)?,
                    members: counter(members)?,
                    leaf: match leaf {
                        "0" => false,
                        "1" => true,
                        _ => return Err(Error::protocol("invalid cell leaf flag")),
                    },
                    name: unhex(name_hex)?,
                });
            }
            CellReply::Children {
                source: src,
                cell: answered,
                name: unhex(f.required("name_hex")?)?,
                insts: counter(f.required("insts")?)?,
                height: counter(f.required("height")?)?,
                unit,
                bbox: optional_box(f.required("bbox")?)?,
                total,
                children,
            }
        }
        CellRequest::Find { source, limit, .. } => {
            let src = f.required("src")?;
            match source {
                Some(s) => {
                    if index(src)? != *s {
                        return Err(Error::protocol("cell reply source mismatch"));
                    }
                }
                None => {
                    if src != "-1" {
                        return Err(Error::protocol("cell reply source mismatch"));
                    }
                }
            }
            let n = counter(f.required("n")?)?;
            let total = counter(f.required("total")?)?;
            if n > total {
                return Err(Error::protocol("cell match count exceeds total"));
            }
            let mut matches = Vec::new();
            for row in rows(f.required("matches")?, ',', n, *limit)? {
                let [src, ci, insts, name_hex] = columns::<4>(row)?;
                matches.push(CellMatch {
                    source: index(src)?,
                    cell: cell_index(ci)?,
                    insts: counter(insts)?,
                    name: unhex(name_hex)?,
                });
            }
            CellReply::Find { total, matches }
        }
        CellRequest::Bbox { source, cell, .. } => {
            let src = source_field(*source)?;
            if cell_index(f.required("cell")?)? != *cell {
                return Err(Error::protocol("cell reply cell mismatch"));
            }
            CellReply::Bbox {
                source: src,
                cell: *cell,
                insts: counter(f.required("insts")?)?,
                approx: f.flag("approx")?,
                bbox: optional_box(f.required("bbox")?)?,
            }
        }
        CellRequest::Insts {
            source, cell, cap, ..
        } => {
            let src = source_field(*source)?;
            if cell_index(f.required("cell")?)? != *cell {
                return Err(Error::protocol("cell reply cell mismatch"));
            }
            let n = counter(f.required("n")?)?;
            let boxes = rows(f.required("boxes")?, ';', n, *cap)?
                .into_iter()
                .map(f64_box)
                .collect::<Result<Vec<_>>>()?;
            CellReply::Insts {
                source: src,
                cell: *cell,
                more: f.flag("more")?,
                visited: counter(f.required("visited")?)?,
                boxes,
            }
        }
    };
    Ok(Ok(reply))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fields(text: &str) -> (String, Fields) {
        let line = crate::protocol::parse_line(text.as_bytes()).unwrap();
        (line.kind, line.fields)
    }
    fn reply(text: &str, request: &CellRequest) -> Result<CellOutcome> {
        let (kind, f) = fields(text);
        parse_reply(&kind, &f, request)
    }
    #[test]
    fn commands_are_canonical_hex_encoded_and_bounded() {
        assert_eq!(
            CellRequest::Sources.command(3).unwrap(),
            "cell_sources seq=3"
        );
        assert_eq!(
            CellRequest::Children {
                source: 0,
                cell: None
            }
            .command(4)
            .unwrap(),
            "cells seq=4 src=0"
        );
        assert_eq!(
            CellRequest::Children {
                source: 2,
                cell: Some(17)
            }
            .command(4)
            .unwrap(),
            "cells seq=4 src=2 cell=17"
        );
        assert_eq!(
            CellRequest::Find {
                source: None,
                pattern: "*inv?".into(),
                limit: 2000
            }
            .command(5)
            .unwrap(),
            "cell_find seq=5 src=-1 limit=2000 pat_hex=2a696e763f"
        );
        assert_eq!(
            CellRequest::Find {
                source: Some(1),
                pattern: String::new(),
                limit: 1
            }
            .command(5)
            .unwrap(),
            "cell_find seq=5 src=1 limit=1"
        );
        assert_eq!(
            CellRequest::Bbox {
                source: 0,
                cell: 9,
                root: Some(17)
            }
            .command(6)
            .unwrap(),
            "cell_bbox seq=6 src=0 cell=9 root=17"
        );
        assert_eq!(
            CellRequest::Insts {
                source: 0,
                cell: 9,
                view: [0., -5., 10.5, 20.],
                cap: 4096,
                root: None
            }
            .command(7)
            .unwrap(),
            "cell_insts seq=7 src=0 cell=9 view=0,-5,10.5,20 cap=4096"
        );
        for bad in [
            CellRequest::Find {
                source: None,
                pattern: "x".repeat(CELL_PATTERN_BYTES + 1),
                limit: 1,
            },
            CellRequest::Find {
                source: None,
                pattern: String::new(),
                limit: 0,
            },
            CellRequest::Find {
                source: None,
                pattern: String::new(),
                limit: CELL_FIND_CAP + 1,
            },
            CellRequest::Insts {
                source: 0,
                cell: 1,
                view: [0., 0., 0., 1.],
                cap: 1,
                root: None,
            },
            CellRequest::Insts {
                source: 0,
                cell: 1,
                view: [0., 0., f64::NAN, 1.],
                cap: 1,
                root: None,
            },
            CellRequest::Insts {
                source: 0,
                cell: 1,
                view: [0., 0., 1., 1.],
                cap: CELL_INSTS_CAP + 1,
                root: None,
            },
        ] {
            assert!(bad.command(1).is_err(), "{bad:?}");
        }
        assert!(CellRequest::Sources.command(0).is_err());
        assert!(CellRequest::Sources.command(i64::MAX as u64 + 1).is_err());
    }
    #[test]
    fn sources_and_children_decode_hex_names_and_empty_lists() {
        let r = reply(
            "cell_sources seq=1 found=1 n=2 sources=0:1:2f612f746f702e6f6173,1:3:2f622e6f6173",
            &CellRequest::Sources,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            r,
            CellReply::Sources(vec![
                CellSource {
                    source: 0,
                    placements: 1,
                    path: "/a/top.oas".into()
                },
                CellSource {
                    source: 1,
                    placements: 3,
                    path: "/b.oas".into()
                }
            ])
        );
        assert_eq!(
            reply(
                "cell_sources seq=1 found=1 n=0 sources=-",
                &CellRequest::Sources
            )
            .unwrap()
            .unwrap(),
            CellReply::Sources(vec![])
        );
        let top = CellRequest::Children {
            source: 0,
            cell: None,
        };
        let r = reply(
            "cells seq=2 src=0 found=1 cell=17 name_hex=544f5020ed959ceab880 insts=12 height=3 unit=0.001 bbox=-10,-20.5,30,40 n=2 total=5 children=3:2:0:41,9:1:1:42",
            &top,
        )
        .unwrap()
        .unwrap();
        let CellReply::Children {
            cell,
            name,
            insts,
            height,
            unit,
            bbox,
            total,
            children,
            ..
        } = r
        else {
            panic!("children expected")
        };
        assert_eq!(
            (cell, name.as_str(), insts, height),
            (17, "TOP 한글", 12, 3)
        );
        assert_eq!(unit, 0.001);
        assert_eq!(bbox, Some([-10., -20.5, 30., 40.]));
        assert_eq!(total, 5);
        assert_eq!(
            children,
            vec![
                CellChild {
                    cell: 3,
                    members: 2,
                    leaf: false,
                    name: "A".into()
                },
                CellChild {
                    cell: 9,
                    members: 1,
                    leaf: true,
                    name: "B".into()
                }
            ]
        );
        let leaf = reply(
            "cells seq=2 src=0 found=1 cell=9 name_hex=42 insts=1 height=0 unit=0.001 bbox=- n=0 total=0 children=-",
            &CellRequest::Children {
                source: 0,
                cell: Some(9),
            },
        )
        .unwrap()
        .unwrap();
        assert!(matches!(
            leaf,
            CellReply::Children {
                bbox: None,
                total: 0,
                ref children,
                ..
            } if children.is_empty()
        ));
        for bad in [
            // count disagrees with the rows
            "cells seq=2 src=0 found=1 cell=17 name_hex=41 insts=1 height=1 unit=0.001 bbox=- n=1 total=1 children=-",
            // rows above the distinct total
            "cells seq=2 src=0 found=1 cell=17 name_hex=41 insts=1 height=1 unit=0.001 bbox=- n=1 total=0 children=3:2:0:41",
            // another source answered
            "cells seq=2 src=1 found=1 cell=17 name_hex=41 insts=1 height=1 unit=0.001 bbox=- n=0 total=0 children=-",
            // bad unit / bbox / leaf flag / hex
            "cells seq=2 src=0 found=1 cell=17 name_hex=41 insts=1 height=1 unit=0 bbox=- n=0 total=0 children=-",
            "cells seq=2 src=0 found=1 cell=17 name_hex=41 insts=1 height=1 unit=0.001 bbox=1,0,0,0 n=0 total=0 children=-",
            "cells seq=2 src=0 found=1 cell=17 name_hex=41 insts=1 height=1 unit=0.001 bbox=- n=1 total=1 children=3:2:2:41",
            "cells seq=2 src=0 found=1 cell=17 name_hex=zz insts=1 height=1 unit=0.001 bbox=- n=0 total=0 children=-",
            // the wrong kind
            "cell_bbox seq=2 src=0 cell=17 found=1 insts=1 approx=0 bbox=-",
        ] {
            assert!(reply(bad, &top).is_err(), "{bad}");
        }
        // a requested cell must be the answered cell
        assert!(reply(
            "cells seq=2 src=0 found=1 cell=17 name_hex=41 insts=1 height=1 unit=0.001 bbox=- n=0 total=0 children=-",
            &CellRequest::Children {
                source: 0,
                cell: Some(16)
            }
        )
        .is_err());
    }
    #[test]
    fn find_bbox_and_insts_are_bounded_by_the_request() {
        let find = CellRequest::Find {
            source: None,
            pattern: "a".into(),
            limit: 2,
        };
        let r = reply(
            "cell_find seq=3 src=-1 found=1 total=7 n=2 matches=0:4:10:41,1:5:0:6162",
            &find,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            r,
            CellReply::Find {
                total: 7,
                matches: vec![
                    CellMatch {
                        source: 0,
                        cell: 4,
                        insts: 10,
                        name: "A".into()
                    },
                    CellMatch {
                        source: 1,
                        cell: 5,
                        insts: 0,
                        name: "ab".into()
                    }
                ]
            }
        );
        assert!(matches!(
            reply("cell_find seq=3 src=-1 found=1 total=0 n=0 matches=-", &find)
                .unwrap()
                .unwrap(),
            CellReply::Find { total: 0, ref matches } if matches.is_empty()
        ));
        // more rows than the limit asked for, or a source we did not ask
        assert!(reply(
            "cell_find seq=3 src=-1 found=1 total=7 n=3 matches=0:4:10:41,1:5:0:42,1:6:0:43",
            &find
        )
        .is_err());
        assert!(reply("cell_find seq=3 src=0 found=1 total=0 n=0 matches=-", &find).is_err());
        let bbox = CellRequest::Bbox {
            source: 0,
            cell: 9,
            root: Some(17),
        };
        assert_eq!(
            reply(
                "cell_bbox seq=4 src=0 cell=9 found=1 insts=3 approx=1 bbox=0,0,10,20",
                &bbox
            )
            .unwrap()
            .unwrap(),
            CellReply::Bbox {
                source: 0,
                cell: 9,
                insts: 3,
                approx: true,
                bbox: Some([0., 0., 10., 20.])
            }
        );
        assert!(matches!(
            reply(
                "cell_bbox seq=4 src=0 cell=9 found=1 insts=0 approx=0 bbox=-",
                &bbox
            )
            .unwrap()
            .unwrap(),
            CellReply::Bbox { bbox: None, .. }
        ));
        assert!(reply(
            "cell_bbox seq=4 src=0 cell=8 found=1 insts=0 approx=0 bbox=-",
            &bbox
        )
        .is_err());
        let insts = CellRequest::Insts {
            source: 0,
            cell: 9,
            view: [0., 0., 100., 100.],
            cap: 2,
            root: None,
        };
        let r = reply(
            "cell_insts seq=5 src=0 cell=9 found=1 n=2 more=1 visited=77 boxes=0,0,1,1;2.5,3,4,5",
            &insts,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            r,
            CellReply::Insts {
                source: 0,
                cell: 9,
                more: true,
                visited: 77,
                boxes: vec![[0., 0., 1., 1.], [2.5, 3., 4., 5.]]
            }
        );
        assert!(matches!(
            reply(
                "cell_insts seq=5 src=0 cell=9 found=1 n=0 more=0 visited=0 boxes=-",
                &insts
            )
            .unwrap()
            .unwrap(),
            CellReply::Insts { ref boxes, more: false, .. } if boxes.is_empty()
        ));
        assert!(reply(
            "cell_insts seq=5 src=0 cell=9 found=1 n=3 more=0 visited=0 boxes=0,0,1,1;0,0,1,1;0,0,1,1",
            &insts
        )
        .is_err());
        assert!(reply(
            "cell_insts seq=5 src=0 cell=9 found=1 n=1 more=0 visited=0 boxes=0,0,1",
            &insts
        )
        .is_err());
    }
    #[test]
    fn refusals_carry_their_code_and_decoded_message() {
        for (code, expected) in [
            ("nohier", CellFailureCode::NoHier),
            ("superseded", CellFailureCode::Superseded),
            ("state", CellFailureCode::State),
            ("query", CellFailureCode::Query),
        ] {
            let text = format!("cells seq=4 found=0 code={code} err_hex=6e6f2073756d6d617279");
            let failure = reply(
                &text,
                &CellRequest::Children {
                    source: 0,
                    cell: None,
                },
            )
            .unwrap()
            .unwrap_err();
            assert_eq!(failure.code, expected);
            assert_eq!(failure.message, "no summary");
            assert_eq!(failure.code.wire(), code);
        }
        for bad in [
            "cells seq=4 found=0 code=lost err_hex=41",
            "cells seq=4 found=0 err_hex=41",
            "cells seq=4 found=0 code=query",
            "cells seq=4 found=2 code=query err_hex=41",
        ] {
            assert!(
                reply(
                    bad,
                    &CellRequest::Children {
                        source: 0,
                        cell: None
                    }
                )
                .is_err(),
                "{bad}"
            );
        }
    }
}
