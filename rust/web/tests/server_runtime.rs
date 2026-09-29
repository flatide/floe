//! Synthetic native per-session workers. No TLS/public listener/browser or
//! real user/design data. Invoked by validate_rust.sh --only server_runtime.
use axum::http::HeaderMap;
use floe_app_core::{
    cache::revision::set,
    index::IndexOptions,
    managed::{Limits, Resources, Usage},
    managed_index::{ManagedIndex, Phase},
    native::{Discovery, Indexer},
    registered::{AccessScope, RegisteredSource},
    render::RenderOptions,
    server::Config,
    view::{Navigation, Patch},
};
use floe_oasis::{
    doc::{RectRec, Rep},
    write::write_cell,
};
use floe_web::broker::{runtime::Runtime, Access, Broker, Error, LaunchRequest, Lifetimes};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant},
};
const PROXY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const DELEGATE: &str = "2222222222222222222222222222222222222222222222222222222222222222";
fn headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in [
        ("host", "service.example.test"),
        ("origin", "https://service.example.test"),
        ("x-floe-proxy-key", PROXY),
        ("x-floe-delegation-key", DELEGATE),
        ("x-floe-service-client", "teebox"),
    ] {
        h.insert(k, v.parse().unwrap());
    }
    h
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).unwrap();
        let p = std::env::temp_dir().join(format!(
            "floe-server-native-{:x}",
            u128::from_ne_bytes(bytes)
        ));
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        let p = fs::canonicalize(p).unwrap();
        for name in ["data", "work", "runtime", "outside"] {
            fs::create_dir(p.join(name)).unwrap();
        }
        let bytes = write_cell(
            "TOP",
            1000.,
            &mut [RectRec {
                layer: 7,
                dt: 0,
                x: 0,
                y: 0,
                w: 10000,
                h: 6000,
                rep: Rep::One,
            }],
            &mut [],
        )
        .unwrap();
        for name in ["a.oas", "b.oas", "missing.oas"] {
            fs::write(p.join("data").join(name), &bytes).unwrap();
        }
        fs::write(p.join("work/personal-sentinel"), b"do not change").unwrap();
        Self(p)
    }
    fn broker(&self) -> Arc<Broker> {
        self.broker_at("127.0.0.1:49000".parse().unwrap(), Lifetimes::default())
    }
    fn broker_at(&self, addr: std::net::SocketAddr, lifetimes: Lifetimes) -> Arc<Broker> {
        let c: Config = serde_json::from_value(json!({"version":1,"public_origin":"https://service.example.test","runtime_root":self.0.join("runtime"),"max_sessions":4,
            "deployment":{"mode":"teebox","client_id":"teebox","user_namespace":"teebox-users","shared_root":self.0.join("data"),"work_root":self.0.join("work"),"index":{"max_running":1,"max_entries":32,"jobs":1}}})).unwrap();
        Arc::new(Broker::new(addr, c.validate().unwrap(), PROXY, DELEGATE, lifetimes).unwrap())
    }
    fn index(&self, resources: &Arc<Resources>, name: &str) -> ManagedIndex {
        let scope = AccessScope::new(&[self.0.join("data")]).unwrap();
        let source = RegisteredSource::register(
            scope,
            &self.0.join("data").join(name),
            &AtomicUsize::new(0),
        )
        .unwrap();
        let indexer = Indexer::discover(&Discovery {
            override_path: Some(binary()),
            development_root: None,
            executable: std::env::current_exe().unwrap(),
            search_path: None,
        })
        .unwrap();
        ManagedIndex::start_revisions(
            resources,
            source,
            None,
            IndexOptions {
                jobs: 1,
                ..Default::default()
            },
            indexer,
        )
        .unwrap()
    }
}

#[path = "support/server_stream.rs"]
mod stream;
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn binary() -> PathBuf {
    PathBuf::from(
        std::env::var_os("FLOE_INDEX_BIN").expect("synthetic native gate requires FLOE_INDEX_BIN"),
    )
}
fn options() -> RenderOptions {
    RenderOptions {
        binary: binary().with_file_name("floe-renderd"),
        budget_mb: 64,
        decode_jobs: 1,
        raster_jobs: 1,
        tile_px: 128,
        round_pages: 1024,
        open_timeout_s: 30,
        label_font_px: 14,
        raw: true,
        debug: false,
    }
}
fn resources() -> Arc<Resources> {
    Resources::new(Limits {
        workers: 2,
        decoded_mb: 128,
        ..Default::default()
    })
    .unwrap()
}
fn until(mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !f() {
        assert!(
            Instant::now() < deadline,
            "native broker condition timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn indexed(mut job: ManagedIndex) {
    until(|| job.is_finished());
    job.close().unwrap();
    assert_eq!(
        job.snapshot().phase,
        Phase::Succeeded,
        "{:?}",
        job.snapshot()
    );
}
fn login(b: &Broker, user: &str, source: &str) -> (HeaderMap, Access) {
    let l = b
        .launch(
            &headers(),
            LaunchRequest {
                user_id: user.into(),
                source: source.into(),
            },
            Instant::now(),
        )
        .unwrap();
    let c = b
        .exchange(&l.id, &l.bootstrap.expose(), &headers(), Instant::now())
        .unwrap();
    let mut h = headers();
    h.remove("x-floe-delegation-key");
    h.remove("x-floe-service-client");
    h.insert(
        "cookie",
        format!("__Secure-floe_server_{}={}", l.id, c.cookie.expose())
            .parse()
            .unwrap(),
    );
    h.insert("x-floe-csrf", c.csrf.expose().parse().unwrap());
    let a = b.authorize(&l.id, &h, Instant::now()).unwrap();
    (h, a)
}
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, p: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(p).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(
                    p.strip_prefix(root).unwrap().to_owned(),
                    fs::read(&p).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
#[ignore = "synthetic native server_runtime gate"]
fn two_users_share_index_bytes_not_view_state_or_cancellation() {
    let f = Fixture::new();
    let r = resources();
    indexed(f.index(&r, "a.oas"));
    let data_before = files(&f.0.join("data"));
    let b = f.broker();
    let mut runtime = Runtime::new(Arc::clone(&b), Arc::clone(&r), options()).unwrap();
    assert!(matches!(
        Runtime::new(Arc::clone(&b), Arc::clone(&r), options()),
        Err(Error::Busy)
    ));
    let (ha, a) = login(&b, "alice", "a.oas");
    let (_hb, c) = login(&b, "bob", "a.oas");
    let (_, third) = login(&b, "charlie", "a.oas");
    runtime.open(&a, 96, 64).unwrap();
    runtime.open(&c, 96, 64).unwrap();
    assert_eq!(runtime.open(&a, 96, 64), Err(Error::Busy));
    assert_eq!(runtime.open(&third, 96, 64), Err(Error::Busy));
    until(|| runtime.latest(&a).unwrap().is_some() && runtime.latest(&c).unwrap().is_some());
    let af = runtime.latest(&a).unwrap().unwrap();
    let bf = runtime.latest(&c).unwrap().unwrap();
    assert_eq!(af.frame.bytes, bf.frame.bytes);
    assert_ne!(af.worker_epoch, bf.worker_epoch);
    assert_eq!(
        r.usage(),
        Usage {
            cpu_slots: 4,
            workers: 2,
            decoded_mb: 128,
            index_jobs: 0
        }
    );
    let original = runtime.snapshot(&c).unwrap().unwrap();
    let s = runtime.snapshot(&a).unwrap().unwrap();
    runtime
        .edit(
            &a,
            s.state_rev,
            Patch {
                navigation: Some(Navigation::Pan {
                    x: 0.5,
                    y: 0.,
                    snap: false,
                }),
                ..Default::default()
            },
        )
        .unwrap();
    until(|| {
        runtime
            .latest(&a)
            .unwrap()
            .is_some_and(|f| f.render_rev > s.render_rev)
    });
    assert_eq!(
        runtime.snapshot(&c).unwrap().unwrap().state.viewport,
        original.state.viewport
    );
    assert_eq!(
        runtime.latest(&c).unwrap().unwrap().frame.bytes,
        bf.frame.bytes
    );
    assert_eq!(
        files(&f.0.join("data")),
        data_before,
        "open/pan must not mutate index bytes"
    );
    // The separately owned shared index job uses the SAME resources but never
    // the user's cancellation flag. User A and later the broker may close.
    let index = f.index(&r, "b.oas");
    b.logout(a.id(), &ha, Instant::now()).unwrap();
    assert!(matches!(
        runtime.edit(&a, s.state_rev, Patch::default()),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(runtime.latest(&a), Err(Error::Unauthorized)));
    until(|| {
        runtime.maintain().unwrap();
        r.usage().workers == 1
    });
    assert!(runtime.latest(&c).unwrap().is_some());
    runtime.open(&third, 96, 64).unwrap();
    until(|| runtime.latest(&third).unwrap().is_some());
    runtime.close().unwrap();
    indexed(index);
    assert_eq!(r.usage(), Usage::default());
    assert_eq!(b.pending_workers().unwrap(), 0);
    assert_eq!(
        fs::read(f.0.join("work/personal-sentinel")).unwrap(),
        b"do not change"
    );
    assert_eq!(fs::read_dir(f.0.join("runtime")).unwrap().count(), 0);
}

#[test]
#[ignore = "synthetic native server_runtime gate"]
fn missing_revision_bad_native_and_drop_release_only_after_cleanup() {
    let f = Fixture::new();
    let r = resources();
    let b = f.broker();
    let before = files(&f.0);
    let mut runtime = Runtime::new(Arc::clone(&b), Arc::clone(&r), options()).unwrap();
    let (h, a) = login(&b, "alice", "missing.oas");
    runtime.open(&a, 96, 64).unwrap();
    until(|| b.authorize(a.id(), &h, Instant::now()).is_err());
    runtime.close().unwrap();
    assert_eq!(
        files(&f.0),
        before,
        "opening missing revisions must not index or migrate"
    );
    assert_eq!(r.usage(), Usage::default());
    indexed(f.index(&r, "a.oas"));
    let b = f.broker();
    let (_, a) = login(&b, "alice", "a.oas");
    let mut bad = options();
    bad.binary = f.0.join("absent-renderd");
    let mut runtime = Runtime::new(Arc::clone(&b), Arc::clone(&r), bad).unwrap();
    runtime.open(&a, 96, 64).unwrap();
    until(|| r.usage().workers == 0);
    runtime.close().unwrap();
    assert_eq!(b.pending_workers().unwrap(), 0);
    let b = f.broker();
    let (_, a) = login(&b, "alice", "a.oas");
    let runtime = Runtime::new(Arc::clone(&b), Arc::clone(&r), options()).unwrap();
    runtime.open(&a, 96, 64).unwrap();
    until(|| runtime.latest(&a).unwrap().is_some());
    drop(runtime);
    assert_eq!(b.pending_workers().unwrap(), 0);
    assert_eq!(r.usage(), Usage::default());
}

#[test]
#[ignore = "synthetic native server_runtime gate"]
fn hidden_deck_dependency_escape_is_rejected_before_native_open() {
    let f = Fixture::new();
    let r = resources();
    fs::write(
        f.0.join("data/escape.jb"),
        "CHIP A\n$ (1,A,TC=a.oas)\n$ (2,B,TC=../outside/secret.oas)\n",
    )
    .unwrap();
    let before = files(&f.0);
    let b = f.broker();
    let (h, a) = login(&b, "alice", "escape.jb");
    let mut runtime = Runtime::new(Arc::clone(&b), Arc::clone(&r), options()).unwrap();
    runtime.open(&a, 96, 64).unwrap();
    until(|| b.authorize(a.id(), &h, Instant::now()).is_err());
    runtime.close().unwrap();
    assert_eq!(files(&f.0), before);
    assert_eq!(r.usage(), Usage::default());
    assert_eq!(b.pending_workers().unwrap(), 0);
}

#[test]
#[ignore = "synthetic native server_runtime gate"]
fn jobdeck_keeps_old_revision_readers_until_session_native_cleanup() {
    let f = Fixture::new();
    let r = resources();
    let deck = f.0.join("data/mask.jb");
    fs::write(&deck,"CHIP C\n$ (1,A,TC=a.oas,AD=0.001,LY={7},DT={0},UX=100,UY=100)\n$ (2,B,TC=b.oas,AD=0.001,LY={7},DT={0},UX=100,UY=100)\nROWS 0/0\n").unwrap();
    indexed(f.index(&r, "mask.jb"));
    let paths = BTreeSet::from([f.0.join("data/a.oas"), f.0.join("data/b.oas")]);
    let store = set::Store::new(&deck).unwrap();
    let pin = store
        .pin(&paths, &None, &AtomicUsize::new(0))
        .unwrap()
        .unwrap();
    let old_id = pin.id().to_owned();
    let member = pin.members().values().next().unwrap().directory();
    drop(pin);
    let b = f.broker();
    let (_, a) = login(&b, "alice", "mask.jb");
    let mut runtime = Runtime::new(Arc::clone(&b), Arc::clone(&r), options()).unwrap();
    runtime.open(&a, 96, 64).unwrap();
    until(|| runtime.latest(&a).unwrap().is_some());
    assert!(!runtime.snapshot(&a).unwrap().unwrap().state.labels);
    let before = runtime.latest(&a).unwrap().unwrap();
    let probe = fs::File::open(&member).unwrap();
    assert!(matches!(
        probe.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    indexed(f.index(&r, "mask.jb"));
    let next = store
        .pin(&paths, &None, &AtomicUsize::new(0))
        .unwrap()
        .unwrap();
    assert_ne!(old_id, next.id());
    drop(next);
    assert_eq!(
        runtime.latest(&a).unwrap().unwrap().worker_epoch,
        before.worker_epoch
    );
    assert!(
        matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        "publishing a new set must not unpin an open view"
    );
    runtime.close().unwrap();
    probe.try_lock().unwrap();
    assert!(member.is_dir(), "closing readers must not reclaim files");
    assert_eq!(r.usage(), Usage::default());
    assert_eq!(b.pending_workers().unwrap(), 0);
}
