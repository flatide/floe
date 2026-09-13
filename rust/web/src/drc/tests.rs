use super::*;
use serde_json::json;
fn rules() -> Request {
    serde_json::from_value(json!({"kind":"rules","start":"0","search":"","limit":64})).unwrap()
}
#[test]
fn bounded_queue_drop_cancels_and_stop_drains_without_runtime_waits() {
    let s = Service {
        id: "id".into(),
        revision: "revision".into(),
        source_id: "source".into(),
        title: "DRC".into(),
        registration: Registration {
            resources: Resources::new(floe_app_core::managed::Limits::default()).unwrap(),
            scope: AccessScope::new(&[std::env::temp_dir()]).unwrap(),
            path: std::env::temp_dir().join("queue-test-not-opened.db"),
            waives: None,
            rules: None,
            source_id: "source".into(),
        },
        inner: Arc::new(Inner {
            state: Mutex::new(State {
                pending: VecDeque::new(),
                active: None,
                closed: false,
                failure: None,
                metadata: None,
            }),
            wake: Condvar::new(),
        }),
        thread: Mutex::new(None),
    };
    let a = s.submit(rules()).unwrap();
    let b = s.submit(rules()).unwrap();
    let c = s.submit(rules()).unwrap();
    let d = s.submit(rules()).unwrap();
    assert!(matches!(s.submit(rules()), Err("drc_busy")));
    let flag = Arc::clone(&a.stop);
    drop(a);
    assert_ne!(flag.load(Ordering::Relaxed), 0);
    let e = s.submit(rules()).unwrap();
    assert_eq!(s.inner.state.lock().unwrap().pending.len(), 4);
    let active = Arc::new(AtomicUsize::new(0));
    s.inner.state.lock().unwrap().active = Some(Arc::clone(&active));
    s.request_stop();
    assert_ne!(active.load(Ordering::Relaxed), 0);
    assert!(s.inner.state.lock().unwrap().pending.is_empty());
    assert!(matches!(s.submit(rules()), Err("drc_closed")));
    drop((b, c, d, e));
}
#[test]
fn wire_rejects_paths_noncanonical_counters_and_unbounded_reads() {
    for v in [
        json!({"kind":"rules","start":"00","search":"","limit":1}),
        json!({"kind":"rules","start":"0","search":"","limit":65}),
        json!({"kind":"rules","start":"0","search":"x".repeat(257),"limit":1}),
        json!({"kind":"rules","start":"0","search":"","limit":1,"metric":""}),
        json!({"kind":"rules","start":"0","search":"","limit":1,"metric":"x".repeat(65)}),
        json!({"kind":"types","start":"00","limit":64}),
        json!({"kind":"types","start":"0","limit":65}),
        json!({"kind":"comparison","check":"00","error":"0"}),
        json!({"kind":"query","bbox_um":["NaN","0","1","1"],"checks":null,"waived":null,"cursor":{"check":"0","error":"0"},"limit":1}),
        json!({"kind":"geometry","check":"0","error":"0","start":"0","limit":2049}),
        json!({"kind":"errors","check":"0","start":"-1","waived":null,"limit":1}),
        json!({"kind":"step","check":"0","backwards":false,"after":"00"}),
        json!({"kind":"step","check":"0","backwards":false,"after":"0","cursor":{"next":"1","remaining":"2"}}),
        json!({"kind":"step","check":"0","backwards":false,"cursor":{"next":"0","remaining":"-1"}}),
        json!({"kind":"step","check":"0","backwards":false,"bbox_um":["0","0","-1","1"]}),
        json!({"kind":"measurements","check":"00","error":"0"}),
        json!({"kind":"measurements","check":"0","error":"-1"}),
        json!({"kind":"records","check":"00","errors":[]}),
        json!({"kind":"records","check":"0","errors":["00"]}),
        json!({"kind":"records","check":"0","errors":vec!["0";65]}),
        json!({"kind":"list","check":"0","start":"0","limit":65,"in_view":false}),
        json!({"kind":"list","check":"0","start":"0","limit":64,"in_view":false,"selection_rev":"0"}),
        json!({"kind":"filtered_step","check":"0","backwards":false,"in_view":false,"selection_rev":"1","cursor":{"next":"1","remaining":"1"}}),
    ] {
        assert!(serde_json::from_value::<Request>(v)
            .unwrap()
            .core()
            .is_err());
    }
    for v in [
        json!({"kind":"rule","check":0}),
        json!({"kind":"rules","start":"0","search":"","limit":1,"metric":true}),
        json!({"kind":"types","start":"0","limit":64,"path":"/etc/passwd"}),
        json!({"kind":"comparison","check":"0","error":"0","points":[[0,0],[1,1]]}),
        json!({"kind":"comparison","check":"0","error":"0","rule":"other"}),
        json!({"kind":"rule","check":"0","path":"/etc/passwd"}),
        json!({"kind":"rule","check":"0","reviewer":"other"}),
        json!({"kind":"step","check":"0","backwards":1}),
        json!({"kind":"step","check":"0","backwards":false,"cursor":{"next":0,"remaining":"1"}}),
        json!({"kind":"step","check":"0","backwards":false,"cursor":{"next":"0","remaining":"1","path":"/etc/passwd"}}),
        json!({"kind":"measurements","check":"0","error":1}),
        json!({"kind":"measurements","check":"0","error":"1","path":"/etc/passwd"}),
        json!({"kind":"measurements","check":"0","error":"1","points":[[0,0],[1,1]]}),
        json!({"kind":"records","check":"0","errors":[0]}),
        json!({"kind":"records","check":"0","errors":[],"path":"/etc/passwd"}),
        json!({"kind":"list","check":"0","start":"0","limit":64,"in_view":1}),
        json!({"kind":"list","check":"0","start":"0","limit":64,"in_view":false,"errors":["1"]}),
        json!({"kind":"filtered_step","check":"0","backwards":false,"in_view":false,"path":"/etc/passwd"}),
    ] {
        assert!(serde_json::from_value::<Request>(v).is_err());
    }
    assert!(rules().core().is_ok());
    let s = "18446744073709551615";
    assert!(serde_json::from_value::<Request>(
        json!({"kind":"errors","check":"0","start":s,"waived":true,"limit":1})
    )
    .unwrap()
    .core()
    .is_ok());
}

#[test]
fn list_filters_require_authoritative_context_and_keep_empty_selection_empty() {
    use super::dto::{Command, FocusContext};
    use std::collections::BTreeSet;
    for kind in ["list", "filtered_step"] {
        for in_view in [false, true] {
            for selected in [false, true] {
                let mut value = json!({"kind":kind,"check":"2","in_view":in_view,
                    "selection_rev":selected.then_some("9007199254740993")});
                if kind == "list" {
                    value["start"] = json!("0");
                    value["limit"] = json!(64);
                } else {
                    value["backwards"] = json!(true);
                }
                let request: Request = serde_json::from_value(value).unwrap();
                assert_eq!(
                    request.selection_filter().unwrap(),
                    selected.then_some((9007199254740993, 2))
                );
                let mut filters = match request.core().unwrap() {
                    Command::List { filters, .. } | Command::FilteredStep { filters, .. } => {
                        filters
                    }
                    _ => unreachable!(),
                };
                assert_eq!(filters.bounds().is_err(), in_view);
                assert_eq!(filters.selection().is_err(), selected);
                filters.context = Some(FocusContext {
                    bbox_dbu: [-100., -50., 100., 50.],
                    dbu: 0.001,
                    pixels: [800, 400],
                });
                assert_eq!(
                    filters.bounds().unwrap(),
                    in_view.then_some([-0.1, -0.05, 0.1, 0.05])
                );
                filters.selected = Some(BTreeSet::new());
                if selected {
                    assert!(filters.selection().unwrap().unwrap().is_empty());
                } else {
                    assert!(
                        filters.selection().is_err(),
                        "unsolicited selection changed an all-errors request"
                    );
                }
            }
        }
    }
}
