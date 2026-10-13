//! Box selection over a whole rule, cluster or delta group, with no count
//! limit (floe_oracle/drc_selection.py `select_rect`): every error of the
//! scope whose exact bbox meets the box - a pack's blocks wholly outside or
//! inside it decided without decoding - combined with the earlier selection
//! (replace, add, toggle, remove, intersect).
use super::scope::{Membership, Selection};
use super::source::{ErrorBox, Source, Statuses};
use crate::{check_cancelled, Error, Result};
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

const CHUNK: u64 = 1 << 16;

/// What a box selection asks.
pub struct Request<'a> {
    pub check: usize,
    /// the box, um (None: the scope and filter alone)
    pub bounds_um: Option<[f64; 4]>,
    pub members: Option<&'a dyn Membership>,
    pub waived: Option<bool>,
    pub previous: Option<&'a Selection>,
    pub mode: &'a str,
}

/// Build the selection; `progress(processed, total, found)` about every
/// half second.
pub fn select(
    source: &dyn Source,
    statuses: &Statuses,
    r: &Request,
    stop: &AtomicUsize,
    progress: &mut dyn FnMut(u64, u64, u64),
) -> Result<Selection> {
    let n = source.count(r.check);
    if !matches!(r.mode, "replace" | "add" | "toggle" | "remove" | "intersect") {
        return Err(Error::input("invalid selection mode"));
    }
    if r.previous.is_some_and(|p| p.size != n) {
        return Err(Error::input("previous selection belongs to a different rule size"));
    }
    if let Some(b) = r.bounds_um {
        if !b.iter().all(|v| v.is_finite()) {
            return Err(Error::input("selection bounds must contain four finite values"));
        }
    }
    let precision = source.precision();
    let um = |v: i64| v as f64 / precision;
    let mut words = vec![0u64; n.div_ceil(64) as usize];
    let mut found = 0u64;
    let mut last = Instant::now();
    let mut keep = vec![false; CHUNK as usize];
    let mut old = vec![false; CHUNK as usize];
    let mut start = 0;
    while start < n {
        check_cancelled(stop)?;
        let len = (n - start).min(CHUNK) as usize;
        let keep = &mut keep[..len];
        match r.members {
            Some(m) => m.mask(start, keep),
            None => keep.fill(true),
        }
        if let Some(want) = r.waived {
            let st = statuses.read(r.check, start, len as u64)?;
            for (k, s) in keep.iter_mut().zip(&st) {
                *k &= (*s == 1) == want;
            }
        }
        if let (Some(b), true) = (r.bounds_um, keep.iter().any(|&k| k)) {
            let [bx0, by0, bx1, by1] = b;
            if bx0 > bx1 || by0 > by1 {
                keep.fill(false);
            } else {
                let hits = |e: [f64; 4]| e[0] <= bx1 && e[2] >= bx0 && e[1] <= by1 && e[3] >= by0;
                // a pack's blocks: wholly outside or inside the box decide
                // their members at once
                let mut exact = vec![true; len];
                if let Some(blocks) = source.blocks() {
                    let first = start / 64;
                    let boxes = blocks.block_boxes(r.check, first, (len as u64).div_ceil(64))?;
                    for (i, bb) in boxes.iter().enumerate() {
                        let e = [um(bb[0]), um(bb[1]), um(bb[2]), um(bb[3])];
                        let (a, z) = (i * 64, ((i + 1) * 64).min(len));
                        if !hits(e) {
                            keep[a..z].fill(false);
                            exact[a..z].fill(false);
                        } else if e[0] >= bx0 && e[2] <= bx1 && e[1] >= by0 && e[3] <= by1 {
                            exact[a..z].fill(false);
                        }
                    }
                }
                // the rest: decode only blocks with an undecided member
                let mut b0 = 0;
                while b0 < len {
                    let b1 = (b0 + 64).min(len);
                    if (b0..b1).any(|j| keep[j] && exact[j]) {
                        let base = start + b0 as u64;
                        source.visit_boxes(r.check, base, start + b1 as u64, stop, &mut |ei, eb| {
                            let j = (ei - start) as usize;
                            if keep[j] && exact[j] {
                                let e = match eb {
                                    ErrorBox::Dbu(d) => [um(d[0]), um(d[1]), um(d[2]), um(d[3])],
                                    ErrorBox::Um(u) => u,
                                };
                                keep[j] = hits(e);
                            }
                            Ok(())
                        })?;
                    }
                    b0 = b1;
                }
            }
        }
        if r.mode != "replace" {
            let old = &mut old[..len];
            match r.previous {
                Some(p) => p.mask(start, old),
                None => old.fill(false),
            }
            for (k, o) in keep.iter_mut().zip(old.iter()) {
                *k = match r.mode {
                    "add" => *o || *k,
                    "toggle" => *o ^ *k,
                    "remove" => *o && !*k,
                    _ => *o && *k,
                };
            }
        }
        for (j, &k) in keep.iter().enumerate() {
            if k {
                let ei = start + j as u64;
                words[(ei / 64) as usize] |= 1 << (ei % 64);
                found += 1;
            }
        }
        start += len as u64;
        if last.elapsed() >= Duration::from_millis(500) {
            last = Instant::now();
            progress(start, n, found);
        }
    }
    check_cancelled(stop)?;
    Ok(Selection::from_words(n, words))
}
