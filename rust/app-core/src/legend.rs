//! A capture's layer legend (user 2026-10-13: "floe2 render --legend" - the
//! layers it draws, as a DRC capture lists its isolated ones): flateyes
//! `legend=` lines, `box COLOUR FILL NAME L/D`, made from the styles the
//! frame is drawn with, so the legend is what the picture shows - the layer
//! properties' colours and patterns, --layer-colors, --archival's solid
//! fills, a jobdeck's level colours. Embedded as metadata (annotations::png)
//! beside the pixels, never in them; flateyes shows it, `fe-embed --dump`
//! prints it.
use crate::{annotations, dataset::Dataset, styles, Error, Result};
use floe_worker_client::{Fill, Layers, Style};
use std::collections::{BTreeMap, BTreeSet};

struct Row<'a> {
    key: (u32, u32),
    name: &'a str,
    /// on the layer panel when every layer is on
    listed: bool,
    /// the row it stands under (a jobdeck row its level view hides: the
    /// level's head)
    under: Option<(u32, u32)>,
}

fn pair(layer: i64, datatype: i64) -> Result<(u32, u32)> {
    Ok((
        u32::try_from(layer).map_err(|_| Error::input("invalid deck layer"))?,
        u32::try_from(datatype).map_err(|_| Error::input("invalid deck datatype"))?,
    ))
}

fn rows(dataset: &Dataset) -> Result<Vec<Row<'_>>> {
    match dataset {
        // a layout's pairs that hold no shape or text (the LAYERNAME table's
        // alone) are not on the panel (desktop::list_layers)
        Dataset::Layout(l) => Ok(l
            .metadata
            .layers
            .iter()
            .map(|l| Row {
                key: l.key(),
                name: &l.name,
                listed: l.stored_shapes > 0,
                under: None,
            })
            .collect()),
        Dataset::Deck(d) => {
            let heads = d
                .metadata
                .layers
                .iter()
                .filter(|r| r.jobdeck_head)
                .map(|r| Ok((r.layer, pair(r.layer, r.datatype)?)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            d.metadata
                .layers
                .iter()
                .map(|r| {
                    Ok(Row {
                        key: pair(r.layer, r.datatype)?,
                        name: &r.name,
                        listed: !r.jobdeck_hidden,
                        under: if r.jobdeck_hidden {
                            heads.get(&r.layer).copied()
                        } else {
                            None
                        },
                    })
                })
                .collect()
        }
    }
}

/// A fill as a legend names it: solid, speckle, clear (shown as none), a
/// fillpatterns.def name, or the bitmap itself (pat:HEX64).
pub fn fill_name(fill: &Fill) -> String {
    match fill {
        Fill::Solid => "solid".into(),
        Fill::Speckle => "speckle".into(),
        Fill::Clear => "clear".into(),
        Fill::Pattern(rows) => styles::pattern_name(fill).map_or_else(
            || {
                let hex: String = rows.iter().map(|r| format!("{r:04x}")).collect();
                format!("pat:{hex}")
            },
            str::to_owned,
        ),
    }
}

/// The legend of a frame drawn with `styles` showing `layers`: the given
/// pairs in their order, or with every layer on the layer panel's rows - a
/// layout's pairs that hold something, a jobdeck's shown rows - in (layer,
/// datatype) order. A row the jobdeck's level view hides stands under its
/// level's head, once. Empty: nothing to list.
pub fn lines(dataset: &Dataset, styles: &[Style], layers: &Layers) -> Result<Vec<String>> {
    let rows = rows(dataset)?;
    let by_key: BTreeMap<_, _> = rows.iter().map(|r| (r.key, r)).collect();
    let picks: Vec<(u32, u32)> = match layers {
        Layers::None => Vec::new(),
        Layers::All => {
            let mut keys: Vec<_> = rows.iter().filter(|r| r.listed).map(|r| r.key).collect();
            keys.sort_unstable();
            keys
        }
        Layers::Only(keys) => {
            let mut seen = BTreeSet::new();
            keys.iter()
                .filter_map(|k| by_key.get(k))
                .map(|r| r.under.unwrap_or(r.key))
                .filter(|k| seen.insert(*k))
                .collect()
        }
    };
    let styles: BTreeMap<_, _> = styles.iter().map(|s| (s.layer, s)).collect();
    let mut lines = Vec::with_capacity(picks.len());
    for key in picks {
        let (Some(row), Some(style)) = (by_key.get(&key), styles.get(&key)) else {
            continue;
        };
        let pair = format!("{}/{}", key.0, key.1);
        let name = row.name.trim();
        let label = if name.is_empty() || name == pair {
            pair.clone()
        } else {
            format!("{name} {pair}")
        };
        let line = |label: &str| {
            annotations::legend_line(&format!(
                "box {} {} {label}",
                styles::color_text(style.color),
                fill_name(&style.fill)
            ))
        };
        // a name the metadata cannot carry (a control character): the pair
        lines.push(line(&label).or_else(|_| line(&pair))?);
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fills_are_named_as_flateyes_reads_them() {
        assert_eq!(fill_name(&Fill::Solid), "solid");
        assert_eq!(fill_name(&Fill::Speckle), "speckle");
        assert_eq!(fill_name(&Fill::Clear), "clear");
        let named = styles::pattern("diagonal_right_wide").unwrap();
        assert_eq!(fill_name(&named), "diagonal_right_wide");
        let mut rows = [0u16; 16];
        rows[0] = 0x8001;
        let own = fill_name(&Fill::Pattern(rows));
        assert_eq!(own, format!("pat:8001{}", "0000".repeat(15)));
        // the legend grammar takes every one of them
        for fill in ["solid", "speckle", "clear", "diagonal_right_wide", own.as_str()] {
            annotations::legend_line(&format!("box #ff3f3f {fill} M1 31/0")).unwrap();
        }
    }
}
