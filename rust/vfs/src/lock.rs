//! Locks on what the indexer writes and the renderer reads, between
//! processes, users and hosts (user 2026-10-09: "if the same file is being
//! indexed or used, a run must be refused - whoever runs it";
//! docs/CACHE-NAMING.ko.md "잠금").
//!
//! Kernel advisory locks (flock: std `File::try_lock`; floe/indexlock.py
//! takes the same locks through `fcntl.flock`), released by the kernel when
//! the holder exits or is killed - there is no stale lock to clear. Each
//! target has two lock files in `.floe-lock/` beside it, named by its key
//! (`X.oas.vfs`, `X.db.pack`, `deck.cal.rules` - the legacy names of a
//! target share its key):
//! - `<key>.build`: exclusive for every run that writes the target; the
//!   holder writes who it is into it;
//! - `<key>.use`: shared for the readers, exclusive for a run that rebuilds
//!   the target whole.
//!
//! Neither file is ever deleted (a new file would be a second lock). A
//! reader registers in `use.<host>.<pid>.<n>` - who it is and the keys it
//! holds, the file exclusive-locked by its owner, so one whose lock can be
//! taken is a dead owner's - and a refused writer names the people in its
//! way. Locks are never waited for: a busy target is refused (BUSY_EXIT)
//! after a retry of about RETRY that rides out another process's probe.
//! FLOE_LOCK=off turns it all off.

use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The lock folder beside a target.
pub const DIR: &str = ".floe-lock";
/// The exit status of a run refused for a busy target (EX_TEMPFAIL).
pub const BUSY_EXIT: i32 = 75;
/// Prefix of an open error that is a busy target (renderd answers it as
/// `error code=locked`).
pub const LOCKED: &str = "locked: ";
const RETRY: Duration = Duration::from_millis(1500);
const RETRY_STEP: Duration = Duration::from_millis(50);

/// What a target is: a layout's VFS cache, a DRC results pack, a rule
/// deck's sidecar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Vfs,
    Pack,
    Rules,
}

/// A target's lock: its `.floe-lock` folder and name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    /// the `.floe-lock` folder beside the target
    pub dir: PathBuf,
    /// the target's lock name (`X.oas.vfs`)
    pub name: String,
    /// what messages call the target (`X.oas`, `X.db`, the rules file)
    pub subject: String,
}

/// The key of the target at `target` (a cache folder, a pack, a rules
/// file): the source's name and the kind, so a legacy name (`X.oas.floe`,
/// `X.db.ice`) shares the lock of the current one (floe/indexlock.py `key`).
pub fn key(kind: Kind, target: &str) -> Key {
    let path = Path::new(target);
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| target.to_string());
    let folder = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let strip = |pre: &str, suf: &str| -> Option<String> {
        base.strip_prefix(pre)
            .and_then(|rest| rest.strip_suffix(suf))
            .filter(|n| !n.is_empty())
            .map(str::to_string)
    };
    let (subject, name) = match kind {
        Kind::Vfs => {
            let n = strip(".", ".ice").or_else(|| strip("", ".floe")).unwrap_or_else(|| base.clone());
            (n.clone(), format!("{n}.vfs"))
        }
        Kind::Pack => {
            let n = strip(".", ".tray").or_else(|| strip("", ".ice")).unwrap_or_else(|| base.clone());
            (n.clone(), format!("{n}.pack"))
        }
        Kind::Rules => {
            let name = base
                .strip_suffix(".json")
                .filter(|n| n.ends_with(".rules"))
                .map(str::to_string)
                .unwrap_or_else(|| format!("{base}.rules"));
            (base.clone(), name)
        }
    };
    Key { dir: folder.join(DIR), name, subject }
}

/// Who holds a lock (floe/indexlock.py `holder`): the reviewer (accounts are
/// shared on the servers - FLOE_REVIEWER, else the account), where they sit
/// (their DISPLAY host or ssh client), the server, the process, since when
/// and what it runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holder {
    pub who: String,
    pub from: String,
    pub host: String,
    pub pid: u32,
    pub since: String,
    pub what: String,
}

fn one_line(s: &str) -> String {
    s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect::<String>().trim().to_string()
}

impl Holder {
    /// This process, running `what`.
    pub fn me(what: &str) -> Self {
        let env = |name: &str| std::env::var(name).unwrap_or_default().trim().to_string();
        let mut who = env("FLOE_REVIEWER");
        if who.is_empty() {
            who = env("USER");
        }
        if who.is_empty() {
            who = env("LOGNAME");
        }
        if who.is_empty() {
            who = format!("uid {}", unsafe { libc::getuid() });
        }
        let display = env("DISPLAY");
        let display_host = display.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or_default();
        let mut from = String::new();
        if !display_host.is_empty()
            && !display_host.starts_with('/')
            && !matches!(display_host.as_str(), "localhost" | "127.0.0.1" | "::1" | "unix")
        {
            from = display_host;
        }
        if from.is_empty() {
            let ssh = env("SSH_CONNECTION");
            let ssh = if ssh.is_empty() { env("SSH_CLIENT") } else { ssh };
            from = ssh.split_whitespace().next().unwrap_or_default().to_string();
        }
        Holder {
            who: one_line(&who),
            from: one_line(&from),
            host: hostname(),
            pid: std::process::id(),
            since: local_now(),
            what: one_line(what),
        }
    }

    fn to_text(&self, keys: &[&str]) -> String {
        let mut text = format!(
            "who={}\nfrom={}\nhost={}\npid={}\nsince={}\nwhat={}\n",
            self.who, self.from, self.host, self.pid, self.since, self.what
        );
        for k in keys {
            text.push_str(&format!("key={k}\n"));
        }
        text
    }

    fn parse(text: &str) -> (Holder, Vec<String>) {
        let mut h = Holder::default();
        let mut keys = Vec::new();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k {
                "who" => h.who = v.to_string(),
                "from" => h.from = v.to_string(),
                "host" => h.host = v.to_string(),
                "pid" => h.pid = v.parse().unwrap_or(0),
                "since" => h.since = v.to_string(),
                "what" => h.what = v.to_string(),
                "key" => keys.push(v.to_string()),
                _ => {}
            }
        }
        (h, keys)
    }
}

impl fmt::Display for Holder {
    /// `kim (from 10.1.2.3) on srv02, pid 4242, since 2026-10-09 15:20:11
    /// (floe-index vfs)` - floe/indexlock.py `describe` says it alike
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", if self.who.is_empty() { "someone" } else { &self.who })?;
        if !self.from.is_empty() && self.from != self.who {
            write!(f, " (from {})", self.from)?;
        }
        if !self.host.is_empty() {
            write!(f, " on {}", self.host)?;
        }
        if self.pid != 0 {
            write!(f, ", pid {}", self.pid)?;
        }
        if !self.since.is_empty() {
            write!(f, ", since {}", self.since)?;
        }
        if !self.what.is_empty() {
            write!(f, " ({})", self.what)?;
        }
        Ok(())
    }
}

/// Why a lock was not taken.
#[derive(Debug)]
pub enum Busy {
    /// a run writes the target (its `.build` is held); `opening`: a reader
    /// was refused
    Indexing { subject: String, holder: Option<Holder>, opening: bool },
    /// readers hold the target and the run would pull it from under them;
    /// `away`: only those on other hosts count (an additive write)
    InUse { subject: String, users: Vec<Holder>, away: bool },
    /// the lock itself could not be taken
    Error { subject: String, message: String },
}

fn users_text(users: &[Holder]) -> String {
    if users.is_empty() {
        return "a viewer that did not say who it is".to_string();
    }
    let shown: Vec<String> = users.iter().take(3).map(|h| h.to_string()).collect();
    let more = users.len().saturating_sub(3);
    if more > 0 {
        format!("{} and {} more", shown.join("; "), more)
    } else {
        shown.join("; ")
    }
}

impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Busy::Indexing { subject, holder, opening } => {
                let by = holder.as_ref().map(|h| h.to_string()).unwrap_or_else(|| "another process".to_string());
                if *opening {
                    write!(f, "{subject} is being indexed by {by} - open it when the index is done")
                } else {
                    write!(f, "{subject} is being indexed by {by} - try again when it finishes")
                }
            }
            Busy::InUse { subject, users, away: false } => write!(
                f,
                "{subject} is in use by {} - re-indexing it would pull it from under them; close it there first",
                users_text(users)
            ),
            Busy::InUse { subject, users, away: true } => write!(
                f,
                "{subject} is in use on another host by {} - adding to its index from this host would pull files from under that host; close it there first, or run this on that host",
                users_text(users)
            ),
            Busy::Error { subject, message } => write!(
                f,
                "{subject}: cannot take its lock: {message} (FLOE_LOCK=off runs without the lock)"
            ),
        }
    }
}

/// What a reader says it runs: FLOE_LOCK_WHAT when the front end that
/// started it set one (`floe2 view`, `floe2 render`), else `default`. A
/// writer names itself (`floe-index vfs`): it may be started by a viewer.
pub fn reader_label(default: &str) -> String {
    let named = std::env::var("FLOE_LOCK_WHAT").unwrap_or_default();
    if named.trim().is_empty() { default.to_string() } else { named.trim().to_string() }
}

/// FLOE_LOCK=off: no lock is taken (the kill switch).
pub fn disabled() -> bool {
    matches!(
        std::env::var("FLOE_LOCK").unwrap_or_default().trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "no"
    )
}

fn retry_window() -> Duration {
    std::env::var("FLOE_LOCK_RETRY_MS").ok().and_then(|v| v.trim().parse().ok()).map(Duration::from_millis).unwrap_or(RETRY)
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) } == 0;
    if !ok {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

fn local_now() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

fn file_safe(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect()
}

/// The permission bits of the folder the lock folder sits in: the lock
/// folder and files take its group/other write bits (and setgid), so who
/// may write targets there may lock them - never wider than the folder.
fn folder_mode(dir: &Path) -> u32 {
    use std::os::unix::fs::MetadataExt;
    dir.parent()
        .and_then(|p| std::fs::metadata(if p.as_os_str().is_empty() { Path::new(".") } else { p }).ok())
        .map(|m| m.mode() & 0o7777)
        .unwrap_or(0o755)
}

fn dir_mode(folder: u32) -> u32 {
    let shared = folder & 0o022;
    0o755 | shared | (folder & 0o2000) | if shared != 0 { 0o1000 } else { 0 }
}

fn file_mode(folder: u32) -> u32 {
    0o644 | (folder & 0o022)
}

fn ensure_dir(dir: &Path, folder: u32) -> std::io::Result<()> {
    match std::fs::create_dir(dir) {
        Ok(()) => {
            // the umask narrowed it: the folder's bits it is
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(dir_mode(folder)));
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
        Err(e) => Err(e),
    }
}

/// A lock file opened for writing (an exclusive lock needs it on NFS),
/// made with the folder's bits when new.
fn open_rw(path: &Path, folder: u32) -> std::io::Result<File> {
    match OpenOptions::new().read(true).write(true).create_new(true).mode(file_mode(folder)).open(path) {
        Ok(f) => {
            let _ = f.set_permissions(std::fs::Permissions::from_mode(file_mode(folder)));
            Ok(f)
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => OpenOptions::new().read(true).write(true).open(path),
        Err(e) => Err(e),
    }
}

/// A lock file opened for a shared lock: read-only (a user who may not
/// write the folder may still read), made when missing and possible.
fn open_ro(path: &Path, folder: u32) -> std::io::Result<File> {
    match File::open(path) {
        Ok(f) => Ok(f),
        Err(e) if e.kind() == ErrorKind::NotFound => open_rw(path, folder),
        Err(e) => Err(e),
    }
}

enum Got {
    Locked,
    Busy,
    Unsupported,
    Failed(std::io::Error),
}

fn unsupported(e: &std::io::Error) -> bool {
    e.kind() == ErrorKind::Unsupported
        || matches!(e.raw_os_error(), Some(code) if code == libc::ENOLCK || code == libc::EOPNOTSUPP || code == libc::ENOSYS)
}

fn lock_retry(file: &File, exclusive: bool) -> Got {
    let started = Instant::now();
    let window = retry_window();
    loop {
        let tried = if exclusive { file.try_lock() } else { file.try_lock_shared() };
        match tried {
            Ok(()) => return Got::Locked,
            Err(TryLockError::WouldBlock) => {
                if started.elapsed() >= window {
                    return Got::Busy;
                }
                std::thread::sleep(RETRY_STEP);
            }
            Err(TryLockError::Error(e)) if unsupported(&e) => return Got::Unsupported,
            Err(TryLockError::Error(e)) => return Got::Failed(e),
        }
    }
}

static WARNED_UNSUPPORTED: AtomicBool = AtomicBool::new(false);
static WARNED_FILES: AtomicBool = AtomicBool::new(false);
static WARNED_MOUNT: AtomicBool = AtomicBool::new(false);

fn warn_unsupported(dir: &Path) {
    if !WARNED_UNSUPPORTED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "[lock] file locks are not supported on {} - another run on the same files is not refused",
            dir.display()
        );
    }
}

/// An NFS mount without network locks holds locks on this host only: said
/// once (Linux, /proc/mounts).
fn warn_local_locks(dir: &Path) {
    if WARNED_MOUNT.load(Ordering::Relaxed) {
        return;
    }
    let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else { return };
    let Ok(real) = std::fs::canonicalize(dir) else { return };
    let mut best: Option<(usize, String, String, String)> = None;
    for line in mounts.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let point = f[1].replace("\\040", " ");
        if real.starts_with(&point) && best.as_ref().map(|b| point.len() > b.0).unwrap_or(true) {
            best = Some((point.len(), point, f[2].to_string(), f[3].to_string()));
        }
    }
    if let Some((_, point, fstype, opts)) = best {
        let local = opts.split(',').find(|o| *o == "nolock" || *o == "local_lock=flock" || *o == "local_lock=all");
        if fstype.starts_with("nfs") {
            if let Some(opt) = local {
                if !WARNED_MOUNT.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "[lock] {point} ({fstype}) is mounted with {opt}: locks hold on this host only - a run on another host is not refused"
                    );
                }
            }
        }
    }
}

fn read_text(path: &Path) -> Option<String> {
    let mut text = String::new();
    File::open(path).ok()?.read_to_string(&mut text).ok()?;
    Some(text)
}

fn read_holder(path: &Path) -> Option<Holder> {
    let (h, _) = Holder::parse(&read_text(path)?);
    (!h.who.is_empty() || h.pid != 0).then_some(h)
}

/// The live readers registered in `dir` that hold `name` (a registration
/// whose lock can be taken is a dead owner's: skipped, removed when this
/// user may).
fn users_of(dir: &Path, name: &str) -> Vec<Holder> {
    let mut users = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return users };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| {
                    let n = n.to_string_lossy();
                    n.starts_with("use.") && !n.ends_with(".tmp")
                })
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    for path in paths {
        let Ok(file) = File::open(&path) else { continue };
        match file.try_lock_shared() {
            Ok(()) => {
                drop(file);
                let _ = std::fs::remove_file(&path);
            }
            Err(TryLockError::WouldBlock) => {
                if let Some(text) = read_text(&path) {
                    let (h, keys) = Holder::parse(&text);
                    if keys.iter().any(|k| k == name) {
                        users.push(h);
                    }
                }
            }
            Err(TryLockError::Error(_)) => {}
        }
    }
    users
}

/// How a run writes a target: whole (its readers must be out), or by adding
/// files atomically beside the others (readers on this host stay).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Full,
    Additive,
}

/// A writer's locks: `.build` until dropped, `.use` (a whole rebuild) until
/// dropped or `release_use`.
#[must_use]
#[derive(Default)]
pub struct WriterGuard {
    build: Option<File>,
    use_lock: Option<File>,
}

impl WriterGuard {
    /// Let readers in while the run still writes (a whole build after its
    /// commit marker: the occupancy density it adds next is additive).
    pub fn release_use(&mut self) {
        self.use_lock = None;
    }

    /// Whether the run holds its target's `.use` exclusive.
    pub fn holds_use(&self) -> bool {
        self.use_lock.is_some()
    }
}

/// Take the writer's locks on `key`, running `what`: refused when another
/// run writes it, or (Full) when it has readers, or (Additive) when it has
/// readers on another host - a mapped file replaced by rename from another
/// NFS client is pulled from under them.
pub fn writer(key: &Key, mode: Mode, what: &str) -> Result<WriterGuard, Busy> {
    if disabled() {
        return Ok(WriterGuard::default());
    }
    let folder = folder_mode(&key.dir);
    let error = |message: String| Busy::Error { subject: key.subject.clone(), message };
    ensure_dir(&key.dir, folder).map_err(|e| error(format!("{}: {e}", key.dir.display())))?;
    warn_local_locks(&key.dir);
    let build_path = key.dir.join(format!("{}.build", key.name));
    let mut build = open_rw(&build_path, folder).map_err(|e| {
        error(format!(
            "{}: {e} (its folder must be writable by everyone who indexes here - ask its owner to `chmod g+w` it)",
            build_path.display()
        ))
    })?;
    match lock_retry(&build, true) {
        Got::Locked => {}
        Got::Busy => {
            return Err(Busy::Indexing { subject: key.subject.clone(), holder: read_holder(&build_path), opening: false })
        }
        Got::Unsupported => {
            warn_unsupported(&key.dir);
            return Ok(WriterGuard::default());
        }
        Got::Failed(e) => return Err(error(format!("{}: {e}", build_path.display()))),
    }
    let mut guard = WriterGuard { build: None, use_lock: None };
    match mode {
        Mode::Full => {
            let use_path = key.dir.join(format!("{}.use", key.name));
            let use_lock = open_rw(&use_path, folder).map_err(|e| error(format!("{}: {e}", use_path.display())))?;
            match lock_retry(&use_lock, true) {
                Got::Locked => guard.use_lock = Some(use_lock),
                Got::Busy => {
                    return Err(Busy::InUse {
                        subject: key.subject.clone(),
                        users: users_of(&key.dir, &key.name),
                        away: false,
                    })
                }
                Got::Unsupported => warn_unsupported(&key.dir),
                Got::Failed(e) => return Err(error(format!("{}: {e}", use_path.display()))),
            }
        }
        Mode::Additive => {
            let here = hostname();
            let away: Vec<Holder> = users_of(&key.dir, &key.name).into_iter().filter(|h| h.host != here).collect();
            if !away.is_empty() {
                return Err(Busy::InUse { subject: key.subject.clone(), users: away, away: true });
            }
        }
    }
    // who holds it, for the runs it refuses (synced: the file stays open)
    let _ = write_over(&mut build, &Holder::me(what).to_text(&[]));
    guard.build = Some(build);
    Ok(guard)
}

fn write_over(file: &mut File, text: &str) -> std::io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(text.as_bytes())?;
    file.sync_data()
}

/// A reader's locks: `.use` shared on each key and one registration per
/// lock folder, until dropped.
#[must_use]
#[derive(Default)]
pub struct ReaderGuard {
    locks: Vec<File>,
    registrations: Vec<(PathBuf, File)>,
}

impl Drop for ReaderGuard {
    fn drop(&mut self) {
        for (path, _) in &self.registrations {
            let _ = std::fs::remove_file(path);
        }
    }
}

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// Register this process in `dir` as holding `names`: written under a
/// temporary name, locked, then renamed in place, so a registration is
/// never seen unlocked while its owner lives (pid reuse cannot fake one).
fn register(dir: &Path, names: &[&str], what: &str) -> Option<(PathBuf, File)> {
    let n = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let base = format!("use.{}.{}.{}", file_safe(&hostname()), std::process::id(), n);
    let tmp = dir.join(format!("{base}.tmp"));
    let path = dir.join(&base);
    let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(true).mode(0o644).open(&tmp).ok()?;
    let done = file.try_lock().is_ok()
        && file.write_all(Holder::me(what).to_text(names).as_bytes()).is_ok()
        && std::fs::rename(&tmp, &path).is_ok();
    if !done {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    let _ = file.sync_data();
    Some((path, file))
}

/// Take a reader's locks on `keys`, running `what`: refused when a run
/// rebuilds one whole. A folder this user cannot lock in (read-only, too
/// many open files) is read unlocked - a reader is never stopped by its
/// own lock.
pub fn readers(keys: &[Key], what: &str) -> Result<ReaderGuard, Busy> {
    let mut guard = ReaderGuard::default();
    if disabled() || keys.is_empty() {
        return Ok(guard);
    }
    // the locks never take the files the caches themselves need: past
    // the open-file limit less a reserve, the rest are opened unlocked
    let room = open_file_room();
    let mut held: Vec<&Key> = Vec::new();
    for key in keys {
        if held.len() >= room {
            if !WARNED_FILES.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "[lock] {} caches over the open-file limit's room for locks ({room}): the rest are opened without their locks",
                    keys.len()
                );
            }
            break;
        }
        let folder = folder_mode(&key.dir);
        if ensure_dir(&key.dir, folder).is_err() {
            continue;
        }
        warn_local_locks(&key.dir);
        let use_path = key.dir.join(format!("{}.use", key.name));
        let file = match open_ro(&use_path, folder) {
            Ok(file) => file,
            Err(e) => {
                if e.raw_os_error() == Some(libc::EMFILE) && !WARNED_FILES.swap(true, Ordering::Relaxed) {
                    eprintln!("[lock] too many open files: the rest of the caches are opened without their locks");
                }
                continue;
            }
        };
        match lock_retry(&file, false) {
            Got::Locked => {
                guard.locks.push(file);
                held.push(key);
            }
            Got::Busy => {
                return Err(Busy::Indexing {
                    subject: key.subject.clone(),
                    holder: read_holder(&key.dir.join(format!("{}.build", key.name))),
                    opening: true,
                })
            }
            Got::Unsupported => warn_unsupported(&key.dir),
            Got::Failed(_) => {}
        }
    }
    let mut dirs: Vec<&Path> = held.iter().map(|k| k.dir.as_path()).collect();
    dirs.sort();
    dirs.dedup();
    for dir in dirs {
        let names: Vec<&str> = held.iter().filter(|k| k.dir == dir).map(|k| k.name.as_str()).collect();
        if let Some(registration) = register(dir, &names, what) {
            guard.registrations.push(registration);
        }
    }
    Ok(guard)
}

/// How many reader locks fit under the open-file limit, leaving a reserve
/// (a quarter of it, 16 to 256 files) for the caches' own files, pipes and
/// threads.
fn open_file_room() -> usize {
    let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 || limit.rlim_cur == libc::RLIM_INFINITY {
        return usize::MAX;
    }
    let cur = limit.rlim_cur as u64;
    let reserve = (cur / 4).clamp(16, 256);
    usize::try_from(cur.saturating_sub(reserve)).unwrap_or(usize::MAX)
}

/// Raise this process's open-file limit to its hard limit: a deck holds a
/// lock per source (a field deck has 667).
pub fn raise_open_file_limit() {
    unsafe {
        let mut limit: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 || limit.rlim_cur >= limit.rlim_max {
            return;
        }
        let want = limit.rlim_max;
        limit.rlim_cur = want;
        if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
            // macOS refuses more than OPEN_MAX even under an unlimited hard
            limit.rlim_cur = want.min(10240);
            let _ = libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("floe-lock-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn keys_name_the_source_and_share_the_legacy_names() {
        let k = key(Kind::Vfs, "/a/b/.X.oas.ice");
        assert_eq!((k.dir.clone(), k.name.as_str(), k.subject.as_str()), (PathBuf::from("/a/b/.floe-lock"), "X.oas.vfs", "X.oas"));
        assert_eq!(key(Kind::Vfs, "/a/b/X.oas.floe"), k);
        assert_eq!(key(Kind::Vfs, "/a/b/out").name, "out.vfs");
        let p = key(Kind::Pack, "/r/.run.db.tray");
        assert_eq!((p.name.as_str(), p.subject.as_str()), ("run.db.pack", "run.db"));
        assert_eq!(key(Kind::Pack, "/r/run.db.ice"), p);
        let r = key(Kind::Rules, "/d/deck.cal.rules.json");
        assert_eq!((r.name.as_str(), r.subject.as_str()), ("deck.cal.rules", "deck.cal.rules.json"));
        assert_eq!(key(Kind::Rules, "/d/x.json").name, "x.json.rules");
        assert_eq!(key(Kind::Vfs, ".X.oas.ice").dir, PathBuf::from("./.floe-lock"));
    }

    #[test]
    fn writers_exclude_writers_and_whole_builds_exclude_readers() {
        let dir = scratch("matrix");
        let k = key(Kind::Vfs, dir.join(".X.oas.ice").to_str().unwrap());
        std::env::set_var("FLOE_LOCK_RETRY_MS", "100");
        let w = writer(&k, Mode::Full, "first").expect("a free target");
        // another writer, whole or adding: refused, naming the first
        for mode in [Mode::Full, Mode::Additive] {
            match writer(&k, mode, "second") {
                Err(Busy::Indexing { holder: Some(h), opening: false, .. }) => {
                    assert_eq!((h.pid, h.what.as_str()), (std::process::id(), "first"))
                }
                other => panic!("{mode:?}: {:?}", other.err()),
            }
        }
        // a reader of a target rebuilt whole: refused, "being indexed"
        match readers(&[k.clone()], "viewer") {
            Err(Busy::Indexing { opening: true, .. }) => {}
            other => panic!("{:?}", other.err()),
        }
        let mut w = w;
        w.release_use();
        // after the commit: readers in, writers still out
        let r = readers(&[k.clone()], "viewer").expect("readers after release_use");
        assert!(matches!(writer(&k, Mode::Additive, "third"), Err(Busy::Indexing { .. })));
        drop(w);
        // a reader in: a whole rebuild is refused and names it; an
        // addition from this host goes ahead
        match writer(&k, Mode::Full, "rebuild") {
            Err(Busy::InUse { users, away: false, .. }) => {
                assert_eq!(users.len(), 1);
                assert_eq!((users[0].pid, users[0].what.as_str()), (std::process::id(), "viewer"));
            }
            other => panic!("{:?}", other.err()),
        }
        let added = writer(&k, Mode::Additive, "hier").expect("an addition beside a reader on this host");
        drop(added);
        drop(r);
        // the reader gone: its registration with it, the rebuild goes ahead
        assert!(users_of(&k.dir, &k.name).is_empty());
        drop(writer(&k, Mode::Full, "rebuild").expect("free again"));
        std::env::remove_var("FLOE_LOCK_RETRY_MS");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dead_registration_is_not_a_user_and_another_hosts_reader_stops_an_addition() {
        let dir = scratch("registrations");
        let k = key(Kind::Vfs, dir.join(".Y.oas.ice").to_str().unwrap());
        std::fs::create_dir_all(&k.dir).unwrap();
        let away = Holder { who: "lee".into(), from: "pc-17".into(), host: "elsewhere".into(), pid: 77, since: "x".into(), what: "floe2 view".into() };
        // no lock on it: its owner is gone
        let dead = k.dir.join("use.elsewhere.77.0");
        std::fs::write(&dead, away.to_text(&[&k.name])).unwrap();
        assert!(users_of(&k.dir, &k.name).is_empty());
        assert!(!dead.exists(), "a dead registration is removed by who may");
        // locked: alive - another host's reader stops an addition from here
        let live = k.dir.join("use.elsewhere.78.0");
        std::fs::write(&live, Holder { pid: 78, ..away.clone() }.to_text(&[&k.name])).unwrap();
        let owner = File::open(&live).unwrap();
        owner.try_lock().unwrap();
        std::env::set_var("FLOE_LOCK_RETRY_MS", "100");
        match writer(&k, Mode::Additive, "ovs") {
            Err(Busy::InUse { users, away: true, .. }) => {
                assert_eq!(users.len(), 1);
                let text = Busy::InUse { subject: "Y.oas".into(), users, away: true }.to_string();
                assert!(text.contains("lee (from pc-17) on elsewhere, pid 78"), "{text}");
            }
            other => panic!("{:?}", other.err()),
        }
        drop(owner);
        drop(writer(&k, Mode::Additive, "ovs").expect("the reader gone"));
        std::env::remove_var("FLOE_LOCK_RETRY_MS");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lock_folder_takes_the_folders_write_bits() {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(dir_mode(0o755), 0o755);
        assert_eq!(dir_mode(0o775), 0o1775);
        assert_eq!(dir_mode(0o2775), 0o3775);
        assert_eq!(dir_mode(0o777), 0o1777);
        assert_eq!(file_mode(0o755), 0o644);
        assert_eq!(file_mode(0o775), 0o664);
        let dir = scratch("modes");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o775)).unwrap();
        let k = key(Kind::Pack, dir.join(".z.db.tray").to_str().unwrap());
        drop(writer(&k, Mode::Full, "drc").unwrap());
        assert_eq!(std::fs::metadata(&k.dir).unwrap().mode() & 0o7777, 0o1775);
        assert_eq!(std::fs::metadata(k.dir.join("z.db.pack.build")).unwrap().mode() & 0o777, 0o664);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
