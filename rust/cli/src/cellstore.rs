//! What the per-cell pipeline of the index build - the cell planner,
//! the text index, the page encoder, the ordered committer and the
//! meta writer - reads of a parsed source: one cell at a time by Doc
//! index, plus the global tables. The in-memory build hands the whole
//! Doc through it (`impl CellStore for Doc`); the streaming build
//! (docs/STREAMING_INDEX_PLAN.ko.md S4) hands out the cells it has
//! resident and the inventory's tables, and drops a cell once its
//! pages are written. The refactor behind this trait (S2) changes no
//! output byte: gate G0 compares the caches a pre-S2 floe-index and
//! this one build from the same sources.

use floe_oasis::doc::{Cell, Doc};
use std::collections::HashMap;
use std::ops::{Deref, Range};
use std::sync::Arc;

/// a cell handed out by a store: borrowed from a resident Doc, or
/// shared with a streaming store that lets go of its own reference
/// once the cell's pages are written
pub enum CellRef<'a> {
    Borrowed(&'a Cell),
    /// constructed by the streaming store (S4)
    #[allow(dead_code)]
    Shared(Arc<Cell>),
}

impl Deref for CellRef<'_> {
    type Target = Cell;
    fn deref(&self) -> &Cell {
        match self {
            CellRef::Borrowed(c) => c,
            CellRef::Shared(c) => c,
        }
    }
}

pub trait CellStore: Sync {
    /// cells in Doc index order (CELL record order)
    fn n_cells(&self) -> usize;
    fn unit(&self) -> f64;
    fn top(&self) -> usize;
    /// (layer, datatype) pairs in file-appearance order
    fn layer_order(&self) -> &[(u32, u32)];
    fn layer_names(&self) -> &HashMap<(u32, u32), String>;
    fn layer_aliases(&self) -> &HashMap<(u32, u32), Vec<String>>;
    /// a cell's name, resident or not
    fn cell_name(&self, ci: usize) -> &str;
    /// the cell's records. Residency contract (what build() reads): a
    /// cell is first asked for by its planner (inside the admission
    /// window past the commit point), again by the committer and the
    /// text index when it commits, and once more for every page of
    /// the encode chunk that holds it - the encoder re-reads records
    /// by index - so a streaming store keeps a cell from its first
    /// `cell(ci)` until `release` names it, i.e. up to two windows of
    /// cells (the chunk being encoded and the plans past it) are
    /// resident at a flush.
    fn cell(&self, ci: usize) -> CellRef<'_>;
    /// the pages of `cells` are written (the committer's chunk flush,
    /// where its arenas drop): a streaming store may let them go. The
    /// Doc keeps everything.
    fn release(&self, _cells: Range<usize>) {}
    /// a streaming store under memory pressure (its byte budget spent
    /// on committed cells the count window has not flushed yet) asks
    /// the committer to flush the chunk now; without this a byte-
    /// budgeted reader and the count-only flush predicate deadlock.
    fn flush_wanted(&self) -> bool {
        false
    }
    /// a streaming store whose reader failed on a range the planners
    /// still wait for says so here; the committer polls it while it
    /// waits and aborts the build (the store wakes its waiters too)
    fn failed(&self) -> Option<String> {
        None
    }
    /// the build is aborting (a planner or the committer panicked): a
    /// streaming store wakes every planner parked in `cell` so build()'s
    /// thread scope can join and the panic surface
    fn abort(&self, _why: &str) {}
}

impl CellStore for Doc {
    fn n_cells(&self) -> usize {
        self.cells.len()
    }
    fn unit(&self) -> f64 {
        self.unit
    }
    fn top(&self) -> usize {
        self.top
    }
    fn layer_order(&self) -> &[(u32, u32)] {
        &self.layer_order
    }
    fn layer_names(&self) -> &HashMap<(u32, u32), String> {
        &self.layer_names
    }
    fn layer_aliases(&self) -> &HashMap<(u32, u32), Vec<String>> {
        &self.layer_aliases
    }
    fn cell_name(&self, ci: usize) -> &str {
        &self.cells[ci].name
    }
    fn cell(&self, ci: usize) -> CellRef<'_> {
        CellRef::Borrowed(&self.cells[ci])
    }
}
