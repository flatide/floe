//! Layer-ordered decode probe (docs/LAYER_DECODE_PROBE_PLAN.ko.md).
//!
//! F2R-28's measurement of 2026-09-20 found that with every layer visible
//! 92-99 % of the pages a frame decodes never light a pixel: the raster skips
//! what an upper layer already covered, but only after the decode. The probe
//! compares three ways of painting the SAME plan, so the decode a layer-ordered
//! frame could skip can be measured before anything about it is made the
//! default.
//!
//! * `baseline` - the normal path: decode the whole selection, one raster.
//! * `ordered` - the same selection, painted by `LayerRasterSession` one pass
//!   at a time, top layer first. It measures what the order itself costs.
//! * `occlusion` - `ordered` without the pages that can no longer reach an
//!   open pixel, decided at each block's start from the masks that block
//!   starts with.
//!
//! `ordered` and `occlusion` of the SAME block size are the pair to compare:
//! only that pair separates the decode the coverage saves from what stopping
//! between blocks costs.

/// Which of the three ways of painting one plan the probe runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeMode {
    Baseline,
    Ordered,
    Occlusion,
}

impl ProbeMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "baseline" => Ok(ProbeMode::Baseline),
            "ordered" => Ok(ProbeMode::Ordered),
            "occlusion" => Ok(ProbeMode::Occlusion),
            other => Err(format!(
                "probe mode must be baseline, ordered or occlusion: {other}"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ProbeMode::Baseline => "baseline",
            ProbeMode::Ordered => "ordered",
            ProbeMode::Occlusion => "occlusion",
        }
    }
}

/// What one probe run did, beside the frame itself. Times are microseconds of
/// wall clock in the driver, so they add up to the run and no new cost hides
/// inside another phase (plan §8).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayerProbeReport {
    pub mode: &'static str,
    /// pages of the plan, and of the selection the decode budget left
    pub planned_pages: u64,
    pub selected_pages: u64,
    /// pages actually handed to the page cache, and what it had to read
    pub requested_pages: u64,
    pub decoded_pages: u64,
    pub cache_hits: u64,
    /// pages of the selection never asked for, and their STORED bytes
    /// (what the read would have cost; a page's decoded size is not known
    /// without decoding it)
    pub skipped_pages: u64,
    pub skipped_bytes: u64,
    /// the decoded memory the pages that were read charge to the budget
    pub decoded_bytes: u64,
    pub cache_misses: u64,
    /// (page, instance) pairs the demand looked at, and why they were left
    /// out: outside the frame, written to the last pixel, or a deferred edge
    /// the probe does not walk (its layer is then asked for whole)
    pub demand_candidates: u64,
    pub demand_out_of_view: u64,
    pub demand_occluded: u64,
    pub demand_unsure: u64,
    /// passes the session ran, the blocks they were painted in (one stop of
    /// every worker each) and the layers among the passes
    pub passes: u64,
    pub blocks: u64,
    pub layer_passes: u64,
    /// the frame's phases. `prepare_us + paint_us` is the raster in every
    /// mode - the normal path prepares inside its one render call, so it
    /// reports that as paint and leaves prepare at 0 - and `decode_us` and
    /// `demand_us` are taken out of a layered run's paint, where they happen
    /// between blocks, on the driver's thread.
    pub decode_us: u64,
    pub read_us: u64,
    pub decode_sum_us: u64,
    pub demand_us: u64,
    /// raising the decode workers and letting them go: neither decode nor
    /// raster, and only a layered run has it
    pub pool_us: u64,
    pub scene_us: u64,
    pub prepare_us: u64,
    pub paint_us: u64,
    pub total_us: u64,
}

impl LayerProbeReport {
    pub fn new(mode: ProbeMode) -> Self {
        LayerProbeReport {
            mode: mode.name(),
            ..Default::default()
        }
    }
}
