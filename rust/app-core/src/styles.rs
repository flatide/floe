//! Design-default palette/properties. Bundled .def assets are compiled into the
//! executable, not read from a Python installation at runtime.
use crate::{
    catalog::{regular_file, Layout},
    Error, Result,
};
use floe_worker_client::{Fill, Layers, Style};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

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
/// What a layer properties file named for a run came to on its source
/// (`floe2 render SRC --layerprops FILE`, user 2026-11-12: "floe2 render에도
/// --layerprops 추가해줘" - the viewer's `floe2 view SRC --layerprops FILE`
/// of 0.12.451 for a capture; Dataset::set_layer_props).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamedProps {
    /// the file as it was read (a link read through)
    pub file: PathBuf,
    /// its rows, and those of them that name a layer the source has
    pub rows: usize,
    pub matched: usize,
    /// the rows of the source it hides (a jobdeck's level head hides its
    /// chips, a chip's own flag before its head's): what a capture that
    /// names no layers leaves out (Dataset::resolve_layers)
    pub hidden: BTreeSet<(u32, u32)>,
}
/// The rows of the layer properties file a command line names
/// (`--layerprops FILE`): a Calibre .layerprops, read as the viewer's load
/// reads one (desktop::read_props - text, 4 MiB at most) but through a link:
/// the file is the command line's own, where the design default beside a
/// source is opened without following one (regular_file). Its real path and
/// its rows; what cannot be read is said with the file's name.
pub fn read_named(path: &Path) -> Result<(PathBuf, Vec<crate::layerprops::Row>)> {
    let named = |e: Error| {
        Error::new(
            e.kind,
            format!("layer properties file {}: {e}", path.display()),
        )
    };
    let real = std::fs::canonicalize(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::opening(e, "layer properties file", path)
        } else {
            named(e.into())
        }
    })?;
    let f = regular_file(&real).map_err(|e| {
        if e.kind == crate::ErrorKind::Io {
            named(e)
        } else {
            // (regular_file names the path it was given)
            named(Error::new(e.kind, "not a regular file"))
        }
    })?;
    let mut text = String::new();
    f.take(crate::layerprops::MAX_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|e| {
            named(if e.kind() == std::io::ErrorKind::InvalidData {
                Error::input("not a text file (UTF-8)")
            } else {
                e.into()
            })
        })?;
    let doc = crate::layerprops::parse(&text).map_err(named)?;
    Ok((real, doc.rows))
}
/// What a named file's rows say of the visibility of the layers in `known`:
/// each layer's last flag (`0` hides, `1` shows; another token says nothing).
pub fn visibility(
    rows: &[crate::layerprops::Row],
    known: &BTreeSet<(u32, u32)>,
) -> BTreeMap<(u32, u32), bool> {
    rows.iter()
        .filter(|r| known.contains(&r.layer))
        .filter_map(|r| r.visible().map(|v| (r.layer, v)))
        .collect()
}
/// A named file's `rows` over the design default's (`base`), as the viewer's
/// Layer > load layer properties puts a file on a layout that has its
/// default (gui.py _props_tables, view::properties apply_properties): of
/// the layers in `known` the file names, a colour that is one replaces the
/// default's, a fill that is a pattern's name its fill, a width that is a
/// number its width - 1 or less takes a width the default gave away -, a
/// flag its flag; a field that says nothing leaves the default's. A layer
/// the file does not name keeps the default's rows, a row of a layer the
/// source has not is left out. Each layer named comes out as one row, so
/// what reads the list in order (set_layer_colors, layout_styles, a deck's
/// colours and styles) reads the file's say last.
pub fn overlay(
    base: &[LayerProps],
    rows: &[crate::layerprops::Row],
    known: &BTreeSet<(u32, u32)>,
) -> Vec<LayerProps> {
    let named: BTreeSet<(u32, u32)> = rows
        .iter()
        .map(|r| r.layer)
        .filter(|k| known.contains(k))
        .collect();
    let mut folded: BTreeMap<(u32, u32), LayerProps> = BTreeMap::new();
    let blank = |layer| LayerProps {
        layer,
        color: None,
        fill: String::new(),
        width: 1,
        visible: None,
    };
    // the default's say of those layers, as its readers fold its rows
    for p in base.iter().filter(|p| named.contains(&p.layer)) {
        let row = folded.entry(p.layer).or_insert_with(|| blank(p.layer));
        if p.color.is_some() {
            row.color = p.color;
        }
        if pattern(&p.fill).is_some() {
            row.fill.clone_from(&p.fill);
        }
        if p.width > 1 {
            row.width = p.width;
        }
        if p.visible.is_some() {
            row.visible = p.visible;
        }
    }
    for r in rows.iter().filter(|r| named.contains(&r.layer)) {
        let row = folded.entry(r.layer).or_insert_with(|| blank(r.layer));
        if let Some(c) = color(&r.color) {
            row.color = Some(c);
        }
        if pattern(&r.fill).is_some() {
            row.fill.clone_from(&r.fill);
        }
        if let Some(width) = r.line_width() {
            row.width = width;
        }
        if let Some(visible) = r.visible() {
            row.visible = Some(visible);
        }
    }
    base.iter()
        .filter(|p| !named.contains(&p.layer))
        .cloned()
        .chain(folded.into_values())
        .collect()
}
/// The layers a capture that names none draws of `shown` - the rows of the
/// source, in (layer, datatype) order - when a named file hides some of them
/// (`hidden`): the others, as the viewer draws the layout with the file
/// loaded. Every layer where it hides none of them; more rows than a render
/// command names (4096) are refused.
pub fn shown_layers(shown: Vec<(u32, u32)>, hidden: &BTreeSet<(u32, u32)>) -> Result<Layers> {
    if !shown.iter().any(|k| hidden.contains(k)) {
        return Ok(Layers::All);
    }
    let mut keys: Vec<_> = shown.into_iter().filter(|k| !hidden.contains(k)).collect();
    keys.sort_unstable();
    keys.dedup();
    if keys.len() > 4096 {
        return Err(Error::input(format!(
            "the layer properties file hides layers of a source with {} rows left on: a capture names 4096 at most - say the layers to draw with --layers / --layerid (--layers all: every layer)",
            keys.len()
        )));
    }
    Ok(if keys.is_empty() {
        Layers::None
    } else {
        Layers::Only(keys)
    })
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
    fn row(layer: (u32, u32), color: &str, fill: &str, visibility: &str, width: &str) -> crate::layerprops::Row {
        crate::layerprops::Row {
            layer,
            color: color.into(),
            fill: fill.into(),
            name: "N".into(),
            visibility: visibility.into(),
            width: width.into(),
        }
    }
    fn prop(layer: (u32, u32), color: Option<&str>, fill: &str, width: u8, visible: Option<bool>) -> LayerProps {
        LayerProps {
            layer,
            color: color.map(|c| super::color(c).unwrap()),
            fill: fill.into(),
            width,
            visible,
        }
    }
    /// what the list's readers come to for a layer: the last colour, the
    /// last fill that is a pattern's, the last width over 1 (set_layer_colors,
    /// layout_styles), the last flag
    fn read(props: &[LayerProps], layer: (u32, u32)) -> (Option<String>, Option<String>, u8, Option<bool>) {
        let (mut color, mut fill, mut width, mut visible) = (None, None, 1, None);
        for p in props.iter().filter(|p| p.layer == layer) {
            if let Some(c) = p.color {
                color = Some(color_text(c));
            }
            if pattern(&p.fill).is_some() {
                fill = Some(p.fill.clone());
            }
            if p.width > 1 {
                width = p.width;
            }
            if p.visible.is_some() {
                visible = p.visible;
            }
        }
        (color, fill, width, visible)
    }
    #[test]
    fn a_named_file_goes_over_the_design_default() {
        // user 2026-11-12: `floe2 render SRC --layerprops FILE`, as the
        // viewer's load puts a file on a layout that has its default
        let known: BTreeSet<(u32, u32)> = [(1, 0), (2, 0), (3, 0), (4, 1), (5, 0)].into();
        let base = [
            prop((1, 0), Some("#808080"), "solid", 4, Some(true)),
            prop((2, 0), Some("#111111"), "solid", 5, Some(false)),
            // two rows of one layer: read in order
            prop((3, 0), Some("#222222"), "nosuchfill", 1, None),
            prop((3, 0), None, "solid", 3, None),
            prop((4, 1), Some("#ffff00"), "solid", 2, Some(false)),
        ];
        let rows = [
            // every field says something: the file's
            row((1, 0), "#ff0000", "clear", "0", "2"),
            // nothing of it says anything (no colour of that name, no such
            // fill, no flag, no number): the default's, whole
            row((2, 0), "nocolour", "nosuchfill", "x", "wide"),
            // a width of 1 or less takes the default's width away; the
            // colour by its name
            row((3, 0), "blue", "speckle", "1", "1"),
            // a layer the default does not name, twice: the later row last
            row((5, 0), "#010203", "solid", "1", "3"),
            row((5, 0), "#040506", "nosuchfill", "0", "0"),
            // a layer the source has not
            row((77, 0), "#123456", "solid", "0", "8"),
        ];
        let merged = overlay(&base, &rows, &known);
        let s = |v: &str| Some(v.to_string());
        assert_eq!(read(&merged, (1, 0)), (s("#ff0000"), s("clear"), 2, Some(false)));
        assert_eq!(read(&merged, (2, 0)), (s("#111111"), s("solid"), 5, Some(false)));
        // (speckle is a fill's name as any other: the file's)
        assert_eq!(read(&merged, (3, 0)), (s("#0000ff"), s("speckle"), 1, Some(true)));
        // a layer the file does not name: the default's rows as they were
        assert_eq!(read(&merged, (4, 1)), (s("#ffff00"), s("solid"), 2, Some(false)));
        assert_eq!(read(&merged, (5, 0)), (s("#040506"), s("solid"), 1, Some(false)));
        assert!(merged.iter().all(|p| p.layer != (77, 0)));
        // one row a layer named, so a width taken away is not read again
        // from an earlier row
        for layer in [(1, 0), (2, 0), (3, 0), (5, 0)] {
            assert_eq!(merged.iter().filter(|p| p.layer == layer).count(), 1, "{layer:?}");
        }
        // no default at all, and no file row: each as it is
        assert_eq!(read(&overlay(&[], &rows, &known), (3, 0)), (s("#0000ff"), s("speckle"), 1, Some(true)));
        assert_eq!(read(&overlay(&[], &rows, &known), (2, 0)), (None, None, 1, None));
        assert_eq!(overlay(&base, &[], &known).len(), base.len());
        // the flags: each layer's last, of the layers the source has
        let flags = visibility(&rows, &known);
        assert_eq!(
            flags.into_iter().collect::<Vec<_>>(),
            [((1, 0), false), ((3, 0), true), ((5, 0), false)]
        );
    }
    #[test]
    fn a_shot_that_names_no_layers_leaves_the_hidden_ones_out() {
        let shown = vec![(3, 0), (1, 0), (2, 0), (4, 1)];
        let hidden = |keys: &[(u32, u32)]| keys.iter().copied().collect::<BTreeSet<_>>();
        // none of the rows hidden (a flag of a pair that holds nothing):
        // every layer, as without the file
        assert_eq!(shown_layers(shown.clone(), &hidden(&[])).unwrap(), Layers::All);
        assert_eq!(shown_layers(shown.clone(), &hidden(&[(9, 0)])).unwrap(), Layers::All);
        // the others, in (layer, datatype) order - a legend lists them so
        assert_eq!(
            shown_layers(shown.clone(), &hidden(&[(2, 0)])).unwrap(),
            Layers::Only(vec![(1, 0), (3, 0), (4, 1)])
        );
        assert_eq!(
            shown_layers(shown.clone(), &hidden(&[(1, 0), (2, 0), (3, 0), (4, 1)])).unwrap(),
            Layers::None
        );
        // more rows left on than a render command names
        let many: Vec<(u32, u32)> = (0..4098).map(|n| (n, 0)).collect();
        let e = shown_layers(many.clone(), &hidden(&[(0, 0)])).unwrap_err();
        assert_eq!(e.kind, crate::ErrorKind::InvalidInput);
        assert!(e.message.contains("4097 rows left on") && e.message.contains("--layers all"), "{e}");
        assert!(matches!(
            shown_layers(many[..4097].to_vec(), &hidden(&[(0, 0)])).unwrap(),
            Layers::Only(keys) if keys.len() == 4096
        ));
    }
    #[test]
    fn a_named_file_is_read_through_a_link_and_refused_by_its_name() {
        let dir = std::env::temp_dir().join(format!("floe-named-props-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.layerprops");
        std::fs::write(&file, "# colours\n1.0 red solid M1 0 3\nnot a row\n2 blue hatch\n").unwrap();
        let real = std::fs::canonicalize(&file).unwrap();
        let (path, rows) = read_named(&file).unwrap();
        assert_eq!(path, real);
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].layer, rows[0].visible(), rows[0].line_width()), ((1, 0), Some(false), Some(3)));
        assert_eq!((rows[1].layer, rows[1].visible(), rows[1].line_width()), ((2, 0), Some(true), Some(1)));
        // the command line's own file: a link is read through, and the
        // file's real path is what is kept (an output must not take its place)
        let link = dir.join("link.layerprops");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert_eq!(read_named(&link).unwrap().0, real);
        // what cannot be read is said with the name given
        let missing = dir.join("nope.layerprops");
        let e = read_named(&missing).unwrap_err();
        assert_eq!(e.message, format!("layer properties file not found: {}", missing.display()));
        let e = read_named(&dir).unwrap_err();
        assert_eq!(e.message, format!("layer properties file {}: not a regular file", dir.display()));
        assert_eq!(e.kind, crate::ErrorKind::InvalidInput);
        let binary = dir.join("bad.layerprops");
        std::fs::write(&binary, [0xffu8, 0xfe, 0, 1]).unwrap();
        let e = read_named(&binary).unwrap_err();
        assert_eq!(e.message, format!("layer properties file {}: not a text file (UTF-8)", binary.display()));
        let big = dir.join("big.layerprops");
        std::fs::write(&big, "1.0 red solid M1 1 1\n".repeat(210_000)).unwrap();
        let e = read_named(&big).unwrap_err();
        assert_eq!(e.message, format!("layer properties file {}: layerprops exceeds 4 MiB", big.display()));
        std::fs::remove_dir_all(&dir).unwrap();
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
