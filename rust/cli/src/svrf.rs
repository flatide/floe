//! `floe-index svrf`: Calibre SVRF rule deck SUBSET parser -> per-check
//! rule metadata, the `<deck>.rules.json` sidecar the viewer and
//! `floe drc` read (docs/SPEC-FORMATS.ko.md, docs/DRC.ko.md).
//!
//! Moved from `floe svrf` (floe/svrf.py) on 2026-09-29 (user call: the
//! tools that build files from inputs live in floe-index - `vfs` builds
//! the layout cache, `drc` the result pack, `svrf` the rule sidecar).
//! The port is exact: the same statements are recognised, the same
//! warnings are raised, and the sidecar is byte-identical to what
//! `json.dump(data, indent=1, sort_keys=True)` wrote (Python float repr,
//! ASCII-only `\uXXXX` escapes) - only `generated_by` names this tool.
//! The Python side keeps the READER half (floe/svrf.py: load_rules,
//! rhs_operands).
//!
//! floe does NOT implement SVRF geometry semantics - per check block it
//! extracts the @ description, the measurement constraints (operator +
//! numeric bound), the referenced layer names and, through the
//! derivation graph, the source GDS layers. Every operator
//! (AND/NOT/SIZE/...) is IGNORED and only the operand NAMES on the
//! right-hand side become edges of a directed graph, so "which drawn
//! layers feed this rule" never requires implementing the operations.
//!
//! SVRF is not a public grammar: unrecognized statements are counted into
//! a histogram and skipped, never fatal. Known gaps, deliberate:
//!   - DMACRO/CMACRO are NOT expanded (usage is counted; --scan makes the
//!     gap visible before trusting a converted deck).
//!   - TVF (Tcl) decks are out of scope - parse the SVRF that Calibre
//!     generates from them, never the Tcl itself.
//!   - Statements are line-oriented; a derivation wrapped across lines is
//!     joined only in the recognised wrap styles.
//!   - Digits are ASCII (Python's `\d` also took other decimal scripts).
//! Preprocessing (INCLUDE / #DEFINE / #UNDEFINE / #IFDEF / #IFNDEF /
//! #ELSE / #ENDIF / VARIABLE) IS implemented because in-house decks gate
//! optional rules on switches: pass the same -D set as the Calibre run or
//! the check list will differ. #IFDEF/#IFNDEF support the two-arg value
//! form (`#IFDEF STACK 6LM` = defined AND equal), directive lines strip
//! // comments, values may be quoted, and INCLUDE paths expand
//! $VAR/${VAR}/~ from the environment. Switch names the deck tests also
//! FALL BACK to the environment (sourceme workflow: `source sourceme.* &&
//! floe-index svrf ...` - lazy per-name lookup, never a bulk env import;
//! -D wins; used names are reported and stored in the sidecar;
//! --no-env-switches disables).

use std::collections::{HashMap, HashSet};

pub const FORMAT: &str = "floe-svrf-rules";
pub const VERSION: i64 = 1;

// measurement statement -> the metric its bound constrains
const MEAS: [(&str, &str); 12] = [
    ("INTERNAL", "width"),
    ("INT", "width"),
    ("EXTERNAL", "space"),
    ("EXT", "space"),
    ("ENCLOSURE", "enclosure"),
    ("ENC", "enclosure"),
    ("AREA", "area"),
    ("DENSITY", "density"),
    ("LENGTH", "length"),
    ("ANGLE", "angle"),
    ("PERIMETER", "perimeter"),
    ("VERTEX", "vertex"),
];

// operator / option words excluded from operand-name extraction (a layer
// whose NAME collides with one of these is mis-filtered - the unresolved
// list makes that visible instead of silently wrong); MEAS heads join
const KEYWORD_WORDS: [&str; 70] = [
    "AND", "OR", "NOT", "XOR", "INTERACT", "INSIDE", "OUTSIDE", "TOUCH", "CUT", "ENCLOSE", "BY",
    "SIZE", "GROW", "SHRINK", "EXTENT", "EXTENTS", "HOLES", "WITH", "EDGE", "CONVEX", "OPPOSITE",
    "ABUT", "SINGULAR", "REGION", "PROJECTING", "PARALLEL", "PERPENDICULAR", "ONLY", "ALSO",
    "OVER", "UNDER", "UNDEROVER", "COPY", "NET", "RATIO", "WINDOW", "STEP", "TRUNCATE", "INNER",
    "OUTER", "MEASURE", "ALL", "PRINT", "RECTANGLE", "SQUARE", "COUNT", "COINCIDENT", "EXPAND",
    "TOP", "LEFT", "RIGHT", "BOTTOM", "GOOD", "BAD", "MAX", "MIN", "EVEN", "ODD", "MULTI",
    "ORTHOGONAL", "POLYGON", "CORNER", "CENTERLINE", "SPACE", "WIDTH", "NOTCH",
    // repeated in the Python set; harmless
    "OPPOSITE", "AND", "OR", "NOT",
];

// statement heads that are structural, never checks or layers - skipped
// without polluting the unknown histogram
const IGNORED_HEADS: [&str; 35] = [
    "PRECISION", "RESOLUTION", "TITLE", "DRC", "LAYOUT", "TEXT", "CONNECT", "SCONNECT", "VIRTUAL",
    "ATTACH", "MASK", "LVS", "ERC", "PEX", "SOURCE", "GROUND", "UNIT", "FLAG", "GROUP", "PORT",
    "EXCLUDE", "CAPACITANCE", "RESISTANCE", "DEVICE", "TRACE", "SVRF", "PUSHDOWN", "POLYGON",
    "FILTER", "DFM", "RDB", "DVPARAMS", "OFFGRID", "NET", "FLATTEN",
];

// `NAME { ... }` blocks that are NOT rule checks: hybrid decks wrap Tcl in
// VERBATIM blocks (sfa14 field scan 2026-08-18), and Tcl control flow
// surfaces if/else/... at statement level. Bodies are brace-skipped;
// INCLUDEs inside are inventoried (and followed under --scan).
const NONCHECK_BLOCKS: [&str; 8] = ["VERBATIM", "IF", "ELSE", "ELSEIF", "FOREACH", "WHILE", "PROC", "SWITCH"];

fn meas_metric(head_upper: &str) -> Option<&'static str> {
    MEAS.iter().find(|(h, _)| *h == head_upper).map(|(_, m)| *m)
}

struct Words {
    keywords: HashSet<&'static str>,
    ignored: HashSet<&'static str>,
    noncheck: HashSet<&'static str>,
}

impl Words {
    fn new() -> Self {
        let mut keywords: HashSet<&'static str> = KEYWORD_WORDS.iter().copied().collect();
        for (h, _) in MEAS.iter() {
            keywords.insert(h);
        }
        Words {
            keywords,
            ignored: IGNORED_HEADS.iter().copied().collect(),
            noncheck: NONCHECK_BLOCKS.iter().copied().collect(),
        }
    }
    fn keyword(&self, upper: &str) -> bool {
        self.keywords.contains(upper)
    }
}

// ---------------------------------------------------------------- Python str

/// Python `str.isspace()`: Unicode White_Space plus U+001C..U+001F.
fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

fn py_lstrip(s: &str) -> &str {
    s.trim_start_matches(py_isspace)
}

fn py_rstrip(s: &str) -> &str {
    s.trim_end_matches(py_isspace)
}

/// Python `s.split()`.
fn py_split(s: &str) -> Vec<&str> {
    s.split(py_isspace).filter(|t| !t.is_empty()).collect()
}

/// Python `s.split(None, 1)`: the first word and the rest (its leading
/// whitespace dropped, trailing kept).
fn py_split1(s: &str) -> (Option<&str>, Option<&str>) {
    let t = py_lstrip(s);
    if t.is_empty() {
        return (None, None);
    }
    match t.char_indices().find(|&(_, c)| py_isspace(c)) {
        None => (Some(t), None),
        Some((i, _)) => {
            let rest = py_lstrip(&t[i..]);
            (Some(&t[..i]), if rest.is_empty() { None } else { Some(rest) })
        }
    }
}

fn upper(s: &str) -> String {
    s.to_uppercase()
}

fn head_upper(s: &str) -> String {
    py_split1(s).0.map(upper).unwrap_or_default()
}

fn push_unique(v: &mut Vec<String>, n: &str) {
    if !v.iter().any(|x| x == n) {
        v.push(n.to_string());
    }
}

// ------------------------------------------------------------ the regexes

fn is_id_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'
}

/// `[A-Za-z_][A-Za-z0-9_.\-]*` findall.
fn id_findall(s: &str) -> Vec<String> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        if is_id_start(cs[i]) {
            let st = i;
            i += 1;
            while i < cs.len() && is_id_char(cs[i]) {
                i += 1;
            }
            out.push(cs[st..i].iter().collect());
        } else {
            i += 1;
        }
    }
    out
}

/// Operand NAMES of a derivation right-hand side - operators, options and
/// numbers dropped (floe/svrf.py rhs_operands, which the viewer keeps).
fn rhs_operands(w: &Words, rhs: &str) -> Vec<String> {
    id_findall(rhs).into_iter().filter(|t| !w.keyword(&upper(t))).collect()
}

const OPS: [&str; 6] = ["<=", ">=", "==", "!=", "<", ">"];

/// `(?<![A-Za-z_])`
fn lookbehind_ok(cs: &[char], pos: usize) -> bool {
    pos == 0 || !(cs[pos - 1].is_ascii_alphabetic() || cs[pos - 1] == '_')
}

fn starts_at(cs: &[char], pos: usize, lit: &str) -> bool {
    let mut j = pos;
    for c in lit.chars() {
        if j >= cs.len() || cs[j] != c {
            return false;
        }
        j += 1;
    }
    true
}

/// `_OP_RX` at `pos`: a comparator not glued to an identifier tail.
fn op_at(cs: &[char], pos: usize) -> bool {
    lookbehind_ok(cs, pos) && OPS.iter().any(|op| starts_at(cs, pos, op))
}

/// `_OP_RX.search`: the first comparator's position.
fn op_search(cs: &[char]) -> Option<usize> {
    (0..cs.len()).find(|&p| op_at(cs, p))
}

fn is_val_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' || c == '+'
}

/// `_BOUND_RX.match(text, pos)`: (op, value, end).
fn bound_at(cs: &[char], pos: usize) -> Option<(String, String, usize)> {
    if !lookbehind_ok(cs, pos) {
        return None;
    }
    for op in OPS {
        if !starts_at(cs, pos, op) {
            continue;
        }
        let mut j = pos + op.chars().count();
        while j < cs.len() && py_isspace(cs[j]) {
            j += 1;
        }
        let v0 = j;
        while j < cs.len() && is_val_char(cs[j]) {
            j += 1;
        }
        if j > v0 {
            return Some((op.to_string(), cs[v0..j].iter().collect(), j));
        }
    }
    None
}

/// The CONTIGUOUS comparator+value chain starting at pos - the
/// statement's own bounds end at the first non-comparator token, so option
/// comparators further right (ABUT>0<90, OPPOSITE EXTENDED < x) never read
/// as constraints.
fn chain(cs: &[char], mut pos: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while let Some((op, v, end)) = bound_at(cs, pos) {
        out.push((op, v));
        pos = end;
        while pos < cs.len() && py_isspace(cs[pos]) {
            pos += 1;
        }
    }
    out
}

/// `^[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?$` (ASCII digits).
fn is_num(t: &str) -> bool {
    let b = t.as_bytes();
    let n = b.len();
    let digits = |i: usize| b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    let mut i = 0;
    if i < n && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let d1 = digits(i);
    if d1 > 0 {
        i += d1;
        if i < n && b[i] == b'.' {
            i += 1;
            i += digits(i);
        }
    } else if i < n && b[i] == b'.' {
        i += 1;
        let d2 = digits(i);
        if d2 == 0 {
            return false;
        }
        i += d2;
    } else {
        return false;
    }
    if i < n && (b[i] == b'e' || b[i] == b'E') {
        let save = i;
        i += 1;
        if i < n && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let d = digits(i);
        if d == 0 {
            i = save;
        } else {
            i += d;
        }
    }
    i == n
}

/// `^("?)([A-Za-z0-9_.\-$]+)\1\s*\{(.*)$` -> (name, rest after `{`).
fn check_match(s: &str) -> Option<(String, String)> {
    let cs: Vec<char> = s.chars().collect();
    let is_name = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' || c == '$';
    let attempt = |start: usize, quoted: bool| -> Option<(String, String)> {
        let mut j = start;
        while j < cs.len() && is_name(cs[j]) {
            j += 1;
        }
        if j == start {
            return None;
        }
        let name: String = cs[start..j].iter().collect();
        if quoted {
            if cs.get(j) != Some(&'"') {
                return None;
            }
            j += 1;
        }
        while j < cs.len() && py_isspace(cs[j]) {
            j += 1;
        }
        if cs.get(j) != Some(&'{') {
            return None;
        }
        Some((name, cs[j + 1..].iter().collect()))
    };
    if cs.first() == Some(&'"') {
        if let Some(r) = attempt(1, true) {
            return Some(r);
        }
    }
    attempt(0, false)
}

/// `^([A-Za-z_][A-Za-z0-9_.\-]*)\s*=\s*(.+)$` -> (lhs, rhs).
fn assign_match(s: &str) -> Option<(String, String)> {
    let cs: Vec<char> = s.chars().collect();
    if cs.is_empty() || !is_id_start(cs[0]) {
        return None;
    }
    let mut j = 1;
    while j < cs.len() && is_id_char(cs[j]) {
        j += 1;
    }
    let lhs: String = cs[..j].iter().collect();
    while j < cs.len() && py_isspace(cs[j]) {
        j += 1;
    }
    if cs.get(j) != Some(&'=') {
        return None;
    }
    let rest = &cs[j + 1..];
    if rest.is_empty() {
        return None;
    }
    // greedy `\s*`, leaving `(.+)` at least one character
    let mut k = 0;
    while k + 1 < rest.len() && py_isspace(rest[k]) {
        k += 1;
    }
    Some((lhs, rest[k..].iter().collect()))
}

fn is_uint(t: &str) -> bool {
    !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit())
}

fn is_int(t: &str) -> bool {
    is_uint(t.strip_prefix('-').unwrap_or(t))
}

fn parse_int(t: &str) -> Option<i128> {
    t.parse::<i128>().ok()
}

// ------------------------------------------------------ ordered containers

/// An insertion-ordered map (Python dict / OrderedDict): assigning an
/// existing key keeps its place, removing and re-adding moves it last.
#[derive(Clone, Debug)]
pub struct OMap<V> {
    keys: Vec<String>,
    vals: Vec<V>,
    idx: HashMap<String, usize>,
}

impl<V> Default for OMap<V> {
    fn default() -> Self {
        OMap { keys: Vec::new(), vals: Vec::new(), idx: HashMap::new() }
    }
}

impl<V> OMap<V> {
    pub fn get(&self, k: &str) -> Option<&V> {
        self.idx.get(k).map(|&i| &self.vals[i])
    }
    fn get_mut(&mut self, k: &str) -> Option<&mut V> {
        match self.idx.get(k) {
            Some(&i) => Some(&mut self.vals[i]),
            None => None,
        }
    }
    pub fn contains(&self, k: &str) -> bool {
        self.idx.contains_key(k)
    }
    fn insert(&mut self, k: &str, v: V) {
        match self.idx.get(k) {
            Some(&i) => self.vals[i] = v,
            None => {
                self.idx.insert(k.to_string(), self.keys.len());
                self.keys.push(k.to_string());
                self.vals.push(v);
            }
        }
    }
    fn remove(&mut self, k: &str) {
        if let Some(i) = self.idx.remove(k) {
            self.keys.remove(i);
            self.vals.remove(i);
            for j in i..self.keys.len() {
                *self.idx.get_mut(&self.keys[j]).unwrap() = j;
            }
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.keys.iter().zip(self.vals.iter())
    }
    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

impl OMap<i64> {
    /// Counter `c[k] += n`
    fn add(&mut self, k: &str, n: i64) {
        match self.get_mut(k) {
            Some(v) => *v += n,
            None => self.insert(k, n),
        }
    }
    /// Counter `c[k]` (0 when absent)
    pub fn n(&self, k: &str) -> i64 {
        self.get(k).copied().unwrap_or(0)
    }
    /// Counter.most_common(limit): by count, ties in insertion order.
    fn most_common(&self, limit: Option<usize>) -> Vec<(String, i64)> {
        let mut v: Vec<(String, i64)> = self.iter().map(|(k, n)| (k.clone(), *n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        if let Some(l) = limit {
            v.truncate(l);
        }
        v
    }
}

// ------------------------------------------------------------- the model

#[derive(Clone, Debug)]
pub struct Constraint {
    pub metric: String,
    pub op: String,
    pub value: Option<f64>,
    pub text: String,
    pub raw: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub name: String,
    pub desc: Vec<String>,
    pub constraints: Vec<Constraint>,
    pub layers: Vec<String>,
    pub source_gds: Vec<(i128, Option<i128>)>,
    pub unresolved: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spec {
    Num(i128),
    Pair(i128, i128),
}

#[derive(Clone, Debug)]
pub enum VarVal {
    Num(f64),
    Str(String),
}

/// Parse result: layer tables, derivation graph, checks, stats.
#[derive(Default)]
pub struct Deck {
    pub path: String,
    pub defines: OMap<Option<String>>,
    pub variables: OMap<VarVal>,
    pub layers: OMap<Vec<Spec>>,
    pub layer_maps: Vec<(i128, Option<i128>, i128)>,
    pub derived: OMap<String>,
    pub derived_ops: OMap<Vec<String>>,
    /// every check block opened, in order (a duplicate name replaces the
    /// map entry, the earlier block stays here - Python object identity)
    arena: Vec<Check>,
    pub checks: OMap<usize>,
    pub includes: Vec<String>,
    pub warnings: Vec<String>,
    pub stats: OMap<i64>,
    pub unknown: OMap<i64>,
    pub switches: Vec<String>,
    pub switch_values: OMap<Vec<String>>,
    pub verbatim_includes: Vec<String>,
    pub env_used: OMap<String>,
    pub meas_hist: OMap<i64>,
}

impl Deck {
    pub fn check(&self, name: &str) -> Option<&Check> {
        self.checks.get(name).map(|&i| &self.arena[i])
    }

    pub fn checks_in_order(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().map(|(_, &i)| &self.arena[i])
    }

    fn gds_of_layer(&self, name: &str) -> Vec<(i128, Option<i128>)> {
        let mut out = Vec::new();
        for spec in self.layers.get(name).map(|v| v.as_slice()).unwrap_or(&[]) {
            match *spec {
                Spec::Pair(l, d) => out.push((l, Some(d))),
                Spec::Num(n) => {
                    let mapped: Vec<(i128, Option<i128>)> =
                        self.layer_maps.iter().filter(|(_, _, t)| *t == n).map(|(g, d, _)| (*g, *d)).collect();
                    if mapped.is_empty() {
                        out.push((n, None));
                    } else {
                        out.extend(mapped);
                    }
                }
            }
        }
        out
    }

    /// Fill source_gds/unresolved of every check by walking the operand
    /// graph down to LAYER names (cycle-safe).
    fn resolve(&mut self) {
        let mut results: Vec<(usize, Vec<(i128, Option<i128>)>, Vec<String>)> = Vec::new();
        for (_, &ci) in self.checks.iter() {
            let c = &self.arena[ci];
            let mut seen: HashSet<String> = HashSet::new();
            let mut gds: HashSet<(i128, Option<i128>)> = HashSet::new();
            let mut unres: Vec<String> = Vec::new();
            let mut stack: Vec<String> = c.layers.clone();
            while let Some(n) = stack.pop() {
                if !seen.insert(n.clone()) {
                    continue;
                }
                if self.layers.contains(&n) {
                    gds.extend(self.gds_of_layer(&n));
                } else if let Some(ops) = self.derived_ops.get(&n) {
                    stack.extend(ops.iter().cloned());
                } else if !self.variables.contains(&n) {
                    unres.push(n);
                }
            }
            let mut gds: Vec<(i128, Option<i128>)> = gds.into_iter().collect();
            gds.sort_by_key(|p| (p.0, p.1.unwrap_or(-1), p.1.is_some()));
            let mut unres: Vec<String> = unres.into_iter().collect::<HashSet<_>>().into_iter().collect();
            unres.sort();
            results.push((ci, gds, unres));
        }
        for (ci, gds, unres) in results {
            self.arena[ci].source_gds = gds;
            self.arena[ci].unresolved = unres;
        }
    }

    /// The sidecar (floe/svrf.py Deck.to_json).
    pub fn to_json(&self) -> J {
        let pair = |g: i128, d: Option<i128>| J::Arr(vec![J::Int(g), d.map(J::Int).unwrap_or(J::Null)]);
        let mut checks = Vec::new();
        for c in self.checks_in_order() {
            let mut e = vec![
                ("desc".to_string(), J::Str(c.desc.join("\n"))),
                ("constraints".to_string(), J::Arr(c.constraints.iter().map(constraint_json).collect())),
                ("layers".to_string(), J::Arr(c.layers.iter().map(|s| J::Str(s.clone())).collect())),
                ("source_gds".to_string(), J::Arr(c.source_gds.iter().map(|&(g, d)| pair(g, d)).collect())),
            ];
            if !c.unresolved.is_empty() {
                e.push(("unresolved".to_string(), J::Arr(c.unresolved.iter().map(|s| J::Str(s.clone())).collect())));
            }
            checks.push((c.name.clone(), J::Obj(e)));
        }
        let layers = self
            .layers
            .iter()
            .map(|(n, _)| (n.clone(), J::Arr(self.gds_of_layer(n).into_iter().map(|(g, d)| pair(g, d)).collect())))
            .collect();
        let stats = vec![
            ("files".to_string(), J::Int(self.stats.n("files") as i128)),
            ("lines".to_string(), J::Int(self.stats.n("lines") as i128)),
            ("checks".to_string(), J::Int(self.checks.len() as i128)),
            ("derivations".to_string(), J::Int(self.derived.len() as i128)),
            ("skipped".to_string(), J::Int(self.stats.n("unknown") as i128)),
            ("cmacro_calls".to_string(), J::Int(self.stats.n("cmacro") as i128)),
            ("includes".to_string(), J::Arr(self.includes.iter().map(|s| J::Str(s.clone())).collect())),
            ("env_switches".to_string(), J::Obj(self.env_used.iter().map(|(k, v)| (k.clone(), J::Str(v.clone()))).collect())),
            ("warnings".to_string(), J::Arr(self.warnings.iter().map(|s| J::Str(s.clone())).collect())),
        ];
        J::Obj(vec![
            ("format".to_string(), J::Str(FORMAT.to_string())),
            ("version".to_string(), J::Int(VERSION as i128)),
            ("deck".to_string(), J::Str(abspath(&self.path))),
            ("generated_by".to_string(), J::Str(format!("floe-index {}", env!("CARGO_PKG_VERSION")))),
            ("defines".to_string(), J::Obj(self.defines.iter().map(|(k, v)| (k.clone(), opt_str(v))).collect())),
            ("variables".to_string(), J::Obj(self.variables.iter().map(|(k, v)| (k.clone(), var_json(v))).collect())),
            ("layers".to_string(), J::Obj(layers)),
            ("derived".to_string(), J::Obj(self.derived.iter().map(|(k, v)| (k.clone(), J::Str(v.clone()))).collect())),
            ("checks".to_string(), J::Obj(checks)),
            ("stats".to_string(), J::Obj(stats)),
        ])
    }

    /// The whole parse state, for the gate (tools/validate_svrf.py): what
    /// the Python gate read off the parser object before the port.
    pub fn state_json(&self) -> J {
        let pair = |g: i128, d: Option<i128>| J::Arr(vec![J::Int(g), d.map(J::Int).unwrap_or(J::Null)]);
        let strs = |v: &[String]| J::Arr(v.iter().map(|s| J::Str(s.clone())).collect());
        let counter = |c: &OMap<i64>| J::Arr(c.iter().map(|(k, n)| J::Arr(vec![J::Str(k.clone()), J::Int(*n as i128)])).collect());
        let checks = self
            .checks_in_order()
            .map(|c| {
                J::Obj(vec![
                    ("name".to_string(), J::Str(c.name.clone())),
                    ("desc".to_string(), strs(&c.desc)),
                    ("constraints".to_string(), J::Arr(c.constraints.iter().map(constraint_json).collect())),
                    ("layers".to_string(), strs(&c.layers)),
                    ("source_gds".to_string(), J::Arr(c.source_gds.iter().map(|&(g, d)| pair(g, d)).collect())),
                    ("unresolved".to_string(), strs(&c.unresolved)),
                ])
            })
            .collect();
        let spec = |s: &Spec| match *s {
            Spec::Num(n) => J::Arr(vec![J::Int(n)]),
            Spec::Pair(l, d) => J::Arr(vec![J::Int(l), J::Int(d)]),
        };
        J::Obj(vec![
            ("checks".to_string(), J::Arr(checks)),
            ("layers".to_string(), J::Arr(self.layers.iter().map(|(k, v)| J::Arr(vec![J::Str(k.clone()), J::Arr(v.iter().map(spec).collect())])).collect())),
            ("defines".to_string(), J::Arr(self.defines.iter().map(|(k, v)| J::Arr(vec![J::Str(k.clone()), opt_str(v)])).collect())),
            ("variables".to_string(), J::Arr(self.variables.iter().map(|(k, v)| J::Arr(vec![J::Str(k.clone()), var_json(v)])).collect())),
            ("derived".to_string(), J::Arr(self.derived.iter().map(|(k, v)| J::Arr(vec![J::Str(k.clone()), J::Str(v.clone())])).collect())),
            ("derived_ops".to_string(), J::Arr(self.derived_ops.iter().map(|(k, v)| J::Arr(vec![J::Str(k.clone()), strs(v)])).collect())),
            ("layer_maps".to_string(), J::Arr(self.layer_maps.iter().map(|&(g, d, t)| J::Arr(vec![J::Int(g), d.map(J::Int).unwrap_or(J::Null), J::Int(t)])).collect())),
            ("includes".to_string(), strs(&self.includes)),
            ("warnings".to_string(), strs(&self.warnings)),
            ("stats".to_string(), counter(&self.stats)),
            ("unknown".to_string(), counter(&self.unknown)),
            ("meas_hist".to_string(), counter(&self.meas_hist)),
            ("switches".to_string(), strs(&self.switches)),
            ("switch_values".to_string(), J::Arr(self.switch_values.iter().map(|(k, v)| J::Arr(vec![J::Str(k.clone()), strs(v)])).collect())),
            ("verbatim_includes".to_string(), strs(&self.verbatim_includes)),
            ("env_used".to_string(), J::Arr(self.env_used.iter().map(|(k, v)| J::Arr(vec![J::Str(k.clone()), J::Str(v.clone())])).collect())),
            ("scan".to_string(), J::Str(format_scan(self))),
            // the operator words (the viewer's floe/svrf.py rhs_operands
            // must split derivations with the same list; the gate holds them
            // equal)
            ("keywords".to_string(), {
                let mut k: Vec<&str> = Words::new().keywords.into_iter().collect();
                k.sort();
                J::Arr(k.into_iter().map(|w| J::Str(w.to_string())).collect())
            }),
        ])
    }
}

fn opt_str(v: &Option<String>) -> J {
    v.as_ref().map(|s| J::Str(s.clone())).unwrap_or(J::Null)
}

fn var_json(v: &VarVal) -> J {
    match v {
        VarVal::Num(f) => J::Float(*f),
        VarVal::Str(s) => J::Str(s.clone()),
    }
}

fn constraint_json(c: &Constraint) -> J {
    let mut e = vec![
        ("metric".to_string(), J::Str(c.metric.clone())),
        ("op".to_string(), J::Str(c.op.clone())),
        ("value".to_string(), c.value.map(J::Float).unwrap_or(J::Null)),
        ("text".to_string(), J::Str(c.text.clone())),
    ];
    if let Some(raw) = &c.raw {
        e.push(("raw".to_string(), J::Str(raw.clone())));
    }
    J::Obj(e)
}

// ----------------------------------------------------------------- JSON

/// A JSON value written the way Python's `json.dump(v, indent=1,
/// sort_keys=True)` writes it.
#[derive(Clone, Debug)]
pub enum J {
    Null,
    Int(i128),
    Float(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

/// Python `float.__repr__` (the shortest round-trip digits; exponent form
/// below 1e-4 and from 1e16, as `1e-05` / `1.5e+16`).
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    let e_form = format!("{:e}", x.abs());
    let (mant, exp) = e_form.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let n = digits.len() as i32;
    let decpt = exp + 1;
    let sign = if x < 0.0 { "-" } else { "" };
    let body = if decpt <= -4 || decpt > 16 {
        let m = if n == 1 { digits.clone() } else { format!("{}.{}", &digits[..1], &digits[1..]) };
        let e = exp;
        format!("{}e{}{:02}", m, if e < 0 { '-' } else { '+' }, e.abs())
    } else if decpt <= 0 {
        format!("0.{}{}", "0".repeat((-decpt) as usize), digits)
    } else if decpt < n {
        format!("{}.{}", &digits[..decpt as usize], &digits[decpt as usize..])
    } else {
        format!("{}{}.0", digits, "0".repeat((decpt - n) as usize))
    };
    format!("{}{}", sign, body)
}

/// Python json's `ensure_ascii` string encoding.
pub fn py_json_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
        }
    }
    out.push('"');
}

impl J {
    fn write(&self, level: usize, out: &mut String) {
        match self {
            J::Null => out.push_str("null"),
            J::Int(i) => out.push_str(&i.to_string()),
            J::Float(f) => out.push_str(&py_float_repr(*f)),
            J::Str(s) => py_json_str(s, out),
            J::Arr(v) => {
                if v.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push_str("[\n");
                for (k, item) in v.iter().enumerate() {
                    if k > 0 {
                        out.push_str(",\n");
                    }
                    out.push_str(&" ".repeat(level + 1));
                    item.write(level + 1, out);
                }
                out.push('\n');
                out.push_str(&" ".repeat(level));
                out.push(']');
            }
            J::Obj(v) => {
                if v.is_empty() {
                    out.push_str("{}");
                    return;
                }
                let mut items: Vec<&(String, J)> = v.iter().collect();
                items.sort_by(|a, b| a.0.cmp(&b.0));
                out.push_str("{\n");
                for (k, (key, item)) in items.iter().enumerate() {
                    if k > 0 {
                        out.push_str(",\n");
                    }
                    out.push_str(&" ".repeat(level + 1));
                    py_json_str(key, out);
                    out.push_str(": ");
                    item.write(level + 1, out);
                }
                out.push('\n');
                out.push_str(&" ".repeat(level));
                out.push('}');
            }
        }
    }

    /// The document plus the trailing newline Python's writer added.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        self.write(0, &mut out);
        out.push('\n');
        out
    }
}

// ------------------------------------------------------------- os.path

fn env_get(key: &str) -> Option<String> {
    if key.is_empty() || key.contains('=') || key.contains('\0') {
        return None;
    }
    std::env::var_os(key).map(|v| v.to_string_lossy().into_owned())
}

/// posixpath.expandvars: `$name` / `${name}` from the environment; an
/// unset one keeps its text.
pub fn expandvars(path: &str) -> String {
    if !path.contains('$') {
        return path.to_string();
    }
    let mut cs: Vec<char> = path.chars().collect();
    let mut i = 0;
    loop {
        // the next `$` that starts a match
        let mut found: Option<(usize, usize, String)> = None;
        let mut p = i;
        while p < cs.len() {
            if cs[p] == '$' {
                let q = p + 1;
                let mut e = q;
                while e < cs.len() && (cs[e].is_ascii_alphanumeric() || cs[e] == '_') {
                    e += 1;
                }
                if e > q {
                    found = Some((p, e, cs[q..e].iter().collect()));
                    break;
                }
                if cs.get(q) == Some(&'{') {
                    if let Some(off) = cs[q + 1..].iter().position(|&c| c == '}') {
                        let end = q + 1 + off + 1;
                        found = Some((p, end, cs[q + 1..end - 1].iter().collect()));
                        break;
                    }
                }
            }
            p += 1;
        }
        let Some((start, end, name)) = found else {
            break;
        };
        match env_get(&name) {
            Some(value) => {
                let tail: Vec<char> = cs[end..].to_vec();
                cs.truncate(start);
                cs.extend(value.chars());
                i = cs.len();
                cs.extend(tail);
            }
            None => i = end,
        }
    }
    cs.into_iter().collect()
}

fn home_dir_of(user: Option<&str>) -> Option<String> {
    use std::ffi::{CStr, CString};
    // SAFETY: getpwnam/getpwuid return a pointer to static storage or
    // null; the fields are read before any other passwd call.
    unsafe {
        let pw = match user {
            Some(name) => {
                let c = CString::new(name).ok()?;
                libc::getpwnam(c.as_ptr())
            }
            None => libc::getpwuid(libc::getuid()),
        };
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned())
    }
}

/// posixpath.expanduser: `~` = $HOME (else the password entry), `~user`.
pub fn expanduser(path: &str) -> String {
    if !path.starts_with('~') {
        return path.to_string();
    }
    let i = path[1..].find('/').map(|k| k + 1).unwrap_or(path.len());
    let home = if i == 1 {
        match env_get("HOME") {
            Some(h) => h,
            None => match home_dir_of(None) {
                Some(h) => h,
                None => return path.to_string(),
            },
        }
    } else {
        match home_dir_of(Some(&path[1..i])) {
            Some(h) => h,
            None => return path.to_string(),
        }
    };
    let home = home.trim_end_matches('/');
    let r = format!("{}{}", home, &path[i..]);
    if r.is_empty() {
        "/".to_string()
    } else {
        r
    }
}

fn pjoin(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{}{}", a, b)
    } else {
        format!("{}/{}", a, b)
    }
}

fn dirname(p: &str) -> String {
    let i = p.rfind('/').map(|k| k + 1).unwrap_or(0);
    let head = &p[..i];
    if !head.is_empty() && head.chars().any(|c| c != '/') {
        head.trim_end_matches('/').to_string()
    } else {
        head.to_string()
    }
}

fn basename(p: &str) -> &str {
    let i = p.rfind('/').map(|k| k + 1).unwrap_or(0);
    &p[i..]
}

pub fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let mut initial = if path.starts_with('/') { 1 } else { 0 };
    if initial == 1 && path.starts_with("//") && !path.starts_with("///") {
        initial = 2;
    }
    let mut comps: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial == 0 && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let joined = format!("{}{}", "/".repeat(initial), comps.join("/"));
    if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

pub fn abspath(path: &str) -> String {
    if path.starts_with('/') {
        return normpath(path);
    }
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    normpath(&pjoin(&cwd, path))
}

fn realpath(path: &str) -> String {
    std::fs::canonicalize(path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| abspath(path))
}

fn is_file(p: &str) -> bool {
    std::path::Path::new(p).is_file()
}

fn find_include(tgt: &str, src: &str, incdirs: &[String]) -> Option<String> {
    if tgt.is_empty() {
        return None;
    }
    // Calibre expands $VAR / ${VAR} (and ~) in INCLUDE paths
    let tgt = expanduser(&expandvars(tgt));
    let cands: Vec<String> = if tgt.starts_with('/') {
        vec![tgt]
    } else {
        std::iter::once(pjoin(&dirname(src), &tgt)).chain(incdirs.iter().map(|d| pjoin(d, &tgt))).collect()
    };
    cands.into_iter().find(|c| is_file(c))
}

/// Python's `str(OSError)` for an open failure: `[Errno N] text: 'path'`.
fn os_error_text(e: &std::io::Error, path: &str) -> String {
    let text = e.to_string();
    match e.raw_os_error() {
        Some(n) => {
            let base = text.split(" (os error").next().unwrap_or(&text).to_string();
            format!("[Errno {}] {}: '{}'", n, base, path)
        }
        None => text,
    }
}

/// A text file as Python reads it: UTF-8 (invalid bytes replaced),
/// universal newlines, split into lines.
fn read_lines(path: &str) -> std::io::Result<Vec<String>> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n").replace('\r', "\n");
    Ok(text.split_inclusive('\n').map(|l| l.to_string()).collect())
}

// ------------------------------------------------------------- the parser

pub struct Options {
    pub defines: Vec<(String, Option<String>)>,
    pub include_dirs: Vec<String>,
    pub scan_all: bool,
    pub follow_verbatim: bool,
    pub env_switches: bool,
}

struct Cont {
    check: usize,
    metric: String,
    text: String,
    had_bound: bool,
}

struct Parser {
    d: Deck,
    w: Words,
    scan_all: bool,
    /// stack of booleans (active branch?)
    cond: Vec<bool>,
    /// the open check (arena index)
    cur: Option<usize>,
    depth: i64,
    /// >0: inside a DMACRO body (skipped)
    macro_depth: i64,
    /// DMACRO header seen, body { on a LATER line
    macro_pending: bool,
    /// >0: inside a VERBATIM/Tcl block
    verbatim_depth: i64,
    /// inside a /* ... */ banner
    in_comment: bool,
    /// last statement was IGNORED: its wrapped continuation lines are
    /// classified quietly too
    icont: bool,
    follow_verbatim: bool,
    env_switches: bool,
    /// the last measurement - a comparator-leading next line continues it
    cont: Option<Cont>,
    /// (lhs, check) of the last assign - wrapped derivations continue on
    /// operator-leading lines
    acont: Option<(String, Option<usize>)>,
    /// rhs ended with an operator: the NEXT line continues regardless
    acont_open: bool,
    /// the #DEFINE substitution: names longest first, and their values
    sub_names: Vec<String>,
    sub_map: HashMap<String, String>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Parser {
    fn active(&self) -> bool {
        self.cond.iter().all(|&b| b)
    }

    fn cur_check(&mut self) -> &mut Check {
        let i = self.cur.unwrap();
        &mut self.d.arena[i]
    }

    /// (defined, value) of a preprocessor switch: -D / #DEFINE first, then
    /// the ENVIRONMENT - the in-house flow exports every deck switch via
    /// `source sourceme.*` (user call 2026-08-18), so names the deck TESTS
    /// are looked up lazily (never a bulk import). A hit is promoted into
    /// defines (value substitution + provenance in env_used); -D wins.
    fn switch_val(&mut self, name: &str) -> (bool, Option<String>) {
        if let Some(v) = self.d.defines.get(name) {
            return (true, v.clone());
        }
        if self.env_switches {
            let key = name.strip_prefix('$').unwrap_or(name);
            if let Some(v) = env_get(key) {
                self.d.env_used.insert(name, v.clone());
                let val = if v.is_empty() { None } else { Some(v) };
                if !name.starts_with('$') {
                    self.d.defines.insert(name, val.clone());
                    self.rebuild_sub();
                }
                return (true, val);
            }
        }
        (false, None)
    }

    fn rebuild_sub(&mut self) {
        let vals: Vec<(String, String)> = self
            .d
            .defines
            .iter()
            .filter_map(|(n, v)| v.as_ref().filter(|s| !s.is_empty()).map(|s| (n.clone(), s.clone())))
            .collect();
        let mut names: Vec<String> = vals.iter().map(|(n, _)| n.clone()).collect();
        // sorted(key=len, reverse=True) is stable: equal lengths keep the
        // defines' order
        names.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()));
        self.sub_names = names;
        self.sub_map = vals.into_iter().collect();
    }

    /// `\b(name|...)\b` -> value, left to right, names tried longest first.
    fn substitute(&self, s: &str) -> String {
        let cs: Vec<char> = s.chars().collect();
        let names: Vec<Vec<char>> = self.sub_names.iter().map(|n| n.chars().collect()).collect();
        let bnd = |i: usize| -> bool {
            let before = i > 0 && is_word(cs[i - 1]);
            let after = i < cs.len() && is_word(cs[i]);
            before != after
        };
        let mut out = String::with_capacity(s.len());
        let mut i = 0;
        while i < cs.len() {
            let mut hit = None;
            if bnd(i) {
                for (k, n) in names.iter().enumerate() {
                    if i + n.len() <= cs.len() && cs[i..i + n.len()] == n[..] && bnd(i + n.len()) {
                        hit = Some(k);
                        break;
                    }
                }
            }
            match hit {
                Some(k) => {
                    out.push_str(&self.sub_map[&self.sub_names[k]]);
                    i += names[k].len();
                }
                None => {
                    out.push(cs[i]);
                    i += 1;
                }
            }
        }
        out
    }

    fn directive(&mut self, s: &str) {
        // comments ride on directive lines too: `#DEFINE W 5 // um`
        let s = py_strip(s.split("//").next().unwrap_or(""));
        if s.is_empty() {
            return;
        }
        let tok = py_split(s);
        let head = upper(tok[0]);
        let unq = |t: &str| -> String {
            let cs: Vec<char> = t.chars().collect();
            if cs.len() >= 2 && (cs[0] == '"' || cs[0] == '\'') && cs[cs.len() - 1] == cs[0] {
                cs[1..cs.len() - 1].iter().collect()
            } else {
                t.to_string()
            }
        };
        if head == "#DEFINE" && tok.len() >= 2 {
            if self.active() || self.scan_all {
                let val = unq(&tok[2..].join(" "));
                self.d.defines.insert(tok[1], if val.is_empty() { None } else { Some(val) });
                self.rebuild_sub();
            }
        } else if head == "#UNDEFINE" && tok.len() >= 2 {
            if self.active() || self.scan_all {
                self.d.defines.remove(tok[1]);
                self.rebuild_sub();
            }
        } else if (head == "#IFDEF" || head == "#IFNDEF") && tok.len() >= 2 {
            let name = tok[1];
            push_unique(&mut self.d.switches, name);
            let mut on = if tok.len() >= 3 {
                // Calibre two-arg form: true iff NAME is defined AND its
                // value equals the literal
                let want = unq(&tok[2..].join(" "));
                match self.d.switch_values.get_mut(name) {
                    Some(v) => push_unique(v, &want),
                    None => self.d.switch_values.insert(name, vec![want.clone()]),
                }
                let (ok, cur) = self.switch_val(name);
                ok && cur.as_deref() == Some(want.as_str())
            } else {
                self.switch_val(name).0
            };
            if head == "#IFNDEF" {
                on = !on;
            }
            self.cond.push(on || self.scan_all);
        } else if head == "#ELSE" {
            match self.cond.last_mut() {
                Some(last) => *last = !*last || self.scan_all,
                None => self.d.warnings.push("#ELSE without #IFDEF".to_string()),
            }
        } else if head == "#ENDIF" {
            if self.cond.pop().is_none() {
                self.d.warnings.push("#ENDIF without #IFDEF".to_string());
            }
        } else {
            self.d.stats.add("unknown_directive", 1);
        }
    }

    fn include_not_found(&mut self, tgt: &str, in_verbatim: bool) {
        let exp = expanduser(&expandvars(tgt));
        let mut msg = if in_verbatim {
            format!("INCLUDE (in VERBATIM) not found: {}", tgt)
        } else {
            format!("INCLUDE not found: {}", tgt)
        };
        if exp != tgt {
            msg.push_str(&format!(" -> {}", exp));
        }
        if exp.contains('$') {
            msg.push_str(" (env var unset in this shell?)");
        }
        self.d.warnings.push(msg);
    }

    fn feed_file(&mut self, path: &str, incdirs: &[String], chain: &[String]) {
        let real = realpath(path);
        if chain.contains(&real) {
            self.d.warnings.push(format!("INCLUDE cycle: {}", path));
            return;
        }
        let lines = match read_lines(path) {
            Ok(l) => l,
            Err(e) => {
                self.d.warnings.push(format!("INCLUDE unreadable: {} ({})", path, os_error_text(&e, path)));
                return;
            }
        };
        self.d.stats.add("files", 1);
        let mut sub_chain: Vec<String> = chain.to_vec();
        sub_chain.push(real);
        for line in &lines {
            self.d.stats.add("lines", 1);
            let mut s: String = py_strip(line).to_string();
            if s.is_empty() {
                continue;
            }
            // /* ... */ block comments
            if self.in_comment {
                match s.find("*/") {
                    None => continue,
                    Some(j) => {
                        s = py_strip(&s[j + 2..]).to_string();
                        self.in_comment = false;
                        if s.is_empty() {
                            continue;
                        }
                    }
                }
            }
            while !s.starts_with('@') && s.contains("/*") {
                let i = s.find("/*").unwrap();
                match s[i + 2..].find("*/") {
                    None => {
                        s = py_rstrip(&s[..i]).to_string();
                        self.in_comment = true;
                        break;
                    }
                    Some(off) => {
                        let j = i + 2 + off;
                        s = py_strip(&format!("{} {}", &s[..i], &s[j + 2..])).to_string();
                    }
                }
            }
            if s.is_empty() {
                continue;
            }
            if s.starts_with('#') {
                self.directive(&s);
                continue;
            }
            if !self.active() {
                continue;
            }
            if !s.starts_with('@') {
                s = py_strip(s.split("//").next().unwrap_or("")).to_string();
                if s.is_empty() {
                    continue;
                }
            }
            if !self.sub_names.is_empty() && !s.starts_with('@') {
                s = self.substitute(&s);
            }
            let (tok_head, tok_rest) = py_split1(&s);
            let is_include = tok_head.map(|h| upper(h) == "INCLUDE").unwrap_or(false);
            let target = || -> String {
                tok_rest.map(|r| py_strip(r).trim_matches(|c| c == '"' || c == '\'').to_string()).unwrap_or_default()
            };
            if is_include && self.verbatim_depth > 0 {
                // Tcl-conditional include: inventory it always, dive into
                // it only under --scan / --follow-verbatim
                let tgt = target();
                if !tgt.is_empty() {
                    push_unique(&mut self.d.verbatim_includes, &tgt);
                }
                if (self.scan_all || self.follow_verbatim) && !tgt.is_empty() {
                    match find_include(&tgt, path, incdirs) {
                        Some(inc) => {
                            self.d.includes.push(inc.clone());
                            let sav = self.verbatim_depth;
                            self.verbatim_depth = 0;
                            self.feed_file(&inc, incdirs, &sub_chain);
                            self.verbatim_depth = sav;
                        }
                        None => self.include_not_found(&tgt, true),
                    }
                }
                continue;
            }
            if is_include && (self.cur.is_some() || self.macro_depth > 0 || self.macro_pending) {
                // an INCLUDE textually inside a macro body or an open
                // block never executes: say so instead of losing the file
                let what = if self.macro_depth > 0 || self.macro_pending {
                    "a DMACRO body".to_string()
                } else {
                    format!("open block {}", self.d.arena[self.cur.unwrap()].name)
                };
                self.d.warnings.push(format!("INCLUDE swallowed by {}: {}", what, s));
                self.statement(&s);
                continue;
            }
            if is_include {
                let tgt = target();
                match find_include(&tgt, path, incdirs) {
                    Some(inc) => {
                        self.d.includes.push(inc.clone());
                        self.feed_file(&inc, incdirs, &sub_chain);
                    }
                    None => self.include_not_found(&tgt, false),
                }
                continue;
            }
            self.statement(&s);
        }
        // parse state open at a file boundary is always an anomaly
        let base = basename(path).to_string();
        if let Some(ci) = self.cur {
            let name = self.d.arena[ci].name.clone();
            self.d.warnings.push(format!("unclosed block {} at end of {}", name, base));
        }
        if self.macro_depth > 0 || self.macro_pending {
            self.d.warnings.push(format!("unclosed DMACRO body at end of {} (brace drift?)", base));
        }
        if self.verbatim_depth > 0 {
            // contain Tcl brace drift to the file it happened in
            self.d.warnings.push(format!("unclosed VERBATIM/Tcl block at end of {} (brace drift?)", base));
            self.verbatim_depth = 0;
        }
        if self.in_comment {
            self.d.warnings.push(format!("unclosed /* comment at end of {}", base));
            self.in_comment = false;
        }
        if !self.cond.is_empty() && chain.is_empty() {
            self.d.warnings.push("unbalanced #IFDEF at end of deck".to_string());
        }
    }

    fn brace_delta(s: &str) -> i64 {
        s.matches('{').count() as i64 - s.matches('}').count() as i64
    }

    fn statement(&mut self, s: &str) {
        if self.verbatim_depth > 0 {
            // VERBATIM/Tcl body: pure brace tracking
            self.verbatim_depth = (self.verbatim_depth + Self::brace_delta(s)).max(0);
            return;
        }
        if self.macro_depth > 0 || self.macro_pending {
            // inside (or awaiting) a DMACRO body: skip, track braces
            let d = Self::brace_delta(s);
            if self.macro_pending {
                if !s.contains('{') {
                    return; // header continuation line
                }
                self.macro_pending = false;
                self.macro_depth = d.max(0);
            } else {
                self.macro_depth = (self.macro_depth + d).max(0);
            }
            return;
        }
        if self.cur.is_some() {
            self.block_line(s);
            return;
        }
        if self.try_cont(s) {
            return;
        }
        self.cont = None;
        let m = check_match(s);
        if let Some((name, rest)) = &m {
            if self.w.noncheck.contains(upper(name).as_str()) {
                // VERBATIM / Tcl control block: never a check
                self.d.stats.add("verbatim", 1);
                self.verbatim_depth = (1 + Self::brace_delta(rest)).max(0);
                self.cont = None;
                self.acont = None;
                return;
            }
        }
        if let Some((name, rest)) = m {
            if !self.w.ignored.contains(upper(&name).as_str()) && assign_match(s).is_none() {
                if self.d.checks.contains(&name) {
                    self.d.warnings.push(format!("duplicate check {} (kept last)", name));
                }
                let ci = self.d.arena.len();
                self.d.arena.push(Check {
                    name: name.clone(),
                    desc: Vec::new(),
                    constraints: Vec::new(),
                    layers: Vec::new(),
                    source_gds: Vec::new(),
                    unresolved: Vec::new(),
                });
                self.d.checks.insert(&name, ci);
                self.cur = Some(ci);
                self.depth = 1;
                self.acont = None;
                self.icont = false;
                let rest = py_strip(&rest).to_string();
                if !rest.is_empty() {
                    self.block_line(&rest);
                }
                return;
            }
        }
        let head = head_upper(s);
        if head.is_empty() {
            return; // (Python would fail on a blank substituted line)
        }
        if head == "LAYER" {
            self.acont = None;
            self.icont = false;
            self.layer_stmt(s);
        } else if head == "VARIABLE" {
            self.acont = None;
            self.icont = false;
            self.variable_stmt(s);
        } else if head == "DMACRO" {
            // macros are NOT expanded; the body is skipped by brace depth
            self.acont = None;
            self.icont = false;
            self.d.stats.add("dmacro", 1);
            let d = Self::brace_delta(s);
            if s.contains('{') {
                if d > 0 {
                    self.macro_depth = d;
                }
            } else {
                self.macro_pending = true;
            }
        } else if head == "CMACRO" {
            self.acont = None;
            self.d.stats.add("cmacro", 1);
        } else if let Some((lhs, rhs)) = assign_match(s) {
            self.assign(&lhs, &rhs, None);
        } else if self.try_acont(s) {
            self.icont = false;
        } else if self.w.ignored.contains(head.as_str()) {
            self.acont = None;
            self.icont = true;
            self.d.stats.add("ignored", 1);
        } else if head.starts_with(['[', '~', '(']) {
            // DFM property math / expression continuations
            self.acont = None;
            self.d.stats.add("prop_expr", 1);
        } else if self.icont {
            // wrapped continuation of an ignored statement
            self.d.stats.add("ignored", 1);
        } else {
            self.acont = None;
            self.d.stats.add("unknown", 1);
            self.d.unknown.add(&head, 1);
        }
    }

    fn block_line(&mut self, s: &str) {
        if s.starts_with('@') {
            let mut t: String = py_strip(s.trim_start_matches('@')).to_string();
            let cs: Vec<char> = t.chars().collect();
            if cs.len() >= 2 && cs[0] == '"' && cs[cs.len() - 1] == '"' {
                t = cs[1..cs.len() - 1].iter().collect();
            }
            self.cur_check().desc.push(t);
            return;
        }
        let mut s: String = s.to_string();
        while s.starts_with('}') {
            self.depth -= 1;
            s = py_lstrip(&s[1..]).to_string();
            if self.depth <= 0 {
                self.cur = None;
                self.cont = None;
                if !s.is_empty() {
                    self.statement(&s);
                }
                return;
            }
        }
        if s.is_empty() {
            return;
        }
        let mut closes = 0i64;
        while s.ends_with('}') {
            closes += 1;
            s = py_rstrip(&s[..s.len() - 1]).to_string();
        }
        let nested = if s.is_empty() { 0 } else { Self::brace_delta(&s) };
        if !s.is_empty() {
            if self.try_cont(&s) {
                // continued the previous measurement
            } else if let Some((lhs, rhs)) = assign_match(&s) {
                self.cont = None;
                let cur = self.cur;
                self.assign(&lhs, &rhs, cur);
            } else {
                let head = head_upper(&s);
                if meas_metric(&head).is_some() {
                    self.acont = None;
                    self.icont = false;
                    let cur = self.cur;
                    self.measurement(&s, cur);
                } else if self.try_acont(&s) {
                    self.icont = false;
                } else if self.cont.is_some() && self.w.keyword(&head) {
                    // operator-leading wrap of the previous measurement:
                    // its operands join the check (source closure)
                    let ck = self.cont.as_ref().unwrap().check;
                    for n in rhs_operands(&self.w, &s) {
                        push_unique(&mut self.d.arena[ck].layers, &n);
                    }
                    self.icont = false;
                } else if self.w.ignored.contains(head.as_str()) {
                    // DFM RDB / spec statements inside checks
                    self.cont = None;
                    self.acont = None;
                    self.icont = true;
                    self.d.stats.add("ignored", 1);
                } else if head.starts_with(['[', '~', '(']) {
                    // DFM property math wraps ([expr], ~(..))
                    self.cont = None;
                    self.acont = None;
                    self.d.stats.add("prop_expr", 1);
                } else if self.icont {
                    self.cont = None;
                    self.d.stats.add("ignored", 1);
                } else {
                    self.cont = None;
                    self.acont = None;
                    self.d.stats.add("unknown_in_block", 1);
                    self.d.unknown.add(&head, 1);
                }
            }
        }
        self.depth += nested - closes;
        if self.depth <= 0 {
            self.cur = None;
            self.cont = None;
        }
    }

    fn layer_stmt(&mut self, s: &str) {
        let tok = py_split(s);
        if tok.len() >= 2 && upper(tok[1]) == "MAP" {
            let ints: Vec<i128> = tok[2..].iter().filter(|t| is_int(t)).filter_map(|t| parse_int(t)).collect();
            let has_dt = tok.iter().any(|t| upper(t) == "DATATYPE");
            if ints.len() >= 2 {
                let (gds, tgt) = (ints[0], ints[ints.len() - 1]);
                let dts: Vec<Option<i128>> =
                    if has_dt { ints[1..ints.len() - 1].iter().map(|&d| Some(d)).collect() } else { vec![None] };
                let dts = if dts.is_empty() { vec![None] } else { dts };
                for dt in dts {
                    self.d.layer_maps.push((gds, dt, tgt));
                }
            }
            self.d.stats.add("layer_map", 1);
            return;
        }
        if tok.len() < 3 {
            self.d.stats.add("unknown", 1);
            return;
        }
        let name = tok[1];
        let mut specs = Vec::new();
        for t in &tok[2..] {
            if is_uint(t) {
                if let Some(n) = parse_int(t) {
                    specs.push(Spec::Num(n));
                }
            } else if let Some((l, d)) = t.split_once('.') {
                if is_uint(l) && is_uint(d) {
                    if let (Some(l), Some(d)) = (parse_int(l), parse_int(d)) {
                        specs.push(Spec::Pair(l, d));
                    }
                }
            }
        }
        if !specs.is_empty() {
            self.d.layers.insert(name, specs);
            self.d.stats.add("layer", 1);
        } else {
            self.d.stats.add("unknown", 1);
            self.d.unknown.add("LAYER", 1);
        }
    }

    fn variable_stmt(&mut self, s: &str) {
        let tok = py_split(s);
        if tok.len() >= 3 {
            let v = if is_num(tok[2]) { tok[2].parse::<f64>().ok() } else { None };
            let value = match v {
                Some(f) => VarVal::Num(f),
                None => VarVal::Str(tok[2..].join(" ")),
            };
            self.d.variables.insert(tok[1], value);
        }
    }

    fn to_num(&self, tok: &str) -> Option<f64> {
        if is_num(tok) {
            return tok.parse::<f64>().ok();
        }
        match self.d.variables.get(tok) {
            Some(VarVal::Num(f)) => Some(*f),
            _ => None,
        }
    }

    /// name = expr. Operators ignored: RHS identifier names, minus
    /// keywords/numbers, are the graph edges. A measurement RHS inside a
    /// check block ALSO records its constraint.
    fn assign(&mut self, lhs: &str, rhs: &str, check: Option<usize>) {
        self.d.derived.insert(lhs, py_strip(rhs).to_string());
        let head = head_upper(rhs);
        if meas_metric(&head).is_some() {
            let ops = self.measurement(rhs, check);
            self.d.derived_ops.insert(lhs, ops);
            self.acont = None;
        } else {
            let names = rhs_operands(&self.w, rhs);
            if let Some(ci) = check {
                for n in &names {
                    push_unique(&mut self.d.arena[ci].layers, n);
                }
            }
            self.d.derived_ops.insert(lhs, names);
            self.icont = false;
            // the rhs may wrap onto following lines
            let toks = py_split(rhs);
            self.acont = Some((lhs.to_string(), check));
            self.acont_open = toks.last().map(|t| self.w.keyword(&upper(t))).unwrap_or(false);
        }
        self.d.stats.add("assign", 1);
    }

    /// Continuation line of a wrapped derivation: taken when the previous
    /// assign's rhs ended with an operator, or this line LEADS with one.
    fn try_acont(&mut self, s: &str) -> bool {
        let Some((lhs, check)) = self.acont.clone() else {
            return false;
        };
        let head = head_upper(s);
        if !(self.acont_open || self.w.keyword(&head)) {
            return false;
        }
        let joined = format!("{} {}", self.d.derived.get(&lhs).cloned().unwrap_or_default(), py_strip(s));
        self.d.derived.insert(&lhs, joined);
        let found = rhs_operands(&self.w, s);
        let ops_now: Vec<String> = self.d.derived_ops.get(&lhs).cloned().unwrap_or_default();
        let names: Vec<String> = found.into_iter().filter(|n| !ops_now.contains(n)).collect();
        match self.d.derived_ops.get_mut(&lhs) {
            Some(ops) => ops.extend(names.iter().cloned()),
            None => self.d.derived_ops.insert(&lhs, names.clone()),
        }
        if let Some(ci) = check {
            for n in &names {
                push_unique(&mut self.d.arena[ci].layers, n);
            }
        }
        let toks = py_split(s);
        self.acont_open = toks.last().map(|t| self.w.keyword(&upper(t))).unwrap_or(false);
        true
    }

    fn add_bounds(&mut self, check: usize, metric: &str, bounds: &[(String, String)], text: &str) {
        for (op, tok) in bounds {
            let value = self.to_num(tok);
            let raw = if value.is_none() { Some(tok.clone()) } else { None };
            self.d.arena[check].constraints.push(Constraint {
                metric: metric.to_string(),
                op: op.clone(),
                value,
                text: text.to_string(),
                raw,
            });
        }
    }

    /// A comparator-leading line continues the previous measurement
    /// statement - real decks wrap the constraint onto its own line.
    fn try_cont(&mut self, s: &str) -> bool {
        if self.cont.is_none() {
            return false;
        }
        let cs: Vec<char> = s.chars().collect();
        if !op_at(&cs, 0) {
            return false;
        }
        let bounds = chain(&cs, 0);
        if bounds.is_empty() {
            return false;
        }
        let c = self.cont.take().unwrap();
        let text = format!("{} {}", c.text, py_strip(s));
        self.add_bounds(c.check, &c.metric, &bounds, &text);
        if !c.had_bound {
            self.d.stats.add("meas_no_bound", -1);
        }
        self.cont = Some(Cont { check: c.check, metric: c.metric, text, had_bound: true });
        true
    }

    /// INTERNAL/EXTERNAL/... [layers] op value [op value] opts. Returns the
    /// operand names (for assignment RHS reuse).
    fn measurement(&mut self, s: &str, check: Option<usize>) -> Vec<String> {
        let (h, rest) = py_split1(s);
        let head = upper(h.unwrap_or(""));
        let rest = rest.unwrap_or("");
        let metric = meas_metric(&head).unwrap_or("").to_string();
        self.d.meas_hist.add(&head, 1);
        let rc: Vec<char> = rest.chars().collect();
        let m = op_search(&rc);
        let headpart: String = match m {
            Some(p) => rc[..p].iter().collect(),
            None => rest.to_string(),
        };
        let ops: Vec<String> = id_findall(&headpart).into_iter().filter(|t| !self.w.keyword(&upper(t))).collect();
        if let Some(ci) = check {
            for n in &ops {
                push_unique(&mut self.d.arena[ci].layers, n);
            }
            let bounds = match m {
                Some(p) => chain(&rc, p),
                None => Vec::new(),
            };
            let text = py_strip(s).to_string();
            self.add_bounds(ci, &metric, &bounds, &text);
            if bounds.is_empty() {
                self.d.stats.add("meas_no_bound", 1);
            }
            self.cont = Some(Cont { check: ci, metric, text, had_bound: !bounds.is_empty() });
        }
        ops
    }
}

/// Parse a deck. The top-level file must be readable (Python wrote an
/// empty sidecar with only a warning; here it is an error).
pub fn parse_deck(path: &str, opts: &Options) -> Result<Deck, String> {
    if let Err(e) = std::fs::File::open(path) {
        return Err(format!("{}: {}", path, e));
    }
    if std::path::Path::new(path).is_dir() {
        return Err(format!("{}: is a directory", path));
    }
    let mut d = Deck { path: path.to_string(), ..Deck::default() };
    for (k, v) in &opts.defines {
        d.defines.insert(k, v.clone());
    }
    let mut p = Parser {
        d,
        w: Words::new(),
        scan_all: opts.scan_all,
        cond: Vec::new(),
        cur: None,
        depth: 0,
        macro_depth: 0,
        macro_pending: false,
        verbatim_depth: 0,
        in_comment: false,
        icont: false,
        follow_verbatim: opts.follow_verbatim,
        env_switches: opts.env_switches,
        cont: None,
        acont: None,
        acont_open: false,
        sub_names: Vec::new(),
        sub_map: HashMap::new(),
    };
    if p.d.defines.len() > 0 {
        p.rebuild_sub();
    }
    p.feed_file(path, &opts.include_dirs, &[]);
    if let Some(ci) = p.cur {
        let name = p.d.arena[ci].name.clone();
        p.d.warnings.push(format!("unterminated check block {}", name));
    }
    if !p.d.verbatim_includes.is_empty() && !opts.scan_all && !opts.follow_verbatim {
        let n = p.d.verbatim_includes.len();
        p.d.warnings.push(format!(
            "{} INCLUDE(s) inside VERBATIM/Tcl blocks were NOT followed (Tcl-conditional; --scan or --follow-verbatim follows them)",
            n
        ));
    }
    let mut d = p.d;
    d.resolve();
    Ok(d)
}

/// --scan: the inventory that decides the parser scope BEFORE trusting a
/// converted deck (macros? switches? statement kinds?).
pub fn format_scan(d: &Deck) -> String {
    let mut l: Vec<String> = vec![
        format!("deck scan: {}", d.path),
        format!("  files {} ({} includes), {} lines", d.stats.n("files"), d.includes.len(), d.stats.n("lines")),
    ];
    for inc in d.includes.iter().take(20) {
        l.push(format!("    include {}", inc));
    }
    if d.includes.len() > 20 {
        l.push(format!("    ... {} more", d.includes.len() - 20));
    }
    let sw: Vec<String> = d
        .switches
        .iter()
        .map(|name| match d.switch_values.get(name) {
            Some(vals) if !vals.is_empty() => format!("{}({})", name, vals.join("|")),
            _ => name.clone(),
        })
        .collect();
    l.push(format!("  switches (#IFDEF): {}", if sw.is_empty() { "-".to_string() } else { sw.join(", ") }));
    if d.env_used.len() > 0 {
        let used: Vec<String> =
            d.env_used.iter().map(|(n, v)| if v.is_empty() { n.clone() } else { format!("{}={}", n, v) }).collect();
        l.push(format!("  switches satisfied from the environment: {}", used.join(", ")));
    }
    let mut defs: Vec<&String> = d.defines.iter().map(|(k, _)| k).collect();
    defs.sort();
    l.push(format!(
        "  defines in effect: {}",
        if defs.is_empty() { "-".to_string() } else { defs.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ") }
    ));
    l.push(format!(
        "  layers {}, layer maps {}, variables {}",
        d.stats.n("layer"),
        d.stats.n("layer_map"),
        d.variables.len()
    ));
    l.push(format!("  derivations {}, checks {}", d.stats.n("assign"), d.checks.len()));
    let meas: Vec<String> = d.meas_hist.most_common(None).iter().map(|(k, n)| format!("{} {}", k, n)).collect();
    l.push(format!("  measurements: {}", if meas.is_empty() { "-".to_string() } else { meas.join(", ") }));
    let (dm, cm) = (d.stats.n("dmacro"), d.stats.n("cmacro"));
    l.push(format!(
        "  DMACRO {} / CMACRO {}{}",
        dm,
        cm,
        if cm != 0 { "  << macros in use: expansion is NOT implemented, metadata will be incomplete" } else { "" }
    ));
    if d.stats.n("verbatim") != 0 || !d.verbatim_includes.is_empty() {
        l.push(format!(
            "  VERBATIM/Tcl blocks {}; INCLUDEs inside {} (--scan follows them, the normal parse skips)",
            d.stats.n("verbatim"),
            d.verbatim_includes.len()
        ));
        for t in d.verbatim_includes.iter().take(10) {
            l.push(format!("    verbatim include {}", t));
        }
        if d.verbatim_includes.len() > 10 {
            l.push(format!("    ... {} more", d.verbatim_includes.len() - 10));
        }
    }
    if d.stats.n("prop_expr") != 0 {
        l.push(format!("  property-expression continuations skipped {}", d.stats.n("prop_expr")));
    }
    let unk = d.unknown.most_common(Some(20));
    l.push(format!(
        "  skipped statements {}{}",
        d.stats.n("unknown") + d.stats.n("unknown_in_block"),
        if unk.is_empty() { "" } else { ":" }
    ));
    for (name, n) in &unk {
        l.push(format!("    {:<24} {}", name, n));
    }
    for w in d.warnings.iter().take(20) {
        l.push(format!("  warn: {}", w));
    }
    l.join("\n")
}

fn write_atomic(out: &str, text: &str) -> Result<(), String> {
    let tmp = format!("{}.tmp", out);
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {}", tmp, e))?;
    std::fs::rename(&tmp, out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{} -> {}: {}", tmp, out, e)
    })
}

const USAGE: &str = "usage: floe-index svrf <deck> [-o OUT] [--scan] [-D NAME[=VAL]]... [-I DIR]... [--follow-verbatim] [--no-env-switches]";

/// `floe-index svrf <deck> ...` (the options `floe svrf` had).
pub fn svrf_cmd(args: &[String]) {
    let mut deck: Option<String> = None;
    let mut out: Option<String> = None;
    let mut scan = false;
    let mut defines: Vec<(String, Option<String>)> = Vec::new();
    let mut include_dirs: Vec<String> = Vec::new();
    let mut follow_verbatim = false;
    let mut env_switches = true;
    // gate-only: the whole parse state as JSON (tools/validate_svrf.py)
    let mut dump_state: Option<String> = None;
    let mut i = 0;
    let value = |i: &mut usize, flag: &str| -> String {
        *i += 1;
        match args.get(*i) {
            Some(v) => v.clone(),
            None => {
                eprintln!("floe-index svrf: {} needs a value\n{}", flag, USAGE);
                std::process::exit(2);
            }
        }
    };
    while i < args.len() {
        let a = args[i].as_str();
        let (flag, attached): (&str, Option<String>) = if let Some((f, v)) = a.split_once('=').filter(|_| a.starts_with("--")) {
            (f, Some(v.to_string()))
        } else if (a.starts_with("-o") || a.starts_with("-D") || a.starts_with("-I")) && a.len() > 2 && !a.starts_with("--") {
            // argparse: `-DNAME` and `-D=NAME` are `-D NAME`
            let v = &a[2..];
            (&a[..2], Some(v.strip_prefix('=').unwrap_or(v).to_string()))
        } else {
            (a, None)
        };
        match flag {
            "-o" | "--out" => out = Some(attached.unwrap_or_else(|| value(&mut i, flag))),
            "-D" | "--define" => {
                let d = attached.unwrap_or_else(|| value(&mut i, flag));
                let (name, val) = match d.split_once('=') {
                    Some((n, v)) => (n.to_string(), v.to_string()),
                    None => (d.clone(), String::new()),
                };
                if !name.is_empty() {
                    let val = if val.is_empty() { None } else { Some(val) };
                    match defines.iter_mut().find(|(n, _)| *n == name) {
                        Some(slot) => slot.1 = val,
                        None => defines.push((name, val)),
                    }
                }
            }
            "-I" | "--include-dir" => include_dirs.push(attached.unwrap_or_else(|| value(&mut i, flag))),
            "--scan" if attached.is_none() => scan = true,
            "--follow-verbatim" if attached.is_none() => follow_verbatim = true,
            "--no-env-switches" if attached.is_none() => env_switches = false,
            "--dump-state" => dump_state = Some(attached.unwrap_or_else(|| value(&mut i, flag))),
            f if f.starts_with('-') && f.len() > 1 => crate::unknown_option("svrf", a),
            _ => {
                if deck.is_some() {
                    eprintln!("floe-index svrf: one deck only\n{}", USAGE);
                    std::process::exit(2);
                }
                deck = Some(a.to_string());
            }
        }
        i += 1;
    }
    let Some(deck) = deck else {
        eprintln!("{}", USAGE);
        std::process::exit(2);
    };
    let opts = Options { defines, include_dirs, scan_all: scan, follow_verbatim, env_switches };
    let d = match parse_deck(&deck, &opts) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("svrf: {}", e);
            std::process::exit(1);
        }
    };
    if let Some(path) = &dump_state {
        if let Err(e) = write_atomic(path, &d.state_json().dump()) {
            eprintln!("svrf: {}", e);
            std::process::exit(1);
        }
    }
    if scan {
        println!("{}", format_scan(&d));
        return;
    }
    let out = out.unwrap_or_else(|| format!("{}.rules.json", deck));
    if let Err(e) = write_atomic(&out, &d.to_json().dump()) {
        eprintln!("svrf: {}", e);
        std::process::exit(1);
    }
    println!(
        "{}: {} checks, {} derivations, {} layers -> {}",
        deck,
        d.checks.len(),
        d.derived.len(),
        d.layers.len(),
        out
    );
    let cmacro = d.stats.n("cmacro");
    if cmacro != 0 {
        eprintln!("[svrf][warn] {} CMACRO calls NOT expanded - metadata is incomplete for macro-generated rules", cmacro);
    }
    let skipped = d.stats.n("unknown");
    if skipped != 0 {
        eprintln!("[svrf] {} unrecognized statements skipped (--scan lists them)", skipped);
    }
    for w in d.warnings.iter().take(10) {
        eprintln!("[svrf][warn] {}", w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_as_python_repr() {
        for (x, want) in [
            (0.05, "0.05"),
            (0.1, "0.1"),
            (1.0, "1.0"),
            (100.0, "100.0"),
            (0.031, "0.031"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (1e16, "1e+16"),
            (1.5e16, "1.5e+16"),
            (1234567890123456.0, "1234567890123456.0"),
            (-2.5, "-2.5"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1e100, "1e+100"),
            (123.456e-10, "1.23456e-08"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (0.1 + 0.2, "0.30000000000000004"),
        ] {
            assert_eq!(py_float_repr(x), want, "{x}");
        }
    }

    #[test]
    fn json_is_python_indent_1_sorted_ascii() {
        let j = J::Obj(vec![
            ("b".into(), J::Arr(vec![J::Int(1), J::Null, J::Float(0.5)])),
            ("a".into(), J::Str("é\"\\\n\u{1}\u{7f}😀".into())),
            ("c".into(), J::Obj(vec![])),
            ("d".into(), J::Arr(vec![])),
        ]);
        assert_eq!(
            j.dump(),
            "{\n \"a\": \"\\u00e9\\\"\\\\\\n\\u0001\\u007f\\ud83d\\ude00\",\n \"b\": [\n  1,\n  null,\n  0.5\n ],\n \"c\": {},\n \"d\": []\n}\n"
        );
    }

    #[test]
    fn the_regexes_behave_like_pythons() {
        assert_eq!(id_findall("(a AND L3) SIZE BY 0.01 x.5 1b"), vec!["a", "AND", "L3", "SIZE", "BY", "x.5", "b"]);
        let cs: Vec<char> = "M1 <0.03 ABUT<90".chars().collect();
        assert_eq!(op_search(&cs), Some(3));
        assert_eq!(chain(&cs, 3), vec![("<".to_string(), "0.03".to_string())]);
        let cs: Vec<char> = "> 0 < 0.1 ABUT>0<90".chars().collect();
        assert_eq!(chain(&cs, 0).len(), 2);
        assert!(!op_at(&"ABUT<90".chars().collect::<Vec<_>>(), 4));
        assert!(is_num("0.05") && is_num(".05") && is_num("1.") && is_num("-1e-5") && is_num("+2E+03"));
        assert!(!is_num("1e") && !is_num(".") && !is_num("a1") && !is_num("1.2.3") && !is_num(""));
        assert_eq!(check_match("\"Q.RULE.1\" { @ x"), Some(("Q.RULE.1".into(), " @ x".into())));
        assert_eq!(check_match("R.1{"), Some(("R.1".into(), "".into())));
        assert_eq!(check_match("\"Q.1 { x"), None);
        assert_eq!(assign_match("a = L1 NOT L2"), Some(("a".into(), "L1 NOT L2".into())));
        assert_eq!(assign_match("X == 5"), Some(("X".into(), "= 5".into())));
        assert_eq!(assign_match("a ="), None);
        assert_eq!(py_split1("  a  b c  "), (Some("a"), Some("b c  ")));
        assert_eq!(py_split1("a   "), (Some("a"), None));
    }

    #[test]
    fn paths_behave_like_posixpath() {
        assert_eq!(normpath("/a//b/./c/../d"), "/a/b/d");
        assert_eq!(normpath("//a/b"), "//a/b");
        assert_eq!(normpath("///a"), "/a");
        assert_eq!(normpath("../x/.."), "..");
        assert_eq!(normpath(""), ".");
        assert_eq!(dirname("deck.cal"), "");
        assert_eq!(dirname("/x/y"), "/x");
        assert_eq!(dirname("/y"), "/");
        assert_eq!(dirname("a//b"), "a");
        assert_eq!(pjoin("", "x"), "x");
        assert_eq!(pjoin("a/", "x"), "a/x");
        assert_eq!(pjoin("a", "/x"), "/x");
        std::env::set_var("FLOE_SVRF_UNIT_T", "/opt/t");
        assert_eq!(expandvars("$FLOE_SVRF_UNIT_T/a ${FLOE_SVRF_UNIT_T}/b $NOPE_FLOE_X/c ${} $"), "/opt/t/a /opt/t/b $NOPE_FLOE_X/c ${} $");
        std::env::remove_var("FLOE_SVRF_UNIT_T");
        assert_eq!(expanduser("x/~"), "x/~");
    }
}
