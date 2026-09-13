//! Rule-list filters are intersected before page limits. Empty selection means
//! empty results, never an accidental all-errors query.
use super::{Cursor, InfoHit, Pack, StepPage, StepRequest, SELECTION_INPUT, SELECTION_ITEMS};
use crate::{check_cancelled, Error, Result};
use std::{collections::BTreeSet, sync::atomic::AtomicUsize};

pub struct ListRequest<'a> {
    pub check: usize,
    /// Rule-local source ordinal, not the index into the current selection.
    pub start: u64,
    pub waived: Option<bool>,
    pub bbox_um: Option<[f64; 4]>,
    pub selected: Option<&'a BTreeSet<u64>>,
    pub limit: usize,
}
pub struct ListPage {
    pub hits: Vec<InfoHit>,
    pub next: Option<u64>,
    pub scanned: u64,
}
impl Pack {
    fn validate_filter(&self, check: usize, ids: Option<&BTreeSet<u64>>) -> Result<u64> {
        self.unchanged()?;
        let count = self
            .checks
            .get(check)
            .ok_or_else(|| Error::input("DRC rule index"))?
            .count;
        if ids.is_some_and(|s| s.len() > SELECTION_ITEMS || s.last().is_some_and(|&i| i >= count)) {
            return Err(Error::input("invalid DRC selection filter"));
        }
        Ok(count)
    }
    pub fn filtered_errors(&mut self, r: ListRequest<'_>, stop: &AtomicUsize) -> Result<ListPage> {
        let count = self.validate_filter(r.check, r.selected)?;
        if r.start > count || !(1..=SELECTION_INPUT).contains(&r.limit) {
            return Err(Error::input("invalid filtered error page"));
        }
        // Also validates bounds for empty rules/selections, before early exits.
        self.selection_candidates(r.check, &[], r.bbox_um, r.waived, stop)?;
        if let Some(ids) = r.selected {
            let mut page = ListPage {
                hits: Vec::new(),
                next: None,
                scanned: 0,
            };
            let mut iter = ids.range(r.start..).copied().peekable();
            while iter.peek().is_some() && page.hits.len() < r.limit {
                check_cancelled(stop)?;
                let chunk: Vec<_> = iter.by_ref().take(r.limit - page.hits.len()).collect();
                page.scanned += chunk.len() as u64;
                page.hits
                    .extend(self.selection_candidates(r.check, &chunk, r.bbox_um, r.waived, stop)?);
            }
            page.next = iter.peek().copied();
            self.unchanged()?;
            return Ok(page);
        }
        let bounds = match r.bbox_um {
            Some(b) => Some(b),
            None => self.checks[r.check]
                .bbox
                .map(|b| self.bbox_um(b))
                .transpose()?,
        };
        let Some(bounds) = bounds else {
            return Ok(ListPage {
                hits: Vec::new(),
                next: None,
                scanned: 0,
            });
        };
        let page = self.query_info(
            bounds,
            Some(&BTreeSet::from([r.check])),
            r.waived,
            Cursor {
                check: r.check,
                error: r.start,
            },
            r.limit,
            stop,
        )?;
        Ok(ListPage {
            hits: page.hits,
            next: page.next.filter(|c| c.check == r.check).map(|c| c.error),
            scanned: page.scanned,
        })
    }
    pub fn filtered_step(
        &mut self,
        r: StepRequest,
        ids: Option<&BTreeSet<u64>>,
        stop: &AtomicUsize,
    ) -> Result<StepPage> {
        let Some(ids) = ids else {
            return self.step(r, stop);
        };
        let count = self.validate_filter(r.check, Some(ids))?;
        if r.cursor.is_some() || r.after.is_some_and(|n| n >= count) {
            return Err(Error::input("invalid selected error step"));
        }
        self.selection_candidates(r.check, &[], r.bbox_um, r.waived, stop)?;
        let mut page = StepPage {
            hit: None,
            next: None,
            scanned: 0,
        };
        let ids: Vec<_> = ids.iter().copied().collect();
        if ids.is_empty() {
            return Ok(page);
        }
        let n = ids.len();
        let first = if r.backwards {
            r.after.map_or(n - 1, |after| {
                (ids.partition_point(|&i| i < after) + n - 1) % n
            })
        } else {
            r.after
                .map_or(0, |after| ids.partition_point(|&i| i <= after) % n)
        };
        for offset in (0..n).step_by(SELECTION_INPUT) {
            check_cancelled(stop)?;
            let chunk: Vec<_> = (offset..(offset + SELECTION_INPUT).min(n))
                .map(|j| {
                    ids[if r.backwards {
                        (first + n - j) % n
                    } else {
                        (first + j) % n
                    }]
                })
                .collect();
            let hits = self.selection_candidates(r.check, &chunk, r.bbox_um, r.waived, stop)?;
            page.scanned += chunk.len() as u64;
            for id in chunk {
                if let Ok(i) = hits.binary_search_by_key(&id, |h| h.local) {
                    page.hit = Some(hits[i]);
                    return Ok(page);
                }
            }
        }
        Ok(page)
    }
}
