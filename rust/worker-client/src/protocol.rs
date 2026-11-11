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
            | "cell_info"
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

/// A frame's `drawn=` (asked with RenderRequest::drawn): the layers that
/// show in the frame, as their L/D keys - `1/0,2/3`, or `none` when no layer
/// does. None: the frame does not say (not asked, or a plain layout's).
pub(crate) fn parse_drawn(value: Option<&str>) -> Result<Option<Vec<(u32, u32)>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value == "none" {
        return Ok(Some(Vec::new()));
    }
    value
        .split(',')
        .map(|pair| {
            let (layer, datatype) = pair.split_once('/')?;
            let number = |text: &str| {
                (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| text.parse::<u32>().ok())
                    .flatten()
            };
            Some((number(layer)?, number(datatype)?))
        })
        .collect::<Option<Vec<_>>>()
        .map(Some)
        .ok_or_else(|| Error::protocol("invalid drawn"))
}

/// A row - a deck layer by its L/D key - and the colour it was painted in
/// (`Distinct`, Frame::distinct).
pub type RowColor = ((u32, u32), [u8; 4]);

/// Which rows are one colour and decided together (`Distinct`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowsBy {
    /// every L/D its own
    #[default]
    Pair,
    /// the rows of a layer number (a jobdeck's level view: a level and the
    /// chips under it; its layer view: a source layer's datatypes)
    Layer,
    /// the rows of a datatype number (a jobdeck's chip view: a source chip,
    /// whose place among the deck's sources is its rows' datatype, under
    /// every level)
    Datatype,
}

impl RowsBy {
    /// The key the rows that are one colour with `key` share.
    pub fn group(self, key: (u32, u32)) -> (u32, u32) {
        match self {
            Self::Pair => key,
            Self::Layer => (key.0, 0),
            Self::Datatype => (0, key.1),
        }
    }
    fn wire(self) -> &'static str {
        match self {
            Self::Pair => "pair",
            Self::Layer => "layer",
            Self::Datatype => "datatype",
        }
    }
}

/// A frame's `distinct=` (asked with RenderRequest::distinct): the rows that
/// painted in the frame and the colour each was painted in -
/// `1/0:0000ff,11/2:418dd9`, or `none` when no row painted. None: the frame
/// does not say (not asked, or a plain layout's).
pub(crate) fn parse_distinct(value: Option<&str>) -> Result<Option<Vec<RowColor>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value == "none" {
        return Ok(Some(Vec::new()));
    }
    value
        .split(',')
        .map(|entry| {
            let (row, color) = entry.split_once(':')?;
            let (layer, datatype) = row.split_once('/')?;
            let number = |text: &str| {
                (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| text.parse::<u32>().ok())
                    .flatten()
            };
            if color.len() != 6 || !color.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let byte = |at: usize| u8::from_str_radix(&color[at..at + 2], 16).ok();
            Some(((number(layer)?, number(datatype)?), [byte(0)?, byte(2)?, byte(4)?, 255]))
        })
        .collect::<Option<Vec<_>>>()
        .map(Some)
        .ok_or_else(|| Error::protocol("invalid distinct"))
}

/// RenderRequest::distinct: the layers a jobdeck's frame paints are told
/// apart by their colours (user 2026-11-11: "실제로 칠할때 색을 결정하고
/// 이전에 그린 색과 최대한 겹치지 않게 선택"). renderd decides a row's
/// colour when a pass of it first leaves a pixel in the frame: its own while
/// no row painted before it has that one, else the colour of `palette`
/// farthest from those painted so far (floe_render_core::distinct). The
/// rows a view colours alike on purpose stay one colour (`by`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Distinct {
    /// the rows that are one colour, decided together
    pub by: RowsBy,
    /// the colours to choose from, in tiers: a tier is turned to when every
    /// colour of those before it is in use
    pub palette: Vec<Vec<[u8; 4]>>,
    /// rows whose colour is theirs to keep (the layer properties'): the rows
    /// that are one colour with such a row are left as they are, and no
    /// other row is given a colour of theirs
    pub fixed: Vec<(u32, u32)>,
    /// the colours the picture's earlier frames decided (a mosaic's tiles),
    /// by a row of those that are one colour
    pub taken: Vec<RowColor>,
}

impl Distinct {
    fn wire(&self, command: &mut String) {
        let hex = |[r, g, b, _]: &[u8; 4]| format!("{r:02x}{g:02x}{b:02x}");
        write!(command, " distinct={}", self.by.wire()).unwrap();
        let tiers: Vec<String> = self
            .palette
            .iter()
            .filter(|tier| !tier.is_empty())
            .map(|tier| tier.iter().map(hex).collect::<Vec<_>>().join(","))
            .collect();
        if !tiers.is_empty() {
            write!(command, " distinct_palette={}", tiers.join(";")).unwrap();
        }
        if !self.fixed.is_empty() {
            let rows: Vec<String> = self.fixed.iter().map(|(l, d)| format!("{l}/{d}")).collect();
            write!(command, " distinct_fixed={}", rows.join(",")).unwrap();
        }
        if !self.taken.is_empty() {
            let rows: Vec<String> = self
                .taken
                .iter()
                .map(|((l, d), color)| format!("{l}/{d}:{}", hex(color)))
                .collect();
            write!(command, " distinct_taken={}", rows.join(",")).unwrap();
        }
    }
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
    /// The occupancy summary for this frame (`occupancy=on|off`, the viewer's
    /// toggle, 2026-10-20): on draws a layout's wide views from its
    /// design.ovo as a jobdeck's passes are drawn, off draws none for a
    /// jobdeck's passes either. None leaves renderd's rule (a jobdeck on, a
    /// plain layout off, FLOE_RUST_OCCUPANCY).
    pub occupancy: Option<bool>,
    /// A light cache's frame may go out before its placement fills are done
    /// and be drawn again after them (renderd's light attempts,
    /// DIRECT_READ_PLAN §5.4): the viewer's way (true, the default). A
    /// capture sets false (`progressive=0`): its frame waits for the fills.
    pub progressive: bool,
    /// The frame says which layers show in it (`drawn=1`; Frame::drawn): a
    /// capture's legend lists them alone (user 2026-11-10: "실제 그려진
    /// 것만 레전드를 추가하도록"). renderd says it of a jobdeck's frames -
    /// the deck layers left with a pixel of the composite, at one u32 a
    /// pixel while the frame is drawn - so only a capture that embeds a
    /// legend asks; the viewer never does.
    pub drawn: bool,
    /// The layers the frame paints are told apart by their colours
    /// (`distinct=`; `Distinct`, Frame::distinct): a jobdeck capture asks,
    /// the viewer never does. None: every layer in its own colour.
    pub distinct: Option<Distinct>,
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
            occupancy: None,
            progressive: true,
            drawn: false,
            distinct: None,
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
        if let Some(on) = self.occupancy {
            write!(command, " occupancy={}", if on { "on" } else { "off" }).unwrap();
        }
        if !self.progressive {
            command.push_str(" progressive=0");
        }
        if self.drawn {
            command.push_str(" drawn=1");
        }
        if let Some(distinct) = &self.distinct {
            distinct.wire(&mut command);
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
        assert!(
            !plain.contains(" vw=") && !plain.contains(" density=") && !plain.contains(" occupancy=")
        );
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
        // the occupancy summary toggle (2026-10-20): only when given
        r.occupancy = Some(false);
        assert!(r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .ends_with(" density=on occupancy=off"));
        r.occupancy = Some(true);
        assert!(r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .ends_with(" density=on occupancy=on"));
        // a capture's frame waits for a light cache's fills (L6): said only
        // when it does
        r.progressive = false;
        assert!(r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .ends_with(" density=on occupancy=on progressive=0"));
        // a capture's legend asks which layers show in the frame: said only
        // when asked
        r.drawn = true;
        assert!(r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .ends_with(" density=on occupancy=on progressive=0 drawn=1"));
        r.drawn = false;
        r.progressive = true;
        assert!(!r.command(1, 1, "/tmp/f", 1_000_000).unwrap().contains(" drawn="));
        // a jobdeck capture asks for its layers' colours to be told apart:
        // said only when asked, the lists only when there is something in
        // them, an empty tier never
        assert!(!r.command(1, 1, "/tmp/f", 1_000_000).unwrap().contains(" distinct"));
        r.distinct = Some(Distinct {
            by: RowsBy::Layer,
            ..Distinct::default()
        });
        assert!(r
            .command(1, 1, "/tmp/f", 1_000_000)
            .unwrap()
            .ends_with(" density=on occupancy=on distinct=layer"));
        r.distinct = Some(Distinct {
            by: RowsBy::Datatype,
            ..Distinct::default()
        });
        assert!(r.command(1, 1, "/tmp/f", 1_000_000).unwrap().ends_with(" distinct=datatype"));
        assert_eq!(RowsBy::Pair.group((7, 3)), (7, 3));
        assert_eq!(RowsBy::Layer.group((7, 3)), (7, 0));
        assert_eq!(RowsBy::Datatype.group((7, 3)), (0, 3));
        r.distinct = Some(Distinct {
            by: RowsBy::Pair,
            palette: vec![
                vec![[0, 0, 255, 255], [255, 255, 0, 255]],
                vec![],
                vec![[0x41, 0x8d, 0xd9, 255]],
            ],
            fixed: vec![(5, 0), (2, 3)],
            taken: vec![((1, 0), [255, 0, 0, 255]), ((11, 2), [0, 255, 0, 7])],
        });
        assert!(r.command(1, 1, "/tmp/f", 1_000_000).unwrap().ends_with(
            " distinct=pair distinct_palette=0000ff,ffff00;418dd9 distinct_fixed=5/0,2/3 distinct_taken=1/0:ff0000,11/2:00ff00"
        ));
        // rows past the line's limit: the command is refused, not cut
        r.distinct = Some(Distinct {
            taken: (0..6000).map(|n| ((n, 0), [0, 0, 0, 255])).collect(),
            ..Distinct::default()
        });
        assert!(r.command(1, 1, "/tmp/f", 1_000_000).is_err());
        r.distinct = None;
        r.viewport = Some((0, 480));
        assert!(r.command(1, 1, "/tmp/f", 1_000_000).is_err());
    }

    #[test]
    fn a_frames_colours_are_read_by_row() {
        // not said: None (not asked, or a plain layout's frame)
        assert_eq!(parse_distinct(None).unwrap(), None);
        // said: the rows that painted with their colours; `none` is no row
        assert_eq!(parse_distinct(Some("none")).unwrap(), Some(Vec::new()));
        assert_eq!(
            parse_distinct(Some("1/0:0000ff,11/2:418DD9")).unwrap(),
            Some(vec![((1, 0), [0, 0, 255, 255]), ((11, 2), [0x41, 0x8d, 0xd9, 255])])
        );
        for bad in [
            "1/0", "1/0:", "1/0:0000f", "1/0:0000fff", "1/0:#0000ff", "1:0000ff", "1/a:0000ff",
            "1/0:0000ff,", ",1/0:0000ff", "1/0:0000gg", "1/0:0000ff 2/0:ff0000", "-1/0:0000ff",
        ] {
            assert!(parse_distinct(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_frames_drawn_layers_are_read_as_pairs() {
        // not said: None (not asked, or a plain layout's frame)
        assert_eq!(parse_drawn(None).unwrap(), None);
        // said: the keys in the daemon's order; `none` is no layer
        assert_eq!(parse_drawn(Some("none")).unwrap(), Some(Vec::new()));
        assert_eq!(
            parse_drawn(Some("2/1,2/3,10/0")).unwrap(),
            Some(vec![(2, 1), (2, 3), (10, 0)])
        );
        assert_eq!(
            parse_drawn(Some("4294967295/0")).unwrap(),
            Some(vec![(u32::MAX, 0)])
        );
        for bad in ["2", "2/", "/1", "2/1,", ",2/1", "2/1/3", "a/1", "2/-1", "+2/1", "4294967296/0", "2/1 3/1"] {
            assert!(parse_drawn(Some(bad)).is_err(), "{bad}");
        }
    }
}
