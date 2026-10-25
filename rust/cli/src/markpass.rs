//! The streaming occupancy pass's marking threads (STREAMING_INDEX_PLAN
//! §4.6, 0.12.387; DIRECT_READ_PLAN §8.10, L7b). For one layer group,
//! `jobs` threads take the cells in file order and mark each at every
//! instance (`StreamOcc::mark_cell`), parsing it themselves right before
//! (`StreamStore::parse_cell`) and letting it go right after - no reader
//! pool running ahead, no window of resident cells waiting for a marker,
//! no handoff: the resident memory is the cells being marked. A cell
//! enters while the estimated Doc bytes of the cells in flight plus its
//! own fit the budget (a lone cell always enters), so `--mem` still
//! bounds the pass; under the default budget the threads run full
//! (field 2026-10-20: `--mem 0.5` on a 50 GB source left two cells
//! resident between twelve readers and twelve markers, 1,766 s nearly
//! serial). What the group does not need is not read: a cell the walk
//! never visits or without a layer of the group below it, a small cell
//! (its subtree is one bbox) and a cell whose own records hold none of
//! the group's layers (its placements' charges come from the placement
//! store, floe_vfs::places) - so a layout's later groups parse only the
//! cells with records on their layers. A cell is parsed for its shapes
//! alone (`StreamStore::parse_cell_shapes`, 0.12.399): its PLACEMENT and
//! TEXT records are read for the modal state and not materialized - the
//! store holds the placements - and it is charged its shapes' estimate
//! (`cell_shape_bytes`); on the MAIN01-like chip the parsed big cells
//! took about three times their Doc estimate, half of it placements.
//!
//! Cells whose definitions share ranges - CELL records inside a CBLOCK of
//! another cell's range, or a cell defined in several ranges that define
//! others too - are a set (`StreamStore::shared_def_of`; 0.12.391,
//! generalized 0.12.392, walked 0.12.393): the marker that reaches the
//! first of them takes the set in progress - its cells claimed, the ones
//! the group parses pending, the others marked from what is at hand - and
//! the markers then take the set's ranges in file order, one at a time as
//! the budget admits them (`Task::Range`, `StreamStore::parse_range`),
//! each parsed once; a range's charge is the cells it defines first and a
//! cell is complete - joined, marked, released - when its last range is
//! filed, as the reader pool does, so the resident pieces are the cells
//! open between their definitions: the file's multi-definition span
//! (`StreamStore::min_budget`, which the pass budget is at least). The
//! ranges of every set go out in one file order (`StreamStore::
//! shared_ranges`, 0.12.394), as the reader pool reads the file, so sets
//! are in progress together as the budget allows and the span bound
//! holds across them; a range is parsed for the cells the group needs
//! alone - the other cells' records are parsed, as the modal state runs
//! through them, and not materialized (`parse_cell_bytes_keeping`).
//! Before, each cell's turn parsed every range of the cell again charging
//! that cell alone (review 2026-10-23: a 663 KiB block of 24 cells read
//! 24 times; the review of 0.12.391: the same when the 24 cells were
//! defined on their own as well); 0.12.392 parsed a set's ranges all at
//! once charging the whole set, which a chain of CBLOCK definitions -
//! every cell's block defining the next, the file one set - held whole
//! past the budget (the review of 0.12.392: a 32-cell chain, 58 MB
//! resident under a 4.6 MB budget); and 0.12.393 materialized a range's
//! every cell and dropped the ones the group did not need, so a range's
//! big cells of another layer, charged nothing, were resident in every
//! marker at once (the review of 0.12.393: 12 markers, 26.9 MB over a
//! 4.8 MB budget that saw 2.9 KB) and ran one set at a time.
//!
//! The marks are the ones the reader-pool pass made (bits are ORs, the
//! charges per operation, the set of operations the walk's), so the
//! bytes are unchanged: gates vfs_stream and light_cache, unit test
//! stream::tests::streaming_occupancy_matches_in_memory.

use crate::stream::StreamStore;
use floe_oasis::doc::Cell;
use floe_vfs::occupancy::{Instances, StreamGroup, StreamOcc, StreamScratch};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

/// what one group's marking did
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MarkReport {
    /// cells the markers parsed (own records on the group's layers)
    pub parsed: usize,
    /// cells marked from their kept placements alone
    pub places_only: usize,
    /// small cells (their subtree bbox; nothing read)
    pub small: usize,
    /// cells the group does not visit
    pub skipped: usize,
    /// of `parsed`, cells completed from the ranges of a set, each parsed
    /// once (`StreamStore::parse_range`), and those ranges
    pub shared: usize,
    pub shared_ranges: usize,
    /// source bytes the markers read
    pub bytes: u64,
    /// the most the admitted cells' Doc estimates were at once (the
    /// resident budget bounds it: a lone cell is at most the largest, a
    /// set's walk at most the multi-definition span)
    pub peak: u64,
    /// records of cells the group does not parse met in a set's ranges:
    /// parsed and not kept (`StreamStore::parse_range`)
    pub dropped: u64,
    /// thread seconds spent parsing / marking, summed over the threads
    pub parse_s: f64,
    pub mark_s: f64,
}

impl MarkReport {
    pub fn add(&mut self, o: &MarkReport) {
        self.parsed += o.parsed;
        self.places_only += o.places_only;
        self.small += o.small;
        self.skipped += o.skipped;
        self.shared += o.shared;
        self.shared_ranges += o.shared_ranges;
        self.bytes += o.bytes;
        self.peak = self.peak.max(o.peak);
        self.dropped += o.dropped;
        self.parse_s += o.parse_s;
        self.mark_s += o.mark_s;
    }
}

/// what a group needs of a cell
#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    /// the walk never visits it, or no layer of the group lies below it
    None,
    /// a small cell: its subtree bbox, nothing read
    Small,
    /// its placements' charges alone (no own record on the group's layers)
    Places,
    /// its own records: parsed
    Shapes,
}

fn need_of(occ: &StreamOcc, group: &StreamGroup, inst: &Instances<'_>, ci: usize) -> Need {
    if inst.count(ci) == 0 || !occ.group_visits(group, ci) {
        return Need::None;
    }
    if occ.small()[ci] {
        return Need::Small;
    }
    if !occ.group_own(group, ci) {
        return Need::Places;
    }
    Need::Shapes
}

struct Admit {
    /// the next cell in cell order (a cell of a set is the cursor's)
    next: usize,
    /// cells claimed (by their own turn, or with their set)
    claimed: Vec<bool>,
    /// estimated Doc bytes of the cells being parsed or marked, and the
    /// most it has been
    resident: u64,
    peak: u64,
    /// the next of the sets' ranges in file order
    /// (`StreamStore::shared_ranges`): the sets' ranges go out in the
    /// file's order, one at a time, as the reader pool reads them, so the
    /// cells open between their definitions - whichever sets they are of -
    /// are within the file's multi-definition span
    cursor: usize,
    /// the sets the cursor has met (their cells claimed then), and the
    /// sets with cells still to parse, mark or release
    activated: Vec<bool>,
    runs: HashMap<usize, SetRun>,
    /// what will release memory without another admission: set ranges
    /// out and not yet filed, set cells complete and being marked, lone
    /// cells being parsed or marked
    in_flight: usize,
    marking: usize,
    lone: usize,
    failed: Option<String>,
}

/// a set of cells whose definitions share ranges (`SharedDef`) with
/// cells the group parses: its ranges go out in file order, a range at a
/// time, parsed for those cells alone, and a cell completes - is joined,
/// normalized, marked and let go - when its last range is filed, as the
/// reader pool does; resident are the pieces of the cells opened and not
/// yet complete, and the cells being marked
struct SetRun {
    /// the cells the group parses, in cell order (a range's parse keeps
    /// their records alone)
    wanted: Arc<Vec<usize>>,
    /// their pieces so far (range position, piece) and the ranges
    /// defining them still to file
    pending: HashMap<usize, Pend>,
    /// ranges of the set not yet filed (read, or skipped as holding no
    /// cell the group parses)
    left: usize,
    /// cells complete and being marked (their bytes still charged)
    marking: usize,
    /// the set's share of `Admit::resident`
    charged: u64,
}

struct Pend {
    pieces: Vec<(usize, Cell)>,
    remaining: u32,
}

impl SetRun {
    fn done(&self) -> bool {
        self.left == 0 && self.pending.is_empty() && self.marking == 0
    }
}

/// what a marker claims at a time
enum Task {
    /// one cell, its need and its charge (its Doc bytes when parsed)
    Cell(usize, Need, u64),
    /// cells of a set the group reads nothing of (claimed with the set):
    /// marked from what is at hand
    Unparsed(Vec<(usize, Need)>),
    /// range `k` of set `si` (charged at the claim: the cells it defines
    /// first that the group parses; released cell by cell) and the cells
    /// the group parses of the set
    Range(usize, usize, Arc<Vec<usize>>),
}

fn fail(adm: &Mutex<Admit>, cv: &Condvar, e: String) {
    let mut a = adm.lock().unwrap_or_else(|p| p.into_inner());
    if a.failed.is_none() {
        a.failed = Some(e);
    }
    cv.notify_all();
}

/// a lone cell let go: its bytes off the budget
fn release(adm: &Mutex<Admit>, cv: &Condvar, cost: u64) {
    let mut a = adm.lock().unwrap_or_else(|p| p.into_inner());
    a.resident = a.resident.saturating_sub(cost);
    a.lone -= 1;
    cv.notify_all();
}

/// a cell marked from what is at hand: nothing (skipped), its subtree
/// bbox, or its kept placements
fn mark_unparsed(
    occ: &StreamOcc,
    group: &StreamGroup,
    inst: &Instances<'_>,
    ci: usize,
    need: Need,
    empty: &Cell,
    sc: &mut StreamScratch,
    rep: &mut MarkReport,
) {
    match need {
        Need::None => rep.skipped += 1,
        Need::Small => {
            let t = Instant::now();
            occ.mark_cell(group, empty, ci, inst, sc);
            rep.mark_s += t.elapsed().as_secs_f64();
            rep.small += 1;
        }
        Need::Places => {
            let t = Instant::now();
            occ.mark_cell(group, empty, ci, inst, sc);
            rep.mark_s += t.elapsed().as_secs_f64();
            rep.places_only += 1;
        }
        Need::Shapes => unreachable!("a parsed cell"),
    }
}

/// a parsed cell marked at every instance
fn mark_parsed(
    occ: &StreamOcc,
    group: &StreamGroup,
    inst: &Instances<'_>,
    ci: usize,
    cell: &Cell,
    sc: &mut StreamScratch,
    rep: &mut MarkReport,
) {
    let t = Instant::now();
    occ.mark_cell(group, cell, ci, inst, sc);
    rep.mark_s += t.elapsed().as_secs_f64();
    rep.parsed += 1;
}

struct StopOnDrop<'a>(&'a AtomicBool);
impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Relaxed);
    }
}

/// the next task of a marker, or None when nothing is left to claim (or
/// a marker failed): the sets' next range in file order when it fits the
/// budget - or when nothing is in flight at all, so nothing but this
/// range frees memory (the sets' open pieces and the range are within the
/// file's multi-definition span, `StreamStore::min_budget`, which the pass
/// budget is at least: within it the case never comes) - a set the cursor
/// meets first claiming its cells (the ones the group parses pending, the
/// others marked from what is at hand); else the next lone cell in cell
/// order by its bytes (a lone cell always enters an empty budget); waits
/// otherwise
#[allow(clippy::too_many_arguments)]
fn claim(
    store: &StreamStore,
    occ: &StreamOcc,
    group: &StreamGroup,
    inst: &Instances<'_>,
    adm: &Mutex<Admit>,
    cv: &Condvar,
    budget: u64,
    n: usize,
) -> Option<Task> {
    let ranges = store.shared_ranges();
    let mut guard = adm.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let a: &mut Admit = &mut guard;
        if a.failed.is_some() {
            return None;
        }
        // 1. the sets' ranges, in file order
        while a.cursor < ranges.len() {
            let (ri, si, k) = ranges[a.cursor];
            let set = store.shared_def(si);
            if !a.activated[si] {
                // the set met: its cells claimed, the ones the group parses
                // pending with the ranges defining them, the others marked
                // from what is at hand by this marker
                a.activated[si] = true;
                let mut unparsed: Vec<(usize, Need)> = Vec::new();
                let mut wanted: Vec<usize> = Vec::new();
                let mut pending: HashMap<usize, Pend> = HashMap::new();
                for &c in &set.cells {
                    if a.claimed[c] {
                        continue;
                    }
                    a.claimed[c] = true;
                    let nd = need_of(occ, group, inst, c);
                    if nd == Need::Shapes {
                        wanted.push(c);
                        pending.insert(c, Pend { pieces: Vec::new(), remaining: store.def_count(c) });
                    } else {
                        unparsed.push((c, nd));
                    }
                }
                if !pending.is_empty() {
                    a.runs.insert(
                        si,
                        SetRun {
                            wanted: Arc::new(wanted),
                            pending,
                            left: set.ranges.len(),
                            marking: 0,
                            charged: 0,
                        },
                    );
                }
                if !unparsed.is_empty() {
                    // (the other markers take the ranges meanwhile)
                    cv.notify_all();
                    return Some(Task::Unparsed(unparsed));
                }
                continue;
            }
            let Some(run) = a.runs.get_mut(&si) else {
                // nothing of the set is parsed by this group
                a.cursor += 1;
                continue;
            };
            // a range holding no cell the group parses is not read
            if !set.defines[k].iter().any(|c| run.pending.contains_key(c)) {
                run.left -= 1;
                let gone = run.done();
                a.cursor += 1;
                if gone {
                    a.runs.remove(&si);
                    cv.notify_all();
                }
                continue;
            }
            let cost: u64 = store
                .range_charges(ri)
                .iter()
                .filter(|c| run.pending.contains_key(c))
                .map(|&c| store.cell_shape_bytes(c))
                .sum();
            let alone = a.in_flight == 0 && a.marking == 0 && a.lone == 0;
            if alone || a.resident.saturating_add(cost) <= budget {
                run.charged = run.charged.saturating_add(cost);
                let wanted = run.wanted.clone();
                a.cursor += 1;
                a.in_flight += 1;
                a.resident = a.resident.saturating_add(cost);
                a.peak = a.peak.max(a.resident);
                return Some(Task::Range(si, k, wanted));
            }
            break;
        }
        // 2. the next lone cell in cell order
        while a.next < n && (a.claimed[a.next] || store.shared_def_of(a.next).is_some()) {
            a.next += 1;
        }
        if a.next < n {
            let ci = a.next;
            let need = need_of(occ, group, inst, ci);
            let cost = if need == Need::Shapes { store.cell_shape_bytes(ci) } else { 0 };
            if a.resident == 0 || a.resident.saturating_add(cost) <= budget {
                a.next += 1;
                a.claimed[ci] = true;
                if need == Need::Shapes {
                    a.lone += 1;
                }
                a.resident = a.resident.saturating_add(cost);
                a.peak = a.peak.max(a.resident);
                return Some(Task::Cell(ci, need, cost));
            }
        } else if a.cursor >= ranges.len() {
            return None;
        }
        guard = cv.wait(guard).unwrap_or_else(|e| e.into_inner());
    }
}

/// range `k` of set `si` read, parsed for the cells the group parses
/// (`wanted`; the other cells' records are parsed and not kept) and its
/// pieces filed; the cells it completes (their last range) joined, marked
/// and let go, the set's turn over with its last cell. False when the
/// parse failed (reported) or the set is gone (a failure elsewhere)
#[allow(clippy::too_many_arguments)]
fn mark_range(
    store: &StreamStore,
    occ: &StreamOcc,
    group: &StreamGroup,
    inst: &Instances<'_>,
    adm: &Mutex<Admit>,
    cv: &Condvar,
    done: &AtomicUsize,
    si: usize,
    k: usize,
    wanted: &[usize],
    buf: &mut Vec<u8>,
    sc: &mut StreamScratch,
    rep: &mut MarkReport,
) -> bool {
    let set = store.shared_def(si);
    let t = Instant::now();
    let parsed = store.parse_range_shapes(set.ranges[k], buf, &|c| wanted.binary_search(&c).is_ok());
    rep.parse_s += t.elapsed().as_secs_f64();
    let pieces = match parsed {
        Ok((pieces, bytes, skipped)) => {
            rep.bytes += bytes;
            rep.dropped += skipped;
            pieces
        }
        Err(e) => {
            fail(adm, cv, e);
            return false;
        }
    };
    rep.shared_ranges += 1;
    let complete: Vec<(usize, Vec<(usize, Cell)>)> = {
        let mut guard = adm.lock().unwrap_or_else(|e| e.into_inner());
        let a: &mut Admit = &mut guard;
        let Some(run) = a.runs.get_mut(&si) else {
            return false;
        };
        // (a piece of a cell the group does not parse is empty: dropped)
        for (ci, piece) in pieces {
            if let Some(p) = run.pending.get_mut(&ci) {
                p.pieces.push((k, piece));
            }
        }
        let mut complete = Vec::new();
        for &ci in &set.defines[k] {
            if let Some(p) = run.pending.get_mut(&ci) {
                p.remaining = p.remaining.saturating_sub(1);
                if p.remaining == 0 {
                    let p = run.pending.remove(&ci).expect("a pending cell");
                    complete.push((ci, p.pieces));
                }
            }
        }
        run.left -= 1;
        run.marking += complete.len();
        let gone = run.done();
        a.in_flight -= 1;
        a.marking += complete.len();
        if gone {
            a.runs.remove(&si);
        }
        cv.notify_all();
        complete
    };
    for (ci, pieces) in complete {
        let cell = StreamStore::joined(store.inventory().cells[ci].name.clone(), pieces);
        mark_parsed(occ, group, inst, ci, &cell, sc, rep);
        rep.shared += 1;
        drop(cell);
        let bytes = store.cell_shape_bytes(ci);
        {
            let mut guard = adm.lock().unwrap_or_else(|e| e.into_inner());
            let a: &mut Admit = &mut guard;
            a.resident = a.resident.saturating_sub(bytes);
            a.marking -= 1;
            let gone = match a.runs.get_mut(&si) {
                Some(run) => {
                    run.charged = run.charged.saturating_sub(bytes);
                    run.marking -= 1;
                    run.done()
                }
                None => false,
            };
            if gone {
                a.runs.remove(&si);
            }
            cv.notify_all();
        }
        done.fetch_add(1, Relaxed);
    }
    true
}

/// Mark one group: the threads' scratches (for `StreamOcc::end_group`)
/// and the report, or why a cell could not be parsed (the source
/// changed under the inventory). `heartbeat` runs on a thread of its
/// own every 20 ms with the cells done so far (the progress line, the
/// RSS guard).
pub(crate) fn mark_group(
    store: &StreamStore,
    occ: &StreamOcc,
    group: &StreamGroup,
    inst: &Instances<'_>,
    jobs: usize,
    budget: u64,
    heartbeat: &(dyn Fn(usize) + Sync),
) -> Result<(Vec<StreamScratch>, MarkReport), String> {
    let n = store.inventory().cells.len();
    let adm = Mutex::new(Admit {
        next: 0,
        claimed: vec![false; n],
        resident: 0,
        peak: 0,
        cursor: 0,
        activated: vec![false; store.shared_count()],
        runs: HashMap::new(),
        in_flight: 0,
        marking: 0,
        lone: 0,
        failed: None,
    });
    let cv = Condvar::new();
    let done = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let reports: Mutex<Vec<MarkReport>> = Mutex::new(Vec::new());
    let scratches: Vec<StreamScratch> = std::thread::scope(|s| {
        // the heartbeat ends when the markers are done or one of them
        // panicked (the scope must join either way)
        let _stop = StopOnDrop(&stop);
        let (stop_ref, done_ref) = (&stop, &done);
        s.spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if stop_ref.load(Relaxed) {
                return;
            }
            heartbeat(done_ref.load(Relaxed).min(n));
        });
        let handles: Vec<_> = (0..jobs.max(1))
            .map(|_| {
                let (adm, cv, done, reports) = (&adm, &cv, &done, &reports);
                s.spawn(move || {
                    let mut sc = occ.scratch(group);
                    let mut buf: Vec<u8> = Vec::new();
                    let mut rep = MarkReport::default();
                    let empty = Cell::default();
                    loop {
                        let task = match claim(store, occ, group, inst, adm, cv, budget, n) {
                            Some(t) => t,
                            None => break,
                        };
                        match task {
                            Task::Cell(ci, need, cost) => {
                                if need == Need::Shapes {
                                    let t = Instant::now();
                                    let parsed = store.parse_cell_shapes(ci, &mut buf);
                                    rep.parse_s += t.elapsed().as_secs_f64();
                                    match parsed {
                                        Ok((cell, bytes)) => {
                                            rep.bytes += bytes;
                                            mark_parsed(occ, group, inst, ci, &cell, &mut sc, &mut rep);
                                            drop(cell);
                                        }
                                        Err(e) => {
                                            fail(adm, cv, e);
                                            break;
                                        }
                                    }
                                    release(adm, cv, cost);
                                } else {
                                    mark_unparsed(occ, group, inst, ci, need, &empty, &mut sc, &mut rep);
                                }
                                done.fetch_add(1, Relaxed);
                            }
                            Task::Unparsed(cells) => {
                                for (ci, need) in cells {
                                    mark_unparsed(occ, group, inst, ci, need, &empty, &mut sc, &mut rep);
                                    done.fetch_add(1, Relaxed);
                                }
                            }
                            Task::Range(si, k, wanted) => {
                                if !mark_range(store, occ, group, inst, adm, cv, done, si, k, &wanted, &mut buf, &mut sc, &mut rep) {
                                    break;
                                }
                            }
                        }
                    }
                    reports.lock().unwrap_or_else(|e| e.into_inner()).push(rep);
                    sc
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("occupancy marker"))
            .collect()
    });
    let a = adm.into_inner().unwrap_or_else(|e| e.into_inner());
    if let Some(why) = a.failed {
        return Err(why);
    }
    let mut total = MarkReport::default();
    for r in reports.into_inner().unwrap_or_else(|e| e.into_inner()).iter() {
        total.add(r);
    }
    total.peak = a.peak;
    Ok((scratches, total))
}
