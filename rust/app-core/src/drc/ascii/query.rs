//! ASCII queries touch resident bbox/offset metadata, not coordinate records.
use super::*;
use crate::drc::{
    filters::Filtering, Cursor, InfoHit, InfoPage, RecordInfo, StepCursor, StepPage, StepRequest,
    PAGE_ITEMS, SCAN_ITEMS, SELECTION_INPUT,
};
use std::collections::BTreeSet;
fn valid(b: [f64; 4]) -> Result<()> {
    if !b.iter().all(|n| n.is_finite()) || b[0] > b[2] || b[1] > b[3] {
        Err(Error::input("invalid DRC query bounds"))
    } else {
        Ok(())
    }
}
fn intersects(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
}
impl Ascii {
    fn info(&self, check: usize, local: u64) -> Result<RecordInfo<[f64; 4]>> {
        let c = self
            .checks
            .get(check)
            .filter(|c| local < c.count)
            .ok_or_else(|| Error::input("ASCII DRC error index"))?;
        let i = c
            .start
            .checked_add(local)
            .ok_or_else(|| Error::input("ASCII record index"))?;
        let r = usize::try_from(i)
            .ok()
            .and_then(|i| self.records.get(i))
            .ok_or_else(|| Error::input("ASCII record index"))?;
        Ok(RecordInfo {
            kind: r.kind,
            number: i + 1,
            bbox: r.bbox_um,
            points: r.points,
        })
    }
    pub fn error_info(
        &self,
        ci: usize,
        ei: u64,
        stop: &AtomicUsize,
    ) -> Result<RecordInfo<[f64; 4]>> {
        check_cancelled(stop)?;
        self.unchanged()?;
        self.info(ci, ei)
    }
    pub(in crate::drc) fn cached_error(
        &mut self,
        ci: usize,
        ei: u64,
        stop: &AtomicUsize,
    ) -> Result<&AsciiViolation> {
        self.unchanged()?;
        check_cancelled(stop)?;
        if self
            .cached
            .as_ref()
            .is_none_or(|(c, e, _)| *c != ci || *e != ei)
        {
            // Drop before decode so two maximum-sized records are not retained.
            self.cached = None;
            self.cached = Some((ci, ei, self.error(ci, ei, stop)?));
        }
        Ok(&self.cached.as_ref().unwrap().2)
    }
    fn hit(&self, ci: usize, ei: u64) -> Result<InfoHit<[f64; 4]>> {
        Ok(InfoHit {
            check: ci,
            local: ei,
            status: 0,
            record: self.info(ci, ei)?,
        })
    }
    pub(super) fn step_limited(
        &self,
        r: StepRequest,
        cap: u64,
        stop: &AtomicUsize,
    ) -> Result<StepPage<[f64; 4]>> {
        self.unchanged()?;
        check_cancelled(stop)?;
        let n = self.count(r.check)?;
        if !(1..=SCAN_ITEMS).contains(&cap)
            || r.after.is_some_and(|i| i >= n)
            || r.cursor
                .is_some_and(|c| c.next >= n || c.remaining == 0 || c.remaining > n)
            || r.after.is_some() && r.cursor.is_some()
        {
            return Err(Error::input("invalid DRC step cursor"));
        }
        if let Some(b) = r.bbox_um {
            valid(b)?;
        }
        let mut page = StepPage {
            hit: None,
            next: None,
            scanned: 0,
        };
        if n == 0
            || r.waived == Some(true)
            || r.bbox_um.is_some_and(|b| {
                self.checks[r.check]
                    .bbox_um
                    .is_none_or(|c| !intersects(b, c))
            })
        {
            return Ok(page);
        }
        let advance = |i| {
            if r.backwards {
                if i == 0 {
                    n - 1
                } else {
                    i - 1
                }
            } else if i == n - 1 {
                0
            } else {
                i + 1
            }
        };
        let mut cursor = r.cursor.unwrap_or_else(|| StepCursor {
            next: r.after.map_or(if r.backwards { n - 1 } else { 0 }, advance),
            remaining: n,
        });
        while cursor.remaining > 0 && page.scanned < cap {
            if page.scanned % 1024 == 0 {
                check_cancelled(stop)?;
            }
            let ei = cursor.next;
            cursor.next = advance(ei);
            cursor.remaining -= 1;
            page.scanned += 1;
            let hit = self.hit(r.check, ei)?;
            if r.bbox_um.is_none_or(|b| intersects(b, hit.record.bbox)) {
                page.hit = Some(hit);
                break;
            }
        }
        page.next = (page.hit.is_none() && cursor.remaining > 0).then_some(cursor);
        check_cancelled(stop)?;
        self.unchanged()?;
        Ok(page)
    }
}
impl Filtering for Ascii {
    type Bounds = [f64; 4];
    fn unchanged(&self) -> Result<()> {
        self.unchanged()
    }
    fn count(&self, ci: usize) -> Result<u64> {
        self.checks
            .get(ci)
            .map(|c| c.count)
            .ok_or_else(|| Error::input("DRC rule index"))
    }
    fn bounds(&self, ci: usize) -> Result<Option<[f64; 4]>> {
        self.checks
            .get(ci)
            .map(|c| c.bbox_um)
            .ok_or_else(|| Error::input("DRC rule index"))
    }
    fn selection_candidates(
        &mut self,
        ci: usize,
        ids: &[u64],
        b: Option<[f64; 4]>,
        w: Option<bool>,
        stop: &AtomicUsize,
    ) -> Result<Vec<InfoHit<[f64; 4]>>> {
        self.unchanged()?;
        check_cancelled(stop)?;
        let n = self.count(ci)?;
        if ids.len() > SELECTION_INPUT || ids.iter().any(|&i| i >= n) {
            return Err(Error::input("selection candidates/bounds"));
        }
        if let Some(b) = b {
            valid(b)?;
        }
        let mut hits = Vec::new();
        for ei in ids.iter().copied().collect::<BTreeSet<_>>() {
            check_cancelled(stop)?;
            let hit = self.hit(ci, ei)?;
            if w != Some(true) && b.is_none_or(|b| intersects(b, hit.record.bbox)) {
                hits.push(hit);
            }
        }
        self.unchanged()?;
        Ok(hits)
    }
    fn query_info(
        &mut self,
        b: [f64; 4],
        cs: Option<&BTreeSet<usize>>,
        w: Option<bool>,
        mut cursor: Cursor,
        limit: usize,
        stop: &AtomicUsize,
    ) -> Result<InfoPage<[f64; 4]>> {
        self.unchanged()?;
        check_cancelled(stop)?;
        valid(b)?;
        if !(1..=PAGE_ITEMS).contains(&limit)
            || cursor.check > self.checks.len()
            || cursor.check == self.checks.len() && cursor.error != 0
            || cursor.check < self.checks.len() && cursor.error > self.checks[cursor.check].count
            || cs.is_some_and(|cs| cs.iter().any(|&ci| ci >= self.checks.len()))
        {
            return Err(Error::input("invalid DRC query/cursor/limit"));
        }
        let mut page = InfoPage {
            hits: Vec::new(),
            next: None,
            scanned: 0,
        };
        let mut steps = 0;
        while cursor.check < self.checks.len()
            && steps < 4096
            && page.scanned < SCAN_ITEMS
            && page.hits.len() < limit
        {
            check_cancelled(stop)?;
            steps += 1;
            let ci = cursor.check;
            let c = &self.checks[ci];
            if cs.is_some_and(|cs| !cs.contains(&ci))
                || w == Some(true)
                || c.bbox_um.is_none_or(|c| !intersects(b, c))
                || cursor.error == c.count
            {
                cursor = Cursor {
                    check: ci + 1,
                    error: 0,
                };
                continue;
            }
            // Same bounded 64-slot granularity as the packed index, but no
            // source I/O or geometry allocation for metadata-only queries.
            let end = (cursor.error + 64)
                .min(c.count)
                .min(cursor.error + SCAN_ITEMS - page.scanned);
            while cursor.error < end && page.hits.len() < limit {
                let hit = self.hit(ci, cursor.error)?;
                cursor.error += 1;
                page.scanned += 1;
                if intersects(b, hit.record.bbox) {
                    page.hits.push(hit);
                }
            }
            if cursor.error == c.count {
                cursor = Cursor {
                    check: ci + 1,
                    error: 0,
                };
            }
        }
        page.next = (cursor.check < self.checks.len()).then_some(cursor);
        check_cancelled(stop)?;
        self.unchanged()?;
        Ok(page)
    }
    fn step(&mut self, r: StepRequest, stop: &AtomicUsize) -> Result<StepPage<[f64; 4]>> {
        self.step_limited(r, SCAN_ITEMS, stop)
    }
}
