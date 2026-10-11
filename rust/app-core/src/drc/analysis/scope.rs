//! Which errors of a rule a view, a list or an edit takes: a user cluster's
//! runs, a delta group, a selection - and their intersection. Membership is
//! asked a bounded range at a time; nothing builds a rule-sized list.
use std::sync::Arc;

/// Rule-local membership, read a range at a time (thread-safe snapshots).
pub trait Membership: Send + Sync {
    /// `out[i]` = whether `start + i` is a member.
    fn mask(&self, start: u64, out: &mut [bool]);
    fn contains(&self, ei: u64) -> bool;
}

/// Disjoint sorted half-open runs (a user cluster).
#[derive(Clone, Debug, Default)]
pub struct Runs {
    pub starts: Vec<u64>,
    pub stops: Vec<u64>,
    /// members up to and including each run
    pub prefix: Vec<u64>,
    pub total: u64,
}
impl Runs {
    pub fn new(starts: Vec<u64>, stops: Vec<u64>) -> Self {
        let mut prefix = Vec::with_capacity(starts.len());
        let mut sum = 0;
        for (a, b) in starts.iter().zip(&stops) {
            sum += b - a;
            prefix.push(sum);
        }
        Self {
            starts,
            stops,
            prefix,
            total: sum,
        }
    }
}
impl Membership for Runs {
    fn mask(&self, start: u64, out: &mut [bool]) {
        out.fill(false);
        let end = start + out.len() as u64;
        let lo = self.stops.partition_point(|&s| s <= start);
        let hi = self.starts.partition_point(|&s| s < end);
        for k in lo..hi {
            let a = self.starts[k].max(start) - start;
            let b = self.stops[k].min(end) - start;
            out[a as usize..b as usize].fill(true);
        }
    }
    fn contains(&self, ei: u64) -> bool {
        let k = self.starts.partition_point(|&s| s <= ei);
        k > 0 && ei < self.stops[k - 1]
    }
}

/// The intersection of memberships (none: every error).
#[derive(Clone, Default)]
pub struct Scope(pub Vec<Arc<dyn Membership>>);
impl Scope {
    pub fn all() -> Self {
        Self(Vec::new())
    }
    pub fn is_all(&self) -> bool {
        self.0.is_empty()
    }
    pub fn and(mut self, m: Arc<dyn Membership>) -> Self {
        self.0.push(m);
        self
    }
}
impl Membership for Scope {
    fn mask(&self, start: u64, out: &mut [bool]) {
        out.fill(true);
        let mut part = vec![false; out.len()];
        for m in &self.0 {
            m.mask(start, &mut part);
            for (o, p) in out.iter_mut().zip(&part) {
                *o &= *p;
            }
        }
    }
    fn contains(&self, ei: u64) -> bool {
        self.0.iter().all(|m| m.contains(ei))
    }
}

/// Selection chunk: the prefix counts are kept per this many errors.
pub const SELECTION_CHUNK: u64 = 1 << 16;

/// A complete selection of one rule (feature/jobdeck-drc drc_selection):
/// one bit per error - 200M errors in 25 MB - and per-chunk running counts,
/// so count, contains, a page and a rank need no listing. Immutable once
/// built; an edit makes a new one.
#[derive(Clone, Debug)]
pub struct Selection {
    pub size: u64,
    words: Vec<u64>,
    prefix: Vec<u64>,
}
impl Selection {
    pub fn empty(size: u64) -> Self {
        Self::from_words(size, vec![0; size.div_ceil(64) as usize])
    }
    pub fn from_indices(size: u64, indices: impl IntoIterator<Item = u64>) -> Self {
        let mut words = vec![0u64; size.div_ceil(64) as usize];
        for ei in indices {
            if ei < size {
                words[(ei / 64) as usize] |= 1 << (ei % 64);
            }
        }
        Self::from_words(size, words)
    }
    /// From packed bits (bit `i` of word `i / 64`), past `size` ignored.
    pub fn from_words(size: u64, mut words: Vec<u64>) -> Self {
        words.resize(size.div_ceil(64) as usize, 0);
        if size % 64 != 0 {
            if let Some(last) = words.last_mut() {
                *last &= (1u64 << (size % 64)) - 1;
            }
        }
        let per = (SELECTION_CHUNK / 64) as usize;
        let mut prefix = Vec::with_capacity(words.len() / per + 2);
        prefix.push(0);
        let mut sum = 0u64;
        for chunk in words.chunks(per) {
            sum += chunk.iter().map(|w| u64::from(w.count_ones())).sum::<u64>();
            prefix.push(sum);
        }
        Self {
            size,
            words,
            prefix,
        }
    }
    pub fn total(&self) -> u64 {
        *self.prefix.last().unwrap_or(&0)
    }
    pub fn words(&self) -> &[u64] {
        &self.words
    }
    /// The `start..start+limit`-th selected indices.
    pub fn page(&self, start: u64, limit: u64) -> Vec<u64> {
        let mut out = Vec::new();
        if limit == 0 || start >= self.total() {
            return out;
        }
        let per = (SELECTION_CHUNK / 64) as usize;
        let mut chunk = self.prefix.partition_point(|&p| p <= start) - 1;
        let mut skip = start - self.prefix[chunk];
        while chunk + 1 < self.prefix.len() && (out.len() as u64) < limit {
            let first = chunk * per;
            for (i, &w) in self.words[first..(first + per).min(self.words.len())]
                .iter()
                .enumerate()
            {
                let mut w = w;
                while w != 0 {
                    let bit = w.trailing_zeros() as u64;
                    w &= w - 1;
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    out.push((first + i) as u64 * 64 + bit);
                    if out.len() as u64 >= limit {
                        return out;
                    }
                }
            }
            chunk += 1;
        }
        out
    }
    /// The rank of `ei` among the selected, None when it is not selected.
    pub fn rank(&self, ei: u64) -> Option<u64> {
        if !self.contains(ei) {
            return None;
        }
        let per = SELECTION_CHUNK / 64;
        let chunk = ei / SELECTION_CHUNK;
        let mut n = self.prefix[chunk as usize];
        let first = (chunk * per) as usize;
        let word = (ei / 64) as usize;
        for w in &self.words[first..word] {
            n += u64::from(w.count_ones());
        }
        n += u64::from((self.words[word] & ((1u64 << (ei % 64)) - 1)).count_ones());
        Some(n)
    }
    /// `self` combined with `other` (the same rule): replace, add, toggle,
    /// remove, intersect.
    pub fn combine(&self, other: &Selection, mode: &str) -> crate::Result<Selection> {
        if self.size != other.size {
            return Err(crate::Error::input(
                "cannot combine selections from different rule sizes",
            ));
        }
        let words = match mode {
            "replace" => other.words.clone(),
            "add" => self.words.iter().zip(&other.words).map(|(a, b)| a | b).collect(),
            "toggle" => self.words.iter().zip(&other.words).map(|(a, b)| a ^ b).collect(),
            "remove" => self.words.iter().zip(&other.words).map(|(a, b)| a & !b).collect(),
            "intersect" => self.words.iter().zip(&other.words).map(|(a, b)| a & b).collect(),
            _ => return Err(crate::Error::input("invalid selection mode")),
        };
        Ok(Self::from_words(self.size, words))
    }
}
impl Membership for Selection {
    fn mask(&self, start: u64, out: &mut [bool]) {
        for (i, o) in out.iter_mut().enumerate() {
            let ei = start + i as u64;
            *o = ei < self.size && (self.words[(ei / 64) as usize] >> (ei % 64)) & 1 == 1;
        }
    }
    fn contains(&self, ei: u64) -> bool {
        ei < self.size && (self.words[(ei / 64) as usize] >> (ei % 64)) & 1 == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selections_page_rank_and_combine() {
        let s = Selection::from_indices(200_000, [3, 70_000, 70_001, 199_999]);
        assert_eq!(s.total(), 4);
        assert_eq!(s.page(1, 2), [70_000, 70_001]);
        assert_eq!(s.page(3, 9), [199_999]);
        assert_eq!(s.rank(70_001), Some(2));
        assert_eq!(s.rank(4), None);
        let t = Selection::from_indices(200_000, [3, 5]);
        assert_eq!(s.combine(&t, "toggle").unwrap().page(0, 9), [5, 70_000, 70_001, 199_999]);
        assert_eq!(s.combine(&t, "intersect").unwrap().page(0, 9), [3]);
        assert_eq!(s.combine(&t, "remove").unwrap().total(), 3);
        let mut out = vec![false; 4];
        s.mask(69_999, &mut out);
        assert_eq!(out, [false, true, true, false]);
        let r = Runs::new(vec![0, 10], vec![2, 12]);
        let scope = Scope::all().and(Arc::new(r)).and(Arc::new(Selection::from_indices(20, [1, 10, 15])));
        let mut out = vec![false; 12];
        scope.mask(0, &mut out);
        assert_eq!(out.iter().enumerate().filter(|(_, &m)| m).map(|(i, _)| i).collect::<Vec<_>>(), [1, 10]);
    }
}
