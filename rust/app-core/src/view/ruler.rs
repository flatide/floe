//! Manual ruler arithmetic. Browser input is a displayed-viewport fraction;
//! accepted points are annotations, not a new geometry/query authority.
use super::Viewport;
use crate::{Error, Result};
use floe_worker_client::SnapKind;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Coordinate {
    Integer(i64),
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
            Self::Fraction(n) => decimal(n),
        }
    }
    fn value(self) -> f64 {
        match self {
            Self::Integer(n) => n as f64,
            Self::Fraction(n) => n,
        }
    }
    fn minus(self, other: Self) -> f64 {
        match (self, other) {
            // Two snapped coordinates may differ by one DBU above 2^53. Do
            // not round their absolute values before subtracting them.
            (Self::Integer(a), Self::Integer(b)) => (i128::from(a) - i128::from(b)) as f64,
            _ => self.value() - other.value(),
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

#[cfg(test)]
mod tests {
    use super::*;
    fn p(x: i64, y: i64) -> RulerPoint {
        RulerPoint::snapped(x, y)
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
