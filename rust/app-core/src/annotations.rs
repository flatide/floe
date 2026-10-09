//! Flateyes key=value annotation metadata, not an overlay rasterizer.
//! Keep the Python fe_embed serialization/option order; never run Python.
use crate::{check_cancelled, Error, Result};
use serde_json::{json, Map, Value};
use std::sync::atomic::AtomicUsize;

pub mod png;
pub const MAX_TEXT: usize = 16 * 1024 * 1024;
pub const MAX_ANNOTATIONS: usize = 100_000;
const DEFAULT_LINE: &str = "#FF5040";
const PALETTE: [(&str, &str); 7] = [
    ("black", "#000000"),
    ("white", "#FFFFFF"),
    ("red", "#FF5040"),
    ("orange", "#FF9F1A"),
    ("green", "#3DDC55"),
    ("sky", "#35C5FF"),
    ("pink", "#FF4FD8"),
];
const PATTERNS: &str = include_str!("../../../floe/fillpatterns.def");

pub fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\n', "\\n")
}
pub fn unescape(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('n') => {
                    chars.next();
                    out.push('\n');
                    continue;
                }
                Some('\\') => {
                    chars.next();
                    out.push('\\');
                    continue;
                }
                _ => (),
            }
        }
        out.push(c);
    }
    out
}
fn text_value(value: &Value) -> Result<&str> {
    value
        .as_str()
        .ok_or_else(|| Error::input("expected a string"))
}
fn safe_text(s: &str, multiline: bool) -> Result<()> {
    if s.len() > MAX_TEXT
        || s.chars().any(|c| {
            matches!(c, '\u{2028}' | '\u{2029}')
                || (c.is_control() && !(multiline && matches!(c, '\n' | '\t')))
        })
    {
        return Err(Error::input(
            "metadata text exceeds 16 MiB or contains an unsupported control character",
        ));
    }
    Ok(())
}
fn finite(v: &Value) -> Result<f64> {
    let n = v
        .as_f64()
        .or_else(|| v.as_str()?.trim().parse().ok())
        .filter(|n: &f64| n.is_finite())
        .ok_or_else(|| Error::input("coordinates/scale must be finite numbers"))?;
    Ok(n)
}
pub fn number(s: &str) -> Result<f64> {
    finite(&Value::String(s.into()))
}
fn integer(v: &Value, min: i64, max: i64) -> Result<i64> {
    let n = if let Some(s) = v.as_str() {
        s.trim()
            .parse::<i64>()
            .map_err(|_| Error::input("expected an integer"))?
    } else {
        let f = finite(v)?.trunc();
        if f < min as f64 || f > max as f64 {
            return Err(Error::input("integer out of range"));
        }
        f as i64
    };
    if !(min..=max).contains(&n) {
        return Err(Error::input(format!("integer must be {min}..{max}")));
    }
    Ok(n)
}
fn truth(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.),
        Value::String(s) => !s.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}
fn hex_color(s: &str, digits: usize) -> bool {
    s.starts_with('#')
        && s.len() == digits + 1
        && s.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}
fn color(s: &str, preserve_hex: bool) -> Result<String> {
    let s = s.trim();
    if let Some((_, rgb)) = PALETTE
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(s))
    {
        return Ok((*rgb).into());
    }
    if hex_color(s, 6) {
        return Ok(if preserve_hex {
            s.into()
        } else {
            s.to_ascii_uppercase()
        });
    }
    Err(Error::input(
        "color must be a flateyes palette name or #RRGGBB",
    ))
}
fn pattern(s: &str) -> Result<String> {
    let s = s.trim().to_ascii_lowercase();
    if PATTERNS
        .lines()
        .filter(|l| !l.starts_with('#'))
        .any(|l| l.split_whitespace().next() == Some(s.as_str()))
        || s.strip_prefix("pat:")
            .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Ok(s);
    }
    Err(Error::input(
        "unknown fill pattern (or pat: requires 64 hex digits)",
    ))
}
pub fn legend_line(s: &str) -> Result<String> {
    safe_text(s, true)?;
    let p: Vec<_> = s.split_whitespace().collect();
    if p.len() < 3 {
        return Err(Error::input("legend needs kind, color and label"));
    }
    let kind = p[0].to_ascii_lowercase();
    if !matches!(kind.as_str(), "box" | "line") {
        return Err(Error::input("legend kind must be box or line"));
    }
    let rgb = color(p[1], true)?;
    let token = p[2].to_ascii_lowercase();
    let style = match (kind.as_str(), token.as_str()) {
        (_, "solid") => Some("solid".into()),
        ("box", "none" | "outline" | "empty" | "clear") => Some("none".into()),
        ("box", "hatch") => Some("hatch".into()),
        ("box", "cross" | "crosshatch") => Some("cross".into()),
        ("box", "dots" | "dotted") => Some("dots".into()),
        ("line", "dash" | "dashed") => Some("dashed".into()),
        ("line", "dot" | "dotted") => Some("dotted".into()),
        ("box", _) if token.starts_with("pat:") => Some(pattern(&token)?),
        ("box", _) => pattern(&token).ok(),
        _ => None,
    };
    let start = if style.is_some() { 3 } else { 2 };
    if start == p.len() {
        return Err(Error::input("legend is missing a label"));
    }
    Ok(format!(
        "{kind} {rgb} {} {}",
        style.as_deref().unwrap_or("solid"),
        p[start..].join(" ")
    ))
}
/// Python %.10g, including its exponent switch after rounding and signed zero.
fn coord(v: f64) -> String {
    let scientific = format!("{v:.9e}");
    let (mantissa, exp) = scientific.split_once('e').expect("finite float exponent");
    let exp: i32 = exp.parse().expect("formatted exponent");
    if !(-4..10).contains(&exp) {
        format!(
            "{}e{exp:+03}",
            mantissa.trim_end_matches('0').trim_end_matches('.')
        )
    } else {
        let precision = (9 - exp) as usize;
        let fixed = format!("{v:.precision$}");
        if precision == 0 {
            fixed
        } else {
            fixed.trim_end_matches('0').trim_end_matches('.').into()
        }
    }
}
fn point(v: &Value) -> Result<[f64; 2]> {
    let p = v
        .as_array()
        .filter(|p| p.len() == 2)
        .ok_or_else(|| Error::input("point must contain exactly X,Y"))?;
    Ok([finite(&p[0])?, finite(&p[1])?])
}
fn take_point(m: &mut Map<String, Value>, key: &str, x: &str, y: &str) -> Result<[f64; 2]> {
    if let Some(v) = m.remove(key).filter(truth) {
        point(&v)
    } else {
        point(&json!([
            m.remove(x)
                .ok_or_else(|| Error::input(format!("missing {x}")))?,
            m.remove(y)
                .ok_or_else(|| Error::input(format!("missing {y}")))?
        ]))
    }
}
fn unknown(m: &Map<String, Value>) -> Result<()> {
    if !m.is_empty() {
        Err(Error::input(format!(
            "unknown annotation fields: {}",
            m.keys().cloned().collect::<Vec<_>>().join(", ")
        )))
    } else {
        Ok(())
    }
}
/// A validated metadata record. Geometry overlays use their own typed model;
/// this stores the interoperable line without a second point-vector copy.
#[derive(Clone, Debug)]
pub struct Annotation(String);
impl Annotation {
    pub fn as_line(&self) -> &str {
        &self.0
    }
    pub fn from_json(value: Value) -> Result<Self> {
        let Value::Object(mut m) = value else {
            return Err(Error::input("annotation needs an object"));
        };
        let kind = m
            .remove("kind")
            .ok_or_else(|| Error::input("annotation needs kind"))?;
        let kind = text_value(&kind)?;
        if !matches!(
            kind,
            "box" | "ellipse" | "line" | "path" | "polygon" | "ruler" | "text"
        ) {
            return Err(Error::input("unknown annotation kind"));
        }
        let points = if matches!(kind, "path" | "polygon") {
            let points = m
                .remove("points")
                .ok_or_else(|| Error::input("missing points"))?;
            let points = points
                .as_array()
                .filter(|p| p.len() >= if kind == "polygon" { 3 } else { 2 })
                .ok_or_else(|| Error::input("path/polygon needs at least 2/3 points"))?;
            if points.len() > 1_000_000 {
                return Err(Error::input("annotation exceeds 1M points"));
            }
            points.iter().map(point).collect::<Result<Vec<_>>>()?
        } else if kind == "text" {
            vec![take_point(&mut m, "at", "x", "y")?]
        } else {
            vec![
                take_point(&mut m, "a", "x1", "y1")?,
                take_point(&mut m, "b", "x2", "y2")?,
            ]
        };
        let xy = points
            .iter()
            .flat_map(|p| p.iter().map(|v| coord(*v)))
            .collect::<Vec<_>>()
            .join(",");
        if kind == "ruler" {
            unknown(&m)?;
            return Ok(Self(format!("ruler={xy}")));
        }
        let rgb = m
            .remove("color")
            .map(|v| color(text_value(&v)?, false))
            .transpose()?
            .unwrap_or_else(|| DEFAULT_LINE.into());
        let line = if kind == "text" {
            let content = m
                .remove("text")
                .ok_or_else(|| Error::input("text is required"))?;
            let content = text_value(&content)?;
            safe_text(content, true)?;
            if content.trim().is_empty() {
                return Err(Error::input("empty text"));
            }
            let size = m
                .remove("size")
                .map(|v| integer(&v, 6, 96))
                .transpose()?
                .unwrap_or(16);
            let bg = m.remove("bg").is_none_or(|v| truth(&v));
            let opaque = m.remove("bg_opaque").is_some_and(|v| truth(&v));
            let background = m
                .remove("bg_color")
                .map(|v| color(text_value(&v)?, false))
                .transpose()?
                .unwrap_or_else(|| "#000000".into());
            let bg = if bg {
                format!("{background}{:02X}", if opaque { 255 } else { 89 })
            } else {
                "0".into()
            };
            format!("text={xy},{size},{rgb},{bg},{}", escape(content))
        } else {
            let width = m
                .remove("width")
                .map(|v| integer(&v, 1, 8))
                .transpose()?
                .unwrap_or(1);
            let dash = match m.remove("dash") {
                None => 0,
                Some(Value::String(s)) => match s.to_ascii_lowercase().as_str() {
                    "solid" => 0,
                    "dashed" => 1,
                    "dotted" => 2,
                    _ => return Err(Error::input("dash must be solid/dashed/dotted")),
                },
                Some(v) => {
                    let n = finite(&v)?;
                    if !matches!(n, 0. | 1. | 2.) {
                        return Err(Error::input("dash must be 0/1/2"));
                    }
                    n as i64
                }
            };
            let casing = m.remove("casing").is_none_or(|v| truth(&v));
            let mut outline = true;
            let mut fill = "0".to_string();
            if matches!(kind, "box" | "ellipse" | "polygon") {
                outline = m.remove("outline").is_none_or(|v| truth(&v));
                let alpha = m
                    .remove("fill_alpha")
                    .filter(|v| !v.is_null())
                    .map(|v| integer(&v, 0, 255))
                    .transpose()?;
                let mut pat = if kind == "polygon" {
                    m.remove("fill_pat")
                        .filter(|v| !v.is_null())
                        .map(|v| pattern(text_value(&v)?))
                        .transpose()?
                } else {
                    None
                };
                if let Some(value) = m.remove("fill").filter(|v| !v.is_null()) {
                    let mut value = text_value(&value)?;
                    if kind == "polygon" {
                        if let Some((a, b)) = value.split_once(':') {
                            value = a;
                            if pat.is_none() {
                                pat = Some(pattern(b)?);
                            }
                        }
                    }
                    let value = value.trim();
                    let (fill_color, embedded) = if hex_color(value, 8) {
                        (
                            value[..7].to_string(),
                            Some(i64::from(u8::from_str_radix(&value[7..], 16).unwrap())),
                        )
                    } else {
                        (color(value, false)?, None)
                    };
                    let alpha = alpha
                        .or(embedded)
                        .unwrap_or(if pat.is_some() { 255 } else { 89 });
                    fill = format!("{fill_color}{alpha:02X}");
                    if let Some(p) = pat {
                        fill.push(':');
                        fill.push_str(&p);
                    }
                } else if !outline || pat.is_some() {
                    return Err(Error::input("no-outline/pattern requires a fill color"));
                }
            }
            let halo = if casing { "" } else { ",0" };
            format!(
                "{kind}={xy},{},{fill},{width},{dash}{halo}",
                if outline { rgb.as_str() } else { "0" }
            )
        };
        unknown(&m)?;
        if line.len() > MAX_TEXT {
            return Err(Error::input("annotation exceeds 16 MiB"));
        }
        Ok(Self(line))
    }
    pub fn option(kind: &str, value: &str) -> Result<Self> {
        let mut m = Map::new();
        m.insert("kind".into(), json!(kind));
        if kind == "text" {
            let p: Vec<_> = value.splitn(3, ',').collect();
            if p.len() != 3 {
                return Err(Error::input("text expects X,Y[,STYLE...],TEXT"));
            }
            m.insert("at".into(), json!([number(p[0])?, number(p[1])?]));
            let mut rest = p[2].to_string();
            loop {
                let (head, tail) = rest
                    .split_once(',')
                    .map_or((rest.as_str(), None), |(a, b)| (a, Some(b)));
                let Some((key, val)) = head.split_once('=') else {
                    break;
                };
                let key = key.trim().to_ascii_lowercase();
                if key == "text" {
                    rest = format!(
                        "{val}{}{}",
                        if tail.is_some() { "," } else { "" },
                        tail.unwrap_or("")
                    );
                    break;
                }
                if !matches!(key.as_str(), "size" | "color" | "bg") {
                    break;
                }
                let tail = tail
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| Error::input("TEXT missing after style"))?;
                let val = val.trim();
                match key.as_str() {
                    "size" => {
                        m.insert(
                            "size".into(),
                            json!(val
                                .parse::<i64>()
                                .map_err(|_| Error::input("invalid text size"))?),
                        );
                    }
                    "color" => {
                        m.insert("color".into(), json!(val));
                    }
                    _ if val == "0" => {
                        m.insert("bg".into(), json!(false));
                    }
                    _ => {
                        m.insert("bg".into(), json!(true));
                        let (rgb, opaque) = if hex_color(val, 8) {
                            (&val[..7], u8::from_str_radix(&val[7..], 16).unwrap() >= 128)
                        } else {
                            (val, false)
                        };
                        m.insert("bg_color".into(), json!(rgb));
                        m.insert("bg_opaque".into(), json!(opaque));
                    }
                }
                rest = tail.into();
            }
            m.insert("text".into(), json!(unescape(&rest)));
        } else {
            let p: Vec<_> = value.split(',').map(str::trim).collect();
            let mut start = 4;
            if matches!(kind, "path" | "polygon") {
                let mut values = Vec::new();
                for s in &p {
                    if let Ok(v) = s.parse::<f64>() {
                        values.push(v);
                    } else {
                        break;
                    }
                }
                if values.len() % 2 == 1 && values.last() == Some(&0.) {
                    values.pop();
                }
                start = values.len();
                if start < if kind == "polygon" { 6 } else { 4 } || start % 2 != 0 {
                    return Err(Error::input("bad path/polygon coordinate list"));
                }
                if values.iter().any(|v| !v.is_finite()) {
                    return Err(Error::input("coordinates must be finite"));
                }
                m.insert(
                    "points".into(),
                    json!(values.chunks_exact(2).collect::<Vec<_>>()),
                );
            } else {
                if p.len() < 4 {
                    return Err(Error::input("expects X1,Y1,X2,Y2"));
                }
                m.insert("a".into(), json!([number(p[0])?, number(p[1])?]));
                m.insert("b".into(), json!([number(p[2])?, number(p[3])?]));
            }
            let mut tail = p[start..].iter();
            if kind != "ruler" {
                if let Some(&c) = tail.next() {
                    if c == "0" && kind != "path" {
                        if kind == "line" {
                            return Err(Error::input("line needs a color"));
                        }
                        m.insert("outline".into(), json!(false));
                    } else if !c.is_empty() {
                        m.insert("color".into(), json!(c));
                    }
                }
                if !matches!(kind, "line" | "path") {
                    if let Some(&f) = tail.next().filter(|f| !matches!(**f, "" | "0")) {
                        m.insert("fill".into(), json!(f));
                    }
                }
                if let Some(w) = tail.next() {
                    m.insert(
                        "width".into(),
                        json!(w
                            .parse::<i64>()
                            .map_err(|_| Error::input("invalid width"))?),
                    );
                }
                if let Some(d) = tail.next() {
                    m.insert("dash".into(), json!(d));
                }
            }
            if tail.next().is_some() {
                return Err(Error::input("too many annotation fields"));
            }
        }
        Self::from_json(Value::Object(m))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub annotations: Vec<Annotation>,
    pub ppu: Option<f64>,
    pub unit: Option<String>,
    pub note: Option<String>,
    pub legend: Option<Vec<String>>,
}
impl Document {
    pub fn from_json(value: Value, cancelled: &AtomicUsize) -> Result<Self> {
        let mut doc = Self::default();
        let entries = match value {
            Value::Array(a) => a,
            Value::Object(mut m) => {
                doc.ppu = m
                    .remove("ppu")
                    .filter(|v| !v.is_null())
                    .map(|v| finite(&v))
                    .transpose()?;
                doc.unit = m
                    .remove("unit")
                    .filter(|v| !v.is_null())
                    .map(|v| text_value(&v).map(|s| s.trim().to_string()))
                    .transpose()?;
                doc.note = m
                    .remove("note")
                    .filter(|v| !v.is_null())
                    .map(|v| text_value(&v).map(str::to_string))
                    .transpose()?;
                if let Some(v) = m.remove("legend").filter(|v| !v.is_null()) {
                    let a = v
                        .as_array()
                        .filter(|a| !a.is_empty())
                        .ok_or_else(|| Error::input("legend needs a nonempty array"))?;
                    doc.legend = Some(
                        a.iter()
                            .map(|v| legend_line(text_value(v)?))
                            .collect::<Result<_>>()?,
                    );
                }
                let a = m.remove("annotations").unwrap_or_else(|| json!([]));
                unknown(&m)?;
                let Value::Array(a) = a else {
                    return Err(Error::input("annotations must be an array"));
                };
                a
            }
            _ => return Err(Error::input("JSON document must be an array or object")),
        };
        if entries.len() > MAX_ANNOTATIONS {
            return Err(Error::input("metadata exceeds 100k annotations"));
        }
        for value in entries {
            check_cancelled(cancelled)?;
            doc.annotations.push(Annotation::from_json(value)?);
        }
        doc.validate()?;
        Ok(doc)
    }
    pub fn validate(&self) -> Result<()> {
        if self.annotations.len() > MAX_ANNOTATIONS {
            return Err(Error::input("metadata exceeds 100k annotations"));
        }
        if self.ppu.is_some_and(|v| !v.is_finite() || v <= 0.) {
            return Err(Error::input("ppu must be finite and positive"));
        }
        if let Some(unit) = &self.unit {
            safe_text(unit, false)?;
            if unit.trim().is_empty() {
                return Err(Error::input("empty unit"));
            }
        }
        if let Some(note) = &self.note {
            safe_text(note, true)?;
            if note.trim().is_empty() {
                return Err(Error::input("empty note"));
            }
        }
        if let Some(legend) = &self.legend {
            if legend.is_empty() {
                return Err(Error::input("empty legend"));
            }
            for line in legend {
                legend_line(line)?;
            }
        }
        Ok(())
    }
    pub fn empty(&self) -> bool {
        self.annotations.is_empty()
            && self.ppu.is_none()
            && self.note.is_none()
            && self.legend.is_none()
    }
    pub fn serialize(
        &self,
        existing: Option<&str>,
        append: bool,
        cancelled: &AtomicUsize,
    ) -> Result<Option<String>> {
        self.validate()?;
        let mut out = String::from("# flateyes annotations\n");
        let mut line = |s: &str| -> Result<()> {
            check_cancelled(cancelled)?;
            if out
                .len()
                .checked_add(s.len() + 1)
                .is_none_or(|n| n > MAX_TEXT)
            {
                return Err(Error::input("metadata exceeds 16 MiB"));
            }
            out.push_str(s);
            out.push('\n');
            Ok(())
        };
        if let Some(ppu) = self.ppu {
            line(&format!("ppu={}", coord(ppu)))?;
            line(&format!("unit={}", self.unit.as_deref().unwrap_or("um")))?;
        }
        if let Some(note) = &self.note {
            line(&format!("note={}", escape(note.trim())))?;
        }
        if let Some(legend) = &self.legend {
            for entry in legend {
                line(&format!("legend={}", legend_line(entry)?))?;
            }
        }
        if append {
            for raw in existing
                .unwrap_or("")
                .split([
                    '\n', '\r', '\u{000b}', '\u{000c}', '\u{001c}', '\u{001d}', '\u{001e}',
                    '\u{0085}', '\u{2028}', '\u{2029}',
                ])
                .map(str::trim)
            {
                if raw.is_empty()
                    || raw.starts_with('#')
                    || (self.ppu.is_some() && (raw.starts_with("ppu=") || raw.starts_with("unit=")))
                    || (self.note.is_some() && raw.starts_with("note="))
                    || (self.legend.is_some() && raw.starts_with("legend="))
                {
                    continue;
                }
                line(raw)?;
            }
        }
        for a in &self.annotations {
            line(a.as_line())?;
        }
        Ok((out != "# flateyes annotations\n").then_some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_document_and_append_contract() {
        let flag = AtomicUsize::new(0);
        let a = Annotation::option("ruler", "0,0,1,1").unwrap();
        let too_many = Document {
            annotations: vec![a.clone(); MAX_ANNOTATIONS + 1],
            ..Document::default()
        };
        assert!(too_many.validate().is_err());
        let too_long = Document {
            note: Some("x".repeat(MAX_TEXT)),
            ..Document::default()
        };
        assert!(too_long.serialize(None, false, &flag).is_err());
        let doc = Document {
            ppu: Some(2.),
            note: Some("새 메모".into()),
            annotations: vec![a],
            ..Document::default()
        };
        let old = "# old\nppu=1\nunit=nm\nnote=old\nlegend=box red solid kept\nunknown=keep\nruler=1,1,2,2\n";
        assert_eq!(doc.serialize(Some(old),true,&flag).unwrap().unwrap(),"# flateyes annotations\nppu=2\nunit=um\nnote=새 메모\nlegend=box red solid kept\nunknown=keep\nruler=1,1,2,2\nruler=0,0,1,1\n");
        assert!(Annotation::option("text", "0,0,hidden\u{2028}line").is_err());
        flag.store(2, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            doc.serialize(Some(old), true, &flag).unwrap_err().kind,
            crate::ErrorKind::Cancelled
        );
    }
    #[test]
    fn format_and_field_contracts() {
        assert_eq!(coord(-0.), "-0");
        assert_eq!(coord(1e-5), "1e-05");
        assert_eq!(coord(1e10), "1e+10");
        assert_eq!(coord(9999999999.5), "1e+10");
        let a = Annotation::option("polygon", "0,0,10,0,10,10,0,#aabbcc80:brick,2,dashed").unwrap();
        assert_eq!(a.as_line(), "polygon=0,0,10,0,10,10,0,#aabbcc80:brick,2,1");
        assert_eq!(
            legend_line("box sky clear NW 3/0").unwrap(),
            "box #35C5FF none NW 3/0"
        );
        assert_eq!(unescape(&escape("메모\\n\nline")), "메모\\n\nline");
        for v in [
            json!({"kind":"ruler","a":[0,0],"b":[1,1],"color":"red"}),
            json!({"kind":"text","at":[0,0],"text":""}),
            json!({"kind":"line","a":[0,0],"b":[1,1],"width":9}),
        ] {
            assert!(Annotation::from_json(v).is_err());
        }
        assert!(Annotation::option("line", "NaN,0,1,1").is_err());
        assert!(Annotation::option("box", "0,0,1,1,0").is_err());
    }
}
