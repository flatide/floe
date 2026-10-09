//! UI keys are level/source pairs, not the analysis CLI's CHIP-block keys.
//! First-use source ordinals and colors always use the entire deck.
use super::{
    color::{ColorKey, ColorMap, ColorScheme, Mode},
    geom::{Placement, PlanStats},
    parser::JobDeck,
};
use crate::{check_cancelled, Error, Result};
use floe_worker_client::Layers;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicUsize;

const ROW_LIMIT: usize = 65_536;
const TABLE_BYTES: usize = 128 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(untagged)]
pub enum ViewKey {
    Head(i64),
    Source(i64, String),
    Layer(i64, i64),
}
#[derive(Clone, Debug, Serialize)]
pub struct ViewRow {
    pub key: ViewKey,
    pub layer: i64,
    pub datatype: i64,
    pub name: String,
    pub color: String,
    pub out: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
}
impl ViewRow {
    pub fn pair(&self) -> (i64, i64) {
        (self.layer, self.datatype)
    }
    pub fn native_pair(&self) -> Result<(u32, u32)> {
        Ok((
            u32::try_from(self.layer).map_err(|_| Error::input("deck layer must fit u32"))?,
            u32::try_from(self.datatype).map_err(|_| Error::input("deck datatype must fit u32"))?,
        ))
    }
}
#[derive(Debug)]
pub struct ViewRows {
    pub rows: Vec<ViewRow>,
    outputs: BTreeMap<ViewKey, usize>,
    mode: Mode,
}
struct Budget(usize);
impl Budget {
    fn charge(&mut self, n: usize) -> Result<()> {
        self.0 = self
            .0
            .checked_sub(n)
            .ok_or_else(|| Error::input("jobdeck view table memory limit exceeded"))?;
        Ok(())
    }
}
impl ViewRows {
    pub fn build(
        deck: &JobDeck,
        stats: &PlanStats,
        scheme: &ColorScheme,
        colors: &ColorMap,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        scheme.validate()?;
        check_cancelled(cancelled)?;
        let kept = |idx: &i64| stats.selection.as_ref().is_none_or(|s| s.contains(idx));
        let mut rows = Vec::new();
        let mut budget = Budget(TABLE_BYTES);
        let mut push = |mut row: ViewRow| -> Result<()> {
            if rows.len() >= ROW_LIMIT {
                return Err(Error::input("jobdeck view exceeds 65536 rows"));
            }
            budget.charge(
                512 + row.name.len()
                    + row.color.len()
                    + row.source.as_ref().map_or(0, String::len) * 2
                    + row.tooltip.as_ref().map_or(0, String::len),
            )?;
            row.out = rows.len();
            rows.push(row);
            Ok(())
        };
        match scheme.mode {
            Mode::Level | Mode::Chip => {
                struct Group {
                    tc: String,
                    chips: Vec<String>,
                    seen: BTreeSet<String>,
                }
                let mut sources = BTreeMap::new();
                let mut groups: BTreeMap<i64, Vec<Group>> = BTreeMap::new();
                let mut group_index = BTreeMap::new();
                let mut group_budget = Budget(TABLE_BYTES);
                for c in &deck.chips {
                    for e in &c.entries {
                        check_cancelled(cancelled)?;
                        let tc = normalize_tc(&e.tc);
                        if !sources.contains_key(&tc) {
                            if sources.len() >= ROW_LIMIT {
                                return Err(Error::input("jobdeck view source limit exceeded"));
                            }
                            group_budget.charge(128 + tc.len())?;
                            sources.insert(tc.clone(), sources.len() + 1);
                        }
                        let key = (e.idx, tc.clone());
                        let group = groups.entry(e.idx).or_default();
                        let i = if let Some(&i) = group_index.get(&key) {
                            i
                        } else {
                            if group_index.len() >= ROW_LIMIT {
                                return Err(Error::input("jobdeck view group limit exceeded"));
                            }
                            group_budget.charge(256 + tc.len() * 2)?;
                            let i = group.len();
                            group.push(Group {
                                tc,
                                chips: Vec::new(),
                                seen: BTreeSet::new(),
                            });
                            group_index.insert(key, i);
                            i
                        };
                        if !group[i].seen.contains(&c.id) {
                            group_budget.charge(128 + c.id.len() * 2)?;
                            group[i].seen.insert(c.id.clone());
                            group[i].chips.push(c.id.clone());
                        }
                    }
                }
                let levels = deck.levels();
                for (pos, &idx) in levels.iter().enumerate() {
                    check_cancelled(cancelled)?;
                    if !kept(&idx) {
                        continue;
                    }
                    let level_color = if scheme.mode == Mode::Level {
                        colors
                            .values
                            .get(&ColorKey::Level(idx))
                            .ok_or_else(|| Error::input("missing level color"))?
                            .as_str()
                    } else {
                        scheme.palette_color(pos)
                    };
                    push(ViewRow {
                        key: ViewKey::Head(idx),
                        layer: idx,
                        datatype: 0,
                        name: if deck.title(idx).is_empty() {
                            format!("LEVEL{idx}")
                        } else {
                            deck.title(idx).into()
                        },
                        color: level_color.into(),
                        out: 0,
                        head: Some(true),
                        hidden: None,
                        source: None,
                        tooltip: None,
                    })?;
                    for g in groups.get(&idx).into_iter().flatten() {
                        let ordinal = sources[&g.tc];
                        push(ViewRow {
                            key: ViewKey::Source(idx, g.tc.clone()),
                            layer: idx,
                            datatype: ordinal as i64,
                            name: basename(&g.tc).into(),
                            color: if scheme.mode == Mode::Level {
                                level_color
                            } else {
                                scheme.palette_color(levels.len() + ordinal - 1)
                            }
                            .into(),
                            out: 0,
                            head: None,
                            hidden: Some(scheme.mode == Mode::Level),
                            source: Some(g.tc.clone()),
                            tooltip: Some(format!(
                                "source: {}\nCHIP: {}",
                                g.tc,
                                g.chips.join(", ")
                            )),
                        })?;
                    }
                }
            }
            Mode::Layer => {
                let pairs: BTreeSet<_> = stats
                    .layer_table
                    .iter()
                    .filter(|r| kept(&r.idx))
                    .map(|r| (r.ly, r.dt))
                    .collect();
                for (ly, dt) in pairs {
                    check_cancelled(cancelled)?;
                    push(ViewRow {
                        key: ViewKey::Layer(ly, dt),
                        layer: ly,
                        datatype: dt,
                        name: format!("LY{ly}.DT{dt}"),
                        color: colors
                            .values
                            .get(&ColorKey::Layer(ly, dt))
                            .unwrap_or(&scheme.fallback)
                            .clone(),
                        out: 0,
                        head: None,
                        hidden: None,
                        source: None,
                        tooltip: None,
                    })?;
                }
            }
        }
        let outputs = rows.iter().map(|r| (r.key.clone(), r.out)).collect();
        Ok(Self {
            rows,
            outputs,
            mode: scheme.mode,
        })
    }
    pub fn output_layer(&self, p: &Placement) -> Result<usize> {
        let key = match self.mode {
            Mode::Level | Mode::Chip => ViewKey::Source(p.idx, normalize_tc(&p.tc)),
            Mode::Layer => ViewKey::Layer(p.ly, p.dt),
        };
        self.outputs
            .get(&key)
            .copied()
            .ok_or_else(|| Error::input("placement has no deck view row"))
    }
    pub fn metadata(&self, placements: &[Placement]) -> Result<Vec<LayerMetadata>> {
        let mut counts = vec![0u64; self.rows.len()];
        for p in placements {
            counts[self.output_layer(p)?] += 1;
        }
        Ok(self
            .rows
            .iter()
            .map(|r| LayerMetadata {
                layer: r.layer,
                datatype: r.datatype,
                name: r.name.clone(),
                color: r.color.clone(),
                stored_shapes: counts[r.out],
                jobdeck_head: r.head.unwrap_or(false),
                jobdeck_hidden: r.hidden.unwrap_or(false),
                jobdeck_source: r.source.clone(),
                tooltip: r.tooltip.clone().unwrap_or_default(),
            })
            .collect())
    }
    pub fn resolve_layers(&self, deck: &JobDeck, spec: Option<&str>) -> Result<Layers> {
        let Some(spec) = spec.filter(|s| !s.is_empty() && *s != "all") else {
            return Ok(Layers::All);
        };
        let mut names: BTreeMap<String, Vec<(i64, i64)>> = BTreeMap::new();
        let mut keys = BTreeMap::new();
        let mut heads = BTreeSet::new();
        let mut by_level: BTreeMap<i64, Vec<(i64, i64)>> = BTreeMap::new();
        for r in &self.rows {
            names.entry(r.name.clone()).or_default().push(r.pair());
            keys.insert(r.pair(), r.native_pair()?);
            by_level.entry(r.layer).or_default().push(r.pair());
            if r.head == Some(true) {
                heads.insert(r.pair());
                names
                    .entry(format!("${}", r.layer))
                    .or_default()
                    .push(r.pair());
                if !deck.title(r.layer).is_empty() {
                    names
                        .entry(format!("${} {}", r.layer, deck.title(r.layer)))
                        .or_default()
                        .push(r.pair());
                }
            }
        }
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        for token in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let named = if let Some(named) = names.get(token) {
                named.clone()
            } else if let Some((l, d)) = token.split_once('/') {
                vec![(
                    l.parse().map_err(|_| Error::input("invalid deck layer"))?,
                    d.parse()
                        .map_err(|_| Error::input("invalid deck datatype"))?,
                )]
            } else {
                return Err(Error::input(format!("unknown deck layer: {token:?}")));
            };
            for key in named {
                let keys_to_add = if heads.contains(&key) {
                    by_level[&key.0].clone()
                } else {
                    vec![key]
                };
                for key in keys_to_add {
                    let &native = keys
                        .get(&key)
                        .ok_or_else(|| Error::input(format!("unknown deck layer: {token:?}")))?;
                    if seen.insert(native) {
                        out.push(native);
                    }
                }
            }
        }
        Ok(if out.is_empty() {
            Layers::None
        } else {
            Layers::Only(out)
        })
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct LayerMetadata {
    pub layer: i64,
    pub datatype: i64,
    pub name: String,
    pub color: String,
    pub stored_shapes: u64,
    pub jobdeck_head: bool,
    pub jobdeck_hidden: bool,
    pub jobdeck_source: Option<String>,
    pub tooltip: String,
}
#[derive(Debug, Serialize)]
pub struct LevelRow {
    pub level: i64,
    pub name: String,
    pub chips: Vec<String>,
    pub instances: u64,
    pub sources: Vec<String>,
}
pub fn level_rows(deck: &JobDeck, cancelled: &AtomicUsize) -> Result<Vec<LevelRow>> {
    let mut rows: BTreeMap<i64, LevelRow> = BTreeMap::new();
    let mut sources: BTreeSet<(i64, String)> = BTreeSet::new();
    let mut budget = Budget(TABLE_BYTES);
    for c in &deck.chips {
        let mut seen = BTreeSet::new();
        for e in &c.entries {
            check_cancelled(cancelled)?;
            if !rows.contains_key(&e.idx) {
                if rows.len() >= ROW_LIMIT {
                    return Err(Error::input("deck load dialog level limit exceeded"));
                }
                budget.charge(256 + deck.title(e.idx).len())?;
                rows.insert(
                    e.idx,
                    LevelRow {
                        level: e.idx,
                        name: deck.title(e.idx).into(),
                        chips: Vec::new(),
                        instances: 0,
                        sources: Vec::new(),
                    },
                );
            }
            let row = rows.get_mut(&e.idx).expect("inserted level");
            if seen.insert(e.idx) {
                budget.charge(32 + c.id.len())?;
                row.chips.push(c.id.clone());
            }
            row.instances = row
                .instances
                .checked_add(c.rows.len().max(1) as u64)
                .ok_or_else(|| Error::input("deck load instance count overflow"))?;
            if !sources.contains(&(e.idx, e.tc.clone())) {
                budget.charge(128 + e.tc.len() * 2)?;
                sources.insert((e.idx, e.tc.clone()));
                row.sources.push(e.tc.clone());
            }
        }
    }
    Ok(rows.into_values().collect())
}
/// POSIX normpath, without resolving symlinks or making a relative TC absolute.
pub fn normalize_tc(text: &str) -> String {
    let absolute = text.starts_with('/');
    let prefix = if text.starts_with("//") && !text.starts_with("///") {
        "//"
    } else if absolute {
        "/"
    } else {
        ""
    };
    let mut parts = Vec::new();
    for p in text.split('/') {
        match p {
            "" | "." => (),
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            ".." if absolute => (),
            _ => parts.push(p),
        }
    }
    let out = format!("{prefix}{}", parts.join("/"));
    if out.is_empty() {
        ".".into()
    } else {
        out
    }
}
fn basename(tc: &str) -> &str {
    tc.rsplit('/').next().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalized_source_identity_is_lexical() {
        for (s, want) in [
            ("a/./b/../c", "a/c"),
            ("../a", "../a"),
            ("a/../../b", "../b"),
            ("//a///b", "//a/b"),
            ("///a//b", "/a/b"),
            ("", "."),
            ("/../../", "/"),
        ] {
            assert_eq!(normalize_tc(s), want);
        }
    }
}
