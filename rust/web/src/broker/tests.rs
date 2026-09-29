use super::*;
use floe_app_core::server::{Config, IndexPolicy};
use std::os::unix::fs::symlink;

const PROXY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const DELEGATOR: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const ORIGIN: &str = "https://floe.example.test";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "floe-broker-unit-{}",
            crate::auth::public_id().unwrap()
        ));
        fs::create_dir(&path).unwrap();
        let path = fs::canonicalize(path).unwrap();
        for name in ["data", "work", "runtime", "outside"] {
            fs::create_dir(path.join(name)).unwrap();
        }
        fs::write(path.join("data/sample.oas"), b"synthetic metadata only").unwrap();
        fs::write(path.join("outside/secret.oas"), b"synthetic out-of-scope").unwrap();
        Self(path)
    }
    fn config(&self, max_sessions: u16) -> Config {
        Config {
            version: 1,
            public_origin: ORIGIN.into(),
            runtime_root: self.0.join("runtime"),
            max_sessions,
            deployment: Deployment::TeeBox {
                client_id: "teebox-service".into(),
                user_namespace: "teebox".into(),
                shared_root: self.0.join("data"),
                work_root: self.0.join("work"),
                index: IndexPolicy {
                    max_running: 1,
                    max_entries: 32,
                    jobs: 8,
                },
            },
        }
    }
    fn broker(&self, sessions: u16) -> Broker {
        Broker::new(
            "127.0.0.1:49000".parse().unwrap(),
            self.config(sessions).validate().unwrap(),
            PROXY,
            DELEGATOR,
            Lifetimes {
                bootstrap: Duration::from_secs(2),
                session: Duration::from_secs(5),
            },
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn boundary() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("host", "floe.example.test".parse().unwrap());
    h.insert("origin", ORIGIN.parse().unwrap());
    h.insert("x-floe-proxy-key", PROXY.parse().unwrap());
    h
}
fn delegated() -> HeaderMap {
    let mut h = boundary();
    h.insert("x-floe-service-client", "teebox-service".parse().unwrap());
    h.insert("x-floe-delegation-key", DELEGATOR.parse().unwrap());
    h
}
fn request(user: &str) -> LaunchRequest {
    LaunchRequest {
        user_id: user.into(),
        source: "sample.oas".into(),
    }
}
fn session(b: &Broker, user: &str, now: Instant) -> (Launch, HeaderMap, Access) {
    let launch = b.launch(&delegated(), request(user), now).unwrap();
    let credentials = b
        .exchange(&launch.id, &launch.bootstrap.expose(), &boundary(), now)
        .unwrap();
    let mut h = boundary();
    h.insert(
        "cookie",
        format!(
            "{}={}",
            cookie_name(&launch.id).unwrap(),
            credentials.cookie.expose()
        )
        .parse()
        .unwrap(),
    );
    h.insert("x-floe-csrf", credentials.csrf.expose().parse().unwrap());
    let access = b.authorize(&launch.id, &h, now).unwrap();
    (launch, h, access)
}

#[test]
fn proxy_identity_and_independent_delegation_proof_are_all_required() {
    let f = Fixture::new();
    let b = f.broker(4);
    let now = Instant::now();
    for field in [
        "x-floe-delegation-key",
        "x-floe-proxy-key",
        "x-floe-service-client",
        "host",
    ] {
        let mut headers = delegated();
        headers.remove(field);
        assert!(matches!(
            b.launch(&headers, request("alice"), now),
            Err(Error::Unauthorized)
        ));
        let mut headers = delegated();
        headers.append(field, "forged".parse().unwrap());
        assert!(matches!(
            b.launch(&headers, request("alice"), now),
            Err(Error::Unauthorized)
        ));
    }
    for (field, value) in [
        ("x-floe-service-client", "alice"),
        ("host", "evil.test"),
        ("origin", "https://evil.test"),
        ("x-floe-delegation-key", PROXY),
    ] {
        let mut h = delegated();
        h.insert(field, value.parse().unwrap());
        assert!(matches!(
            b.launch(&h, request("alice"), now),
            Err(Error::Unauthorized)
        ));
    }
    let mut h = delegated();
    h.remove("origin"); // trusted server-to-server client
    let l = b.launch(&h, request("actual-user"), now).unwrap();
    assert!(!format!("{l:?}").contains(&l.bootstrap.expose()));
    assert_eq!(
        b.state.lock().unwrap().entries[&l.id]
            .binding
            .principal
            .subject(),
        "actual-user"
    );
}
#[test]
fn sessions_bind_principal_source_and_credentials_independently() {
    let f = Fixture::new();
    let b = f.broker(4);
    let now = Instant::now();
    let (a, ha, aa) = session(&b, "alice", now);
    let (c, hc, ac) = session(&b, "bob", now);
    assert_ne!(a.id, c.id);
    assert_ne!(a.bootstrap.expose(), c.bootstrap.expose());
    assert_eq!(aa.binding().source(), ac.binding().source()); // shared raw source, not copied
    assert_ne!(aa.binding().principal(), ac.binding().principal());
    assert!(b.authorize(&a.id, &hc, now).is_err());
    assert!(b.authorize(&c.id, &ha, now).is_err());
    let mut mixed = ha.clone();
    mixed.insert("x-floe-csrf", hc["x-floe-csrf"].clone());
    assert!(b.authorize(&a.id, &mixed, now).is_err());
    let mut duplicate = ha.clone();
    duplicate.append("cookie", ha["cookie"].clone());
    assert!(b.authorize(&a.id, &duplicate, now).is_err());
    assert!(b.launch(&ha, request("bob"), now).is_err());
    assert!(b.revoke(&c.id, &ha, now).is_err());
    assert!(b
        .exchange(&a.id, &a.bootstrap.expose(), &boundary(), now)
        .is_err());
}
#[test]
fn exchange_is_atomic_one_use_wrong_target_and_invalid_origin_do_not_consume_it() {
    let f = Fixture::new();
    let b = Arc::new(f.broker(4));
    let now = Instant::now();
    let a = b.launch(&delegated(), request("alice"), now).unwrap();
    let c = b.launch(&delegated(), request("bob"), now).unwrap();
    assert!(b
        .exchange(&a.id, &c.bootstrap.expose(), &boundary(), now)
        .is_err());
    let mut h = boundary();
    h.remove("origin");
    assert!(b.exchange(&a.id, &a.bootstrap.expose(), &h, now).is_err());
    let token = a.bootstrap.expose();
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let b = Arc::clone(&b);
            let token = token.clone();
            let id = a.id.clone();
            std::thread::spawn(move || b.exchange(&id, &token, &boundary(), now).is_ok())
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| usize::from(t.join().unwrap()))
            .sum::<usize>(),
        1
    );
    assert!(b
        .exchange(&c.id, &c.bootstrap.expose(), &boundary(), now)
        .is_ok());
}
#[test]
fn logout_expiry_and_worker_reap_preserve_other_users_and_capacity() {
    let f = Fixture::new();
    let b = f.broker(2);
    let now = Instant::now();
    let (a, ha, aa) = session(&b, "alice", now);
    let (c, hc, _) = session(&b, "bob", now);
    let worker = b.claim_worker(&aa, now).unwrap();
    assert!(matches!(b.claim_worker(&aa, now), Err(Error::Busy)));
    assert!(!*worker.cancellation().borrow());
    b.logout(&a.id, &ha, now).unwrap();
    assert!(*worker.cancellation().borrow());
    assert!(b.authorize(&a.id, &ha, now).is_err());
    assert!(b.authorize(&c.id, &hc, now).is_ok());
    assert!(matches!(
        b.launch(&delegated(), request("carol"), now),
        Err(Error::Busy)
    ));
    assert!(f.broker(2).worker_reaped(&worker).is_err());
    b.worker_reaped(&worker).unwrap();
    assert!(b.worker_reaped(&worker).is_err());
    assert!(b.launch(&delegated(), request("carol"), now).is_ok());
    b.maintain(now + Duration::from_secs(5)).unwrap();
    assert!(b
        .authorize(&c.id, &hc, now + Duration::from_secs(5))
        .is_err());
    assert_eq!(b.lock().unwrap().entries.len(), 0);
}
#[test]
fn expired_claimed_session_is_cancelled_but_not_freed_until_reap() {
    let f = Fixture::new();
    let b = f.broker(1);
    let now = Instant::now();
    let (_, _, access) = session(&b, "alice", now);
    let worker = b.claim_worker(&access, now).unwrap();
    let stop = worker.cancellation();
    drop(worker);
    b.maintain(now + Duration::from_secs(5)).unwrap();
    assert!(*stop.borrow());
    assert_eq!(b.pending_workers().unwrap(), 1);
    assert!(matches!(
        b.launch(&delegated(), request("bob"), now + Duration::from_secs(6)),
        Err(Error::Busy)
    ));
    b.stop().unwrap();
    assert!(matches!(
        b.launch(&delegated(), request("bob"), now),
        Err(Error::Unavailable)
    ));
}

#[test]
fn dropping_broker_signals_claimed_worker_and_never_reuses_its_credentials() {
    let f = Fixture::new();
    let b = f.broker(1);
    let now = Instant::now();
    let (launch, headers, access) = session(&b, "alice", now);
    let worker = b.claim_worker(&access, now).unwrap();
    let cancel = worker.cancellation();
    drop(b);
    assert!(*cancel.borrow());
    let replacement = f.broker(1);
    assert!(replacement.authorize(&launch.id, &headers, now).is_err());
    assert!(replacement.worker_reaped(&worker).is_err());
}
#[test]
fn scope_stamp_and_alias_are_checked_before_worker_claim() {
    let f = Fixture::new();
    let b = f.broker(4);
    let now = Instant::now();
    symlink(f.0.join("outside"), f.0.join("data/out")).unwrap();
    for source in [
        "../outside/secret.oas",
        "/etc/passwd",
        "out/secret.oas",
        "missing.oas",
    ] {
        assert!(matches!(
            b.launch(
                &delegated(),
                LaunchRequest {
                    user_id: "alice".into(),
                    source: source.into()
                },
                now
            ),
            Err(Error::Invalid)
        ));
    }
    symlink(f.0.join("data/sample.oas"), f.0.join("data/alias.oas")).unwrap();
    let a = b
        .launch(
            &delegated(),
            LaunchRequest {
                user_id: "alice".into(),
                source: "alias.oas".into(),
            },
            now,
        )
        .unwrap();
    assert_eq!(
        b.lock().unwrap().entries[&a.id].binding.source,
        f.0.join("data/sample.oas")
    );
    let (_, _, access) = session(&b, "bob", now);
    fs::write(f.0.join("data/sample.oas"), b"changed metadata fixture").unwrap();
    assert!(matches!(b.claim_worker(&access, now), Err(Error::Invalid)));
}
#[test]
fn configured_capacity_is_atomic_under_concurrent_launches() {
    let f = Fixture::new();
    let b = Arc::new(f.broker(4));
    let now = Instant::now();
    let threads: Vec<_> = (0..24)
        .map(|n| {
            let b = Arc::clone(&b);
            std::thread::spawn(move || b.launch(&delegated(), request(&format!("user-{n}")), now))
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 4);
    assert!(results
        .iter()
        .filter(|r| r.is_err())
        .all(|r| matches!(r, Err(Error::Busy))));
}
#[test]
fn unredeemed_launch_expiry_revoke_and_invalid_construction_fail_closed() {
    let f = Fixture::new();
    let b = f.broker(1);
    let now = Instant::now();
    let a = b.launch(&delegated(), request("alice"), now).unwrap();
    assert!(b
        .exchange(
            &a.id,
            &a.bootstrap.expose(),
            &boundary(),
            now + Duration::from_secs(2)
        )
        .is_err());
    let c = b
        .launch(&delegated(), request("bob"), now + Duration::from_secs(2))
        .unwrap();
    b.revoke(&c.id, &delegated(), now + Duration::from_secs(2))
        .unwrap();
    assert!(b
        .exchange(
            &c.id,
            &c.bootstrap.expose(),
            &boundary(),
            now + Duration::from_secs(2)
        )
        .is_err());
    assert!(b
        .claim_worker(&session(&f.broker(2), "other", now).2, now)
        .is_err());
    for (addr, p, d) in [
        ("0.0.0.0:9000", PROXY, DELEGATOR),
        ("127.0.0.1:0", PROXY, DELEGATOR),
        ("127.0.0.1:9000", PROXY, PROXY),
        ("127.0.0.1:9000", "bad", DELEGATOR),
    ] {
        assert!(Broker::new(
            addr.parse().unwrap(),
            f.config(4).validate().unwrap(),
            p,
            d,
            Lifetimes::default()
        )
        .is_err());
    }
    let mut c = f.config(4);
    c.deployment = Deployment::PublicDemo {
        data_root: f.0.join("data"),
        samples: vec![floe_app_core::server::Sample {
            id: "demo".into(),
            source: "sample.oas".into(),
        }],
    };
    assert!(Broker::new(
        "127.0.0.1:9000".parse().unwrap(),
        c.validate().unwrap(),
        PROXY,
        DELEGATOR,
        Lifetimes::default()
    )
    .is_err());
}

fn render_options() -> floe_app_core::render::RenderOptions {
    floe_app_core::render::RenderOptions {
        binary: PathBuf::from("/synthetic-never-executed/floe-renderd"),
        budget_mb: 64,
        decode_jobs: 1,
        raster_jobs: 1,
        tile_px: 128,
        round_pages: 1024,
        open_timeout_s: 10,
        label_font_px: 14,
        raw: true,
        debug: false,
    }
}

#[test]
fn render_reservation_precedes_preparation_and_drop_releases_it() {
    use floe_app_core::{
        managed::{Limits, Resources, Usage},
        view::{ControllerOptions, ViewController},
    };
    let r = Resources::new(Limits {
        workers: 1,
        decoded_mb: 64,
        ..Default::default()
    })
    .unwrap();
    let pending =
        ViewController::reserve(&r, render_options(), ControllerOptions::default()).unwrap();
    assert_eq!(
        r.usage(),
        Usage {
            cpu_slots: 2,
            workers: 1,
            decoded_mb: 64,
            index_jobs: 0
        }
    );
    assert!(ViewController::reserve(&r, render_options(), ControllerOptions::default()).is_err());
    drop(pending);
    assert_eq!(r.usage(), Usage::default());
}

#[test]
fn runtime_rejects_foreign_and_logged_out_access_without_reserving_or_reading() {
    use floe_app_core::managed::{Limits, Resources, Usage};
    let f = Fixture::new();
    let b = Arc::new(f.broker(4));
    let r = Resources::new(Limits::default()).unwrap();
    let mut runtime =
        runtime::Runtime::new(Arc::clone(&b), Arc::clone(&r), render_options()).unwrap();
    let (_, _, foreign) = session(&f.broker(4), "other", Instant::now());
    assert_eq!(runtime.open(&foreign, 64, 64), Err(Error::Unauthorized));
    let (l, h, a) = session(&b, "alice", Instant::now());
    assert_eq!(runtime.open(&a, 0, 64), Err(Error::Invalid));
    assert_eq!(r.usage(), Usage::default());
    b.logout(&l.id, &h, Instant::now()).unwrap();
    assert_eq!(runtime.open(&a, 64, 64), Err(Error::Unauthorized));
    assert!(matches!(runtime.snapshot(&a), Err(Error::Unauthorized)));
    runtime.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
    assert_eq!(b.pending_workers().unwrap(), 0);
}

#[test]
fn logout_and_expiry_interrupt_only_the_matching_prepare_flag() {
    let f = Fixture::new();
    let b = f.broker(4);
    let now = Instant::now();
    let (a, ha, aa) = session(&b, "alice", now);
    let (_, _, bb) = session(&b, "bob", now);
    let la = b.claim_worker(&aa, now).unwrap();
    let lb = b.claim_worker(&bb, now).unwrap();
    assert_eq!(la.stop.load(Ordering::Relaxed), 0);
    b.logout(&a.id, &ha, now).unwrap();
    assert_eq!(la.stop.load(Ordering::Relaxed), 1);
    assert_eq!(lb.stop.load(Ordering::Relaxed), 0);
    b.maintain(now + Duration::from_secs(5)).unwrap();
    assert_eq!(lb.stop.load(Ordering::Relaxed), 1);
    assert_eq!(b.pending_workers().unwrap(), 2);
    b.worker_reaped(&la).unwrap();
    b.worker_reaped(&lb).unwrap();
}

#[test]
fn runtime_admission_failure_leaves_session_retryable_and_has_no_claim() {
    use floe_app_core::{
        managed::{Limits, Resources, Usage},
        view::{ControllerOptions, ViewController},
    };
    let f = Fixture::new();
    let b = Arc::new(f.broker(4));
    let r = Resources::new(Limits {
        workers: 1,
        decoded_mb: 64,
        ..Default::default()
    })
    .unwrap();
    let mut runtime =
        runtime::Runtime::new(Arc::clone(&b), Arc::clone(&r), render_options()).unwrap();
    let (l, h, a) = session(&b, "alice", Instant::now());
    let other =
        ViewController::reserve(&r, render_options(), ControllerOptions::default()).unwrap();
    assert_eq!(runtime.open(&a, 64, 64), Err(Error::Busy));
    assert!(b.authorize(&l.id, &h, Instant::now()).is_ok());
    assert_eq!(b.pending_workers().unwrap(), 0);
    drop(other);
    runtime.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
}

#[test]
fn changed_source_before_async_claim_closes_session_without_child() {
    use floe_app_core::managed::{Limits, Resources, Usage};
    let f = Fixture::new();
    let b = Arc::new(f.broker(4));
    let r = Resources::new(Limits::default()).unwrap();
    let mut runtime =
        runtime::Runtime::new(Arc::clone(&b), Arc::clone(&r), render_options()).unwrap();
    let (l, h, a) = session(&b, "alice", Instant::now());
    fs::write(
        f.0.join("data/sample.oas"),
        b"changed synthetic bytes before claim",
    )
    .unwrap();
    runtime.open(&a, 64, 64).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while b.authorize(&l.id, &h, Instant::now()).is_ok() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    runtime.close().unwrap();
    assert_eq!(r.usage(), Usage::default());
    assert_eq!(b.pending_workers().unwrap(), 0);
}
