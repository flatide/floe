//! The streaming index build's cell store (docs/STREAMING_INDEX_PLAN.ko.md
//! §4, S4): the second pass. Pass 1 is the inventory
//! (`floe_oasis::inventory`, S1); this store owns it and a pool of
//! readers that `pread` the file's cell ranges in file order, parse
//! each range with the Doc parser's own grammar (`parse_cell_bytes`,
//! S3), join a cell's pieces once every range defining it is parsed,
//! normalize the cell on its own (`normalize_cells`) and hand it to the
//! planners through `CellStore::cell` (S2). Cells enter by a BYTE budget:
//! a range is read only while the inventory's estimated Doc bytes of
//! the cells resident - being parsed, planned, or committed but not yet
//! encoded - plus the range's own fit the budget (a single range always
//! enters an empty store). The ordered committer gives cells back with
//! `release` after the chunk holding their pages is written, and a reader
//! blocked on the budget asks it to flush early through `flush_wanted`
//! (otherwise a full budget of committed cells and a count window that
//! has not filled would wait for each other). A reader's parse error
//! reaches the committer through `failed` and every waiting planner
//! through a panic, so the build aborts instead of hanging.
//!
//! Output bytes are the in-memory build's (gate G3): the cell table,
//! layer order and pre-passes come from the inventory, the records from
//! the same parser and normalization, the planner / committer / encoder
//! are the ones the Doc store drives, and the commit order is the file
//! order either way.

use crate::cellstore::{CellRef, CellStore};
use floe_oasis::doc::{normalize_cells, parse_cell_bytes, Cell, CellTables, PlaceRec};
use floe_oasis::inventory::Inventory;
use std::collections::HashMap;
use std::ops::Range;
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Condvar, Mutex};

/// one byte range the readers take in file order
struct RangeJob {
    start: u64,
    end: u64,
    cellname_base: u64,
    textstring_base: u64,
    /// the cells this range defines first (charged to the budget when
    /// the range is admitted)
    charges: Vec<usize>,
    cost: u64,
}

enum Slot {
    /// pieces parsed so far (range index, records) and the ranges still
    /// to come
    Pending {
        pieces: Vec<(usize, Cell)>,
        remaining: u32,
    },
    Ready(Arc<Cell>),
    Released,
}

/// the placements of the cells released so far, for the occupancy pass
/// (STREAMING_INDEX_PLAN §4.6): kept up to `cap` records in all, dropped
/// past it (the pass is then skipped)
struct KeptPlaces {
    cap: u64,
    total: u64,
    overflow: bool,
    by_cell: Vec<Option<Vec<PlaceRec>>>,
}

struct State {
    slots: Vec<Slot>,
    keep: Option<KeptPlaces>,
    /// estimated Doc bytes of the cells charged and not yet released
    resident: u64,
    next_range: usize,
    ranges_done: usize,
    /// a reader waits for the budget: the committer should flush
    budget_blocked: bool,
    failed: Option<String>,
    stop: bool,
}

pub struct StreamStore {
    inv: Inventory,
    cell_index: HashMap<String, usize>,
    file: std::fs::File,
    ranges: Vec<RangeJob>,
    budget: u64,
    /// the smallest budget this file streams under: the largest cell, or
    /// the largest span of ranges a cell defined in several places holds
    /// resident (what `new` refused below)
    min_budget: u64,
    st: Mutex<State>,
    cv: Condvar,
}

/// why `StreamStore::new` refused the file
#[derive(Debug)]
pub enum NewError {
    /// the file cannot be opened
    Open(String),
    /// a cell, or the span of cells a multi-definition holds resident,
    /// does not fit the budget (STREAMING_INDEX_PLAN §5): the memory
    /// refusal, ended with crate::guard::EXIT_MEMORY
    Budget(String),
    /// the inventory contradicts itself (a bug, or a file that changed)
    Inventory(String),
}

impl std::fmt::Display for NewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NewError::Open(s) | NewError::Budget(s) | NewError::Inventory(s) => f.write_str(s),
        }
    }
}

/// what `StreamStore::new` found about the file
pub struct StreamPlan {
    pub ranges: usize,
    pub largest_cell_bytes: u64,
    pub largest_cell: String,
}

impl StreamStore {
    /// `budget`: resident Doc bytes (the inventory's estimate) the
    /// readers may admit; a cell larger than it is refused before any
    /// output is touched (STREAMING_INDEX_PLAN §5).
    pub fn new(inv: Inventory, path: &str, budget: u64) -> Result<(StreamStore, StreamPlan), NewError> {
        let file = std::fs::File::open(path)
            .map_err(|e| NewError::Open(format!("open {}: {}", path, e)))?;
        let n = inv.cells.len();
        let (mut largest, mut largest_name) = (0u64, String::new());
        for c in &inv.cells {
            if c.doc_bytes > largest {
                largest = c.doc_bytes;
                largest_name = c.name.clone();
            }
        }
        if largest > budget {
            return Err(NewError::Budget(format!(
                "cell {} needs about {} resident, the budget is {} (--mem): a cell larger \
                 than the budget needs intra-cell splitting, which is not implemented \
                 (STREAMING_INDEX_PLAN §4.7); raise --mem if the host allows",
                largest_name,
                crate::fmt_size(largest),
                crate::fmt_size(budget)
            )));
        }
        // ranges in file order: the head first, then every cell's
        // definitions; a cell is charged to the budget by the range that
        // defines it first
        let head_start = inv.head.start;
        let mut ranges: Vec<RangeJob> = Vec::with_capacity(1 + n);
        ranges.push(RangeJob {
            start: inv.head.start,
            end: inv.head.end,
            cellname_base: inv.head.cellname_base,
            textstring_base: inv.head.textstring_base,
            charges: Vec::new(),
            cost: 0,
        });
        for c in &inv.cells {
            for r in &c.ranges {
                ranges.push(RangeJob {
                    start: r.start,
                    end: r.end,
                    cellname_base: r.cellname_base,
                    textstring_base: r.textstring_base,
                    charges: Vec::new(),
                    cost: 0,
                });
            }
        }
        ranges.sort_by_key(|r| r.start);
        let mut by_start: HashMap<u64, usize> = HashMap::with_capacity(ranges.len());
        for (i, r) in ranges.iter().enumerate() {
            by_start.insert(r.start, i);
        }
        // a cell defined by CELL records in several ranges completes only
        // when its last range is read, and the committer cannot pass it
        // before: every cell first defined between its ranges stays
        // resident until then, so that span must fit the budget too
        let mut spans: Vec<(usize, usize, usize)> = Vec::new();
        let mut slots: Vec<Slot> = Vec::with_capacity(n);
        for (ci, c) in inv.cells.iter().enumerate() {
            if c.def_ranges.is_empty() {
                // a placement-only target: an empty cell, resident from
                // the start (it costs nothing)
                slots.push(Self::empty_slot(&c.name));
                continue;
            }
            let first = c.def_ranges[0];
            let ri = match by_start.get(&first) {
                Some(&i) => i,
                None => {
                    if first != head_start {
                        return Err(NewError::Inventory(format!(
                            "inventory: cell {} defined at offset {} outside every range",
                            c.name, first
                        )));
                    }
                    0
                }
            };
            ranges[ri].charges.push(ci);
            ranges[ri].cost += c.doc_bytes;
            if c.def_ranges.len() > 1 {
                let last = c.def_ranges[c.def_ranges.len() - 1];
                let li = by_start.get(&last).copied().unwrap_or(0);
                spans.push((ci, ri, li.max(ri)));
            }
            slots.push(Slot::Pending {
                pieces: Vec::new(),
                remaining: c.def_ranges.len() as u32,
            });
        }
        let mut min_budget = largest;
        for (ci, first, last) in spans {
            let span: u64 = ranges[first..=last].iter().map(|r| r.cost).sum();
            min_budget = min_budget.max(span);
            if span > budget {
                return Err(NewError::Budget(format!(
                    "cell {} is defined in several places of the file; the cells between \
                     its definitions need about {} resident together, the budget is {} \
                     (--mem): raise --mem if the host allows",
                    inv.cells[ci].name,
                    crate::fmt_size(span),
                    crate::fmt_size(budget)
                )));
            }
        }
        let plan = StreamPlan {
            ranges: ranges.len(),
            largest_cell_bytes: largest,
            largest_cell: largest_name,
        };
        let cell_index = inv.cell_index();
        Ok((
            StreamStore {
                inv,
                cell_index,
                file,
                ranges,
                budget,
                min_budget,
                st: Mutex::new(State {
                    slots,
                    keep: None,
                    resident: 0,
                    next_range: 0,
                    ranges_done: 0,
                    budget_blocked: false,
                    failed: None,
                    stop: false,
                }),
                cv: Condvar::new(),
            },
            plan,
        ))
    }

    /// Run `f` with `readers` reader threads parsing the file's ranges
    /// in file order behind the budget; the readers stop when `f`
    /// returns (or panics - the waiters are woken first so the scope can
    /// join).
    pub fn with_readers<R>(&self, readers: usize, f: impl FnOnce() -> R) -> R {
        let readers = readers.max(1).min(self.ranges.len().max(1));
        std::thread::scope(|s| {
            for _ in 0..readers {
                s.spawn(move || self.reader());
            }
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
            {
                let mut st = self.lock();
                st.stop = true;
                self.cv.notify_all();
            }
            match out {
                Ok(v) => v,
                Err(e) => std::panic::resume_unwind(e),
            }
        })
    }

    fn empty_slot(name: &str) -> Slot {
        Slot::Ready(Arc::new(Cell {
            name: name.to_string(),
            ..Default::default()
        }))
    }

    pub fn inventory(&self) -> &Inventory {
        &self.inv
    }

    /// keep every released cell's placements (for the occupancy pass),
    /// up to `cap` placement records in all; call before the readers run
    pub fn keep_places(&self, cap: u64) {
        let n = self.inv.cells.len();
        self.lock().keep = Some(KeptPlaces {
            cap,
            total: 0,
            overflow: false,
            by_cell: (0..n).map(|_| None).collect(),
        });
    }

    /// the kept placements per cell, or why the occupancy pass cannot
    /// run: the record count overflowed the cap, or a cell was never
    /// released by the build (its placements were never seen)
    pub fn take_places(&self) -> Result<Vec<Vec<PlaceRec>>, String> {
        let mut st = self.lock();
        match st.keep.take() {
            None => Err("no placements were kept".to_string()),
            Some(k) if k.overflow => Err(format!(
                "{} placement records exceed the {} the occupancy pass keeps",
                k.total, k.cap
            )),
            Some(k) => {
                if let Some(ci) = k.by_cell.iter().position(|p| p.is_none()) {
                    return Err(format!(
                        "cell {} ({}) was never released by the build",
                        ci, self.inv.cells[ci].name
                    ));
                }
                Ok(k.by_cell.into_iter().map(|p| p.unwrap_or_default()).collect())
            }
        }
    }

    /// the smallest budget this file streams under (the largest cell or
    /// multi-definition span)
    pub fn min_budget(&self) -> u64 {
        self.min_budget
    }

    /// another pass over the file under `budget` (at least `min_budget`):
    /// every cell pending again, nothing resident (no readers may be
    /// running)
    pub fn reset(&mut self, budget: u64) {
        self.budget = budget.max(self.min_budget);
        let mut slots: Vec<Slot> = Vec::with_capacity(self.inv.cells.len());
        for c in &self.inv.cells {
            if c.def_ranges.is_empty() {
                slots.push(Self::empty_slot(&c.name));
            } else {
                slots.push(Slot::Pending {
                    pieces: Vec::new(),
                    remaining: c.def_ranges.len() as u32,
                });
            }
        }
        // in place: the condition variable is bound to this mutex
        *self.lock() = State {
            slots,
            keep: None,
            resident: 0,
            next_range: 0,
            ranges_done: 0,
            budget_blocked: false,
            failed: None,
            stop: false,
        };
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // a planner that panics inside `cell` (a reader failure) poisons
        // the lock; the state is still consistent for the paths that
        // run during an abort
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fail(&self, msg: String) {
        let mut st = self.lock();
        if st.failed.is_none() {
            st.failed = Some(msg);
        }
        self.cv.notify_all();
    }

    /// one reader: claim the next range (the budget permitting), read
    /// and parse it, file its pieces, finish the cells it completes
    fn reader(&self) {
        loop {
            let ri = {
                let mut st = self.lock();
                loop {
                    if st.stop || st.failed.is_some() || st.next_range >= self.ranges.len() {
                        return;
                    }
                    let cost = self.ranges[st.next_range].cost;
                    if st.resident == 0 || st.resident.saturating_add(cost) <= self.budget {
                        break;
                    }
                    st.budget_blocked = true;
                    st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
                }
                st.budget_blocked = false;
                let ri = st.next_range;
                st.next_range += 1;
                st.resident = st.resident.saturating_add(self.ranges[ri].cost);
                ri
            };
            let r = &self.ranges[ri];
            let len = (r.end - r.start) as usize;
            let mut buf = vec![0u8; len];
            if let Err(e) = self.file.read_exact_at(&mut buf, r.start) {
                self.fail(format!("read {}..{}: {}", r.start, r.end, e));
                return;
            }
            let tables = CellTables::new(&self.inv.cellnames, &self.inv.textstrings, &self.cell_index);
            let pieces = match parse_cell_bytes(&buf, r.start, r.cellname_base, r.textstring_base, &tables) {
                Ok(p) => p,
                Err(e) => {
                    self.fail(format!("range {}..{}: {}", r.start, r.end, e));
                    return;
                }
            };
            drop(buf);
            // file the pieces; a cell whose every defining range is parsed
            // is complete - its join and normalization run outside the lock
            let mut complete: Vec<(usize, Vec<(usize, Cell)>)> = Vec::new();
            {
                let mut st = self.lock();
                let mut seen: Vec<usize> = Vec::new();
                for p in pieces {
                    let ci = p.index;
                    match &mut st.slots[ci] {
                        Slot::Pending { pieces, remaining } => {
                            pieces.push((ri, p.cell));
                            if !seen.contains(&ci) {
                                seen.push(ci);
                                *remaining = remaining.saturating_sub(1);
                                if *remaining == 0 {
                                    let taken = std::mem::take(pieces);
                                    complete.push((ci, taken));
                                }
                            }
                        }
                        Slot::Ready(_) | Slot::Released => {
                            drop(st);
                            self.fail(format!(
                                "range {}..{} defines cell {} again after it was complete",
                                r.start, r.end, self.inv.cells[ci].name
                            ));
                            return;
                        }
                    }
                }
                st.ranges_done += 1;
            }
            for (ci, mut pieces) in complete {
                pieces.sort_by_key(|(ri, _)| *ri);
                let mut cell = Cell {
                    name: self.inv.cells[ci].name.clone(),
                    ..Default::default()
                };
                for (_, p) in pieces {
                    cell.rects.extend(p.rects);
                    cell.polys.extend(p.polys);
                    cell.paths.extend(p.paths);
                    cell.places.extend(p.places);
                    cell.texts.extend(p.texts);
                }
                normalize_cells(std::slice::from_mut(&mut cell), 1);
                let mut st = self.lock();
                st.slots[ci] = Slot::Ready(Arc::new(cell));
                self.cv.notify_all();
            }
        }
    }

    /// resident estimate and the ranges read so far (the heartbeat)
    pub fn progress(&self) -> (u64, usize, usize) {
        let st = self.lock();
        (st.resident, st.ranges_done, self.ranges.len())
    }
}

impl CellStore for StreamStore {
    fn n_cells(&self) -> usize {
        self.inv.cells.len()
    }
    fn unit(&self) -> f64 {
        self.inv.unit()
    }
    fn top(&self) -> usize {
        self.inv.top
    }
    fn layer_order(&self) -> &[(u32, u32)] {
        &self.inv.layer_order
    }
    fn layer_names(&self) -> &HashMap<(u32, u32), String> {
        &self.inv.layer_names
    }
    fn layer_aliases(&self) -> &HashMap<(u32, u32), Vec<String>> {
        &self.inv.layer_aliases
    }
    fn cell_name(&self, ci: usize) -> &str {
        &self.inv.cells[ci].name
    }
    fn cell(&self, ci: usize) -> CellRef<'_> {
        let mut st = self.lock();
        loop {
            match &st.slots[ci] {
                Slot::Ready(c) => return CellRef::Shared(c.clone()),
                Slot::Released => panic!(
                    "streaming store: cell {} ({}) asked for after its release",
                    ci, self.inv.cells[ci].name
                ),
                Slot::Pending { .. } => {}
            }
            if let Some(e) = &st.failed {
                panic!("streaming store: the source reader failed: {}", e);
            }
            if st.stop {
                panic!("streaming store: stopped while cell {} was awaited", ci);
            }
            st = self.cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }
    fn release(&self, cells: Range<usize>) {
        let mut st = self.lock();
        let st = &mut *st;
        let mut freed = 0u64;
        for ci in cells {
            if !matches!(st.slots[ci], Slot::Released) {
                if let (Slot::Ready(c), Some(k)) = (&st.slots[ci], &mut st.keep) {
                    if !k.overflow {
                        k.total = k.total.saturating_add(c.places.len() as u64);
                        if k.total > k.cap {
                            k.overflow = true;
                            k.by_cell.clear();
                        } else {
                            k.by_cell[ci] = Some(c.places.clone());
                        }
                    }
                }
                st.slots[ci] = Slot::Released;
                freed = freed.saturating_add(self.inv.cells[ci].doc_bytes);
            }
        }
        // (a placement-only cell was never charged: its doc_bytes is 0)
        st.resident = st.resident.saturating_sub(freed);
        st.budget_blocked = false;
        self.cv.notify_all();
    }
    fn flush_wanted(&self) -> bool {
        self.lock().budget_blocked
    }
    fn failed(&self) -> Option<String> {
        self.lock().failed.clone()
    }
    fn abort(&self, why: &str) {
        self.fail(format!("build aborted: {}", why));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepass::PrePass;
    use floe_oasis::doc::parse_doc_parallel_phased;
    use floe_oasis::inventory::fixtures;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("floe-stream-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("tmpdir");
        d
    }

    fn file_bytes(p: &std::path::Path) -> Vec<u8> {
        std::fs::read(p).unwrap_or_default()
    }

    /// the in-memory build and the streaming build of one stream, bytes
    /// against bytes: design.ovm (returned), design.ovp / ovt / ovb
    /// (written), the recursive boxes and layer members handed on
    fn build_both(data: &[u8], tag: &str, jobs: usize, plan_batch: usize, tight: bool) {
        let dir = tmpdir(tag);
        let src = dir.join("src.oas");
        std::fs::write(&src, data).expect("write src");
        let meta = std::fs::metadata(&src).expect("meta");
        let size = meta.len();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let out_a = dir.join("mem");
        let out_b = dir.join("stream");
        std::fs::create_dir_all(&out_a).unwrap();
        std::fs::create_dir_all(&out_b).unwrap();
        let doc = parse_doc_parallel_phased(data, jobs, |_| {}).expect("parse");
        let pre = PrePass::from_doc(&doc, jobs);
        let (ovm_a, rbb_a, lmems_a, _) = crate::vfs::build(
            &doc,
            pre,
            std::time::Instant::now(),
            size,
            mtime,
            out_a.to_str().unwrap(),
            jobs,
            4,
            plan_batch,
            64 * 1024,
            true,
            true,
            1e9,
            None,
        );
        let inv = floe_oasis::inventory(data, jobs).expect("inventory");
        let largest = inv.cells.iter().map(|c| c.doc_bytes).max().unwrap_or(0);
        // tight: exactly the largest cell - every other admission waits
        // for a release, so the committer's on-demand flush runs
        let budget = if tight { largest.max(1) } else { u64::MAX / 4 };
        let pre = PrePass::from_inventory(&inv);
        let (store, plan) = StreamStore::new(inv, src.to_str().unwrap(), budget).expect("store");
        assert!(plan.ranges >= 1);
        let (ovm_b, rbb_b, lmems_b, _) = store.with_readers(jobs, || {
            crate::vfs::build(
                &store,
                pre,
                std::time::Instant::now(),
                size,
                mtime,
                out_b.to_str().unwrap(),
                jobs,
                4,
                plan_batch,
                64 * 1024,
                true,
                true,
                1e9,
                None,
            )
        });
        let (resident, done, total) = store.progress();
        assert_eq!(done, total, "{}: every range read", tag);
        assert_eq!(resident, 0, "{}: everything released", tag);
        assert_eq!(rbb_a, rbb_b, "{} jobs {}: rbb", tag, jobs);
        assert_eq!(lmems_a, lmems_b, "{} jobs {}: lmems", tag, jobs);
        assert!(ovm_a == ovm_b, "{} jobs {}: design.ovm bytes differ", tag, jobs);
        for f in ["design.ovp", "design.ovt", "design.ovb"] {
            // (design.ovt is legitimately empty for a text-free stream)
            assert!(out_a.join(f).is_file() && out_b.join(f).is_file(), "{}: {} missing", tag, f);
            let a = file_bytes(&out_a.join(f));
            let b = file_bytes(&out_b.join(f));
            assert!(a == b, "{} jobs {}: {} bytes differ", tag, jobs, f);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// G3 at unit scale: the adversarial streams (late and early tables,
    /// CBLOCK bodies, a cell in two pieces, a cell re-opened in its own
    /// range, a cell defined in the head's CBLOCK) through both builds at
    /// jobs 1, 2 and 4, with the budget at exactly the largest cell
    /// (admission waits, on-demand flushes) and wide open
    #[test]
    fn streaming_build_matches_in_memory() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("late", fixtures::adversarial(false, false)),
            ("early-cblock", fixtures::adversarial(true, true)),
            ("dup", fixtures::dup_in_range()),
            ("head", fixtures::head_cblock_cell()),
        ];
        for (tag, data) in &cases {
            for jobs in [1usize, 2, 4] {
                build_both(data, &format!("{}-j{}", tag, jobs), jobs, if jobs == 1 { 1 } else { 8 }, true);
            }
        }
        build_both(&cases[0].1, "late-wide", 2, 2, false);
    }


    /// design.ovo (S5): the streaming occupancy against the in-memory
    /// one, byte for byte - prune on and off, a coarse and a fine cell,
    /// the planes in one group and in groups small enough to need several
    /// passes over the file
    #[test]
    fn streaming_occupancy_matches_in_memory() {
        use floe_oasis::doc::{PathRec, PolyRec, RectRec, Rep, TextRec};
        use floe_oasis::write::{write_tree, WCell};
        use floe_vfs::occupancy as occ;
        let leaf_rects = vec![
            RectRec { layer: 1, dt: 0, x: 0, y: 0, w: 300, h: 300, rep: Rep::One },
            RectRec { layer: 2, dt: 0, x: 50, y: 50, w: 40, h: 40, rep: Rep::Grid { na: 3, nb: 3, va: (60, 0), vb: (0, 60) } },
        ];
        let leaf = WCell { name: "LEAF".to_string(), rects: &leaf_rects, polys: &[], paths: &[], texts: &[], places: vec![] };
        let tiny_rects = vec![RectRec { layer: 3, dt: 0, x: 0, y: 0, w: 20, h: 20, rep: Rep::One }];
        let tiny = WCell { name: "TINY".to_string(), rects: &tiny_rects, polys: &[], paths: &[], texts: &[], places: vec![] };
        let pts = Rep::Pts(vec![(0, 0), (5000, 100), (-200, 2500), (9100, 7600)].into());
        let dense = Rep::Grid { na: 40, nb: 30, va: (200, 0), vb: (0, 200) };
        let sparse = Rep::Grid { na: 4, nb: 3, va: (5000, 0), vb: (0, 4000) };
        let one = Rep::One;
        let mid_rects = vec![
            RectRec { layer: 1, dt: 0, x: -100, y: -100, w: 12000, h: 50, rep: Rep::One },
            RectRec { layer: 4, dt: 1, x: 0, y: 0, w: 90, h: 90, rep: Rep::Grid { na: 20, nb: 20, va: (100, 0), vb: (0, 100) } },
            RectRec { layer: 4, dt: 1, x: 3000, y: 3000, w: 90, h: 90, rep: Rep::Grid { na: 5, nb: 5, va: (900, 0), vb: (0, 900) } },
        ];
        let mid_polys = vec![PolyRec { layer: 5, dt: 0, pts: vec![(100, 100), (4100, 300), (2600, 3900)], rep: Rep::One }];
        let mid_paths = vec![PathRec { layer: 5, dt: 0, pts: vec![(0, 6000), (7000, 6000), (7000, 9000)], hw: 150, es: 0, ee: 150, rep: Rep::One }];
        let mid_texts = vec![TextRec { layer: 1, dt: 0, x: 10, y: 10, rep: Rep::One, s: "t".to_string() }];
        let mid = WCell {
            name: "MID".to_string(),
            rects: &mid_rects,
            polys: &mid_polys,
            paths: &mid_paths,
            texts: &mid_texts,
            places: vec![
                ("TINY", 100, 100, 0, false, &dense),
                ("TINY", 200, 300, 1, true, &sparse),
                ("LEAF", 8000, 0, 2, false, &pts),
                ("LEAF", 0, 8000, 3, true, &one),
            ],
        };
        let top_rects = vec![RectRec { layer: 1, dt: 0, x: 0, y: 0, w: 60000, h: 50, rep: Rep::One }];
        let top = WCell {
            name: "TOP".to_string(),
            rects: &top_rects,
            polys: &[],
            paths: &[],
            texts: &[],
            places: vec![
                ("MID", 0, 0, 0, false, &one),
                ("MID", 30000, 0, 1, false, &Rep::Grid { na: 2, nb: 2, va: (14000, 0), vb: (0, 14000) }),
                ("LEAF", 20000, 40000, 0, true, &one),
            ],
        };
        let mut cases: Vec<(&str, Vec<u8>)> = vec![
            ("tree", write_tree(&[tiny, leaf, mid, top], 1000.0).expect("write_tree")),
            ("adv", fixtures::adversarial(false, true)),
            ("head", fixtures::head_cblock_cell()),
        ];
        for (tag, data) in cases.drain(..) {
            let dir = tmpdir(&format!("ovo-{}", tag));
            let src = dir.join("src.oas");
            std::fs::write(&src, &data).unwrap();
            let doc = parse_doc_parallel_phased(&data, 2, |_| {}).expect("parse");
            // (prune, base cell, plane budget, flat-instance budget): one
            // pass or one per layer; instances materialized or enumerated
            // through the parents
            for (prune, base_um, plane_budget, flat_budget) in [(true, 1.0, u64::MAX, 0), (false, 1.0, 1, u64::MAX), (true, 0.25, 1, 0), (false, 0.25, u64::MAX, u64::MAX)] {
                let opts = occ::Opts { base_um, jobs: 2, prune, progress: None, ..Default::default() };
                let reference_occ = occ::build(&doc, 11, 22, &opts).expect("in-memory occupancy");
                let ok_layers = reference_occ.layers.iter().filter(|l| l.status == occ::STATUS_OK).count();
                if tag == "tree" {
                    assert!(ok_layers >= 4, "tree prune {} base {} um: only {} layers marked", prune, base_um, ok_layers);
                }
                let reference = occ::write_ovo(&reference_occ);
                // the streaming side: the main pass keeps the placements,
                // the occupancy passes re-read the file per layer group
                let inv = floe_oasis::inventory(&data, 2).expect("inventory");
                let pre = PrePass::from_inventory(&inv);
                let (mut store, _) = StreamStore::new(inv, src.to_str().unwrap(), u64::MAX / 4).expect("store");
                store.keep_places(1 << 20);
                let out = dir.join(format!("out-{}-{}", prune, base_um));
                std::fs::create_dir_all(&out).unwrap();
                store.with_readers(2, || {
                    crate::vfs::build(&store, pre, std::time::Instant::now(), 11, 22, out.to_str().unwrap(), 2, 4, 8, 64 * 1024, true, false, 1e9, None)
                });
                let places = store.take_places().expect("placements kept");
                let (n, top_ci, mut built, topo) = {
                    let inv = store.inventory();
                    let children = inv.children();
                    let dsmask: Vec<Vec<u8>> = inv.cells.iter().map(|c| c.dsmask.clone()).collect();
                    let built = occ::StreamOcc::new(inv.unit(), inv.top, &inv.cells[inv.top].name, &inv.layer_order, &inv.rbb_notext, &dsmask, &inv.topo, &children, 11, 22, &opts).expect("stream occupancy");
                    (inv.cells.len(), inv.top, built, inv.topo.clone())
                };
                let instances = occ::Instances::with_flat_budget(top_ci, &places, built.small(), &built.has_any(), &topo, built.cell_dbu(), flat_budget);
                assert!(instances.total() > 0, "{}: the walk visits something", tag);
                if tag == "tree" && prune {
                    assert!(built.small().iter().any(|&s| s), "tree: the pruned walk must have small cells");
                }
                if flat_budget == 0 {
                    assert_eq!(instances.flat_cells(), 0, "{}: nothing materialized under a zero budget", tag);
                }
                let mut passes = 0;
                while let Some(group) = built.begin_group(plane_budget) {
                    passes += 1;
                    store.reset(u64::MAX / 4);
                    let st: &StreamStore = &store;
                    let (g, b, inst) = (&group, &built, &instances);
                    let scratches: Vec<occ::StreamScratch> = st.with_readers(2, || {
                        let next = std::sync::atomic::AtomicUsize::new(0);
                        std::thread::scope(|s| {
                            let next = &next;
                            let hs: Vec<_> = (0..2).map(|_| s.spawn(move || {
                                let mut sc = b.scratch(g);
                                loop {
                                    let ci = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    if ci >= n { break; }
                                    { let cell = st.cell(ci); b.mark_cell(g, &cell, ci, inst, &mut sc); }
                                    st.release(ci..ci + 1);
                                }
                                sc
                            })).collect();
                            hs.into_iter().map(|h| h.join().unwrap()).collect()
                        })
                    });
                    built.end_group(group, scratches);
                }
                drop(instances);
                let streamed = occ::write_ovo(&built.finish());
                assert!(streamed == reference, "{} prune {} base {} um flat {}: design.ovo differs ({} passes)", tag, prune, base_um, flat_budget, passes);
                if plane_budget == 1 && tag == "tree" {
                    assert!(passes > 1, "{}: the plane budget of 1 byte must force several passes", tag);
                }
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// a corrupt range (the file changed under the inventory): the reader
    /// fails, every planner parked in the store wakes, the committer stops,
    /// build() returns through its panic and the store names the cause -
    /// within a watchdog, not a hang
    #[test]
    fn reader_failure_aborts_the_build() {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(180));
            eprintln!("watchdog: the streaming failure test hung");
            std::process::abort();
        });
        let data = fixtures::adversarial(false, true);
        let dir = tmpdir("fail");
        let src = dir.join("src.oas");
        std::fs::write(&src, &data).unwrap();
        let inv = floe_oasis::inventory(&data, 1).expect("inventory");
        // the second cell's range starts with its CELL record: an unknown
        // record id there fails the parse of that range only
        let at = inv.cells[1].ranges[0].start as usize;
        let mut bad = data.clone();
        bad[at] = 99;
        std::fs::write(&src, &bad).unwrap();
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let pre = PrePass::from_inventory(&inv);
        let (store, _) = StreamStore::new(inv, src.to_str().unwrap(), u64::MAX / 4).expect("store");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.with_readers(2, || {
                crate::vfs::build(
                    &store,
                    pre,
                    std::time::Instant::now(),
                    bad.len() as u64,
                    0,
                    out.to_str().unwrap(),
                    2,
                    4,
                    8,
                    64 * 1024,
                    true,
                    true,
                    1e9,
                    None,
                )
            })
        }));
        assert!(result.is_err(), "the build must not complete on a corrupt range");
        let why = store.failed().expect("the store names the failure");
        assert!(why.contains("range") && why.contains("unknown record"), "{}", why);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// a cell defined in two places with other cells between them: the span
    /// must fit the budget, or the build would wait forever - refused up front
    #[test]
    fn span_above_budget_is_refused() {
        let data = fixtures::adversarial(false, false);
        let dir = tmpdir("span");
        let src = dir.join("src.oas");
        std::fs::write(&src, &data).unwrap();
        let inv = floe_oasis::inventory(&data, 1).expect("inventory");
        // LEAF (index 2) is defined before LBL (3) and again after it
        assert_eq!(inv.cells[2].def_ranges.len(), 2);
        let span = inv.cells[2].doc_bytes + inv.cells[3].doc_bytes;
        let largest = inv.cells.iter().map(|c| c.doc_bytes).max().unwrap();
        if largest < span {
            let e = StreamStore::new(inv, src.to_str().unwrap(), span - 1).err().expect("refused");
            assert!(matches!(e, NewError::Budget(_)), "{}", e);
            let e = e.to_string();
            assert!(e.contains("several places"), "{}", e);
        } else {
            // the largest cell already covers the span: admissible
            let (store, _) = StreamStore::new(inv, src.to_str().unwrap(), largest).expect("store");
            drop(store);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// a budget below the largest cell is refused before anything is built
    #[test]
    fn budget_below_largest_cell_is_refused() {
        let data = fixtures::adversarial(false, false);
        let dir = tmpdir("refuse");
        let src = dir.join("src.oas");
        std::fs::write(&src, &data).unwrap();
        let inv = floe_oasis::inventory(&data, 1).expect("inventory");
        let largest = inv.cells.iter().map(|c| c.doc_bytes).max().unwrap();
        let e = StreamStore::new(inv, src.to_str().unwrap(), largest - 1).err().expect("refused");
        assert!(matches!(e, NewError::Budget(_)), "{}", e);
        let e = e.to_string();
        assert!(e.contains("--mem"), "{}", e);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
