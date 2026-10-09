use super::*;

fn model() -> Model {
    Model {
        minimap: Arc::default(),
        dataset_revision: 1,
        dbu: 0.001,
        bbox: [0., 0., 100., 100.],
        deck: true,
        skipped: 0,
        source_stale: false,
        styles: Arc::new(vec![Style {
            layer: (1, 1),
            color: [255; 4],
            fill: floe_worker_client::Fill::Solid,
            width: 1,
        }]),
        pairs: [(1, 0), (1, 1)].into(),
        groups: [((1, 0), vec![(1, 1)])].into(),
        folded_groups: BTreeSet::new(),
        initial_layers: Layers::All,
        assignments: Arc::default(),
        property_names: vec![((1, 1), "CHIP-A".into())],
    }
}

#[test]
fn revision_state_preserves_every_field_and_rejects_reinterpreted_planes() {
    let old = model();
    let mut next = model();
    next.dataset_revision = 2;
    next.bbox = [-100., -100., 200., 200.];
    let state = ViewState::initial(&old, 137, 103)
        .unwrap()
        .edit(
            &old,
            Patch {
                navigation: Some(Navigation::Goto {
                    center_um: [12., 13.],
                    width_um: Some(20.),
                }),
                detail: Some(Detail::High),
                thin: Some(Thin::Keep),
                depth: Some(Depth::Levels(7)),
                mono: Some(true),
                frames: Some(true),
                layers: Some(Layers::None),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(state.for_index_revision(&old, &next).unwrap(), state);
    next.dbu = 0.002;
    assert!(state.for_index_revision(&old, &next).is_err());
    next.dbu = old.dbu;
    next.property_names[0].1 = "CHIP-B".into();
    assert!(state.for_index_revision(&old, &next).is_err());
    next.property_names = old.property_names.clone();
    next.groups.clear();
    assert!(state.for_index_revision(&old, &next).is_err());
    next.groups = old.groups.clone();
    next.pairs.insert((2, 0));
    assert!(state.for_index_revision(&old, &next).is_err());
}
