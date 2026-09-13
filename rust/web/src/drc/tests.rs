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
        json!({"kind":"query","bbox_um":["NaN","0","1","1"],"checks":null,"waived":null,"cursor":{"check":"0","error":"0"},"limit":1}),
        json!({"kind":"geometry","check":"0","error":"0","start":"0","limit":2049}),
        json!({"kind":"errors","check":"0","start":"-1","waived":null,"limit":1}),
        json!({"kind":"step","check":"0","backwards":false,"after":"00"}),
        json!({"kind":"step","check":"0","backwards":false,"after":"0","cursor":{"next":"1","remaining":"2"}}),
        json!({"kind":"step","check":"0","backwards":false,"cursor":{"next":"0","remaining":"-1"}}),
        json!({"kind":"step","check":"0","backwards":false,"bbox_um":["0","0","-1","1"]}),
        json!({"kind":"measurements","check":"00","error":"0"}),
        json!({"kind":"measurements","check":"0","error":"-1"}),
    ] {
        assert!(serde_json::from_value::<Request>(v)
            .unwrap()
            .core()
            .is_err());
    }
    for v in [
        json!({"kind":"rule","check":0}),
        json!({"kind":"rule","check":"0","path":"/etc/passwd"}),
        json!({"kind":"rule","check":"0","reviewer":"other"}),
        json!({"kind":"step","check":"0","backwards":1}),
        json!({"kind":"step","check":"0","backwards":false,"cursor":{"next":0,"remaining":"1"}}),
        json!({"kind":"step","check":"0","backwards":false,"cursor":{"next":"0","remaining":"1","path":"/etc/passwd"}}),
        json!({"kind":"measurements","check":"0","error":1}),
        json!({"kind":"measurements","check":"0","error":"1","path":"/etc/passwd"}),
        json!({"kind":"measurements","check":"0","error":"1","points":[[0,0],[1,1]]}),
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
