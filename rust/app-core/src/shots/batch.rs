//! Python shots' named capture grammar. No shell expansion or execution.
use super::{lengths, normalize, over_limit, pixels, Anchor, Fitted, LayerChoice, Pixels, Shot};
use crate::{Error, Result};
use std::collections::BTreeSet;

pub const MAX_BATCH_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SHOTS: usize = 4096;
pub const TILE_NAMES: [&str; 4] = ["tl", "tr", "bl", "br"];

pub fn read_from(
    mut input: impl std::io::Read,
    cancelled: &std::sync::atomic::AtomicUsize,
) -> Result<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        crate::check_cancelled(cancelled)?;
        let n = match input.read(&mut buffer) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            n => n?,
        };
        if n == 0 {
            break;
        }
        if bytes.len() + n > MAX_BATCH_BYTES {
            return Err(Error::input("text input exceeds 16 MiB"));
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    String::from_utf8(bytes).map_err(|_| Error::input("text input must be UTF-8"))
}
pub fn read_file(
    path: &std::path::Path,
    cancelled: &std::sync::atomic::AtomicUsize,
) -> Result<String> {
    let path = std::fs::canonicalize(path)?;
    let file = crate::catalog::regular_file(&path)?;
    if file.metadata()?.len() > MAX_BATCH_BYTES as u64 {
        return Err(Error::input("text input exceeds 16 MiB"));
    }
    read_from(file, cancelled)
}

#[derive(Clone, Debug)]
pub struct Capture {
    pub shot: Shot,
    /// CLI input order is clockwise: TL, TR, BR, BL.
    pub mosaic: Option<[[f64; 2]; 4]>,
    pub corners: Option<[f64; 4]>,
    /// `--corners` alone (or `fit`): the corners of the default region -
    /// the chips' extent with chips named, else the whole source
    pub corners_fit: bool,
    pub line: f64,
    pub line_color: String,
    pub keep_tiles: bool,
    /// a jobdeck's chips (jobdeck::chips, user 2026-10-06): these alone on
    pub chip: Option<String>,
    /// these off, the others on
    pub chip_off: Option<String>,
    /// the default region is these chips' extent (`NAME#K`: one placement)
    pub fit_chip: Option<String>,
    /// `--in-chip NAME[#K]` (jobdeck::chips::ChipFrame, user 2026-11-09):
    /// the region's lengths are this placement's chip coordinates - the
    /// source's um, placed as the deck places it (`in_frame`); alone, the
    /// region is the placement itself
    pub in_chip: Option<String>,
}
impl Default for Capture {
    fn default() -> Self {
        Self {
            shot: Shot::default(),
            mosaic: None,
            corners: None,
            corners_fit: false,
            line: 2.,
            line_color: "#ffffff".into(),
            keep_tiles: false,
            chip: None,
            chip_off: None,
            fit_chip: None,
            in_chip: None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct NamedCapture {
    pub name: String,
    pub capture: Capture,
}
pub fn points(text: &str) -> Result<[[f64; 2]; 4]> {
    text.split(';')
        .filter(|s| !s.trim().is_empty())
        .map(lengths)
        .collect::<Result<Vec<_>>>()?
        .try_into()
        .map_err(|_| Error::input("mosaic needs four X,Y points separated by ';'"))
}
pub fn color(text: &str) -> Result<[u8; 3]> {
    let s = text.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return Err(Error::input("line color must be #rrggbb"));
    }
    let mut color = [0; 3];
    for (i, c) in color.iter_mut().enumerate() {
        *c = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
            .map_err(|_| Error::input("line color must be #rrggbb"))?;
    }
    Ok(color)
}
impl Capture {
    pub fn is_mosaic(&self) -> bool {
        self.mosaic.is_some() || self.corners.is_some() || self.corners_fit
    }
    /// A region of its own - not the default region (the chips' or the
    /// whole source), which `--corners` alone also takes.
    pub fn explicit_region(&self) -> bool {
        self.shot.bbox.is_some()
            || self.shot.at.is_some()
            || self.mosaic.is_some()
            || self.corners.is_some()
    }
    pub fn names_chips(&self) -> bool {
        self.chip.is_some() || self.chip_off.is_some() || self.fit_chip.is_some()
    }
    /// A region at all - its own, its chips' extent or its `--in-chip`
    /// placement's: what a deck's capture window takes (jobdeck::window);
    /// without one the shot draws the whole source.
    pub fn has_region(&self) -> bool {
        self.explicit_region() || self.names_chips() || self.in_chip.is_some()
    }
    /// The capture with its region placed as the frame places it
    /// (`--in-chip`: jobdeck::chips::ChipFrame) - a point `(dx + mag * x,
    /// dy + mag * y)`, a length `mag` times its own: --bbox, --at, --size,
    /// --mosaic-at and --corners X0,Y0,X1,Y1 alike; what is not a length
    /// (px, line, the chips, the layers) stays. A frame that takes a length
    /// out of the finite is refused.
    pub fn in_frame(&self, mag: f64, dx: f64, dy: f64) -> Result<Capture> {
        let point = |p: [f64; 2]| [dx + mag * p[0], dy + mag * p[1]];
        let rect = |b: [f64; 4]| [dx + mag * b[0], dy + mag * b[1], dx + mag * b[2], dy + mag * b[3]];
        let mut c = self.clone();
        c.shot.bbox = self.shot.bbox.map(rect);
        c.shot.at = self.shot.at.map(point);
        c.shot.size = self.shot.size.map(|s| [mag * s[0], mag * s[1]]);
        c.mosaic = self.mosaic.map(|points| points.map(point));
        c.corners = self.corners.map(rect);
        let finite = c
            .shot
            .bbox
            .iter()
            .chain(c.corners.iter())
            .flatten()
            .chain(c.shot.at.iter().chain(c.shot.size.iter()).flatten())
            .chain(c.mosaic.iter().flatten().flatten())
            .all(|v| v.is_finite());
        if !finite {
            return Err(Error::input(
                "the region in the chip's coordinates is not finite in the deck",
            ));
        }
        Ok(c)
    }
    pub fn validate(&self) -> Result<()> {
        self.shot.validate()?;
        if [
            self.shot.bbox.is_some(),
            self.shot.at.is_some(),
            self.mosaic.is_some(),
            self.corners.is_some() || self.corners_fit,
        ]
        .into_iter()
        .filter(|v| *v)
        .count()
            > 1
        {
            return Err(Error::input(
                "bbox/at/mosaic/corners are mutually exclusive",
            ));
        }
        if self.is_mosaic() && self.shot.size.is_none() {
            return Err(Error::input("mosaic/corners requires --size W,H"));
        }
        if self.fit_chip.is_some() && self.explicit_region() {
            return Err(Error::input(
                "--fit-chip frames the default region; --bbox, --at, --mosaic-at and --corners X1,Y1,X2,Y2 give a region of their own",
            ));
        }
        if self.in_chip.is_some() && self.fit_chip.is_some() {
            return Err(Error::input(
                "--in-chip NAME#K frames its placement in the chip's coordinates; --fit-chip frames another region - give one",
            ));
        }
        if self.shot.layers.is_some() && (self.chip.is_some() || self.chip_off.is_some()) {
            return Err(Error::input(
                "--layers and --chip/--chip-off both say what is on; give one",
            ));
        }
        if !self.line.is_finite() || self.line < 0. {
            return Err(Error::input("line width must be finite and nonnegative"));
        }
        color(&self.line_color)?;
        Ok(())
    }
    /// Return already-fitted row-major tiles. All requests use the first tile's
    /// pixel size, as Python tile_boxes does (including fractional DBU views).
    pub fn tiles(&self, default_box: [f64; 4]) -> Result<Vec<Shot>> {
        self.plan(default_box).map(|t| t.shots)
    }
    /// The tiles, and the first's frame as fitted (Shot::frame): its `asked`
    /// says the 16 Mpx limit reduced them - every tile has the first's size.
    pub fn plan(&self, default_box: [f64; 4]) -> Result<Tiles> {
        self.validate()?;
        let mut tiles = Vec::new();
        if let Some(p) = self.mosaic {
            for i in [0, 1, 3, 2] {
                let mut shot = self.shot.clone();
                shot.at = Some(p[i]);
                tiles.push(shot);
            }
        } else if let Some(b) = self.corners.or(self.corners_fit.then_some(default_box)) {
            let [x0, y0, x1, y1] = normalize(b)?;
            let [w, h] = self.shot.size.unwrap();
            for b in [
                [x0, y1 - h, x0 + w, y1],
                [x1 - w, y1 - h, x1, y1],
                [x0, y0, x0 + w, y0 + h],
                [x1 - w, y0, x1, y0 + h],
            ] {
                let mut shot = self.shot.clone();
                shot.bbox = Some(b);
                tiles.push(shot);
            }
        } else {
            tiles.push(self.shot.clone());
        }
        let policy = over_limit();
        let fitted = tiles
            .iter()
            .map(|s| s.frame(default_box, policy))
            .collect::<Result<Vec<_>>>()?;
        let frame = fitted[0];
        for (shot, f) in tiles.iter_mut().zip(fitted) {
            shot.at = None;
            shot.bbox = Some(f.bbox);
            shot.pixels = Pixels::Size(frame.width, frame.height);
            shot.stretch = true;
        }
        Ok(Tiles {
            shots: tiles,
            frame,
        })
    }
}
/// A capture's tiles as fitted (Capture::plan): one for a shot, four for a
/// mosaic, every one of the first's size.
pub struct Tiles {
    pub shots: Vec<Shot>,
    /// the first tile's frame: `asked` when the limit reduced the size
    pub frame: Fitted,
}
/// `corners=fit`: the corners of the default region.
pub fn is_fit(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("fit")
}
fn flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "on" | "true" | "yes"
    )
}
fn optional<const N: usize>(s: &str) -> Result<Option<[f64; N]>> {
    if s.is_empty() {
        Ok(None)
    } else {
        lengths(s).map(Some)
    }
}
/// Equivalent to shlex.split(comments=False, posix=True) on one batch line.
/// In particular #rrggbb is a value, not an inline comment; quoted empty
/// values are retained, and double-quoted backslashes escape only quote/slash.
fn words(line: &str) -> Result<Vec<String>> {
    let mut chars = line.chars();
    let (mut words, mut word, mut started, mut quote) = (Vec::new(), String::new(), false, None);
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if q == c => quote = None,
            (Some('\''), c) => word.push(c),
            (q, '\\') => {
                let n = chars
                    .next()
                    .ok_or_else(|| Error::input("unfinished escape"))?;
                if q == Some('"') && n != '"' && n != '\\' {
                    word.push('\\');
                }
                word.push(n);
                started = true;
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, ' ' | '\t' | '\n' | '\r') => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            _ => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err(Error::input("unclosed quote"));
    }
    if started {
        words.push(word);
    }
    Ok(words)
}
pub fn parse(text: &str, defaults: &Capture) -> Result<Vec<NamedCapture>> {
    if text.len() > MAX_BATCH_BYTES {
        return Err(Error::input("batch exceeds 16 MiB"));
    }
    let mut result = Vec::new();
    let mut names = BTreeSet::new();
    for (line, raw) in text.lines().enumerate() {
        let raw = raw.trim();
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let capture = (|| {
            let tokens = words(raw)?;
            let name = &tokens[0];
            if name.is_empty()
                || name.len() > 247
                || name.contains(['=', '/'])
                || matches!(name.as_str(), "." | "..")
                || name.chars().any(char::is_control)
            {
                return Err(Error::input(
                    "shot name must be a nonempty safe filename (up to 247 UTF-8 bytes)",
                ));
            }
            if !names.insert(name.clone()) {
                return Err(Error::input("batch shot names must be unique"));
            }
            let fields = tokens[1..]
                .iter()
                .map(|s| {
                    s.split_once('=')
                        .ok_or_else(|| Error::input("expected key=value"))
                })
                .collect::<Result<Vec<_>>>()?;
            let mut c = defaults.clone();
            // a line's own region form replaces the defaults' form - every
            // region key, fit_chip too; fit_chip or corners=fit, which frame
            // the default region, replace the explicit forms alone
            // (floe.shots parse_batch)
            if fields.iter().any(|(key, v)| {
                matches!(*key, "bbox" | "at" | "mosaic") || (*key == "corners" && !is_fit(v))
            }) {
                c.shot.bbox = None;
                c.shot.at = None;
                c.mosaic = None;
                c.corners = None;
                c.corners_fit = false;
                c.fit_chip = None;
            } else if fields
                .iter()
                .any(|(key, v)| *key == "fit_chip" || (*key == "corners" && is_fit(v)))
            {
                c.shot.bbox = None;
                c.shot.at = None;
                c.mosaic = None;
                c.corners = None;
            }
            // a line's own frame (in_chip) replaces the command line's
            // fit_chip, its fit_chip the command line's frame: each frames
            // the default region; a line's coordinates keep the command
            // line's frame - they are read in it
            if fields.iter().any(|(key, _)| *key == "fit_chip") {
                c.in_chip = None;
            }
            if fields.iter().any(|(key, _)| *key == "in_chip") {
                c.fit_chip = None;
            }
            // and a line's own say of what is on: its layers (by name or
            // id) replace the defaults' chips and layers, its chips the
            // defaults' layers
            let says_layers = fields
                .iter()
                .any(|(key, _)| matches!(*key, "layers" | "layer" | "layerid"));
            let mut layers = LayerChoice::default();
            if says_layers {
                c.chip = None;
                c.chip_off = None;
            }
            if fields
                .iter()
                .any(|(key, _)| matches!(*key, "chip" | "chip_off"))
            {
                c.shot.layers = None;
            }
            for (key, v) in fields {
                match key {
                    "bbox" => c.shot.bbox = optional(v)?,
                    "at" => c.shot.at = optional(v)?,
                    "size" => c.shot.size = optional(v)?,
                    "mosaic" => c.mosaic = if v.is_empty() { None } else { Some(points(v)?) },
                    "corners" => {
                        c.corners_fit = is_fit(v);
                        c.corners = if c.corners_fit { None } else { optional(v)? };
                    }
                    "chip" => c.chip = (!v.is_empty()).then(|| v.to_string()),
                    "chip_off" => c.chip_off = (!v.is_empty()).then(|| v.to_string()),
                    "fit_chip" => c.fit_chip = (!v.is_empty()).then(|| v.to_string()),
                    "in_chip" => c.in_chip = (!v.is_empty()).then(|| v.to_string()),
                    "px" => c.shot.pixels = pixels(v)?,
                    "anchor" => {
                        c.shot.anchor = match v {
                            "center" => Anchor::Center,
                            "lb" => Anchor::LowerLeft,
                            _ => return Err(Error::input("anchor must be center or lb")),
                        }
                    }
                    "layers" | "layer" => layers.names(v),
                    "layerid" => layers.ids(v)?,
                    "depth" => {
                        c.shot.depth = if matches!(v, "" | "full") {
                            None
                        } else {
                            let n = v
                                .trim()
                                .parse::<i64>()
                                .map_err(|_| Error::input("invalid depth"))?;
                            if n >= 999 {
                                None
                            } else {
                                Some(n.max(0) as u32)
                            }
                        }
                    }
                    "stretch" => c.shot.stretch = flag(v),
                    "keep_tiles" => c.keep_tiles = flag(v),
                    "line" => {
                        c.line = v
                            .trim()
                            .parse()
                            .map_err(|_| Error::input("invalid line width"))?
                    }
                    "linecolor" => c.line_color = v.into(),
                    _ => return Err(Error::input(format!("unknown batch key: {key}"))),
                }
            }
            if layers.said() {
                c.shot.layers = layers.spec();
            }
            c.validate()?;
            Ok(NamedCapture {
                name: name.clone(),
                capture: c,
            })
        })()
        .map_err(|e: Error| Error::new(e.kind, format!("batch line {}: {e}", line + 1)))?;
        if result.len() == MAX_SHOTS {
            return Err(Error::input("batch exceeds 4096 shots"));
        }
        result.push(capture);
    }
    if result.is_empty() {
        return Err(Error::input("batch file names no shots"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grammar_bounds_and_invalid_inputs() {
        let defaults = Capture::default();
        for text in [
            "",
            "# only comment",
            "\"\"",
            "x\nx",
            "x unknown=yes",
            "x line=NaN",
            "x line=-1",
            "x linecolor=한글",
            "x '\\",
            "x 'open",
            "x size=1,1 at=0,0 bbox=0,0,1,1",
        ] {
            assert!(parse(text, &defaults).is_err(), "{text:?}");
        }
        assert!(parse(&"a".repeat(247), &defaults).is_ok());
        assert!(parse(&"a".repeat(248), &defaults).is_err());
        let many = (0..MAX_SHOTS)
            .map(|i| format!("shot{i}\n"))
            .collect::<String>();
        assert_eq!(parse(&many, &defaults).unwrap().len(), MAX_SHOTS);
        assert!(parse(&(many + "last\n"), &defaults).is_err());
        let flag = std::sync::atomic::AtomicUsize::new(0);
        assert!(read_from(&b"\xff"[..], &flag).is_err());
        let oversized = vec![b'#'; MAX_BATCH_BYTES + 1];
        assert!(read_from(&oversized[..], &flag).is_err());
        assert!(parse(std::str::from_utf8(&oversized).unwrap(), &defaults).is_err());
        flag.store(1, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            read_from(&b"x"[..], &flag).unwrap_err().kind,
            crate::ErrorKind::Cancelled
        );
        assert_eq!(
            words(r#"'a b' linecolor=#123456 value="a\q" empty=''"#).unwrap(),
            ["a b", "linecolor=#123456", r"value=a\q", "empty="]
        );
    }
    #[test]
    fn grammar_defaults_and_clockwise_order() {
        let mut defaults = Capture::default();
        defaults.shot.at = Some([5., 5.]);
        defaults.shot.size = Some([2., 2.]);
        let parsed = parse("# comment\n'한 글' bbox=0,0,2,2 linecolor=#112233\nsecond mosaic='0,2;2,2;2,0;0,0' keep_tiles=yes", &defaults).unwrap();
        assert!(parsed[0].capture.shot.at.is_none());
        assert_eq!(parsed[0].name, "한 글");
        let tiles = parsed[1].capture.tiles([0., 0., 2., 2.]).unwrap();
        assert_eq!(
            tiles.iter().map(|s| s.bbox.unwrap()).collect::<Vec<_>>(),
            [
                [-1., 1., 1., 3.],
                [1., 1., 3., 3.],
                [-1., -1., 1., 1.],
                [1., -1., 3., 1.]
            ]
        );
        assert_eq!(
            words("a 'x y' \"a\\nb\" c\\ d empty=\"\"").unwrap(),
            ["a", "x y", "a\\nb", "c d", "empty="]
        );
        for bad in [
            "",
            "'x",
            "x extra",
            "x foo=1",
            "../x",
            "x\nx",
            "x line=nan",
            "x linecolor=한글",
            "x bbox=0,0,1,1 at=0,0",
        ] {
            assert!(parse(bad, &defaults).is_err(), "{bad}");
        }
    }
    #[test]
    fn a_frame_places_the_regions_lengths_and_a_line_keeps_or_replaces_it() {
        // --in-chip (2026-11-09): every length of the region in the chip's
        // um placed as the deck places the chip; what is not a length stays
        let mut c = Capture::default();
        c.shot.bbox = Some([100., 200., 300., 400.]);
        c.shot.size = Some([10., 20.]);
        c.corners = Some([0., 0., 1000., 1000.]);
        c.mosaic = Some([[0., 0.], [1., 0.], [1., 1.], [0., 1.]]);
        c.line = 3.;
        c.in_chip = Some("2:chipB.oas#1".into());
        let d = c.in_frame(2., 43020., 88000.).unwrap();
        assert_eq!(d.shot.bbox, Some([43220., 88400., 43620., 88800.]));
        assert_eq!(d.shot.size, Some([20., 40.]));
        assert_eq!(d.corners, Some([43020., 88000., 45020., 90000.]));
        assert_eq!(
            d.mosaic,
            Some([
                [43020., 88000.],
                [43022., 88000.],
                [43022., 88002.],
                [43020., 88002.]
            ])
        );
        assert_eq!((d.line, d.in_chip.as_deref()), (3., Some("2:chipB.oas#1")));
        let mut at = Capture::default();
        at.shot.at = Some([250., 250.]);
        at.shot.size = Some([100., 100.]);
        let d = at.in_frame(4., 43020., 88000.).unwrap();
        assert_eq!((d.shot.at, d.shot.size), (Some([44020., 89000.]), Some([400., 400.])));
        assert!(at.in_frame(f64::INFINITY, 0., 0.).is_err());
        // a frame alone has a region (the placement's); with --fit-chip refused
        let mut alone = Capture::default();
        alone.in_chip = Some("ID002#2".into());
        assert!(alone.has_region() && !alone.explicit_region() && !alone.names_chips());
        assert!(alone.validate().is_ok());
        alone.fit_chip = Some("ID001".into());
        assert!(alone.validate().is_err());
        assert!(!Capture::default().has_region());
        // batch: the command line's frame holds for a line's coordinates; a
        // line's own in_chip or fit_chip replaces it, an empty one clears it
        let mut defaults = Capture::default();
        defaults.in_chip = Some("2:chipB.oas#1".into());
        let parsed = parse(
            "a bbox=100,200,300,400\nb in_chip=5:chipB.oas#1 at=1,1 size=2,2\n\
             c fit_chip=ID002#2\nd in_chip= bbox=0,0,1,1\ne corners=fit size=5,5",
            &defaults,
        )
        .unwrap();
        let frames: Vec<_> = parsed
            .iter()
            .map(|c| (c.capture.in_chip.as_deref(), c.capture.fit_chip.as_deref()))
            .collect();
        assert_eq!(
            frames,
            [
                (Some("2:chipB.oas#1"), None),
                (Some("5:chipB.oas#1"), None),
                (None, Some("ID002#2")),
                (None, None),
                (Some("2:chipB.oas#1"), None)
            ]
        );
        assert_eq!(parsed[0].capture.shot.bbox, Some([100., 200., 300., 400.]));
        let mut fit = Capture::default();
        fit.fit_chip = Some("ID001".into());
        assert_eq!(
            parse("x in_chip=ID002#1", &fit).unwrap()[0].capture.fit_chip,
            None
        );
        assert!(parse("x in_chip=ID002#1 fit_chip=ID001", &fit).is_err());
    }
    #[test]
    fn a_line_names_its_layers_by_name_or_id() {
        // user 2026-10-26: layer= is layers=, layerid= the layer panel's
        // numbers; a line that says either replaces the defaults' layers
        let mut defaults = Capture::default();
        defaults.shot.layers = Some("ALIGN".into());
        let parsed = parse(
            "a layerid=2\nb layer=METAL1 layerid=3.1\nc layers=all layerid=1\n\
             d\ne layer=VIA1\nf chip=x\ng layer=all",
            &defaults,
        )
        .unwrap();
        assert_eq!(
            parsed
                .iter()
                .map(|c| c.capture.shot.layers.as_deref())
                .collect::<Vec<_>>(),
            [
                Some("2/0"),
                Some("METAL1,3/1"),
                Some("1/0"),
                Some("ALIGN"),
                Some("VIA1"),
                None,
                None
            ]
        );
        assert!(parse("a layerid=METAL1", &defaults).is_err());
    }
}
