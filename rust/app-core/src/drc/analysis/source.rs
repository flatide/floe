//! What the analysis reads of an open results database, from worker
//! threads: errors in rule order (a pack's coordinate blocks, or the ASCII
//! file's records) and the review's status bytes.
use super::super::{desktop::StatusReader, Ascii, Blocks};
use crate::{check_cancelled, Result};
use std::sync::{atomic::AtomicUsize, Arc};

/// An error's bounding box: a pack's in integer database units, the ASCII
/// file's in micrometres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ErrorBox {
    Dbu([i64; 4]),
    Um([f64; 4]),
}

/// One open results database, as worker threads read it.
pub trait Source: Send + Sync {
    fn check_count(&self) -> usize;
    fn count(&self, check: usize) -> u64;
    /// Database units per micrometre.
    fn precision(&self) -> f64;
    /// Errors `start..end` of rule `check` in order: `visit(ei, kind, um)`.
    fn visit(
        &self,
        check: usize,
        start: u64,
        end: u64,
        stop: &AtomicUsize,
        visit: &mut dyn FnMut(u64, char, &[[f64; 2]]) -> Result<()>,
    ) -> Result<()>;
    /// Their bounding boxes: `visit(ei, box)`.
    fn visit_boxes(
        &self,
        check: usize,
        start: u64,
        end: u64,
        stop: &AtomicUsize,
        visit: &mut dyn FnMut(u64, ErrorBox) -> Result<()>,
    ) -> Result<()>;
    /// A pack's blocks (64 errors each), for block-level shortcuts.
    fn blocks(&self) -> Option<&Blocks> {
        None
    }
}

/// A pack's errors.
pub struct PackSource(pub Blocks);

impl Source for PackSource {
    fn check_count(&self) -> usize {
        self.0.check_count()
    }
    fn count(&self, check: usize) -> u64 {
        self.0.count(check)
    }
    fn precision(&self) -> f64 {
        self.0.precision
    }
    fn visit(
        &self,
        check: usize,
        start: u64,
        end: u64,
        stop: &AtomicUsize,
        visit: &mut dyn FnMut(u64, char, &[[f64; 2]]) -> Result<()>,
    ) -> Result<()> {
        let end = end.min(self.count(check));
        let precision = self.0.precision;
        let mut um: Vec<[f64; 2]> = Vec::new();
        let mut block = start / 64;
        while block * 64 < end {
            check_cancelled(stop)?;
            let base = block * 64;
            self.0.decode(check, block, &mut |j, kind, pts| {
                let ei = base + j as u64;
                if ei < start || ei >= end {
                    return Ok(());
                }
                um.clear();
                um.extend(
                    pts.iter()
                        .map(|p| [p[0] as f64 / precision, p[1] as f64 / precision]),
                );
                visit(ei, kind, &um)
            })?;
            block += 1;
        }
        Ok(())
    }
    fn visit_boxes(
        &self,
        check: usize,
        start: u64,
        end: u64,
        stop: &AtomicUsize,
        visit: &mut dyn FnMut(u64, ErrorBox) -> Result<()>,
    ) -> Result<()> {
        let end = end.min(self.count(check));
        let mut block = start / 64;
        while block * 64 < end {
            check_cancelled(stop)?;
            let base = block * 64;
            self.0.decode(check, block, &mut |j, _kind, pts| {
                let ei = base + j as u64;
                if ei < start || ei >= end {
                    return Ok(());
                }
                let mut b = [pts[0][0], pts[0][1], pts[0][0], pts[0][1]];
                for p in &pts[1..] {
                    b[0] = b[0].min(p[0]);
                    b[1] = b[1].min(p[1]);
                    b[2] = b[2].max(p[0]);
                    b[3] = b[3].max(p[1]);
                }
                visit(ei, ErrorBox::Dbu(b))
            })?;
            block += 1;
        }
        Ok(())
    }
    fn blocks(&self) -> Option<&Blocks> {
        Some(&self.0)
    }
}

/// An ASCII results file read whole.
pub struct AsciiSource(pub Arc<Ascii>);

impl Source for AsciiSource {
    fn check_count(&self) -> usize {
        self.0.checks.len()
    }
    fn count(&self, check: usize) -> u64 {
        self.0.checks.get(check).map_or(0, |c| c.count)
    }
    fn precision(&self) -> f64 {
        self.0.precision
    }
    fn visit(
        &self,
        check: usize,
        start: u64,
        end: u64,
        stop: &AtomicUsize,
        visit: &mut dyn FnMut(u64, char, &[[f64; 2]]) -> Result<()>,
    ) -> Result<()> {
        for ei in start..end.min(self.count(check)) {
            let v = self.0.error(check, ei, stop)?;
            visit(ei, v.kind, &v.points_um)?;
        }
        Ok(())
    }
    fn visit_boxes(
        &self,
        check: usize,
        start: u64,
        end: u64,
        stop: &AtomicUsize,
        visit: &mut dyn FnMut(u64, ErrorBox) -> Result<()>,
    ) -> Result<()> {
        self.visit(check, start, end, stop, &mut |ei, _kind, pts| {
            let mut b = [pts[0][0], pts[0][1], pts[0][0], pts[0][1]];
            for p in &pts[1..] {
                b[0] = b[0].min(p[0]);
                b[1] = b[1].min(p[1]);
                b[2] = b[2].max(p[0]);
                b[3] = b[3].max(p[1]);
            }
            visit(ei, ErrorBox::Um(b))
        })
    }
}

/// Review statuses for worker threads; an ASCII file has none (all 0).
#[derive(Clone)]
pub enum Statuses {
    Review(StatusReader),
    None,
}

impl Statuses {
    pub fn read(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>> {
        match self {
            Self::Review(r) => r.statuses(check, start, count),
            Self::None => Ok(vec![0; count as usize]),
        }
    }
}

/// The worker count: FLOE_DRC_JOBS, else min(4, CPUs) (floe_oracle
/// drc_native `worker_count`), 1..=256.
pub fn worker_count(jobs: Option<usize>) -> Result<usize> {
    let jobs = match jobs {
        Some(j) => Some(j),
        None => match std::env::var("FLOE_DRC_JOBS") {
            Ok(v) => Some(v.trim().parse::<usize>().map_err(|_| {
                crate::Error::input("DRC worker count must be an integer from 1 to 256")
            })?),
            Err(_) => None,
        },
    };
    match jobs {
        None => Ok(std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(4)),
        Some(j) if (1..=256).contains(&j) => Ok(j),
        Some(_) => Err(crate::Error::input(
            "DRC worker count must be an integer from 1 to 256",
        )),
    }
}

/// Run `task(i)` for `0..tasks` on `jobs` threads; the first error wins and
/// stops the others between tasks.
pub fn parallel(
    tasks: usize,
    jobs: usize,
    stop: &AtomicUsize,
    task: &(dyn Fn(usize) -> Result<()> + Sync),
) -> Result<()> {
    use std::sync::atomic::Ordering;
    let next = AtomicUsize::new(0);
    let failed = std::sync::Mutex::new(None);
    let quit = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, tasks.max(1)) {
            scope.spawn(|| loop {
                if quit.load(Ordering::Relaxed) {
                    return;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= tasks {
                    return;
                }
                let result = check_cancelled(stop).and_then(|_| task(i));
                if let Err(e) = result {
                    quit.store(true, Ordering::Relaxed);
                    let mut f = failed.lock().unwrap();
                    if f.is_none() {
                        *f = Some(e);
                    }
                    return;
                }
            });
        }
    });
    match failed.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
