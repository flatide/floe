//! Occupancy summary planes (docs/OCCUPANCY_PLAN.ko.md M2).
//!
//! At a wide view under the mask policy (`thin=keep`) a layer is not
//! decoded and rastered from its pages: its cells of the design.ovo
//! pyramid level whose cell is at most one screen pixel are projected
//! to a screen mask and styled (raster.rs `paint_summary_plane`). The
//! selection is per request and per layer: every one of the five
//! conditions must hold (keep policy, not exact, full depth, a valid
//! design.ovo matching the cache, level-0 cell <= 1 px) and the
//! layer's status in the file must be ok; anything else takes the
//! exact page path unchanged. FLOE_RUST_OCCUPANCY=off is the kill
//! switch (renderd passes it as `disabled`).

use std::sync::Arc;

use floe_vfs::occupancy::{OvoFile, STATUS_OK};

/// One layer's summary for one frame: the pyramid level to paint and
/// its grid in source dbu. Cheap to clone (the file is shared).
#[derive(Clone)]
pub struct SummaryPlane {
    pub layer_idx: u32,
    file: Arc<OvoFile>,
    layer_k: usize,
    pub level: u32,
    /// cell of `level` in dbu; the grid origin is the file's bbox corner
    pub cell_dbu: i64,
    pub x0: i64,
    pub y0: i64,
    pub w: u32,
    pub h: u32,
}

impl SummaryPlane {
    pub fn get(&self, i: u32, j: u32) -> bool {
        self.file.get(self.layer_k, self.level as usize, i, j)
    }

    /// row-padded bits of the level (row j at `j * row_bytes`)
    pub fn bits(&self) -> &[u8] {
        self.file
            .level(self.layer_k, self.level as usize)
            .map(|(_, _, bits)| bits)
            .unwrap_or(&[])
    }

    pub fn row_bytes(&self) -> usize {
        (self.w as usize + 7) / 8
    }
}

/// Why a request draws no summary (single tokens: they travel on the
/// frame line as `summary_none=`).
pub const NONE_POLICY: &str = "policy";
pub const NONE_EXACT: &str = "exact";
pub const NONE_DEPTH: &str = "depth";
pub const NONE_OFF: &str = "off";
pub const NONE_NOFILE: &str = "nofile";
pub const NONE_INVALID: &str = "invalid";
pub const NONE_NEAR: &str = "near";
pub const NONE_LAYERS: &str = "layers";

/// The summary decision of one request.
#[derive(Clone)]
pub struct SummarySelection {
    /// planes to paint, in cache layer-index order (empty = none)
    pub planes: Vec<SummaryPlane>,
    /// the pyramid level chosen (meaningful when planes is non-empty)
    pub level: u32,
    /// the file's base cell (level 0) in dbu, 0 when there is no file
    pub base_cell_dbu: i64,
    pub unit: f64,
    /// why there is no summary; None when planes is non-empty
    pub none: Option<&'static str>,
    /// identity of the file the planes came from (size, mtime), for
    /// the daemon's retained-frame key; (0, 0) without a file
    pub stamp: (u64, u64),
}

impl SummarySelection {
    pub fn none(reason: &'static str) -> Self {
        SummarySelection {
            planes: Vec::new(),
            level: 0,
            base_cell_dbu: 0,
            unit: 0.0,
            none: Some(reason),
            stamp: (0, 0),
        }
    }

    pub fn is_active(&self) -> bool {
        !self.planes.is_empty()
    }

    pub fn cell_um(&self) -> f64 {
        if self.unit > 0.0 && self.base_cell_dbu > 0 {
            (self.base_cell_dbu as f64) * (1u64 << self.level) as f64 / self.unit
        } else {
            0.0
        }
    }

    pub fn layer_indices(&self) -> Vec<u32> {
        self.planes.iter().map(|p| p.layer_idx).collect()
    }
}

/// Builds one plane per selected layer from an opened file.
pub(crate) fn planes_for(
    file: &Arc<OvoFile>,
    level: u32,
    layer_ks: impl IntoIterator<Item = (u32, usize)>,
) -> Vec<SummaryPlane> {
    let mut planes = Vec::new();
    for (layer_idx, k) in layer_ks {
        let layer = &file.layers[k];
        if layer.status != STATUS_OK {
            continue;
        }
        let Some(entry) = layer.levels.get(level as usize) else {
            continue;
        };
        planes.push(SummaryPlane {
            layer_idx,
            file: Arc::clone(file),
            layer_k: k,
            level,
            cell_dbu: file.cell_dbu << level,
            x0: file.bbox.0,
            y0: file.bbox.1,
            w: entry.w,
            h: entry.h,
        });
    }
    planes
}

/// The default bound of a summary cell on screen (pixels): the
/// coarsest level whose cell is at most this is painted.
pub const DEFAULT_MAX_CELL_PX: f64 = 1.0;

/// The bound in force: FLOE_RUST_OCCUPANCY_PX (diagnostic, e.g. 0.5
/// for the M5 A/B - finer cells close fewer gaps at 4x the paint
/// work) or the default.
pub fn max_cell_px() -> f64 {
    std::env::var("FLOE_RUST_OCCUPANCY_PX")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| *v > 0.0 && *v <= 4.0)
        .unwrap_or(DEFAULT_MAX_CELL_PX)
}

/// The coarsest level whose cell is at most `max_px` screen pixels,
/// or None when even level 0 is wider (near view).
pub fn level_for(base_cell_dbu: i64, px_per_dbu: f64, n_levels: u32, max_px: f64) -> Option<u32> {
    if base_cell_dbu <= 0 || !(px_per_dbu > 0.0) || n_levels == 0 || !(max_px > 0.0) {
        return None;
    }
    let cell_px = base_cell_dbu as f64 * px_per_dbu;
    if cell_px > max_px {
        return None;
    }
    let mut level = 0u32;
    while level + 1 < n_levels && cell_px * (1u64 << (level + 1)) as f64 <= max_px {
        level += 1;
    }
    Some(level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_is_the_coarsest_cell_at_most_one_pixel() {
        // 4 um cells at 10 um/px: 0.4 px -> 8 um 0.8 px -> 16 um 1.6 px
        assert_eq!(level_for(4000, 0.0001, 4, 1.0), Some(1));
        // exactly one pixel counts
        assert_eq!(level_for(4000, 0.000125, 4, 1.0), Some(1));
        // just over: back to level 0 (0.52 px)
        assert_eq!(level_for(4000, 0.00013, 4, 1.0), Some(0));
        // near view: level 0 wider than a pixel
        assert_eq!(level_for(4000, 0.0003, 4, 1.0), None);
        // the pyramid's top caps the level
        assert_eq!(level_for(4000, 0.000001, 3, 1.0), Some(2));
        assert_eq!(level_for(0, 0.1, 3, 1.0), None);
        // a half-pixel bound picks the finer level and narrows the near view
        assert_eq!(level_for(4000, 0.0001, 4, 0.5), Some(0));
        assert_eq!(level_for(4000, 0.00013, 4, 0.5), None);
    }
}
