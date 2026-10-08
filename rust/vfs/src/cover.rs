//! The cells' cover (hier.rs HierOpts::cell_cover; a reviewer 2026-10-05:
//! "a bright lump that comes of the stored hierarchy alone is not wanted"): the
//! area a cell's shapes cover on each layer - its own pages' by their
//! occupancy records (design.ovb, Ovm::page_occ_area) and its children's by
//! the hierarchy summary (design.ovh: each distinct child and how many of it
//! the cell places) - what a sub-cut placement of the cell stands for under
//! the density stack's brightness, where its whole box was counted (the
//! standard-cell layout's 2/0 alone drew x6.9 of its exact cover).
//!
//! Nothing here is stored: a cell's areas are worked out on first use from
//! the two files the index already has, once per open index, and shared by
//! the plans of every frame and thread.

use crate::hier::REM_FULL;
use crate::hiersum::HierSummary;
use floe_ovm::Ovm;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// A cell's covered area by layer: (layer index, dbu^2), ascending by layer
pub type LayerAreas = Arc<[(u32, f64)]>;

/// The cells' covered areas of one index (see the module).
pub struct CellCover {
    summary: Arc<HierSummary>,
    /// at full depth, cell by cell
    full: Vec<OnceLock<LayerAreas>>,
    /// with `rem` levels shown below the cell, for a cell deeper than that
    limited: Mutex<HashMap<(u32, u32), LayerAreas>>,
}

impl std::fmt::Debug for CellCover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CellCover(cells={})", self.full.len())
    }
}

impl CellCover {
    /// The cover of `ovm`'s cells by its occupancy records and `summary`, a
    /// summary of that index (render-core cells::HierHandle checks it); None
    /// without design.ovb or with a summary of another cell table.
    pub fn new(ovm: &Ovm, summary: Arc<HierSummary>) -> Option<CellCover> {
        if !ovm.has_page_occ() || summary.n_cells != ovm.n_cells {
            return None;
        }
        Some(CellCover { summary, full: (0..ovm.n_cells).map(|_| OnceLock::new()).collect(), limited: Mutex::new(HashMap::new()) })
    }

    /// The area cell `ci`'s shapes cover on each layer with `rem` levels
    /// shown below it (REM_FULL: all of them): its own pages', and from one
    /// level down its children's by their placed members.
    pub fn areas(&self, ovm: &Ovm, ci: u32, rem: u32) -> LayerAreas {
        let Some(slot) = self.full.get(ci as usize) else {
            return Arc::from([]);
        };
        if rem == REM_FULL || rem >= ovm.cell_height(ci) {
            return Arc::clone(slot.get_or_init(|| self.work_out(ovm, ci, REM_FULL)));
        }
        if let Some(known) = self.limited().get(&(ci, rem)) {
            return Arc::clone(known);
        }
        // (worked out with the table free: a child asks for its own)
        let made = self.work_out(ovm, ci, rem);
        Arc::clone(self.limited().entry((ci, rem)).or_insert(made))
    }

    fn limited(&self) -> std::sync::MutexGuard<'_, HashMap<(u32, u32), LayerAreas>> {
        match self.limited.lock() {
            Ok(table) => table,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn work_out(&self, ovm: &Ovm, ci: u32, rem: u32) -> LayerAreas {
        // its own pages, layer by layer (a page without a record covers
        // nothing: zero-area shapes)
        let mut own: Vec<(u32, f64)> = Vec::new();
        let (start, count) = ovm.cell_pranges(ci);
        for pri in start..start.saturating_add(count) {
            let pr = ovm.prange(pri);
            let area: f64 = (pr.page_lo..pr.page_lo.saturating_add(pr.page_count)).map(|pi| ovm.page_occ_area(pi).unwrap_or(0.0)).sum();
            if area > 0.0 {
                own.push((pr.layer_idx, area));
            }
        }
        let height = ovm.cell_height(ci);
        if rem == 0 || height == 0 || !self.summary.has_children(ci) {
            own.sort_by_key(|&(layer, _)| layer);
            let mut areas: Vec<(u32, f64)> = Vec::with_capacity(own.len());
            for (layer, area) in own {
                match areas.last_mut() {
                    Some(last) if last.0 == layer => last.1 += area,
                    _ => areas.push((layer, area)),
                }
            }
            return areas.into();
        }
        // with its children's by their placed members: summed by layer
        let mut sums = vec![0.0f64; ovm.n_layers as usize];
        for (layer, area) in own {
            if let Some(sum) = sums.get_mut(layer as usize) {
                *sum += area;
            }
        }
        for edge in self.summary.children(ci) {
            // (a child is lower than its parent: never around a loop)
            if edge.child >= ovm.n_cells || ovm.cell_height(edge.child) >= height {
                continue;
            }
            let below = self.areas(ovm, edge.child, if rem == REM_FULL { REM_FULL } else { rem - 1 });
            let members = edge.members as f64;
            for &(layer, area) in below.iter() {
                if let Some(sum) = sums.get_mut(layer as usize) {
                    *sum += area * members;
                }
            }
        }
        sums.iter().enumerate().filter(|(_, &area)| area > 0.0).map(|(layer, &area)| (layer as u32, area)).collect::<Vec<_>>().into()
    }

    /// Every cell's areas at full depth, worked out now (the top's ask for
    /// all below it): what a first zoomed-out frame would otherwise wait for
    /// (render-core Cache::cell_cover runs it on a thread of its own).
    pub fn warm(&self, ovm: &Ovm) {
        if ovm.top < ovm.n_cells {
            let _ = self.areas(ovm, ovm.top, REM_FULL);
        }
    }
}

/// The area the layers of `areas` that `visible` picks cover together within
/// a box of `boxed` dbu^2: one layer its own area, several as if they lay
/// independently of one another over the box (two halves cover three
/// quarters) - never more than the box.
pub fn cover_within(areas: &[(u32, f64)], boxed: f64, visible: impl Fn(u32) -> bool) -> f64 {
    if !(boxed > 0.0) {
        return 0.0;
    }
    // the part left open, as the sum of its logarithms: a layer's small
    // share of a large box keeps its digits (1 - share would drop them)
    let mut open_ln = 0.0f64;
    for &(layer, area) in areas {
        if visible(layer) {
            open_ln += (-(area / boxed).clamp(0.0, 1.0)).ln_1p();
        }
    }
    boxed * -open_ln.exp_m1()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_cover_a_box_as_if_independent_and_never_past_it() {
        let areas = [(0u32, 50.0), (1, 50.0), (3, 400.0)];
        let near = |got: f64, want: f64| (got - want).abs() <= want.abs() * 1e-12;
        assert!(near(cover_within(&areas, 100.0, |l| l == 0), 50.0));
        assert!(near(cover_within(&areas, 100.0, |l| l < 2), 75.0));
        assert_eq!(cover_within(&areas, 100.0, |_| true), 100.0);
        assert_eq!(cover_within(&areas, 100.0, |l| l == 2), 0.0);
        assert_eq!(cover_within(&areas, 0.0, |_| true), 0.0);
        // a small area in a large box is itself, not rounded away
        assert!(near(cover_within(&[(7, 12.5)], 1e18, |_| true), 12.5));
    }
}
