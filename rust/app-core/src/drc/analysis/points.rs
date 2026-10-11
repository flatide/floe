//! Whole-scope DRC point markers (floe_oracle/drc_points.py, drc_markers):
//! every selected error whose exact bbox centre lies in the view becomes a
//! 5 px circle - errors past the list page and past any count included -
//! one per screen pixel: there the selected error, else the first not
//! waived, else the first. The answer is an RGBA image and a pixel map of
//! error ids, bounded by the view's size, never a list of errors.
//!
//! A pack's centres are cached per rule, 16 bytes an error: the sums of the
//! integer bbox extremes (exact, half-DBU centres kept), built in parallel
//! the first time and published whole; a build in progress already feeds
//! the picture.
use super::scope::Membership;
use super::source::{parallel, ErrorBox, Source, Statuses};
use super::store::{self, BuildLock, Column, Sink, Staging};
use crate::{check_cancelled, Error, ErrorKind, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const VERSION: u32 = 1;
const TASK: u64 = 1 << 16;
const UNSELECTED: u64 = 1 << 63;
const WAIVED: u64 = 1 << 62;
const ID_MASK: u64 = WAIVED - 1;
const EMPTY: u64 = u64::MAX;
pub const RED: u32 = 0xFF52_52FF;
pub const GREEN: u32 = 0x00E6_76FF;
pub const GOLD: u32 = 0xFFD7_00FF;
/// A partial picture at most this often.
pub const PROGRESS: Duration = Duration::from_millis(500);

/// A rule's cached centres: (x_min + x_max, y_min + y_max) in dbu.
pub struct Centers {
    pub scale: f64,
    pairs: Column<i64>,
}
impl Centers {
    pub fn get(&self, i: u64) -> (f64, f64) {
        let p = self.pairs.as_slice();
        (
            p[2 * i as usize] as f64 / self.scale,
            p[2 * i as usize + 1] as f64 / self.scale,
        )
    }
    pub fn len(&self) -> u64 {
        self.pairs.len() as u64 / 2
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn center_dir(cache: &super::delta::Cache, check: usize) -> PathBuf {
    cache
        .dir
        .join(format!("centers-r{VERSION}-{}-{check}", cache.identity))
}

fn center_key(cache: &super::delta::Cache, check: usize, n: u64, precision: f64) -> serde_json::Value {
    json!({"version": VERSION, "pack": cache.identity, "rule": check, "count": n, "precision": precision})
}

/// The cached centres of rule `check`, when complete.
pub fn cached_centers(cache: &super::delta::Cache, check: usize, n: u64, precision: f64) -> Option<Centers> {
    let dir = center_dir(cache, check);
    store::manifest(&dir, &center_key(cache, check, n, precision))?;
    Some(Centers {
        scale: 2.0 * precision,
        pairs: Column::map(&dir.join("centers.bin"), 2 * n as usize).ok()?,
    })
}

/// Visit rule `check`'s centres (um) by chunks on `jobs` threads:
/// `consume(start, centres)` - from the cache, or decoded (a pack's cache
/// built on the way when `cache` is given). Chunks may come in any order.
pub fn visit_centers(
    source: &dyn Source,
    check: usize,
    cache: Option<&super::delta::Cache>,
    jobs: usize,
    stop: &AtomicUsize,
    consume: &(dyn Fn(u64, &[(f64, f64)]) -> Result<()> + Sync),
) -> Result<()> {
    let n = source.count(check);
    let precision = source.precision();
    let tasks = n.div_ceil(TASK) as usize;
    if let Some(cache) = cache {
        if let Some(c) = cached_centers(cache, check, n, precision) {
            return parallel(tasks, jobs, stop, &|i| {
                let start = i as u64 * TASK;
                let end = (start + TASK).min(n);
                let chunk: Vec<(f64, f64)> = (start..end).map(|ei| c.get(ei)).collect();
                consume(start, &chunk)
            });
        }
        if source.blocks().is_some() {
            match build_centers(source, check, cache, jobs, stop, consume) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
                // the build fed `consume` before failing: no second pass
                Err(e) => return Err(e),
            }
        }
    }
    parallel(tasks, jobs, stop, &|i| {
        let start = i as u64 * TASK;
        let end = (start + TASK).min(n);
        let mut chunk = Vec::with_capacity((end - start) as usize);
        source.visit_boxes(check, start, end, stop, &mut |_ei, b| {
            chunk.push(match b {
                ErrorBox::Dbu(b) => (
                    (b[0] + b[2]) as f64 / (2.0 * precision),
                    (b[1] + b[3]) as f64 / (2.0 * precision),
                ),
                ErrorBox::Um(b) => (b[0] / 2.0 + b[2] / 2.0, b[1] / 2.0 + b[3] / 2.0),
            });
            Ok(())
        })?;
        consume(start, &chunk)
    })
}

fn build_centers(
    source: &dyn Source,
    check: usize,
    cache: &super::delta::Cache,
    jobs: usize,
    stop: &AtomicUsize,
    consume: &(dyn Fn(u64, &[(f64, f64)]) -> Result<()> + Sync),
) -> Result<()> {
    let n = source.count(check);
    let precision = source.precision();
    let dir = center_dir(cache, check);
    let key = center_key(cache, check, n, precision);
    // a folder that cannot be written still draws (no cache)
    let setup = (|| -> Result<(BuildLock, Staging, Sink<i64>)> {
        let lock = BuildLock::take(&dir, stop)?;
        let staging = Staging::new(&dir)?;
        let sink = Sink::<i64>::create(&staging.dir.join("centers.bin"), 2 * n)?;
        Ok((lock, staging, sink))
    })();
    let (lock, staging, sink) = match setup {
        Ok(s) => (Some(s.0), Some(s.1), Some(s.2)),
        Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
        Err(_) => (None, None, None),
    };
    if lock.is_some() {
        if let Some(c) = cached_centers(cache, check, n, precision) {
            // another builder finished while this one waited
            drop(staging);
            let tasks = n.div_ceil(TASK) as usize;
            return parallel(tasks, jobs, stop, &|i| {
                let start = i as u64 * TASK;
                let end = (start + TASK).min(n);
                let chunk: Vec<(f64, f64)> = (start..end).map(|ei| c.get(ei)).collect();
                consume(start, &chunk)
            });
        }
    }
    let tasks = n.div_ceil(TASK) as usize;
    let scale = 2.0 * precision;
    parallel(tasks, jobs, stop, &|i| {
        let start = i as u64 * TASK;
        let end = (start + TASK).min(n);
        let mut pairs = Vec::with_capacity(2 * (end - start) as usize);
        source.visit_boxes(check, start, end, stop, &mut |_ei, b| {
            match b {
                ErrorBox::Dbu(b) => {
                    pairs.push(b[0] + b[2]);
                    pairs.push(b[1] + b[3]);
                }
                ErrorBox::Um(_) => return Err(Error::input("centre cache needs a pack")),
            }
            Ok(())
        })?;
        if let Some(sink) = &sink {
            sink.put(2 * start, &pairs)?;
        }
        let chunk: Vec<(f64, f64)> = pairs
            .chunks_exact(2)
            .map(|p| (p[0] as f64 / scale, p[1] as f64 / scale))
            .collect();
        consume(start, &chunk)
    })?;
    if let (Some(staging), Some(sink)) = (staging, sink) {
        sink.finish()?;
        check_cancelled(stop)?;
        let published = store::write_manifest(&staging.dir, &json!({"key": key}))
            .and_then(|_| staging.publish());
        // an unpublished cache is only a cache: the picture stands
        let _ = published;
    }
    drop(lock);
    Ok(())
}

/// A marker picture: RGBA rows top-down and per pixel the error drawn there
/// (rule-local index, -1 none), with what it counted.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub ids: Vec<i64>,
    /// errors of the scope whose centre is in view (duplicates included)
    pub visible: u64,
    /// pixels drawn
    pub occupied: u64,
    pub processed: u64,
    pub total: u64,
}

impl Picture {
    /// The marker file the GUI reads: `FLOEMRK1`, width, height (u32),
    /// visible, occupied, processed, total (u64), RGBA, then i64 ids.
    pub fn write(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        f.write_all(b"FLOEMRK1")?;
        f.write_all(&self.width.to_le_bytes())?;
        f.write_all(&self.height.to_le_bytes())?;
        for v in [self.visible, self.occupied, self.processed, self.total] {
            f.write_all(&v.to_le_bytes())?;
        }
        f.write_all(&self.rgba)?;
        f.write_all(store::bytes_of(&self.ids))?;
        f.into_inner().map_err(|e| e.into_error())?;
        Ok(())
    }
}

/// The 5 px solid circle's coverage (floe_oracle drc_marker_style
/// `circle_rgba(2, RED, solid=True)`'s alpha).
fn sprite() -> [[u8; 5]; 5] {
    let mut out = [[0u8; 5]; 5];
    for (row, yy) in (-2i32..=2).enumerate() {
        for (col, xx) in (-2i32..=2).enumerate() {
            let d = f64::from(xx * xx + yy * yy).sqrt();
            let coverage = (2.5 - d).clamp(0.0, 1.0);
            let alpha = 255.0 * coverage;
            out[row][col] = (alpha * 255.0 / 255.0).round_ties_even() as u8;
        }
    }
    out
}

/// What a marker query asks.
pub struct Query<'a> {
    pub bounds_um: [f64; 4],
    pub width: u32,
    pub height: u32,
    pub check: usize,
    /// the scope (None: the whole rule)
    pub members: Option<&'a dyn Membership>,
    /// review filter (None: any)
    pub waived: Option<bool>,
    /// highlighted errors (gold, drawn first)
    pub selected: Option<&'a dyn Membership>,
}

/// Draw `query`: `partial(picture)` about every half second while it runs.
pub fn query(
    source: &dyn Source,
    statuses: &Statuses,
    cache: Option<&super::delta::Cache>,
    q: &Query,
    jobs: usize,
    stop: &AtomicUsize,
    partial: &(dyn Fn(Picture) + Sync),
) -> Result<Picture> {
    let [x0, y0, x1, y1] = q.bounds_um;
    if !q.bounds_um.iter().all(|v| v.is_finite()) {
        return Err(Error::input("marker bounds must contain four finite values"));
    }
    let (w, h) = (q.width as usize, q.height as usize);
    if w.checked_mul(h).is_none_or(|p| p > 64 << 20) {
        return Err(Error::input("marker picture too large"));
    }
    let ranks: Vec<AtomicU64> = (0..w * h).map(|_| AtomicU64::new(EMPTY)).collect();
    let n = source.count(q.check);
    let visible = AtomicU64::new(0);
    let processed = AtomicU64::new(0);
    let last = Mutex::new(Instant::now());
    let snapshot = || picture(&ranks, w, h, &visible, &processed, n);
    if w > 0 && h > 0 && x0 < x1 && y0 < y1 && n > 0 {
        let fx = w as f64 / (x1 - x0);
        let fy = h as f64 / (y1 - y0);
        visit_centers(source, q.check, cache, jobs, stop, &|start, centers| {
            let len = centers.len();
            let mut keep = vec![true; len];
            if let Some(m) = q.members {
                m.mask(start, &mut keep);
            }
            let status = statuses.read(q.check, start, len as u64)?;
            let mut chosen = vec![false; len];
            if let Some(s) = q.selected {
                s.mask(start, &mut chosen);
            }
            let mut seen = 0;
            for (j, &(cx, cy)) in centers.iter().enumerate() {
                if !(keep[j] && cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1) {
                    continue;
                }
                let waived = status[j] == 1;
                if let Some(want) = q.waived {
                    if waived != want {
                        continue;
                    }
                }
                seen += 1;
                // closed view edges land on the last pixel
                let xp = ((cx - x0) * fx).round_ties_even().clamp(0.0, (w - 1) as f64) as usize;
                let yp = ((y1 - cy) * fy).round_ties_even().clamp(0.0, (h - 1) as f64) as usize;
                let mut rank = start + j as u64;
                if waived {
                    rank |= WAIVED;
                }
                if q.selected.is_none() || !chosen[j] {
                    rank |= UNSELECTED;
                }
                ranks[yp * w + xp].fetch_min(rank, Ordering::Relaxed);
            }
            visible.fetch_add(seen, Ordering::Relaxed);
            processed.fetch_add(len as u64, Ordering::Relaxed);
            let mut l = last.lock().unwrap();
            if l.elapsed() >= PROGRESS {
                *l = Instant::now();
                drop(l);
                check_cancelled(stop)?;
                partial(snapshot());
            }
            Ok(())
        })?;
    } else {
        processed.store(0, Ordering::Relaxed);
    }
    check_cancelled(stop)?;
    Ok(snapshot())
}

fn picture(ranks: &[AtomicU64], w: usize, h: usize, visible: &AtomicU64, processed: &AtomicU64, total: u64) -> Picture {
    let r: Vec<u64> = ranks.iter().map(|a| a.load(Ordering::Relaxed)).collect();
    let sprite = sprite();
    let mut rgba = vec![0u8; w * h * 4];
    let mut ids = vec![-1i64; w * h];
    let mut occupied = 0;
    for (i, &v) in r.iter().enumerate() {
        if v != EMPTY {
            ids[i] = (v & ID_MASK) as i64;
            occupied += 1;
        }
    }
    // green (waived), then red (not waived), then gold (selected): each
    // colour's circles over the earlier ones wherever it covers a pixel
    let classes: [(u32, &dyn Fn(u64) -> bool); 3] = [
        (GREEN, &|v| v != EMPTY && v >= UNSELECTED && v & WAIVED != 0),
        (RED, &|v| v != EMPTY && v >= UNSELECTED && v & WAIVED == 0),
        (GOLD, &|v| v != EMPTY && v < UNSELECTED),
    ];
    let mut alpha = vec![0u8; w * h];
    for (color, member) in classes {
        if !r.iter().any(|&v| member(v)) {
            continue;
        }
        alpha.iter_mut().for_each(|a| *a = 0);
        for (i, &v) in r.iter().enumerate() {
            if !member(v) {
                continue;
            }
            let (px, py) = ((i % w) as i64, (i / w) as i64);
            for (sy, row) in sprite.iter().enumerate() {
                for (sx, &coverage) in row.iter().enumerate() {
                    if coverage == 0 {
                        continue;
                    }
                    let (x, y) = (px + sx as i64 - 2, py + sy as i64 - 2);
                    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                        continue;
                    }
                    let a = &mut alpha[y as usize * w + x as usize];
                    *a = (*a).max(coverage);
                }
            }
        }
        let rgb = [(color >> 24) as u8, (color >> 16) as u8, (color >> 8) as u8];
        for (i, &a) in alpha.iter().enumerate() {
            if a > 0 {
                rgba[i * 4..i * 4 + 3].copy_from_slice(&rgb);
                rgba[i * 4 + 3] = a;
            }
        }
    }
    Picture {
        width: w as u32,
        height: h as u32,
        rgba,
        ids,
        visible: visible.load(Ordering::Relaxed),
        occupied,
        processed: processed.load(Ordering::Relaxed),
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sprite_is_the_python_circle() {
        let s = sprite();
        assert_eq!(s[2], [128, 255, 255, 255, 128]);
        assert_eq!(s[0], [0, 67, 128, 67, 0]);
    }
}
