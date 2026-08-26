use floe_oasis::doc::Rep;
use floe_ovm::BBox;
use floe_vfs::hier::{grid_ranges, GridVis};
use std::cell::Cell;

use crate::RenderCancellation;

// Degenerate 2-D arrays cannot use the determinant-based viewport inverse.
// Small forms occur in valid field data (most commonly one zero vector), but
// accepting an unchecked 2^31 x 2^31 form would turn one tile into a permanent
// enumeration.  This is a per-repetition, per-view bound; normal non-degenerate
// grids remain analytically pruned and are not subject to it.
const MAX_DEGENERATE_GRID_VISITS: u64 = 1_048_576;

/// Consecutive `Rep::Pts` members covered by one decode-time chunk bbox
/// (F2R-03b 2a). The chunk tables in `PageIndex` and the chunked walk below
/// must agree on this size; a table with a different chunk count is ignored
/// and the full scan runs instead.
pub(crate) const PTS_CHUNK_POINTS: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RepVisit {
    pub tested: u64,
    pub visible: u64,
}

pub(crate) fn for_each_visible_offset(
    rep: &Rep,
    base_bbox: BBox,
    local_view: BBox,
    visit: impl FnMut(i64, i64) -> Result<(), String>,
) -> Result<RepVisit, String> {
    for_each_visible_offset_impl(rep, None, base_bbox, local_view, None, None, visit)
}

/// Raster-only variant: `pts_chunks` carries the decode-time per-chunk
/// offset bboxes for this repetition's `Pts` form (see `PageIndex`), letting
/// the walk skip whole chunks whose bbox misses the offset region. Surviving
/// points keep their source order and duplicates.
pub(crate) fn for_each_visible_offset_chunked(
    rep: &Rep,
    pts_chunks: Option<&[BBox]>,
    base_bbox: BBox,
    local_view: BBox,
    visit: impl FnMut(i64, i64) -> Result<(), String>,
) -> Result<RepVisit, String> {
    for_each_visible_offset_impl(rep, pts_chunks, base_bbox, local_view, None, None, visit)
}

pub(crate) fn for_each_visible_offset_bounded(
    rep: &Rep,
    base_bbox: BBox,
    local_view: BBox,
    remaining: &Cell<usize>,
    limit_error: &str,
    visit: impl FnMut(i64, i64) -> Result<(), String>,
) -> Result<RepVisit, String> {
    for_each_visible_offset_impl(
        rep,
        None,
        base_bbox,
        local_view,
        Some((remaining, limit_error)),
        None,
        visit,
    )
}

pub(crate) fn for_each_visible_offset_cancellable(
    rep: &Rep,
    base_bbox: BBox,
    local_view: BBox,
    generation: u64,
    cancellation: &RenderCancellation,
    visit: impl FnMut(i64, i64) -> Result<(), String>,
) -> Result<RepVisit, String> {
    for_each_visible_offset_impl(
        rep,
        None,
        base_bbox,
        local_view,
        None,
        Some((generation, cancellation)),
        visit,
    )
}

#[allow(clippy::too_many_arguments)]
fn for_each_visible_offset_impl(
    rep: &Rep,
    pts_chunks: Option<&[BBox]>,
    base_bbox: BBox,
    local_view: BBox,
    mut budget: Option<(&Cell<usize>, &str)>,
    cancellation: Option<(u64, &RenderCancellation)>,
    mut visit: impl FnMut(i64, i64) -> Result<(), String>,
) -> Result<RepVisit, String> {
    check_repetition_cancelled(cancellation)?;
    if base_bbox.is_empty() || local_view.is_empty() {
        return Ok(RepVisit::default());
    }
    let mut cancel_member = 0u16;
    let offsets = offset_region(local_view, base_bbox);
    match rep {
        Rep::One => {
            charge_member(&mut budget, cancellation, &mut cancel_member)?;
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
            let determinant = va.0 as i128 * vb.1 as i128 - va.1 as i128 * vb.0 as i128;
            if na > 1 && nb > 1 && determinant == 0 && count > MAX_DEGENERATE_GRID_VISITS {
                return Err(format!(
                    "limit exceeded: degenerate 2-D grid visible members {} > {} \
                     ({}x{}, vectors ({},{}) and ({},{}))",
                    count, MAX_DEGENERATE_GRID_VISITS, na, nb, va.0, va.1, vb.0, vb.1
                ));
            }
            for i in i0..=i1 {
                for j in j0..=j1 {
                    charge_member(&mut budget, cancellation, &mut cancel_member)?;
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
            if let Some(chunks) =
                pts_chunks.filter(|chunks| chunks.len() == points.len().div_ceil(PTS_CHUNK_POINTS))
            {
                let mut tested = 0u64;
                for (chunk, chunk_bbox) in points.chunks(PTS_CHUNK_POINTS).zip(chunks) {
                    if !offsets.intersects(chunk_bbox) {
                        continue;
                    }
                    for &(x, y) in chunk {
                        charge_member(&mut budget, cancellation, &mut cancel_member)?;
                        tested = tested.saturating_add(1);
                        if offsets.contains_pt(x, y) {
                            visit(x, y)?;
                            visible = visible.saturating_add(1);
                        }
                    }
                }
                return Ok(RepVisit { tested, visible });
            }
            for &(x, y) in points.iter() {
                charge_member(&mut budget, cancellation, &mut cancel_member)?;
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

fn charge_member(
    budget: &mut Option<(&Cell<usize>, &str)>,
    cancellation: Option<(u64, &RenderCancellation)>,
    cancel_member: &mut u16,
) -> Result<(), String> {
    if let Some((remaining, limit_error)) = budget.as_mut() {
        let value = remaining.get();
        if value == 0 {
            return Err((*limit_error).to_string());
        }
        remaining.set(value - 1);
    }
    if *cancel_member == 0 {
        check_repetition_cancelled(cancellation)?;
    }
    *cancel_member = cancel_member.wrapping_add(1) & 1023;
    Ok(())
}

fn check_repetition_cancelled(
    cancellation: Option<(u64, &RenderCancellation)>,
) -> Result<(), String> {
    if let Some((generation, cancellation)) = cancellation {
        cancellation.check(generation)?;
    }
    Ok(())
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

    #[test]
    fn small_zero_vector_grid_preserves_duplicates_and_prunes_axis() {
        let rep = Rep::Grid {
            na: 12,
            nb: 2,
            va: (2_400, 0),
            vb: (0, 0),
        };
        let mut offsets = Vec::new();
        let stats = for_each_visible_offset(
            &rep,
            bbox(0, 0, 100, 100),
            bbox(4_800, 0, 7_300, 100),
            |x, y| {
                offsets.push((x, y));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            offsets,
            vec![(4_800, 0), (4_800, 0), (7_200, 0), (7_200, 0)]
        );
        assert_eq!(stats.tested, 4);
        assert_eq!(stats.visible, 4);
    }

    #[test]
    fn enormous_degenerate_2d_grid_returns_an_error_before_enumeration() {
        let rep = Rep::Grid {
            na: 1 << 31,
            nb: 1 << 31,
            va: (1, 1),
            vb: (2, 2),
        };
        let mut visits = 0;
        let error = for_each_visible_offset(&rep, bbox(0, 0, 1, 1), bbox(0, 0, 1, 1), |_, _| {
            visits += 1;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(visits, 0);
        assert!(
            error.contains("limit exceeded: degenerate 2-D grid"),
            "{error}"
        );
    }

    #[test]
    fn bounded_pts_counts_non_visible_members() {
        let rep = Rep::Pts(Arc::from([(100, 100), (200, 200), (0, 0)]));
        let remaining = Cell::new(2);
        let mut visits = 0;
        let error = for_each_visible_offset_bounded(
            &rep,
            bbox(0, 0, 1, 1),
            bbox(0, 0, 1, 1),
            &remaining,
            "member cap",
            |_, _| {
                visits += 1;
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error, "member cap");
        assert_eq!(remaining.get(), 0);
        assert_eq!(visits, 0);
    }

    fn chunk_bboxes(points: &[(i64, i64)]) -> Vec<BBox> {
        points
            .chunks(PTS_CHUNK_POINTS)
            .map(|chunk| {
                let mut bbox = BBox::EMPTY;
                for &(x, y) in chunk {
                    bbox.grow(&BBox {
                        x0: x,
                        y0: y,
                        x1: x,
                        y1: y,
                    });
                }
                bbox
            })
            .collect()
    }

    #[test]
    fn chunked_pts_matches_full_scan_and_skips_far_chunks() {
        // Chunk 0 sits near the origin, chunks 1-2 are far away, chunk 3
        // mixes near duplicates with far points.
        let mut points = Vec::new();
        for index in 0..64i64 {
            points.push((index % 8, index / 8));
        }
        for index in 0..128i64 {
            points.push((10_000 + index, 10_000));
        }
        for index in 0..64i64 {
            points.push(if index % 2 == 0 { (5, 5) } else { (20_000, 0) });
        }
        let rep = Rep::Pts(Arc::from(points.clone()));
        let chunks = chunk_bboxes(&points);
        let base = bbox(0, 0, 2, 2);
        let view = bbox(0, 0, 40, 40);

        let mut full_offsets = Vec::new();
        let full = for_each_visible_offset(&rep, base, view, |x, y| {
            full_offsets.push((x, y));
            Ok(())
        })
        .unwrap();
        let mut chunked_offsets = Vec::new();
        let chunked = for_each_visible_offset_chunked(&rep, Some(&chunks), base, view, |x, y| {
            chunked_offsets.push((x, y));
            Ok(())
        })
        .unwrap();

        // Identical visible members, order, and duplicates; far chunks are
        // skipped without testing their points.
        assert_eq!(chunked_offsets, full_offsets);
        assert_eq!(chunked.visible, full.visible);
        assert_eq!(full.tested, points.len() as u64);
        assert_eq!(chunked.tested, 128, "only chunks 0 and 3 are scanned");

        // A stale or mismatched table must fall back to the full scan.
        let mut fallback_offsets = Vec::new();
        let fallback =
            for_each_visible_offset_chunked(&rep, Some(&chunks[..1]), base, view, |x, y| {
                fallback_offsets.push((x, y));
                Ok(())
            })
            .unwrap();
        assert_eq!(fallback_offsets, full_offsets);
        assert_eq!(fallback.tested, full.tested);
    }

    #[test]
    fn cancellable_repetition_stops_before_enumeration() {
        let cancellation = RenderCancellation::new();
        cancellation.cancel_before(2);
        let rep = Rep::Pts(Arc::from([(100, 100); 2048]));
        let mut visits = 0;
        let error = for_each_visible_offset_cancellable(
            &rep,
            bbox(0, 0, 1, 1),
            bbox(0, 0, 1, 1),
            1,
            &cancellation,
            |_, _| {
                visits += 1;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.contains("render cancelled"), "{error}");
        assert_eq!(visits, 0);
    }
}
