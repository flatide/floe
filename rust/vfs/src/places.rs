//! The placement store of the streaming occupancy pass (docs/
//! STREAMING_INDEX_PLAN.ko.md §4.6, 0.12.398): every cell's normalized
//! placements, kept by the child they place instead of in memory as
//! `Vec<Vec<PlaceRec>>` - so the pass's memory no longer grows with the
//! file's placements (the MAIN01-like chip's 796 M records were 76 GB
//! at about 96 B each, and the pass was skipped past a cap).
//!
//! The walk enumerates a cell's instances through its parents'
//! (`occupancy::Instances::each`): for each parent, its instances, and
//! at each the placements of the parent that place this cell. So the
//! records are grouped as **runs**: one run per (child, parent) edge,
//! holding the parent's records of that child in the parent's record
//! order, encoded compactly (a tag byte, zigzag varints: about 12 B a
//! plain placement against 96 B in memory). A cell's instances need its
//! own incoming runs and its ancestors', never the whole file's; the
//! cell's own outgoing records (the charges and the footprints of small
//! children's grids, `StreamOcc::mark_instance`) are the runs of its
//! children in which it is the parent, found through a second index.
//!
//! Backing: the runs live in memory when the file's placements fit a
//! quarter of the streaming budget at `BYTES_EST` each, in a staging
//! file in the cache directory (`FILE_NAME`, removed when the store is
//! dropped) otherwise - a `PlaceWriter` appends a cell's runs as the
//! cell is released (the streaming build), read from the segment
//! directory (`floe-index light`) or by the placements pass (the
//! occupancy-only run), and `finish` sorts the run table by (child,
//! parent). Nothing is read back until the marking; the file is read
//! with pread, run by run, so the resident memory is the run table
//! (one entry per distinct edge: 602 K for the MAIN01-like chip) and
//! the runs a marker holds while enumerating.

use floe_oasis::doc::{PlaceRec, Rep};
use std::borrow::Cow;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// the staging file's name in the cache directory
pub const FILE_NAME: &str = "design.places.tmp";
/// the estimate of a stored record's bytes, for the backing decision
/// (a plain placement is about 12 B; grids and point lists more)
pub const BYTES_EST: u64 = 16;
/// `FLOE_OCCUPANCY_PLACES=file|mem` forces the backing (diagnosis and
/// the gates; the budget decides otherwise)
pub const ENV_BACKING: &str = "FLOE_OCCUPANCY_PLACES";
/// a memory block of the in-memory backing
const BLOCK_BYTES: usize = 32 << 20;

/// where the runs are kept
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backing {
    Memory,
    File(PathBuf),
}

impl Backing {
    /// The backing for `placements` records under a streaming budget:
    /// memory while the estimate (`BYTES_EST` each) fits a quarter of
    /// the budget, else the staging file in `dir`; `ENV_BACKING`
    /// overrides either way.
    pub fn choose(placements: u64, budget: u64, dir: &Path) -> Backing {
        match std::env::var(ENV_BACKING).ok().as_deref() {
            Some("file") => return Backing::File(dir.join(FILE_NAME)),
            Some("mem") | Some("memory") => return Backing::Memory,
            _ => {}
        }
        if placements.saturating_mul(BYTES_EST) <= budget / 4 {
            Backing::Memory
        } else {
            Backing::File(dir.join(FILE_NAME))
        }
    }

    pub fn is_file(&self) -> bool {
        matches!(self, Backing::File(_))
    }
}

/// one (child, parent) edge's records
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub child: u32,
    pub parent: u32,
    /// where the encoded records are (memory: block << 32 | offset;
    /// file: the byte offset)
    loc: u64,
    pub bytes: u32,
    pub records: u32,
    /// the walk's descents through the run: the members of its records,
    /// a prunable grid of a small child counting none (the walk fills
    /// its footprint and does not visit the members)
    pub visits: u64,
}

// ---------------------------------------------------------------- codec

fn put_uvar(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn put_svar(out: &mut Vec<u8>, v: i64) {
    put_uvar(out, ((v << 1) ^ (v >> 63)) as u64);
}

fn get_uvar(b: &[u8], at: &mut usize) -> u64 {
    let mut v = 0u64;
    let mut shift = 0u32;
    while *at < b.len() {
        let x = b[*at];
        *at += 1;
        if shift < 64 {
            v |= ((x & 0x7f) as u64) << shift;
        }
        if x & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    v
}

fn get_svar(b: &[u8], at: &mut usize) -> i64 {
    let u = get_uvar(b, at);
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

const KIND_ONE: u8 = 0;
const KIND_GRID: u8 = 1;
const KIND_PTS: u8 = 2;

/// a record as encoded: tag (rot bits 0-1, flip bit 2, kind bits 3-4),
/// x, y, then the repetition
fn encode_rec(out: &mut Vec<u8>, pl: &PlaceRec) {
    let kind = match pl.rep {
        Rep::One => KIND_ONE,
        Rep::Grid { .. } => KIND_GRID,
        Rep::Pts(_) => KIND_PTS,
    };
    out.push((pl.rot & 3) | (u8::from(pl.flip) << 2) | (kind << 3));
    put_svar(out, pl.x);
    put_svar(out, pl.y);
    match &pl.rep {
        Rep::One => {}
        Rep::Grid { na, nb, va, vb } => {
            put_uvar(out, *na);
            put_uvar(out, *nb);
            put_svar(out, va.0);
            put_svar(out, va.1);
            put_svar(out, vb.0);
            put_svar(out, vb.1);
        }
        Rep::Pts(p) => {
            put_uvar(out, p.len() as u64);
            for &(dx, dy) in p.iter() {
                put_svar(out, dx);
                put_svar(out, dy);
            }
        }
    }
}

/// the repetition of a stored record
#[derive(Clone, Copy, Debug)]
pub enum StoredRep<'a> {
    One,
    Grid { na: u64, nb: u64, va: (i64, i64), vb: (i64, i64) },
    /// the member offsets, still encoded
    Pts { n: u64, bytes: &'a [u8] },
}

/// a stored record, decoded but for a point list's members
#[derive(Clone, Copy, Debug)]
pub struct StoredPlace<'a> {
    pub x: i64,
    pub y: i64,
    pub rot: u8,
    pub flip: bool,
    pub rep: StoredRep<'a>,
}

impl StoredPlace<'_> {
    pub fn members(&self) -> u64 {
        match self.rep {
            StoredRep::One => 1,
            StoredRep::Grid { na, nb, .. } => na.saturating_mul(nb),
            StoredRep::Pts { n, .. } => n,
        }
    }

    /// the members in the walk's order (Grid: i fastest), until `f`
    /// returns false
    pub fn each_member(&self, mut f: impl FnMut(i64, i64) -> bool) -> bool {
        match self.rep {
            StoredRep::One => f(0, 0),
            StoredRep::Grid { na, nb, va, vb } => {
                for j in 0..nb as i64 {
                    for i in 0..na as i64 {
                        if !f(i * va.0 + j * vb.0, i * va.1 + j * vb.1) {
                            return false;
                        }
                    }
                }
                true
            }
            StoredRep::Pts { n, bytes } => {
                let mut at = 0usize;
                for _ in 0..n {
                    let dx = get_svar(bytes, &mut at);
                    let dy = get_svar(bytes, &mut at);
                    if !f(dx, dy) {
                        return false;
                    }
                }
                true
            }
        }
    }

    /// whether the record is an axis-aligned Grid whose pitch is at most
    /// `c` on both axes (occupancy's `grid_prunable`)
    pub fn grid_prunable(&self, c: i64) -> bool {
        match self.rep {
            StoredRep::Grid { na, nb, va, vb } => {
                let axis = |v: (i64, i64)| (v.0 == 0 || v.1 == 0) && v.0.abs() <= c && v.1.abs() <= c;
                na >= 1 && nb >= 1 && axis(va) && axis(vb)
            }
            _ => false,
        }
    }
}

/// decode the next record of `b` at `*at`
pub fn decode_rec<'a>(b: &'a [u8], at: &mut usize) -> StoredPlace<'a> {
    let tag = b[*at];
    *at += 1;
    let x = get_svar(b, at);
    let y = get_svar(b, at);
    let rep = match (tag >> 3) & 3 {
        KIND_ONE => StoredRep::One,
        KIND_GRID => {
            let na = get_uvar(b, at);
            let nb = get_uvar(b, at);
            let va = (get_svar(b, at), get_svar(b, at));
            let vb = (get_svar(b, at), get_svar(b, at));
            StoredRep::Grid { na, nb, va, vb }
        }
        _ => {
            let n = get_uvar(b, at);
            let start = *at;
            // (skip the members: n varint pairs)
            for _ in 0..n {
                get_uvar(b, at);
                get_uvar(b, at);
            }
            StoredRep::Pts { n, bytes: &b[start..*at] }
        }
    };
    StoredPlace {
        x,
        y,
        rot: tag & 3,
        flip: tag & 4 != 0,
        rep,
    }
}

/// the records of one run's bytes
pub struct Records<'a> {
    bytes: &'a [u8],
    at: usize,
    left: u32,
}

impl<'a> Iterator for Records<'a> {
    type Item = StoredPlace<'a>;
    fn next(&mut self) -> Option<StoredPlace<'a>> {
        if self.left == 0 || self.at >= self.bytes.len() {
            return None;
        }
        self.left -= 1;
        Some(decode_rec(self.bytes, &mut self.at))
    }
}

pub fn records(bytes: &[u8], count: u32) -> Records<'_> {
    Records { bytes, at: 0, left: count }
}

// --------------------------------------------------------------- writer

enum Sink {
    Memory { blocks: Vec<Vec<u8>> },
    File { file: std::fs::File, path: PathBuf, len: u64 },
}

struct WriterState {
    sink: Sink,
    runs: Vec<Run>,
    /// cells whose runs were added (a cell added twice is an error of
    /// the caller's: its records would be counted twice)
    added: Vec<bool>,
    /// cells that must have been added by `finish` (`expect`: the cells
    /// the inventory counts placements of), None = no check
    expected: Option<Vec<bool>>,
    total_records: u64,
    total_bytes: u64,
    failed: Option<String>,
}

/// Collects every cell's normalized placements as runs (`add_cell`) and
/// becomes a `PlaceStore` (`finish`). Thread-safe: the streaming build's
/// committer releases cells from several threads.
pub struct PlaceWriter {
    st: Mutex<WriterState>,
    backing: Backing,
    n_cells: usize,
    /// the generator's prune rule: a small child's prunable grid is not
    /// descended into (`Run::visits`)
    small: Vec<bool>,
    cell_dbu: i64,
}

impl PlaceWriter {
    /// `small` / `cell_dbu`: the generator's small cells and grid cell
    /// (`occupancy::small_cells`), which decide a run's `visits`
    pub fn new(n_cells: usize, backing: Backing, small: Vec<bool>, cell_dbu: i64) -> io::Result<PlaceWriter> {
        let sink = match &backing {
            Backing::Memory => Sink::Memory { blocks: Vec::new() },
            Backing::File(path) => {
                let _ = std::fs::remove_file(path);
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(path)?;
                Sink::File {
                    file,
                    path: path.clone(),
                    len: 0,
                }
            }
        };
        Ok(PlaceWriter {
            st: Mutex::new(WriterState {
                sink,
                runs: Vec::new(),
                added: vec![false; n_cells],
                expected: None,
                total_records: 0,
                total_bytes: 0,
                failed: None,
            }),
            backing,
            n_cells,
            small,
            cell_dbu,
        })
    }

    pub fn backing(&self) -> &Backing {
        &self.backing
    }

    /// the cells `finish` requires to have been added (the inventory's
    /// cells with placements): a cell the build never released, or the
    /// directory never yielded, is an error there instead of a summary
    /// missing its instances
    pub fn expect(&self, cells: Vec<bool>) {
        self.lock().expected = Some(cells);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WriterState> {
        self.st.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Cell `parent`'s placements (normalized: `floe_oasis::doc::
    /// normalize_cells`), as runs per child in the parent's record order.
    /// A record placing a cell outside the inventory is dropped (the
    /// walk drops it too). A failed write is remembered and reported by
    /// `finish`.
    pub fn add_cell(&self, parent: usize, places: &[PlaceRec]) {
        if places.is_empty() || parent >= self.n_cells {
            return;
        }
        // the records grouped by child, each child's in record order
        let mut order: Vec<u32> = (0..places.len() as u32).collect();
        order.sort_by_key(|&i| places[i as usize].cell);
        let mut buf: Vec<u8> = Vec::new();
        let mut new_runs: Vec<(Run, usize, usize)> = Vec::new();
        let mut i = 0usize;
        while i < order.len() {
            let child = places[order[i] as usize].cell;
            let mut j = i;
            let start = buf.len();
            let mut records = 0u32;
            let mut visits = 0u64;
            while j < order.len() && places[order[j] as usize].cell == child {
                let pl = &places[order[j] as usize];
                if child < self.n_cells {
                    encode_rec(&mut buf, pl);
                    records += 1;
                    let m = match &pl.rep {
                        Rep::One => 1,
                        Rep::Grid { na, nb, va, vb } => {
                            let small = self.small.get(child).copied().unwrap_or(false);
                            let axis = |v: &(i64, i64)| {
                                (v.0 == 0 || v.1 == 0) && v.0.abs() <= self.cell_dbu && v.1.abs() <= self.cell_dbu
                            };
                            if small && *na >= 1 && *nb >= 1 && axis(va) && axis(vb) {
                                0
                            } else {
                                na.saturating_mul(*nb)
                            }
                        }
                        Rep::Pts(p) => p.len() as u64,
                    };
                    visits = visits.saturating_add(m);
                }
                j += 1;
            }
            if records > 0 {
                new_runs.push((
                    Run {
                        child: child as u32,
                        parent: parent as u32,
                        loc: 0,
                        bytes: (buf.len() - start) as u32,
                        records,
                        visits,
                    },
                    start,
                    buf.len(),
                ));
            }
            i = j;
        }
        let mut st = self.lock();
        if st.failed.is_some() {
            return;
        }
        if st.added[parent] {
            st.failed = Some(format!("cell {} was added to the placement store twice", parent));
            return;
        }
        st.added[parent] = true;
        if new_runs.is_empty() {
            // (every record placed a cell outside the inventory)
            return;
        }
        // the whole buffer in one piece: the runs are consecutive in it
        let base = match &mut st.sink {
            Sink::Memory { blocks } => {
                let need = buf.len();
                let fits = blocks.last().is_some_and(|b| b.len() + need <= b.capacity());
                if !fits {
                    blocks.push(Vec::with_capacity(need.max(BLOCK_BYTES)));
                }
                let bi = blocks.len() - 1;
                let b = &mut blocks[bi];
                let off = b.len();
                b.extend_from_slice(&buf);
                ((bi as u64) << 32) | off as u64
            }
            Sink::File { file, path, len } => {
                use std::os::unix::fs::FileExt;
                if let Err(e) = file.write_all_at(&buf, *len) {
                    st.failed = Some(format!("{}: write: {}", path.display(), e));
                    return;
                }
                let at = *len;
                *len += buf.len() as u64;
                at
            }
        };
        for (mut run, start, _end) in new_runs {
            run.loc = base + start as u64;
            st.total_records += run.records as u64;
            st.total_bytes += run.bytes as u64;
            st.runs.push(run);
        }
    }

    /// what has been added so far: (cells, records, bytes)
    pub fn stats(&self) -> (usize, u64, u64) {
        let st = self.lock();
        (st.added.iter().filter(|&&a| a).count(), st.total_records, st.total_bytes)
    }

    /// The store: the run table sorted by (child, parent) with a per-
    /// child index, and the same runs indexed by parent. Err when a
    /// write failed.
    pub fn finish(self) -> Result<PlaceStore, String> {
        let n = self.n_cells;
        let st = self.st.into_inner().unwrap_or_else(|p| p.into_inner());
        if let Some(why) = st.failed {
            return Err(why);
        }
        let WriterState {
            sink,
            mut runs,
            total_records,
            total_bytes,
            added,
            expected,
            ..
        } = st;
        if let Some(expected) = expected {
            if let Some(ci) = expected.iter().zip(added.iter()).position(|(&e, &a)| e && !a) {
                return Err(format!("cell {} was never added to the placement store", ci));
            }
        }
        runs.sort_by_key(|r| (r.child, r.parent));
        let mut first = vec![0u32; n + 1];
        for r in &runs {
            first[r.child as usize + 1] += 1;
        }
        for i in 0..n {
            first[i + 1] += first[i];
        }
        let mut by_parent: Vec<u32> = (0..runs.len() as u32).collect();
        by_parent.sort_by_key(|&k| (runs[k as usize].parent, runs[k as usize].child));
        let mut pfirst = vec![0u32; n + 1];
        for r in &runs {
            pfirst[r.parent as usize + 1] += 1;
        }
        for i in 0..n {
            pfirst[i + 1] += pfirst[i];
        }
        let backing = match sink {
            Sink::Memory { blocks } => Store::Memory { blocks },
            Sink::File { file, path, .. } => Store::File { file, path },
        };
        Ok(PlaceStore {
            backing,
            runs,
            first,
            by_parent,
            pfirst,
            total_records,
            total_bytes,
            failed: Mutex::new(None),
        })
    }
}

// ---------------------------------------------------------------- store

enum Store {
    Memory { blocks: Vec<Vec<u8>> },
    File { file: std::fs::File, path: PathBuf },
}

/// The finished store: the runs by child (`incoming`) and by parent
/// (`outgoing`), their bytes read on demand.
pub struct PlaceStore {
    backing: Store,
    runs: Vec<Run>,
    /// per child: its runs are runs[first[c]..first[c + 1]]
    first: Vec<u32>,
    /// run indices sorted by (parent, child); per parent:
    /// by_parent[pfirst[p]..pfirst[p + 1]]
    by_parent: Vec<u32>,
    pfirst: Vec<u32>,
    total_records: u64,
    total_bytes: u64,
    /// the first read failure (the file backing)
    failed: Mutex<Option<String>>,
}

impl Drop for PlaceStore {
    fn drop(&mut self) {
        if let Store::File { path, .. } = &self.backing {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl PlaceStore {
    /// an empty store (no placements at all)
    pub fn empty(n_cells: usize) -> PlaceStore {
        PlaceStore {
            backing: Store::Memory { blocks: Vec::new() },
            runs: Vec::new(),
            first: vec![0; n_cells + 1],
            by_parent: Vec::new(),
            pfirst: vec![0; n_cells + 1],
            total_records: 0,
            total_bytes: 0,
            failed: Mutex::new(None),
        }
    }

    pub fn n_cells(&self) -> usize {
        self.first.len() - 1
    }

    pub fn total_records(&self) -> u64 {
        self.total_records
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn n_runs(&self) -> usize {
        self.runs.len()
    }

    pub fn is_file(&self) -> bool {
        matches!(self.backing, Store::File { .. })
    }

    /// the staging file's path, if any
    pub fn path(&self) -> Option<&Path> {
        match &self.backing {
            Store::File { path, .. } => Some(path),
            Store::Memory { .. } => None,
        }
    }

    /// bytes resident for the records (the memory backing's blocks)
    pub fn resident_bytes(&self) -> u64 {
        match &self.backing {
            Store::Memory { blocks } => blocks.iter().map(|b| b.capacity() as u64).sum(),
            Store::File { .. } => 0,
        }
    }

    /// the runs placing `child`, ascending by parent
    pub fn incoming(&self, child: usize) -> &[Run] {
        if child + 1 >= self.first.len() {
            return &[];
        }
        &self.runs[self.first[child] as usize..self.first[child + 1] as usize]
    }

    /// the runs of `parent`'s own placements, ascending by child
    pub fn outgoing(&self, parent: usize) -> impl Iterator<Item = &Run> + '_ {
        let (a, b) = if parent + 1 < self.pfirst.len() {
            (self.pfirst[parent] as usize, self.pfirst[parent + 1] as usize)
        } else {
            (0, 0)
        };
        self.by_parent[a..b].iter().map(move |&k| &self.runs[k as usize])
    }

    /// `parent`'s records placing `child`, if any
    pub fn run_of(&self, child: usize, parent: usize) -> Option<&Run> {
        let runs = self.incoming(child);
        runs.binary_search_by_key(&(parent as u32), |r| r.parent).ok().map(|k| &runs[k])
    }

    /// a run's encoded records: borrowed from memory, read from the file
    /// (an empty slice after a read failure, which `failed` reports)
    pub fn run_bytes(&self, run: &Run) -> Cow<'_, [u8]> {
        match &self.backing {
            Store::Memory { blocks } => {
                let bi = (run.loc >> 32) as usize;
                let off = (run.loc & 0xffff_ffff) as usize;
                Cow::Borrowed(&blocks[bi][off..off + run.bytes as usize])
            }
            Store::File { file, path } => {
                use std::os::unix::fs::FileExt;
                let mut buf = vec![0u8; run.bytes as usize];
                match file.read_exact_at(&mut buf, run.loc) {
                    Ok(()) => Cow::Owned(buf),
                    Err(e) => {
                        let mut f = self.failed.lock().unwrap_or_else(|p| p.into_inner());
                        if f.is_none() {
                            *f = Some(format!("{}: read {} bytes at {}: {}", path.display(), run.bytes, run.loc, e));
                        }
                        Cow::Owned(Vec::new())
                    }
                }
            }
        }
    }

    /// the first read failure, if any
    pub fn failed(&self) -> Option<String> {
        self.failed.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// every record of `parent`'s own placements with the child placed,
    /// in child order (and the parent's record order within a child),
    /// until `f` returns false
    pub fn for_each_outgoing(&self, parent: usize, f: &mut dyn FnMut(usize, &StoredPlace<'_>) -> bool) -> bool {
        for run in self.outgoing(parent) {
            let bytes = self.run_bytes(run);
            for rec in records(&bytes, run.records) {
                if !f(run.child as usize, &rec) {
                    return false;
                }
            }
        }
        true
    }

    /// the records of every run, decoded back (tests and diagnosis)
    pub fn all_records(&self) -> Vec<(usize, usize, PlaceRec)> {
        let mut out = Vec::new();
        for run in &self.runs {
            let bytes = self.run_bytes(run);
            for rec in records(&bytes, run.records) {
                let rep = match rec.rep {
                    StoredRep::One => Rep::One,
                    StoredRep::Grid { na, nb, va, vb } => Rep::Grid { na, nb, va, vb },
                    StoredRep::Pts { .. } => {
                        let mut pts = Vec::new();
                        rec.each_member(|dx, dy| {
                            pts.push((dx, dy));
                            true
                        });
                        Rep::Pts(pts.into())
                    }
                };
                out.push((
                    run.child as usize,
                    run.parent as usize,
                    PlaceRec {
                        cell: run.child as usize,
                        x: rec.x,
                        y: rec.y,
                        rot: rec.rot,
                        flip: rec.flip,
                        rep,
                    },
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(cell: usize, x: i64, y: i64, rot: u8, flip: bool, rep: Rep) -> PlaceRec {
        PlaceRec { cell, x, y, rot, flip, rep }
    }

    fn sample() -> Vec<(usize, Vec<PlaceRec>)> {
        let pts = Rep::Pts(vec![(0, 0), (5000, 100), (-200, 2500), (i64::MAX / 4, i64::MIN / 4)].into());
        vec![
            (
                3,
                vec![
                    rec(1, 10, -20, 0, false, Rep::One),
                    rec(0, 1 << 40, -(1 << 41), 3, true, Rep::Grid { na: 4, nb: 5, va: (100, 0), vb: (0, -100) }),
                    rec(1, 7, 7, 2, false, pts.clone()),
                    rec(2, -1, -1, 1, true, Rep::Grid { na: 2, nb: 2, va: (50, 50), vb: (-50, 50) }),
                    rec(0, 0, 0, 0, false, Rep::One),
                ],
            ),
            (2, vec![rec(0, 5, 5, 0, false, Rep::Grid { na: 3, nb: 1, va: (10, 0), vb: (0, 10) })]),
            (1, vec![rec(0, 9, 9, 0, true, Rep::One), rec(9, 1, 1, 0, false, Rep::One)]),
        ]
    }

    fn check_store(store: &PlaceStore, file: bool) {
        assert_eq!(store.is_file(), file);
        assert_eq!(store.n_cells(), 4);
        // records: cell 3's five (two of them of cell 0 -> one run of
        // two, so three runs), cell 2's one, cell 1's one (the record of
        // cell 9, outside the inventory, dropped): seven records, five runs
        assert_eq!(store.total_records(), 7);
        assert_eq!(store.n_runs(), 5);
        let in0: Vec<(u32, u32, u64)> = store.incoming(0).iter().map(|r| (r.parent, r.records, r.visits)).collect();
        assert_eq!(in0, vec![(1, 1, 1), (2, 1, 0), (3, 2, 21)], "cell 0's incoming runs by parent; the 3 x 1 grid of parent 2 is prunable (cell 0 small, pitch 10 <= 10)");
        let in1: Vec<(u32, u32, u64)> = store.incoming(1).iter().map(|r| (r.parent, r.records, r.visits)).collect();
        assert_eq!(in1, vec![(3, 2, 5)]);
        assert_eq!(store.incoming(2).len(), 1);
        assert!(store.incoming(3).is_empty());
        assert!(store.incoming(7).is_empty());
        let out3: Vec<(u32, u32)> = store.outgoing(3).map(|r| (r.child, r.records)).collect();
        assert_eq!(out3, vec![(0, 2), (1, 2), (2, 1)]);
        assert!(store.outgoing(0).next().is_none());
        assert_eq!(store.run_of(0, 3).map(|r| r.records), Some(2));
        assert!(store.run_of(0, 7).is_none());
        // decoded back: every record of every parent, in child order
        let all = store.all_records();
        let want = sample();
        for (parent, places) in &want {
            let mut mine: Vec<&PlaceRec> = all.iter().filter(|(_, p, _)| p == parent).map(|(_, _, r)| r).collect();
            let mut theirs: Vec<&PlaceRec> = places.iter().filter(|p| p.cell < 4).collect();
            mine.sort_by_key(|r| (r.cell, r.x, r.y));
            theirs.sort_by_key(|r| (r.cell, r.x, r.y));
            assert_eq!(mine.len(), theirs.len(), "parent {}", parent);
            for (a, b) in mine.iter().zip(theirs.iter()) {
                assert_eq!(a.cell, b.cell);
                assert_eq!((a.x, a.y, a.rot, a.flip), (b.x, b.y, b.rot, b.flip));
                assert_eq!(a.rep, b.rep, "parent {} child {}", parent, a.cell);
            }
        }
        // the outgoing view walks the same records
        let mut n = 0;
        store.for_each_outgoing(3, &mut |child, r| {
            n += 1;
            assert!(child < 3);
            assert!(r.members() >= 1);
            true
        });
        assert_eq!(n, 5);
        // the record order within a run is the parent's
        let run = store.run_of(0, 3).unwrap();
        let bytes = store.run_bytes(run);
        let recs: Vec<StoredPlace<'_>> = records(&bytes, run.records).collect();
        assert_eq!((recs[0].x, recs[1].x), (1 << 40, 0));
        assert!(recs[0].flip && recs[0].rot == 3);
        assert!(matches!(recs[0].rep, StoredRep::Grid { na: 4, nb: 5, .. }));
        assert_eq!(store.failed(), None);
    }

    /// the codec and the store round-trip every repetition kind, in
    /// memory and in the staging file, the runs grouped by child and
    /// parent, the visits leaving prunable grids of small cells out
    #[test]
    fn the_store_keeps_every_record_by_child_and_parent() {
        let small = vec![true, false, false, false];
        for file in [false, true] {
            let dir = std::env::temp_dir().join(format!("floe-places-{}-{}", std::process::id(), file));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let backing = if file { Backing::File(dir.join(FILE_NAME)) } else { Backing::Memory };
            let w = PlaceWriter::new(4, backing, small.clone(), 10).expect("writer");
            for (parent, places) in sample() {
                w.add_cell(parent, &places);
            }
            w.add_cell(0, &[]);
            assert_eq!(w.stats().0, 3);
            let store = w.finish().expect("finish");
            if file {
                assert!(dir.join(FILE_NAME).is_file());
                assert_eq!(std::fs::metadata(dir.join(FILE_NAME)).unwrap().len(), store.total_bytes());
            }
            check_store(&store, file);
            let path = store.path().map(Path::to_path_buf);
            drop(store);
            if let Some(p) = path {
                assert!(!p.exists(), "the staging file is removed with the store");
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// a cell added twice is refused at `finish` (its records would be
    /// counted twice), so is an expected cell never added, and an empty
    /// store answers every query empty
    #[test]
    fn a_cell_added_twice_is_an_error_and_an_empty_store_is_empty() {
        let w = PlaceWriter::new(2, Backing::Memory, vec![false; 2], 10).unwrap();
        w.add_cell(1, &[rec(0, 0, 0, 0, false, Rep::One)]);
        w.add_cell(1, &[rec(0, 0, 0, 0, false, Rep::One)]);
        assert!(w.finish().is_err());
        let w = PlaceWriter::new(3, Backing::Memory, vec![false; 3], 10).unwrap();
        w.expect(vec![false, true, true]);
        w.add_cell(1, &[rec(0, 0, 0, 0, false, Rep::One)]);
        assert!(w.finish().err().expect("a missing cell is an error").contains("cell 2 was never added"));
        let w = PlaceWriter::new(3, Backing::Memory, vec![false; 3], 10).unwrap();
        w.expect(vec![false, true, true]);
        w.add_cell(1, &[rec(0, 0, 0, 0, false, Rep::One)]);
        // a cell whose records all place cells outside the inventory counts as added
        w.add_cell(2, &[rec(9, 0, 0, 0, false, Rep::One)]);
        assert_eq!(w.finish().ok().expect("a store").total_records(), 1);
        let e = PlaceStore::empty(3);
        assert_eq!(e.n_cells(), 3);
        assert!(e.incoming(1).is_empty() && e.outgoing(1).next().is_none());
        assert_eq!(e.total_records(), 0);
    }

    /// the backing follows the budget's quarter at BYTES_EST a record,
    /// and the environment overrides it
    #[test]
    fn the_backing_follows_the_budget() {
        let dir = Path::new("/x");
        std::env::remove_var(ENV_BACKING);
        assert_eq!(Backing::choose(1_000, 64 << 10, dir), Backing::Memory);
        assert_eq!(Backing::choose(1_000_000, 1 << 20, dir), Backing::File(dir.join(FILE_NAME)));
        // 796 M placements under a 16 GB laptop's default budget (8 GiB)
        assert!(Backing::choose(796_000_000, 8 << 30, dir).is_file());
        // and 82.6 M (p10) fit it in memory
        assert_eq!(Backing::choose(82_600_000, 8 << 30, dir), Backing::Memory);
        std::env::set_var(ENV_BACKING, "file");
        assert!(Backing::choose(1, u64::MAX, dir).is_file());
        std::env::set_var(ENV_BACKING, "mem");
        assert_eq!(Backing::choose(u64::MAX, 1, dir), Backing::Memory);
        std::env::remove_var(ENV_BACKING);
    }

    /// the varints: zigzag both ends of i64, u64's top
    #[test]
    fn the_varints_round_trip_the_extremes() {
        for v in [0i64, 1, -1, 63, -64, 64, -65, i64::MAX, i64::MIN, 1 << 40, -(1 << 40)] {
            let mut b = Vec::new();
            put_svar(&mut b, v);
            let mut at = 0;
            assert_eq!(get_svar(&b, &mut at), v);
            assert_eq!(at, b.len());
        }
        for v in [0u64, 127, 128, u64::MAX, 1 << 63] {
            let mut b = Vec::new();
            put_uvar(&mut b, v);
            let mut at = 0;
            assert_eq!(get_uvar(&b, &mut at), v);
        }
    }
}
