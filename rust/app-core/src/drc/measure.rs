//! Existing floe/drc.py CD policy: simple edges/gaps and rectangle spans,
//! not a general polygon width, signoff check, or SVRF constraint evaluator.
use crate::{check_cancelled, Error, ErrorKind, Result};
use std::sync::atomic::AtomicUsize;

#[derive(Clone, Debug, PartialEq)]
pub struct CdSegment {
    pub endpoints_um: [[f64; 2]; 2],
    pub distance_um: f64,
    /// Draw parallel to a single error edge, with screen-space extensions.
    pub offset: bool,
}
type Point = [f64; 2];
type Segment = [Point; 2];
fn distance(a: Point, b: Point) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}
fn foot(p: Point, a: Point, b: Point) -> Point {
    let [dx, dy] = [b[0] - a[0], b[1] - a[1]];
    let squared = dx * dx + dy * dy;
    if squared == 0. {
        return a;
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / squared).clamp(0., 1.);
    [a[0] + t * dx, a[1] + t * dy]
}
fn orient(a: Point, b: Point, c: Point) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
fn simple(kind: char, p: &[Point]) -> Vec<Segment> {
    match (kind, p) {
        ('e', [a, b]) if distance(*a, *b) > 0. => vec![[*a, *b]],
        ('e', [a0, a1, b0, b1]) => {
            if (orient(*a0, *a1, *b0) > 0.) != (orient(*a0, *a1, *b1) > 0.)
                && (orient(*b0, *b1, *a0) > 0.) != (orient(*b0, *b1, *a1) > 0.)
            {
                return vec![];
            }
            let candidates = [
                [*a0, foot(*a0, *b0, *b1)],
                [*a1, foot(*a1, *b0, *b1)],
                [foot(*b0, *a0, *a1), *b0],
                [foot(*b1, *a0, *a1), *b1],
            ];
            // Strictly smaller preserves Python min's first-candidate tie.
            let mut best = candidates[0];
            let mut dmin = distance(best[0], best[1]);
            for c in &candidates[1..] {
                let d = distance(c[0], c[1]);
                if d < dmin {
                    best = *c;
                    dmin = d;
                }
            }
            if dmin <= 0. {
                return vec![];
            }
            let mid = [(a0[0] + a1[0]) * 0.5, (a0[1] + a1[1]) * 0.5];
            let fm = foot(mid, *b0, *b1);
            if distance(mid, fm) <= dmin * 1.0001 {
                return vec![[mid, fm]];
            }
            let mut out = vec![best];
            let [adx, ady] = [a1[0] - a0[0], a1[1] - a0[1]];
            let [bdx, bdy] = [b1[0] - b0[0], b1[1] - b0[1]];
            let al2 = adx * adx + ady * ady;
            let bl2 = bdx * bdx + bdy * bdy;
            let parallel =
                al2 > 0. && bl2 > 0. && (adx * bdy - ady * bdx).abs() <= 1e-12 * (al2 * bl2).sqrt();
            if parallel && best[0][0] != best[1][0] && best[0][1] != best[1][1] {
                let [lo, hi] = if best[0] <= best[1] {
                    best
                } else {
                    [best[1], best[0]]
                };
                let elbow = [hi[0], lo[1]];
                out.push([lo, elbow]);
                out.push([elbow, hi]);
            }
            out
        }
        ('p', [_, _, _, _]) => {
            let mut xs: Vec<_> = p.iter().map(|v| v[0]).collect();
            let mut ys: Vec<_> = p.iter().map(|v| v[1]).collect();
            xs.sort_by(f64::total_cmp);
            ys.sort_by(f64::total_cmp);
            xs.dedup();
            ys.dedup();
            if xs.len() != 2
                || ys.len() != 2
                || !xs.iter().all(|x| ys.iter().all(|y| p.contains(&[*x, *y])))
            {
                return vec![];
            }
            let mx = (xs[0] + xs[1]) * 0.5;
            let my = (ys[0] + ys[1]) * 0.5;
            vec![[[xs[0], my], [xs[1], my]], [[mx, ys[0]], [mx, ys[1]]]]
        }
        _ => vec![],
    }
}

/// At most three complete rulers, in legacy order: edge length; closest
/// gap then optional X/Y diagnostics; or rectangle width then height.
/// Unsupported, coincident/touching/crossing, and complex shapes return none.
pub fn cd_segments(kind: char, points_dbu: &[[i64; 2]], precision: f64) -> Result<Vec<CdSegment>> {
    if !precision.is_finite() || precision <= 0. {
        return Err(Error::input("invalid DRC measurement precision"));
    }
    if !matches!((kind, points_dbu.len()), ('e', 2 | 4) | ('p', 4)) {
        return Ok(vec![]);
    }
    // Translate in i128 BEFORE converting to f64: a one-DBU edge near i64::MAX
    // must not collapse to zero. Power-of-two scaling keeps all products
    // bounded, independently of precision and world position. No new cut/cap.
    let origin = points_dbu[0];
    let deltas: Vec<_> = points_dbu
        .iter()
        .map(|p| {
            [
                i128::from(p[0]) - i128::from(origin[0]),
                i128::from(p[1]) - i128::from(origin[1]),
            ]
        })
        .collect();
    let extent = deltas
        .iter()
        .flatten()
        .map(|v| v.unsigned_abs())
        .max()
        .unwrap();
    if extent == 0 {
        return Ok(vec![]);
    }
    let scale = 2f64.powi((128 - extent.leading_zeros()) as i32);
    let points: Vec<_> = deltas.iter().map(|p| p.map(|v| v as f64 / scale)).collect();
    let segments = simple(kind, &points);
    segments
        .into_iter()
        .map(|s| {
            let world = |p: Point| {
                [
                    (origin[0] as f64 + p[0] * scale) / precision,
                    (origin[1] as f64 + p[1] * scale) / precision,
                ]
            };
            let endpoints_um = [world(s[0]), world(s[1])];
            let distance_um = distance(s[0], s[1]) * scale / precision;
            if !endpoints_um.iter().flatten().all(|v| v.is_finite())
                || !distance_um.is_finite()
                || distance_um <= 0.
            {
                return Err(Error::input("unrepresentable DRC measurement"));
            }
            Ok(CdSegment {
                endpoints_um,
                distance_um,
                offset: kind == 'e' && points_dbu.len() == 2,
            })
        })
        .collect()
}

/// Legacy rule-detail measurements, in um or um². Unsupported geometry/metric
/// returns None, never a bounding-box approximation or a signoff verdict.
pub fn measured(
    kind: char,
    p: &[[i64; 2]],
    precision: f64,
    metric: &str,
    stop: &AtomicUsize,
) -> Result<Option<f64>> {
    check_cancelled(stop)?;
    if !precision.is_finite() || precision <= 0. {
        return Err(Error::input("invalid DRC measurement precision"));
    }
    let value = match metric {
        "area" if kind == 'p' && p.len() >= 3 => {
            if p.len() > super::RECORD_POINTS {
                return Err(super::limit("measurement point limit"));
            }
            // Integer, translated shoelace preserves one-DBU regions near i64
            // limits. Checked products/sum reject extremes rather than wrap or
            // quietly cancel large absolute floating-point products.
            let origin = p[0];
            let local = |q: [i64; 2]| {
                [
                    i128::from(q[0]) - i128::from(origin[0]),
                    i128::from(q[1]) - i128::from(origin[1]),
                ]
            };
            let mut sum = 0i128;
            for i in 0..p.len() {
                if i % 1024 == 0 {
                    check_cancelled(stop)?;
                }
                let (a, b) = (local(p[i]), local(p[(i + 1) % p.len()]));
                sum = a[0]
                    .checked_mul(b[1])
                    .and_then(|x| a[1].checked_mul(b[0]).and_then(|y| x.checked_sub(y)))
                    .and_then(|cross| sum.checked_add(cross))
                    .ok_or_else(|| {
                        Error::new(ErrorKind::Incomplete, "DRC area arithmetic overflow")
                    })?;
            }
            let area = sum.unsigned_abs() as f64 * 0.5 / precision / precision;
            if sum != 0 && area == 0. {
                return Err(Error::input("unrepresentable DRC area (underflow)"));
            }
            area
        }
        "width" | "space" | "enclosure" if kind == 'p' || (kind == 'e' && p.len() == 4) => {
            let segments = cd_segments(kind, p, precision)?;
            let Some(first) = segments.first() else {
                return Ok(None);
            };
            if kind == 'e' {
                first.distance_um
            } else {
                segments
                    .iter()
                    .map(|s| s.distance_um)
                    .fold(f64::INFINITY, f64::min)
            }
        }
        "length" if kind == 'e' && p.len() == 2 => {
            let dx = (i128::from(p[1][0]) - i128::from(p[0][0])) as f64 / precision;
            let dy = (i128::from(p[1][1]) - i128::from(p[0][1])) as f64 / precision;
            dx.hypot(dy)
        }
        _ => return Ok(None),
    };
    if !value.is_finite() {
        return Err(Error::input("unrepresentable DRC measurement"));
    }
    Ok(Some(value))
}

/// ASCII sources may contain fractional DBU values. Keep their parsed f64
/// micrometres; never round them into the packed integer measurement path.
pub fn measured_um(
    kind: char,
    points: &[[f64; 2]],
    metric: &str,
    stop: &AtomicUsize,
) -> Result<Option<f64>> {
    check_cancelled(stop)?;
    if points.len() > super::RECORD_POINTS {
        return Err(super::limit("measurement points"));
    }
    let supported = match metric {
        "area" => kind == 'p' && points.len() >= 3,
        "length" => kind == 'e' && points.len() == 2,
        "width" | "space" | "enclosure" => matches!(kind, 'p' | 'e') && points.len() == 4,
        _ => false,
    };
    if !supported {
        return Ok(None);
    }
    let mut extent = 0f64;
    for (i, v) in points.iter().enumerate() {
        if i % 1024 == 0 {
            check_cancelled(stop)?;
        }
        for &x in v {
            if !x.is_finite() {
                return Err(Error::input("non-finite ASCII DRC measurement coordinate"));
            }
            extent = extent.max(x.abs());
        }
    }
    let scale = if extent == 0. {
        1.
    } else {
        2f64.powi((extent.log2().floor() as i32).clamp(-1022, 1023))
    };
    let origin = points[0].map(|v| v / scale);
    let mut local = Vec::new();
    local
        .try_reserve_exact(points.len())
        .map_err(|_| super::limit("measurement allocation"))?;
    for (i, v) in points.iter().enumerate() {
        if i % 1024 == 0 {
            check_cancelled(stop)?;
        }
        local.push([v[0] / scale - origin[0], v[1] / scale - origin[1]]);
    }
    let value = match metric {
        "area" => {
            let mut sum = 0.;
            for i in 0..local.len() {
                if i % 1024 == 0 {
                    check_cancelled(stop)?;
                }
                let (a, b) = (local[i], local[(i + 1) % local.len()]);
                sum += a[0] * b[1] - a[1] * b[0];
            }
            let area = sum.abs() * 0.5 * scale * scale;
            if sum != 0. && area == 0. {
                return Err(Error::input("unrepresentable ASCII DRC area (underflow)"));
            }
            area
        }
        "length" => distance(local[0], local[1]) * scale,
        _ => {
            let s = simple(kind, &local);
            let Some(first) = s.first() else {
                return Ok(None);
            };
            (if kind == 'e' {
                distance(first[0], first[1])
            } else {
                s.iter()
                    .map(|v| distance(v[0], v[1]))
                    .fold(f64::INFINITY, f64::min)
            }) * scale
        }
    };
    if !value.is_finite() {
        return Err(Error::input("unrepresentable ASCII DRC measurement"));
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ascii_fractional_measurements_do_not_round_to_integer_dbu() {
        let stop = AtomicUsize::new(0);
        for origin in [0., -32.125, 2f64.powi(40)] {
            let p = [
                [origin, 0.125],
                [origin + 0.375, 0.125],
                [origin + 0.375, 0.625],
                [origin, 0.625],
            ];
            assert_eq!(measured_um('p', &p, "area", &stop).unwrap(), Some(0.1875));
            assert_eq!(measured_um('p', &p, "width", &stop).unwrap(), Some(0.375));
            assert_eq!(
                measured_um('e', &[p[0], p[2]], "length", &stop).unwrap(),
                Some(0.625)
            );
            assert_eq!(measured_um('e', &p, "space", &stop).unwrap(), Some(0.5));
            assert_eq!(measured_um('p', &p, "density", &stop).unwrap(), None);
        }
        assert_eq!(
            measured_um('e', &[[0.125, 0.125]; 2], "length", &stop).unwrap(),
            Some(0.)
        );
        assert_eq!(
            measured_um('p', &[[0., 0.]; 3], "width", &stop).unwrap(),
            None
        );
    }
    #[test]
    fn ascii_measurement_extremes_limits_and_cancellation_are_explicit() {
        let stop = AtomicUsize::new(0);
        for invalid in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert!(measured_um('e', &[[0., 0.], [invalid, 1.]], "length", &stop).is_err());
        }
        assert!(measured_um('e', &[[-f64::MAX, 0.], [f64::MAX, 0.]], "length", &stop).is_err());
        for size in [1e200, 1e-200] {
            assert!(measured_um('p', &[[0., 0.], [size, 0.], [0., size]], "area", &stop).is_err());
        }
        let too_many = vec![[0., 0.]; super::super::RECORD_POINTS + 1];
        assert_eq!(
            measured_um('p', &too_many, "area", &stop).unwrap_err().kind,
            ErrorKind::Incomplete
        );
        assert_eq!(
            measured_um('e', &[[0., 0.], [1., 1.]], "length", &AtomicUsize::new(1))
                .unwrap_err()
                .kind,
            ErrorKind::Cancelled
        );
    }
    fn ends(kind: char, p: &[[i64; 2]]) -> Vec<Segment> {
        cd_segments(kind, p, 1.)
            .unwrap()
            .into_iter()
            .map(|s| s.endpoints_um)
            .collect()
    }
    #[test]
    fn legacy_simple_shapes_and_component_order() {
        assert_eq!(ends('e', &[[0, 0], [3, 4]]), vec![[[0., 0.], [3., 4.]]]);
        assert_eq!(
            ends('p', &[[0, 0], [4, 0], [4, 2], [0, 2]]),
            vec![[[0., 1.], [4., 1.]], [[2., 0.], [2., 2.]]]
        );
        assert_eq!(
            ends('e', &[[0, 0], [3, 0], [0, 1], [3, 1]]),
            vec![[[1.5, 0.], [1.5, 1.]]]
        );
        let p = [[0, 0], [1, 0], [2, 1], [3, 1]];
        let expected = vec![
            [[1., 0.], [2., 1.]],
            [[1., 0.], [2., 0.]],
            [[2., 0.], [2., 1.]],
        ];
        assert_eq!(ends('e', &p), expected);
        assert_eq!(ends('e', &[p[1], p[0], p[3], p[2]]), expected);
        let v = cd_segments('e', &p, 1000.).unwrap();
        assert!((v[0].distance_um - 2f64.sqrt() / 1000.).abs() < 1e-15);
        assert!(v.iter().all(|s| !s.offset));
        assert_eq!(
            cd_segments('e', &[[0, 0], [3, 4]], 1000.).unwrap()[0].distance_um,
            0.005
        );
        assert!(cd_segments('e', &[[0, 0], [3, 4]], 1000.).unwrap()[0].offset);
    }
    #[test]
    fn no_invented_rulers_for_complex_zero_touch_or_cross() {
        for (kind, p) in [
            ('p', vec![[0, 0], [1, 0], [0, 1]]),
            ('p', vec![[0, 0], [1, 1], [0, 2], [-1, 1]]),
            ('p', vec![[0, 0], [1, 0], [1, 1], [0, 1], [0, 0]]),
            ('e', vec![[3, 4], [3, 4]]),
            ('e', vec![[0, 0], [3, 0], [3, 0], [4, 0]]),
            ('e', vec![[0, 0], [3, 0], [1, -1], [1, 1]]),
            ('e', vec![[0, 0], [3, 0], [1, 0], [2, 0]]),
            ('x', vec![[0, 0], [3, 4]]),
        ] {
            assert!(ends(kind, &p).is_empty(), "{kind} {p:?}");
        }
    }
    #[test]
    fn local_delta_keeps_small_edges_at_large_origins_and_precision_extremes() {
        for origin in [i64::MIN, -100, 0, i64::MAX - 1] {
            let v = cd_segments('e', &[[origin, origin], [origin + 1, origin]], 1.).unwrap();
            assert_eq!(v[0].distance_um, 1.);
        }
        let v = cd_segments('e', &[[i64::MIN, 0], [i64::MAX, 0]], 1.).unwrap();
        assert_eq!(v[0].distance_um, (u64::MAX as f64));
        assert!(cd_segments('e', &[[0, 0], [1, 0]], f64::MAX).unwrap()[0].distance_um > 0.);
        assert!(
            cd_segments('e', &[[0, 0], [1, 0]], f64::MIN_POSITIVE).unwrap()[0]
                .distance_um
                .is_finite()
        );
        assert!(cd_segments('e', &[[0, 0], [1, 0]], f64::from_bits(1)).is_err());
        for invalid in [0., -1., f64::INFINITY, f64::NAN] {
            assert!(cd_segments('e', &[[0, 0], [1, 0]], invalid).is_err());
        }
    }
}
