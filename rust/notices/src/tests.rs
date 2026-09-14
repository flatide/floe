use super::*;
use std::{
    os::unix::fs::{symlink, DirBuilderExt},
    path::PathBuf,
    sync::atomic::AtomicU64,
};
static SEQ: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "floe-notices-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        fs::create_dir(p.join("NOTICES")).unwrap();
        Self(p)
    }
    fn put(&self, name: &str, data: &[u8]) {
        fs::write(self.0.join("NOTICES").join(name), data).unwrap();
    }
    fn index(&self, names: &[&str]) -> String {
        let bytes = build_index(
            &self.0,
            &names
                .iter()
                .map(|n| format!("NOTICES/{n}"))
                .collect::<Vec<_>>(),
            "rev",
            "target",
            &AtomicUsize::new(0),
        )
        .unwrap();
        let hash = digest(&bytes);
        fs::write(self.0.join(INDEX_NAME), bytes).unwrap();
        hash
    }
    fn open(&self, hash: &str) -> io::Result<Catalog> {
        Catalog::open(&self.0, hash, "rev", "target", &AtomicUsize::new(0))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn original_utf8_binary_empty_and_html_are_lossless_bounded_pages() {
    let t = Temp::new();
    let text = "a".repeat(CHUNK_BYTES - 1) + &"한글🦀".repeat(CHUNK_BYTES / 3);
    t.put("notice.txt", text.as_bytes());
    t.put("empty", b"");
    t.put("html", b"<script>alert('not executed')</script>");
    t.put("binary", &[0, 255, 128, 10]);
    let hash = t.index(&["notice.txt", "empty", "html", "binary"]);
    let c = t.open(&hash).unwrap();
    let mut rebuilt = String::new();
    for p in 0..c.info(0).unwrap().pages {
        let v = c.page(0, p, &AtomicUsize::new(0)).unwrap();
        assert!(v.bytes <= CHUNK_BYTES);
        assert_eq!(v.offset, rebuilt.len() as u64);
        rebuilt.push_str(&v.text);
    }
    assert_eq!(rebuilt, text);
    assert_eq!(c.page(1, 0, &AtomicUsize::new(0)).unwrap().text, "");
    assert_eq!(
        c.page(2, 0, &AtomicUsize::new(0)).unwrap().text,
        "<script>alert('not executed')</script>"
    );
    assert_eq!(
        c.page(3, 0, &AtomicUsize::new(0)).unwrap().text,
        "00 ff 80 0a "
    );
    assert!(c.page(4, 0, &AtomicUsize::new(0)).is_err());
    assert!(c.page(0, usize::MAX, &AtomicUsize::new(0)).is_err());
}
#[test]
fn listing_is_paged_and_never_discovers_unlisted_files() {
    let t = Temp::new();
    let names: Vec<_> = (0..70).map(|i| i.to_string()).collect();
    for n in &names {
        t.put(n, b"x");
    }
    let hash = t.index(&names.iter().map(String::as_str).collect::<Vec<_>>());
    t.put("unlisted-secret", b"not a notice");
    let c = t.open(&hash).unwrap();
    assert_eq!(c.list(0).unwrap().files.len(), 64);
    assert_eq!(c.list(0).unwrap().next, Some(64));
    assert_eq!(c.list(64).unwrap().files.len(), 6);
    assert!(c.list(1).is_err());
    assert!(c.list(128).is_err());
    assert!(c.page(70, 0, &AtomicUsize::new(0)).is_err());
}
#[test]
fn rejects_corruption_identity_mismatch_and_cancel() {
    let t = Temp::new();
    t.put("a", b"original");
    let hash = t.index(&["a"]);
    let c = t.open(&hash).unwrap();
    assert!(t.open(&"0".repeat(40)).is_err());
    assert!(Catalog::open(&t.0, &hash, "other", "target", &AtomicUsize::new(0)).is_err());
    assert!(Catalog::open(&t.0, &hash, "rev", "other", &AtomicUsize::new(0)).is_err());
    assert!(Catalog::open(&t.0, &hash, "rev", "target", &AtomicUsize::new(1)).is_err());
    assert!(c.page(0, 0, &AtomicUsize::new(1)).is_err());
    t.put("a", b"modified");
    assert!(c.page(0, 0, &AtomicUsize::new(0)).is_err());
    t.put("a", b"short");
    assert!(c.page(0, 0, &AtomicUsize::new(0)).is_err());
    fs::write(t.0.join(INDEX_NAME), b"{}").unwrap();
    assert!(t.open(&hash).is_err());
    t.put("a", b"original");
    assert!(
        c.page(0, 0, &AtomicUsize::new(0)).is_ok(),
        "open catalogue stays pinned to its validated index"
    );
}
#[test]
fn no_symlink_fifo_directory_or_path_traversal() {
    let t = Temp::new();
    t.put("a", b"public");
    let hash = t.index(&["a"]);
    let c = t.open(&hash).unwrap();
    fs::rename(t.0.join("NOTICES/a"), t.0.join("outside")).unwrap();
    symlink("../outside", t.0.join("NOTICES/a")).unwrap();
    assert!(c.page(0, 0, &AtomicUsize::new(0)).is_err());
    fs::remove_file(t.0.join("NOTICES/a")).unwrap();
    fs::create_dir(t.0.join("NOTICES/a")).unwrap();
    assert!(c.page(0, 0, &AtomicUsize::new(0)).is_err());
    fs::remove_dir(t.0.join("NOTICES/a")).unwrap();
    let name = CString::new(t.0.join("NOTICES/a").to_str().unwrap()).unwrap();
    // SAFETY: valid new path inside this test's owned temporary directory.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(c.page(0, 0, &AtomicUsize::new(0)).is_err());
    fs::remove_file(t.0.join("NOTICES/a")).unwrap();
    t.put("a", b"public");
    fs::rename(t.0.join("NOTICES"), t.0.join("moved")).unwrap();
    symlink("moved", t.0.join("NOTICES")).unwrap();
    assert!(c.page(0, 0, &AtomicUsize::new(0)).is_err());
    for name in [
        "NOTICES/../outside",
        "/etc/passwd",
        "NOTICES//a",
        "NOTICES/./a",
        "NOTICES/a\n",
        "NOTICES/a\\b",
    ] {
        let index = Index {
            format: 1,
            source_revision: "rev".into(),
            target: "target".into(),
            files: vec![Entry {
                name: name.into(),
                bytes: 0,
                encoding: Encoding::Utf8,
                pages: vec![Page {
                    offset: 0,
                    bytes: 0,
                    digest: digest(b""),
                }],
            }],
        };
        assert!(index.validate().is_err());
    }
}
#[test]
fn rejects_malformed_tables_before_reading_notice_files() {
    let t = Temp::new();
    t.put("a", b"x");
    let hash = t.index(&["a"]);
    let original = fs::read(t.0.join(INDEX_NAME)).unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&original).unwrap();
    v["files"][0]["pages"][0]["offset"] = 1.into();
    let bytes = serde_json::to_vec(&v).unwrap();
    fs::write(t.0.join(INDEX_NAME), &bytes).unwrap();
    assert!(t.open(&digest(&bytes)).is_err());
    for bytes in [b"{".as_slice(), b"null", b"[]"] {
        fs::write(t.0.join(INDEX_NAME), bytes).unwrap();
        assert!(t.open(&digest(bytes)).is_err());
    }
    let f = File::create(t.0.join(INDEX_NAME)).unwrap();
    f.set_len(INDEX_BYTES as u64 + 1).unwrap();
    assert!(t.open(&hash).is_err());
    let f = File::create(t.0.join("NOTICES/a")).unwrap();
    f.set_len(TOTAL_BYTES + 1).unwrap();
    assert!(build_index(
        &t.0,
        &["NOTICES/a".into()],
        "rev",
        "target",
        &AtomicUsize::new(0)
    )
    .is_err());
}
