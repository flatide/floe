//! A capture's layer legend (user 2026-10-13: "floe2 render --legend" - the
//! layers it draws, as a DRC capture lists its isolated ones): flateyes
//! `legend=` lines, `box COLOUR FILL NAME L/D`, made from the styles the
//! frame is drawn with, so the legend is what the picture shows - the layer
//! properties' colours and patterns, --layer-colors, --archival's solid
//! fills, a jobdeck's level colours or its chip view's chip colours - and,
//! of a jobdeck, the rows with a placement in the frame (user 2026-11-10:
//! "현재 캡쳐된 화면에 있는 것들만 레전드에 포함되게"; captures.rs
//! placed_pairs). Embedded as metadata (annotations::png) beside the
//! pixels, never in them; flateyes shows it, `fe-embed --dump` prints it.
use crate::{annotations, dataset::Dataset, styles, Error, Result};
use floe_worker_client::{Fill, Layers, Style};
use std::collections::{BTreeMap, BTreeSet};

/// The view's L/D pairs with a placement in the frame (a jobdeck's; the
/// capture window's judgment, jobdeck::window::DeckWindow::reaches). None:
/// every row of the load - a layout, a DRC capture, FLOE_LEGEND_IN_VIEW=off.
pub type Placed<'a> = Option<&'a BTreeSet<(u32, u32)>>;

struct Row<'a> {
    key: (u32, u32),
    name: &'a str,
    /// on the layer panel when every layer is on: a layout's pair holding
    /// something, a jobdeck row its view does not hide
    shown: bool,
    /// the row it stands under (a jobdeck row its level view hides: the
    /// level's head)
    under: Option<(u32, u32)>,
    /// a jobdeck level's head whose chips are listed on their own (the chip
    /// view hides none): a group with no shapes of its own and no colour
    /// of the picture, never listed (2026-11-09)
    group: bool,
    /// a placement of it in the frame - a head's: one of its level's; a
    /// layout's rows always (2026-11-10)
    placed: bool,
}
impl Row<'_> {
    /// Listed when every layer is on: shown, a colour of the picture, in
    /// the frame.
    fn listed(&self) -> bool {
        self.shown && !self.group && self.placed
    }
}

fn pair(layer: i64, datatype: i64) -> Result<(u32, u32)> {
    Ok((
        u32::try_from(layer).map_err(|_| Error::input("invalid deck layer"))?,
        u32::try_from(datatype).map_err(|_| Error::input("invalid deck datatype"))?,
    ))
}

fn rows<'a>(dataset: &'a Dataset, placed: Placed) -> Result<Vec<Row<'a>>> {
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
                shown: l.stored_shapes > 0,
                under: None,
                group: false,
                placed: true,
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
            // the levels whose chips are shown on their own (the chip view
            // hides none): their heads are groups, not colours of the picture
            let own_rows: BTreeSet<i64> = d
                .metadata
                .layers
                .iter()
                .filter(|r| !r.jobdeck_head && !r.jobdeck_hidden)
                .map(|r| r.layer)
                .collect();
            d.metadata
                .layers
                .iter()
                .map(|r| {
                    let key = pair(r.layer, r.datatype)?;
                    // in the frame: a placement of the row's own, or of a
                    // head's level
                    let placed = placed.is_none_or(|p| {
                        if r.jobdeck_head {
                            p.range((key.0, 0)..=(key.0, u32::MAX)).next().is_some()
                        } else {
                            p.contains(&key)
                        }
                    });
                    Ok(Row {
                        key,
                        name: &r.name,
                        shown: !r.jobdeck_hidden,
                        under: if r.jobdeck_hidden {
                            heads.get(&r.layer).copied()
                        } else {
                            None
                        },
                        group: r.jobdeck_head && own_rows.contains(&r.layer),
                        placed,
                    })
                })
                .collect()
        }
    }
}

/// The pairs listed, in order: the given ones as given (a hidden row as its
/// head, once; a group, or a row with no placement in the frame, skipped),
/// or with every layer on the listed rows in (layer, datatype) order.
fn picks(rows: &[Row], layers: &Layers) -> Vec<(u32, u32)> {
    match layers {
        Layers::None => Vec::new(),
        Layers::All => {
            let mut keys: Vec<_> = rows.iter().filter(|r| r.listed()).map(|r| r.key).collect();
            keys.sort_unstable();
            keys
        }
        Layers::Only(keys) => {
            let by_key: BTreeMap<_, _> = rows.iter().map(|r| (r.key, r)).collect();
            let mut seen = BTreeSet::new();
            keys.iter()
                .filter_map(|k| by_key.get(k))
                .filter(|r| !r.group && r.placed)
                .map(|r| r.under.unwrap_or(r.key))
                .filter(|k| seen.insert(*k))
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
/// level's head, once; in the chip view the chips are listed and the level
/// heads, groups of no shapes, are not. Of a jobdeck, with `placed` the
/// rows with a placement in the frame alone: a chip one of whose placements
/// is in it, a level one of whose placements is. Empty: nothing to list.
pub fn lines(
    dataset: &Dataset,
    styles: &[Style],
    layers: &Layers,
    placed: Placed,
) -> Result<Vec<String>> {
    let rows = rows(dataset, placed)?;
    let by_key: BTreeMap<_, _> = rows.iter().map(|r| (r.key, r)).collect();
    let picks = picks(&rows, layers);
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

    fn row(
        key: (u32, u32),
        shown: bool,
        under: Option<(u32, u32)>,
        group: bool,
        placed: bool,
    ) -> Row<'static> {
        Row {
            key,
            name: "",
            shown,
            under,
            group,
            placed,
        }
    }

    #[test]
    fn the_rows_listed_are_the_pictures_and_in_the_frame() {
        // a deck's level view: level 2's head over its two chips, hidden
        // under it, and level 3's head over one chip; of the placements only
        // level 2's first chip is in the frame
        let level = [
            row((2, 0), true, None, false, true),
            row((2, 1), false, Some((2, 0)), false, true),
            row((2, 3), false, Some((2, 0)), false, false),
            row((3, 0), true, None, false, false),
            row((3, 2), false, Some((3, 0)), false, false),
        ];
        assert_eq!(picks(&level, &Layers::All), vec![(2, 0)]);
        // the chips on, as --chip gives them: their head, once; a chip out
        // of the frame lists nothing, nor does a level out of it by its head
        assert_eq!(
            picks(&level, &Layers::Only(vec![(2, 3), (2, 1), (3, 2), (3, 0)])),
            vec![(2, 0)]
        );
        assert!(picks(&level, &Layers::Only(vec![(2, 3), (3, 0)])).is_empty());
        // the chip view: the heads are groups, the chips listed on their own
        let chip = [
            row((2, 0), true, None, true, true),
            row((2, 1), true, None, false, true),
            row((2, 3), true, None, false, false),
            row((3, 0), true, None, true, false),
            row((3, 2), true, None, false, false),
        ];
        assert_eq!(picks(&chip, &Layers::All), vec![(2, 1)]);
        assert_eq!(
            picks(&chip, &Layers::Only(vec![(2, 3), (2, 1), (2, 0)])),
            vec![(2, 1)]
        );
        assert!(picks(&chip, &Layers::None).is_empty());
        // every row placed (a layout; a deck with FLOE_LEGEND_IN_VIEW=off):
        // the view's rows as before - the level view's heads, the chip
        // view's chips, in (layer, datatype) order
        let every = |rows: &[Row<'static>]| -> Vec<Row<'static>> {
            rows.iter()
                .map(|r| row(r.key, r.shown, r.under, r.group, true))
                .collect()
        };
        assert_eq!(picks(&every(&level), &Layers::All), vec![(2, 0), (3, 0)]);
        assert_eq!(
            picks(&every(&chip), &Layers::All),
            vec![(2, 1), (2, 3), (3, 2)]
        );
        assert_eq!(
            picks(&every(&level), &Layers::Only(vec![(3, 2), (2, 3), (2, 1)])),
            vec![(3, 0), (2, 0)]
        );
        // a layout's pair holding nothing is not listed with every layer on
        // but is when named
        let layout = [row((1, 0), true, None, false, true), row((2, 0), false, None, false, true)];
        assert_eq!(picks(&layout, &Layers::All), vec![(1, 0)]);
        assert_eq!(picks(&layout, &Layers::Only(vec![(2, 0)])), vec![(2, 0)]);
    }
}
