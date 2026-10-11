use crate::{Error, ErrorKind, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// A command line: every request the client writes fits comfortably here.
pub const MAX_LINE_BYTES: usize = 64 * 1024;
/// A reply line. The cell tree's answers carry hex-encoded names by the
/// thousand (a `cells` answer holds up to 20,000 children, ~150 bytes each
/// with a 64-byte name), so replies get their own limit. A longer line is
/// still not buffered: the reader keeps its first LINE_PREFIX_BYTES (the
/// kind and `seq=`), drops the rest, and the client fails that one cell
/// query instead of the worker.
pub const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;
pub const LINE_PREFIX_BYTES: usize = 256;

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
    /// Only the LINE_PREFIX_BYTES head of an oversize line was parsed.
    pub truncated: bool,
}

/// The parsed head of a line over MAX_REPLY_BYTES: whole tokens only, so the
/// kind and `seq=` are trustworthy even though the payload is gone.
pub(crate) fn parse_prefix(bytes: &[u8]) -> Result<Line> {
    let end = bytes
        .iter()
        .rposition(|b| b.is_ascii_whitespace())
        .unwrap_or(0);
    let mut line = parse_line(&bytes[..end])?;
    line.truncated = true;
    Ok(line)
}

pub(crate) fn parse_line(bytes: &[u8]) -> Result<Line> {
    if bytes.len() > MAX_REPLY_BYTES {
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
            | "query_cancelled"
            | "clip"
            | "cell_sources"
            | "cells"
            | "cell_find"
            | "cell_bbox"
            | "cell_insts"
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
        truncated: false,
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
    /// Optional margin work must never change a foreground budget-fit decision.
    pub background: bool,
    pub raster_jobs: u16,
    pub decode_jobs: u16,
    pub tile_px: u32,
    pub round_pages: u32,
    /// Explicit native decode ceiling (diagnostics); None leaves it unlimited.
    /// A partial result is still reported, never silently marked complete.
    pub decode_pages: Option<usize>,
    pub thin: ThinPolicy,
    pub format: FrameFormat,
    /// The view root (SPEC-VIEWER §8c): the plan starts from this cell in
    /// ITS coordinates; None = the top. A jobdeck has none.
    pub root: Option<u32>,
    /// The viewer's viewport (w, h px) when this frame is not it (a margin:
    /// the viewport and the area around it at the viewport's scale) -
    /// `vw=`/`vh=`: the fit view the density dots thin past is the
    /// viewport's (renderd, 2026-10-04). None: the frame is the viewport.
    pub viewport: Option<(u32, u32)>,
    /// The density under the cut for this frame (`density=on|off`, the
    /// desktop viewer's toggle, 2026-10-05); None leaves renderd's default
    /// (FLOE_RUST_DENSITY_STACK).
    pub density: Option<bool>,
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
            background: false,
            raster_jobs: jobs.min(4),
            decode_jobs: jobs,
            tile_px: 384,
            round_pages: 1 << 30,
            decode_pages: None,
            thin: ThinPolicy::Cull,
            format: FrameFormat::Raw,
            root: None,
            viewport: None,
            density: None,
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
        if self.background {
            command.push_str(" bg=on");
        }
        if let Some(limit) = self.decode_pages {
            write!(command, " decode_pages={limit}").unwrap();
        }
        if let Some(root) = self.root {
            write!(command, " root={root}").unwrap();
        }
        if let Some((vw, vh)) = self.viewport {
            if vw == 0 || vh == 0 {
                return Err(Error::input("invalid viewport size"));
            }
            write!(command, " vw={vw} vh={vh}").unwrap();
        }
        if let Some(density) = self.density {
            write!(command, " density={}", if density { "on" } else { "off" }).unwrap();
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
        assert!(parse_line(&vec![b'x'; MAX_REPLY_BYTES + 1]).is_err());
        // A reply may exceed the command limit: the cell tree's answers do.
        let mut long = b"cells seq=1 found=1 children=".to_vec();
        long.resize(MAX_LINE_BYTES + 1, b'a');
        assert_eq!(parse_line(&long).unwrap().kind, "cells");
        let fields = parse_line(b"frame final=0 gen=1 new_metric=ok")
            .unwrap()
            .fields;
        assert!(!fields.flag("final").unwrap());
        assert_eq!(fields.get("new_metric"), Some("ok"));
        assert!(fields.u64("absent").is_err());
    }
    #[test]
    fn an_oversize_prefix_keeps_whole_tokens_only() {
        let head = parse_prefix(b"cell_find seq=12 src=-1 found=1 total=9 n=9 matc").unwrap();
        assert!(head.truncated);
        assert_eq!(head.kind, "cell_find");
        assert_eq!(head.fields.get("seq"), Some("12"));
        assert_eq!(head.fields.get("n"), Some("9"));
        assert!(head.fields.get("matc").is_none());
        assert!(parse_prefix(b"cells").is_err());
        assert!(parse_prefix(b"surprise seq=1 ").is_err());
    }
    #[test]
    fn a_view_root_is_an_explicit_trailing_field() {
        let mut r = RenderRequest::default();
        assert!(!r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .contains(" root="));
        r.root = Some(17);
        assert!(r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .ends_with(" root=17"));
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

    #[test]
    fn background_is_explicit_and_foreground_is_the_default() {
        let mut r = RenderRequest::default();
        assert!(!r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .contains(" bg="));
        r.background = true;
        assert!(r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .ends_with(" bg=on"));
    }

    #[test]
    fn viewport_and_density_go_on_the_wire_only_when_given() {
        let mut r = RenderRequest::default();
        let plain = r.command(1, 1, "/tmp/f", 1_000_000).unwrap();
        assert!(!plain.contains(" vw=") && !plain.contains(" density="));
        r.background = true;
        r.viewport = Some((640, 480));
        r.density = Some(false);
        assert!(r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .ends_with(" bg=on vw=640 vh=480 density=off"));
        r.density = Some(true);
        assert!(r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .ends_with(" density=on"));
        r.viewport = Some((0, 480));
        assert!(r.command(1, 1, "/tmp/f", 1_000_000).is_err());
    }
}
