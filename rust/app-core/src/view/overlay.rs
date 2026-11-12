//! Shapes a host draws over a frame - a DRC error's marker - as targets of a
//! snap. They are no part of the scene the worker answers from, so a snap
//! never saw them; the controller joins them to the worker's answer by the
//! scene's own rule: a vertex before an edge, then the nearer of two alike.
use crate::{Error, Result};
use floe_worker_client::{QueryRequest, SnapHit, SnapKind};

/// The segments one snap may be given: what a host draws around the cursor,
/// not a whole results file.
pub const SNAP_SEGMENTS_MAX: usize = 256;

/// A coordinate this near a whole dbu is that dbu: hosts hold their shapes
/// in micrometres, and `25.481 / 0.001` is not `25481`.
const WHOLE: f64 = 1e-6;

/// One segment of a drawn shape in the view's coordinates (dbu, the
/// root's). Its ends need not be on the dbu grid: a results file may be
/// finer than the layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapSegment([f64; 4]);

impl SnapSegment {
    /// `[x0, y0, x1, y1]`; a segment whose ends are one point is that
    /// point alone (a vertex, no edge).
    pub fn new(ends: [f64; 4]) -> Result<Self> {
        let bound = (1u64 << 62) as f64;
        if !ends.iter().all(|n| n.is_finite() && n.abs() <= bound) {
            return Err(Error::input(
                "snap overlay coordinates must be finite and within the view's range",
            ));
        }
        Ok(Self(ends.map(|n| {
            let whole = n.round();
            if (n - whole).abs() <= WHOLE {
                whole
            } else {
                n
            }
        })))
    }
    pub fn ends(self) -> [f64; 4] {
        self.0
    }
}

/// Where a snap landed on a drawn shape: an end of a segment as it was
/// given, or a point of a segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlaySnap {
    pub x: f64,
    pub y: f64,
    pub kind: SnapKind,
}

pub(super) fn validate(segments: &[SnapSegment]) -> Result<()> {
    if segments.len() > SNAP_SEGMENTS_MAX {
        return Err(Error::input(format!(
            "a snap takes at most {SNAP_SEGMENTS_MAX} overlay segments"
        )));
    }
    Ok(())
}

/// A coordinate from the point asked about. Whole coordinates are
/// subtracted as integers: past 2^53 two of them a dbu apart are one f64.
fn off(v: f64, origin: i64) -> f64 {
    if v.fract() == 0. {
        (v as i128 - i128::from(origin)) as f64
    } else {
        v - origin as f64
    }
}

/// The scene's edge snap says a whole dbu, and so does this one along its
/// segment: a coordinate both ends share is the segment's own, on the grid
/// or off it, and a rounded one stays between the ends - the point said is
/// on an axis-parallel segment exactly.
fn on_grid(q: f64, a: f64, b: f64) -> f64 {
    if a == b {
        a
    } else {
        q.round_ties_even().clamp(a.min(b), a.max(b))
    }
}

/// The drawn shapes' own answer around the point asked: the nearest end
/// within the radius, else the nearest point of a segment within it (the
/// scene's rule, render-core `snap_scene`; the first of two as near).
fn nearest(segments: &[SnapSegment], x: i64, y: i64, radius: i64) -> Option<OverlaySnap> {
    let reach = radius as f64 * radius as f64;
    let mut vertex = None::<(f64, f64, f64)>;
    let mut edge = None::<(f64, f64, f64)>;
    for segment in segments {
        let [x0, y0, x1, y1] = segment.0;
        let (ax, ay, bx, by) = (off(x0, x), off(y0, y), off(x1, x), off(y1, y));
        for (dx, dy, px, py) in [(ax, ay, x0, y0), (bx, by, x1, y1)] {
            let d = dx * dx + dy * dy;
            if d <= reach && vertex.is_none_or(|v| d < v.0) {
                vertex = Some((d, px, py));
            }
        }
        let (vx, vy) = (bx - ax, by - ay);
        let length = vx * vx + vy * vy;
        if length == 0. {
            continue;
        }
        let along = ((-ax * vx - ay * vy) / length).clamp(0., 1.);
        let (qx, qy) = (ax + along * vx, ay + along * vy);
        let d = qx * qx + qy * qy;
        if d <= reach && edge.is_none_or(|e| d < e.0) {
            edge = Some((
                d,
                on_grid(x as f64 + qx, x0, x1),
                on_grid(y as f64 + qy, y0, y1),
            ));
        }
    }
    let said = |kind| move |(_, x, y): (f64, f64, f64)| OverlaySnap { x, y, kind };
    vertex
        .map(said(SnapKind::Vertex))
        .or_else(|| edge.map(said(SnapKind::Edge)))
}

/// The answer of a snap that was given drawn shapes, when it is on one of
/// them: the scene had nothing in reach, or the shape's point is the better
/// by the scene's rule - a vertex before an edge, the nearer of two alike.
/// Two as near are the scene's, so a shape drawn on the layout's own
/// geometry changes no answer. `None`: the scene's answer stands.
///
/// It is judged around the point and within the radius the worker was
/// asked, whatever became of that question: a scene that is summarized, not
/// complete or gone has no say about what is drawn over it.
pub(super) fn over(
    segments: &[SnapSegment],
    request: &QueryRequest,
    scene: Option<&SnapHit>,
) -> Option<OverlaySnap> {
    if segments.is_empty() {
        return None;
    }
    let drawn = nearest(segments, request.x, request.y, request.radius)?;
    let Some(scene) = scene else {
        return Some(drawn);
    };
    let better = match (drawn.kind, scene.kind) {
        (SnapKind::Vertex, SnapKind::Edge) => true,
        (SnapKind::Edge, SnapKind::Vertex) => false,
        _ => {
            let (dx, dy) = (off(drawn.x, request.x), off(drawn.y, request.y));
            let sx = (i128::from(scene.x) - i128::from(request.x)) as f64;
            let sy = (i128::from(scene.y) - i128::from(request.y)) as f64;
            dx * dx + dy * dy < sx * sx + sy * sy
        }
    };
    better.then_some(drawn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_worker_client::{Layers, QueryOperation, SceneId};

    fn asked(x: i64, y: i64, radius: i64) -> QueryRequest {
        QueryRequest {
            scene: SceneId {
                generation: 1,
                round: 1,
            },
            operation: QueryOperation::Snap,
            x,
            y,
            radius,
            layers: Layers::All,
            pick_max: None,
        }
    }
    fn segments(ends: &[[f64; 4]]) -> Vec<SnapSegment> {
        ends.iter().map(|e| SnapSegment::new(*e).unwrap()).collect()
    }
    fn hit(x: i64, y: i64, kind: SnapKind) -> SnapHit {
        SnapHit { x, y, kind }
    }
    fn at(x: f64, y: f64, kind: SnapKind) -> Option<OverlaySnap> {
        Some(OverlaySnap { x, y, kind })
    }

    #[test]
    fn a_drawn_shape_is_snapped_to_as_the_scene_is() {
        // an edge pair's marker: two horizontal edges, corner to corner
        let pair = segments(&[[0., 200., 1000., 200.], [1500., 600., 2500., 600.]]);
        // an end within reach, whichever of the segment's
        for (x, y, end) in [
            (995, 203, [1000., 200.]),
            (4, 196, [0., 200.]),
            (1503, 606, [1500., 600.]),
            (2500, 590, [2500., 600.]),
        ] {
            assert_eq!(
                over(&pair, &asked(x, y, 10), None),
                at(end[0], end[1], SnapKind::Vertex),
                "{x} {y}"
            );
        }
        // the reach is the scene's: at the radius is in, past it is out
        assert_eq!(
            over(&pair, &asked(1010, 200, 10), None),
            at(1000., 200., SnapKind::Vertex)
        );
        assert_eq!(over(&pair, &asked(1011, 200, 10), None), None);
        assert_eq!(over(&pair, &asked(1008, 207, 10), None), None);
        // no end in reach: the point of the segment under the cursor
        assert_eq!(
            over(&pair, &asked(500, 204, 10), None),
            at(500., 200., SnapKind::Edge)
        );
        assert_eq!(
            over(&pair, &asked(2000, 591, 10), None),
            at(2000., 600., SnapKind::Edge)
        );
        assert_eq!(
            over(&pair, &asked(500, 210, 10), None),
            at(500., 200., SnapKind::Edge)
        );
        assert_eq!(over(&pair, &asked(500, 211, 10), None), None);
        // an end in reach is taken before a nearer point of an edge
        assert_eq!(
            over(&pair, &asked(991, 200, 10), None),
            at(1000., 200., SnapKind::Vertex)
        );
        // the nearest of several ends, the first of two as near
        let fan = segments(&[[0., 0., 6., 0.], [0., 0., 0., 8.], [3., 4., 3., 4.]]);
        assert_eq!(
            over(&fan, &asked(3, 3, 10), None),
            at(3., 4., SnapKind::Vertex)
        );
        assert_eq!(
            over(&fan[..2], &asked(3, 0, 5), None),
            at(0., 0., SnapKind::Vertex)
        );
        // a segment that is one point has no edge
        let point = segments(&[[3., 4., 3., 4.]]);
        assert_eq!(
            over(&point, &asked(3, 14, 10), None),
            at(3., 4., SnapKind::Vertex)
        );
        assert_eq!(over(&point, &asked(3, 15, 10), None), None);
        // nothing drawn, nothing said
        assert_eq!(over(&[], &asked(0, 0, 10), None), None);
    }

    #[test]
    fn an_edge_point_is_a_whole_dbu_along_its_segment() {
        // off the grid (a results file finer than the layout): the shared
        // coordinate is the segment's own, the other a whole dbu
        let fine = segments(&[[10.25, 100.5, 90.75, 100.5], [40.5, 0.25, 40.5, 60.75]]);
        assert_eq!(
            over(&fine, &asked(50, 103, 5), None),
            at(50., 100.5, SnapKind::Edge)
        );
        assert_eq!(
            over(&fine, &asked(43, 30, 5), None),
            at(40.5, 30., SnapKind::Edge)
        );
        // an end off the grid is said as it is
        assert_eq!(
            over(&fine, &asked(91, 100, 5), None),
            at(90.75, 100.5, SnapKind::Vertex)
        );
        assert_eq!(
            over(&segments(&[[10.75, 7., 40.25, 7.]]), &asked(13, 2, 5), None),
            at(13., 7., SnapKind::Edge)
        );
        // a rounded coordinate does not leave the segment
        assert_eq!(on_grid(0.4, 0.25, 10.25), 0.25);
        assert_eq!(on_grid(10.6, 10.25, 0.25), 10.25);
        assert_eq!(on_grid(10.4, 0.25, 10.25), 10.);
        assert_eq!(on_grid(7.3, 4.5, 4.5), 4.5);
        // a slanted segment: the whole dbu nearest the foot, a half to the
        // even one, as the scene's
        let slant = segments(&[[0., 0., 100., 100.]]);
        assert_eq!(
            over(&slant, &asked(52, 47, 10), None),
            at(50., 50., SnapKind::Edge)
        );
        assert_eq!(
            over(&slant, &asked(52, 45, 10), None),
            at(48., 48., SnapKind::Edge)
        );
        assert_eq!(
            over(&segments(&[[0., 0., 90., 30.]]), &asked(40, 20, 10), None),
            at(42., 14., SnapKind::Edge)
        );
        // micrometres over the dbu are whole dbu again
        let noisy = SnapSegment::new([25.481 / 0.001, 0.7 / 0.001, 0.3 / 0.001, 188.97 / 0.001]);
        assert_eq!(noisy.unwrap().ends(), [25481., 700., 300., 188970.]);
        assert_eq!(
            SnapSegment::new([0.5, -0.25, 1e-3, 3.0000005])
                .unwrap()
                .ends(),
            [0.5, -0.25, 1e-3, 3.]
        );
        for bad in [f64::NAN, f64::INFINITY, -f64::INFINITY, 1e19, -1e19] {
            assert!(SnapSegment::new([0., bad, 1., 1.]).is_err(), "{bad}");
        }
        assert!(validate(&vec![slant[0]; SNAP_SEGMENTS_MAX]).is_ok());
        assert!(validate(&vec![slant[0]; SNAP_SEGMENTS_MAX + 1]).is_err());
    }

    #[test]
    fn the_scene_and_the_drawn_shape_are_one_snap() {
        let edge = segments(&[[100., 200., 300., 200.]]);
        let q = asked(296, 203, 10);
        // nothing of the scene in reach: the shape's
        assert_eq!(over(&edge, &q, None), at(300., 200., SnapKind::Vertex));
        // a vertex before an edge, either way round
        assert_eq!(
            over(&edge, &q, Some(&hit(296, 200, SnapKind::Edge))),
            at(300., 200., SnapKind::Vertex)
        );
        let mid = asked(200, 203, 10);
        assert_eq!(
            over(&edge, &mid, Some(&hit(205, 207, SnapKind::Vertex))),
            None
        );
        // two vertices: the nearer; two as near: the scene's
        assert_eq!(
            over(&edge, &q, Some(&hit(290, 203, SnapKind::Vertex))),
            at(300., 200., SnapKind::Vertex)
        );
        assert_eq!(
            over(&edge, &q, Some(&hit(298, 203, SnapKind::Vertex))),
            None
        );
        assert_eq!(
            over(&edge, &q, Some(&hit(300, 200, SnapKind::Vertex))),
            None
        );
        assert_eq!(
            over(&edge, &q, Some(&hit(292, 200, SnapKind::Vertex))),
            None
        );
        // two edges: the nearer; the error's edge lying on the layout's is
        // the layout's answer, the point being the same
        assert_eq!(
            over(&edge, &mid, Some(&hit(200, 200, SnapKind::Edge))),
            None
        );
        assert_eq!(
            over(&edge, &mid, Some(&hit(200, 208, SnapKind::Edge))),
            at(200., 200., SnapKind::Edge)
        );
        assert_eq!(
            over(&edge, &mid, Some(&hit(200, 204, SnapKind::Edge))),
            None
        );
        // past the shape's reach the scene's stands
        let far = asked(200, 240, 10);
        assert_eq!(
            over(&edge, &far, Some(&hit(200, 241, SnapKind::Edge))),
            None
        );
        assert_eq!(over(&edge, &far, None), None);
        // past f64's whole numbers a dbu is still a dbu: the scene's point
        // two away is nearer than the shape's three away
        let big = 1i64 << 60;
        let wide = segments(&[[big as f64, 0., big as f64, 50.]]);
        let q = asked(big + 3, 20, 10);
        assert_eq!(over(&wide, &q, None), at(big as f64, 20., SnapKind::Edge));
        assert_eq!(
            over(&wide, &q, Some(&hit(big + 1, 20, SnapKind::Edge))),
            None
        );
        assert_eq!(
            over(&wide, &q, Some(&hit(big + 7, 20, SnapKind::Edge))),
            at(big as f64, 20., SnapKind::Edge)
        );
    }
}
