//! Conservative measurement conditions from DRC rule descriptions.
//!
//! A description is prose, not an executable rule. Accept one evidenced
//! dimension with explicit units; invert requirement wording to an error
//! predicate. Keep the original clause separate from normalized selector
//! text, and never let a description replace explicit SVRF constraints.
use serde_json::{json, Value};

const AMBIGUOUS: &[&str] = &[
    "if",
    "unless",
    "except",
    "when",
    "otherwise",
    "or",
    "and",
    "between",
    "range",
    "depending",
    "only",
    "inside",
    "outside",
    "within",
    "depends",
    "dependent",
    "condition",
    "conditional",
    "provided",
    "applies",
    "where",
];
const UNITS: &[&str] = &[
    "micrometers",
    "micrometres",
    "micrometer",
    "micrometre",
    "nanometers",
    "nanometres",
    "nanometer",
    "nanometre",
    "microns",
    "micron",
    "마이크로미터",
    "나노미터",
    "um",
    "nm",
];
const COMPARATORS: &[&str] = &["<=", ">=", "==", "!=", "<", ">", "="];

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c| !word(c)).filter(|s| !s.is_empty())
}
fn spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn normalized(s: &str) -> String {
    let s = s
        .replace(['µ', 'μ'], "u")
        .replace('²', "^2")
        .replace('≥', ">=")
        .replace('≤', "<=");
    s.split_whitespace()
        .map(|w| {
            if w.eq_ignore_ascii_case("min.") || w.eq_ignore_ascii_case("max.") {
                w[..w.len() - 1].to_lowercase()
            } else {
                w.to_lowercase()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn starts_word<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.strip_prefix(prefix)
        .filter(|rest| rest.chars().next().is_none_or(|c| !word(c)))
}
fn requirement(s: &str) -> bool {
    ["must be", "shall be", "should be"]
        .iter()
        .any(|p| starts_word(s, p).is_some())
}
fn note(s: &str) -> bool {
    let s = normalized(s);
    if ["rule file pathname", "rule file title"].iter().any(|p| {
        s.strip_prefix(p)
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
    }) || [
        "expected", "example", "examples", "waiver", "waived", "waivable",
    ]
    .iter()
    .any(|p| starts_word(&s, p).is_some())
        || s.starts_with("local #")
        || s.strip_prefix('#')
            .and_then(|s| s.chars().next())
            .is_some_and(|c| c.is_ascii_digit())
        || ["측정값", "예시", "예제", "예상 값", "예상 델타"]
            .iter()
            .any(|p| s.starts_with(p))
    {
        return true;
    }
    let s = ["selected ", "actual ", "measured "]
        .iter()
        .find_map(|p| s.strip_prefix(p))
        .unwrap_or(&s);
    starts_word(s, "cd").is_some_and(|rest| {
        let rest = rest.trim_start();
        !rest.starts_with(['<', '>', '=']) && !requirement(rest)
    })
}
fn ambiguous(s: &str) -> bool {
    words(s).any(|w| AMBIGUOUS.contains(&w))
        || [
            "경우", "조건", "영역", "또는", "혹은", "제외", "따라", "일 때", "일때",
        ]
        .iter()
        .any(|w| s.contains(w))
}
fn signal(s: &str) -> bool {
    s.contains(['<', '>', '='])
        || words(s).any(|w| ["minimum", "maximum", "min", "max", "limit"].contains(&w))
        || ["최소", "최대", "이상", "이하", "미만", "초과"]
            .iter()
            .any(|w| s.contains(w))
}
fn metric_word(s: &str) -> Option<&'static str> {
    Some(match s {
        "width" | "폭" | "선폭" => "width",
        "space" | "spacing" | "간격" | "이격" => "space",
        "notch" | "노치" => "notch",
        "enclosure" | "enc" | "인클로저" => "enclosure",
        "overlap" | "겹침" | "중첩" => "overlap",
        "extension" | "연장" | "확장" => "extension",
        "length" | "길이" => "length",
        "area" | "면적" => "area",
        _ => return None,
    })
}
fn name_metric(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    let mut found = None;
    for token in lower.split(|c: char| !c.is_ascii_alphabetic() && !('가'..='힣').contains(&c)) {
        if let Some(m) = metric_word(token) {
            if found.is_some_and(|old| old != m) {
                return None;
            }
            found = Some(m);
        }
    }
    found
}
fn metric(prefix: &str, name: &str) -> Option<&'static str> {
    match prefix {
        "two-layer internal overlap" | "two layer internal overlap" => return Some("overlap"),
        "enclosure extension" => return Some("extension"),
        _ => {}
    }
    let prefix = ["은", "는", "이", "가", "의"]
        .iter()
        .find_map(|p| prefix.strip_suffix(p))
        .unwrap_or(prefix)
        .trim();
    match prefix {
        "" | "dimension" | "cd" | "치수" => name_metric(name),
        _ => metric_word(prefix),
    }
}

struct Quantity<'a> {
    start: usize,
    end: usize,
    number: &'a str,
    nanometers: bool,
    squared: bool,
}
fn number_end(s: &str, start: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = start;
    if b.get(i).is_some_and(|c| matches!(c, b'+' | b'-')) {
        i += 1;
    }
    let mut digits = 0;
    while b.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
        digits += 1;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    if b.get(i).is_some_and(|c| matches!(c, b'e' | b'E')) {
        i += 1;
        if b.get(i).is_some_and(|c| matches!(c, b'+' | b'-')) {
            i += 1;
        }
        let first = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == first {
            return None;
        }
    }
    Some(i)
}
fn quantity_at(s: &str, start: usize) -> Option<Quantity<'_>> {
    if s[..start]
        .chars()
        .next_back()
        .is_some_and(|c| word(c) || c == '.')
    {
        return None;
    }
    let end_number = number_end(s, start)?;
    let rest = s[end_number..].trim_start();
    let unit = UNITS.iter().find(|u| rest.starts_with(**u))?;
    let mut end = s.len() - rest.len() + unit.len();
    let squared = if s[end..].starts_with("^2") {
        end += 2;
        true
    } else if s[end..].starts_with('2') {
        end += 1;
        true
    } else {
        false
    };
    if s[end..].chars().next().is_some_and(|c| word(c) || c == '^') {
        return None;
    }
    Some(Quantity {
        start,
        end,
        number: &s[start..end_number],
        nanometers: unit.starts_with('n') || *unit == "나노미터",
        squared,
    })
}
fn quantities(s: &str) -> Vec<Quantity<'_>> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if let Some(q) = quantity_at(s, i) {
            i = q.end;
            found.push(q);
        } else {
            i += s[i..].chars().next().expect("in bounds").len_utf8();
        }
    }
    found
}

/// Shift decimal exponent before parsing the float, so converting nm does
/// not introduce an extra binary division/rounding at a CD tick boundary.
fn value(q: &Quantity<'_>) -> Option<(f64, String)> {
    if q.number.len() > 64 {
        return None;
    }
    let (mantissa, exponent) = q.number.split_once(['e', 'E']).unwrap_or((q.number, "0"));
    let exponent: i64 = exponent.parse().ok()?;
    let shift = if q.nanometers {
        if q.squared {
            6
        } else {
            3
        }
    } else {
        0
    };
    let exponent = exponent.checked_sub(shift)?;
    let nonzero = mantissa.bytes().any(|c| (b'1'..=b'9').contains(&c));
    if mantissa.starts_with('-') && nonzero {
        return None;
    }
    let numeric: f64 = format!("{mantissa}e{exponent}").parse().ok()?;
    if !numeric.is_finite() || (nonzero && numeric == 0.0) {
        return None;
    }
    if !nonzero {
        return Some((numeric, "0".into()));
    }
    let mantissa = mantissa.trim_start_matches(['+', '-']);
    let fractional = mantissa.split_once('.').map_or(0, |(_, frac)| frac.len());
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_start_matches('0');
    let trailing = digits.len() - digits.trim_end_matches('0').len();
    let digits = digits.trim_end_matches('0');
    let exponent = exponent
        .checked_sub(fractional as i64)?
        .checked_add(trailing as i64)?;
    let adjusted = exponent.checked_add(digits.len() as i64 - 1)?;
    let text = if exponent > 0 || adjusted < -6 {
        let head = if digits.len() == 1 {
            digits.into()
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        format!("{head}E{adjusted:+}")
    } else if exponent == 0 {
        digits.into()
    } else {
        let point = digits.len() as i64 + exponent;
        if point > 0 {
            format!(
                "{}.{}",
                &digits[..point as usize],
                &digits[point as usize..]
            )
        } else {
            // Nonzero finite f64 above limits the padding to at most 324.
            format!("0.{}{digits}", "0".repeat((-point) as usize))
        }
    };
    Some((numeric, text))
}
fn remove_words(s: &str, targets: &[&str]) -> (String, bool) {
    let mut found = false;
    let mut out = String::new();
    let mut start = 0;
    let mut iter = s.char_indices().peekable();
    while let Some((at, c)) = iter.next() {
        if !word(c) {
            continue;
        }
        let mut end = at + c.len_utf8();
        while let Some(&(next, c)) = iter.peek() {
            if !word(c) {
                break;
            }
            iter.next();
            end = next + c.len_utf8();
        }
        if targets.contains(&&s[at..end]) {
            out.push_str(&s[start..at]);
            out.push(' ');
            start = end;
            found = true;
        }
    }
    out.push_str(&s[start..]);
    (spaces(&out), found)
}
fn inverse(op: &str) -> Option<&'static str> {
    Some(match op {
        "<" => ">=",
        "<=" => ">",
        ">" => "<=",
        ">=" => "<",
        _ => return None,
    })
}
fn korean(s: &str) -> Option<(&'static str, bool)> {
    for (word, op) in [("이상", ">="), ("이하", "<="), ("초과", ">"), ("미만", "<")] {
        let Some(rest) = s.strip_prefix(word) else {
            continue;
        };
        let rest = rest.trim();
        if rest.is_empty() {
            return Some((op, false));
        }
        for required in ["이어야", "어야", "여야"] {
            if let Some(end) = rest.strip_prefix(required) {
                if ["", "한다", "함", "합니다", "됨", "됩니다"].contains(&end.trim()) {
                    return Some((op, true));
                }
            }
        }
    }
    None
}
fn clause(name: &str, evidence: &str) -> Option<Value> {
    let text = normalized(evidence);
    if ambiguous(&text) {
        return None;
    }
    let qs = quantities(&text);
    if qs.len() != 1 {
        return None;
    }
    let q = &qs[0];
    let (numeric, bound) = value(q)?;
    let mut prefix = text[..q.start].trim().to_string();
    let mut suffix = text[q.end..].trim().trim_end_matches('.').trim();
    for label in [
        format!("{}:", name.to_lowercase()),
        format!("{} :", name.to_lowercase()),
    ] {
        if let Some(rest) = prefix.strip_prefix(&label) {
            prefix = rest.trim().into();
            break;
        }
    }
    let mut required = false;
    for phrase in ["must be", "shall be", "should be"] {
        if let Some(at) = prefix.find(phrase) {
            let rest = &prefix[at + phrase.len()..];
            if (at == 0 || prefix[..at].chars().next_back().is_none_or(|c| !word(c)))
                && rest.chars().next().is_none_or(|c| !word(c))
            {
                required = true;
                prefix.replace_range(at..at + phrase.len(), "");
            }
        }
    }
    prefix = spaces(&prefix);
    if required {
        for (phrase, op) in [
            ("at least", ">="),
            ("at most", "<="),
            ("greater than", ">"),
            ("less than", "<"),
        ] {
            if let Some(start) = prefix.strip_suffix(phrase) {
                prefix = format!("{} {op}", start.trim());
                break;
            }
        }
    }
    let mut op = None;
    for &candidate in COMPARATORS {
        if let Some(rest) = prefix.strip_suffix(candidate) {
            op = Some(candidate);
            prefix = rest.trim().into();
            break;
        }
    }
    let (p, mut minimum) = remove_words(&prefix, &["minimum", "min"]);
    let (p, mut maximum) = remove_words(&p, &["maximum", "max"]);
    minimum |= p.contains("최소") || ["minimum", "min", "최소"].contains(&suffix);
    maximum |= p.contains("최대") || ["maximum", "max", "최대"].contains(&suffix);
    if minimum && maximum {
        return None;
    }
    prefix = spaces(&p.replace("최소", "").replace("최대", ""));
    for tail in [" is", " of"] {
        if let Some(rest) = prefix.strip_suffix(tail) {
            prefix = rest.trim().into();
        }
    }
    if ["minimum", "min", "maximum", "max", "최소", "최대"].contains(&suffix) {
        suffix = "";
    }
    if !suffix.is_empty() {
        if op.is_some() {
            return None;
        }
        let (korean_op, korean_required) = korean(suffix)?;
        op = Some(korean_op);
        required |= korean_required;
    }
    let metric = metric(&prefix, name)?;
    if q.squared != (metric == "area") {
        return None;
    }
    if minimum || maximum {
        op = match op {
            None | Some("=") => Some(if minimum { "<" } else { ">" }),
            Some(op)
                if (minimum && [">", ">="].contains(&op))
                    || (maximum && ["<", "<="].contains(&op)) =>
            {
                inverse(op)
            }
            _ => return None,
        };
    } else if required {
        op = op.and_then(inverse);
    }
    let op = op.filter(|op| inverse(op).is_some())?;
    Some(json!({"metric": metric, "op": op, "value": numeric,
        "text": format!("{metric} {op} {bound}"), "source": "description", "description": evidence.trim()}))
}

fn clauses(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (at, c) in line.char_indices() {
        let sentence = if c == '.' {
            let before = &line.as_bytes()[..at];
            let after = &line[at + 1..];
            let whitespace = after.chars().next().is_some_and(char::is_whitespace);
            let digit = before.last().is_some_and(u8::is_ascii_digit);
            let next = after.trim_start().chars().next();
            whitespace
                && if digit {
                    next.is_some_and(|c| {
                        c.is_ascii_alphabetic() || c == '#' || ('가'..='힣').contains(&c)
                    })
                } else {
                    let tail = &before[before.len().saturating_sub(3)..];
                    !tail.eq_ignore_ascii_case(b"min") && !tail.eq_ignore_ascii_case(b"max")
                }
        } else {
            false
        };
        if c == ';' || sentence {
            out.push(&line[start..at]);
            start = at + c.len_utf8();
        }
    }
    out.push(&line[start..]);
    out
}

/// One evidenced constraint, or an empty vector for unresolved prose.
/// Units are explicit um/nm (or squared for area); all results are inferred.
pub fn description_constraints(name: &str, description: &str) -> Vec<Value> {
    if description.len() > 65536 {
        return vec![];
    }
    let mut first: Option<Value> = None;
    for line in description.lines() {
        let line = line.trim().trim_start_matches('@').trim();
        if line.is_empty() || note(line) {
            continue;
        }
        for evidence in clauses(line) {
            let evidence = evidence.trim();
            if evidence.is_empty() || note(evidence) {
                continue;
            }
            let text = normalized(evidence);
            if ambiguous(&text) {
                return vec![];
            }
            let Some(con) = clause(name, evidence) else {
                if signal(&text) || !quantities(&text).is_empty() {
                    return vec![];
                }
                continue;
            };
            if let Some(old) = &first {
                if ["metric", "op", "value"]
                    .iter()
                    .any(|key| old.get(key) != con.get(key))
                {
                    return vec![];
                }
            } else {
                first = Some(con);
            }
        }
    }
    first.into_iter().collect()
}

/// Existing nonempty SVRF constraints remain authoritative, even when their
/// metric or bound is unsupported. Description inference is only a fallback.
pub fn constraints_for(name: &str, description: &str, metadata: Option<&Value>) -> Vec<Value> {
    if let Some(cons) = metadata
        .and_then(|m| m.get("checks"))
        .and_then(|c| c.get(name))
        .and_then(|c| c.get("constraints"))
        .and_then(Value::as_array)
        .filter(|c| !c.is_empty())
    {
        return cons.clone();
    }
    description_constraints(name, description)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drc::analysis::measure::{measurement_ticks, Selector};

    fn check(name: &str, desc: &str, metric: &str, op: &str, value: f64) {
        let c = description_constraints(name, desc);
        assert_eq!(c.len(), 1, "{desc}");
        assert_eq!(c[0]["metric"], metric, "{desc}");
        assert_eq!(c[0]["op"], op, "{desc}");
        assert_eq!(c[0]["value"].as_f64(), Some(value), "{desc}");
        assert_eq!(c[0]["source"], "description");
        assert!(desc.contains(c[0]["description"].as_str().unwrap()));
    }
    #[test]
    fn minimum_maximum_and_requirement_boundaries() {
        for (text, op) in [
            ("Minimum width 0.05000 um", "<"),
            ("Width minimum = 0.05 um", "<"),
            ("Minimum width >= 0.05 um", "<"),
            ("Minimum width > 0.05 um", "<="),
            ("Maximum width 0.05 um", ">"),
            ("Width 0.05 um maximum", ">"),
            ("Maximum width <= 0.05 um", ">"),
            ("Maximum width < 0.05 um", ">="),
            ("Width must be >= 0.05 um", "<"),
            ("Width must be > 0.05 um", "<="),
            ("Width shall be <= 0.05 um", ">"),
            ("Width should be < 0.05 um", ">="),
            ("Width must be at least 0.05 um", "<"),
            ("Width must be less than 0.05 um", ">="),
            ("Width < 0.05 um", "<"),
            ("Width ≤ 0.05 µm", "<="),
            ("Width ≥ 0.05 μm", ">="),
        ] {
            check("M1.WIDTH", text, "width", op, 0.05);
        }
    }
    #[test]
    fn explicit_units_metrics_and_korean() {
        for (text, metric, op, value) in [
            ("Min. spacing 50 nm", "space", "<", 0.05),
            ("Notch < 5e1 nm", "notch", "<", 0.05),
            ("Enclosure < .05 um", "enclosure", "<", 0.05),
            ("Two-layer INTERNAL overlap < 0.05 um", "overlap", "<", 0.05),
            ("ENCLOSURE extension < 0.05 um", "extension", "<", 0.05),
            ("Minimum length 50 nanometers", "length", "<", 0.05),
            ("Area < 10000 nm²", "area", "<", 0.01),
            ("Maximum area 0.01 um^2", "area", ">", 0.01),
            ("Area < 0.01 μm2", "area", "<", 0.01),
            ("최소 폭 50 nm", "width", "<", 0.05),
            ("최대 선폭 0.05 um", "width", ">", 0.05),
            ("간격은 최소 0.05 um", "space", "<", 0.05),
            ("폭은 0.05 um 이상이어야 함", "width", "<", 0.05),
            ("폭이 0.05 um 초과여야 한다", "width", "<=", 0.05),
            ("길이는 0.05 um 이하이어야 한다", "length", ">", 0.05),
            ("폭 0.05 um 미만", "width", "<", 0.05),
            ("겹침 < 50 나노미터", "overlap", "<", 0.05),
        ] {
            check("R", text, metric, op, value);
        }
    }
    #[test]
    fn name_fallback_notes_and_metadata_precedence() {
        check("M1.WIDTH.MIN", "Minimum 0.05 um", "width", "<", 0.05);
        check(
            "M1.WIDTH",
            "M1.WIDTH: Minimum width 0.05 um",
            "width",
            "<",
            0.05,
        );
        check("M1.WIDTH", "CD must be >= 0.05 um", "width", "<", 0.05);
        assert!(description_constraints("R", "Minimum 0.05 um").is_empty());
        assert!(description_constraints("M1.WIDTH.SPACE", "Minimum 0.05 um").is_empty());
        let desc = "Rule File Pathname: /Minimum width 0.01 um/deck.cal\nRule File Title: Maximum width 0.02 um\nWaiver: Width < 0.03 um\nMinimum width 0.05 um; CD 0.02 and 0.03 um.\nSelected CD: 0.01 um.\nExpected deltas: 0.03 um.";
        check("M1.WIDTH", desc, "width", "<", 0.05);
        let cons = vec![json!({"metric":"density", "op":"<", "value":"UNRESOLVED"})];
        let metadata = json!({"checks":{"M1.WIDTH":{"constraints":cons}}});
        assert_eq!(constraints_for("M1.WIDTH", desc, Some(&metadata)), cons);
        assert_eq!(
            constraints_for("M1.WIDTH", desc, Some(&json!({}))),
            description_constraints("M1.WIDTH", desc)
        );
        check(
            "M1.WIDTH",
            "Minimum width 0.05 um\nWidth < 50 nm",
            "width",
            "<",
            0.05,
        );
    }
    #[test]
    fn ambiguous_incomplete_and_extreme_numbers_are_withheld() {
        for desc in [
            "Minimum width 0.05",
            "Width < MIN_WIDTH um",
            "Width < 0.05 * SCALE um",
            "Width < 0.05 um or Width > 0.08 um",
            "Width < 0.05 um and length > 0.1 um",
            "Minimum notch 0.05 um if wire width > 0.10 um",
            "Width < 0.05 um except corners",
            "Width between 0.05 um and 0.1 um",
            "Width < 0.05 um; if wire is wide",
            "Minimum width 0.05 um; Maximum width 0.08 um",
            "Minimum width 0.05 um\nMaximum length 0.1 um",
            "Minimum width < 0.05 um",
            "Maximum width > 0.05 um",
            "Minimum maximum width 0.05 um",
            "Width = 0.05 um",
            "Width != 0.05 um",
            "Width < 0.05 um²",
            "Area < 0.05 um",
            "Width < 0.05 mm",
            "Width < -0.05 um",
            "Width < NaN um",
            "Width < 1e999 um",
            "Width < 1e-999 um",
            "Minimum width 1e999999999 nm",
            "Minimum width 1e-999999999 nm",
            "Width < 0.05 um3",
            "Width < 0.05 um^3",
            "Width < 0.05 um for wide wires",
            "Width < 0.05 um\nEnclosure > UNKNOWN",
            "Width < 0.05 um; limit 0.08 um",
            "최소 폭 0.05 um 또는 최대 길이 0.1 um",
            "Minimum width 0.05 um (see exceptions)",
            "Notch < 0.05 um; applies only inside macro",
            "Notch < 0.05 um\nInside the macro",
            "Notch < 0.05 um; 배선 영역",
            "CD sequence repeats 0.01, 0.02, 0.03 um.",
            "Selected CD is 0.02 um",
            "Expected area deltas: 0.006 um^2",
            "Rule File Title: Minimum width 0.05 um",
        ] {
            assert!(
                description_constraints("M1.WIDTH", desc).is_empty(),
                "{desc}"
            );
        }
    }
    #[test]
    fn sample_rule_types_measure_without_a_sidecar() {
        let rect = |w, h| vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
        for (name, desc, pts, want) in [
            ("M1.WIDTH.MIN", "Minimum 0.05000 um; #1 horizontal, #2 vertical, #3 both. Selected CD is 0.02000 um.", rect(0.02,0.03), 2000),
            ("M1.WIDTH.MAX", "Maximum 0.08000 um; #1 horizontal, #2 vertical, #3 both. Selected CD: 0.10000, 0.10000, 0.12000 um.", rect(0.1,0.12), 12000),
            ("M1.NOTCH", "Notch < 0.05000 um; #1 horizontal, #2 vertical, #3 single 0.02345 um gap.", rect(0.02,0.1), 2000),
            ("VIA1.ENCLOSURE", "Enclosure < 0.05000 um; CD 0.02000 and 0.02500 um.", rect(0.02,0.1), 2000),
            ("M1.M2.OVERLAP", "Two-layer INTERNAL overlap < 0.05000 um; CD 0.02000 and 0.01500 um.", rect(0.1,0.02), 2000),
            ("M1.EXTENSION", "ENCLOSURE extension < 0.05000 um; CD 0.02000 and 0.03000 um.", rect(0.02,0.1), 2000),
            ("M1.AREA", "Area < 0.01000 um^2. #1 rectangle: 0.00400; #2 triangle: 0.00600; #3 concave L: 0.00360 um^2.\nExpected area deltas: 0.00600, 0.00400, 0.00640 um^2; ratios: 60.00000%, 40.00000%, 64.00000%.", rect(0.1,0.04), 400),
        ] {
            let cons = description_constraints(name, desc);
            let selector = Selector::new(&cons);
            let pick = selector.pick('p', &pts).expect(name);
            assert_eq!(measurement_ticks(&pick).unwrap().0, want, "{name}");
            assert!(pick.estimated, "{name}");
            assert!(pick.reason.contains("description"), "{name}");
        }
        assert!(description_constraints("M1.SPACE.1", "Synthetic spacing markers; CD sequence repeats 0.01001, 0.01500, 0.02000, 0.02345, 0.03000 um.").is_empty());
    }
}
