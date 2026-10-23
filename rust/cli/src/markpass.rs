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
//! the group's layers (its placements' charges come from the kept
//! placements) - so a layout's later groups parse only the cells with
//! records on their layers.
//!
//! Cells whose definitions share ranges - CELL records inside a CBLOCK of
//! another cell's range, or a cell defined in several ranges that define
//! others too - are parsed as one set (`StreamStore::shared_def_of`,
//! `parse_ranges`; 0.12.391, generalized 0.12.392): the marker that
//! reaches the first of them claims every cell of the set not yet
//! claimed, charges the set's cost (what parsing its ranges materializes:
//! every cell's records) and marks them in cell order from that one parse
//! of the set's ranges; before, each cell's turn parsed every range of the
//! cell again charging that cell alone (review 2026-10-23: a 663 KiB
//! block of 24 cells read 24 times, the budget under-charged by the other
//! 23; the review of 0.12.391: the same when the 24 cells were defined on
//! their own as well, as 0.12.391 took a cell's first range alone).
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
use std::sync::{Condvar, Mutex};
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
    /// of `parsed`, cells served from the ranges of a set parsed once for
    /// several cells (`StreamStore::parse_ranges`), and those ranges
    pub shared: usize,
    pub shared_ranges: usize,
    /// source bytes the markers read
    pub bytes: u64,
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
    next: usize,
    /// cells claimed (by their own turn, or with their set)
    claimed: Vec<bool>,
    /// estimated Doc bytes of the cells being parsed or marked
    resident: u64,
    failed: Option<String>,
}

/// what a marker claims at a time
enum Task {
    /// one cell, its need and its charge (its Doc bytes when parsed)
    Cell(usize, Need, u64),
    /// a set of cells whose definitions share ranges (`SharedDef`), its
    /// ranges parsed once: the set, the cells claimed with it (cell order)
    /// and the set's charge
    Shared(usize, Vec<(usize, Need)>, u64),
}

fn fail(adm: &Mutex<Admit>, cv: &Condvar, e: String) {
    let mut a = adm.lock().unwrap_or_else(|p| p.into_inner());
    if a.failed.is_none() {
        a.failed = Some(e);
    }
    cv.notify_all();
}

fn release(adm: &Mutex<Admit>, cv: &Condvar, cost: u64) {
    let mut a = adm.lock().unwrap_or_else(|p| p.into_inner());
    a.resident = a.resident.saturating_sub(cost);
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
            occ.mark_cell(group, empty, &[], ci, inst, sc);
            rep.mark_s += t.elapsed().as_secs_f64();
            rep.small += 1;
        }
        Need::Places => {
            let t = Instant::now();
            occ.mark_cell(group, empty, inst.places(ci), ci, inst, sc);
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
    occ.mark_cell(group, cell, inst.places(ci), ci, inst, sc);
    rep.mark_s += t.elapsed().as_secs_f64();
    rep.parsed += 1;
}

struct StopOnDrop<'a>(&'a AtomicBool);
impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Relaxed);
    }
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
                        // the next cell in file order, admitted by its
                        // bytes - with every other cell of its set when
                        // its ranges define several (parsed once, charged
                        // whole)
                        let task = {
                            let mut a = adm.lock().unwrap_or_else(|e| e.into_inner());
                            let taken = loop {
                                while a.next < n && a.claimed[a.next] {
                                    a.next += 1;
                                }
                                if a.failed.is_some() || a.next >= n {
                                    break None;
                                }
                                let ci = a.next;
                                let need = need_of(occ, group, inst, ci);
                                let shared = if need == Need::Shapes { store.shared_def_of(ci) } else { None };
                                let cost = match shared {
                                    Some(d) => d.cost,
                                    None if need == Need::Shapes => store.cell_bytes(ci),
                                    None => 0,
                                };
                                if a.resident == 0 || a.resident.saturating_add(cost) <= budget {
                                    a.next += 1;
                                    a.claimed[ci] = true;
                                    a.resident = a.resident.saturating_add(cost);
                                    break Some(match shared {
                                        None => Task::Cell(ci, need, cost),
                                        Some(d) => {
                                            // (ci is the first unclaimed cell of all, so
                                            // the set's unclaimed cells follow it)
                                            let mut cells = Vec::with_capacity(d.cells.len());
                                            for &c in &d.cells {
                                                if c == ci {
                                                    cells.push((c, need));
                                                } else if !a.claimed[c] {
                                                    a.claimed[c] = true;
                                                    cells.push((c, need_of(occ, group, inst, c)));
                                                }
                                            }
                                            Task::Shared(ci, cells, cost)
                                        }
                                    });
                                }
                                a = cv.wait(a).unwrap_or_else(|e| e.into_inner());
                            };
                            match taken {
                                Some(t) => t,
                                None => break,
                            }
                        };
                        match task {
                            Task::Cell(ci, need, cost) => {
                                if need == Need::Shapes {
                                    let t = Instant::now();
                                    let parsed = store.parse_cell(ci, &mut buf);
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
                            Task::Shared(first, cells, cost) => {
                                let set = store.shared_def_of(first).expect("a claimed set");
                                let t = Instant::now();
                                let parsed = store.parse_ranges(&set.ranges, &mut buf);
                                rep.parse_s += t.elapsed().as_secs_f64();
                                let mut by_cell: HashMap<usize, Cell> = match parsed {
                                    Ok((by_cell, bytes)) => {
                                        rep.bytes += bytes;
                                        by_cell
                                    }
                                    Err(e) => {
                                        fail(adm, cv, e);
                                        break;
                                    }
                                };
                                rep.shared_ranges += set.ranges.len();
                                for &(ci, need) in &cells {
                                    if need != Need::Shapes {
                                        mark_unparsed(occ, group, inst, ci, need, &empty, &mut sc, &mut rep);
                                    } else {
                                        // complete: the set's ranges are every range
                                        // that defines it
                                        let cell = by_cell.remove(&ci).unwrap_or_else(|| Cell {
                                            name: store.inventory().cells[ci].name.clone(),
                                            ..Default::default()
                                        });
                                        mark_parsed(occ, group, inst, ci, &cell, &mut sc, &mut rep);
                                        rep.shared += 1;
                                        drop(cell);
                                    }
                                    done.fetch_add(1, Relaxed);
                                }
                                drop(by_cell);
                                release(adm, cv, cost);
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
    Ok((scratches, total))
}
