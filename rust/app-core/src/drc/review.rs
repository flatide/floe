//! Portable review sidecars and transactional in-memory note groups.
//! No path lookup, autosave, pack mutation, reviewer authority or HTTP endpoint.
//! Legacy source size/mtime is a compatibility tag, NOT a unique run identity.
//! The eventual store must also bind an open pack identity and review revision.
use crate::{annotations, check_cancelled, Error, ErrorKind, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    sync::atomic::AtomicUsize,
};

pub const NOTE_BYTES: usize = 64 * 1024;
pub const NOTE_MEMBERS: usize = 100_000;
pub const EDIT_ITEMS: usize = 5000;
pub const SIDECAR_BYTES: usize = annotations::MAX_TEXT;
const STATUS_CHUNK: usize = 64 * 1024;
const MAX_CHECKS: usize = super::META_BYTES / 16;
pub mod managed;
pub mod store;

/// Opaque local pack identity, not an HTTP revision or authorization token.
/// Produced by a registered store and compared with the reader's open pack.
/// Deliberately not serializable: do not expose filesystem identity on the wire.
#[derive(Clone, PartialEq, Eq)]
pub struct Identity(pub(super) Vec<u8>);

/// Bounded positional reads, preserving caller order, duplicates and reserved
/// status bytes. Adjacent IDs share one read; sparse IDs never read the gap.
/// The caller validates the file identity before/after this operation.
pub(super) fn selected_statuses(
    file: &std::fs::File,
    base: u64,
    total: u64,
    gids: &[u64],
    stop: &AtomicUsize,
) -> Result<Vec<u8>> {
    use std::os::unix::fs::FileExt;
    check_cancelled(stop)?;
    if gids.len() > EDIT_ITEMS {
        return Err(bounded("selected status count"));
    }
    if gids.iter().any(|&id| id >= total) {
        return Err(Error::input("review error index out of range"));
    }
    let mut sorted: Vec<_> = gids.iter().copied().zip(0..gids.len()).collect();
    sorted.sort_unstable();
    let mut result = vec![0; gids.len()];
    let mut bytes = vec![0; gids.len()];
    let mut start = 0;
    while start < sorted.len() {
        check_cancelled(stop)?;
        let first = sorted[start].0;
        let mut last = first;
        let mut end = start + 1;
        while end < sorted.len() && sorted[end].0 - last <= 1 {
            last = sorted[end].0;
            end += 1;
        }
        let n = (last - first + 1) as usize; // <= bounded number of selected IDs
        let offset = base
            .checked_add(first)
            .filter(|o| o.checked_add(n as u64).is_some())
            .ok_or_else(|| Error::input("review status offset overflow"))?;
        file.read_exact_at(&mut bytes[..n], offset)?;
        for &(id, index) in &sorted[start..end] {
            result[index] = bytes[(id - first) as usize];
        }
        start = end;
    }
    check_cancelled(stop)?;
    Ok(result)
}

fn bounded(what: &str) -> Error {
    Error::new(
        ErrorKind::Incomplete,
        format!("DRC review {what} limit; no complete result"),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub source_size: u64,
    pub source_mtime: u64,
    pub total: u64,
}
impl Fingerprint {
    fn tag(self) -> String {
        format!("{},{},{}", self.source_size, self.source_mtime, self.total)
    }
}

/// Checked file-order layout. Zero-error rules are retained in the counter table.
#[derive(Clone, Debug)]
pub struct Layout {
    fingerprint: Fingerprint,
    counts: Vec<u32>,
}
impl Layout {
    pub fn new(fingerprint: Fingerprint, counts: &[u64]) -> Result<Self> {
        if counts.len() > MAX_CHECKS {
            return Err(bounded("check count"));
        }
        let mut total = 0u64;
        let mut checked = Vec::new();
        checked
            .try_reserve_exact(counts.len())
            .map_err(|_| bounded("allocation"))?;
        for &n in counts {
            total = total
                .checked_add(n)
                .ok_or_else(|| Error::input("review count overflow"))?;
            checked
                .push(u32::try_from(n).map_err(|_| Error::input("waive rule count exceeds u32"))?);
        }
        if total != fingerprint.total || total.checked_add(40 + counts.len() as u64 * 4).is_none() {
            return Err(Error::input("review error counts do not match the pack"));
        }
        Ok(Self {
            fingerprint,
            counts: checked,
        })
    }
    pub fn from_pack(pack: &super::Pack) -> Result<Self> {
        pack.unchanged()?;
        Self::new(
            Fingerprint {
                source_size: pack.source_size,
                source_mtime: pack.source_mtime,
                total: pack.total,
            },
            &pack.checks.iter().map(|c| c.count).collect::<Vec<_>>(),
        )
    }
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }
    pub fn header(&self) -> [u8; 40] {
        let mut h = [0; 40];
        h[..8].copy_from_slice(b"FLOEWAIV");
        h[8..12].copy_from_slice(&1u32.to_le_bytes());
        h[12..20].copy_from_slice(&self.fingerprint.source_size.to_le_bytes());
        h[20..28].copy_from_slice(&self.fingerprint.source_mtime.to_le_bytes());
        h[28..36].copy_from_slice(&self.fingerprint.total.to_le_bytes());
        h[36..40].copy_from_slice(&(self.counts.len() as u32).to_le_bytes());
        h
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct WaiveStats {
    pub waived: u64,
    pub per_rule: Vec<u32>,
}
/// Validate/import and optionally edit a FLOEWAIV v1 stream. Counter bytes are
/// never trusted: recompute status==1 counts, preserving every reserved byte.
/// `output` MUST be an unpublished staging sink: an EOF/I/O/cancel failure can
/// leave a prefix there. Ok is not a filesystem commit or durability receipt.
/// Memory is O(checks + edits + 64KiB), not O(total errors). Blocking Read/Write
/// cannot be interrupted; cancellation is checked between bounded chunks.
pub fn rewrite_waives(
    mut input: impl Read,
    mut output: impl Write,
    layout: &Layout,
    changes: &[(u64, u8)],
    stop: &AtomicUsize,
) -> Result<WaiveStats> {
    check_cancelled(stop)?;
    if changes.len() > EDIT_ITEMS {
        return Err(bounded("edit count"));
    }
    if changes
        .iter()
        .any(|(gid, _)| *gid >= layout.fingerprint.total)
    {
        return Err(Error::input("review error index out of range"));
    }
    let changes: BTreeMap<_, _> = changes.iter().copied().collect(); // last assignment wins
    let mut h = [0; 40];
    input.read_exact(&mut h)?;
    if h != layout.header() {
        return Err(Error::input("waive sidecar does not match this DRC pack"));
    }
    output.write_all(&h)?;
    let mut bytes = [0; STATUS_CHUNK];
    let mut counts = Vec::with_capacity(layout.counts.len());
    let mut start = 0u64;
    for (ci, &count) in layout.counts.iter().enumerate() {
        if ci % 1024 == 0 {
            check_cancelled(stop)?;
        }
        let mut remaining = u64::from(count);
        let mut waived = 0u32;
        while remaining > 0 {
            check_cancelled(stop)?;
            let n = remaining.min(STATUS_CHUNK as u64) as usize;
            input.read_exact(&mut bytes[..n])?;
            for (&gid, &value) in changes.range(start..start + n as u64) {
                bytes[(gid - start) as usize] = value;
            }
            waived += bytes[..n].iter().filter(|&&b| b == 1).count() as u32;
            output.write_all(&bytes[..n])?;
            start += n as u64;
            remaining -= n as u64;
        }
        counts.push(waived);
    }
    let mut remaining = layout.counts.len() * 4;
    while remaining > 0 {
        check_cancelled(stop)?;
        let n = remaining.min(bytes.len());
        input.read_exact(&mut bytes[..n])?;
        remaining -= n;
    }
    let mut tail = [0; 1];
    loop {
        check_cancelled(stop)?;
        match input.read(&mut tail) {
            Ok(0) => break,
            Ok(_) => return Err(Error::input("trailing bytes in waive sidecar")),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    // Only canonical counters reach the staging sink, after complete input validation.
    for chunk in counts.chunks(STATUS_CHUNK / 4) {
        check_cancelled(stop)?;
        for (slot, count) in bytes.chunks_exact_mut(4).zip(chunk) {
            slot.copy_from_slice(&count.to_le_bytes());
        }
        output.write_all(&bytes[..chunk.len() * 4])?;
    }
    check_cancelled(stop)?;
    Ok(WaiveStats {
        waived: counts.iter().map(|&n| u64::from(n)).sum(),
        per_rule: counts,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub members: BTreeSet<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notes {
    fingerprint: Fingerprint,
    groups: BTreeMap<u64, Note>,
    by_gid: BTreeMap<u64, u64>,
    next: u64,
    text_bytes: usize,
}
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct ImportReport {
    pub skipped_lines: usize,
    pub invalid_members: usize,
    pub reassigned_members: usize,
}
fn annotation(at: [f64; 2], text: &str) -> Result<annotations::Annotation> {
    annotations::Annotation::from_json(
        serde_json::json!({"kind":"text", "at":at, "text":text, "color":"#FFD819"}),
    )
}
fn append(out: &mut String, s: &str) -> Result<()> {
    if s.len() > SIDECAR_BYTES - out.len() {
        return Err(bounded("note export bytes"));
    }
    out.push_str(s);
    Ok(())
}
impl Notes {
    pub fn new(fingerprint: Fingerprint) -> Self {
        Self {
            fingerprint,
            groups: BTreeMap::new(),
            by_gid: BTreeMap::new(),
            next: 1,
            text_bytes: 0,
        }
    }
    pub fn get(&self, gid: u64) -> Option<&str> {
        self.groups
            .get(self.by_gid.get(&gid)?)
            .map(|n| n.text.as_str())
    }
    /// Creation order, with sorted members; matches GTK's shared-note semantics.
    pub fn groups(&self) -> impl Iterator<Item = &Note> {
        self.groups.values()
    }
    pub fn member_count(&self) -> usize {
        self.by_gid.len()
    }
    /// All input/limits/cancellation checks precede mutation. Empty/whitespace
    /// text clears only these members; remaining members retain the old note.
    pub fn set(&mut self, gids: &[u64], text: &str, stop: &AtomicUsize) -> Result<()> {
        if gids.len() > EDIT_ITEMS {
            return Err(bounded("edit count"));
        }
        let ids = gids.iter().copied().collect();
        self.assign(ids, text, stop)
    }
    fn assign(&mut self, ids: BTreeSet<u64>, text: &str, stop: &AtomicUsize) -> Result<()> {
        check_cancelled(stop)?;
        if ids.iter().any(|&g| g >= self.fingerprint.total) {
            return Err(Error::input("note error index out of range"));
        }
        let text = text.trim();
        if text.len() > NOTE_BYTES {
            return Err(bounded("note text"));
        }
        if !text.is_empty() {
            annotation([0., 0.], text)?;
        }
        let add = !text.is_empty() && !ids.is_empty();
        let next = if add {
            self.next
                .checked_add(1)
                .ok_or_else(|| bounded("note sequence"))?
        } else {
            self.next
        };
        let mut removed = BTreeMap::<u64, usize>::new();
        let mut old_members = 0;
        for (i, gid) in ids.iter().enumerate() {
            if i % 1024 == 0 {
                check_cancelled(stop)?;
            }
            if let Some(&nid) = self.by_gid.get(gid) {
                *removed.entry(nid).or_default() += 1;
                old_members += 1;
            }
        }
        let reclaimed: usize = removed
            .iter()
            .filter_map(|(nid, n)| {
                let old = &self.groups[nid];
                (*n == old.members.len()).then_some(old.text.len())
            })
            .sum();
        let text_bytes = self.text_bytes - reclaimed + if add { text.len() } else { 0 };
        if text_bytes > SIDECAR_BYTES
            || self.by_gid.len() - old_members + if add { ids.len() } else { 0 } > NOTE_MEMBERS
        {
            return Err(bounded("note membership/text"));
        }
        check_cancelled(stop)?;
        // Infallible commit after validation; no cancellation midway through a group edit.
        for gid in &ids {
            if let Some(nid) = self.by_gid.remove(gid) {
                let old = self.groups.get_mut(&nid).unwrap();
                old.members.remove(gid);
                if old.members.is_empty() {
                    self.groups.remove(&nid);
                }
            }
        }
        if add {
            for gid in &ids {
                self.by_gid.insert(*gid, self.next);
            }
            self.groups.insert(
                self.next,
                Note {
                    text: text.into(),
                    members: ids,
                },
            );
        }
        self.next = next;
        self.text_bytes = text_bytes;
        Ok(())
    }
    /// Parse into a separate model; caller replaces its live model only on Ok.
    /// Require exactly one matching fingerprint (legacy missing/last-tag-wins
    /// behavior cannot safely bind global error IDs to a run). Ignore text=
    /// mirrors. Malformed records/IDs are counted; overlapping groups use last
    /// assignment and detach old membership so serialization cannot resurrect it.
    pub fn parse(
        input: &str,
        fingerprint: Fingerprint,
        stop: &AtomicUsize,
    ) -> Result<(Self, ImportReport)> {
        check_cancelled(stop)?;
        if input.len() > SIDECAR_BYTES {
            return Err(bounded("note import bytes"));
        }
        let mut tags = input
            .lines()
            .map(str::trim)
            .filter_map(|s| s.strip_prefix("floe_pack="));
        if tags.next() != Some(fingerprint.tag().as_str()) || tags.next().is_some() {
            return Err(Error::input(
                "notes require one matching DRC pack fingerprint",
            ));
        }
        let mut notes = Self::new(fingerprint);
        let mut report = ImportReport::default();
        for raw in input.lines() {
            check_cancelled(stop)?;
            let Some(body) = raw.trim().strip_prefix("floe_note=") else {
                continue;
            };
            let Some((ids, txt)) = body.split_once('|') else {
                report.skipped_lines += 1;
                continue;
            };
            let mut members = BTreeSet::new();
            for (i, id) in ids.split(',').enumerate() {
                if i % 1024 == 0 {
                    check_cancelled(stop)?;
                }
                let id = id.trim();
                let gid = if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                    id.parse::<u64>().ok()
                } else {
                    None
                };
                if let Some(gid) = gid.filter(|&g| g < fingerprint.total) {
                    members.insert(gid);
                } else {
                    report.invalid_members += 1;
                }
                if members.len() > NOTE_MEMBERS {
                    return Err(bounded("note members"));
                }
            }
            let text = annotations::unescape(txt);
            if members.is_empty() || text.trim().is_empty() {
                report.skipped_lines += 1;
                continue;
            }
            report.reassigned_members += members
                .iter()
                .filter(|gid| notes.by_gid.contains_key(gid))
                .count();
            notes.assign(members, &text, stop)?;
        }
        check_cancelled(stop)?;
        Ok((notes, report))
    }
    /// Valid FE text mirrors at exact error bbox centers. Callback errors abort
    /// export, never fabricate (0,0). None denotes an empty model, NOT permission
    /// to delete a file. File replacement/deletion belongs to a separate store.
    pub fn serialize(
        &self,
        mut center: impl FnMut(u64) -> Result<[f64; 2]>,
        stop: &AtomicUsize,
    ) -> Result<Option<String>> {
        check_cancelled(stop)?;
        if self.groups.is_empty() {
            return Ok(None);
        }
        let mut out = format!(
            "# flateyes annotations\nfloe_pack={}\nppu=1\nunit=um\n",
            self.fingerprint.tag()
        );
        for note in self.groups.values() {
            append(&mut out, "floe_note=")?;
            for (i, gid) in note.members.iter().enumerate() {
                if i % 1024 == 0 {
                    check_cancelled(stop)?;
                }
                if i != 0 {
                    append(&mut out, ",")?;
                }
                append(&mut out, &gid.to_string())?;
            }
            append(&mut out, "|")?;
            append(&mut out, &annotations::escape(&note.text))?;
            append(&mut out, "\n")?;
        }
        for note in self.groups.values() {
            for &gid in &note.members {
                check_cancelled(stop)?;
                append(&mut out, annotation(center(gid)?, &note.text)?.as_line())?;
                append(&mut out, "\n")?;
            }
        }
        check_cancelled(stop)?;
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests;
