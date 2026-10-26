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
    pub fn effective(self, _deck: bool) -> floe_worker_client::ThinPolicy {
        use floe_worker_client::ThinPolicy as P;
        match self {
            // Upstream 2026-09-23: retain thin pages for layouts as well.
            Self::Auto | Self::Keep => P::Keep,
            Self::Cull => P::Cull,
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
    /// The density under the cut (`--density on|off`, user 2026-10-12: "a
    /// --density option for --detail high" - the viewer's --density / View >
    /// density under the cut): the shapes the detail's cut drops, drawn by
    /// the area they cover (renderd's density stack). None: as the viewer
    /// starts - on wherever a cut drops shapes (user 2026-10-12: "turn the
    /// density on by default"), off for the export look (density_on).
    pub density: Option<bool>,
    /// The occupancy summary (`--occupancy on|off`, the viewer's View >
    /// occupancy summary, 2026-10-20): on draws a layout's wide views from
    /// its design.ovo (a jobdeck's source opened on its own has one), off
    /// none for a jobdeck's passes either. None: renderd's rule - a jobdeck
    /// on, a plain layout off (user decision 2026-09-24).
    pub occupancy: Option<bool>,
    /// Solid fills, no layer-property patterns - the export look
    /// (Dataset::styles(archival)); false: the viewer's fills.
    pub archival: bool,
}
/// `--layerid` / a batch line's `layerid=` (user 2026-10-26: "--layerid 1"):
/// layers by the layer panel's number - `N` (datatype 0) or `N.D` (`N/D`
/// too), comma-separated; a jobdeck's level head is `N` (level N, every chip
/// of it, as its panel row). The pair spec the layer resolvers take
/// (`1/0,3/2`), None for an empty value.
pub fn layer_ids(text: &str) -> Result<Option<String>> {
    let mut out = Vec::new();
    for token in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let (l, d) = token.split_once(&['.', '/'][..]).unwrap_or((token, "0"));
        let number = |s: &str| s.trim().parse::<u32>().ok();
        let (Some(l), Some(d)) = (number(l), number(d)) else {
            return Err(Error::input(format!(
                "invalid layer id {token:?}: N or N.D, the layer panel's number \
                 (a jobdeck's level N is N)"
            )));
        };
        out.push(format!("{l}/{d}"));
    }
    Ok((!out.is_empty()).then(|| out.join(",")))
}
/// What a shot draws, said by name (`--layers` / `--layer`; all or empty:
/// none named) and by id (`--layerid`, layer_ids): each the last given, the
/// two in the order they were given - a legend lists them so; nothing named
/// is every layer.
#[derive(Default)]
pub struct LayerChoice {
    names: Option<String>,
    ids: Option<String>,
    ids_first: bool,
    said: bool,
}
impl LayerChoice {
    pub fn names(&mut self, value: &str) {
        self.said = true;
        self.names = (!matches!(value, "" | "all")).then(|| value.to_string());
        self.ids_first = true;
    }
    pub fn ids(&mut self, value: &str) -> Result<()> {
        self.said = true;
        self.ids = layer_ids(value)?;
        self.ids_first = false;
        Ok(())
    }
    /// Whether either was given at all (the shot's own say).
    pub fn said(&self) -> bool {
        self.said
    }
    /// The spec the layer resolvers take, None for every layer.
    pub fn spec(self) -> Option<String> {
        let (a, b) = if self.ids_first {
            (self.ids, self.names)
        } else {
            (self.names, self.ids)
        };
        match (a, b) {
            (Some(a), Some(b)) => Some(format!("{a},{b}")),
            (a, b) => a.or(b),
        }
    }
}
/// The density's start where no one said (user 2026-10-12: "turn the density
/// on by default"; gui.py density_on, the same rule): on, but off for the
/// export look (`--archival`, as before 0.12.351). FLOE_RUST_DENSITY_STACK, the
/// environment the viewer shares with renderd: `off` is the kill switch, `top`
/// (how the field turned it on before) on for the export look too.
pub fn density_default(stack: Option<&str>, archival: bool) -> bool {
    match stack {
        Some("off") => false,
        Some("top") => true,
        _ => !archival,
    }
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
            density: None,
            occupancy: None,
            archival: true,
        }
    }
}
impl Shot {
    /// The command line's capture: what the viewer draws at the same view
    /// (user 2026-10-12: "the viewer's default mode has thin keep, the
    /// density and so on; a capture must come out the same") - floe2 view
    /// --goto's start: detail medium, full depth, thin keep (auto), the
    /// density as the viewer starts it (density_on), the viewer's fills and
    /// layer properties, cell frames and labels. Default stays the export
    /// look (`floe2 render --archival`, FLOE_RENDER_DEFAULTS=archival): cut
    /// 0, solid fills, no frames or labels.
    pub fn viewer() -> Self {
        Self {
            detail: Detail::Medium,
            frames: true,
            labels: true,
            archival: false,
            ..Self::default()
        }
    }
    /// The density under the cut this shot asks renderd for - always said,
    /// as the viewer does (gui.py density_on), so the sub-cut dots its
    /// brightness is made of come with it. Unsaid, the viewer's start
    /// (density_default) wherever the detail cuts: exact drops nothing, and
    /// a density there would only take pass 2's reserve from pass 1's budget.
    pub fn density_on(&self) -> bool {
        self.density.unwrap_or_else(|| {
            self.detail != Detail::Exact
                && density_default(
                    std::env::var("FLOE_RUST_DENSITY_STACK").ok().as_deref(),
                    self.archival,
                )
        })
    }
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
    fn layer_ids_are_the_layer_panels_numbers() {
        assert_eq!(layer_ids("1").unwrap().as_deref(), Some("1/0"));
        assert_eq!(
            layer_ids(" 1, 3.2 ,4/5").unwrap().as_deref(),
            Some("1/0,3/2,4/5")
        );
        assert_eq!(layer_ids("").unwrap(), None);
        assert_eq!(layer_ids(" , ").unwrap(), None);
        for bad in ["x", "1.x", "-1", "1.2.3", "1/", "METAL1", "$1"] {
            let e = layer_ids(bad).unwrap_err();
            assert!(e.to_string().contains("invalid layer id"), "{bad}: {e}");
        }
        // names and ids: each the last given, the two in the given order
        let spec = |steps: &[(&str, &str)]| {
            let mut c = LayerChoice::default();
            for &(kind, v) in steps {
                if kind == "id" {
                    c.ids(v).unwrap();
                } else {
                    c.names(v);
                }
            }
            (c.said(), c.spec())
        };
        let s = |v: &str| Some(v.to_string());
        assert_eq!(spec(&[]), (false, None));
        assert_eq!(
            spec(&[("name", "METAL1"), ("id", "2")]),
            (true, s("METAL1,2/0"))
        );
        assert_eq!(
            spec(&[("id", "2"), ("name", "ALIGN")]),
            (true, s("2/0,ALIGN"))
        );
        assert_eq!(
            spec(&[("name", "A"), ("id", "1"), ("name", "B")]),
            (true, s("1/0,B"))
        );
        assert_eq!(spec(&[("name", "A"), ("name", "B")]), (true, s("B")));
        assert_eq!(spec(&[("name", "all"), ("id", "2")]), (true, s("2/0")));
        assert_eq!(spec(&[("name", "all")]), (true, None));
        assert_eq!(spec(&[("id", "")]), (true, None));
    }
    #[test]
    fn auto_keeps_thin_pages_for_layouts_and_decks() {
        use floe_worker_client::ThinPolicy as P;
        for deck in [false, true] {
            assert_eq!(Thin::Auto.effective(deck), P::Keep);
            assert_eq!(Thin::Keep.effective(deck), P::Keep);
            assert_eq!(Thin::Cull.effective(deck), P::Cull);
        }
    }
    #[test]
    fn the_density_starts_on_where_a_cut_drops_shapes() {
        assert!(density_default(None, false));
        assert!(!density_default(None, true));
        assert!(!density_default(Some("off"), false));
        assert!(density_default(Some("top"), true));
        assert!(density_default(Some("anything"), false));
        // said, it is what was said; exact cuts nothing: no density unsaid
        let mut s = Shot::viewer();
        s.density = Some(false);
        assert!(!s.density_on());
        s.density = Some(true);
        assert!(s.density_on());
        s.density = None;
        s.detail = Detail::Exact;
        assert!(!s.density_on());
        assert!(!Shot::default().density_on());
    }
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
