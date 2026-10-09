//! In-session error groups, separate from focus and immutable pack contents.
use super::{InfoHit, Pack};
use crate::{check_cancelled, Error, ErrorKind, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicUsize,
};

pub const SELECTION_INPUT: usize = 64;
pub const SELECTION_ITEMS: usize = 5000;

#[derive(Clone, Copy, Debug)]
pub enum SelectionMode {
    Replace,
    Add,
    Toggle,
}

#[derive(Default)]
pub struct Selections {
    rules: BTreeMap<usize, BTreeSet<u64>>,
    total: usize,
}
impl Selections {
    pub fn total(&self) -> usize {
        self.total
    }
    pub fn ids(&self, check: usize) -> BTreeSet<u64> {
        self.rules.get(&check).cloned().unwrap_or_default()
    }
    pub fn rules(&self) -> impl Iterator<Item = (usize, &BTreeSet<u64>)> {
        self.rules.iter().map(|(&ci, ids)| (ci, ids))
    }
    pub fn clear(&mut self) {
        self.rules.clear();
        self.total = 0;
    }
    /// IDs must already have been validated against the pack. Build the next
    /// rule in isolation; failure never partially toggles or drops a selection.
    pub fn apply(&mut self, check: usize, hits: &[u64], mode: SelectionMode) -> Result<()> {
        if hits.len() > SELECTION_INPUT {
            return Err(Error::input("selection input limit"));
        }
        let previous = self.rules.get(&check);
        let old = previous.map_or(0, BTreeSet::len);
        let mut next = if matches!(mode, SelectionMode::Replace) {
            BTreeSet::new()
        } else {
            previous.cloned().unwrap_or_default()
        };
        // Duplicated input is a set, particularly important for Ctrl-toggle.
        for id in hits.iter().copied().collect::<BTreeSet<_>>() {
            if matches!(mode, SelectionMode::Toggle) && next.remove(&id) {
                continue;
            }
            next.insert(id);
        }
        let total = self.total - old + next.len();
        if total > SELECTION_ITEMS {
            return Err(Error::new(
                ErrorKind::Incomplete,
                "selection limit; previous selection preserved",
            ));
        }
        if next.is_empty() {
            self.rules.remove(&check);
        } else {
            self.rules.insert(check, next);
        }
        self.total = total;
        Ok(())
    }
}
impl Pack {
    /// Test only explicitly supplied page members, never scan the entire pack.
    /// Inclusive bbox intersection matches GTK's _esel_apply, not containment
    /// of marker centers. No vertex arrays are cloned for grouping/metadata.
    pub fn selection_candidates(
        &mut self,
        check: usize,
        errors: &[u64],
        bbox: Option<[f64; 4]>,
        waived: Option<bool>,
        stop: &AtomicUsize,
    ) -> Result<Vec<InfoHit>> {
        self.unchanged()?;
        check_cancelled(stop)?;
        let count = self
            .checks
            .get(check)
            .ok_or_else(|| Error::input("selection rule index"))?
            .count;
        if errors.len() > SELECTION_INPUT
            || errors.iter().any(|&ei| ei >= count)
            || bbox.is_some_and(|b| !b.iter().all(|n| n.is_finite()) || b[0] > b[2] || b[1] > b[3])
        {
            return Err(Error::input("selection candidates/bounds"));
        }
        let mut hits = Vec::new();
        for ei in errors.iter().copied().collect::<BTreeSet<_>>() {
            check_cancelled(stop)?;
            let status = self.status(check, ei)?;
            if waived.is_some_and(|w| (status == 1) != w) {
                continue;
            }
            let record = self.error_info(check, ei, stop)?;
            let b = self.bbox_um(record.bbox)?;
            if bbox.is_some_and(|q| b[0] > q[2] || b[2] < q[0] || b[1] > q[3] || b[3] < q[1]) {
                continue;
            }
            hits.push(InfoHit {
                check,
                local: ei,
                status,
                record,
            });
        }
        self.unchanged()?;
        check_cancelled(stop)?;
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(s: &Selections) -> Vec<(usize, Vec<u64>)> {
        s.rules()
            .map(|(ci, ids)| (ci, ids.iter().copied().collect()))
            .collect()
    }
    #[test]
    fn replace_add_toggle_are_rule_local_sorted_and_keep_u64() {
        let mut s = Selections::default();
        s.apply(2, &[9, 1, 9], SelectionMode::Replace).unwrap();
        s.apply(2, &[3, 1], SelectionMode::Add).unwrap();
        s.apply(1, &[u64::MAX, 9007199254740993], SelectionMode::Replace)
            .unwrap();
        s.apply(2, &[3, 3, 7], SelectionMode::Toggle).unwrap();
        assert_eq!(
            snapshot(&s),
            vec![(1, vec![9007199254740993, u64::MAX]), (2, vec![1, 7, 9])]
        );
        assert_eq!(s.total(), 5);
        s.apply(2, &[], SelectionMode::Replace).unwrap();
        assert_eq!(s.total(), 2);
        assert_eq!(snapshot(&s).len(), 1);
        s.clear();
        assert_eq!(s.total(), 0);
        assert!(snapshot(&s).is_empty());
    }
    #[test]
    fn limits_are_atomic_across_rules_and_toggle_duplicates_once() {
        let mut s = Selections::default();
        for start in (0..SELECTION_ITEMS as u64).step_by(SELECTION_INPUT) {
            let ids = (start..(start + SELECTION_INPUT as u64).min(SELECTION_ITEMS as u64))
                .collect::<Vec<_>>();
            s.apply(0, &ids, SelectionMode::Add).unwrap();
        }
        let before = snapshot(&s);
        assert!(s.apply(1, &[8], SelectionMode::Add).is_err());
        assert!(s
            .apply(0, &[0; SELECTION_INPUT + 1], SelectionMode::Replace)
            .is_err());
        assert_eq!(snapshot(&s), before);
        s.apply(0, &[0, 0, 1, 1], SelectionMode::Toggle).unwrap();
        assert_eq!(s.total(), SELECTION_ITEMS - 2);
        s.apply(1, &[8, 9], SelectionMode::Add).unwrap();
        assert_eq!(s.total(), SELECTION_ITEMS);
    }
}
