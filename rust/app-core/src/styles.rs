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

#[derive(Clone, Debug)]
pub struct LayerProps {
    pub layer: (u32, u32),
    pub color: Option<[u8; 4]>,
    pub fill: String,
    pub width: u8,
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
pub fn layer_color(layer: u32) -> [u8; 4] {
    // Same operation order as colorsys.hsv_to_rgb + int(x*255) and
    // floe-index's palette: layer-number keyed, not datatype/row keyed.
    let h = (f64::from(layer) * 137.508) % 360. / 360.;
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
        let mut props = Vec::new();
        for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            let p: Vec<_> = line.split_whitespace().collect();
            if p.len() < 3 {
                continue;
            }
            let mut ld = p[0].split('.');
            let (Ok(l), Ok(d)) = (
                ld.next().unwrap_or("").parse(),
                ld.next().unwrap_or("0").parse(),
            ) else {
                continue;
            };
            let width = p
                .get(5)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(1)
                .clamp(1, 8) as u8;
            props.push(LayerProps {
                layer: (l, d),
                color: color(p[1]),
                fill: p[2].into(),
                width,
            });
        }
        return Ok(props);
    }
    Ok(vec![])
}
pub(crate) fn pattern(name: &str) -> Option<Fill> {
    for line in PATTERNS.lines() {
        let mut parts = line.split_whitespace();
        if !parts.next().is_some_and(|s| s.eq_ignore_ascii_case(name)) {
            continue;
        }
        let rows = parts
            .map(|p| u16::from_str_radix(p, 16))
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()?;
        return rows.try_into().ok().map(Fill::Pattern);
    }
    None
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
}
