//! Numeric counterpart of Python's MeasurementSelector.
//!
//! Statement chains are compiled by Python, including their representative
//! criterion and exact decimal bound ticks. Geometry and ordinary selection
//! run here. Choice -2 asks Python to resolve a rare floating-point boundary;
//! choice -1 is an ordinary unmeasurable/ambiguous result.

#[derive(Clone, Debug)]
pub struct Predicate {
    pub op: u8,
    pub bound: f64,
}

#[derive(Clone, Debug)]
pub struct Chain {
    pub metric: u8,
    pub ci: i32,
    pub op: u8,
    pub bound: f64,
    pub bound_ticks: Option<i64>,
    pub options: bool,
    pub predicates: Vec<Predicate>,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub uncertain_alternatives: bool,
    pub chains: Vec<Chain>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measurement {
    pub ticks: i64,
    pub choice: i32,
    pub estimated: u8,
}

const UNKNOWN: Measurement = Measurement {
    ticks: 0,
    choice: -1,
    estimated: 0,
};
const FALLBACK: Measurement = Measurement {
    ticks: 0,
    choice: -2,
    estimated: 0,
};
const SCALE: f64 = 100_000.0;

#[derive(Clone, Copy)]
struct Candidates {
    values: [f64; 2],
    len: usize,
    // Python's compensated hypot and the host libm can differ by an ulp.
    // This matters only next to a predicate or five-decimal tick boundary.
    inexact_hypot: bool,
    fallback: bool,
}

impl Candidates {
    fn empty() -> Self {
        Self {
            values: [0.0; 2],
            len: 0,
            inexact_hypot: false,
            fallback: false,
        }
    }

    fn one(value: f64, inexact_hypot: bool) -> Self {
        if !value.is_finite() {
            return Self::empty();
        }
        Self {
            values: [value, 0.0],
            len: 1,
            inexact_hypot,
            fallback: false,
        }
    }

    fn fallback() -> Self {
        Self {
            fallback: true,
            ..Self::empty()
        }
    }
}

fn ulp(value: f64) -> f64 {
    let exponent = (value.abs().to_bits() >> 52) & 0x7ff;
    match exponent {
        0 => f64::from_bits(1),
        0x7ff => value.abs(),
        1..=52 => f64::from_bits(1_u64 << (exponent - 1)),
        _ => f64::from_bits((exponent - 52) << 52),
    }
}

fn predicate(op: u8, value: f64, bound: f64) -> Option<bool> {
    Some(match op {
        0 => value < bound,
        1 => value <= bound,
        2 => value > bound,
        3 => value >= bound,
        4 => value == bound,
        5 => value != bound,
        _ => return None,
    })
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn foot(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let length2 = dx * dx + dy * dy;
    if length2 <= 0.0 {
        return a;
    }
    let t = ((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length2;
    let t = t.clamp(0.0, 1.0);
    (a.0 + t * dx, a.1 + t * dy)
}

fn orient(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

fn cd_candidates(kind: u8, points: &[(f64, f64)]) -> Candidates {
    if points.len() != 4 {
        return Candidates::empty();
    }
    if kind == 0 {
        let mut x0 = points[0].0;
        let mut x1 = x0;
        let mut y0 = points[0].1;
        let mut y1 = y0;
        for &(x, y) in &points[1..] {
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        if x0 == x1 || y0 == y1 {
            return Candidates::empty();
        }
        let mut corners = 0_u8;
        for &(x, y) in points {
            if (x != x0 && x != x1) || (y != y0 && y != y1) {
                return Candidates::empty();
            }
            let corner = u8::from(x == x1) + 2 * u8::from(y == y1);
            corners |= 1 << corner;
        }
        if corners != 15 {
            return Candidates::empty();
        }
        let w = x1 - x0;
        let h = y1 - y0;
        if !w.is_finite() || !h.is_finite() {
            return Candidates::fallback();
        }
        if w == h {
            return Candidates::one(w, false);
        }
        return Candidates {
            values: [w.min(h), w.max(h)],
            len: 2,
            inexact_hypot: false,
            fallback: false,
        };
    }
    if kind != 1 {
        return Candidates::empty();
    }
    let (a0, a1, b0, b1) = (points[0], points[1], points[2], points[3]);
    if ((orient(a0, a1, b0) > 0.0) != (orient(a0, a1, b1) > 0.0))
        && ((orient(b0, b1, a0) > 0.0) != (orient(b0, b1, a1) > 0.0))
    {
        return Candidates::empty();
    }
    let pairs = [
        (a0, foot(a0, b0, b1)),
        (a1, foot(a1, b0, b1)),
        (foot(b0, a0, a1), b0),
        (foot(b1, a0, a1), b1),
    ];
    let mut minimum = f64::INFINITY;
    let mut inexact = false;
    for (a, b) in pairs {
        let d = distance(a, b);
        if !d.is_finite() {
            return Candidates::fallback();
        }
        minimum = minimum.min(d);
        inexact |= a.0 != b.0 && a.1 != b.1;
    }
    if minimum <= 0.0 {
        return Candidates::empty();
    }
    // cd_segments may center the drawing when that distance equals dmin.
    // Its later horizontal/vertical diagnostic rulers are never candidates.
    Candidates::one(minimum, inexact)
}

// Error-free partial sums, including the final half-even fix used by fsum.
// A fixed buffer avoids a heap allocation per polygon. Its conservative
// overflow fallback is safe even for unusual exponent mixtures.
struct PreciseSum {
    partials: [f64; 64],
    len: usize,
    failed: bool,
}

impl PreciseSum {
    fn new() -> Self {
        Self {
            partials: [0.0; 64],
            len: 0,
            failed: false,
        }
    }

    fn add(&mut self, mut x: f64) {
        if !x.is_finite() || self.failed {
            self.failed = true;
            return;
        }
        let mut used = 0;
        for j in 0..self.len {
            let mut y = self.partials[j];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let yr = hi - x;
            let lo = y - yr;
            if !hi.is_finite() {
                self.failed = true;
                return;
            }
            if lo != 0.0 {
                self.partials[used] = lo;
                used += 1;
            }
            x = hi;
        }
        self.len = used;
        if x != 0.0 {
            if self.len == self.partials.len() {
                self.failed = true;
            } else {
                self.partials[self.len] = x;
                self.len += 1;
            }
        }
    }

    fn finish(&self) -> Option<f64> {
        if self.failed {
            return None;
        }
        if self.len == 0 {
            return Some(0.0);
        }
        let mut n = self.len - 1;
        let mut hi = self.partials[n];
        let mut lo = 0.0;
        while n != 0 {
            n -= 1;
            let x = hi;
            let y = self.partials[n];
            hi = x + y;
            let yr = hi - x;
            lo = y - yr;
            if lo != 0.0 {
                break;
            }
        }
        if n != 0
            && ((lo < 0.0 && self.partials[n - 1] < 0.0)
                || (lo > 0.0 && self.partials[n - 1] > 0.0))
        {
            let y = lo * 2.0;
            let x = hi + y;
            let yr = x - hi;
            if y == yr {
                hi = x;
            }
        }
        hi.is_finite().then_some(hi)
    }
}

fn area_candidates(kind: u8, points: &[(f64, f64)]) -> Candidates {
    if kind != 0 || points.len() < 3 {
        return Candidates::empty();
    }
    let (ox, oy) = points[0];
    let mut sum = PreciseSum::new();
    for i in 0..points.len() {
        let (x0, y0) = points[i];
        let (x1, y1) = points[(i + 1) % points.len()];
        sum.add((x0 - ox) * (y1 - oy) - (x1 - ox) * (y0 - oy));
    }
    match sum.finish() {
        Some(value) => Candidates::one(value.abs() / 2.0, false),
        None => Candidates::fallback(),
    }
}

fn area_roundoff(points: &[(f64, f64)], coordinate_ulp: f64) -> Option<f64> {
    let mut perimeter = PreciseSum::new();
    for i in 0..points.len() {
        perimeter.add(distance(points[i], points[(i + 1) % points.len()]));
    }
    // Preserve Python's multiply order: ulp *= perimeter; return 4 * ulp.
    perimeter.finish().map(|p| 4.0 * (coordinate_ulp * p))
}

fn to_ticks(value: f64) -> Result<i64, Measurement> {
    // Decimal(str(value)) rejects a decimal exponent greater than 13.
    if !value.is_finite() || value.abs() >= 1.0e14 {
        return Err(UNKNOWN);
    }
    let scaled = value * SCALE;
    let guard = (8.0 * ulp(scaled)).max(8.0 * ulp(value) * SCALE);
    let distance_to_half = (scaled.abs().fract() - 0.5).abs();
    // Away from this tiny interval, binary multiplication and Python's
    // shortest-decimal HALF_UP conversion necessarily round to one tick.
    if distance_to_half <= guard {
        return Err(FALLBACK);
    }
    let rounded = scaled.round();
    if rounded >= i64::MAX as f64 || rounded <= -(i64::MAX as f64) {
        return Err(FALLBACK);
    }
    Ok(rounded as i64)
}

/// Measure a polygon (kind 0) or edge record (kind 1) using compiled chains.
/// Metric IDs: width, space, notch, enclosure, overlap, extension, length,
/// area = 0..7. Operator IDs: <, <=, >, >=, ==, != = 0..5.
pub fn measure(kind: u8, points: &[(f64, f64)], plan: &Plan) -> Measurement {
    if kind > 1
        || points
            .iter()
            .any(|&(x, y)| !x.is_finite() || !y.is_finite())
    {
        return UNKNOWN;
    }
    let coordinate_ulp = points
        .iter()
        .fold(0.0_f64, |u, &(x, y)| u.max(ulp(x)).max(ulp(y)));
    let mut cd = None;
    let mut length = None;
    let mut area = None;
    let mut area_tolerance = None;
    let mut selected: Option<(&Chain, f64)> = None;
    let mut estimated = plan.uncertain_alternatives;
    for chain in &plan.chains {
        if chain.metric > 7
            || chain.ci < 0
            || chain.op > 5
            || !chain.bound.is_finite()
            || chain
                .predicates
                .iter()
                .any(|p| p.op > 5 || !p.bound.is_finite())
        {
            return UNKNOWN;
        }
        let candidates = match chain.metric {
            0..=5 => *cd.get_or_insert_with(|| cd_candidates(kind, points)),
            6 => *length.get_or_insert_with(|| {
                if kind == 1 && points.len() == 2 {
                    Candidates::one(
                        distance(points[0], points[1]),
                        points[0].0 != points[1].0 && points[0].1 != points[1].1,
                    )
                } else {
                    Candidates::empty()
                }
            }),
            _ => *area.get_or_insert_with(|| area_candidates(kind, points)),
        };
        if candidates.fallback {
            return FALLBACK;
        }
        if candidates.len == 0 {
            continue;
        }
        let roundoff = if chain.metric == 7 {
            match *area_tolerance.get_or_insert_with(|| area_roundoff(points, coordinate_ulp)) {
                Some(value) => value,
                None => return FALLBACK,
            }
        } else {
            4.0 * coordinate_ulp
        };
        let mut actual = None;
        for &candidate in &candidates.values[..candidates.len] {
            let mut accepted = true;
            for condition in &chain.predicates {
                let tolerance = roundoff
                    .max(4.0 * ulp(candidate))
                    .max(4.0 * ulp(condition.bound));
                // A libm result one ulp outside Python's guard may land
                // inside it under Python's compensated hypot. Include a
                // safety band on both sides of that guard, not just at the
                // mathematical predicate boundary itself. AREA's perimeter
                // also contains hypots, although its shoelace CD does not.
                let margin = if candidates.inexact_hypot {
                    8.0 * ulp(candidate).max(ulp(condition.bound))
                } else if chain.metric == 7 {
                    8.0 * ulp(roundoff)
                } else {
                    0.0
                };
                // Python's guard can alter both a match and its estimated
                // provenance. Resolve all such cases in its exact routine.
                if (candidate != condition.bound || candidates.inexact_hypot)
                    && (candidate - condition.bound).abs() <= tolerance + margin
                {
                    return FALLBACK;
                }
                accepted &= predicate(condition.op, candidate, condition.bound).unwrap_or(false);
            }
            if !accepted {
                continue;
            }
            match actual {
                None => actual = Some(candidate),
                Some(_) if chain.op == 2 || chain.op == 3 => actual = Some(candidate),
                Some(old) if chain.op == 5 => {
                    let old_delta = (old - chain.bound).abs();
                    let new_delta = (candidate - chain.bound).abs();
                    if new_delta > old_delta || (new_delta == old_delta && candidate > old) {
                        actual = Some(candidate);
                    }
                }
                _ => (),
            }
        }
        let Some(actual) = actual else { continue };
        if let Some((previous, previous_actual)) = selected {
            if chain.metric != previous.metric
                || chain.op != previous.op
                || chain.bound != previous.bound
                || actual != previous_actual
            {
                return UNKNOWN;
            }
        } else {
            selected = Some((chain, actual));
        }
        estimated |= candidates.len > 1 || chain.options;
    }
    let Some((chain, actual)) = selected else {
        return UNKNOWN;
    };
    let Some(bound_ticks) = chain.bound_ticks else {
        return UNKNOWN;
    };
    let ticks = match to_ticks(actual) {
        Ok(value) => value,
        Err(result) => return result,
    };
    if (i128::from(ticks) - i128::from(bound_ticks)).abs() > i128::from(i64::MAX) {
        return UNKNOWN;
    }
    Measurement {
        ticks,
        choice: chain.ci,
        estimated: u8::from(estimated),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(metric: u8, op: u8, bound: f64) -> Chain {
        Chain {
            metric,
            ci: 0,
            op,
            bound,
            bound_ticks: Some((bound * SCALE).round() as i64),
            options: false,
            predicates: vec![Predicate { op, bound }],
        }
    }

    fn plan(chain: Chain) -> Plan {
        Plan {
            uncertain_alternatives: false,
            chains: vec![chain],
        }
    }

    fn rectangle(w: f64, h: f64) -> [(f64, f64); 4] {
        [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]
    }

    #[test]
    fn rectangle_chooses_worst_and_marks_only_distinct_spans() {
        assert_eq!(
            measure(0, &rectangle(0.02, 0.03), &plan(chain(0, 0, 0.05))),
            Measurement {
                ticks: 2000,
                choice: 0,
                estimated: 1
            }
        );
        assert_eq!(
            measure(0, &rectangle(0.10, 0.12), &plan(chain(0, 2, 0.08))),
            Measurement {
                ticks: 12000,
                choice: 0,
                estimated: 1
            }
        );
        assert_eq!(
            measure(0, &rectangle(0.02, 0.02), &plan(chain(0, 0, 0.05))).estimated,
            0
        );
        let mut points = rectangle(0.02, 0.03);
        points.swap(1, 2); // The established selector uses a corner set.
        assert_eq!(measure(0, &points, &plan(chain(0, 0, 0.05))).ticks, 2000);
    }

    #[test]
    fn wrapped_ranges_and_all_six_cd_metrics() {
        for metric in 0..6 {
            let mut con = chain(metric, 0, 0.05);
            con.ci = 3;
            con.predicates.insert(0, Predicate { op: 2, bound: 0.0 });
            assert_eq!(
                measure(0, &rectangle(0.02, 0.10), &plan(con)),
                Measurement {
                    ticks: 2000,
                    choice: 3,
                    estimated: 1
                }
            );
        }
    }

    #[test]
    fn exact_predicate_bounds_are_native_for_axis_aligned_geometry() {
        let points = rectangle(0.02, 0.02);
        for (op, known) in [
            (0, false),
            (1, true),
            (2, false),
            (3, true),
            (4, true),
            (5, false),
        ] {
            assert_eq!(
                measure(0, &points, &plan(chain(0, op, 0.02))).choice,
                if known { 0 } else { -1 }
            );
        }
        assert_eq!(
            measure(0, &rectangle(0.02, 0.08), &plan(chain(0, 5, 0.0))).ticks,
            8000
        );
    }

    #[test]
    fn edge_pair_uses_only_true_gap_and_rejects_crossing_or_touching() {
        let con = plan(chain(1, 0, 1.0));
        let facing = [(0.0, 0.0), (0.1, 0.0), (0.0, 0.02345), (0.1, 0.02345)];
        assert_eq!(
            measure(1, &facing, &con),
            Measurement {
                ticks: 2345,
                choice: 0,
                estimated: 0
            }
        );
        let disjoint = [(0.0, 0.0), (1.0, 0.0), (1.03, 0.04), (2.0, 0.04)];
        assert_eq!(measure(1, &disjoint, &con).ticks, 5000);
        let crossing = [(0.0, 0.0), (1.0, 1.0), (0.0, 1.0), (1.0, 0.0)];
        assert_eq!(measure(1, &crossing, &con), UNKNOWN);
        let touching = [(0.0, 0.0), (1.0, 0.0), (1.0, 0.0), (2.0, 0.0)];
        assert_eq!(measure(1, &touching, &con), UNKNOWN);
    }

    #[test]
    fn single_edge_length_includes_zero_but_is_not_a_cd_pair() {
        let con = plan(chain(6, 0, 10.0));
        assert_eq!(measure(1, &[(0.0, 0.0), (3.0, 4.0)], &con).ticks, 500000);
        assert_eq!(measure(1, &[(0.0, 0.0); 2], &con).choice, 0);
        assert_eq!(
            measure(1, &[(0.0, 0.0), (0.02, 0.0)], &plan(chain(0, 0, 0.05))),
            UNKNOWN
        );
    }

    #[test]
    fn area_uses_polygon_not_bbox_and_survives_translation_and_winding() {
        let con = plan(chain(7, 0, 30.0));
        let shapes = [
            (vec![(0.0, 0.0), (4.0, 0.0), (0.0, 3.0)], 600000),
            (
                vec![
                    (0.0, 0.0),
                    (3.0, 0.0),
                    (3.0, 1.0),
                    (1.0, 1.0),
                    (1.0, 3.0),
                    (0.0, 3.0),
                ],
                500000,
            ),
        ];
        for (points, expected) in shapes {
            for offset in [0.0, 1.0e9] {
                let mut translated: Vec<_> = points
                    .iter()
                    .map(|&(x, y)| (x + offset, y - offset))
                    .collect();
                assert_eq!(measure(0, &translated, &con).ticks, expected);
                translated.reverse();
                translated.push(translated[0]);
                assert_eq!(measure(0, &translated, &con).ticks, expected);
            }
        }
    }

    #[test]
    fn equivalent_duplicates_keep_first_choice_and_all_provenance() {
        let mut first = chain(0, 0, 0.05);
        first.ci = 7;
        let mut second = first.clone();
        second.ci = 9;
        second.options = true;
        let rules = Plan {
            uncertain_alternatives: false,
            chains: vec![first, second],
        };
        assert_eq!(
            measure(0, &rectangle(0.02, 0.02), &rules),
            Measurement {
                ticks: 2000,
                choice: 7,
                estimated: 1
            }
        );
        let mut rules = plan(chain(0, 0, 0.05));
        rules.uncertain_alternatives = true;
        assert_eq!(measure(0, &rectangle(0.02, 0.02), &rules).estimated, 1);
    }

    #[test]
    fn incompatible_matches_and_missing_bound_ticks_stay_unknown() {
        let mut rules = plan(chain(0, 0, 0.05));
        rules.chains.push(chain(1, 0, 0.05));
        assert_eq!(measure(0, &rectangle(0.02, 0.03), &rules), UNKNOWN);
        let mut con = chain(0, 0, 0.05);
        con.bound_ticks = None;
        assert_eq!(measure(0, &rectangle(0.02, 0.03), &plan(con)), UNKNOWN);
    }

    #[test]
    fn half_ticks_and_roundoff_guard_request_python_only_at_boundaries() {
        assert_eq!(
            measure(1, &[(0.0, 0.0), (0.000005, 0.0)], &plan(chain(6, 0, 1.0))),
            FALLBACK
        );
        let just_under = f64::from_bits(0.02_f64.to_bits() - 1);
        assert_eq!(
            measure(
                0,
                &rectangle(just_under, just_under),
                &plan(chain(0, 0, 0.02))
            ),
            FALLBACK
        );
        assert_eq!(
            measure(1, &[(0.0, 0.0), (0.001001, 0.0)], &plan(chain(6, 0, 1.0))).ticks,
            100
        );
    }

    #[test]
    fn hypot_guard_includes_roundoff_boundary_safety_band() {
        // Five ulps is just beyond the selector's four-ulp guard; another
        // hypot implementation one ulp lower could therefore change whether
        // the result is marked estimated. Resolve that provenance in Python.
        let bound = f64::from_bits(5.0_f64.to_bits() - 5);
        let con = plan(chain(6, 2, bound));
        assert_eq!(measure(1, &[(0.0, 0.0), (3.0, 4.0)], &con), FALLBACK);
        // Axis-aligned hypot is exact and keeps its fast path.
        assert_eq!(measure(1, &[(0.0, 0.0), (5.0, 0.0)], &con).choice, 0);
    }

    #[test]
    fn malformed_or_unsupported_geometry_does_not_panic() {
        let con = plan(chain(0, 0, 1.0));
        for kind in [0, 1, 2, 255] {
            assert_eq!(measure(kind, &[], &con), UNKNOWN);
        }
        assert_eq!(
            measure(0, &[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)], &con),
            UNKNOWN
        );
        assert_eq!(measure(0, &[(f64::NAN, 0.0); 4], &con), UNKNOWN);
    }

    #[test]
    fn compensated_sum_keeps_small_residuals_and_half_even_fix() {
        for (terms, expected) in [
            (vec![1.0e16, 1.0, -1.0e16], 1.0),
            (vec![1.0, 1.0e-16, 1.0e-16], 1.0000000000000002),
            (vec![0.0, -0.0, 0.0], 0.0),
        ] {
            let mut sum = PreciseSum::new();
            for value in terms {
                sum.add(value);
            }
            assert_eq!(sum.finish(), Some(expected));
        }
    }
}
