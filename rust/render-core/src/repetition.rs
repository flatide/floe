use floe_oasis::doc::Rep;
use floe_ovm::BBox;
use floe_vfs::hier::{grid_ranges, GridVis};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RepVisit {
    pub tested: u64,
    pub visible: u64,
}

pub(crate) fn for_each_visible_offset(
    rep: &Rep,
    base_bbox: BBox,
    local_view: BBox,
    mut visit: impl FnMut(i64, i64) -> Result<(), String>,
) -> Result<RepVisit, String> {
    if base_bbox.is_empty() || local_view.is_empty() {
        return Ok(RepVisit::default());
    }
    let offsets = offset_region(local_view, base_bbox);
    match rep {
        Rep::One => {
            let visible = offsets.contains_pt(0, 0);
            if visible {
                visit(0, 0)?;
            }
            Ok(RepVisit {
                tested: 1,
                visible: visible as u64,
            })
        }
        Rep::Grid { na, nb, va, vb } => {
            let na: i64 = (*na)
                .try_into()
                .map_err(|_| format!("limit exceeded: grid na = {}", na))?;
            let nb: i64 = (*nb)
                .try_into()
                .map_err(|_| format!("limit exceeded: grid nb = {}", nb))?;
            let GridVis::Range { i0, i1, j0, j1 } = grid_ranges(na, nb, *va, *vb, &offsets) else {
                return Ok(RepVisit::default());
            };
            let ni = (i1 as i128 - i0 as i128 + 1) as u128;
            let nj = (j1 as i128 - j0 as i128 + 1) as u128;
            let count: u64 = ni
                .checked_mul(nj)
                .and_then(|value| value.try_into().ok())
                .ok_or_else(|| "limit exceeded: visible grid members".to_string())?;
            for i in i0..=i1 {
                for j in j0..=j1 {
                    let ox = i as i128 * va.0 as i128 + j as i128 * vb.0 as i128;
                    let oy = i as i128 * va.1 as i128 + j as i128 * vb.1 as i128;
                    visit(
                        checked_i64(ox, "grid offset x")?,
                        checked_i64(oy, "grid offset y")?,
                    )?;
                }
            }
            Ok(RepVisit {
                tested: count,
                visible: count,
            })
        }
        Rep::Pts(points) => {
            let mut visible = 0u64;
            for &(x, y) in points.iter() {
                if offsets.contains_pt(x, y) {
                    visit(x, y)?;
                    visible = visible.saturating_add(1);
                }
            }
            Ok(RepVisit {
                tested: points.len().try_into().unwrap_or(u64::MAX),
                visible,
            })
        }
    }
}

fn offset_region(view: BBox, object: BBox) -> BBox {
    BBox {
        x0: saturating_i64(view.x0 as i128 - object.x1 as i128),
        y0: saturating_i64(view.y0 as i128 - object.y1 as i128),
        x1: saturating_i64(view.x1 as i128 - object.x0 as i128),
        y1: saturating_i64(view.y1 as i128 - object.y0 as i128),
    }
}

fn saturating_i64(value: i128) -> i64 {
    value.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

fn checked_i64(value: i128, field: &str) -> Result<i64, String> {
    value
        .try_into()
        .map_err(|_| format!("coordinate overflow: {} = {}", field, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn bbox(x0: i64, y0: i64, x1: i64, y1: i64) -> BBox {
        BBox { x0, y0, x1, y1 }
    }

    #[test]
    fn grid_visits_only_view_intersections() {
        let rep = Rep::Grid {
            na: 100,
            nb: 1,
            va: (10, 0),
            vb: (0, 0),
        };
        let mut offsets = Vec::new();
        let stats = for_each_visible_offset(&rep, bbox(0, 0, 4, 4), bbox(25, 0, 44, 4), |x, y| {
            offsets.push((x, y));
            Ok(())
        })
        .unwrap();
        assert_eq!(offsets, vec![(30, 0), (40, 0)]);
        assert_eq!(stats.visible, 2);
    }

    #[test]
    fn pts_preserves_duplicates_and_source_order() {
        let rep = Rep::Pts(Arc::from([(0, 0), (20, 0), (0, 0)]));
        let mut offsets = Vec::new();
        let stats = for_each_visible_offset(&rep, bbox(0, 0, 5, 5), bbox(0, 0, 5, 5), |x, y| {
            offsets.push((x, y));
            Ok(())
        })
        .unwrap();
        assert_eq!(offsets, vec![(0, 0), (0, 0)]);
        assert_eq!(stats.tested, 3);
        assert_eq!(stats.visible, 2);
    }
}
