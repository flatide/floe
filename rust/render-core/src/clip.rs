//! Exact, CPU-only export clipping for the Rust renderer.
//!
//! Geometry is flattened from the immutable exact `FrameScene`, clipped in
//! integer DBU, and written as one `FLOE_CLIP` OASIS cell.  KLayout rounds a
//! boundary intersection to nearest DBU with half ties toward +infinity; this
//! module owns that rule instead of inheriting the tiler's truncating tile
//! clip.  Sutherland-Hodgman may bridge disconnected components of a concave
//! subject along the clip boundary, so overlapping reverse boundary segments
//! are split, cancelled, and traced back into separate simple polygons.

use crate::query::{
    canonical_polygon, polygon_bbox, signed_area2, visit_scene_shapes,
    visit_scene_shapes_cancellable, SceneShapeKind,
};
use crate::{FrameScene, RenderCancellation, SceneQueryLayer};
use floe_oasis::doc::{PathRec, PolyRec, RectRec, Rep, TextRec};
use floe_oasis::write::{write_tree, WCell};
use floe_ovm::BBox;
use std::collections::BTreeMap;

type Point = (i64, i64);
type Edge = (Point, Point);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rational {
    numerator: i128,
    denominator: i128,
}

impl Rational {
    fn integer(value: i64) -> Self {
        Self {
            numerator: value as i128,
            denominator: 1,
        }
    }

    fn new(numerator: i128, denominator: i128) -> Result<Self, String> {
        if denominator == 0 {
            return Err("invalid clip rational denominator".to_string());
        }
        let (numerator, denominator) = if denominator < 0 {
            (
                numerator
                    .checked_neg()
                    .ok_or_else(|| "coordinate overflow: clip rational numerator".to_string())?,
                denominator
                    .checked_neg()
                    .ok_or_else(|| "coordinate overflow: clip rational denominator".to_string())?,
            )
        } else {
            (numerator, denominator)
        };
        let divisor = gcd(numerator.unsigned_abs(), denominator as u128) as i128;
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    fn subtract(self, other: Self) -> Result<Self, String> {
        let divisor = gcd(self.denominator as u128, other.denominator as u128) as i128;
        let left_scale = other.denominator / divisor;
        let right_scale = self.denominator / divisor;
        let left = self
            .numerator
            .checked_mul(left_scale)
            .ok_or_else(|| "coordinate overflow: clip rational subtraction".to_string())?;
        let right = other
            .numerator
            .checked_mul(right_scale)
            .ok_or_else(|| "coordinate overflow: clip rational subtraction".to_string())?;
        let numerator = left
            .checked_sub(right)
            .ok_or_else(|| "coordinate overflow: clip rational subtraction".to_string())?;
        let denominator = self
            .denominator
            .checked_mul(left_scale)
            .ok_or_else(|| "coordinate overflow: clip rational subtraction".to_string())?;
        Self::new(numerator, denominator)
    }

    fn add(self, other: Self) -> Result<Self, String> {
        self.subtract(Self::new(
            other
                .numerator
                .checked_neg()
                .ok_or_else(|| "coordinate overflow: clip rational addition".to_string())?,
            other.denominator,
        )?)
    }

    fn multiply(self, other: Self) -> Result<Self, String> {
        let left_cancel = gcd(self.numerator.unsigned_abs(), other.denominator as u128) as i128;
        let right_cancel = gcd(other.numerator.unsigned_abs(), self.denominator as u128) as i128;
        let numerator = (self.numerator / left_cancel)
            .checked_mul(other.numerator / right_cancel)
            .ok_or_else(|| "coordinate overflow: clip rational multiplication".to_string())?;
        let denominator = (self.denominator / right_cancel)
            .checked_mul(other.denominator / left_cancel)
            .ok_or_else(|| "coordinate overflow: clip rational multiplication".to_string())?;
        Self::new(numerator, denominator)
    }

    fn divide(self, other: Self) -> Result<Self, String> {
        if other.numerator == 0 {
            return Err("invalid clip edge parallel to its boundary".to_string());
        }
        self.multiply(Self::new(other.denominator, other.numerator)?)
    }

    fn compare_integer(self, value: i64) -> Result<std::cmp::Ordering, String> {
        let scaled = (value as i128)
            .checked_mul(self.denominator)
            .ok_or_else(|| "coordinate overflow: clip rational comparison".to_string())?;
        Ok(self.numerator.cmp(&scaled))
    }

    fn round_half_up(self) -> Result<i64, String> {
        let rounded = round_half_up_ratio(self.numerator, self.denominator)?;
        i64::try_from(rounded).map_err(|_| "coordinate overflow: rounded clip point".to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RationalPoint {
    x: Rational,
    y: Rational,
}

impl RationalPoint {
    fn integer(point: Point) -> Self {
        Self {
            x: Rational::integer(point.0),
            y: Rational::integer(point.1),
        }
    }

    fn round_half_up(self) -> Result<Point, String> {
        Ok((self.x.round_half_up()?, self.y.round_half_up()?))
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a.max(1)
}

#[derive(Debug, Default)]
pub struct ClipGeometry {
    pub rects: Vec<RectRec>,
    pub polys: Vec<PolyRec>,
}

impl ClipGeometry {
    pub fn records(&self) -> usize {
        self.rects.len().saturating_add(self.polys.len())
    }

    pub fn append_scene(
        &mut self,
        scene: &FrameScene,
        clip: BBox,
        layers: &[SceneQueryLayer],
    ) -> Result<(), String> {
        self.append_scene_impl(scene, clip, layers, None)
    }

    pub fn append_scene_cancellable(
        &mut self,
        scene: &FrameScene,
        clip: BBox,
        layers: &[SceneQueryLayer],
        generation: u64,
        cancellation: &RenderCancellation,
    ) -> Result<(), String> {
        self.append_scene_impl(scene, clip, layers, Some((generation, cancellation)))
    }

    fn append_scene_impl(
        &mut self,
        scene: &FrameScene,
        clip: BBox,
        layers: &[SceneQueryLayer],
        cancellation: Option<(u64, &RenderCancellation)>,
    ) -> Result<(), String> {
        if clip.x0 >= clip.x1 || clip.y0 >= clip.y1 {
            return Ok(());
        }
        let mut append = |shape: crate::query::SceneShape| {
            match shape.kind {
                SceneShapeKind::Rectangle => {
                    let bbox = polygon_bbox(&shape.points)
                        .ok_or_else(|| "clip rectangle has no bbox".to_string())?;
                    let x0 = bbox.x0.max(clip.x0);
                    let y0 = bbox.y0.max(clip.y0);
                    let x1 = bbox.x1.min(clip.x1);
                    let y1 = bbox.y1.min(clip.y1);
                    if x0 < x1 && y0 < y1 {
                        self.rects.push(RectRec {
                            layer: shape.layer,
                            dt: shape.datatype,
                            x: x0,
                            y: y0,
                            w: x1 - x0,
                            h: y1 - y0,
                            rep: Rep::One,
                        });
                    }
                }
                SceneShapeKind::Polygon => {
                    for points in clip_polygon_components(&shape.points, clip)? {
                        self.polys.push(PolyRec {
                            layer: shape.layer,
                            dt: shape.datatype,
                            pts: points,
                            rep: Rep::One,
                        });
                    }
                }
            }
            Ok(())
        };
        match cancellation {
            Some((generation, cancellation)) => visit_scene_shapes_cancellable(
                scene,
                clip,
                layers,
                generation,
                cancellation,
                &mut append,
            ),
            None => visit_scene_shapes(scene, clip, layers, &mut append),
        }
    }

    pub fn oasis_bytes(&self, unit: f64) -> Result<Vec<u8>, String> {
        self.oasis_bytes_named(unit, "FLOE_CLIP")
    }

    pub fn oasis_bytes_named(&self, unit: f64, cell_name: &str) -> Result<Vec<u8>, String> {
        if !unit.is_finite() || unit <= 0.0 {
            return Err(format!("invalid OASIS unit: {unit}"));
        }
        if cell_name.is_empty() {
            return Err("clip cell name must not be empty".to_string());
        }
        let paths: [PathRec; 0] = [];
        let texts: [TextRec; 0] = [];
        write_tree(
            &[WCell {
                name: cell_name.to_string(),
                rects: &self.rects,
                polys: &self.polys,
                paths: &paths,
                texts: &texts,
                places: Vec::new(),
            }],
            unit,
        )
        .map_err(|error| format!("encode clip OASIS: {error}"))
    }
}

fn clip_polygon_components(points: &[Point], clip: BBox) -> Result<Vec<Vec<Point>>, String> {
    if points.len() < 3 || clip.x0 >= clip.x1 || clip.y0 >= clip.y1 {
        return Ok(Vec::new());
    }
    let canonical = match canonical_polygon(points.to_vec()) {
        Ok(points) => points,
        Err(error) if error.contains("fewer than 3 distinct vertices") => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut current: Vec<RationalPoint> =
        canonical.into_iter().map(RationalPoint::integer).collect();
    for edge in 0..4 {
        current = clip_one_edge(&current, clip, edge)?;
        if current.len() < 3 {
            return Ok(Vec::new());
        }
    }
    let rounded = current
        .into_iter()
        .map(RationalPoint::round_half_up)
        .collect::<Result<Vec<_>, _>>()?;
    split_cancel_trace(rounded, clip)
}

fn clip_one_edge(
    points: &[RationalPoint],
    clip: BBox,
    edge: u8,
) -> Result<Vec<RationalPoint>, String> {
    let inside = |point: RationalPoint| -> Result<bool, String> {
        Ok(match edge {
            0 => point.x.compare_integer(clip.x0)?.is_ge(),
            1 => point.y.compare_integer(clip.y0)?.is_ge(),
            2 => point.x.compare_integer(clip.x1)?.is_le(),
            3 => point.y.compare_integer(clip.y1)?.is_le(),
            _ => unreachable!(),
        })
    };
    let mut output = Vec::with_capacity(points.len().saturating_add(4));
    let mut previous = *points
        .last()
        .ok_or_else(|| "clip polygon is empty".to_string())?;
    for &point in points {
        let point_inside = inside(point)?;
        let previous_inside = inside(previous)?;
        if point_inside {
            if !previous_inside {
                output.push(edge_intersection(previous, point, clip, edge)?);
            }
            output.push(point);
        } else if previous_inside {
            output.push(edge_intersection(previous, point, clip, edge)?);
        }
        previous = point;
    }
    output.dedup();
    if output.len() > 1 && output.first() == output.last() {
        output.pop();
    }
    Ok(output)
}

fn edge_intersection(
    a: RationalPoint,
    b: RationalPoint,
    clip: BBox,
    edge: u8,
) -> Result<RationalPoint, String> {
    let (line, vertical) = match edge {
        0 => (clip.x0, true),
        1 => (clip.y0, false),
        2 => (clip.x1, true),
        3 => (clip.y1, false),
        _ => unreachable!(),
    };
    if vertical {
        let along = Rational::integer(line)
            .subtract(a.x)?
            .divide(b.x.subtract(a.x)?)?;
        let y = a.y.add(b.y.subtract(a.y)?.multiply(along)?)?;
        Ok(RationalPoint {
            x: Rational::integer(line),
            y,
        })
    } else {
        let along = Rational::integer(line)
            .subtract(a.y)?
            .divide(b.y.subtract(a.y)?)?;
        let x = a.x.add(b.x.subtract(a.x)?.multiply(along)?)?;
        Ok(RationalPoint {
            x,
            y: Rational::integer(line),
        })
    }
}

/// Nearest integer, with exact half ties toward positive infinity. This is
/// KLayout's integer polygon-intersection rule (including negative values).
fn round_half_up_ratio(numerator: i128, denominator: i128) -> Result<i128, String> {
    if denominator == 0 {
        return Err("invalid clip edge parallel to its boundary".to_string());
    }
    let (numerator, denominator) = if denominator < 0 {
        (
            numerator
                .checked_neg()
                .ok_or_else(|| "coordinate overflow: clip ratio numerator".to_string())?,
            denominator
                .checked_neg()
                .ok_or_else(|| "coordinate overflow: clip ratio denominator".to_string())?,
        )
    } else {
        (numerator, denominator)
    };
    let quotient = numerator.div_euclid(denominator);
    let remainder = numerator.rem_euclid(denominator);
    let round_up = remainder
        .checked_mul(2)
        .ok_or_else(|| "coordinate overflow: clip ratio remainder".to_string())?
        >= denominator;
    quotient
        .checked_add(i128::from(round_up))
        .ok_or_else(|| "coordinate overflow: clip rounded ratio".to_string())
}

fn split_cancel_trace(points: Vec<Point>, clip: BBox) -> Result<Vec<Vec<Point>>, String> {
    let mut counts = BTreeMap::<Edge, usize>::new();
    for (&a, &b) in points.iter().zip(points.iter().cycle().skip(1)) {
        for edge in split_boundary_edge(a, b, &points, clip) {
            if edge.0 == edge.1 {
                continue;
            }
            let reverse = (edge.1, edge.0);
            if let Some(count) = counts.get_mut(&reverse) {
                *count -= 1;
                if *count == 0 {
                    counts.remove(&reverse);
                }
            } else {
                *counts.entry(edge).or_default() += 1;
            }
        }
    }

    let mut adjacency = BTreeMap::<Point, BTreeMap<Point, usize>>::new();
    let mut edge_count = 0usize;
    for ((a, b), count) in counts {
        if count == 0 {
            continue;
        }
        *adjacency.entry(a).or_default().entry(b).or_default() += count;
        edge_count = edge_count
            .checked_add(count)
            .ok_or_else(|| "limit exceeded: clipped polygon edges".to_string())?;
    }

    let mut components = Vec::new();
    while edge_count > 0 {
        let (&start, outgoing) = adjacency
            .iter()
            .find(|(_, outgoing)| !outgoing.is_empty())
            .ok_or_else(|| "invalid clipped polygon: missing start edge".to_string())?;
        let &first = outgoing
            .keys()
            .next()
            .ok_or_else(|| "invalid clipped polygon: empty start adjacency".to_string())?;
        take_edge(&mut adjacency, start, first)?;
        edge_count -= 1;
        let mut cycle = vec![start];
        let mut previous = start;
        let mut current = first;
        while current != start {
            cycle.push(current);
            let outgoing = adjacency
                .get(&current)
                .filter(|outgoing| !outgoing.is_empty())
                .ok_or_else(|| {
                    format!(
                        "invalid clipped polygon: open boundary at {},{}",
                        current.0, current.1
                    )
                })?;
            let next = choose_clockwise(previous, current, outgoing.keys().copied())?;
            take_edge(&mut adjacency, current, next)?;
            edge_count -= 1;
            previous = current;
            current = next;
            if cycle.len() > points.len().saturating_mul(4).saturating_add(16) {
                return Err("invalid clipped polygon: boundary cycle overflow".to_string());
            }
        }
        if cycle.len() < 3 {
            continue;
        }
        let canonical = match canonical_polygon(cycle) {
            Ok(points) => points,
            Err(error) if error.contains("fewer than 3 distinct vertices") => continue,
            Err(error) => return Err(error),
        };
        if signed_area2(&canonical)? != 0 {
            components.push(canonical);
        }
    }
    components.sort();
    Ok(components)
}

fn split_boundary_edge(a: Point, b: Point, points: &[Point], clip: BBox) -> Vec<Edge> {
    let vertical = if a.0 == b.0 && (a.0 == clip.x0 || a.0 == clip.x1) {
        Some(true)
    } else if a.1 == b.1 && (a.1 == clip.y0 || a.1 == clip.y1) {
        Some(false)
    } else {
        None
    };
    let Some(vertical) = vertical else {
        return vec![(a, b)];
    };
    let mut stops = vec![a, b];
    for &point in points {
        let same_line = if vertical {
            point.0 == a.0
        } else {
            point.1 == a.1
        };
        let between = if vertical {
            point.1 >= a.1.min(b.1) && point.1 <= a.1.max(b.1)
        } else {
            point.0 >= a.0.min(b.0) && point.0 <= a.0.max(b.0)
        };
        if same_line && between {
            stops.push(point);
        }
    }
    stops.sort_by_key(|point| if vertical { point.1 } else { point.0 });
    stops.dedup();
    let forward = if vertical { a.1 <= b.1 } else { a.0 <= b.0 };
    if !forward {
        stops.reverse();
    }
    stops
        .windows(2)
        .map(|window| (window[0], window[1]))
        .collect()
}

fn take_edge(
    adjacency: &mut BTreeMap<Point, BTreeMap<Point, usize>>,
    from: Point,
    to: Point,
) -> Result<(), String> {
    let outgoing = adjacency
        .get_mut(&from)
        .ok_or_else(|| "invalid clipped polygon: missing adjacency".to_string())?;
    let count = outgoing
        .get_mut(&to)
        .ok_or_else(|| "invalid clipped polygon: missing directed edge".to_string())?;
    *count -= 1;
    if *count == 0 {
        outgoing.remove(&to);
    }
    Ok(())
}

fn choose_clockwise(
    previous: Point,
    current: Point,
    candidates: impl Iterator<Item = Point>,
) -> Result<Point, String> {
    let incoming = (
        current.0 as i128 - previous.0 as i128,
        current.1 as i128 - previous.1 as i128,
    );
    let mut best = None;
    for candidate in candidates {
        let direction = (
            candidate.0 as i128 - current.0 as i128,
            candidate.1 as i128 - current.1 as i128,
        );
        best = Some(match best {
            None => (candidate, direction),
            Some((best_point, best_direction)) => {
                if clockwise_before(incoming, direction, best_direction)?
                    || (same_direction(direction, best_direction)? && candidate < best_point)
                {
                    (candidate, direction)
                } else {
                    (best_point, best_direction)
                }
            }
        });
    }
    best.map(|(point, _)| point)
        .ok_or_else(|| "invalid clipped polygon: no outgoing edge".to_string())
}

fn clockwise_before(
    reference: (i128, i128),
    a: (i128, i128),
    b: (i128, i128),
) -> Result<bool, String> {
    let (a_cross, a_dot) = cross_dot(reference, a)?;
    let (b_cross, b_dot) = cross_dot(reference, b)?;
    let a_half = !(a_cross < 0 || (a_cross == 0 && a_dot >= 0));
    let b_half = !(b_cross < 0 || (b_cross == 0 && b_dot >= 0));
    if a_half != b_half {
        return Ok(!a_half);
    }
    Ok(vector_cross(a, b)? < 0)
}

fn same_direction(a: (i128, i128), b: (i128, i128)) -> Result<bool, String> {
    Ok(vector_cross(a, b)? == 0 && vector_dot(a, b)? > 0)
}

fn cross_dot(a: (i128, i128), b: (i128, i128)) -> Result<(i128, i128), String> {
    Ok((vector_cross(a, b)?, vector_dot(a, b)?))
}

fn vector_cross(a: (i128, i128), b: (i128, i128)) -> Result<i128, String> {
    let first =
        a.0.checked_mul(b.1)
            .ok_or_else(|| "coordinate overflow: clip turn cross".to_string())?;
    let second =
        a.1.checked_mul(b.0)
            .ok_or_else(|| "coordinate overflow: clip turn cross".to_string())?;
    first
        .checked_sub(second)
        .ok_or_else(|| "coordinate overflow: clip turn cross".to_string())
}

fn vector_dot(a: (i128, i128), b: (i128, i128)) -> Result<i128, String> {
    let first =
        a.0.checked_mul(b.0)
            .ok_or_else(|| "coordinate overflow: clip turn dot".to_string())?;
    let second =
        a.1.checked_mul(b.1)
            .ok_or_else(|| "coordinate overflow: clip turn dot".to_string())?;
    first
        .checked_add(second)
        .ok_or_else(|| "coordinate overflow: clip turn dot".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box10() -> BBox {
        BBox {
            x0: 0,
            y0: 0,
            x1: 10,
            y1: 10,
        }
    }

    #[test]
    fn diagonal_intersections_match_klayout_half_up_rounding() {
        let got = clip_polygon_components(&[(-3, 1), (13, 4), (2, 13)], box10()).unwrap();
        assert_eq!(
            got,
            vec![vec![(0, 2), (0, 8), (1, 10), (6, 10), (10, 6), (10, 3)]]
        );
        assert_eq!(round_half_up_ratio(1, 2).unwrap(), 1);
        assert_eq!(round_half_up_ratio(-1, 2).unwrap(), 0);
        assert_eq!(round_half_up_ratio(-3, 2).unwrap(), -1);
    }

    #[test]
    fn concave_clip_splits_disconnected_components_like_klayout() {
        let got = clip_polygon_components(
            &[
                (-5, 2),
                (8, 2),
                (8, 4),
                (-2, 4),
                (-2, 6),
                (8, 6),
                (8, 8),
                (-5, 8),
            ],
            box10(),
        )
        .unwrap();
        assert_eq!(
            got,
            vec![
                vec![(0, 2), (0, 4), (8, 4), (8, 2)],
                vec![(0, 6), (0, 8), (8, 8), (8, 6)],
            ]
        );
    }

    #[test]
    fn boundary_touch_and_degenerate_intersections_emit_nothing() {
        assert!(
            clip_polygon_components(&[(10, 2), (12, 2), (12, 5), (10, 5)], box10())
                .unwrap()
                .is_empty()
        );
        assert!(clip_polygon_components(&[(0, 0), (0, 5), (0, 10)], box10())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn clipped_oasis_is_a_single_parseable_cell() {
        let geometry = ClipGeometry {
            rects: vec![RectRec {
                layer: 1,
                dt: 0,
                x: 0,
                y: 0,
                w: 5,
                h: 5,
                rep: Rep::One,
            }],
            polys: vec![PolyRec {
                layer: 2,
                dt: 3,
                pts: vec![(0, 2), (0, 4), (8, 4), (8, 2)],
                rep: Rep::One,
            }],
        };
        let bytes = geometry.oasis_bytes(1000.0).unwrap();
        let parsed = floe_oasis::doc::parse_doc(&bytes).unwrap();
        assert_eq!(parsed.cells.len(), 1);
        assert_eq!(parsed.cells[0].name, "FLOE_CLIP");
        assert_eq!(parsed.cells[0].rects.len(), 1);
        assert_eq!(parsed.cells[0].polys.len(), 1);
        assert!(parsed.cells[0].places.is_empty());
        assert!(parsed.cells[0].texts.is_empty());
    }
}
