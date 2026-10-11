//! Observed 10-color order. The analysis CLI's historical CHIP-block colors
//! are separate from the viewer's level -> source-chip tree.
use super::{
    geom::{entry_pairs, Placement},
    parser::JobDeck,
};
use crate::{catalog::regular_file, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

pub const PALETTE: [&str; 10] = [
    "#0000ff", "#ffff00", "#ff0000", "#ffc0cb", "#ffa500", "#ffffff", "#a020f0", "#00ffff",
    "#ff00ff", "#00ff00",
];
const NAMES: [&str; 10] = [
    "blue", "yellow", "red", "pink", "orange", "white", "purple", "cyan", "magenta", "green",
];
const RESERVE: [&str; 49] = [
    "#ff0000", "#00ff00", "#2626ff", "#ffff00", "#ff00ff", "#00ffff", "#ff8000", "#80ff00",
    "#00ff80", "#0080ff", "#8000ff", "#ff0080", "#ff7373", "#73ff73", "#7373ff", "#ffff73",
    "#ff73ff", "#73ffff", "#ffb973", "#b9ff73", "#73ffb9", "#73b9ff", "#b973ff", "#ff73b9",
    "#b80000", "#00b800", "#4949b8", "#b8b800", "#b800b8", "#00b8b8", "#b85c00", "#5cb800",
    "#00b85c", "#005cb8", "#732eb8", "#b8005c", "#d94141", "#41d941", "#4141d9", "#d9d941",
    "#d941d9", "#41d9d9", "#d98d41", "#8dd941", "#41d98d", "#418dd9", "#8d41d9", "#d9418d",
    "#ffffff",
];

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    #[serde(alias = "identifier", alias = "id", alias = "levels")]
    Level,
    #[serde(alias = "chips")]
    Chip,
    #[serde(alias = "layers")]
    Layer,
}
impl Mode {
    pub fn parse(text: &str) -> Result<Self> {
        match text.to_ascii_lowercase().as_str() {
            "level" | "identifier" | "id" | "levels" => Ok(Self::Level),
            "chip" | "chips" => Ok(Self::Chip),
            "layer" | "layers" => Ok(Self::Layer),
            _ => Err(Error::input("jobdeck mode must be level, chip or layer")),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct ColorScheme {
    pub mode: Mode,
    pub palette: Vec<String>,
    pub overrides: BTreeMap<String, String>,
    pub cross_ly_dt: bool,
    pub fallback: String,
}
impl Default for ColorScheme {
    fn default() -> Self {
        Self {
            mode: Mode::Level,
            palette: PALETTE.into_iter().map(str::to_owned).collect(),
            overrides: BTreeMap::new(),
            cross_ly_dt: true,
            fallback: "#808080".into(),
        }
    }
}
#[derive(Deserialize)]
#[serde(untagged)]
enum PaletteSpec {
    Named(String),
    Colors(Vec<String>),
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct SavedScheme {
    mode: Option<String>,
    palette: Option<PaletteSpec>,
    overrides: Option<BTreeMap<String, String>>,
    cross_ly_dt: Option<bool>,
    fallback: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColorKey {
    Level(i64),
    Chip(String),
    Layer(i64, i64),
}
impl ColorKey {
    fn text(&self) -> String {
        match self {
            Self::Level(i) => i.to_string(),
            Self::Chip(c) => c.clone(),
            Self::Layer(l, d) => format!("{l}/{d}"),
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct OrderRow {
    pub pos: usize,
    pub key: String,
    pub color: String,
    pub name: String,
    pub source: String,
}
#[derive(Debug)]
pub struct ColorMap {
    pub values: BTreeMap<ColorKey, String>,
    pub order: Vec<OrderRow>,
}
impl ColorMap {
    pub fn report(&self, mode: Mode) -> Value {
        let map: BTreeMap<_, _> = self.values.iter().map(|(k, v)| (k.text(), v)).collect();
        json!({"mode":mode,"order":self.order,"map":map})
    }
}
impl ColorScheme {
    pub fn load(path: &Path) -> Result<Self> {
        let f = regular_file(path)?;
        let mut bytes = Vec::new();
        f.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(Error::input("color scheme exceeds 4 MiB"));
        }
        Self::from_json(&bytes)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let saved: SavedScheme = serde_json::from_slice(bytes)
            .map_err(|e| Error::input(format!("invalid color scheme: {e}")))?;
        let mut scheme = Self::default();
        if let Some(mode) = saved.mode {
            scheme.mode = Mode::parse(&mode)?;
        }
        if let Some(palette) = saved.palette {
            scheme.palette = match palette {
                PaletteSpec::Named(name) => match name.to_ascii_lowercase().as_str() {
                    "jobdeck" => PALETTE.to_vec(),
                    "reserve" => RESERVE.to_vec(),
                    _ => {
                        return Err(Error::input(
                            "unknown palette; use jobdeck, reserve, or a list",
                        ))
                    }
                }
                .into_iter()
                .map(str::to_owned)
                .collect(),
                PaletteSpec::Colors(colors) => colors,
            };
        }
        if let Some(overrides) = saved.overrides {
            scheme.overrides = overrides;
        }
        if let Some(cross) = saved.cross_ly_dt {
            scheme.cross_ly_dt = cross;
        }
        if let Some(fallback) = saved.fallback {
            scheme.fallback = fallback;
        }
        scheme.validate()?;
        Ok(scheme)
    }
    pub fn validate(&self) -> Result<()> {
        if self.palette.is_empty() || self.palette.len() > 65_536 {
            return Err(Error::input("color palette must have 1..65536 entries"));
        }
        for color in self
            .palette
            .iter()
            .chain(self.overrides.values())
            .chain(std::iter::once(&self.fallback))
        {
            if color.len() > 128
                || !color.is_ascii()
                || color.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                return Err(Error::input("invalid color token"));
            }
        }
        Ok(())
    }
    pub fn palette_color(&self, position: usize) -> &str {
        // Public model fields can be edited before validate/build. No panic
        // on a temporarily empty palette; build/view still reject it.
        if self.palette.is_empty() {
            &self.fallback
        } else {
            &self.palette[position % self.palette.len()]
        }
    }
    fn pinned(&self, key: &str) -> Option<&str> {
        let bare = key
            .strip_prefix("CHIP:")
            .or_else(|| key.strip_prefix('$'))
            .or_else(|| key.strip_prefix("LY"))?;
        self.overrides
            .get(bare)
            .filter(|s| !s.is_empty())
            .map(String::as_str)
    }
    pub fn build(&self, deck: &JobDeck, ids: Option<&BTreeSet<i64>>) -> Result<ColorMap> {
        self.validate()?;
        let chip_ids = chip_ids(deck);
        let pairs = if self.mode == Mode::Layer {
            layer_pairs(deck, self.cross_ly_dt)?
        } else {
            BTreeSet::new()
        };
        let order: Vec<String> = match self.mode {
            Mode::Level => deck.levels().iter().map(|i| format!("${i}")).collect(),
            Mode::Chip => chip_order(&chip_ids, ids.unwrap_or(&deck.levels())),
            Mode::Layer => pairs
                .iter()
                .map(|(l, _)| *l)
                .collect::<BTreeSet<_>>()
                .iter()
                .map(|l| format!("LY{l}"))
                .collect(),
        };
        let order: Vec<_> = order
            .into_iter()
            .enumerate()
            .map(|(pos, key)| {
                let pinned = self.pinned(&key);
                let color = pinned.unwrap_or_else(|| self.palette_color(pos)).to_owned();
                OrderRow {
                    pos,
                    key,
                    name: color_name(&color).into(),
                    color,
                    source: if pinned.is_some() {
                        "pinned"
                    } else {
                        "palette"
                    }
                    .into(),
                }
            })
            .collect();
        let lookup: BTreeMap<_, _> = order
            .iter()
            .map(|r| (r.key.as_str(), r.color.as_str()))
            .collect();
        let mut values = BTreeMap::new();
        match self.mode {
            Mode::Level => {
                for level in deck.levels() {
                    values.insert(
                        ColorKey::Level(level),
                        lookup[format!("${level}").as_str()].into(),
                    );
                }
            }
            Mode::Chip => {
                for chip in chip_ids {
                    values.insert(
                        ColorKey::Chip(chip.into()),
                        lookup[format!("CHIP:{chip}").as_str()].into(),
                    );
                }
            }
            Mode::Layer => {
                for (ly, dt) in pairs {
                    let color = self
                        .overrides
                        .get(&format!("{ly}/{dt}"))
                        .filter(|s| !s.is_empty())
                        .map(String::as_str)
                        .unwrap_or(lookup[format!("LY{ly}").as_str()]);
                    values.insert(ColorKey::Layer(ly, dt), color.into());
                }
            }
        }
        Ok(ColorMap { values, order })
    }
    pub fn color_of<'a>(&'a self, map: &'a ColorMap, placement: &Placement) -> &'a str {
        let key = match self.mode {
            Mode::Level => ColorKey::Level(placement.idx),
            Mode::Chip => ColorKey::Chip(placement.chip.clone()),
            Mode::Layer => ColorKey::Layer(placement.ly, placement.dt),
        };
        map.values.get(&key).map_or(&self.fallback, String::as_str)
    }
}
pub fn chip_ids(deck: &JobDeck) -> Vec<&str> {
    let mut seen = BTreeSet::new();
    deck.chips
        .iter()
        .filter_map(|c| seen.insert(c.id.as_str()).then_some(c.id.as_str()))
        .collect()
}
pub fn layer_pairs(deck: &JobDeck, cross: bool) -> Result<BTreeSet<(i64, i64)>> {
    let mut pairs = BTreeSet::new();
    let mut visited = 0usize;
    for e in deck.chips.iter().flat_map(|c| &c.entries) {
        let row = entry_pairs(e, cross, 65_536)?;
        visited += row.len();
        if visited > 4_000_000 {
            return Err(Error::input("jobdeck color table visit limit exceeded"));
        }
        pairs.extend(row);
        if pairs.len() > 65_536 {
            return Err(Error::input("jobdeck color table size limit exceeded"));
        }
    }
    Ok(pairs)
}
pub fn chip_order(chips: &[&str], ids: &BTreeSet<i64>) -> Vec<String> {
    let mut out = Vec::new();
    let mut taken = 0;
    for &level in ids {
        let want = level.saturating_sub(1).max(0).min(chips.len() as i64) as usize;
        while taken < want {
            out.push(format!("CHIP:{}", chips[taken]));
            taken += 1;
        }
        out.push(format!("${level}"));
    }
    out.extend(chips[taken..].iter().map(|c| format!("CHIP:{c}")));
    out
}
fn color_name(color: &str) -> &str {
    PALETTE
        .iter()
        .position(|p| p.eq_ignore_ascii_case(color))
        .map_or(color, |i| NAMES[i])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn order_pins_and_aliases() {
        assert_eq!(
            chip_order(&["C1", "C2", "C3"], &BTreeSet::from([1, 3])),
            ["$1", "CHIP:C1", "CHIP:C2", "$3", "CHIP:C3"]
        );
        assert_eq!(Mode::parse("IDENTIFIER").unwrap(), Mode::Level);
        assert!(ColorScheme::from_json(br#"{"palette":[]}"#).is_err());
        assert!(ColorScheme::from_json(br#"{"palette":["red\nsource"]}"#).is_err());
        let scheme = ColorScheme::from_json(
            br##"{"mode":"chip","palette":"reserve","overrides":{"C1":"#123456"}}"##,
        )
        .unwrap();
        assert_eq!(scheme.palette.len(), 49);
        assert_eq!(scheme.pinned("CHIP:C1"), Some("#123456"));
    }
}
