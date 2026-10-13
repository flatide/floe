//! Fixed-precision measurement values: five decimals, ties away from zero
//! (floe_oracle/drc_delta.py `value_ticks`, `parse_step`, `format_ticks`,
//! `percent_ticks`). A value is read as the exact decimal Python's
//! `Decimal(str(value))` reads - a float by its shortest round-trip digits,
//! a text as written - so a CD and its bound round the same everywhere.
use serde_json::Value;

/// Ticks per unit: 0.00001.
pub const SCALE: i64 = 100_000;

/// An exact finite decimal: `digits` (ASCII, no leading zeros; empty for
/// zero) times ten to `exponent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decimal {
    negative: bool,
    digits: Vec<u8>,
    exponent: i64,
}

/// Python `str.isspace` (what `Decimal()`, `float()` and `str.strip` strip).
pub(crate) fn py_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0b}'
            | '\u{0c}'
            | '\r'
            | '\u{1c}'..='\u{1f}'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// ASCII digits with single underscores between them (PEP 515), cleaned.
fn grouped_digits(s: &str) -> Option<String> {
    if s.is_empty() || s.starts_with('_') || s.ends_with('_') || s.contains("__") {
        return None;
    }
    s.bytes()
        .all(|b| b.is_ascii_digit() || b == b'_')
        .then(|| s.replace('_', ""))
}

impl Decimal {
    /// Python `Decimal(text)` for a finite numeral; None for anything else
    /// (infinities and NaNs included - every caller refuses those).
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let t = text.trim_matches(py_space);
        let (negative, t) = match t.as_bytes().first() {
            Some(b'-') => (true, &t[1..]),
            Some(b'+') => (false, &t[1..]),
            _ => (false, t),
        };
        let (mantissa, exp) = match t.find(['e', 'E']) {
            Some(i) => (&t[..i], Some(&t[i + 1..])),
            None => (t, None),
        };
        let exponent: i64 = match exp {
            None => 0,
            Some(e) => {
                let (sign, body) = match e.as_bytes().first() {
                    Some(b'-') => (-1i64, &e[1..]),
                    Some(b'+') => (1, &e[1..]),
                    _ => (1, e),
                };
                let body = grouped_digits(body)?;
                // beyond any range a caller accepts; keeps the arithmetic small
                let body = body.trim_start_matches('0');
                if body.len() > 12 {
                    return None;
                }
                sign * body.parse::<i64>().unwrap_or(0)
            }
        };
        let (int, frac) = match mantissa.find('.') {
            Some(i) => (&mantissa[..i], &mantissa[i + 1..]),
            None => (mantissa, ""),
        };
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        let int = if int.is_empty() {
            String::new()
        } else {
            grouped_digits(int)?
        };
        let frac = if frac.is_empty() {
            String::new()
        } else {
            grouped_digits(frac)?
        };
        let all = format!("{int}{frac}");
        let digits = all.trim_start_matches('0').as_bytes().to_vec();
        Some(Self {
            negative,
            digits,
            exponent: exponent - frac.len() as i64,
        })
    }

    /// The decimal Python's `str(float)` writes: the shortest digits that
    /// read back as `v` (None when `v` is not finite).
    pub(crate) fn of_f64(v: f64) -> Option<Self> {
        if !v.is_finite() {
            return None;
        }
        // `{:e}` is the shortest round-trip form, e.g. "-1.2345e-5"
        Self::parse(&format!("{v:e}"))
    }

    fn of_integer(v: i128) -> Self {
        Self::parse(&v.to_string()).expect("an integer is a decimal")
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.digits.is_empty()
    }

    /// The power of ten of the leading digit (Python `Decimal.adjusted`).
    pub(crate) fn adjusted(&self) -> i64 {
        self.exponent + self.digits.len() as i64 - 1
    }

    /// The exponent once trailing zeros are dropped.
    fn normalized_exponent(&self) -> i64 {
        let zeros = self.digits.iter().rev().take_while(|&&d| d == b'0').count();
        self.exponent + zeros as i64
    }

    /// `round(self * 10^places)`, ties away from zero; None past i128.
    fn scaled_half_up(&self, places: i64) -> Option<i128> {
        if self.is_zero() {
            return Some(0);
        }
        let shift = self.exponent + places; // power of ten of the last digit
        // digits kept left of the point after scaling (may be <= 0)
        let kept = self.digits.len() as i64 + shift.min(0);
        let keep = kept.clamp(0, self.digits.len() as i64) as usize;
        let mut out: i128 = 0;
        for &d in &self.digits[..keep] {
            out = out.checked_mul(10)?.checked_add(i128::from(d - b'0'))?;
        }
        if shift > 0 {
            for _ in 0..shift {
                out = out.checked_mul(10)?;
            }
        } else if kept >= 0 && keep < self.digits.len() && self.digits[keep] >= b'5' {
            // the first dropped digit decides a half-up rounding (a
            // dropped part below the first digit, kept < 0, is < 0.1)
            out = out.checked_add(1)?;
        }
        Some(if self.negative { -out } else { out })
    }
}

/// A measurement in ticks (floe_oracle drc_delta `value_ticks`).
pub fn value_ticks(value: f64) -> Result<i64, String> {
    if !value.is_finite() {
        return Err("measurement must be finite".into());
    }
    decimal_ticks(&Decimal::of_f64(value).expect("finite"))
}

pub(crate) fn decimal_ticks(value: &Decimal) -> Result<i64, String> {
    if value.is_zero() {
        return Ok(0);
    }
    if value.adjusted() > 13 {
        return Err("measurement exceeds fixed-precision range".into());
    }
    if value.adjusted() < -6 {
        return Ok(0);
    }
    let ticks = value
        .scaled_half_up(5)
        .ok_or("measurement exceeds fixed-precision range")?;
    i64::try_from(ticks)
        .ok()
        .filter(|t| *t != i64::MIN)
        .ok_or_else(|| "measurement exceeds fixed-precision range".into())
}

/// The decimal `Decimal(str(value))` reads for a rules.json value: a float
/// by its repr, an integer as written, a string as written; None for
/// anything Decimal refuses (null, booleans, lists).
pub(crate) fn json_decimal(value: &Value) -> Option<Decimal> {
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Decimal::of_integer(i128::from(i)))
            } else if let Some(u) = n.as_u64() {
                Some(Decimal::of_integer(i128::from(u)))
            } else {
                Decimal::of_f64(n.as_f64()?)
            }
        }
        Value::String(s) => Decimal::parse(s),
        _ => None,
    }
}

/// A rules.json value's ticks (the bound of a delta), or None.
pub fn json_ticks(value: &Value) -> Option<i64> {
    json_decimal(value).and_then(|d| decimal_ticks(&d).ok())
}

/// Python `float(value)` of a rules.json value; None where it raises or
/// gives a non-finite number.
pub fn json_float(value: &Value) -> Option<f64> {
    let v = match value {
        Value::Number(n) => n.as_f64()?,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::String(s) => {
            let t = s.trim_matches(py_space);
            let body = t.strip_prefix(['+', '-']).unwrap_or(t);
            let lower = body.to_ascii_lowercase();
            if matches!(lower.as_str(), "inf" | "infinity" | "nan") {
                return None;
            }
            // underscores only between digits, as Python's float() takes them
            if t.contains('_') {
                let ok = t.as_bytes().windows(3).all(|w| {
                    w[1] != b'_' || (w[0].is_ascii_digit() && w[2].is_ascii_digit())
                }) && !t.starts_with('_')
                    && !t.ends_with('_');
                if !ok {
                    return None;
                }
            }
            let clean = t.replace('_', "");
            if clean.is_empty()
                || !clean
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
            {
                return None;
            }
            clean.parse::<f64>().ok()?
        }
        _ => return None,
    };
    v.is_finite().then_some(v)
}

/// A grouping step typed by the user: positive, at most five meaningful
/// decimals (trailing zeros allowed), in ticks.
pub fn parse_step(text: &str) -> Result<i64, String> {
    let invalid = || "invalid grouping step".to_string();
    let t = text.trim_matches(py_space);
    let body = t.strip_prefix(['+', '-']).unwrap_or(t).to_ascii_lowercase();
    if matches!(
        body.as_str(),
        "inf" | "infinity" | "nan" | "snan"
    ) || body.starts_with("nan")
        || body.starts_with("snan")
    {
        return Err("step must be a positive finite number".into());
    }
    let value = Decimal::parse(t).ok_or_else(invalid)?;
    if value.is_zero() || value.negative {
        return Err("step must be a positive finite number".into());
    }
    if value.adjusted() > 13 {
        return Err("step exceeds fixed-precision range".into());
    }
    if value.normalized_exponent() < -5 {
        return Err("step supports at most five decimal places".into());
    }
    let ticks = value.scaled_half_up(5).ok_or_else(invalid)?;
    i64::try_from(ticks).map_err(|_| "step exceeds fixed-precision range".into())
}

/// `[-]whole.fffff` (`+` too when `signed`).
pub fn format_ticks(ticks: i64, signed: bool) -> String {
    let sign = if ticks < 0 {
        "-"
    } else if signed {
        "+"
    } else {
        ""
    };
    let a = ticks.unsigned_abs();
    format!(
        "{sign}{}.{:05}",
        a / SCALE as u64,
        a % SCALE as u64
    )
}

/// `|delta| / |bound| * 100` in ticks, half up, exact; None for a zero
/// bound or past the tick range.
pub fn percent_ticks(delta: i64, bound: i64) -> Option<i64> {
    if bound == 0 {
        return None;
    }
    let numerator = i128::from(delta).abs() * 100 * i128::from(SCALE);
    let denominator = i128::from(bound).abs();
    let mut value = numerator / denominator;
    if (numerator % denominator) * 2 >= denominator {
        value += 1;
    }
    i64::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_round_half_up_from_their_shortest_digits() {
        for (v, t) in [
            (0.0, 0),
            (0.1, 10000),
            (0.000005, 1),
            (-0.000005, -1),
            (0.0000049999, 0),
            (1e-7, 0),
            (0.123455, 12346),
            (2.675, 267500),
            (0.010005, 1001),
            (1.0e13, 1_000_000_000_000_000_000),
            (12345.678905, 1234567891),
        ] {
            assert_eq!(value_ticks(v), Ok(t), "{v}");
        }
        assert!(value_ticks(1.0e14).is_err());
        assert!(value_ticks(f64::NAN).is_err());
        // 92233720368547.75807 ticks past i64
        assert!(value_ticks(9.9e13).is_err());
    }

    #[test]
    fn json_values_read_as_python_decimal_reads_str_of_them() {
        assert_eq!(json_ticks(&json!(0.05)), Some(5000));
        assert_eq!(json_ticks(&json!(5)), Some(500000));
        assert_eq!(json_ticks(&json!("0.100005")), Some(10001));
        assert_eq!(json_ticks(&json!(" 1e-3 ")), Some(100));
        assert_eq!(json_ticks(&json!(null)), None);
        assert_eq!(json_ticks(&json!(true)), None);
        assert_eq!(json_ticks(&json!("abc")), None);
        assert_eq!(json_float(&json!("0.1")), Some(0.1));
        assert_eq!(json_float(&json!(true)), Some(1.0));
        assert_eq!(json_float(&json!("inf")), None);
        assert_eq!(json_float(&json!(null)), None);
        assert_eq!(json_float(&json!("1_0.5")), Some(10.5));
    }

    #[test]
    fn steps_take_five_meaningful_decimals() {
        assert_eq!(parse_step("0.1"), Ok(10000));
        assert_eq!(parse_step("0.10000"), Ok(10000));
        assert_eq!(parse_step(" 0.00001 "), Ok(1));
        assert_eq!(parse_step("1e-5"), Ok(1));
        assert_eq!(parse_step("2"), Ok(200000));
        assert!(parse_step("0.000006").is_err());
        assert!(parse_step("0").is_err());
        assert!(parse_step("-1").is_err());
        assert!(parse_step("inf").is_err());
        assert!(parse_step("abc").is_err());
        assert!(parse_step("1e14").is_err());
    }

    #[test]
    fn ticks_format_and_ratio() {
        assert_eq!(format_ticks(12345, false), "0.12345");
        assert_eq!(format_ticks(-5, false), "-0.00005");
        assert_eq!(format_ticks(100000, true), "+1.00000");
        assert_eq!(percent_ticks(6510, 10500), Some(6200000));
        assert_eq!(percent_ticks(1, 3), Some(3333333));
        assert_eq!(percent_ticks(5, 0), None);
        assert_eq!(percent_ticks(-2, -4), Some(5000000));
    }
}
