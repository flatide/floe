//! Rule-aware CD of one DRC marker (floe_oracle/drc_delta.py
//! `MeasurementSelector`, `measurement_candidates`, `ruler_segments`,
//! `area_label`; floe_oracle/drc.py `cd_segments`): which dimension of the
//! marker the SVRF condition flagged, its rulers and an AREA rule's label.
//!
//! The arithmetic is Python's, on the marker's micrometre floats: CPython's
//! `math.hypot` (lossless scaling and a differential correction) and
//! `math.fsum` (exact partials, half-even fix-up), so a value at a
//! comparison or five-decimal boundary falls on the side the reference put
//! it - the delta groups of a 100M-error rule and its detail line agree.
use super::ticks::{self, json_float, py_space};
use serde_json::Value;

pub type Point = [f64; 2];
/// A ruler, um: x0, y0, x1, y1.
pub type Segment = [f64; 4];

const CD_METRICS: [&str; 6] = ["width", "space", "notch", "enclosure", "overlap", "extension"];
const UPPER: [&str; 3] = ["<", "<=", "=="];

/// SVRF operator and option words (floe_oracle/svrf.py KEYWORDS: the MEAS
/// heads and the builder's KEYWORD_WORDS, rust/cli/src/svrf.rs).
const KEYWORDS: [&str; 81] = [
    "INTERNAL", "INT", "EXTERNAL", "EXT", "ENCLOSURE", "ENC", "AREA", "DENSITY", "LENGTH",
    "ANGLE", "PERIMETER", "VERTEX", "AND", "OR", "NOT", "XOR", "INTERACT", "INSIDE", "OUTSIDE",
    "TOUCH", "CUT", "ENCLOSE", "BY", "SIZE", "GROW", "SHRINK", "EXTENT", "EXTENTS", "HOLES",
    "WITH", "EDGE", "CONVEX", "OPPOSITE", "ABUT", "SINGULAR", "REGION", "PROJECTING", "PARALLEL",
    "PERPENDICULAR", "ONLY", "ALSO", "OVER", "UNDER", "UNDEROVER", "COPY", "NET", "RATIO",
    "WINDOW", "STEP", "TRUNCATE", "INNER", "OUTER", "MEASURE", "ALL", "PRINT", "RECTANGLE",
    "SQUARE", "COUNT", "COINCIDENT", "EXPAND", "TOP", "LEFT", "RIGHT", "BOTTOM", "GOOD", "BAD",
    "MAX", "MIN", "EVEN", "ODD", "MULTI", "ORTHOGONAL", "POLYGON", "CORNER", "CENTERLINE",
    "SPACE", "WIDTH", "NOTCH", "OVERLAP", "INTERSECTING", "EXTENDED",
];

fn keyword(word: &str) -> bool {
    KEYWORDS.contains(&word)
}

// ---- Python arithmetic ----------------------------------------------------

fn frexp_exponent(x: f64) -> i32 {
    // x > 0, finite: x = m * 2^e with m in [0.5, 1)
    let bits = x.to_bits();
    let raw = ((bits >> 52) & 0x7ff) as i32;
    if raw == 0 {
        frexp_exponent(x * f64::from_bits(((54 + 1023) as u64) << 52)) - 54
    } else {
        raw - 1022
    }
}

fn pow2(n: i32) -> f64 {
    if n >= -1022 {
        f64::from_bits(((n + 1023) as u64) << 52)
    } else {
        f64::from_bits(1u64 << (n + 1074))
    }
}

fn dl_mul(x: f64, y: f64) -> (f64, f64) {
    let z = x * y;
    (z, x.mul_add(y, -z))
}

fn dl_fast_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let z = x - a;
    (x, b - z)
}

fn vector_norm(vec: &mut [f64], max: f64) -> f64 {
    if max == 0.0 || vec.len() <= 1 {
        return max;
    }
    let max_e = frexp_exponent(max);
    if max_e < -1023 {
        for v in vec.iter_mut() {
            *v /= f64::MIN_POSITIVE;
        }
        return f64::MIN_POSITIVE * vector_norm(vec, max / f64::MIN_POSITIVE);
    }
    let scale = pow2(-max_e);
    let (mut csum, mut frac1, mut frac2) = (1.0f64, 0.0f64, 0.0f64);
    for &v in vec.iter() {
        let x = v * scale;
        let (hi, lo) = dl_mul(x, x);
        let (s, e) = dl_fast_sum(csum, hi);
        csum = s;
        frac1 += lo;
        frac2 += e;
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let (hi, lo) = dl_mul(-h, h);
    let (s, e) = dl_fast_sum(csum, hi);
    csum = s;
    frac1 += lo;
    frac2 += e;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h);
    h / scale
}

/// CPython's `math.hypot(x, y)` (Modules/mathmodule.c vector_norm).
pub fn hypot(x: f64, y: f64) -> f64 {
    let (x, y) = (x.abs(), y.abs());
    if x.is_infinite() || y.is_infinite() {
        return f64::INFINITY;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    vector_norm(&mut [x, y], x.max(y))
}

/// CPython's `math.fsum`: exact partials (Shewchuk), half-even fix-up of
/// the last bit; None where Python raises (an intermediate overflow) or
/// a summand is not finite.
pub fn fsum(values: impl IntoIterator<Item = f64>) -> Option<f64> {
    let mut partials: Vec<f64> = Vec::new();
    for v in values {
        if !v.is_finite() {
            return None;
        }
        let mut x = v;
        let mut i = 0;
        for j in 0..partials.len() {
            let mut y = partials[j];
            if x.abs() < y.abs() {
                std::mem::swap(&mut x, &mut y);
            }
            let hi = x + y;
            let yr = hi - x;
            let lo = y - yr;
            if lo != 0.0 {
                partials[i] = lo;
                i += 1;
            }
            x = hi;
        }
        partials.truncate(i);
        if x != 0.0 {
            if !x.is_finite() {
                return None;
            }
            partials.push(x);
        }
    }
    let mut n = partials.len();
    if n == 0 {
        return Some(0.0);
    }
    n -= 1;
    let mut hi = partials[n];
    let mut lo = 0.0;
    while n > 0 {
        let x = hi;
        n -= 1;
        let y = partials[n];
        hi = x + y;
        let yr = hi - x;
        lo = y - yr;
        if lo != 0.0 {
            break;
        }
    }
    if n > 0 && ((lo < 0.0 && partials[n - 1] < 0.0) || (lo > 0.0 && partials[n - 1] > 0.0)) {
        let y = lo * 2.0;
        let x = hi + y;
        let yr = x - hi;
        if y == yr {
            hi = x;
        }
    }
    Some(hi)
}

/// Python `math.ulp` of a finite value.
pub fn ulp(x: f64) -> f64 {
    let x = x.abs();
    if !x.is_finite() {
        return x;
    }
    if x == f64::MAX {
        return x - f64::from_bits(x.to_bits() - 1);
    }
    f64::from_bits(x.to_bits() + 1) - x
}

fn dist(p: Point, q: Point) -> f64 {
    hypot(p[0] - q[0], p[1] - q[1])
}

fn foot(p: Point, a: Point, b: Point) -> Point {
    let (ax, ay) = (a[0], a[1]);
    let (dx, dy) = (b[0] - ax, b[1] - ay);
    let l2 = dx * dx + dy * dy;
    if l2 <= 0.0 {
        return a;
    }
    let t = ((p[0] - ax) * dx + (p[1] - ay) * dy) / l2;
    let t = t.min(1.0).max(0.0);
    [ax + t * dx, ay + t * dy]
}

fn orient(a: Point, b: Point, c: Point) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// A shared point of two finite edges, without a near-zero tolerance. Keep
/// original endpoints for overlap/touch, and compute the proper intersection
/// only when both segments straddle each other's supporting line.
pub(crate) fn edge_pair_zero_point(a0: Point, a1: Point, b0: Point, b1: Point) -> Option<Point> {
    if [a0, a1, b0, b1].iter().flatten().any(|v| !v.is_finite()) {
        return None;
    }
    let on_segment = |p: Point, a: Point, b: Point| {
        orient(a, b, p) == 0.0
            && (0..2).all(|axis| a[axis].min(b[axis]) <= p[axis] && p[axis] <= a[axis].max(b[axis]))
    };
    for (p, a, b) in [(a0, b0, b1), (a1, b0, b1), (b0, a0, a1), (b1, a0, a1)] {
        if on_segment(p, a, b) {
            return Some(p);
        }
    }
    if !((orient(a0, a1, b0) > 0.0) != (orient(a0, a1, b1) > 0.0)
        && (orient(b0, b1, a0) > 0.0) != (orient(b0, b1, a1) > 0.0)) {
        return None;
    }
    let (dx, dy) = (a1[0] - a0[0], a1[1] - a0[1]);
    let (vx, vy) = (b1[0] - b0[0], b1[1] - b0[1]);
    let (wx, wy) = (b0[0] - a0[0], b0[1] - a0[1]);
    let denominator = dx * vy - dy * vx;
    if denominator == 0.0 || !denominator.is_finite() {
        return None;
    }
    let t = (wx * vy - wy * vx) / denominator;
    let p = [a0[0] + t * dx, a0[1] + t * dy];
    (t.is_finite() && p.iter().all(|v| v.is_finite())).then_some(p)
}

fn seg(p: Point, q: Point) -> Segment {
    [p[0], p[1], q[0], q[1]]
}

fn sorted_unique(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(f64::total_cmp);
    v.dedup_by(|a, b| a == b);
    v
}

/// The CD rulers of a simple marker, um (floe_oracle drc.py `cd_segments`):
/// a single edge's length; a facing edge pair's closest gap - at the
/// midpoint when the edges are parallel and that is the minimum itself,
/// with X/Y components after a diagonal minimum of parallel edges; an
/// axis-aligned rectangle's width then height through its middle. Complex
/// shapes get none.
pub fn cd_segments(kind: char, pts: &[Point]) -> Vec<Segment> {
    match (kind, pts.len()) {
        ('e', 2) => {
            if dist(pts[0], pts[1]) <= 0.0 {
                return vec![];
            }
            vec![seg(pts[0], pts[1])]
        }
        ('e', 4) => {
            let (a0, a1, b0, b1) = (pts[0], pts[1], pts[2], pts[3]);
            if let Some(p) = edge_pair_zero_point(a0, a1, b0, b1) {
                return vec![seg(p, p)];
            }
            if (orient(a0, a1, b0) > 0.0) != (orient(a0, a1, b1) > 0.0)
                && (orient(b0, b1, a0) > 0.0) != (orient(b0, b1, a1) > 0.0)
            {
                return vec![];
            }
            let cand = [
                (a0, foot(a0, b0, b1)),
                (a1, foot(a1, b0, b1)),
                (foot(b0, a0, a1), b0),
                (foot(b1, a0, a1), b1),
            ];
            // Python min(): the first of equal keys
            let mut best = 0;
            let mut dmin = dist(cand[0].0, cand[0].1);
            for (i, c) in cand.iter().enumerate().skip(1) {
                let d = dist(c.0, c.1);
                if d < dmin {
                    best = i;
                    dmin = d;
                }
            }
            let (p, q) = cand[best];
            let (adx, ady) = (a1[0] - a0[0], a1[1] - a0[1]);
            let (bdx, bdy) = (b1[0] - b0[0], b1[1] - b0[1]);
            let (al2, bl2) = (adx * adx + ady * ady, bdx * bdx + bdy * bdy);
            let cross = adx * bdy - ady * bdx;
            let parallel = al2 > 0.0 && bl2 > 0.0 && cross.abs() <= 1e-12 * (al2 * bl2).sqrt();
            let mid = [(a0[0] + a1[0]) / 2.0, (a0[1] + a1[1]) / 2.0];
            let fm = foot(mid, b0, b1);
            if parallel && dist(mid, fm) == dmin {
                return vec![seg(mid, fm)];
            }
            let mut out = vec![seg(p, q)];
            if parallel && p[0] != q[0] && p[1] != q[1] {
                // tuple order, then the elbow at (hi.x, lo.y)
                let (lo, hi) = if (p[0], p[1]) <= (q[0], q[1]) { (p, q) } else { (q, p) };
                let elbow = [hi[0], lo[1]];
                out.push(seg(lo, elbow));
                out.push(seg(elbow, hi));
            }
            out
        }
        ('p', 4) => {
            let xs = sorted_unique(pts.iter().map(|p| p[0]).collect());
            let ys = sorted_unique(pts.iter().map(|p| p[1]).collect());
            if xs.len() != 2 || ys.len() != 2 {
                return vec![];
            }
            let corners = [[xs[0], ys[0]], [xs[0], ys[1]], [xs[1], ys[0]], [xs[1], ys[1]]];
            let same = |a: &Point, b: &Point| a[0] == b[0] && a[1] == b[1];
            if !corners.iter().all(|c| pts.iter().any(|p| same(p, c)))
                || !pts.iter().all(|p| corners.iter().any(|c| same(p, c)))
            {
                return vec![];
            }
            let (w, h) = (xs[1] - xs[0], ys[1] - ys[0]);
            let mut out = Vec::new();
            if w > 0.0 {
                let my = (ys[0] + ys[1]) / 2.0;
                out.push(seg([xs[0], my], [xs[1], my]));
            }
            if h > 0.0 {
                let mx = (xs[0] + xs[1]) / 2.0;
                out.push(seg([mx, ys[0]], [mx, ys[1]]));
            }
            out
        }
        _ => vec![],
    }
}

/// The marker's distinct finite CDs for `metric`, ascending: a polygon's
/// area (`area`); a rectangle's two spans or an edge pair's closest gap
/// (the CD metrics - the gap's X/Y rulers are not candidates); a single
/// edge's length (`length`). Complex geometry has none.
pub fn candidates(kind: char, pts: &[Point], metric: &str) -> Vec<f64> {
    let values: Vec<f64> = if metric == "area" {
        if kind != 'p' || pts.len() < 3 {
            return vec![];
        }
        let (ox, oy) = (pts[0][0], pts[0][1]);
        let n = pts.len();
        let terms = (0..n).map(|i| {
            let (x0, y0) = (pts[i][0], pts[i][1]);
            let (x1, y1) = (pts[(i + 1) % n][0], pts[(i + 1) % n][1]);
            (x0 - ox) * (y1 - oy) - (x1 - ox) * (y0 - oy)
        });
        match fsum(terms) {
            Some(s) => vec![s.abs() / 2.0],
            None => return vec![],
        }
    } else if CD_METRICS.contains(&metric) {
        if kind == 'p' || (kind == 'e' && pts.len() == 4) {
            let mut segments = cd_segments(kind, pts);
            if segments.is_empty() {
                return vec![];
            }
            if kind == 'e' {
                segments.truncate(1);
            }
            segments
                .iter()
                .map(|s| hypot(s[2] - s[0], s[3] - s[1]))
                .collect()
        } else {
            return vec![];
        }
    } else if metric == "length" && kind == 'e' && pts.len() == 2 {
        vec![hypot(pts[1][0] - pts[0][0], pts[1][1] - pts[0][1])]
    } else {
        return vec![];
    };
    sorted_unique(values.into_iter().filter(|v| v.is_finite()).collect())
}

// ---- the rules.json statement scan ----------------------------------------

fn is_word_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}
fn is_bound_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-')
}

/// `_BOUND.match(text, i)`: an operator, spaces, a value token - its end.
pub(crate) fn bound_at(cs: &[char], i: usize) -> Option<usize> {
    // the alternation tries the two-character operators first, then backs
    // off to the one-character ones
    let mut lengths = Vec::new();
    if i > 0 && is_word_start(cs[i - 1]) {
        return None;
    }
    match (cs.get(i), cs.get(i + 1)) {
        (Some('<' | '>'), Some('=')) => lengths.extend([2, 1]),
        (Some('=' | '!'), Some('=')) => lengths.push(2),
        (Some('<' | '>'), _) => lengths.push(1),
        _ => return None,
    }
    for len in lengths {
        let mut j = i + len;
        while j < cs.len() && py_space(cs[j]) {
            j += 1;
        }
        let start = j;
        while j < cs.len() && is_bound_char(cs[j]) {
            j += 1;
        }
        if j > start {
            return Some(j);
        }
    }
    None
}

/// `_BOUND.search(text)`: the first bound's (start, end).
fn bound_search(cs: &[char]) -> Option<(usize, usize)> {
    (0..cs.len()).find_map(|i| bound_at(cs, i).map(|end| (i, end)))
}

/// `_WORD.findall(text)`.
fn words(cs: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        if is_word_start(cs[i]) {
            let s = i;
            i += 1;
            while i < cs.len() && (cs[i].is_ascii_alphanumeric() || matches!(cs[i], '_' | '.' | '-')) {
                i += 1;
            }
            out.push(cs[s..i].iter().collect());
        } else {
            i += 1;
        }
    }
    out
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(py_space)
}

/// floe_oracle drc_delta `_has_options`: options outside the comparator
/// chain (after it, or before it past the statement head), which the
/// selector does not evaluate.
fn has_options(text: &str, allowed_tail: &[&str]) -> bool {
    if text.is_empty() {
        return false;
    }
    let cs: Vec<char> = text.chars().collect();
    let Some((first_start, first_end)) = bound_search(&cs) else {
        return false;
    };
    let mut end = first_end;
    loop {
        let rest: String = cs[end..].iter().collect();
        let offset = rest.chars().take_while(|&c| py_space(c)).count();
        match bound_at(&cs, end + offset) {
            None => {
                let tail = py_strip(&rest);
                if !tail.is_empty() && !allowed_tail.contains(&tail.to_uppercase().as_str()) {
                    return true;
                }
                break;
            }
            Some(e) => end = e,
        }
    }
    words(&cs[..first_start])
        .iter()
        .skip(1)
        .any(|w| keyword(&w.to_uppercase()))
}

/// `^(?:<=|>=|==|!=|<|>)`.
fn leading_bound(s: &str) -> bool {
    let cs: Vec<char> = s.chars().collect();
    match (cs.first(), cs.get(1)) {
        (Some('<' | '>' | '=' | '!'), Some('=')) => true,
        (Some('<' | '>'), _) => true,
        _ => false,
    }
}

fn op_matches(op: &str, a: f64, b: f64) -> bool {
    match op {
        "<" => a < b,
        "<=" => a <= b,
        ">" => a > b,
        ">=" => a >= b,
        "==" => a == b,
        _ => a != b,
    }
}

fn known_op(op: &str) -> bool {
    matches!(op, "<" | "<=" | ">" | ">=" | "==" | "!=")
}

/// floe_oracle drc_delta `_coordinate_roundoff`: four ulps of the largest
/// coordinate (times the perimeter for an area).
fn coordinate_roundoff(pts: &[Point], area: bool) -> Option<f64> {
    let mut u = pts
        .iter()
        .flatten()
        .filter(|v| v.is_finite())
        .map(|&v| ulp(v))
        .fold(0.0f64, f64::max);
    if area {
        let n = pts.len();
        let perimeter = fsum((0..n).map(|i| {
            hypot(
                pts[(i + 1) % n][0] - pts[i][0],
                pts[(i + 1) % n][1] - pts[i][1],
            )
        }))?;
        u *= perimeter;
    }
    Some(4.0 * u)
}

/// (match, changed by the roundoff guard) of a raw CD against a bound.
fn compare(cd: f64, bound: f64, op: &str, roundoff: f64) -> (bool, bool) {
    let exact = op_matches(op, cd, bound);
    let tolerance = roundoff.max(4.0 * ulp(cd)).max(4.0 * ulp(bound));
    if cd != bound && (cd - bound).abs() <= tolerance {
        let guarded = op_matches(op, bound, bound);
        return (guarded, guarded != exact);
    }
    (exact, false)
}

#[derive(Clone, Debug)]
struct Record {
    ci: usize,
    op: Option<String>,
    value: Value,
    numeric: Option<f64>,
}

#[derive(Clone, Debug)]
struct Chain {
    metric: String,
    representative: Record,
    predicates: Vec<(String, f64)>,
    options: bool,
}

/// One marker's chosen CD: the constraint (index into the rule's list),
/// its bound as rules.json gives it, the CD and why it is an estimate.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub constraint: usize,
    pub bound: Value,
    pub cd: f64,
    pub estimated: bool,
    pub reason: String,
    pub candidates: Vec<f64>,
}

/// A rule's statement chains, compiled once (floe_oracle drc_delta
/// `MeasurementSelector`): bounds on one statement are a conjunction;
/// wrapped comparator lines join their statement; unrelated statements are
/// never combined into an invented range.
#[derive(Clone, Debug, Default)]
pub struct Selector {
    constraints: Vec<Value>,
    chains: Vec<Chain>,
    uncertain_alternatives: bool,
    uncertain_ruler_options: bool,
}

fn text_of(con: &Value) -> String {
    match con.get("text") {
        Some(Value::String(s)) => s.split(py_space).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

impl Selector {
    /// `constraints`: a rule's rules.json constraint objects.
    pub fn new(constraints: &[Value]) -> Self {
        struct Open {
            metric: Value,
            text: String,
            records: Vec<Record>,
            options: bool,
        }
        let mut chains: Vec<Open> = Vec::new();
        let mut uncertain_ruler_options = false;
        for (ci, con) in constraints.iter().enumerate() {
            let metric = con.get("metric").cloned().unwrap_or(Value::Null);
            let text = text_of(con);
            let same = chains.last().is_some_and(|prev| {
                !text.is_empty()
                    && metric == prev.metric
                    && (text == prev.text
                        || (text.starts_with(&format!("{} ", prev.text))
                            && leading_bound(text[prev.text.len()..].trim_start_matches(py_space))))
            });
            if !same {
                chains.push(Open {
                    metric: metric.clone(),
                    text: text.clone(),
                    records: Vec::new(),
                    options: false,
                });
            }
            let chain = chains.last_mut().expect("a chain");
            chain.text = text.clone();
            chain.options |= has_options(&text, &[]);
            let m = metric.as_str().unwrap_or("");
            let allowed: &[&str] = if matches!(m, "space" | "notch") {
                &["NOTCH", "SPACE"]
            } else {
                &[]
            };
            uncertain_ruler_options |= has_options(&text, allowed);
            let value = con.get("value").cloned().unwrap_or(Value::Null);
            let numeric = json_float(&value);
            chain.records.push(Record {
                ci,
                op: con.get("op").and_then(Value::as_str).map(str::to_string),
                value,
                numeric,
            });
        }
        let supported = |m: &str| CD_METRICS.contains(&m) || m == "length" || m == "area";
        let mut out = Self {
            constraints: constraints.to_vec(),
            chains: Vec::new(),
            uncertain_alternatives: false,
            uncertain_ruler_options,
        };
        for chain in chains {
            let metric = chain.metric.as_str().unwrap_or("");
            if chain.metric.as_str().is_none()
                || !supported(metric)
                || chain.records.iter().any(|r| {
                    r.op.as_deref().is_none_or(|op| !known_op(op)) || r.numeric.is_none()
                })
            {
                out.uncertain_alternatives = true;
                continue;
            }
            let representative = chain
                .records
                .iter()
                .find(|r| r.op.as_deref().is_some_and(|op| UPPER.contains(&op)))
                .unwrap_or(&chain.records[0])
                .clone();
            let predicates = chain
                .records
                .iter()
                .map(|r| (r.op.clone().unwrap_or_default(), r.numeric.unwrap_or(0.0)))
                .collect();
            out.chains.push(Chain {
                metric: metric.to_string(),
                representative,
                predicates,
                options: chain.options,
            });
        }
        out
    }

    pub fn can_measure(&self) -> bool {
        !self.chains.is_empty()
    }

    pub fn constraints(&self) -> &[Value] {
        &self.constraints
    }

    /// The rulers: a rectangle keeps the directions that meet a dimensional
    /// error condition (both when both do, or when the rule cannot tell);
    /// an AREA-only rule has none (its area is a label).
    pub fn ruler_segments(&self, kind: char, pts: &[Point]) -> Vec<Segment> {
        if !self.constraints.is_empty()
            && self
                .constraints
                .iter()
                .all(|c| c.get("metric").and_then(Value::as_str) == Some("area"))
        {
            return vec![];
        }
        let segments = cd_segments(kind, pts);
        if kind != 'p' || segments.len() != 2 {
            return segments;
        }
        if self.chains.is_empty()
            || self.uncertain_alternatives
            || self.uncertain_ruler_options
            || self.chains.iter().any(|c| !CD_METRICS.contains(&c.metric.as_str()))
        {
            return segments;
        }
        let Some(roundoff) = coordinate_roundoff(pts, false) else {
            return segments;
        };
        let matched: Vec<Segment> = segments
            .iter()
            .filter(|s| {
                let cd = hypot(s[2] - s[0], s[3] - s[1]);
                self.chains.iter().any(|chain| {
                    chain
                        .predicates
                        .iter()
                        .all(|(op, bound)| compare(cd, *bound, op, roundoff).0)
                })
            })
            .copied()
            .collect();
        if matched.is_empty() {
            segments
        } else {
            matched
        }
    }

    /// The CD the rule's condition flagged on this marker, or None
    /// (unmeasurable, or statements that disagree on the criterion).
    pub fn pick(&self, kind: char, pts: &[Point]) -> Option<Pick> {
        let mut cd_values: Option<Vec<f64>> = None;
        let mut length_values: Option<Vec<f64>> = None;
        let mut area_values: Option<Vec<f64>> = None;
        let mut roundoffs: [Option<Option<f64>>; 2] = [None, None];
        let mut matches: Vec<((String, String, f64, f64), Pick)> = Vec::new();
        let mut numerical_uncertainty = false;
        for chain in &self.chains {
            let metric = chain.metric.as_str();
            let slot = if CD_METRICS.contains(&metric) {
                &mut cd_values
            } else if metric == "area" {
                &mut area_values
            } else {
                &mut length_values
            };
            let cands = slot
                .get_or_insert_with(|| candidates(kind, pts, metric))
                .clone();
            if cands.is_empty() {
                continue;
            }
            let area = metric == "area";
            let Some(roundoff) =
                *roundoffs[usize::from(area)].get_or_insert_with(|| coordinate_roundoff(pts, area))
            else {
                continue;
            };
            let mut survivors = Vec::new();
            for &cd in &cands {
                let mut accepted = true;
                for (op, bound) in &chain.predicates {
                    let (m, uncertain) = compare(cd, *bound, op, roundoff);
                    numerical_uncertainty |= uncertain;
                    accepted &= m;
                }
                if accepted {
                    survivors.push(cd);
                }
            }
            if survivors.is_empty() {
                continue;
            }
            let rep = &chain.representative;
            let op = rep.op.clone().unwrap_or_default();
            let numeric = rep.numeric.unwrap_or(0.0);
            let actual = match op.as_str() {
                ">" | ">=" => *survivors.last().expect("survivors"),
                "!=" => survivors
                    .iter()
                    .copied()
                    .reduce(|best, cd| {
                        let (db, dc) = ((best - numeric).abs(), (cd - numeric).abs());
                        if dc > db || (dc == db && cd > best) {
                            cd
                        } else {
                            best
                        }
                    })
                    .expect("survivors"),
                _ => survivors[0],
            };
            let mut reasons = Vec::new();
            if cands.len() > 1 {
                reasons.push("rectangle CD inferred from the rule condition");
                if survivors.len() > 1 {
                    reasons.push(match op.as_str() {
                        ">" | ">=" => "multiple dimensions match; selected the maximum",
                        "!=" => "multiple dimensions match; selected the farthest from the bound",
                        _ => "multiple dimensions match; selected the minimum",
                    });
                }
            }
            if chain.options {
                reasons.push("SVRF options are not geometrically evaluated");
            }
            matches.push((
                (metric.to_string(), op, numeric, actual),
                Pick {
                    constraint: rep.ci,
                    bound: rep.value.clone(),
                    cd: actual,
                    estimated: !reasons.is_empty(),
                    reason: reasons.join("; "),
                    candidates: cands,
                },
            ));
        }
        let (criterion, first) = matches.first()?;
        if matches[1..].iter().any(|(other, _)| other != criterion) {
            return None;
        }
        let mut reasons: Vec<String> = Vec::new();
        for (_, pick) in &matches {
            for r in pick.reason.split("; ").filter(|r| !r.is_empty()) {
                if !reasons.iter().any(|x| x == r) {
                    reasons.push(r.to_string());
                }
            }
        }
        if numerical_uncertainty {
            reasons.push("comparison is within floating-point roundoff of the bound".into());
        }
        if self.uncertain_alternatives {
            reasons.push(
                "unresolved or unsupported alternative constraints leave the producing statement uncertain"
                    .into(),
            );
        }
        Some(Pick {
            estimated: !reasons.is_empty(),
            reason: reasons.join("; "),
            ..first.clone()
        })
    }
}

/// A pick's displayed CD, bound and absolute difference, in ticks
/// (floe_oracle drc_delta `measurement_ticks`).
pub fn measurement_ticks(pick: &Pick) -> Result<(i64, i64, i64), String> {
    let measured = ticks::value_ticks(pick.cd)?;
    let bound = ticks::json_decimal(&pick.bound)
        .ok_or("invalid measurement")
        .and_then(|d| ticks::decimal_ticks(&d).map_err(|_| "invalid measurement"))?;
    let delta = (i128::from(measured) - i128::from(bound)).abs();
    let delta = i64::try_from(delta)
        .map_err(|_| "measurement difference exceeds fixed-precision range".to_string())?;
    Ok((measured, bound, delta))
}

/// An AREA rule's label: (x, y, "area 0.12345 µm²") at the marker's
/// bbox centre, from its polygon area; None for other rules or markers.
pub fn area_label(kind: char, pts: &[Point], constraints: &[Value]) -> Option<(f64, f64, String)> {
    if !constraints
        .iter()
        .any(|c| c.get("metric").and_then(Value::as_str) == Some("area"))
    {
        return None;
    }
    let area = *candidates(kind, pts, "area").first()?;
    let text = format!("area {} µm²", ticks::format_ticks(ticks::value_ticks(area).ok()?, false));
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in pts {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn con(metric: &str, op: &str, value: f64, text: &str) -> Value {
        json!({"metric": metric, "op": op, "value": value, "text": text})
    }
    fn rect(w: f64, h: f64) -> Vec<Point> {
        vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
    }

    #[test]
    fn hypot_and_fsum_are_pythons() {
        assert_eq!(hypot(3.0, 4.0), 5.0);
        assert_eq!(hypot(0.0, 0.0), 0.0);
        assert_eq!(hypot(-5.0, 0.0), 5.0);
        assert_eq!(fsum([1e16, 1.0, -1e16]), Some(1.0));
        assert_eq!(fsum([1.0, 1e-16, 1e-16]), Some(1.0000000000000002));
        assert_eq!(fsum([0.1; 10]), Some(1.0));
        assert_eq!(ulp(0.0), 5e-324);
        assert_eq!(ulp(1.0), f64::EPSILON);
    }

    #[test]
    fn rectangle_picks_the_side_the_condition_flags() {
        let s = Selector::new(&[con("width", "<", 0.05, "INT a < 0.05")]);
        let p = s.pick('p', &rect(0.02, 0.03)).unwrap();
        assert_eq!((p.constraint, p.cd, p.estimated), (0, 0.02, true));
        assert!(p.reason.starts_with("rectangle CD inferred"));
        let s = Selector::new(&[con("width", ">", 0.08, "INT a > 0.08")]);
        assert_eq!(s.pick('p', &rect(0.10, 0.12)).unwrap().cd, 0.12);
        // one side fits a range: no "multiple" reason
        let s = Selector::new(&[
            con("width", ">", 0.0, "INT a > 0 < 0.025"),
            con("width", "<", 0.025, "INT a > 0 < 0.025"),
        ]);
        let p = s.pick('p', &rect(0.02, 0.03)).unwrap();
        assert_eq!((p.constraint, p.cd), (1, 0.02));
        assert_eq!(p.reason, "rectangle CD inferred from the rule condition");
        // rulers: only the flagged direction
        assert_eq!(s.ruler_segments('p', &rect(0.02, 0.03)).len(), 1);
    }

    #[test]
    fn unrelated_statements_and_area_rules() {
        let s = Selector::new(&[
            con("width", "<", 0.05, "INT a < 0.05"),
            con("space", "<", 0.05, "EXT a < 0.05"),
        ]);
        // both statements match with different criteria: undecidable
        assert_eq!(s.pick('p', &rect(0.02, 0.02)), None);
        let area = [con("area", "<", 30.0, "AREA a < 30")];
        let s = Selector::new(&area);
        assert!(s.ruler_segments('p', &rect(1.0, 2.0)).is_empty());
        let tri = [[0.0, 0.0], [4.0, 0.0], [0.0, 3.0]];
        assert_eq!(s.pick('p', &tri).unwrap().cd, 6.0);
        assert_eq!(
            area_label('p', &tri, &area),
            Some((2.0, 1.5, "area 6.00000 µm²".to_string()))
        );
    }

    #[test]
    fn options_make_an_estimate_and_keep_both_rulers() {
        let s = Selector::new(&[con("space", "<", 0.05, "EXT a < 0.05 OPPOSITE")]);
        let p = s.pick('p', &rect(0.02, 0.02)).unwrap();
        assert!(p.estimated);
        assert_eq!(s.ruler_segments('p', &rect(0.02, 0.03)).len(), 2);
        let s = Selector::new(&[con("space", "<", 0.05, "EXT a < 0.05 NOTCH")]);
        assert_eq!(s.ruler_segments('p', &rect(0.02, 0.06)).len(), 1);
    }

    #[test]
    fn edge_pairs_measure_their_true_gap() {
        let facing = [[0.0, 0.0], [0.1, 0.0], [0.0, 0.02345], [0.1, 0.02345]];
        assert_eq!(cd_segments('e', &facing).len(), 1);
        let near = [[0.0, 0.0], [2.0, 0.0], [0.0, 1.0], [2.0, 1.0001]];
        let s = cd_segments('e', &near);
        assert_eq!(hypot(s[0][2] - s[0][0], s[0][3] - s[0][1]), 1.0);
        let far = [[0.0, 0.0], [2.0, 0.0], [3.0, 1000.0], [4.0, 1000.0]];
        assert_eq!(cd_segments('e', &far).len(), 3);
        let crossing = [[0.0, 0.0], [1.0, 1.0], [0.0, 1.0], [1.0, 0.0]];
        assert_eq!(cd_segments('e', &crossing), vec![[0.5, 0.5, 0.5, 0.5]]);
    }

    #[test]
    fn coincident_pair_is_zero_cd_not_its_own_edge_length() {
        let pts = [[2626.7775, 1734.6100], [2626.7775, 1734.6060],
                   [2626.7775, 1734.6100], [2626.7775, 1734.6060]];
        let selector = Selector::new(&[con("space", "<", 0.0275, "EXT M1 < 0.0275")]);
        for order in [[0, 1, 2, 3], [1, 0, 2, 3], [0, 1, 3, 2], [3, 2, 1, 0]] {
            let points = order.map(|i| pts[i]);
            let pick = selector.pick('e', &points).unwrap();
            assert_eq!(pick.cd, 0.0);
            assert_eq!(pick.candidates, vec![0.0]);
            assert_eq!(measurement_ticks(&pick).unwrap(), (0, 2750, 2750));
            assert_eq!(ticks::percent_ticks(2750, 2750), Some(10_000_000));
            assert!(!pick.estimated);
            let rulers = selector.ruler_segments('e', &points);
            assert_eq!(rulers.len(), 1);
            assert_eq!(rulers[0][..2], rulers[0][2..]);
        }
        let length = Selector::new(&[con("length", "<", 0.0275, "LENGTH M1 < 0.0275")]);
        assert_eq!(measurement_ticks(&length.pick('e', &pts[..2]).unwrap()).unwrap(), (400, 2750, 2350));
        assert!(selector.pick('e', &pts[..2]).is_none());
    }

    #[test]
    fn overlapping_touching_and_crossing_pairs_obey_zero_comparisons() {
        for points in [
            [[0.0, 0.0], [3.0, 0.0], [1.0, 0.0], [4.0, 0.0]],
            [[0.0, 0.0], [3.0, 4.0], [1.5, 2.0], [4.5, 6.0]],
            [[0.0, 0.0], [3.0, 0.0], [3.0, 0.0], [4.0, 1.0]],
            [[0.0, 0.0], [2.0, 2.0], [0.0, 2.0], [2.0, 0.0]],
            [[0.0, 0.0]; 4],
        ] {
            for (op, matches) in [("<", false), ("<=", true), (">", false),
                                   (">=", true), ("==", true), ("!=", false)] {
                let selector = Selector::new(&[con("space", op, 0.0, &format!("EXT M1 {op} 0"))]);
                assert_eq!(selector.pick('e', &points).map(|p| p.cd), matches.then_some(0.0), "{op}: {points:?}");
            }
            let rulers = cd_segments('e', &points);
            assert_eq!(rulers.len(), 1);
            assert_eq!(rulers[0][..2], rulers[0][2..]);
            assert!(rulers[0].iter().all(|v| v.is_finite()));
        }
        let crossing = [[0.0, 0.0], [2.0, 2.0], [0.0, 2.0], [2.0, 0.0]];
        assert_eq!(cd_segments('e', &crossing), vec![[1.0, 1.0, 1.0, 1.0]]);
    }

    #[test]
    fn positive_edge_gaps_are_not_snapped_to_zero() {
        let selector = Selector::new(&[con("space", "<", 0.168, "EXT M1 < 0.168")]);
        let disjoint = [[25.4810, 188.9700], [25.5275, 188.9700],
                        [25.3355, 189.0460], [25.3820, 189.0460]];
        assert_eq!(measurement_ticks(&selector.pick('e', &disjoint).unwrap()).unwrap(), (12481, 16800, 4319));
        assert_eq!(selector.ruler_segments('e', &disjoint).len(), 3);
        let tiny = [[0.0, 0.0], [0.1, 0.0], [0.0, 1e-9], [0.1, 1e-9]];
        assert_eq!(selector.pick('e', &tiny).unwrap().cd, 1e-9);
        let greater = Selector::new(&[con("space", ">", 0.0, "EXT M1 > 0")]);
        assert_eq!(greater.pick('e', &tiny).unwrap().cd, 1e-9);
        assert!(cd_segments('e', &[[0.0, 0.0]; 2]).is_empty());
    }

    #[test]
    fn statement_scan_matches_python_regexes() {
        assert!(!has_options("INT a < 0.05", &[]));
        assert!(has_options("INT a < 0.05 OPPOSITE", &[]));
        assert!(!has_options("EXT a < 0.05 NOTCH", &["NOTCH", "SPACE"]));
        assert!(has_options("EXT a < 0.05 NOTCH", &[]));
        assert!(!has_options("INT a >0<0.05", &[]));
        assert!(has_options("INT a OPPOSITE < 0.05", &[]));
        // a comparison glued to a name is not a bound
        assert!(!has_options("INT a<0.05", &[]));
        assert!(leading_bound("<= 1"));
        assert!(!leading_bound("= 1"));
    }
}
