//! DRC sources: bounded layout-4 pack queries and a streaming ASCII fallback.
//! Reads never implicitly write; build is a separate, explicitly approved job.
//! No mmap: truncation of a shared file is an I/O error rather than SIGBUS.
mod ascii;
pub mod build;
pub mod capture;
mod database;
mod filters;
mod local;
mod measure;
mod pack;
pub mod review;
mod selection;
pub use ascii::{Ascii, AsciiCheck, AsciiViolation};
pub use database::{
    Database, ReadCheck, ReadHit, ReadInfo, ReadInfoHit, ReadPage, ReadPointPage, ReadPoints,
    ReadViolation,
};
pub use filters::{ListPage, ListRequest};
pub use local::{open_current, reviewer_tag, waive_paths};
pub use measure::{cd_segments, cd_segments_um, measured, measured_um, CdSegment};
pub use pack::{
    Check, Cursor, Hit, InfoHit, InfoPage, Pack, Page, PointPage, RecordInfo, StepCursor, StepPage,
    StepRequest, Violation,
};
pub use selection::{SelectionMode, Selections, SELECTION_INPUT, SELECTION_ITEMS};

use crate::{Error, ErrorKind};
pub const MAGIC: &[u8; 8] = b"FLOEICE\0";
pub const VERSION: u32 = 4;
pub const META_BYTES: usize = 64 * 1024 * 1024;
pub const BLOCK_BYTES: usize = 16 * 1024 * 1024;
pub const BLOCK_POINTS: usize = 1024 * 1024;
pub const RECORD_POINTS: usize = 262_144;
pub const CACHE_BYTES: usize = 32 * 1024 * 1024;
pub const PAGE_ITEMS: usize = 2000;
pub const SCAN_ITEMS: u64 = 262_144;

fn corrupt(what: &str) -> Error {
    Error::new(
        ErrorKind::Cache,
        format!("corrupt DRC pack: {what}; rebuild with floe-index drc <db>"),
    )
}
fn limit(what: &str) -> Error {
    Error::new(
        ErrorKind::Incomplete,
        format!("DRC {what} exceeds the read limit; no partial geometry returned"),
    )
}

#[cfg(test)]
mod tests;
