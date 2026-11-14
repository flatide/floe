//! The desktop viewer's DRC review: floe/drc.py IcePack's review state, for
//! `floe2 gtk-service` (docs/SHARED_APP_LAYER.ko.md P2b). The files are the
//! review store's (`review`): the per-reviewer waive sidecar
//! `.<db>.waive.<tag>` (FLOEWAIV, version 2) and the notes `.<db>.notes.<tag>.fe`
//! beside the pack, byte for byte. What differs from the web's transactional
//! store is how they are kept, as the GTK viewer always kept them:
//!
//! - every waive click is written at once - one status byte and the rule's
//!   waived counter, in place (a sidecar of 100M errors is never rewritten) -
//!   and flushed to the disk before the request answers (`sync_waives`,
//!   0.12.446: the statistics are read by a command of their own while the
//!   viewer runs, on any host; FLOE_WAIVE_SYNC=off leaves the bytes to the
//!   kernel's writeback as before);
//! - a sidecar of another DRC run (header or size) is moved aside
//!   (`.stale-<epoch>`) and a fresh one seeded from the pack's embedded
//!   sections; the first open creates it;
//! - a results folder that cannot be written keeps the review in the system
//!   temp folder (path-hashed names); when that fails too the pack's own
//!   embedded statuses are shown read-only - one reviewer never changes a
//!   shared result (feature/jobdeck-drc 2026-10-09);
//! - notes are rewritten whole on every edit and the file removed when the
//!   last note goes; the reader takes what Python wrote (lenient lines);
//! - why errors were waived or set aside from the review is said once for
//!   the action, in `.<db>.reasons.<tag>` (`reasons`, 0.12.463): a line an
//!   action, whatever its size. The status writers here drop the reason of
//!   an error they turn back or change without one (`reason_drop`), so a
//!   reason is only ever held by an error that is waived;
//! - a waive a confirmer has confirmed is locked (`confirm::Locks`,
//!   0.12.467): the status writers here leave it as it is - waived, with the
//!   reason it has - unless the session is a confirmer's, and say which
//!   errors they left. A confirmer who turns a waive back takes their own
//!   confirmation of it back with it;
//! - a waive a confirmer has rejected is a state of its own (the
//!   confirmers' rejects logs, 0.12.468; a state since 0.12.469): the waive
//!   stays in this file with its reason, and reads as no waive - `statuses`
//!   says 0 of it, `states` says rejected - until its reviewer waives the
//!   error again with a reason (`resubmit`, which keeps the rejection and
//!   the reason the waive had in this reviewer's own rejects log), turns it
//!   back, or the confirmer takes the rejection back. The status writers do
//!   not waive an error under a rejection its reviewer has not answered,
//!   and say which they left; `look` is what a host calls as often as it
//!   wants the review to follow the confirmers (the viewer every couple of
//!   seconds); a confirmer rejects with `reject_set` and takes it back with
//!   `reject_drop`;
//! - what a status byte does not say - rejected, confirmed - is read beside
//!   it (`Overlay`), by the review and by its readers on other threads, and
//!   a host's filter picks the class of errors those readers say 1 of
//!   (`Lens`: the counts, pages and ranks of every scope follow it);
//! - a session of a role (`Role`, 0.12.472: "error -> waive request ->
//!   waive -> confirm ... role은 requester, waiver, confirmer로 나누어야 함")
//!   does its role's step and no other. A requester's marks are requests,
//!   kept in a file of their own (`.<db>.request.<tag>`, the status store
//!   of that session); a waiver waives the errors somebody asked for, and
//!   no other; a confirmer confirms and rejects and writes no status. A
//!   request is locked while somebody has its error waived, as a waive is
//!   while it is confirmed. The states such a session reads are the
//!   ERROR's - made of every reviewer's request file, waive sidecar and
//!   confirm file (`confirm::Peers`, as the last `look` found them): not
//!   waived, waive requested, waived, confirmed, rejected. A session
//!   without a role is a reviewer as before, and reads its own waives.
//!
//! A sidecar is bound to the result's content (the pack's content id and
//! error count - what the errors are), not to a file's time or inode: a
//! re-pack, a copy of the folder, a .db whose time alone changed and a
//! re-run with the same errors keep their review (0.12.441).
use super::{
    changes::{ChangeLog, CHUNK as CHANGE_CHUNK},
    confirm::{self, ConfirmFiles, Confirms, Locks, Peers, Rejected, Standing, Verdict},
    local::{aside_path, pack_reader, required_reviewer, waive_paths, ReviewerSource, Roles},
    reasons::{self, Kind, Reasons, Run},
    review::Layout,
    session, Pack, Violation,
};
use crate::{annotations, check_cancelled, Error, ErrorKind, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::FileExt;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub const STATUS_NONE: u8 = 0;
pub const STATUS_WAIVED: u8 = 1;
/// An error's state as its reviewer has it (`Review::states`; user
/// 2026-11-14: "메인 목록에서 상태 4가지로 필터할 수 있어야 하고 reject 된
/// 에러는 rejected로 분류되어야"): not waived; waived, and no confirmer has
/// looked at it yet; waived, and a confirmer has rejected the waive; waived
/// and confirmed.
pub const STATE_NONE: u8 = 0;
pub const STATE_WAIVED: u8 = 1;
pub const STATE_REJECTED: u8 = 2;
pub const STATE_CONFIRMED: u8 = 3;
/// A requester has asked for the error's waive and nobody has waived it
/// (0.12.472; user 2026-11-14: "error 상태에 waive requested가 추가되어야
/// 함"). A state of a role's session, whose states are the error's - what
/// everybody's files say of it -, not one reviewer's waive's.
pub const STATE_REQUESTED: u8 = 4;
/// Somebody asked for the error's waive and a waiver declined every such
/// request (0.12.473; user 2026-11-14: "요청 거절 상태도 만들어줘"): it
/// waits for its requester, who asks again with a reason - as a rejected
/// waive waits for its waiver.
pub const STATE_DECLINED: u8 = 5;

/// Which file a session's own marks are in - its highest marking role's: a
/// waiver's waive sidecar (and a session's without a role), else a
/// requester's request file, else none (a session that only confirms makes
/// no mark, and no file for one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store {
    None,
    Requests,
    Waives,
}

impl Store {
    pub fn of(roles: Roles) -> Self {
        if roles.waiver || !roles.any() {
            Store::Waives
        } else if roles.requester {
            Store::Requests
        } else {
            Store::None
        }
    }

    /// The verdict there is on a mark of this store: a confirmer's
    /// rejection of a waive, a waiver's decline of a request.
    fn verdict(self) -> Verdict {
        match self {
            Store::Requests => Verdict::DECLINE,
            _ => Verdict::REJECT,
        }
    }
}

/// Which errors the status readers of a session say 1 of (`Review::lensed`,
/// `StatusReader::lensed`): the class a host's filter shows. Every scope's
/// counts, pages and ranks are made of those bytes, so a filter of four
/// states costs the scopes nothing new.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lens {
    /// waived and not rejected, confirmed or not: what "waived" has always
    /// been
    Live,
    /// waived in this reviewer's file, rejected or not: every state but
    /// "not waived"
    Held,
    /// waived, not confirmed yet
    Waived,
    Confirmed,
    Rejected,
    /// waive requested, and waived by nobody yet (a role's session: none
    /// of a session without a role)
    Requested,
    /// waive requested, and every request of it declined by a waiver
    Declined,
}

impl Lens {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "live" => Some(Lens::Live),
            "held" => Some(Lens::Held),
            "waived" => Some(Lens::Waived),
            "confirmed" => Some(Lens::Confirmed),
            "rejected" => Some(Lens::Rejected),
            "requested" => Some(Lens::Requested),
            "declined" => Some(Lens::Declined),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Lens::Live => "live",
            Lens::Held => "held",
            Lens::Waived => "waived",
            Lens::Confirmed => "confirmed",
            Lens::Rejected => "rejected",
            Lens::Requested => "requested",
            Lens::Declined => "declined",
        }
    }
    fn of(byte: u8) -> Self {
        match byte {
            1 => Lens::Held,
            2 => Lens::Waived,
            3 => Lens::Confirmed,
            4 => Lens::Rejected,
            5 => Lens::Requested,
            6 => Lens::Declined,
            _ => Lens::Live,
        }
    }
    /// Whether its class asks who confirmed (the confirm files are read).
    fn confirms(self) -> bool {
        matches!(self, Lens::Waived | Lens::Confirmed)
    }
    /// Whether a state is of its class.
    fn has(self, state: u8) -> bool {
        match self {
            Lens::Live => state == STATE_WAIVED || state == STATE_CONFIRMED,
            Lens::Held => state != STATE_NONE,
            Lens::Waived => state == STATE_WAIVED,
            Lens::Confirmed => state == STATE_CONFIRMED,
            Lens::Rejected => state == STATE_REJECTED,
            Lens::Requested => state == STATE_REQUESTED,
            Lens::Declined => state == STATE_DECLINED,
        }
    }
    /// The class's place among a rule's errors by state (`Review::tally`).
    fn count(self, by_state: &[u64; 6]) -> u64 {
        (0..6u8).filter(|&state| self.has(state)).map(|state| by_state[state as usize]).sum()
    }
}

/// What a status byte does not say and its readers need: which of this
/// reviewer's waives are rejected, where the confirmers' files are - and
/// the class the host's filter shows. One a review, shared with its readers
/// on other threads.
pub struct Overlay {
    lens: AtomicU8,
    /// this reviewer's errors under a rejection they have not answered, by
    /// rule (sorted runs): a waive of one of them is rejected
    rejected: RwLock<BTreeMap<usize, Arc<Vec<Run>>>>,
    /// every confirmer's confirm file as the locks last found them
    confirms: ConfirmFiles,
    /// which file the session's own marks are in (`Store`)
    store: Store,
    /// a role's session: the states are the error's - made of everybody's
    /// files, `peers` - and the status file is this reviewer's own stage
    /// of them
    team: bool,
    /// every reviewer's request file, waive sidecar and confirm file as the
    /// last look found them (a role's session; this session's own status
    /// file is not among them)
    peers: RwLock<Arc<Peers>>,
}

impl Overlay {
    fn new(confirms: ConfirmFiles, store: Store, team: bool) -> Self {
        Self {
            lens: AtomicU8::new(Lens::Live as u8),
            rejected: RwLock::default(),
            confirms,
            store,
            team,
            peers: RwLock::default(),
        }
    }

    pub fn lens(&self) -> Lens {
        Lens::of(self.lens.load(Ordering::Relaxed))
    }

    fn peers(&self) -> Arc<Peers> {
        Arc::clone(&self.peers.read().unwrap_or_else(|p| p.into_inner()))
    }

    fn set_peers(&self, peers: Peers) {
        *self.peers.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(peers);
    }

    /// Status bytes `raw` of rule `check`'s errors from `start` become this
    /// reviewer's own mark of each: 1 where the file has the error marked
    /// and - a waive - the mark is not rejected. What decides an action of
    /// this reviewer's, whatever everybody else's files say.
    fn own(&self, check: usize, start: u64, raw: &mut [u8]) {
        for s in raw.iter_mut() {
            *s = u8::from(*s == STATUS_WAIVED);
        }
        if !raw.contains(&1) {
            return;
        }
        let stop = start + raw.len() as u64;
        if let Some(runs) = self.rejected(check) {
            let mut i = runs.partition_point(|r| r.1 <= start);
            while let Some(&(a, b)) = runs.get(i) {
                if a >= stop {
                    break;
                }
                raw[(a.max(start) - start) as usize..(b.min(stop) - start) as usize].fill(0);
                i += 1;
            }
        }
    }

    /// Status bytes `raw` of rule `check`'s errors from `start` (`first`:
    /// the rule's first error among all) become the errors' states as
    /// everybody's files say them - a role's session. This session's own
    /// file is its store's stage (a requester's requests, a waiver's
    /// waives); the other reviewers' request files, waive sidecars and
    /// confirm files are the ones the last look found. An error is waived
    /// while somebody's waive of it is not rejected, confirmed when a
    /// confirmer confirmed that, rejected when every waive of it is;
    /// requested when somebody's request of it is not declined and nobody
    /// waived, declined when every request of it is.
    fn team(&self, check: usize, first: u64, start: u64, raw: &mut [u8]) {
        // somebody's request file has the error; ... in a request that is
        // not declined; somebody's waive sidecar has it; ... in a waive
        // that is not rejected; a confirmer confirmed it
        const ASKED: u8 = 1;
        const ASKING: u8 = 16;
        const HELD: u8 = 2;
        const LIVE: u8 = 4;
        const SEEN: u8 = 8;
        let n = raw.len();
        let peers = self.peers();
        let stop = start + n as u64;
        let mut bits = vec![0u8; n];
        let clear = |runs: &[Run], bits: &mut [u8], bit: u8| {
            let mut i = runs.partition_point(|r| r.1 <= start);
            while let Some(&(a, b)) = runs.get(i) {
                if a >= stop {
                    break;
                }
                for b in &mut bits[(a.max(start) - start) as usize..(b.min(stop) - start) as usize] {
                    *b &= !bit;
                }
                i += 1;
            }
        };
        // this session's own marks: its store's stage, less the ones under
        // a verdict its reviewer has not answered
        let own = match self.store {
            Store::Requests => Some((ASKED, ASKING)),
            Store::Waives => Some((HELD, LIVE)),
            Store::None => None,
        };
        if let Some((held, live)) = own {
            for (bit, s) in bits.iter_mut().zip(raw.iter()) {
                if *s == STATUS_WAIVED {
                    *bit = held | live;
                }
            }
            if let Some(runs) = self.rejected(check) {
                clear(&runs, &mut bits, live);
            }
        }
        let mut bytes = vec![0u8; n];
        let mut marks = vec![0u8; n];
        for (group, held, live) in [(&peers.requests, ASKED, ASKING), (&peers.waives, HELD, LIVE)] {
            for peer in group.iter().filter(|p| p.any(check)) {
                if !peer.read(first + start, &mut bytes) {
                    continue;
                }
                // (this file's marks, less the ones its reviewer has not
                // answered a verdict on)
                for (m, b) in marks.iter_mut().zip(&bytes) {
                    *m = if *b == STATUS_WAIVED { held | live } else { 0 };
                }
                if let Some(runs) = peer.rejected.get(&check) {
                    clear(runs, &mut marks, live);
                }
                for (bit, m) in bits.iter_mut().zip(&marks) {
                    *bit |= m;
                }
            }
        }
        if bits.iter().any(|bit| bit & LIVE != 0) {
            for peer in peers.confirms.iter().filter(|p| p.any(check)) {
                if peer.read(first + start, &mut bytes) {
                    for (bit, b) in bits.iter_mut().zip(&bytes) {
                        if *b == confirm::CONFIRMED {
                            *bit |= SEEN;
                        }
                    }
                }
            }
        }
        for (s, bit) in raw.iter_mut().zip(&bits) {
            *s = if bit & LIVE != 0 {
                if bit & SEEN != 0 {
                    STATE_CONFIRMED
                } else {
                    STATE_WAIVED
                }
            } else if bit & HELD != 0 {
                STATE_REJECTED
            } else if bit & ASKING != 0 {
                STATE_REQUESTED
            } else if bit & ASKED != 0 {
                STATE_DECLINED
            } else {
                STATE_NONE
            };
        }
    }

    fn rejected(&self, check: usize) -> Option<Arc<Vec<Run>>> {
        self.rejected.read().unwrap_or_else(|p| p.into_inner()).get(&check).cloned()
    }

    /// The rules that have rejected errors.
    fn rejected_rules(&self) -> Vec<usize> {
        self.rejected.read().unwrap_or_else(|p| p.into_inner()).keys().copied().collect()
    }

    /// Rule `check`'s rejected errors are `runs` from now: whether they are
    /// others than they were.
    fn set_rejected(&self, check: usize, runs: Vec<Run>) -> bool {
        let mut held = self.rejected.write().unwrap_or_else(|p| p.into_inner());
        if held.get(&check).map_or(runs.is_empty(), |was| **was == runs) {
            return false;
        }
        if runs.is_empty() {
            held.remove(&check);
        } else {
            held.insert(check, Arc::new(runs));
        }
        true
    }

    fn any_confirms(&self) -> bool {
        !self.confirms.read().unwrap_or_else(|p| p.into_inner()).is_empty()
    }

    /// Status bytes `raw` of rule `check`'s errors from `start` (`first`:
    /// the rule's first error among all) become their states: a waive of a
    /// rejected error is rejected, and - `confirms` - one a confirmer has
    /// confirmed is confirmed (their files are read for it).
    fn states(&self, check: usize, first: u64, start: u64, raw: &mut [u8], confirms: bool) {
        if self.team {
            return self.team(check, first, start, raw);
        }
        let mut any = false;
        for s in raw.iter_mut() {
            if *s == STATUS_WAIVED {
                any = true;
            } else {
                *s = STATE_NONE;
            }
        }
        if !any {
            return;
        }
        let stop = start + raw.len() as u64;
        if let Some(runs) = self.rejected(check) {
            let mut i = runs.partition_point(|r| r.1 <= start);
            while let Some(&(a, b)) = runs.get(i) {
                if a >= stop {
                    break;
                }
                for s in &mut raw[(a.max(start) - start) as usize..(b.min(stop) - start) as usize] {
                    if *s == STATE_WAIVED {
                        *s = STATE_REJECTED;
                    }
                }
                i += 1;
            }
        }
        if !confirms || !raw.contains(&STATE_WAIVED) {
            return;
        }
        let files = Arc::clone(&self.confirms.read().unwrap_or_else(|p| p.into_inner()));
        let mut bytes = vec![0u8; raw.len()];
        for file in files.iter() {
            if file.read_exact_at(&mut bytes, confirm::HEADER + first + start).is_err() {
                continue;
            }
            for (s, c) in raw.iter_mut().zip(&bytes) {
                if *s == STATE_WAIVED && *c == confirm::CONFIRMED {
                    *s = STATE_CONFIRMED;
                }
            }
        }
    }

    /// `states`, then 1 where the state is of `lens`'s class and 0
    /// elsewhere.
    fn class(&self, check: usize, first: u64, start: u64, raw: &mut [u8], lens: Lens) {
        if lens == Lens::Held && !self.team {
            for s in raw.iter_mut() {
                *s = u8::from(*s == STATUS_WAIVED);
            }
            return;
        }
        self.states(check, first, start, raw, lens.confirms());
        for s in raw.iter_mut() {
            *s = u8::from(lens.has(*s));
        }
    }
}

/// Whether sorted, disjoint `runs` hold `error`.
fn within(runs: &[Run], error: u64) -> bool {
    let i = runs.partition_point(|r| r.1 <= error);
    runs.get(i).is_some_and(|r| r.0 <= error)
}

/// The parts of `runs` inside `inside` (both sorted and disjoint).
fn meet(runs: &[Run], inside: &[Run]) -> Vec<Run> {
    let mut out = Vec::new();
    let mut j = 0;
    for &(a, b) in runs {
        while j < inside.len() && inside[j].1 <= a {
            j += 1;
        }
        let mut k = j;
        while k < inside.len() && inside[k].0 < b {
            out.push((a.max(inside[k].0), b.min(inside[k].1)));
            k += 1;
        }
    }
    out
}
/// Status scan granularity: rank and page jumps read cached per-chunk
/// waived counts and touch at most one chunk of status bytes.
const STATUS_CHUNK: u64 = 1 << 22;
/// The colour of a note's flateyes text annotation.
const NOTE_COLOR: &str = "#FFD819";
const HEADER: u64 = 40;

/// What a status write left as it was: the errors whose waive is locked (a
/// confirmer has confirmed it, and the session is no confirmer's), and the
/// errors it would have waived that are under a rejection their reviewer
/// has not answered - those are waived again with a reason (`resubmit`), or
/// not at all.
///
/// A role's session (0.12.472): `locked` is also a requester's request of
/// an error somebody has waived, which is not taken back; and `unrequested`
/// the errors a waiver would have waived that nobody asked the waive of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Left<T> {
    pub locked: Vec<T>,
    pub rejected: Vec<T>,
    pub unrequested: Vec<T>,
}

impl<T> Left<T> {
    pub fn is_empty(&self) -> bool {
        self.locked.is_empty() && self.rejected.is_empty() && self.unrequested.is_empty()
    }
}

/// `Left`, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    pub locked: u64,
    pub rejected: u64,
    pub unrequested: u64,
}

/// Waives of this reviewer a look found rejected: a rule's, with who
/// rejected them and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Returned {
    pub check: usize,
    pub runs: Vec<Run>,
    pub count: u64,
    pub by: Vec<Rejected>,
}

/// What `Review::look` found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Looked {
    /// this reviewer's waives that are rejected and were not said by a
    /// look before: rule by rule
    pub rejected: Vec<Returned>,
    /// the rules whose errors are classed otherwise than before the look
    /// (their waives are rejected now, or no longer)
    pub rules: Vec<usize>,
    /// the confirmers' files are not what they were at the look before:
    /// what a host has read of the states is to be read again
    pub changed: bool,
}

/// One event of an error's waive, as its history says it (`Review::history`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Event {
    /// "waived": the reason the waive had before it was waived again;
    /// "rejected": a rejection that was answered by waiving again;
    /// "requested": the reason the request had before it was asked again;
    /// "declined": a decline that was answered by asking again
    pub what: &'static str,
    /// who rejected, or declined (empty for a reason: its reviewer's)
    pub by: String,
    pub text: String,
    /// seconds since the epoch
    pub time: u64,
}

fn push_run(runs: &mut Vec<Run>, error: u64) {
    match runs.last_mut() {
        Some(last) if last.1 == error => last.1 = error + 1,
        _ => runs.push((error, error + 1)),
    }
}

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
    /// why errors were waived or set aside, an action at a time (`reasons`)
    pub reasons: Reasons,
    /// the waives this reviewer has confirmed (`confirm`; 0.12.466): a file
    /// of their own, made at the first confirmation
    pub confirms: Confirms,
    /// what this session does in the workflow (`Roles`; 0.12.472, several
    /// at once since 0.12.473): the launcher's word, said at the open
    roles: Roles,
    /// which file its own marks are in: its highest marking role's
    store: Store,
    /// what a mark of the status store is the reason's kind of: a waive's,
    /// or - a requester's session - a request's
    mark: Kind,
    /// a role's session: each rule's errors by state (`STATE_*`), counted
    /// when asked and again after anything that may have moved them
    by_state: RefCell<Vec<Option<[u64; 6]>>>,
    /// a role's session confirmed or rejected: the states are read from
    /// everybody's files again when the request ends (`sync_waives`) - its
    /// own verdict is in files it reads as it reads the others'
    restage: bool,
    /// every confirmer's confirmations, as the status writers read them:
    /// a waive one of them has confirmed is locked for a session that is
    /// no confirmer's (0.12.467)
    locks: Locks,
    /// this reviewer's rejects log, `.<db>.rejects.<reviewer>` (`reasons`):
    /// the waives they rejected as a confirmer (`Kind::Reject`, 0.12.468),
    /// and - whoever they are - the rejections of their own waives they
    /// answered by waiving again, with the reason each waive had before
    /// (`Kind::Answered`, `Kind::Earlier`; 0.12.469)
    pub rejects: Reasons,
    /// what the status bytes do not say, shared with the readers on other
    /// threads: which of this reviewer's waives are rejected, the
    /// confirmers' files, the lens
    overlay: Arc<Overlay>,
    /// each rule's count of this reviewer's waives that are rejected, and
    /// of the ones that are confirmed: counted when asked, again after a
    /// write to the rule or a change of what the confirmers say
    tallies: RefCell<Vec<(Option<u64>, Option<u64>)>>,
    /// this reviewer's rejected errors a look has said already, by rule
    said: BTreeMap<usize, Vec<Run>>,
    /// what the other reviewers' rejects logs and the confirm files were
    /// at the last look
    seen: Option<Vec<(String, u64, u64)>>,
    /// the rules whose errors were classed anew outside a look (a write
    /// that met a rejection it had not known): for the host's counts
    reclassed: BTreeSet<usize>,
    /// which chunks of which rule this review changed, said as each change
    /// is settled (`changes`; 0.12.465): what the team's kept counts are
    /// brought up to date from. Shared with the flags store, whose changes
    /// it says too (drc::team).
    pub changes: Arc<Mutex<ChangeLog>>,
    /// what the open had to say (floe/drc.py wrote these to stderr)
    pub notices: Vec<String>,
    /// the reviewer tag the sidecars are named by (drc::team names the
    /// visited/not-required store and snapshots by it too)
    pub reviewer: String,
    /// how that tag was found: stated (`--userid`, FLOE_REVIEWER) or taken
    /// from where the viewer runs - the viewer shows it (2026-11-10)
    pub reviewer_source: ReviewerSource,
    /// this review's hold on its waive sidecar (drc::session): a second
    /// session of the reviewer on this result is refused while it lives
    _session: Option<std::sync::Arc<session::Session>>,
    /// bumped on every status write (a worker reading statuses apart
    /// knows whether they moved under it)
    generation: u64,
    /// whether the in-place waive writes are flushed to the disk at the end
    /// of the request or job that made them (FLOE_WAIVE_SYNC at the open)
    sync: bool,
    /// status bytes written since the last flush
    unsynced: bool,
    /// flushes made (what the tests count)
    syncs: u64,
}

/// FLOE_WAIVE_SYNC: on unless `off` (`0`, `no`, `false`) - the kill switch of
/// the flush after each waive request (0.12.446).
pub(super) fn waive_sync_of(value: Option<&str>) -> bool {
    !matches!(
        value.unwrap_or_default().trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "no" | "false"
    )
}

impl Drop for Review {
    /// What an error left unflushed goes to the disk with the review.
    fn drop(&mut self) {
        let _ = self.sync_waives();
    }
}

fn io(e: Error) -> std::io::Error {
    std::io::Error::other(e.message)
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

/// The rejects log's names, from the waive names likewise.
fn reject_paths(waive: &[PathBuf; 2]) -> [PathBuf; 2] {
    [confirm::rejects_beside(&waive[0]), confirm::rejects_beside(&waive[1])]
}

/// The reasons file's names, from the waive names likewise.
fn reason_paths(waive: &[PathBuf; 2]) -> [PathBuf; 2] {
    let rename = |p: &Path| {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        p.with_file_name(name.replacen(".waive.", ".reasons.", 1))
    };
    [rename(&waive[0]), rename(&waive[1])]
}

/// Seconds since the epoch: an action's time.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether a file can be made beside `side` (the results folder takes the
/// reviewer's files): a probe made and removed, named as a sidecar being
/// made is - which no reader takes for a reviewer's file.
fn writable(side: &Path) -> bool {
    let probe = with_suffix(side, &format!(".tmp-{}", std::process::id()));
    let made = OpenOptions::new().write(true).create_new(true).open(&probe).is_ok();
    if made {
        let _ = fs::remove_file(&probe);
    }
    made
}

/// A file of `len` zero bytes that is nobody's: the status store of a
/// session that makes no mark (nothing set, every counter 0). Made in the
/// temp folder and unlinked at once - it lives as long as its handles.
fn blank(len: u64) -> std::io::Result<File> {
    static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        ".floe-drc-blank-{}-{}",
        std::process::id(),
        MADE.fetch_add(1, Ordering::Relaxed)
    ));
    let file = OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
    let sized = file.set_len(len);
    let _ = fs::remove_file(&path);
    sized.map(|()| file)
}

/// Create or validate the waive sidecar at `side`; returns it. Another run's
/// file is moved aside, a new one seeded from the pack's sections - or,
/// `blank`, made of `header` and nothing marked (a requester's request
/// file: the pack's own statuses are waives, not requests).
fn ensure_sidecar(
    side: &Path,
    pack: &Pack,
    header: &[u8; 40],
    blank: bool,
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
            let aside = aside_path(side);
            fs::rename(side, &aside)?;
            notices.push(format!(
                "[drc] {} autosave does not match this pack (DRC re-run?); moved aside: {}",
                if blank { "request" } else { "waive" },
                aside.display()
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let tmp = with_suffix(side, &format!(".tmp-{}", std::process::id()));
    let result = (|| {
        let mut out = File::create(&tmp)?;
        if blank {
            // (zeros after the header: nothing asked for, every counter 0)
            out.write_all(header)?;
            out.set_len(size)?;
        } else {
            let copied = std::io::copy(&mut pack.review_seed().map_err(io)?, &mut out)?;
            if copied != size {
                return Err(std::io::Error::other("pack status sections truncated"));
            }
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
        Self::open_with(pack_path, source, reviewer, cancelled, false, Roles::NONE)
    }
    /// `open`, as a session of `roles` (`Roles`): what the launcher said
    /// this session does in the workflow. Its own marks are its highest
    /// marking role's - a waiver's waives, else a requester's requests, in
    /// a file of their own, else none and no file -; the states any role's
    /// session reads are the error's (see the module docs).
    pub fn open_as(
        pack_path: &Path,
        source: Option<&Path>,
        reviewer: Option<&str>,
        roles: Roles,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        Self::open_with(pack_path, source, reviewer, cancelled, false, roles)
    }
    /// `open`. `whole`: a source whose time changed is read and compared
    /// whatever its size (Pack::source_current) - the load that would parse
    /// the ASCII file were the pack stale. Without it the read stops at the
    /// inline limit (Pack::source_matches): the open of a pack chosen with
    /// its .db, where a stale pack is asked about instead.
    fn open_with(
        pack_path: &Path,
        source: Option<&Path>,
        reviewer: Option<&str>,
        cancelled: &AtomicUsize,
        whole: bool,
        roles: Roles,
    ) -> Result<Self> {
        let lock = pack_reader(pack_path)?;
        let mut pack = Pack::open(pack_path, cancelled)?;
        pack.hold(lock);
        if let Some(source) = source {
            let current = if whole {
                pack.source_current(source, cancelled)?
            } else {
                pack.source_matches(source)?
            };
            if !current {
                return Err(Error::new(
                    ErrorKind::Cache,
                    format!(
                        "{}: stale pack (the source is not the file that was indexed)",
                        pack_path.display()
                    ),
                ));
            }
        }
        let header = Layout::from_pack(&pack)?.header();
        // (a stated id where FLOE_REVIEWER_REQUIRED asks for one)
        let who = required_reviewer(reviewer)?;
        let tag = who.tag.clone();
        let waive = waive_paths(&pack.path, &tag)?;
        let [note_beside, note_temp] = note_paths(&waive);
        let mut notices = Vec::new();
        let (status_section, wcount_section) = pack.review_sections();
        // the status store: the reviewer's waive sidecar - or, where the
        // session's marks are requests (a requester who is no waiver),
        // their request file, named as it and made with nothing asked for
        // - or, for a session that makes no mark (a confirmer who is
        // neither), no file at all: it reads nothing marked, and its
        // reviewer's other files are named where they can be written
        let kind = Store::of(roles);
        let requester = kind == Store::Requests;
        let (store, store_header) = if requester {
            (
                [confirm::request_beside(&waive[0]), confirm::request_beside(&waive[1])],
                confirm::request_header_of(&header),
            )
        } else {
            (waive.clone(), header)
        };
        let (waive_path, status_off, wcount_off) = if kind == Store::None {
            let at = if writable(&waive[0]) {
                waive[0].clone()
            } else {
                notices.push(format!(
                    "[drc] results folder not writable; the review's files go to {}",
                    waive[1].parent().unwrap_or(Path::new(".")).display()
                ));
                waive[1].clone()
            };
            (at, HEADER, HEADER + pack.total)
        } else {
            match ensure_sidecar(&store[0], &pack, &store_header, requester, &mut notices) {
            Ok(p) => (p, HEADER, HEADER + pack.total),
            Err(_) => match ensure_sidecar(&store[1], &pack, &store_header, requester, &mut notices) {
                Ok(p) => {
                    notices.push(format!(
                        "[drc] results folder not writable; waive autosave in {} (save waives as… to keep the review)",
                        p.display()
                    ));
                    (p, HEADER, HEADER + pack.total)
                }
                Err(e) => {
                    notices.push(format!(
                        "[drc] waive autosave unavailable ({e}); embedded statuses are read-only; no shared pack changes will be made"
                    ));
                    (pack.path.clone(), status_section, wcount_section)
                }
            },
            }
        };
        // the reviewer's waive sidecar's name where the status store is:
        // what their other files are named from, whatever the store (a
        // requester's request file is beside it)
        let beside = if waive_path == store[1] { waive[1].clone() } else { waive[0].clone() };
        // one session a reviewer and result: the sidecar's counters are kept
        // in memory by the session that writes them (drc::session); the
        // pack's own embedded statuses are read-only and need none. A
        // reviewer's session is one whatever its role: their reasons, notes
        // and flags are one set of files
        let session = if waive_path == pack.path {
            None
        } else {
            let database = crate::cache::database_path(&pack.path)?;
            session::hold(
                &beside,
                &tag,
                &database.file_name().unwrap_or_default().to_string_lossy(),
                &floe_vfs::lock::reader_label(&format!("{} drc review", crate::program())),
                &mut notices,
            )?
        };
        // (a session without a store reads a file of nothing: no status
        // byte set, every counter 0 - its own, gone with the session)
        let reader = if kind == Store::None {
            blank(HEADER + pack.total + 4 * pack.checks.len() as u64)?
        } else {
            File::open(&waive_path)?
        };
        let mut raw = vec![0u8; pack.checks.len() * 4];
        reader.read_exact_at(&mut raw, wcount_off)?;
        let wcount = raw
            .chunks_exact(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        // (the result's content, as the waive file's header: review.rs)
        let note_tag = super::review::Fingerprint::of(&pack).tag();
        let sync = waive_sync_of(std::env::var("FLOE_WAIVE_SYNC").ok().as_deref());
        // the reasons: beside the waive sidecar in use, bound to the result
        // as the notes are; the pack shown read-only has none to write
        let mut reasons = Reasons::open(
            reason_paths(&waive),
            note_tag.clone(),
            pack.checks.iter().map(|c| c.count).collect(),
            sync,
        );
        notices.append(&mut reasons.notices);
        // the rejections this reviewer made as a confirmer: a log of the
        // same shape, in a file of its own
        let mut rejects = Reasons::open_as(
            "# floe drc waive rejections",
            reasons::REJECTS_VERSION,
            reject_paths(&waive),
            note_tag.clone(),
            pack.checks.iter().map(|c| c.count).collect(),
            sync,
        );
        notices.append(&mut rejects.notices);
        // the confirmations: beside the sidecar in use (a file of another
        // result is moved aside, as the sidecar's was); the pack shown
        // read-only has none to write
        let mut confirms = if waive_path == pack.path {
            Confirms::none(pack.checks.len())
        } else {
            Confirms::open(
                confirm::path_beside(&beside),
                confirm::header_of(&header),
                pack.checks.iter().map(|c| (c.start, c.count)).collect(),
                sync,
            )
        };
        notices.append(&mut confirms.notices);
        // which waives are locked: every confirmer's file of this result,
        // named as this reviewer's would be (the read-only pack writes
        // nothing, so it asks nothing)
        let mut locks = if waive_path == pack.path {
            Locks::none(pack.checks.len())
        } else {
            Locks::beside(
                &waive,
                &tag,
                confirm::header_of(&header),
                pack.checks.iter().map(|c| (c.start, c.count)).collect(),
                note_tag.clone(),
            )
        };
        // a role's session reads every stage: the request files and the
        // waive sidecars of the result are found with the confirm files -
        // but for its own status store, which it reads itself
        if roles.any() && waive_path != pack.path {
            locks.follow(header, kind == Store::Waives, requester);
        }
        let overlay = Arc::new(Overlay::new(locks.shared(), kind, roles.any()));
        let rules = pack.checks.len();
        // the change log: beside the sidecar in use; the pack shown
        // read-only has no file of its own and says nothing
        let changes = if waive_path == pack.path {
            ChangeLog::none()
        } else if !super::changes::enabled() {
            ChangeLog::silent(&beside)
        } else {
            let identity: [u8; 28] = header[12..40].try_into().expect("28 of 40 bytes");
            ChangeLog::open(&beside, identity, sync)
        };
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
            reasons,
            confirms,
            roles,
            store: kind,
            mark: if requester { Kind::Request } else { Kind::Waive },
            by_state: RefCell::new(vec![None; rules]),
            restage: false,
            locks,
            rejects,
            overlay,
            tallies: RefCell::new(vec![(None, None); rules]),
            said: BTreeMap::new(),
            seen: None,
            reclassed: BTreeSet::new(),
            changes: Arc::new(Mutex::new(changes)),
            notices,
            reviewer: tag,
            reviewer_source: who.source,
            _session: session,
            generation: 0,
            sync,
            unsynced: false,
            syncs: 0,
        };
        review.load_notes();
        // which of this reviewer's waives are rejected, and where the
        // confirmers' files are: read once now, so that the first status
        // the host reads is the state (`look` says them, and follows)
        if !review.read_only() {
            review.classify(true)?;
            review.reclassed.clear();
        }
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

    /// Check `check`'s status bytes `start..start+count` (clamped) as the
    /// file has them: 1 = waived here, rejected or not.
    fn raw(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>> {
        let (first, n) = self.check(check)?;
        let start = start.min(n);
        let count = count.min(n - start);
        let mut out = vec![0u8; count as usize];
        self.reader
            .read_exact_at(&mut out, self.status_off + first + start)?;
        Ok(out)
    }

    /// One error's status byte as the file has it.
    fn raw_one(&self, check: usize, error: u64) -> Result<u8> {
        let gid = self.error_gid(check, error)?;
        let mut b = [0u8; 1];
        self.reader.read_exact_at(&mut b, self.status_off + gid)?;
        Ok(b[0])
    }

    fn classed(&self, check: usize, start: u64, count: u64, lens: Lens) -> Result<Vec<u8>> {
        let (first, n) = self.check(check)?;
        let mut out = self.raw(check, start, count)?;
        self.overlay.class(check, first, start.min(n), &mut out, lens);
        Ok(out)
    }

    /// Check `check`'s statuses `start..start+count` (clamped): 1 = waived
    /// - and not rejected. A waive a confirmer has rejected reads as no
    /// waive, here and in every count made of these bytes; `states` says
    /// which is which.
    ///
    /// These are this reviewer's own marks whatever the session's role - a
    /// requester's requests, anybody else's waives: what decides an action
    /// of theirs. A role's session reads the error's state beside
    /// (`states`, `lensed`).
    pub fn statuses(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>> {
        let (_, n) = self.check(check)?;
        let mut out = self.raw(check, start, count)?;
        self.overlay.own(check, start.min(n), &mut out);
        Ok(out)
    }

    pub fn status(&self, check: usize, error: u64) -> Result<u8> {
        self.error_gid(check, error)?;
        Ok(self.statuses(check, error, 1)?[0])
    }

    /// Check `check`'s states `start..start+count` (clamped): `STATE_NONE`,
    /// `STATE_WAIVED`, `STATE_REJECTED`, `STATE_CONFIRMED` - this reviewer's
    /// waive of each error, and what the confirmers have said of it (their
    /// files as the last look found them; no folder is listed for it).
    pub fn states(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>> {
        let (first, n) = self.check(check)?;
        let mut out = self.raw(check, start, count)?;
        self.overlay.states(check, first, start.min(n), &mut out, true);
        Ok(out)
    }

    /// Check `check`'s errors `start..start+count` (clamped): 1 = of the
    /// class the lens shows (`set_lens`). What the scopes' counts, pages
    /// and ranks are made of.
    pub fn lensed(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>> {
        self.classed(check, start, count, self.overlay.lens())
    }

    /// The class the status readers of this review say 1 of from now: the
    /// host's filter (`Lens`). The readers on other threads follow at once;
    /// what a host counted through them is to be counted again.
    pub fn set_lens(&mut self, lens: Lens) {
        self.overlay.lens.store(lens as u8, Ordering::Relaxed);
        self.chunks.clear();
    }

    pub fn lens(&self) -> Lens {
        self.overlay.lens()
    }

    /// A status reader of its own for worker threads (drc::analysis): the
    /// same store, positional reads; the review's writes are seen as made,
    /// and what the confirmers say as the review last read it.
    pub fn status_reader(&self) -> Result<StatusReader> {
        Ok(StatusReader {
            file: std::sync::Arc::new(self.reader.try_clone()?),
            status_off: self.status_off,
            checks: std::sync::Arc::new(
                self.pack.checks.iter().map(|c| (c.start, c.count)).collect(),
            ),
            overlay: Arc::clone(&self.overlay),
        })
    }

    /// (waived, total) of one rule: the stored counter less the waives
    /// that are rejected. This reviewer's own marks, as `statuses`.
    pub fn status_counts(&self, check: usize) -> Result<(u64, u64)> {
        let (_, n) = self.check(check)?;
        let held = u64::from(self.wcount[check]);
        Ok((held.saturating_sub(self.tally_rejected(check)?), n))
    }

    /// One rule's errors by state: [not waived, waived and not confirmed
    /// yet, rejected, confirmed, waive requested, request declined]
    /// (`STATE_*`). A session without a role says its reviewer's own
    /// waives', and none requested; a role's the errors' (`states`).
    pub fn state_counts(&self, check: usize) -> Result<[u64; 6]> {
        let (_, n) = self.check(check)?;
        if self.roles.any() {
            return self.tally(check);
        }
        let held = u64::from(self.wcount[check]).min(n);
        let rejected = self.tally_rejected(check)?.min(held);
        let confirmed = self.tally_confirmed(check)?.min(held - rejected);
        Ok([n - held, held - rejected - confirmed, rejected, confirmed, 0, 0])
    }

    /// (errors of the lens's class, total) of one rule.
    pub fn lens_counts(&self, check: usize) -> Result<(u64, u64)> {
        let (_, n) = self.check(check)?;
        if self.roles.any() {
            return Ok((self.overlay.lens().count(&self.tally(check)?), n));
        }
        let held = u64::from(self.wcount[check]).min(n);
        let count = match self.overlay.lens() {
            Lens::Held => held,
            Lens::Live => held.saturating_sub(self.tally_rejected(check)?),
            Lens::Rejected => self.tally_rejected(check)?.min(held),
            Lens::Waived => self.state_counts(check)?[1],
            Lens::Confirmed => self.state_counts(check)?[3],
            Lens::Requested | Lens::Declined => 0,
        };
        Ok((count, n))
    }

    /// A role's session: rule `check`'s errors by state, counted from every
    /// reviewer's files (kept until something may have moved them). A rule
    /// nobody has anything in is counted without a read.
    fn tally(&self, check: usize) -> Result<[u64; 6]> {
        if let Some(counts) = self.by_state.borrow()[check] {
            return Ok(counts);
        }
        let (_, n) = self.check(check)?;
        let mut counts = [0u64; 6];
        let peers = self.overlay.peers();
        let marked = self.wcount[check] > 0
            || peers.requests.iter().chain(&peers.waives).any(|peer| peer.any(check));
        if !marked {
            counts[STATE_NONE as usize] = n;
        } else {
            const STEP: u64 = 1 << 20;
            let mut at = 0;
            while at < n {
                let len = (n - at).min(STEP);
                for state in self.states(check, at, len)? {
                    counts[usize::from(state.min(5))] += 1;
                }
                at += len;
            }
        }
        self.by_state.borrow_mut()[check] = Some(counts);
        Ok(counts)
    }

    /// This reviewer's waives of rule `check` that are rejected.
    fn tally_rejected(&self, check: usize) -> Result<u64> {
        if let Some(count) = self.tallies.borrow()[check].0 {
            return Ok(count);
        }
        let mut count = 0u64;
        if self.wcount[check] > 0 {
            if let Some(runs) = self.overlay.rejected(check) {
                const STEP: u64 = 1 << 20;
                for &(a, b) in runs.iter() {
                    let mut at = a;
                    while at < b {
                        let len = (b - at).min(STEP);
                        count += self.raw(check, at, len)?.iter().filter(|&&s| s == STATUS_WAIVED).count() as u64;
                        at += len;
                    }
                }
            }
        }
        self.tallies.borrow_mut()[check].0 = Some(count);
        Ok(count)
    }

    /// This reviewer's waives of rule `check` that are confirmed.
    fn tally_confirmed(&self, check: usize) -> Result<u64> {
        if let Some(count) = self.tallies.borrow()[check].1 {
            return Ok(count);
        }
        let (_, n) = self.check(check)?;
        let mut count = 0u64;
        if self.wcount[check] > 0 && self.overlay.any_confirms() {
            const STEP: u64 = 1 << 20;
            let mut at = 0;
            while at < n {
                let len = (n - at).min(STEP);
                count += self.states(check, at, len)?.iter().filter(|&&s| s == STATE_CONFIRMED).count() as u64;
                at += len;
            }
        }
        self.tallies.borrow_mut()[check].1 = Some(count);
        Ok(count)
    }

    /// Whether rule `check`'s per-chunk counts follow a status byte alone:
    /// kept in step by the writers then, counted again otherwise (a host
    /// that keeps counts of its own asks the same).
    pub fn steps(&self, check: usize) -> bool {
        !self.roles.any()
            && matches!(self.overlay.lens(), Lens::Live | Lens::Held)
            && self.overlay.rejected(check).is_none()
    }

    /// A write changed rule `check`'s statuses: its counts of rejected and
    /// confirmed waives are made again when asked.
    fn recount(&mut self, check: usize) {
        self.tallies.borrow_mut()[check] = (None, None);
        self.by_state.borrow_mut()[check] = None;
        if !self.steps(check) {
            self.chunks.remove(&check);
        }
    }

    /// The waive store is the pack's embedded sections, shown read-only
    /// (no sidecar could be made).
    pub fn read_only(&self) -> bool {
        self.waive_path == self.pack.path
    }

    /// The 40-byte header of this pack's waive files (FLOEWAIV, version 2).
    pub fn waive_header(&self) -> [u8; 40] {
        self.header
    }

    /// Bumped by every status write.
    pub fn status_generation(&self) -> u64 {
        self.generation
    }

    fn writer(&mut self) -> Result<&File> {
        if self.read_only() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "per-reviewer waive storage unavailable; shared pack is read-only",
            ));
        }
        self.generation += 1;
        self.unsynced = true;
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

    /// Flush the waive writes made since the last flush to the disk: the
    /// request or the job that made them ends with this (user 2026-11-11:
    /// "waive 처리 작업을 매번 자동 저장" - every click was written at once
    /// but stayed in the page cache until the kernel wrote it back, which a
    /// host that goes down loses and a reader on another host of an NFS
    /// folder does not see for tens of seconds; the statistics are read by a
    /// command of their own while the viewer runs). One flush a request or a
    /// job, whatever its size. FLOE_WAIVE_SYNC=off: none, as before 0.12.446.
    pub fn sync_waives(&mut self) -> Result<()> {
        // (the reasons a job took back with the statuses: said in their
        // file now, one line a rule, and flushed with them)
        if let Err(e) = self.reasons.flush(now()) {
            self.notices.push(format!(
                "[drc] review reasons: {} ({})",
                e.message,
                self.reasons.path.display()
            ));
        }
        self.notices.append(&mut self.reasons.notices);
        // (and the rejections taken back: a line a rule of the rejects log)
        if let Err(e) = self.rejects.flush(now()) {
            self.notices.push(format!(
                "[drc] waive rejections: {} ({})",
                e.message,
                self.rejects.path.display()
            ));
        }
        self.notices.append(&mut self.rejects.notices);
        let flushed = self.flush_statuses().and(self.confirms.flush());
        // what this review changed - its statuses and, through the flags
        // store that shares the log, its flags - is said now that the bits
        // are on the disk (`changes`): a line of its own file, so the
        // team's counts are brought up to date from what changed
        let mut log = self.changes.lock().unwrap_or_else(|p| p.into_inner());
        log.rejects_at(&self.rejects.path);
        log.settle();
        self.notices.append(&mut log.notices);
        drop(log);
        // (a role's session that confirmed or rejected reads the states
        // from the files again: what it said is in them)
        if std::mem::take(&mut self.restage) && !self.read_only() {
            self.classify(true)?;
        }
        flushed
    }

    fn flush_statuses(&mut self) -> Result<()> {
        if !std::mem::take(&mut self.unsynced) || !self.sync {
            return Ok(());
        }
        let Some(writer) = &self.writer else {
            return Ok(());
        };
        match writer.sync_data() {
            Ok(()) => {
                self.syncs += 1;
                Ok(())
            }
            // a file system without the call writes back in its own time
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
                ) =>
            {
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Rule `check`'s errors `first..=last` changed their status: said in
    /// the change log at the next `sync_waives`.
    fn touched(&self, check: usize, first: u64, last: u64) {
        self.changes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .touched(check, first / CHANGE_CHUNK, last / CHANGE_CHUNK);
    }

    /// The flushes `sync_waives` made.
    pub fn waive_syncs(&self) -> u64 {
        self.syncs
    }

    /// Turn the flushes on or off (FLOE_WAIVE_SYNC decides at the open).
    pub fn set_waive_sync(&mut self, on: bool) {
        self.sync = on;
        self.reasons.set_sync(on);
        self.rejects.set_sync(on);
        self.confirms.set_sync(on);
        self.changes.lock().unwrap_or_else(|p| p.into_inner()).set_sync(on);
    }

    // ---- confirmations (0.12.466) ---------------------------------------

    /// Whether this session confirms and rejects waives: the confirmer's
    /// role, which the launcher gives (`floe2 view --role confirmer`, alone
    /// or with others) - a role, not a check of who the reviewer is (the
    /// ids are the launcher's too). In a session that is a waiver too, the
    /// role is also what turns its own confirmed waive back: no lock holds
    /// it, and its own confirmation goes with the waive.
    pub fn confirmer(&self) -> bool {
        self.roles.confirmer
    }

    /// What this session does in the workflow (`Roles`), as its open said.
    pub fn roles(&self) -> Roles {
        self.roles
    }

    /// Which file this session's own marks are in (`Store`).
    pub fn store(&self) -> Store {
        self.store
    }

    /// Whether this session waives only what somebody asked the waive of:
    /// a waiver that is no requester. (One that is both is its own
    /// requester; a session without a role waives as it always did.)
    fn strict(&self) -> bool {
        self.store == Store::Waives && self.roles.any() && !self.roles.requester
    }

    /// What a mark of this session's status store is the reason's kind of:
    /// a waive's, or - a requester's session - a request's. The kind a host
    /// gives the reason of the marks it makes, and asks them by.
    pub fn mark(&self) -> Kind {
        self.mark
    }

    // ---- the lock of a confirmed waive (0.12.467) ------------------------

    /// Rule `check`'s errors `start..start+len` (clamped): true where a
    /// confirmer has the error confirmed, which locks this reviewer's waive
    /// of it - not turned back, its reason not changed, by any writer here.
    /// None when none is, and for a confirmer's session, which no lock
    /// holds (user 2026-11-13: "confirm된 waive는 되돌릴 수 없게 잠구고
    /// confirmer만 되돌릴 수 있게 해줘"). Whether the error is waived here
    /// is the caller's to ask: an error this reviewer has not waived has
    /// nothing to lock.
    ///
    /// A waive that is rejected is never locked, whoever confirmed it: its
    /// reviewer waives it again, or turns it back.
    ///
    /// A requester's session (0.12.472): true where somebody has the error
    /// waived, which locks this reviewer's request of it - a request is not
    /// taken back from under a waive.
    pub fn locked(&mut self, check: usize, start: u64, len: u64) -> Result<Option<Vec<bool>>> {
        if self.store == Store::Requests {
            return self.locks.waived(check, start, len);
        }
        if self.roles.confirmer {
            return Ok(None);
        }
        let Some(mut locked) = self.locks.mask(check, start, len)? else {
            return Ok(None);
        };
        let (_, n) = self.check(check)?;
        let start = start.min(n);
        // (the mask's look may have found a log: read as it is now)
        let rejected = self.rejected_runs(check, true)?;
        for (a, b) in meet(&[(start, start + locked.len() as u64)], &rejected) {
            locked[(a - start) as usize..(b - start) as usize].fill(false);
        }
        Ok(locked.contains(&true).then_some(locked))
    }

    // ---- the rejection of a waive (0.12.468; a state since 0.12.469) -----

    /// Every rejection of rule `check` that stands: the other reviewers' as
    /// the locks read their logs (`look`: `Locks::standing`), and this
    /// reviewer's own, which is known here at once.
    ///
    /// The verdict is the one there is on this session's own marks
    /// (`Store::verdict`): a confirmer's rejection of a waive - or, where
    /// the marks are requests, a waiver's decline (0.12.473).
    fn standing(&mut self, check: usize, look: bool) -> Result<Vec<Standing>> {
        let made = self.store.verdict().made;
        let mut standing = self.locks.standing_by(check, look, made)?;
        standing.extend(confirm::standing_by(&self.reviewer, &self.rejects, check, made));
        Ok(standing)
    }

    /// Rule `check`'s errors under a rejection this reviewer has not
    /// answered (sorted runs), as the logs say now: kept for the status
    /// readers, and the rule's counts made again, where they are others
    /// than they were.
    fn rejected_runs(&mut self, check: usize, look: bool) -> Result<Arc<Vec<Run>>> {
        self.check(check)?;
        // (a session without marks has none under a verdict)
        if self.read_only() || self.store == Store::None {
            return Ok(Arc::default());
        }
        let standing = self.standing(check, look)?;
        let runs = confirm::unanswered_by(&standing, Some(&self.rejects), check, self.store.verdict().answered);
        if self.overlay.set_rejected(check, runs) {
            self.reclass(check);
        }
        let now = self.overlay.rejected(check).unwrap_or_default();
        // (what is rejected no longer - answered, taken back - is said
        // again should it be rejected again before the next look)
        if let Some(said) = self.said.get_mut(&check) {
            *said = meet(said, &now);
        }
        Ok(now)
    }

    /// Rule `check`'s errors are classed otherwise than they were, though
    /// no status byte changed.
    fn reclass(&mut self, check: usize) {
        self.tallies.borrow_mut()[check] = (None, None);
        self.by_state.borrow_mut()[check] = None;
        self.chunks.remove(&check);
        self.reclassed.insert(check);
        self.generation += 1;
    }

    /// The rules whose errors were classed anew since this was last asked -
    /// a rejection found, answered or taken back: what a host counted of
    /// them through the status readers is to be counted again.
    pub fn take_reclassed(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.reclassed).into_iter().collect()
    }

    /// Rule `check`'s errors `start..start+len` (clamped): true where the
    /// error is under a rejection this reviewer has not answered - a waive
    /// of it here is rejected, and it is waived again with a reason
    /// (`resubmit`) or not at all (user 2026-11-14: "confirmer가 다른
    /// 리뷰어의 waive를 반려할 수 있게 해줘"). None when none is. No folder
    /// is listed for it: a confirmer's first rejection - a new file - is
    /// known from the next `look` on.
    pub fn rejected(&mut self, check: usize, start: u64, len: u64) -> Result<Option<Vec<bool>>> {
        let (_, n) = self.check(check)?;
        let start = start.min(n);
        let len = len.min(n - start);
        let runs = self.rejected_runs(check, false)?;
        let pieces = meet(&[(start, start + len)], &runs);
        if pieces.is_empty() {
            return Ok(None);
        }
        let mut out = vec![false; len as usize];
        for (a, b) in pieces {
            out[(a - start) as usize..(b - start) as usize].fill(true);
        }
        Ok(Some(out))
    }

    /// Who has rejected - and this reviewer not answered - the waive of any
    /// of rule `check`'s errors `runs`, and why as they said: what the
    /// answer of a write that left rejected errors says.
    pub fn rejected_by(&mut self, check: usize, runs: &[Run]) -> Result<Vec<Rejected>> {
        self.check(check)?;
        if runs.is_empty() {
            return Ok(Vec::new());
        }
        let standing = self.standing(check, false)?;
        let mut by =
            confirm::rejecting_by(&standing, Some(&self.rejects), check, runs, self.store.verdict().answered);
        by.sort_by(|a, b| (&a.reviewer, a.time).cmp(&(&b.reviewer, b.time)));
        Ok(by)
    }

    fn may_reject(&self) -> Result<()> {
        if !self.confirmer() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "rejecting waives is the confirmer's: start the viewer with --role confirmer",
            ));
        }
        if self.read_only() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "this review is read-only here: nothing can be rejected",
            ));
        }
        Ok(())
    }

    /// Reject the waive of rule `check`'s `runs` (sorted, disjoint, not
    /// touching), `text` saying why (it may be empty): one line of this
    /// confirmer's rejects log, whatever the number of errors - a new
    /// rejection, to be answered anew by every reviewer who has the errors
    /// waived. Nobody's file but this log is written for it: the waives
    /// stay where they are and are rejected from now. The confirmer's own
    /// waive of those errors is turned back and their own confirmation of
    /// them taken back with it. Whose waive the errors are is the caller's
    /// to ask (drc::team::waives).
    pub fn reject_set(&mut self, check: usize, runs: &[Run], text: &str) -> Result<()> {
        self.may_reject()?;
        let (_, n) = self.check(check)?;
        if runs.is_empty() {
            return Ok(());
        }
        if runs.last().is_some_and(|r| r.1 > n) {
            return Err(Error::input("DRC error index out of range"));
        }
        // (each rejection of a log has a time of its own: who and when are
        // what its reviewers name it by when they answer)
        let time = now().max(self.rejects.latest(Kind::Reject) + 1);
        let said = self.rejects.set(Kind::Reject, check, runs, text, time);
        self.restage = self.roles.any();
        self.notices.append(&mut self.rejects.notices);
        said.map_err(|e| Error::new(e.kind, format!("{} ({})", e.message, self.rejects.path.display())))?;
        // (the log's change is a change of every chunk it names: the
        // team's counts follow)
        for &(a, b) in runs {
            self.touched(check, a, b - 1);
        }
        self.rejected_runs(check, false)?;
        self.turn_back(check, runs)?;
        self.unconfirm_runs(check, runs)?;
        self.sync_waives()
    }

    /// This confirmer's own confirmations of rule `check`'s `runs` are
    /// taken back: a rejection is their other verdict on the same errors.
    fn unconfirm_runs(&mut self, check: usize, runs: &[Run]) -> Result<()> {
        const STEP: u64 = 1 << 16;
        if !self.confirms.present() || self.confirms.counts()[check] == 0 {
            return Ok(());
        }
        for &(a, b) in runs {
            let mut at = a;
            while at < b {
                let len = (b - at).min(STEP);
                let was = self.confirms.statuses(check, at, len)?;
                if was.contains(&confirm::CONFIRMED) {
                    self.confirms.write_span(check, at, &was, &vec![0u8; was.len()])?;
                }
                at += len;
            }
        }
        Ok(())
    }

    /// Take this confirmer's rejection of rule `check`'s `runs` back:
    /// whether any of them was rejected. Their waives are as they were
    /// before it - waived, to be confirmed - at once.
    pub fn reject_drop(&mut self, check: usize, runs: &[Run]) -> Result<bool> {
        self.may_reject()?;
        self.check(check)?;
        let held = self.rejects.held(Kind::Reject, check, runs);
        if held.is_empty() {
            return Ok(false);
        }
        self.restage = self.roles.any();
        self.rejects.clear(Kind::Reject, check, &held);
        for &(a, b) in &held {
            self.touched(check, a, b - 1);
        }
        self.rejected_runs(check, false)?;
        self.sync_waives()?;
        Ok(true)
    }

    // ---- the decline of a request (0.12.473) ----------------------------

    fn may_decline(&self) -> Result<()> {
        if !self.roles.waiver {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "declining requests is the waiver's: start the viewer with --role waiver",
            ));
        }
        if self.read_only() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "this review is read-only here: nothing can be declined",
            ));
        }
        Ok(())
    }

    /// Decline the request of rule `check`'s `runs` (sorted, disjoint, not
    /// touching), `text` saying why (it may be empty): one line of this
    /// waiver's rejects log, whatever its size (user 2026-11-14: "요청 거절
    /// 상태도 만들어줘"). A waiver's verdict on a request, as a rejection
    /// is a confirmer's on a waive: the request stays in its requester's
    /// file, declined - it counts as none, and no waiver's session waives
    /// the error for it - until its requester asks again with a reason
    /// (`resubmit` in their session: what was is kept), takes it back, or
    /// this waiver takes the decline back (`decline_drop`) or waives the
    /// error. Which errors have a request to decline is the caller's to
    /// ask (drc::team::waives `decline_runs`).
    pub fn decline_set(&mut self, check: usize, runs: &[Run], text: &str) -> Result<()> {
        self.may_decline()?;
        let (_, n) = self.check(check)?;
        if runs.is_empty() {
            return Ok(());
        }
        if runs.last().is_some_and(|r| r.1 > n) {
            return Err(Error::input("DRC error index out of range"));
        }
        // (each decline of a log has a time of its own, as each rejection:
        // who and when name it to the requester who answers)
        let time = now().max(self.rejects.latest(Kind::Decline) + 1);
        let said = self.rejects.set(Kind::Decline, check, runs, text, time);
        self.restage = true;
        self.notices.append(&mut self.rejects.notices);
        said.map_err(|e| Error::new(e.kind, format!("{} ({})", e.message, self.rejects.path.display())))?;
        for &(a, b) in runs {
            self.touched(check, a, b - 1);
        }
        self.sync_waives()
    }

    /// Take this waiver's decline of rule `check`'s `runs` back: whether
    /// any of them was declined. Their requests are as they were before it
    /// - asked for, waiting for a waiver - at once.
    pub fn decline_drop(&mut self, check: usize, runs: &[Run]) -> Result<bool> {
        self.may_decline()?;
        self.check(check)?;
        if !self.undecline(check, runs)? {
            return Ok(false);
        }
        self.sync_waives()?;
        Ok(true)
    }

    /// This waiver's own declines of rule `check`'s `runs` are taken back
    /// (they said so, or they waive the errors): whether there were any.
    /// The states are read again when the request ends.
    fn undecline(&mut self, check: usize, runs: &[Run]) -> Result<bool> {
        if !self.rejects.any(Kind::Decline, check) {
            return Ok(false);
        }
        let held = self.rejects.held(Kind::Decline, check, runs);
        if held.is_empty() {
            return Ok(false);
        }
        self.rejects.clear(Kind::Decline, check, &held);
        for &(a, b) in &held {
            self.touched(check, a, b - 1);
        }
        // (the requests are requests again for this session's own waive of
        // them, made in the same request: read now)
        self.classify(true)?;
        Ok(true)
    }

    /// A confirmer confirmed rule `check`'s `runs`: their own rejection of
    /// those errors is taken back - a confirmation is their other verdict
    /// on the same errors.
    fn unreject(&mut self, check: usize, runs: &[Run]) -> Result<()> {
        if !self.rejects.any(Kind::Reject, check) {
            return Ok(());
        }
        let held = self.rejects.held(Kind::Reject, check, runs);
        if held.is_empty() {
            return Ok(());
        }
        self.rejects.clear(Kind::Reject, check, &held);
        for &(a, b) in &held {
            self.touched(check, a, b - 1);
        }
        self.rejected_runs(check, false)?;
        Ok(())
    }

    /// Turn this reviewer's waives among rule `check`'s `runs` back: the
    /// runs that were waived. (No lock holds a rejected waive.)
    fn turn_back(&mut self, check: usize, runs: &[Run]) -> Result<Vec<Run>> {
        const STEP: u64 = 1 << 16;
        let mut back = Vec::new();
        if self.wcount[check] == 0 {
            return Ok(back);
        }
        for &(a, b) in runs {
            let mut at = a;
            while at < b {
                let len = (b - at).min(STEP);
                let before = self.raw(check, at, len)?;
                if before.contains(&STATUS_WAIVED) {
                    let after: Vec<u8> = before
                        .iter()
                        .map(|&s| if s == STATUS_WAIVED { STATUS_NONE } else { s })
                        .collect();
                    self.write_span(check, at, &before, &after)?;
                    for (j, &s) in before.iter().enumerate() {
                        if s == STATUS_WAIVED {
                            push_run(&mut back, at + j as u64);
                        }
                    }
                }
                at += len;
            }
        }
        Ok(back)
    }

    /// The parts of rule `check`'s `runs` this reviewer has waived, as the
    /// file says.
    fn held_in(&self, check: usize, runs: &[Run]) -> Result<Vec<Run>> {
        const STEP: u64 = 1 << 20;
        let mut held = Vec::new();
        for &(a, b) in runs {
            let mut at = a;
            while at < b {
                let len = (b - at).min(STEP);
                for (j, &s) in self.raw(check, at, len)?.iter().enumerate() {
                    if s == STATUS_WAIVED {
                        push_run(&mut held, at + j as u64);
                    }
                }
                at += len;
            }
        }
        Ok(held)
    }

    /// The waive reasons of rule `check`'s `runs` are about to go, or to
    /// be given anew, while their waive is rejected: kept as they were in
    /// this reviewer's rejects log (`Kind::Earlier`), a line a reason.
    fn keep_earlier(&mut self, check: usize, runs: &[Run]) -> Result<()> {
        if runs.is_empty() || !self.reasons.any(self.mark, check) {
            return Ok(());
        }
        let mut held: BTreeMap<usize, Vec<Run>> = BTreeMap::new();
        for (a, b, entry) in self.reasons.parts(self.mark, check, runs) {
            let list = held.entry(entry).or_default();
            match list.last_mut() {
                Some(last) if last.1 == a => last.1 = b,
                _ => list.push((a, b)),
            }
        }
        for (entry, runs) in held {
            let (time, text) = {
                let entry = &self.reasons.entries()[entry];
                (entry.time, entry.text.clone())
            };
            let kept = self.rejects.keep(self.store.verdict().earlier, check, &runs, time, "", &text);
            self.notices.append(&mut self.rejects.notices);
            kept.map_err(|e| Error::new(e.kind, format!("{} ({})", e.message, self.rejects.path.display())))?;
        }
        Ok(())
    }

    /// Waive rule `check`'s `runs` again (sorted, disjoint, not touching):
    /// the errors of them under a rejection this reviewer has not answered
    /// - what a status write left in `Left::rejected` - are waived from
    /// now, here and for the team, and wait for a confirmer again (user
    /// 2026-11-14: "리뷰어가 사유를 수정해서 다시 waive를 할 수 있게 하되,
    /// 이 때 이전의 waive사유나 반려사유는 계속 유지되어야 ... REJECTED에서
    /// ... waive로 WAIVED로 가도록"). What was is kept in this reviewer's
    /// rejects log: every rejection answered - who, when, why - and the
    /// reason each waive had. The caller gives the new reason next
    /// (`reason_set`): a rejected waive is waived again with a reason or
    /// not at all, which is the caller's to hold to. How many errors were
    /// waived again.
    pub fn resubmit(&mut self, check: usize, runs: &[Run]) -> Result<u64> {
        const STEP: u64 = 1 << 16;
        self.may_mark()?;
        let (_, n) = self.check(check)?;
        if runs.last().is_some_and(|r| r.1 > n) {
            return Err(Error::input("DRC error index out of range"));
        }
        let standing = self.standing(check, false)?;
        let answered = self.store.verdict().answered;
        let mine = meet(runs, &confirm::unanswered_by(&standing, Some(&self.rejects), check, answered));
        if mine.is_empty() {
            return Ok(0);
        }
        // (refused before anything is kept, where nothing can be written)
        self.writer()?;
        // each rejection's part of them, before any is kept as answered
        let parts: Vec<(Vec<Run>, &Standing)> = standing
            .iter()
            .map(|s| {
                let open = confirm::unanswered_by(std::slice::from_ref(s), Some(&self.rejects), check, answered);
                (meet(&mine, &open), s)
            })
            .filter(|(part, _)| !part.is_empty())
            .collect();
        self.keep_earlier(check, &mine)?;
        for (part, s) in parts {
            let kept = self.rejects.keep(answered, check, &part, s.time, &s.by, &s.text);
            self.notices.append(&mut self.rejects.notices);
            kept.map_err(|e| Error::new(e.kind, format!("{} ({})", e.message, self.rejects.path.display())))?;
        }
        // the ones turned back since are waived; the others are already
        for &(a, b) in &mine {
            let mut at = a;
            while at < b {
                let len = (b - at).min(STEP);
                let before = self.raw(check, at, len)?;
                if before.iter().any(|&s| s != STATUS_WAIVED) {
                    self.write_span(check, at, &before, &vec![STATUS_WAIVED; before.len()])?;
                }
                at += len;
            }
            // (no byte of a waive that stayed changed, and yet it counts
            // from now: the team's counts follow)
            self.touched(check, a, b - 1);
        }
        self.rejected_runs(check, false)?;
        self.sync_waives()?;
        Ok(mine.iter().map(|r| r.1 - r.0).sum())
    }

    /// What is kept of this reviewer's waive of rule `check`'s error
    /// `error`: the reasons it had and the rejections that were answered,
    /// in the order they happened.
    pub fn history(&self, check: usize, error: u64) -> Result<Vec<Event>> {
        self.error_gid(check, error)?;
        Ok(history_of(&self.rejects, check, error))
    }

    /// Which of this reviewer's waives are rejected, read anew from every
    /// rejects log - the folders listed again, which is also what finds
    /// the confirmers' files for the status readers: whether those logs
    /// and files are others than at the last ask.
    fn classify(&mut self, first: bool) -> Result<bool> {
        let (rejections, declines, seen) = self.locks.verdicts();
        let changed = self.seen.as_ref() != Some(&seen);
        self.seen = Some(seen);
        if !changed && !first {
            // (what this session did itself was classed as it did it)
            return Ok(false);
        }
        // this session's own marks under a verdict its reviewer has not
        // answered: a rejection of its waives - or, where its marks are
        // requests, a decline (a session without marks has none)
        let verdict = self.store.verdict();
        let others = if verdict.made == Kind::Decline { &declines } else { &rejections };
        let mut rules: BTreeSet<usize> = others.keys().copied().collect();
        rules.extend(self.rejects.checks(verdict.made));
        rules.extend(self.overlay.rejected_rules());
        let marks = self.store != Store::None;
        for check in rules.into_iter().filter(|_| marks) {
            let mut standing = others.get(&check).cloned().unwrap_or_default();
            standing.extend(confirm::standing_by(&self.reviewer, &self.rejects, check, verdict.made));
            let runs = confirm::unanswered_by(&standing, Some(&self.rejects), check, verdict.answered);
            if self.overlay.set_rejected(check, runs) {
                self.reclass(check);
            }
        }
        if self.roles.any() {
            self.follow(&rejections, &declines);
        }
        // (who confirmed what may be another thing now)
        for tally in self.tallies.borrow_mut().iter_mut() {
            tally.1 = None;
        }
        if self.overlay.lens().confirms() {
            self.chunks.clear();
        }
        Ok(changed)
    }

    /// A role's session: every reviewer's request file, waive sidecar and
    /// confirm file as the listing just made found them are what the states
    /// are read from now - each waive sidecar with the errors under a
    /// rejection its reviewer has not answered, each request file with the
    /// ones under a decline its reviewer has not answered (`rejections`,
    /// `declines`: the verdicts that stand in the other reviewers' logs, by
    /// rule) -, and the rules they may mark otherwise than before are
    /// classed anew.
    fn follow(
        &mut self,
        rejections: &BTreeMap<usize, Vec<Standing>>,
        declines: &BTreeMap<usize, Vec<Standing>>,
    ) {
        let mut found = self.locks.peers().clone();
        for (group, others, verdict) in [
            (&mut found.waives, rejections, Verdict::REJECT),
            (&mut found.requests, declines, Verdict::DECLINE),
        ] {
            let mut rules: BTreeSet<usize> = others.keys().copied().collect();
            rules.extend(self.rejects.checks(verdict.made));
            for check in rules {
                let mut standing = others.get(&check).cloned().unwrap_or_default();
                standing.extend(confirm::standing_by(&self.reviewer, &self.rejects, check, verdict.made));
                if standing.is_empty() {
                    continue;
                }
                for peer in group.iter_mut().filter(|peer| peer.any(check)) {
                    // (what a reviewer answered is in their own log: this
                    // session's is its review's, the others' the locks')
                    let answers = if peer.tag == self.reviewer {
                        Some(&self.rejects)
                    } else {
                        self.locks.answers_of(&peer.tag)
                    };
                    let runs = confirm::unanswered_by(&standing, answers, check, verdict.answered);
                    if !runs.is_empty() {
                        peer.rejected.insert(check, runs);
                    }
                }
            }
        }
        let moved = found.moved(&self.overlay.peers());
        self.overlay.set_peers(found);
        let rules = self.pack.checks.len();
        for check in moved.into_iter().filter(|&check| check < rules) {
            self.reclass(check);
        }
    }

    /// Follow the confirmers: every rejects log and confirm file is looked
    /// for anew and read where it changed, and this review's states are
    /// what they say from now. Nothing of this reviewer's is written. What
    /// comes back is what a host says and draws again: this reviewer's
    /// waives that are rejected and no look has said yet - rule by rule,
    /// who rejected them and why -, the rules whose errors are classed
    /// otherwise than before, and whether the confirmers' files changed at
    /// all. A host calls it at the open and as often as it wants to follow
    /// (the look lists the folders the files are in, and does little more
    /// while they are as they were).
    pub fn look(&mut self) -> Result<Looked> {
        if self.read_only() {
            return Ok(Looked::default());
        }
        let changed = self.classify(false)?;
        let rules = self.take_reclassed();
        let mut rejected = Vec::new();
        let mut now: BTreeMap<usize, Vec<Run>> = BTreeMap::new();
        for check in self.overlay.rejected_rules() {
            let runs = self.overlay.rejected(check).unwrap_or_default();
            let fresh = match self.said.get(&check) {
                Some(said) => reasons::minus(&runs, said),
                None => runs.to_vec(),
            };
            let held = if fresh.is_empty() { Vec::new() } else { self.held_in(check, &fresh)? };
            if !held.is_empty() {
                let by = self.rejected_by(check, &held)?;
                rejected.push(Returned {
                    check,
                    count: held.iter().map(|r| r.1 - r.0).sum(),
                    runs: held,
                    by,
                });
            }
            now.insert(check, runs.to_vec());
        }
        self.said = now;
        Ok(Looked { rejected, rules, changed })
    }

    /// Who has rule `check`'s error `error` confirmed.
    pub fn confirmed_by(&mut self, check: usize, error: u64) -> Result<Vec<String>> {
        self.locks.by(check, error)
    }

    /// Who has any of rule `check`'s errors `runs` confirmed: what the
    /// answer of a write that left locked waives says, so their reviewer
    /// knows whom to ask.
    ///
    /// A requester's session: who has any of them waived - what locks a
    /// request.
    pub fn confirmers_of(&mut self, check: usize, runs: &[Run]) -> Result<Vec<String>> {
        if self.store == Store::Requests {
            return self.locks.waivers_of(check, runs);
        }
        self.locks.who(check, runs)
    }

    /// Who has asked for the waive of any of rule `check`'s errors `runs`:
    /// the requesters whose request files the last look found (a requester's
    /// own session does not count itself).
    pub fn requesters_of(&mut self, check: usize, runs: &[Run]) -> Result<Vec<String>> {
        self.locks.requesters_of(check, runs)
    }

    /// Rule `check`'s errors `start..start+len` (clamped): true where a
    /// requester has asked for the error's waive - a request that is not
    /// declined (its requester's file has it, and they are under no decline
    /// they have not answered: as the last look read the logs). None when
    /// none has. What a strict waiver's session waives: an error nobody
    /// asked the waive of is left (`Left::unrequested`).
    pub fn requested(&self, check: usize, start: u64, len: u64) -> Result<Option<Vec<bool>>> {
        let (first, n) = self.check(check)?;
        let start = start.min(n);
        let len = len.min(n - start) as usize;
        let peers = self.overlay.peers();
        let mut out: Option<Vec<bool>> = None;
        let mut bytes = vec![0u8; len];
        for peer in &peers.requests {
            if !peer.read(first + start, &mut bytes) || !bytes.contains(&STATUS_WAIVED) {
                continue;
            }
            let declined = peer.rejected.get(&check).map(Vec::as_slice).unwrap_or(&[]);
            let out = out.get_or_insert_with(|| vec![false; len]);
            for (j, b) in bytes.iter().enumerate() {
                if *b == STATUS_WAIVED && !within(declined, start + j as u64) {
                    out[j] = true;
                }
            }
        }
        Ok(out.filter(|asked| asked.contains(&true)))
    }

    /// A status is this session's to write: not one's that makes no mark -
    /// a confirmer's who is neither waiver nor requester, whose role
    /// confirms and rejects the waives of others and makes none.
    fn may_mark(&self) -> Result<()> {
        if self.store == Store::None {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "a confirmer's session does not waive: it confirms or rejects the team's waives (--role waiver waives)",
            ));
        }
        Ok(())
    }

    /// How old what was read of the confirmers' files may be before it is
    /// read anew (`confirm::LOCK_REFRESH`; the tests ask at each look).
    pub fn set_lock_refresh(&mut self, every: std::time::Duration) {
        self.locks.set_refresh(every);
    }

    /// A confirmer turned rule `check`'s waives `back` (not empty; within a
    /// span the caller wrote) back: their own confirmation of them goes
    /// with it. Left, it would count again - and lock - the moment anybody
    /// waived the error anew, unseen by its confirmer.
    fn unconfirm(&mut self, check: usize, back: &[u64]) -> Result<()> {
        if !self.roles.confirmer || !self.confirms.present() {
            return Ok(());
        }
        let (Some(&first), Some(&last)) = (back.iter().min(), back.iter().max()) else {
            return Ok(());
        };
        if (last - first) / 8 > back.len() as u64 {
            // scattered: an error at a time
            self.confirms.set(check, back, false)?;
            return Ok(());
        }
        let was = self.confirms.statuses(check, first, last - first + 1)?;
        if !was.contains(&confirm::CONFIRMED) {
            return Ok(());
        }
        let mut now = was.clone();
        for &error in back {
            now[(error - first) as usize] = 0;
        }
        self.confirms.write_span(check, first, &was, &now)?;
        Ok(())
    }

    fn may_confirm(&self) -> Result<()> {
        if self.confirmer() {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::Unsupported,
                "confirming waives is the confirmer's: start the viewer with --role confirmer",
            ))
        }
    }

    /// Confirm (`on`) the waives of rule `check`'s `errors`, or take this
    /// reviewer's confirmation of them back: written in place and flushed;
    /// the changed count. Whether somebody has the errors waived is the
    /// caller's to ask (drc::team::waives): a confirmation counts while
    /// somebody does.
    pub fn confirm_set(&mut self, check: usize, errors: &[u64], on: bool) -> Result<u64> {
        self.may_confirm()?;
        self.restage = self.roles.any();
        let changed = self.confirms.set(check, errors, on)?;
        for &error in &changed {
            self.touched(check, error, error);
        }
        if on {
            self.unreject(check, &reasons::runs_of(errors))?;
        }
        self.sync_waives()?;
        Ok(changed.len() as u64)
    }

    /// One span of a job that confirms: rule `check`'s confirmation bytes
    /// `start..` from `before` (as read) to `after`; the changed count. The
    /// job ends with `sync_waives`.
    pub fn confirm_span(&mut self, check: usize, start: u64, before: &[u8], after: &[u8]) -> Result<u64> {
        self.may_confirm()?;
        self.restage = self.roles.any();
        let changed = self.confirms.write_span(check, start, before, after)?;
        if changed > 0 {
            self.touched(check, start, start + after.len() as u64 - 1);
        }
        if self.rejects.any(Kind::Reject, check) {
            let confirmed: Vec<u64> = (0..after.len())
                .filter(|&j| after[j] == confirm::CONFIRMED)
                .map(|j| start + j as u64)
                .collect();
            self.unreject(check, &reasons::runs_of(&confirmed))?;
        }
        Ok(changed)
    }

    // ---- reasons (0.12.463) --------------------------------------------

    /// The reason of an action - `kind` - on rule `check`'s `runs`: every
    /// error of them holds it from now, in place of what it held; written
    /// and flushed. The caller has made the errors waived (or set them
    /// aside) first: a reason that cannot be kept leaves that done, and the
    /// error says so.
    pub fn reason_set(&mut self, kind: Kind, check: usize, runs: &[Run], text: &str) -> Result<()> {
        let result = self.reasons.set(kind, check, runs, text, now());
        self.notices.append(&mut self.reasons.notices);
        result.map_err(|e| {
            Error::new(
                e.kind,
                format!("{} ({})", e.message, self.reasons.path.display()),
            )
        })
    }

    /// The reasons of rule `check`'s `runs` are gone: the errors were turned
    /// back, or changed by something that gave no reason. No longer shown
    /// from now; said in the file by the caller's `sync_waives` - the end
    /// of its request or job -, where a file that cannot be written is said
    /// in the notices and stops nothing (the readers ask the status besides).
    pub fn reason_drop(&mut self, kind: Kind, check: usize, runs: &[Run]) {
        if !runs.is_empty() {
            self.reasons.clear(kind, check, runs);
        }
    }

    /// Why rule `check`'s error `error` is waived: None when it is not
    /// waived, or was waived without a reason. (A rejected waive has the
    /// reason it was given: it is what its reviewer waives it again from.)
    pub fn waive_reason(&self, check: usize, error: u64) -> Result<Option<&reasons::Entry>> {
        if self.raw_one(check, error)? != STATUS_WAIVED {
            return Ok(None);
        }
        Ok(self.reasons.of(self.mark, check, error))
    }

    /// The waive reasons of rule `check`'s `changed` errors go with the
    /// write that changed them - the ones of `gone`, rejected waives turned
    /// back, kept as they were first.
    fn drop_waive_reasons(&mut self, check: usize, changed: &[u64], gone: &[u64]) -> Result<()> {
        if !self.reasons.any(self.mark, check) {
            return Ok(());
        }
        self.keep_earlier(check, &reasons::runs_of(gone))?;
        self.reason_drop(self.mark, check, &reasons::runs_of(changed));
        Ok(())
    }

    /// Set the status of check `check`'s errors `errors` to `value`, in
    /// place, keeping the rule's waived counter in step; flushed. The
    /// errors it changes lose their waive reason: turned back they have
    /// none, and waived here they are waived without one until the caller
    /// gives it (`reason_set`, which is every error's of the action then).
    ///
    /// The errors of `errors` this write leaves as they are come back,
    /// each list sorted: the ones whose waive is locked (`locked`) - not
    /// turned back, and, waived again, not the caller's to give a reason to
    /// - and the ones it would have waived that are under a rejection this
    /// reviewer has not answered (`rejected`): those are waived again by
    /// `resubmit`, when the caller has a reason for them. A rejected waive
    /// is turned back like any other - its reason kept as it was.
    ///
    /// A role's session: a requester's status is a request - made of any
    /// error, and not taken back while somebody has the error waived
    /// (`locked`); a waiver waives the errors somebody asked the waive of,
    /// and the others come back in `unrequested`; a confirmer writes none.
    pub fn set_status(&mut self, check: usize, errors: &[u64], value: u8) -> Result<Left<u64>> {
        self.may_mark()?;
        let (start, n) = self.check(check)?;
        if errors.iter().any(|&e| e >= n) {
            return Err(Error::input("DRC error index out of range"));
        }
        let rejected = self.rejected_runs(check, false)?;
        let waiver = self.strict() && value == STATUS_WAIVED;
        // (a waiver who waives what they declined the request of takes the
        // decline back with it: the waive is their other word on it)
        if value == STATUS_WAIVED && self.roles.waiver && self.rejects.any(Kind::Decline, check) {
            let mut targets = errors.to_vec();
            targets.sort_unstable();
            targets.dedup();
            self.undecline(check, &reasons::runs_of(&targets))?;
        }
        let steps = self.steps(check);
        let mut changed = Vec::new();
        let (mut left, mut back, mut gone) = (Left::default(), Vec::new(), Vec::new());
        for &error in errors {
            let gid = start + error;
            let mut old = [0u8; 1];
            self.reader.read_exact_at(&mut old, self.status_off + gid)?;
            let is_rejected = within(&rejected, error);
            if value == STATUS_WAIVED && is_rejected {
                // (waived here or not: under a rejection it is not this
                // write's to waive)
                left.rejected.push(error);
                continue;
            }
            if old[0] == STATUS_WAIVED && !is_rejected && self.locked(check, error, 1)?.is_some() {
                left.locked.push(error);
                continue;
            }
            if old[0] == value {
                continue;
            }
            // (a waiver waives what was asked for)
            if waiver && self.requested(check, error, 1)?.is_none() {
                left.unrequested.push(error);
                continue;
            }
            if old[0] == STATUS_WAIVED {
                back.push(error);
                if is_rejected {
                    gone.push(error);
                }
            }
            changed.push(error);
            let (status_off, wcount_off) = (self.status_off, self.wcount_off);
            self.writer()?.write_all_at(&[value], status_off + gid)?;
            let (was, now) = (old[0] == STATUS_WAIVED, value == STATUS_WAIVED);
            if was != now {
                let cur = i64::from(self.wcount[check]) + if now { 1 } else { -1 };
                let cur = cur.max(0) as u32;
                self.writer()?
                    .write_all_at(&cur.to_le_bytes(), wcount_off + 4 * check as u64)?;
                self.wcount[check] = cur;
                if let Some(chunks) = self.chunks.get_mut(&check).filter(|_| steps) {
                    let k = (error / STATUS_CHUNK) as usize;
                    if now {
                        chunks[k] += 1;
                    } else {
                        chunks[k] = chunks[k].saturating_sub(1);
                    }
                }
            }
        }
        if !changed.is_empty() {
            self.recount(check);
        }
        self.drop_waive_reasons(check, &changed, &gone)?;
        for &error in &changed {
            self.touched(check, error, error);
        }
        self.unconfirm(check, &back)?;
        self.sync_waives()?;
        for list in [&mut left.locked, &mut left.rejected, &mut left.unrequested] {
            list.sort_unstable();
            list.dedup();
        }
        Ok(left)
    }

    /// One chunk of `set_status_members`: errors `a..a+len` of rule `check`;
    /// the changed count. The members this write leaves as they are - the
    /// ones whose waive is locked (`locked`), the ones not waived because
    /// their waive is rejected (`rejected`) - are added to `left` as runs,
    /// in order when the chunks are asked in order.
    pub fn set_status_range(
        &mut self,
        check: usize,
        members: &dyn super::analysis::scope::Membership,
        value: u8,
        a: u64,
        len: u64,
        left: &mut Left<Run>,
    ) -> Result<u64> {
        self.may_mark()?;
        let (start, n) = self.check(check)?;
        let len = len.min(n.saturating_sub(a));
        let mut mask = vec![false; len as usize];
        members.mask(a, &mut mask);
        if !mask.iter().any(|&m| m) {
            return Ok(0);
        }
        let before = self.raw(check, a, len)?;
        let rejected = self.rejected(check, a, len)?;
        if value == STATUS_WAIVED {
            if let Some(rejected) = &rejected {
                for j in (0..len as usize).filter(|&j| rejected[j]) {
                    if std::mem::take(&mut mask[j]) {
                        push_run(&mut left.rejected, a + j as u64);
                    }
                }
            }
            // (a waiver waives what was asked for: the members nobody
            // asked the waive of are left, and said)
            if self.roles.waiver && self.rejects.any(Kind::Decline, check) {
                let targets: Vec<u64> = (0..len as usize).filter(|&j| mask[j]).map(|j| a + j as u64).collect();
                self.undecline(check, &reasons::runs_of(&targets))?;
            }
            if self.strict() {
                let asked = self.requested(check, a, len)?;
                for j in 0..len as usize {
                    if mask[j] && before[j] != STATUS_WAIVED && !asked.as_ref().is_some_and(|asked| asked[j]) {
                        mask[j] = false;
                        push_run(&mut left.unrequested, a + j as u64);
                    }
                }
            }
        }
        if (0..len as usize).any(|j| mask[j] && before[j] == STATUS_WAIVED) {
            if let Some(locked) = self.locked(check, a, len)? {
                for j in (0..len as usize).filter(|&j| locked[j] && before[j] == STATUS_WAIVED) {
                    if std::mem::take(&mut mask[j]) {
                        push_run(&mut left.locked, a + j as u64);
                    }
                }
            }
        }
        let positions: Vec<usize> = (0..len as usize)
            .filter(|&j| mask[j] && before[j] != value)
            .collect();
        let (Some(&lo), Some(&hi)) = (positions.first(), positions.last()) else {
            return Ok(0);
        };
        let mut after = before[lo..=hi].to_vec();
        for &j in &positions {
            after[j - lo] = value;
        }
        let delta: i64 = positions
            .iter()
            .map(|&j| i64::from(value == STATUS_WAIVED) - i64::from(before[j] == STATUS_WAIVED))
            .sum();
        let (status_off, wcount_off) = (self.status_off, self.wcount_off);
        self.writer()?
            .write_all_at(&after, status_off + start + a + lo as u64)?;
        if delta != 0 {
            let cur = (i64::from(self.wcount[check]) + delta).max(0) as u32;
            self.writer()?
                .write_all_at(&cur.to_le_bytes(), wcount_off + 4 * check as u64)?;
            self.wcount[check] = cur;
        }
        let steps = self.steps(check);
        self.recount(check);
        // (as `set_status`: what changed holds no reason until one is
        // given, and a rejected waive turned back has its reason kept)
        if self.reasons.any(self.mark, check) {
            let changed: Vec<u64> = positions.iter().map(|&j| a + j as u64).collect();
            let gone: Vec<u64> = match &rejected {
                Some(rejected) if value != STATUS_WAIVED => positions
                    .iter()
                    .filter(|&&j| rejected[j] && before[j] == STATUS_WAIVED)
                    .map(|&j| a + j as u64)
                    .collect(),
                _ => Vec::new(),
            };
            self.drop_waive_reasons(check, &changed, &gone)?;
        }
        self.touched(check, a + lo as u64, a + hi as u64);
        if self.roles.confirmer && value != STATUS_WAIVED {
            let back: Vec<u64> = positions
                .iter()
                .filter(|&&j| before[j] == STATUS_WAIVED)
                .map(|&j| a + j as u64)
                .collect();
            self.unconfirm(check, &back)?;
        }
        if let Some(chunks) = self.chunks.get_mut(&check).filter(|_| steps) {
            for &j in &positions {
                let k = ((a + j as u64) / STATUS_CHUNK) as usize;
                if value == STATUS_WAIVED {
                    chunks[k] += 1;
                } else if before[j] == STATUS_WAIVED {
                    chunks[k] = chunks[k].saturating_sub(1);
                }
            }
        }
        Ok(positions.len() as u64)
    }

    /// Set the status of every member of `members` in rule `check` to
    /// `value`, a 65536-error chunk at a time (feature/jobdeck-drc
    /// `set_status_members`): each chunk's changed span written in place,
    /// the counter after it; `progress(done, total)` after each chunk;
    /// cancelling leaves the chunks written so far. The changed count; the
    /// members left as they were - locked, rejected - are added to `left`.
    pub fn set_status_members(
        &mut self,
        check: usize,
        members: &dyn super::analysis::scope::Membership,
        value: u8,
        cancelled: &AtomicUsize,
        progress: &mut dyn FnMut(u64, u64),
        left: &mut Left<Run>,
    ) -> Result<u64> {
        const CHUNK: u64 = 1 << 16;
        let (_, n) = self.check(check)?;
        let mut changed = 0u64;
        let mut a = 0;
        while a < n {
            if let Err(e) = check_cancelled(cancelled) {
                // the chunks written so far stay: flushed with the rest
                self.sync_waives()?;
                return Err(e);
            }
            let len = (n - a).min(CHUNK);
            changed += self.set_status_range(check, members, value, a, len, left)?;
            a += len;
            progress(a, n);
        }
        self.sync_waives()?;
        Ok(changed)
    }

    /// Write rule `check`'s statuses `start..start+after.len()` from
    /// `before` (as `statuses` read them) to `after` in one span, the
    /// counters in step (floe_oracle drc.py `_write_status_span`). A waive
    /// `after` would turn back stays waived when it is locked (`locked`),
    /// and an error `after` would waive stays as it is when it is under a
    /// rejection this reviewer has not answered (`rejected` - an applied
    /// decision carries no reason to waive it again with): how many of
    /// each. An error the caller leaves as it read it stays as the file
    /// has it - a rejected waive, which reads as no waive, among them.
    pub fn write_status_span(
        &mut self,
        check: usize,
        start: u64,
        before: &[u8],
        after: &[u8],
    ) -> Result<Kept> {
        self.may_mark()?;
        let (_, n) = self.check(check)?;
        if before.len() != after.len() || start + after.len() as u64 > n {
            return Err(Error::input("status span outside its rule"));
        }
        let len = after.len() as u64;
        let raw = self.raw(check, start, len)?;
        let turned = |b: u8, a: u8| b == STATUS_WAIVED && a != STATUS_WAIVED;
        // what the caller asks, in the file's terms
        let mut now: Vec<u8> = (0..after.len())
            .map(|j| if after[j] == before[j] { raw[j] } else { after[j] })
            .collect();
        let mut kept = Kept::default();
        if before.iter().zip(after).any(|(b, a)| turned(*a, *b)) {
            if let Some(rejected) = self.rejected(check, start, len)? {
                for j in 0..now.len() {
                    if rejected[j] && turned(after[j], before[j]) {
                        now[j] = raw[j];
                        kept.rejected += 1;
                    }
                }
            }
        }
        if raw.iter().zip(&now).any(|(b, a)| turned(*b, *a)) {
            if let Some(locked) = self.locked(check, start, len)? {
                for j in 0..now.len() {
                    if locked[j] && turned(raw[j], now[j]) {
                        now[j] = STATUS_WAIVED;
                        kept.locked += 1;
                    }
                }
            }
        }
        // (a waiver waives what was asked for: an applied decision too)
        if self.strict() && raw.iter().zip(&now).any(|(b, a)| turned(*a, *b)) {
            let asked = self.requested(check, start, len)?;
            for j in 0..now.len() {
                if turned(now[j], raw[j]) && !asked.as_ref().is_some_and(|asked| asked[j]) {
                    now[j] = raw[j];
                    kept.unrequested += 1;
                }
            }
        }
        self.write_span(check, start, &raw, &now)?;
        Ok(kept)
    }

    /// `write_status_span`'s write, nothing asked: the bytes, the rule's
    /// counter, the reasons of what changed, the change log's word - and a
    /// confirmer's own confirmation of what they turn back.
    fn write_span(&mut self, check: usize, start: u64, before: &[u8], after: &[u8]) -> Result<()> {
        let (first, _) = self.check(check)?;
        let turned = |b: u8, a: u8| b == STATUS_WAIVED && a != STATUS_WAIVED;
        let any_back = before.iter().zip(after).any(|(b, a)| turned(*b, *a));
        let waived = |s: &[u8]| s.iter().filter(|&&v| v == STATUS_WAIVED).count() as i64;
        let delta = waived(after) - waived(before);
        let (status_off, wcount_off) = (self.status_off, self.wcount_off);
        if before == after {
            return Ok(());
        }
        self.writer()?.write_all_at(after, status_off + first + start)?;
        let steps = self.steps(check);
        self.recount(check);
        // a peer's decision or a previous run's carries no reason of mine
        // (a rejected waive it turns back has its reason kept as it was)
        if self.reasons.any(self.mark, check) {
            let changed: Vec<u64> = before
                .iter()
                .zip(after)
                .enumerate()
                .filter(|(_, (b, a))| b != a)
                .map(|(j, _)| start + j as u64)
                .collect();
            let gone: Vec<u64> = match self.overlay.rejected(check) {
                Some(rejected) if any_back => (0..after.len())
                    .filter(|&j| turned(before[j], after[j]) && within(&rejected, start + j as u64))
                    .map(|j| start + j as u64)
                    .collect(),
                _ => Vec::new(),
            };
            self.drop_waive_reasons(check, &changed, &gone)?;
        }
        if !after.is_empty() {
            self.touched(check, start, start + after.len() as u64 - 1);
        }
        if self.roles.confirmer && any_back {
            let back: Vec<u64> = (0..after.len())
                .filter(|&j| turned(before[j], after[j]))
                .map(|j| start + j as u64)
                .collect();
            self.unconfirm(check, &back)?;
        }
        if delta != 0 {
            let cur = (i64::from(self.wcount[check]) + delta).max(0) as u32;
            self.writer()?
                .write_all_at(&cur.to_le_bytes(), wcount_off + 4 * check as u64)?;
            self.wcount[check] = cur;
            if let Some(chunks) = self.chunks.get_mut(&check).filter(|_| steps) {
                for (j, (b, a)) in before.iter().zip(after).enumerate() {
                    let k = ((start + j as u64) / STATUS_CHUNK) as usize;
                    if *a == STATUS_WAIVED && *b != STATUS_WAIVED {
                        chunks[k] += 1;
                    } else if *b == STATUS_WAIVED && *a != STATUS_WAIVED {
                        chunks[k] = chunks[k].saturating_sub(1);
                    }
                }
            }
        }
        Ok(())
    }

    /// Per-chunk counts of one rule's errors of the lens's class - the
    /// waived ones, with no filter of states on: built on the first
    /// filtered access with one pass, then kept in step by the writers
    /// where a status byte is all there is to the class (`steps`).
    fn wf_chunks(&mut self, check: usize) -> Result<&Vec<u64>> {
        if !self.chunks.contains_key(&check) {
            let (_, n) = self.check(check)?;
            let mut counts = Vec::new();
            let mut a = 0;
            while a < n {
                let b = (a + STATUS_CHUNK).min(n);
                let st = self.lensed(check, a, b - a)?;
                counts.push(st.iter().filter(|&&s| s == STATUS_WAIVED).count() as u64);
                a = b;
            }
            self.chunks.insert(check, counts);
        }
        Ok(&self.chunks[&check])
    }

    /// The `start`..`start+limit`-th errors of one rule whose waived-ness
    /// is `waived`, as rule-local indices. ("Waived" is the lens's class
    /// here and in `status_rank` and the queries: `lensed`.)
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
            let st = self.lensed(check, a, b - a)?;
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
        self.error_gid(check, error)?;
        if (self.lensed(check, error, 1)?[0] == STATUS_WAIVED) != waived {
            return Ok(None);
        }
        let k = error / STATUS_CHUNK;
        let before: u64 = self.wf_chunks(check)?[..k as usize].iter().sum();
        let a = k * STATUS_CHUNK;
        let w_in = self
            .lensed(check, a, error - a)?
            .iter()
            .filter(|&&s| s == STATUS_WAIVED)
            .count() as u64;
        Ok(Some(if waived {
            before + w_in
        } else {
            (a - before) + (error - a - w_in)
        }))
    }

    /// `query` restricted to `members` too (a cluster, a delta group, a
    /// selection - of the rule the query is limited to), before the cap.
    pub fn query_scoped(
        &mut self,
        bbox_um: [f64; 4],
        cap: usize,
        checks: Option<&BTreeSet<usize>>,
        waived: Option<bool>,
        members: Option<&dyn super::analysis::scope::Membership>,
        cancelled: &AtomicUsize,
    ) -> Result<Vec<(usize, u64, Violation)>> {
        let Some(members) = members else {
            return self.query(bbox_um, cap, checks, waived, cancelled);
        };
        let mut out = Vec::new();
        let mut cursor = super::Cursor::default();
        while out.len() < cap {
            check_cancelled(cancelled)?;
            let page = self.pack.query(bbox_um, checks, None, cursor, super::PAGE_ITEMS, cancelled)?;
            for hit in page.hits {
                if !members.contains(hit.local) {
                    continue;
                }
                if let Some(w) = waived {
                    if (self.lensed(hit.check, hit.local, 1)?[0] == STATUS_WAIVED) != w {
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
                    if (self.lensed(hit.check, hit.local, 1)?[0] == STATUS_WAIVED) != w {
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
                let aside = aside_path(&path);
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

    /// Save the review state to `dst`, the autosave's format: the status
    /// bytes as they are - a rejected waive is a waive of the file, as it
    /// is of the sidecar; whether it is rejected is the confirmers' logs'.
    ///
    /// A session whose marks are not waives - a requester's requests, a
    /// confirmer's none - has no waives of its own to save.
    pub fn waive_export(&self, dst: &Path) -> Result<()> {
        if self.store != Store::Waives {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "this session has no waives of its own to save (--role waiver waives)",
            ));
        }
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
    /// size); the counters recomputed. The waived count, and what stays as
    /// it was: the waives the file would have turned back that are locked
    /// (`locked`), and the errors it would have waived that are under a
    /// rejection this reviewer has not answered (`rejected`).
    ///
    /// A role's session: a waiver's takes the file's waives of the errors
    /// somebody asked the waive of (`unrequested`: the others); a
    /// requester's and a confirmer's take none - a saved waive file is no
    /// request, and a confirmer waives nothing.
    pub fn waive_import(&mut self, src: &Path) -> Result<(u64, Kept)> {
        self.may_mark()?;
        if self.store == Store::Requests {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "a requester's session loads no waive file: its marks are requests (--role waiver waives)",
            ));
        }
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
        let mut status = data[HEADER as usize..HEADER as usize + n].to_vec();
        // what the file would turn back: a locked waive stays, and a
        // confirmer's own confirmation goes with the waive it turns back
        let mut current = vec![0u8; n];
        self.reader.read_exact_at(&mut current, self.status_off)?;
        let rules: Vec<(usize, usize)> = self
            .pack
            .checks
            .iter()
            .map(|c| (c.start as usize, c.count as usize))
            .collect();
        let mut kept = Kept::default();
        for (check, &(first, count)) in rules.iter().enumerate() {
            // an error the file would waive that is under a rejection stays
            // as it is: turned back it is not waived again without a reason
            // (a rejected waive this review has stays, rejected)
            if (first..first + count).any(|g| status[g] == STATUS_WAIVED) {
                if let Some(rejected) = self.rejected(check, 0, count as u64)? {
                    for j in 0..count {
                        if rejected[j] && status[first + j] == STATUS_WAIVED && current[first + j] != STATUS_WAIVED {
                            status[first + j] = current[first + j];
                            kept.rejected += 1;
                        }
                    }
                }
            }
            // (a waiver waives what was asked for: a waive of the file's
            // that nobody asked for is not taken)
            if self.strict()
                && (first..first + count).any(|g| status[g] == STATUS_WAIVED && current[g] != STATUS_WAIVED)
            {
                let asked = self.requested(check, 0, count as u64)?;
                for j in 0..count {
                    if status[first + j] == STATUS_WAIVED
                        && current[first + j] != STATUS_WAIVED
                        && !asked.as_ref().is_some_and(|asked| asked[j])
                    {
                        status[first + j] = current[first + j];
                        kept.unrequested += 1;
                    }
                }
            }
            let turned = |g: usize, status: &[u8]| current[g] == STATUS_WAIVED && status[g] != STATUS_WAIVED;
            if !(first..first + count).any(|g| turned(g, &status)) {
                continue;
            }
            if self.roles.confirmer {
                if self.confirms.present() && self.confirms.counts()[check] > 0 {
                    let was = self.confirms.statuses(check, 0, count as u64)?;
                    let mut now = was.clone();
                    for j in (0..count).filter(|&j| turned(first + j, &status)) {
                        now[j] = 0;
                    }
                    self.confirms.write_span(check, 0, &was, &now)?;
                }
            } else if let Some(locked) = self.locked(check, 0, count as u64)? {
                for j in 0..count {
                    if locked[j] && turned(first + j, &status) {
                        status[first + j] = STATUS_WAIVED;
                        kept.locked += 1;
                    }
                }
            }
        }
        drop(current);
        let status = &status[..];
        let counts: Vec<u32> = rules
            .iter()
            .map(|&(first, count)| {
                status[first..first + count]
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
        for tally in self.tallies.borrow_mut().iter_mut() {
            *tally = (None, None);
        }
        for counts in self.by_state.borrow_mut().iter_mut() {
            *counts = None;
        }
        // a reason stays with an error the loaded file has waived too, and
        // goes with one it turns back
        for check in self.reasons.checks(self.mark) {
            let first = self.pack.checks[check].start as usize;
            let mut back = Vec::new();
            for (a, b, _) in self.reasons.runs(self.mark, check) {
                back.extend((a..b).filter(|&e| status[first + e as usize] != STATUS_WAIVED));
            }
            self.reason_drop(self.mark, check, &reasons::runs_of(&back));
        }
        // (every status may have changed: the team counts this review again)
        self.changes.lock().unwrap_or_else(|p| p.into_inner()).everything();
        self.sync_waives()?;
        Ok((status.iter().filter(|&&s| s == STATUS_WAIVED).count() as u64, kept))
    }
}

/// What a reviewer's rejects log keeps of rule `check`'s error `error`: the
/// waive reasons it had and the rejections that were answered, in the order
/// they happened (the log's: `Reasons::kept_at`).
pub fn history_of(log: &Reasons, check: usize, error: u64) -> Vec<Event> {
    log.kept_at(check, error)
        .into_iter()
        .map(|e| Event {
            what: match e.kind {
                Kind::Answered => "rejected",
                Kind::Reasked => "declined",
                Kind::Asked => "requested",
                _ => "waived",
            },
            by: e.by.clone(),
            text: e.text.clone(),
            time: e.time,
        })
        .collect()
}

/// Statuses read apart from the review (worker threads): the same bytes,
/// and what the review last read of the confirmers beside them.
#[derive(Clone)]
pub struct StatusReader {
    file: std::sync::Arc<File>,
    status_off: u64,
    checks: std::sync::Arc<Vec<(u64, u64)>>,
    overlay: Arc<Overlay>,
}
impl StatusReader {
    fn classed(&self, check: usize, start: u64, count: u64, lens: Lens) -> Result<Vec<u8>> {
        let &(first, n) = self
            .checks
            .get(check)
            .ok_or_else(|| Error::input("DRC check index out of range"))?;
        let start = start.min(n);
        let count = count.min(n - start);
        let mut out = vec![0u8; count as usize];
        self.file
            .read_exact_at(&mut out, self.status_off + first + start)?;
        self.overlay.class(check, first, start, &mut out, lens);
        Ok(out)
    }

    /// Check `check`'s statuses `start..start+count` (clamped): 1 = waived
    /// and not rejected - this reviewer's own marks (`Review::statuses`).
    pub fn statuses(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>> {
        let &(first, n) = self
            .checks
            .get(check)
            .ok_or_else(|| Error::input("DRC check index out of range"))?;
        let start = start.min(n);
        let count = count.min(n - start);
        let mut out = vec![0u8; count as usize];
        self.file
            .read_exact_at(&mut out, self.status_off + first + start)?;
        self.overlay.own(check, start, &mut out);
        Ok(out)
    }

    /// 1 = of the class the review's lens shows (`Review::lensed`).
    pub fn lensed(&self, check: usize, start: u64, count: u64) -> Result<Vec<u8>> {
        self.classed(check, start, count, self.overlay.lens())
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
    Ascii(std::sync::Arc<super::Ascii>),
}

impl Opened {
    /// floe/drc.py load_db: a pack given (by its magic), or the .db's pack
    /// when it is current, else the ASCII file itself - the reason said.
    pub fn load(
        path: &Path,
        reviewer: Option<&str>,
        cancelled: &AtomicUsize,
    ) -> Result<(Self, Vec<String>)> {
        Self::load_as(path, reviewer, Roles::NONE, cancelled)
    }

    /// `load`, the review opened as a session of `roles` (`Review::open_as`).
    pub fn load_as(
        path: &Path,
        reviewer: Option<&str>,
        roles: Roles,
        cancelled: &AtomicUsize,
    ) -> Result<(Self, Vec<String>)> {
        let mut magic = [0u8; 8];
        let packed = File::open(path)?.read_exact(&mut magic).is_ok() && &magic == super::MAGIC;
        if packed {
            return Ok((
                Self::Pack(Box::new(Review::open_as(path, None, reviewer, roles, cancelled)?)),
                Vec::new(),
            ));
        }
        let mut warnings = Vec::new();
        let side = crate::cache::pack_path(path)?;
        if side.exists() {
            match Review::open_with(&side, Some(path), reviewer, cancelled, true, roles) {
                Ok(review) => return Ok((Self::Pack(Box::new(review)), Vec::new())),
                // a pack being rebuilt, a review open in another session, a
                // reviewer id required and not given: said as they are, never
                // answered with the ASCII file (which has no review)
                Err(e)
                    if matches!(
                        e.kind,
                        ErrorKind::Busy | ErrorKind::Cancelled | ErrorKind::Admission
                    ) =>
                {
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
            Self::Ascii(std::sync::Arc::new(super::Ascii::open(path, cancelled)?)),
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
