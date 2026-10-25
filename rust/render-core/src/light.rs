//! The light cache in the renderer (floe_vfs::light; docs/DIRECT_READ_PLAN.ko.md
//! §5.3-5.4, L4): the index the cache plans on is a synthesized image that
//! grows as cells are visited, so it sits in a slot every reader takes a
//! snapshot of (`VfsSlot`), a plan that expanded a cell the image holds no
//! placements of is made again after the fill (`LightLink::run`), and a
//! page is decoded from its segment of the source instead of a page file
//! (`decode_pages`). A full cache goes through the same slot and link,
//! with nothing to fill and the page files to read.

use crate::cache::{check_decode_cancelled, decode_shrink, elapsed_us, shrink_records, DecodedPage};
use crate::{RenderCancellation, RenderStats};
use floe_oasis::doc::{Cell, Doc};
use floe_vfs::hiersum::HierSummary;
use floe_vfs::light::LightIndex;
use floe_vfs::Vfs;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

/// the plan -> fill -> plan rounds one request may take: a round fills the
/// cells the plan expanded without their placements, so the hierarchy's
/// depth bounds the rounds a complete plan needs
const MAX_FILL_ROUNDS: u32 = 64;

/// FLOE_LIGHT_FIRST_MS: how long a viewer frame's plan fills before the
/// frame goes out partial and the fills go on behind it (DIRECT_READ_PLAN
/// §5.4: the viewer sees the first rounds' picture first, as it sees a
/// refining frame; renderd run_render's light attempts). Default 300 ms;
/// `0` / `off` = a plan runs its rounds to the end before any frame (the
/// L4 behaviour, the kill switch).
pub fn light_first_budget() -> Option<std::time::Duration> {
    static MS: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    (*MS.get_or_init(|| match std::env::var("FLOE_LIGHT_FIRST_MS") {
        Err(_) => Some(300),
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            if v.is_empty() {
                Some(300)
            } else if v == "off" || v == "0" {
                None
            } else {
                v.parse::<u64>().ok().filter(|&ms| ms > 0)
            }
        }
    }))
    .map(std::time::Duration::from_millis)
}

/// FLOE_LIGHT_LOG=on: a line per fill round on stderr (diagnostic)
fn light_log() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("FLOE_LIGHT_LOG").is_ok_and(|v| !v.is_empty() && v != "off" && v != "0")
    })
}

/// The index a cache plans on, replaced whole when the light cache's image
/// grows; a reader takes the current one and works on it to the end.
pub(crate) struct VfsSlot {
    current: RwLock<Arc<Vfs>>,
}

impl VfsSlot {
    pub fn new(vfs: Arc<Vfs>) -> Arc<VfsSlot> {
        Arc::new(VfsSlot {
            current: RwLock::new(vfs),
        })
    }

    pub fn get(&self) -> Arc<Vfs> {
        Arc::clone(&self.current.read().unwrap_or_else(|p| p.into_inner()))
    }

    /// the slot for a fill to grow its image in place (or put a copy in)
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Arc<Vfs>> {
        self.current.write().unwrap_or_else(|p| p.into_inner())
    }
}

/// The light cache's state behind a `Cache`: the index files and the
/// source, the summary made from the inventory, and the lock that
/// serializes fills with the image swaps they end in.
pub(crate) struct LightState {
    pub index: LightIndex,
    pub summary: Arc<HierSummary>,
    fill_lock: Mutex<()>,
    /// images assembled after the first, and the rounds requests took
    pub images: AtomicU64,
    pub rounds: AtomicU64,
    /// the prefill caps of a fill (floe_vfs::light::prefill_caps, read at
    /// the open; a test sets them)
    prefill: Mutex<(u64, u64)>,
    /// requests in flight (`LightLink::run_within`): the first to start
    /// clears the image's used marks, so a fill keeps what the requests in
    /// flight use and not what the previous one did
    active: AtomicUsize,
}

/// a request in flight on a light cache (`LightState::begin_request`)
struct RequestGuard<'a>(&'a AtomicUsize);

impl Drop for RequestGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl LightState {
    /// Opens the light cache in `dir`: its files, the source they were made
    /// from, the summary from the inventory's edges and the first image (no
    /// placements yet).
    pub fn open(dir: &str) -> Result<(LightState, Arc<Vfs>), String> {
        let index = LightIndex::open(dir)?;
        let (bytes, _) = index.summary();
        let summary = Arc::new(HierSummary::from_bytes(bytes)?);
        let (ovm, ovt) = index.ovm()?;
        let vfs = Arc::new(Vfs::light(ovm, index.source.clone(), ovt));
        Ok((
            LightState {
                index,
                summary,
                fill_lock: Mutex::new(()),
                images: AtomicU64::new(0),
                rounds: AtomicU64::new(0),
                prefill: Mutex::new(floe_vfs::light::prefill_caps()),
                active: AtomicUsize::new(0),
            },
            vfs,
        ))
    }

    /// A request starts: the image's used marks (floe_ovm `light_used`, what
    /// a fill keeps) become this request's when no other is in flight - a
    /// concurrent request's marks stay, which only keeps more.
    fn begin_request(&self, slot: &VfsSlot) -> RequestGuard<'_> {
        if self.active.fetch_add(1, Ordering::SeqCst) == 0 {
            slot.get().ovm.light_reset_used();
        }
        RequestGuard(&self.active)
    }

    /// Fills `cells` (keeping `keep`, the cells the request read from the
    /// image it planned on: floe_vfs::light::LightIndex::fill_keeping -
    /// the rounds would otherwise refill what the round before let go, up
    /// to MAX_FILL_ROUNDS) and grows the slot's image by them in place
    /// (`LightIndex::extend_image`; DIRECT_READ_PLAN §8.9) - the request
    /// let its snapshot go, so no one else holds the image; a reader that
    /// does gets to keep it and a copy grows instead. Serialized with every
    /// other fill.
    fn fill_into(&self, cells: &[u32], keep: &[u32], slot: &VfsSlot, round: u32) -> Result<(), String> {
        let _guard = self.fill_lock.lock().unwrap_or_else(|p| p.into_inner());
        let started = Instant::now();
        let (places_cap, bytes_cap) = *self.prefill.lock().unwrap_or_else(|p| p.into_inner());
        let set = self.index.fill_set_within(cells, places_cap, bytes_cap);
        let report = self.index.fill_keeping(&set, keep)?;
        let fill_us = elapsed_us(started);
        if report.cells == 0 && report.evicted == 0 && !self.index.image_behind() {
            // (another thread's fill put them in the image meanwhile)
            return Ok(());
        }
        let image_started = Instant::now();
        let mut current = slot.write();
        let (how, extended) = match Arc::get_mut(&mut *current) {
            Some(vfs) => ("in place", self.index.extend_image(vfs)?),
            None => {
                let mut copy = current
                    .clone_light()
                    .ok_or_else(|| "light index: the session's image is not a light one".to_string())?;
                let extended = self.index.extend_image(&mut copy)?;
                *current = Arc::new(copy);
                ("copied", extended)
            }
        };
        drop(current);
        self.images.fetch_add(1, Ordering::Relaxed);
        if light_log() {
            eprintln!(
                "[light] round {}: {} cell(s) asked, {} filled ({} placements, {} texts from {} segments, {} evicted) in {} ms; image {} MB {} (+{} cells, {} holed{}) in {} ms{}",
                round,
                cells.len(),
                report.cells,
                report.placements,
                report.texts,
                report.segments,
                report.evicted,
                fill_us / 1000,
                extended.bytes >> 20,
                if extended.rebuilt { "rebuilt" } else { how },
                extended.appended,
                extended.holed,
                if extended.dead_bytes > 0 {
                    format!(", {} MB dead", extended.dead_bytes >> 20)
                } else {
                    String::new()
                },
                elapsed_us(image_started) / 1000,
                if report.over_budget > 0 {
                    format!(
                        "; {} MB over the places budget, kept (the request uses them)",
                        report.over_budget >> 20
                    )
                } else {
                    String::new()
                }
            );
        }
        Ok(())
    }
}

/// What a `Cache` and its `HierHandle` share: the slot and, for a light
/// cache, its state.
#[derive(Clone)]
pub(crate) struct LightLink {
    pub slot: Arc<VfsSlot>,
    pub light: Option<Arc<LightState>>,
}

impl LightLink {
    /// (tests) the prefill caps of this light cache's fills: 0 = the asked
    /// cells alone, a round per hierarchy level
    pub fn set_prefill(&self, places: u64, bytes: u64) {
        if let Some(light) = &self.light {
            *light.prefill.lock().unwrap_or_else(|p| p.into_inner()) = (places, bytes);
        }
    }

    /// (tests) the resident placement sections' budget of this light cache
    /// (floe_vfs::light PLACES_BUDGET_DEFAULT, FLOE_LIGHT_PLACES_MB)
    pub fn set_places_budget(&self, bytes: u64) {
        if let Some(light) = &self.light {
            light.index.set_places_budget(bytes);
        }
    }

    /// a full cache's: the index as opened, nothing to fill
    pub fn full(vfs: Arc<Vfs>) -> LightLink {
        LightLink {
            slot: VfsSlot::new(vfs),
            light: None,
        }
    }

    pub fn vfs(&self) -> Arc<Vfs> {
        self.slot.get()
    }

    pub fn is_light(&self) -> bool {
        self.light.is_some()
    }

    /// Runs `work` on the current index and, on a light cache, again after
    /// filling the cells it expanded without their placements or texts
    /// (floe_ovm LightMarks), until it expands none such - the frame flow of
    /// DIRECT_READ_PLAN §5.4 (a round per hierarchy level at most). `work`
    /// must be repeatable: a plan, a probe, a walk.
    pub fn run<T>(&self, work: impl Fn(&Vfs) -> Result<T, String>) -> Result<T, String> {
        self.run_within(work, None).map(|(out, _)| out)
    }

    /// `run` under a deadline: past it, with at least one fill made by this
    /// call, the work's last result goes out with `true` (its marks stay on
    /// the image; the next call takes them up where this one left) - the
    /// viewer's partial frame of a light cache (light_first_budget). None:
    /// to the end.
    pub fn run_within<T>(
        &self,
        work: impl Fn(&Vfs) -> Result<T, String>,
        deadline: Option<Instant>,
    ) -> Result<(T, bool), String> {
        let _request = self.light.as_ref().map(|light| light.begin_request(&self.slot));
        let mut rounds = 0u32;
        loop {
            let vfs = self.slot.get();
            let out = work(&vfs)?;
            let Some(light) = &self.light else {
                return Ok((out, false));
            };
            let touched = vfs.ovm.light_touched();
            if touched.is_empty() {
                return Ok((out, false));
            }
            if rounds > 0 && deadline.is_some_and(|d| Instant::now() >= d) {
                return Ok((out, true));
            }
            rounds += 1;
            light.rounds.fetch_add(1, Ordering::Relaxed);
            if rounds > MAX_FILL_ROUNDS {
                return Err(format!(
                    "light index: {} rounds of placement fills did not complete the request ({} cells still without their placements)",
                    rounds - 1,
                    touched.len()
                ));
            }
            // the cells this request read from the image stay resident; the
            // plan and the snapshot go before the fill, so the slot's image
            // is held by no one and grows in place (fill_into)
            let keep = vfs.ovm.light_used();
            drop(out);
            drop(vfs);
            light.fill_into(&touched, &keep, &self.slot, rounds)?;
        }
    }
}

#[cfg(test)]
mod budget_tests {
    /// FLOE_LIGHT_PLACES_MB (floe_vfs::light::LightIndex::fill_keeping): a
    /// request keeps the cells it uses even past the budget - its rounds
    /// would otherwise refill every round what the round before let go, up
    /// to MAX_FILL_ROUNDS (the exact fit of a 909 MB chip did: 64 rounds,
    /// 228 s, then the error) - and the next request's fill lets the cells
    /// it does not use go
    #[test]
    fn a_request_past_the_budget_keeps_what_it_uses_and_the_next_one_lets_the_rest_go() {
        let (root, dir) = super::tests::light_dir_of("budget", &super::tests::two_arms());
        let cache = crate::Cache::open(&dir).unwrap();
        cache.set_light_prefill_for_tests(0, 0);
        cache.set_light_places_budget_for_tests(1);
        let left = super::tests::exact(crate::ViewBox::new(0, 0, 1000, 1000).unwrap());
        let right = super::tests::exact(crate::ViewBox::new(6000, 6000, 7000, 7000).unwrap());
        // the left arm: TOP's placements (round 1) and ARM_A's (round 2);
        // the budget is past from the first fill on, nothing the request
        // uses goes
        let (a, pending) = cache.plan_within_for_tests(&left, None).unwrap();
        assert!(!pending);
        let (_, rounds, _, filled, evicted) = cache.light_stats().unwrap();
        assert_eq!((rounds, filled, evicted), (2, 2, 0));
        assert!(a.plan.stats.inst_edges >= 2, "{:?}", a.plan.stats);
        // the right arm: ARM_B is filled; ARM_A, which this request does
        // not use, goes
        let (b, pending) = cache.plan_within_for_tests(&right, None).unwrap();
        assert!(!pending);
        let (_, rounds, _, filled, evicted) = cache.light_stats().unwrap();
        assert_eq!((rounds, filled, evicted), (3, 3, 1));
        assert!(b.plan.stats.inst_edges >= 2, "{:?}", b.plan.stats);
        // back to the left: ARM_A is read again, ARM_B goes
        let (a2, pending) = cache.plan_within_for_tests(&left, None).unwrap();
        assert!(!pending);
        assert_eq!(a2.plan.pages, a.plan.pages);
        let (_, rounds, _, filled, evicted) = cache.light_stats().unwrap();
        assert_eq!((rounds, filled, evicted), (4, 4, 2));
        // the default budget holds both arms
        let cache2 = crate::Cache::open(&dir).unwrap();
        cache2.set_light_prefill_for_tests(0, 0);
        cache2.plan_within_for_tests(&left, None).unwrap();
        cache2.plan_within_for_tests(&right, None).unwrap();
        let (_, rounds, _, filled, evicted) = cache2.light_stats().unwrap();
        assert_eq!((rounds, filled, evicted), (3, 3, 0));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn a_deadline_lets_the_work_out_after_one_fill_and_the_next_call_goes_on() {
        let (root, dir) = super::tests::light_dir("deadline");
        let cache = crate::Cache::open(&dir).unwrap();
        // the asked cells alone per round (no prefill): the tree's two
        // levels of placements take two rounds
        cache.set_light_prefill_for_tests(0, 0);
        let view = crate::ViewBox::new(-100, -100, 1100, 1100).unwrap();
        let request = super::tests::exact(view);
        // a deadline already past: the first call makes one fill round (the
        // top's) and returns pending
        let (first, pending) = cache
            .plan_within_for_tests(&request, Some(Instant::now()))
            .unwrap();
        assert!(pending, "the plan did not finish within a past deadline");
        let after_first = cache.light_stats().unwrap();
        assert_eq!(after_first.1, 1, "one round: {:?}", after_first);
        assert_eq!(after_first.3, 1, "the top filled: {:?}", after_first);
        // the next call takes the marks up (MID's placements) and ends
        let (second, pending) = cache
            .plan_within_for_tests(&request, Some(Instant::now()))
            .unwrap();
        assert!(!pending, "the second call did not end the fills");
        let after_second = cache.light_stats().unwrap();
        assert_eq!((after_second.1, after_second.3), (2, 2), "{:?}", after_second);
        // (LEAF's pages were reached through TOP's own placement of it in
        // the first round; MID's placements of it come with the second)
        assert!(first.plan.pages.len() <= second.plan.pages.len());
        assert!(first.plan.stats.inst_edges < second.plan.stats.inst_edges);
        assert_eq!(second.plan.stats.wc_cells, 3);
        // nothing left: a call with no deadline plans the same without a round
        let (third, pending) = cache.plan_within_for_tests(&request, None).unwrap();
        assert!(!pending);
        assert_eq!(third.plan.pages, second.plan.pages);
        assert_eq!(cache.light_stats().unwrap().1, 2);
        // no deadline on a fresh cache: the whole thing in one call
        let cache2 = crate::Cache::open(&dir).unwrap();
        cache2.set_light_prefill_for_tests(0, 0);
        let (whole, pending) = cache2.plan_within_for_tests(&request, None).unwrap();
        assert!(!pending);
        assert_eq!(whole.plan.pages, third.plan.pages);
        assert_eq!(cache2.light_stats().unwrap().1, 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}

fn clone_cell(c: &Cell) -> Cell {
    Cell {
        name: c.name.clone(),
        rects: c.rects.clone(),
        polys: c.polys.clone(),
        paths: c.paths.clone(),
        places: c.places.clone(),
        texts: c.texts.clone(),
    }
}

/// `Cache::decode_pages_parallel` on a light cache: the pages' segments
/// read from the source in file order, each parsed once with its snapshot
/// (floe_oasis doc::parse_segment) on `workers` threads, and its records
/// split by layer into the pages asked for - a page is the records of one
/// layer of one segment, in a one-cell Doc as a page file decodes to.
pub(crate) fn decode_pages(
    light: &LightState,
    vfs: &Vfs,
    page_ids: &[u32],
    workers: u16,
    guard: Option<(u64, &RenderCancellation)>,
) -> Result<(Vec<DecodedPage>, RenderStats), String> {
    let index = &light.index;
    let unit = vfs.ovm.unit;
    // the pages by segment, segments in file order
    let mut by_seg: BTreeMap<u64, (u32, Vec<(usize, u32)>)> = BTreeMap::new();
    for (pos, &pi) in page_ids.iter().enumerate() {
        let lp = index
            .page(pi)
            .ok_or_else(|| format!("page {} out of range ({} pages)", pi, index.n_pages()))?;
        let offset = index.segment(lp.seg).offset;
        by_seg
            .entry(offset)
            .or_insert_with(|| (lp.seg, Vec::new()))
            .1
            .push((pos, pi));
    }
    let jobs: Vec<(u32, Vec<(usize, u32)>)> = by_seg.into_values().collect();
    if jobs.is_empty() {
        return Ok((Vec::new(), RenderStats::default()));
    }
    check_decode_cancelled(guard)?;
    let read_started = Instant::now();
    // adjacent segments as one pread, the runs in parallel
    // (LightIndex::read_segments)
    let sis: Vec<u32> = jobs.iter().map(|(si, _)| *si).collect();
    let raws: Vec<Vec<u8>> = index.read_segments(&sis, workers.max(1) as usize)?;
    check_decode_cancelled(guard)?;
    let page_read_us = elapsed_us(read_started);
    let decode_started = Instant::now();
    type Out = (usize, Result<Vec<(usize, DecodedPage, u64)>, String>, u64);
    let decode_job = |k: usize| -> Out {
        let started = Instant::now();
        let result = (|| -> Result<Vec<(usize, DecodedPage, u64)>, String> {
            let (si, pages) = &jobs[k];
            let seg = index.segment(*si);
            let mut cell = Cell::default();
            for p in index.parse_segment(*si, &raws[k])? {
                if p.index != seg.cell as usize {
                    return Err(format!(
                        "light index: segment at {} holds records of cell {} ({}) besides its own",
                        seg.offset, p.index, p.cell.name
                    ));
                }
                cell.rects.extend(p.cell.rects);
                cell.polys.extend(p.cell.polys);
                cell.paths.extend(p.cell.paths);
            }
            // the pages wanted, by their layer; the records go to their
            // layer's bucket, the other layers' are dropped
            let mut want: BTreeMap<(u32, u32), Vec<(usize, u32)>> = BTreeMap::new();
            for &(pos, pi) in pages {
                let key = index.layer_key(index.page(pi).expect("page checked").layer_idx);
                want.entry(key).or_default().push((pos, pi));
            }
            let mut buckets: HashMap<(u32, u32), Cell> =
                want.keys().map(|k| (*k, Cell::default())).collect();
            for r in cell.rects.drain(..) {
                if let Some(b) = buckets.get_mut(&(r.layer, r.dt)) {
                    b.rects.push(r);
                }
            }
            for p in cell.polys.drain(..) {
                if let Some(b) = buckets.get_mut(&(p.layer, p.dt)) {
                    b.polys.push(p);
                }
            }
            for p in cell.paths.drain(..) {
                if let Some(b) = buckets.get_mut(&(p.layer, p.dt)) {
                    b.paths.push(p);
                }
            }
            let mut out = Vec::with_capacity(pages.len());
            for (key, list) in want {
                let mut bucket = buckets.remove(&key).unwrap_or_default();
                let last = list.len() - 1;
                for (j, (pos, pi)) in list.into_iter().enumerate() {
                    check_decode_cancelled(guard)?;
                    let body = if j == last {
                        std::mem::take(&mut bucket)
                    } else {
                        clone_cell(&bucket)
                    };
                    let lp = index.page(pi).expect("page checked");
                    let doc = Doc {
                        unit,
                        cells: vec![Cell {
                            name: floe_ovm::page_cell_name(seg.cell, lp.layer_idx, lp.seq),
                            rects: body.rects,
                            polys: body.polys,
                            paths: body.paths,
                            places: Vec::new(),
                            texts: Vec::new(),
                        }],
                        top: 0,
                        layer_order: vec![key],
                        norm_s: 0.0,
                        layer_names: HashMap::new(),
                        layer_aliases: HashMap::new(),
                    };
                    let index_started = Instant::now();
                    let pindex = crate::PageIndex::build_cancellable(&doc, &mut || {
                        check_decode_cancelled(guard)
                    })?;
                    let index_us = elapsed_us(index_started);
                    let mut page = DecodedPage {
                        page_id: pi,
                        layer_idx: lp.layer_idx,
                        bbox: lp.bbox,
                        encoded_bytes: lp.usize_,
                        records: lp.records,
                        members: lp.members,
                        doc,
                        index: pindex,
                        grown_bytes: 0,
                    };
                    if decode_shrink() {
                        let grown = page.estimated_bytes();
                        shrink_records(&mut page.doc);
                        page.grown_bytes = grown;
                    }
                    out.push((pos, page, index_us));
                }
            }
            Ok(out)
        })();
        (k, result, elapsed_us(started))
    };
    let worker_count = usize::from(workers).min(jobs.len()).max(1);
    let n_jobs_total = jobs.len();
    let outputs: Vec<Out> = if worker_count <= 1 {
        let mut outputs = Vec::with_capacity(jobs.len());
        for k in 0..jobs.len() {
            check_decode_cancelled(guard)?;
            outputs.push(decode_job(k));
        }
        outputs
    } else {
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_count);
            for _ in 0..worker_count {
                let next = &next;
                let decode_job = &decode_job;
                let n_jobs = jobs.len();
                handles.push(scope.spawn(move || {
                    let mut outputs = Vec::new();
                    loop {
                        check_decode_cancelled(guard)?;
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        if k >= n_jobs {
                            break;
                        }
                        outputs.push(decode_job(k));
                    }
                    Ok::<_, String>(outputs)
                }));
            }
            let mut outputs = Vec::with_capacity(n_jobs_total);
            let mut worker_error = None;
            for handle in handles {
                match handle.join() {
                    Ok(Ok(mut got)) => outputs.append(&mut got),
                    Ok(Err(error)) => {
                        worker_error.get_or_insert(error);
                    }
                    Err(_) => {
                        worker_error.get_or_insert_with(|| "segment decode worker panicked".to_string());
                    }
                }
            }
            match worker_error {
                Some(e) => Err(e),
                None => Ok(outputs),
            }
        })?
    };
    check_decode_cancelled(guard)?;
    let mut slots: Vec<Option<DecodedPage>> = (0..page_ids.len()).map(|_| None).collect();
    let mut page_decode_sum_us = 0u64;
    let mut page_decode_max_us = 0u64;
    let mut page_index_us = 0u64;
    let mut decoded_bytes = 0u64;
    for (_, result, job_us) in outputs {
        page_decode_sum_us = page_decode_sum_us.saturating_add(job_us);
        page_decode_max_us = page_decode_max_us.max(job_us);
        for (pos, page, index_us) in result? {
            page_index_us = page_index_us.saturating_add(index_us);
            decoded_bytes = decoded_bytes
                .checked_add(page.encoded_bytes as u64)
                .ok_or_else(|| "limit exceeded: decoded page bytes".to_string())?;
            slots[pos] = Some(page);
        }
    }
    let mut decoded = Vec::with_capacity(page_ids.len());
    for (pos, slot) in slots.into_iter().enumerate() {
        decoded.push(slot.ok_or_else(|| {
            format!("internal error: page {} (position {}) was not decoded", page_ids[pos], pos)
        })?);
    }
    Ok((
        decoded,
        RenderStats {
            page_read_us,
            page_decode_us: elapsed_us(decode_started),
            page_decode_sum_us,
            page_decode_max_us,
            page_index_us,
            decode_workers_used: worker_count.try_into().unwrap_or(u16::MAX),
            decoded_cache_miss: page_ids.len().try_into().unwrap_or(u32::MAX),
            decoded_cache_bytes: decoded_bytes,
            ..RenderStats::default()
        },
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::{Cache, DecodedPageCache, PlanRequest, ViewBox, FULL_DEPTH};
    use floe_oasis::doc::{PathRec, PolyRec, RectRec, Rep};
    use floe_oasis::segments::DEFAULT_SPAN_BYTES;
    use floe_oasis::write::{write_tree, WCell};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn rect(layer: u32, x: i64, y: i64, w: i64, h: i64) -> RectRec {
        RectRec { layer, dt: 0, x, y, w, h, rep: Rep::One }
    }

    /// the floe_vfs light tests' tree: LEAF (a rect on 1/0, a path on 3/0),
    /// MID (a polygon on 2/0; LEAF once and as a 2x3 grid), TOP (a rect on
    /// 1/0; MID at the origin and turned at (500, 100); LEAF flipped as a
    /// point list of three) - LEAF is instantiated 17 times under TOP
    fn tree() -> Vec<u8> {
        let leaf_rects = [rect(1, 0, 0, 10, 4)];
        let leaf_paths = [PathRec {
            layer: 3,
            dt: 0,
            pts: vec![(0, 2), (10, 2)],
            hw: 1,
            es: 0,
            ee: 0,
            rep: Rep::One,
        }];
        let mid_polys = [PolyRec {
            layer: 2,
            dt: 0,
            pts: vec![(0, 0), (100, 0), (100, 100), (0, 100)],
            rep: Rep::One,
        }];
        let top_rects = [rect(1, 0, 0, 1000, 1000)];
        let grid = Rep::Grid {
            na: 2,
            nb: 3,
            va: (30, 0),
            vb: (0, 10),
        };
        let pts = Rep::Pts(vec![(0, 0), (50, 0), (0, 50)].into());
        let one = Rep::One;
        let cells = [
            WCell {
                name: "LEAF".into(),
                rects: &leaf_rects,
                polys: &[],
                paths: &leaf_paths,
                texts: &[],
                places: vec![],
            },
            WCell {
                name: "MID".into(),
                rects: &[],
                polys: &mid_polys,
                paths: &[],
                texts: &[],
                places: vec![("LEAF", 5, 5, 0, false, &one), ("LEAF", 20, 50, 0, false, &grid)],
            },
            WCell {
                name: "TOP".into(),
                rects: &top_rects,
                polys: &[],
                paths: &[],
                texts: &[],
                places: vec![
                    ("MID", 0, 0, 0, false, &one),
                    ("MID", 500, 100, 1, false, &one),
                    ("LEAF", 800, 800, 0, true, &pts),
                ],
            },
        ];
        write_tree(&cells, 1000.0).unwrap()
    }

    /// two arms under the top - ARM_A at the origin, ARM_B far away, each
    /// placing LEAF once - so a view over one arm expands that arm alone
    /// (budget_tests)
    pub(super) fn two_arms() -> Vec<u8> {
        let leaf_rects = [rect(1, 0, 0, 10, 4)];
        let top_rects = [rect(1, 0, 0, 10000, 10000)];
        let one = Rep::One;
        let cells = [
            WCell {
                name: "LEAF".into(),
                rects: &leaf_rects,
                polys: &[],
                paths: &[],
                texts: &[],
                places: vec![],
            },
            WCell {
                name: "ARM_A".into(),
                rects: &[],
                polys: &[],
                paths: &[],
                texts: &[],
                places: vec![("LEAF", 10, 10, 0, false, &one)],
            },
            WCell {
                name: "ARM_B".into(),
                rects: &[],
                polys: &[],
                paths: &[],
                texts: &[],
                places: vec![("LEAF", 10, 10, 0, false, &one)],
            },
            WCell {
                name: "TOP".into(),
                rects: &top_rects,
                polys: &[],
                paths: &[],
                texts: &[],
                places: vec![("ARM_A", 0, 0, 0, false, &one), ("ARM_B", 6000, 6000, 0, false, &one)],
            },
        ];
        write_tree(&cells, 1000.0).unwrap()
    }

    /// A deck of light sources opens them `jobs` at a time and keeps the
    /// spec's order (deck.rs `open_sources`; DIRECT_READ_PLAN §8.18); a
    /// source that fails is the error whatever the worker count.
    #[test]
    fn a_deck_opens_its_light_sources_in_parallel_in_order() {
        use crate::deck::{Deck, DeckSpec};
        let hex = |text: &str| -> String { text.bytes().map(|b| format!("{b:02x}")).collect() };
        let kept: Vec<(std::path::PathBuf, String)> =
            (0..7).map(|i| light_dir_of(&format!("deck-open-{i}"), &tree())).collect();
        let dirs: Vec<String> = kept.iter().map(|(_, dir)| dir.clone()).collect();
        let spec_text = |dirs: &[String]| -> String {
            let mut text = String::from("deck unit=0.001\n");
            for dir in dirs {
                text += &format!("source path_hex={}\n", hex(dir));
            }
            text += &format!("layer out=0 key=1/0 name_hex={} color=#0000ff fill=solid width=1\n", hex("L1"));
            for (i, _) in dirs.iter().enumerate() {
                text += &format!(
                    "placement source={i} layer=1/0 out=0 scale=1 dx={} dy=0 order={i}\n",
                    i * 2000
                );
            }
            text
        };
        for jobs in [1usize, 4, 16] {
            let spec = DeckSpec::parse(&spec_text(&dirs)).unwrap();
            let deck = Deck::open_with(spec, 1 << 20, jobs).unwrap();
            assert_eq!(deck.source_paths(), &dirs[..], "jobs {jobs}");
            let info = deck.info();
            assert_eq!((info.sources, info.placements, info.light_sources), (7, 7, 7));
        }
        // the third source missing: its error, first in order, at any count
        let mut broken = dirs.clone();
        broken[2] = kept[2].0.join("missing.ice").to_string_lossy().into_owned();
        broken[5] = kept[5].0.join("missing-too.ice").to_string_lossy().into_owned();
        for jobs in [1usize, 4] {
            let spec = DeckSpec::parse(&spec_text(&broken)).unwrap();
            let error = match Deck::open_with(spec, 1 << 20, jobs) {
                Err(error) => error,
                Ok(_) => panic!("a deck with a missing source opened (jobs {jobs})"),
            };
            assert!(error.contains("missing.ice") && !error.contains("missing-too"), "{error}");
        }
        for (d, _) in &kept {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// the tree as a source file with its light files beside it
    pub(super) fn light_dir(tag: &str) -> (std::path::PathBuf, String) {
        light_dir_of(tag, &tree())
    }

    pub(super) fn light_dir_of(tag: &str, data: &[u8]) -> (std::path::PathBuf, String) {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("floe-rc-light-{}-{}-{}", std::process::id(), n, tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let src = d.join("tree.oas");
        std::fs::write(&src, &data).unwrap();
        let id = floe_vfs::light::file_identity(src.to_str().unwrap()).unwrap();
        let dir = d.join(".tree.oas.ice").to_string_lossy().into_owned();
        floe_vfs::light::write_files(data, id, &dir, 1, DEFAULT_SPAN_BYTES).unwrap();
        (d, dir)
    }

    pub(super) fn exact(view: ViewBox) -> PlanRequest {
        PlanRequest {
            view,
            cut_dbu: 0,
            visible_layers: None,
            depth: FULL_DEPTH,
            px_per_dbu: 0.0,
            exact: true,
            sub_cut_wash: false,
            page_reps: false,
            decode_budget: 0,
            page_hairline: true,
            summary_layers: Vec::new(),
            prune_summary: false,
            sub_cut_box: false,
            shape_cut: false,
            shape_cut_max: false,
            frames: true,
            page_wash: true,
            lod_swap: true,
            regions: Vec::new(),
            density_mask: None,
            density_layers: None,
            visible_indices: None,
            fixed_fit: None,
            root: None,
            sub_cut_dots: None,
            dot_records: None,
            probe_limit: 0,
            free_pages: None,
            empty_top: false,
            dot_bright: None,
            dot_occ_first: None,
            fit_rank: None,
        }
    }

    #[test]
    fn a_light_cache_plans_fills_decodes_and_answers_the_cell_tree() {
        let (root, dir) = light_dir("plan");
        let cache = Cache::open(&dir).unwrap();
        assert!(cache.is_light());
        let info = cache.info();
        assert_eq!((info.cells, info.layers), (3, 3));
        assert_eq!(info.max_depth, 2);
        // before any plan: no image after the first, no round
        assert_eq!(cache.light_stats().map(|s| (s.0, s.1)), Some((0, 0)));
        let before = cache.vfs();
        let view = ViewBox::new(-100, -100, 1100, 1100).unwrap();
        let planned = cache.plan(&exact(view)).unwrap();
        // the plan reached every level: the top's three placements, the
        // MIDs' and the leaves' pages - and took at least one fill round
        assert_eq!(planned.plan.stats.wc_cells, 3);
        let stats = cache.light_stats().unwrap();
        assert!(stats.0 >= 1 && stats.1 >= 1, "{:?}", stats);
        assert_eq!(stats.3, 2, "TOP and MID filled: {:?}", stats);
        assert!(!std::sync::Arc::ptr_eq(&before, &cache.vfs()), "the image was replaced");
        // a plan again: nothing to fill
        let images = stats.0;
        let again = cache.plan(&exact(view)).unwrap();
        assert_eq!(again.plan.pages, planned.plan.pages);
        assert_eq!(cache.light_stats().unwrap().0, images);
        // every page decodes to the shapes of its layer; the members of all
        // pages are the cells' own members
        let all: Vec<u32> = (0..info.pages).collect();
        let (pages, _) = cache.decode_pages_parallel(&all, 2).unwrap();
        let mut members = std::collections::BTreeMap::new();
        for p in &pages {
            assert_eq!(p.doc.cells.len(), 1);
            let key = cache.layers()[p.layer_idx as usize].clone();
            let key = (key.layer, key.datatype);
            for r in &p.doc.cells[0].rects {
                assert_eq!((r.layer, r.dt), key);
            }
            for q in &p.doc.cells[0].polys {
                assert_eq!((q.layer, q.dt), key);
            }
            for q in &p.doc.cells[0].paths {
                assert_eq!((q.layer, q.dt), key);
            }
            assert!(p.doc.cells[0].places.is_empty() && p.doc.cells[0].texts.is_empty());
            *members.entry(key).or_insert(0u64) += p.members;
        }
        assert_eq!(
            members,
            [((1, 0), 2u64), ((2, 0), 1), ((3, 0), 1)].into_iter().collect()
        );
        // the pool path decodes the same pages
        let mut page_cache = DecodedPageCache::new(64 << 20);
        let (pooled, _) = cache
            .with_decode_pool(2, None, |pool| page_cache.load_pooled(pool, &all))
            .unwrap();
        let (pooled, _) = pooled.unwrap();
        assert_eq!(pooled.len(), pages.len());
        for (a, b) in pooled.iter().zip(&pages) {
            assert_eq!((a.page_id, a.records, a.members), (b.page_id, b.records, b.members));
        }
        // a page file read is refused: the pages are source segments
        let e = cache.read_pages(&[0]).unwrap_err();
        assert!(e.contains("light index"), "{}", e);
        // the cell tree: the summary from the inventory, the walks through
        // the fills
        let h = cache.hier();
        let names = h.names();
        let top = (0..3).find(|&ci| names[ci] == "TOP").unwrap() as u32;
        let leaf = (0..3).find(|&ci| names[ci] == "LEAF").unwrap() as u32;
        let kids = crate::cell_children(&h, None, 10).unwrap();
        assert_eq!(kids.name, "TOP");
        let rows: Vec<(&str, u64)> = kids.rows.iter().map(|r| (r.name.as_str(), r.members)).collect();
        assert_eq!(rows, vec![("LEAF", 3), ("MID", 2)]);
        let inst = crate::cell_instances(&h, None, leaf, floe_ovm::BBox { x0: -1000, y0: -1000, x1: 5000, y1: 5000 }, 100, 1_000_000).unwrap();
        assert_eq!(inst.boxes.len(), 17);
        let info = crate::cell_info(&h, Some(top), leaf, 1_000_000).unwrap();
        assert_eq!(info.insts, 17);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn labels_on_a_light_cache_come_from_the_filled_texts() {
        let data = floe_oasis::inventory::fixtures::labelled_tree();
        let (root, dir) = light_dir_of("labels", &data);
        let cache = Cache::open(&dir).unwrap();
        let view = ViewBox::new(-100, -100, 1100, 1100).unwrap();
        // the label walk takes what the image holds and asks for no
        // placement fill (0.12.400): on the fresh image TOP's placements are
        // not in, so the walk neither descends nor reads TOP's own texts -
        // no rows, nothing filled (until 0.12.399 the walk's touch filled TOP
        // and every cell under it, five rounds at the chip's fit)
        assert_eq!(cache.light_stats().map(|s| (s.0, s.1, s.3)), Some((0, 0, 0)));
        // (cut 0: TXT's recursive box is its text's anchor, a point, which a
        // cut of any size would fold under - on the full index as well)
        let request = PlanRequest {
            exact: false,
            px_per_dbu: 1.0,
            ..exact(view)
        };
        let none = cache.plan_labels(&request, false, 14.0).unwrap();
        assert!(none.rows.iter().all(|r| r.block), "a fresh light image labels nothing: {:?}", none.rows.len());
        assert_eq!(cache.light_stats().map(|s| (s.0, s.1, s.3)), Some((0, 0, 0)), "the label walk filled");
        // the geometry plan fills the cells it walks (TOP, MID, LEAF, TXT at
        // cut 0); the labels then come from the filled texts - TXT's, a cell
        // without placements, filled by the label walk itself
        let planned = cache.plan(&exact(view)).unwrap();
        assert_eq!(planned.plan.stats.wc_cells, 4);
        let labels = cache.plan_labels(&request, false, 14.0).unwrap();
        let stats = cache.light_stats().unwrap();
        assert!(stats.0 >= 1 && stats.1 >= 1, "{:?}", stats);
        assert_eq!(stats.3, 4, "TOP, MID, LEAF and TXT filled: {:?}", stats);
        let texts: Vec<(&str, i64, i64)> = labels
            .rows
            .iter()
            .filter(|r| !r.block)
            .map(|r| (r.text.as_str(), r.x, r.y))
            .collect();
        assert!(texts.contains(&("T", 500, 990)), "{:?}", texts);
        assert!(texts.contains(&("X", 907, 907)), "{:?}", texts);
        assert!(texts.iter().any(|t| t.0 == "L") && texts.iter().any(|t| t.0 == "M"), "{:?}", texts);
        // planned again, and the geometry planned: nothing more to fill,
        // the same rows
        let images = cache.light_stats().unwrap().0;
        let again = cache.plan_labels(&request, false, 14.0).unwrap();
        assert_eq!(again.rows.len(), labels.rows.len());
        let planned = cache.plan(&exact(view)).unwrap();
        assert_eq!(planned.plan.stats.wc_cells, 4);
        assert_eq!(cache.light_stats().unwrap().0, images);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_directory_with_a_full_index_opens_as_before() {
        // (the light path is taken only without design.ovm: a plain cache
        // directory with neither is still refused as before)
        let d = std::env::temp_dir().join(format!("floe-rc-light-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let e = match Cache::open(&d) {
            Ok(_) => panic!("an empty directory opened as a cache"),
            Err(e) => e,
        };
        assert!(e.contains("design.ovm"), "{}", e);
        let _ = std::fs::remove_dir_all(&d);
    }
}
