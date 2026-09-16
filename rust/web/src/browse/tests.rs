use super::*;
use floe_app_core::{
    managed::{Limits, Usage},
    native::{Discovery, Indexer},
    render::RenderOptions,
};
use std::{
    fs,
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p =
            std::env::temp_dir().join(format!("floe-picker-{}", crate::auth::public_id().unwrap()));
        fs::create_dir(&p).unwrap();
        Self(fs::canonicalize(p).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn service(resources: &Arc<Resources>) -> Arc<Service> {
    // Selection never invokes these binaries: metadata/proposal only.
    let indexer = Indexer::discover(&Discovery {
        override_path: Some("/bin/sh".into()),
        development_root: None,
        executable: "/none".into(),
        search_path: None,
    })
    .unwrap();
    let options = RenderOptions {
        binary: "/never-invoke-renderd".into(),
        budget_mb: 64,
        decode_jobs: 1,
        raster_jobs: 1,
        tile_px: 384,
        round_pages: 1 << 30,
        open_timeout_s: 1,
        label_font_px: 14,
        raw: true,
        debug: false,
    };
    Service::start(vec![], Arc::clone(resources), options, indexer).unwrap()
}
fn terminal(p: &Picker, seq: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let v = p.operation(seq).unwrap();
        if ["succeeded", "failed", "cancelled"].contains(&v["phase"].as_str().unwrap()) {
            return v;
        }
        assert!(
            Instant::now() < deadline,
            "picker operation did not stop: {v}"
        );
        thread::sleep(Duration::from_millis(2));
    }
}
fn request(p: &Picker, seq: u64) -> Request {
    Request::List {
        seq: seq.to_string(),
        directory: p.roots[0].handle.clone(),
        filter: Filter::AllFiles,
        query: String::new(),
    }
}
fn handle(v: &Value, name: &str) -> String {
    v["result"]["page"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap()["handle"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn catalogue_is_once_only_bounded_and_releases_resources() {
    let f = Fixture::new();
    fs::write(f.0.join("a.oas"), b"not a source").unwrap();
    let r = Resources::new(Limits::default()).unwrap();
    let s = service(&r);
    let l = Launches::new();
    let p = Picker::start(std::slice::from_ref(&f.0), &r, Arc::clone(&s), l).unwrap();
    assert_eq!(r.usage().cpu_slots, 1);
    assert_eq!(r.usage().decoded_mb, 192);
    let input = request(&p, 1);
    let expected = serde_json::to_value(&input).unwrap();
    p.submit(input).unwrap();
    let first = terminal(&p, 1);
    assert_eq!(first["phase"], "succeeded");
    assert_eq!(first["request"], expected);
    fs::write(f.0.join("b.oas"), b"new").unwrap();
    assert_eq!(p.submit(request(&p, 1)).unwrap(), first);
    assert_eq!(
        p.submit(Request::Select {
            seq: "1".into(),
            handle: handle(&first, "a.oas")
        })
        .unwrap_err(),
        "operation_conflict"
    );
    assert_eq!(p.submit(request(&p, 3)).unwrap_err(), "operation_sequence");
    for n in 2..=35 {
        p.submit(request(&p, n)).unwrap();
        assert_eq!(terminal(&p, n)["phase"], "succeeded");
    }
    assert!(p.operation(1).is_none());
    assert_eq!(p.submit(request(&p, 1)).unwrap_err(), "operation_expired");
    assert!(
        p.snapshot().get("history").is_none(),
        "polling must not clone 32 directory pages"
    );
    assert_eq!(s.catalog()["sources"], json!([]));
    assert_eq!(s.operations()["last_seq"], "0");
    p.request_stop();
    let end = Instant::now() + Duration::from_secs(3);
    while !p.is_finished() {
        assert!(
            Instant::now() < end,
            "picker did not release its reservation"
        );
        thread::sleep(Duration::from_millis(2));
    }
    drop(p);
    assert_eq!(r.usage(), Usage::default());
}

#[test]
fn select_registers_but_never_indexes_opens_or_repeats() {
    let f = Fixture::new();
    let path = f.0.join("test.jb");
    fs::write(
        &path,
        "MTITLE 1,One\nMTITLE 2,Two\nCHIP A\n$ (1,A,TC=missingA)\n$ (2,B,TC=missingB)\n",
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    let r = Resources::new(Limits::default()).unwrap();
    let s = service(&r);
    let l = Launches::new();
    let p = Picker::start(
        std::slice::from_ref(&f.0),
        &r,
        Arc::clone(&s),
        Arc::clone(&l),
    )
    .unwrap();
    p.submit(request(&p, 1)).unwrap();
    let page = terminal(&p, 1);
    let h = handle(&page, "test.jb");
    let select = || Request::Select {
        seq: "2".into(),
        handle: h.clone(),
    };
    p.submit(select()).unwrap();
    let selected = terminal(&p, 2);
    assert_eq!(selected["phase"], "succeeded", "{selected}");
    assert_eq!(p.submit(select()).unwrap(), selected);
    assert_eq!(s.catalog()["sources"].as_array().unwrap().len(), 1);
    assert_eq!(s.operations()["last_seq"], "0");
    assert!(s.current().is_none());
    assert_eq!(
        l.reserve().unwrap_err().kind,
        ErrorKind::Busy,
        "exactly one proposal remains pending"
    );
    let late = p.cancel(2).unwrap();
    assert_eq!(
        late, selected,
        "late cancel reports committed proposal, not cancelled"
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
    p.request_stop();
    drop(p);
}

#[test]
fn changed_selection_and_invalid_files_never_become_proposals() {
    let f = Fixture::new();
    let path = f.0.join("secret.oas");
    fs::write(&path, b"not an oasis file").unwrap();
    let r = Resources::new(Limits::default()).unwrap();
    let s = service(&r);
    let l = Launches::new();
    let p = Picker::start(
        std::slice::from_ref(&f.0),
        &r,
        Arc::clone(&s),
        Arc::clone(&l),
    )
    .unwrap();
    p.submit(request(&p, 1)).unwrap();
    let page = terminal(&p, 1);
    let h = handle(&page, "secret.oas");
    fs::write(&path, b"changed").unwrap();
    p.submit(Request::Select {
        seq: "2".into(),
        handle: h,
    })
    .unwrap();
    assert_eq!(terminal(&p, 2)["error"], "browse_changed");
    p.submit(request(&p, 3)).unwrap();
    let h = handle(&terminal(&p, 3), "secret.oas");
    p.submit(Request::Select {
        seq: "4".into(),
        handle: h,
    })
    .unwrap();
    let rejected = terminal(&p, 4);
    assert_eq!(rejected["phase"], "failed");
    assert!(!rejected.to_string().contains(f.0.to_str().unwrap()));
    assert!(!rejected.to_string().contains("changed"));
    assert_eq!(s.catalog()["sources"], json!([]));
    assert!(l.reserve().is_ok());
}

#[test]
fn cancellation_stop_and_invalid_wire_are_fenced() {
    let f = Fixture::new();
    let r = Resources::new(Limits::default()).unwrap();
    let s = service(&r);
    let l = Launches::new();
    let p = Picker::start(std::slice::from_ref(&f.0), &r, s, l).unwrap();
    let good = json!({"kind":"list","seq":"1","directory":p.roots[0].handle,"filter":"all_files","query":""});
    for (key, value) in [
        ("path", json!("/etc")),
        ("directory", json!("/etc")),
        ("filter", json!("*")),
        ("query", json!("../")),
        ("seq", json!("01")),
    ] {
        let mut v = good.clone();
        v[key] = value;
        if let Ok(req) = serde_json::from_value::<Request>(v) {
            assert!(p.submit(req).is_err());
        }
        assert_eq!(p.snapshot()["last_seq"], "0");
    }
    // Hold the actor gate while installing a pre-cancelled queued operation.
    // Deterministic: no dependency on directory size, scheduler or sleep.
    {
        let mut state = p.inner.state.lock().unwrap();
        let req = request(&p, 1);
        state
            .ledger
            .admit(1, serde_json::to_string(&req).unwrap(), "list")
            .unwrap();
        let stop = Arc::new(AtomicUsize::new(1));
        state.active_stop = Some(Arc::clone(&stop));
        state.pending = Some(Work {
            seq: 1,
            request: req,
            stop,
        });
        p.inner.wake.notify_one();
    }
    assert_eq!(terminal(&p, 1)["phase"], "cancelled");
    assert_eq!(p.cancel(1).unwrap()["phase"], "cancelled");
    p.request_stop();
    assert_eq!(p.submit(request(&p, 2)).unwrap_err(), "closed");
}
