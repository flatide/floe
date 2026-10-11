//! Ruler arithmetic. Browser input is a displayed-viewport fraction;
//! accepted points are annotations, not a new geometry/query authority.
use super::Viewport;
use crate::{Error, Result};
use floe_worker_client::SnapKind;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Coordinate {
    Integer(i64),
    /// Exact bbox midpoints, including quarter DBU above f64's integer range.
    Quarter(i128),
    Fraction(f64),
}
impl Coordinate {
    fn number(n: f64) -> Result<Self> {
        if !n.is_finite() || n.abs() > (1u64 << 62) as f64 {
            return Err(Error::input("ruler coordinate overflow"));
        }
        Ok(if n.fract() == 0. {
            Self::Integer(n as i64)
        } else {
            Self::Fraction(n)
        })
    }
    fn parse(s: &str) -> Result<Self> {
        if s.len() > 64 || s.is_empty() {
            return Err(Error::input("invalid ruler coordinate"));
        }
        if let Ok(n) = s.parse::<i64>() {
            if n.to_string() == s {
                return Ok(Self::Integer(n));
            }
        }
        if let Some((whole, fraction)) = s.split_once('.') {
            let fraction = match fraction {
                "25" => Some(1),
                "5" => Some(2),
                "75" => Some(3),
                _ => None,
            };
            if let (Ok(n), Some(f)) = (whole.parse::<i64>(), fraction) {
                let q = i128::from(n) * 4 + if s.starts_with('-') { -f } else { f };
                if (i128::from(i64::MIN) * 4..=i128::from(i64::MAX) * 4).contains(&q) {
                    let v = Self::quarter(q);
                    if v.text() == s {
                        return Ok(v);
                    }
                }
            }
        }
        let n = s
            .parse::<f64>()
            .map_err(|_| Error::input("invalid ruler coordinate"))?;
        if n.fract() == 0. {
            return Err(Error::input("noncanonical ruler coordinate"));
        }
        let value = Self::number(n)?;
        if value.text() != s {
            return Err(Error::input("noncanonical ruler coordinate"));
        }
        Ok(value)
    }
    fn text(self) -> String {
        match self {
            Self::Integer(n) => n.to_string(),
            Self::Quarter(n) => format!(
                "{}{}.{}",
                if n < 0 { "-" } else { "" },
                n.abs() / 4,
                ["", "25", "5", "75"][(n.abs() % 4) as usize]
            ),
            Self::Fraction(n) => decimal(n),
        }
    }
    fn value(self) -> f64 {
        match self {
            Self::Integer(n) => n as f64,
            Self::Quarter(n) => n as f64 / 4.,
            Self::Fraction(n) => n,
        }
    }
    fn minus(self, other: Self) -> f64 {
        match (self.quarters(), other.quarters()) {
            // Two snapped coordinates may differ by one DBU above 2^53. Do
            // not round their absolute values before subtracting them.
            (Some(a), Some(b)) => (a - b) as f64 / 4.,
            _ => self.value() - other.value(),
        }
    }
    fn quarters(self) -> Option<i128> {
        match self {
            Self::Integer(n) => Some(i128::from(n) * 4),
            Self::Quarter(n) => Some(n),
            Self::Fraction(_) => None,
        }
    }
    fn quarter(n: i128) -> Self {
        if n % 4 == 0 {
            Self::Integer((n / 4) as i64)
        } else {
            Self::Quarter(n)
        }
    }
}
fn decimal(n: f64) -> String {
    let s = n.to_string();
    if s.len() > 64 {
        format!("{n:e}")
    } else {
        s
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RulerPoint([Coordinate; 2]);
impl RulerPoint {
    pub fn parse(point: [&str; 2]) -> Result<Self> {
        Ok(Self([
            Coordinate::parse(point[0])?,
            Coordinate::parse(point[1])?,
        ]))
    }
    pub fn strings(self) -> [String; 2] {
        self.0.map(Coordinate::text)
    }
    pub(super) fn snapped(x: i64, y: i64) -> Self {
        Self([Coordinate::Integer(x), Coordinate::Integer(y)])
    }
    pub(super) fn cursor(v: Viewport, position: [f64; 2]) -> Result<Self> {
        if !position
            .iter()
            .all(|n| n.is_finite() && (0.0..=1.0).contains(n))
        {
            return Err(Error::input("ruler requires viewport fractions"));
        }
        v.validate()?;
        let [x0, y0, x1, y1] = v.bbox;
        Ok(Self([
            Coordinate::number(x0 + position[0] * (x1 - x0))?,
            Coordinate::number(y1 - position[1] * (y1 - y0))?,
        ]))
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct RulerSegment {
    pub endpoints: [RulerPoint; 2],
    pub delta_um: [f64; 2],
    pub distance_um: f64,
}
impl RulerSegment {
    pub fn delta_strings(&self) -> [String; 2] {
        self.delta_um.map(decimal)
    }
    pub fn distance_string(&self) -> String {
        decimal(self.distance_um)
    }
}
#[derive(Clone, Debug)]
pub struct RulerMeasurement {
    /// Unconstrained cursor/snap point, also used for the crosshair.
    pub point: RulerPoint,
    pub snap: Option<SnapKind>,
    pub segment: Option<RulerSegment>,
}
pub(super) fn measure(
    start: Option<RulerPoint>,
    point: RulerPoint,
    free_angle: bool,
    dbu: f64,
    snap: Option<SnapKind>,
) -> Result<RulerMeasurement> {
    if !dbu.is_finite() || dbu <= 0. {
        return Err(Error::input("invalid ruler DBU"));
    }
    let segment = start
        .map(|start| {
            let mut end = point;
            let [dx, dy] = [point.0[0].minus(start.0[0]), point.0[1].minus(start.0[1])];
            if !free_angle {
                let axis = usize::from(dx.abs() >= dy.abs()); // tie is horizontal, as in GTK
                end.0[axis] = start.0[axis];
            }
            let delta_um = [
                end.0[0].minus(start.0[0]) * dbu,
                end.0[1].minus(start.0[1]) * dbu,
            ];
            let distance_um = delta_um[0].hypot(delta_um[1]);
            if !delta_um.iter().all(|n| n.is_finite()) || !distance_um.is_finite() {
                return Err(Error::input("ruler distance overflow"));
            }
            Ok(RulerSegment {
                endpoints: [start, end],
                delta_um,
                distance_um,
            })
        })
        .transpose()?;
    Ok(RulerMeasurement {
        point,
        snap,
        segment,
    })
}

/// GTK `_measure_selection`: nearest bbox neighbour for each selected shape,
/// stable selection-order ties, unique sorted pairs, horizontal then vertical.
/// These are captured annotations, not polygon contour distances. At most 64
/// boxes, 4032 comparisons and 128 segments; no geometry traversal or I/O.
pub(super) fn measure_selection(boxes: &[[i64; 4]], dbu: f64) -> Result<Vec<RulerSegment>> {
    if boxes.len() > 64 || boxes.iter().any(|b| b[0] > b[2] || b[1] > b[3]) {
        return Err(Error::input("invalid ruler selection bounds"));
    }
    if !dbu.is_finite() || dbu <= 0. {
        return Err(Error::input("invalid ruler DBU"));
    }
    let separation = |a: &[i64; 4], b: &[i64; 4], axis: usize| {
        (i128::from(a[axis]) - i128::from(b[axis + 2]))
            .max(i128::from(b[axis]) - i128::from(a[axis + 2]))
            .max(0) as f64
    };
    let mut pairs = std::collections::BTreeSet::new();
    for (i, a) in boxes.iter().enumerate() {
        let mut nearest = None;
        let mut distance = f64::INFINITY;
        for (j, b) in boxes.iter().enumerate().filter(|(j, _)| *j != i) {
            let d = separation(a, b, 0).hypot(separation(a, b, 1));
            if d < distance {
                nearest = Some(j);
                distance = d;
            }
        }
        if let Some(j) = nearest {
            pairs.insert((i.min(j), i.max(j)));
        }
    }
    let mut out = Vec::with_capacity(pairs.len() * 2);
    for (i, j) in pairs {
        let (a, b) = (boxes[i], boxes[j]);
        for axis in 0..2 {
            let (e0, e1) = if b[axis] > a[axis + 2] {
                (a[axis + 2], b[axis])
            } else if a[axis] > b[axis + 2] {
                (b[axis + 2], a[axis])
            } else {
                continue;
            };
            let cross = 1 - axis;
            let (lo, hi) = (a[cross].max(b[cross]), a[cross + 2].min(b[cross + 2]));
            let q = if lo <= hi {
                (i128::from(lo) + i128::from(hi)) * 2
            } else {
                [a[cross], a[cross + 2], b[cross], b[cross + 2]]
                    .into_iter()
                    .map(i128::from)
                    .sum()
            };
            let mut first = RulerPoint([Coordinate::quarter(q); 2]);
            let mut second = first;
            first.0[axis] = Coordinate::Integer(e0);
            second.0[axis] = Coordinate::Integer(e1);
            out.push(
                measure(Some(first), second, true, dbu, None)?
                    .segment
                    .unwrap(),
            );
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(x: i64, y: i64) -> RulerPoint {
        RulerPoint::snapped(x, y)
    }
    #[test]
    fn selection_gaps_are_nearest_unique_stable_and_not_union_spans() {
        let boxes = [[0, 0, 2, 2], [4, 0, 6, 2], [-4, 0, -2, 2], [4, 4, 6, 6]];
        let r = measure_selection(&boxes, 0.001).unwrap();
        let ends: Vec<_> = r
            .iter()
            .map(|s| s.endpoints.map(RulerPoint::strings))
            .collect();
        assert_eq!(
            ends,
            vec![
                [["2", "1"], ["4", "1"]],
                [["-2", "1"], ["0", "1"]],
                [["5", "2"], ["5", "4"]]
            ]
        );
        assert!(r.iter().all(|s| s.distance_um == 0.002));
        assert!(measure_selection(&[], 1.).unwrap().is_empty());
        assert!(measure_selection(&boxes[..1], 1.).unwrap().is_empty());
        for b in [[0, 0, 2, 2], [1, 1, 3, 3], [2, 0, 4, 2]] {
            assert!(measure_selection(&[boxes[0], b], 1.).unwrap().is_empty());
        }
        let diagonal = measure_selection(&[[0, 0, 1, 1], [3, 4, 5, 6]], 1.).unwrap();
        assert_eq!(
            diagonal[0].endpoints.map(RulerPoint::strings),
            [["1", "2.75"], ["3", "2.75"]]
        );
        assert_eq!(
            diagonal[1].endpoints.map(RulerPoint::strings),
            [["2.25", "1"], ["2.25", "4"]]
        );
        let ties = measure_selection(&[[0, 0, 1, 1], [2, 0, 3, 1], [2, 0, 3, 1]], 1.).unwrap();
        assert_eq!(ties.len(), 1); // equal-distance tie chooses the first neighbour
    }
    #[test]
    fn selection_extreme_midpoints_and_small_gaps_are_exact() {
        let (lo, hi) = (i64::MIN, i64::MAX);
        for y in [lo, hi - 1] {
            let r = measure_selection(&[[hi - 3, y, hi - 2, y + 1], [hi - 1, y, hi, y + 1]], 1.)
                .unwrap();
            assert_eq!(r[0].distance_um, 1.);
            assert_eq!(
                r[0].endpoints[0].strings()[1],
                if y == lo {
                    "-9223372036854775807.5"
                } else {
                    "9223372036854775806.5"
                }
            );
            for p in r[0].endpoints {
                let s = p.strings();
                assert_eq!(RulerPoint::parse([&s[0], &s[1]]).unwrap(), p);
            }
        }
        let r = measure_selection(&[[lo, lo, lo, hi], [hi, lo, hi, hi]], 1.).unwrap();
        assert_eq!(r[0].endpoints[0].strings()[1], "-0.5");
        assert_eq!(r[0].distance_um, u64::MAX as f64);
        assert!(measure_selection(&[[1, 0, 0, 1]], 1.).is_err());
        assert!(measure_selection(&[[0; 4]; 65], 1.).is_err());
        assert!(measure_selection(&[[lo, 0, lo, 0], [hi, 0, hi, 0]], f64::MAX).is_err());
        assert!(measure_selection(&[], f64::NAN).is_err());
        for s in [
            "9223372036854775807.25",
            "-9223372036854775808.5",
            "00.25",
            "-00.25",
            "+0.5",
        ] {
            assert!(RulerPoint::parse([s, "0"]).is_err(), "{s}");
        }
    }
    #[test]
    fn coordinate_contract_preserves_integer_snaps_and_fractional_cursor() {
        for s in [
            "0",
            "-1",
            "0.25",
            "-10.5",
            "9223372036854775807",
            "-9223372036854775808",
            "1e-100",
        ] {
            assert_eq!(RulerPoint::parse([s, s]).unwrap().strings(), [s, s]);
        }
        for s in [
            "",
            " 1",
            "+1",
            "01",
            "-0",
            "1.0",
            "NaN",
            "inf",
            "1e400",
            "9223372036854775808",
            "한글",
        ] {
            assert!(RulerPoint::parse([s, "0"]).is_err(), "{s}");
        }
        let v = Viewport::new([-10.75, -5.5, 9.25, 10.5], 800, 640).unwrap();
        assert_eq!(
            RulerPoint::cursor(v, [0.5, 0.5]).unwrap().strings(),
            ["-0.75", "2.5"]
        );
        for pos in [[-0.1, 0.], [0., 1.1], [f64::NAN, 0.]] {
            assert!(RulerPoint::cursor(v, pos).is_err());
        }
    }
    #[test]
    fn dominant_axis_shift_free_zero_and_extreme_deltas() {
        let start = p(1, 2);
        let horizontal = measure(Some(start), p(4, 5), false, 0.001, None)
            .unwrap()
            .segment
            .unwrap();
        assert_eq!(horizontal.endpoints, [start, p(4, 2)]);
        assert_eq!(horizontal.delta_um, [0.003, 0.]);
        let vertical = measure(Some(start), p(-2, -2), false, 0.001, None)
            .unwrap()
            .segment
            .unwrap();
        assert_eq!(vertical.endpoints, [start, p(1, -2)]);
        let free = measure(Some(start), p(4, 6), true, 0.001, None)
            .unwrap()
            .segment
            .unwrap();
        assert_eq!(free.distance_um, 0.005);
        let huge = measure(Some(p(0, 0)), p(1, 0), false, 1e100, None)
            .unwrap()
            .segment
            .unwrap();
        assert_eq!(huge.distance_string(), "1e100");
        assert!(huge.delta_strings().iter().all(|s| s.len() <= 64));
        assert_eq!(
            measure(Some(start), start, false, 1., None)
                .unwrap()
                .segment
                .unwrap()
                .distance_um,
            0.
        );
        let exact = measure(
            Some(p(i64::MAX - 1, i64::MIN)),
            p(i64::MAX, i64::MIN + 1),
            false,
            1.,
            None,
        )
        .unwrap()
        .segment
        .unwrap();
        assert_eq!(exact.delta_um, [1., 0.]);
        let span = measure(Some(p(i64::MIN, 0)), p(i64::MAX, 0), true, 1., None)
            .unwrap()
            .segment
            .unwrap();
        assert_eq!(span.distance_um, u64::MAX as f64);
        assert!(measure(Some(p(i64::MIN, 0)), p(i64::MAX, 0), true, f64::MAX, None).is_err());
    }
}
