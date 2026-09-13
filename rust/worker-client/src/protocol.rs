use crate::{Error, ErrorKind, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// Unknown telemetry fields survive a round trip; required control fields are
/// strict. Duplicate keys and malformed tokens must never silently win.
#[derive(Clone, Debug)]
pub struct Fields(pub BTreeMap<String, String>);

impl Fields {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
    pub(crate) fn required(&self, key: &str) -> Result<&str> {
        self.get(key)
            .ok_or_else(|| Error::protocol(format!("missing {key}")))
    }
    pub fn u64(&self, key: &str) -> Result<u64> {
        let value = self.required(key)?;
        if !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::protocol(format!("invalid {key}")));
        }
        value
            .parse()
            .map_err(|_| Error::protocol(format!("invalid {key}")))
    }
    pub(crate) fn flag(&self, key: &str) -> Result<bool> {
        match self.required(key)? {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(Error::protocol(format!("invalid {key}"))),
        }
    }
}

pub(crate) struct Line {
    pub kind: String,
    pub fields: Fields,
}

pub(crate) fn parse_line(bytes: &[u8]) -> Result<Line> {
    if bytes.len() > MAX_LINE_BYTES {
        return Err(Error::protocol("daemon line exceeds limit"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Error::protocol("non-UTF8 daemon line"))?;
    if text.chars().any(|c| c.is_control() && c != '\t') {
        return Err(Error::protocol("control character in daemon line"));
    }
    let mut words = text.split_whitespace();
    let kind = words
        .next()
        .ok_or_else(|| Error::protocol("empty daemon line"))?;
    if !matches!(
        kind,
        "ready"
            | "opened"
            | "styled"
            | "frame"
            | "cancelled"
            | "dropped"
            | "error"
            | "bye"
            | "pick"
            | "snap"
    ) {
        return Err(Error::protocol(format!("unexpected response kind {kind}")));
    }
    let mut fields = BTreeMap::new();
    for word in words {
        let (key, value) = word
            .split_once('=')
            .ok_or_else(|| Error::protocol("malformed daemon field"))?;
        if key.is_empty()
            || value.is_empty()
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(Error::protocol("invalid daemon field"));
        }
        if fields.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(Error::protocol(format!("duplicate {key}")));
        }
    }
    Ok(Line {
        kind: kind.into(),
        fields: Fields(fields),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameFormat {
    Png,
    Raw,
}
impl FrameFormat {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Raw => "raw",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThinPolicy {
    Keep,
    Cull,
}
impl ThinPolicy {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Cull => "cull",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Layers {
    All,
    None,
    Only(Vec<(u32, u32)>),
}
impl Layers {
    pub(crate) fn wire(&self) -> Result<String> {
        match self {
            Self::All => Ok("all".into()),
            Self::None => Ok("none".into()),
            Self::Only(layers) => {
                if layers.is_empty() || layers.len() > 4096 {
                    return Err(Error::input("explicit layers must contain 1..4096 entries"));
                }
                Ok(layers
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .map(|(l, d)| format!("{l}/{d}"))
                    .collect::<Vec<_>>()
                    .join(","))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fill {
    Solid,
    Speckle,
    Clear,
    Pattern([u16; 16]),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Style {
    pub layer: (u32, u32),
    pub color: [u8; 4],
    pub fill: Fill,
    pub width: u8,
}

pub(crate) fn style_text(styles: &[Style]) -> Result<String> {
    if styles.is_empty() || styles.len() > 65536 {
        return Err(Error::input("styles must contain 1..65536 entries"));
    }
    let mut seen = BTreeSet::new();
    let mut text = String::new();
    for style in styles {
        if !seen.insert(style.layer) || !(1..=8).contains(&style.width) {
            return Err(Error::input("duplicate style layer or width outside 1..8"));
        }
        let fill = match style.fill {
            Fill::Solid => "solid".into(),
            Fill::Speckle => "speckle".into(),
            Fill::Clear => "clear".into(),
            Fill::Pattern(rows) => {
                let mut text = String::from("pat:");
                for row in rows {
                    write!(text, "{row:04x}").unwrap();
                }
                text
            }
        };
        let [r, g, b, a] = style.color;
        writeln!(
            text,
            "{}/{} #{r:02x}{g:02x}{b:02x}{a:02x} {fill} {}",
            style.layer.0, style.layer.1, style.width
        )
        .unwrap();
    }
    Ok(text)
}

/// View coordinates are raw DBU, not microns. None means full depth. Values
/// mirror the native worker defaults: no refinement, decode/raster separated.
#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub view: [f64; 4],
    pub width: u32,
    pub height: u32,
    pub depth: Option<u32>,
    pub cut_px: f64,
    pub exact: bool,
    pub layers: Layers,
    pub frames: bool,
    pub labels: bool,
    pub font_px: u32,
    pub mono: bool,
    pub frame_cache: bool,
    pub raster_jobs: u16,
    pub decode_jobs: u16,
    pub tile_px: u32,
    pub round_pages: u32,
    /// Explicit native decode ceiling (diagnostics); None leaves it unlimited.
    /// A partial result is still reported, never silently marked complete.
    pub decode_pages: Option<usize>,
    pub thin: ThinPolicy,
    pub format: FrameFormat,
}

impl Default for RenderRequest {
    fn default() -> Self {
        let jobs = std::thread::available_parallelism().map_or(1, |n| n.get().min(8) as u16);
        Self {
            view: [0., 0., 1., 1.],
            width: 800,
            height: 600,
            depth: None,
            cut_px: 0.,
            exact: false,
            layers: Layers::All,
            frames: false,
            labels: false,
            font_px: 14,
            mono: false,
            frame_cache: true,
            raster_jobs: jobs.min(4),
            decode_jobs: jobs,
            tile_px: 384,
            round_pages: 1 << 30,
            decode_pages: None,
            thin: ThinPolicy::Cull,
            format: FrameFormat::Raw,
        }
    }
}

impl RenderRequest {
    pub(crate) fn command(
        &self,
        generation: u64,
        epoch: u64,
        out: &str,
        max_pixels: u64,
    ) -> Result<String> {
        let [x0, y0, x1, y1] = self.view;
        if !self.view.iter().all(|x| x.is_finite())
            || x0 >= x1
            || y0 >= y1
            || !(x1 - x0).is_finite()
            || !(y1 - y0).is_finite()
        {
            return Err(Error::input("invalid view bbox"));
        }
        if self.width == 0
            || self.height == 0
            || u64::from(self.width) * u64::from(self.height) > max_pixels
        {
            return Err(Error::input("frame dimensions exceed pixel limit"));
        }
        if !self.cut_px.is_finite()
            || self.cut_px < 0.
            || !(6..=96).contains(&self.font_px)
            || !(1..=256).contains(&self.raster_jobs)
            || !(1..=256).contains(&self.decode_jobs)
            || !(1..=4096).contains(&self.tile_px)
            || self.round_pages == 0
        {
            return Err(Error::input("invalid render policy"));
        }
        if self.exact && (self.depth.is_some() || self.cut_px != 0. || self.frames) {
            return Err(Error::input(
                "exact requires full depth, cut=0 and frames=off",
            ));
        }
        let depth = self.depth.map_or_else(|| "full".into(), |d| d.to_string());
        let mut command = format!("render gen={generation} view={x0},{y0},{x1},{y1} w={} h={} depth={depth} cut={} exact={} layers={} frames={} labels={} font_px={} mono={} frame_cache={} jobs={} decode_jobs={} tile_px={} round_pages={} round_paths=1 frame_format={} thin={} style_epoch={epoch} out={out}",
            self.width, self.height, self.cut_px, u8::from(self.exact), self.layers.wire()?, u8::from(self.frames), u8::from(self.labels), self.font_px, u8::from(self.mono), u8::from(self.frame_cache), self.raster_jobs, self.decode_jobs, self.tile_px, self.round_pages, self.format.wire(), self.thin.wire());
        if let Some(limit) = self.decode_pages {
            write!(command, " decode_pages={limit}").unwrap();
        }
        if command.len() > MAX_LINE_BYTES {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "render command exceeds limit",
            ));
        }
        Ok(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_rejects_bad_utf8_duplicates_and_unbounded_lines() {
        for line in [
            b"frame gen=1 gen=2".as_slice(),
            b"frame gen",
            b"frame gen=",
            b"frame\0gen=1",
            b"frame gen=\xff",
            b"surprise a=1",
        ] {
            assert!(parse_line(line).is_err(), "{line:?}");
        }
        assert!(parse_line(&vec![b'x'; MAX_LINE_BYTES + 1]).is_err());
        let fields = parse_line(b"frame final=0 gen=1 new_metric=ok")
            .unwrap()
            .fields;
        assert!(!fields.flag("final").unwrap());
        assert_eq!(fields.get("new_metric"), Some("ok"));
        assert!(fields.u64("absent").is_err());
    }
    #[test]
    fn request_validation_and_distinct_layer_policies() {
        let mut r = RenderRequest::default();
        assert!(r.command(1, 1, "/tmp/f", 1).is_err());
        r.view[0] = f64::NAN;
        assert!(r.command(1, 1, "/tmp/f", 1_000_000).is_err());
        r.view = [0., 0., 1., 1.];
        r.exact = true;
        r.frames = true;
        assert!(r.command(1, 1, "/tmp/f", 1_000_000).is_err());
        assert_eq!(Layers::None.wire().unwrap(), "none");
        r.exact = false;
        r.frames = false;
        r.decode_pages = Some(0);
        assert!(r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .ends_with("decode_pages=0"));
        assert_eq!(Layers::All.wire().unwrap(), "all");
        assert!(Layers::Only(vec![]).wire().is_err());
    }
    #[test]
    fn styles_keep_order_and_reject_duplicates() {
        let s = Style {
            layer: (3, 300),
            color: [1, 2, 3, 255],
            fill: Fill::Pattern([0xffff; 16]),
            width: 2,
        };
        assert!(style_text(std::slice::from_ref(&s))
            .unwrap()
            .starts_with("3/300 #010203ff pat:ffff"));
        assert!(style_text(&[s.clone(), s]).is_err());
    }
}
