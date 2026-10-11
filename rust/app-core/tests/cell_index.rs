//! Small actual-native cell index lifecycle; no browser or field data.
use floe_app_core::{
    cache::{self, CacheState},
    cell_index::{self, Outcome, Report, Stage},
    managed::{Limits, Resources, Usage},
    native::{Discovery, Indexer},
    registered::{AccessScope, RegisteredSource},
    ErrorKind,
};
use floe_oasis::{
    doc::{RectRec, Rep},
    write::{write_tree, WCell},
};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};

struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn indexer() -> Indexer {
    Indexer::discover(&Discovery {
        override_path: Some(PathBuf::from(
            std::env::var_os("FLOE_INDEX_BIN").expect("FLOE_INDEX_BIN required"),
        )),
        development_root: None,
        executable: std::env::current_exe().unwrap(),
        search_path: None,
    })
    .unwrap()
}
/// TOP places LEAF twice: a summary with two cells and one edge.
fn layout(path: &Path) {
    let rect = RectRec {
        layer: 7,
        dt: 0,
        x: 0,
        y: 0,
        w: 100,
        h: 40,
        rep: Rep::One,
    };
    let one = Rep::One;
    let cells = [
        WCell {
            name: "LEAF".into(),
            rects: std::slice::from_ref(&rect),
            polys: &[],
            paths: &[],
            texts: &[],
            places: Vec::new(),
        },
        WCell {
            name: "TOP".into(),
            rects: &[],
            polys: &[],
            paths: &[],
            texts: &[],
            places: vec![
                ("LEAF", 0, 0, 0, false, &one),
                ("LEAF", 500, 0, 0, false, &one),
            ],
        },
    ];
    fs::write(path, write_tree(&cells, 1000.).unwrap()).unwrap();
}
/// A cache as built before design.ovh existed.
fn index_without_hier(indexer: &Indexer, source: &Path) -> PathBuf {
    let dir = cache::default_cache_path(source).unwrap();
    let status = Command::new(indexer.path())
        .arg("vfs")
        .arg(source)
        .arg(&dir)
        .args(["--jobs", "1", "--no-lod", "--no-hier"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!dir.join("design.ovh").exists());
    assert_eq!(cache::inspect(source, &dir).unwrap(), CacheState::Current);
    dir
}
fn check(indexer: &Indexer, dir: &Path) -> bool {
    Command::new(indexer.path())
        .arg("hier")
        .arg(dir)
        .arg("--check")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .unwrap()
        .success()
}
fn counts(r: &Report) -> [usize; 5] {
    [r.total, r.built, r.kept, r.skipped, r.failed]
}

#[test]
#[ignore = "requires an explicit release FLOE_INDEX_BIN; included in the cell_index battery gate"]
fn actual_summary_is_added_beside_a_live_reader_then_kept() {
    let mut random = [0; 16];
    getrandom::fill(&mut random).unwrap();
    let root = Root(
        std::env::temp_dir().join(format!("floe-cell-index-{:x}", u128::from_ne_bytes(random))),
    );
    fs::DirBuilder::new().mode(0o700).create(&root.0).unwrap();
    let indexer = indexer();
    let flag = AtomicUsize::new(0);
    let a = root.0.join("한 글.oas");
    let b = root.0.join("b.oas");
    let c = root.0.join("c.oas");
    layout(&a);
    fs::copy(&a, &b).unwrap();
    fs::copy(&a, &c).unwrap();
    let dir = index_without_hier(&indexer, &a);
    let ovm = fs::read(dir.join("design.ovm")).unwrap();
    // b: a cache that no longer matches its source; c: none at all.
    index_without_hier(&indexer, &b);
    fs::File::options()
        .write(true)
        .open(&b)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(10))
        .unwrap();

    let resources = Resources::new(Limits::default()).unwrap();
    let scope = AccessScope::new(std::slice::from_ref(&root.0)).unwrap();
    let source = RegisteredSource::register(Arc::clone(&scope), &a, &flag).unwrap();
    // An open view's read lease does not block the summary.
    let reader = resources.read(cache::cache_paths(&a).unwrap()).unwrap();
    let mut seen = Vec::new();
    let report = cell_index::run(&resources, &source, None, &indexer, &flag, &mut |r| {
        seen.push(*r)
    })
    .unwrap();
    assert_eq!(counts(&report), [1, 1, 0, 0, 0]);
    assert_eq!(report.stage, Stage::Done);
    let stages: Vec<_> = seen.iter().map(|r| r.stage).collect();
    assert!(
        stages.contains(&Stage::Checking) && stages.contains(&Stage::Building),
        "{stages:?}"
    );
    assert!(dir.join("design.ovh").is_file());
    assert!(!dir.join("design.ovh.tmp").exists());
    assert!(check(&indexer, &dir));
    assert_eq!(
        fs::read(dir.join("design.ovm")).unwrap(),
        ovm,
        "cache changed"
    );
    assert_eq!(resources.usage(), Usage::default(), "reservation kept");

    let report = cell_index::run(&resources, &source, None, &indexer, &flag, &mut |_| {}).unwrap();
    assert_eq!(counts(&report), [1, 0, 1, 0, 0]);
    assert_eq!(
        cell_index::ensure(&a, &indexer, &flag).unwrap(),
        Outcome::Kept
    );
    // A summary that no longer validates is rebuilt, not kept.
    let summary = dir.join("design.ovh");
    let mut bytes = fs::read(&summary).unwrap();
    bytes[0] = b'?';
    fs::write(&summary, bytes).unwrap();
    match cell_index::ensure(&a, &indexer, &flag).unwrap() {
        Outcome::Built(stats) => assert_eq!((stats.cells, stats.edges), (2, 1)),
        other => panic!("{other:?}"),
    }
    assert!(check(&indexer, &dir));

    // Stale or missing caches are refused, never indexed here.
    for (path, words) in [(&b, "not current"), (&c, "no index")] {
        let e = cell_index::ensure(path, &indexer, &flag).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Cache);
        assert!(e.message.contains(words), "{}", e.message);
    }
    assert!(!cache::default_cache_path(&c).unwrap().exists());
    assert!(!cache::default_cache_path(&b)
        .unwrap()
        .join("design.ovh")
        .exists());

    // A deck summarizes its selected, current sources and skips the others.
    let deck_path = root.0.join("test.jb");
    fs::write(
        &deck_path,
        "MTITLE 1,Mask\nCHIP C\n$ (1,A,TC='한 글.oas')\n$ (2,B,TC=b.oas)\n$ (3,C,TC=c.oas)\nROWS 0/0\n",
    )
    .unwrap();
    let deck = RegisteredSource::register(Arc::clone(&scope), &deck_path, &flag).unwrap();
    let report = cell_index::run(&resources, &deck, None, &indexer, &flag, &mut |_| {}).unwrap();
    assert_eq!(counts(&report), [3, 0, 1, 2, 0]);
    let first = BTreeSet::from([1]);
    let report = cell_index::run(
        &resources,
        &deck,
        Some(&first),
        &indexer,
        &flag,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(counts(&report), [1, 0, 1, 0, 0]);

    // A managed rebuild of these caches refuses it; so does cancellation.
    drop(reader);
    let writer = resources.index(cache::cache_paths(&a).unwrap(), 1).unwrap();
    let e = cell_index::run(&resources, &source, None, &indexer, &flag, &mut |_| {}).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Busy);
    drop(writer);
    flag.store(1, Ordering::Relaxed);
    fs::remove_file(&summary).unwrap();
    let e = cell_index::run(&resources, &source, None, &indexer, &flag, &mut |_| {
        panic!("progress after cancellation")
    })
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Cancelled);
    assert_eq!(
        cell_index::ensure(&a, &indexer, &flag).unwrap_err().kind,
        ErrorKind::Cancelled
    );
    assert!(!summary.exists());
    assert_eq!(resources.usage(), Usage::default());
}
