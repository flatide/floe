use super::*;
use serde_json::json;
use std::{
    io::Write,
    os::unix::fs::{symlink, PermissionsExt},
    sync::{atomic::AtomicU64, Arc},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = Path::new("/tmp").join(format!(
            "fi-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    fn endpoint(&self, display: &str) -> Endpoint {
        Endpoint::in_directory(&self.0, &Key::new("floe2-web", Some(display)).unwrap()).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[track_caller]
fn owner(endpoint: &Endpoint) -> Owner {
    match endpoint.claim("build-A").unwrap() {
        Claim::Owner(o) => o,
        _ => panic!("already owned"),
    }
}
fn start(
    endpoint: &Endpoint,
) -> (
    Arc<AtomicUsize>,
    thread::JoinHandle<Result<()>>,
    Arc<AtomicUsize>,
) {
    let owner = owner(endpoint);
    let stop = Arc::new(AtomicUsize::new(0));
    let count = Arc::new(AtomicUsize::new(0));
    let (s, c) = (Arc::clone(&stop), Arc::clone(&count));
    let thread = thread::spawn(move || {
        owner.serve(&s, |body, _| {
            let n = c.fetch_add(1, Ordering::Relaxed) + 1;
            Ok(json!({"count":n,"body":body}))
        })
    });
    (stop, thread, count)
}
fn halt(stop: Arc<AtomicUsize>, thread: thread::JoinHandle<Result<()>>) {
    stop.store(1, Ordering::Relaxed);
    thread.join().unwrap().unwrap();
}
#[test]
fn display_key_is_product_and_screen_scoped_without_requiring_x11() {
    for (a, b) in [
        (":1.0", ":1"),
        ("host:99.8", "host:99"),
        (" :2.0 ", ":2"),
        ("host.name:1.2", "host.name:1"),
    ] {
        assert_eq!(
            Key::new("floe2-web", Some(a)).unwrap().0,
            Key::new("floe2-web", Some(b)).unwrap().0
        );
    }
    assert_ne!(
        Key::new("floe2-web", Some(":1")).unwrap().0,
        Key::new("floe2-web", Some(":2")).unwrap().0
    );
    assert_ne!(
        Key::new("floe2", Some(":1")).unwrap().0,
        Key::new("floe2-web", Some(":1")).unwrap().0
    );
    assert!(Key::new("../bad", None).is_err());
    assert!(Key::new("floe2-web", Some("a\0b")).is_err());
    assert_eq!(
        Key::new("floe2-web", None).unwrap().0,
        Key::new("floe2-web", Some("")).unwrap().0
    );
}
#[test]
fn owner_lock_persists_and_stale_socket_recovery_never_unlinks_foreign_entries() {
    let temp = Temp::new();
    let endpoint = temp.endpoint(":1");
    let first = owner(&endpoint);
    let lock_id = identity(&fs::metadata(&endpoint.lock).unwrap());
    assert!(matches!(
        endpoint.claim("build-A").unwrap(),
        Claim::Running(_)
    ));
    assert_eq!(
        fs::metadata(&endpoint.socket).unwrap().mode() & 0o777,
        0o600
    );
    drop(first);
    assert!(!endpoint.socket.exists());
    let old = Socket::new(Domain::UNIX, Type::STREAM, None).unwrap();
    old.bind(&SockAddr::unix(&endpoint.socket).unwrap())
        .unwrap();
    drop(old);
    let second = owner(&endpoint);
    assert_eq!(identity(&fs::metadata(&endpoint.lock).unwrap()), lock_id);
    drop(second);
    fs::write(&endpoint.socket, b"keep").unwrap();
    assert!(endpoint.claim("build-A").is_err());
    assert_eq!(fs::read(&endpoint.socket).unwrap(), b"keep");
    fs::remove_file(&endpoint.socket).unwrap();
    symlink("missing", &endpoint.socket).unwrap();
    assert!(endpoint.claim("build-A").is_err());
    assert_eq!(
        fs::read_link(&endpoint.socket).unwrap(),
        Path::new("missing")
    );
}
#[test]
fn unsafe_directory_lock_and_unlocked_live_listener_are_preserved() {
    let temp = Temp::new();
    let endpoint = temp.endpoint(":3");
    fs::set_permissions(&endpoint.directory, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(endpoint.claim("build-A").is_err());
    fs::set_permissions(&endpoint.directory, fs::Permissions::from_mode(0o700)).unwrap();
    let target = temp.0.join("target");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, &endpoint.lock).unwrap();
    assert!(endpoint.claim("build-A").is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    fs::remove_file(&endpoint.lock).unwrap();
    fs::hard_link(&target, &endpoint.lock).unwrap();
    assert!(endpoint.claim("build-A").is_err());
    fs::remove_file(&endpoint.lock).unwrap();
    let live = Socket::new(Domain::UNIX, Type::STREAM, None).unwrap();
    live.bind(&SockAddr::unix(&endpoint.socket).unwrap())
        .unwrap();
    live.listen(1).unwrap();
    let id = identity(&fs::symlink_metadata(&endpoint.socket).unwrap());
    assert!(endpoint.claim("build-A").is_err());
    assert_eq!(
        identity(&fs::symlink_metadata(&endpoint.socket).unwrap()),
        id
    );
}
#[test]
fn cleanup_only_removes_the_owned_socket_inode() {
    let temp = Temp::new();
    let endpoint = temp.endpoint(":4");
    let o = owner(&endpoint);
    fs::remove_file(&endpoint.socket).unwrap();
    fs::write(&endpoint.socket, b"replacement").unwrap();
    drop(o);
    assert_eq!(fs::read(&endpoint.socket).unwrap(), b"replacement");
}
#[test]
fn owner_drop_releases_lease_even_while_its_open_description_is_duplicated() {
    let temp = Temp::new();
    let endpoint = temp.endpoint(":lease-dup");
    let first = owner(&endpoint);
    // dup and fork refer to the same flock. This models a concurrent child's
    // short fork-to-exec interval, which FD_CLOEXEC does not cover yet.
    let inherited = first.lock.try_clone().unwrap();
    assert!(matches!(
        endpoint.claim("build-A").unwrap(),
        Claim::Running(_)
    ));
    drop(first);
    let next = owner(&endpoint);
    drop(inherited);
    // Closing the old description must not release the new owner's lease.
    assert!(matches!(
        endpoint.claim("build-A").unwrap(),
        Claim::Running(_)
    ));
    drop(next);
    drop(owner(&endpoint));
}
#[test]
fn foreign_process_drop_does_not_release_the_creators_lease_or_socket() {
    let temp = Temp::new();
    let endpoint = temp.endpoint(":lease-foreign");
    let mut inherited = owner(&endpoint);
    let creators_description = inherited.lock.try_clone().unwrap();
    let socket_id = inherited.socket_id;
    // Exercise the fork-child destructor branch without running Rust cleanup
    // after a multithreaded fork. The current process can never have PID 0.
    inherited.creator_pid = 0;
    drop(inherited);
    assert_eq!(
        identity(&fs::metadata(&endpoint.socket).unwrap()),
        socket_id
    );
    assert!(matches!(
        endpoint.claim("build-A").unwrap(),
        Claim::Running(_)
    ));
    drop(creators_description);
    // Once all descriptors close, the ordinary stale-socket probe can reclaim.
    drop(owner(&endpoint));
}
#[test]
fn wire_lost_ack_and_replays_do_not_repeat_the_handler() {
    let temp = Temp::new();
    let endpoint = temp.endpoint(":5");
    let (stop, thread, count) = start(&endpoint);
    let client = endpoint.connect("build-A", &stop).unwrap();
    let intent = client
        .intent(json!({"path":"한 글.oas","depth":"0"}))
        .unwrap();
    // Deliver the whole request but lose its ACK. Recovery uses the same intent.
    wire::write(&client.socket, &intent.0, Instant::now() + DEADLINE, &stop).unwrap();
    drop(client);
    let reply = endpoint
        .connect("build-A", &stop)
        .unwrap()
        .submit(&intent, &stop)
        .unwrap();
    assert_eq!(
        reply,
        Outcome::Handled {
            reply: json!({"count":1,"body":intent.0.body})
        }
    );
    assert_eq!(
        endpoint
            .connect("build-A", &stop)
            .unwrap()
            .submit(&intent, &stop)
            .unwrap(),
        reply
    );
    let mut conflict = intent.clone();
    conflict.0.body = json!({"path":"different"});
    assert_eq!(
        endpoint
            .connect("build-A", &stop)
            .unwrap()
            .submit(&conflict, &stop)
            .unwrap(),
        Outcome::Rejected {
            code: Reject::Conflict
        }
    );
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert!(matches!(endpoint.connect("build-B",&stop),Err(e) if e.kind==ErrorKind::Version));
    halt(stop, thread);
    let (stop, thread, count) = start(&endpoint);
    assert_eq!(
        endpoint
            .connect("build-A", &stop)
            .unwrap()
            .submit(&intent, &stop)
            .unwrap(),
        Outcome::Rejected {
            code: Reject::OwnerChanged
        }
    );
    assert_eq!(count.load(Ordering::Relaxed), 0);
    halt(stop, thread);
}
#[test]
fn ledger_history_expiry_limits_cancel_and_application_failures_are_at_most_once() {
    let mut ledger = Ledger::default();
    let stop = AtomicUsize::new(0);
    let mut called = 0;
    let mut handle = |_: &Value, _: &AtomicUsize| {
        called += 1;
        Err(Error::input("private path must not leak"))
    };
    let r = |seq: &str, body| Request {
        epoch: "owner".into(),
        seq: seq.into(),
        body,
    };
    for seq in ["0", "01", "+1", "-1", "18446744073709551616", "2"] {
        assert!(matches!(
            ledger
                .dispatch("owner", r(seq, json!({})), &stop, &mut handle)
                .outcome,
            Outcome::Rejected {
                code: Reject::Sequence
            }
        ));
    }
    for n in 1..=40 {
        assert_eq!(ledger.reserve(), Some(n));
        assert_eq!(
            ledger
                .dispatch("owner", r(&n.to_string(), json!({})), &stop, &mut handle)
                .outcome,
            Outcome::Failed {
                code: Failure::InvalidInput
            }
        );
    }
    assert_eq!(ledger.history.len(), HISTORY);
    assert_eq!(ledger.reserve(), Some(41));
    assert_eq!(
        ledger
            .dispatch("owner", r("1", json!({})), &stop, &mut handle)
            .outcome,
        Outcome::Rejected {
            code: Reject::Expired
        }
    );
    assert_eq!(
        ledger
            .dispatch(
                "owner",
                r("41", json!("x".repeat(BODY_BYTES))),
                &stop,
                &mut handle
            )
            .outcome,
        Outcome::Rejected {
            code: Reject::BodyLimit
        }
    );
    stop.store(1, Ordering::Relaxed);
    assert_eq!(
        ledger
            .dispatch("owner", r("41", json!({})), &stop, &mut handle)
            .outcome,
        Outcome::Rejected {
            code: Reject::Closing
        }
    );
    assert_eq!(called, 40);
    stop.store(0, Ordering::Relaxed);
    let outcome = ledger
        .dispatch("owner", r("41", json!({})), &stop, &mut |_, _| {
            Ok(json!("x".repeat(REPLY_BYTES)))
        })
        .outcome;
    assert_eq!(
        outcome,
        Outcome::Failed {
            code: Failure::ReplyLimit
        }
    );
    assert_eq!(
        ledger
            .dispatch("owner", r("41", json!({})), &stop, &mut |_, _| panic!(
                "replayed"
            ))
            .outcome,
        outcome
    );
}
#[test]
fn malformed_frames_fail_without_dispatch_and_shutdown_interrupts_partial_input() {
    let temp = Temp::new();
    let endpoint = temp.endpoint(":6");
    let (stop, thread, count) = start(&endpoint);
    for bytes in [
        vec![255, 255, 255, 255],
        vec![0, 0, 0, 1, 255],
        b"\0\0\0\x02[]".to_vec(),
        b"\0\0\0\x04null".to_vec(),
    ] {
        let client = endpoint.connect("build-A", &stop).unwrap();
        (&client.socket).write_all(&bytes).unwrap();
        drop(client);
    }
    let client = endpoint.connect("build-A", &stop).unwrap();
    (&client.socket).write_all(&[0]).unwrap();
    let before = Instant::now();
    halt(stop, thread);
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(count.load(Ordering::Relaxed), 0);
}
#[test]
fn framing_uses_an_absolute_deadline_not_per_byte_timeouts() {
    let (left, right) = std::os::unix::net::UnixStream::pair().unwrap();
    let left = Socket::from(std::os::fd::OwnedFd::from(left));
    let right = Socket::from(std::os::fd::OwnedFd::from(right));
    left.set_nonblocking(true).unwrap();
    right.set_nonblocking(true).unwrap();
    let stop = AtomicUsize::new(0);
    let writer = thread::spawn(move || {
        for byte in [0, 0, 0, 10, b'{'] {
            if (&right).write_all(&[byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    let before = Instant::now();
    assert!(wire::read::<Value>(&left, before + Duration::from_millis(55), &stop).is_err());
    assert!(before.elapsed() < Duration::from_millis(250));
    drop(left);
    writer.join().unwrap();
}

#[test]
fn forged_or_oversized_receipts_are_uncertain_not_success() {
    for bad in 0..3 {
        let (left, right) = std::os::unix::net::UnixStream::pair().unwrap();
        let left = Socket::from(std::os::fd::OwnedFd::from(left));
        let right = Socket::from(std::os::fd::OwnedFd::from(right));
        left.set_nonblocking(true).unwrap();
        right.set_nonblocking(true).unwrap();
        let epoch = "a".repeat(64);
        let client = Connection {
            socket: left,
            hello: Hello {
                protocol: PROTOCOL,
                epoch: epoch.clone(),
                build: "build-A".into(),
                next: "1".into(),
            },
        };
        let stop = AtomicUsize::new(0);
        let intent = client.intent(json!({"open":1})).unwrap();
        let peer = thread::spawn(move || {
            let stop = AtomicUsize::new(0);
            let request: Request = wire::read(&right, Instant::now() + DEADLINE, &stop).unwrap();
            assert_eq!(request.seq, "1");
            let reply = match bad {
                0 => json!({"epoch":epoch,"seq":"2","outcome":{"status":"handled","reply":{}}}),
                1 => {
                    json!({"epoch":epoch,"seq":"1","outcome":{"status":"handled","reply":"x".repeat(REPLY_BYTES)}})
                }
                _ => {
                    json!({"epoch":epoch,"seq":"1","outcome":{"status":"failed","code":"private path"}})
                }
            };
            wire::write(&right, &reply, Instant::now() + DEADLINE, &stop).unwrap();
        });
        assert!(matches!(client.submit(&intent,&stop),Err(e) if e.kind==ErrorKind::Incomplete));
        peer.join().unwrap();
    }
}

#[test]
fn disconnected_ticket_is_not_reused_for_a_new_identical_action() {
    let temp = Temp::new();
    let endpoint = temp.endpoint(":7");
    let (stop, thread, count) = start(&endpoint);
    let first = endpoint.connect("build-A", &stop).unwrap();
    let old = first.intent(json!({"pan":0.5})).unwrap();
    drop(first);
    let second = endpoint.connect("build-A", &stop).unwrap();
    let new = second.intent(json!({"pan":0.5})).unwrap();
    assert_ne!(old.0.seq, new.0.seq);
    assert!(
        matches!(second.submit(&new,&stop).unwrap(),Outcome::Handled{reply} if reply["count"]==1)
    );
    assert!(
        matches!(endpoint.connect("build-A",&stop).unwrap().submit(&old,&stop).unwrap(),Outcome::Handled{reply} if reply["count"]==2)
    );
    assert!(
        matches!(endpoint.connect("build-A",&stop).unwrap().submit(&old,&stop).unwrap(),Outcome::Handled{reply} if reply["count"]==2)
    );
    assert_eq!(count.load(Ordering::Relaxed), 2);
    halt(stop, thread);
}

#[test]
#[ignore = "run tools/validate_instance_key.py for the GTK source oracle"]
fn gtk_display_key_oracle() {
    let file = std::env::var_os("FLOE_INSTANCE_ORACLE").expect("GTK source oracle");
    let cases: Vec<(String, String)> = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
    assert!(cases.len() >= 128);
    for (display, want) in &cases {
        assert_eq!(&normalize_display(display), want, "{display:?}");
    }
    println!(
        "GTK INSTANCE KEY: ALL OK ({} display normalization cases)",
        cases.len()
    );
}
