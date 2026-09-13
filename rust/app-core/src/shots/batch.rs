//! Python shots' named capture grammar. No shell expansion or execution.
use super::{lengths, normalize, pixels, Anchor, Shot};
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
            return Err(Error::input("batch exceeds 16 MiB"));
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    String::from_utf8(bytes).map_err(|_| Error::input("batch must be UTF-8"))
}
pub fn read_file(
    path: &std::path::Path,
    cancelled: &std::sync::atomic::AtomicUsize,
) -> Result<String> {
    let path = std::fs::canonicalize(path)?;
    let file = crate::catalog::regular_file(&path)?;
    if file.metadata()?.len() > MAX_BATCH_BYTES as u64 {
        return Err(Error::input("batch exceeds 16 MiB"));
    }
    read_from(file, cancelled)
}

#[derive(Clone, Debug)]
pub struct Capture {
    pub shot: Shot,
    /// CLI input order is clockwise: TL, TR, BR, BL.
    pub mosaic: Option<[[f64; 2]; 4]>,
    pub corners: Option<[f64; 4]>,
    pub line: f64,
    pub line_color: String,
    pub keep_tiles: bool,
}
impl Default for Capture {
    fn default() -> Self {
        Self {
            shot: Shot::default(),
            mosaic: None,
            corners: None,
            line: 2.,
            line_color: "#ffffff".into(),
            keep_tiles: false,
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
        self.mosaic.is_some() || self.corners.is_some()
    }
    pub fn validate(&self) -> Result<()> {
        self.shot.validate()?;
        if [
            self.shot.bbox.is_some(),
            self.shot.at.is_some(),
            self.mosaic.is_some(),
            self.corners.is_some(),
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
        if !self.line.is_finite() || self.line < 0. {
            return Err(Error::input("line width must be finite and nonnegative"));
        }
        color(&self.line_color)?;
        Ok(())
    }
    /// Return already-fitted row-major tiles. All requests use the first tile's
    /// pixel size, as Python tile_boxes does (including fractional DBU views).
    pub fn tiles(&self, default_box: [f64; 4]) -> Result<Vec<Shot>> {
        self.validate()?;
        let mut tiles = Vec::new();
        if let Some(p) = self.mosaic {
            for i in [0, 1, 3, 2] {
                let mut shot = self.shot.clone();
                shot.at = Some(p[i]);
                tiles.push(shot);
            }
        } else if let Some(b) = self.corners {
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
        let fitted = tiles
            .iter()
            .map(|s| s.fitted(default_box))
            .collect::<Result<Vec<_>>>()?;
        let (_, width, height) = fitted[0];
        for (shot, (bbox, _, _)) in tiles.iter_mut().zip(fitted) {
            shot.at = None;
            shot.bbox = Some(bbox);
            shot.pixels = (width, Some(height));
            shot.stretch = true;
        }
        Ok(tiles)
    }
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
            if fields
                .iter()
                .any(|(key, _)| matches!(*key, "bbox" | "at" | "mosaic" | "corners"))
            {
                c.shot.bbox = None;
                c.shot.at = None;
                c.mosaic = None;
                c.corners = None;
            }
            for (key, v) in fields {
                match key {
                    "bbox" => c.shot.bbox = optional(v)?,
                    "at" => c.shot.at = optional(v)?,
                    "size" => c.shot.size = optional(v)?,
                    "mosaic" => c.mosaic = if v.is_empty() { None } else { Some(points(v)?) },
                    "corners" => c.corners = optional(v)?,
                    "px" => c.shot.pixels = pixels(v)?,
                    "anchor" => {
                        c.shot.anchor = match v {
                            "center" => Anchor::Center,
                            "lb" => Anchor::LowerLeft,
                            _ => return Err(Error::input("anchor must be center or lb")),
                        }
                    }
                    "layers" => {
                        c.shot.layers = if matches!(v, "" | "all") {
                            None
                        } else {
                            Some(v.into())
                        }
                    }
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
}
