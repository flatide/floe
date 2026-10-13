//! Reviewer snapshots (`.floereview`), the team aggregate and applying a
//! peer's decisions (floe_oracle drc_team.py). A snapshot is a SQLite file
//! written whole and renamed into place:
//!
//! - metadata(id=1, value): `{"type":"floe-review-snapshot","version":1,
//!   "fingerprint","sizes","reviewer","snapshot_id","exported_ns",
//!   "visited_known","waive_known","not_required_known", ...}` (+ the
//!   source of a converted file, or `aggregate` and its `sources`);
//! - rules(ci, total, v, w, n, c, a, done);
//! - chunks(ci, k, v, w, n, c, a blobs, vc..ac counts, ve..ae running
//!   ends) WITHOUT ROWID, with a partial paging index per plane.
//!
//! Planes: visited, waived, not-required, conflict, work (anything to
//! carry). Unknown dimensions of an old waive file or a working review file
//! stay unknown, never false. A conflict needs two visited reviewers with
//! opposite known decisions. Applying protects one's own visited errors
//! with a different decision unless forced; visits are never copied.
use super::bits::{Bits, BYTES, CHUNK};
use super::flags::{exists, open_read_only, realpath, sql, NOT_REQUIRED};
use super::{ascii_json, random_hex, review, stamp, waived_bits, Identity, Own, Progress, TempDir};
use crate::drc::analysis::scope::{Membership, Selection};
use crate::drc::desktop::{STATUS_NONE, STATUS_WAIVED};
use crate::{check_cancelled, Error, ErrorKind, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{BinaryHeap, VecDeque};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const TYPE: &str = "floe-review-snapshot";
const VERSION: i64 = 1;
const PLANES: &str = "vwnca";

fn identity_error() -> Error {
    Error::new(
        ErrorKind::Cache,
        "review source belongs to another DRC result; source preserved",
    )
}

/// A plane's column letter.
pub fn plane(kind: &str) -> Result<char> {
    Ok(match kind {
        "visited" => 'v',
        "waived" => 'w',
        "not_required" => 'n',
        "conflict" | "conflicts" => 'c',
        "work" => 'a',
        _ => return Err(Error::input(format!("unsupported review membership: {kind}"))),
    })
}

/// A snapshot's counts of a rule or of the database.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Counts {
    pub total: u64,
    pub visited: u64,
    pub waived: u64,
    pub not_required: u64,
    pub conflicts: u64,
    pub work: u64,
    pub pending: u64,
    /// an aggregate's reviewers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewers: Option<u64>,
}
impl Counts {
    fn plane(&self, col: char) -> u64 {
        match col {
            'v' => self.visited,
            'w' => self.waived,
            'n' => self.not_required,
            'c' => self.conflicts,
            _ => self.work,
        }
    }
}

/// One error's state in a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub visited: bool,
    pub waived: bool,
    pub not_required: bool,
    pub conflict: bool,
}

/// One stored chunk: rule, chunk and its five planes (v, w, n, c, a).
pub type Chunk = (usize, u64, [Bits; 5]);

fn connect(path: &Path, busy: Duration) -> Result<Connection> {
    open_read_only(path, busy)
}

/// A read-only snapshot (see the module docs).
pub struct Snapshot {
    pub path: PathBuf,
    /// what the user loaded (a converted file's own path)
    pub original_path: PathBuf,
    pub metadata: Value,
    pub fingerprint: String,
    pub sizes: Vec<u64>,
    pub reviewer: String,
    pub snapshot_id: String,
    pub exported_ns: i64,
    pub visited_known: bool,
    pub waive_known: bool,
    pub not_required_known: bool,
    counts: Vec<Counts>,
    totals: Counts,
    conn: Mutex<Option<Connection>>,
    cache: Mutex<VecDeque<((usize, u64), [Bits; 5])>>,
    // dropped after the connection: a converted file's private folder
    _owner: Option<Arc<TempDir>>,
}

impl Snapshot {
    /// Open the snapshot at `path` of the review `id`.
    pub fn open(id: &Identity, path: &Path, owner: Option<Arc<TempDir>>) -> Result<Self> {
        let conn = connect(path, Duration::from_millis(50))?;
        let raw: String = conn
            .query_row("SELECT value FROM metadata WHERE id=1", [], |r| r.get(0))
            .map_err(sql)?;
        let metadata: Value = serde_json::from_str(&raw)
            .map_err(|e| Error::new(ErrorKind::Cache, format!("reviewer snapshot metadata: {e}")))?;
        if metadata.get("type").and_then(Value::as_str) != Some(TYPE)
            || metadata.get("version").and_then(Value::as_i64) != Some(VERSION)
        {
            return Err(Error::input("unsupported reviewer snapshot"));
        }
        check_identity(id, &metadata)?;
        let text = |k: &str| -> Result<String> {
            match metadata.get(k) {
                Some(Value::String(s)) => Ok(s.clone()),
                Some(v) if !v.is_null() => Ok(v.to_string()),
                _ => Err(Error::input(format!("reviewer snapshot lacks {k}"))),
            }
        };
        let known = |k: &str| metadata.get(k).and_then(Value::as_bool).unwrap_or(true);
        let sizes = id.sizes.clone();
        let mut counts: Vec<Option<Counts>> = vec![None; sizes.len()];
        let mut totals = Counts {
            total: id.total(),
            pending: id.total(),
            ..Counts::default()
        };
        {
            let mut st = conn
                .prepare("SELECT ci,total,v,w,n,c,a,done FROM rules")
                .map_err(sql)?;
            let mut rows = st.query([]).map_err(sql)?;
            while let Some(row) = rows.next().map_err(sql)? {
                let v: Vec<i64> = (0..8).map(|i| row.get(i)).collect::<rusqlite::Result<_>>().map_err(sql)?;
                let (ci, total, vv, w, n, c, a, done) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]);
                if ci < 0 || ci as usize >= sizes.len() || total != sizes[ci as usize] as i64 {
                    return Err(Error::input("invalid snapshot rule counters"));
                }
                let pending = total - done;
                if [total, vv, w, n, c, a, pending].iter().any(|&x| x < 0 || x > total) {
                    return Err(Error::input("invalid snapshot counters"));
                }
                if !(vv.max(n) <= done && done <= total.min(vv + n))
                    || !(vv.max(w).max(n).max(c) <= a && a <= total.min(vv + w + n + c))
                    || c > vv
                {
                    return Err(Error::input("inconsistent snapshot counters"));
                }
                let k = Counts {
                    total: total as u64,
                    visited: vv as u64,
                    waived: w as u64,
                    not_required: n as u64,
                    conflicts: c as u64,
                    work: a as u64,
                    pending: pending as u64,
                    reviewers: None,
                };
                totals.visited += k.visited;
                totals.waived += k.waived;
                totals.not_required += k.not_required;
                totals.conflicts += k.conflicts;
                totals.work += k.work;
                totals.pending -= done as u64;
                counts[ci as usize] = Some(k);
            }
        }
        let counts: Vec<Counts> = counts
            .into_iter()
            .collect::<Option<_>>()
            .ok_or_else(|| Error::input("incomplete reviewer snapshot"))?;
        // the paging indexes' last ranks = the counters, without reading
        // the bitmaps
        for col in PLANES.chars() {
            let mut ends = vec![0u64; sizes.len()];
            let mut st = conn
                .prepare(&format!(
                    "SELECT ci,MAX({col}e) FROM chunks WHERE {col}c>0 GROUP BY ci"
                ))
                .map_err(sql)?;
            let mut rows = st.query([]).map_err(sql)?;
            while let Some(row) = rows.next().map_err(sql)? {
                let ci: i64 = row.get(0).map_err(sql)?;
                let end: i64 = row.get(1).map_err(sql)?;
                if ci < 0 || ci as usize >= sizes.len() {
                    return Err(Error::input("snapshot counters do not match their paging indexes"));
                }
                ends[ci as usize] = end as u64;
            }
            if ends.iter().zip(&counts).any(|(e, c)| *e != c.plane(col)) {
                return Err(Error::input("snapshot counters do not match their paging indexes"));
            }
        }
        if metadata.get("aggregate").and_then(Value::as_bool) == Some(true) {
            totals.reviewers = Some(metadata.get("sources").and_then(Value::as_array).map_or(0, |a| a.len() as u64));
        }
        Ok(Self {
            path: path.to_owned(),
            original_path: path.to_owned(),
            fingerprint: text("fingerprint")?,
            sizes,
            reviewer: text("reviewer")?,
            snapshot_id: text("snapshot_id")?,
            exported_ns: metadata
                .get("exported_ns")
                .and_then(Value::as_i64)
                .ok_or_else(|| Error::input("reviewer snapshot lacks exported_ns"))?,
            visited_known: known("visited_known"),
            waive_known: known("waive_known"),
            not_required_known: known("not_required_known"),
            metadata,
            counts,
            totals,
            conn: Mutex::new(Some(conn)),
            cache: Mutex::new(VecDeque::new()),
            _owner: owner,
        })
    }

    pub fn aggregate(&self) -> bool {
        self.metadata.get("aggregate").and_then(Value::as_bool) == Some(true)
    }

    pub fn rule(&self, check: usize) -> Result<u64> {
        self.sizes
            .get(check)
            .copied()
            .ok_or_else(|| Error::input("DRC check index out of range"))
    }

    pub fn counts(&self, check: Option<usize>) -> Result<Counts> {
        let mut c = match check {
            None => return Ok(self.totals),
            Some(ci) => {
                self.rule(ci)?;
                self.counts[ci]
            }
        };
        c.reviewers = self.totals.reviewers;
        Ok(c)
    }

    fn with_conn<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let g = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        match &*g {
            Some(c) => f(c),
            None => Err(Error::new(ErrorKind::Io, "review snapshot is closed")),
        }
    }

    /// The five planes of chunk `k` of rule `check` (zeros when absent).
    pub fn chunk(&self, check: usize, k: u64) -> Result<[Bits; 5]> {
        {
            let mut cache = self.cache.lock().unwrap();
            if let Some(pos) = cache.iter().position(|(key, _)| *key == (check, k)) {
                let e = cache.remove(pos).unwrap();
                let v = e.1;
                cache.push_back(e);
                return Ok(v);
            }
        }
        let v = self.with_conn(|c| {
            let row: Option<Vec<Vec<u8>>> = c
                .query_row(
                    "SELECT v,w,n,c,a FROM chunks WHERE ci=? AND k=?",
                    params![check as i64, k as i64],
                    |r| (0..5).map(|i| r.get(i)).collect(),
                )
                .optional()
                .map_err(sql)?;
            match row {
                None => Ok([Bits::ZERO; 5]),
                Some(b) => {
                    let mut out = [Bits::ZERO; 5];
                    for (o, blob) in out.iter_mut().zip(&b) {
                        *o = Bits::from_blob(blob, "reviewer bitmap")?;
                    }
                    Ok(out)
                }
            }
        })?;
        let mut cache = self.cache.lock().unwrap();
        cache.push_back(((check, k), v));
        if cache.len() > 128 {
            cache.pop_front();
        }
        Ok(v)
    }

    pub fn get(&self, check: usize, error: u64) -> Result<State> {
        let size = self.rule(check)?;
        if error >= size {
            return Err(Error::input("DRC error index out of range"));
        }
        let p = self.chunk(check, error / CHUNK)?;
        let i = error % CHUNK;
        Ok(State {
            visited: p[0].get(i),
            waived: p[1].get(i),
            not_required: p[2].get(i),
            conflict: p[3].get(i),
        })
    }

    /// The members of one plane of a rule.
    pub fn members(&self, check: usize, kind: &str) -> Result<Members<'_>> {
        let col = plane(kind)?;
        let size = self.rule(check)?;
        Ok(Members {
            source: self,
            check,
            size,
            col,
            total: self.counts[check].plane(col),
        })
    }

    /// Every stored chunk in (rule, chunk) order, read by a connection of
    /// its own (a long build never holds the paging connection).
    pub fn chunks(&self, check: Option<usize>) -> Result<ChunkStream> {
        if let Some(c) = check {
            self.rule(c)?;
        }
        ChunkStream::new(&self.path, check)
    }

    /// Close the paging connection (the files stay while shared).
    pub fn close(&self) {
        *self.conn.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.cache.lock().unwrap().clear();
    }
}

fn check_identity(id: &Identity, metadata: &Value) -> Result<()> {
    let fingerprint = metadata.get("fingerprint").and_then(Value::as_str);
    let sizes: Option<Vec<u64>> = metadata
        .get("sizes")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().map(Value::as_u64).collect());
    if fingerprint != Some(id.fingerprint.as_str()) || sizes.as_deref() != Some(&id.sizes[..]) {
        return Err(identity_error());
    }
    Ok(())
}

/// Stored chunks streamed in key order, a page of rows at a time.
pub struct ChunkStream {
    conn: Connection,
    check: Option<usize>,
    after: Option<(i64, i64)>,
    rows: VecDeque<Chunk>,
    done: bool,
}
impl ChunkStream {
    fn new(path: &Path, check: Option<usize>) -> Result<Self> {
        let conn = connect(path, Duration::from_secs(10))?;
        conn.execute_batch("BEGIN").map_err(sql)?;
        Ok(Self {
            conn,
            check,
            after: None,
            rows: VecDeque::new(),
            done: false,
        })
    }
    pub fn next(&mut self) -> Result<Option<Chunk>> {
        if self.rows.is_empty() && !self.done {
            let mut q = String::from("SELECT ci,k,v,w,n,c,a FROM chunks");
            let mut cond = Vec::new();
            if let Some(c) = self.check {
                cond.push(format!("ci={c}"));
            }
            if let Some((ci, k)) = self.after {
                cond.push(format!("(ci,k)>({ci},{k})"));
            }
            if !cond.is_empty() {
                q.push_str(" WHERE ");
                q.push_str(&cond.join(" AND "));
            }
            q.push_str(" ORDER BY ci,k LIMIT 1024");
            let mut st = self.conn.prepare(&q).map_err(sql)?;
            let mut rows = st.query([]).map_err(sql)?;
            while let Some(row) = rows.next().map_err(sql)? {
                let ci: i64 = row.get(0).map_err(sql)?;
                let k: i64 = row.get(1).map_err(sql)?;
                let mut planes = [Bits::ZERO; 5];
                for (i, p) in planes.iter_mut().enumerate() {
                    let blob: Vec<u8> = row.get(2 + i).map_err(sql)?;
                    *p = Bits::from_blob(&blob, "reviewer bitmap")?;
                }
                self.after = Some((ci, k));
                self.rows.push_back((ci as usize, k as u64, planes));
            }
            if self.rows.len() < 1024 {
                self.done = true;
            }
        }
        Ok(self.rows.pop_front())
    }
}

/// One plane of one rule: count, pages (through the plane's prefix
/// index), ranks, masks.
pub struct Members<'a> {
    source: &'a Snapshot,
    pub check: usize,
    pub size: u64,
    col: char,
    pub total: u64,
}
impl Members<'_> {
    pub fn contains(&self, error: u64) -> Result<bool> {
        if error >= self.size {
            return Ok(false);
        }
        let p = self.source.chunk(self.check, error / CHUNK)?;
        Ok(p[PLANES.find(self.col).unwrap()].get(error % CHUNK))
    }
    pub fn contains_many(&self, errors: &[u64]) -> Result<Vec<bool>> {
        errors.iter().map(|&e| self.contains(e)).collect()
    }
    pub fn mask(&self, start: u64, out: &mut [bool]) -> Result<()> {
        out.fill(false);
        let lo = start.min(self.size);
        let hi = (start + out.len() as u64).min(self.size);
        let slot = PLANES.find(self.col).unwrap();
        let mut k = lo / CHUNK;
        while k * CHUNK < hi {
            let bits = self.source.chunk(self.check, k)?[slot];
            if !bits.is_zero() {
                let a = lo.max(k * CHUNK);
                let b = hi.min((k + 1) * CHUNK);
                for e in a..b {
                    out[(e - start) as usize] = bits.get(e - k * CHUNK);
                }
            }
            k += 1;
        }
        Ok(())
    }
    /// Members `start..start+limit` (rule-local error indices).
    pub fn page(&self, start: u64, limit: u64) -> Result<Vec<u64>> {
        if limit == 0 || start >= self.total {
            return Ok(Vec::new());
        }
        let c = self.col;
        self.source.with_conn(|conn| {
            let mut st = conn
                .prepare(&format!(
                    "SELECT k,{c},{c}c,{c}e FROM chunks WHERE ci=? AND {c}c>0 AND {c}e>? ORDER BY {c}e,k"
                ))
                .map_err(sql)?;
            let mut rows = st.query(params![self.check as i64, start as i64]).map_err(sql)?;
            let mut out = Vec::new();
            while (out.len() as u64) < limit {
                let Some(row) = rows.next().map_err(sql)? else { break };
                let k: i64 = row.get(0).map_err(sql)?;
                let blob: Vec<u8> = row.get(1).map_err(sql)?;
                let count: i64 = row.get(2).map_err(sql)?;
                let end: i64 = row.get(3).map_err(sql)?;
                let bits = Bits::from_blob(&blob, "reviewer bitmap")?;
                let skip = (start as i64 - (end - count)).max(0) as usize;
                let want = (limit - out.len() as u64) as usize;
                out.extend(bits.ones().skip(skip).take(want).map(|i| i + k as u64 * CHUNK));
            }
            Ok(out)
        })
    }
    pub fn rank(&self, error: u64) -> Result<Option<u64>> {
        if !self.contains(error)? {
            return Ok(None);
        }
        let c = self.col;
        let (blob, count, end): (Vec<u8>, i64, i64) = self.source.with_conn(|conn| {
            conn.query_row(
                &format!("SELECT {c},{c}c,{c}e FROM chunks WHERE ci=? AND k=?"),
                params![self.check as i64, (error / CHUNK) as i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(sql)
        })?;
        let bits = Bits::from_blob(&blob, "reviewer bitmap")?;
        Ok(Some((end - count) as u64 + bits.count_below(error % CHUNK)))
    }
}

/// A snapshot being written: a temporary file beside the destination,
/// committed, synced and renamed into place by `finish`.
pub struct Writer {
    path: PathBuf,
    temporary: Option<PathBuf>,
    sizes: Vec<u64>,
    metadata: Vec<(String, Value)>,
    conn: Option<Connection>,
    counters: Vec<[u64; 6]>,
}

impl Writer {
    pub fn new(id: &Identity, path: &Path, reviewer: &str, extra: Vec<(String, Value)>) -> Result<Self> {
        let path = crate::cache::absolute(path)?;
        let dir = path.parent().ok_or_else(|| Error::input("snapshot path has no folder"))?.to_owned();
        std::fs::create_dir_all(&dir)?;
        let mut temporary = None;
        for _ in 0..100 {
            let t = dir.join(format!(".review-snapshot-{}.sqlite3", random_hex(4)));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&t) {
                Ok(_) => {
                    temporary = Some(t);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        let temporary = temporary.ok_or_else(|| Error::new(ErrorKind::Io, "no temporary snapshot name"))?;
        let mut id_bytes = [0u8; 16];
        for (i, b) in (0..16).zip(random_hex(16).as_bytes().chunks(2)) {
            id_bytes[i] = u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap();
        }
        id_bytes[6] = (id_bytes[6] & 0x0f) | 0x40;
        id_bytes[8] = (id_bytes[8] & 0x3f) | 0x80;
        let snapshot_id: String = id_bytes.iter().map(|b| format!("{b:02x}")).collect();
        let exported_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let mut metadata: Vec<(String, Value)> = vec![
            ("type".into(), json!(TYPE)),
            ("version".into(), json!(VERSION)),
            ("fingerprint".into(), json!(id.fingerprint)),
            ("sizes".into(), json!(id.sizes)),
            ("reviewer".into(), json!(reviewer)),
            ("snapshot_id".into(), json!(snapshot_id)),
            ("exported_ns".into(), json!(exported_ns)),
            ("visited_known".into(), json!(true)),
            ("waive_known".into(), json!(true)),
            ("not_required_known".into(), json!(true)),
        ];
        for (k, v) in extra {
            match metadata.iter_mut().find(|(key, _)| *key == k) {
                Some(slot) => slot.1 = v,
                None => metadata.push((k, v)),
            }
        }
        let mut w = Self {
            path,
            temporary: Some(temporary.clone()),
            sizes: id.sizes.clone(),
            metadata,
            conn: None,
            counters: vec![[0; 6]; id.sizes.len()],
        };
        let conn = Connection::open_with_flags(
            &temporary,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql)?;
        conn.pragma_update(None, "synchronous", "FULL").map_err(sql)?;
        let mut ddl = String::from(
            "BEGIN;
             CREATE TABLE metadata(id INTEGER PRIMARY KEY,value TEXT NOT NULL);
             CREATE TABLE rules(ci INTEGER PRIMARY KEY,total INTEGER,v INTEGER,w INTEGER,n INTEGER,c INTEGER,a INTEGER,done INTEGER);
             CREATE TABLE chunks(ci INTEGER,k INTEGER,v BLOB,w BLOB,n BLOB,c BLOB,a BLOB,vc INTEGER,wc INTEGER,nc INTEGER,cc INTEGER,ac INTEGER,ve INTEGER,we INTEGER,ne INTEGER,ce INTEGER,ae INTEGER,PRIMARY KEY(ci,k)) WITHOUT ROWID;",
        );
        for c in PLANES.chars() {
            ddl.push_str(&format!("CREATE INDEX page_{c} ON chunks(ci,{c}e,k) WHERE {c}c>0;"));
        }
        conn.execute_batch(&ddl).map_err(sql)?;
        w.conn = Some(conn);
        Ok(w)
    }

    /// Add chunk `k` of rule `check` (chunks of a rule in increasing order);
    /// `work` defaults to v | w | n.
    pub fn add(&mut self, check: usize, k: u64, v: Bits, w: Bits, n: Bits, conflict: Bits, work: Option<Bits>) -> Result<()> {
        let size = *self
            .sizes
            .get(check)
            .ok_or_else(|| Error::input("snapshot chunk is outside its rule"))?;
        if k * CHUNK >= size {
            return Err(Error::input("snapshot chunk is outside its rule"));
        }
        let limit = (size - k * CHUNK).min(CHUNK);
        let values = [v, w, n, conflict, work.unwrap_or(v | w | n)];
        if values.iter().any(|x| x.beyond(limit)) {
            return Err(Error::input("snapshot chunk has out-of-range bits"));
        }
        if values.iter().all(Bits::is_zero) {
            return Ok(());
        }
        let counts: Vec<u64> = values.iter().map(Bits::count).collect();
        let c = &mut self.counters[check];
        for (i, n) in counts.iter().enumerate() {
            c[i] += n;
        }
        c[5] += (v | n).count();
        let ends = [c[0], c[1], c[2], c[3], c[4]];
        let blobs: Vec<Vec<u8>> = values.iter().map(Bits::blob).collect();
        let conn = self.conn.as_ref().expect("open writer");
        let mut st = conn
            .prepare("INSERT INTO chunks VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .map_err(sql)?;
        st.execute(params![
            check as i64, k as i64, blobs[0], blobs[1], blobs[2], blobs[3], blobs[4],
            counts[0] as i64, counts[1] as i64, counts[2] as i64, counts[3] as i64, counts[4] as i64,
            ends[0] as i64, ends[1] as i64, ends[2] as i64, ends[3] as i64, ends[4] as i64
        ])
        .map_err(sql)?;
        Ok(())
    }

    /// Commit, sync and rename the snapshot into place.
    pub fn finish(mut self) -> Result<PathBuf> {
        let conn = self.conn.take().expect("open writer");
        for (ci, size) in self.sizes.iter().enumerate() {
            let c = self.counters[ci];
            conn.execute(
                "INSERT INTO rules VALUES(?,?,?,?,?,?,?,?)",
                params![ci as i64, *size as i64, c[0] as i64, c[1] as i64, c[2] as i64, c[3] as i64, c[4] as i64, c[5] as i64],
            )
            .map_err(sql)?;
        }
        let parts: Vec<String> = self
            .metadata
            .iter()
            .map(|(k, v)| format!("{}:{}", ascii_json(&json!(k)), ascii_json(v)))
            .collect();
        conn.execute("INSERT INTO metadata VALUES(1,?)", [format!("{{{}}}", parts.join(","))])
            .map_err(sql)?;
        conn.execute_batch("COMMIT").map_err(sql)?;
        conn.close().map_err(|(_, e)| sql(e))?;
        let temporary = self.temporary.take().unwrap();
        std::fs::File::open(&temporary)?.sync_all()?;
        std::fs::rename(&temporary, &self.path)?;
        Ok(self.path.clone())
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        self.conn = None;
        if let Some(t) = self.temporary.take() {
            let _ = std::fs::remove_file(t);
        }
    }
}

/// A writer to `path` that may not replace the pack or one's working files.
fn destination(own: &Own, path: &Path, reviewer: &str, extra: Vec<(String, Value)>) -> Result<Writer> {
    let target = realpath(&crate::cache::absolute(path)?);
    let prohibited = [own.id.pack.clone(), own.id.waive_path.clone(), own.flags.path()];
    if prohibited.iter().any(|p| realpath(p) == target) {
        return Err(Error::input(
            "a snapshot cannot replace the DRC pack or working review files",
        ));
    }
    Writer::new(own.id, path, reviewer, extra)
}

fn reviewer_of_file(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if let Some((_, r)) = name.rsplit_once(".review.") {
        return r.strip_suffix(".sqlite3").unwrap_or(r).to_owned();
    }
    if let Some((_, r)) = name.rsplit_once(".waive.") {
        return r.to_owned();
    }
    name
}

/// The waive file beside a working review file, by name.
fn paired_waive(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".sqlite3")?;
    let (base, reviewer) = stem.rsplit_once(".review.")?;
    let candidate = path.with_file_name(format!("{base}.waive.{reviewer}"));
    exists(&candidate).ok()?.then_some(candidate)
}

/// The (visited, nr) planes of a working review's rule, in chunk order.
struct FlagRows<'c> {
    rows: rusqlite::Rows<'c>,
}
impl FlagRows<'_> {
    fn next(&mut self) -> Result<Option<(u64, Bits, Bits)>> {
        match self.rows.next().map_err(sql)? {
            None => Ok(None),
            Some(row) => {
                let k: i64 = row.get(0).map_err(sql)?;
                let v: Vec<u8> = row.get(1).map_err(sql)?;
                let n: Vec<u8> = row.get(2).map_err(sql)?;
                Ok(Some((
                    k as u64,
                    Bits::from_blob(&v, "working review bitmap")?,
                    Bits::from_blob(&n, "working review bitmap")?,
                )))
            }
        }
    }
}

/// Write rule by rule the (v, w, n) planes of a working review (`flags`,
/// a read transaction) and waived statuses (`status`: rule, start, count
/// -> bytes), as export and peer conversion do.
#[allow(clippy::too_many_arguments)]
fn write_planes(
    writer: &mut Writer,
    id: &Identity,
    flags: Option<&Connection>,
    waived_counts: Option<&[u64]>,
    status: Option<&dyn Fn(usize, u64, u64) -> Result<Vec<u8>>>,
    cancelled: &AtomicUsize,
    progress: &mut Progress,
    base: u64,
) -> Result<u64> {
    let mut processed = 0u64;
    for (ci, &size) in id.sizes.iter().enumerate() {
        let mut st = match flags {
            Some(c) => Some(
                c.prepare("SELECT chunk,visited,nr FROM chunks WHERE ci=? ORDER BY chunk")
                    .map_err(sql)?,
            ),
            None => None,
        };
        let mut rows = match &mut st {
            Some(s) => Some(FlagRows {
                rows: s.query([ci as i64]).map_err(sql)?,
            }),
            None => None,
        };
        let next = |rows: &mut Option<FlagRows>| -> Result<Option<(u64, Bits, Bits)>> {
            match rows {
                Some(r) => r.next(),
                None => Ok(None),
            }
        };
        let mut row = next(&mut rows)?;
        let zero = status.is_none() || waived_counts.is_some_and(|w| w[ci] == 0);
        if zero {
            // only the sparse review chunks of a rule with nothing waived
            while let Some((k, v, n)) = row {
                check_cancelled(cancelled)?;
                writer.add(ci, k, v, Bits::ZERO, n, Bits::ZERO, None)?;
                progress.report(base + processed + size.min((k + 1) * CHUNK));
                row = next(&mut rows)?;
            }
            processed += size;
            progress.report(base + processed);
            continue;
        }
        let status = status.unwrap();
        let mut start = 0;
        while start < size {
            check_cancelled(cancelled)?;
            let (k, count) = (start / CHUNK, (size - start).min(CHUNK));
            let (mut v, mut n) = (Bits::ZERO, Bits::ZERO);
            if let Some((rk, rv, rn)) = row {
                if rk == k {
                    (v, n) = (rv, rn);
                    row = next(&mut rows)?;
                }
            }
            let w = waived_bits(&status(ci, start, count)?);
            writer.add(ci, k, v, w, n, Bits::ZERO, None)?;
            processed += count;
            progress.report(base + processed);
            start += count;
        }
    }
    Ok(processed)
}

/// Export one's own visited, waived and not-required together to an
/// atomic snapshot at `path`.
pub fn export(
    own: &Own,
    path: &Path,
    reviewer: Option<&str>,
    cancelled: &AtomicUsize,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<Snapshot> {
    let reviewer = reviewer.unwrap_or(&own.id.reviewer).to_owned();
    let total = own.id.total();
    let mut report = Progress::new(progress, total);
    check_cancelled(cancelled)?;
    let (waived, generation) = own.waived_and_generation()?;
    let guard = own.flags.write_guard();
    let waive_stamp = stamp(&own.id.waive_path)?;
    let flags = if own.flags.present()? {
        let c = open_read_only(&own.flags.path(), Duration::from_secs(10))?;
        c.execute_batch("BEGIN").map_err(sql)?;
        Some(c)
    } else {
        None
    };
    let mut writer = destination(own, path, &reviewer, Vec::new())?;
    let read = |ci: usize, start: u64, count: u64| own.statuses.statuses(ci, start, count);
    let processed = write_planes(
        &mut writer,
        own.id,
        flags.as_ref(),
        Some(&waived),
        Some(&read),
        cancelled,
        &mut report,
        0,
    )?;
    check_cancelled(cancelled)?;
    if stamp(&own.id.waive_path)? != waive_stamp {
        return Err(Error::new(
            ErrorKind::Busy,
            "own waive file changed in another session during export; retry",
        ));
    }
    drop(flags);
    let out = writer.finish()?;
    drop(guard);
    if own.generation()? != generation {
        let _ = std::fs::remove_file(&out);
        return Err(Error::new(ErrorKind::Busy, "own waive statuses changed during export; retry"));
    }
    report.finish(processed);
    Snapshot::open(own.id, &out, None)
}

/// Read a peer's snapshot, working review file (with its waive file beside
/// it) or waive file, unchanged. Missing dimensions stay unknown.
pub fn open_review(
    id: &Identity,
    path: &Path,
    waive_path: Option<&Path>,
    reviewer: Option<&str>,
    cancelled: &AtomicUsize,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<Snapshot> {
    let path = crate::cache::absolute(path)?;
    check_cancelled(cancelled)?;
    let mut magic = [0u8; 16];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(&path)?;
        let n = f.read(&mut magic)?;
        if n < 8 {
            magic[n..].fill(0);
        }
    }
    let mut raw = None;
    if &magic == b"SQLite format 3\0" {
        let conn = connect(&path, Duration::from_secs(10))?;
        let text: String = conn
            .query_row("SELECT value FROM metadata WHERE id=1", [], |r| r.get(0))
            .map_err(sql)?;
        let meta: Value = serde_json::from_str(&text)
            .map_err(|e| Error::new(ErrorKind::Cache, format!("review metadata: {e}")))?;
        if meta.get("type").and_then(Value::as_str) == Some(TYPE) {
            drop(conn);
            if meta.get("aggregate").and_then(Value::as_bool) == Some(true) {
                return Err(Error::input(
                    "load individual reviewer snapshots, not a derived team result",
                ));
            }
            return Snapshot::open(id, &path, None);
        }
        if meta.get("version").and_then(Value::as_i64) != Some(1) {
            return Err(Error::input("unsupported working review database"));
        }
        check_identity(id, &meta)?;
        conn.execute_batch("BEGIN").map_err(sql)?;
        raw = Some(conn);
    } else if !magic.starts_with(b"FLOEWAIV") {
        return Err(Error::input(
            "not a reviewer snapshot, working review database, or waive file",
        ));
    }
    let waive_path: Option<PathBuf> = match (&raw, waive_path) {
        (None, _) => Some(path.clone()),
        (Some(_), Some(w)) => Some(crate::cache::absolute(w)?),
        (Some(_), None) => paired_waive(&path),
    };
    let owner = Arc::new(TempDir::new("floe-peer-")?);
    let output = owner.path().join("snapshot.sqlite3");
    let total = id.total();
    let mut report = Progress::new(progress, total);
    let status_file = match &waive_path {
        Some(w) => {
            let s = stamp(w)?;
            let f = std::fs::File::open(w)?;
            let mut head = [0u8; 40];
            f.read_exact_at(&mut head, 0).ok();
            let size = f.metadata()?.len();
            if head != id.waive_header || size != 40 + total + 4 * id.sizes.len() as u64 {
                return Err(Error::new(ErrorKind::Cache, "waive source does not match this DRC result"));
            }
            Some((f, s))
        }
        None => None,
    };
    let extra = vec![
        ("visited_known".into(), json!(raw.is_some())),
        ("not_required_known".into(), json!(raw.is_some())),
        ("waive_known".into(), json!(status_file.is_some())),
        ("source_path".into(), json!(path)),
        ("source_waive_path".into(), json!(waive_path)),
        (
            "source_format".into(),
            json!(if raw.is_some() { "working-review" } else { "legacy-waive" }),
        ),
    ];
    let name = match reviewer {
        Some(r) => r.to_owned(),
        None => reviewer_of_file(&path),
    };
    let mut writer = Writer::new(id, &output, &name, extra)?;
    let starts = id.starts.clone();
    let read = |ci: usize, start: u64, count: u64| -> Result<Vec<u8>> {
        let (f, _) = status_file.as_ref().unwrap();
        let mut out = vec![0u8; count as usize];
        f.read_exact_at(&mut out, 40 + starts[ci] + start)?;
        Ok(out)
    };
    let processed = write_planes(
        &mut writer,
        id,
        raw.as_ref(),
        None,
        status_file.as_ref().map(|_| &read as &dyn Fn(usize, u64, u64) -> Result<Vec<u8>>),
        cancelled,
        &mut report,
        0,
    )?;
    check_cancelled(cancelled)?;
    if let (Some(w), Some((_, s))) = (&waive_path, &status_file) {
        if stamp(w)? != *s {
            return Err(Error::new(
                ErrorKind::Busy,
                "waive source changed while reading; load a saved snapshot or retry",
            ));
        }
    }
    drop(raw);
    writer.finish()?;
    report.finish(processed);
    let mut snap = Snapshot::open(id, &output, Some(owner))?;
    snap.original_path = path;
    Ok(snap)
}

/// Combine one's own review (exported first, when `include_own`) with the
/// peers' newest snapshot each: visits united, waives and exclusions
/// carried where no visited reviewers disagree; a conflict is visited
/// reviewers with opposite known decisions.
pub fn aggregate(
    own: &Own,
    sources: &[Arc<Snapshot>],
    include_own: bool,
    path: Option<&Path>,
    cancelled: &AtomicUsize,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<Snapshot> {
    check_cancelled(cancelled)?;
    let mut chosen: std::collections::BTreeMap<String, Arc<Snapshot>> = Default::default();
    for s in sources {
        if s.aggregate() {
            return Err(Error::input("a derived team result cannot vote as a reviewer"));
        }
        check_identity(own.id, &s.metadata)?;
        let newer = chosen
            .get(&s.reviewer)
            .is_none_or(|p| (s.exported_ns, &s.snapshot_id) > (p.exported_ns, &p.snapshot_id));
        if newer {
            chosen.insert(s.reviewer.clone(), Arc::clone(s));
        }
    }
    let total = own.id.total();
    let phase = if include_own { total } else { 0 };
    let mut offsets = vec![0u64];
    for s in &own.id.sizes {
        offsets.push(offsets.last().unwrap() + s);
    }
    let mut cb = progress;
    let own_owner;
    if include_own {
        own_owner = Arc::new(TempDir::new("floe-own-review-")?);
        let mut inner = |d: u64, _t: u64, _s: &str| cb(d, total + phase, "");
        let mut mine = export(own, &own_owner.path().join("snapshot.sqlite3"), None, cancelled, &mut inner)?;
        mine._owner = Some(Arc::clone(&own_owner));
        chosen.insert(mine.reviewer.clone(), Arc::new(mine));
    }
    let (output, owner) = match path {
        Some(p) => (p.to_owned(), None),
        None => {
            let o = Arc::new(TempDir::new("floe-team-")?);
            (o.path().join("snapshot.sqlite3"), Some(o))
        }
    };
    let selected: Vec<Arc<Snapshot>> = chosen.into_values().collect();
    let target = realpath(&crate::cache::absolute(&output)?);
    if selected.iter().any(|s| realpath(&s.path) == target) {
        return Err(Error::input("aggregation cannot replace a source snapshot"));
    }
    let extra = vec![
        (
            "sources".into(),
            json!(selected
                .iter()
                .map(|s| json!({"reviewer": s.reviewer, "snapshot_id": s.snapshot_id}))
                .collect::<Vec<_>>()),
        ),
        ("aggregate".into(), json!(true)),
    ];
    let mut report = Progress::new(&mut cb, total + phase);
    let mut streams = selected
        .iter()
        .map(|s| s.chunks(None))
        .collect::<Result<Vec<_>>>()?;
    // (ci, k, source index) min-heap
    let mut heap = BinaryHeap::new();
    let mut heads: Vec<Option<[Bits; 5]>> = vec![None; streams.len()];
    for (i, s) in streams.iter_mut().enumerate() {
        if let Some((ci, k, p)) = s.next()? {
            heads[i] = Some(p);
            heap.push(std::cmp::Reverse((ci, k, i)));
        }
    }
    let mut writer = Writer::new(own.id, &output, "team", extra)?;
    while let Some(&std::cmp::Reverse((ci, k, _))) = heap.peek() {
        check_cancelled(cancelled)?;
        let (mut v, mut w, mut n, mut work) = (Bits::ZERO, Bits::ZERO, Bits::ZERO, Bits::ZERO);
        let (mut wy, mut wn, mut ny, mut nn) = (Bits::ZERO, Bits::ZERO, Bits::ZERO, Bits::ZERO);
        while let Some(&std::cmp::Reverse((c2, k2, i))) = heap.peek() {
            if (c2, k2) != (ci, k) {
                break;
            }
            heap.pop();
            let [sv, sw, sn, _sc, sa] = heads[i].take().unwrap();
            let s = &selected[i];
            v |= sv;
            w |= sw;
            n |= sn;
            work |= sa;
            if s.visited_known {
                if s.waive_known {
                    wy |= sv & sw;
                    wn |= sv & !sw;
                }
                if s.not_required_known {
                    ny |= sv & sn;
                    nn |= sv & !sn;
                }
            }
            if let Some((c3, k3, p)) = streams[i].next()? {
                heads[i] = Some(p);
                heap.push(std::cmp::Reverse((c3, k3, i)));
            }
        }
        let conflict = (wy & wn) | (ny & nn);
        writer.add(ci, k, v, w & !conflict, n & !conflict, conflict, Some(work))?;
        report.report(phase + offsets[ci] + own.id.sizes[ci].min((k + 1) * CHUNK));
    }
    check_cancelled(cancelled)?;
    drop(streams);
    writer.finish()?;
    report.finish(phase + total);
    Snapshot::open(own.id, &output, owner)
}

/// One's own (visited, waived, not-required) planes of chunk `k` and the
/// status bytes, as committed now; call with the review locked and the
/// flags' write lock held.
pub(crate) fn own_bits(
    own: &Own,
    review: &crate::drc::desktop::Review,
    check: usize,
    k: u64,
) -> Result<(Bits, Bits, Bits, Vec<u8>)> {
    let size = own.id.rule(check)?;
    if k * CHUNK >= size {
        return Err(Error::input("source chunk is outside its rule"));
    }
    let count = (size - k * CHUNK).min(CHUNK);
    let (v, n) = own.flags.chunk_fresh(check, k)?;
    let before = review.statuses(check, k * CHUNK, count)?;
    Ok((v, waived_bits(&before), n, before))
}

fn different(s: &Snapshot, ow: Bits, on: Bits, pw: Bits, pn: Bits) -> Bits {
    (if s.waive_known { ow ^ pw } else { Bits::ZERO })
        | (if s.not_required_known { on ^ pn } else { Bits::ZERO })
}

/// One's own visited errors whose decision differs from `source`'s (what a
/// safe application protects), as an aggregate snapshot.
pub fn import_conflicts(
    own: &Own,
    source: &Snapshot,
    path: Option<&Path>,
    cancelled: &AtomicUsize,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<Snapshot> {
    check_identity(own.id, &source.metadata)?;
    let (output, owner) = match path {
        Some(p) => (p.to_owned(), None),
        None => {
            let o = Arc::new(TempDir::new("floe-import-conflicts-")?);
            (o.path().join("snapshot.sqlite3"), Some(o))
        }
    };
    if realpath(&crate::cache::absolute(&output)?) == realpath(&source.path) {
        return Err(Error::input("comparison cannot replace a source snapshot"));
    }
    let total = own.id.total();
    let mut offsets = vec![0u64];
    for s in &own.id.sizes {
        offsets.push(offsets.last().unwrap() + s);
    }
    let mut report = Progress::new(progress, total);
    let extra = vec![
        (
            "sources".into(),
            json!([{"reviewer": source.reviewer, "snapshot_id": source.snapshot_id}]),
        ),
        ("aggregate".into(), json!(true)),
    ];
    let mut writer = destination(own, &output, "import-conflicts", extra)?;
    let mut stream = source.chunks(None)?;
    while let Some((ci, k, [_v, w, n, _c, work])) = stream.next()? {
        check_cancelled(cancelled)?;
        let mut g = own.lock();
        let r = review(&mut g)?;
        let _guard = own.flags.write_guard();
        let (ov, ow, on, _) = own_bits(own, r, ci, k)?;
        let conflict = ov & work & different(source, ow, on, w, n);
        if !conflict.is_zero() {
            writer.add(ci, k, ov & conflict, ow & conflict, on & conflict, conflict, Some(conflict))?;
        }
        report.report(offsets[ci] + own.id.sizes[ci].min((k + 1) * CHUNK));
    }
    check_cancelled(cancelled)?;
    drop(stream);
    writer.finish()?;
    report.finish(total);
    Snapshot::open(own.id, &output, owner)
}

/// The conflicts of an aggregate in rule `check` within `members` (a
/// cluster or CD group) and the waive filter, as a selection.
pub fn conflict_selection(
    own: &Own,
    aggregate: &Snapshot,
    check: usize,
    members: Option<&dyn Membership>,
    waived: Option<bool>,
    cancelled: &AtomicUsize,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<Selection> {
    check_cancelled(cancelled)?;
    check_identity(own.id, &aggregate.metadata)?;
    if !aggregate.aggregate() {
        return Err(Error::input("a team comparison is required for conflict filtering"));
    }
    let size = aggregate.rule(check)?;
    let mut report = Progress::new(progress, size);
    if size == 0 || aggregate.counts(Some(check))?.conflicts == 0 {
        report.finish(size);
        return Ok(Selection::empty(size));
    }
    let generation = match waived {
        Some(_) => Some(own.generation()?),
        None => None,
    };
    let mut words = vec![0u64; size.div_ceil(64) as usize];
    let conn = connect(&aggregate.path, Duration::from_secs(10))?;
    let mut st = conn
        .prepare("SELECT k,c,cc FROM chunks INDEXED BY page_c WHERE ci=? AND cc>0 ORDER BY ce,k")
        .map_err(sql)?;
    let mut rows = st.query([check as i64]).map_err(sql)?;
    let mut mask = vec![false; CHUNK as usize];
    while let Some(row) = rows.next().map_err(sql)? {
        check_cancelled(cancelled)?;
        let k: i64 = row.get(0).map_err(sql)?;
        let blob: Vec<u8> = row.get(1).map_err(sql)?;
        let declared: i64 = row.get(2).map_err(sql)?;
        let start = k as u64 * CHUNK;
        if k < 0 || start >= size || blob.len() != BYTES {
            return Err(Error::input("corrupt conflict chunk"));
        }
        let count = (size - start).min(CHUNK);
        let mut value = Bits::from_blob(&blob, "conflict chunk")?;
        if value.beyond(count) || value.count() != declared as u64 {
            return Err(Error::input("conflict bitmap does not match its count"));
        }
        if let Some(m) = members {
            let mask = &mut mask[..count as usize];
            m.mask(start, mask);
            value &= Bits::from_mask(mask);
        }
        if let (Some(want), false) = (waived, value.is_zero()) {
            let w = waived_bits(&own.statuses.statuses(check, start, count)?);
            value &= if want { w } else { !w & Bits::first(count) };
        }
        let first = (start / 64) as usize;
        for (i, word) in value.0.iter().enumerate() {
            if let Some(slot) = words.get_mut(first + i) {
                *slot |= *word;
            }
        }
        report.report(start + count);
    }
    check_cancelled(cancelled)?;
    if let Some(g) = generation {
        if own.generation()? != g {
            return Err(Error::new(ErrorKind::Busy, "waive status changed; retry conflict filtering"));
        }
    }
    report.finish(size);
    Ok(Selection::from_words(size, words))
}

/// What an application changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Applied {
    pub changed_waived: u64,
    pub changed_not_required: u64,
    pub conflicts_skipped: u64,
    pub processed: u64,
    pub applied: u64,
}

/// Apply `source`'s known waive and not-required decisions over its work
/// (a rule and a membership of it when given). One's own visited errors
/// with a different decision are kept unless `force`. Each chunk commits
/// on its own; `changed(rule)` is told of every rule whose statuses moved.
#[allow(clippy::too_many_arguments)]
pub fn apply(
    own: &Own,
    source: &Snapshot,
    check: Option<usize>,
    members: Option<&dyn Membership>,
    force: bool,
    cancelled: &AtomicUsize,
    progress: &mut dyn FnMut(u64, u64, &str),
    changed: &mut dyn FnMut(usize),
) -> Result<Applied> {
    check_identity(own.id, &source.metadata)?;
    if source.aggregate() {
        return Err(Error::input("choose a reviewer source when applying decisions"));
    }
    if members.is_some() && check.is_none() {
        return Err(Error::input("a rule is required for selected review application"));
    }
    if let Some(c) = check {
        own.id.rule(c)?;
    }
    let sizes = &own.id.sizes;
    let mut offsets = vec![0u64];
    for s in sizes {
        offsets.push(offsets.last().unwrap() + s);
    }
    let total = match check {
        None => own.id.total(),
        Some(c) => sizes[c],
    };
    let mut report = Progress::new(progress, total);
    let mut result = Applied::default();
    check_cancelled(cancelled)?;
    let mut stream = source.chunks(check)?;
    let mut mask = vec![false; CHUNK as usize];
    while let Some((rule, k, [_v, w, n, _c, work])) = stream.next()? {
        check_cancelled(cancelled)?;
        let count = (sizes[rule] - k * CHUNK).min(CHUNK);
        let mut eligible = work;
        if let Some(m) = members {
            let mask = &mut mask[..count as usize];
            m.mask(k * CHUNK, mask);
            eligible &= Bits::from_mask(mask);
        }
        if !eligible.is_zero() {
            let mut g = own.lock();
            let r = review(&mut g)?;
            let mut guard = own.flags.write_guard();
            check_cancelled(cancelled)?;
            let (ov, ow, on, before) = own_bits(own, r, rule, k)?;
            let diff = different(source, ow, on, w, n);
            let conflicts = eligible & ov & diff;
            if !force {
                result.conflicts_skipped += conflicts.count();
                eligible &= !conflicts;
            }
            let changes = eligible & diff;
            if source.waive_known {
                let change_w = eligible & (ow ^ w);
                if !change_w.is_zero() {
                    let mut after = before.clone();
                    for i in change_w.ones() {
                        after[i as usize] = if w.get(i) { STATUS_WAIVED } else { STATUS_NONE };
                    }
                    r.write_status_span(rule, k * CHUNK, &before, &after)?;
                    result.changed_waived += change_w.count();
                    changed(rule);
                }
            }
            if source.not_required_known {
                let (set, clear) = (eligible & n & !on, eligible & on & !n);
                if !set.is_zero() {
                    result.changed_not_required +=
                        own.flags.change_many(&mut guard, rule, &[(k, set)], NOT_REQUIRED, true)?;
                }
                if !clear.is_zero() {
                    result.changed_not_required +=
                        own.flags.change_many(&mut guard, rule, &[(k, clear)], NOT_REQUIRED, false)?;
                }
            }
            result.applied += changes.count();
        }
        result.processed = if check.is_none() { offsets[rule] } else { 0 } + sizes[rule].min((k + 1) * CHUNK);
        report.report(result.processed);
    }
    check_cancelled(cancelled)?;
    result.processed = total;
    report.finish(total);
    Ok(result)
}
