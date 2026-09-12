use crate::{check_cancelled, Error, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::AtomicUsize;

// Input bounds are explicit errors, including in lenient mode. No geometry
// is loaded here and ROWS x entries is never materialized by the parser.
pub const MAX_DECK_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Entry {
    pub idx: i64,
    pub name: String,
    pub ad: Option<f64>,
    pub sf: f64,
    pub tc: String,
    pub ly: Vec<i64>,
    pub dt: Vec<i64>,
    pub bx: f64,
    pub by: f64,
    pub ux: f64,
    pub uy: f64,
    pub extra: BTreeMap<String, Value>,
    pub lineno: usize,
}
impl Entry {
    fn new(idx: i64, lineno: usize) -> Self {
        Self {
            idx,
            lineno,
            name: String::new(),
            ad: None,
            sf: 1.0,
            tc: String::new(),
            ly: Vec::new(),
            dt: Vec::new(),
            bx: 0.0,
            by: 0.0,
            ux: 0.0,
            uy: 0.0,
            extra: BTreeMap::new(),
        }
    }
    fn same_definition(&self, other: &Self) -> bool {
        // Unknown keys and line numbers do not affect the observed placement
        // signature. A repeated level may have different AD/SF/TC in each CHIP.
        self.name == other.name
            && self.ad == other.ad
            && self.sf == other.sf
            && self.tc == other.tc
            && self.ly == other.ly
            && self.dt == other.dt
            && self.bx == other.bx
            && self.by == other.by
            && self.ux == other.ux
            && self.uy == other.uy
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Chip {
    pub id: String,
    pub entries: Vec<Entry>,
    /// Y FIRST, X SECOND, shared by every entry in this CHIP block.
    pub rows: Vec<(f64, f64)>,
    pub lineno: usize,
    pub tail: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct JobDeck {
    pub header: BTreeMap<String, String>,
    pub option_flags: Vec<String>,
    pub options: BTreeMap<String, Value>,
    pub mtitles: BTreeMap<i64, String>,
    pub comments: Vec<(usize, String)>,
    pub chips: Vec<Chip>,
    pub unknown: Vec<(usize, String)>,
    pub errors: Vec<(usize, String)>,
    pub warnings: Vec<(usize, String)>,
    pub path: String,
}
impl JobDeck {
    pub fn read(path: &Path, strict: bool, cancelled: &AtomicUsize) -> Result<Self> {
        check_cancelled(cancelled)?;
        let mut f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        let meta = f.metadata()?;
        if !meta.is_file() || meta.len() > MAX_DECK_BYTES as u64 {
            return Err(Error::input(
                "jobdeck must be a regular file at most 64 MiB",
            ));
        }
        let mut bytes = Vec::new();
        let mut chunk = [0; 64 * 1024];
        loop {
            check_cancelled(cancelled)?;
            let n = f.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            if bytes.len() + n > MAX_DECK_BYTES {
                return Err(Error::input("jobdeck exceeds 64 MiB"));
            }
            bytes.extend_from_slice(&chunk[..n]);
        }
        // Match the legacy parser's errors="replace" for old text decks.
        Self::parse(
            path.to_str()
                .ok_or_else(|| Error::input("jobdeck path must be UTF-8"))?,
            &String::from_utf8_lossy(&bytes),
            strict,
            cancelled,
        )
    }

    pub fn parse(path: &str, text: &str, strict: bool, cancelled: &AtomicUsize) -> Result<Self> {
        if text.len() > MAX_DECK_BYTES {
            return Err(Error::input("jobdeck exceeds 64 MiB"));
        }
        let mut deck = Self {
            path: path.into(),
            ..Self::default()
        };
        for (n, raw) in text.lines().enumerate() {
            check_cancelled(cancelled)?;
            let line_no = n + 1;
            if raw.len() > MAX_LINE_BYTES {
                return Err(Error::input(format!(
                    "line {line_no}: jobdeck line exceeds 1 MiB"
                )));
            }
            let line = raw.trim();
            if line.eq_ignore_ascii_case("END") {
                break;
            }
            if let Err(error) = deck.line(line_no, line) {
                deck.errors.push((line_no, error.message));
            }
        }
        deck.post_checks();
        if strict && !deck.errors.is_empty() {
            let detail = deck
                .errors
                .iter()
                .map(|(n, s)| format!("  line {n}: {s}"))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(Error::input(format!("{path}: {} structural error(s); refusing to return a deck that would place geometry wrongly.\n{detail}", deck.errors.len())));
        }
        Ok(deck)
    }

    fn line(&mut self, n: usize, line: &str) -> Result<()> {
        if line.is_empty() {
            return Ok(());
        }
        if line.starts_with('*') {
            let directive = line.split_whitespace().next().unwrap_or("");
            if !["*PLACE-INFO", "*END-PLACE"]
                .iter()
                .any(|s| directive.eq_ignore_ascii_case(s))
            {
                let comment = line.trim_start_matches('*').trim();
                if !comment.is_empty() {
                    self.comments.push((n, comment.into()));
                }
            }
            return Ok(());
        }
        if let Some(body) = keyword(line, "OPTION") {
            for t in split_top_level(body)? {
                if let Some((key, value)) = t.split_once('=') {
                    let value = value.trim();
                    self.options.insert(
                        key.trim().to_uppercase(),
                        finite(value).map_or_else(|_| json!(value), |v| json!(v)),
                    );
                } else {
                    self.option_flags.push(t.to_uppercase());
                }
            }
            return Ok(());
        }
        if let Some(body) = keyword(line, "MTITLE") {
            let tokens = split_top_level(body)?;
            if let Some(first) = tokens.first() {
                let (id, title) = match integer(first) {
                    Ok(id) => (id, tokens.get(1).copied().unwrap_or("")),
                    Err(_) => (self.mtitles.len() as i64 + 1, *first),
                };
                self.mtitles.insert(id, title.into());
            }
            return Ok(());
        }
        if let Some(body) = keyword(line, "CHIP") {
            let end = body
                .find(|c: char| c == ',' || c.is_whitespace())
                .unwrap_or(body.len());
            let id = &body[..end];
            let tail = body[end..]
                .trim_start()
                .strip_prefix(',')
                .unwrap_or(body[end..].trim_start())
                .trim();
            if id.is_empty() {
                self.errors
                    .push((n, format!("CHIP line without an id: {line}")));
            }
            self.chips.push(Chip {
                id: id.into(),
                tail: tail.into(),
                lineno: n,
                rows: Vec::new(),
                entries: Vec::new(),
            });
            return Ok(());
        }
        if let Some(body) = line.strip_prefix('$') {
            let body = body.trim_start();
            if self.chips.is_empty() || !body.starts_with(['(', '{']) {
                self.unknown.push((n, line.into()));
                return Ok(());
            }
            if !body.ends_with([')', '}']) {
                return Err(Error::input(format!(
                    "unterminated $ entry (no closing ')'): {line}"
                )));
            }
            if !matches!(
                (body.as_bytes()[0], body.as_bytes()[body.len() - 1]),
                (b'(', b')') | (b'{', b'}')
            ) {
                return Err(Error::input("mismatched $ entry brackets"));
            }
            let entry = parse_entry(&body[1..body.len() - 1], n)?;
            self.chips
                .last_mut()
                .expect("CHIP checked")
                .entries
                .push(entry);
            return Ok(());
        }
        if let Some(body) = keyword(line, "ROWS") {
            if self.chips.is_empty() {
                self.unknown.push((n, line.into()));
                return Ok(());
            }
            // Parse the whole line before appending: a bad last token must not
            // create a partial placement list in lenient mode.
            let rows = body
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .map(|t| {
                    let (y, x) = t
                        .split_once('/')
                        .ok_or_else(|| Error::input(format!("bad ROWS token {}", quoted(t))))?;
                    Ok((finite(y)?, finite(x)?))
                })
                .collect::<Result<Vec<_>>>()?;
            self.chips
                .last_mut()
                .expect("CHIP checked")
                .rows
                .extend(rows);
            return Ok(());
        }
        if self.chips.is_empty() {
            let end = line.find(char::is_whitespace).unwrap_or(line.len());
            self.header
                .insert(line[..end].to_uppercase(), line[end..].trim().into());
        } else if orphan_key(line) {
            return Err(Error::input(format!(
                "orphan 'KEY=VALUE' line inside CHIP {} (wrapped $ entry?): {line}",
                self.chips.last().expect("CHIP checked").id
            )));
        } else {
            self.unknown.push((n, line.into()));
        }
        Ok(())
    }

    fn post_checks(&mut self) {
        let mut seen = BTreeMap::new();
        for chip in &self.chips {
            if let Some(first) = seen.get(&chip.id) {
                self.warnings.push((chip.lineno, format!("duplicate CHIP id {} (first at line {first}); chip-mode colours and per-chip listings merge them", quoted(&chip.id))));
            } else {
                seen.insert(&chip.id, chip.lineno);
            }
        }
        for chip in &self.chips {
            for e in &chip.entries {
                if e.ad.is_none() {
                    self.warnings.push((
                        e.lineno,
                        format!(
                            "chip {} ${}: AD missing; placement will raise",
                            chip.id, e.idx
                        ),
                    ));
                }
                if e.ux <= e.bx || e.uy <= e.by {
                    self.warnings.push((
                        e.lineno,
                        format!(
                            "chip {} ${}: degenerate bbox {}x{} um (BX..UY missing?)",
                            chip.id,
                            e.idx,
                            general(e.ux - e.bx),
                            general(e.uy - e.by)
                        ),
                    ));
                }
                if e.ly.is_empty() {
                    self.warnings.push((
                        e.lineno,
                        format!("chip {} ${}: no LY given", chip.id, e.idx),
                    ));
                }
            }
            if chip.rows.is_empty() {
                self.warnings.push((
                    chip.lineno,
                    format!("chip {}: no ROWS; nothing will be placed", chip.id),
                ));
            }
        }
    }

    pub fn name(&self) -> &str {
        self.comments
            .iter()
            .find(|(_, c)| c.to_ascii_lowercase().ends_with(".jb"))
            .map_or("", |(_, c)| c)
    }
    pub fn levels(&self) -> BTreeSet<i64> {
        self.chips
            .iter()
            .flat_map(|c| c.entries.iter().map(|e| e.idx))
            .collect()
    }
    pub fn sources(&self, selected: Option<&BTreeSet<i64>>) -> Vec<&str> {
        let mut seen = BTreeSet::new();
        self.chips
            .iter()
            .flat_map(|c| &c.entries)
            .filter(|e| selected.is_none_or(|ids| ids.contains(&e.idx)))
            .filter_map(|e| seen.insert(e.tc.as_str()).then_some(e.tc.as_str()))
            .collect()
    }
    pub fn title(&self, level: i64) -> &str {
        self.mtitles.get(&level).map_or("", String::as_str)
    }
    pub fn instance_count(&self) -> Result<u64> {
        self.chips.iter().try_fold(0u64, |n, c| {
            (c.rows.len() as u64)
                .checked_mul(c.entries.len() as u64)
                .and_then(|v| n.checked_add(v))
                .ok_or_else(|| Error::input("jobdeck instance count overflow"))
        })
    }
    pub fn varying_levels(&self) -> BTreeSet<i64> {
        let mut first: BTreeMap<i64, &Entry> = BTreeMap::new();
        self.chips
            .iter()
            .flat_map(|c| &c.entries)
            .filter_map(|e| match first.get(&e.idx) {
                Some(other) if !e.same_definition(other) => Some(e.idx),
                Some(_) => None,
                None => {
                    first.insert(e.idx, e);
                    None
                }
            })
            .collect()
    }
    pub fn coverage(&self) -> Result<Value> {
        let ids = self.levels();
        if ids
            .len()
            .checked_mul(self.chips.len())
            .is_none_or(|n| n > 4_000_000)
        {
            return Err(Error::input(
                "jobdeck coverage report exceeds 4M CHIP/level entries",
            ));
        }
        let mut complete = 0;
        let rows: Vec<_> = self
            .chips
            .iter()
            .map(|c| {
                let present: BTreeSet<_> = c.entries.iter().map(|e| e.idx).collect();
                let missing: Vec<_> = ids.difference(&present).copied().collect();
                complete += usize::from(missing.is_empty());
                json!({"chip":c.id, "present":present, "missing":missing, "rows":c.rows.len()})
            })
            .collect();
        Ok(
            json!({"identifiers":ids, "chips":rows, "complete":complete, "partial":self.chips.len()-complete}),
        )
    }
    pub fn report(&self) -> Result<Value> {
        let coverage = self.coverage()?;
        let ids = self.levels();
        let titles: BTreeSet<_> = self.mtitles.keys().copied().collect();
        let mut extras: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
        for c in &self.chips {
            for e in &c.entries {
                for (k, v) in &e.extra {
                    extras
                        .entry(k)
                        .or_default()
                        .push(json!({"chip":c.id, "idx":e.idx, "line":e.lineno, "value":v}));
                }
            }
        }
        let tails: BTreeMap<_, _> = self
            .chips
            .iter()
            .filter(|c| !c.tail.is_empty())
            .map(|c| (&c.id, &c.tail))
            .collect();
        let issues = |rows: &[(usize, String)]| {
            rows.iter()
                .map(|(n, msg)| json!({"line":n, "msg":msg}))
                .collect::<Vec<_>>()
        };
        Ok(json!({
            "path":self.path, "jb_name":self.name(),
            "not_interpreted":{"note":"parsed and preserved; nothing reads these and they do not affect placement",
                "header":self.header, "option_flags":self.option_flags, "options":self.options, "chip_line_tails":tails},
            "chip_lines":self.chips.iter().map(|c| json!({"chip":c.id, "line":c.lineno, "tail":c.tail, "entries":c.entries.len(), "rows":c.rows.len()})).collect::<Vec<_>>(),
            "undocumented_entry_fields":extras, "mtitles":self.mtitles,
            "title_check":{"identifiers":ids, "mtitle_keys":titles,
                "missing_title":ids.difference(&titles).collect::<Vec<_>>(), "unused_title":titles.difference(&ids).collect::<Vec<_>>()},
            "coverage":coverage, "identifiers_varying_across_chips":self.varying_levels(),
            "errors":issues(&self.errors), "warnings":issues(&self.warnings),
            "unparsed_lines":self.unknown.iter().map(|(n,s)| json!({"line":n,"text":s})).collect::<Vec<_>>()
        }))
    }
}

fn keyword<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    if !line.get(..name.len())?.eq_ignore_ascii_case(name) {
        return None;
    }
    let rest = &line[name.len()..];
    if rest.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        None
    } else {
        Some(rest.trim())
    }
}
fn orphan_key(line: &str) -> bool {
    let Some((key, _)) = line.split_once('=') else {
        return false;
    };
    let mut chars = key.trim_end().chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}
pub fn split_top_level(text: &str) -> Result<Vec<&str>> {
    let mut stack = Vec::new();
    let mut start = 0;
    let mut tokens = Vec::new();
    for (i, ch) in text.char_indices() {
        match ch {
            '{' | '(' | '[' => {
                if stack.len() == 64 {
                    return Err(Error::input("jobdeck bracket nesting exceeds 64"));
                }
                stack.push(ch);
            }
            '}' | ')' | ']' => {
                if !matches!(
                    (stack.pop(), ch),
                    (Some('{'), '}') | (Some('('), ')') | (Some('['), ']')
                ) {
                    return Err(Error::input("unbalanced jobdeck brackets"));
                }
            }
            ',' if stack.is_empty() => {
                let token = text[start..i].trim();
                if !token.is_empty() {
                    tokens.push(token);
                }
                start = i + 1;
            }
            _ => (),
        }
    }
    if !stack.is_empty() {
        return Err(Error::input("unbalanced jobdeck brackets"));
    }
    let token = text[start..].trim();
    if !token.is_empty() {
        tokens.push(token);
    }
    Ok(tokens)
}
fn finite(s: &str) -> Result<f64> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| Error::input(format!("invalid finite jobdeck number: {s}")))
}
pub fn integer(s: &str) -> Result<i64> {
    // The legacy grammar permits 1.0 and truncates fractional identifiers.
    // Reject out-of-range values rather than Rust's saturating float cast.
    let value = finite(s)?.trunc();
    if !(-((1u64 << 63) as f64)..(1u64 << 63) as f64).contains(&value) {
        return Err(Error::input(format!(
            "jobdeck integer out of i64 range: {s}"
        )));
    }
    Ok(value as i64)
}
pub fn parse_braced_list(s: &str) -> Result<Vec<i64>> {
    split_top_level(s.trim().trim_matches(['{', '}']).trim())?
        .into_iter()
        .map(integer)
        .collect()
}
fn parse_entry(body: &str, lineno: usize) -> Result<Entry> {
    let tokens = split_top_level(body)?;
    let Some(first) = tokens.first() else {
        return Err(Error::input("empty $ entry"));
    };
    let mut e = Entry::new(integer(first)?, lineno);
    let mut rest = &tokens[1..];
    if let Some(name) = rest.first().filter(|t| !t.contains('=')) {
        e.name = (*name).into();
        rest = &rest[1..];
    }
    for &t in rest {
        let Some((key, value)) = t.split_once('=') else {
            e.extra
                .entry("_positional".into())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .expect("positional list")
                .push(json!(t));
            continue;
        };
        let key = key.trim().to_uppercase();
        let value = value.trim();
        match key.as_str() {
            "AD" => e.ad = Some(finite(value)?),
            "SF" => e.sf = finite(value)?,
            "BX" => e.bx = finite(value)?,
            "BY" => e.by = finite(value)?,
            "UX" => e.ux = finite(value)?,
            "UY" => e.uy = finite(value)?,
            "LY" => e.ly = parse_braced_list(value)?,
            "DT" => e.dt = parse_braced_list(value)?,
            "TC" => e.tc = value.trim_matches(['\'', '"']).into(),
            _ => {
                e.extra.insert(key, json!(value));
            }
        }
    }
    Ok(e)
}
/// Legacy `%g` diagnostics (six significant digits), not coordinate wire
/// serialization. Scientific rounding decides the fixed/scientific switch.
pub(super) fn general(value: f64) -> String {
    let scientific = format!("{value:.5e}");
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return scientific;
    };
    let exponent: i32 = exponent.parse().expect("formatted exponent");
    if !(-4..6).contains(&exponent) {
        format!(
            "{}e{exponent:+03}",
            mantissa.trim_end_matches('0').trim_end_matches('.')
        )
    } else {
        let precision = (5 - exponent) as usize;
        let fixed = format!("{value:.precision$}");
        if precision == 0 {
            fixed
        } else {
            fixed.trim_end_matches('0').trim_end_matches('.').into()
        }
    }
}
fn quoted(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(q);
    for ch in s.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' => out.push_str("\\\\"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(text: &str, strict: bool) -> Result<JobDeck> {
        JobDeck::parse("sample.jb", text, strict, &AtomicUsize::new(0))
    }
    #[test]
    fn tokens_unknowns_and_y_first() {
        let d = parse("* 이름.jb\nOPTION PA, AA=0.2, OTHER=unknown\nMTITLE 1,한 글\nCHIP C1, * tail\n$ (1.0, A, TC='한 글.oas', AD=0.001, LY={7,8}, DT={0}, UX=2, UY=3, ZZ={9,10}, extra)\nROWS 10/20, 30/40\nEND\nignored=1", true).unwrap();
        assert_eq!(d.sources(None), ["한 글.oas"]);
        assert_eq!(d.chips[0].rows, [(10., 20.), (30., 40.)]);
        assert_eq!(d.instance_count().unwrap(), 2);
        assert_eq!(d.chips[0].entries[0].extra["ZZ"], "{9,10}");
        assert_eq!(d.chips[0].entries[0].extra["_positional"], json!(["extra"]));
        assert_eq!(d.name(), "이름.jb");
        assert_eq!(d.title(1), "한 글");
        assert_eq!(d.report().unwrap()["coverage"]["complete"], 1);
        assert!(d.unknown.is_empty());
    }
    #[test]
    fn broken_entries_are_never_partially_placed() {
        let input = "CHIP C\n$ (1, A, AD=1,\nUX=3,UY=4)\nROWS 1/2\n";
        let d = parse(input, false).unwrap();
        assert_eq!(d.errors.len(), 2);
        assert!(d.chips[0].entries.is_empty());
        assert!(parse(input, true).unwrap_err().message.contains("orphan"));
        for bad in ["AD=NaN", "SF=inf", "LY={1,2)", "LY={1e100}"] {
            assert!(
                parse(&format!("CHIP C\n$ (1,A,{bad})"), true).is_err(),
                "{bad}"
            );
        }
        let d = parse("CHIP C\nROWS 1/2, 3/no\n", false).unwrap();
        assert!(d.chips[0].rows.is_empty());
    }
    #[test]
    fn signatures_selection_and_duplicate_ids() {
        let d = parse("CHIP C\n$ (1,A,AD=1,TC=a,LY=1,UX=1,UY=1)\n$ (2,B,AD=1,TC=b,LY=2,UX=1,UY=1)\nROWS 0/0\nCHIP C\n$ (1,A,AD=2,TC=a,LY=1,UX=1,UY=1)\nROWS 0/0", true).unwrap();
        assert_eq!(d.varying_levels(), BTreeSet::from([1]));
        assert_eq!(d.sources(Some(&BTreeSet::from([2]))), ["b"]);
        assert_eq!(d.sources(Some(&BTreeSet::new())), Vec::<&str>::new());
        assert_eq!(d.warnings.len(), 1);
        assert_eq!(d.coverage().unwrap()["partial"], 1);
    }
    #[test]
    fn bounds_utf8_and_cancellation() {
        assert_eq!(
            split_top_level("a, {1,2}, (3,4), 한글").unwrap(),
            ["a", "{1,2}", "(3,4)", "한글"]
        );
        assert!(split_top_level(&"(".repeat(65)).is_err());
        assert!(integer("9223372036854775808").is_err());
        assert_eq!(integer("-9223372036854775808").unwrap(), i64::MIN);
        assert_eq!(integer("2.9").unwrap(), 2);
        assert!(parse(&"x".repeat(MAX_LINE_BYTES + 1), false).is_err());
        assert!(JobDeck::parse("x", "CHIP C", true, &AtomicUsize::new(2)).is_err());
        let mut large = String::new();
        for i in 0..2001 {
            large.push_str(&format!("CHIP C{i}\n$ ({i},A)\n"));
        }
        let d = parse(&large, true).unwrap();
        assert!(d.report().unwrap_err().message.contains("coverage report"));
    }
}
