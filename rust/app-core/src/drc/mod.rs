//! DRC sources: bounded layout-4 pack queries and a streaming ASCII fallback.
//! Reads never implicitly write; build is a separate, explicitly approved job.
//! No mmap: truncation of a shared file is an I/O error rather than SIGBUS.
pub mod analysis;
mod ascii;
pub mod build;
pub mod capture;
mod database;
pub mod desktop;
mod filters;
mod local;
pub mod matcher;
mod measure;
mod pack;
pub mod reasons;
pub mod review;
mod selection;
pub mod session;
#[cfg(feature = "review-sqlite")]
pub mod team;
pub use ascii::{Ascii, AsciiCheck, AsciiViolation};
pub use database::{
    Database, ReadCheck, ReadHit, ReadInfo, ReadInfoHit, ReadPage, ReadPointPage, ReadPoints,
    ReadViolation,
};
pub use filters::{ListPage, ListRequest};
pub use local::{
    is_packed_source, open_current, open_pack, required_reviewer, reviewer, reviewer_required,
    reviewer_tag,
    select_current_source, select_review, waive_paths, ReadSelection, Reviewer, ReviewerSource,
    SourceSelection,
};
pub use measure::{cd_segments, cd_segments_um, measured, measured_um, CdSegment};
pub use pack::{
    Blocks, Check, Cursor, Hit, InfoHit, InfoPage, Pack, Page, PointPage, RecordInfo, StepCursor, StepPage,
    StepRequest, Violation,
};
pub use selection::{SelectionMode, Selections, SELECTION_INPUT, SELECTION_ITEMS};

use crate::{Error, ErrorKind};
pub const MAGIC: &[u8; 8] = b"FLOEICE\0";
/// The pack's layout revision: 5 since 0.12.441 (the footer carries the
/// source's CRC-32 and the content id reviews bind to).
pub const VERSION: u32 = 5;
/// The pack's footer.
pub const FOOTER_BYTES: u64 = 168;
pub const META_BYTES: usize = 64 * 1024 * 1024;
pub const BLOCK_BYTES: usize = 16 * 1024 * 1024;
pub const BLOCK_POINTS: usize = 1024 * 1024;
pub const RECORD_POINTS: usize = 262_144;
pub const CACHE_BYTES: usize = 32 * 1024 * 1024;
pub const PAGE_ITEMS: usize = 2000;
pub const SCAN_ITEMS: u64 = 262_144;

/// Hold `pack` as a reader (a re-pack waits, as for a review).
pub fn pack_reader_lock(pack: &std::path::Path) -> crate::Result<floe_vfs::lock::ReaderGuard> {
    local::pack_reader(pack)
}

/// Whether no one holds `pack` now - no reader, no run writing it: what a
/// clean-up of the files derived from it asks before it removes any (the
/// analysis caches of other identities, `floe2 drc-prepare`). A probe, as
/// `floe2 index --force --wait` probes a cache: no lock is kept, and where
/// no lock can be tried nothing is known and it says yes.
pub fn pack_idle(pack: &std::path::Path) -> bool {
    let key = floe_vfs::lock::key(floe_vfs::lock::Kind::Pack, &pack.to_string_lossy());
    floe_vfs::lock::reader_refusal(&key).is_none() && floe_vfs::lock::writing_refusal(&key).is_none()
}

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
