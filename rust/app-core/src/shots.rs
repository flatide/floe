//! Capture coordinates are micrometres. Keep Python shots' operation order,
//! half-even pixel sizing and anchor semantics, including fractional DBU views.
use crate::{Error, Result};
pub mod batch;
pub mod mosaic;

pub const MAX_PIXELS: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Anchor {
    #[default]
    Center,
    LowerLeft,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Detail {
    #[default]
    Exact,
    Low,
    Medium,
    High,
}
impl Detail {
    pub fn cut_px(self) -> f64 {
        match self {
            Self::Exact => 0.,
            Self::Low => 5.,
            Self::Medium => 3.,
            Self::High => 1.,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Thin {
    #[default]
    Auto,
    Keep,
    Cull,
}
impl Thin {
    pub fn effective(self, deck: bool) -> floe_worker_client::ThinPolicy {
        use floe_worker_client::ThinPolicy as P;
        match self {
            Self::Keep => P::Keep,
            Self::Cull => P::Cull,
            Self::Auto if deck => P::Keep,
            _ => P::Cull,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Keep => "keep",
            Self::Cull => "cull",
        }
    }
}
pub fn length(text: &str) -> Result<f64> {
    let s = text.trim();
    let (number, factor) = [
        ("nm", 1e-3),
        ("um", 1.),
        ("µm", 1.),
        ("μm", 1.),
        ("mm", 1e3),
        ("cm", 1e4),
        ("m", 1e6),
    ]
    .into_iter()
    .find_map(|(suffix, f)| s.strip_suffix(suffix).map(|n| (n.trim(), f)))
    .unwrap_or((s, 1.));
    let n = number
        .parse::<f64>()
        .map_err(|_| Error::input(format!("invalid length: {text}")))?
        * factor;
    if !n.is_finite() {
        return Err(Error::input("length must be finite"));
    }
    Ok(n)
}
pub fn lengths<const N: usize>(text: &str) -> Result<[f64; N]> {
    text.split([',', ';'])
        .filter(|p| !p.trim().is_empty())
        .map(length)
        .collect::<Result<Vec<_>>>()?
        .try_into()
        .map_err(|_| Error::input(format!("expected {N} coordinates")))
}
pub fn pixels(text: &str) -> Result<(u32, Option<u32>)> {
    let s = text.trim().to_lowercase();
    let (w, h) = s
        .split_once('x')
        .map_or((s.as_str(), None), |(w, h)| (w, Some(h)));
    let parse = |s: &str| -> Result<u32> {
        s.trim()
            .parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::input("--px must be positive W or WxH"))
    };
    Ok((parse(w)?, h.map(parse).transpose()?))
}
pub fn normalize(b: [f64; 4]) -> Result<[f64; 4]> {
    if !b.iter().all(|v| v.is_finite()) || b[0] == b[2] || b[1] == b[3] {
        return Err(Error::input(
            "region must have finite coordinates and positive area",
        ));
    }
    let b = [
        b[0].min(b[2]),
        b[1].min(b[3]),
        b[0].max(b[2]),
        b[1].max(b[3]),
    ];
    if !(b[2] - b[0]).is_finite() || !(b[3] - b[1]).is_finite() {
        return Err(Error::input("region span overflow"));
    }
    Ok(b)
}
#[derive(Clone, Debug)]
pub struct Shot {
    pub bbox: Option<[f64; 4]>,
    pub at: Option<[f64; 2]>,
    pub size: Option<[f64; 2]>,
    pub anchor: Anchor,
    pub pixels: (u32, Option<u32>),
    pub stretch: bool,
    pub layers: Option<String>,
    pub depth: Option<u32>,
    pub detail: Detail,
    pub thin: Thin,
    pub frames: bool,
    pub labels: bool,
    pub font_px: u32,
}
impl Default for Shot {
    fn default() -> Self {
        Self {
            bbox: None,
            at: None,
            size: None,
            anchor: Anchor::Center,
            pixels: (1200, None),
            stretch: false,
            layers: None,
            depth: None,
            detail: Detail::Exact,
            thin: Thin::Auto,
            frames: false,
            labels: false,
            font_px: 14,
        }
    }
}
impl Shot {
    pub fn validate(&self) -> Result<()> {
        if self.bbox.is_some() && self.at.is_some() {
            return Err(Error::input("--bbox and --at are mutually exclusive"));
        }
        if self.at.is_some() && self.size.is_none() {
            return Err(Error::input("--at needs --size W,H"));
        }
        if self
            .size
            .is_some_and(|v| v.iter().any(|n| !n.is_finite() || *n <= 0.))
        {
            return Err(Error::input("--size must be positive and finite"));
        }
        if !(6..=96).contains(&self.font_px) {
            return Err(Error::input("--label-font-px must be 6..96"));
        }
        if self.pixels.0 == 0 || self.pixels.1 == Some(0) {
            return Err(Error::input("--px must be positive"));
        }
        Ok(())
    }
    pub fn fitted(&self, default_box: [f64; 4]) -> Result<([f64; 4], u32, u32)> {
        self.validate()?;
        let b = if let Some([x, y]) = self.at {
            let [w, h] = self.size.unwrap();
            match self.anchor {
                Anchor::LowerLeft => [x, y, x + w, y + h],
                Anchor::Center => [x - w / 2., y - h / 2., x + w / 2., y + h / 2.],
            }
        } else {
            self.bbox.unwrap_or(default_box)
        };
        let mut b = normalize(b)?;
        let (w, h) = self.pixels;
        let h = if let Some(h) = h {
            h
        } else {
            let n = (f64::from(w) * (b[3] - b[1]) / (b[2] - b[0]))
                .round_ties_even()
                .max(1.);
            if n > f64::from(u32::MAX) {
                return Err(Error::input("derived image height overflow"));
            }
            n as u32
        };
        if u64::from(w) * u64::from(h) > MAX_PIXELS {
            return Err(Error::input("frame exceeds 16 Mpx limit"));
        }
        if self.pixels.1.is_some() && !self.stretch {
            let want = f64::from(w) / f64::from(h);
            let have = (b[2] - b[0]) / (b[3] - b[1]);
            if (want - have).abs() >= 1e-12 {
                let (bw, bh) = if have < want {
                    ((b[3] - b[1]) * want, b[3] - b[1])
                } else {
                    (b[2] - b[0], (b[2] - b[0]) / want)
                };
                b = match self.anchor {
                    Anchor::LowerLeft => [b[0], b[1], b[0] + bw, b[1] + bh],
                    Anchor::Center => {
                        let (x, y) = ((b[0] + b[2]) / 2., (b[1] + b[3]) / 2.);
                        [x - bw / 2., y - bh / 2., x + bw / 2., y + bh / 2.]
                    }
                };
            }
        }
        Ok((normalize(b)?, w, h))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn units_aspect_anchor_and_ties() {
        assert_eq!(
            lengths::<4>("1mm,2000nm;3µm,4μm").unwrap(),
            [1000., 2., 3., 4.]
        );
        let mut s = Shot {
            bbox: Some([0., 0., 10., 25.]),
            pixels: (1, None),
            ..Default::default()
        };
        assert_eq!(s.fitted([0.; 4]).unwrap(), ([0., 0., 10., 25.], 1, 2));
        s.pixels = (100, Some(100));
        assert_eq!(s.fitted([0.; 4]).unwrap().0, [-7.5, 0., 17.5, 25.]);
        s.anchor = Anchor::LowerLeft;
        assert_eq!(s.fitted([0.; 4]).unwrap().0, [0., 0., 25., 25.]);
        s.stretch = true;
        assert_eq!(s.fitted([0.; 4]).unwrap().0, [0., 0., 10., 25.]);
    }
    #[test]
    fn reject_invalid_or_unbounded_inputs() {
        for text in ["nan", "inf", "1e309mm"] {
            assert!(length(text).is_err());
        }
        let s = Shot {
            pixels: (65536, Some(65536)),
            ..Default::default()
        };
        assert!(s.fitted([0., 0., 1., 1.]).is_err());
        assert!(normalize([0., 0., 0., 1.]).is_err());
        assert!(normalize([-1e308, 0., 1e308, 1.]).is_err());
    }

    #[test]
    fn json_coordinates_keep_full_f64_precision() {
        for text in [
            "0.00019999999999999998",
            "-10.937500000000002",
            "1.2345678901234567e-9",
            "9007199254740991.0",
        ] {
            let json: f64 = serde_json::from_str(text).unwrap();
            assert_eq!(
                json.to_bits(),
                text.parse::<f64>().unwrap().to_bits(),
                "{text}"
            );
        }
    }
}
