//! Read-only ASCII fallback. Keep bounded offsets/bboxes, not the source text
//! or every coordinate. No offset sidecar, implicit pack build, or mmap.
use super::{pack::Input, META_BYTES, RECORD_POINTS};
use crate::{check_cancelled, Error, ErrorKind, Result};
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read},
    mem::size_of,
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    sync::atomic::AtomicUsize,
};

const LINE_BYTES: usize = 1024 * 1024;
#[derive(Clone, Debug)]
pub struct AsciiCheck {
    pub name: String,
    pub desc: String,
    /// Advisory integers are strings so enormous or negative declarations do
    /// not drive allocations or lose information. Actual counts are u64.
    pub declared: String,
    pub original: String,
    pub start: u64,
    pub count: u64,
    pub bbox_um: Option<[f64; 4]>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AsciiViolation {
    pub kind: char,
    pub number: u64,
    pub points_um: Vec<[f64; 2]>,
    pub bbox_um: [f64; 4],
}
#[derive(Clone, Copy)]
struct Record {
    kind: char,
    begin: u64,
    end: u64,
    points: usize,
    bbox_um: [f64; 4],
}
pub struct Ascii {
    input: Input,
    pub path: PathBuf,
    pub cell: String,
    pub precision: f64,
    pub checks: Vec<AsciiCheck>,
    pub total: u64,
    pub truncated_records: u64,
    records: Vec<Record>,
    // At most one complete record (<=4 MiB) for paged outline/CD reads.
    cached: Option<(usize, u64, AsciiViolation)>,
}
fn limit(what: &str) -> Error {
    Error::new(
        ErrorKind::Incomplete,
        format!("ASCII DRC {what} exceeds the read limit; use a smaller source or explicitly index an integral-DBU source with floe-index drc"),
    )
}
fn whitespace(c: char) -> bool {
    // Python str.split/strip also recognize the four C0 separators.
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}
fn tokens(s: &str) -> impl Iterator<Item = &str> {
    s.split(whitespace).filter(|s| !s.is_empty())
}
fn integer(s: &str) -> Option<String> {
    let negative = s.starts_with('-');
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty()
        || !digits.as_bytes().first()?.is_ascii_digit()
        || !digits.as_bytes().last()?.is_ascii_digit()
        || digits.as_bytes().windows(2).any(|p| p == b"__")
        || !digits.bytes().all(|c| c.is_ascii_digit() || c == b'_')
    {
        return None;
    }
    let clean = digits.replace('_', "");
    let clean = clean.trim_start_matches('0');
    Some(if clean.is_empty() {
        "0".into()
    } else {
        format!("{}{clean}", if negative { "-" } else { "" })
    })
}
fn count(s: &str) -> u64 {
    if s.starts_with('-') {
        0
    } else {
        s.parse().unwrap_or(u64::MAX)
    }
}
fn geom(s: &str) -> Option<(char, &str, &str)> {
    let mut words = tokens(s);
    let kind = words.next()?;
    let mut chars = kind.chars();
    let k = chars.next()?;
    if !k.is_alphabetic() || chars.next().is_some() {
        return None;
    }
    let ordinal = words.next()?;
    let size = words.next()?;
    let intish = |s: &str| {
        let d = s.trim_start_matches('-');
        !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit())
    };
    if !intish(ordinal) || !intish(size) {
        return None;
    }
    // Repeated '-' passes the legacy header predicate but float/int parsing
    // rejects it. Do not turn a malformed header into a gigantic allocation.
    Some((k.to_ascii_lowercase(), ordinal, size))
}
fn number(s: &str) -> Option<f64> {
    if s.contains('_') {
        let b = s.as_bytes();
        if b.iter().enumerate().any(|(i, &v)| {
            v == b'_'
                && (i == 0
                    || i + 1 == b.len()
                    || !b[i - 1].is_ascii_digit()
                    || !b[i + 1].is_ascii_digit())
        }) {
            return None;
        }
        s.replace('_', "").parse().ok()
    } else {
        s.parse().ok()
    }
}
// A private pread cursor: independent readers never share a seek position.
struct Range<'a> {
    file: &'a File,
    pos: u64,
    end: u64,
}
impl Read for Range<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = out
            .len()
            .min((self.end - self.pos).min(usize::MAX as u64) as usize);
        if n == 0 {
            return Ok(0);
        }
        let got = self.file.read_at(&mut out[..n], self.pos)?;
        if got == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "DRC source truncated",
            ));
        }
        self.pos += got as u64;
        Ok(got)
    }
}
struct Line {
    text: String,
    begin: u64,
}
struct Lines<'a> {
    reader: BufReader<Range<'a>>,
    pos: u64,
    line: Option<Line>,
    cap: usize,
}
impl<'a> Lines<'a> {
    fn new(input: &'a Input, begin: u64, end: u64, cap: usize) -> Self {
        Self {
            reader: BufReader::with_capacity(
                128 * 1024,
                Range {
                    file: &input.file,
                    pos: begin,
                    end,
                },
            ),
            pos: begin,
            line: None,
            cap,
        }
    }
    fn fill(&mut self, stop: &AtomicUsize) -> Result<bool> {
        if self.line.is_some() {
            return Ok(true);
        }
        let begin = self.pos;
        let mut bytes = Vec::new();
        loop {
            check_cancelled(stop)?;
            let data = self.reader.fill_buf()?;
            if data.is_empty() {
                break;
            }
            let n = data
                .iter()
                .position(|&c| c == b'\n' || c == b'\r')
                .unwrap_or(data.len());
            if bytes.len().checked_add(n).is_none_or(|n| n > self.cap) {
                return Err(limit("line"));
            }
            bytes.try_reserve(n).map_err(|_| limit("line allocation"))?;
            bytes.extend_from_slice(&data[..n]);
            let terminator = data.get(n).copied();
            let taken = n + usize::from(terminator.is_some());
            self.reader.consume(taken);
            self.pos += taken as u64;
            if let Some(c) = terminator {
                if c == b'\r' && self.reader.fill_buf()?.first() == Some(&b'\n') {
                    self.reader.consume(1);
                    self.pos += 1;
                }
                break;
            }
        }
        if self.pos == begin {
            return Ok(false);
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if text.len() > self.cap {
            return Err(limit("decoded line"));
        }
        self.line = Some(Line { text, begin });
        Ok(true)
    }
    fn peek(&mut self, stop: &AtomicUsize) -> Result<Option<&Line>> {
        self.fill(stop)?;
        Ok(self.line.as_ref())
    }
    fn take(&mut self, stop: &AtomicUsize) -> Result<Option<Line>> {
        self.fill(stop)?;
        Ok(self.line.take())
    }
    fn position(&self) -> u64 {
        self.line.as_ref().map_or(self.pos, |l| l.begin)
    }
}
fn coordinates(
    line: &str,
    precision: f64,
    keep: bool,
    stop: &AtomicUsize,
) -> Result<Option<Vec<[f64; 2]>>> {
    let mut values = Vec::new();
    for (i, token) in tokens(line).enumerate() {
        if i % 1024 == 0 {
            check_cancelled(stop)?;
        }
        let Some(v) = number(token) else {
            return Ok(None);
        };
        if keep && (!v.is_finite() || !(v / precision).is_finite()) {
            return Err(Error::input("non-finite ASCII DRC coordinate"));
        }
        values
            .try_reserve(1)
            .map_err(|_| limit("coordinate allocation"))?;
        values.push(v);
    }
    if values.len() < 2 {
        return Ok(None);
    }
    if !keep {
        return Ok(Some(Vec::new()));
    }
    let mut points = Vec::new();
    points
        .try_reserve_exact(values.len() / 2)
        .map_err(|_| limit("coordinate allocation"))?;
    for p in values.chunks_exact(2) {
        points.push([p[0] / precision, p[1] / precision]);
    }
    Ok(Some(points))
}
fn charge(used: &mut usize, n: usize, cap: usize) -> Result<()> {
    *used = used
        .checked_add(n)
        .filter(|v| *v <= cap)
        .ok_or_else(|| limit("metadata"))?;
    Ok(())
}
fn push<T>(items: &mut Vec<T>, value: T, used: &mut usize, cap: usize) -> Result<()> {
    if items.len() == items.capacity() {
        let extra = ((cap - *used) / size_of::<T>()).min(1024);
        if extra == 0 {
            return Err(limit("metadata"));
        }
        let before = items.capacity();
        items
            .try_reserve_exact(extra)
            .map_err(|_| limit("metadata allocation"))?;
        charge(used, (items.capacity() - before) * size_of::<T>(), cap)?;
    }
    items.push(value);
    Ok(())
}
impl Ascii {
    pub fn open(path: &Path, stop: &AtomicUsize) -> Result<Self> {
        Self::open_limits(path, stop, META_BYTES, LINE_BYTES, RECORD_POINTS)
    }
    fn open_limits(
        path: &Path,
        stop: &AtomicUsize,
        meta_cap: usize,
        line_cap: usize,
        points_cap: usize,
    ) -> Result<Self> {
        check_cancelled(stop)?;
        let input = Input::open(path)?;
        let mut lines = Lines::new(&input, 0, input.len(), line_cap);
        let header = loop {
            let line = lines
                .take(stop)?
                .ok_or_else(|| Error::input("empty ASCII DRC file"))?;
            if !line.text.trim_matches(whitespace).is_empty() {
                break line;
            }
        };
        let mut words = tokens(&header.text);
        let cell = words.next().unwrap().to_owned();
        let precision = words.next().and_then(number).unwrap_or(1000.);
        if !precision.is_finite() {
            return Err(Error::input("non-finite ASCII DRC precision"));
        }
        let precision = if precision > 0. { precision } else { 1000. };
        let mut used = cell.capacity();
        if used > meta_cap {
            return Err(limit("metadata"));
        }
        let mut checks = Vec::new();
        let mut records = Vec::new();
        let mut truncated_records = 0;
        while let Some(line) = lines.take(stop)? {
            let name = line.text.trim_matches(whitespace);
            if name.is_empty() {
                continue;
            }
            let ignored = name.starts_with("__RVE_") && name.ends_with("__");
            let mut c = AsciiCheck {
                name: name.into(),
                desc: String::new(),
                declared: "0".into(),
                original: "0".into(),
                start: records.len() as u64,
                count: 0,
                bbox_um: None,
            };
            if let Some(line) = lines.peek(stop)? {
                let ints: Vec<_> = tokens(&line.text).take(3).map_while(integer).collect();
                if let Some(first) = ints.first() {
                    c.declared = first.clone();
                    c.original = ints.get(1).cloned().unwrap_or_else(|| "0".into());
                    let n = ints.get(2).map_or(0, |s| count(s));
                    lines.take(stop)?;
                    for i in 0..n {
                        let Some(line) = lines.peek(stop)? else {
                            break;
                        };
                        if geom(&line.text).is_some() {
                            break;
                        }
                        let text = line.text.trim_matches(whitespace);
                        if c.desc
                            .len()
                            .checked_add(text.len() + 1)
                            .is_none_or(|v| v > line_cap)
                        {
                            return Err(limit("description"));
                        }
                        if i > 0 {
                            c.desc.push('\n');
                        }
                        c.desc.push_str(text);
                        lines.take(stop)?;
                    }
                }
            }
            while let Some(line) = lines.peek(stop)? {
                if tokens(&line.text).next().is_none() {
                    lines.take(stop)?;
                    continue;
                }
                let Some((kind, ordinal, n)) = geom(&line.text) else {
                    break;
                };
                integer(ordinal).ok_or_else(|| Error::input("invalid ASCII DRC ordinal"))?;
                let n = integer(n)
                    .ok_or_else(|| Error::input("invalid ASCII DRC coordinate-line count"))?;
                lines.take(stop)?;
                let begin = lines.position();
                let keep = !ignored && matches!(kind, 'p' | 'e');
                let mut bbox: Option<[f64; 4]> = None;
                let mut points = 0usize;
                let mut got = 0;
                while got < count(&n) {
                    let Some(line) = lines.peek(stop)? else {
                        break;
                    };
                    if tokens(&line.text).next().is_none() {
                        lines.take(stop)?;
                        continue;
                    }
                    let Some(values) = coordinates(&line.text, precision, keep, stop)? else {
                        break;
                    };
                    points = points
                        .checked_add(values.len())
                        .filter(|&n| n <= points_cap)
                        .ok_or_else(|| limit("record points"))?;
                    for [x, y] in values {
                        bbox = Some(bbox.map_or([x, y, x, y], |b| {
                            [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
                        }));
                    }
                    got += 1;
                    lines.take(stop)?;
                }
                if keep {
                    if got < count(&n) {
                        truncated_records += 1;
                    }
                    if let Some(bbox_um) = bbox {
                        c.bbox_um = Some(c.bbox_um.map_or(bbox_um, |b| {
                            [
                                b[0].min(bbox_um[0]),
                                b[1].min(bbox_um[1]),
                                b[2].max(bbox_um[2]),
                                b[3].max(bbox_um[3]),
                            ]
                        }));
                        push(
                            &mut records,
                            Record {
                                kind,
                                begin,
                                end: lines.position(),
                                points,
                                bbox_um,
                            },
                            &mut used,
                            meta_cap,
                        )?;
                        c.count += 1;
                    }
                }
            }
            if !ignored && !(c.name.ends_with("_RDBS") && c.count == 0) {
                charge(
                    &mut used,
                    c.name.capacity()
                        + c.desc.capacity()
                        + c.declared.capacity()
                        + c.original.capacity(),
                    meta_cap,
                )?;
                push(&mut checks, c, &mut used, meta_cap)?;
            }
        }
        drop(lines);
        input.unchanged()?;
        Ok(Self {
            input,
            path: path.to_owned(),
            cell,
            precision,
            total: records.len() as u64,
            checks,
            records,
            truncated_records,
            cached: None,
        })
    }
    pub fn unchanged(&self) -> Result<()> {
        self.input.unchanged()
    }
    pub fn error(&self, check: usize, local: u64, stop: &AtomicUsize) -> Result<AsciiViolation> {
        self.unchanged()?;
        check_cancelled(stop)?;
        let c = self
            .checks
            .get(check)
            .filter(|c| local < c.count)
            .ok_or_else(|| Error::input("ASCII DRC error index out of range"))?;
        let index = c
            .start
            .checked_add(local)
            .ok_or_else(|| Error::input("ASCII DRC record index overflow"))?;
        let r = usize::try_from(index)
            .ok()
            .and_then(|i| self.records.get(i))
            .ok_or_else(|| Error::input("ASCII DRC record index out of range"))?;
        let mut lines = Lines::new(&self.input, r.begin, r.end, LINE_BYTES);
        let mut points_um = Vec::new();
        points_um
            .try_reserve_exact(r.points)
            .map_err(|_| limit("record allocation"))?;
        while let Some(line) = lines.take(stop)? {
            if tokens(&line.text).next().is_none() {
                continue;
            }
            let p = coordinates(&line.text, self.precision, true, stop)?
                .ok_or_else(|| Error::new(ErrorKind::Cache, "ASCII DRC record changed"))?;
            if points_um.len() + p.len() > r.points {
                return Err(Error::new(
                    ErrorKind::Cache,
                    "ASCII DRC point count changed",
                ));
            }
            points_um.extend(p);
        }
        if points_um.len() != r.points {
            return Err(Error::new(
                ErrorKind::Cache,
                "ASCII DRC point count changed",
            ));
        }
        self.unchanged()?;
        Ok(AsciiViolation {
            kind: r.kind,
            number: index + 1,
            points_um,
            bbox_um: r.bbox_um,
        })
    }
}

mod query;
#[cfg(test)]
mod tests;
