use super::*;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    sync::atomic::AtomicU64,
};
static SERIAL: AtomicU64 = AtomicU64::new(0);
pub(crate) fn file(bytes: &[u8]) -> File {
    let p = std::env::temp_dir().join(format!(
        "floe-artifact-test-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let mut writer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .unwrap();
    writer.write_all(bytes).unwrap();
    drop(writer);
    let file = File::open(&p).unwrap();
    fs::remove_file(p).unwrap();
    file
}
fn store() -> Arc<Store> {
    Store::new(Limits {
        entries: 2,
        artifact_bytes: 100,
        total_bytes: 150,
        readers: 2,
        ttl: Duration::from_secs(60),
    })
    .unwrap()
}
fn reserve(store: &Arc<Store>, id: u64) -> Reservation {
    store.reserve(id, Arc::new(AtomicUsize::new(0))).unwrap()
}
#[test]
fn reservations_size_validation_and_failed_commit_release_capacity() {
    let s = store();
    let r = reserve(&s, 1);
    assert_eq!(
        s.usage(),
        Usage {
            entries: 1,
            bytes: 100,
            pending: 1,
            readers: 0
        }
    );
    assert!(s.reserve(2, Arc::new(AtomicUsize::new(0))).is_err());
    assert!(r.commit(file(b"size"), 5).is_err());
    assert_eq!(s.usage(), Usage::default());
    assert!(reserve(&s, 2).commit(file(&[0; 101]), 101).is_err());
    assert_eq!(s.usage(), Usage::default());
    reserve(&s, 3).commit(file(b"hello"), 5).unwrap();
    let pending = reserve(&s, 4);
    assert_eq!(s.usage().bytes, 105);
    assert!(s.reserve(5, Arc::new(AtomicUsize::new(0))).is_err());
    drop(pending);
    assert_eq!(s.usage().bytes, 5);
    assert!(s.release(3));
    assert_eq!(s.usage(), Usage::default());
}
#[test]
fn chunks_have_independent_offsets_and_readers_are_bounded() {
    let s = store();
    reserve(&s, 1).commit(file(b"abcdefgh"), 8).unwrap();
    assert_eq!(s.info(1).unwrap().size_bytes, 8);
    assert!(s.info(1).unwrap().expires_in_ms <= 60_000);
    assert_eq!(s.usage().readers, 0);
    let mut a = s.open(1).unwrap();
    let mut b = s.open(1).unwrap();
    assert!(s.open(1).is_err());
    assert_eq!(a.read_chunk(3).unwrap(), b"abc");
    assert_eq!(b.read_chunk(5).unwrap(), b"abcde");
    assert_eq!(a.read_chunk(30).unwrap(), b"defgh");
    assert!(a.read_chunk(2).unwrap().is_empty());
    assert!(b.read_chunk(0).is_err());
    assert!(b.read_chunk(CHUNK_BYTES + 1).is_err());
    drop(a);
    assert_eq!(s.usage().readers, 1);
    assert_eq!(b.read_chunk(5).unwrap(), b"fgh");
    s.release(1);
    assert!(s.info(1).is_none());
    assert!(!b.is_available());
    assert_eq!(s.usage().bytes, 8);
    assert!(b.read_chunk(5).is_err());
    drop(b);
    assert_eq!(s.usage(), Usage::default());
}
#[test]
fn revoked_pending_work_keeps_reservation_until_cleanup() {
    let s = store();
    let flag = Arc::new(AtomicUsize::new(0));
    let r = s.reserve(1, Arc::clone(&flag)).unwrap();
    s.close();
    assert_ne!(flag.load(Ordering::Relaxed), 0);
    assert_eq!(s.usage().pending, 1);
    assert_eq!(s.usage().bytes, 100);
    assert!(r.commit(file(b"a"), 1).is_err());
    assert_eq!(s.usage(), Usage::default());
    assert!(s.open(1).is_err());
}
#[test]
fn expiry_is_automatic_but_active_files_stay_charged() {
    let s = store();
    reserve(&s, 1).commit(file(b"a"), 1).unwrap();
    reserve(&s, 2).commit(file(b"b"), 1).unwrap();
    let mut reader = s.open(1).unwrap();
    // Advance the stored deadlines only after acquiring the reader, avoiding
    // a 30ms scheduling race on loaded builders. No lookup/notification drives
    // expiry: the background reaper must discover both expired entries itself.
    for entry in s.shared.state.lock().unwrap().rows.values_mut() {
        entry.expires = Some(Instant::now());
    }
    assert!(s.info(1).is_none());
    assert!(!reader.is_available());
    let end = Instant::now() + Duration::from_secs(3);
    while s.usage().entries == 2 && Instant::now() < end {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(s.usage().entries, 1);
    assert_eq!(s.usage().bytes, 1);
    assert!(reader.read_chunk(1).is_err());
    drop(reader);
    assert_eq!(s.usage(), Usage::default());
}
#[test]
fn reactor_revocation_is_immediate_and_disposal_is_eventually_reaped() {
    for close in [false, true] {
        let s = store();
        reserve(&s, 1).commit(file(b"artifact"), 8).unwrap();
        if close {
            s.request_close();
        } else {
            assert!(s.request_release(1));
        }
        assert!(s.info(1).is_none());
        assert!(s.open(1).is_err());
        let end = Instant::now() + Duration::from_secs(3);
        while s.usage() != Usage::default() {
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(5));
        }
    }
}
#[test]
fn named_input_or_open_writer_size_change_cannot_be_served() {
    let s = store();
    let named = File::open(std::env::current_exe().unwrap()).unwrap();
    let size = named.metadata().unwrap().len();
    // A correct declared length must still be rejected for a named file.
    let named_store = Store::new(Limits::default()).unwrap();
    assert!(reserve(&named_store, 1).commit(named, size).is_err());
    assert_eq!(named_store.usage(), Usage::default());
    assert_eq!(s.usage(), Usage::default());
    let p = std::env::temp_dir().join(format!(
        "floe-artifact-writer-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let mut writer = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&p)
        .unwrap();
    writer.write_all(b"old").unwrap();
    let f = File::open(&p).unwrap();
    fs::remove_file(&p).unwrap();
    reserve(&s, 2).commit(f, 3).unwrap();
    let mut download = s.open(2).unwrap();
    writer.write_all(b"new").unwrap();
    assert!(download.read_chunk(3).is_err());
}
#[test]
fn limits_are_explicit_and_store_close_revokes_existing_handles() {
    assert!(Store::new(Limits {
        entries: 0,
        ..Limits::default()
    })
    .is_err());
    assert!(Store::new(Limits {
        total_bytes: 1,
        ..Limits::default()
    })
    .is_err());
    let s = store();
    reserve(&s, 1).commit(file(b"complete"), 8).unwrap();
    let mut r = s.open(1).unwrap();
    s.close();
    assert!(r.read_chunk(8).is_err());
    assert!(s.open(1).is_err());
    assert_eq!(s.usage().bytes, 8);
    drop(r);
    assert_eq!(s.usage(), Usage::default());
}
