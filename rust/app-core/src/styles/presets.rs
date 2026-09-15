//! Ordered bundled palette, with no runtime file or Python dependency.
use crate::{Error, Result};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Serialize)]
pub struct Color {
    pub name: &'static str,
    pub color: String,
}
#[derive(Debug, Serialize)]
pub struct Pattern {
    pub name: &'static str,
    pub rows: [u16; 16],
}
#[derive(Debug, Serialize)]
pub struct Palette {
    pub colors: Vec<Color>,
    pub fills: Vec<Pattern>,
}
fn lines(text: &'static str, words: usize) -> Result<Vec<Vec<&'static str>>> {
    let mut names = BTreeSet::new();
    let mut result = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let row: Vec<_> = line.split_whitespace().collect();
        if row.len() != words
            || row[0].len() > 64
            || !row[0]
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
            || !names.insert(row[0])
            || result.len() == 256
        {
            return Err(Error::input("invalid bundled palette"));
        }
        result.push(row);
    }
    if result.is_empty() {
        return Err(Error::input("empty bundled palette"));
    }
    Ok(result)
}
fn parse(colors: &'static str, fills: &'static str) -> Result<Palette> {
    let colors = lines(colors, 2)?
        .into_iter()
        .map(|row| {
            let value = format!("#{}", row[1]);
            super::color(&value)
                .map(|v| Color {
                    name: row[0],
                    color: super::color_text(v),
                })
                .ok_or_else(|| Error::input("invalid bundled color"))
        })
        .collect::<Result<Vec<_>>>()?;
    let fills = lines(fills, 17)?
        .into_iter()
        .map(|row| {
            let mut rows = [0; 16];
            for (out, word) in rows.iter_mut().zip(&row[1..]) {
                if word.len() != 4 || !word.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(Error::input("invalid bundled fill"));
                }
                *out = u16::from_str_radix(word, 16)
                    .map_err(|_| Error::input("invalid bundled fill"))?;
            }
            Ok(Pattern { name: row[0], rows })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Palette { colors, fills })
}
pub fn bundled() -> Result<Palette> {
    parse(super::COLORS, super::PATTERNS)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_preserves_order_aliases_and_exact_rows() {
        let p = bundled().unwrap();
        assert_eq!((p.colors.len(), p.fills.len()), (49, 20));
        assert_eq!(
            (p.colors[0].name, p.colors[48].name),
            ("darkorange", "maroon")
        );
        assert_eq!((p.colors[7].name, p.colors[8].name), ("yellow", "yellow1"));
        assert_eq!(p.colors[7].color, p.colors[8].color);
        for color in &p.colors {
            assert_eq!(
                super::super::color(color.name),
                super::super::color(&color.color)
            );
        }
        for p in &p.fills {
            assert_eq!(
                super::super::pattern(p.name),
                Some(floe_worker_client::Fill::Pattern(p.rows))
            );
        }
        assert_eq!(p.fills[18].rows, [u16::MAX; 16]);
        assert_eq!(p.fills[19].rows, [0; 16]);
        assert_eq!(p.fills[0].rows[..4], [0x0101, 0x0202, 0x0404, 0x0808]);
    }
    #[test]
    fn corrupt_bundled_tables_are_errors_not_partial_palettes() {
        for colors in [
            "",
            "a ff0000\na 00ffff",
            "a zzzzzz",
            "a 123456 extra",
            "bad/name 123456",
            "a 한글",
        ] {
            assert!(parse(colors, super::super::PATTERNS).is_err());
        }
        for fills in [
            "",
            "a 0000",
            "a zzzz 0000 0000 0000 0000 0000 0000 0000 0000 0000 0000 0000 0000 0000 0000 0000",
        ] {
            assert!(parse(super::super::COLORS, fills).is_err());
        }
    }
    #[test]
    #[ignore = "run tools/validate_palette_styles.py with actual GTK tables"]
    fn gtk_preset_tables_match() {
        let path = std::env::var_os("FLOE_PRESET_ORACLE").expect("preset oracle required");
        let expected: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(bundled().unwrap()).unwrap(), expected);
        println!("GTK PRESET TABLES: ALL OK (49 colors + 20 exact 16x16 bitmaps)");
    }
}
