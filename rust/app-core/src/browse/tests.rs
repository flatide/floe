use super::*;
use std::{
    fs,
    os::unix::{ffi::OsStringExt, fs::symlink},
    sync::atomic::Ordering,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("floe-browse-{}", id().unwrap()));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn file(&self, name: &str) {
        fs::write(self.0.join(name), b"synthetic catalogue fixture").unwrap();
    }
    fn dir(&self, name: &str) {
        fs::create_dir_all(self.0.join(name)).unwrap();
    }
    fn browser(&self) -> Browser {
        Browser::new(std::slice::from_ref(&self.0)).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn stop() -> AtomicUsize {
    AtomicUsize::new(0)
}
fn listing(b: &mut Browser) -> Page {
    b.list(&b.roots()[0].handle, Filter::AllFiles, "", &stop())
        .unwrap()
}
fn handle(p: &Page, name: &str) -> String {
    p.rows
        .iter()
        .find(|r| r.name == name)
        .unwrap()
        .handle
        .clone()
        .unwrap()
}
fn names(p: &Page) -> Vec<&str> {
    p.rows.iter().map(|r| r.name.as_str()).collect()
}

#[test]
fn gtk_sort_hidden_suffixes_filters_and_path_free_wire() {
    let f = Fixture::new();
    for n in ["z_dir", "A_dir", "foo.oas.floe", "hidden.ICE", ".private"] {
        f.dir(n);
    }
    for n in [
        "z.oas",
        "A.oas",
        "a2.oas",
        "한국.oas",
        "deck.JB",
        "PATTERN01.TE",
        ".secret",
        "cache.FLOE",
        "side.ice",
    ] {
        f.file(n);
    }
    let mut b = f.browser();
    let p = listing(&mut b);
    assert_eq!(
        names(&p),
        [
            "A_dir",
            "z_dir",
            "A.oas",
            "a2.oas",
            "deck.JB",
            "PATTERN01.TE",
            "z.oas",
            "한국.oas"
        ]
    );
    assert_eq!(p.total, 8);
    assert_eq!(p.next, None);
    assert_eq!(p.rows[0].bytes, None);
    assert_eq!(p.rows[2].bytes, Some("27".into()));
    let wire = serde_json::to_string(&p).unwrap();
    assert!(!wire.contains(f.0.parent().unwrap().to_str().unwrap()));
    assert!(!wire.contains("synthetic catalogue fixture"));
    for row in &p.rows {
        assert_eq!(row.handle.as_ref().unwrap().len(), 64);
    }
    let root = b.roots()[0].handle.clone();
    assert_eq!(
        names(&b.list(&root, Filter::Layouts, "", &stop()).unwrap()),
        ["A_dir", "z_dir", "A.oas", "a2.oas", "z.oas", "한국.oas"]
    );
    assert_eq!(
        names(&b.list(&root, Filter::Jobdecks, "", &stop()).unwrap()),
        ["A_dir", "z_dir", "deck.JB"]
    );
    assert_eq!(
        names(&b.list(&root, Filter::AllFiles, "PATTERN", &stop()).unwrap()),
        ["PATTERN01.TE"]
    );
    assert!(b.select(&root, &stop()).is_err());
}

#[test]
fn pagination_is_complete_repeatable_and_snapshot_bound() {
    let f = Fixture::new();
    for n in 0..300 {
        f.file(&format!("{n:04}.oas"));
    }
    let mut b = f.browser();
    let p = listing(&mut b);
    assert_eq!(p.rows.len(), PAGE_ROWS);
    assert_eq!(p.total, 300);
    assert_eq!(p, b.page(&p.snapshot, 0, &stop()).unwrap());
    let p2 = b.page(&p.snapshot, p.next.unwrap(), &stop()).unwrap();
    let p3 = b.page(&p.snapshot, p2.next.unwrap(), &stop()).unwrap();
    assert_eq!(p3.next, None);
    assert_eq!(p3.rows.len(), 44);
    let all: Vec<_> = p
        .rows
        .iter()
        .chain(&p2.rows)
        .chain(&p3.rows)
        .map(|r| r.name.clone())
        .collect();
    assert_eq!(
        all,
        (0..300).map(|n| format!("{n:04}.oas")).collect::<Vec<_>>()
    );
    for offset in [1, 300, 384, usize::MAX] {
        assert!(b.page(&p.snapshot, offset, &stop()).is_err());
    }
    assert!(b.page("unknown", 0, &stop()).is_err());
    let again = listing(&mut b); // every readdir starts at offset zero, not dup's shared offset
    assert_eq!(names(&p), names(&again));
    assert_ne!(p.snapshot, again.snapshot);
    assert!(b.page(&p.snapshot, 0, &stop()).is_err());
}

#[test]
fn bounded_handles_include_breadcrumb_replay_and_keep_roots() {
    let f = Fixture::new();
    f.dir("child");
    f.file("child/a.oas");
    let mut b = f.browser();
    let root = b.roots()[0].handle.clone();
    let first = listing(&mut b);
    let child = handle(&first, "child");
    let p = b.list(&child, Filter::AllFiles, "", &stop()).unwrap();
    let oldcrumb = p.breadcrumbs[1].handle.clone();
    // Evict only the breadcrumb; a cached page must not replay dead Up/current IDs.
    b.handles.remove(&oldcrumb);
    let fresh = b.page(&p.snapshot, 0, &stop()).unwrap();
    assert_ne!(oldcrumb, fresh.breadcrumbs[1].handle);
    assert!(b.node(&fresh.directory).is_ok());
    let oldfile = handle(&fresh, "a.oas");
    let node = b.node(&oldfile).unwrap();
    for _ in 0..HANDLES {
        b.issue(node.clone()).unwrap();
    }
    assert_eq!(b.order.len(), HANDLES);
    assert!(b.handles.len() <= HANDLES);
    assert!(b.node(&oldfile).is_err());
    assert!(b.node(&root).is_ok());
    assert!(b.select(&child, &stop()).is_err());
}

#[test]
fn scans_fail_explicitly_at_limits_and_keep_previous_snapshot() {
    let f = Fixture::new();
    for n in ["a.oas", "b.oas", "c.oas"] {
        f.file(n);
    }
    let mut b = f.browser();
    let p = listing(&mut b);
    let root = b.roots()[0].handle.clone();
    b.max_matches = 2;
    assert_eq!(
        b.list(&root, Filter::AllFiles, "", &stop())
            .unwrap_err()
            .kind,
        ErrorKind::Busy
    );
    assert_eq!(b.page(&p.snapshot, 0, &stop()).unwrap(), p);
    assert_eq!(
        b.list(&root, Filter::AllFiles, "a.", &stop())
            .unwrap()
            .total,
        1
    );
    b.max_examined = 2;
    assert_eq!(
        b.list(&root, Filter::AllFiles, "nonexistent", &stop())
            .unwrap_err()
            .kind,
        ErrorKind::Busy
    );
}

#[test]
fn cancellation_invalid_input_and_empty_directory() {
    let f = Fixture::new();
    let mut b = f.browser();
    let p = listing(&mut b);
    let root = b.roots()[0].handle.clone();
    assert!(p.rows.is_empty());
    assert_eq!(p.total, 0);
    assert_eq!(p.next, None);
    let cancelled = AtomicUsize::new(1);
    assert_eq!(
        b.list(&root, Filter::AllFiles, "", &cancelled)
            .unwrap_err()
            .kind,
        ErrorKind::Cancelled
    );
    assert_eq!(
        b.page(&p.snapshot, 0, &cancelled).unwrap_err().kind,
        ErrorKind::Cancelled
    );
    for query in ["../", "a\0", "a\n", &"x".repeat(129)] {
        assert_eq!(
            b.list(&root, Filter::AllFiles, query, &stop())
                .unwrap_err()
                .kind,
            ErrorKind::InvalidInput
        );
    }
    assert!(b.list("/etc", Filter::AllFiles, "", &stop()).is_err());
    assert!(Browser::new(&[]).is_err());
    assert!(Browser::new(&[PathBuf::from("/")]).is_err());
    assert!(Browser::new(&vec![f.0.clone(); 33]).is_err());
    assert_eq!(
        Browser::new(&[f.0.clone(), f.0.clone()])
            .unwrap()
            .roots()
            .len(),
        1
    );
}

#[test]
fn non_utf8_symlinks_and_specials_are_counted_not_followed() {
    let f = Fixture::new();
    let outside = Fixture::new();
    outside.file("secret.oas");
    f.file("a.oas");
    let non_utf8 = fs::write(f.0.join(OsString::from_vec(vec![0xff])), b"x");
    // APFS rejects invalid UTF-8 filenames at creation; Linux permits them.
    #[cfg(target_os = "linux")]
    assert!(non_utf8.is_ok());
    #[cfg(target_os = "macos")]
    if let Err(e) = &non_utf8 {
        assert_eq!(e.raw_os_error(), Some(libc::EILSEQ));
    }
    symlink(&outside.0, f.0.join("external")).unwrap();
    symlink(f.0.join("a.oas"), f.0.join("inside.oas")).unwrap();
    let fifo = std::ffi::CString::new(f.0.join("pipe.oas").as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: terminated path in this test's unique directory; mkfifo creates no open fd.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let p = listing(&mut f.browser());
    assert_eq!(names(&p), ["a.oas"]);
    assert_eq!(p.skipped_names, usize::from(non_utf8.is_ok()));
    assert_eq!(p.skipped_links, 3);
}

#[test]
fn directory_mutation_invalidates_pages() {
    let f = Fixture::new();
    f.file("a.oas");
    let mut b = f.browser();
    let p = listing(&mut b);
    f.file("new.oas");
    assert_eq!(
        b.page(&p.snapshot, 0, &stop()).unwrap_err().kind,
        ErrorKind::Cache
    );
    assert_eq!(listing(&mut b).total, 2);
}

#[test]
fn replacement_and_in_place_changes_invalidate_selection_witness() {
    let f = Fixture::new();
    f.file("a.oas");
    let mut b = f.browser();
    let p = listing(&mut b);
    let h = handle(&p, "a.oas");
    let chosen = b.select(&h, &stop()).unwrap();
    chosen.validate(&stop()).unwrap();
    fs::write(f.0.join("a.oas"), b"changed").unwrap();
    assert!(b.select(&h, &stop()).is_err());
    assert!(chosen.validate(&stop()).is_err());
    let p = listing(&mut b);
    let h = handle(&p, "a.oas");
    let chosen = b.select(&h, &stop()).unwrap();
    let times = fs::metadata(f.0.join("a.oas")).unwrap().modified().unwrap();
    f.file("replacement");
    File::options()
        .write(true)
        .open(f.0.join("replacement"))
        .unwrap()
        .set_modified(times)
        .unwrap();
    fs::rename(f.0.join("replacement"), f.0.join("a.oas")).unwrap();
    assert!(b.select(&h, &stop()).is_err());
    assert!(chosen.validate(&stop()).is_err());
}

#[test]
fn parent_and_root_replacement_cannot_redirect_handles() {
    let f = Fixture::new();
    let outside = Fixture::new();
    f.dir("child");
    f.file("child/a.oas");
    outside.file("a.oas");
    let mut b = f.browser();
    let p = listing(&mut b);
    let child = handle(&p, "child");
    let p = b.list(&child, Filter::AllFiles, "", &stop()).unwrap();
    let h = handle(&p, "a.oas");
    let chosen = b.select(&h, &stop()).unwrap();
    fs::rename(f.0.join("child"), f.0.join("old")).unwrap();
    symlink(&outside.0, f.0.join("child")).unwrap();
    assert!(b.select(&h, &stop()).is_err());
    assert!(b.list(&child, Filter::AllFiles, "", &stop()).is_err());
    assert!(chosen.validate(&stop()).is_err());
    fs::remove_file(f.0.join("child")).unwrap();
    fs::rename(f.0.join("old"), f.0.join("child")).unwrap();
    let moved = f.0.with_extension("moved");
    fs::rename(&f.0, &moved).unwrap();
    symlink(&outside.0, &f.0).unwrap();
    assert!(b.select(&h, &stop()).is_err());
    assert!(chosen.validate(&stop()).is_err());
    assert!(b.page(&p.snapshot, 0, &stop()).is_err());
    fs::remove_file(&f.0).unwrap();
    fs::rename(moved, &f.0).unwrap();
}

#[test]
fn depth_overflow_is_visible_but_not_selectable_and_reads_never_write() {
    let f = Fixture::new();
    let deep = vec!["d"; DEPTH].join("/");
    f.dir(&deep);
    f.file(&format!("{deep}/a.oas"));
    let before = fs::read(f.0.join(&deep).join("a.oas")).unwrap();
    let mut b = f.browser();
    let mut p = listing(&mut b);
    for _ in 0..DEPTH {
        p = b
            .list(&handle(&p, "d"), Filter::AllFiles, "", &stop())
            .unwrap();
    }
    assert_eq!(names(&p), ["a.oas"]);
    assert_eq!(p.rows[0].handle, None);
    assert_eq!(fs::read(f.0.join(&deep).join("a.oas")).unwrap(), before);
    let cancelled = stop();
    cancelled.store(1, Ordering::Relaxed);
    assert!(b.page(&p.snapshot, 0, &cancelled).is_err());
}
