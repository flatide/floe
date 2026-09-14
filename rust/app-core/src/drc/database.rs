//! CLI-neutral read facade. ASCII retains fractional micrometre coordinates;
//! packed input keeps the existing i64 path and checked integer measurements.
use super::{Ascii, Cursor, Pack, BLOCK_POINTS, PAGE_ITEMS};
use crate::{
    check_cancelled,
    svrf::{Comparison, Rule},
    Error, Result,
};
use std::{borrow::Cow, path::Path, sync::atomic::AtomicUsize};

enum Backend {
    Pack(Box<Pack>),
    Ascii(Box<Ascii>),
}
pub struct Database {
    backend: Backend,
    pub warnings: Vec<String>,
}
pub struct ReadCheck<'a> {
    pub name: &'a str,
    pub desc: &'a str,
    pub count: u64,
    pub declared: Cow<'a, str>,
    pub original: Cow<'a, str>,
}
pub enum ReadPoints {
    Dbu(Vec<[i64; 2]>, f64),
    Um(Vec<[f64; 2]>),
}
pub struct ReadViolation {
    pub kind: char,
    pub number: u64,
    pub bbox_um: [f64; 4],
    points: ReadPoints,
}
pub struct ReadHit {
    pub local: u64,
    pub status: u8,
    pub violation: ReadViolation,
}
pub struct ReadPage {
    pub hits: Vec<ReadHit>,
    pub next: Option<Cursor>,
}
impl ReadViolation {
    pub(crate) fn points_um(&self, stop: &AtomicUsize) -> Result<Vec<[f64; 2]>> {
        let n = match &self.points {
            ReadPoints::Dbu(p, _) => p.len(),
            ReadPoints::Um(p) => p.len(),
        };
        let mut points = Vec::with_capacity(n);
        for i in 0..n {
            if i % 1024 == 0 {
                check_cancelled(stop)?;
            }
            let point = match &self.points {
                ReadPoints::Dbu(p, precision) => p[i].map(|v| v as f64 / precision),
                ReadPoints::Um(p) => p[i],
            };
            if !point.iter().all(|n| n.is_finite()) {
                return Err(Error::input("unrepresentable DRC capture point"));
            }
            points.push(point);
        }
        Ok(points)
    }
    pub fn comparison<'a>(
        &self,
        rule: &'a Rule,
        stop: &AtomicUsize,
    ) -> Result<Option<Comparison<'a>>> {
        match &self.points {
            ReadPoints::Dbu(p, precision) => rule.compare(self.kind, p, *precision, stop),
            ReadPoints::Um(p) => rule.compare_um(self.kind, p, stop),
        }
    }
}
impl Database {
    pub fn has_waives(&self) -> bool {
        matches!(&self.backend, Backend::Pack(p) if p.has_waives())
    }
    pub(super) fn validate_waive_identity(&self, identity: &super::review::Identity) -> Result<()> {
        match &self.backend {
            Backend::Pack(p) if p.geometry_binding()? == identity.0 => Ok(()),
            Backend::Pack(_) => Err(Error::new(
                crate::ErrorKind::Cache,
                "waive snapshot belongs to another pack",
            )),
            Backend::Ascii(_) => Err(Error::input("waive refresh requires a registered pack")),
        }
    }
    pub(super) fn install_waives(
        &mut self,
        identity: &super::review::Identity,
        input: Option<super::pack::Input>,
        counts: Vec<u32>,
        stop: &AtomicUsize,
    ) -> Result<()> {
        match &mut self.backend {
            Backend::Pack(p) => p.install_waives(identity, input, counts, stop),
            Backend::Ascii(_) => Err(Error::input("waive refresh requires a registered pack")),
        }
    }
    /// Convert canonical check/local references only after binding this reader
    /// to the review store. Displayed global numbers are one-based, store gids
    /// zero-based: never use the UI's displayed number as a write index.
    pub fn review_targets(
        &self,
        identity: &super::review::Identity,
        refs: &[(usize, u64)],
        stop: &AtomicUsize,
    ) -> Result<Vec<u64>> {
        check_cancelled(stop)?;
        if refs.is_empty() || refs.len() > super::review::EDIT_ITEMS {
            return Err(Error::input("review requires 1..5000 error references"));
        }
        self.validate_review_identity(identity)?;
        let Backend::Pack(p) = &self.backend else {
            unreachable!()
        };
        let mut ids = Vec::with_capacity(refs.len());
        for &(ci, local) in refs {
            check_cancelled(stop)?;
            let c = p
                .checks
                .get(ci)
                .filter(|c| local < c.count)
                .ok_or_else(|| Error::input("review error reference out of range"))?;
            ids.push(
                c.start
                    .checked_add(local)
                    .ok_or_else(|| Error::input("review gid overflow"))?,
            );
        }
        p.unchanged_at()?;
        Ok(ids)
    }
    /// Bind a prepared review to THIS already-open reader, not merely another
    /// pack with matching legacy headers. Call on the read actor, off-reactor.
    /// Path replacement/touch is conservatively rejected until reopen.
    pub fn validate_review_identity(&self, identity: &super::review::Identity) -> Result<()> {
        match &self.backend {
            Backend::Pack(p) if p.review_binding()? == identity.0 => Ok(()),
            Backend::Pack(_) => Err(Error::new(
                crate::ErrorKind::Cache,
                "review and reader refer to different DRC packs",
            )),
            Backend::Ascii(_) => Err(Error::input("DRC review requires a registered pack")),
        }
    }
    pub(super) fn packed(pack: Pack) -> Self {
        Self {
            backend: Backend::Pack(Box::new(pack)),
            warnings: Vec::new(),
        }
    }
    pub(super) fn ascii(ascii: Ascii, warning: Option<String>) -> Self {
        let mut warnings: Vec<_> = warning.into_iter().collect();
        if ascii.truncated_records > 0 {
            warnings.push(format!(
                "{} truncated ASCII DRC record(s); the readable prefix matches legacy parsing",
                ascii.truncated_records
            ));
        }
        Self {
            backend: Backend::Ascii(Box::new(ascii)),
            warnings,
        }
    }
    pub fn path(&self) -> &Path {
        match &self.backend {
            Backend::Pack(p) => &p.path,
            Backend::Ascii(p) => &p.path,
        }
    }
    pub fn cell(&self) -> &str {
        match &self.backend {
            Backend::Pack(p) => &p.cell,
            Backend::Ascii(p) => &p.cell,
        }
    }
    pub fn precision(&self) -> f64 {
        match &self.backend {
            Backend::Pack(p) => p.precision,
            Backend::Ascii(p) => p.precision,
        }
    }
    pub fn total(&self) -> u64 {
        match &self.backend {
            Backend::Pack(p) => p.total,
            Backend::Ascii(p) => p.total,
        }
    }
    pub fn check_count(&self) -> usize {
        match &self.backend {
            Backend::Pack(p) => p.checks.len(),
            Backend::Ascii(p) => p.checks.len(),
        }
    }
    pub fn check(&self, i: usize) -> Result<ReadCheck<'_>> {
        match &self.backend {
            Backend::Pack(p) => p.checks.get(i).map(|c| ReadCheck {
                name: &c.name,
                desc: &c.desc,
                count: c.count,
                declared: Cow::Owned(c.declared.to_string()),
                original: Cow::Owned(c.original.to_string()),
            }),
            Backend::Ascii(p) => p.checks.get(i).map(|c| ReadCheck {
                name: &c.name,
                desc: &c.desc,
                count: c.count,
                declared: Cow::Borrowed(&c.declared),
                original: Cow::Borrowed(&c.original),
            }),
        }
        .ok_or_else(|| Error::input("DRC check index out of range"))
    }
    pub fn waived_count(&self, check: usize) -> Result<u64> {
        self.check(check)?;
        match &self.backend {
            Backend::Pack(p) => p.waived_count(check),
            Backend::Ascii(_) => Ok(0),
        }
    }
    pub fn unchanged(&self) -> Result<()> {
        match &self.backend {
            Backend::Pack(p) => p.unchanged(),
            Backend::Ascii(p) => p.unchanged(),
        }
    }
    pub fn errors(
        &mut self,
        check: usize,
        start: u64,
        limit: usize,
        stop: &AtomicUsize,
    ) -> Result<ReadPage> {
        match &mut self.backend {
            Backend::Pack(p) => {
                let v = p.errors(check, start, limit, stop)?;
                let mut hits = Vec::with_capacity(v.hits.len());
                for h in v.hits {
                    let e = h.violation;
                    let bbox_um = p.bbox_um(e.bbox)?;
                    hits.push(ReadHit {
                        local: h.local,
                        status: h.status,
                        violation: ReadViolation {
                            kind: e.kind,
                            number: e.number,
                            bbox_um,
                            points: ReadPoints::Dbu(e.points, p.precision),
                        },
                    });
                }
                Ok(ReadPage { hits, next: v.next })
            }
            Backend::Ascii(p) => {
                p.unchanged()?;
                check_cancelled(stop)?;
                let count = p
                    .checks
                    .get(check)
                    .filter(|c| start <= c.count)
                    .ok_or_else(|| Error::input("invalid DRC error page"))?
                    .count;
                if !(1..=PAGE_ITEMS).contains(&limit) {
                    return Err(Error::input("invalid DRC page limit"));
                }
                let mut hits = Vec::new();
                let mut points = 0;
                let mut i = start;
                while i < count && hits.len() < limit {
                    let e = p.error(check, i, stop)?;
                    if points + e.points_um.len() > BLOCK_POINTS {
                        break;
                    }
                    points += e.points_um.len();
                    hits.push(ReadHit {
                        local: i,
                        status: 0,
                        violation: ReadViolation {
                            kind: e.kind,
                            number: e.number,
                            bbox_um: e.bbox_um,
                            points: ReadPoints::Um(e.points_um),
                        },
                    });
                    i += 1;
                }
                p.unchanged()?;
                Ok(ReadPage {
                    hits,
                    next: (i < count).then_some(Cursor { check, error: i }),
                })
            }
        }
    }
}
mod queries;
pub use queries::{ReadInfo, ReadInfoHit, ReadPointPage};

#[cfg(test)]
mod waive_tests;
