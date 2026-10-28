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

#[test]
fn a_cut_of_its_own_stands_for_the_detail_until_a_detail_is_set() {
    let m = model();
    let cut = |s: &ViewState| s.request(&m, RenderRequest::default()).cut_px;
    let s = ViewState::initial(&m, 100, 100).unwrap();
    assert_eq!(cut(&s), 3.);
    let own = s
        .edit(
            &m,
            Patch {
                cut_px: Some(0.1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        (own.detail, own.cut_px, cut(&own)),
        (Detail::Medium, Some(0.1), 0.1)
    );
    assert!(!own.same_policy(&s, true), "another cut is another frame");
    // the state carries it across an index revision
    assert_eq!(own.for_index_revision(&m, &m).unwrap(), own);
    // a detail ends it; a patch with both keeps the cut
    let high = Patch {
        detail: Some(Detail::High),
        ..Default::default()
    };
    let back = own.edit(&m, high.clone()).unwrap();
    assert_eq!(
        (back.detail, back.cut_px, cut(&back)),
        (Detail::High, None, 1.)
    );
    let both = Patch {
        cut_px: Some(0.5),
        ..high
    };
    assert_eq!(cut(&own.edit(&m, both).unwrap()), 0.5);
    for bad in [0., -1., f64::NAN, f64::INFINITY, MAX_CUT_PX + 1.] {
        let refused = own.edit(
            &m,
            Patch {
                cut_px: Some(bad),
                ..Default::default()
            },
        );
        assert!(refused.is_err(), "{bad}");
    }
    assert_eq!(
        cut(&own
            .edit(
                &m,
                Patch {
                    cut_px: Some(MAX_CUT_PX),
                    ..Default::default()
                }
            )
            .unwrap()),
        MAX_CUT_PX
    );
}
