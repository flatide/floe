use floe_ovm::BBox;

/// The raster's arithmetic in i64 and shifts where it gives what the i128
/// arithmetic gives, value for value (2026-10-04, the field chip's density
/// raster: 3.6 s for 33 pages; on the routing chip a third of the raster's
/// samples were i128 conversions and divisions in software - __fixdfti,
/// __divti3 - and the i128 transform): an orthogonal transform's row in i64
/// adds (the i128 sum where one overflows), a floor or ceil division by a
/// power of two as a shift, a device coordinate or a width converted through
/// i64 where it is within i64. FLOE_RUST_FAST_ARITH=off is the kill switch.
pub(crate) fn fast_arith() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FLOE_RUST_FAST_ARITH").as_deref() != Ok("off"))
}

/// Checked orthogonal transform: `p -> M*p + t`, where matrix entries are
/// -1, 0, or 1. Overflow is a render error rather than wrapped geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OrthoTransform {
    matrix: [[i8; 2]; 2],
    translation: (i64, i64),
}

impl OrthoTransform {
    pub fn identity() -> Self {
        Self {
            matrix: [[1, 0], [0, 1]],
            translation: (0, 0),
        }
    }

    pub fn place(x: i64, y: i64, rot: u8, flip: bool) -> Result<Self, String> {
        if rot > 3 {
            return Err(format!("invalid orthogonal rotation: {}", rot));
        }
        let f = if flip { -1 } else { 1 };
        let (c, s) = match rot {
            0 => (1, 0),
            1 => (0, 1),
            2 => (-1, 0),
            3 => (0, -1),
            _ => unreachable!(),
        };
        Ok(Self {
            matrix: [[c, -s * f], [s, c * f]],
            translation: (x, y),
        })
    }

    /// Returns `self(inner(point))`.
    pub fn compose(&self, inner: &Self) -> Result<Self, String> {
        let a = self.matrix;
        let b = inner.matrix;
        let matrix = [
            [
                a[0][0] * b[0][0] + a[0][1] * b[1][0],
                a[0][0] * b[0][1] + a[0][1] * b[1][1],
            ],
            [
                a[1][0] * b[0][0] + a[1][1] * b[1][0],
                a[1][0] * b[0][1] + a[1][1] * b[1][1],
            ],
        ];
        Ok(Self {
            matrix,
            translation: self.apply(inner.translation.0, inner.translation.1)?,
        })
    }

    pub fn apply(&self, x: i64, y: i64) -> Result<(i64, i64), String> {
        // a row of an orthogonal matrix in i64 adds (fast_arith); the i128
        // sum where one overflows
        if fast_arith() {
            if let (Some(tx), Some(ty)) = (self.row(0, x, y), self.row(1, x, y)) {
                return Ok((tx, ty));
            }
        }
        let tx = self.matrix[0][0] as i128 * x as i128
            + self.matrix[0][1] as i128 * y as i128
            + self.translation.0 as i128;
        let ty = self.matrix[1][0] as i128 * x as i128
            + self.matrix[1][1] as i128 * y as i128
            + self.translation.1 as i128;
        Ok((
            checked_i64(tx, "transform x")?,
            checked_i64(ty, "transform y")?,
        ))
    }

    /// Row `r` of `apply` in i64: None where a term or a sum overflows.
    #[inline]
    fn row(&self, r: usize, x: i64, y: i64) -> Option<i64> {
        let term = |m: i8, v: i64| match m {
            0 => Some(0),
            1 => Some(v),
            -1 => v.checked_neg(),
            _ => None,
        };
        let t = if r == 0 { self.translation.0 } else { self.translation.1 };
        term(self.matrix[r][0], x)?.checked_add(term(self.matrix[r][1], y)?)?.checked_add(t)
    }

    pub fn invert(&self) -> Result<Self, String> {
        let matrix = [
            [self.matrix[0][0], self.matrix[1][0]],
            [self.matrix[0][1], self.matrix[1][1]],
        ];
        let tx = -(matrix[0][0] as i128 * self.translation.0 as i128
            + matrix[0][1] as i128 * self.translation.1 as i128);
        let ty = -(matrix[1][0] as i128 * self.translation.0 as i128
            + matrix[1][1] as i128 * self.translation.1 as i128);
        Ok(Self {
            matrix,
            translation: (checked_i64(tx, "inverse x")?, checked_i64(ty, "inverse y")?),
        })
    }

    pub fn apply_bbox(&self, bbox: BBox) -> Result<BBox, String> {
        if bbox.is_empty() {
            return Ok(BBox::EMPTY);
        }
        let mut result = BBox::EMPTY;
        for (x, y) in [
            (bbox.x0, bbox.y0),
            (bbox.x0, bbox.y1),
            (bbox.x1, bbox.y0),
            (bbox.x1, bbox.y1),
        ] {
            let (x, y) = self.apply(x, y)?;
            result.grow(&BBox {
                x0: x,
                y0: y,
                x1: x,
                y1: y,
            });
        }
        Ok(result)
    }
}

fn checked_i64(value: i128, field: &str) -> Result<i64, String> {
    value
        .try_into()
        .map_err(|_| format!("coordinate overflow: {} = {}", field, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_rotation_and_flip_in_documented_order() {
        let rotated = OrthoTransform::place(10, 20, 1, false).unwrap();
        assert_eq!(rotated.apply(2, 3).unwrap(), (7, 22));
        let flipped = OrthoTransform::place(10, 20, 0, true).unwrap();
        assert_eq!(flipped.apply(2, 3).unwrap(), (12, 17));
    }

    #[test]
    fn inverse_round_trips_all_orientations() {
        for rot in 0..4 {
            for flip in [false, true] {
                let transform = OrthoTransform::place(-17, 23, rot, flip).unwrap();
                let inverse = transform.invert().unwrap();
                let point = transform.apply(91, -37).unwrap();
                assert_eq!(inverse.apply(point.0, point.1).unwrap(), (91, -37));
            }
        }
    }

    /// fast_arith: a row in i64 adds is the i128 sum, value for value - and an
    /// error where that sum leaves i64 - for every orientation, at the ends
    /// of i64 as well.
    #[test]
    fn the_i64_rows_are_the_i128_sums() {
        let reference = |t: &OrthoTransform, x: i64, y: i64| -> Option<(i64, i64)> {
            let tx = t.matrix[0][0] as i128 * x as i128 + t.matrix[0][1] as i128 * y as i128 + t.translation.0 as i128;
            let ty = t.matrix[1][0] as i128 * x as i128 + t.matrix[1][1] as i128 * y as i128 + t.translation.1 as i128;
            Some((i64::try_from(tx).ok()?, i64::try_from(ty).ok()?))
        };
        let ends = [i64::MIN, i64::MIN + 1, -(1 << 40), -7, -1, 0, 1, 7, 1 << 40, i64::MAX - 1, i64::MAX];
        let mut rng = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng as i64) >> (rng % 40)
        };
        let mut values: Vec<i64> = ends.to_vec();
        values.extend((0..40).map(|_| next()));
        for rot in 0..4 {
            for flip in [false, true] {
                for &tx in &values[..12] {
                    for &ty in &[0, -3, 1 << 50, i64::MIN, i64::MAX] {
                        let t = OrthoTransform::place(tx, ty, rot, flip).unwrap();
                        for &x in &values {
                            for &y in &values {
                                assert_eq!(t.apply(x, y).ok(), reference(&t, x, y), "rot {rot} flip {flip} t ({tx}, {ty}) at ({x}, {y})");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn rejects_coordinate_overflow() {
        let transform = OrthoTransform::place(i64::MAX, 0, 0, false).unwrap();
        assert!(transform.apply(1, 0).is_err());
    }
}
