use floe_ovm::BBox;

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

    #[test]
    fn rejects_coordinate_overflow() {
        let transform = OrthoTransform::place(i64::MAX, 0, 0, false).unwrap();
        assert!(transform.apply(1, 0).is_err());
    }
}
