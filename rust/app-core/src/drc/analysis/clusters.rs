//! User clusters of a DRC rule's errors (`<db>.clusters`, docs/
//! SPEC-FORMATS.ko.md; floe_oracle/drc_clusters.py):
//!
//! ```text
//! [M1.SPACE.1]
//! cluster 1 = 1-1000000, 1000010
//! cluster 2 = 1000001-1000009
//! ```
//!
//! Numbers are the rule-local error numbers the grid shows (one based,
//! ranges inclusive). A cluster is kept as disjoint runs, never one entry
//! per member; the errors no cluster names form `Unclustered`. The whole
//! file is checked before any of it is used.
use super::scope::{Membership, Runs};
use super::ticks::py_space;
use crate::{Error, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The cluster of a rule's errors no line names.
pub const UNCLUSTERED: &str = "Unclustered";
/// Status counts are kept per run piece of at most this many errors.
const STATUS_CHUNK: u64 = 1 << 20;

/// Where the review statuses of a rule's errors are read (one byte per
/// error, 1 = waived); a database without review state reads none.
pub trait Statuses {
    /// Check `check`'s status bytes `start..start+count`.
    fn statuses(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>>;
    /// (waived, total) of the whole rule, when known at no cost.
    fn rule_counts(&self, check: usize) -> Option<(u64, u64)>;
}

/// No review state: every error is not waived.
pub struct NoStatuses;
impl Statuses for NoStatuses {
    fn statuses(&self, _check: usize, _start: u64, count: u64) -> Result<Vec<u8>> {
        Ok(vec![0; count as usize])
    }
    fn rule_counts(&self, _check: usize) -> Option<(u64, u64)> {
        None
    }
}

#[derive(Clone, Debug, Default)]
struct StatusIndex {
    // the runs split at STATUS_CHUNK boundaries, with each piece's waived
    // count and the running sums of waived / not-waived members
    starts: Vec<u64>,
    stops: Vec<u64>,
    waived: Vec<u64>,
    wprefix: Vec<u64>,
    nprefix: Vec<u64>,
}

/// One cluster: disjoint, sorted half-open runs of rule-local indices.
#[derive(Clone, Debug)]
pub struct Cluster {
    pub name: String,
    pub check: usize,
    /// the members (a thread-safe snapshot for workers)
    pub runs: Arc<Runs>,
    pub total: u64,
    index: Option<StatusIndex>,
    /// None unknown; Some(false) all not waived; Some(true) all waived
    uniform: Option<Option<bool>>,
}

impl Cluster {
    fn new(name: String, check: usize, starts: Vec<u64>, stops: Vec<u64>) -> Self {
        let runs = Runs::new(starts, stops);
        Self {
            name,
            check,
            total: runs.total,
            runs: Arc::new(runs),
            index: None,
            uniform: None,
        }
    }

    /// The runs (rule-local, half open).
    pub fn runs(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.runs.starts.iter().copied().zip(self.runs.stops.iter().copied())
    }

    pub fn contains(&self, ei: u64) -> bool {
        self.runs.contains(ei)
    }

    /// Membership of the rule-local indices `start..start+count`.
    pub fn mask(&self, start: u64, count: usize) -> Vec<bool> {
        let mut out = vec![false; count];
        self.runs.mask(start, &mut out);
        out
    }

    fn uniform(&mut self, statuses: &dyn Statuses) -> Option<bool> {
        if self.uniform.is_none() {
            // a rule all waived or none (no review state at all: none) needs
            // no status scan (floe_oracle drc_clusters `_uniform_status`)
            self.uniform = Some(match statuses.rule_counts(self.check) {
                Some((0, _)) | None => Some(false),
                Some((w, n)) if w == n => Some(true),
                Some(_) => None,
            });
        }
        self.uniform.flatten()
    }

    fn pieces(&self) -> StatusIndex {
        let mut index = StatusIndex::default();
        for (mut a, b) in self.runs() {
            while a < b {
                let end = b.min((a / STATUS_CHUNK + 1) * STATUS_CHUNK);
                index.starts.push(a);
                index.stops.push(end);
                a = end;
            }
        }
        index
    }

    fn finish(index: &mut StatusIndex) {
        let (mut w, mut n) = (0, 0);
        index.wprefix.clear();
        index.nprefix.clear();
        for ((a, b), waived) in index.starts.iter().zip(&index.stops).zip(&index.waived) {
            w += waived;
            n += (b - a) - waived;
            index.wprefix.push(w);
            index.nprefix.push(n);
        }
    }

    /// Members whose waived-ness is `waived` (None: all).
    pub fn count(&mut self, waived: Option<bool>, statuses: &dyn Statuses) -> Result<u64> {
        let Some(waived) = waived else {
            return Ok(self.total);
        };
        if let Some(u) = self.uniform(statuses) {
            return Ok(if u == waived { self.total } else { 0 });
        }
        self.ensure(statuses)?;
        let index = self.index.as_ref().expect("built");
        let w = index.wprefix.last().copied().unwrap_or(0);
        Ok(if waived { w } else { self.total - w })
    }

    /// (waived, total).
    pub fn status_counts(&mut self, statuses: &dyn Statuses) -> Result<(u64, u64)> {
        Ok((self.count(Some(true), statuses)?, self.total))
    }

    fn ensure(&mut self, statuses: &dyn Statuses) -> Result<()> {
        if self.index.is_none() {
            let mut list = [&mut *self];
            prepare(&mut list, statuses)?;
        }
        Ok(())
    }

    fn slice_waived(&self, statuses: &dyn Statuses, a: u64, b: u64) -> Result<Vec<bool>> {
        Ok(statuses
            .statuses(self.check, a, b - a)?
            .into_iter()
            .map(|s| s == 1)
            .collect())
    }

    /// The members `start..start+limit` (in rule order) whose waived-ness
    /// is `waived` (None: all), as rule-local indices.
    pub fn page(
        &mut self,
        start: u64,
        limit: u64,
        waived: Option<bool>,
        statuses: &dyn Statuses,
    ) -> Result<Vec<u64>> {
        if limit == 0 {
            return Ok(vec![]);
        }
        if let Some(w) = waived {
            if let Some(u) = self.uniform(statuses) {
                return if u == w {
                    self.page(start, limit, None, statuses)
                } else {
                    Ok(vec![])
                };
            }
        }
        let mut out = Vec::new();
        let Some(w) = waived else {
            let r = &*self.runs;
            let mut k = r.prefix.partition_point(|&p| p <= start);
            let mut before = if k > 0 { r.prefix[k - 1] } else { 0 };
            while k < r.starts.len() && (out.len() as u64) < limit {
                let a = r.starts[k] + start.saturating_sub(before);
                let b = r.stops[k].min(a + limit - out.len() as u64);
                out.extend(a..b);
                before = r.prefix[k];
                k += 1;
            }
            return Ok(out);
        };
        self.ensure(statuses)?;
        let index = self.index.as_ref().expect("built");
        let prefix = if w { &index.wprefix } else { &index.nprefix };
        let mut k = prefix.partition_point(|&p| p <= start);
        let mut before = if k > 0 { prefix[k - 1] } else { 0 };
        while k < index.starts.len() && (out.len() as u64) < limit {
            if prefix[k] > before {
                let (a, b) = (index.starts[k], index.stops[k]);
                let skip = start.saturating_sub(before);
                let mask = self.slice_waived(statuses, a, b)?;
                out.extend(
                    mask.iter()
                        .enumerate()
                        .filter(|(_, &m)| m == w)
                        .skip(skip as usize)
                        .take((limit - out.len() as u64) as usize)
                        .map(|(i, _)| a + i as u64),
                );
            }
            before = prefix[k];
            // pieces that contribute nothing are jumped over
            k = prefix[k + 1..].partition_point(|&p| p <= before) + k + 1;
        }
        Ok(out)
    }

    /// The rank of `ei` among the members whose waived-ness is `waived`
    /// (None: all); None when it is not one of them.
    pub fn rank(
        &mut self,
        ei: u64,
        waived: Option<bool>,
        statuses: &dyn Statuses,
    ) -> Result<Option<u64>> {
        let r = Arc::clone(&self.runs);
        let k = r.starts.partition_point(|&s| s <= ei);
        if k == 0 || ei >= r.stops[k - 1] {
            return Ok(None);
        }
        let k = k - 1;
        if let Some(w) = waived {
            if let Some(u) = self.uniform(statuses) {
                return if u == w {
                    self.rank(ei, None, statuses)
                } else {
                    Ok(None)
                };
            }
        }
        let Some(w) = waived else {
            let before = if k > 0 { r.prefix[k - 1] } else { 0 };
            return Ok(Some(before + ei - r.starts[k]));
        };
        if self.slice_waived(statuses, ei, ei + 1)?[0] != w {
            return Ok(None);
        }
        self.ensure(statuses)?;
        let index = self.index.as_ref().expect("built");
        let prefix = if w { &index.wprefix } else { &index.nprefix };
        let j = index.starts.partition_point(|&s| s <= ei) - 1;
        let before = if j > 0 { prefix[j - 1] } else { 0 };
        let a = index.starts[j];
        let n = self
            .slice_waived(statuses, a, ei)?
            .iter()
            .filter(|&&m| m)
            .count() as u64;
        Ok(Some(before + if w { n } else { ei - a - n }))
    }

    /// Forget the counts (the statuses changed wholesale).
    pub fn reset_status(&mut self) {
        self.index = None;
        self.uniform = None;
    }
}

/// Build the status counts of `clusters` (one rule's) reading each status
/// block once, however the clusters interleave.
fn prepare(clusters: &mut [&mut Cluster], statuses: &dyn Statuses) -> Result<()> {
    let mut pending: Vec<(usize, StatusIndex, usize)> = Vec::new();
    for (i, c) in clusters.iter().enumerate() {
        if c.index.is_none() {
            let mut index = c.pieces();
            index.waived = vec![0; index.starts.len()];
            pending.push((i, index, 0));
        }
    }
    if pending.is_empty() {
        return Ok(());
    }
    let check = clusters[pending[0].0].check;
    loop {
        // the next status block any cluster needs
        let Some(block) = pending
            .iter()
            .filter_map(|(_, index, cursor)| index.starts.get(*cursor).map(|s| s / STATUS_CHUNK))
            .min()
        else {
            break;
        };
        let (a, b) = (block * STATUS_CHUNK, (block + 1) * STATUS_CHUNK);
        let lo = pending
            .iter()
            .filter_map(|(_, index, cursor)| index.starts.get(*cursor).copied())
            .filter(|&s| s < b)
            .min()
            .unwrap_or(a);
        let hi = pending
            .iter()
            .flat_map(|(_, index, cursor)| {
                index.starts[*cursor..]
                    .iter()
                    .zip(&index.stops[*cursor..])
                    .take_while(|(s, _)| **s < b)
                    .map(|(_, e)| *e)
            })
            .max()
            .unwrap_or(lo);
        let bytes = statuses.statuses(check, lo, hi - lo)?;
        let mut running = Vec::with_capacity(bytes.len() + 1);
        running.push(0u64);
        for s in &bytes {
            running.push(running.last().unwrap() + u64::from(*s == 1));
        }
        for (_, index, cursor) in pending.iter_mut() {
            while *cursor < index.starts.len() && index.starts[*cursor] < b {
                let (s, e) = (index.starts[*cursor], index.stops[*cursor]);
                index.waived[*cursor] = running[(e - lo) as usize] - running[(s - lo) as usize];
                *cursor += 1;
            }
        }
    }
    for (i, mut index, _) in pending {
        Cluster::finish(&mut index);
        clusters[i].index = Some(index);
    }
    Ok(())
}

/// A loaded clusters file: per rule (check index) its clusters, in the
/// file's first-appearance order, `Unclustered` last.
#[derive(Clone, Debug)]
pub struct ClusterFile {
    pub path: PathBuf,
    pub rules: std::collections::BTreeMap<usize, Vec<Cluster>>,
}

impl ClusterFile {
    /// Count every mixed-status cluster of rule `check` in one pass.
    pub fn prepare_counts(&mut self, check: usize, statuses: &dyn Statuses) -> Result<()> {
        let Some(clusters) = self.rules.get_mut(&check) else {
            return Ok(());
        };
        let mut todo: Vec<&mut Cluster> = Vec::new();
        for c in clusters.iter_mut() {
            if c.index.is_none() && c.uniform(statuses).is_none() {
                todo.push(c);
            }
        }
        let result = prepare(&mut todo, statuses);
        if result.is_err() {
            for c in todo {
                c.reset_status();
            }
        }
        result
    }

    /// Statuses of rule `check`'s errors `eis` changed: refresh the counts
    /// of the pieces holding them.
    pub fn status_changed(&mut self, check: usize, eis: &[u64], statuses: &dyn Statuses) -> Result<()> {
        let Some(clusters) = self.rules.get_mut(&check) else {
            return Ok(());
        };
        for c in clusters.iter_mut() {
            c.uniform = None;
            if c.index.is_none() {
                continue;
            }
            let mut touched: Vec<usize> = {
                let index = c.index.as_ref().expect("built");
                eis.iter()
                    .filter_map(|&ei| {
                        let k = index.starts.partition_point(|&s| s <= ei);
                        (k > 0 && ei < index.stops[k - 1]).then(|| k - 1)
                    })
                    .collect()
            };
            touched.sort_unstable();
            touched.dedup();
            for k in touched {
                let (a, b) = {
                    let index = c.index.as_ref().expect("built");
                    (index.starts[k], index.stops[k])
                };
                let w = c.slice_waived(statuses, a, b)?.iter().filter(|&&m| m).count() as u64;
                c.index.as_mut().expect("built").waived[k] = w;
            }
            Cluster::finish(c.index.as_mut().expect("built"));
        }
        Ok(())
    }

    pub fn reset_status(&mut self) {
        for clusters in self.rules.values_mut() {
            for c in clusters {
                c.reset_status();
            }
        }
    }
}

/// Python `repr` of a str, as error messages quote names.
fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python text-mode lines: `\n`, `\r\n` and `\r` end a line.
fn lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut s = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\n' || b[i] == b'\r' {
            out.push(&text[s..i]);
            if b[i] == b'\r' && i + 1 < b.len() && b[i + 1] == b'\n' {
                i += 1;
            }
            s = i + 1;
        }
        i += 1;
    }
    if s < b.len() {
        out.push(&text[s..]);
    }
    out
}

/// Read and check a clusters file against the rules `names` (the database's
/// check names, in check order) and their error `counts`. Every error names
/// the file and the line (`path:line: message`); nothing is kept on error.
pub fn load(path: &Path, names: &[String], counts: &[u64]) -> Result<ClusterFile> {
    let shown = path.display().to_string();
    let fail = |line: usize, message: String| Error::input(format!("{shown}:{line}: {message}"));
    let raw = std::fs::read(path)?;
    let raw = raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&raw);
    let text = std::str::from_utf8(raw)
        .map_err(|e| Error::input(format!("{shown}: not UTF-8 text ({e})")))?;
    let mut by_name: std::collections::HashMap<&str, Vec<usize>> = Default::default();
    for (ci, name) in names.iter().enumerate() {
        by_name.entry(name.as_str()).or_default().push(ci);
    }
    // per rule: labels in first-appearance order, their (start, stop, line)
    let mut pending: Vec<(usize, Vec<(String, Vec<(u64, u64, usize)>)>)> = Vec::new();
    let mut current: Option<usize> = None;
    for (no, raw_line) in lines(text).into_iter().enumerate() {
        let lineno = no + 1;
        let line = raw_line.trim_matches(py_space);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') && line.len() >= 2 {
            let name = line[1..line.len() - 1].trim_matches(py_space);
            let matches = by_name.get(name).map(Vec::as_slice).unwrap_or(&[]);
            if matches.is_empty() {
                return Err(fail(lineno, format!("unknown rule {}", py_repr(name))));
            }
            if matches.len() != 1 {
                return Err(fail(lineno, format!("ambiguous duplicate rule name {}", py_repr(name))));
            }
            let ci = matches[0];
            current = Some(ci);
            if !pending.iter().any(|(c, _)| *c == ci) {
                pending.push((ci, Vec::new()));
            }
            continue;
        }
        let Some(ci) = current else {
            return Err(fail(lineno, "cluster needs a [rule name] section".into()));
        };
        let (label, body) = match line.split_once('=') {
            Some((l, b)) => (l.trim_matches(py_space), b),
            None => ("", ""),
        };
        if !line.contains('=') || label.is_empty() || body.trim_matches(py_space).is_empty() {
            return Err(fail(lineno, "expected cluster name = 1-10, 20".into()));
        }
        if label == UNCLUSTERED {
            return Err(fail(lineno, format!("{} is reserved for unassigned errors", py_repr(label))));
        }
        let n = counts[ci];
        // `([0-9]+)(?:\s*-\s*([0-9]+))?` matches, only " ,\t" between them
        let cs: Vec<char> = body.chars().collect();
        let mut ranges = Vec::new();
        let (mut i, mut end, mut count) = (0usize, 0usize, 0usize);
        let gap_ok = |a: usize, b: usize| -> bool {
            let s: String = cs[a..b].iter().collect();
            s.trim_matches([' ', ',', '\t']).is_empty()
        };
        while i < cs.len() {
            if !cs[i].is_ascii_digit() {
                i += 1;
                continue;
            }
            if !gap_ok(end, i) {
                let near: String = cs[end..i].iter().collect();
                return Err(fail(lineno, format!("invalid member range near {}", py_repr(&near))));
            }
            let s1 = i;
            while i < cs.len() && cs[i].is_ascii_digit() {
                i += 1;
            }
            let first: String = cs[s1..i].iter().collect();
            let mut second: Option<String> = None;
            let mut j = i;
            while j < cs.len() && py_space(cs[j]) {
                j += 1;
            }
            if j < cs.len() && cs[j] == '-' {
                let mut k = j + 1;
                while k < cs.len() && py_space(cs[k]) {
                    k += 1;
                }
                let s2 = k;
                while k < cs.len() && cs[k].is_ascii_digit() {
                    k += 1;
                }
                if k > s2 {
                    second = Some(cs[s2..k].iter().collect());
                    i = k;
                }
            }
            let parse = |t: &str| -> Option<u64> {
                let t = t.trim_start_matches('0');
                if t.is_empty() {
                    Some(0)
                } else if t.len() > 19 {
                    None
                } else {
                    t.parse::<u64>().ok()
                }
            };
            // a number past u64 is past every rule: reported as out of range
            let a = parse(&first);
            let b = match &second {
                Some(t) => parse(t),
                None => a,
            };
            match (a, b) {
                (Some(a), Some(b)) if a >= 1 && b >= a && b <= n => ranges.push((a - 1, b, lineno)),
                _ => {
                    let shown_num = |t: &str| t.trim_start_matches('0').to_string();
                    let (fa, fb) = (
                        shown_num(&first),
                        shown_num(second.as_deref().unwrap_or(&first)),
                    );
                    return Err(fail(
                        lineno,
                        format!(
                            "range {}-{} outside rule errors 1-{n}",
                            if fa.is_empty() { "0".into() } else { fa },
                            if fb.is_empty() { "0".into() } else { fb }
                        ),
                    ));
                }
            }
            end = i;
            count += 1;
        }
        let tail: String = cs[end.min(cs.len())..].iter().collect();
        if count == 0 || !tail.trim_matches([' ', ',', '\t']).is_empty() {
            return Err(fail(lineno, format!("invalid member range {}", py_repr(&tail))));
        }
        let labels = &mut pending.iter_mut().find(|(c, _)| *c == ci).expect("section").1;
        match labels.iter_mut().find(|(l, _)| l == label) {
            Some((_, r)) => r.extend(ranges),
            None => labels.push((label.to_string(), ranges)),
        }
    }
    let mut rules = std::collections::BTreeMap::new();
    for (ci, labels) in pending {
        if labels.is_empty() {
            continue;
        }
        // (start, stop, line, owner), sorted by start (stable: file order)
        let mut all: Vec<(u64, u64, usize, usize)> = labels
            .iter()
            .enumerate()
            .flat_map(|(owner, (_, r))| r.iter().map(move |&(a, b, l)| (a, b, l, owner)))
            .collect();
        all.sort_by_key(|r| r.0);
        if let Some(k) = (1..all.len()).find(|&k| all[k].0 < all[k - 1].1) {
            return Err(fail(
                all[k].2,
                format!(
                    "overlapping memberships in {} and {}",
                    py_repr(&labels[all[k - 1].3].0),
                    py_repr(&labels[all[k].3].0)
                ),
            ));
        }
        let mut clusters = Vec::new();
        for (owner, (label, _)) in labels.iter().enumerate() {
            let (mut starts, mut stops): (Vec<u64>, Vec<u64>) = (Vec::new(), Vec::new());
            for &(a, b, _, o) in &all {
                if o != owner {
                    continue;
                }
                if stops.last() == Some(&a) {
                    *stops.last_mut().unwrap() = b; // adjacent runs merge
                } else {
                    starts.push(a);
                    stops.push(b);
                }
            }
            clusters.push(Cluster::new(label.clone(), ci, starts, stops));
        }
        let n = counts[ci];
        let (mut starts, mut stops) = (Vec::new(), Vec::new());
        let mut at = 0;
        for &(a, b, _, _) in &all {
            if at < a {
                starts.push(at);
                stops.push(a);
            }
            at = b;
        }
        if at < n {
            starts.push(at);
            stops.push(n);
        }
        if !starts.is_empty() {
            clusters.push(Cluster::new(UNCLUSTERED.into(), ci, starts, stops));
        }
        rules.insert(ci, clusters);
    }
    Ok(ClusterFile {
        path: path.to_path_buf(),
        rules,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Vec<u8>);
    impl Statuses for Fixed {
        fn statuses(&self, _c: usize, start: u64, count: u64) -> Result<Vec<u8>> {
            Ok(self.0[start as usize..(start + count) as usize].to_vec())
        }
        fn rule_counts(&self, _c: usize) -> Option<(u64, u64)> {
            Some((self.0.iter().filter(|&&s| s == 1).count() as u64, self.0.len() as u64))
        }
    }

    fn write(text: &str) -> (tempdir::Dir, PathBuf) {
        let d = tempdir::Dir::new();
        let p = d.0.join("r.db.clusters");
        std::fs::write(&p, text).unwrap();
        (d, p)
    }
    mod tempdir {
        pub struct Dir(pub std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                let p = std::env::temp_dir().join(format!(
                    "floe-clusters-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn runs_merge_and_unclustered_fills_the_gaps() {
        let names = vec!["A".to_string(), "B".to_string()];
        let (_d, p) = write("\u{feff}# c\n[A]\nx = 1-3, 7\ny = 4 - 5\nx = 8,9\n\n[B]\n");
        let f = load(&p, &names, &[10, 5]).unwrap();
        let a = &f.rules[&0];
        assert_eq!(a.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["x", "y", UNCLUSTERED]);
        assert_eq!(a[0].runs().collect::<Vec<_>>(), [(0, 3), (6, 9)]);
        assert_eq!(a[2].runs().collect::<Vec<_>>(), [(5, 6), (9, 10)]);
        assert!(!f.rules.contains_key(&1));
        assert_eq!(a[0].mask(2, 6), [true, false, false, false, true, true]);
    }

    #[test]
    fn errors_name_file_and_line() {
        let names = vec!["A".to_string(), "A".to_string(), "C".to_string()];
        for (text, want) in [
            ("[Z]\n", ":1: unknown rule 'Z'"),
            ("[A]\n", ":1: ambiguous duplicate rule name 'A'"),
            ("x = 1\n", ":1: cluster needs a [rule name] section"),
            ("[C]\nx 1\n", ":2: expected cluster name = 1-10, 20"),
            ("[C]\nUnclustered = 1\n", ":2: 'Unclustered' is reserved for unassigned errors"),
            ("[C]\nx = 0-2\n", ":2: range 0-2 outside rule errors 1-5"),
            ("[C]\nx = 3-2\n", ":2: range 3-2 outside rule errors 1-5"),
            ("[C]\nx = 1-6\n", ":2: range 1-6 outside rule errors 1-5"),
            ("[C]\nx = 1; 2\n", ":2: invalid member range near '; '"),
            ("[C]\nx = 1 a\n", ":2: invalid member range ' a'"),
            ("[C]\nx = 1-2\ny = 2\n", ":3: overlapping memberships in 'x' and 'y'"),
        ] {
            let (_d, p) = write(text);
            let e = load(&p, &names, &[5, 5, 5]).unwrap_err().message;
            assert!(e.ends_with(want), "{text:?}: {e}");
        }
    }

    #[test]
    fn pages_and_ranks_follow_the_waive_filter() {
        let names = vec!["A".to_string()];
        let (_d, p) = write("[A]\nx = 1-4, 9-10\n");
        let mut f = load(&p, &names, &[10]).unwrap();
        let st = Fixed(vec![1, 0, 1, 0, 0, 0, 0, 0, 1, 1]);
        f.prepare_counts(0, &st).unwrap();
        let c = &mut f.rules.get_mut(&0).unwrap()[0];
        assert_eq!(c.count(Some(true), &st).unwrap(), 4);
        assert_eq!(c.count(Some(false), &st).unwrap(), 2);
        assert_eq!(c.page(0, 10, None, &st).unwrap(), [0, 1, 2, 3, 8, 9]);
        assert_eq!(c.page(1, 2, Some(true), &st).unwrap(), [2, 8]);
        assert_eq!(c.page(0, 5, Some(false), &st).unwrap(), [1, 3]);
        assert_eq!(c.rank(8, Some(true), &st).unwrap(), Some(2));
        assert_eq!(c.rank(3, Some(false), &st).unwrap(), Some(1));
        assert_eq!(c.rank(3, Some(true), &st).unwrap(), None);
        assert_eq!(c.rank(5, None, &st).unwrap(), None);
        assert_eq!(c.rank(9, None, &st).unwrap(), Some(5));
        let st2 = Fixed(vec![1, 1, 1, 0, 0, 0, 0, 0, 1, 1]);
        f.status_changed(0, &[1], &st2).unwrap();
        let c = &mut f.rules.get_mut(&0).unwrap()[0];
        assert_eq!(c.count(Some(true), &st2).unwrap(), 5);
    }
}
