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
/// `--px`: `W` (the height from the region's aspect), `WxH`, or `max` (the
/// largest size of the region's aspect within MAX_PIXELS).
pub fn pixels(text: &str) -> Result<Pixels> {
    let s = text.trim().to_lowercase();
    if s == "max" {
        return Ok(Pixels::Largest);
    }
    let (w, h) = s
        .split_once('x')
        .map_or((s.as_str(), None), |(w, h)| (w, Some(h)));
    let parse = |s: &str| -> Result<u32> {
        s.trim()
            .parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::input("--px must be positive W, WxH or max"))
    };
    Ok(match (parse(w)?, h.map(parse).transpose()?) {
        (w, Some(h)) => Pixels::Size(w, h),
        (w, None) => Pixels::Width(w),
    })
}
/// The frame size `--px` names, or none. Unsaid, a capture's frame is the
/// largest size of its region's aspect within MAX_PIXELS (user 2026-11-05:
/// "지정하지 않을 경우 가능한 최대 해상도로"; 1200 wide before 0.12.433),
/// or what `FLOE_CAPTURE_PX` says (default_pixels). A DRC snapshot keeps its
/// own 1200 square (drc::capture).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pixels {
    #[default]
    Unsaid,
    Largest,
    Width(u32),
    Size(u32, u32),
}
impl Pixels {
    /// This size with an unsaid one resolved: `FLOE_CAPTURE_PX` as `--px`
    /// would read it, else the largest.
    pub fn resolved(self) -> Result<Pixels> {
        match self {
            Pixels::Unsaid => default_pixels(),
            said => Ok(said),
        }
    }
}
/// The frame size when `--px` is not given: `FLOE_CAPTURE_PX` (`W`, `WxH` or
/// `max`; a site that wants the 1200 of before 0.12.433 sets it so), else
/// the largest size of the region's aspect within MAX_PIXELS.
pub fn default_pixels() -> Result<Pixels> {
    default_pixels_from(std::env::var("FLOE_CAPTURE_PX").ok().as_deref())
}
fn default_pixels_from(value: Option<&str>) -> Result<Pixels> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(Pixels::Largest),
        Some(v) => pixels(v).map_err(|_| {
            Error::input(format!(
                "FLOE_CAPTURE_PX={v}: the default frame size is positive W, WxH or max"
            ))
        }),
    }
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
    pub pixels: Pixels,
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
            pixels: Pixels::Unsaid,
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
        if matches!(self.pixels, Pixels::Width(0) | Pixels::Size(0, _) | Pixels::Size(_, 0)) {
            return Err(Error::input("--px must be positive"));
        }
        Ok(())
    }
    /// The frame as the options ask, fitted to the limit: the policy for a
    /// frame over it from the environment (over_limit).
    pub fn fitted(&self, default_box: [f64; 4]) -> Result<([f64; 4], u32, u32)> {
        self.frame(default_box, over_limit())
            .map(|f| (f.bbox, f.width, f.height))
    }
    /// The region drawn and its size in px: `--px W` derives the height from
    /// the region's aspect, `--px WxH` widens the region to W:H unless
    /// `--stretch`. A frame over MAX_PIXELS is drawn at the largest size of
    /// its aspect within the limit, `asked` keeping the size the options
    /// derived (user 2026-11-04: "16MPixel이 넘어갈 경우 가능한 최대 크기로
    /// 줄여서 저장"), or refused naming that size and the region
    /// (OverLimit::Refuse - FLOE_CAPTURE_REDUCE=off; before 0.12.431 every
    /// frame over the limit was, with the bare "frame exceeds 16 Mpx limit").
    pub fn frame(&self, default_box: [f64; 4], policy: OverLimit) -> Result<Fitted> {
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
        let said = self.pixels.resolved()?;
        // the size asked: `WxH`, or a width with its height from the region's
        // aspect (half-even, as the Python shots derived it); none when the
        // frame is the largest of the region's aspect (`--px max`, and `--px`
        // unsaid since 0.12.433)
        let size = match said {
            Pixels::Size(w, h) => Some((f64::from(w), f64::from(h))),
            Pixels::Width(w) => {
                let n = (f64::from(w) * (b[3] - b[1]) / (b[2] - b[0]))
                    .round_ties_even()
                    .max(1.);
                if !n.is_finite() {
                    return Err(Error::input("derived image height overflow"));
                }
                Some((f64::from(w), n))
            }
            Pixels::Largest | Pixels::Unsaid => None,
        };
        let (w, h, asked) = match size {
            None => {
                let (w, h) = largest(b[2] - b[0], b[3] - b[1]);
                (w, h, None)
            }
            Some((aw, ah)) if aw * ah > MAX_PIXELS as f64 => {
                let (rw, rh) = reduced(aw, ah);
                (rw, rh, Some((aw as u64, ah as u64)))
            }
            Some((aw, ah)) => (aw as u32, ah as u32, None),
        };
        if let (Pixels::Size(aw, ah), false) = (said, self.stretch) {
            // the region to W:H as asked (the reduced size keeps that aspect
            // to a pixel)
            let want = f64::from(aw) / f64::from(ah);
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
        let fitted = Fitted {
            bbox: normalize(b)?,
            width: w,
            height: h,
            asked,
        };
        if policy == OverLimit::Refuse {
            if let Some(text) = fitted.over_limit_text() {
                return Err(Error::input(format!(
                    "frame {text}; --px {w}x{h} fits (FLOE_CAPTURE_REDUCE=off: refused, not reduced)"
                )));
            }
        }
        Ok(fitted)
    }
}
/// A shot's frame as fitted: the region drawn, its size in px and, when the
/// limit reduced it, the size the options derived.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fitted {
    pub bbox: [f64; 4],
    pub width: u32,
    pub height: u32,
    /// the size asked (derived from `--px` and the region), when it was over
    /// MAX_PIXELS and the frame is the largest of its aspect within the limit
    pub asked: Option<(u64, u64)>,
}
impl Fitted {
    /// The frame's scale in px per um along x and along y: the renderer maps
    /// each axis on its own (raster sx / sy), so the two agree within a pixel
    /// over the frame unless `--stretch` drew a region of another aspect.
    /// The x scale is the flateyes metadata's `ppu` (user 2026-11-05).
    pub fn ppu(&self) -> (f64, f64) {
        let [x0, y0, x1, y1] = self.bbox;
        (f64::from(self.width) / (x1 - x0), f64::from(self.height) / (y1 - y0))
    }
    /// What the limit reduced, for the capture's warning and the refusal:
    /// `1280x13440 px for 40.0000x420.0000 um (0.0000,0.0000,40.0000,420.0000 um)
    /// exceeds 16 Mpx`; None when the frame is as asked.
    pub fn over_limit_text(&self) -> Option<String> {
        let (aw, ah) = self.asked?;
        let [x0, y0, x1, y1] = self.bbox;
        Some(format!(
            "{aw}x{ah} px for {:.4}x{:.4} um ({x0:.4},{y0:.4},{x1:.4},{y1:.4} um) exceeds 16 Mpx",
            x1 - x0,
            y1 - y0
        ))
    }
}
/// What a frame over MAX_PIXELS becomes: the largest of its aspect within
/// the limit (the default since 0.12.431), or a refusal naming the size and
/// the region (`FLOE_CAPTURE_REDUCE=off`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverLimit {
    Reduce,
    Refuse,
}
pub fn over_limit() -> OverLimit {
    if std::env::var_os("FLOE_CAPTURE_REDUCE").is_some_and(|v| v == "off") {
        OverLimit::Refuse
    } else {
        OverLimit::Reduce
    }
}
/// The largest size of `width:height`'s aspect within MAX_PIXELS: the widest
/// width whose height, derived as `--px W` derives it (half-even), keeps the
/// product within the limit - the product grows with the width, so it is
/// found by bisection. An aspect too tall for a 1 px width gets the limit as
/// its height. The frame of `--px max`, and of `--px` unsaid (0.12.433).
pub fn largest(width: f64, height: f64) -> (u32, u32) {
    widest(width, height, MAX_PIXELS)
}
/// A size over MAX_PIXELS reduced: the largest of its aspect within the
/// limit, no wider than it was asked.
pub fn reduced(width: f64, height: f64) -> (u32, u32) {
    widest(width, height, (width as u64).min(MAX_PIXELS))
}
fn widest(width: f64, height: f64, hi: u64) -> (u32, u32) {
    let limit = MAX_PIXELS as f64;
    let derived = |rw: u64| ((rw as f64) * height / width).round_ties_even().max(1.);
    let fits = |rw: u64| rw as f64 * derived(rw) <= limit;
    let (mut lo, mut hi) = (1u64, hi.max(1));
    if !fits(lo) {
        return (1, MAX_PIXELS as u32);
    }
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    (lo as u32, derived(lo) as u32)
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
            pixels: Pixels::Width(1),
            ..Default::default()
        };
        assert_eq!(s.fitted([0.; 4]).unwrap(), ([0., 0., 10., 25.], 1, 2));
        s.pixels = Pixels::Size(100, 100);
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
            pixels: Pixels::Size(65536, 65536),
            ..Default::default()
        };
        assert!(s.frame([0., 0., 1., 1.], OverLimit::Refuse).is_err());
        let f = s.frame([0., 0., 1., 1.], OverLimit::Reduce).unwrap();
        assert_eq!((f.width, f.height, f.asked), (4096, 4096, Some((65536, 65536))));
        assert!(normalize([0., 0., 0., 1.]).is_err());
        assert!(normalize([-1e308, 0., 1e308, 1.]).is_err());
    }

    #[test]
    fn a_frame_over_the_limit_is_the_largest_of_its_aspect_or_refused_with_its_size_and_region() {
        // --px 1280 over a 40 x 420 um strip derives 1280x13440 = 17.2 Mpx
        let s = Shot {
            bbox: Some([0., 0., 40., 420.]),
            pixels: Pixels::Width(1280),
            ..Default::default()
        };
        let f = s.frame([0.; 4], OverLimit::Reduce).unwrap();
        assert_eq!(f.asked, Some((1280, 13440)));
        assert_eq!(f.bbox, [0., 0., 40., 420.]);
        assert!(u64::from(f.width) * u64::from(f.height) <= MAX_PIXELS);
        // the aspect to a pixel, and no wider width keeps the limit
        let derived = |w: u32| (f64::from(w) * 10.5).round_ties_even() as u64;
        assert_eq!(u64::from(f.height), derived(f.width));
        assert!(u64::from(f.width + 1) * derived(f.width + 1) > MAX_PIXELS);
        assert_eq!((f.width, f.height), (1264, 13272));
        let text = f.over_limit_text().unwrap();
        assert_eq!(
            text,
            "1280x13440 px for 40.0000x420.0000 um (0.0000,0.0000,40.0000,420.0000 um) exceeds 16 Mpx"
        );
        let e = s.frame([0.; 4], OverLimit::Refuse).unwrap_err().to_string();
        assert!(e.starts_with(&format!("frame {text}; --px 1264x13272 fits")), "{e}");
        assert!(e.contains("FLOE_CAPTURE_REDUCE=off"), "{e}");
        // a frame within the limit is as asked, nothing to say
        let f = Shot {
            bbox: Some([0., 0., 40., 420.]),
            pixels: Pixels::Width(1200),
            ..Default::default()
        }
        .frame([0.; 4], OverLimit::Refuse)
        .unwrap();
        assert_eq!((f.width, f.height, f.asked), (1200, 12600, None));
        assert_eq!(f.over_limit_text(), None);
        // --px WxH: the region widened to W:H as asked, the size reduced
        let s = Shot {
            bbox: Some([0., 0., 100., 100.]),
            pixels: Pixels::Size(8000, 4000),
            ..Default::default()
        };
        let f = s.frame([0.; 4], OverLimit::Reduce).unwrap();
        assert_eq!(f.bbox, [-50., 0., 150., 100.]);
        assert_eq!((f.width, f.height, f.asked), (5793, 2896, Some((8000, 4000))));
        assert_eq!((f.width, f.height), reduced(8000., 4000.));
        // the default policy reads FLOE_CAPTURE_REDUCE: unset reduces
        if std::env::var_os("FLOE_CAPTURE_REDUCE").is_none() {
            assert_eq!(s.fitted([0.; 4]).unwrap().1, 5793);
        }
    }
    #[test]
    fn the_reduced_size_is_within_the_limit_and_the_widest_of_its_aspect() {
        for (w, h) in [
            (1280., 13440.),
            (5000., 5000.),
            (5000., 5531.),
            (32768., 1024.),
            (4097., 4096.),
            (3., 1e8),
            (1., 1e12),
            (1e6, 1e6),
        ] {
            let (rw, rh) = reduced(w, h);
            let pixels = u64::from(rw) * u64::from(rh);
            assert!(pixels <= MAX_PIXELS && rw >= 1 && rh >= 1, "{w}x{h}: {rw}x{rh}");
            let derived = |x: u32| (f64::from(x) * h / w).round_ties_even().max(1.);
            if rw > 1 {
                assert_eq!(f64::from(rh), derived(rw), "{w}x{h}");
                assert!(f64::from(rw + 1) * derived(rw + 1) > MAX_PIXELS as f64, "{w}x{h}: {rw}x{rh} is not the widest");
            }
        }
        assert_eq!(reduced(1., 1e12), (1, MAX_PIXELS as u32));
        assert_eq!(reduced(5000., 5000.), (4096, 4096));
        assert_eq!(reduced(1280., 13440.), (1264, 13272));
        // a width over the limit at 1 px of height: the limit wide
        assert_eq!(reduced(2e7, 1.), (MAX_PIXELS as u32, 1));
        // largest: the same property with no asked width to stay under
        for (w, h) in [
            (100., 100.),
            (40., 420.),
            (403.378, 446.051),
            (0.3, 0.1),
            (1., 1e12),
            (2e7, 1.),
        ] {
            let (rw, rh) = largest(w, h);
            let pixels = u64::from(rw) * u64::from(rh);
            assert!(pixels <= MAX_PIXELS && rw >= 1 && rh >= 1, "{w}x{h}: {rw}x{rh}");
            let derived = |x: u32| (f64::from(x) * h / w).round_ties_even().max(1.);
            if rw > 1 && u64::from(rw) < MAX_PIXELS {
                assert_eq!(f64::from(rh), derived(rw), "{w}x{h}");
                assert!(f64::from(rw + 1) * derived(rw + 1) > MAX_PIXELS as f64, "{w}x{h}: {rw}x{rh} is not the largest");
            }
        }
        assert_eq!(largest(100., 100.), (4096, 4096));
        assert_eq!(largest(40., 420.), (1264, 13272));
        assert_eq!(largest(1., 1e12), (1, MAX_PIXELS as u32));
        assert_eq!(largest(2e7, 1.), (MAX_PIXELS as u32, 1));
    }
    #[test]
    fn unsaid_or_max_the_frame_is_the_largest_of_the_regions_aspect() {
        assert_eq!(pixels("max").unwrap(), Pixels::Largest);
        assert_eq!(pixels(" MAX ").unwrap(), Pixels::Largest);
        assert_eq!(pixels("1200").unwrap(), Pixels::Width(1200));
        assert_eq!(pixels("1200x900").unwrap(), Pixels::Size(1200, 900));
        for bad in ["", "0", "max1", "1200x", "1200x0", "-1"] {
            let e = pixels(bad).unwrap_err().to_string();
            assert_eq!(e, "--px must be positive W, WxH or max", "{bad}");
        }
        // the default: FLOE_CAPTURE_PX as --px reads it, else the largest
        assert_eq!(default_pixels_from(None).unwrap(), Pixels::Largest);
        assert_eq!(default_pixels_from(Some(" ")).unwrap(), Pixels::Largest);
        assert_eq!(default_pixels_from(Some("max")).unwrap(), Pixels::Largest);
        assert_eq!(default_pixels_from(Some("1200")).unwrap(), Pixels::Width(1200));
        assert_eq!(default_pixels_from(Some(" 1200x900 ")).unwrap(), Pixels::Size(1200, 900));
        let e = default_pixels_from(Some("big")).unwrap_err().to_string();
        assert_eq!(e, "FLOE_CAPTURE_PX=big: the default frame size is positive W, WxH or max");
        assert_eq!(Shot::default().pixels, Pixels::Unsaid);
        assert_eq!(Pixels::Width(7).resolved().unwrap(), Pixels::Width(7));
        // --px max: the strip of the 1280 case at its aspect's largest, no
        // size asked, nothing to warn about, the region as given
        let f = Shot {
            bbox: Some([0., 0., 40., 420.]),
            pixels: Pixels::Largest,
            ..Default::default()
        }
        .frame([0.; 4], OverLimit::Refuse)
        .unwrap();
        assert_eq!((f.bbox, f.width, f.height, f.asked), ([0., 0., 40., 420.], 1264, 13272, None));
        assert_eq!(f.over_limit_text(), None);
        assert_eq!(f.ppu(), (31.6, 13272. / 420.));
        // a stretched frame: the two scales apart
        let f = Shot {
            bbox: Some([0., 0., 100., 50.]),
            pixels: Pixels::Size(200, 200),
            stretch: true,
            ..Default::default()
        }
        .frame([0.; 4], OverLimit::Refuse)
        .unwrap();
        assert_eq!((f.bbox, f.ppu()), ([0., 0., 100., 50.], (2., 4.)));
        // unsaid, the same (the environment's default read in the process)
        if std::env::var_os("FLOE_CAPTURE_PX").is_none() {
            let f = Shot {
                bbox: Some([0., 0., 100., 100.]),
                ..Default::default()
            }
            .frame([0.; 4], OverLimit::Refuse)
            .unwrap();
            assert_eq!((f.bbox, f.width, f.height, f.asked), ([0., 0., 100., 100.], 4096, 4096, None));
            assert_eq!(f.ppu(), (40.96, 40.96));
            // the gate fixture's whole bbox: within the limit, the largest
            let f = Shot::default()
                .frame([0.023, 0.214, 403.401, 446.265], OverLimit::Refuse)
                .unwrap();
            assert!(u64::from(f.width) * u64::from(f.height) <= MAX_PIXELS);
            assert_eq!((f.width, f.height), largest(403.378, 446.051));
            assert_eq!(f.bbox, [0.023, 0.214, 403.401, 446.265]);
        }
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
