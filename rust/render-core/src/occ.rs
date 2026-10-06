//! The density stack's pass 2 from the occupancy density (design.ovs,
//! floe_vfs::occ_density; 2026-10-06, opt-in): no plan, no walk, no decode -
//! for each pixel pass 1 left open, the visible layers from the top, each
//! present where design.ovs's bit of the pixel's cell is set (the planes a
//! request depth draws, OR'd) at its group's mean cover, lit by the density
//! pattern's rank like a summary item (raster.rs paint_occ_plane /
//! paint_occ_lower). FLOE_RUST_DENSITY_OCC=on (renderd) asks for it.

use std::sync::Arc;

use floe_vfs::occ_density::{OvsFile, OVS_GROUP};
use floe_vfs::occupancy::plane_drawn_at;

/// One visible layer at one level for one request depth: the OR of the
/// design.ovs planes that depth draws, and per group the mean cover of the
/// cells they mark (each plane's mean weighted by the cells it marks).
pub struct OccLayer {
    bits: Arc<[u8]>,
    row_bytes: usize,
    means: Arc<[u8]>,
    gw: u32,
    w: u32,
    h: u32,
}

impl OccLayer {
    /// The cover (0..=1) of cell (i, j): 0 where the layer marks nothing.
    #[inline]
    pub fn cover(&self, i: u32, j: u32) -> f32 {
        if i >= self.w || j >= self.h {
            return 0.0;
        }
        let byte = self.bits[j as usize * self.row_bytes + (i / 8) as usize];
        if (byte >> (i % 8)) & 1 == 0 {
            return 0.0;
        }
        f32::from(self.means[(j / OVS_GROUP) as usize * self.gw as usize + (i / OVS_GROUP) as usize]) / 255.0
    }

    /// Whether the layer marks a cell of [i0, i1] x [j0, j1] (a tile's
    /// layers, before its pixels).
    pub fn any_in(&self, i0: i64, i1: i64, j0: i64, j1: i64) -> bool {
        if self.w == 0 || self.h == 0 {
            return false;
        }
        let (i0, i1) = (i0.max(0), i1.min(self.w as i64 - 1));
        let (j0, j1) = (j0.max(0), j1.min(self.h as i64 - 1));
        if i0 > i1 || j0 > j1 {
            return false;
        }
        let (b0, b1) = ((i0 / 8) as usize, (i1 / 8) as usize);
        (j0..=j1).any(|j| {
            let row = &self.bits[j as usize * self.row_bytes..];
            row[b0..=b1].iter().any(|&b| b != 0)
        })
    }
}

/// The occupancy density of one request: the level, its grid, and the
/// visible layers by cache layer index.
pub struct OccDensity {
    pub level: u32,
    /// the level's cell, dbu, and the grid's corner (design.ovs's)
    pub cell: i64,
    pub x0: i64,
    pub y0: i64,
    pub w: u32,
    pub h: u32,
    pub layers: Vec<Option<Arc<OccLayer>>>,
    /// the level's cell, um (the frame reports it)
    pub cell_um: f64,
}

impl OccDensity {
    pub fn layer(&self, layer_idx: u32) -> Option<&OccLayer> {
        self.layers.get(layer_idx as usize)?.as_deref()
    }

    /// cell (i or j) of a world coordinate along x (`x`) or y
    #[inline]
    pub fn cell_of(&self, v: f64, x: bool) -> i64 {
        let origin = if x { self.x0 } else { self.y0 } as f64;
        ((v - origin) / self.cell as f64).floor() as i64
    }

    pub fn layers_present(&self) -> usize {
        self.layers.iter().filter(|l| l.is_some()).count()
    }
}

impl OccLayer {
    /// What the layer holds, bytes (Cache::occ_density's cap)
    pub fn bytes(&self) -> usize {
        self.bits.len() + self.means.len()
    }
}

/// The level a view of `px_dbu` per pixel draws the occupancy density at,
/// for design.ovs's `n_levels` levels of `cell_dbu` doubling: the coarsest
/// whose cell is at most a pixel (as the summary takes it; past it, a finer
/// one costs more and shows no more), else - and when that one's layers
/// would hold more than `cap` bytes (`bytes_at`) - the next coarser ones
/// while their cell is at most `max_cell_px` pixels; None when none is (the
/// plans draw the view).
pub fn choose_level(cell_dbu: i64, n_levels: u32, px_dbu: f64, max_cell_px: f64, bytes_at: impl Fn(u32) -> u64, cap: u64) -> Option<u32> {
    if cell_dbu <= 0 || !(px_dbu > 0.0) {
        return None;
    }
    let cell = |lv: u32| cell_dbu as f64 * f64::from(1u32 << lv.min(31));
    let start = (0..n_levels).filter(|&lv| cell(lv) <= px_dbu).max().unwrap_or(0);
    (start..n_levels).take_while(|&lv| cell(lv) <= max_cell_px * px_dbu).find(|&lv| bytes_at(lv) <= cap)
}

/// One layer's OccLayer at `lv` for `depth` (None: every plane): the OR of
/// the planes the depth draws, each group at its planes' means weighted by
/// the cells each marks; None where it has no density there.
pub(crate) fn combine(ovs: &OvsFile, k: usize, lv: usize, depth: Option<u32>) -> Option<Arc<OccLayer>> {
    let planes = ovs.layers.get(k)?;
    if lv as u32 >= ovs.grid.n_levels {
        return None;
    }
    let (w, h) = ovs.grid.level_dims(lv as u32);
    let row_bytes = (w as usize).div_ceil(8);
    let (gw, gh) = (w.div_ceil(OVS_GROUP), h.div_ceil(OVS_GROUP));
    let occ_in = |plane_bits: &[u8], gi: u32, gj: u32| -> u32 {
        (gj * OVS_GROUP..((gj + 1) * OVS_GROUP).min(h)).map(|row| plane_bits[row as usize * row_bytes + gi as usize].count_ones()).sum()
    };
    let mut bits = vec![0u8; row_bytes * h as usize];
    // sum over the planes drawn of mean x the cells the plane marks
    let mut sum = vec![0f64; (gw * gh) as usize];
    for (p, plane) in planes.iter().enumerate() {
        if !plane_drawn_at(plane.depth, depth) {
            continue;
        }
        let (Some(pbits), Some(means)) = (ovs.bits(k, p, lv), ovs.means(k, p, lv)) else { continue };
        bits.iter_mut().zip(&pbits).for_each(|(a, b)| *a |= b);
        for gj in 0..gh {
            for gi in 0..gw {
                let m = means[(gj * gw + gi) as usize];
                if m != 0 {
                    sum[(gj * gw + gi) as usize] += f64::from(m) * f64::from(occ_in(&pbits, gi, gj));
                }
            }
        }
    }
    let mut means = vec![0u8; (gw * gh) as usize];
    let mut any = false;
    for gj in 0..gh {
        for gi in 0..gw {
            let s = sum[(gj * gw + gi) as usize];
            if !(s > 0.0) {
                continue;
            }
            let occ = occ_in(&bits, gi, gj);
            if occ == 0 {
                continue;
            }
            means[(gj * gw + gi) as usize] = (s / f64::from(occ)).round().clamp(1.0, 255.0) as u8;
            any = true;
        }
    }
    any.then(|| Arc::new(OccLayer { bits: Arc::from(bits), row_bytes, means: Arc::from(means), gw, w, h }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_vfs::occ_density::{encode, OvsGrid};
    use std::io::Write;

    fn deflate(bytes: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(6));
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    /// 16 x 16 cells' row-padded bytes with the cells in [i0, i1) x [j0, j1)
    /// set
    fn bits(boxes: &[(u32, u32, u32, u32)]) -> Vec<u8> {
        let mut bits = vec![0u8; 2 * 16];
        for &(i0, i1, j0, j1) in boxes {
            for j in j0..j1 {
                for i in i0..i1 {
                    bits[(j * 2 + i / 8) as usize] |= 1 << (i % 8);
                }
            }
        }
        bits
    }

    /// design.ovs: one layer, 16 x 16 cells of 100 dbu at one level; its
    /// depth-0 plane over the low left group at mean 100, its depth-1 plane
    /// over half of that group at 200 and all of the high right one at 50
    fn files() -> OvsFile {
        let g = OvsGrid::new(1000.0, 0.1, (0, 0, 1600, 1600)).unwrap();
        assert_eq!((g.cell_dbu, g.w, g.h, g.n_levels), (100, 16, 16, 1));
        let blobs = vec![vec![
            (0, vec![(deflate(&bits(&[(0, 8, 0, 8)])), deflate(&[100, 0, 0, 0]))]),
            (1, vec![(deflate(&bits(&[(0, 4, 0, 8), (8, 16, 8, 16)])), deflate(&[200, 0, 0, 50]))]),
        ]];
        OvsFile::from_bytes(encode(11, 22, &g, &blobs)).unwrap()
    }

    #[test]
    fn a_depth_draws_its_planes_bits_at_their_weighted_mean() {
        let ovs = files();
        // every plane: the low left group's 64 cells all set, at
        // (100 x 64 + 200 x 32) / 64 = 200; the high right one at 50
        let all = combine(&ovs, 0, 0, None).unwrap();
        assert_eq!(all.cover(2, 2), 200.0 / 255.0);
        assert_eq!(all.cover(6, 7), 200.0 / 255.0);
        assert_eq!(all.cover(12, 12), 50.0 / 255.0);
        assert_eq!(all.cover(12, 2), 0.0);
        assert_eq!(all.cover(16, 2), 0.0);
        assert!(all.any_in(8, 15, 8, 15) && !all.any_in(8, 15, 0, 7) && !all.any_in(20, 30, 0, 15));
        // depth 1 draws both planes, as every plane does
        let one = combine(&ovs, 0, 0, Some(1)).unwrap();
        assert_eq!((one.cover(2, 2), one.cover(12, 12)), (all.cover(2, 2), all.cover(12, 12)));
        // depth 0: its own plane alone, the high right group not there
        let top = combine(&ovs, 0, 0, Some(0)).unwrap();
        assert_eq!(top.cover(2, 2), 100.0 / 255.0);
        assert_eq!(top.cover(12, 12), 0.0);
        assert!(!top.any_in(8, 15, 8, 15));
        assert_eq!(top.bytes(), 2 * 16 + 4);
        // no such layer or level
        assert!(combine(&ovs, 1, 0, None).is_none());
        assert!(combine(&ovs, 0, 1, None).is_none());
    }

    #[test]
    fn a_layer_with_no_density_where_its_bits_are_is_none() {
        // bits with no means, and no bits at all
        let g = OvsGrid::new(1000.0, 0.1, (0, 0, 1600, 1600)).unwrap();
        let blobs = vec![vec![(0, vec![(deflate(&bits(&[(0, 8, 0, 8)])), Vec::new())]), (1, vec![(Vec::new(), Vec::new())])]];
        let empty = OvsFile::from_bytes(encode(11, 22, &g, &blobs)).unwrap();
        assert!(combine(&empty, 0, 0, None).is_none());
    }

    #[test]
    fn the_level_is_the_coarsest_within_a_pixel_and_the_cap() {
        let free = |_| 0u64;
        // cells of 100, 200, 400, ... dbu
        assert_eq!(choose_level(100, 6, 250.0, 2.0, free, 1), Some(1));
        assert_eq!(choose_level(100, 6, 150.0, 2.0, free, 1), Some(0));
        assert_eq!(choose_level(100, 6, 10_000.0, 2.0, free, 1), Some(5));
        // closer in than a cell a pixel: level 0 while it is at most two
        assert_eq!(choose_level(100, 6, 80.0, 2.0, free, 1), Some(0));
        assert_eq!(choose_level(100, 6, 40.0, 2.0, free, 1), None);
        assert_eq!(choose_level(100, 6, 40.0, 4.0, free, 1), Some(0));
        // over the cap: the next coarser level while its cell is within the
        // pixels allowed, else none
        let costly = |lv: u32| if lv < 2 { 10 } else { 1 };
        assert_eq!(choose_level(100, 6, 250.0, 2.0, costly, 5), Some(2));
        assert_eq!(choose_level(100, 6, 250.0, 1.5, costly, 5), None);
        assert_eq!(choose_level(100, 6, 250.0, 2.0, |_| 10, 5), None);
        assert_eq!(choose_level(100, 0, 250.0, 2.0, free, 1), None);
        assert_eq!(choose_level(0, 6, 250.0, 2.0, free, 1), None);
    }
}
