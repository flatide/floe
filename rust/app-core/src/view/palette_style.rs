//! Palette assignments, not resolved-style replacement. GTK submits only
//! selected colors but the entire sparse fill/width maps to the deck adapter.
use super::properties::AssignedFill;
use super::{Model, ViewState};
use crate::{Error, Result};
use floe_worker_client::{Fill, Style};
use std::{collections::BTreeSet, sync::Arc};

const LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug)]
pub enum WidthEdit {
    Set(u8),
    Step(i8),
}
#[derive(Clone, Debug, Default)]
pub struct StyleBatch {
    pub pairs: Vec<(u32, u32)>,
    pub collapsed: Vec<(u32, u32)>,
    pub color: Option<[u8; 4]>,
    pub fill: Option<Fill>,
    pub fill_slot: Option<String>,
    pub width: Option<WidthEdit>,
}
impl StyleBatch {
    pub fn validate(&self) -> Result<()> {
        if self.pairs.is_empty()
            || self.pairs.len() > LIMIT
            || self.collapsed.len() > LIMIT
            || self.color.is_some_and(|c| c[3] != 255)
            || self.width.is_some_and(|w| match w {
                WidthEdit::Set(n) => !(1..=8).contains(&n),
                WidthEdit::Step(n) => ![-1, 1].contains(&n),
            })
            || (self.fill.is_some() && self.fill_slot.is_some())
            || self
                .fill_slot
                .as_ref()
                .is_some_and(|s| crate::styles::pattern_slot(s).is_none())
            || (self.color.is_none()
                && self.fill.is_none()
                && self.fill_slot.is_none()
                && self.width.is_none())
        {
            return Err(Error::input("invalid palette style batch"));
        }
        Ok(())
    }
}
impl ViewState {
    pub(super) fn apply_style_batch(&mut self, model: &Model, batch: StyleBatch) -> Result<()> {
        batch.validate()?;
        let mut targets: BTreeSet<_> = batch.pairs.iter().copied().collect();
        if !targets.is_subset(&model.pairs) {
            return Err(Error::input("unknown palette row"));
        }
        let mut folded: BTreeSet<_> = batch.collapsed.into_iter().collect();
        for &head in &folded {
            let mut rows = model.pairs.range((head.0, 0)..=(head.0, u32::MAX));
            if !targets.contains(&head) || rows.next() != Some(&head) || rows.next().is_none() {
                return Err(Error::input("collapsed rows must be selected group heads"));
            }
        }
        folded.extend(model.folded_groups.intersection(&targets).copied());
        for head in folded {
            for &p in model.pairs.range((head.0, 0)..=(head.0, u32::MAX)) {
                targets.insert(p);
                if targets.len() > LIMIT {
                    return Err(Error::input("palette style expands beyond 4096 rows"));
                }
            }
        }
        // A synthetic head's color propagates even when expanded. Fills and
        // widths instead inherit only where no child assignment exists. Count
        // possible downstream effects too, before changing either Arc.
        let mut affected = targets.clone();
        for p in &targets {
            if let Some(children) = model.groups.get(p) {
                for &child in children {
                    affected.insert(child);
                    if affected.len() > LIMIT {
                        return Err(Error::input("palette style affects more than 4096 rows"));
                    }
                }
            }
        }
        let fill = batch.fill.map(AssignedFill::Direct).or_else(|| {
            batch.fill_slot.as_deref().map(|name| {
                AssignedFill::Slot(crate::styles::pattern_slot(name).expect("validated slot"))
            })
        });
        let repattern = fill.is_some() || batch.width.is_some();
        if repattern {
            let assignments = Arc::make_mut(&mut self.assignments);
            for p in &targets {
                if let Some(fill) = &fill {
                    assignments.fills.insert(*p, fill.clone());
                }
                if let Some(edit) = batch.width {
                    let width = match edit {
                        WidthEdit::Set(n) => n,
                        // GTK steps the sparse assignment (default 1), not
                        // the inherited display width of the child.
                        WidthEdit::Step(n) => {
                            (i16::from(assignments.widths.get(p).copied().unwrap_or(1))
                                + i16::from(n))
                            .clamp(1, 8) as u8
                        }
                    };
                    if width == 1 {
                        assignments.widths.remove(p);
                    } else {
                        assignments.widths.insert(*p, width);
                    }
                }
            }
        }
        self.styles = Arc::new(
            self.styles
                .iter()
                .map(|old| {
                    let parent = (old.layer.0, 0);
                    let inherited = model.groups.contains_key(&parent);
                    Style {
                        layer: old.layer,
                        color: batch
                            .color
                            .filter(|_| affected.contains(&old.layer))
                            .unwrap_or(old.color),
                        fill: if repattern {
                            self.assignments
                                .fills
                                .get(&old.layer)
                                .or_else(|| {
                                    inherited
                                        .then(|| self.assignments.fills.get(&parent))
                                        .flatten()
                                })
                                .map(|f| self.assignments.resolve(f))
                                .unwrap_or(Fill::Speckle)
                        } else {
                            old.fill.clone()
                        },
                        width: if repattern {
                            self.assignments
                                .widths
                                .get(&old.layer)
                                .or_else(|| {
                                    inherited
                                        .then(|| self.assignments.widths.get(&parent))
                                        .flatten()
                                })
                                .copied()
                                .unwrap_or(1)
                        } else {
                            old.width
                        },
                    }
                })
                .collect(),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Patch;
    use floe_worker_client::Layers;
    use std::collections::BTreeMap;

    fn model(pairs: &[(u32, u32)], heads: &[(u32, u32)], folded: &[(u32, u32)]) -> Model {
        Model {
            minimap: Arc::default(),
            dataset_revision: 1,
            dbu: 1.,
            bbox: [0., 0., 100., 100.],
            deck: !heads.is_empty(),
            skipped: 0,
            source_stale: false,
            styles: Arc::new(
                pairs
                    .iter()
                    .map(|&layer| Style {
                        layer,
                        color: [255; 4],
                        fill: Fill::Speckle,
                        width: 1,
                    })
                    .collect(),
            ),
            pairs: pairs.iter().copied().collect(),
            groups: heads
                .iter()
                .map(|&h| {
                    (
                        h,
                        pairs
                            .iter()
                            .copied()
                            .filter(|p| p.0 == h.0 && *p != h)
                            .collect(),
                    )
                })
                .collect(),
            folded_groups: folded.iter().copied().collect(),
            initial_layers: Layers::All,
            assignments: Arc::default(),
            property_names: pairs.iter().map(|&p| (p, "MASK".into())).collect(),
        }
    }
    fn edit(s: &ViewState, m: &Model, b: StyleBatch) -> ViewState {
        s.edit(
            m,
            Patch {
                style_batch: Some(b),
                ..Default::default()
            },
        )
        .unwrap()
    }
    #[test]
    fn expanded_assignment_inheritance_and_collapsed_overrides_are_distinct() {
        let m = model(&[(1, 0), (1, 1), (1, 2), (2, 0)], &[(1, 0)], &[]);
        let s = ViewState::initial(&m, 100, 100)
            .unwrap()
            .edit(
                &m,
                Patch {
                    properties: Some(
                        crate::layerprops::parse("1 red solid HEAD 1 6\n1.1 blue clear CHILD 1 3")
                            .unwrap(),
                    ),
                    ..Default::default()
                },
            )
            .unwrap();
        let b = StyleBatch {
            pairs: vec![(1, 0)],
            color: Some([34, 170, 136, 255]),
            fill: Some(Fill::Pattern([0x1234; 16])),
            width: Some(WidthEdit::Set(5)),
            ..Default::default()
        };
        let expanded = edit(&s, &m, b.clone());
        assert!(expanded.styles[..3]
            .iter()
            .all(|s| s.color == [34, 170, 136, 255]));
        assert_eq!(wire(&expanded.styles[1].fill), "clear");
        assert_eq!(expanded.styles[1].width, 3);
        assert_eq!(expanded.styles[2].fill, Fill::Pattern([0x1234; 16]));
        assert_eq!(expanded.styles[2].width, 5);
        assert_eq!(expanded.assignments.fills.get(&(1, 2)), None);
        let collapsed = edit(
            &s,
            &m,
            StyleBatch {
                collapsed: vec![(1, 0)],
                ..b.clone()
            },
        );
        assert!(collapsed.styles[..3]
            .iter()
            .all(|s| s.fill == Fill::Pattern([0x1234; 16]) && s.width == 5));
        let mut level = m;
        level.folded_groups.insert((1, 0));
        assert_eq!(
            edit(&s, &level, b),
            collapsed,
            "hidden level children are permanently folded"
        );
        assert_eq!(collapsed.layers, s.layers);
        assert_eq!(collapsed.viewport, s.viewport);
    }
    #[test]
    fn steps_use_sparse_width_and_color_does_not_materialize_assignments() {
        let m = model(&[(1, 0), (1, 1), (1, 2), (2, 0)], &[(1, 0)], &[]);
        let s = ViewState::initial(&m, 100, 100)
            .unwrap()
            .edit(
                &m,
                Patch {
                    properties: Some(crate::layerprops::parse("1 red solid HEAD 1 6").unwrap()),
                    ..Default::default()
                },
            )
            .unwrap();
        let colored = edit(
            &s,
            &m,
            StyleBatch {
                pairs: vec![(1, 1)],
                color: Some([34, 170, 136, 255]),
                ..Default::default()
            },
        );
        assert_eq!(colored.assignments, s.assignments);
        assert_eq!(colored.styles[1].width, 6);
        let narrow = edit(
            &colored,
            &m,
            StyleBatch {
                pairs: vec![(1, 1)],
                width: Some(WidthEdit::Step(1)),
                ..Default::default()
            },
        );
        assert_eq!(narrow.styles[1].width, 2);
        assert_eq!(narrow.styles[2].width, 6);
        let inherit = edit(
            &narrow,
            &m,
            StyleBatch {
                pairs: vec![(1, 1)],
                width: Some(WidthEdit::Step(-1)),
                ..Default::default()
            },
        );
        assert_eq!(inherit, colored);
        assert_eq!(
            edit(
                &s,
                &m,
                StyleBatch {
                    pairs: vec![(1, 1)],
                    width: Some(WidthEdit::Set(1)),
                    ..Default::default()
                }
            ),
            s
        );
        let restored = ViewState::initial(&m, 100, 100)
            .unwrap()
            .edit(
                &m,
                Patch {
                    settings: Some(narrow.settings(&m)),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(restored, narrow);
    }
    #[test]
    fn invalid_and_overexpanded_styles_are_atomic() {
        let m = model(&[(3, 1), (3, 2), (3, 300), (7, 0)], &[], &[]);
        let s = ViewState::initial(&m, 100, 100).unwrap();
        let valid = StyleBatch {
            pairs: vec![(3, 1)],
            fill: Some(Fill::Clear),
            ..Default::default()
        };
        for b in [
            StyleBatch::default(),
            StyleBatch {
                pairs: vec![(9, 9)],
                ..valid.clone()
            },
            StyleBatch {
                collapsed: vec![(3, 2)],
                ..valid.clone()
            },
            StyleBatch {
                pairs: vec![(7, 0)],
                collapsed: vec![(7, 0)],
                ..valid.clone()
            },
            StyleBatch {
                width: Some(WidthEdit::Set(0)),
                ..valid.clone()
            },
            StyleBatch {
                width: Some(WidthEdit::Step(2)),
                ..valid.clone()
            },
            StyleBatch {
                color: Some([1, 2, 3, 0]),
                ..valid.clone()
            },
        ] {
            assert!(s
                .edit(
                    &m,
                    Patch {
                        style_batch: Some(b),
                        ..Default::default()
                    }
                )
                .is_err());
        }
        for p in [
            Patch {
                style_deltas: vec![super::super::StyleDelta {
                    layer: (3, 1),
                    width: Some(2),
                    ..Default::default()
                }],
                ..Default::default()
            },
            Patch {
                properties: Some(crate::layerprops::parse("").unwrap()),
                ..Default::default()
            },
            Patch {
                settings: Some(s.settings(&m)),
                ..Default::default()
            },
        ] {
            assert!(s
                .edit(
                    &m,
                    Patch {
                        style_batch: Some(valid.clone()),
                        ..p
                    }
                )
                .is_err());
        }
        assert_eq!(s, ViewState::initial(&m, 100, 100).unwrap());
        let m = model(
            &(0..4097).map(|dt| (1, dt)).collect::<Vec<_>>(),
            &[(1, 0)],
            &[],
        );
        let s = ViewState::initial(&m, 100, 100).unwrap();
        assert!(s
            .edit(
                &m,
                Patch {
                    style_batch: Some(StyleBatch {
                        pairs: vec![(1, 0)],
                        width: Some(WidthEdit::Set(2)),
                        ..Default::default()
                    }),
                    ..Default::default()
                }
            )
            .is_err());
    }
    #[derive(serde::Deserialize)]
    struct Case {
        pairs: Vec<(u32, u32)>,
        heads: Vec<(u32, u32)>,
        forced: Vec<(u32, u32)>,
        selected: Vec<(u32, u32)>,
        collapsed: Vec<(u32, u32)>,
        fills: Vec<((u32, u32), String)>,
        widths: Vec<((u32, u32), u8)>,
        action: String,
        value: serde_json::Value,
        expected: Vec<Expected>,
        assigned_fills: Vec<((u32, u32), String)>,
        assigned_widths: Vec<((u32, u32), u8)>,
    }
    #[derive(serde::Deserialize)]
    struct Expected {
        pair: (u32, u32),
        color: String,
        fill: String,
        width: u8,
    }
    fn wire(fill: &Fill) -> String {
        match fill {
            Fill::Solid => "solid".into(),
            Fill::Clear => "clear".into(),
            Fill::Speckle => "speckle".into(),
            Fill::Pattern(rows) if rows.iter().all(|&n| n == u16::MAX) => "solid".into(),
            Fill::Pattern(rows) if rows.iter().all(|&n| n == 0) => "clear".into(),
            Fill::Pattern(rows)
                if rows
                    .iter()
                    .enumerate()
                    .all(|(i, &n)| n == if i % 2 == 0 { 0xaaaa } else { 0x5555 }) =>
            {
                "speckle".into()
            }
            Fill::Pattern(rows) => format!(
                "pat:{}",
                rows.iter().map(|n| format!("{n:04X}")).collect::<String>()
            ),
        }
    }
    #[test]
    #[ignore = "run tools/validate_palette_styles.py for actual GTK + adapter oracle"]
    fn gtk_palette_style_oracle() {
        let cases: Vec<Case> = serde_json::from_slice(
            &std::fs::read(std::env::var_os("FLOE_PALETTE_STYLE_ORACLE").unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(cases.len(), 10584);
        for (i, c) in cases.iter().enumerate() {
            let m = model(&c.pairs, &c.heads, &c.forced);
            let mut s = ViewState::initial(&m, 100, 100).unwrap();
            let a = Arc::make_mut(&mut s.assignments);
            a.fills = c
                .fills
                .iter()
                .map(|(p, f)| {
                    (
                        *p,
                        AssignedFill::Slot(crate::styles::pattern_slot(f).unwrap()),
                    )
                })
                .collect();
            a.widths = c.widths.iter().copied().collect();
            s.apply_properties(&m, &crate::layerprops::parse("").unwrap())
                .unwrap();
            let mut b = StyleBatch {
                pairs: c.selected.clone(),
                collapsed: c.collapsed.clone(),
                ..Default::default()
            };
            match c.action.as_str() {
                "color" => b.color = Some(crate::styles::color(c.value.as_str().unwrap()).unwrap()),
                "fill" => b.fill_slot = Some(c.value.as_str().unwrap().into()),
                "width" => b.width = Some(WidthEdit::Set(c.value.as_u64().unwrap() as u8)),
                "step" => b.width = Some(WidthEdit::Step(c.value.as_i64().unwrap() as i8)),
                _ => panic!(),
            };
            let out = edit(&s, &m, b);
            for (got, want) in out.styles.iter().zip(&c.expected) {
                assert_eq!(
                    (
                        got.layer,
                        crate::styles::color_text(got.color),
                        wire(&got.fill),
                        got.width
                    ),
                    (want.pair, want.color.clone(), want.fill.clone(), want.width),
                    "case {i} {} {}",
                    c.action,
                    c.value
                );
            }
            assert_eq!(out.styles.len(), c.expected.len());
            assert_eq!(
                out.assignments
                    .fills
                    .iter()
                    .map(|(p, f)| (*p, wire(&out.assignments.resolve(f))))
                    .collect::<BTreeMap<_, _>>(),
                c.assigned_fills.iter().cloned().collect(),
                "fill assignments {i}"
            );
            assert_eq!(
                out.assignments.widths,
                c.assigned_widths.iter().copied().collect(),
                "width assignments {i}"
            );
        }
        println!("GTK PALETTE STYLE: ALL OK (10584 GTK + adapter cases)");
    }
}
