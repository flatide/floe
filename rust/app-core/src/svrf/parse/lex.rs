//! Small lexical predicates for the *existing* line-oriented subset. No Tcl,
//! shell, macro expansion, expression evaluation, or geometry interpretation.
use super::{limit, Result, MAX_TEXT};
use std::{borrow::Cow, collections::BTreeMap};

pub(super) fn whitespace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}
pub(super) fn trim(s: &str) -> &str {
    s.trim_matches(whitespace)
}
pub(super) fn tokens(s: &str) -> impl Iterator<Item = &str> {
    s.split(whitespace).filter(|s| !s.is_empty())
}
pub(super) fn head_rest(s: &str) -> (&str, &str) {
    let s = s.trim_start_matches(whitespace);
    s.find(whitespace).map_or((s, ""), |i| {
        (&s[..i], s[i..].trim_start_matches(whitespace))
    })
}
pub(super) fn unquote(s: &str) -> &str {
    if s.len() >= 2
        && matches!(s.as_bytes()[0], b'\'' | b'"')
        && s.as_bytes().last() == s.as_bytes().first()
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}
pub(super) fn delta(s: &str) -> i64 {
    s.bytes().filter(|&b| b == b'{').count() as i64
        - s.bytes().filter(|&b| b == b'}').count() as i64
}
pub(super) fn metric(s: &str) -> Option<&'static str> {
    Some(match s {
        "INTERNAL" | "INT" => "width",
        "EXTERNAL" | "EXT" => "space",
        "ENCLOSURE" | "ENC" => "enclosure",
        "AREA" => "area",
        "DENSITY" => "density",
        "LENGTH" => "length",
        "ANGLE" => "angle",
        "PERIMETER" => "perimeter",
        "VERTEX" => "vertex",
        _ => return None,
    })
}
pub(super) fn ignored(s: &str) -> bool {
    "PRECISION RESOLUTION TITLE DRC LAYOUT TEXT CONNECT SCONNECT VIRTUAL ATTACH MASK LVS ERC PEX SOURCE GROUND UNIT FLAG GROUP PORT EXCLUDE CAPACITANCE RESISTANCE DEVICE TRACE SVRF PUSHDOWN POLYGON FILTER DFM RDB DVPARAMS OFFGRID NET FLATTEN"
        .split_ascii_whitespace().any(|k| k == s)
}
pub(super) fn noncheck(s: &str) -> bool {
    matches!(
        s,
        "VERBATIM" | "IF" | "ELSE" | "ELSEIF" | "FOREACH" | "WHILE" | "PROC" | "SWITCH"
    )
}
pub(super) fn check(s: &str) -> Option<(&str, &str)> {
    let quote = s.starts_with('"');
    let text = if quote { &s[1..] } else { s };
    let n = text
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || b"_.-$".contains(b))
        .count();
    if n == 0 {
        return None;
    }
    let rest = &text[n..];
    let rest = if quote { rest.strip_prefix('"')? } else { rest };
    Some((
        &text[..n],
        rest.trim_start_matches(whitespace).strip_prefix('{')?,
    ))
}
pub(super) fn assign(s: &str) -> Option<(&str, &str)> {
    if !s
        .as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
    {
        return None;
    }
    let n = s
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || b"_.-".contains(b))
        .count();
    let rest = s[n..]
        .trim_start_matches(whitespace)
        .strip_prefix('=')?
        .trim_start_matches(whitespace);
    (!rest.is_empty()).then_some((&s[..n], rest))
}
pub(super) fn op(s: &str, pos: usize) -> Option<&str> {
    if pos > 0 && (s.as_bytes()[pos - 1].is_ascii_alphabetic() || s.as_bytes()[pos - 1] == b'_') {
        return None;
    }
    let rest = s.get(pos..)?;
    ["<=", ">=", "==", "!=", "<", ">"]
        .into_iter()
        .find(|o| rest.starts_with(o))
}
pub(super) fn first_op(s: &str) -> Option<usize> {
    s.char_indices().find_map(|(i, _)| op(s, i).map(|_| i))
}
pub(super) fn chain(s: &str, mut pos: usize) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    while let Some(op) = op(s, pos) {
        let rest = s[pos + op.len()..].trim_start_matches(whitespace);
        let n = rest
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || b"_.-+".contains(b))
            .count();
        if n == 0 {
            break;
        }
        out.push((op, &rest[..n]));
        pos = s.len() - rest[n..].trim_start_matches(whitespace).len();
    }
    out
}
pub(super) fn numeric(s: &str) -> Result<Option<f64>> {
    let normalized = decimal_digits(s);
    let s = normalized.as_ref();
    // Match the decimal/exponent grammar before float parsing (no NaN/inf,
    // underscores or Rust's other spellings). Nonfinite *decimal* is an error.
    let b = s.as_bytes();
    let mut i = usize::from(b.first().is_some_and(|b| b"+-".contains(b)));
    let start = i;
    while b.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    let mut digits = i - start;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        digits += i - start;
    }
    if digits == 0 {
        return Ok(None);
    }
    if b.get(i).is_some_and(|b| b"eE".contains(b)) {
        i += 1;
        if b.get(i).is_some_and(|b| b"+-".contains(b)) {
            i += 1;
        }
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return Ok(None);
        }
    }
    if i != b.len() {
        return Ok(None);
    }
    let n: f64 = s.parse().map_err(|_| limit("unrepresentable number"))?;
    if !n.is_finite() {
        return Err(limit("nonfinite number"));
    }
    Ok(Some(n))
}
pub(super) fn unsigned(s: &str) -> Result<Option<i64>> {
    let normalized = decimal_digits(s);
    let s = normalized.as_ref();
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    s.parse()
        .map(Some)
        .map_err(|_| limit("layer integer outside signed 64-bit range"))
}
pub(super) fn signed(s: &str) -> Result<Option<i64>> {
    let normalized = decimal_digits(s);
    let s = normalized.as_ref();
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    s.parse()
        .map(Some)
        .map_err(|_| limit("layer integer outside signed 64-bit range"))
}

fn decimal_digits(s: &str) -> Cow<'_, str> {
    if s.is_ascii() {
        return Cow::Borrowed(s);
    }
    // Python regex \d / float / int accept Unicode Nd, not other numeric
    // characters (e.g. fractions). Zero codepoints, Unicode 16.0 Nd ranges.
    // Keep the ASCII hot path allocation-free; each range contains 10 digits.
    const ZEROES: &[u32] = &[
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
        0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
        0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
        0xff10, 0x104a0, 0x10d30, 0x10d40, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450,
        0x114d0, 0x11650, 0x116c0, 0x116d0, 0x116da, 0x11730, 0x118e0, 0x11950, 0x11bf0, 0x11c50,
        0x11d50, 0x11da0, 0x11f50, 0x16130, 0x16a60, 0x16ac0, 0x16b50, 0x16d70, 0x1ccf0, 0x1d7ce,
        0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e5f1, 0x1e950, 0x1fbf0,
    ];
    Cow::Owned(
        s.chars()
            .map(|c| {
                let n = c as u32;
                let i = ZEROES.partition_point(|&z| z <= n);
                if i > 0 && n - ZEROES[i - 1] < 10 {
                    char::from(b'0' + (n - ZEROES[i - 1]) as u8)
                } else {
                    c
                }
            })
            .collect(),
    )
}

#[derive(Default)]
struct Node {
    next: BTreeMap<char, usize>,
    value: Option<String>,
}
#[derive(Default)]
pub(super) struct Substitution {
    nodes: Vec<Node>,
}
fn word(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || c == '_')
}
impl Substitution {
    pub fn rebuild(&mut self, defines: &BTreeMap<String, Option<String>>) -> Result<()> {
        self.nodes = vec![Node::default()];
        for (name, value) in defines {
            self.set(name, value.as_deref())?;
        }
        Ok(())
    }
    pub fn set(&mut self, name: &str, value: Option<&str>) -> Result<()> {
        let value = value.filter(|v| !v.is_empty());
        if self.nodes.is_empty() {
            self.nodes.push(Node::default());
        }
        let mut at = 0;
        for c in name.chars() {
            at = if let Some(&next) = self.nodes[at].next.get(&c) {
                next
            } else {
                if value.is_none() {
                    return Ok(());
                }
                if self.nodes.len() == 262144 {
                    return Err(limit("define trie exceeds 262144 nodes"));
                }
                let next = self.nodes.len();
                self.nodes.push(Node::default());
                self.nodes[at].next.insert(c, next);
                next
            };
        }
        self.nodes[at].value = value.map(str::to_owned);
        Ok(())
    }
    pub fn apply(&self, s: &str) -> Result<String> {
        let mut out = String::new();
        let mut pos = 0;
        let mut previous = None;
        let mut steps = 0;
        while pos < s.len() {
            let c = s[pos..].chars().next().unwrap();
            let mut best = None;
            if !self.nodes.is_empty() && word(previous) != word(Some(c)) {
                let mut at = 0;
                for (i, c) in s[pos..].char_indices() {
                    steps += 1;
                    if steps > 8 * 1024 * 1024 {
                        return Err(limit("define match work exceeds 8M steps per line"));
                    }
                    let Some(&next) = self.nodes[at].next.get(&c) else {
                        break;
                    };
                    at = next;
                    let end = pos + i + c.len_utf8();
                    if let Some(value) = &self.nodes[at].value {
                        if word(Some(c)) != word(s[end..].chars().next()) {
                            best = Some((end, value));
                        }
                    }
                }
            }
            let (end, value) = best.map_or(
                (pos + c.len_utf8(), &s[pos..pos + c.len_utf8()]),
                |(end, v)| (end, v.as_str()),
            );
            if out.len() + value.len() > MAX_TEXT {
                return Err(limit("expanded line exceeds 64 KiB"));
            }
            out.push_str(value);
            previous = s[..end].chars().next_back();
            pos = end;
        }
        Ok(out)
    }
}
