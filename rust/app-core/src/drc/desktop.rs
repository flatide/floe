//! The desktop viewer's DRC review: floe/drc.py IcePack's review state, for
//! `floe2 gtk-service` (docs/SHARED_APP_LAYER.ko.md P2b). The files are the
//! review store's (`review`): the per-reviewer waive sidecar
//! `.<db>.waive.<tag>` (FLOEWAIV v1) and the notes `.<db>.notes.<tag>.fe`
//! beside the pack, byte for byte. What differs from the web's transactional
//! store is how they are kept, as the GTK viewer always kept them:
//!
//! - every waive click is written at once - one status byte and the rule's
//!   waived counter, in place (a sidecar of 100M errors is never rewritten);
//! - a sidecar of another DRC run (header or size) is moved aside
//!   (`.stale-<epoch>`) and a fresh one seeded from the pack's embedded
//!   sections; the first open creates it;
//! - a results folder that cannot be written keeps the review in the system
//!   temp folder (path-hashed names), and when that fails too, in the pack;
//! - notes are rewritten whole on every edit and the file removed when the
//!   last note goes; the reader takes what Python wrote (lenient lines).
//!
//! A sidecar is bound to the pack's fingerprint (source size/mtime, error
//! count), not to its inode: a re-pack of the same .db keeps its review.
use super::{
    local::{pack_reader, reviewer_tag, waive_paths},
    review::Layout,
    Pack, Violation,
};
use crate::{annotations, check_cancelled, Error, ErrorKind, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

pub const STATUS_NONE: u8 = 0;
pub const STATUS_WAIVED: u8 = 1;
/// Status scan granularity: rank and page jumps read cached per-chunk
/// waived counts and touch at most one chunk of status bytes.
const STATUS_CHUNK: u64 = 1 << 22;
/// The colour of a note's flateyes text annotation.
const NOTE_COLOR: &str = "#FFD819";
const HEADER: u64 = 40;

/// One note shared by its member errors (global ids).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub members: BTreeSet<u64>,
}

/// An open pack and its review (see the module docs).
pub struct Review {
    pub pack: Pack,
    header: [u8; 40],
    /// the waive store: a sidecar (statuses at 40), or the pack itself
    pub waive_path: PathBuf,
    status_off: u64,
    wcount_off: u64,
    wcount: Vec<u32>,
    reader: File,
    writer: Option<File>,
    chunks: BTreeMap<usize, Vec<u64>>,
    notes: BTreeMap<u64, Note>,
    note_of: HashMap<u64, u64>,
    note_next: u64,
    pub note_path: PathBuf,
    note_beside: PathBuf,
    note_temp: PathBuf,
    note_tag: String,
    /// what the open had to say (floe/drc.py wrote these to stderr)
    pub notices: Vec<String>,
}

fn io(e: Error) -> std::io::Error {
    std::io::Error::other(e.message)
}

fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Write `bytes` to `path` through `.<name>.tmp-<pid>` in its folder, fsync,
/// rename (floe/drc.py `_note_atomic_write`, `waive_export`).
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let folder = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("not a file name"))?
        .to_string_lossy();
    let tmp = folder.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// The notes sidecar names beside the pack and in the temp folder, from the
/// waive names (same database name, reviewer and path hash).
fn note_paths(waive: &[PathBuf; 2]) -> [PathBuf; 2] {
    let rename = |p: &Path, fe: bool| {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        let name = name.replacen(".waive.", ".notes.", 1);
        p.with_file_name(if fe { format!("{name}.fe") } else { name })
    };
    [rename(&waive[0], true), rename(&waive[1], true)]
}

/// Create or validate the waive sidecar at `side`; returns it. Another run's
/// file is moved aside, a new one seeded from the pack's sections.
fn ensure_sidecar(
    side: &Path,
    pack: &Pack,
    header: &[u8; 40],
    notices: &mut Vec<String>,
) -> std::io::Result<PathBuf> {
    let size = HEADER + pack.total + 4 * pack.checks.len() as u64;
    match File::open(side) {
        Ok(mut f) => {
            let mut head = [0u8; 40];
            let ok =
                f.read_exact(&mut head).is_ok() && head == *header && f.metadata()?.len() == size;
            if ok {
                return Ok(side.to_path_buf());
            }
            let aside = with_suffix(side, &format!(".stale-{}", epoch()));
            fs::rename(side, &aside)?;
            notices.push(format!(
                "[drc] waive autosave does not match this pack (DRC re-run?); moved aside: {}",
                aside.display()
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let tmp = with_suffix(side, &format!(".tmp-{}", std::process::id()));
    let result = (|| {
        let mut out = File::create(&tmp)?;
        let copied = std::io::copy(&mut pack.review_seed().map_err(io)?, &mut out)?;
        if copied != size {
            return Err(std::io::Error::other("pack status sections truncated"));
        }
        out.sync_all()?;
        fs::rename(&tmp, side)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map(|_| side.to_path_buf())
}

impl Review {
    /// Open `pack_path` for review: its reader lock held while open (a
    /// re-pack refuses it), `source` - the .db - checked against the pack's
    /// fingerprint when given, the reviewer `reviewer` (else FLOE_REVIEWER,
    /// the DISPLAY host, the ssh client, the account).
    pub fn open(
        pack_path: &Path,
        source: Option<&Path>,
        reviewer: Option<&str>,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        let lock = pack_reader(pack_path)?;
        let mut pack = Pack::open(pack_path, cancelled)?;
        pack.hold(lock);
        if let Some(source) = source {
            if !pack.source_matches(source)? {
                return Err(Error::new(
                    ErrorKind::Cache,
                    format!(
                        "{}: stale pack (source size/mtime changed)",
                        pack_path.display()
                    ),
                ));
            }
        }
        let header = Layout::from_pack(&pack)?.header();
        let tag = reviewer_tag(reviewer);
        let waive = waive_paths(&pack.path, &tag)?;
        let [note_beside, note_temp] = note_paths(&waive);
        let mut notices = Vec::new();
        let (status_section, wcount_section) = pack.review_sections();
        let (waive_path, status_off, wcount_off) = match ensure_sidecar(
            &waive[0],
            &pack,
            &header,
            &mut notices,
        ) {
            Ok(p) => (p, HEADER, HEADER + pack.total),
            Err(_) => match ensure_sidecar(&waive[1], &pack, &header, &mut notices) {
                Ok(p) => {
                    notices.push(format!(
                        "[drc] results folder not writable; waive autosave in {} (save waives as… to keep the review)",
                        p.display()
                    ));
                    (p, HEADER, HEADER + pack.total)
                }
                Err(e) => {
                    notices.push(format!(
                        "[drc] waive autosave unavailable ({e}); statuses fall back INTO the pack (shared, needs write permission)"
                    ));
                    (pack.path.clone(), status_section, wcount_section)
                }
            },
        };
        let reader = File::open(&waive_path)?;
        let mut raw = vec![0u8; pack.checks.len() * 4];
        reader.read_exact_at(&mut raw, wcount_off)?;
        let wcount = raw
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let note_tag = format!("{},{},{}", pack.source_size, pack.source_mtime, pack.total);
        let mut review = Self {
            pack,
            header,
            waive_path,
            status_off,
            wcount_off,
            wcount,
            reader,
            writer: None,
            chunks: BTreeMap::new(),
            notes: BTreeMap::new(),
            note_of: HashMap::new(),
            note_next: 1,
            note_path: note_beside.clone(),
            note_beside,
            note_temp,
            note_tag,
            notices,
        };
        review.load_notes();
        Ok(review)
    }

    fn check(&self, check: usize) -> Result<(u64, u64)> {
        self.pack
            .checks
            .get(check)
            .map(|c| (c.start, c.count))
            .ok_or_else(|| Error::input("DRC check index out of range"))
    }

    /// Global 0-based error id (the status byte's and the notes' key).
    pub fn error_gid(&self, check: usize, error: u64) -> Result<u64> {
        let (start, count) = self.check(check)?;
        if error >= count {
            return Err(Error::input("DRC error index out of range"));
        }
        Ok(start + error)
    }

    /// Check `check`'s status bytes `start..start+count` (clamped).
    pub fn statuses(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>> {
        let (first, n) = self.check(check)?;
        let start = start.min(n);
        let count = count.min(n - start);
        let mut out = vec![0u8; count as usize];
        self.reader
            .read_exact_at(&mut out, self.status_off + first + start)?;
        Ok(out)
    }

    pub fn status(&self, check: usize, error: u64) -> Result<u8> {
        let gid = self.error_gid(check, error)?;
        let mut b = [0u8; 1];
        self.reader.read_exact_at(&mut b, self.status_off + gid)?;
        Ok(b[0])
    }

    /// (waived, total) of one rule, from the stored counters.
    pub fn status_counts(&self, check: usize) -> Result<(u64, u64)> {
        let (_, n) = self.check(check)?;
        Ok((u64::from(self.wcount[check]), n))
    }

    fn writer(&mut self) -> Result<&File> {
        if self.writer.is_none() {
            self.writer = Some(
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&self.waive_path)?,
            );
        }
        Ok(self.writer.as_ref().expect("opened"))
    }

    /// Set the status of check `check`'s errors `errors` to `value`, in
    /// place, keeping the rule's waived counter in step.
    pub fn set_status(&mut self, check: usize, errors: &[u64], value: u8) -> Result<()> {
        let (start, n) = self.check(check)?;
        if errors.iter().any(|&e| e >= n) {
            return Err(Error::input("DRC error index out of range"));
        }
        for &error in errors {
            let gid = start + error;
            let mut old = [0u8; 1];
            self.reader.read_exact_at(&mut old, self.status_off + gid)?;
            if old[0] == value {
                continue;
            }
            let (status_off, wcount_off) = (self.status_off, self.wcount_off);
            self.writer()?.write_all_at(&[value], status_off + gid)?;
            let (was, now) = (old[0] == STATUS_WAIVED, value == STATUS_WAIVED);
            if was != now {
                let cur = i64::from(self.wcount[check]) + if now { 1 } else { -1 };
                let cur = cur.max(0) as u32;
                self.writer()?
                    .write_all_at(&cur.to_le_bytes(), wcount_off + 4 * check as u64)?;
                self.wcount[check] = cur;
                if let Some(chunks) = self.chunks.get_mut(&check) {
                    let k = (error / STATUS_CHUNK) as usize;
                    if now {
                        chunks[k] += 1;
                    } else {
                        chunks[k] = chunks[k].saturating_sub(1);
                    }
                }
            }
        }
        Ok(())
    }

    /// Per-chunk waived counts of one rule: built on the first filtered
    /// access with one pass, then kept in step by set_status.
    fn wf_chunks(&mut self, check: usize) -> Result<&Vec<u64>> {
        if !self.chunks.contains_key(&check) {
            let (_, n) = self.check(check)?;
            let mut counts = Vec::new();
            let mut a = 0;
            while a < n {
                let b = (a + STATUS_CHUNK).min(n);
                let st = self.statuses(check, a, b - a)?;
                counts.push(st.iter().filter(|&&s| s == STATUS_WAIVED).count() as u64);
                a = b;
            }
            self.chunks.insert(check, counts);
        }
        Ok(&self.chunks[&check])
    }

    /// The `start`..`start+limit`-th errors of one rule whose waived-ness
    /// is `waived`, as rule-local indices.
    pub fn status_page(
        &mut self,
        check: usize,
        waived: bool,
        start: u64,
        limit: u64,
    ) -> Result<Vec<u64>> {
        let (_, n) = self.check(check)?;
        if n == 0 || limit == 0 {
            return Ok(Vec::new());
        }
        let chunks = self.wf_chunks(check)?.clone();
        let mut out = Vec::new();
        let mut seen = 0u64;
        for (k, &w) in chunks.iter().enumerate() {
            let a = k as u64 * STATUS_CHUNK;
            let b = (a + STATUS_CHUNK).min(n);
            let cnt = if waived { w } else { (b - a) - w };
            if seen + cnt <= start {
                seen += cnt;
                continue;
            }
            let st = self.statuses(check, a, b - a)?;
            let skip = start.saturating_sub(seen);
            for (i, _) in st
                .iter()
                .enumerate()
                .filter(|(_, &s)| (s == STATUS_WAIVED) == waived)
                .skip(skip as usize)
                .take((limit - out.len() as u64) as usize)
            {
                out.push(a + i as u64);
            }
            seen += cnt;
            if out.len() as u64 >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Rank of `error` among the rule's errors whose waived-ness is
    /// `waived` (None when it does not match).
    pub fn status_rank(&mut self, check: usize, waived: bool, error: u64) -> Result<Option<u64>> {
        if (self.status(check, error)? == STATUS_WAIVED) != waived {
            return Ok(None);
        }
        let k = error / STATUS_CHUNK;
        let before: u64 = self.wf_chunks(check)?[..k as usize].iter().sum();
        let a = k * STATUS_CHUNK;
        let w_in = self
            .statuses(check, a, error - a)?
            .iter()
            .filter(|&&s| s == STATUS_WAIVED)
            .count() as u64;
        Ok(Some(if waived {
            before + w_in
        } else {
            (a - before) + (error - a - w_in)
        }))
    }

    /// Errors of `checks` (None: all) whose geometry meets the um rect, at
    /// most `cap`, file order; `waived` filters by review status before the
    /// cap (floe/drc.py query_rect).
    pub fn query(
        &mut self,
        bbox_um: [f64; 4],
        cap: usize,
        checks: Option<&BTreeSet<usize>>,
        waived: Option<bool>,
        cancelled: &AtomicUsize,
    ) -> Result<Vec<(usize, u64, Violation)>> {
        let mut out = Vec::new();
        let mut cursor = super::Cursor::default();
        let checks = match (waived, checks) {
            // a rule with no waived error has nothing to give
            (Some(true), only) => Some(
                (0..self.pack.checks.len())
                    .filter(|c| only.is_none_or(|o| o.contains(c)) && self.wcount[*c] > 0)
                    .collect::<BTreeSet<_>>(),
            ),
            (_, only) => only.cloned(),
        };
        while out.len() < cap {
            check_cancelled(cancelled)?;
            let page = self.pack.query(
                bbox_um,
                checks.as_ref(),
                None,
                cursor,
                super::PAGE_ITEMS,
                cancelled,
            )?;
            for hit in page.hits {
                if let Some(w) = waived {
                    if (self.status(hit.check, hit.local)? == STATUS_WAIVED) != w {
                        continue;
                    }
                }
                out.push((hit.check, hit.local, hit.violation));
                if out.len() >= cap {
                    break;
                }
            }
            match page.next {
                Some(next) => cursor = next,
                None => break,
            }
        }
        Ok(out)
    }

    // ---- notes ---------------------------------------------------------

    /// The note shared by global error `gid`, if any.
    pub fn note(&self, gid: u64) -> Option<&str> {
        self.note_of
            .get(&gid)
            .map(|nid| self.notes[nid].text.as_str())
    }

    /// Every note with members, in note order.
    pub fn notes(&self) -> Vec<&Note> {
        self.notes
            .values()
            .filter(|n| !n.members.is_empty())
            .collect()
    }

    fn detach(&mut self, gids: &[u64]) {
        for g in gids {
            if let Some(nid) = self.note_of.remove(g) {
                let empty = {
                    let note = self.notes.get_mut(&nid).expect("note");
                    note.members.remove(g);
                    note.members.is_empty()
                };
                if empty {
                    self.notes.remove(&nid);
                }
            }
        }
    }

    /// One shared note on `gids` (each detached from its earlier note
    /// first); empty text only clears. Ids out of range are dropped.
    pub fn set_note(&mut self, gids: &[u64], text: &str, cancelled: &AtomicUsize) -> Result<()> {
        let gids: Vec<u64> = gids
            .iter()
            .copied()
            .filter(|&g| g < self.pack.total)
            .collect();
        self.detach(&gids);
        let text = text.trim();
        if !text.is_empty() && !gids.is_empty() {
            let nid = self.note_next;
            self.note_next += 1;
            self.notes.insert(
                nid,
                Note {
                    text: text.to_string(),
                    members: gids.iter().copied().collect(),
                },
            );
            for g in gids {
                self.note_of.insert(g, nid);
            }
        }
        self.write_notes(cancelled)
    }

    pub fn clear_note(&mut self, gids: &[u64], cancelled: &AtomicUsize) -> Result<()> {
        let gids: Vec<u64> = gids
            .iter()
            .copied()
            .filter(|&g| g < self.pack.total)
            .collect();
        self.detach(&gids);
        self.write_notes(cancelled)
    }

    /// The um centre of an error, the anchor of its note's text line.
    fn center(&mut self, gid: u64, cancelled: &AtomicUsize) -> [f64; 2] {
        let found = self
            .pack
            .checks
            .iter()
            .enumerate()
            .rfind(|(_, c)| c.start <= gid && gid < c.start + c.count)
            .map(|(i, c)| (i, gid - c.start));
        let precision = self.pack.precision;
        found
            .and_then(|(ci, local)| self.pack.error(ci, local, cancelled).ok())
            .map(|v| {
                let x0 = v
                    .points
                    .iter()
                    .map(|p| p[0] as f64 / precision)
                    .fold(f64::INFINITY, f64::min);
                let x1 = v
                    .points
                    .iter()
                    .map(|p| p[0] as f64 / precision)
                    .fold(f64::NEG_INFINITY, f64::max);
                let y0 = v
                    .points
                    .iter()
                    .map(|p| p[1] as f64 / precision)
                    .fold(f64::INFINITY, f64::min);
                let y1 = v
                    .points
                    .iter()
                    .map(|p| p[1] as f64 / precision)
                    .fold(f64::NEG_INFINITY, f64::max);
                [(x0 + x1) / 2., (y0 + y1) / 2.]
            })
            .filter(|c| c.iter().all(|v| v.is_finite()))
            .unwrap_or([0., 0.])
    }

    /// The notes' flateyes .fe text, None when there are none
    /// (floe/drc.py `_serialize_notes`, byte for byte).
    pub fn serialize_notes(&mut self, cancelled: &AtomicUsize) -> Result<Option<String>> {
        let notes: Vec<Note> = self.notes().into_iter().cloned().collect();
        if notes.is_empty() {
            return Ok(None);
        }
        let mut out = format!(
            "# flateyes annotations\nfloe_pack={}\nppu=1\nunit=um\n",
            self.note_tag
        );
        for note in &notes {
            let ids: Vec<String> = note.members.iter().map(u64::to_string).collect();
            out.push_str(&format!(
                "floe_note={}|{}\n",
                ids.join(","),
                annotations::escape(&note.text)
            ));
        }
        for note in &notes {
            for &gid in &note.members {
                check_cancelled(cancelled)?;
                let [x, y] = self.center(gid, cancelled);
                out.push_str(&format!(
                    "text={},{},16,{NOTE_COLOR},#00000059,{}\n",
                    annotations::coord(x),
                    annotations::coord(y),
                    annotations::escape(&note.text)
                ));
            }
        }
        Ok(Some(out))
    }

    /// Persist the notes (or remove the file when none remain); a results
    /// folder that refuses switches once to the temp folder.
    fn write_notes(&mut self, cancelled: &AtomicUsize) -> Result<()> {
        let Some(text) = self.serialize_notes(cancelled)? else {
            let _ = fs::remove_file(&self.note_path);
            return Ok(());
        };
        if let Err(e) = atomic_write(&self.note_path, text.as_bytes()) {
            if self.note_path != self.note_beside {
                return Err(e.into());
            }
            self.note_path = self.note_temp.clone();
            atomic_write(&self.note_path, text.as_bytes())?;
            self.notices.push(format!(
                "[drc] results folder not writable; note autosave in {} (save notes explicitly to keep them)",
                self.note_path.display()
            ));
        }
        Ok(())
    }

    /// The notes of `text`: false when its pack fingerprint is another
    /// pack's (floe/drc.py `_parse_notes` - malformed lines skipped).
    fn parse_notes(&mut self, text: &str) -> bool {
        let mut notes = BTreeMap::new();
        let mut note_of = HashMap::new();
        let mut nid = 1u64;
        let mut tag_ok = true;
        for raw in text.lines() {
            let line = raw.trim();
            if let Some(tag) = line.strip_prefix("floe_pack=") {
                tag_ok = tag == self.note_tag;
            } else if let Some(body) = line.strip_prefix("floe_note=") {
                let Some((ids, txt)) = body.split_once('|') else {
                    continue;
                };
                let members: BTreeSet<u64> = ids
                    .split(',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
                    .filter_map(|t| t.parse::<u64>().ok())
                    .filter(|&g| g < self.pack.total)
                    .collect();
                let txt = annotations::unescape(txt).trim().to_string();
                if members.is_empty() || txt.is_empty() {
                    continue;
                }
                for &g in &members {
                    note_of.insert(g, nid);
                }
                notes.insert(nid, Note { text: txt, members });
                nid += 1;
            }
        }
        if !tag_ok {
            return false;
        }
        self.notes = notes;
        self.note_of = note_of;
        self.note_next = nid;
        true
    }

    /// The notes autosave: beside the pack, else the temp folder's; another
    /// pack's file is moved aside.
    fn load_notes(&mut self) {
        for path in [self.note_beside.clone(), self.note_temp.clone()] {
            let Ok(text) = fs::read(&path) else { continue };
            let text = String::from_utf8_lossy(&text).into_owned();
            if self.parse_notes(&text) {
                self.note_path = path;
            } else {
                let aside = with_suffix(&path, &format!(".stale-{}", epoch()));
                if fs::rename(&path, &aside).is_ok() {
                    self.notices.push(format!(
                        "[drc] note sidecar does not match this pack (DRC re-run?); moved aside: {}",
                        aside.display()
                    ));
                }
                self.notes.clear();
                self.note_of.clear();
                self.note_next = 1;
            }
            return;
        }
    }

    /// Save the notes to `dst` (an empty set removes it).
    pub fn note_export(&mut self, dst: &Path, cancelled: &AtomicUsize) -> Result<()> {
        match self.serialize_notes(cancelled)? {
            Some(text) => Ok(atomic_write(dst, text.as_bytes())?),
            None => {
                let _ = fs::remove_file(dst);
                Ok(())
            }
        }
    }

    /// Replace the notes from a saved .fe; another pack's file is refused.
    /// The note count.
    pub fn note_import(&mut self, src: &Path, cancelled: &AtomicUsize) -> Result<usize> {
        let text = String::from_utf8_lossy(&fs::read(src)?).into_owned();
        if !self.parse_notes(&text) {
            return Err(Error::input(format!(
                "{}: note file does not match this pack (different DRC run / re-packed db?)",
                src.display()
            )));
        }
        self.write_notes(cancelled)?;
        Ok(self.notes.len())
    }

    // ---- waive files ---------------------------------------------------

    /// Save the review state to `dst`, the autosave's format.
    pub fn waive_export(&self, dst: &Path) -> Result<()> {
        let mut data = self.header.to_vec();
        let mut status = vec![0u8; self.pack.total as usize];
        self.reader.read_exact_at(&mut status, self.status_off)?;
        data.extend_from_slice(&status);
        for w in &self.wcount {
            data.extend_from_slice(&w.to_le_bytes());
        }
        Ok(atomic_write(dst, &data)?)
    }

    /// Replace the review state from a saved file of this pack (header and
    /// size); the counters recomputed. The waived count.
    pub fn waive_import(&mut self, src: &Path) -> Result<u64> {
        let data = fs::read(src)?;
        let n = self.pack.total as usize;
        let checks = self.pack.checks.len();
        if data.len() != HEADER as usize + n + 4 * checks || data[..HEADER as usize] != self.header
        {
            return Err(Error::input(format!(
                "{}: waive file does not match this pack (different DRC run / re-packed db?)",
                src.display()
            )));
        }
        let status = &data[HEADER as usize..HEADER as usize + n];
        let counts: Vec<u32> = self
            .pack
            .checks
            .iter()
            .map(|c| {
                status[c.start as usize..(c.start + c.count) as usize]
                    .iter()
                    .filter(|&&s| s == STATUS_WAIVED)
                    .count() as u32
            })
            .collect();
        let (status_off, wcount_off) = (self.status_off, self.wcount_off);
        let w = self.writer()?;
        if n > 0 {
            w.write_all_at(status, status_off)?;
        }
        let raw: Vec<u8> = counts.iter().flat_map(|c| c.to_le_bytes()).collect();
        if !raw.is_empty() {
            w.write_all_at(&raw, wcount_off)?;
        }
        self.wcount = counts;
        self.chunks.clear();
        Ok(status.iter().filter(|&&s| s == STATUS_WAIVED).count() as u64)
    }
}

/// One rule as the viewer lists it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct CheckRow {
    pub name: String,
    pub desc: String,
    /// the count the results file declares (a number where it is one)
    pub declared: serde_json::Value,
    pub count: u64,
    /// the global id of its first error
    pub start: u64,
}

/// One error: kind ('p' polygon, 'e' edge), its global 1-based number and
/// its points in micrometres (floe/drc.py DrcError).
#[derive(Clone, Debug, PartialEq)]
pub struct ErrorRow {
    pub kind: char,
    pub number: u64,
    pub points: Vec<[f64; 2]>,
}

fn row(v: &Violation, precision: f64) -> ErrorRow {
    ErrorRow {
        kind: v.kind,
        number: v.number,
        points: v
            .points
            .iter()
            .map(|p| [p[0] as f64 / precision, p[1] as f64 / precision])
            .collect(),
    }
}

/// An open results database: a pack under review, or an ASCII file read
/// whole (no review state, no spatial query - as floe/drc.py DrcDb).
pub enum Opened {
    Pack(Box<Review>),
    Ascii(Box<super::Ascii>),
}

impl Opened {
    /// floe/drc.py load_db: a pack given (by its magic), or the .db's pack
    /// when it is current, else the ASCII file itself - the reason said.
    pub fn load(
        path: &Path,
        reviewer: Option<&str>,
        cancelled: &AtomicUsize,
    ) -> Result<(Self, Vec<String>)> {
        let mut magic = [0u8; 8];
        let packed = File::open(path)?.read_exact(&mut magic).is_ok() && &magic == super::MAGIC;
        if packed {
            return Ok((
                Self::Pack(Box::new(Review::open(path, None, reviewer, cancelled)?)),
                Vec::new(),
            ));
        }
        let mut warnings = Vec::new();
        let side = crate::cache::pack_path(path)?;
        if side.exists() {
            match Review::open(&side, Some(path), reviewer, cancelled) {
                Ok(review) => return Ok((Self::Pack(Box::new(review)), Vec::new())),
                Err(e) if matches!(e.kind, ErrorKind::Busy | ErrorKind::Cancelled) => {
                    return Err(e)
                }
                Err(e) => warnings.push(format!(
                    "[drc] {}: {e}; parsing ASCII instead (rerun: floe-index drc {})",
                    side.display(),
                    path.display()
                )),
            }
        }
        Ok((
            Self::Ascii(Box::new(super::Ascii::open(path, cancelled)?)),
            warnings,
        ))
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Pack(r) => &r.pack.path,
            Self::Ascii(a) => &a.path,
        }
    }
    pub fn cell(&self) -> &str {
        match self {
            Self::Pack(r) => &r.pack.cell,
            Self::Ascii(a) => &a.cell,
        }
    }
    pub fn precision(&self) -> f64 {
        match self {
            Self::Pack(r) => r.pack.precision,
            Self::Ascii(a) => a.precision,
        }
    }
    pub fn total(&self) -> u64 {
        match self {
            Self::Pack(r) => r.pack.total,
            Self::Ascii(a) => a.total,
        }
    }

    pub fn checks(&self) -> Vec<CheckRow> {
        let declared = |text: &str| {
            text.trim()
                .parse::<i64>()
                .map(serde_json::Value::from)
                .unwrap_or_else(|_| serde_json::Value::from(text))
        };
        match self {
            Self::Pack(r) => r
                .pack
                .checks
                .iter()
                .map(|c| CheckRow {
                    name: c.name.clone(),
                    desc: c.desc.clone(),
                    declared: serde_json::Value::from(c.declared),
                    count: c.count,
                    start: c.start,
                })
                .collect(),
            Self::Ascii(a) => a
                .checks
                .iter()
                .map(|c| CheckRow {
                    name: c.name.clone(),
                    desc: c.desc.clone(),
                    declared: declared(&c.declared),
                    count: c.count,
                    start: c.start,
                })
                .collect(),
        }
    }

    fn count(&self, check: usize) -> Result<u64> {
        let count = match self {
            Self::Pack(r) => r.pack.checks.get(check).map(|c| c.count),
            Self::Ascii(a) => a.checks.get(check).map(|c| c.count),
        };
        count.ok_or_else(|| Error::input("DRC check index out of range"))
    }

    /// Check `check`'s errors `start..start+count` (clamped), in order.
    pub fn errors(
        &mut self,
        check: usize,
        start: u64,
        count: u64,
        cancelled: &AtomicUsize,
    ) -> Result<Vec<ErrorRow>> {
        let n = self.count(check)?;
        let end = start.saturating_add(count).min(n);
        let mut out = Vec::new();
        let mut i = start.min(n);
        while i < end {
            check_cancelled(cancelled)?;
            match self {
                Self::Pack(r) => {
                    let precision = r.pack.precision;
                    let limit = ((end - i) as usize).min(super::PAGE_ITEMS);
                    let page = r.pack.errors(check, i, limit, cancelled)?;
                    if page.hits.is_empty() {
                        break;
                    }
                    for hit in page.hits {
                        if hit.local >= end {
                            break;
                        }
                        out.push(row(&hit.violation, precision));
                        i = hit.local + 1;
                    }
                }
                Self::Ascii(a) => {
                    let v = a.error(check, i, cancelled)?;
                    out.push(ErrorRow {
                        kind: v.kind,
                        number: v.number,
                        points: v.points_um,
                    });
                    i += 1;
                }
            }
        }
        Ok(out)
    }

    /// The CD ruler segments of one simple error, um (floe/drc.py
    /// cd_segments): none for complex shapes.
    pub fn cd(
        &mut self,
        check: usize,
        error: u64,
        cancelled: &AtomicUsize,
    ) -> Result<Vec<[f64; 4]>> {
        let segments = match self {
            Self::Pack(r) => {
                let v = r.pack.error(check, error, cancelled)?;
                super::cd_segments(v.kind, &v.points, r.pack.precision)?
            }
            Self::Ascii(a) => {
                let v = a.error(check, error, cancelled)?;
                super::cd_segments_um(v.kind, &v.points_um, cancelled)?
            }
        };
        Ok(segments
            .iter()
            .map(|s| {
                [
                    s.endpoints_um[0][0],
                    s.endpoints_um[0][1],
                    s.endpoints_um[1][0],
                    s.endpoints_um[1][1],
                ]
            })
            .collect())
    }
}

/// The ErrorRow of a query hit.
pub fn query_rows(
    review: &Review,
    hits: Vec<(usize, u64, Violation)>,
) -> Vec<(usize, u64, ErrorRow)> {
    let precision = review.pack.precision;
    hits.into_iter()
        .map(|(c, e, v)| (c, e, row(&v, precision)))
        .collect()
}
