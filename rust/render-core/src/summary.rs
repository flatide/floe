//! Occupancy summary planes (docs/OCCUPANCY_PLAN.ko.md M2).
//!
//! At a wide view a layer is not decoded and rastered from its pages:
//! its cells of the design.ovo pyramid level whose cell is at most one
//! screen pixel are projected to a screen mask and styled (raster.rs
//! `paint_summary_plane`). The selection is per request and per layer:
//! every one of the five conditions must hold (the policy: keep, or
//! cull since 2026-09-18 unless FLOE_RUST_OCCUPANCY_CULL=off - under
//! cull the near view then returns to the cull page path, which drops
//! the sub-cut pages; not exact; a valid design.ovo
//! matching the cache; level-0 cell <= 1 px; and the depth: with a
//! version-2 file - one bit plane per placement depth, 2026-09-16 -
//! any depth qualifies and the planes at or above the request depth
//! are drawn, exactly the shapes the page path draws there; with a
//! version-1 file, or under FLOE_RUST_OCCUPANCY_DEPTH=off, only a
//! depth that draws the layer whole - unlimited, at least the
//! hierarchy height, or at least the layer's deepest page-holding
//! cell) and the layer's status in the file must be ok; anything
//! else takes the exact page path unchanged. FLOE_RUST_OCCUPANCY=off
//! is the kill switch (renderd passes it as `disabled`).

use std::sync::Arc;

use floe_vfs::occupancy::{OvoFile, DEPTH_CAP, STATUS_EMPTY, STATUS_OK};

/// One layer's summary for one frame: the pyramid level to paint and
/// its grid in source dbu; the bits are the OR of the file's planes
/// the request depth draws. Cheap to clone (the bits are shared).
#[derive(Clone)]
pub struct SummaryPlane {
    pub layer_idx: u32,
    bits: Arc<[u8]>,
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
        if i >= self.w || j >= self.h {
            return false;
        }
        (self.bits[j as usize * self.row_bytes() + (i / 8) as usize] >> (i % 8)) & 1 == 1
    }

    /// row-padded bits of the level (row j at `j * row_bytes`)
    pub fn bits(&self) -> &[u8] {
        &self.bits
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
/// a plain layout's frame (not a jobdeck pass): the summary is off by
/// default there (user decision 2026-09-24); FLOE_RUST_OCCUPANCY=on or the
/// request's `occupancy=on` (the viewer's toggle, 2026-10-20) uses it
pub const NONE_LAYOUT: &str = "layout";

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

/// The combined bits of (layer k, level, depth key) of one file, kept
/// by the cache across frames: a level-0 plane of a 30 mm source is
/// megabytes, and a deck asks per pass. Bounded in entries and in bytes
/// (PLANE_CACHE_CAP, PLANE_CACHE_BYTES): the whole is emptied when an
/// entry would pass either, as the entry count did alone until 0.12.395
/// - with the generator's caps at 2^32 cells and 4 GiB (2026-10-24) a
/// whole-reticle layer at 4 um is 181 MB at level 0 and 242 MB over its
/// levels, and a count alone could hold gigabytes.
pub struct PlaneCache {
    map: std::collections::HashMap<(usize, u32, u32), Arc<[u8]>>,
    bytes: usize,
    cap: usize,
    max_bytes: usize,
}

/// combined planes kept before the cache is emptied (a session that
/// walks many depths and levels stays bounded)
const PLANE_CACHE_CAP: usize = 512;
/// bytes of combined planes kept before the cache is emptied: two
/// whole-reticle layers at 4 um over all their levels, or one at two
/// depth keys
pub const PLANE_CACHE_BYTES: usize = 512 << 20;

impl Default for PlaneCache {
    fn default() -> Self {
        Self::with_bounds(PLANE_CACHE_CAP, PLANE_CACHE_BYTES)
    }
}

impl PlaneCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// a cache emptied past `cap` entries or `max_bytes` bytes
    pub fn with_bounds(cap: usize, max_bytes: usize) -> Self {
        PlaneCache {
            map: std::collections::HashMap::new(),
            bytes: 0,
            cap: cap.max(1),
            max_bytes,
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// bytes of the planes held
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn keys(&self) -> impl Iterator<Item = &(usize, u32, u32)> {
        self.map.keys()
    }

    pub fn get(&self, key: &(usize, u32, u32)) -> Option<Arc<[u8]>> {
        self.map.get(key).cloned()
    }

    /// Keeps `bits` under `key`; the cache is emptied first when the
    /// entry would pass either bound (an entry wider than the byte bound
    /// alone is kept on its own until the next insert).
    pub fn insert(&mut self, key: (usize, u32, u32), bits: Arc<[u8]>) {
        if self.map.len() >= self.cap || self.bytes.saturating_add(bits.len()) > self.max_bytes {
            self.clear();
        }
        self.bytes = self.bytes.saturating_add(bits.len());
        if let Some(old) = self.map.insert(key, bits) {
            self.bytes = self.bytes.saturating_sub(old.len());
        }
    }
}

/// The cache key of a request depth (None = unlimited): every depth
/// at or above the cap draws every plane, like the unlimited one.
pub fn depth_key(depth: Option<u32>) -> u32 {
    depth.map_or(u32::MAX, |d| d.min(DEPTH_CAP as u32))
}

/// FLOE_RUST_OCCUPANCY_DEPTH=off: a version-2 file is used like a
/// version-1 one (only at a depth that draws the layer whole) - the
/// kill switch of the per-depth planes (2026-09-16).
pub fn depth_aware_enabled() -> bool {
    std::env::var("FLOE_RUST_OCCUPANCY_DEPTH").as_deref() != Ok("off")
}

/// Builds one plane per selected layer from an opened file: the OR of
/// the file's planes the request depth draws (None = unlimited),
/// through `cache` when one is given.
pub(crate) fn planes_for(
    file: &Arc<OvoFile>,
    level: u32,
    layer_ks: impl IntoIterator<Item = (u32, usize)>,
    depth: Option<u32>,
    mut cache: Option<&mut PlaneCache>,
) -> Vec<SummaryPlane> {
    let mut planes = Vec::new();
    let key_depth = depth_key(depth);
    for (layer_idx, k) in layer_ks {
        let layer = &file.layers[k];
        // an empty layer (no positive-area shape) is summarized as
        // nothing to draw: a plane without cells, counted as a summary
        // layer, its (nonexistent) pages skipped; so is a layer none of
        // whose planes the request depth draws
        if layer.status != STATUS_OK && layer.status != STATUS_EMPTY {
            continue;
        }
        let mut plane = SummaryPlane {
            layer_idx,
            bits: Arc::from(Vec::new()),
            level,
            cell_dbu: file.cell_dbu << level,
            x0: file.bbox.0,
            y0: file.bbox.1,
            w: 0,
            h: 0,
        };
        if layer.status == STATUS_OK {
            let Some(entry) = layer
                .planes
                .first()
                .and_then(|p| p.levels.get(level as usize))
            else {
                continue;
            };
            let cache_key = (k, level, key_depth);
            let cached = cache.as_deref().and_then(|c| c.get(&cache_key));
            let bits = match cached {
                Some(bits) => bits,
                None => {
                    let combined = file.level_at_depth(k, level as usize, depth);
                    let bits: Arc<[u8]> = match combined {
                        Some((_, _, cow)) => Arc::from(cow.into_owned()),
                        None => Arc::from(Vec::new()),
                    };
                    if let Some(c) = cache.as_deref_mut() {
                        c.insert(cache_key, Arc::clone(&bits));
                    }
                    bits
                }
            };
            if !bits.is_empty() {
                plane.w = entry.w;
                plane.h = entry.h;
                plane.bits = bits;
            }
        }
        planes.push(plane);
    }
    planes
}

/// The default bound of a summary cell on screen (pixels): the
/// coarsest level whose cell is at most this is painted.
pub const DEFAULT_MAX_CELL_PX: f64 = 1.0;

/// The bound in force: FLOE_RUST_OCCUPANCY_PX (diagnostic, e.g. 0.5
/// for the M5 A/B - finer cells close fewer gaps at 4x the paint
/// work) or the default. A value above one pixel is ignored: the
/// cell-centre projection lights one pixel per cell, so a wider cell
/// would leave a lattice of false holes inside filled shapes (review
/// 2026-09-11 (2nd, follow-up) P2: 2 px filled 1,600 of 4,096 pixels).
/// The 2026-09-18 rule: cull requests take the summary too, unless the
/// kill switch FLOE_RUST_OCCUPANCY_CULL=off restores the keep-only rule.
/// Callers pass `thin_keep || cull_allowed()` as the policy condition.
pub fn cull_allowed() -> bool {
    std::env::var("FLOE_RUST_OCCUPANCY_CULL").as_deref() != Ok("off")
}

/// A plain layout's frames draw no occupancy summary by default (user
/// decision 2026-09-24): the summary stands in for pages at wide views,
/// which a jobdeck's mask sources need and a layout's own view does not
/// (a layout indexed with --occupancy kept replacing its wide keep views,
/// and hid the page path from measurements). FLOE_RUST_OCCUPANCY=on uses
/// it for layouts again (diagnostic); =off turns it off for decks too.
/// Jobdeck passes (deck.rs) are not affected. A request's `occupancy=on|off`
/// decides for its frame before either (renderd summary_switches, the
/// viewer's toggle, 2026-10-20).
pub fn layout_allowed() -> bool {
    std::env::var("FLOE_RUST_OCCUPANCY").as_deref() == Ok("on")
}

pub fn max_cell_px() -> f64 {
    parse_max_cell_px(std::env::var("FLOE_RUST_OCCUPANCY_PX").ok().as_deref())
}

pub fn parse_max_cell_px(value: Option<&str>) -> f64 {
    value
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| *v > 0.0 && *v <= 1.0)
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

    #[test]
    fn an_empty_layer_is_a_plane_without_cells() {
        use floe_vfs::occupancy::{write_ovo, Layer, Level, Occupancy, Plane, STATUS_NONE_WORK};
        let ok = Layer {
            layer: 1,
            dt: 0,
            status: STATUS_OK,
            work: 3,
            planes: vec![Plane {
                depth: 0,
                levels: vec![
                    Level {
                        w: 8,
                        h: 2,
                        bits: vec![0b0000_0101, 0],
                    },
                    Level {
                        w: 4,
                        h: 1,
                        bits: vec![0b0011],
                    },
                ],
            }],
        };
        let empty = Layer {
            layer: 2,
            dt: 0,
            status: STATUS_EMPTY,
            work: 0,
            planes: Vec::new(),
        };
        let none = Layer {
            layer: 3,
            dt: 0,
            status: STATUS_NONE_WORK,
            work: 9,
            planes: Vec::new(),
        };
        let occ = Occupancy {
            unit: 1000.0,
            src_size: 1,
            src_mtime: 2,
            top: "T".into(),
            cell_dbu: 10,
            bbox: (0, 0, 80, 20),
            w: 8,
            h: 2,
            n_levels: 2,
            layers: vec![ok, empty, none],
            paths_skipped: 0,
        };
        let file = Arc::new(OvoFile::from_bytes(write_ovo(&occ)).unwrap());
        let planes = planes_for(&file, 0, [(0u32, 0usize), (1, 1), (2, 2)], None, None);
        // the ok and the empty layer are planes, the none:work layer is not
        assert_eq!(
            planes.iter().map(|p| p.layer_idx).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert!(planes[0].get(0, 0) && planes[0].get(2, 0) && !planes[0].get(1, 0));
        assert_eq!((planes[1].w, planes[1].h), (0, 0));
        assert!(!planes[1].get(0, 0) && planes[1].bits().is_empty() && planes[1].row_bytes() == 0);
    }

    #[test]
    fn a_request_depth_draws_the_planes_at_or_above_it() {
        use floe_vfs::occupancy::{write_ovo, Layer, Level, Occupancy, Plane};
        // depth 0 lights cell 0, depth 2 lights cell 2; a second layer
        // has nothing shallower than depth 1
        let two = Layer {
            layer: 1,
            dt: 0,
            status: STATUS_OK,
            work: 1,
            planes: vec![
                Plane {
                    depth: 0,
                    levels: vec![
                        Level {
                            w: 8,
                            h: 1,
                            bits: vec![0b0000_0001],
                        },
                        Level {
                            w: 4,
                            h: 1,
                            bits: vec![0b0001],
                        },
                    ],
                },
                Plane {
                    depth: 2,
                    levels: vec![
                        Level {
                            w: 8,
                            h: 1,
                            bits: vec![0b0000_0100],
                        },
                        Level {
                            w: 4,
                            h: 1,
                            bits: vec![0b0010],
                        },
                    ],
                },
            ],
        };
        let deep = Layer {
            layer: 2,
            dt: 0,
            status: STATUS_OK,
            work: 1,
            planes: vec![Plane {
                depth: 1,
                levels: vec![
                    Level {
                        w: 8,
                        h: 1,
                        bits: vec![0b1000_0000],
                    },
                    Level {
                        w: 4,
                        h: 1,
                        bits: vec![0b1000],
                    },
                ],
            }],
        };
        let occ = Occupancy {
            unit: 1000.0,
            src_size: 1,
            src_mtime: 2,
            top: "T".into(),
            cell_dbu: 10,
            bbox: (0, 0, 80, 10),
            w: 8,
            h: 1,
            n_levels: 2,
            layers: vec![two, deep],
            paths_skipped: 0,
        };
        let file = Arc::new(OvoFile::from_bytes(write_ovo(&occ)).unwrap());
        let mut cache = PlaneCache::new();
        let at = |depth: Option<u32>, cache: &mut PlaneCache| {
            planes_for(&file, 0, [(0u32, 0usize), (1, 1)], depth, Some(cache))
        };
        let d0 = at(Some(0), &mut cache);
        assert!(d0[0].get(0, 0) && !d0[0].get(2, 0));
        // nothing of the deep layer at depth 0: a plane without cells
        assert_eq!((d0[1].w, d0[1].h), (0, 0));
        let d1 = at(Some(1), &mut cache);
        assert!(d1[0].get(0, 0) && !d1[0].get(2, 0) && d1[1].get(7, 0));
        let d2 = at(Some(2), &mut cache);
        assert!(d2[0].get(0, 0) && d2[0].get(2, 0));
        let all = at(None, &mut cache);
        assert_eq!(all[0].bits(), d2[0].bits());
        // level 1 follows the same rule
        let l1 = planes_for(&file, 1, [(0u32, 0usize)], Some(0), None);
        assert!(l1[0].get(0, 0) && !l1[0].get(1, 0));
        // the cache holds one entry per (layer, level, depth key): two
        // layers at depths 0, 1, 2 and unlimited; depths at or beyond
        // the cap share a key, the unlimited depth has its own
        assert_eq!(cache.len(), 2 * 4, "{:?}", cache.keys().collect::<Vec<_>>());
        assert_eq!(depth_key(Some(40)), depth_key(Some(15)));
        assert_ne!(depth_key(Some(15)), depth_key(None));
    }

    #[test]
    fn the_plane_cache_is_emptied_at_either_bound() {
        let plane = |n: usize| -> Arc<[u8]> { Arc::from(vec![0u8; n]) };
        // the byte bound: three planes of 30 bytes fit under 100, the
        // fourth empties the cache and is kept alone
        let mut cache = PlaneCache::with_bounds(8, 100);
        for k in 0..3 {
            cache.insert((k, 0, 0), plane(30));
        }
        assert_eq!((cache.len(), cache.bytes()), (3, 90));
        assert!(cache.get(&(0, 0, 0)).is_some());
        cache.insert((3, 0, 0), plane(30));
        assert_eq!((cache.len(), cache.bytes()), (1, 30));
        assert!(cache.get(&(0, 0, 0)).is_none() && cache.get(&(3, 0, 0)).is_some());
        // a plane wider than the bound is kept on its own
        cache.insert((4, 0, 0), plane(150));
        assert_eq!((cache.len(), cache.bytes()), (1, 150));
        cache.insert((5, 0, 0), plane(1));
        assert_eq!((cache.len(), cache.bytes()), (1, 1));
        // the entry bound as before (512 by default)
        let mut cache = PlaneCache::with_bounds(4, usize::MAX);
        for k in 0..4 {
            cache.insert((k, 0, 0), plane(2));
        }
        assert_eq!((cache.len(), cache.bytes()), (4, 8));
        cache.insert((4, 0, 0), plane(2));
        assert_eq!((cache.len(), cache.bytes()), (1, 2));
        // the same key again replaces its bytes
        cache.insert((4, 0, 0), plane(5));
        assert_eq!((cache.len(), cache.bytes()), (1, 5));
        let default = PlaneCache::new();
        assert_eq!((default.cap, default.max_bytes), (PLANE_CACHE_CAP, PLANE_CACHE_BYTES));
        assert!(default.is_empty());
    }

    #[test]
    fn the_pixel_bound_knob_never_exceeds_one_pixel() {
        assert_eq!(parse_max_cell_px(None), 1.0);
        assert_eq!(parse_max_cell_px(Some("0.5")), 0.5);
        assert_eq!(parse_max_cell_px(Some(" 1 ")), 1.0);
        // wider cells would punch holes through filled shapes
        assert_eq!(parse_max_cell_px(Some("2")), 1.0);
        assert_eq!(parse_max_cell_px(Some("4")), 1.0);
        assert_eq!(parse_max_cell_px(Some("0")), 1.0);
        assert_eq!(parse_max_cell_px(Some("nan")), 1.0);
        assert_eq!(parse_max_cell_px(Some("x")), 1.0);
    }
}
