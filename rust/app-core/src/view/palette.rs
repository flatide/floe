//! GTK palette visibility semantics, independent of widget and render order.
//! Collapse affects the target set, never layer geometry or style inheritance.
use super::{Model, ViewState};
use crate::{Error, Result};
use floe_worker_client::Layers;
use std::collections::BTreeSet;

const LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug)]
pub enum LayerAction {
    Show,
    Hide,
    Toggle,
}
#[derive(Clone, Debug)]
pub struct LayerBatch {
    pub action: LayerAction,
    pub pairs: Vec<(u32, u32)>,
    /// Selected physical group heads whose children are collapsed in the
    /// panel. Jobdeck heads always affect all children, expanded or not.
    pub collapsed: Vec<(u32, u32)>,
}
impl LayerBatch {
    pub fn validate(&self) -> Result<()> {
        if self.pairs.is_empty() || self.pairs.len() > LIMIT || self.collapsed.len() > LIMIT {
            return Err(Error::input("palette batch requires 1..4096 selected rows"));
        }
        Ok(())
    }
}
impl ViewState {
    pub(super) fn edit_layer_batch(&self, model: &Model, batch: LayerBatch) -> Result<Layers> {
        batch.validate()?;
        let rows: BTreeSet<_> = batch.pairs.into_iter().collect();
        if !rows.is_subset(&model.pairs) {
            return Err(Error::input("unknown palette row"));
        }
        let collapsed: BTreeSet<_> = batch.collapsed.into_iter().collect();
        for &head in &collapsed {
            let mut group = model.pairs.range((head.0, 0)..=(head.0, u32::MAX));
            if !rows.contains(&head) || group.next() != Some(&head) || group.next().is_none() {
                return Err(Error::input("collapsed rows must be selected group heads"));
            }
        }
        let render_pairs = model
            .styles
            .iter()
            .filter(|s| !model.groups.contains_key(&s.layer));
        let total = render_pairs.clone().count();
        let mut selected: BTreeSet<_> = match &self.layers {
            Layers::All => render_pairs.map(|s| s.layer).collect(),
            Layers::None => BTreeSet::new(),
            Layers::Only(p) => p.iter().copied().collect(),
        };
        let mut covered = BTreeSet::new();
        let mut changed = false;
        // Palette order is (layer,datatype), not browser selection order.
        // A selected collapsed/jobdeck parent wins over selected children:
        // no double toggle and no intermediate render of a half-applied batch.
        for row in rows {
            if covered.contains(&row) {
                continue;
            }
            let targets: Vec<_> = if let Some(children) = model.groups.get(&row) {
                children.iter().take(LIMIT + 1).copied().collect()
            } else if collapsed.contains(&row) {
                model
                    .pairs
                    .range((row.0, 0)..=(row.0, u32::MAX))
                    .take(LIMIT + 1)
                    .copied()
                    .collect()
            } else {
                vec![row]
            };
            if targets.is_empty() {
                return Err(Error::input("layer group has no renderable children"));
            }
            let active = if model.groups.contains_key(&row) {
                targets.iter().any(|p| selected.contains(p))
            } else {
                selected.contains(&row)
            };
            let on = match batch.action {
                LayerAction::Show => true,
                LayerAction::Hide => false,
                LayerAction::Toggle => !active,
            };
            for pair in targets {
                covered.insert(pair);
                if covered.len() > LIMIT {
                    return Err(Error::input("palette batch expands beyond 4096 layers"));
                }
                if on {
                    changed |= selected.insert(pair);
                } else {
                    changed |= selected.remove(&pair);
                }
            }
        }
        if !changed {
            // Keep equivalent explicit-all representations unchanged too;
            // merely canonicalizing a no-op must not spend a render revision.
            Ok(self.layers.clone())
        } else if selected.len() == total {
            Ok(Layers::All)
        } else if selected.is_empty() {
            Ok(Layers::None)
        } else {
            model.layers(&Layers::Only(selected.into_iter().collect()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Patch;
    use floe_worker_client::{Fill, Style};
    use std::{collections::BTreeMap, sync::Arc};
    fn make_model(pairs: &[(u32, u32)], heads: &[(u32, u32)]) -> Model {
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
                        fill: Fill::Solid,
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
                .collect::<BTreeMap<_, _>>(),
            initial_layers: Layers::All,
            folded_groups: BTreeSet::new(),
            assignments: Arc::default(),
            property_names: Vec::new(),
        }
    }
    fn batch(pairs: Vec<(u32, u32)>, collapsed: Vec<(u32, u32)>, action: LayerAction) -> Patch {
        Patch {
            layer_batch: Some(LayerBatch {
                pairs,
                collapsed,
                action,
            }),
            ..Default::default()
        }
    }
    #[test]
    fn collapsed_parent_wins_but_expanded_physical_rows_toggle_independently() {
        let model = make_model(&[(3, 1), (3, 2), (3, 300), (9, 0)], &[]);
        let mut state = ViewState::initial(&model, 100, 100).unwrap();
        state.layers = Layers::Only(vec![(3, 2), (9, 0)]);
        let next = state
            .edit(
                &model,
                batch(
                    vec![(3, 2), (3, 1), (3, 2)],
                    vec![(3, 1)],
                    LayerAction::Toggle,
                ),
            )
            .unwrap();
        assert_eq!(next.layers, Layers::All);
        let next = state
            .edit(
                &model,
                batch(vec![(3, 2), (3, 1)], vec![], LayerAction::Toggle),
            )
            .unwrap();
        assert_eq!(next.layers, Layers::Only(vec![(3, 1), (9, 0)]));
        assert_eq!(next.viewport, state.viewport);
        assert!(Arc::ptr_eq(&next.styles, &state.styles));
        assert_eq!(state.layers, Layers::Only(vec![(3, 2), (9, 0)]));
    }
    #[test]
    fn invalid_or_conflicting_batch_never_changes_input() {
        let model = make_model(&[(3, 1), (3, 2), (9, 0)], &[]);
        let state = ViewState::initial(&model, 100, 100).unwrap();
        for p in [
            batch(vec![], vec![], LayerAction::Hide),
            batch(vec![(3, 1), (99, 0)], vec![], LayerAction::Hide),
            batch(vec![(3, 1)], vec![(3, 2)], LayerAction::Hide),
            batch(vec![(3, 2)], vec![(3, 2)], LayerAction::Hide),
            batch(vec![(9, 0)], vec![(9, 0)], LayerAction::Hide),
            batch(vec![(3, 1); LIMIT + 1], vec![], LayerAction::Show),
            Patch {
                layers: Some(Layers::None),
                ..batch(vec![(3, 1)], vec![], LayerAction::Hide)
            },
            Patch {
                layer_change: Some(((3, 1), true)),
                ..batch(vec![(3, 1)], vec![], LayerAction::Hide)
            },
            Patch {
                layer_isolation: Some(super::super::LayerIsolation::Restore),
                ..batch(vec![(3, 1)], vec![], LayerAction::Hide)
            },
            Patch {
                properties: Some(crate::layerprops::Document {
                    rows: vec![],
                    malformed: 0,
                }),
                ..batch(vec![(3, 1)], vec![], LayerAction::Hide)
            },
        ] {
            assert!(state.edit(&model, p).is_err());
            assert_eq!(state.layers, Layers::All);
        }
        let model = make_model(&(0..=LIMIT).map(|n| (3, n as u32)).collect::<Vec<_>>(), &[]);
        let state = ViewState::initial(&model, 100, 100).unwrap();
        assert!(state
            .edit(&model, batch(vec![(3, 0)], vec![(3, 0)], LayerAction::Hide))
            .is_err());
        assert_eq!(state.layers, Layers::All);
    }
    #[test]
    fn partial_jobdeck_head_toggles_all_children_once() {
        let model = make_model(&[(3, 0), (3, 1), (3, 2), (9, 0)], &[(3, 0)]);
        let mut state = ViewState::initial(&model, 100, 100).unwrap();
        state.layers = Layers::Only(vec![(3, 2), (9, 0)]);
        let off = state
            .edit(
                &model,
                batch(vec![(3, 2), (3, 0)], vec![], LayerAction::Toggle),
            )
            .unwrap();
        assert_eq!(off.layers, Layers::Only(vec![(9, 0)]));
        let on = off
            .edit(
                &model,
                batch(vec![(3, 1), (3, 0)], vec![], LayerAction::Toggle),
            )
            .unwrap();
        assert_eq!(on.layers, Layers::All);
    }
    #[test]
    fn no_op_preserves_explicit_all_without_render_state_change() {
        let model = make_model(&[(3, 1), (3, 2), (9, 0)], &[]);
        let mut state = ViewState::initial(&model, 100, 100).unwrap();
        state.layers = Layers::Only(vec![(3, 1), (3, 2), (9, 0)]);
        let next = state
            .edit(&model, batch(vec![(3, 1)], vec![(3, 1)], LayerAction::Show))
            .unwrap();
        assert_eq!(next, state);
    }
    #[derive(serde::Deserialize)]
    struct Case {
        pairs: Vec<(u32, u32)>,
        heads: Vec<(u32, u32)>,
        selected: Vec<(u32, u32)>,
        collapsed: Vec<(u32, u32)>,
        visible: Vec<(u32, u32)>,
        expected: Vec<(u32, u32)>,
        action: String,
    }
    #[test]
    #[ignore = "run tools/validate_layer_palette.py for source-derived GTK oracle"]
    fn gtk_palette_batch_oracle() {
        let path = std::env::var_os("FLOE_LAYER_PALETTE_ORACLE").unwrap();
        let cases: Vec<Case> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(cases.len(), 12096);
        for (i, c) in cases.iter().enumerate() {
            let model = make_model(&c.pairs, &c.heads);
            let mut state = ViewState::initial(&model, 100, 100).unwrap();
            state.layers = if c.visible.is_empty() {
                Layers::None
            } else {
                Layers::Only(c.visible.clone())
            };
            let action = match c.action.as_str() {
                "show" => LayerAction::Show,
                "hide" => LayerAction::Hide,
                "toggle" => LayerAction::Toggle,
                _ => panic!(),
            };
            let result = state
                .edit(
                    &model,
                    batch(c.selected.clone(), c.collapsed.clone(), action),
                )
                .unwrap();
            let got = match result.layers {
                Layers::None => vec![],
                Layers::Only(p) => p,
                Layers::All => c
                    .pairs
                    .iter()
                    .copied()
                    .filter(|p| !c.heads.contains(p))
                    .collect(),
            };
            assert_eq!(got, c.expected, "case {i}: {}", c.action);
            assert_eq!(result.viewport, state.viewport);
            assert!(Arc::ptr_eq(&result.styles, &state.styles));
        }
        println!("GTK PALETTE: ALL OK (12096 source-derived batch cases)");
    }
}
