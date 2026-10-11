//! Explicit local SVRF subset conversion. Compatibility oracle: floe/svrf.py.
//! This never executes Tcl, macros, commands, or SVRF geometry operations.
mod files;
mod lex;
#[cfg(test)]
mod tests;

use super::{keyword, operands, Constraint, MAX_ENTRIES, MAX_RULES_BYTES, MAX_TEXT};
use crate::{artifact, cache, check_cancelled, Error, ErrorKind, Result};
use lex::*;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Debug, Clone)]
pub struct Options {
    pub defines: BTreeMap<String, Option<String>>,
    pub include_dirs: Vec<PathBuf>,
    pub scan_all: bool,
    pub follow_verbatim: bool,
    pub env_switches: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            defines: BTreeMap::new(),
            include_dirs: Vec::new(),
            scan_all: false,
            follow_verbatim: false,
            env_switches: true,
        }
    }
}
#[derive(Clone, Copy)]
struct Limits {
    input: usize,
    model: usize,
    files: usize,
    depth: usize,
    graph: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            input: 256 * 1024 * 1024,
            model: 64 * 1024 * 1024,
            files: 4096,
            depth: 64,
            graph: 16 * 1024 * 1024,
        }
    }
}
fn limit(s: &str) -> Error {
    Error::new(ErrorKind::Incomplete, format!("SVRF {s}"))
}
fn bounded(n: usize, max: usize, what: &str) -> Result<()> {
    if n > max {
        Err(limit(what))
    } else {
        Ok(())
    }
}
#[derive(Default)]
struct Histogram {
    values: Vec<(String, usize)>,
    index: BTreeMap<String, usize>,
}
impl Histogram {
    fn add(&mut self, s: &str) -> Result<()> {
        if let Some(&i) = self.index.get(s) {
            self.values[i].1 += 1;
        } else {
            bounded(
                self.values.len() + 1,
                MAX_ENTRIES,
                "histogram exceeds 65536 heads",
            )?;
            self.index.insert(s.into(), self.values.len());
            self.values.push((s.into(), 1));
        }
        Ok(())
    }
    fn most_common(&self) -> Vec<&(String, usize)> {
        let mut out: Vec<_> = self.values.iter().collect();
        // Stable sorting retains Counter's first-seen order on equal counts.
        out.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        out
    }
}
#[derive(Serialize)]
#[serde(untagged)]
enum Variable {
    Number(f64),
    Text(String),
}
enum Spec {
    Number(i64),
    Pair(i64, i64),
}
type Gds = (i64, Option<i64>);
#[derive(Serialize, Default)]
struct Check {
    desc: String,
    constraints: Vec<Constraint>,
    layers: Vec<String>,
    source_gds: Vec<Gds>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unresolved: Vec<String>,
    #[serde(skip)]
    desc_lines: usize,
}
#[derive(Default)]
pub struct Deck {
    path: PathBuf,
    defines: BTreeMap<String, Option<String>>,
    variables: BTreeMap<String, Variable>,
    layers: BTreeMap<String, Vec<Spec>>,
    layer_maps: Vec<(i64, Option<i64>, i64)>,
    derived: BTreeMap<String, String>,
    derived_ops: BTreeMap<String, Vec<String>>,
    checks: BTreeMap<String, Check>,
    includes: Vec<PathBuf>,
    warnings: Vec<String>,
    stats: BTreeMap<&'static str, usize>,
    unknown: Histogram,
    switches: Vec<String>,
    switch_values: BTreeMap<String, Vec<String>>,
    verbatim_includes: Vec<String>,
    env_used: Vec<(String, String)>,
    meas_hist: Histogram,
    inputs: BTreeMap<PathBuf, files::Stamp>,
    protected: BTreeSet<PathBuf>,
}
impl Deck {
    pub fn check_count(&self) -> usize {
        self.checks.len()
    }
    pub fn derivation_count(&self) -> usize {
        self.derived.len()
    }
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub fn stat(&self, key: &str) -> usize {
        *self.stats.get(key).unwrap_or(&0)
    }
    fn inc(&mut self, key: &'static str) {
        *self.stats.entry(key).or_default() += 1;
    }

    /// Current inputs must still match the files parsed, including symlink
    /// aliases. This is a pre-commit check, not a cross-process editing lock.
    pub fn validate_inputs(&self, stop: &AtomicUsize) -> Result<()> {
        for (path, stamp) in &self.inputs {
            check_cancelled(stop)?;
            if &files::Stamp::from(std::fs::metadata(path)?) != stamp {
                return Err(limit("input changed during conversion"));
            }
        }
        Ok(())
    }
    pub fn write_json(&self, out: &Path, stop: &AtomicUsize) -> Result<()> {
        let files: Vec<_> = self.protected.iter().cloned().collect();
        let out = artifact::protected_output(out, &files, &[])?;
        files::check_destination(&out)?;
        let bytes = self.json_bytes(stop)?;
        let staged = artifact::StagedArtifact::write(&out, stop, |file| {
            for b in bytes.chunks(65536) {
                check_cancelled(stop)?;
                file.write_all(b)?;
            }
            Ok(())
        })?;
        self.validate_inputs(stop)?;
        files::check_destination(&out)?;
        staged.commit(stop)
    }
    pub fn json_bytes(&self, stop: &AtomicUsize) -> Result<Vec<u8>> {
        let mut graph_left = Limits::default().graph;
        let layers = self.mapped_layers(&mut graph_left, &mut Limits::default().model, stop)?;
        #[derive(Serialize)]
        struct Stats<'a> {
            files: usize,
            lines: usize,
            checks: usize,
            derivations: usize,
            skipped: usize,
            cmacro_calls: usize,
            includes: &'a [PathBuf],
            env_switches: BTreeMap<&'a str, &'a str>,
            warnings: &'a [String],
        }
        #[derive(Serialize)]
        struct Output<'a> {
            format: &'static str,
            version: u32,
            deck: PathBuf,
            generated_by: String,
            defines: &'a BTreeMap<String, Option<String>>,
            variables: &'a BTreeMap<String, Variable>,
            layers: BTreeMap<&'a str, Vec<Gds>>,
            derived: &'a BTreeMap<String, String>,
            checks: &'a BTreeMap<String, Check>,
            stats: Stats<'a>,
        }
        let output = Output {
            format: "floe-svrf-rules",
            version: 1,
            deck: cache::absolute(&self.path)?,
            generated_by: format!("floe2-web {}", env!("CARGO_PKG_VERSION")),
            defines: &self.defines,
            variables: &self.variables,
            layers,
            derived: &self.derived,
            checks: &self.checks,
            stats: Stats {
                files: self.stat("files"),
                lines: self.stat("lines"),
                checks: self.check_count(),
                derivations: self.derivation_count(),
                skipped: self.stat("unknown"),
                cmacro_calls: self.stat("cmacro"),
                includes: &self.includes,
                env_switches: self
                    .env_used
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect(),
                warnings: &self.warnings,
            },
        };
        struct Writer<'a> {
            bytes: Vec<u8>,
            stop: &'a AtomicUsize,
        }
        impl Write for Writer<'_> {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                if self.stop.load(Ordering::Relaxed) != 0 {
                    return Err(std::io::Error::other("operation cancelled"));
                }
                if b.len() > MAX_RULES_BYTES - self.bytes.len() {
                    return Err(std::io::Error::other("SVRF sidecar exceeds 16 MiB"));
                }
                self.bytes.extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut w = Writer {
            bytes: Vec::new(),
            stop,
        };
        let result = serde_json::to_writer_pretty(&mut w, &output);
        check_cancelled(stop)?;
        result.map_err(|e| limit(&e.to_string()))?;
        w.write_all(b"\n").map_err(|e| limit(&e.to_string()))?;
        // The converter cannot publish a sidecar its existing Rust reader
        // cannot load. This also enforces rule text/list and GDS number limits.
        super::Rules::parse(&w.bytes, stop)?;
        Ok(w.bytes)
    }
    fn mapped_layers<'a>(
        &'a self,
        left: &mut usize,
        bytes: &mut usize,
        stop: &AtomicUsize,
    ) -> Result<BTreeMap<&'a str, Vec<Gds>>> {
        let mut by_target: BTreeMap<i64, Vec<Gds>> = BTreeMap::new();
        for &(g, d, t) in &self.layer_maps {
            graph_bytes(bytes, 128)?;
            by_target.entry(t).or_default().push((g, d));
        }
        let mut layers = BTreeMap::new();
        for (name, specs) in &self.layers {
            check_cancelled(stop)?;
            let mut values = Vec::new();
            for spec in specs {
                let pairs = match spec {
                    Spec::Number(n) => by_target.get(n).map(Vec::as_slice),
                    Spec::Pair(_, _) => None,
                };
                if let Some(pairs) = pairs {
                    graph_step(left, pairs.len(), stop)?;
                    bounded(
                        values.len() + pairs.len(),
                        4096,
                        "layer mapping exceeds 4096 pairs",
                    )?;
                    graph_bytes(bytes, pairs.len() * 64)?;
                    values.extend_from_slice(pairs);
                } else {
                    graph_step(left, 1, stop)?;
                    bounded(values.len() + 1, 4096, "layer mapping exceeds 4096 pairs")?;
                    graph_bytes(bytes, 64)?;
                    values.push(match spec {
                        Spec::Number(n) => (*n, None),
                        Spec::Pair(g, d) => (*g, Some(*d)),
                    });
                }
            }
            graph_bytes(bytes, 128)?;
            layers.insert(name.as_str(), values);
        }
        Ok(layers)
    }
    fn resolve(&mut self, mut left: usize, stop: &AtomicUsize) -> Result<()> {
        // Preindex once. Traversal uses borrowed names and queues a name once,
        // bounding dense/cyclic graphs without changing the sorted closure.
        let mut bytes = Limits::default().model;
        let mapped = self.mapped_layers(&mut left, &mut bytes, stop)?;
        let mut resolved = BTreeMap::new();
        for (name, check) in &self.checks {
            let mut seen = BTreeSet::new();
            let mut stack = Vec::new();
            for n in &check.layers {
                if seen.insert(n.as_str()) {
                    stack.push(n.as_str());
                }
            }
            let (mut gds, mut missing) = (BTreeSet::new(), BTreeSet::new());
            while let Some(n) = stack.pop() {
                graph_step(&mut left, 1, stop)?;
                if let Some(pairs) = mapped.get(n) {
                    graph_step(&mut left, pairs.len(), stop)?;
                    for &pair in pairs {
                        if !gds.contains(&pair) {
                            graph_bytes(&mut bytes, 128)?;
                        }
                        gds.insert(pair);
                    }
                } else if let Some(ops) = self.derived_ops.get(n) {
                    graph_step(&mut left, ops.len(), stop)?;
                    for n in ops {
                        if seen.insert(n.as_str()) {
                            stack.push(n.as_str());
                        }
                    }
                } else if !self.variables.contains_key(n) {
                    graph_bytes(&mut bytes, n.len() + 128)?;
                    missing.insert(n.to_owned());
                }
                bounded(
                    gds.len().max(missing.len()),
                    4096,
                    "check closure exceeds 4096 entries",
                )?;
            }
            graph_bytes(&mut bytes, name.len() + 128)?;
            resolved.insert(
                name.clone(),
                (gds.into_iter().collect(), missing.into_iter().collect()),
            );
        }
        drop(mapped);
        for (name, (gds, missing)) in resolved {
            let check = self.checks.get_mut(&name).unwrap();
            check.source_gds = gds;
            check.unresolved = missing;
        }
        Ok(())
    }
    pub fn format_scan(&self, stop: &AtomicUsize) -> Result<String> {
        check_cancelled(stop)?;
        let mut lines = vec![
            format!("deck scan: {}", self.path.display()),
            format!(
                "  files {} ({} includes), {} lines",
                self.stat("files"),
                self.includes.len(),
                self.stat("lines")
            ),
        ];
        for p in self.includes.iter().take(20) {
            lines.push(format!("    include {}", p.display()));
        }
        if self.includes.len() > 20 {
            lines.push(format!("    ... {} more", self.includes.len() - 20));
        }
        let switches: Vec<_> = self
            .switches
            .iter()
            .map(
                |name| match self.switch_values.get(name).filter(|v| !v.is_empty()) {
                    Some(v) => format!("{name}({})", v.join("|")),
                    None => name.clone(),
                },
            )
            .collect();
        let joined = |v: Vec<String>| {
            if v.is_empty() {
                "-".into()
            } else {
                v.join(", ")
            }
        };
        lines.push(format!("  switches (#IFDEF): {}", joined(switches)));
        if !self.env_used.is_empty() {
            lines.push(format!(
                "  switches satisfied from the environment: {}",
                self.env_used
                    .iter()
                    .map(|(n, v)| if v.is_empty() {
                        n.clone()
                    } else {
                        format!("{n}={v}")
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        lines.push(format!(
            "  defines in effect: {}",
            joined(self.defines.keys().cloned().collect())
        ));
        lines.push(format!(
            "  layers {}, layer maps {}, variables {}",
            self.stat("layer"),
            self.stat("layer_map"),
            self.variables.len()
        ));
        lines.push(format!(
            "  derivations {}, checks {}",
            self.stat("assign"),
            self.check_count()
        ));
        lines.push(format!(
            "  measurements: {}",
            joined(
                self.meas_hist
                    .most_common()
                    .iter()
                    .map(|(n, v)| format!("{n} {v}"))
                    .collect()
            )
        ));
        lines.push(format!(
            "  DMACRO {} / CMACRO {}{}",
            self.stat("dmacro"),
            self.stat("cmacro"),
            if self.stat("cmacro") == 0 {
                ""
            } else {
                "  << macros in use: expansion is NOT implemented, metadata will be incomplete"
            }
        ));
        if self.stat("verbatim") > 0 || !self.verbatim_includes.is_empty() {
            lines.push(format!("  VERBATIM/Tcl blocks {}; INCLUDEs inside {} (--scan follows them, the normal parse skips)", self.stat("verbatim"), self.verbatim_includes.len()));
            for t in self.verbatim_includes.iter().take(10) {
                lines.push(format!("    verbatim include {t}"));
            }
            if self.verbatim_includes.len() > 10 {
                lines.push(format!(
                    "    ... {} more",
                    self.verbatim_includes.len() - 10
                ));
            }
        }
        if self.stat("prop_expr") > 0 {
            lines.push(format!(
                "  property-expression continuations skipped {}",
                self.stat("prop_expr")
            ));
        }
        lines.push(format!(
            "  skipped statements {}{}",
            self.stat("unknown") + self.stat("unknown_in_block"),
            if self.unknown.values.is_empty() {
                ""
            } else {
                ":"
            }
        ));
        for (n, v) in self.unknown.most_common().into_iter().take(20) {
            lines.push(format!("    {n:<24} {v}"));
        }
        for w in self.warnings.iter().take(20) {
            lines.push(format!("  warn: {w}"));
        }
        check_cancelled(stop)?;
        Ok(lines.join("\n"))
    }
}
fn graph_step(left: &mut usize, n: usize, stop: &AtomicUsize) -> Result<()> {
    check_cancelled(stop)?;
    *left = left
        .checked_sub(n)
        .ok_or_else(|| limit("derivation/mapping work exceeds limit"))?;
    Ok(())
}
fn graph_bytes(left: &mut usize, n: usize) -> Result<()> {
    *left = left
        .checked_sub(n)
        .ok_or_else(|| limit("mapped layer/closure expansion exceeds 64 MiB"))?;
    Ok(())
}
struct Continuation {
    check: String,
    metric: &'static str,
    text: String,
    had: bool,
}
struct Parser<'a> {
    d: Deck,
    options: &'a Options,
    stop: &'a AtomicUsize,
    env: &'a dyn Fn(&str) -> Option<String>,
    limits: Limits,
    input_bytes: usize,
    charged: usize,
    cond: Vec<bool>,
    inactive: usize,
    cur: Option<String>,
    depth: i64,
    macro_depth: i64,
    macro_pending: bool,
    verbatim_depth: i64,
    in_comment: bool,
    icont: bool,
    cont: Option<Continuation>,
    acont: Option<(String, Option<String>)>,
    acont_open: bool,
    substitution: Substitution,
}
impl<'a> Parser<'a> {
    fn new(
        options: &'a Options,
        stop: &'a AtomicUsize,
        env: &'a dyn Fn(&str) -> Option<String>,
        limits: Limits,
    ) -> Result<Self> {
        let mut p = Self {
            d: Deck::default(),
            options,
            stop,
            env,
            limits,
            input_bytes: 0,
            charged: 0,
            cond: Vec::new(),
            inactive: 0,
            cur: None,
            depth: 0,
            macro_depth: 0,
            macro_pending: false,
            verbatim_depth: 0,
            in_comment: false,
            icont: false,
            cont: None,
            acont: None,
            acont_open: false,
            substitution: Substitution::default(),
        };
        bounded(options.defines.len(), MAX_ENTRIES, "too many defines")?;
        bounded(
            options.include_dirs.len(),
            4096,
            "too many include directories",
        )?;
        for (n, v) in &options.defines {
            p.text(n)?;
            if let Some(v) = v {
                p.text(v)?;
            }
            p.d.defines
                .insert(n.clone(), v.clone().filter(|s| !s.is_empty()));
        }
        p.substitution.rebuild(&p.d.defines)?;
        Ok(p)
    }
    fn charge(&mut self, n: usize) -> Result<()> {
        check_cancelled(self.stop)?;
        self.charged = self
            .charged
            .checked_add(n)
            .ok_or_else(|| limit("model allocation overflow"))?;
        bounded(
            self.charged,
            self.limits.model,
            "metadata/expansion work exceeds 64 MiB",
        )
    }
    fn text(&mut self, s: &str) -> Result<()> {
        bounded(s.len(), MAX_TEXT, "text exceeds 64 KiB")?;
        self.charge(s.len() + 64)
    }
    fn warn(&mut self, s: String) -> Result<()> {
        self.text(&s)?;
        bounded(self.d.warnings.len() + 1, MAX_ENTRIES, "too many warnings")?;
        self.d.warnings.push(s);
        Ok(())
    }
    fn active(&self) -> bool {
        self.inactive == 0
    }
    fn switch(&mut self, name: &str) -> Result<(bool, Option<String>)> {
        if let Some(value) = self.d.defines.get(name) {
            return Ok((true, value.clone()));
        }
        if self.options.env_switches {
            if let Some(v) = (self.env)(name.strip_prefix('$').unwrap_or(name)) {
                self.text(&v)?;
                if let Some((_, old)) = self.d.env_used.iter_mut().find(|(n, _)| n == name) {
                    *old = v.clone();
                } else {
                    self.d.env_used.push((name.into(), v.clone()));
                }
                let v = (!v.is_empty()).then_some(v);
                if !name.starts_with('$') {
                    self.d.defines.insert(name.into(), v.clone());
                    bounded(self.d.defines.len(), MAX_ENTRIES, "too many defines")?;
                    self.substitution.set(name, v.as_deref())?;
                }
                return Ok((true, v));
            }
        }
        Ok((false, None))
    }
    fn directive(&mut self, s: &str) -> Result<()> {
        let toks: Vec<_> = tokens(s.split_once("//").map_or(s, |(a, _)| a)).collect();
        let Some(head) = toks.first() else {
            return Ok(());
        };
        match head.to_ascii_uppercase().as_str() {
            "#DEFINE" if toks.len() >= 2 => {
                if self.active() || self.options.scan_all {
                    let v = toks[2..].join(" ");
                    let v = unquote(&v);
                    self.text(v)?;
                    self.d
                        .defines
                        .insert(toks[1].into(), (!v.is_empty()).then(|| v.into()));
                    bounded(self.d.defines.len(), MAX_ENTRIES, "too many defines")?;
                    self.substitution
                        .set(toks[1], (!v.is_empty()).then_some(v))?;
                }
            }
            "#UNDEFINE" if toks.len() >= 2 => {
                if self.active() || self.options.scan_all {
                    self.d.defines.remove(toks[1]);
                    self.substitution.set(toks[1], None)?;
                }
            }
            "#IFDEF" | "#IFNDEF" if toks.len() >= 2 => {
                let name = toks[1];
                if !self.d.switches.iter().any(|n| n == name) {
                    self.text(name)?;
                    self.d.switches.push(name.into());
                }
                let want = (toks.len() >= 3).then(|| unquote(&toks[2..].join(" ")).to_owned());
                if let Some(want) = &want {
                    self.text(want)?;
                    let values = self.d.switch_values.entry(name.into()).or_default();
                    if !values.contains(want) {
                        values.push(want.clone());
                    }
                }
                let (defined, value) = self.switch(name)?;
                let mut on = defined && want.as_ref().is_none_or(|w| value.as_ref() == Some(w));
                if head.eq_ignore_ascii_case("#IFNDEF") {
                    on = !on;
                }
                bounded(
                    self.cond.len() + 1,
                    4096,
                    "conditional nesting exceeds 4096",
                )?;
                let on = on || self.options.scan_all;
                self.inactive += usize::from(!on);
                self.cond.push(on);
            }
            "#ELSE" => {
                if let Some(last) = self.cond.last_mut() {
                    self.inactive -= usize::from(!*last);
                    *last = !*last || self.options.scan_all;
                    self.inactive += usize::from(!*last);
                } else {
                    self.warn("#ELSE without #IFDEF".into())?;
                }
            }
            "#ENDIF" => {
                if let Some(on) = self.cond.pop() {
                    self.inactive -= usize::from(!on);
                } else {
                    self.warn("#ENDIF without #IFDEF".into())?;
                }
            }
            _ => self.d.inc("unknown_directive"),
        }
        Ok(())
    }
    fn statement(&mut self, mut s: &str) -> Result<()> {
        // A closed block can start another statement on the same line. Use a
        // loop, not recursive calls driven by untrusted `} NAME {` sequences.
        loop {
            check_cancelled(self.stop)?;
            if self.verbatim_depth > 0 {
                self.verbatim_depth = (self.verbatim_depth + delta(s)).max(0);
                return Ok(());
            }
            if self.macro_depth > 0 || self.macro_pending {
                if self.macro_pending {
                    if s.contains('{') {
                        self.macro_pending = false;
                        self.macro_depth = delta(s).max(0);
                    }
                } else {
                    self.macro_depth = (self.macro_depth + delta(s)).max(0);
                }
                return Ok(());
            }
            if self.cur.is_some() {
                if let Some(rest) = self.block_line(s)? {
                    s = rest;
                    continue;
                }
                return Ok(());
            }
            if self.try_cont(s)? {
                return Ok(());
            }
            self.cont = None;
            if let Some((name, rest)) = check(s) {
                let head = name.to_ascii_uppercase();
                if noncheck(&head) {
                    self.d.inc("verbatim");
                    self.verbatim_depth = (1 + delta(rest)).max(0);
                    self.acont = None;
                    return Ok(());
                }
                if !ignored(&head) && assign(s).is_none() {
                    if self.d.checks.contains_key(name) {
                        self.warn(format!("duplicate check {name} (kept last)"))?;
                    }
                    self.text(name)?;
                    self.d.checks.insert(name.into(), Check::default());
                    bounded(self.d.checks.len(), MAX_ENTRIES, "too many checks")?;
                    self.cur = Some(name.into());
                    self.depth = 1;
                    self.acont = None;
                    self.icont = false;
                    s = trim(rest);
                    if s.is_empty() {
                        return Ok(());
                    }
                    continue;
                }
            }
            let head = head_rest(s).0.to_ascii_uppercase();
            match head.as_str() {
                "LAYER" => {
                    self.acont = None;
                    self.icont = false;
                    self.layer(s)?;
                }
                "VARIABLE" => {
                    self.acont = None;
                    self.icont = false;
                    self.variable(s)?;
                }
                "DMACRO" => {
                    self.acont = None;
                    self.icont = false;
                    self.d.inc("dmacro");
                    if s.contains('{') {
                        self.macro_depth = delta(s).max(0);
                    } else {
                        self.macro_pending = true;
                    }
                }
                "CMACRO" => {
                    self.acont = None;
                    self.d.inc("cmacro");
                }
                _ => {
                    if let Some((lhs, rhs)) = assign(s) {
                        self.assignment(lhs, rhs, None)?;
                    } else if self.try_acont(s)? {
                        self.icont = false;
                    } else {
                        self.other(&head, false)?;
                    }
                }
            }
            return Ok(());
        }
    }
    fn block_line<'s>(&mut self, mut s: &'s str) -> Result<Option<&'s str>> {
        if s.starts_with('@') {
            let mut desc = trim(s.trim_start_matches('@'));
            if desc.len() >= 2 && desc.starts_with('"') && desc.ends_with('"') {
                desc = &desc[1..desc.len() - 1];
            }
            self.text(desc)?;
            let c = self.d.checks.get_mut(self.cur.as_ref().unwrap()).unwrap();
            bounded(
                c.desc.len() + desc.len() + 1,
                MAX_TEXT,
                "description exceeds 64 KiB",
            )?;
            if c.desc_lines > 0 {
                c.desc.push('\n');
            }
            c.desc.push_str(desc);
            c.desc_lines += 1;
            return Ok(None);
        }
        while let Some(rest) = s.strip_prefix('}') {
            self.depth -= 1;
            s = rest.trim_start_matches(whitespace);
            if self.depth <= 0 {
                self.cur = None;
                self.cont = None;
                return Ok((!s.is_empty()).then_some(s));
            }
        }
        if s.is_empty() {
            return Ok(None);
        }
        let mut closes = 0;
        while let Some(rest) = s.strip_suffix('}') {
            closes += 1;
            s = rest.trim_end_matches(whitespace);
        }
        let nested = delta(s);
        if !s.is_empty() && !self.try_cont(s)? {
            if let Some((lhs, rhs)) = assign(s) {
                self.cont = None;
                self.assignment(lhs, rhs, self.cur.clone())?;
            } else {
                let head = head_rest(s).0.to_ascii_uppercase();
                if metric(&head).is_some() {
                    self.acont = None;
                    self.icont = false;
                    self.measurement(s, self.cur.clone())?;
                } else if self.try_acont(s)? {
                    self.icont = false;
                } else if self.cont.is_some() && keyword(&head) {
                    let names = self.names(s)?;
                    let name = self.cont.as_ref().unwrap().check.clone();
                    self.add_operands(&name, &names)?;
                    self.icont = false;
                } else {
                    self.other(&head, true)?;
                }
            }
        }
        self.depth += nested - closes;
        if self.depth <= 0 {
            self.cur = None;
            self.cont = None;
        }
        Ok(None)
    }
    fn other(&mut self, head: &str, block: bool) -> Result<()> {
        if block {
            self.cont = None;
        }
        if ignored(head) {
            self.acont = None;
            self.icont = true;
            self.d.inc("ignored");
        } else if head.starts_with(['[', '~', '(']) {
            self.acont = None;
            self.d.inc("prop_expr");
        } else if self.icont {
            self.d.inc("ignored");
        } else {
            self.acont = None;
            self.d
                .inc(if block { "unknown_in_block" } else { "unknown" });
            if !self.d.unknown.index.contains_key(head) {
                self.text(head)?;
            }
            self.d.unknown.add(head)?;
        }
        Ok(())
    }
    fn layer(&mut self, s: &str) -> Result<()> {
        let t: Vec<_> = tokens(s).collect();
        if t.get(1).is_some_and(|s| s.eq_ignore_ascii_case("MAP")) {
            let mut ints = Vec::new();
            for s in &t[2..] {
                if let Some(n) = signed(s)? {
                    ints.push(n);
                }
            }
            if ints.len() >= 2 {
                let (g, target) = (ints[0], *ints.last().unwrap());
                let dt = t.iter().any(|s| s.eq_ignore_ascii_case("DATATYPE"));
                let dts = if dt && ints.len() > 2 {
                    ints[1..ints.len() - 1].iter().copied().map(Some).collect()
                } else {
                    vec![None]
                };
                self.charge(dts.len() * 32)?;
                bounded(
                    self.d.layer_maps.len() + dts.len(),
                    MAX_ENTRIES,
                    "too many layer mappings",
                )?;
                self.d
                    .layer_maps
                    .extend(dts.into_iter().map(|d| (g, d, target)));
            }
            self.d.inc("layer_map");
            return Ok(());
        }
        if t.len() < 3 {
            self.d.inc("unknown");
            return Ok(());
        }
        let mut specs = Vec::new();
        for s in &t[2..] {
            if let Some(n) = unsigned(s)? {
                specs.push(Spec::Number(n));
            } else if let Some((a, b)) = s.split_once('.') {
                if let (Some(a), Some(b)) = (unsigned(a)?, unsigned(b)?) {
                    specs.push(Spec::Pair(a, b));
                }
            }
        }
        if specs.is_empty() {
            self.d.inc("unknown");
            self.d.unknown.add("LAYER")?;
        } else {
            self.text(t[1])?;
            self.charge(specs.len() * 32)?;
            bounded(specs.len(), 4096, "layer specs exceed 4096")?;
            self.d.layers.insert(t[1].into(), specs);
            bounded(self.d.layers.len(), MAX_ENTRIES, "too many layers")?;
            self.d.inc("layer");
        }
        Ok(())
    }
    fn variable(&mut self, s: &str) -> Result<()> {
        let t: Vec<_> = tokens(s).collect();
        if t.len() >= 3 {
            let v = if let Some(n) = numeric(t[2])? {
                Variable::Number(n)
            } else {
                let s = t[2..].join(" ");
                self.text(&s)?;
                Variable::Text(s)
            };
            self.text(t[1])?;
            self.d.variables.insert(t[1].into(), v);
            bounded(self.d.variables.len(), MAX_ENTRIES, "too many variables")?;
        }
        Ok(())
    }
    fn names(&mut self, s: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for n in operands(s) {
            self.text(n)?;
            out.push(n.into());
        }
        Ok(out)
    }
    fn add_operands(&mut self, check: &str, names: &[String]) -> Result<()> {
        let c = self.d.checks.get_mut(check).unwrap();
        let mut seen: BTreeSet<String> = c.layers.iter().cloned().collect();
        for n in names {
            if seen.insert(n.clone()) {
                c.layers.push(n.clone());
            }
        }
        bounded(c.layers.len(), 4096, "check operands exceed 4096")
    }
    fn assignment(&mut self, lhs: &str, rhs: &str, check: Option<String>) -> Result<()> {
        self.text(lhs)?;
        self.text(rhs)?;
        self.d.derived.insert(lhs.into(), trim(rhs).into());
        bounded(self.d.derived.len(), MAX_ENTRIES, "too many derivations")?;
        let names = if metric(&head_rest(rhs).0.to_ascii_uppercase()).is_some() {
            let names = self.measurement(rhs, check)?;
            self.acont = None;
            names
        } else {
            let names = self.names(rhs)?;
            if let Some(check) = &check {
                self.add_operands(check, &names)?;
            }
            self.icont = false;
            self.acont = Some((lhs.into(), check));
            self.acont_open = tokens(rhs).last().is_some_and(keyword);
            names
        };
        self.d.derived_ops.insert(lhs.into(), names);
        self.d.inc("assign");
        Ok(())
    }
    fn try_acont(&mut self, s: &str) -> Result<bool> {
        let Some((lhs, check)) = self.acont.clone() else {
            return Ok(false);
        };
        if !self.acont_open && !keyword(head_rest(s).0) {
            return Ok(false);
        }
        self.text(s)?;
        let rhs = self.d.derived.get_mut(&lhs).unwrap();
        bounded(
            rhs.len() + s.len() + 1,
            MAX_TEXT,
            "derivation exceeds 64 KiB",
        )?;
        rhs.push(' ');
        rhs.push_str(trim(s));
        let mut names = self.names(s)?;
        let ops = self.d.derived_ops.get_mut(&lhs).unwrap();
        let old: BTreeSet<_> = ops.iter().map(String::as_str).collect();
        names.retain(|n| !old.contains(n.as_str()));
        ops.extend(names.iter().cloned());
        if let Some(check) = check {
            self.add_operands(&check, &names)?;
        }
        self.acont_open = tokens(s).last().is_some_and(keyword);
        Ok(true)
    }
    fn add_bounds(
        &mut self,
        check: &str,
        metric: &'static str,
        bounds: &[(&str, &str)],
        text: &str,
    ) -> Result<()> {
        bounded(
            self.d.checks[check].constraints.len() + bounds.len(),
            1024,
            "check constraints exceed 1024",
        )?;
        for &(op, tok) in bounds {
            let v = numeric(tok)?.or_else(|| match self.d.variables.get(tok) {
                Some(Variable::Number(n)) => Some(*n),
                _ => None,
            });
            self.text(text)?;
            self.text(tok)?;
            self.d
                .checks
                .get_mut(check)
                .unwrap()
                .constraints
                .push(Constraint {
                    metric: metric.into(),
                    op: op.into(),
                    value: v,
                    text: text.into(),
                    raw: v.is_none().then(|| tok.into()),
                });
        }
        Ok(())
    }
    fn try_cont(&mut self, s: &str) -> Result<bool> {
        if self.cont.is_none() || op(s, 0).is_none() {
            return Ok(false);
        }
        let bounds = chain(s, 0);
        if bounds.is_empty() {
            return Ok(false);
        }
        let mut cont = self.cont.take().unwrap();
        bounded(
            cont.text.len() + s.len() + 1,
            MAX_TEXT,
            "measurement exceeds 64 KiB",
        )?;
        cont.text.push(' ');
        cont.text.push_str(trim(s));
        self.add_bounds(&cont.check, cont.metric, &bounds, &cont.text)?;
        if !cont.had {
            *self.d.stats.entry("meas_no_bound").or_default() -= 1;
        }
        cont.had = true;
        self.cont = Some(cont);
        Ok(true)
    }
    fn measurement(&mut self, s: &str, check: Option<String>) -> Result<Vec<String>> {
        let (head, rest) = head_rest(s);
        let head = head.to_ascii_uppercase();
        let metric = metric(&head).unwrap();
        self.d.meas_hist.add(&head)?;
        let pos = first_op(rest);
        let names = self.names(pos.map_or(rest, |i| &rest[..i]))?;
        if let Some(check) = check {
            self.add_operands(&check, &names)?;
            let bounds = pos.map_or_else(Vec::new, |i| chain(rest, i));
            self.add_bounds(&check, metric, &bounds, trim(s))?;
            if bounds.is_empty() {
                self.d.inc("meas_no_bound");
            }
            self.cont = Some(Continuation {
                check,
                metric,
                text: trim(s).into(),
                had: !bounds.is_empty(),
            });
        }
        Ok(names)
    }
    fn finish(mut self) -> Result<Deck> {
        if let Some(name) = &self.cur {
            self.warn(format!("unterminated check block {name}"))?;
        }
        if !self.d.verbatim_includes.is_empty()
            && !self.options.scan_all
            && !self.options.follow_verbatim
        {
            self.warn(format!("{} INCLUDE(s) inside VERBATIM/Tcl blocks were NOT followed (Tcl-conditional; --scan or --follow-verbatim follows them)", self.d.verbatim_includes.len()))?;
        }
        self.d.resolve(self.limits.graph, self.stop)?;
        self.d.validate_inputs(self.stop)?;
        Ok(self.d)
    }
}
/// CLI-only environment fallback: reads just names explicitly tested by the
/// source or used in INCLUDEs. Never bulk import/log the process environment.
pub fn parse_deck(path: &Path, options: &Options, stop: &AtomicUsize) -> Result<Deck> {
    let env = |name: &str| std::env::var(name).ok();
    let mut p = Parser::new(options, stop, &env, Limits::default())?;
    p.d.path = path.into();
    p.feed_file(path, &mut Vec::new())?;
    p.finish()
}
