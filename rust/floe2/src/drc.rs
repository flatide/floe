//! `floe2 gtk-service`'s DRC review (P2b; app-core drc::desktop - floe/drc.py
//! IcePack's files and autosave) and its analysis (feature/jobdeck-drc,
//! ported 2026-10-11; app-core drc::analysis): user clusters, CD delta
//! groups, whole-scope point markers, unlimited selections and bulk review
//! edits. Long work runs on worker threads, one per kind and database -
//! a new request of a kind cancels the one before - and speaks with
//! `{"event": "drc", "handle", "job", "kind", "state": "progress" |
//! "partial" | "done" | "error", ...}` lines between the replies.
//!
//! Requests, by the handle `drc_open` gives:
//!   drc_busy {db}, drc_find {db}, drc_open {path, source?, reviewer?, mode: pack|load}
//!   drc_errors {handle, check, start, count}
//!   drc_status {handle, check, start, count}   status bytes, hex
//!   drc_counts {handle}                        [[waived, total], ...] of every rule
//!   drc_set_status {handle, check, errors, status, reason?}   `reason`: why
//!       they are waived - one line for the action (app-core drc::reasons)
//!   drc_reason_get {handle, check, error}      why it is waived / set aside now
//!   drc_reason_read {handle, check, reason_of: waive|not-required, errors, toggle?}
//!       what the reason dialog starts from: {on, prefill, known}
//!   drc_status_page / drc_status_rank {handle, check, waived, start, limit | error}
//!   drc_query {handle, bbox, cap, checks?, waived?, check?, scope?}
//!   drc_set_note / drc_clear_note {handle, gids[, text]}
//!   drc_note_export / drc_note_import / drc_waive_export / drc_waive_import {handle, path}
//!   drc_cd {handle, check, error}; drc_cd_info {constraints, handle+check+error | markers}
//!   drc_clusters_load {handle, path?}      the `<db>.clusters` beside the db when no path
//!   drc_cluster_counts {handle, check}     [[waived, total], ...] of its clusters
//!   drc_scope_count / drc_scope_page / drc_scope_rank / drc_scope_contains
//!       {handle, check, scope: {cluster} | {group, row} | {selection}, waived?, ...}
//!   drc_delta {handle, check, constraints, basis, step?, cluster?}      job "delta"
//!   drc_delta_cached {handle, basis, checks:[{check,constraints,step?}], prepare?} job "delta_cached"
//!   drc_delta_rows {handle, group, start, limit, waived?}; drc_delta_find {handle, group, key}
//!   drc_delta_drop {handle, group}
//!   drc_markers {handle, check, bounds, width, height, scope?, waived?, selection?}  job "markers"
//!   drc_select {handle, check, bounds?, scope?, waived?, previous?, mode, errors?}   job "select"
//!   drc_selection_page {handle, selection, start, limit}; drc_selection_drop {handle, selection}
//!   drc_review {handle, check, action: waive|not-required|reason-read|note|note-clear|note-read,
//!       selection | errors, on?, text?, reason?, reason_of?, toggle?}  job "review"
//!   drc_inview {handle, check, bbox, cap, scope?, waived?}                 job "inview"
//!   drc_lens {handle, lens: live|held|waived|confirmed|rejected}: the class of
//!       errors a `waived` filter shows from now (app-core desktop::Lens)
//!   drc_cancel {handle, kind}; drc_close {handle}; drc_parse_step {text}
//!   svrf_rules {path}, svrf_operands {rhs}, svrf_rules_candidates {db, deck?}
//! and drc_team.rs's (review flags, snapshots, team, migration).
#[path = "drc_team.rs"]
mod team;

use crate::view::Out;
use floe_app_core::{
    desktop,
    drc::analysis::{
        clusters::{self, ClusterFile, NoStatuses},
        delta::{Basis, Cache, GroupStatus, Groups, Table, CACHE_THRESHOLD},
        measure::{area_label, marker_length_ticks, measurement_ticks, Selector},
        points,
        scope::{Membership, Scope, Selection},
        select,
        source::{worker_count, AsciiSource, PackSource, Source, Statuses},
        ticks,
    },
    drc::desktop::{query_rows, ErrorRow, Left, Lens, Opened, Review},
    drc::reasons::{self, Kind},
    Error, ErrorKind, Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

fn text<'a>(request: &'a Value, key: &str) -> Result<&'a str> {
    request
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::input(format!("{key} must be a string")))
}

fn number(request: &Value, key: &str) -> Result<u64> {
    request
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::input(format!("{key} must be a number")))
}

fn numbers(request: &Value, key: &str) -> Result<Vec<u64>> {
    request
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| Error::input(format!("{key} must be a list")))?
        .iter()
        .map(|v| {
            v.as_u64()
                .ok_or_else(|| Error::input(format!("{key} must hold numbers")))
        })
        .collect()
}

fn floats<const N: usize>(request: &Value, key: &str) -> Result<[f64; N]> {
    let bad = || Error::input(format!("{key} must be {N} numbers"));
    let v = request
        .get(key)
        .and_then(Value::as_array)
        .filter(|b| b.len() == N)
        .ok_or_else(bad)?;
    let mut out = [0.0; N];
    for (o, x) in out.iter_mut().zip(v) {
        *o = x.as_f64().ok_or_else(bad)?;
    }
    Ok(out)
}

fn waived_of(request: &Value) -> Option<bool> {
    request.get("waived").and_then(Value::as_bool)
}

/// The reasons the dialog offers again: the latest given, each once.
const KNOWN_REASONS: usize = 20;

/// A review job over this many errors or more has the team's kept counts
/// brought up to date when it ends (`Shared::team_soon`).
const TEAM_SOON: u64 = 1 << 16;

/// A request's reason as it is kept - refused, when it cannot be, before
/// anything is written.
fn reason_of(request: &Value) -> Result<String> {
    reasons::clean(request.get("reason").and_then(Value::as_str).unwrap_or(""))
}

/// Whose reason a request asks about (`reason_of`; `kind` is the jobs').
fn reason_kind(request: &Value) -> Result<Kind> {
    Kind::parse(text(request, "reason_of")?)
        .ok_or_else(|| Error::input("reason_of is waive or not-required"))
}

fn reason_json(entry: Option<&reasons::Entry>) -> Value {
    entry.map_or(Value::Null, |e| json!({"text": e.text, "time": e.time}))
}

/// Whether any of rule `check`'s `targets` is not waived (the `w` key's
/// rule: a selection that is not waived whole is waived).
fn any_unwaived(shared: &Shared, check: usize, targets: &Selection, n: u64) -> Result<bool> {
    let mut mixed = false;
    let mut a = 0;
    let mut mask = vec![false; 1 << 16];
    while a < n && !mixed {
        let len = (n - a).min(1 << 16);
        targets.mask(a, &mut mask[..len as usize]);
        // (waived and not rejected, whatever the filter shows: a rejected
        // waive is waived again by the key)
        let st = shared.statuses.waived(check, a, len)?;
        mixed = (0..len as usize).any(|j| mask[j] && st[j] != 1);
        a += len;
    }
    Ok(mixed)
}

/// What the reason dialog starts from, for an action of `kind` on rule
/// `check`'s `targets`: `on` - which way the `w` key's toggle goes (asked
/// with `toggle`; an action that is not a toggle is on) -, `prefill` - the
/// one reason every target holds now, which the dialog then edits -,
/// `known` - the reasons given so far - and, of a waive, `rejected_by`: who
/// rejected the waive of any target and why, which this reviewer has not
/// answered yet (waived with a reason, those are waived again).
fn reason_read(
    shared: &Shared,
    check: usize,
    kind: Kind,
    targets: &Selection,
    n: u64,
    toggle: bool,
) -> Result<Value> {
    let on = !toggle || any_unwaived(shared, check, targets, n)?;
    let mut o = shared.opened.lock().unwrap();
    let r = review(&mut o)?;
    let runs = reasons::member_runs(targets, n);
    let rejected_by = if kind == Kind::Waive && on {
        r.rejected_by(check, &runs)?
    } else {
        Vec::new()
    };
    // (why a confirmer rejected is in the rejects log, not the reasons)
    let held = if kind == Kind::Reject { &r.rejects } else { &r.reasons };
    let prefill = if on && held.any(kind, check) {
        held.common(kind, check, &runs).unwrap_or("")
    } else {
        ""
    };
    let mut reply = json!({"on": on, "prefill": prefill, "known": held.texts(kind, KNOWN_REASONS)});
    // (said when there are any: a waive that answers no rejection is asked
    // as it always was)
    if !rejected_by.is_empty() {
        reply["rejected_by"] = json!(rejected_by);
    }
    Ok(reply)
}

fn rows(rows: &[ErrorRow]) -> Value {
    Value::from(
        rows.iter()
            .map(|r| json!([r.kind.to_string(), r.number, r.points]))
            .collect::<Vec<_>>(),
    )
}

fn notes(review: &mut Review) -> Value {
    json!({
        "notes": review.notes().iter().map(|n| json!({"text": n.text, "members": n.members})).collect::<Vec<_>>(),
        "notices": std::mem::take(&mut review.notices),
    })
}

/// Every rule's (errors of the class the lens shows, total): the waived
/// ones, with no filter of states on.
fn counts(review: &Review) -> Result<Value> {
    Ok(Value::from(
        (0..review.pack.checks.len())
            .map(|c| review.lens_counts(c).map(|(w, t)| json!([w, t])))
            .collect::<Result<Vec<_>>>()?,
    ))
}

/// A marker given as `[kind, [[x, y], ...]]` (um).
fn marker(m: &Value) -> Result<(char, Vec<[f64; 2]>)> {
    let bad = || Error::input("a marker is [kind, [[x, y], ...]]");
    let kind = m
        .get(0)
        .and_then(Value::as_str)
        .and_then(|k| k.chars().next())
        .ok_or_else(bad)?;
    let pts = m
        .get(1)
        .and_then(Value::as_array)
        .ok_or_else(bad)?
        .iter()
        .map(|p| match (p.get(0).and_then(Value::as_f64), p.get(1).and_then(Value::as_f64)) {
            (Some(x), Some(y)) => Ok([x, y]),
            _ => Err(bad()),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((kind, pts))
}

/// The detail pane's and the rulers' answer for one marker: its rulers, an
/// AREA rule's label, the CD the rule flagged with its ticks (measured,
/// bound, delta), the ratio, the candidates' ticks and a single edge's
/// marker length independently of whether its rule-aware CD is resolved.
fn cd_info(selector: &Selector, kind: char, pts: &[[f64; 2]]) -> Value {
    let rulers = selector.ruler_segments(kind, pts);
    let area = area_label(kind, pts, selector.constraints()).map(|(x, y, t)| json!([x, y, t]));
    let pick = selector.pick(kind, pts).map(|p| {
        let ticks = measurement_ticks(&p).ok();
        json!({
            "constraint": p.constraint,
            "cd": p.cd,
            "estimated": p.estimated,
            "reason": p.reason,
            "candidates": p.candidates,
            "candidate_ticks": p.candidates.iter().map(|c| ticks::value_ticks(*c).ok()).collect::<Vec<_>>(),
            "ticks": ticks.map(|(m, b, d)| json!([m, b, d])),
            "ratio": ticks.and_then(|(_, b, d)| ticks::percent_ticks(d, b)),
        })
    });
    json!({"rulers": rulers, "area": area, "pick": pick,
           "marker_length_ticks": marker_length_ticks(kind, pts)})
}

/// A grouping handed to the viewer: its directory and review counts.
struct GroupView {
    status: GroupStatus,
    check: usize,
}

/// A job of one kind running for a database.
struct Job {
    stop: Arc<AtomicUsize>,
    thread: Option<JoinHandle<()>>,
}
impl Job {
    fn cancel(&self) {
        self.stop.store(1, Ordering::Relaxed);
    }
}

/// What workers share with the main loop for one database.
struct Shared {
    opened: Mutex<Opened>,
    source: Arc<dyn Source>,
    statuses: Statuses,
    cache: Option<Cache>,
    groups: Mutex<BTreeMap<u64, GroupView>>,
    selections: Mutex<BTreeMap<u64, Arc<Selection>>>,
    tables: Mutex<Vec<Arc<Table>>>,
    /// rules whose statuses a worker changed wholesale (counts to redo)
    dirty: Mutex<BTreeSet<usize>>,
    /// a pack review's flags and team context (drc_team.rs)
    team: Option<team::TeamCtx>,
    snapshots: Mutex<BTreeMap<u64, Arc<floe_app_core::drc::team::snapshot::Snapshot>>>,
    plans: Mutex<BTreeMap<u64, Arc<Mutex<floe_app_core::drc::team::migrate::Plan>>>>,
    out: Arc<Out>,
    handle: u64,
    ids: Arc<AtomicUsize>,
    /// set when the session closes: a merge of the team's counts running
    /// apart stops
    closing: Arc<AtomicUsize>,
    /// that merge's thread, and the word that it shall look once more
    merging: Mutex<Option<std::thread::JoinHandle<()>>>,
    merge_again: Arc<AtomicUsize>,
    /// the team's waives as the list last read them (drc_team.rs
    /// `drc_team_waives`): pages are turned against one state, and a
    /// refresh - or this session's own confirmation - reads anew
    waives: Mutex<Option<floe_app_core::drc::team::waives::TeamWaives>>,
}

impl Shared {
    /// Bring the team's kept counts up to date now, apart (app-core
    /// drc::team::live): after a job that changed many errors, so that the
    /// command that is asked for the counts does not find the whole of it
    /// to count (user 2026-11-13: "사용자가 waive 할때마다 통계를
    /// 만들어두는게 좋을 것 같아"). The few errors of a click are left to
    /// whoever asks next: counting them is nothing. Whoever merges, the
    /// counts are the same.
    fn team_soon(&self) {
        let Some(t) = &self.team else { return };
        if !floe_app_core::drc::changes::enabled() {
            return; // FLOE_TEAM_KEEP=off: nothing is kept
        }
        let mut slot = self.merging.lock().unwrap_or_else(|p| p.into_inner());
        if slot.as_ref().is_some_and(|thread| !thread.is_finished()) {
            // one at a time: the one that runs looks once more when it ends
            self.merge_again.store(1, Ordering::Relaxed);
            return;
        }
        let (id, stop, again) = (t.id.clone(), Arc::clone(&self.closing), Arc::clone(&self.merge_again));
        *slot = std::thread::Builder::new()
            .name("drc-team-counts".into())
            .spawn(move || loop {
                again.store(0, Ordering::Relaxed);
                let merged = floe_app_core::drc::team::live::merge(&id.pack, &id, &stop);
                if merged.is_err() || again.load(Ordering::Relaxed) == 0 {
                    break;
                }
            })
            .ok();
    }
    fn event(&self, job: u64, kind: &str, state: &str, mut fields: Value) {
        fields["event"] = json!("drc");
        fields["handle"] = json!(self.handle);
        fields["job"] = json!(job);
        fields["kind"] = json!(kind);
        fields["state"] = json!(state);
        self.out.line(&fields);
    }
    fn next_id(&self) -> u64 {
        self.ids.fetch_add(1, Ordering::Relaxed) as u64 + 1
    }
}

struct Session {
    shared: Arc<Shared>,
    clusters: Option<ClusterFile>,
    jobs: HashMap<&'static str, Job>,
}

impl Session {
    fn status_source(opened: &Opened) -> &dyn clusters::Statuses {
        match opened {
            Opened::Pack(r) => &**r,
            Opened::Ascii(_) => &NoStatuses,
        }
    }

    /// Bring counts in step with statuses a worker rewrote.
    fn settle(&mut self, cancelled: &AtomicUsize) -> Result<()> {
        let dirty: Vec<usize> = std::mem::take(&mut *self.shared.dirty.lock().unwrap())
            .into_iter()
            .collect();
        if dirty.is_empty() {
            return Ok(());
        }
        if let Some(c) = &mut self.clusters {
            for check in &dirty {
                if let Some(list) = c.rules.get_mut(check) {
                    for cl in list {
                        cl.reset_status();
                    }
                }
            }
        }
        let mut groups = self.shared.groups.lock().unwrap();
        for g in groups.values_mut() {
            if dirty.contains(&g.check) {
                g.status.reset(&self.shared.statuses, cancelled)?;
            }
        }
        Ok(())
    }

    /// Statuses of rule `check`'s `errors` became `value` (in this loop).
    fn changed(&mut self, check: usize, errors: &[u64], value: u8) -> Result<()> {
        let opened = self.shared.opened.lock().unwrap();
        if let Some(c) = &mut self.clusters {
            c.status_changed(check, errors, Self::status_source(&opened))?;
        }
        drop(opened);
        for g in self.shared.groups.lock().unwrap().values_mut() {
            if g.check == check {
                for &e in errors {
                    g.status.changed(e, value == 1);
                }
            }
        }
        Ok(())
    }

    /// The membership a `scope` names (None: the whole rule).
    fn membership(&self, check: usize, scope: Option<&Value>) -> Result<Option<Arc<dyn Membership>>> {
        let Some(scope) = scope.filter(|s| !s.is_null()) else {
            return Ok(None);
        };
        let mut parts: Vec<Arc<dyn Membership>> = Vec::new();
        if scope.get("empty").and_then(Value::as_bool) == Some(true) {
            return Ok(Some(Arc::new(Selection::empty(self.shared.source.count(check)))));
        }
        if let Some(k) = scope.get("cluster").and_then(Value::as_u64) {
            let c = self
                .clusters
                .as_ref()
                .and_then(|c| c.rules.get(&check))
                .and_then(|l| l.get(k as usize))
                .ok_or_else(|| Error::input("no such cluster"))?;
            parts.push(c.runs.clone());
        }
        if let Some(g) = scope.get("group").and_then(Value::as_u64) {
            let r = scope
                .get("row")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::input("a group scope needs its row"))? as usize;
            let groups = self.shared.groups.lock().unwrap();
            let view = groups.get(&g).ok_or_else(|| Error::input("no such grouping"))?;
            if r >= view.status.groups.len() {
                return Err(Error::input("no such group row"));
            }
            parts.push(Arc::new(view.status.groups.members(r)));
        }
        if let Some(s) = scope.get("selection").and_then(Value::as_u64) {
            let sel = self
                .shared
                .selections
                .lock()
                .unwrap()
                .get(&s)
                .cloned()
                .ok_or_else(|| Error::input("no such selection"))?;
            parts.push(sel);
        }
        Ok(match parts.len() {
            0 => None,
            1 => parts.pop(),
            _ => Some(Arc::new(Scope(parts))),
        })
    }

    fn start(
        &mut self,
        kind: &'static str,
        work: impl FnOnce(Arc<Shared>, u64, Arc<AtomicUsize>) + Send + 'static,
    ) -> u64 {
        if let Some(old) = self.jobs.remove(kind) {
            old.cancel();
        }
        let shared = Arc::clone(&self.shared);
        let id = shared.next_id();
        let stop = Arc::new(AtomicUsize::new(0));
        let s2 = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name(format!("drc-{kind}"))
            .spawn(move || work(shared, id, s2))
            .ok();
        self.jobs.insert(kind, Job { stop, thread });
        id
    }

    fn close(mut self) {
        // a review edit in progress finishes; the rest stop
        for (kind, job) in self.jobs.drain() {
            if !matches!(kind, "review" | "team-write" | "migrate-write" | "confirm" | "returned") {
                job.cancel();
            }
            if let Some(t) = job.thread {
                let _ = t.join();
            }
        }
        if let Some(t) = &self.shared.team {
            t.wait_visits();
        }
        // (a merge of the team's counts running apart stops at its next
        // look; whoever asks next counts what it left)
        self.shared.closing.store(1, Ordering::Relaxed);
        let merging = self.shared.merging.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(thread) = merging {
            let _ = thread.join();
        }
    }
}

/// The DRC databases the service holds open.
pub struct Drcs {
    next: u64,
    sessions: BTreeMap<u64, Session>,
    out: Arc<Out>,
    ids: Arc<AtomicUsize>,
}

impl Drop for Drcs {
    fn drop(&mut self) {
        for (_, s) in std::mem::take(&mut self.sessions) {
            s.close();
        }
    }
}

fn job_error(shared: &Shared, id: u64, kind: &str, e: Error) {
    if e.kind == ErrorKind::Cancelled {
        shared.event(id, kind, "cancelled", json!({}));
    } else {
        shared.event(id, kind, "error", json!({"message": e.message}));
    }
}

impl Drcs {
    pub fn new(out: Arc<Out>) -> Self {
        Self {
            next: 0,
            sessions: BTreeMap::new(),
            out,
            ids: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn session(&mut self, request: &Value) -> Result<&mut Session> {
        let handle = number(request, "handle")?;
        self.sessions
            .get_mut(&handle)
            .ok_or_else(|| Error::input("no such DRC handle"))
    }

    pub fn handle(&mut self, op: &str, request: &Value, cancelled: &AtomicUsize) -> Result<Value> {
        match op {
            "drc_busy" => {
                let pack = floe_app_core::cache::pack_path(Path::new(text(request, "db")?))?;
                let key = floe_vfs::lock::key(floe_vfs::lock::Kind::Pack, &pack.to_string_lossy());
                return Ok(floe_vfs::lock::opening_refusal(&key)
                    .map_or(Value::Null, |b| Value::from(b.to_string())));
            }
            "drc_find" => {
                let pack = floe_app_core::cache::pack_path(Path::new(text(request, "db")?))?;
                return Ok(if pack.exists() { json!(pack) } else { Value::Null });
            }
            "drc_open" => return self.open(request, cancelled),
            "drc_cd_info" if request.get("markers").is_some() => {
                let selector = Selector::new(&constraints_of(request)?);
                let markers = request["markers"]
                    .as_array()
                    .ok_or_else(|| Error::input("markers must be a list"))?;
                return markers
                    .iter()
                    .map(|m| {
                        let (kind, pts) = marker(m)?;
                        Ok(cd_info(&selector, kind, &pts))
                    })
                    .collect::<Result<Vec<_>>>()
                    .map(Value::from);
            }
            "drc_parse_step" => {
                return ticks::parse_step(text(request, "text")?)
                    .map(Value::from)
                    .map_err(Error::input);
            }
            "svrf_rules" => {
                let (rules, warning) = desktop::rules_sidecar(Path::new(text(request, "path")?))?;
                return Ok(json!({"rules": rules, "warning": warning}));
            }
            "svrf_operands" => {
                return Ok(json!(floe_app_core::svrf::rhs_operands(text(request, "rhs")?)));
            }
            // where the rules sidecar of a database is looked for, in order
            // (the viewer's load reads the first that is a file): the one
            // `floe2 drc-prepare --svrf` noted for the results, beside the
            // database and the deck, then under the work directory
            "svrf_rules_candidates" => {
                let deck = request
                    .get("deck")
                    .and_then(Value::as_str)
                    .filter(|deck| !deck.is_empty())
                    .map(Path::new);
                let paths =
                    floe_app_core::svrf::rules_candidates(Path::new(text(request, "db")?), deck)?;
                return Ok(json!(paths
                    .iter()
                    .map(|path| path.to_string_lossy())
                    .collect::<Vec<_>>()));
            }
            "drc_close" => {
                if let Some(s) = self.sessions.remove(&number(request, "handle")?) {
                    s.close();
                }
                return Ok(Value::Null);
            }
            _ => (),
        }
        let session = self.session(request)?;
        // Cached CD bin counts depend only on geometry and constraints. Do not
        // scan pending review changes before this read-only background probe.
        if op != "drc_delta_cached" {
            session.settle(cancelled)?;
        }
        session.request(op, request, cancelled)
    }

    fn open(&mut self, request: &Value, cancelled: &AtomicUsize) -> Result<Value> {
        let path = PathBuf::from(text(request, "path")?);
        let reviewer = request.get("reviewer").and_then(Value::as_str);
        // whether this viewer was started to confirm the team's waives
        // (`floe2 view --confirmer`): the launcher's word, per open
        let confirmer = request.get("confirmer").and_then(Value::as_bool).unwrap_or(false);
        let (mut opened, mut notices) = match request.get("mode").and_then(Value::as_str) {
            Some("pack") => {
                let source = request.get("source").and_then(Value::as_str).map(Path::new);
                (
                    Opened::Pack(Box::new(Review::open(&path, source, reviewer, cancelled)?)),
                    Vec::new(),
                )
            }
            _ => Opened::load(&path, reviewer, cancelled)?,
        };
        if let Opened::Pack(r) = &mut opened {
            r.set_confirmer(confirmer);
        }
        let mut result = json!({
            "packed": matches!(opened, Opened::Pack(_)),
            "path": opened.path(),
            "cell": opened.cell(),
            "total": opened.total(),
            "precision": opened.precision(),
            "checks": opened.checks(),
        });
        let team = match &opened {
            Opened::Pack(r) => Some(team::TeamCtx::new(r)?),
            Opened::Ascii(_) => None,
        };
        if let Some(t) = &team {
            notices.extend(std::mem::take(&mut *t.flags.notices.lock().unwrap()));
        }
        let (source, statuses, cache): (Arc<dyn Source>, Statuses, Option<Cache>) = match &mut opened {
            Opened::Pack(r) => {
                result["counts"] = counts(r)?;
                result["reviewer"] = json!(r.reviewer);
                // how the id was found, and whether it was stated (--userid,
                // FLOE_REVIEWER) - the viewer shows both
                result["reviewer_source"] = json!(r.reviewer_source.text());
                result["reviewer_stated"] = json!(r.reviewer_source.stated());
                result["read_only"] = json!(r.read_only());
                result["waive_path"] = json!(r.waive_path);
                result["note_path"] = json!(r.note_path);
                result["reason_path"] = json!(r.reasons.path);
                result["confirm_path"] = json!(r.confirms.path);
                result["confirmer"] = json!(r.confirmer());
                let n = notes(r);
                result["notes"] = n["notes"].clone();
                notices.extend(
                    n["notices"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_string)),
                );
                (
                    Arc::new(PackSource(r.pack.blocks()?)),
                    Statuses::Review(r.status_reader()?),
                    Cache::of_pack(&r.pack.path).ok(),
                )
            }
            Opened::Ascii(a) => (Arc::new(AsciiSource(Arc::clone(a))), Statuses::None, None),
        };
        result["notices"] = json!(notices);
        result["analysis"] = json!(cache.as_ref().map(|c| &c.dir));
        self.next += 1;
        let handle = self.next;
        result["handle"] = json!(handle);
        self.sessions.insert(
            handle,
            Session {
                shared: Arc::new(Shared {
                    opened: Mutex::new(opened),
                    source,
                    statuses,
                    cache,
                    groups: Mutex::new(BTreeMap::new()),
                    selections: Mutex::new(BTreeMap::new()),
                    tables: Mutex::new(Vec::new()),
                    dirty: Mutex::new(BTreeSet::new()),
                    team,
                    snapshots: Mutex::new(BTreeMap::new()),
                    plans: Mutex::new(BTreeMap::new()),
                    out: Arc::clone(&self.out),
                    handle,
                    ids: Arc::clone(&self.ids),
                    closing: Arc::new(AtomicUsize::new(0)),
                    merging: Mutex::new(None),
                    merge_again: Arc::new(AtomicUsize::new(0)),
                    waives: Mutex::new(None),
                }),
                clusters: None,
                jobs: HashMap::new(),
            },
        );
        Ok(result)
    }
}

fn constraints_of(request: &Value) -> Result<Vec<Value>> {
    match request.get("constraints") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(c)) => Ok(c.clone()),
        Some(_) => Err(Error::input("constraints must be a list")),
    }
}

fn review(opened: &mut Opened) -> Result<&mut Review> {
    match opened {
        Opened::Pack(r) => Ok(r),
        Opened::Ascii(_) => Err(Error::new(
            ErrorKind::Unsupported,
            "an ASCII results file has no review state",
        )),
    }
}

impl Session {
    fn request(&mut self, op: &str, request: &Value, cancelled: &AtomicUsize) -> Result<Value> {
        let shared = Arc::clone(&self.shared);
        match op {
            "drc_errors" => {
                let (check, start, count) = (
                    number(request, "check")? as usize,
                    number(request, "start")?,
                    number(request, "count")?,
                );
                Ok(rows(&shared.opened.lock().unwrap().errors(
                    check,
                    start,
                    count.min(4096),
                    cancelled,
                )?))
            }
            "drc_cd" => {
                let (check, error) = (number(request, "check")? as usize, number(request, "error")?);
                Ok(json!(shared.opened.lock().unwrap().cd(check, error, cancelled)?))
            }
            "drc_cd_info" => {
                let selector = Selector::new(&constraints_of(request)?);
                let (check, error) = (number(request, "check")? as usize, number(request, "error")?);
                let row = shared
                    .opened
                    .lock()
                    .unwrap()
                    .errors(check, error, 1, cancelled)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| Error::input("DRC error index out of range"))?;
                Ok(cd_info(&selector, row.kind, &row.points))
            }
            "drc_status" => {
                let (check, start, count) = (
                    number(request, "check")? as usize,
                    number(request, "start")?,
                    number(request, "count")?,
                );
                // an error's state: 0 not waived, 1 waived, 2 waived and
                // rejected, 3 waived and confirmed (app-core desktop STATE_*)
                let mut o = shared.opened.lock().unwrap();
                let bytes = review(&mut o)?.states(check, start, count.min(1 << 22))?;
                Ok(Value::from(
                    bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                ))
            }
            "drc_lens" => {
                let lens = Lens::parse(text(request, "lens")?)
                    .ok_or_else(|| Error::input("lens is live, held, waived, confirmed or rejected"))?;
                let (moved, all) = {
                    let mut o = shared.opened.lock().unwrap();
                    let r = review(&mut o)?;
                    let moved = r.lens() != lens;
                    if moved {
                        r.set_lens(lens);
                    }
                    (moved, counts(r)?)
                };
                if moved {
                    // every count made through the status readers is of
                    // another class now
                    let checks = shared.source.check_count();
                    shared.dirty.lock().unwrap().extend(0..checks);
                    self.settle(cancelled)?;
                }
                Ok(json!({"lens": lens.name(), "counts": all}))
            }
            "drc_set_status" => {
                let check = number(request, "check")? as usize;
                let status = u8::try_from(number(request, "status")?)
                    .map_err(|_| Error::input("status is a byte"))?;
                let errors = numbers(request, "errors")?;
                let reason = reason_of(request)?;
                let (w, t, unkept, free, locked, by, rejected, rejected_by, again, simple) = {
                    let mut o = shared.opened.lock().unwrap();
                    let r = review(&mut o)?;
                    // a waive a confirmer has confirmed is not this
                    // session's to turn back, nor to give another reason
                    // (unless it is a confirmer's): left and said. An error
                    // under a rejection this reviewer has not answered is
                    // waived again when the action has a reason - what was
                    // is kept (app-core desktop::Review resubmit) - and
                    // left and said when it has none
                    let Left { locked, mut rejected } = r.set_status(check, &errors, status)?;
                    let mut again = 0;
                    if status == 1 && !reason.is_empty() && !rejected.is_empty() {
                        again = r.resubmit(check, &reasons::runs_of(&rejected))?;
                        rejected.clear();
                    }
                    let free: Vec<u64> = errors
                        .iter()
                        .copied()
                        .filter(|e| locked.binary_search(e).is_err() && rejected.binary_search(e).is_err())
                        .collect();
                    // why: one line for the action, every error's of it. The
                    // errors are waived whether it can be kept or not - a
                    // reason that could not be is said beside the counts
                    let unkept = if status == 1 && !reason.is_empty() {
                        r.reason_set(Kind::Waive, check, &reasons::runs_of(&free), &reason)
                            .err()
                            .map(|e| e.message)
                    } else {
                        None
                    };
                    // (waived again without a reason, nothing was withheld
                    // from a locked error: not said)
                    let locked = if status == 1 && reason.is_empty() { Vec::new() } else { locked };
                    // (asked only when something was left: a click that
                    // meets no confirmed waive lists no folder)
                    let by = if locked.is_empty() {
                        Vec::new()
                    } else {
                        r.confirmers_of(check, &reasons::runs_of(&locked))?
                    };
                    // (who rejected, and why as they said: asked only
                    // when something was left)
                    let rejected_by = if rejected.is_empty() {
                        Vec::new()
                    } else {
                        r.rejected_by(check, &reasons::runs_of(&rejected))?
                    };
                    // (the counts a scope keeps follow a status byte
                    // where that is all there is to an error's class)
                    let moved = r.take_reclassed();
                    let simple = moved.is_empty() && r.steps(check);
                    shared.dirty.lock().unwrap().extend(moved);
                    let (w, t) = r.lens_counts(check)?;
                    (w, t, unkept, free, locked, by, rejected, rejected_by, again, simple)
                };
                if simple {
                    self.changed(check, &free, status)?;
                } else {
                    shared.dirty.lock().unwrap().insert(check);
                }
                // (the team's waives as the list last read them are another
                // thing now: this reviewer's waive, or what they answered)
                *shared.waives.lock().unwrap() = None;
                let mut reply = json!({"counts": [w, t], "reason_error": unkept, "locked": locked, "locked_by": by,
                                       "rejected": rejected, "rejected_by": rejected_by});
                // (`resubmitted`: the rejected waives the reason waived
                // again, said when there are any)
                if again > 0 {
                    reply["resubmitted"] = json!(again);
                }
                Ok(reply)
            }
            "drc_reason_get" => {
                let (check, error) = (number(request, "check")? as usize, number(request, "error")?);
                // (the flags first: never asked under the review's lock)
                let excluded = shared.team.as_ref().is_some_and(|t| {
                    t.flags
                        .flags(check, &[error])
                        .is_ok_and(|f| f.first().is_some_and(|f| f & team::NOT_REQUIRED != 0))
                });
                let mut o = shared.opened.lock().unwrap();
                let r = review(&mut o)?;
                let excluded = if excluded {
                    r.reasons.of(Kind::NotRequired, check, error)
                } else {
                    None
                };
                let mut reply = json!({
                    "waive": reason_json(r.waive_reason(check, error)?),
                    "not_required": reason_json(excluded),
                });
                // (`history`: what is kept of this reviewer's waive of it -
                // the reasons it had, the rejections that were answered -,
                // said when there is any)
                let history = r.history(check, error)?;
                if !history.is_empty() {
                    reply["history"] = json!(history);
                }
                Ok(reply)
            }
            "drc_reason_read" => {
                let check = number(request, "check")? as usize;
                let n = shared.source.count(check);
                let targets = Selection::from_indices(n, numbers(request, "errors")?);
                let toggle = request.get("toggle").and_then(Value::as_bool).unwrap_or(false);
                reason_read(&shared, check, reason_kind(request)?, &targets, n, toggle)
            }
            "drc_status_page" => {
                let waived = waived_of(request).unwrap_or(false);
                let (check, start, limit) = (
                    number(request, "check")? as usize,
                    number(request, "start")?,
                    number(request, "limit")?,
                );
                let mut o = shared.opened.lock().unwrap();
                Ok(json!(review(&mut o)?.status_page(check, waived, start, limit.min(1 << 20))?))
            }
            "drc_status_rank" => {
                let waived = waived_of(request).unwrap_or(false);
                let (check, error) = (number(request, "check")? as usize, number(request, "error")?);
                let mut o = shared.opened.lock().unwrap();
                Ok(json!(review(&mut o)?.status_rank(check, waived, error)?))
            }
            "drc_query" => {
                let b = floats::<4>(request, "bbox")?;
                let cap = request.get("cap").and_then(Value::as_u64).unwrap_or(2000) as usize;
                let checks = match request.get("checks") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(
                        numbers(request, "checks")?
                            .into_iter()
                            .map(|c| c as usize)
                            .collect::<BTreeSet<_>>(),
                    ),
                };
                let members = match request.get("check").and_then(Value::as_u64) {
                    Some(c) => self.membership(c as usize, request.get("scope"))?,
                    None => None,
                };
                let waived = waived_of(request);
                let mut o = shared.opened.lock().unwrap();
                let r = review(&mut o)?;
                let hits = r.query_scoped(b, cap, checks.as_ref(), waived, members.as_deref(), cancelled)?;
                Ok(Value::from(
                    query_rows(r, hits)
                        .into_iter()
                        .map(|(c, e, row)| json!([c, e, row.kind.to_string(), row.number, row.points]))
                        .collect::<Vec<_>>(),
                ))
            }
            "drc_set_note" | "drc_clear_note" => {
                let gids = numbers(request, "gids")?;
                let mut o = shared.opened.lock().unwrap();
                let r = review(&mut o)?;
                if op == "drc_set_note" {
                    r.set_note(&gids, text(request, "text")?, cancelled)?;
                } else {
                    r.clear_note(&gids, cancelled)?;
                }
                Ok(notes(r))
            }
            "drc_note_export" => {
                let path = PathBuf::from(text(request, "path")?);
                review(&mut shared.opened.lock().unwrap())?.note_export(&path, cancelled)?;
                Ok(Value::Null)
            }
            "drc_note_import" => {
                let path = PathBuf::from(text(request, "path")?);
                let mut o = shared.opened.lock().unwrap();
                let r = review(&mut o)?;
                let count = r.note_import(&path, cancelled)?;
                let mut n = notes(r);
                n["count"] = json!(count);
                Ok(n)
            }
            "drc_waive_export" => {
                let path = PathBuf::from(text(request, "path")?);
                review(&mut shared.opened.lock().unwrap())?.waive_export(&path)?;
                Ok(Value::Null)
            }
            "drc_waive_import" => {
                let path = PathBuf::from(text(request, "path")?);
                let (waived, kept, all) = {
                    let mut o = shared.opened.lock().unwrap();
                    let r = review(&mut o)?;
                    let (waived, kept) = r.waive_import(&path)?;
                    (waived, kept, counts(r)?)
                };
                let checks = shared.source.check_count();
                shared.dirty.lock().unwrap().extend(0..checks);
                *shared.waives.lock().unwrap() = None;
                self.settle(cancelled)?;
                shared.team_soon();
                // (`locked`: waives the file would have turned back, kept
                // because a confirmer has confirmed them; `rejected`: errors
                // it would have waived, left because their waive is rejected)
                Ok(json!({"waived": waived, "counts": all, "locked": kept.locked, "rejected": kept.rejected}))
            }
            "drc_counts" => counts(review(&mut shared.opened.lock().unwrap())?),
            "drc_clusters_load" => self.clusters_load(request),
            "drc_cluster_counts" => {
                let check = number(request, "check")? as usize;
                let opened = shared.opened.lock().unwrap();
                let st = Self::status_source(&opened);
                let Some(c) = &mut self.clusters else {
                    return Ok(json!([]));
                };
                c.prepare_counts(check, st)?;
                let list = c.rules.get_mut(&check).map(Vec::as_mut_slice).unwrap_or(&mut []);
                Ok(Value::from(
                    list.iter_mut()
                        .map(|cl| cl.status_counts(st).map(|(w, t)| json!([w, t])))
                        .collect::<Result<Vec<_>>>()?,
                ))
            }
            "drc_scope_count" | "drc_scope_page" | "drc_scope_rank" | "drc_scope_contains" => {
                self.scope_request(op, request)
            }
            "drc_delta" => self.delta(request),
            "drc_delta_cached" => self.delta_cached(request),
            "drc_delta_rows" => {
                let g = number(request, "group")?;
                let (start, limit) = (number(request, "start")?, number(request, "limit")?.min(1000));
                let waived = waived_of(request);
                let groups = shared.groups.lock().unwrap();
                let view = groups.get(&g).ok_or_else(|| Error::input("no such grouping"))?;
                let gr = &view.status.groups;
                let end = (start + limit).min(gr.len() as u64);
                Ok(Value::from(
                    (start..end)
                        .map(|r| {
                            let r = r as usize;
                            let row = gr.row(r);
                            json!({"row": r, "name": gr.name(r), "label": gr.label(r),
                                   "count": view.status.count(r, waived),
                                   "total": row.total, "estimated": row.estimated,
                                   "key": [row.constraint, row.bin]})
                        })
                        .collect::<Vec<_>>(),
                ))
            }
            "drc_delta_find" => {
                let g = number(request, "group")?;
                let key = request
                    .get("key")
                    .and_then(Value::as_array)
                    .filter(|k| k.len() == 2)
                    .ok_or_else(|| Error::input("key is [constraint, bin]"))?;
                let (c, b) = (
                    key[0].as_i64().unwrap_or(-1) as i32,
                    key[1].as_i64().unwrap_or(0),
                );
                let groups = shared.groups.lock().unwrap();
                let view = groups.get(&g).ok_or_else(|| Error::input("no such grouping"))?;
                Ok(json!(view.status.groups.find(c, b)))
            }
            "drc_delta_drop" => {
                shared.groups.lock().unwrap().remove(&number(request, "group")?);
                Ok(Value::Null)
            }
            "drc_markers" => self.markers(request),
            "drc_select" => self.select(request),
            "drc_selection_page" => {
                let s = number(request, "selection")?;
                let sel = shared
                    .selections
                    .lock()
                    .unwrap()
                    .get(&s)
                    .cloned()
                    .ok_or_else(|| Error::input("no such selection"))?;
                Ok(json!(sel.page(number(request, "start")?, number(request, "limit")?.min(1 << 20))))
            }
            "drc_selection_drop" => {
                shared.selections.lock().unwrap().remove(&number(request, "selection")?);
                Ok(Value::Null)
            }
            "drc_review" => self.review_job(request),
            op if op == "drc_visit"
                || op.starts_with("drc_review_")
                || op.starts_with("drc_snapshot_")
                || op.starts_with("drc_team_")
                || op.starts_with("drc_migrate_")
                || op.starts_with("drc_plan_") =>
            {
                self.team_request(op, request)
            }
            "drc_inview" => self.inview(request),
            "drc_cancel" => {
                let kind = text(request, "kind")?;
                if let Some(job) = self.jobs.get(kind) {
                    job.cancel();
                }
                Ok(Value::Null)
            }
            other => Err(Error::new(
                ErrorKind::Unsupported,
                format!("unknown request: {other}"),
            )),
        }
    }

    fn clusters_load(&mut self, request: &Value) -> Result<Value> {
        let shared = Arc::clone(&self.shared);
        let opened = shared.opened.lock().unwrap();
        let path = match request.get("path").and_then(Value::as_str) {
            Some(p) => PathBuf::from(p),
            None => {
                // `<results.db>.clusters`, a pack's by its database's name
                let db = match &*opened {
                    Opened::Pack(r) => floe_app_core::cache::database_path(&r.pack.path)?,
                    Opened::Ascii(a) => a.path.clone(),
                };
                let beside = |db: PathBuf| {
                    let mut p = db.into_os_string();
                    p.push(".clusters");
                    PathBuf::from(p)
                };
                // under a work directory the pack mirrors the database's
                // folder; the clusters file is the user's, beside the
                // database itself
                let p = match beside(db.clone()) {
                    p if p.is_file() => p,
                    _ => match floe_app_core::cache::original_of(&db).map(beside) {
                        Some(o) if o.is_file() => o,
                        _ => return Ok(Value::Null),
                    },
                };
                p
            }
        };
        let checks = opened.checks();
        let names: Vec<String> = checks.iter().map(|c| c.name.clone()).collect();
        let counts: Vec<u64> = checks.iter().map(|c| c.count).collect();
        drop(opened);
        let file = clusters::load(&path, &names, &counts)?;
        let rules: serde_json::Map<String, Value> = file
            .rules
            .iter()
            .map(|(ci, list)| {
                (
                    ci.to_string(),
                    Value::from(
                        list.iter()
                            .map(|c| json!({"name": c.name, "total": c.total}))
                            .collect::<Vec<_>>(),
                    ),
                )
            })
            .collect();
        self.clusters = Some(file);
        Ok(json!({"path": path, "rules": rules}))
    }

    fn scope_request(&mut self, op: &str, request: &Value) -> Result<Value> {
        let check = number(request, "check")? as usize;
        let waived = waived_of(request);
        let scope = request.get("scope").cloned().unwrap_or(Value::Null);
        let shared = Arc::clone(&self.shared);
        let errors = || numbers(request, "errors");
        if scope.get("empty").and_then(Value::as_bool) == Some(true) {
            return Ok(match op {
                "drc_scope_count" => json!(0),
                "drc_scope_page" => json!([]),
                "drc_scope_rank" => Value::Null,
                _ => json!(errors()?.iter().map(|_| false).collect::<Vec<_>>()),
            });
        }
        if let Some(k) = scope.get("cluster").and_then(Value::as_u64) {
            let opened = shared.opened.lock().unwrap();
            let st = Self::status_source(&opened);
            let c = self
                .clusters
                .as_mut()
                .and_then(|c| c.rules.get_mut(&check))
                .and_then(|l| l.get_mut(k as usize))
                .ok_or_else(|| Error::input("no such cluster"))?;
            return Ok(match op {
                "drc_scope_count" => json!(c.count(waived, st)?),
                "drc_scope_page" => json!(c.page(
                    number(request, "start")?,
                    number(request, "limit")?.min(1 << 20),
                    waived,
                    st
                )?),
                "drc_scope_rank" => json!(c.rank(number(request, "error")?, waived, st)?),
                _ => json!(errors()?.into_iter().map(|e| c.contains(e)).collect::<Vec<_>>()),
            });
        }
        if let Some(g) = scope.get("group").and_then(Value::as_u64) {
            let r = scope
                .get("row")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::input("a group scope needs its row"))? as usize;
            let groups = shared.groups.lock().unwrap();
            let view = groups.get(&g).ok_or_else(|| Error::input("no such grouping"))?;
            if r >= view.status.groups.len() {
                return Err(Error::input("no such group row"));
            }
            let st = &view.status;
            return Ok(match op {
                "drc_scope_count" => json!(st.count(r, waived)),
                "drc_scope_page" => json!(st.page(
                    r,
                    number(request, "start")?,
                    number(request, "limit")?.min(1 << 20),
                    waived
                )),
                "drc_scope_rank" => json!(st.rank(r, number(request, "error")?, waived)),
                _ => json!(errors()?
                    .into_iter()
                    .map(|e| st.groups.row_of(e) == Some(r))
                    .collect::<Vec<_>>()),
            });
        }
        if let Some(s) = scope.get("selection").and_then(Value::as_u64) {
            let sel = shared
                .selections
                .lock()
                .unwrap()
                .get(&s)
                .cloned()
                .ok_or_else(|| Error::input("no such selection"))?;
            return Ok(match op {
                "drc_scope_count" => json!(sel.total()),
                "drc_scope_page" => json!(sel.page(number(request, "start")?, number(request, "limit")?.min(1 << 20))),
                "drc_scope_rank" => json!(sel.rank(number(request, "error")?)),
                _ => json!(errors()?.into_iter().map(|e| sel.contains(e)).collect::<Vec<_>>()),
            });
        }
        Err(Error::input("scope names a cluster, a group row or a selection"))
    }

    fn delta_cached(&mut self, request: &Value) -> Result<Value> {
        let basis = Basis::parse(request.get("basis").and_then(Value::as_str).unwrap_or("absolute"))?;
        let prepare = request.get("prepare").and_then(Value::as_bool).unwrap_or(false);
        let checks = request.get("checks").and_then(Value::as_array)
            .ok_or_else(|| Error::input("checks must be a list"))?;
        let mut wanted = Vec::with_capacity(checks.len());
        for entry in checks {
            let check = usize::try_from(number(entry, "check")?)
                .map_err(|_| Error::input("DRC check index out of range"))?;
            if check >= self.shared.source.check_count() {
                return Err(Error::input("DRC check index out of range"));
            }
            let constraints = constraints_of(entry)?;
            let step = match entry.get("step") {
                None | Some(Value::Null) => None,
                Some(v) => Some(v.as_i64().filter(|s| *s > 0).ok_or_else(|| {
                    Error::input("grouping step must be positive fixed-precision ticks")
                })?),
            };
            wanted.push((check, constraints, step));
        }
        let jobs = worker_count(None)?.clamp(1, 4);
        Ok(json!({"job": self.start("delta_cached", move |shared, id, stop| {
            let result = (|| -> Result<Value> {
                let total = wanted.len();
                let mut rows: Vec<Value> = wanted.iter().map(|(check, _, step)|
                    json!({"check":check,"count":null,"step":step})).collect();
                let mut missing = Vec::new();
                let mut processed = 0usize;
                let mut last = std::time::Instant::now();
                // Finish the entire cheap pass before the first cold rule so
                // a large early rule cannot delay already-prepared badges.
                for (i, (check, constraints, step)) in wanted.iter().enumerate() {
                    floe_app_core::check_cancelled(&stop)?;
                    let n = shared.source.count(*check);
                    let trivial = prepare && (n == 0 || !Selector::new(constraints).can_measure());
                    let summary = if trivial { None } else {
                        shared.cache.as_ref().map(|cache| Groups::cached_summary(
                            cache, *check, n, constraints, *step, basis,
                        ))
                    };
                    let count = if trivial { Some(0) } else { summary.as_ref().and_then(|s| s.count) };
                    let resolved = if trivial { Some(step.unwrap_or(1)) } else {
                        summary.as_ref().and_then(|s| s.step).or(*step)
                    };
                    rows[i] = json!({"check":check,"count":count,"step":resolved});
                    if count.is_some() || !prepare {
                        processed += 1;
                    } else {
                        missing.push((n, i));
                    }
                    if last.elapsed().as_millis() >= 500 {
                        shared.event(id, "delta_cached", "progress", json!({
                            "rows":rows,"processed":processed,"total":total,
                            "text":format!("Checking cached CD groups {} / {total}", i + 1),
                        }));
                        last = std::time::Instant::now();
                    }
                }
                floe_app_core::check_cancelled(&stop)?;
                shared.event(id, "delta_cached", "progress", json!({
                    "rows":rows,"processed":processed,"total":total,
                    "text":format!("CD groups ready {processed} / {total}"),
                }));
                missing.sort_unstable();
                let mut errors = 0usize;
                for (_, i) in missing {
                    floe_app_core::check_cancelled(&stop)?;
                    let (check, constraints, step) = &wanted[i];
                    let progress = |text: String| shared.event(id, "delta_cached", "progress", json!({
                        "rows":rows,"processed":processed,"total":total,
                        "text":format!("CD groups {processed} / {total}, rule {}: {text}", check + 1),
                    }));
                    match Groups::prepare_summary(&*shared.source, *check, constraints,
                        shared.cache.as_ref(), *step, basis, jobs, &stop, &progress) {
                        Ok(summary) => {
                            rows[i] = json!({"check":check,"count":summary.count,"step":summary.step});
                        }
                        Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
                        Err(e) => {
                            errors += 1;
                            rows[i]["error"] = json!(e.message);
                        }
                    }
                    processed += 1;
                    let text = if let Some(error) = rows[i].get("error").and_then(Value::as_str) {
                        format!("CD groups {processed} / {total}, rule {}: {error}", check + 1)
                    } else {
                        format!("CD groups ready {processed} / {total}")
                    };
                    shared.event(id, "delta_cached", "progress", json!({
                        "rows":rows,"processed":processed,"total":total,"text":text,
                    }));
                }
                floe_app_core::check_cancelled(&stop)?;
                Ok(json!({"rows":rows,"processed":processed,"total":total,"errors":errors}))
            })();
            match result {
                Ok(v) => shared.event(id, "delta_cached", "done", v),
                Err(e) => job_error(&shared, id, "delta_cached", e),
            }
        })}))
    }

    fn delta(&mut self, request: &Value) -> Result<Value> {
        let check = number(request, "check")? as usize;
        let constraints = constraints_of(request)?;
        let basis = Basis::parse(request.get("basis").and_then(Value::as_str).unwrap_or("absolute"))?;
        let step = match request.get("step") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_i64().filter(|s| *s > 0).ok_or_else(|| {
                Error::input("grouping step must be positive fixed-precision ticks")
            })?),
        };
        let cluster = match request.get("cluster").and_then(Value::as_u64) {
            Some(k) => {
                let c = self
                    .clusters
                    .as_ref()
                    .and_then(|c| c.rules.get(&check))
                    .and_then(|l| l.get(k as usize))
                    .ok_or_else(|| Error::input("no such cluster"))?;
                let key = cluster_key(c.runs.as_ref());
                Some((Arc::clone(&c.runs), key))
            }
            None => None,
        };
        let jobs = worker_count(None)?;
        Ok(json!({"job": self.start("delta", move |shared, id, stop| {
            let progress = |text: String| shared.event(id, "delta", "progress", json!({"text": text}));
            let result = (|| -> Result<Value> {
                // a rule's table is kept while its grouping changes
                let cached = shared
                    .tables
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|t| t.check == check && t.constraints == constraints)
                    .cloned();
                let table = match cached {
                    Some(t) => t,
                    None => {
                        let n = shared.source.count(check);
                        let t = Arc::new(Table::measure(
                            &*shared.source,
                            check,
                            &constraints,
                            shared.cache.as_ref(),
                            n >= CACHE_THRESHOLD,
                            jobs,
                            &stop,
                            &progress,
                        )?);
                        let mut tables = shared.tables.lock().unwrap();
                        tables.retain(|old| old.check != check);
                        tables.push(Arc::clone(&t));
                        if tables.len() > 2 {
                            tables.remove(0);
                        }
                        t
                    }
                };
                if !table.measurable {
                    return Ok(json!({"measurable": false}));
                }
                progress("Grouping CD deltas and unresolved edge lengths".into());
                let (members, key) = match &cluster {
                    Some((runs, key)) => (Some(&**runs as &dyn Membership), key.clone()),
                    None => (None, "all".to_string()),
                };
                let groups = Arc::new(Groups::build(Arc::clone(&table), step, basis, members, &key, &stop)?);
                progress("Reading current review statuses".into());
                let status = GroupStatus::new(Arc::clone(&groups), &shared.statuses, &stop)?;
                let gid = shared.next_id();
                let out = json!({"measurable": true, "group": gid, "groups": groups.len(),
                                 "cd_groups": groups.cd_group_count(),
                                 "geometry_groups": groups.geometry_group_count(),
                                 "has_cd": table.has_cd(),
                                 "step": groups.step, "auto": groups.auto, "total": groups.total,
                                 "estimated_total": groups.estimated_total(), "basis": basis.name()});
                let mut all = shared.groups.lock().unwrap();
                // one grouping per rule is enough to keep
                all.retain(|_, v| v.check != check);
                all.insert(gid, GroupView { status, check });
                Ok(out)
            })();
            match result {
                Ok(v) => shared.event(id, "delta", "done", v),
                Err(e) => job_error(&shared, id, "delta", e),
            }
        })}))
    }

    fn markers(&mut self, request: &Value) -> Result<Value> {
        let check = number(request, "check")? as usize;
        let bounds = floats::<4>(request, "bounds")?;
        let (width, height) = (number(request, "width")? as u32, number(request, "height")? as u32);
        let members = self.membership(check, request.get("scope"))?;
        let selected = match request.get("selection").and_then(Value::as_u64) {
            Some(s) => Some(
                self.shared
                    .selections
                    .lock()
                    .unwrap()
                    .get(&s)
                    .cloned()
                    .ok_or_else(|| Error::input("no such selection"))?,
            ),
            None => None,
        };
        let waived = waived_of(request);
        let folder = std::env::temp_dir();
        let jobs = worker_count(None)?;
        Ok(json!({"job": self.start("markers", move |shared, id, stop| {
            let file = |tag: &str| folder.join(format!("floe-drc-markers-{}-{id}-{tag}.raw", std::process::id()));
            let frames = AtomicUsize::new(0);
            let send = |pic: &points::Picture, state: &str| -> Result<()> {
                let path = file(&frames.fetch_add(1, Ordering::Relaxed).to_string());
                pic.write(&path)?;
                shared.event(id, "markers", state, json!({"path": path, "width": pic.width,
                    "height": pic.height, "visible": pic.visible, "occupied": pic.occupied,
                    "processed": pic.processed, "total": pic.total}));
                Ok(())
            };
            let q = points::Query {
                bounds_um: bounds,
                width,
                height,
                check,
                members: members.as_deref(),
                waived,
                selected: selected.as_deref().map(|s| s as &dyn Membership),
            };
            let result = points::query(
                &*shared.source,
                &shared.statuses,
                shared.cache.as_ref(),
                &q,
                jobs,
                &stop,
                &|pic| {
                    if stop.load(Ordering::Relaxed) == 0 {
                        let _ = send(&pic, "partial");
                    }
                },
            );
            match result.and_then(|pic| send(&pic, "done")) {
                Ok(()) => (),
                Err(e) => job_error(&shared, id, "markers", e),
            }
        })}))
    }

    fn select(&mut self, request: &Value) -> Result<Value> {
        let check = number(request, "check")? as usize;
        let bounds = match request.get("bounds") {
            None | Some(Value::Null) => None,
            Some(_) => Some(floats::<4>(request, "bounds")?),
        };
        let mut members = self.membership(check, request.get("scope"))?;
        let n = self.shared.source.count(check);
        if let Some(errors) = request.get("errors").filter(|e| !e.is_null()) {
            // explicit errors (grid edits): their selection is the scope
            let list: Vec<u64> = errors
                .as_array()
                .ok_or_else(|| Error::input("errors must be a list"))?
                .iter()
                .filter_map(Value::as_u64)
                .collect();
            let explicit: Arc<dyn Membership> = Arc::new(Selection::from_indices(n, list));
            members = Some(match members {
                Some(m) => Arc::new(Scope(vec![m, explicit])),
                None => explicit,
            });
        }
        let previous = match request.get("previous").and_then(Value::as_u64) {
            Some(s) => Some(
                self.shared
                    .selections
                    .lock()
                    .unwrap()
                    .get(&s)
                    .cloned()
                    .ok_or_else(|| Error::input("no such selection"))?,
            ),
            None => None,
        };
        let mode = request
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("replace")
            .to_string();
        let waived = waived_of(request);
        Ok(json!({"job": self.start("select", move |shared, id, stop| {
            let req = select::Request {
                check,
                bounds_um: bounds,
                members: members.as_deref(),
                waived,
                previous: previous.as_deref(),
                mode: &mode,
            };
            let result = select::select(&*shared.source, &shared.statuses, &req, &stop, &mut |done, total, found| {
                shared.event(id, "select", "progress", json!({"processed": done, "total": total, "count": found}));
            });
            match result {
                Ok(sel) => {
                    let sid = shared.next_id();
                    let count = sel.total();
                    shared.selections.lock().unwrap().insert(sid, Arc::new(sel));
                    shared.event(id, "select", "done", json!({"selection": sid, "count": count}));
                }
                Err(e) => job_error(&shared, id, "select", e),
            }
        })}))
    }

    fn review_job(&mut self, request: &Value) -> Result<Value> {
        let check = number(request, "check")? as usize;
        let n = self.shared.source.count(check);
        let targets: Arc<Selection> = match request.get("selection").and_then(Value::as_u64) {
            Some(s) => self
                .shared
                .selections
                .lock()
                .unwrap()
                .get(&s)
                .cloned()
                .ok_or_else(|| Error::input("no such selection"))?,
            None => Arc::new(Selection::from_indices(n, numbers(request, "errors")?)),
        };
        let op = text(request, "action")?.to_string();
        if !matches!(
            op.as_str(),
            "waive" | "note" | "note-clear" | "note-read" | "not-required" | "reason-read"
        ) {
            return Err(Error::input(
                "review action is waive, not-required, reason-read, note, note-clear or note-read",
            ));
        }
        let on = request.get("on").and_then(Value::as_bool);
        let note = request.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        // why the errors are waived or set aside: one line for the action
        // (refused here, before the job, when it cannot be kept as given)
        let reason = reason_of(request)?;
        let kind = match op.as_str() {
            "reason-read" => Some(reason_kind(request)?),
            _ => None,
        };
        let toggle = request.get("toggle").and_then(Value::as_bool).unwrap_or(false);
        if self.jobs.get("review").is_some_and(|j| j.thread.as_ref().is_some_and(|t| !t.is_finished())) {
            return Err(Error::new(ErrorKind::Busy, "review update is still running"));
        }
        Ok(json!({"job": self.start("review", move |shared, id, stop| {
            let result = (|| -> Result<Value> {
                let start_of = {
                    let o = shared.opened.lock().unwrap();
                    o.checks().get(check).map(|c| c.start).unwrap_or(0)
                };
                match op.as_str() {
                    "reason-read" => {
                        reason_read(&shared, check, kind.expect("asked above"), &targets, n, toggle)
                    }
                    "waive" => {
                        // a mixed selection waives all (the `w` key's rule)
                        let on = match on {
                            Some(on) => on,
                            None => any_unwaived(&shared, check, &targets, n)?,
                        };
                        let value = u8::from(on);
                        let mut changed = 0;
                        let mut a = 0;
                        let mut last = std::time::Instant::now();
                        let mut settled = std::time::Instant::now();
                        // the targets whose waive a confirmer has confirmed
                        // and the ones whose waive a confirmer has rejected:
                        // left as they are (app-core desktop::Review)
                        let mut left: Left<reasons::Run> = Left::default();
                        while a < n {
                            let len = (n - a).min(1 << 16);
                            {
                                let mut o = shared.opened.lock().unwrap();
                                let r = review(&mut o)?;
                                changed += r.set_status_range(check, &*targets, value, a, len, &mut left)?;
                                // (what a long job changed so far is flushed
                                // and said in the change log about once a
                                // second, not only at its end)
                                if settled.elapsed().as_millis() >= 1000 {
                                    r.sync_waives()?;
                                    settled = std::time::Instant::now();
                                }
                            }
                            a += len;
                            if last.elapsed().as_millis() >= 500 {
                                last = std::time::Instant::now();
                                shared.event(id, "review", "progress", json!({"done": a, "total": n}));
                            }
                            if stop.load(Ordering::Relaxed) != 0 {
                                break;
                            }
                        }
                        let mut unkept = Value::Null;
                        let (mut by, mut rejected_by) = (Vec::new(), Vec::new());
                        let Left { locked: mut kept, mut rejected } = left;
                        let mut again = 0;
                        {
                            let mut o = shared.opened.lock().unwrap();
                            let r = review(&mut o)?;
                            // the targets under a rejection this reviewer
                            // has not answered are waived again when the
                            // action has a reason - what was is kept
                            // (app-core desktop::Review resubmit) - and
                            // left, and said, when it has none
                            if on && !reason.is_empty() && !rejected.is_empty() {
                                again = r.resubmit(check, &rejected)?;
                                changed += again;
                                rejected.clear();
                            }
                            // why: one line for the action whatever it took -
                            // the members it handled, as runs (a job that was
                            // stopped handled the rule up to `a`), without
                            // the locked ones: they keep the reason they have
                            if on && !reason.is_empty() {
                                let runs = reasons::minus(&reasons::member_runs(&*targets, a), &kept);
                                let runs = reasons::minus(&runs, &rejected);
                                if let Err(e) = r.reason_set(Kind::Waive, check, &runs, &reason) {
                                    unkept = json!(e.message);
                                }
                            }
                            // (waived again without a reason, nothing was
                            // withheld from a locked error: not said)
                            if on && reason.is_empty() {
                                kept.clear();
                            }
                            if !kept.is_empty() {
                                by = r.confirmers_of(check, &kept)?;
                            }
                            if !rejected.is_empty() {
                                rejected_by = r.rejected_by(check, &rejected)?;
                            }
                            // one flush for the job (app-core drc::desktop
                            // sync_waives): the bytes are on the disk when
                            // it says it is done
                            r.sync_waives()?;
                        }
                        let locked: u64 = kept.iter().map(|r| r.1 - r.0).sum();
                        let rejected: u64 = rejected.iter().map(|r| r.1 - r.0).sum();
                        shared.dirty.lock().unwrap().insert(check);
                        *shared.waives.lock().unwrap() = None;
                        let counts = {
                            let mut o = shared.opened.lock().unwrap();
                            match &mut *o {
                                Opened::Pack(r) => {
                                    let moved = r.take_reclassed();
                                    shared.dirty.lock().unwrap().extend(moved);
                                    json!(r.lens_counts(check)?)
                                }
                                Opened::Ascii(_) => Value::Null,
                            }
                        };
                        let mut reply = json!({"on": on, "changed": changed, "counts": counts, "reason_error": unkept,
                                               "locked": locked, "locked_by": by,
                                               "rejected": rejected, "rejected_by": rejected_by});
                        if again > 0 {
                            reply["resubmitted"] = json!(again);
                        }
                        Ok(reply)
                    }
                    "not-required" => {
                        let t = shared.team.as_ref().ok_or_else(|| {
                            Error::new(ErrorKind::Unsupported, "review flags need a packed index: floe-index drc <db>")
                        })?;
                        let on = on.unwrap_or(true);
                        let holds = {
                            let mut o = shared.opened.lock().unwrap();
                            review(&mut o)?.reasons.any(Kind::NotRequired, check)
                        };
                        // the members as runs: what a reason is said of, and
                        // what one is taken back from (asked only when the
                        // rule holds any)
                        let runs = if holds || (on && !reason.is_empty()) {
                            reasons::member_runs(&*targets, n)
                        } else {
                            Vec::new()
                        };
                        if on && reason.is_empty() && holds {
                            // set aside without a reason: an error that is not
                            // set aside now must not come to hold one that a
                            // run which ended early left on it
                            let held = {
                                let mut o = shared.opened.lock().unwrap();
                                review(&mut o)?.reasons.held(Kind::NotRequired, check, &runs)
                            };
                            let mut left = Vec::new();
                            for (a, b) in held {
                                let mut e = a;
                                while e < b {
                                    let errors: Vec<u64> = (e..(e + 4096).min(b)).collect();
                                    let flags = t.flags.flags(check, &errors)?;
                                    left.extend(
                                        errors.iter().zip(flags).filter(|(_, f)| f & team::NOT_REQUIRED == 0).map(|(e, _)| *e),
                                    );
                                    e += errors.len() as u64;
                                }
                            }
                            let mut o = shared.opened.lock().unwrap();
                            review(&mut o)?.reason_drop(Kind::NotRequired, check, &reasons::runs_of(&left));
                        }
                        let mut last = std::time::Instant::now();
                        let changed = t.flags.set_members(
                            check,
                            Some(&*targets),
                            on,
                            &stop,
                            &mut |done, total, _changed| {
                                if last.elapsed().as_millis() >= 500 {
                                    last = std::time::Instant::now();
                                    shared.event(id, "review", "progress", json!({"done": done, "total": total}));
                                }
                            },
                        )?;
                        let mut unkept = Value::Null;
                        {
                            let mut o = shared.opened.lock().unwrap();
                            let r = review(&mut o)?;
                            if !on {
                                // back in the review: the reason goes with it
                                r.reason_drop(Kind::NotRequired, check, &runs);
                            } else if !reason.is_empty() {
                                if let Err(e) = r.reason_set(Kind::NotRequired, check, &runs, &reason) {
                                    unkept = json!(e.message);
                                }
                            }
                            // (the reason lines taken back reach the disk)
                            r.sync_waives()?;
                        }
                        Ok(json!({"on": on, "changed": changed, "reason_error": unkept}))
                    }
                    "note-read" => {
                        let o = shared.opened.lock().unwrap();
                        let Opened::Pack(r) = &*o else {
                            return Err(Error::new(ErrorKind::Unsupported, "an ASCII results file has no notes"));
                        };
                        let ids = targets.page(0, targets.total());
                        let first = ids.first().and_then(|&e| r.note(start_of + e)).map(str::to_string);
                        let same = ids.iter().all(|&e| r.note(start_of + e) == first.as_deref());
                        let prefill = if same { first.unwrap_or_default() } else { String::new() };
                        Ok(json!({"prefill": prefill}))
                    }
                    _ => {
                        let gids: Vec<u64> = targets.page(0, targets.total()).into_iter().map(|e| start_of + e).collect();
                        let mut o = shared.opened.lock().unwrap();
                        let r = review(&mut o)?;
                        if op == "note" {
                            r.set_note(&gids, &note, &stop)?;
                        } else {
                            r.clear_note(&gids, &stop)?;
                        }
                        Ok(notes(r))
                    }
                }
            })();
            // a job that changed many errors: the team's counts are brought
            // up to date apart, now (whatever it changed before an error too)
            if matches!(op.as_str(), "waive" | "not-required") && targets.total() >= TEAM_SOON {
                shared.team_soon();
            }
            match result {
                Ok(mut v) => {
                    v["action"] = json!(op);
                    v["count"] = json!(targets.total());
                    shared.event(id, "review", "done", v)
                }
                Err(e) => job_error(&shared, id, "review", e),
            }
        })}))
    }

    fn inview(&mut self, request: &Value) -> Result<Value> {
        let check = number(request, "check")? as usize;
        let bbox = floats::<4>(request, "bbox")?;
        let cap = request.get("cap").and_then(Value::as_u64).unwrap_or(1000) as usize;
        let members = self.membership(check, request.get("scope"))?;
        let waived = waived_of(request);
        Ok(json!({"job": self.start("inview", move |shared, id, stop| {
            let result = (|| -> Result<Value> {
                let mut o = shared.opened.lock().unwrap();
                let r = review(&mut o)?;
                let only = BTreeSet::from([check]);
                let hits = r.query_scoped(bbox, cap, Some(&only), waived, members.as_deref(), &stop)?;
                let rows: Vec<Value> = query_rows(r, hits)
                    .into_iter()
                    .map(|(c, e, row)| json!([c, e, row.kind.to_string(), row.number, row.points]))
                    .collect();
                Ok(json!({"rows": rows}))
            })();
            match result {
                Ok(v) => shared.event(id, "inview", "done", v),
                Err(e) => job_error(&shared, id, "inview", e),
            }
        })}))
    }
}

/// A cluster's runs as a cache key (its members alone decide its groups).
fn cluster_key(runs: &floe_app_core::drc::analysis::scope::Runs) -> String {
    let mut text = String::new();
    for (a, b) in runs.starts.iter().zip(&runs.stops) {
        text.push_str(&format!("{a}-{b},"));
    }
    floe_app_core::drc::analysis::store::digest(&json!(text))
}
