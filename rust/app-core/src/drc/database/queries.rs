//! Explicit geometry units, retaining the existing integer/cache query path.
use super::*;
use crate::drc::{
    filters::Filtering, pack::Input, CdSegment, InfoHit, InfoPage, ListPage, ListRequest,
    RecordInfo, StepPage, StepRequest, MAGIC, RECORD_POINTS,
};
use std::{collections::BTreeSet, os::unix::fs::FileExt};
pub type ReadInfo = RecordInfo<[f64; 4]>;
pub type ReadInfoHit = InfoHit<[f64; 4]>;
pub struct ReadPointPage {
    pub record: ReadInfo,
    pub start: usize,
    pub points: ReadPoints,
    pub next: Option<usize>,
}
fn record(p: &Pack, r: RecordInfo) -> Result<ReadInfo> {
    Ok(RecordInfo {
        kind: r.kind,
        number: r.number,
        points: r.points,
        bbox: p.bbox_um(r.bbox)?,
    })
}
fn hits(p: &Pack, rows: Vec<InfoHit>) -> Result<Vec<ReadInfoHit>> {
    rows.into_iter()
        .map(|h| {
            Ok(InfoHit {
                check: h.check,
                local: h.local,
                status: h.status,
                record: record(p, h.record)?,
            })
        })
        .collect()
}
fn step(p: &Pack, v: StepPage) -> Result<StepPage<[f64; 4]>> {
    Ok(StepPage {
        hit: hits(p, v.hit.into_iter().collect())?.into_iter().next(),
        next: v.next,
        scanned: v.scanned,
    })
}
impl Database {
    /// Only explicit registered paths, no adjacent-file/reviewer discovery.
    /// The trusted CLI's separate selection policy lives in open_current.
    pub fn open_explicit(path: &Path, waives: Option<&Path>, stop: &AtomicUsize) -> Result<Self> {
        check_cancelled(stop)?;
        let input = Input::open(path)?;
        let mut magic = [0; 8];
        let packed = input.len() >= 8 && {
            input.file.read_exact_at(&mut magic, 0)?;
            &magic == MAGIC
        };
        input.unchanged()?;
        if packed {
            let mut p = Pack::open(path, stop)?;
            if let Some(w) = waives {
                p.attach_waives(w)?;
            }
            p.unchanged()?;
            Ok(Self::packed(p))
        } else {
            if waives.is_some() {
                return Err(Error::input(
                    "waive sidecars require an explicitly registered ICE pack",
                ));
            }
            Ok(Self::ascii(Ascii::open(path, stop)?, None))
        }
    }
    pub fn format(&self) -> &'static str {
        match &self.backend {
            Backend::Pack(_) => "ice",
            Backend::Ascii(_) => "ascii",
        }
    }
    pub fn truncated_records(&self) -> u64 {
        match &self.backend {
            Backend::Pack(_) => 0,
            Backend::Ascii(p) => p.truncated_records,
        }
    }
    pub fn names(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match &self.backend {
            Backend::Pack(p) => Box::new(p.checks.iter().map(|c| c.name.as_str())),
            Backend::Ascii(p) => Box::new(p.checks.iter().map(|c| c.name.as_str())),
        }
    }
    pub fn bounds(&self, ci: usize) -> Result<Option<[f64; 4]>> {
        match &self.backend {
            Backend::Pack(p) => Filtering::bounds(p.as_ref(), ci),
            Backend::Ascii(p) => Filtering::bounds(p.as_ref(), ci),
        }
    }
    pub fn status(&self, ci: usize, ei: u64) -> Result<u8> {
        match &self.backend {
            Backend::Pack(p) => p.status(ci, ei),
            Backend::Ascii(p) => {
                p.unchanged()?;
                if ei >= self.check(ci)?.count {
                    return Err(Error::input("DRC error index"));
                }
                Ok(0)
            }
        }
    }
    pub fn error_info(&mut self, ci: usize, ei: u64, stop: &AtomicUsize) -> Result<ReadInfo> {
        match &mut self.backend {
            Backend::Pack(p) => {
                let r = p.error_info(ci, ei, stop)?;
                record(p, r)
            }
            Backend::Ascii(p) => p.error_info(ci, ei, stop),
        }
    }
    pub fn selection_candidates(
        &mut self,
        ci: usize,
        ids: &[u64],
        b: Option<[f64; 4]>,
        w: Option<bool>,
        stop: &AtomicUsize,
    ) -> Result<Vec<ReadInfoHit>> {
        match &mut self.backend {
            Backend::Pack(p) => {
                let v = p.selection_candidates(ci, ids, b, w, stop)?;
                hits(p, v)
            }
            Backend::Ascii(p) => p.selection_candidates(ci, ids, b, w, stop),
        }
    }
    pub fn query(
        &mut self,
        b: [f64; 4],
        cs: Option<&BTreeSet<usize>>,
        w: Option<bool>,
        cursor: Cursor,
        limit: usize,
        stop: &AtomicUsize,
    ) -> Result<InfoPage<[f64; 4]>> {
        match &mut self.backend {
            Backend::Pack(p) => {
                // Preserve the existing wire's page boundaries/point budget.
                let v = p.query(b, cs, w, cursor, limit, stop)?;
                let infos = v
                    .hits
                    .iter()
                    .map(|h| InfoHit {
                        check: h.check,
                        local: h.local,
                        status: h.status,
                        record: RecordInfo::from(&h.violation),
                    })
                    .collect();
                Ok(InfoPage {
                    hits: hits(p, infos)?,
                    next: v.next,
                    scanned: v.scanned,
                })
            }
            Backend::Ascii(p) => p.query_info(b, cs, w, cursor, limit, stop),
        }
    }
    pub fn filtered_errors(
        &mut self,
        r: ListRequest<'_>,
        stop: &AtomicUsize,
    ) -> Result<ListPage<[f64; 4]>> {
        match &mut self.backend {
            Backend::Pack(p) => {
                let v = p.filtered_errors(r, stop)?;
                Ok(ListPage {
                    hits: hits(p, v.hits)?,
                    next: v.next,
                    scanned: v.scanned,
                })
            }
            Backend::Ascii(p) => p.filtered_errors(r, stop),
        }
    }
    pub fn step(&mut self, r: StepRequest, stop: &AtomicUsize) -> Result<StepPage<[f64; 4]>> {
        match &mut self.backend {
            Backend::Pack(p) => {
                let v = p.step(r, stop)?;
                step(p, v)
            }
            Backend::Ascii(p) => p.step(r, stop),
        }
    }
    pub fn filtered_step(
        &mut self,
        r: StepRequest,
        ids: Option<&BTreeSet<u64>>,
        stop: &AtomicUsize,
    ) -> Result<StepPage<[f64; 4]>> {
        match &mut self.backend {
            Backend::Pack(p) => {
                let v = p.filtered_step(r, ids, stop)?;
                step(p, v)
            }
            Backend::Ascii(p) => p.filtered_step(r, ids, stop),
        }
    }
    pub fn error_points(
        &mut self,
        ci: usize,
        ei: u64,
        start: usize,
        limit: usize,
        stop: &AtomicUsize,
    ) -> Result<ReadPointPage> {
        if !(1..=2048).contains(&limit) || start > RECORD_POINTS {
            return Err(Error::input("invalid point page"));
        }
        match &mut self.backend {
            Backend::Pack(p) => {
                let v = p.error_points(ci, ei, start, limit, stop)?;
                Ok(ReadPointPage {
                    record: record(p, v.record)?,
                    start,
                    points: ReadPoints::Dbu(v.points, p.precision),
                    next: v.next,
                })
            }
            Backend::Ascii(p) => {
                let record = p.error_info(ci, ei, stop)?;
                if start > record.points {
                    return Err(Error::input("point cursor"));
                }
                let v = p.cached_error(ci, ei, stop)?;
                let end = (start + limit).min(record.points);
                let page = ReadPointPage {
                    record,
                    start,
                    points: ReadPoints::Um(v.points_um[start..end].to_vec()),
                    next: (end < record.points).then_some(end),
                };
                p.unchanged()?;
                check_cancelled(stop)?;
                Ok(page)
            }
        }
    }
    pub fn constraint_comparison<'a>(
        &mut self,
        ci: usize,
        ei: u64,
        rule: Option<&'a Rule>,
        stop: &AtomicUsize,
    ) -> Result<(ReadInfo, Option<Comparison<'a>>)> {
        match &mut self.backend {
            Backend::Pack(p) => {
                let (r, c) = p.constraint_comparison(ci, ei, rule, stop)?;
                Ok((record(p, r)?, c))
            }
            Backend::Ascii(p) => {
                let r = p.error_info(ci, ei, stop)?;
                let c = if let Some(rule) = rule {
                    let v = p.cached_error(ci, ei, stop)?;
                    rule.compare_um(v.kind, &v.points_um, stop)?
                } else {
                    None
                };
                p.unchanged()?;
                check_cancelled(stop)?;
                Ok((r, c))
            }
        }
    }
    pub fn measurements(
        &mut self,
        ci: usize,
        ei: u64,
        stop: &AtomicUsize,
    ) -> Result<(ReadInfo, Vec<CdSegment>)> {
        let info = self.error_info(ci, ei, stop)?;
        if !matches!((info.kind, info.points), ('p', 4) | ('e', 2 | 4)) {
            return Ok((info, Vec::new()));
        }
        let page = self.error_points(ci, ei, 0, 4, stop)?;
        let segments = match page.points {
            ReadPoints::Dbu(p, precision) => crate::drc::cd_segments(info.kind, &p, precision)?,
            ReadPoints::Um(p) => crate::drc::cd_segments_um(info.kind, &p, stop)?,
        };
        Ok((info, segments))
    }
}
