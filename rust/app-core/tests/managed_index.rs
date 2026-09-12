//! Private synthetic input only, invoked with PATH empty by the gate harness.
use floe_app_core::{
    cache,
    index::IndexOptions,
    jobdeck::color::Mode,
    managed::{Limits, ManagedDataset, Resources, Usage},
    managed_index::{ManagedIndex, Phase, Snapshot},
    native::{Discovery, Indexer, INDEX_VERSION},
    registered::{AccessScope, RegisteredSource},
    ErrorKind,
};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{atomic::AtomicUsize, Arc},
    thread,
    time::{Duration, Instant},
};
fn wait(job: &mut ManagedIndex) -> Snapshot {
    let start = Instant::now();
    while !job.is_finished() {
        assert!(
            start.elapsed() < Duration::from_secs(50),
            "{:?}",
            job.snapshot()
        );
        thread::sleep(Duration::from_millis(10));
    }
    let state = job.snapshot();
    job.close().unwrap();
    assert!(state.terminal());
    state
}
fn real() -> Indexer {
    Indexer::discover(&Discovery::local().unwrap()).unwrap()
}
fn options() -> IndexOptions {
    IndexOptions {
        jobs: 2,
        ..Default::default()
    }
}

#[test]
#[ignore = "run tools/validate_managed_index.py with a private fixture"]
fn real_layout_deck_and_bounded_cancellation() {
    let source = PathBuf::from(std::env::var_os("FLOE_MANAGED_FIXTURE").expect("private fixture"));
    let root = source.parent().unwrap();
    let flag = AtomicUsize::new(0);
    let scope = AccessScope::new(&[root.to_owned()]).unwrap();
    let registered = RegisteredSource::register(Arc::clone(&scope), &source, &flag).unwrap();
    let resources = Resources::new(Limits::default()).unwrap();
    let cache_dir = cache::cache_path(&source).unwrap();
    assert!(!cache_dir.exists());
    let read = resources.read([cache_dir.clone()]).unwrap();
    assert!(
        ManagedIndex::start(&resources, Arc::clone(&registered), None, options(), real()).is_err()
    );
    assert!(!cache_dir.exists());
    drop(read);
    let mut job =
        ManagedIndex::start(&resources, Arc::clone(&registered), None, options(), real()).unwrap();
    let built = wait(&mut job);
    assert_eq!(built.phase, Phase::Succeeded, "{built:?}");
    assert_eq!(
        (
            built.completed,
            built.total,
            built.kept,
            built.skipped,
            built.failed
        ),
        (1, 1, 0, 0, 0)
    );
    assert!(built.native.output_bytes > 0);
    assert_eq!(resources.usage(), Usage::default());
    assert_eq!(
        cache::inspect(&source, &cache_dir).unwrap(),
        cache::CacheState::Current
    );
    let pinned = ManagedDataset::open(&resources, &source, None, Mode::Level, &flag).unwrap();
    assert!(ManagedIndex::start(
        &resources,
        Arc::clone(&registered),
        None,
        IndexOptions {
            force: true,
            ..options()
        },
        real()
    )
    .is_err());
    drop(pinned);
    let mut reuse =
        ManagedIndex::start(&resources, Arc::clone(&registered), None, options(), real()).unwrap();
    let reused = wait(&mut reuse);
    assert_eq!(
        (reused.phase, reused.kept, reused.native.output_bytes),
        (Phase::Succeeded, 1, 0)
    );
    let before: Vec<_> = ["design.ovm", "design.ovp", "design.ovt", "meta.json"]
        .into_iter()
        .map(|n| (n, fs::read(cache_dir.join(n)).unwrap()))
        .collect();
    let mut summary = ManagedIndex::start(
        &resources,
        Arc::clone(&registered),
        None,
        IndexOptions {
            occupancy: true,
            ..options()
        },
        real(),
    )
    .unwrap();
    assert_eq!(wait(&mut summary).phase, Phase::Succeeded);
    assert!(cache_dir.join("design.ovo").is_file());
    for (name, bytes) in before {
        assert_eq!(
            fs::read(cache_dir.join(name)).unwrap(),
            bytes,
            "occupancy changed {name}"
        );
    }

    let deck_path = root.join("test.jb");
    fs::write(
        &deck_path,
        format!(
            "MTITLE 1,Mask\nCHIP C\n$ (1,A,TC='{}')\n$ (2,B,TC=missing.oas)\nROWS 0/0\n",
            source.file_name().unwrap().to_str().unwrap()
        ),
    )
    .unwrap();
    let deck = RegisteredSource::register(scope, &deck_path, &flag).unwrap();
    let mut selected = ManagedIndex::start(
        &resources,
        Arc::clone(&deck),
        Some(BTreeSet::from([1])),
        options(),
        real(),
    )
    .unwrap();
    let state = wait(&mut selected);
    assert_eq!(
        (state.phase, state.total, state.kept, state.skipped),
        (Phase::Succeeded, 1, 1, 0)
    );
    let mut partial = ManagedIndex::start(&resources, deck, None, options(), real()).unwrap();
    let state = wait(&mut partial);
    assert_eq!(
        (state.phase, state.total, state.kept, state.skipped),
        (Phase::Incomplete, 2, 1, 1)
    );
    assert_eq!(resources.usage(), Usage::default());

    // No helper children, shell builtins only. SIGSTOP forces the bounded
    // kill/reap fallback; huge pipe output must not block the control thread.
    let fake = root.join("controlled-indexer");
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then printf 'floe-index {}\n'; exit 0; fi
i=0
while [ "$i" -lt 3000 ]; do
    printf '........................................................................................'
    printf '........................................................................................' >&2
    i=$((i+1))
done
printf '\n[vfs] build: pipeline cells 1/9 pages planned=100 encoded=90\n' >&2
printf '%s' "$$" > "$2.started"
kill -STOP "$$"
exit 7
"#,
        INDEX_VERSION
    );
    fs::write(&fake, script).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let native = Indexer::discover(&Discovery {
        override_path: Some(fake),
        development_root: None,
        executable: PathBuf::from("/unused"),
        search_path: None,
    })
    .unwrap();
    let marker = PathBuf::from(format!("{}.started", source.display()));
    let mut cancelled = ManagedIndex::start(
        &resources,
        Arc::clone(&registered),
        None,
        IndexOptions {
            force: true,
            ..options()
        },
        native,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "{:?}", cancelled.snapshot());
        thread::sleep(Duration::from_millis(10));
    }
    assert!(resources.read([cache_dir.clone()]).is_err());
    assert!(
        ManagedIndex::start(&resources, Arc::clone(&registered), None, options(), real()).is_err()
    );
    let pid: i32 = fs::read_to_string(marker).unwrap().parse().unwrap();
    assert!(pid > 0);
    let start = Instant::now();
    cancelled.cancel();
    let state = wait(&mut cancelled);
    assert!(start.elapsed() < Duration::from_secs(4));
    assert_eq!(
        (state.phase, state.failure),
        (Phase::Cancelled, Some(ErrorKind::Cancelled))
    );
    assert!(state.native.output_bytes > 400_000, "{state:?}");
    assert!(state.native.dropped_lines > 0);
    // SAFETY: only checks the known private test child's positive PID.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert_eq!(resources.usage(), Usage::default());
    let read = resources.read([cache_dir]).unwrap();
    drop(read);
    for (version, want) in [
        (INDEX_VERSION, ErrorKind::Worker),
        ("0.0.0", ErrorKind::Version),
    ] {
        let binary = root.join("failing-indexer");
        fs::write(&binary, format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'floe-index {version}\\n'; exit 0; fi\nprintf 'private error /secret/design.oas' >&2\nexit 7\n")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let native = Indexer::discover(&Discovery {
            override_path: Some(binary),
            development_root: None,
            executable: PathBuf::from("/unused"),
            search_path: None,
        })
        .unwrap();
        let mut failed = ManagedIndex::start(
            &resources,
            Arc::clone(&registered),
            None,
            IndexOptions {
                force: true,
                ..options()
            },
            native,
        )
        .unwrap();
        let state = wait(&mut failed);
        assert_eq!((state.phase, state.failure), (Phase::Failed, Some(want)));
        assert!(!format!("{state:?}").contains("/secret/"));
        assert_eq!(resources.usage(), Usage::default());
    }
    // A changed registration fails before native spawn; the old handle cannot
    // silently target newly introduced deck dependencies or a replaced source.
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap()
        .write_all(b"x")
        .unwrap();
    let mut changed = ManagedIndex::start(&resources, registered, None, options(), real()).unwrap();
    let state = wait(&mut changed);
    assert_eq!(
        (state.phase, state.failure, state.native.output_bytes),
        (Phase::Failed, Some(ErrorKind::Cache), 0)
    );
    assert_eq!(resources.usage(), Usage::default());
    println!("RUST MANAGED INDEX: ALL OK (build/reuse/occupancy, deck selection/skips, lease conflicts, bounded pipes, cancel/reap)");
}
