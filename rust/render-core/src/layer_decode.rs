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
//!   open pixel. Not built yet (plan §10 step 3); asking for it is an error
//!   rather than a silent fall back to another mode.

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
    /// pages left out because they can no longer reach an open pixel
    pub skipped_pages: u64,
    /// passes the session ran, and the layers among them
    pub passes: u64,
    pub layer_passes: u64,
    /// the frame's phases
    pub decode_us: u64,
    pub scene_us: u64,
    pub prepare_us: u64,
    pub paint_us: u64,
    pub finish_us: u64,
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
