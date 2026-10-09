//! The cell tree's hierarchy summary (design.ovh) added to current mutable
//! caches by `floe-index hier`, one explicit owner request at a time.
//! It takes the cross-process index writer lock but only READ leases in this
//! process: an open view keeps reading while the summary is written aside and
//! renamed (floe_vfs::hiersum::write), and the renderer picks it up by stat.
//! Sealed revisions are never amended; `floe-index vfs` writes design.ovh
//! into every new build by default.
use crate::{
    cache::{self, CacheState},
    check_cancelled,
    index::WriteLease,
    index_progress,
    jobdeck::index::is_deck,
    managed::Resources,
    native::{self, Indexer},
    registered::RegisteredSource,
    Error, ErrorKind, Result,
};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    io::Read,
    os::unix::process::ExitStatusExt,
    path::Path,
    process::Child,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const OUTPUT_TAIL: usize = 16 * 1024;

/// The numeric part of `floe-index hier`'s report; its path is never kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub cells: u64,
    pub edges: u64,
    pub records: u64,
    pub unplaced: u64,
    pub bytes: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A valid summary matching the cache was already there.
    Kept,
    Built(Stats),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Preparing,
    Checking,
    Building,
    Done,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub stage: Stage,
    /// 1-based position of the source in progress; 0 before the first.
    pub current: usize,
    pub total: usize,
    pub built: usize,
    pub kept: usize,
    /// Deck sources without a current cache: the view shows none of them.
    pub skipped: usize,
    pub failed: usize,
    /// Safe category of the first failed deck source.
    pub failure: Option<ErrorKind>,
}

/// One layout source's current cache. Verifies the matched indexer first.
pub fn ensure(source: &Path, indexer: &Indexer, cancelled: &AtomicUsize) -> Result<Outcome> {
    check_cancelled(cancelled)?;
    indexer.verify(cancelled)?;
    ensure_verified(source, indexer, cancelled, &mut |_| {})
}

/// Every selected source of a registered layout or jobdeck (an open view's
/// levels). A deck source that fails does not stop the others; a layout's
/// failure, and any cancellation, is the whole result.
pub fn run(
    resources: &Arc<Resources>,
    source: &RegisteredSource,
    levels: Option<&BTreeSet<i64>>,
    indexer: &Indexer,
    cancelled: &AtomicUsize,
    progress: &mut dyn FnMut(&Report),
) -> Result<Report> {
    check_cancelled(cancelled)?;
    source.validate(cancelled)?;
    // The managed-index singleton and one CPU slot, with read leases only:
    // views of these caches stay open, a managed rebuild of them is refused.
    let _permit = resources.index_planning(source.cache_paths()?, 1)?;
    indexer.verify(cancelled)?;
    let sources = source.selected_sources(levels, cancelled)?;
    let mut report = Report {
        total: sources.len(),
        ..Default::default()
    };
    progress(&report);
    for path in &sources {
        check_cancelled(cancelled)?;
        report.current += 1;
        report.stage = Stage::Preparing;
        progress(&report);
        if source.deck && !current(path) {
            report.skipped += 1;
            continue;
        }
        let result = ensure_verified(path, indexer, cancelled, &mut |stage| {
            report.stage = stage;
            progress(&report);
        });
        match result {
            Ok(Outcome::Kept) => report.kept += 1,
            Ok(Outcome::Built(_)) => report.built += 1,
            Err(e) if e.kind == ErrorKind::Cancelled || !source.deck => return Err(e),
            Err(e) => {
                report.failed += 1;
                report.failure.get_or_insert(e.kind);
            }
        }
    }
    report.stage = Stage::Done;
    progress(&report);
    Ok(report)
}

fn current(source: &Path) -> bool {
    cache::cache_path(source)
        .and_then(|dir| cache::inspect(source, &dir))
        .is_ok_and(|s| s == CacheState::Current)
}

fn ensure_verified(
    source: &Path,
    indexer: &Indexer,
    cancelled: &AtomicUsize,
    stage: &mut dyn FnMut(Stage),
) -> Result<Outcome> {
    check_cancelled(cancelled)?;
    let source = cache::absolute(source)?;
    if is_deck(&source) {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "a jobdeck has no cache of its own; summarize its sources",
        ));
    }
    cache::fingerprint(&source)?;
    let _lease = WriteLease::acquire_aliases(&cache::cache_paths(&source)?)?;
    // Resolve under both writer locks. Never migrate a legacy name here: an
    // open view reads the cache under the name it resolved.
    let directory = cache::cache_path(&source)?;
    match cache::inspect(&source, &directory)? {
        CacheState::Current => (),
        CacheState::Missing => {
            return Err(Error::new(
                ErrorKind::Cache,
                "this source has no index; index it before building its cell index",
            ))
        }
        CacheState::Unusable(reason) => {
            return Err(Error::new(
                ErrorKind::Cache,
                format!("the index is not current ({reason}); index the source again first"),
            ))
        }
    }
    hier(indexer, &directory, cancelled, stage)
}

/// `hier <dir> --check`, then `hier <dir>` only when no valid summary is
/// there. The caller holds the writer lock over both passes.
fn hier(
    indexer: &Indexer,
    directory: &Path,
    cancelled: &AtomicUsize,
    stage: &mut dyn FnMut(Stage),
) -> Result<Outcome> {
    let dir = OsString::from(cache::utf8(directory)?);
    stage(Stage::Checking);
    let check = native(
        indexer,
        &["hier".into(), dir.clone(), "--check".into()],
        cancelled,
    )?;
    if check.code == 0 {
        return Ok(Outcome::Kept);
    }
    check_cancelled(cancelled)?;
    stage(Stage::Building);
    let built = native(indexer, &["hier".into(), dir], cancelled);
    // A run the index locks refused (floe_vfs::lock::BUSY_EXIT) wrote
    // nothing: an aside file there is the lock holder's, never ours.
    let busy = matches!(&built, Ok(f) if f.code == floe_vfs::lock::BUSY_EXIT);
    let failed = !matches!(&built, Ok(f) if f.code == 0);
    if failed && !busy {
        // A killed or failed writer can leave its aside file; the lease is ours.
        match fs::remove_file(directory.join("design.ovh.tmp")) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => eprintln!("[floe2] cannot clean cell index temp: {e}"),
        }
    }
    let built = built?;
    if built.code != 0 {
        let text = String::from_utf8_lossy(&built.stderr);
        if let Some(line) = text.lines().rev().find(|l| !l.trim().is_empty()) {
            eprintln!("[floe2] floe-index hier failed: {}", line.trim());
        }
        if busy {
            return Err(Error::new(
                ErrorKind::Busy,
                "floe-index hier refused: the cache is being indexed or used",
            ));
        }
        return Err(Error::new(ErrorKind::Worker, "floe-index hier failed"));
    }
    parse(&built.stdout).map(Outcome::Built).ok_or_else(|| {
        Error::new(
            ErrorKind::Worker,
            "floe-index hier reported no summary line",
        )
    })
}

/// `hier file=<path> cells=N edges=N records=N unplaced=N bytes=N seconds=S`:
/// read from the end, so a path spelling cannot supply a field.
fn parse(stdout: &[u8]) -> Option<Stats> {
    let text = String::from_utf8_lossy(stdout);
    if !text.starts_with("hier file=") {
        return None;
    }
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 7 {
        return None;
    }
    let tail = &tokens[tokens.len() - 6..];
    let field = |i: usize, key: &str| -> Option<&str> {
        tail[i]
            .strip_prefix(key)
            .and_then(|v| v.strip_prefix('='))
            .filter(|v| !v.is_empty())
    };
    let number = |i: usize, key: &str| field(i, key)?.parse::<u64>().ok();
    field(5, "seconds")?
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.)?;
    Some(Stats {
        cells: number(0, "cells")?,
        edges: number(1, "edges")?,
        records: number(2, "records")?,
        unplaced: number(3, "unplaced")?,
        bytes: number(4, "bytes")?,
    })
}

struct Finished {
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}
/// Owns the child until it is reaped, including on an early return.
struct Reaped(Option<Child>);
impl Reaped {
    /// SIGTERM, one second to exit, then SIGKILL; like IndexJob::cancel.
    fn terminate(&mut self) -> Result<()> {
        let Some(child) = self.0.as_mut() else {
            return Ok(());
        };
        if child.try_wait()?.is_none() {
            native::signal_child(child, libc::SIGTERM)?;
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline {
                if child.try_wait()?.is_some() {
                    self.0 = None;
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(10));
            }
            child.kill()?;
            child.wait()?;
        }
        self.0 = None;
        Ok(())
    }
}
impl Drop for Reaped {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
/// One native pass. Both pipes are drained on this thread in bounded
/// nonblocking turns and only their tails are kept.
fn native(indexer: &Indexer, args: &[OsString], cancelled: &AtomicUsize) -> Result<Finished> {
    check_cancelled(cancelled)?;
    let mut child = Reaped(Some(indexer.spawn(args, true)?));
    let running = child.0.as_mut().expect("spawned child");
    let mut stdout = running.stdout.take().expect("captured stdout");
    let mut stderr = running.stderr.take().expect("captured stderr");
    index_progress::nonblocking(&stdout)?;
    index_progress::nonblocking(&stderr)?;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    loop {
        if cancelled.load(Ordering::Relaxed) != 0 {
            child.terminate()?;
            return Err(Error::new(ErrorKind::Cancelled, "cell index cancelled"));
        }
        drain(&mut stdout, &mut out)?;
        drain(&mut stderr, &mut err)?;
        let status = child.0.as_mut().expect("unreaped child").try_wait()?;
        if let Some(status) = status {
            child.0 = None;
            // Leader exit is not pipe EOF: a descendant may hold the pipes.
            for _ in 0..8 {
                drain(&mut stdout, &mut out)?;
                drain(&mut stderr, &mut err)?;
            }
            return Ok(Finished {
                code: status
                    .code()
                    .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)),
                stdout: out,
                stderr: err,
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
}
fn drain(reader: &mut impl Read, tail: &mut Vec<u8>) -> Result<()> {
    let mut buf = [0; 8192];
    for _ in 0..8 {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > OUTPUT_TAIL {
                    tail.drain(..tail.len() - OUTPUT_TAIL);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct Root(PathBuf);
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn root(name: &str) -> Root {
        let dir = std::env::temp_dir().join(format!(
            "floe-cell-index-{name}-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Root(dir)
    }
    fn indexer(path: &Path) -> Indexer {
        Indexer::discover(&native::Discovery {
            override_path: Some(path.to_owned()),
            development_root: None,
            executable: PathBuf::from("/unused"),
            search_path: None,
        })
        .unwrap()
    }
    /// A stand-in for `floe-index hier`, driven by marker files in the cache.
    fn fake(root: &Root) -> Indexer {
        let script = root.0.join("fake-index");
        fs::write(
            &script,
            "#!/bin/sh\n\
             [ \"$1\" = hier ] || exit 2\n\
             if [ \"$3\" = --check ]; then echo \"hier file=$2/design.ovh identity=none\"; [ -f \"$2/design.ovh\" ]; exit $?; fi\n\
             [ -f \"$2/fail\" ] && { echo 'hier: synthetic failure' >&2; : > \"$2/design.ovh.tmp\"; exit 1; }\n\
             [ -f \"$2/busy\" ] && { echo '[lock] a.oas is being indexed by someone' >&2; exit 75; }\n\
             if [ -f \"$2/hang\" ]; then : > \"$2/design.ovh.tmp\"; exec sleep 30; fi\n\
             : > \"$2/design.ovh\"\n\
             echo \"hier file=$2/design.ovh cells=7 edges=9 records=12 unplaced=1 bytes=640 seconds=0.0\"\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        indexer(&script)
    }
    #[test]
    fn summary_line_is_read_from_its_numeric_tail_only() {
        let line = b"hier file=/c/.a.oas.ice/design.ovh cells=7 edges=9 records=12 unplaced=1 bytes=640 seconds=0.3\n";
        let stats = Stats {
            cells: 7,
            edges: 9,
            records: 12,
            unplaced: 1,
            bytes: 640,
        };
        assert_eq!(parse(line), Some(stats));
        // A path holding field-shaped words and newlines supplies nothing.
        let tricky = b"hier file=/x cells=99 edges=1\nrecords=2/y z/design.ovh cells=7 edges=9 records=12 unplaced=1 bytes=640 seconds=1.0\n";
        assert_eq!(parse(tricky), Some(stats));
        for bad in [
            &b""[..],
            b"hier file=/x identity=none\n",
            b"hier: cannot open\n",
            b"hier file=/x cells=7 edges=9 records=12 unplaced=1 bytes=640\n",
            b"hier file=/x cells=7 edges=9 records=12 unplaced=1 bytes=-1 seconds=0\n",
            b"hier file=/x cells=7 edges=9 records=12 unplaced=1 bytes=1 seconds=NaN\n",
            b"hier file=/x edges=9 cells=7 records=12 unplaced=1 bytes=1 seconds=0\n",
            b"cells=7 edges=9 records=12 unplaced=1 bytes=640 seconds=0\n",
        ] {
            assert_eq!(parse(bad), None, "{}", String::from_utf8_lossy(bad));
        }
    }
    #[test]
    fn check_then_build_keeps_a_valid_summary_and_cleans_a_failed_writer() {
        let r = root("passes");
        let cache = r.0.join(".a.oas.ice");
        fs::create_dir(&cache).unwrap();
        let fake = fake(&r);
        let flag = AtomicUsize::new(0);
        let mut stages = Vec::new();
        let outcome = hier(&fake, &cache, &flag, &mut |s| stages.push(s)).unwrap();
        assert_eq!(
            outcome,
            Outcome::Built(Stats {
                cells: 7,
                edges: 9,
                records: 12,
                unplaced: 1,
                bytes: 640
            })
        );
        assert_eq!(stages, [Stage::Checking, Stage::Building]);
        stages.clear();
        assert_eq!(
            hier(&fake, &cache, &flag, &mut |s| stages.push(s)).unwrap(),
            Outcome::Kept
        );
        assert_eq!(stages, [Stage::Checking]);
        fs::remove_file(cache.join("design.ovh")).unwrap();
        fs::write(cache.join("fail"), b"").unwrap();
        let e = hier(&fake, &cache, &flag, &mut |_| {}).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Worker);
        assert!(
            !cache.join("design.ovh.tmp").exists(),
            "failed writer temp kept"
        );
        assert!(!cache.join("design.ovh").exists());
    }
    #[test]
    fn a_run_the_index_locks_refuse_is_busy_and_keeps_the_holders_temp() {
        let r = root("busy");
        let cache = r.0.join(".a.oas.ice");
        fs::create_dir(&cache).unwrap();
        // another run (not this service's lease) is writing its aside file
        fs::write(cache.join("design.ovh.tmp"), b"theirs").unwrap();
        fs::write(cache.join("busy"), b"").unwrap();
        let fake = fake(&r);
        let e = hier(&fake, &cache, &AtomicUsize::new(0), &mut |_| {}).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Busy);
        assert_eq!(fs::read(cache.join("design.ovh.tmp")).unwrap(), b"theirs");
        assert!(!cache.join("design.ovh").exists());
    }
    #[test]
    fn cancellation_stops_the_writer_promptly_and_removes_its_temp() {
        let r = root("cancel");
        let cache = r.0.join(".a.oas.ice");
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("hang"), b"").unwrap();
        let fake = fake(&r);
        let flag = AtomicUsize::new(0);
        let started = Instant::now();
        let e = thread::scope(|s| {
            s.spawn(|| {
                let deadline = Instant::now() + Duration::from_secs(10);
                while !cache.join("design.ovh.tmp").exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                flag.store(1, Ordering::Relaxed);
            });
            hier(&fake, &cache, &flag, &mut |_| {}).unwrap_err()
        });
        assert_eq!(e.kind, ErrorKind::Cancelled);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "writer not stopped"
        );
        assert!(!cache.join("design.ovh.tmp").exists());
        assert!(!cache.join("design.ovh").exists());
        // Cancelled before the first pass: nothing is started.
        fs::remove_file(cache.join("hang")).unwrap();
        let e = hier(&fake, &cache, &flag, &mut |_| {}).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Cancelled);
        assert!(!cache.join("design.ovh").exists());
    }
    #[test]
    fn refuses_decks_missing_and_stale_caches_before_any_native_pass() {
        let r = root("refuse");
        // Every refusal precedes the indexer: /usr/bin/false would fail a pass.
        let never = indexer(Path::new("/usr/bin/false"));
        let flag = AtomicUsize::new(0);
        let source = r.0.join("a.oas");
        fs::write(&source, b"%SEMI-OASIS\r\n").unwrap();
        let e = ensure_verified(&source, &never, &flag, &mut |_| panic!("pass")).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Cache);
        assert!(e.message.contains("index it"), "{}", e.message);
        let cache = cache::default_cache_path(&source).unwrap();
        fs::create_dir(&cache).unwrap();
        fs::write(cache.join("meta.json"), b"{}").unwrap();
        let e = ensure_verified(&source, &never, &flag, &mut |_| panic!("pass")).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Cache);
        assert!(e.message.contains("not current"), "{}", e.message);
        assert!(!cache.join("design.ovh").exists());
        let deck = r.0.join("a.jb");
        fs::write(&deck, b"CHIP A\n").unwrap();
        let e = ensure_verified(&deck, &never, &flag, &mut |_| panic!("pass")).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unsupported);
        let e = ensure_verified(&r.0.join("gone.oas"), &never, &flag, &mut |_| {
            panic!("pass")
        })
        .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Io);
        flag.store(1, Ordering::Relaxed);
        let e = ensure(&source, &never, &flag).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Cancelled);
        // Another writer holds the cache: refused, not waited for.
        flag.store(0, Ordering::Relaxed);
        let held = WriteLease::acquire_aliases(&cache::cache_paths(&source).unwrap()).unwrap();
        let e = ensure_verified(&source, &never, &flag, &mut |_| panic!("pass")).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Busy);
        drop(held);
    }
}
