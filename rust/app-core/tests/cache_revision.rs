//! Small actual-native index lifecycle; no browser or field data.
use floe_app_core::{
    cache::{self, revision::Store},
    index::{revision::Build, IndexOptions},
    managed::{Limits, Resources, Usage},
    native::{Discovery, Indexer},
};
use floe_oasis::{
    doc::{RectRec, Rep},
    write::write_cell,
};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    sync::atomic::AtomicUsize,
    time::{Duration, Instant},
};

struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn wait(build: &mut Build) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(code) = build.poll().unwrap() {
            assert_eq!(code, 0);
            return;
        }
        assert!(Instant::now() < deadline, "native revision build timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
#[ignore = "requires an explicit release FLOE_INDEX_BIN; included in cache_revision battery gate"]
fn actual_build_pins_old_bytes_and_publishes_only_complete_generations() {
    let mut random = [0; 16];
    getrandom::fill(&mut random).unwrap();
    let root = Root(std::env::temp_dir().join(format!(
        "floe-native-revision-{:x}",
        u128::from_ne_bytes(random)
    )));
    fs::DirBuilder::new().mode(0o700).create(&root.0).unwrap();
    let source = root.0.join("한 글.oas");
    let shape = RectRec {
        layer: 7,
        dt: 0,
        x: 0,
        y: 0,
        w: 100,
        h: 40,
        rep: Rep::One,
    };
    let bytes = write_cell("TOP", 1000., &mut [shape], &mut []).unwrap();
    fs::write(&source, bytes).unwrap();
    let binary = Indexer::discover(&Discovery {
        override_path: Some(PathBuf::from(
            std::env::var_os("FLOE_INDEX_BIN").expect("FLOE_INDEX_BIN required"),
        )),
        development_root: None,
        executable: std::env::current_exe().unwrap(),
        search_path: None,
    })
    .unwrap();
    let options = IndexOptions {
        jobs: 1,
        occupancy: Some(true),
        representatives: true,
        ..Default::default()
    };
    let resources = Resources::new(Limits::default()).unwrap();
    let store = Store::new(&source).unwrap();
    // A live mutable-cache reader must not block a different private build.
    let legacy = cache::default_cache_path(&source).unwrap();
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("sentinel"), b"do not replace").unwrap();
    let reader = resources
        .read(cache::cache_paths(&source).unwrap())
        .unwrap();
    let stop = AtomicUsize::new(0);
    let mut build = Build::start(&resources, &source, &options, binary.clone(), &stop).unwrap();
    assert_eq!(resources.usage().cpu_slots, 1);
    assert!(store.pin().unwrap().is_none());
    wait(&mut build);
    assert!(store.pin().unwrap().is_none()); // native success is not publication
    let published = build.publish(&stop).unwrap();
    assert!(published.directory_synced);
    let first = published.snapshot;
    let layout = first.open_layout(&stop).unwrap();
    assert_eq!(layout.directory, first.directory());
    assert!(
        floe_app_core::artifact::output_path(&first.directory().join("design.ovp"), &layout)
            .is_err()
    );
    assert!(
        floe_app_core::artifact::output_path(&store.path().join("current.json"), &layout).is_err()
    );
    let old = fs::read(first.directory().join("design.ovp")).unwrap();
    assert!(first.directory().join("design.ovo").is_file());
    assert!(first.directory().join("design.ovr").is_file());
    let mut build = Build::start(&resources, &source, &options, binary.clone(), &stop).unwrap();
    wait(&mut build);
    let second = build.publish(&stop).unwrap().snapshot;
    assert_ne!(first.id(), second.id());
    assert_eq!(store.pin().unwrap().unwrap().id(), second.id());
    first.validate().unwrap();
    assert_eq!(fs::read(first.directory().join("design.ovp")).unwrap(), old);
    assert_eq!(
        fs::read(legacy.join("sentinel")).unwrap(),
        b"do not replace"
    );
    // A completed candidate can still be cancelled without moving current.
    let mut build = Build::start(&resources, &source, &options, binary.clone(), &stop).unwrap();
    let cancelled_dir = build.directory();
    wait(&mut build);
    assert!(build.publish(&AtomicUsize::new(1)).is_err());
    assert!(cancelled_dir.is_dir());
    assert_eq!(store.pin().unwrap().unwrap().id(), second.id());
    // A corrupt or source-raced successful build cannot replace current.
    let mut build = Build::start(&resources, &source, &options, binary.clone(), &stop).unwrap();
    wait(&mut build);
    fs::write(build.directory().join("design.ovm"), b"corrupt").unwrap();
    assert!(build.publish(&stop).is_err());
    assert_eq!(store.pin().unwrap().unwrap().id(), second.id());
    let mut build = Build::start(&resources, &source, &options, binary.clone(), &stop).unwrap();
    wait(&mut build);
    fs::write(&source, fs::read(&source).unwrap()).unwrap(); // same bytes/size; new ns/ctime
    assert!(build.publish(&stop).is_err());
    assert_eq!(store.pin().unwrap().unwrap().id(), second.id());
    first.validate().unwrap();
    assert!(first.open_layout(&stop).unwrap().source_stale);
    let build = Build::start(&resources, &source, &options, binary, &stop).unwrap();
    let dropped_dir = build.directory();
    drop(build); // reap even when the caller never polls
    assert!(dropped_dir.is_dir());
    assert_eq!(store.pin().unwrap().unwrap().id(), second.id());
    drop(reader);
    assert_eq!(resources.usage(), Usage::default());
}
