use super::*;
use crate::{
    layerprops,
    view::{Patch, StyleBatch, StyleDelta},
};
use floe_worker_client::{Fill, Layers, Style};

#[derive(serde::Deserialize)]
struct OracleState {
    slots: Vec<FillSlot>,
    bindings: Vec<((u32, u32), String)>,
    fills: Vec<((u32, u32), [u16; 16])>,
}
#[derive(serde::Deserialize)]
struct Case {
    name: String,
    initial: OracleState,
    edit: Option<[u16; 16]>,
    after: OracleState,
    later: Option<OracleState>,
}
fn assert_oracle(s: &ViewState, expected: &OracleState) {
    assert_eq!(s.fill_slots(), expected.slots);
    assert_eq!(
        s.assignments
            .fills
            .iter()
            .map(|(pair, f)| {
                let AssignedFill::Slot(name) = f else {
                    panic!("GTK fixture binds named slots");
                };
                (*pair, (*name).to_owned())
            })
            .collect::<Vec<_>>(),
        expected.bindings
    );
    assert_eq!(
        s.styles
            .iter()
            .map(|s| (
                s.layer,
                match s.fill {
                    Fill::Solid => [u16::MAX; 16],
                    Fill::Clear => [0; 16],
                    Fill::Speckle =>
                        std::array::from_fn(|y| if y % 2 == 0 { 0xaaaa } else { 0x5555 }),
                    Fill::Pattern(rows) => rows,
                }
            ))
            .collect::<Vec<_>>(),
        expected.fills
    );
}
#[test]
#[ignore = "run tools/validate_palette_styles.py for actual GTK slot oracle"]
fn gtk_bitmap_slots_match() {
    let cases: Vec<Case> = serde_json::from_slice(
        &std::fs::read(std::env::var_os("FLOE_BITMAP_SLOT_ORACLE").expect("slot oracle required"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(cases.len(), 324);
    let m = model(false, &[(3, 0), (3, 1), (7, 0), (7, 1)]);
    for c in cases {
        let mut s = ViewState::initial(&m, 100, 100).unwrap();
        for row in &c.initial.slots {
            if styles::pattern(&row.name) != Some(Fill::Pattern(row.rows)) {
                s = slot(&s, &m, &row.name, row.rows);
            }
        }
        for (pair, name) in &c.initial.bindings {
            s = s
                .edit(
                    &m,
                    Patch {
                        style_batch: Some(StyleBatch {
                            pairs: vec![*pair],
                            fill_slot: Some(name.clone()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        assert_oracle(&s, &c.initial);
        if let Some(rows) = c.edit {
            s = slot(&s, &m, &c.name, rows);
        }
        assert_oracle(&s, &c.after);
        assert_eq!(load(&s, &m, s.settings(&m)), s);
        if let Some(expected) = c.later {
            s = s
                .edit(
                    &m,
                    Patch {
                        style_batch: Some(StyleBatch {
                            pairs: vec![(7, 0)],
                            collapsed: vec![(7, 0)],
                            fill_slot: Some(c.name),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_oracle(&s, &expected);
        }
    }
    println!("GTK BITMAP SLOTS: ALL OK (324 native cases)");
}

fn model(deck: bool, pairs: &[(u32, u32)]) -> Model {
    Model {
        minimap: Arc::default(),
        dataset_revision: 1,
        dbu: 1.,
        bbox: [0., 0., 100., 100.],
        deck,
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
        groups: if deck {
            [(
                (3, 0),
                pairs
                    .iter()
                    .copied()
                    .filter(|p| p.0 == 3 && p.1 != 0)
                    .collect(),
            )]
            .into()
        } else {
            Default::default()
        },
        folded_groups: Default::default(),
        initial_layers: Layers::All,
        assignments: Arc::default(),
        property_names: pairs.iter().map(|&p| (p, "MASK".into())).collect(),
    }
}

#[test]
fn cache_hint_tracks_table_not_assignments_and_survives_settings_roundtrip() {
    let m = model(false, &[(3, 0), (3, 1)]);
    let s = ViewState::initial(&m, 100, 100).unwrap();
    let key = s.fill_slots_key();
    assert_eq!(key.len(), 40);
    let bound = s
        .edit(
            &m,
            Patch {
                style_batch: Some(StyleBatch {
                    pairs: vec![(3, 0)],
                    fill_slot: Some("brick".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(bound.fill_slots_key(), key);
    let edited = slot(&bound, &m, "brick", [1; 16]);
    assert_ne!(edited.fill_slots_key(), key);
    assert_ne!(
        edited.fill_slots_key(),
        slot(&bound, &m, "plus", [1; 16]).fill_slots_key()
    );
    assert_eq!(
        edited.fill_slots_key(),
        slot(&bound, &m, "brick", [1; 16]).fill_slots_key()
    );
    let default = s
        .fill_slots()
        .into_iter()
        .find(|p| p.name == "brick")
        .unwrap()
        .rows;
    assert_eq!(slot(&edited, &m, "brick", default).fill_slots_key(), key);
    let mut moved = edited.clone();
    moved.viewport.bbox[0] += 1.;
    assert_eq!(moved.fill_slots_key(), edited.fill_slots_key());
    assert_eq!(
        load(&s, &m, edited.settings(&m)).fill_slots_key(),
        edited.fill_slots_key()
    );
}
fn standard(deck: bool) -> Model {
    model(deck, &[(3, 0), (3, 1), (3, 2), (7, 0)])
}
fn props(s: &ViewState, m: &Model, text: &str) -> ViewState {
    s.edit(
        m,
        Patch {
            properties: Some(layerprops::parse(text).unwrap()),
            ..Default::default()
        },
    )
    .unwrap()
}
fn slot(s: &ViewState, m: &Model, name: &str, rows: [u16; 16]) -> ViewState {
    s.edit(m, edit(name, rows)).unwrap()
}
fn edit(name: &str, rows: [u16; 16]) -> Patch {
    Patch {
        fill_slot_edit: Some(FillSlotEdit {
            name: name.into(),
            rows,
        }),
        ..Default::default()
    }
}
fn load(s: &ViewState, m: &Model, settings: layerprops::Settings) -> ViewState {
    s.edit(
        m,
        Patch {
            settings: Some(layerprops::parse_settings(&settings.text().unwrap()).unwrap()),
            ..Default::default()
        },
    )
    .unwrap()
}
#[test]
fn slot_fanout_preserves_inheritance_and_direct_value_identity() {
    let m = standard(true);
    let initial = ViewState::initial(&m, 100, 100).unwrap();
    let s = props(
        &initial,
        &m,
        "3 red brick HEAD 1 4\n3.1 blue clear CHILD 1 2",
    );
    let s = s
        .edit(
            &m,
            Patch {
                style_deltas: vec![StyleDelta {
                    layer: (7, 0),
                    fill: Some(styles::pattern("brick").unwrap()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .unwrap();
    let next = slot(&s, &m, "BRICK", [0x1234; 16]);
    assert_eq!(next.styles[0].fill, Fill::Pattern([0x1234; 16]));
    assert_eq!(next.styles[2].fill, next.styles[0].fill);
    assert_eq!(next.styles[1], s.styles[1]);
    assert_eq!(
        next.styles[3], s.styles[3],
        "equal literal is not a slot reference"
    );
    assert_eq!(
        next.styles.iter().map(|s| s.width).collect::<Vec<_>>(),
        [4, 2, 4, 1]
    );
    assert!(!next.same_policy(&s, true));
    let json = next.settings(&m);
    assert_eq!(json.version, 2);
    assert_eq!(json.rows[0].fill_slot.as_deref(), Some("brick"));
    assert!(json.rows[2].fill_slot.is_none() && json.rows[2].fill.is_none());
    assert!(json.rows[3].fill_slot.is_none() && json.rows[3].fill.is_some());
    assert_eq!(load(&initial, &m, json), next);
    assert_eq!(
        next.layerprops(&m).unwrap_err().kind,
        crate::ErrorKind::Unsupported
    );
    let detached = next
        .edit(
            &m,
            Patch {
                style_batch: Some(StyleBatch {
                    pairs: vec![(3, 2)],
                    fill: Some(Fill::Pattern([0x1234; 16])),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(detached.same_policy(&next, true));
    assert_ne!(
        detached, next,
        "identity-only edit must still advance state revision"
    );
    let changed = slot(&detached, &m, "brick", [0x4321; 16]);
    assert_eq!(changed.styles[2].fill, Fill::Pattern([0x1234; 16]));
    assert_eq!(changed.styles[0].fill, Fill::Pattern([0x4321; 16]));
    let assigned = props(&changed, &m, "7 yellow BRICK LEAF 1 1");
    assert_eq!(assigned.styles[3].fill, changed.styles[0].fill);
}
#[test]
fn unused_slots_round_trip_and_reset_without_changing_pixels() {
    let m = standard(false);
    let initial = ViewState::initial(&m, 100, 100).unwrap();
    let s = slot(&initial, &m, "brick", [0x1234; 16]);
    assert_ne!(s, initial);
    assert!(s.same_policy(&initial, false));
    assert!(Arc::ptr_eq(&s.styles, &initial.styles));
    assert_eq!(load(&initial, &m, s.settings(&m)), s);
    let assigned = s
        .edit(
            &m,
            Patch {
                style_batch: Some(StyleBatch {
                    pairs: vec![(3, 0)],
                    collapsed: vec![(3, 0)],
                    fill_slot: Some("brick".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(assigned.styles[..3]
        .iter()
        .all(|s| s.fill == Fill::Pattern([0x1234; 16])));
    assert_eq!(assigned.styles[3], initial.styles[3]);
    let Fill::Pattern(default) = styles::pattern("brick").unwrap() else {
        unreachable!()
    };
    assert_eq!(slot(&s, &m, "brick", default), initial);
    assert_eq!(
        load(&s, &m, initial.settings(&m)),
        initial,
        "v1 resets unused slots too"
    );
}
#[test]
fn legacy_settings_remain_literal_after_loading_and_future_slot_edit() {
    let m = standard(false);
    let initial = ViewState::initial(&m, 100, 100).unwrap();
    let mut legacy = initial.settings(&m);
    assert_eq!(legacy.version, 1);
    legacy.rows[0].fill = Some(layerprops::Bitmap::from(&styles::pattern("brick").unwrap()));
    let s = load(&initial, &m, legacy.clone());
    assert_eq!(s.settings(&m), legacy);
    let edited = slot(&s, &m, "brick", [0x1234; 16]);
    assert!(edited.same_policy(&s, false));
    assert!(edited.settings(&m).rows[0].fill_slot.is_none());
    let s = props(&s, &m, "3 red brick MASK 1 1");
    assert_ne!(slot(&s, &m, "brick", [0x1234; 16]).styles, s.styles);
}
#[test]
fn invalid_fixed_or_conflicting_slots_and_overflow_are_atomic() {
    let m = standard(false);
    let initial = ViewState::initial(&m, 100, 100).unwrap();
    for name in ["solid", "CLEAR", "missing", "한글", &"x".repeat(65)] {
        assert!(initial.edit(&m, edit(name, [1; 16])).is_err());
    }
    for mut bad in [
        Patch {
            style_batch: Some(StyleBatch {
                pairs: vec![(3, 0)],
                fill_slot: Some("brick".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        Patch {
            properties: Some(layerprops::parse("").unwrap()),
            ..Default::default()
        },
        Patch {
            settings: Some(initial.settings(&m)),
            ..Default::default()
        },
        Patch {
            prepared_layers: Some(initial.prepare_layers(&m, Patch::default()).unwrap()),
            ..Default::default()
        },
        Patch {
            style_changes: vec![initial.styles[0].clone()],
            ..Default::default()
        },
        Patch {
            style_deltas: vec![StyleDelta {
                layer: (3, 0),
                width: Some(2),
                ..Default::default()
            }],
            ..Default::default()
        },
    ] {
        bad.fill_slot_edit = edit("brick", [1; 16]).fill_slot_edit;
        assert!(initial.edit(&m, bad).is_err());
    }
    assert!(StyleBatch {
        pairs: vec![(3, 0)],
        fill: Some(Fill::Solid),
        fill_slot: Some("brick".into()),
        ..Default::default()
    }
    .validate()
    .is_err());
    assert!(StyleBatch {
        pairs: vec![(3, 0)],
        fill_slot: Some("missing".into()),
        ..Default::default()
    }
    .validate()
    .is_err());
    for size in [4096, 4097] {
        let m = model(true, &(0..size).map(|dt| (3, dt)).collect::<Vec<_>>());
        let base = props(
            &ViewState::initial(&m, 100, 100).unwrap(),
            &m,
            "3 red brick MASK 1 1",
        );
        let before = base.settings(&m).text().unwrap();
        let next = base.edit(&m, edit("brick", [1; 16]));
        assert_eq!(next.is_ok(), size == 4096);
        if let Ok(next) = next {
            assert!(next.styles.iter().all(|s| s.fill == Fill::Pattern([1; 16])));
        }
        assert_eq!(base.settings(&m).text().unwrap(), before);
    }
}
#[test]
fn native_v2_rejects_incomplete_duplicate_fixed_and_ambiguous_tables() {
    let m = standard(false);
    let s = props(
        &ViewState::initial(&m, 100, 100).unwrap(),
        &m,
        "3 red brick MASK 1 1",
    );
    let valid = serde_json::to_value(s.settings(&m)).unwrap();
    let mut cases = Vec::new();
    let mut bad = valid.clone();
    bad["version"] = 1.into();
    cases.push(bad);
    let mut bad = valid.clone();
    bad["version"] = 3.into();
    cases.push(bad);
    let mut bad = valid.clone();
    bad.as_object_mut().unwrap().remove("fill_slots");
    cases.push(bad);
    let mut bad = valid.clone();
    bad["fill_slots"].as_array_mut().unwrap().pop();
    cases.push(bad);
    let mut bad = valid.clone();
    bad["fill_slots"][1] = bad["fill_slots"][0].clone();
    cases.push(bad);
    let mut bad = valid.clone();
    bad["fill_slots"][18]["rows"][0] = 0.into();
    cases.push(bad);
    let mut bad = valid.clone();
    bad["fill_slots"][0]["name"] = "unknown".into();
    cases.push(bad);
    let mut bad = valid.clone();
    bad["fill_slots"][0]["rows"].as_array_mut().unwrap().pop();
    cases.push(bad);
    let mut bad = valid.clone();
    bad["rows"][0]["fill"] = serde_json::json!({"kind":"clear"});
    cases.push(bad);
    let mut bad = valid.clone();
    bad["rows"][0]["fill_slot"] = "BRICK".into();
    cases.push(bad);
    let mut bad = valid.clone();
    bad["fill_slots"][0]["file"] = "/not/read".into();
    cases.push(bad);
    for bad in cases {
        assert!(
            layerprops::parse_settings(&bad.to_string()).is_err(),
            "{bad}"
        );
    }
    assert_eq!(load(&s, &m, s.settings(&m)), s);
}
