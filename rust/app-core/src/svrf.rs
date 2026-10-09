//! `floe-svrf-rules` metadata, not an SVRF interpreter/signoff engine.
//! The reader never follows recorded paths. Only the explicit local `parse`
//! service reads source decks/includes; it is not a web registration endpoint.
pub mod parse;
use crate::{check_cancelled, drc::measured, Error, ErrorKind, Result};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    fs::{self, OpenOptions},
    io::Read,
    marker::PhantomData,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::atomic::AtomicUsize,
};

pub const MAX_RULES_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 65536;
const MAX_TEXT: usize = 65536;
pub const METRICS: &[&str] = &[
    "width",
    "space",
    "enclosure",
    "area",
    "density",
    "length",
    "angle",
    "perimeter",
    "vertex",
    "other",
];

// Apply collection limits while deserializing, not after an untrusted count
// has allocated a giant tree. Do not trust size_hint or accept duplicate keys.
fn list<'de, D, T, const N: usize>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct V<T, const N: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> de::Visitor<'de> for V<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "at most {N} SVRF entries")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(
            self,
            mut a: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(v) = a.next_element()? {
                if out.len() == N {
                    return Err(de::Error::custom("SVRF list limit exceeded"));
                }
                out.push(v);
            }
            Ok(out)
        }
    }
    d.deserialize_seq(V::<T, N>(PhantomData))
}
fn map<'de, D, T>(d: D) -> std::result::Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct V<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> de::Visitor<'de> for V<T> {
        type Value = BTreeMap<String, T>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a bounded SVRF object with unique names")
        }
        fn visit_map<A: de::MapAccess<'de>>(
            self,
            mut a: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut out = BTreeMap::new();
            while let Some(k) = a.next_key::<String>()? {
                if out.len() == MAX_ENTRIES
                    || k.is_empty()
                    || k.len() > MAX_TEXT
                    || out.contains_key(&k)
                {
                    return Err(de::Error::custom(
                        "SVRF map limit, invalid or duplicate name",
                    ));
                }
                out.insert(k, a.next_value()?);
            }
            Ok(out)
        }
    }
    d.deserialize_map(V::<T>(PhantomData))
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Constraint {
    pub metric: String,
    pub op: String,
    pub value: Option<f64>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct Rule {
    #[serde(default)]
    pub desc: String,
    #[serde(default, deserialize_with = "list::<_, _, 1024>")]
    pub constraints: Vec<Constraint>,
    #[serde(default, deserialize_with = "list::<_, _, 4096>")]
    pub layers: Vec<String>,
    #[serde(default, deserialize_with = "list::<_, _, 4096>")]
    pub source_gds: Vec<(u32, Option<u32>)>,
    #[serde(default, deserialize_with = "list::<_, _, 4096>")]
    pub unresolved: Vec<String>,
}
#[derive(Debug, Deserialize)]
struct Wire {
    format: String,
    version: u32,
    #[serde(deserialize_with = "map")]
    checks: BTreeMap<String, Rule>,
    #[serde(default, deserialize_with = "map")]
    derived: BTreeMap<String, String>,
}
#[derive(Debug)]
pub struct Rules {
    checks: BTreeMap<String, Rule>,
    derived: BTreeMap<String, String>,
}
#[derive(Debug, Serialize)]
pub struct Derivation<'a> {
    pub name: &'a str,
    pub rhs: &'a str,
}
#[derive(Debug, Serialize)]
pub struct Detail<'a> {
    pub rule: &'a Rule,
    pub metrics: Vec<&'a str>,
    pub derivations: Vec<Derivation<'a>>,
    pub derivations_more: bool,
}
#[derive(Debug, Serialize)]
pub struct TypeCount<'a> {
    pub metric: &'a str,
    pub checks: usize,
}
#[derive(Debug, Serialize)]
pub struct Catalog<'a> {
    pub matched: usize,
    pub checks: usize,
    pub types: Vec<TypeCount<'a>>,
}
/// Informational comparison only. No pass/fail verdict is inferred from a
/// Calibre violation marker or the subset parser's unresolved rule semantics.
#[derive(Debug, Serialize)]
pub struct Comparison<'a> {
    pub constraint: usize,
    pub metric: &'a str,
    pub op: &'a str,
    pub unit: &'static str,
    pub measured: f64,
    pub bound: f64,
    pub delta: f64,
    pub percent: Option<f64>,
}

fn order(values: BTreeSet<&str>) -> Vec<&str> {
    METRICS
        .iter()
        .copied()
        .filter(|m| values.contains(m))
        .chain(values.iter().copied().filter(|m| !METRICS.contains(m)))
        .collect()
}
impl Rule {
    pub fn metrics(&self) -> Vec<&str> {
        let mut values: BTreeSet<_> = self
            .constraints
            .iter()
            .map(|c| c.metric.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        if values.is_empty() {
            values.insert("other");
        }
        order(values)
    }
    /// None datatype means every datatype of the given source layer. Callers
    /// must match the current design, never treat an empty match as hide-all.
    pub fn includes_layer(&self, layer: u32, datatype: u32) -> bool {
        self.source_gds
            .iter()
            .any(|&(l, d)| l == layer && d.is_none_or(|d| d == datatype))
    }
    pub fn compare(
        &self,
        kind: char,
        points: &[[i64; 2]],
        precision: f64,
        stop: &AtomicUsize,
    ) -> Result<Option<Comparison<'_>>> {
        self.compare_using(
            |metric| measured(kind, points, precision, metric, stop),
            stop,
        )
    }
    pub fn compare_um(
        &self,
        kind: char,
        points: &[[f64; 2]],
        stop: &AtomicUsize,
    ) -> Result<Option<Comparison<'_>>> {
        self.compare_using(
            |metric| crate::drc::measured_um(kind, points, metric, stop),
            stop,
        )
    }
    fn compare_using(
        &self,
        mut measure: impl FnMut(&str) -> Result<Option<f64>>,
        stop: &AtomicUsize,
    ) -> Result<Option<Comparison<'_>>> {
        let mut values = BTreeMap::new();
        let mut first = None;
        for (i, c) in self.constraints.iter().enumerate() {
            check_cancelled(stop)?;
            let Some(bound) = c.value else { continue };
            let value = match values.get(c.metric.as_str()) {
                Some(v) => *v,
                None => {
                    let v = measure(&c.metric)?;
                    values.insert(c.metric.as_str(), v);
                    v
                }
            };
            let Some(value) = value else { continue };
            if first.is_none() || matches!(c.op.as_str(), "<" | "<=" | "==") {
                first = Some((i, c, bound, value));
            }
            if matches!(c.op.as_str(), "<" | "<=" | "==") {
                break;
            }
        }
        let Some((i, c, bound, measured)) = first else {
            return Ok(None);
        };
        let delta = measured - bound;
        let percent = (bound != 0.).then(|| delta / bound * 100.);
        if !delta.is_finite() || percent.is_some_and(|v| !v.is_finite()) {
            return Err(Error::input("unrepresentable SVRF comparison"));
        }
        Ok(Some(Comparison {
            constraint: i,
            metric: &c.metric,
            op: &c.op,
            unit: if c.metric == "area" { "um2" } else { "um" },
            measured,
            bound,
            delta,
            percent,
        }))
    }
}
impl Rules {
    pub fn parse(bytes: &[u8], stop: &AtomicUsize) -> Result<Self> {
        check_cancelled(stop)?;
        if bytes.len() > MAX_RULES_BYTES {
            return Err(Error::new(
                ErrorKind::Incomplete,
                "SVRF metadata exceeds 16 MiB",
            ));
        }
        let w: Wire = serde_json::from_slice(bytes)
            .map_err(|e| Error::input(format!("invalid SVRF rules metadata: {e}")))?;
        check_cancelled(stop)?;
        if w.format != "floe-svrf-rules" {
            return Err(Error::input("not a floe SVRF rules sidecar"));
        }
        if w.version != 1 {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "unsupported SVRF rules version (expected 1)",
            ));
        }
        for r in w.checks.values() {
            check_cancelled(stop)?;
            let mut size = r.desc.len();
            for text in r.layers.iter().chain(&r.unresolved) {
                size += text.len();
            }
            for c in &r.constraints {
                if c.metric.len() > 64
                    || !matches!(c.op.as_str(), "<" | "<=" | ">" | ">=" | "==" | "!=")
                    || c.value.is_some_and(|v| !v.is_finite())
                {
                    return Err(Error::input("invalid SVRF constraint"));
                }
                size += c.metric.len()
                    + c.op.len()
                    + c.text.len()
                    + c.raw.as_ref().map_or(0, String::len);
            }
            if size > MAX_TEXT {
                return Err(Error::new(
                    ErrorKind::Incomplete,
                    "SVRF rule text exceeds 64 KiB",
                ));
            }
        }
        for text in w.derived.values() {
            check_cancelled(stop)?;
            if text.len() > MAX_TEXT {
                return Err(Error::new(
                    ErrorKind::Incomplete,
                    "SVRF derivation exceeds 64 KiB",
                ));
            }
        }
        Ok(Self {
            checks: w.checks,
            derived: w.derived,
        })
    }
    /// Snapshot load on a worker, not the HTTP reactor. No auto-discovery or
    /// writes; callers remain responsible for approved-root authorization.
    pub fn load(path: &Path, stop: &AtomicUsize) -> Result<Self> {
        check_cancelled(stop)?;
        // A FIFO supplied as a file must fail without waiting for a writer.
        let mut f = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        let before = f.metadata()?;
        if !before.is_file() {
            return Err(Error::input("SVRF metadata must be a regular file"));
        }
        if before.len() > MAX_RULES_BYTES as u64 {
            return Err(Error::new(
                ErrorKind::Incomplete,
                "SVRF metadata exceeds 16 MiB",
            ));
        }
        let mut bytes = Vec::new();
        let mut chunk = [0; 65536];
        loop {
            check_cancelled(stop)?;
            let n = f.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            if n > MAX_RULES_BYTES - bytes.len() {
                return Err(Error::new(
                    ErrorKind::Incomplete,
                    "SVRF metadata exceeds 16 MiB",
                ));
            }
            bytes.extend_from_slice(&chunk[..n]);
        }
        let stamp = |m: &fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        if stamp(&before) != stamp(&f.metadata()?) || stamp(&before) != stamp(&fs::metadata(path)?)
        {
            return Err(Error::new(
                ErrorKind::Cache,
                "SVRF metadata changed while loading",
            ));
        }
        Self::parse(&bytes, stop)
    }
    pub fn rule(&self, name: &str) -> Option<&Rule> {
        self.checks.get(name)
    }
    pub fn catalog<'a>(
        &'a self,
        names: impl IntoIterator<Item = &'a str>,
        stop: &AtomicUsize,
    ) -> Result<Catalog<'a>> {
        let mut counts = BTreeMap::<&str, usize>::new();
        let (mut checks, mut matched) = (0, 0);
        for name in names {
            check_cancelled(stop)?;
            checks += 1;
            let rule = self.rule(name);
            matched += usize::from(rule.is_some());
            if !self.checks.is_empty() {
                for metric in rule.map_or_else(|| vec!["other"], Rule::metrics) {
                    *counts.entry(metric).or_default() += 1;
                }
            }
        }
        let types = order(counts.keys().copied().collect())
            .into_iter()
            .map(|metric| TypeCount {
                metric,
                checks: counts[metric],
            })
            .collect();
        Ok(Catalog {
            matched,
            checks,
            types,
        })
    }
    pub fn detail(&self, name: &str, stop: &AtomicUsize) -> Result<Option<Detail<'_>>> {
        check_cancelled(stop)?;
        let Some(rule) = self.rule(name) else {
            return Ok(None);
        };
        let mut queue: VecDeque<_> = rule.layers.iter().map(String::as_str).collect();
        let mut seen = BTreeSet::new();
        let mut derivations = Vec::new();
        while derivations.len() < 6 {
            check_cancelled(stop)?;
            let Some(name) = queue.pop_front() else { break };
            let Some(rhs) = self.derived.get(name) else {
                continue;
            };
            if !seen.insert(name) {
                continue;
            }
            derivations.push(Derivation { name, rhs });
            queue.extend(operands(rhs));
        }
        let derivations_more = queue
            .iter()
            .any(|n| !seen.contains(n) && self.derived.contains_key(*n));
        Ok(Some(Detail {
            rule,
            metrics: rule.metrics(),
            derivations,
            derivations_more,
        }))
    }
}

/// Same ASCII identifier/operator subset as floe.svrf.rhs_operands. This only
/// follows the already parsed derivation graph for the six-line detail pane.
fn operands(rhs: &str) -> impl Iterator<Item = &str> {
    let mut i = 0;
    std::iter::from_fn(move || {
        let b = rhs.as_bytes();
        while i < b.len() {
            if !b[i].is_ascii_alphabetic() && b[i] != b'_' {
                i += 1;
                continue;
            }
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b"_.-".contains(&b[i])) {
                i += 1;
            }
            let name = &rhs[start..i];
            if !keyword(name) {
                return Some(name);
            }
        }
        None
    })
}

fn keyword(name: &str) -> bool {
    const KEYWORDS: &str = "INTERNAL INT EXTERNAL EXT ENCLOSURE ENC AREA DENSITY LENGTH ANGLE PERIMETER VERTEX AND OR NOT XOR INTERACT INSIDE OUTSIDE TOUCH CUT ENCLOSE BY SIZE GROW SHRINK EXTENT EXTENTS HOLES WITH EDGE CONVEX OPPOSITE ABUT SINGULAR REGION PROJECTING PARALLEL PERPENDICULAR ONLY ALSO OVER UNDER UNDEROVER COPY NET RATIO WINDOW STEP TRUNCATE INNER OUTER MEASURE ALL PRINT RECTANGLE SQUARE COUNT COINCIDENT EXPAND TOP LEFT RIGHT BOTTOM GOOD BAD MAX MIN EVEN ODD MULTI ORTHOGONAL POLYGON CORNER CENTERLINE SPACE WIDTH NOTCH";
    KEYWORDS
        .split_ascii_whitespace()
        .any(|k| name.eq_ignore_ascii_case(k))
}

#[cfg(test)]
mod tests;
