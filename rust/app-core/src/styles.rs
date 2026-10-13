//! Design-default palette/properties. Bundled .def assets are compiled into the
//! executable, not read from a Python installation at runtime.
use crate::{
    catalog::{regular_file, Layout},
    Error, Result,
};
use floe_worker_client::{Fill, Style};
use std::io::Read;
use std::path::Path;

const COLORS: &str = include_str!("../../../floe/colornames.def");
const PATTERNS: &str = include_str!("../../../floe/fillpatterns.def");
pub mod presets;

#[derive(Clone, Debug)]
pub struct LayerProps {
    pub layer: (u32, u32),
    pub color: Option<[u8; 4]>,
    pub fill: String,
    pub width: u8,
    pub visible: Option<bool>,
}
pub fn color_name(value: [u8; 4]) -> String {
    let hex = color_text(value);
    COLORS
        .lines()
        .filter_map(|line| {
            let mut p = line.split_whitespace();
            let name = p.next()?;
            (!name.starts_with('#') && p.next()? == &hex[1..]).then_some(name)
        })
        .next()
        .unwrap_or(&hex)
        .to_string()
}
pub(crate) fn pattern_name(fill: &Fill) -> Option<&'static str> {
    let Fill::Pattern(want) = fill else {
        return None;
    };
    PATTERNS.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        let name = words.next()?;
        if name.starts_with('#') {
            return None;
        }
        for &row in want {
            if words.next().and_then(|s| u16::from_str_radix(s, 16).ok()) != Some(row) {
                return None;
            }
        }
        words.next().is_none().then_some(name)
    })
}
pub fn color_text([r, g, b, _]: [u8; 4]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}
pub fn color(value: &str) -> Option<[u8; 4]> {
    let lower = value.to_lowercase();
    let hex = if let Some(hex) = lower.strip_prefix('#') {
        hex
    } else {
        COLORS
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                (parts.next()? == lower).then(|| parts.next()).flatten()
            })
            .next()?
    };
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
        255,
    ])
}
/// The palette's colour at place `n` (palette_ranks): a golden-angle step
/// (137.508 degrees) of hue per place, saturation 0.75, value 1 - the same
/// operation order as colorsys.hsv_to_rgb + int(x*255), floe-index's
/// meta.json and the oracle (floe_oracle.cache.layer_color).
pub fn layer_color(n: u32) -> [u8; 4] {
    let h = (f64::from(n) * 137.508) % 360. / 360.;
    let i = (h * 6.) as u32;
    let f = h * 6. - f64::from(i);
    let (p, q, t) = (0.25, 1. - 0.75 * f, 1. - 0.75 * (1. - f));
    let (r, g, b) = match i % 6 {
        0 => (1., t, p),
        1 => (q, 1., p),
        2 => (p, 1., t),
        3 => (p, q, 1.),
        4 => (t, p, 1.),
        _ => (1., p, q),
    };
    [(r * 255.) as u8, (g * 255.) as u8, (b * 255.) as u8, 255]
}
/// Each layer's place in the palette where no layer property names its
/// colour (user 2026-10-13: "a group shows one colour - 2.0 2.1 2.2 2.32 -
/// give consecutive layers different colours"; the palette was keyed by the
/// layer number since 2026-08-04): the pairs that hold shapes or text in
/// (layer, datatype) order - the layer panel's rows - then the empty ones
/// (named by the file's LAYERNAME table alone, listed only with
/// FLOE_EMPTY_LAYERS=show), so neighbouring rows are a golden angle apart in
/// hue (LayerColors::Row). floe-index (meta.json) and the oracle
/// (floe_oracle.cache.palette_ranks) place them the same way.
pub fn palette_ranks(
    pairs: impl IntoIterator<Item = ((u32, u32), u64)>,
) -> std::collections::BTreeMap<(u32, u32), u32> {
    let mut order: Vec<_> = pairs.into_iter().map(|(key, stored)| (stored == 0, key)).collect();
    order.sort_unstable();
    order
        .into_iter()
        .enumerate()
        .map(|(n, (_, key))| (key, u32::try_from(n).unwrap_or(u32::MAX)))
        .collect()
}
/// How a layout's default layer colours are given (user 2026-10-13, after
/// the rows: "a group's colours sometimes have to be one too - let a run
/// option choose"): `row`, every row of the layer panel its own colour,
/// neighbours a golden angle apart (palette_ranks; the default), or `group`,
/// one colour per layer number shared by its datatypes (as before 0.12.352).
/// FLOE_LAYER_COLORS=row|group is the default every open takes (the viewer's
/// `--layer-colors` sets it for its service, in a window of its own); `floe2
/// render --layer-colors` overrides it for its run (Layout::set_layer_colors).
/// A jobdeck keeps its own colour scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayerColors {
    #[default]
    Row,
    Group,
}
impl LayerColors {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "row" => Some(Self::Row),
            "group" => Some(Self::Group),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Row => "row",
            Self::Group => "group",
        }
    }
    /// FLOE_LAYER_COLORS: `row` or `group`; unset, row. Another value is
    /// said once and row is used.
    pub fn from_env() -> Self {
        let Ok(text) = std::env::var("FLOE_LAYER_COLORS") else {
            return Self::default();
        };
        Self::parse(&text).unwrap_or_else(|| {
            static SAID: std::sync::Once = std::sync::Once::new();
            SAID.call_once(|| {
                eprintln!(
                    "[{}][warn] FLOE_LAYER_COLORS={text:?}: row or group - row is used",
                    crate::program()
                )
            });
            Self::default()
        })
    }
    /// Each pair's place in the palette (layer_color): its panel row
    /// (palette_ranks) or its layer number.
    pub fn places(
        self,
        pairs: impl IntoIterator<Item = ((u32, u32), u64)>,
    ) -> std::collections::BTreeMap<(u32, u32), u32> {
        match self {
            Self::Row => palette_ranks(pairs),
            Self::Group => pairs.into_iter().map(|(key, _)| (key, key.0)).collect(),
        }
    }
}
pub fn load_props(source: &Path) -> Result<Vec<LayerProps>> {
    let mut full = source.as_os_str().to_owned();
    full.push(".layerprops");
    for path in [full.into(), source.with_extension("layerprops")] {
        let f = match regular_file(&path) {
            Ok(f) => f,
            Err(e) if e.kind == crate::ErrorKind::Io => continue,
            Err(e) => return Err(e),
        };
        let mut text = String::new();
        f.take(4 * 1024 * 1024 + 1).read_to_string(&mut text)?;
        if text.len() > 4 * 1024 * 1024 {
            return Err(Error::input("layerprops exceeds 4 MiB"));
        }
        return Ok(crate::layerprops::parse(&text)?
            .rows
            .into_iter()
            .map(|p| LayerProps {
                layer: p.layer,
                color: color(&p.color),
                width: p.line_width().unwrap_or(1),
                visible: p.visible(),
                fill: p.fill,
            })
            .collect());
    }
    Ok(vec![])
}
pub(crate) fn pattern(name: &str) -> Option<Fill> {
    pattern_definitions()
        .iter()
        .find_map(|(n, rows)| n.eq_ignore_ascii_case(name).then_some(Fill::Pattern(*rows)))
}
/// Canonical identity, never inferred from bitmap equality.
pub(crate) fn pattern_slot(name: &str) -> Option<&'static str> {
    if name.len() > 64 || !name.is_ascii() {
        return None;
    }
    pattern_definitions()
        .iter()
        .find_map(|(candidate, _)| candidate.eq_ignore_ascii_case(name).then_some(*candidate))
}
fn pattern_definitions() -> &'static [(&'static str, [u16; 16])] {
    static TABLE: std::sync::OnceLock<Vec<(&'static str, [u16; 16])>> = std::sync::OnceLock::new();
    // Resolve thousands of slot references without reparsing the compiled
    // immutable bitmap words per row. Invalid definitions cannot be slots.
    TABLE.get_or_init(|| {
        PATTERNS
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let name = parts.next()?;
                if name.starts_with('#') {
                    return None;
                }
                let mut rows = [0; 16];
                for row in &mut rows {
                    *row = u16::from_str_radix(parts.next()?, 16).ok()?;
                }
                parts.next().is_none().then_some((name, rows))
            })
            .collect()
    })
}
pub fn layout_styles(layout: &Layout, archival: bool) -> Result<Vec<Style>> {
    if layout.metadata.layers.is_empty() {
        return Err(Error::input("cache contains no renderable layers"));
    }
    let mut styles = Vec::new();
    for l in &layout.metadata.layers {
        let mut style = Style {
            layer: l.key(),
            color: color(&l.color).ok_or_else(|| Error::input("invalid layer color"))?,
            fill: if archival { Fill::Solid } else { Fill::Speckle },
            width: 1,
        };
        if !archival {
            for prop in layout.layer_props.iter().filter(|p| p.layer == l.key()) {
                // Match the adapter: only named bitmap slots override the
                // live default; export always restores solid / width=1.
                if let Some(fill) = pattern(&prop.fill) {
                    style.fill = fill;
                }
                if prop.width > 1 {
                    style.width = prop.width;
                }
            }
        }
        styles.push(style);
    }
    // Style row order is paint/plane stacking order. The metadata keeps OASIS
    // encounter order, whereas RustRenderWorker._publish_style sorts (L,D).
    styles.sort_by_key(|s| s.layer);
    Ok(styles)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn colors_and_unicode_are_checked() {
        assert_eq!(color("RED"), Some([255, 0, 0, 255]));
        assert_eq!(color("#1a2b3c"), Some([26, 43, 60, 255]));
        assert_eq!(color("#한글"), None);
        assert_eq!(color("#zzffff"), None);
        assert_eq!(layer_color(0), [255, 63, 63, 255]);
    }
    #[test]
    fn neighbouring_rows_take_neighbouring_places() {
        // one layer's datatypes no longer share a colour; the empty pair
        // (LAYERNAME only) goes after every drawn one
        let ranks = palette_ranks([
            ((3, 0), 5),
            ((2, 32), 1),
            ((2, 0), 9),
            ((2, 1), 0),
            ((2, 2), 4),
        ]);
        let places: Vec<_> = ranks.iter().map(|(k, n)| (*k, *n)).collect();
        assert_eq!(
            places,
            [((2, 0), 0), ((2, 1), 4), ((2, 2), 1), ((2, 32), 2), ((3, 0), 3)]
        );
        let colours: Vec<_> = [(2, 0), (2, 2), (2, 32), (3, 0)]
            .iter()
            .map(|k| layer_color(ranks[k]))
            .collect();
        for pair in colours.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
        assert_eq!(colours[0], [255, 63, 63, 255]);
        assert_eq!(colours[1], color("#3fff77").unwrap());
    }
    #[test]
    fn a_group_shares_its_layer_numbers_colour_when_asked() {
        let pairs = [((2, 0), 9), ((2, 1), 0), ((2, 32), 1), ((3, 0), 5)];
        let group = LayerColors::Group.places(pairs);
        assert_eq!(group[&(2, 0)], 2);
        assert_eq!(group[&(2, 1)], 2);
        assert_eq!(group[&(2, 32)], 2);
        assert_eq!(group[&(3, 0)], 3);
        assert_eq!(LayerColors::Row.places(pairs), palette_ranks(pairs));
        for mode in [LayerColors::Row, LayerColors::Group] {
            assert_eq!(LayerColors::parse(mode.name()), Some(mode));
        }
        assert_eq!(LayerColors::parse("groups"), None);
        assert_eq!(LayerColors::default(), LayerColors::Row);
    }
}
