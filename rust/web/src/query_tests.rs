use super::*;
use floe_worker_client::{PickHit, QueryReply, QueryRequest, SceneId, SnapHit};

fn stamp() -> QueryAnchor {
    QueryAnchor {
        dataset_revision: u64::MAX,
        worker_epoch: u64::MAX - 1,
        frame_id: u64::MAX - 2,
        state_rev: 5,
        render_rev: 3,
        render_key: 2,
    }
}
fn request() -> Value {
    json!({"anchor":anchor(stamp()),"operation":{"kind":"pick","nth":"-1"},
        "position":[0.5,0.5],"radius_px":10.,"layers":{"mode":"all"}})
}
#[test]
fn query_dtos_require_canonical_strings_and_strict_bounded_input() {
    let parsed: Request = serde_json::from_value(request()).unwrap();
    let core = parsed.core().unwrap();
    assert_eq!(core.anchor, stamp());
    assert_eq!(core.operation, QueryOperation::Pick { nth: -1 });
    for field in [
        "dataset_revision",
        "worker_epoch",
        "frame_id",
        "state_rev",
        "render_rev",
        "render_key",
    ] {
        for invalid in [json!(1), json!(null), json!({}), json!([])] {
            let mut v = request();
            v["anchor"][field] = invalid;
            assert!(serde_json::from_value::<Request>(v).is_err());
        }
        for invalid in ["0", "01", "+1", "-1", "1.0", "18446744073709551616", "한글"] {
            let mut v = request();
            v["anchor"][field] = json!(invalid);
            assert!(serde_json::from_value::<Request>(v)
                .unwrap()
                .core()
                .is_err());
        }
    }
    for invalid in [
        json!(null),
        json!({"kind":"pick"}),
        json!({"kind":"pick","nth":1}),
        json!({"kind":"snap","nth":"1"}),
        json!({"kind":"clip"}),
    ] {
        let mut v = request();
        v["operation"] = invalid;
        assert!(serde_json::from_value::<Request>(v).is_err());
    }
    for cycle in [
        "-0",
        "01",
        "+1",
        "9223372036854775808",
        "-9223372036854775809",
    ] {
        let mut v = request();
        v["operation"]["nth"] = json!(cycle);
        assert!(serde_json::from_value::<Request>(v)
            .unwrap()
            .core()
            .is_err());
    }
    for cycle in [i64::MIN, i64::MAX, 0] {
        let mut v = request();
        v["operation"]["nth"] = json!(cycle.to_string());
        assert!(serde_json::from_value::<Request>(v).unwrap().core().is_ok());
    }
    for point in [[-0.001, 0.5], [1.001, 0.5], [0.5, 1.001]] {
        let mut v = request();
        v["position"] = json!(point);
        assert!(serde_json::from_value::<Request>(v)
            .unwrap()
            .core()
            .is_err());
    }
    for radius in [-1., 64.001] {
        let mut v = request();
        v["radius_px"] = json!(radius);
        assert!(serde_json::from_value::<Request>(v)
            .unwrap()
            .core()
            .is_err());
    }
    for extra in ["path", "scene_gen", "out", "binary"] {
        let mut v = request();
        v[extra] = json!("/must/not/be/used");
        assert!(serde_json::from_value::<Request>(v).is_err());
    }
    let mut r: Request = serde_json::from_value(request()).unwrap();
    r.radius_px = f64::NAN;
    assert!(r.core().is_err());
    let mut r: Request = serde_json::from_value(request()).unwrap();
    r.position[0] = f64::INFINITY;
    assert!(r.core().is_err());
    for pairs in [vec![], vec![(7, 0); 4097]] {
        let mut r: Request = serde_json::from_value(request()).unwrap();
        r.layers = Selection::Only { pairs };
        assert!(r.core().is_err());
    }
    for s in [r#"{"snap":null}"#, r#""all""#, "null"] {
        assert!(serde_json::from_str::<Kind>(s).is_err());
    }
}

#[test]
fn displayed_receipts_are_bounded_scoped_and_allow_crop_revisions() {
    let a = stamp();
    let receipt = Receipt {
        frame_id: a.frame_id,
        dataset_revision: a.dataset_revision,
        worker_epoch: a.worker_epoch,
        render_key: a.render_key,
        margin: false,
    };
    let mut seen = Receipts::default();
    assert!(!seen.accepts(a));
    seen.displayed(receipt);
    assert!(seen.accepts(a));
    for i in 0..4 {
        let mut bad = a;
        match i {
            0 => bad.frame_id -= 1,
            1 => bad.dataset_revision -= 1,
            2 => bad.worker_epoch -= 1,
            _ => bad.render_key += 1,
        }
        assert!(!seen.accepts(bad));
    }
    let mut margin = a;
    margin.frame_id -= 1;
    seen.displayed(Receipt {
        frame_id: margin.frame_id,
        margin: true,
        ..receipt
    });
    margin.render_rev += 1;
    margin.state_rev += 1;
    assert!(seen.accepts(margin)); // Current crop/state is still checked by core.
    seen.displayed(Receipt {
        frame_id: 1,
        ..receipt
    });
    assert!(!seen.accepts(a));
    assert!(seen.accepts(margin));
    assert!(!Receipts::default().accepts(margin)); // A new connection has no receipts.
    assert_eq!(seen.0.iter().flatten().count(), 2);
}

fn result() -> (Ticket, ViewQueryResult) {
    let t = Ticket {
        sequence: u64::MAX.to_string(),
        id: u64::MAX,
        anchor: stamp(),
        sent: false,
    };
    let r = ViewQueryResult {
        id: t.id,
        anchor: t.anchor,
        reply: QueryReply {
            sequence: 8,
            request: QueryRequest {
                scene: SceneId {
                    generation: 6,
                    round: 2,
                },
                operation: QueryOperation::Snap,
                x: 0,
                y: 0,
                radius: 1,
                layers: Layers::All,
            },
            scene: QueryScene {
                id: Some(SceneId {
                    generation: 6,
                    round: 2,
                }),
                complete: true,
                summary_layers: 0,
            },
            status: QueryStatus::Ok,
            summary_layers: 0,
            error: None,
            hit: Some(QueryHit::Snap(SnapHit {
                x: i64::MIN,
                y: i64::MAX,
                kind: SnapKind::Edge,
            })),
        },
    };
    (t, r)
}
#[test]
fn typed_results_preserve_large_coordinates_and_never_serialize_native_errors() {
    let (t, mut r) = result();
    let v = response(&t, &r, "view", "connection");
    assert_eq!(v["seq"], u64::MAX.to_string());
    assert_eq!(v["anchor"]["dataset_revision"], u64::MAX.to_string());
    assert_eq!(
        v["hit"]["point_dbu"],
        json!([i64::MIN.to_string(), i64::MAX.to_string()])
    );
    assert_eq!(v["status"], "ok");
    for (status, code) in [
        (QueryStatus::Unavailable, "scene_unavailable"),
        (QueryStatus::Mismatch, "scene_mismatch"),
        (QueryStatus::Incomplete, "scene_incomplete"),
        (QueryStatus::Summary, "scene_summary"),
        (QueryStatus::Superseded, "superseded"),
        (QueryStatus::Error, "query_failed"),
    ] {
        r.reply.status = status;
        r.reply.hit = None;
        r.reply.error = Some("/private/synthetic/secret path errno details".into());
        let v = response(&t, &r, "view", "connection");
        assert_eq!(v["status"], code);
        assert!(v["hit"].is_null());
        assert!(!v.to_string().contains("secret"));
        assert!(v.get("error").is_none());
    }
    r.reply.status = QueryStatus::Ok;
    r.reply.error = None;
    r.reply.hit = Some(QueryHit::Pick(PickHit {
        count: 64,
        index: 63,
        layer: (u32::MAX, 300),
        layer_name: "mask <tag> 한글".into(),
        cell_name: "\"title\n".into(),
        area: 123456789.25,
        bbox: [i64::MIN, i64::MIN, i64::MAX, i64::MAX],
        points: vec![(i64::MIN, i64::MAX); 512],
        points_truncated: true,
    }));
    let v = response(&t, &r, "view", "connection");
    assert_eq!(v["hit"]["pair"], json!([u32::MAX, 300]));
    assert_eq!(v["hit"]["area_dbu2"], "123456789.25");
    assert_eq!(v["hit"]["points_truncated"], true);
    assert_eq!(v["hit"]["points_dbu"].as_array().unwrap().len(), 512);
    assert!(v.to_string().len() < view::CONTROL_REPLY_BYTES);
    assert_eq!(serde_json::from_str::<Value>(&v.to_string()).unwrap(), v);
}
