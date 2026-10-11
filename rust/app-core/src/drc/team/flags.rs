//! Durable per-reviewer visited / not-required flags, independent of the
//! waive file (floe_oracle drc_review.py, feature/jobdeck-drc 2026-10-09):
//! `.<db>.review.<reviewer>.sqlite3` beside the results (or under
//! FLOE_DRC_REVIEW_ROOT/<key>, or `$XDG_DATA_HOME/floe/review/<key>/` when
//! the results folder cannot be written; key = sha256 of the .db's real
//! path, 20 hex digits), the reference's schema byte for byte:
//!
//! - metadata(id=1, value) = `{"version":1,"fingerprint":"<pack review
//!   fingerprint>","sizes":[<errors per rule>]}` - a file of another DRC
//!   result is refused and left as it is;
//! - totals(id=1, visited, nr, both), rules(ci, visited, nr, both);
//! - chunks(ci, chunk, visited, nr) WITHOUT ROWID: only touched 4096-error
//!   chunks, one bit an error, an empty blob for an empty plane.
//!
//! Rollback journals (shared result folders: no WAL), synchronous FULL;
//! a bitmap change and the three overlap-aware counters commit together.
//! Reads never create the file; a reader waits at most 50 ms for SQLite or
//! for this store's own first creation, and a failed read is repeated as
//! the same error for a second (never shown as "unvisited").
use super::bits::{Bits, CHUNK};
use crate::drc::analysis::scope::Membership;
use crate::{check_cancelled, Error, ErrorKind, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const VISITED: u8 = 1;
pub const NOT_REQUIRED: u8 = 2;
const SCAN: u64 = 65536;
const CACHE_SIZE: usize = 128;
const VERSION: u64 = 1;

/// A rule's or the database's review counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Counts {
    pub total: u64,
    pub visited: u64,
    pub not_required: u64,
    pub visited_not_required: u64,
    /// errors neither visited nor excluded ("unvisited targets")
    pub pending: u64,
}

fn counts(total: u64, (visited, nr, both): (i64, i64, i64)) -> Result<Counts> {
    let total_i = total as i64;
    if !(0 <= both
        && both <= visited.min(nr)
        && visited.max(nr) <= total_i
        && visited + nr - both <= total_i)
    {
        return Err(Error::new(ErrorKind::Cache, "corrupt review flag counters"));
    }
    Ok(Counts {
        total,
        visited: visited as u64,
        not_required: nr as u64,
        visited_not_required: both as u64,
        pending: (total_i - visited - nr + both) as u64,
    })
}

/// A stat that tells an absent file from one we may not see (a
/// permission error is never taken for "no review yet").
pub(crate) fn exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Python's os.path.realpath for a path that may not exist yet.
pub(crate) fn realpath(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) => realpath(dir).join(name),
        _ => path.to_owned(),
    }
}

/// The 20 hex digits naming a database's folder under a review root.
pub(crate) fn path_key(database: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let digest = Sha256::digest(realpath(database).as_os_str().as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect::<String>()[..20].to_owned()
}

pub(crate) fn sanitize(reviewer: &str) -> String {
    let s: String = reviewer
        .chars()
        .map(|c| if c.is_alphanumeric() || "._-".contains(c) { c } else { '_' })
        .collect();
    if s.is_empty() {
        "user".into()
    } else {
        s
    }
}

fn expand_user(root: &str) -> PathBuf {
    if root == "~" || root.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(root.trim_start_matches('~').trim_start_matches('/'));
        }
    }
    PathBuf::from(root)
}

/// The working review flags file of `pack` for `reviewer`.
pub fn review_path(pack: &Path, reviewer: &str) -> Result<PathBuf> {
    let base = crate::cache::database_path(pack)?;
    let db_name = base.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let name = format!(".{db_name}.review.{}.sqlite3", sanitize(reviewer));
    if let Some(root) = std::env::var("FLOE_DRC_REVIEW_ROOT").ok().filter(|r| !r.is_empty()) {
        let root = crate::cache::absolute(&expand_user(&root))?;
        return Ok(root.join(path_key(&base)).join(name));
    }
    Ok(base.with_file_name(name))
}

/// The durable fallback when the results folder cannot be written.
pub fn fallback_path(pack: &Path, reviewer: &str) -> Result<PathBuf> {
    let root = match std::env::var("XDG_DATA_HOME").ok().filter(|r| !r.is_empty()) {
        Some(r) => r,
        None => match std::env::var("HOME").ok().filter(|h| !h.is_empty()) {
            Some(h) => format!("{h}/.local/share"),
            None => {
                return Err(Error::new(
                    ErrorKind::Io,
                    "no durable user-data directory; set FLOE_DRC_REVIEW_ROOT",
                ))
            }
        },
    };
    let base = crate::cache::database_path(pack)?;
    let name = review_path(pack, reviewer)?
        .file_name()
        .map(|n| n.to_owned())
        .unwrap_or_default();
    Ok(crate::cache::absolute(Path::new(&root))?
        .join("floe")
        .join("review")
        .join(path_key(&base))
        .join(name))
}

pub(crate) fn sql(e: rusqlite::Error) -> Error {
    let kind = match &e {
        rusqlite::Error::SqliteFailure(f, _)
            if matches!(
                f.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            ErrorKind::Busy
        }
        _ => ErrorKind::Io,
    };
    Error::new(kind, e.to_string())
}

/// An open/create failure that a missing primary may sidestep to the
/// fallback (the reference's permission tokens).
fn permission_like(e: &Error) -> bool {
    let m = e.message.to_lowercase();
    ["unable to open database", "readonly", "read-only", "permission denied"]
        .iter()
        .any(|t| m.contains(t))
}

pub(crate) fn open_read_only(path: &Path, busy: Duration) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sql)?;
    conn.busy_timeout(busy).map_err(sql)?;
    Ok(conn)
}

struct Reader {
    conn: Option<Connection>,
    cache: VecDeque<((usize, u64), (Bits, Bits))>,
    rules: HashMap<usize, (i64, i64, i64)>,
    totals: (i64, i64, i64),
    version: Option<i64>,
    checked: Option<Instant>,
    missing_until: Option<Instant>,
    error: Option<(Error, Instant)>,
    closed: bool,
    path: PathBuf,
}

/// The store (see the module docs). Shared between the review's request
/// loop and its workers: reads and writes use separate connections, so a
/// write waiting for another process never blocks a page of flags.
pub struct FlagStore {
    pub sizes: Vec<u64>,
    pub total: u64,
    pub fingerprint: String,
    explicit: bool,
    fallback: Option<PathBuf>,
    reader: Mutex<Reader>,
    writer: Mutex<Option<Connection>>,
    /// stderr notices of the reference (fallback in use)
    pub notices: Mutex<Vec<String>>,
}

fn clone_error(e: &Error) -> Error {
    Error::new(e.kind, e.message.clone())
}

impl FlagStore {
    /// The store of `pack` (its rule sizes and review fingerprint) for
    /// `reviewer`; nothing is opened or created yet.
    pub fn new(pack: &Path, sizes: Vec<u64>, fingerprint: String, reviewer: &str) -> Result<Self> {
        let explicit = std::env::var("FLOE_DRC_REVIEW_ROOT").is_ok_and(|r| !r.is_empty());
        let mut path = review_path(pack, reviewer)?;
        let fallback = if explicit { None } else { Some(fallback_path(pack, reviewer)?) };
        let mut notices = Vec::new();
        // An earlier fallback is the durable working copy unless the
        // primary exists; an unreadable primary is never hidden.
        if let Some(f) = &fallback {
            if !exists(&path)? && exists(f)? {
                path = f.clone();
                notices.push(format!(
                    "[drc] using durable review flags in {} (results-folder fallback)",
                    path.display()
                ));
            }
        }
        Ok(Self {
            total: sizes.iter().sum(),
            sizes,
            fingerprint,
            explicit,
            fallback,
            reader: Mutex::new(Reader {
                conn: None,
                cache: VecDeque::new(),
                rules: HashMap::new(),
                totals: (0, 0, 0),
                version: None,
                checked: None,
                missing_until: None,
                error: None,
                closed: false,
                path,
            }),
            writer: Mutex::new(None),
            notices: Mutex::new(notices),
        })
    }

    /// The file holding (or to hold) the flags.
    pub fn path(&self) -> PathBuf {
        self.reader.lock().unwrap().path.clone()
    }

    pub fn metadata(&self) -> String {
        let sizes: Vec<String> = self.sizes.iter().map(u64::to_string).collect();
        format!(
            "{{\"version\":{VERSION},\"fingerprint\":{},\"sizes\":[{}]}}",
            serde_json::Value::from(self.fingerprint.as_str()),
            sizes.join(",")
        )
    }

    fn rule(&self, check: usize) -> Result<u64> {
        self.sizes
            .get(check)
            .copied()
            .ok_or_else(|| Error::input("DRC check index out of range"))
    }

    /// The reader, waiting at most 50 ms (a first write may be creating
    /// the store under it) and repeating a recent read failure.
    fn read_access(&self) -> Result<MutexGuard<'_, Reader>> {
        let start = Instant::now();
        loop {
            match self.reader.try_lock() {
                Ok(r) => {
                    Self::check_error(&r)?;
                    return Ok(r);
                }
                Err(std::sync::TryLockError::Poisoned(p)) => return Ok(p.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => {
                    if start.elapsed() >= Duration::from_millis(50) {
                        return Err(Error::new(
                            ErrorKind::Busy,
                            "review flags temporarily unavailable during initialization",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    fn check_error(r: &Reader) -> Result<()> {
        if r.closed {
            return Err(Error::new(ErrorKind::Io, "review store is closed"));
        }
        if let Some((e, until)) = &r.error {
            if Instant::now() < *until {
                return Err(clone_error(e));
            }
        }
        Ok(())
    }

    fn remember(r: &mut Reader, e: &Error) {
        let now = Instant::now();
        let same = r.error.as_ref().is_some_and(|(o, until)| o.message == e.message && now < *until);
        if !same {
            r.error = Some((clone_error(e), now + Duration::from_secs(1)));
        }
    }

    fn open_path(&self, r: &mut Reader, create: bool) -> Result<bool> {
        let existed = exists(&r.path)?;
        if !create && !existed {
            return Ok(false);
        }
        if create && !existed {
            if let Some(dir) = r.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        if create {
            flags |= OpenFlags::SQLITE_OPEN_CREATE;
        }
        let conn = Connection::open_with_flags(&r.path, flags).map_err(sql)?;
        conn.busy_timeout(Duration::from_millis(if create { 10000 } else { 50 }))
            .map_err(sql)?;
        let setup = || -> Result<()> {
            if !existed && create {
                conn.pragma_update(None, "journal_mode", "DELETE").map_err(sql)?;
                conn.pragma_update(None, "synchronous", "FULL").map_err(sql)?;
                conn.execute_batch(
                    "BEGIN IMMEDIATE;
                     CREATE TABLE IF NOT EXISTS metadata (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL);
                     CREATE TABLE IF NOT EXISTS totals (id INTEGER PRIMARY KEY CHECK(id=1), visited INTEGER NOT NULL, nr INTEGER NOT NULL, both INTEGER NOT NULL);
                     CREATE TABLE IF NOT EXISTS rules (ci INTEGER PRIMARY KEY, visited INTEGER NOT NULL, nr INTEGER NOT NULL, both INTEGER NOT NULL);
                     CREATE TABLE IF NOT EXISTS chunks (ci INTEGER NOT NULL, chunk INTEGER NOT NULL, visited BLOB NOT NULL, nr BLOB NOT NULL, PRIMARY KEY(ci,chunk)) WITHOUT ROWID;",
                )
                .map_err(sql)?;
                conn.execute("INSERT OR IGNORE INTO metadata VALUES(1,?)", [self.metadata()])
                    .map_err(sql)?;
                conn.execute("INSERT OR IGNORE INTO totals VALUES(1,0,0,0)", [])
                    .map_err(sql)?;
                conn.execute_batch("COMMIT").map_err(sql)?;
            }
            // another opener may see the new file just before its creator
            // commits the schema: retry that narrow case only
            let deadline = Instant::now() + Duration::from_secs(1);
            let row: Option<String> = loop {
                match conn
                    .query_row("SELECT value FROM metadata WHERE id=1", [], |r| r.get(0))
                    .optional()
                {
                    Ok(v) => break v,
                    Err(e) => {
                        if !create
                            || !e.to_string().contains("no such table: metadata")
                            || Instant::now() >= deadline
                        {
                            return Err(sql(e));
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            };
            if row.as_deref() != Some(self.metadata().as_str()) {
                return Err(Error::new(
                    ErrorKind::Cache,
                    format!(
                        "{}: review flags belong to another DRC result; existing file preserved",
                        r.path.display()
                    ),
                ));
            }
            conn.busy_timeout(Duration::from_millis(50)).map_err(sql)?;
            Ok(())
        };
        if let Err(e) = setup() {
            if !conn.is_autocommit() {
                let _ = conn.execute_batch("ROLLBACK");
            }
            return Err(e);
        }
        r.conn = Some(conn);
        Self::refresh(r, true)?;
        Ok(true)
    }

    fn connect_attempt(&self, r: &mut Reader, create: bool) -> Result<bool> {
        let existed = exists(&r.path)?;
        match self.open_path(r, create) {
            Ok(v) => Ok(v),
            Err(e) => {
                // only a missing primary that cannot be created may fall
                // back; an existing (even corrupt) file is never bypassed
                let fallback = match &self.fallback {
                    Some(f) if create && !existed && !self.explicit && permission_like(&e) && r.path != *f => f.clone(),
                    _ => return Err(e),
                };
                r.path = fallback;
                let v = self.open_path(r, true)?;
                self.notices.lock().unwrap().push(format!(
                    "[drc] results folder not writable; durable review flags in {}",
                    r.path.display()
                ));
                Ok(v)
            }
        }
    }

    /// The reader connection (None: no file yet and not `create`).
    fn connect(&self, r: &mut Reader, create: bool, probe: bool) -> Result<bool> {
        if r.closed {
            return Err(Error::new(ErrorKind::Io, "review store is closed"));
        }
        if !create {
            Self::check_error(r)?;
        }
        if r.conn.is_some() {
            return Ok(true);
        }
        if !create && !probe && r.missing_until.is_some_and(|t| Instant::now() < t) {
            return Ok(false);
        }
        match self.connect_attempt(r, create) {
            Ok(found) => {
                r.error = None;
                if !found {
                    r.missing_until = Some(Instant::now() + Duration::from_secs(1));
                }
                Ok(found)
            }
            Err(e) => {
                Self::remember(r, &e);
                Err(e)
            }
        }
    }

    fn refresh(r: &mut Reader, force: bool) -> Result<()> {
        let Some(conn) = &r.conn else { return Ok(()) };
        let version: i64 = conn
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .map_err(sql)?;
        if force || Some(version) != r.version {
            let row: Option<(i64, i64, i64)> = conn
                .query_row("SELECT visited,nr,both FROM totals WHERE id=1", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
                .map_err(sql)?;
            let row = row.ok_or_else(|| Error::new(ErrorKind::Cache, "review flags have no totals record"))?;
            r.totals = row;
            r.rules.clear();
            r.cache.clear();
            r.version = Some(version);
        }
        r.checked = Some(Instant::now());
        Ok(())
    }

    fn read_chunk(r: &mut Reader, check: usize, chunk: u64) -> Result<(Bits, Bits)> {
        if let Some(pos) = r.cache.iter().position(|(k, _)| *k == (check, chunk)) {
            let entry = r.cache.remove(pos).unwrap();
            let v = entry.1;
            r.cache.push_back(entry);
            return Ok(v);
        }
        let conn = r.conn.as_ref().expect("connected");
        let row: Option<(Vec<u8>, Vec<u8>)> = conn
            .query_row(
                "SELECT visited,nr FROM chunks WHERE ci=? AND chunk=?",
                params![check as i64, chunk as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sql)?;
        let v = match row {
            None => (Bits::ZERO, Bits::ZERO),
            Some((a, b)) => (
                Bits::from_blob(&a, "review flag chunk")?,
                Bits::from_blob(&b, "review flag chunk")?,
            ),
        };
        r.cache.push_back(((check, chunk), v));
        if r.cache.len() > CACHE_SIZE {
            r.cache.pop_front();
        }
        Ok(v)
    }

    /// Flags (VISITED | NOT_REQUIRED) of rule `check`'s `errors`.
    pub fn flags(&self, check: usize, errors: &[u64]) -> Result<Vec<u8>> {
        let size = self.rule(check)?;
        if errors.iter().any(|&e| e >= size) {
            return Err(Error::input("DRC error index out of range"));
        }
        let mut r = self.read_access()?;
        let result = (|| -> Result<Vec<u8>> {
            if !self.connect(&mut r, false, false)? {
                return Ok(vec![0; errors.len()]);
            }
            if r.checked.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) {
                Self::refresh(&mut r, false)?;
            }
            errors
                .iter()
                .map(|&e| {
                    let (v, n) = Self::read_chunk(&mut r, check, e / CHUNK)?;
                    let i = e % CHUNK;
                    Ok(if v.get(i) { VISITED } else { 0 } | if n.get(i) { NOT_REQUIRED } else { 0 })
                })
                .collect()
        })();
        if let Err(e) = &result {
            Self::remember(&mut r, e);
        }
        result
    }

    /// The counts of rule `check`, or of the whole database.
    pub fn counts(&self, check: Option<usize>) -> Result<Counts> {
        let size = match check {
            Some(c) => self.rule(c)?,
            None => self.total,
        };
        let mut r = self.read_access()?;
        let result = (|| -> Result<Counts> {
            if self.connect(&mut r, false, true)? {
                Self::refresh(&mut r, false)?;
                if let Some(c) = check {
                    if !r.rules.contains_key(&c) {
                        let row: Option<(i64, i64, i64)> = r
                            .conn
                            .as_ref()
                            .unwrap()
                            .query_row(
                                "SELECT visited,nr,both FROM rules WHERE ci=?",
                                [c as i64],
                                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                            )
                            .optional()
                            .map_err(sql)?;
                        r.rules.insert(c, row.unwrap_or((0, 0, 0)));
                    }
                }
            }
            counts(
                size,
                match check {
                    None => r.totals,
                    Some(c) => r.rules.get(&c).copied().unwrap_or((0, 0, 0)),
                },
            )
        })();
        if let Err(e) = &result {
            Self::remember(&mut r, e);
        }
        result
    }

    /// The (visited, not-required) planes of chunk `chunk` as committed now
    /// (no cache): application decisions read under the write lock.
    pub fn chunk_fresh(&self, check: usize, chunk: u64) -> Result<(Bits, Bits)> {
        let mut r = self.reader.lock().unwrap();
        if !self.connect(&mut r, false, true)? {
            return Ok((Bits::ZERO, Bits::ZERO));
        }
        r.cache.retain(|(k, _)| *k != (check, chunk));
        Self::read_chunk(&mut r, check, chunk)
    }

    /// Whether the store's file exists (opening it for reads).
    pub fn present(&self) -> Result<bool> {
        let mut r = self.reader.lock().unwrap();
        self.connect(&mut r, false, true)
    }

    /// Hold the write lock: no change of this process commits meanwhile
    /// (the reference's `store._write_lock` around export and apply).
    pub fn write_guard(&self) -> WriteGuard<'_> {
        WriteGuard(self.writer.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Set (`enabled`) or clear `flag` for the bits of `edits` (chunk,
    /// bits) of rule `check` in one transaction; the changed count.
    pub fn change_many(
        &self,
        guard: &mut WriteGuard<'_>,
        check: usize,
        edits: &[(u64, Bits)],
        flag: u8,
        enabled: bool,
    ) -> Result<u64> {
        let edits: Vec<_> = edits.iter().filter(|(_, b)| !b.is_zero()).collect();
        if edits.is_empty() {
            return Ok(0);
        }
        let path = {
            let mut r = self.reader.lock().unwrap();
            self.connect(&mut r, true, false)?;
            r.path.clone()
        };
        if guard.0.is_none() {
            let conn = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(sql)?;
            conn.busy_timeout(Duration::from_secs(10)).map_err(sql)?;
            conn.pragma_update(None, "synchronous", "FULL").map_err(sql)?;
            *guard.0 = Some(conn);
        }
        let conn = guard.0.as_ref().unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").map_err(sql)?;
        let slot = usize::from(flag != VISITED);
        let work = || -> Result<(u64, Option<((i64, i64, i64), (i64, i64, i64))>)> {
            let mut changed = 0u64;
            let mut delta = [0i64; 3];
            for (chunk, bits) in &edits {
                // another process may have edited the chunk: read under
                // SQLite's write lock, not from the cache
                let row: Option<(Vec<u8>, Vec<u8>)> = conn
                    .query_row(
                        "SELECT visited,nr FROM chunks WHERE ci=? AND chunk=?",
                        params![check as i64, *chunk as i64],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(sql)?;
                let old = match row {
                    None => [Bits::ZERO, Bits::ZERO],
                    Some((a, b)) => [
                        Bits::from_blob(&a, "review flag chunk")?,
                        Bits::from_blob(&b, "review flag chunk")?,
                    ],
                };
                let mut new = old;
                new[slot] = if enabled { old[slot] | *bits } else { old[slot] & !*bits };
                let here = (old[slot] ^ new[slot]).count();
                if here == 0 {
                    continue;
                }
                changed += here;
                let before = [old[0].count(), old[1].count(), (old[0] & old[1]).count()];
                let after = [new[0].count(), new[1].count(), (new[0] & new[1]).count()];
                for k in 0..3 {
                    delta[k] += after[k] as i64 - before[k] as i64;
                }
                if !new[0].is_zero() || !new[1].is_zero() {
                    conn.execute(
                        "INSERT OR REPLACE INTO chunks VALUES(?,?,?,?)",
                        params![check as i64, *chunk as i64, new[0].blob(), new[1].blob()],
                    )
                    .map_err(sql)?;
                } else {
                    conn.execute(
                        "DELETE FROM chunks WHERE ci=? AND chunk=?",
                        params![check as i64, *chunk as i64],
                    )
                    .map_err(sql)?;
                }
            }
            if changed == 0 {
                return Ok((0, None));
            }
            conn.execute("INSERT OR IGNORE INTO rules VALUES(?,0,0,0)", [check as i64])
                .map_err(sql)?;
            conn.execute(
                "UPDATE rules SET visited=visited+?,nr=nr+?,both=both+? WHERE ci=?",
                params![delta[0], delta[1], delta[2], check as i64],
            )
            .map_err(sql)?;
            conn.execute(
                "UPDATE totals SET visited=visited+?,nr=nr+?,both=both+? WHERE id=1",
                params![delta[0], delta[1], delta[2]],
            )
            .map_err(sql)?;
            // counters may have advanced through another connection
            let three = |row: &rusqlite::Row| Ok((row.get(0)?, row.get(1)?, row.get(2)?));
            let current = conn
                .query_row("SELECT visited,nr,both FROM rules WHERE ci=?", [check as i64], three)
                .map_err(sql)?;
            let totals = conn
                .query_row("SELECT visited,nr,both FROM totals WHERE id=1", [], three)
                .map_err(sql)?;
            Ok((changed, Some((current, totals))))
        };
        let (changed, after) = match work().and_then(|v| {
            conn.execute_batch("COMMIT").map_err(sql)?;
            Ok(v)
        }) {
            Ok(v) => v,
            Err(e) => {
                if !conn.is_autocommit() {
                    let _ = conn.execute_batch("ROLLBACK");
                }
                return Err(e);
            }
        };
        let mut r = self.reader.lock().unwrap();
        r.error = None;
        if let Some((current, totals)) = after {
            // a later commit of another process: the next read takes a
            // fresh snapshot rather than these values
            r.cache.clear();
            r.version = None;
            r.checked = None;
            r.rules.insert(check, current);
            r.totals = totals;
        }
        Ok(changed)
    }

    /// Record one error as visited (its geometry was drawn).
    pub fn mark_visited(&self, check: usize, error: u64) -> Result<u64> {
        let size = self.rule(check)?;
        if error >= size {
            return Err(Error::input("DRC error index out of range"));
        }
        let mut b = Bits::ZERO;
        b.set(error % CHUNK);
        let mut g = self.write_guard();
        self.change_many(&mut g, check, &[(error / CHUNK, b)], VISITED, true)
    }

    /// Set or clear not-required for the members of `members` (None: the
    /// whole rule), 65536 errors a transaction; committed chunks stay when
    /// cancelled. `progress(done, total, changed)`. The changed count.
    pub fn set_members(
        &self,
        check: usize,
        members: Option<&dyn Membership>,
        not_required: bool,
        cancelled: &AtomicUsize,
        progress: &mut dyn FnMut(u64, u64, u64),
    ) -> Result<u64> {
        let size = self.rule(check)?;
        let (mut changed, mut start) = (0u64, 0u64);
        let mut mask = vec![false; SCAN as usize];
        while start < size {
            check_cancelled(cancelled)?;
            let count = (size - start).min(SCAN);
            let mask = &mut mask[..count as usize];
            match members {
                Some(m) => m.mask(start, mask),
                None => mask.fill(true),
            }
            let mut edits = Vec::new();
            let mut offset = 0u64;
            while offset < count {
                let n = (count - offset).min(CHUNK);
                let bits = Bits::from_mask(&mask[offset as usize..(offset + n) as usize]);
                if !bits.is_zero() {
                    edits.push(((start + offset) / CHUNK, bits));
                }
                offset += n;
            }
            check_cancelled(cancelled)?;
            let mut g = self.write_guard();
            changed += self.change_many(&mut g, check, &edits, NOT_REQUIRED, not_required)?;
            drop(g);
            start += count;
            progress(start, size, changed);
        }
        if size == 0 {
            progress(0, 0, 0);
        }
        Ok(changed)
    }

    /// Close both connections; later calls fail.
    pub fn close(&self) {
        let mut w = self.writer.lock().unwrap_or_else(|p| p.into_inner());
        let mut r = self.reader.lock().unwrap_or_else(|p| p.into_inner());
        r.closed = true;
        *w = None;
        r.conn = None;
        r.cache.clear();
    }
}

/// The store's write lock and its connection.
pub struct WriteGuard<'a>(MutexGuard<'a, Option<Connection>>);
