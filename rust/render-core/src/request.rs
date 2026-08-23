use floe_ovm::BBox;

/// Full hierarchy depth sentinel used by `floe-vfs`.
pub const FULL_DEPTH: u32 = u32::MAX;

/// Closed viewport in layout database units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewBox {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl ViewBox {
    pub fn new(x0: i64, y0: i64, x1: i64, y1: i64) -> Result<Self, String> {
        let view = Self { x0, y0, x1, y1 };
        view.validate()?;
        Ok(view)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.x0 > self.x1 || self.y0 > self.y1 {
            return Err(format!(
                "invalid view: expected x0<=x1 and y0<=y1, got {},{},{},{}",
                self.x0, self.y0, self.x1, self.y1
            ));
        }
        Ok(())
    }

    pub fn as_bbox(self) -> BBox {
        BBox {
            x0: self.x0,
            y0: self.y0,
            x1: self.x1,
            y1: self.y1,
        }
    }
}

/// Renderer-facing hierarchy-plan request.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanRequest {
    pub view: ViewBox,
    pub cut_dbu: i64,
    pub visible_layers: Option<Vec<String>>,
    pub depth: u32,
    pub px_per_dbu: f64,
    /// Exact requests disable planner LOD/wash and all size culling.
    pub exact: bool,
}

impl PlanRequest {
    pub fn validate(&self) -> Result<(), String> {
        self.view.validate()?;
        if self.cut_dbu < 0 {
            return Err(format!("invalid cut_dbu: {}", self.cut_dbu));
        }
        if !self.px_per_dbu.is_finite() || self.px_per_dbu < 0.0 {
            return Err(format!("invalid px_per_dbu: {}", self.px_per_dbu));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_rejects_reversed_axes() {
        assert!(ViewBox::new(2, 0, 1, 1).is_err());
        assert!(ViewBox::new(0, 2, 1, 1).is_err());
    }

    #[test]
    fn request_rejects_non_finite_scale() {
        let req = PlanRequest {
            view: ViewBox::new(0, 0, 1, 1).unwrap(),
            cut_dbu: 0,
            visible_layers: None,
            depth: FULL_DEPTH,
            px_per_dbu: f64::NAN,
            exact: true,
        };
        assert!(req.validate().is_err());
    }
}
