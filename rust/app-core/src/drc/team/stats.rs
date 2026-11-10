//! Review statistics of every reviewer of one DRC result, without a viewer
//! (user 2026-11-10: a launcher - TeeBox - collects the statistics of the
//! people who review one layout's result; the viewer's CSVs needed a person
//! to load each peer by hand, and `floe2 drc --rules` gave one reviewer's
//! stored waived counters alone).
//!
//! The reviewers are found by their working files - the waive sidecar
//! `.<db>.waive.<id>` and the flags `.<db>.review.<id>.sqlite3` beside the
//! pack, and where this account's reviews go when the results folder cannot
//! be written (the temp folder's sidecar, FLOE_DRC_REVIEW_ROOT or the
//! user-data folder's flags). Each is counted from what its files hold: the
//! status bytes are read and counted again (the stored counters are only
//! compared - two sessions of one reviewer could leave them wrong before
//! 0.12.440), the flags from their bit planes. The team's numbers are the
//! reviewers window's (snapshot::aggregate, chunk by chunk): visits united,
//! a waive or an exclusion carried unless two reviewers who both visited the
//! error decided differently - that error is a conflict.
//!
//! Nothing is written, no snapshot is made, and the pack must be the
//! current one: a missing or stale pack is an error, never an answer of
//! zeros from the ASCII file.
use super::bits::{Bits, CHUNK};
use super::flags::{self, open_read_only, sql};
use super::Identity;
use crate::drc::desktop::STATUS_WAIVED;
use crate::drc::Pack;
use crate::{check_cancelled, Error, ErrorKind, Result};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

pub const TYPE: &str = "floe-drc-review-statistics";
pub const VERSION: u32 = 1;
const READ_BUFFER: usize = 1 << 20;

/// A reviewer's working files as found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Found {
    /// the waive sidecar and where it is (`results`, `temp`)
    waive: Option<(PathBuf, &'static str)>,
    /// the flags file and where it is (`results`, `review-root`, `user-data`)
    review: Option<(PathBuf, &'static str)>,
}

/// A tag as the sidecars are named (letters, digits, `.`, `_`, `-`), and not
/// a leftover of one: `<id>.stale-<epoch>` (another run's sidecar moved
/// aside) or `<id>.tmp-<pid>` (a sidecar being made).
fn reviewer_of(tag: &str) -> Option<&str> {
    if tag.is_empty()
        || tag.len() > 200
        || !tag.chars().all(|c| c.is_alphanumeric() || "._-".contains(c))
    {
        return None;
    }
    if let Some((_, last)) = tag.rsplit_once('.') {
        for prefix in ["stale-", "tmp-"] {
            if last
                .strip_prefix(prefix)
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            {
                return None;
            }
        }
    }
    Some(tag)
}

/// The names in `folder` (missing: none), or the error when `must`.
fn names(folder: &Path, must: bool) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) if !must || e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(Error::new(
                ErrorKind::Io,
                format!("{}: cannot list the reviewers' files: {e}", folder.display()),
            ))
        }
    };
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(name.to_owned());
                }
            }
            Err(e) if must => return Err(e.into()),
            Err(_) => (),
        }
    }
    Ok(out)
}

/// Every reviewer with a working file of `pack`'s result, by tag.
fn discover(pack: &Path) -> Result<BTreeMap<String, Found>> {
    let database = crate::cache::database_path(pack)?;
    let name = database
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| Error::input("DRC filename must be UTF-8"))?
        .to_owned();
    let folder = database.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    let waive_prefix = format!(".{name}.waive.");
    let review_prefix = format!(".{name}.review.");
    let review_tag = |file: &str| -> Option<String> {
        file.strip_prefix(&review_prefix)
            .and_then(|rest| rest.strip_suffix(".sqlite3"))
            .and_then(reviewer_of)
            .map(str::to_owned)
    };
    // beside the pack: the results folder, or its place under --workdir
    for file in names(&folder, true)? {
        if let Some(tag) = file.strip_prefix(&waive_prefix).and_then(reviewer_of) {
            found.entry(tag.to_owned()).or_default().waive = Some((folder.join(&file), "results"));
        } else if let Some(tag) = review_tag(&file) {
            found.entry(tag).or_default().review = Some((folder.join(&file), "results"));
        }
    }
    // the flags' own root, when one is set: the primary place then
    let probe = flags::review_path(pack, "x")?;
    if let Some(root) = probe.parent().filter(|p| *p != folder) {
        for file in names(root, false)? {
            if let Some(tag) = review_tag(&file) {
                found.entry(tag).or_default().review = Some((root.join(&file), "review-root"));
            }
        }
    }
    // where this account's reviews go when the results folder refuses them:
    // the flags in the user-data folder (only without a primary file, as the
    // store itself reads them), the waive sidecar in the temp folder
    if let Ok(fallback) = flags::fallback_path(pack, "x") {
        if let Some(root) = fallback.parent() {
            for file in names(root, false)? {
                if let Some(tag) = review_tag(&file) {
                    let slot = &mut found.entry(tag).or_default().review;
                    if slot.is_none() {
                        *slot = Some((root.join(&file), "user-data"));
                    }
                }
            }
        }
    }
    let temp_name = crate::drc::waive_paths(pack, "x")?[1]
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    // `.{name}.waive.x-{hash}`: the hash of this database's path
    if let Some(suffix) = temp_name.strip_prefix(&format!("{waive_prefix}x")) {
        let temp = std::env::temp_dir();
        if temp != folder && !suffix.is_empty() {
            for file in names(&temp, false)? {
                let tag = file
                    .strip_prefix(&waive_prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .and_then(reviewer_of);
                if let Some(tag) = tag {
                    let slot = &mut found.entry(tag.to_owned()).or_default().waive;
                    if slot.is_none() {
                        *slot = Some((temp.join(&file), "temp"));
                    }
                }
            }
        }
    }
    Ok(found)
}

/// When a file was last changed, UTC (`2026-11-10T07:12:45Z`).
fn modified(path: &Path) -> Option<String> {
    let at = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = at.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(utc(secs))
}

/// Seconds since the epoch as UTC, ISO 8601 (days to civil: H. Hinnant).
pub(crate) fn utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// The status bytes equal to waived, as bits (at most a chunk).
fn waived_bits(statuses: &[u8]) -> Bits {
    let mut bits = Bits::ZERO;
    for (word, group) in statuses.chunks(64).enumerate() {
        if group.iter().all(|&b| b == 0) {
            continue;
        }
        let mut w = 0u64;
        for (i, &b) in group.iter().enumerate() {
            if b == STATUS_WAIVED {
                w |= 1 << i;
            }
        }
        bits.0[word] = w;
    }
    bits
}

/// One reviewer's counts of a rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Own {
    waived: u64,
    visited: u64,
    not_required: u64,
    both: u64,
}
impl Own {
    fn add(&mut self, o: &Own) {
        self.waived += o.waived;
        self.visited += o.visited;
        self.not_required += o.not_required;
        self.both += o.both;
    }
    fn any(&self) -> bool {
        self.waived + self.visited + self.not_required > 0
    }
    fn json(&self, total: u64, waive: bool, flags: bool) -> Value {
        let mut v = json!({});
        if waive {
            v["waived"] = json!(self.waived);
        }
        if flags {
            v["visited"] = json!(self.visited);
            v["not_required"] = json!(self.not_required);
            v["visited_not_required"] = json!(self.both);
            // neither visited nor excluded: the viewer's "unvisited targets"
            v["pending"] = json!(total - self.visited - self.not_required + self.both);
        }
        v
    }
}

/// The team's counts of a rule (snapshot::Counts' numbers).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Team {
    visited: u64,
    waived: u64,
    not_required: u64,
    conflicts: u64,
    done: u64,
}
impl Team {
    fn add(&mut self, o: &Team) {
        self.visited += o.visited;
        self.waived += o.waived;
        self.not_required += o.not_required;
        self.conflicts += o.conflicts;
        self.done += o.done;
    }
    fn json(&self, total: u64) -> Value {
        json!({
            "errors": total,
            "visited": self.visited,
            "waived": self.waived,
            "not_required": self.not_required,
            "conflicts": self.conflicts,
            "pending": total - self.done,
        })
    }
}

/// One reviewer's planes of a chunk as the team counts them.
#[derive(Clone, Copy)]
struct Planes {
    visited: Bits,
    waived: Bits,
    not_required: Bits,
    visited_known: bool,
    waive_known: bool,
    not_required_known: bool,
}

/// The team's counts of one chunk from its reviewers' planes
/// (snapshot::aggregate's rule).
fn team_of(planes: &[Planes]) -> Team {
    let (mut v, mut w, mut n) = (Bits::ZERO, Bits::ZERO, Bits::ZERO);
    let (mut wy, mut wn, mut ny, mut nn) = (Bits::ZERO, Bits::ZERO, Bits::ZERO, Bits::ZERO);
    for p in planes {
        v |= p.visited;
        w |= p.waived;
        n |= p.not_required;
        if p.visited_known {
            if p.waive_known {
                wy |= p.visited & p.waived;
                wn |= p.visited & !p.waived;
            }
            if p.not_required_known {
                ny |= p.visited & p.not_required;
                nn |= p.visited & !p.not_required;
            }
        }
    }
    let conflict = (wy & wn) | (ny & nn);
    let n = n & !conflict;
    Team {
        visited: v.count(),
        waived: (w & !conflict).count(),
        not_required: n.count(),
        conflicts: conflict.count(),
        done: (v | n).count(),
    }
}

/// A reviewer being counted: where the files are and the counts so far
/// (the open files are kept apart, in the same order).
struct Reviewer {
    tag: String,
    waive: Option<(PathBuf, &'static str)>,
    flags: Option<(PathBuf, &'static str)>,
    rules: Vec<Own>,
    stored: Option<Vec<u32>>,
}

/// The statistics of `source`'s current pack (see the module docs): every
/// reviewer found, or `only` these (an id with no file is listed as absent).
pub fn collect(source: &Path, only: Option<&[String]>, cancelled: &AtomicUsize) -> Result<Value> {
    let pack = crate::drc::open_pack(source, cancelled)?;
    let id = Identity::of_pack(&pack, "statistics")?;
    let found = discover(&pack.path)?;
    let report = collect_from(&pack, &id, found, only, cancelled)?;
    pack.unchanged()?;
    Ok(report)
}

fn collect_from(
    pack: &Pack,
    id: &Identity,
    mut found: BTreeMap<String, Found>,
    only: Option<&[String]>,
    cancelled: &AtomicUsize,
) -> Result<Value> {
    let total = id.total();
    let rules = id.sizes.len();
    let mut absent = Vec::new();
    if let Some(only) = only {
        let wanted: Vec<String> = only.iter().map(|r| flags::sanitize(r.trim())).collect();
        found.retain(|tag, _| wanted.contains(tag));
        for tag in wanted {
            if !found.contains_key(&tag) && !absent.contains(&tag) {
                absent.push(tag);
            }
        }
    }
    let mut skipped = Vec::new();
    let mut skip = |reviewer: &str, file: &Path, reason: String| {
        skipped.push(json!({"reviewer": reviewer, "file": file, "reason": reason}));
    };
    let mut reviewers = Vec::new();
    let mut statuses: Vec<Option<BufReader<File>>> = Vec::new();
    let mut connections: Vec<Option<Connection>> = Vec::new();
    for (tag, files) in found {
        check_cancelled(cancelled)?;
        let mut reviewer = Reviewer {
            tag: tag.clone(),
            waive: None,
            flags: None,
            rules: vec![Own::default(); rules],
            stored: None,
        };
        let (mut reader, mut connection) = (None, None);
        if let Some((path, place)) = files.waive {
            match open_waive(&path, id) {
                Ok((file, stored)) => {
                    reviewer.stored = Some(stored);
                    reviewer.waive = Some((path, place));
                    reader = Some(BufReader::with_capacity(READ_BUFFER, file));
                }
                Err(e) => skip(&tag, &path, e.message),
            }
        }
        if let Some((path, place)) = files.review {
            match open_flags(&path, id) {
                Ok(conn) => {
                    reviewer.flags = Some((path, place));
                    connection = Some(conn);
                }
                Err(e) => skip(&tag, &path, e.message),
            }
        }
        if reader.is_some() || connection.is_some() {
            reviewers.push(reviewer);
            statuses.push(reader);
            connections.push(connection);
        }
    }
    // rule by rule, chunk by chunk: each reviewer's planes, then the team's
    let mut team = vec![Team::default(); rules];
    let mut buffer = vec![0u8; CHUNK as usize];
    for (ci, &size) in id.sizes.iter().enumerate() {
        let mut statements = Vec::with_capacity(reviewers.len());
        for conn in &connections {
            statements.push(match conn {
                Some(conn) => Some(
                    conn.prepare("SELECT chunk,visited,nr FROM chunks WHERE ci=? ORDER BY chunk")
                        .map_err(sql)?,
                ),
                None => None,
            });
        }
        let mut rows = Vec::with_capacity(statements.len());
        for st in statements.iter_mut() {
            rows.push(match st {
                Some(st) => Some(st.query([ci as i64]).map_err(sql)?),
                None => None,
            });
        }
        let mut heads: Vec<Option<(u64, Bits, Bits)>> = Vec::with_capacity(rows.len());
        for r in rows.iter_mut() {
            heads.push(next_flags(r)?);
        }
        let mut owns = vec![Own::default(); reviewers.len()];
        let mut planes = Vec::with_capacity(reviewers.len());
        let mut start = 0u64;
        while start < size {
            if (start / CHUNK).is_multiple_of(256) {
                check_cancelled(cancelled)?;
            }
            let (k, count) = (start / CHUNK, (size - start).min(CHUNK));
            planes.clear();
            for (i, reader) in statuses.iter_mut().enumerate() {
                let r = &reviewers[i];
                let waived = match reader {
                    Some(reader) => {
                        reader.read_exact(&mut buffer[..count as usize]).map_err(|e| {
                            let path = r.waive.as_ref().map(|(p, _)| p.display().to_string());
                            Error::new(ErrorKind::Io, format!("{}: {e}", path.unwrap_or_default()))
                        })?;
                        waived_bits(&buffer[..count as usize])
                    }
                    None => Bits::ZERO,
                };
                let (mut visited, mut not_required) = (Bits::ZERO, Bits::ZERO);
                // (a stored chunk before this one would be out of order: skipped)
                while heads[i].is_some_and(|(rk, ..)| rk < k) {
                    heads[i] = next_flags(&mut rows[i])?;
                }
                if let Some((rk, v, n)) = heads[i] {
                    if rk == k {
                        (visited, not_required) = (v, n);
                        heads[i] = next_flags(&mut rows[i])?;
                    }
                }
                let limit = Bits::first(count);
                let (visited, not_required) = (visited & limit, not_required & limit);
                let own = &mut owns[i];
                own.waived += waived.count();
                own.visited += visited.count();
                own.not_required += not_required.count();
                own.both += (visited & not_required).count();
                planes.push(Planes {
                    visited,
                    waived,
                    not_required,
                    visited_known: r.flags.is_some(),
                    waive_known: r.waive.is_some(),
                    not_required_known: r.flags.is_some(),
                });
            }
            team[ci].add(&team_of(&planes));
            start += count;
        }
        drop(heads);
        drop(rows);
        drop(statements);
        for (r, own) in reviewers.iter_mut().zip(owns) {
            r.rules[ci] = own;
        }
    }
    // the report
    let names: Vec<String> = pack.checks.iter().map(|c| c.name.clone()).collect();
    let mut people = Vec::new();
    for r in &reviewers {
        let (waive, flags) = (r.waive.is_some(), r.flags.is_some());
        let mut all = Own::default();
        let mut by_rule = Vec::new();
        let mut differ = Vec::new();
        for (ci, own) in r.rules.iter().enumerate() {
            all.add(own);
            if own.any() {
                let mut row = own.json(id.sizes[ci], waive, flags);
                row["index"] = json!(ci + 1);
                row["name"] = json!(names[ci]);
                by_rule.push(row);
            }
            if let Some(stored) = &r.stored {
                if u64::from(stored[ci]) != own.waived {
                    differ.push(json!({"index": ci + 1, "name": names[ci], "stored": stored[ci], "counted": own.waived}));
                }
            }
        }
        let mut person = all.json(total, waive, flags);
        person["id"] = json!(r.tag);
        person["present"] = json!(true);
        person["waive_known"] = json!(waive);
        person["visited_known"] = json!(flags);
        person["not_required_known"] = json!(flags);
        if let Some((path, place)) = &r.waive {
            person["waive_file"] = json!(path);
            person["waive_location"] = json!(place);
            person["waive_modified"] = json!(modified(path));
            // the sidecar's stored counters against the statuses counted
            person["counters_match"] = json!(differ.is_empty());
            if !differ.is_empty() {
                person["counter_mismatch"] = json!(differ);
            }
        }
        if let Some((path, place)) = &r.flags {
            person["review_file"] = json!(path);
            person["review_location"] = json!(place);
            person["review_modified"] = json!(modified(path));
        }
        person["rules"] = json!(by_rule);
        people.push(person);
    }
    for tag in absent {
        people.push(json!({"id": tag, "present": false, "rules": []}));
    }
    let mut whole = Team::default();
    let mut team_rules = Vec::with_capacity(rules);
    for (ci, t) in team.iter().enumerate() {
        whole.add(t);
        let mut row = t.json(id.sizes[ci]);
        row["index"] = json!(ci + 1);
        row["name"] = json!(names[ci]);
        team_rules.push(row);
    }
    let mut team_json = whole.json(total);
    team_json["reviewers"] = json!(reviewers.len());
    team_json["rules"] = json!(team_rules);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(json!({
        "type": TYPE,
        "version": VERSION,
        "generated": utc(now),
        "database": crate::cache::database_path(&pack.path)?,
        "pack": pack.path,
        "fingerprint": id.fingerprint,
        "cell": pack.cell,
        "errors": total,
        "rules": names
            .iter()
            .enumerate()
            .map(|(ci, name)| json!({"index": ci + 1, "name": name, "errors": id.sizes[ci]}))
            .collect::<Vec<_>>(),
        "reviewers": people,
        "team": team_json,
        "skipped": skipped,
    }))
}

fn next_flags(rows: &mut Option<rusqlite::Rows<'_>>) -> Result<Option<(u64, Bits, Bits)>> {
    let Some(rows) = rows else { return Ok(None) };
    match rows.next().map_err(sql)? {
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

/// A waive sidecar of this result, positioned at its statuses, and its
/// stored counters.
fn open_waive(path: &Path, id: &Identity) -> Result<(File, Vec<u32>)> {
    let mut file = File::open(path)?;
    let (total, rules) = (id.total(), id.sizes.len());
    let mut head = [0u8; 40];
    let matches = file.read_exact_at(&mut head, 0).is_ok()
        && head == id.waive_header
        && file.metadata()?.len() == 40 + total + 4 * rules as u64;
    if !matches {
        return Err(Error::new(
            ErrorKind::Cache,
            "waive file of another DRC result (its header or size is not this pack's)",
        ));
    }
    let mut raw = vec![0u8; rules * 4];
    file.read_exact_at(&mut raw, 40 + total)?;
    let stored = raw
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    file.seek(SeekFrom::Start(40))?;
    Ok((file, stored))
}

/// A flags file of this result, in a read transaction.
fn open_flags(path: &Path, id: &Identity) -> Result<Connection> {
    let conn = open_read_only(path, Duration::from_secs(10))?;
    let text: String = conn
        .query_row("SELECT value FROM metadata WHERE id=1", [], |r| r.get(0))
        .map_err(sql)?;
    let meta: Value = serde_json::from_str(&text)
        .map_err(|e| Error::new(ErrorKind::Cache, format!("review flags metadata: {e}")))?;
    let sizes: Option<Vec<u64>> = meta
        .get("sizes")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().map(Value::as_u64).collect());
    if meta.get("fingerprint").and_then(Value::as_str) != Some(id.fingerprint.as_str())
        || sizes.as_deref() != Some(&id.sizes[..])
    {
        return Err(Error::new(
            ErrorKind::Cache,
            "review flags of another DRC result (their content id or rule sizes are not this pack's)",
        ));
    }
    conn.execute_batch("BEGIN").map_err(sql)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_working_file_names_its_reviewer_and_leftovers_do_not() {
        for tag in ["kim", "Kim.Lee_corp", "10.1.2.3", "김철수", "a-b", "v1.stale", "x.tmp"] {
            assert_eq!(reviewer_of(tag), Some(tag));
        }
        for tag in ["", "kim.stale-1794300165", "kim.tmp-4242", "a/b", "a b", "kim@x"] {
            assert_eq!(reviewer_of(tag), None, "{tag}");
        }
        assert_eq!(reviewer_of(&"k".repeat(201)), None);
    }

    #[test]
    fn times_are_utc_iso8601() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc(1_794_300_141), "2026-11-10T08:42:21Z");
        assert_eq!(utc(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn waived_statuses_become_bits() {
        let mut statuses = vec![0u8; 200];
        for i in [0usize, 63, 64, 130, 199] {
            statuses[i] = 1;
        }
        statuses[5] = 2; // a reserved status is not waived
        let bits = waived_bits(&statuses);
        assert_eq!(bits.ones().collect::<Vec<_>>(), vec![0, 63, 64, 130, 199]);
        assert!(waived_bits(&[]).is_zero());
    }

    fn bits(ones: &[u64]) -> Bits {
        let mut b = Bits::ZERO;
        for &i in ones {
            b.set(i);
        }
        b
    }

    fn planes(v: &[u64], w: &[u64], n: &[u64], flags: bool, waive: bool) -> Planes {
        Planes {
            visited: bits(v),
            waived: bits(w),
            not_required: bits(n),
            visited_known: flags,
            waive_known: waive,
            not_required_known: flags,
        }
    }

    #[test]
    fn the_team_counts_are_the_reviewers_windows() {
        // kim visited 0..5 and waived 0..10, excluded 100..110; lee visited
        // 0, 5 and 20 and waived 5..15: error 0 is a conflict (both visited
        // it, one waived), 5..10 agree, 10..15 lee's alone (not visited by
        // kim: no conflict); park has a waive file alone (visits unknown)
        let kim = planes(&[0, 1, 2, 3, 4], &(0..10).collect::<Vec<_>>(), &(100..110).collect::<Vec<_>>(), true, true);
        let lee = planes(&[0, 5, 20], &(5..15).collect::<Vec<_>>(), &[], true, true);
        let park = planes(&[], &[], &[], false, true);
        let t = team_of(&[kim, lee, park]);
        assert_eq!(
            t,
            Team { visited: 7, waived: 14, not_required: 10, conflicts: 1, done: 17 }
        );
        // an exclusion two visitors disagree on is a conflict too, and is not
        // counted as excluded; a waive-only file never conflicts
        let a = planes(&[1], &[], &[1], true, true);
        let b = planes(&[1], &[], &[], true, true);
        let c = planes(&[], &[1], &[], false, true);
        let t = team_of(&[a, b, c]);
        assert_eq!(
            t,
            Team { visited: 1, waived: 0, not_required: 0, conflicts: 1, done: 1 }
        );
        // nobody: nothing
        assert_eq!(team_of(&[]), Team::default());
        // one reviewer's own numbers: waived whether visited or not
        let t = team_of(&[planes(&[3], &[3, 4], &[9], true, true)]);
        assert_eq!(
            t,
            Team { visited: 1, waived: 2, not_required: 1, conflicts: 0, done: 2 }
        );
    }
}
