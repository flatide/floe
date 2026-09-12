use crate::{
    cache::{self, CacheState},
    native::{self, Indexer},
    Error, ErrorKind, Result,
};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::os::unix::{
    fs::{MetadataExt, OpenOptionsExt},
    process::ExitStatusExt,
};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileCell {
    Name(String),
    Index(usize),
}
#[derive(Clone, Debug)]
pub struct IndexOptions {
    pub force: bool,
    pub jobs: usize,
    pub page_target_mb: Option<u64>,
    pub lod: bool,
    pub occupancy: bool,
    pub occupancy_only: bool,
    pub occupancy_um: Option<f64>,
    pub slow_cell_s: Option<f64>,
    pub p2_shard_limit_mb: Option<u64>,
    pub profile_cell: Option<ProfileCell>,
    pub profile_jobs: Option<Vec<usize>>,
    pub profile_repeat: usize,
    pub profile_snapshot: Option<PathBuf>,
    pub profile_snapshot_refresh: bool,
}
impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            force: false,
            jobs: 12,
            page_target_mb: None,
            lod: false,
            occupancy: false,
            occupancy_only: false,
            occupancy_um: None,
            slow_cell_s: None,
            p2_shard_limit_mb: None,
            profile_cell: None,
            profile_jobs: None,
            profile_repeat: 1,
            profile_snapshot: None,
            profile_snapshot_refresh: false,
        }
    }
}
impl IndexOptions {
    pub fn validate(&self) -> Result<()> {
        if self.jobs == 0 || self.jobs.checked_mul(8).is_none() || self.profile_repeat == 0 {
            return Err(Error::input(
                "jobs/repeat must be positive and representable",
            ));
        }
        if let Some(v) = self.page_target_mb {
            if v == 0 || v.checked_mul(1 << 20).is_none() {
                return Err(Error::input("invalid page-target-mb"));
            }
        }
        if self
            .p2_shard_limit_mb
            .is_some_and(|n| n.checked_mul(1 << 20).is_none())
        {
            return Err(Error::input("p2-shard-limit-mb overflows bytes"));
        }
        if self.occupancy_um.is_some_and(|v| !v.is_finite() || v <= 0.)
            || self.slow_cell_s.is_some_and(|v| !v.is_finite() || v < 0.)
        {
            return Err(Error::input(
                "occupancy-um must be finite/positive; slow-cell-s finite/nonnegative",
            ));
        }
        if self.occupancy && self.occupancy_only {
            return Err(Error::input(
                "--occupancy and --occupancy-only are mutually exclusive",
            ));
        }
        if self.profile_jobs.as_ref().is_some_and(|v| {
            v.is_empty() || v.iter().any(|&n| n == 0 || n.checked_mul(8).is_none())
        }) {
            return Err(Error::input(
                "profile-jobs must be a nonempty positive list",
            ));
        }
        if matches!(&self.profile_cell, Some(ProfileCell::Name(s)) if s.is_empty()) {
            return Err(Error::input("profile-cell cannot be empty"));
        }
        if self.profile_cell.is_none()
            && (self.profile_jobs.is_some()
                || self.profile_repeat != 1
                || self.profile_snapshot.is_some()
                || self.profile_snapshot_refresh)
        {
            return Err(Error::input(
                "profile jobs/repeat/snapshot options require a profile cell selector",
            ));
        }
        if self.profile_snapshot_refresh && self.profile_snapshot.is_none() {
            return Err(Error::input(
                "--profile-snapshot-refresh requires --profile-snapshot",
            ));
        }
        if self.profile_cell.is_some()
            && (self.force
                || self.occupancy
                || self.occupancy_only
                || self.occupancy_um.is_some()
                || self.slow_cell_s.is_some())
        {
            return Err(Error::input(
                "cell profiling cannot be combined with force, occupancy or slow-cell-s",
            ));
        }
        Ok(())
    }
    fn wants_occupancy(&self) -> bool {
        self.occupancy || self.occupancy_um.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Reuse,
    OccupancyPresent,
    Build,
    OccupancyOnly,
    Profile,
}

/// Pure policy decision, independently testable without spawning a process.
pub fn decide(options: &IndexOptions, state: &CacheState, has_occupancy: bool) -> Result<Action> {
    options.validate()?;
    if options.profile_cell.is_some() {
        return Ok(Action::Profile);
    }
    if options.occupancy_only {
        if *state != CacheState::Current {
            return Err(Error::new(
                ErrorKind::Cache,
                "--occupancy-only needs a current cache",
            ));
        }
        return Ok(Action::OccupancyOnly);
    }
    if *state == CacheState::Current && !options.force {
        return Ok(if options.wants_occupancy() {
            if has_occupancy {
                Action::OccupancyPresent
            } else {
                Action::OccupancyOnly
            }
        } else {
            Action::Reuse
        });
    }
    if *state != CacheState::Missing && !options.force {
        return Err(Error::new(
            ErrorKind::Cache,
            format!("refusing to replace existing cache: {state:?}; rerun with --force"),
        ));
    }
    Ok(Action::Build)
}

fn arguments(
    source: &Path,
    directory: &Path,
    o: &IndexOptions,
    action: &Action,
) -> Result<Vec<OsString>> {
    let mut a: Vec<OsString> = vec!["vfs".into(), source.as_os_str().to_owned()];
    if *action != Action::Profile {
        a.push(directory.as_os_str().to_owned());
    }
    fn add(a: &mut Vec<OsString>, flag: &str, value: impl ToString) {
        a.extend([flag.into(), value.to_string().into()]);
    }
    add(&mut a, "--jobs", o.jobs);
    if *action == Action::OccupancyOnly {
        a.push("--occupancy-only".into());
        if let Some(v) = o.occupancy_um {
            add(&mut a, "--occupancy-um", v);
        }
        return Ok(a);
    }
    if let Some(v) = o.page_target_mb {
        add(&mut a, "--page-target-mb", v);
    }
    if o.wants_occupancy() {
        a.push("--occupancy".into());
        if let Some(v) = o.occupancy_um {
            add(&mut a, "--occupancy-um", v);
        }
    }
    if !o.lod {
        a.push("--no-lod".into());
    }
    if let Some(v) = o.slow_cell_s {
        add(&mut a, "--slow-cell-s", v);
    }
    if let Some(v) = o.p2_shard_limit_mb {
        add(&mut a, "--p2-shard-limit-mb", v);
    }
    match &o.profile_cell {
        Some(ProfileCell::Name(s)) => add(&mut a, "--profile-cell", s),
        Some(ProfileCell::Index(n)) => add(&mut a, "--profile-cell-ci", n),
        None => (),
    }
    if let Some(v) = &o.profile_jobs {
        add(
            &mut a,
            "--profile-jobs",
            v.iter().map(usize::to_string).collect::<Vec<_>>().join(","),
        );
    }
    if o.profile_repeat != 1 {
        add(&mut a, "--profile-repeat", o.profile_repeat);
    }
    if let Some(p) = &o.profile_snapshot {
        a.extend([
            "--profile-snapshot".into(),
            cache::absolute(p)?.into_os_string(),
        ]);
    }
    if o.profile_snapshot_refresh {
        a.push("--profile-snapshot-refresh".into());
    }
    Ok(a)
}

struct WriteLease(File);
impl WriteLease {
    fn acquire(directory: &Path) -> Result<Self> {
        let mut path = directory.as_os_str().to_owned();
        path.push(".index.lock");
        // Persistent zero-byte inode: unlink-on-unlock would allow a second
        // writer to lock a different inode while the first still holds it.
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(PathBuf::from(path))?;
        let m = f.metadata()?;
        if !m.is_file() || m.nlink() != 1 {
            return Err(Error::input("index lock must be a private regular file"));
        }
        match f.try_lock() {
            Ok(()) => Ok(Self(f)),
            Err(std::fs::TryLockError::WouldBlock) => Err(Error::new(
                ErrorKind::Busy,
                "another Rust application is indexing this cache",
            )),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }
}
impl Drop for WriteLease {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

pub struct PreparedIndex {
    action: Action,
    source: PathBuf,
    directory: PathBuf,
    args: Vec<OsString>,
    indexer: Indexer,
    lease: Option<WriteLease>,
    cleanup_occupancy: bool,
}
impl PreparedIndex {
    pub fn action(&self) -> &Action {
        &self.action
    }
    pub fn source(&self) -> &Path {
        &self.source
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn arguments(&self) -> &[OsString] {
        &self.args
    }
    pub fn prepare(
        source: &Path,
        options: &IndexOptions,
        indexer: Indexer,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        options.validate()?;
        if cancelled.load(Ordering::Relaxed) != 0 {
            return Err(Error::new(
                ErrorKind::Cancelled,
                "index cancelled before preparation",
            ));
        }
        let source = cache::absolute(source)?;
        if source
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("jb"))
        {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "jobdeck indexing is not yet ported (M1a-3); use the existing floe2 index",
            ));
        }
        cache::fingerprint(&source)?;
        indexer.verify(cancelled)?;
        let directory = cache::cache_path(&source)?;
        if let Some(snapshot) = &options.profile_snapshot {
            let snapshot = cache::absolute(snapshot)?;
            let name = snapshot
                .file_name()
                .ok_or_else(|| Error::input("snapshot must name a file"))?;
            let resolved = fs::canonicalize(&snapshot)
                .or_else(|_| {
                    fs::canonicalize(snapshot.parent().expect("absolute file parent"))
                        .map(|p| p.join(name))
                })
                .unwrap_or_else(|_| snapshot.clone());
            let cache_resolved = fs::canonicalize(&directory).unwrap_or_else(|_| directory.clone());
            if snapshot.starts_with(&directory) || resolved.starts_with(&cache_resolved) {
                return Err(Error::input(
                    "profile snapshot must be outside the normal .floe cache",
                ));
            }
        }
        let profiling = options.profile_cell.is_some();
        let lease = if profiling {
            None
        } else {
            Some(WriteLease::acquire(&directory)?)
        };
        if !profiling {
            if let Ok(m) = fs::symlink_metadata(&directory) {
                if !m.is_dir() || m.file_type().is_symlink() {
                    return Err(Error::input(
                        "cache destination must be a real directory, even with --force",
                    ));
                }
            }
        }
        let state = if profiling {
            CacheState::Missing
        } else {
            cache::inspect(&source, &directory)?
        };
        let action = decide(options, &state, directory.join("design.ovo").is_file())?;
        if cancelled.load(Ordering::Relaxed) != 0 {
            return Err(Error::new(
                ErrorKind::Cancelled,
                "index cancelled during cache validation",
            ));
        }
        let args = arguments(&source, &directory, options, &action)?;
        Ok(Self {
            action,
            source,
            directory,
            args,
            indexer,
            lease,
            cleanup_occupancy: !profiling && (options.wants_occupancy() || options.occupancy_only),
        })
    }
    pub fn start(self, cancelled: &AtomicUsize) -> Result<IndexJob> {
        self.start_io(cancelled, false)
    }
    pub fn start_captured(self, cancelled: &AtomicUsize) -> Result<IndexJob> {
        self.start_io(cancelled, true)
    }
    fn start_io(mut self, cancelled: &AtomicUsize, capture: bool) -> Result<IndexJob> {
        if matches!(self.action, Action::Reuse | Action::OccupancyPresent) {
            return Err(Error::input("a reused cache does not launch an indexer"));
        }
        if cancelled.load(Ordering::Relaxed) != 0 {
            return Err(Error::new(
                ErrorKind::Cancelled,
                "index cancelled before launch",
            ));
        }
        let mut child = self.indexer.spawn(&self.args, capture)?;
        let capture = if capture {
            match crate::index_progress::Capture::take(&mut child) {
                Ok(c) => Some(c),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e);
                }
            }
        } else {
            None
        };
        Ok(IndexJob {
            child: Some(child),
            lease: self.lease.take(),
            directory: self.directory.clone(),
            cleanup_occupancy: self.cleanup_occupancy,
            finished: None,
            capture,
        })
    }
}

pub struct IndexJob {
    child: Option<Child>,
    lease: Option<WriteLease>,
    directory: PathBuf,
    cleanup_occupancy: bool,
    finished: Option<i32>,
    capture: Option<crate::index_progress::Capture>,
}
impl IndexJob {
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
    pub fn progress(&self) -> Option<&crate::index_progress::Progress> {
        self.capture.as_ref().map(|c| &c.progress)
    }
    pub fn poll(&mut self) -> Result<Option<i32>> {
        if self.finished.is_some() {
            return Ok(self.finished);
        }
        if let Some(c) = self.capture.as_mut() {
            c.drain()?;
        }
        if let Some(status) = self.child.as_mut().expect("running child").try_wait()? {
            let code = status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(1));
            self.finish(code);
            return Ok(Some(code));
        }
        Ok(None)
    }
    pub fn cancel(&mut self, signal: i32) -> Result<i32> {
        if !matches!(signal, libc::SIGINT | libc::SIGTERM) {
            return Err(Error::input("cancel signal must be SIGINT/SIGTERM"));
        }
        if let Some(code) = self.poll()? {
            return Ok(code);
        }
        native::signal_child(self.child.as_ref().expect("running child"), signal)?;
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if self.child.as_mut().unwrap().try_wait()?.is_some() {
                self.finish(128 + signal);
                return Ok(128 + signal);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let child = self.child.as_mut().unwrap();
        child.kill()?;
        child.wait()?;
        self.finish(128 + signal);
        Ok(128 + signal)
    }
    fn finish(&mut self, code: i32) {
        self.child.take();
        // Telemetry is best-effort. Do not turn the native exit status into a
        // different result because of a late pipe failure during final drain.
        if let Some(c) = self.capture.as_mut() {
            let _ = c.finish();
        }
        self.finished = Some(code);
        if code != 0 {
            if let Err(e) = self.discard_occupancy_tmp() {
                // A cleanup problem must not replace the native failure or
                // cancellation status. Keep the path for explicit recovery.
                eprintln!("[floe2-web] cannot clean occupancy temp: {e}");
            }
        }
        self.lease.take();
    }
    fn discard_occupancy_tmp(&self) -> Result<()> {
        if self.cleanup_occupancy {
            match fs::remove_file(self.directory.join("design.ovo.tmp")) {
                Ok(()) => eprintln!(
                    "[floe2-web] discarded {}",
                    self.directory.join("design.ovo.tmp").display()
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}
impl Drop for IndexJob {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = self.discard_occupancy_tmp();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reuse_force_and_additive_policy() {
        let mut o = IndexOptions::default();
        assert_eq!(
            decide(&o, &CacheState::Missing, false).unwrap(),
            Action::Build
        );
        assert_eq!(
            decide(&o, &CacheState::Current, false).unwrap(),
            Action::Reuse
        );
        let stale = CacheState::Unusable("stale".into());
        assert!(decide(&o, &stale, false).is_err());
        o.occupancy = true;
        assert_eq!(
            decide(&o, &CacheState::Current, false).unwrap(),
            Action::OccupancyOnly
        );
        assert_eq!(
            decide(&o, &CacheState::Current, true).unwrap(),
            Action::OccupancyPresent
        );
        o.force = true;
        assert_eq!(decide(&o, &stale, true).unwrap(), Action::Build);
        o.occupancy = false;
        o.occupancy_only = true;
        assert!(decide(&o, &stale, true).is_err());
        assert_eq!(
            decide(&o, &CacheState::Current, true).unwrap(),
            Action::OccupancyOnly
        );
    }
    #[test]
    fn profile_never_names_cache_and_keeps_job_series_order() {
        let o = IndexOptions {
            profile_cell: Some(ProfileCell::Name("TOP 한 글".into())),
            profile_jobs: Some(vec![16, 1, 16]),
            profile_repeat: 2,
            ..Default::default()
        };
        let a = arguments(
            Path::new("/source"),
            Path::new("/NEVER"),
            &o,
            &Action::Profile,
        )
        .unwrap();
        assert!(!a.contains(&OsString::from("/NEVER")));
        assert!(a.contains(&OsString::from("16,1,16")));
        assert!(a.contains(&OsString::from("--no-lod")));
        assert!(o.validate().is_ok());
    }
    #[test]
    fn rejects_invalid_combinations_and_nonfinite_numbers() {
        let o = IndexOptions {
            occupancy_um: Some(f64::NAN),
            ..Default::default()
        };
        assert!(o.validate().is_err());
        let o = IndexOptions {
            profile_repeat: 2,
            ..Default::default()
        };
        assert!(o.validate().is_err());
        let o = IndexOptions {
            profile_cell: Some(ProfileCell::Index(0)),
            force: true,
            ..Default::default()
        };
        assert!(o.validate().is_err());
    }
}
