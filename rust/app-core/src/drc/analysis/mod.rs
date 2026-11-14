//! The desktop DRC review's analysis (feature/jobdeck-drc, ported to Rust
//! 2026-10-11 - floe_oracle drc_delta, drc_clusters, drc_points and
//! drc_selection are the reference): a marker's rule-aware CD and its
//! fixed-precision delta.
pub mod measure;
pub mod ticks;
pub mod clusters;

// (what a cluster counts as waived is the class the review's lens shows:
// the waived errors with no filter of states on - `desktop::Lens`)
impl clusters::Statuses for super::desktop::Review {
    fn statuses(&self, check: usize, start: u64, count: u64) -> crate::Result<Vec<u8>> {
        self.lensed(check, start, count)
    }
    // (always known for a review: None is "no review state" to a cluster,
    // which takes the rule for not waived whole)
    fn rule_counts(&self, check: usize) -> Option<(u64, u64)> {
        self.lens_counts(check).ok()
    }
}
pub mod scope;
pub mod source;
pub mod store;
pub mod delta;
pub mod points;
pub mod select;
