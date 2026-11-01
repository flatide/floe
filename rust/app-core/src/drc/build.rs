//! Explicit pack builds: private native staging, validated atomic publication.
//! Source/pack/review files are never passed to the native cleanup namespace.
use super::{pack::Input, Pack, MAGIC};
use crate::{
    artifact, cache, check_cancelled,
    index::{IndexJob, WriteLease},
    index_progress::Progress,
    managed::Resources,
    native::Indexer,
    registered::AccessScope,
    Error, ErrorKind, Result,
};
use std::{
    fs::{self, DirBuilder, File},
    os::unix::fs::{DirBuilderExt, FileExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub jobs: usize,
    pub force: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            jobs: 12,
            force: false,
        }
    }
}
impl Options {
    pub fn validate(self) -> Result<()> {
        if !(1..=16).contains(&self.jobs) {
            return Err(Error::input("DRC pack jobs must be 1..16"));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Running,
    Validating,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug)]
pub struct Outcome {
    pub reused: bool,
    pub checks: usize,
    pub errors: u64,
    pub bytes: u64,
    /// Rename/link succeeded; a later directory sync failure is NOT cancellation.
    pub directory_synced: bool,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub id: u64,
    pub phase: Phase,
    pub elapsed_ms: u64,
    pub native: Progress,
    pub failure: Option<ErrorKind>,
    pub outcome: Option<Outcome>,
    pub native_pid: Option<u32>,
    pub cleanup_warning: bool,
    pub migration: Option<cache::Migration>,
}
impl Snapshot {
    pub fn terminal(&self) -> bool {
        matches!(
            self.phase,
            Phase::Succeeded | Phase::Failed | Phase::Cancelled
        )
    }
}
struct State {
    snapshot: Snapshot,
    /// why the run failed, in its words (the pack's lock names who is in
    /// the way)
    failure_message: Option<String>,
    // The same mutex serializes cancel and publication. A late cancel must not
    // turn an already published output into a cancelled/ambiguous job result.
    sealed: bool,
}
pub struct Build {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicUsize>,
    started: Instant,
    thread: Option<JoinHandle<()>>,
    output: PathBuf,
}
pub fn output_path(source: &Path) -> Result<PathBuf> {
    let source = cache::absolute(source)?;
    // the work directory's mirror of the database's folder, before the
    // output check canonicalizes it (cache::workdir; the lease that makes
    // it for a cache comes later here)
    cache::ensure_workdir_folder(&source)?;
    let output = cache::pack_paths(&source)?[0].clone();
    artifact::protected_output(&output, &[source], &[])
}
impl Build {
    /// Trusted caller explicitly approves this operation. Browser code must
    /// pass a registered path, never a request-supplied filesystem destination.
    /// Call off the HTTP reactor; scope/key resolution may perform filesystem I/O.
    pub fn start(
        resources: &Arc<Resources>,
        scope: Arc<AccessScope>,
        source: &Path,
        options: Options,
        indexer: Indexer,
    ) -> Result<Self> {
        options.validate()?;
        let source = scope.check(source)?;
        let output = output_path(&source)?;
        scope.check(&output)?;
        let candidates = cache::pack_paths(&source)?;
        for path in &candidates {
            scope.check(path)?;
        }
        // Require the DRC reader to close first; its selections/waives must not
        // silently migrate to the newly built pack's file-order identifiers.
        let permit = resources.index(
            std::iter::once(source.clone()).chain(candidates),
            options.jobs,
        )?;
        let id = resources.next_id()?;
        let started = Instant::now();
        let stop = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(Mutex::new(State {
            sealed: false,
            failure_message: None,
            snapshot: Snapshot {
                id,
                phase: Phase::Preparing,
                elapsed_ms: 0,
                native: Progress::default(),
                failure: None,
                outcome: None,
                native_pid: None,
                cleanup_warning: false,
                migration: None,
            },
        }));
        let (shared, flag, target) = (Arc::clone(&state), Arc::clone(&stop), output.clone());
        let thread = thread::Builder::new()
            .name("floe-drc-build".into())
            .spawn(move || {
                let result = run(&scope, &source, &target, options, &indexer, &flag, &shared);
                // Terminal means validation, child reap, staging cleanup and lease
                // release have finished, independent of progress subscribers.
                drop(permit);
                let mut s = shared.lock().unwrap();
                s.snapshot.native_pid = None;
                s.snapshot.elapsed_ms = millis(started);
                match result {
                    Ok(outcome) => {
                        s.snapshot.outcome = Some(outcome);
                        s.snapshot.phase = Phase::Succeeded;
                    }
                    Err(e) => {
                        eprintln!("[{}] DRC build: {e}", crate::program());
                        s.snapshot.failure = Some(e.kind);
                        s.failure_message = Some(e.message.clone());
                        s.snapshot.phase = if e.kind == ErrorKind::Cancelled {
                            Phase::Cancelled
                        } else {
                            Phase::Failed
                        };
                    }
                }
            })?;
        Ok(Self {
            state,
            stop,
            started,
            thread: Some(thread),
            output,
        })
    }
    pub fn output(&self) -> &Path {
        &self.output
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut s = self.state.lock().unwrap().snapshot.clone();
        if !s.terminal() {
            s.elapsed_ms = millis(self.started);
        }
        s
    }
    /// Why the build failed, in its words (a refused lock says who holds
    /// the pack): None until it has failed.
    pub fn failure_message(&self) -> Option<String> {
        self.state.lock().unwrap().failure_message.clone()
    }
    pub fn cancel(&self) {
        cancel(&self.state, &self.stop);
    }
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }
    pub fn close(&mut self) -> Result<()> {
        self.cancel();
        if let Some(t) = self.thread.take() {
            t.join()
                .map_err(|_| Error::new(ErrorKind::Worker, "DRC build thread panicked"))?;
        }
        Ok(())
    }
}
impl Drop for Build {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
fn millis(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
fn cancel(state: &Mutex<State>, stop: &AtomicUsize) {
    let mut s = state.lock().unwrap();
    if !s.sealed && !s.snapshot.terminal() {
        stop.store(1, Ordering::Relaxed);
        s.snapshot.phase = Phase::Cancelling;
    }
}
fn commit(
    temporary: &Path,
    target: &Path,
    replace: bool,
    state: &Mutex<State>,
    stop: &AtomicUsize,
) -> Result<()> {
    let mut s = state.lock().unwrap();
    check_cancelled(stop)?;
    if replace {
        fs::rename(temporary, target)?;
    } else {
        // Atomic no-clobber even if an uncoordinated writer appears after
        // validation. Staging Drop removes our extra hard link.
        fs::hard_link(temporary, target)?;
    }
    s.sealed = true;
    Ok(())
}
fn phase(state: &Mutex<State>, phase: Phase, stop: &AtomicUsize) -> Result<()> {
    let mut s = state.lock().unwrap();
    check_cancelled(stop)?;
    s.snapshot.phase = phase;
    Ok(())
}
fn existing(path: &Path) -> Result<Option<Input>> {
    match fs::symlink_metadata(path) {
        Ok(m) => {
            if !m.is_file() || m.nlink() != 1 {
                return Err(Error::input(
                    "DRC pack output must be a regular, single-link file",
                ));
            }
            Input::open(path).map(Some)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn unchanged_target(target: &Path, old: Option<&Input>) -> Result<()> {
    if let Some(old) = old {
        return old.unchanged_at(target);
    }
    match fs::symlink_metadata(target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => Err(Error::new(
            ErrorKind::Cache,
            "DRC output appeared during build",
        )),
    }
}
/// The pack's lock key (floe_vfs::lock, kind Pack): `.floe-lock/<db>.pack`
/// beside the pack - in the work directory's mirror under --workdir - the
/// key a `floe-index drc` and the pack's readers (local.rs pack_reader)
/// use; the legacy `<db>.ice` shares it.
fn pack_key(target: &Path) -> floe_vfs::lock::Key {
    floe_vfs::lock::key(floe_vfs::lock::Kind::Pack, &target.to_string_lossy())
}
/// Hold the pack's `.build` for the run: a second build of it - another
/// floe2, a `floe-index drc` - is refused naming this one, and refuses
/// this one (ErrorKind::Busy, exit 75); a lock that cannot be taken is an
/// Io error with what to do (FLOE_LOCK=off runs without it).
fn pack_writer(key: &floe_vfs::lock::Key) -> Result<floe_vfs::lock::WriterGuard> {
    floe_vfs::lock::writer(
        key,
        floe_vfs::lock::Mode::Additive,
        &format!("{} drc --build", crate::program()),
    )
    .map_err(busy_error)
}
/// Hold the pack's readers out (`.use` exclusive) before the run replaces
/// or renames the pack: one holding it - a viewer, a review, a DRC read -
/// refuses the run as in use, and a reader coming in is refused until the
/// run ends.
fn exclude_pack_readers(
    lock: &mut floe_vfs::lock::WriterGuard,
    key: &floe_vfs::lock::Key,
) -> Result<()> {
    lock.exclude_readers(key).map_err(busy_error)
}
fn busy_error(busy: floe_vfs::lock::Busy) -> Error {
    let kind = match busy {
        floe_vfs::lock::Busy::Error { .. } => ErrorKind::Io,
        _ => ErrorKind::Busy,
    };
    Error::new(kind, busy.to_string())
}
fn run(
    scope: &AccessScope,
    source: &Path,
    target: &Path,
    options: Options,
    indexer: &Indexer,
    stop: &AtomicUsize,
    state: &Mutex<State>,
) -> Result<Outcome> {
    check_cancelled(stop)?;
    scope.check(source)?;
    scope.check(target)?;
    let input = Input::open(source)?;
    let mut magic = [0; 8];
    if input.len() >= 8 {
        input.file.read_exact_at(&mut magic, 0)?;
    }
    if &magic == MAGIC {
        return Err(Error::input(
            "DRC build requires an ASCII source, not an ICE pack",
        ));
    }
    let _leases = WriteLease::acquire_aliases(&cache::pack_paths(source)?)?;
    // the pack's own lock (floe_vfs::lock, kind Pack; user 2026-11-01:
    // "pack의 .floe-lock 잠금도 .tray 키로 잡게 고쳐줘"): keyed on the pack
    // - `.r.db.tray` -> `.floe-lock/r.db.pack.*` beside it, in the work
    // directory's mirror under --workdir - as a `floe-index drc` and the
    // pack's readers key theirs. The indexer runs on a private staging path
    // with its own locks off (PRIVATE_TARGET), so until 0.12.427 a floe2
    // build held the app lease alone: a pack's build lock did not refuse
    // it, and it refused neither a `floe-index drc` nor a reader. `.build`
    // is held for the whole run; `.use` exclusive from the point the run
    // replaces or renames the pack (exclude_pack_readers) - a build that
    // reuses the pack as it is leaves its readers be.
    let key = pack_key(target);
    let mut lock = pack_writer(&key)?;
    // output_path resolves the parent but preserves the logical source name.
    // Compare candidates in that same namespace: /var vs /private/var (or any
    // directory symlink) is not a legacy rename. Do not resolve the cache leaf;
    // a symlink there must still be rejected rather than adopted/replaced.
    let selected =
        artifact::protected_output(&cache::pack_path(source)?, &[source.to_owned()], &[])?;
    scope.check(&selected)?;
    let mut old = existing(&selected)?;
    if let Some(previous) = old.as_ref().filter(|_| !options.force) {
        let pack = Pack::open(&selected, stop).map_err(|e| {
            if e.kind == ErrorKind::Cancelled {
                e
            } else {
                Error::new(
                    ErrorKind::Cache,
                    "existing DRC pack is unusable; explicit --force is required",
                )
            }
        })?;
        if !pack.source_matches(source)? {
            return Err(Error::new(
                ErrorKind::Cache,
                "existing DRC pack is stale; explicit --force is required",
            ));
        }
        input.unchanged_at(source)?;
        unchanged_target(&selected, old.as_ref())?;
        if selected != target {
            // the legacy pack is renamed: not under a reader of it
            exclude_pack_readers(&mut lock, &key)?;
        }
        let mut s = state.lock().unwrap();
        check_cancelled(stop)?;
        if selected != target {
            s.snapshot.migration = Some(cache::rename_legacy(
                &selected,
                target,
                &previous.file.metadata()?,
                false,
                stop,
            )?);
        }
        s.sealed = true;
        return Ok(Outcome {
            reused: true,
            checks: pack.checks.len(),
            errors: pack.total,
            bytes: previous.len(),
            directory_synced: s.snapshot.migration.is_none_or(|m| m.directory_synced),
        });
    }
    // the pack is built anew and the one there replaced (a legacy one
    // renamed): its readers must be out, before the indexer's work
    exclude_pack_readers(&mut lock, &key)?;
    indexer.verify(stop)?;
    if selected != target {
        input.unchanged_at(source)?;
        unchanged_target(&selected, old.as_ref())?;
        let expected = old
            .as_ref()
            .ok_or_else(|| Error::new(ErrorKind::Cache, "legacy DRC pack disappeared"))?
            .file
            .metadata()?;
        let mut s = state.lock().unwrap();
        check_cancelled(stop)?;
        s.snapshot.migration = Some(cache::rename_legacy(
            &selected, target, &expected, false, stop,
        )?);
        drop(s);
        // Rename changes ctime. Capture the new stamp, without pretending a
        // subsequent native failure can roll the completed name change back.
        old = existing(target)?;
    }
    let staging = Staging::create(target.parent().expect("absolute target parent"), state)?;
    let temporary = staging.path.join("result.ice");
    phase(state, Phase::Running, stop)?;
    let mut job = IndexJob::captured(
        indexer,
        &[
            "drc".into(),
            source.as_os_str().to_owned(),
            temporary.as_os_str().to_owned(),
            "--jobs".into(),
            options.jobs.to_string().into(),
        ],
        // the staging directory is this build's alone
        crate::index::PRIVATE_TARGET,
    )?;
    state.lock().unwrap().snapshot.native_pid = job.pid();
    loop {
        if stop.load(Ordering::Relaxed) != 0 {
            job.cancel(libc::SIGTERM)?;
            return Err(Error::new(ErrorKind::Cancelled, "DRC pack build cancelled"));
        }
        let code = job.poll()?;
        state.lock().unwrap().snapshot.native = job.progress().cloned().unwrap_or_default();
        match code {
            Some(0) => break,
            Some(_) => {
                return Err(Error::new(
                    ErrorKind::Worker,
                    "native DRC pack build failed",
                ))
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    }
    drop(job);
    state.lock().unwrap().snapshot.native_pid = None;
    phase(state, Phase::Validating, stop)?;
    input.unchanged_at(source)?;
    let pack = Pack::open(&temporary, stop)?;
    if !pack.source_matches(source)? {
        return Err(Error::new(
            ErrorKind::Cache,
            "built DRC fingerprint mismatch",
        ));
    }
    let file = File::open(&temporary)?;
    let mode = old
        .as_ref()
        .map(|f| f.file.metadata().map(|m| m.permissions().mode() & 0o777))
        .transpose()?
        .unwrap_or(0o600);
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.sync_all()?;
    let mut outcome = Outcome {
        reused: false,
        checks: pack.checks.len(),
        errors: pack.total,
        bytes: file.metadata()?.len(),
        directory_synced: false,
    };
    drop(pack);
    scope.check(source)?;
    scope.check(target)?;
    input.unchanged_at(source)?;
    unchanged_target(target, old.as_ref())?;
    commit(&temporary, target, old.is_some(), state, stop)?;
    outcome.directory_synced = File::open(target.parent().unwrap())
        .and_then(|f| f.sync_all())
        .is_ok();
    Ok(outcome)
}

static SERIAL: AtomicU64 = AtomicU64::new(0);
struct Staging<'a> {
    path: PathBuf,
    identity: (u64, u64),
    state: &'a Mutex<State>,
}
impl<'a> Staging<'a> {
    fn create(parent: &Path, state: &'a Mutex<State>) -> Result<Self> {
        for _ in 0..128 {
            let n = SERIAL.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".floe-drc-build-{}-{n}.tmp", std::process::id()));
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => {
                    let m = fs::symlink_metadata(&path)?;
                    return Ok(Self {
                        path,
                        identity: (m.dev(), m.ino()),
                        state,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(Error::input(
            "cannot allocate private DRC staging directory",
        ))
    }
    fn cleanup(&self) -> std::io::Result<()> {
        let m = fs::symlink_metadata(&self.path)?;
        if !m.is_dir() || (m.dev(), m.ino()) != self.identity {
            return Err(std::io::Error::other("DRC staging directory changed"));
        }
        // Native DRC output is flat. Never recursively remove arbitrary trees,
        // sweep a shared <out>.tmp* namespace, or unlink another run's files.
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with("result.ice")
            {
                return Err(std::io::Error::other("unexpected DRC staging entry"));
            }
            fs::remove_file(entry.path())?;
        }
        fs::remove_dir(&self.path)
    }
}
impl Drop for Staging<'_> {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup() {
            self.state.lock().unwrap().snapshot.cleanup_warning = true;
            eprintln!(
                "[{}] retained DRC staging {}: {e}",
                crate::program(),
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests;
