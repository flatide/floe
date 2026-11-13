//! The service's part of the review flags and the team's shared review
//! (feature/jobdeck-drc 2026-10-09..11; app-core drc::team):
//!
//!   drc_visit {handle, check, error}            queued, written in order by
//!       the session's visit thread; `{"kind": "visit", "state": "done" |
//!       "error", "check", "error"}` events
//!   drc_review_flags {handle, check, errors}    VISITED=1 | NOT_REQUIRED=2 each
//!   drc_review_counts {handle, check?}          total, visited, not_required,
//!       visited_not_required, pending
//!   drc_review_path {handle}                    the flags file
//!   drc_review_stats {handle, path}             job "review": the statistics CSV
//!   drc_snapshot_info / _counts {check?} / _page {check, kind, start, limit} /
//!       _get {check, errors} / _contains {check, kind, errors} / _drop
//!       {handle, snapshot}
//!   drc_team_load {handle, paths}                job "team": peers' snapshots,
//!       working review files or waive files
//!   drc_team_aggregate {handle, sources, include_own?}       job "team"
//!   drc_team_import {handle, source}             job "team": import conflicts
//!   drc_team_export {handle, path}               job "team-write"
//!   drc_team_apply {handle, source, check?, errors?, force}  job "team-write"
//!   drc_team_csv {handle, path, sources}         job "team-write"
//!   drc_team_conflicts {handle, aggregate, check, scope?, waived?}
//!                                                job "conflicts": a selection
//!   drc_migrate_prepare {handle, previous, state?}           job "migrate"
//!   drc_migrate_apply {handle, plan}             job "migrate-write"
//!   drc_plan_counts {check?} / _summary / _page {check, kind, start, limit} /
//!       _get {check, errors} / _contains {check, kind, errors} / _drop
//!       {handle, plan}
//!   drc_team_waives {handle, check, show, start, limit, refresh?}
//!                                                job "team-waives": a page of
//!       the team's waives (show `waived` | `unconfirmed` | `confirmed`) -
//!       who waived each error, why, who confirmed it - with the rule's and
//!       the result's counts (app-core drc::team::waives; 0.12.466)
//!   drc_team_waive_get {handle, check, error}    job "team-row": one error's
//!       row, as the reviewers' files say now (a job: finding the files
//!       lists folders, and an error's detail must not wait for that)
//!   drc_team_confirm {handle, check, errors?, on} job "confirm": this
//!       reviewer confirms the team's waives of a rule (`errors`: these;
//!       none: every one), or takes their confirmation back - a session
//!       opened as a confirmer's (`drc_open {confirmer: true}`)
use super::{job_error, number, numbers, text, waived_of, Session, Shared};
use floe_app_core::drc::analysis::scope::Selection;
use floe_app_core::drc::desktop::StatusReader;
use floe_app_core::drc::matcher;
pub(super) use floe_app_core::drc::team::flags::NOT_REQUIRED;
use floe_app_core::drc::team::{
    flags::FlagStore,
    migrate::{self, Plan},
    snapshot::{self, Snapshot},
    waives::{self, TeamWaives},
    Identity, Own, CHUNK,
};
use floe_app_core::{Error, ErrorKind, Result};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

/// A pack review's team context.
pub(super) struct TeamCtx {
    pub id: Identity,
    pub flags: FlagStore,
    pub reader: StatusReader,
    visits: Mutex<(VecDeque<(usize, u64)>, bool)>,
    idle: Condvar,
}

impl TeamCtx {
    pub fn new(review: &floe_app_core::drc::desktop::Review) -> Result<Self> {
        let id = Identity::of(review)?;
        let flags = FlagStore::new(&review.pack.path, id.sizes.clone(), id.fingerprint.clone(), &id.reviewer)?;
        // a flags file of the result before (the DRC ran again): moved aside
        // as the waive sidecar was, so this review starts (0.12.441)
        flags.retire_other_result()?;
        // what the flags' writes change is said in the review's change log,
        // beside its statuses' (the team's kept counts; 0.12.465)
        flags.attach_changes(std::sync::Arc::clone(&review.changes));
        Ok(Self {
            id,
            flags,
            reader: review.status_reader()?,
            visits: Mutex::new((VecDeque::new(), false)),
            idle: Condvar::new(),
        })
    }
    /// Wait for the visits accepted so far (an export or application
    /// sees them).
    pub(super) fn wait_visits(&self) {
        let mut g = self.visits.lock().unwrap();
        while g.1 || !g.0.is_empty() {
            g = self.idle.wait(g).unwrap();
        }
    }
}

fn ctx(shared: &Shared) -> Result<&TeamCtx> {
    shared.team.as_ref().ok_or_else(|| {
        Error::new(
            ErrorKind::Unsupported,
            "review flags need a packed index: floe-index drc <db>",
        )
    })
}

fn own<'a>(shared: &'a Shared, c: &'a TeamCtx) -> Own<'a> {
    Own {
        id: &c.id,
        opened: &shared.opened,
        statuses: &c.reader,
        flags: &c.flags,
    }
}

fn info(id: u64, s: &Snapshot) -> Value {
    json!({
        "snapshot": id,
        "reviewer": s.reviewer,
        "path": s.path,
        "original_path": s.original_path,
        "snapshot_id": s.snapshot_id,
        "exported_ns": s.exported_ns,
        "visited_known": s.visited_known,
        "waive_known": s.waive_known,
        "not_required_known": s.not_required_known,
        "aggregate": s.aggregate(),
        "source_path": s.metadata.get("source_path"),
        "source_waive_path": s.metadata.get("source_waive_path"),
    })
}

fn progress_event(shared: &Arc<Shared>, id: u64, kind: &'static str) -> impl FnMut(u64, u64, &str) + Send {
    let shared = Arc::clone(shared);
    move |done, total, phase: &str| {
        shared.event(id, kind, "progress", json!({"done": done, "total": total, "phase": phase}));
    }
}

impl Shared {
    fn snapshot(&self, request: &Value, key: &str) -> Result<Arc<Snapshot>> {
        let id = number(request, key)?;
        self.snapshots
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::input("no such review snapshot"))
    }
    fn keep(&self, s: Snapshot) -> u64 {
        let id = self.next_id();
        self.snapshots.lock().unwrap().insert(id, Arc::new(s));
        id
    }
    fn plan(&self, request: &Value) -> Result<Arc<Mutex<Plan>>> {
        let id = number(request, "plan")?;
        self.plans
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::input("no such migration plan"))
    }
}

/// Record visits in order on the session's own thread.
fn visit(shared: &Arc<Shared>, check: usize, error: u64) -> Result<()> {
    let c = ctx(shared)?;
    if error >= *c.id.sizes.get(check).ok_or_else(|| Error::input("DRC check index out of range"))? {
        return Err(Error::input("DRC error index out of range"));
    }
    let mut g = c.visits.lock().unwrap();
    g.0.push_back((check, error));
    if g.1 {
        return Ok(());
    }
    g.1 = true;
    drop(g);
    let shared = Arc::clone(shared);
    std::thread::Builder::new()
        .name("drc-visits".into())
        .spawn(move || {
            let c = shared.team.as_ref().unwrap();
            loop {
                let job = {
                    let mut g = c.visits.lock().unwrap();
                    match g.0.pop_front() {
                        Some(j) => j,
                        None => {
                            g.1 = false;
                            c.idle.notify_all();
                            return;
                        }
                    }
                };
                match c.flags.mark_visited(job.0, job.1) {
                    Ok(changed) => shared.event(
                        0,
                        "visit",
                        "done",
                        json!({"check": job.0, "error": job.1, "changed": changed}),
                    ),
                    Err(e) => shared.event(
                        0,
                        "visit",
                        "error",
                        json!({"check": job.0, "error": job.1, "message": e.message}),
                    ),
                }
            }
        })?;
    Ok(())
}

fn errors_of(request: &Value) -> Result<Vec<u64>> {
    numbers(request, "errors")
}

/// The errors of rule `check`.
fn rule_size(c: &TeamCtx, check: usize) -> Result<u64> {
    c.id.sizes
        .get(check)
        .copied()
        .ok_or_else(|| Error::input("DRC check index out of range"))
}

/// The statistics CSV of one's own review (whole database and per rule).
fn stats_csv(c: &TeamCtx, names: &[String], path: &Path) -> Result<()> {
    let mut rows = Vec::new();
    let mut total = [0u64; 5];
    for (ci, name) in names.iter().enumerate() {
        let k = c.flags.counts(Some(ci))?;
        let v = [k.total, k.visited, k.not_required, k.visited_not_required, k.pending];
        for (t, x) in total.iter_mut().zip(v) {
            *t += x;
        }
        rows.push(format!("rule,{},{},{}", ci + 1, csv_field(name), join(&v)));
    }
    let mut text = String::from(
        "scope,rule_index,rule,total,visited,not_required,visited_not_required,unvisited_targets\r\n",
    );
    text.push_str(&format!("database,,,{}\r\n", join(&total)));
    for r in rows {
        text.push_str(&r);
        text.push_str("\r\n");
    }
    write_atomic(path, &text, ".floe-review-")
}

fn join(v: &[u64]) -> String {
    v.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
}

/// A CSV field as Python's csv writer quotes it (minimal quoting).
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn write_atomic(path: &Path, text: &str, prefix: &str) -> Result<()> {
    use std::io::Write;
    let path = floe_app_core::cache::absolute(path)?;
    let dir = path.parent().ok_or_else(|| Error::input("output path has no folder"))?;
    let tmp = dir.join(format!("{prefix}{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// The aggregate's statistics CSV (database and rules).
fn team_csv(names: &[String], agg: &Snapshot, path: &Path) -> Result<()> {
    let fields = |k: snapshot::Counts| {
        join(&[
            k.total,
            k.visited,
            k.waived,
            k.not_required,
            k.pending,
            k.conflicts,
            k.reviewers.unwrap_or(0),
        ])
    };
    let mut text = String::from(
        "scope,rule_index,rule,total,visited,waived,not_required,pending,conflicts,reviewers\r\n",
    );
    text.push_str(&format!("database,,,{}\r\n", fields(agg.counts(None)?)));
    for (ci, name) in names.iter().enumerate() {
        text.push_str(&format!("rule,{},{},{}\r\n", ci + 1, csv_field(name), fields(agg.counts(Some(ci))?)));
    }
    write_atomic(path, &text, ".reviewers-")
}

impl Session {
    fn rule_names(&self) -> Vec<String> {
        self.shared.opened.lock().unwrap().checks().iter().map(|c| c.name.clone()).collect()
    }

    fn running(&self, kind: &str) -> bool {
        self.jobs
            .get(kind)
            .is_some_and(|j| j.thread.as_ref().is_some_and(|t| !t.is_finished()))
    }

    pub(super) fn team_request(&mut self, op: &str, request: &Value) -> Result<Value> {
        let shared = Arc::clone(&self.shared);
        match op {
            "drc_visit" => {
                visit(&shared, number(request, "check")? as usize, number(request, "error")?)?;
                Ok(Value::Null)
            }
            "drc_review_flags" => {
                let c = ctx(&shared)?;
                Ok(json!(c.flags.flags(number(request, "check")? as usize, &errors_of(request)?)?))
            }
            "drc_review_counts" => {
                let c = ctx(&shared)?;
                let check = request.get("check").and_then(Value::as_u64).map(|c| c as usize);
                Ok(json!(c.flags.counts(check)?))
            }
            "drc_review_path" => {
                let c = ctx(&shared)?;
                let notices: Vec<String> = std::mem::take(&mut *c.flags.notices.lock().unwrap());
                Ok(json!({"path": c.flags.path(), "notices": notices}))
            }
            "drc_review_stats" => {
                ctx(&shared)?;
                let path = PathBuf::from(text(request, "path")?);
                let names = self.rule_names();
                if self.running("review") {
                    return Err(Error::new(ErrorKind::Busy, "review update is still running"));
                }
                Ok(json!({"job": self.start("review", move |shared, id, _stop| {
                    let c = shared.team.as_ref().unwrap();
                    c.wait_visits();
                    match stats_csv(c, &names, &path) {
                        Ok(()) => shared.event(id, "review", "done", json!({"action": "stats", "path": path})),
                        Err(e) => job_error(&shared, id, "review", e),
                    }
                })}))
            }
            "drc_snapshot_info" => {
                let s = shared.snapshot(request, "snapshot")?;
                Ok(info(number(request, "snapshot")?, &s))
            }
            "drc_snapshot_counts" => {
                let s = shared.snapshot(request, "snapshot")?;
                let check = request.get("check").and_then(Value::as_u64).map(|c| c as usize);
                Ok(json!(s.counts(check)?))
            }
            "drc_snapshot_page" => {
                let s = shared.snapshot(request, "snapshot")?;
                let m = s.members(number(request, "check")? as usize, text(request, "kind")?)?;
                let ids = m.page(number(request, "start")?, number(request, "limit")?.min(1 << 20))?;
                Ok(json!({"count": m.total, "ids": ids}))
            }
            "drc_snapshot_get" => {
                let s = shared.snapshot(request, "snapshot")?;
                let check = number(request, "check")? as usize;
                let rows = errors_of(request)?
                    .into_iter()
                    .map(|e| {
                        s.get(check, e)
                            .map(|v| json!([v.visited, v.waived, v.not_required, v.conflict]))
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(json!(rows))
            }
            "drc_snapshot_contains" => {
                let s = shared.snapshot(request, "snapshot")?;
                let m = s.members(number(request, "check")? as usize, text(request, "kind")?)?;
                Ok(json!(m.contains_many(&errors_of(request)?)?))
            }
            "drc_snapshot_drop" => {
                let id = number(request, "snapshot")?;
                if let Some(s) = shared.snapshots.lock().unwrap().remove(&id) {
                    s.close();
                }
                Ok(Value::Null)
            }
            "drc_team_load" => {
                ctx(&shared)?;
                let paths: Vec<PathBuf> = request
                    .get("paths")
                    .and_then(Value::as_array)
                    .ok_or_else(|| Error::input("paths must be a list"))?
                    .iter()
                    .map(|p| p.as_str().map(PathBuf::from).ok_or_else(|| Error::input("paths must be text")))
                    .collect::<Result<_>>()?;
                Ok(json!({"job": self.start("team", move |shared, id, stop| {
                    let c = shared.team.as_ref().unwrap();
                    let mut progress = progress_event(&shared, id, "team");
                    let (mut loaded, mut errors) = (Vec::new(), Vec::new());
                    for p in &paths {
                        if stop.load(std::sync::atomic::Ordering::Relaxed) != 0 {
                            break;
                        }
                        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        match snapshot::open_review(&c.id, p, None, None, &stop, &mut progress) {
                            Ok(s) => {
                                let sid = shared.next_id();
                                loaded.push(info(sid, &s));
                                shared.snapshots.lock().unwrap().insert(sid, Arc::new(s));
                            }
                            Err(e) => errors.push(format!("{name}: {}", e.message)),
                        }
                    }
                    shared.event(id, "team", "done", json!({"action": "load", "loaded": loaded, "errors": errors}));
                })}))
            }
            "drc_team_aggregate" | "drc_team_csv" => {
                ctx(&shared)?;
                let sources = request
                    .get("sources")
                    .and_then(Value::as_array)
                    .ok_or_else(|| Error::input("sources must be a list"))?
                    .iter()
                    .map(|v| {
                        let sid = v.as_u64().ok_or_else(|| Error::input("a source is a snapshot"))?;
                        shared.snapshots.lock().unwrap().get(&sid).cloned().ok_or_else(|| Error::input("no such review snapshot"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let include_own = request.get("include_own").and_then(Value::as_bool).unwrap_or(true);
                let csv = (op == "drc_team_csv").then(|| text(request, "path").map(PathBuf::from)).transpose()?;
                let names = self.rule_names();
                let kind = if csv.is_some() { "team-write" } else { "team" };
                Ok(json!({"job": self.start(kind, move |shared, id, stop| {
                    let c = shared.team.as_ref().unwrap();
                    c.wait_visits();
                    let mut progress = progress_event(&shared, id, kind);
                    let result = snapshot::aggregate(&own(&shared, c), &sources, include_own, None, &stop, &mut progress)
                        .and_then(|agg| match &csv {
                            Some(path) => team_csv(&names, &agg, path).map(|_| json!({"action": "csv", "path": path})),
                            None => {
                                let counts = agg.counts(None)?;
                                let sid = shared.keep(agg);
                                let snap = shared.snapshots.lock().unwrap()[&sid].clone();
                                Ok(json!({"action": "aggregate", "snapshot": sid, "info": info(sid, &snap), "counts": counts}))
                            }
                        });
                    match result {
                        Ok(v) => shared.event(id, kind, "done", v),
                        Err(e) => job_error(&shared, id, kind, e),
                    }
                })}))
            }
            "drc_team_import" => {
                ctx(&shared)?;
                let source = shared.snapshot(request, "source")?;
                Ok(json!({"job": self.start("team", move |shared, id, stop| {
                    let c = shared.team.as_ref().unwrap();
                    c.wait_visits();
                    let mut progress = progress_event(&shared, id, "team");
                    match snapshot::import_conflicts(&own(&shared, c), &source, None, &stop, &mut progress) {
                        Ok(s) => {
                            let counts = s.counts(None);
                            let sid = shared.keep(s);
                            shared.event(id, "team", "done", json!({"action": "import", "snapshot": sid, "counts": counts.ok()}))
                        }
                        Err(e) => job_error(&shared, id, "team", e),
                    }
                })}))
            }
            "drc_team_export" => {
                ctx(&shared)?;
                let path = PathBuf::from(text(request, "path")?);
                Ok(json!({"job": self.start("team-write", move |shared, id, stop| {
                    let c = shared.team.as_ref().unwrap();
                    c.wait_visits();
                    let mut progress = progress_event(&shared, id, "team-write");
                    match snapshot::export(&own(&shared, c), &path, None, &stop, &mut progress) {
                        Ok(s) => shared.event(id, "team-write", "done", json!({"action": "export", "path": s.path, "counts": s.counts(None).ok()})),
                        Err(e) => job_error(&shared, id, "team-write", e),
                    }
                })}))
            }
            "drc_team_apply" => {
                ctx(&shared)?;
                let source = shared.snapshot(request, "source")?;
                let check = request.get("check").and_then(Value::as_u64).map(|c| c as usize);
                let members = match (check, request.get("errors").filter(|v| !v.is_null())) {
                    (Some(ci), Some(_)) => Some(Selection::from_indices(source.rule(ci)?, errors_of(request)?)),
                    (None, Some(_)) => return Err(Error::input("a rule is required for selected review application")),
                    _ => None,
                };
                let force = request.get("force").and_then(Value::as_bool).unwrap_or(false);
                Ok(json!({"job": self.start("team-write", move |shared, id, stop| {
                    let c = shared.team.as_ref().unwrap();
                    c.wait_visits();
                    let mut progress = progress_event(&shared, id, "team-write");
                    let mut changed = |rule: usize| {
                        shared.dirty.lock().unwrap().insert(rule);
                    };
                    let result = snapshot::apply(
                        &own(&shared, c),
                        &source,
                        check,
                        members.as_ref().map(|m| m as &dyn floe_app_core::drc::analysis::scope::Membership),
                        force,
                        &stop,
                        &mut progress,
                        &mut changed,
                    );
                    let dirty: Vec<usize> = shared.dirty.lock().unwrap().iter().copied().collect();
                    // (a peer's decisions applied: the team's counts follow)
                    shared.team_soon();
                    match result {
                        Ok(a) => shared.event(id, "team-write", "done", json!({"action": "apply", "result": a, "rules": dirty})),
                        Err(e) => {
                            shared.event(id, "team-write", "error", json!({"message": e.message, "rules": dirty, "cancelled": e.kind == ErrorKind::Cancelled}));
                        }
                    }
                })}))
            }
            "drc_team_conflicts" => {
                ctx(&shared)?;
                let aggregate = shared.snapshot(request, "aggregate")?;
                let check = number(request, "check")? as usize;
                let members = self.membership(check, request.get("scope"))?;
                let waived = waived_of(request);
                Ok(json!({"job": self.start("conflicts", move |shared, id, stop| {
                    let c = shared.team.as_ref().unwrap();
                    let mut progress = progress_event(&shared, id, "conflicts");
                    match snapshot::conflict_selection(&own(&shared, c), &aggregate, check, members.as_deref(), waived, &stop, &mut progress) {
                        Ok(sel) => {
                            let total = sel.total();
                            let sid = shared.next_id();
                            shared.selections.lock().unwrap().insert(sid, Arc::new(sel));
                            shared.event(id, "conflicts", "done", json!({"selection": sid, "total": total, "check": check}))
                        }
                        Err(e) => job_error(&shared, id, "conflicts", e),
                    }
                })}))
            }
            "drc_migrate_prepare" => {
                ctx(&shared)?;
                let previous = PathBuf::from(text(request, "previous")?);
                let state = request.get("state").and_then(Value::as_str).filter(|s| !s.is_empty()).map(PathBuf::from);
                Ok(json!({"job": self.start("migrate", move |shared, id, stop| {
                    let c = shared.team.as_ref().unwrap();
                    c.wait_visits();
                    let mut progress = progress_event(&shared, id, "migrate");
                    let options = matcher::Options {
                        jobs: floe_app_core::drc::analysis::source::worker_count(None).unwrap_or(4),
                        ..matcher::Options::default()
                    };
                    match migrate::prepare(&own(&shared, c), &previous, state.as_deref(), None, options, &stop, &mut progress) {
                        Ok(plan) => {
                            let summary = plan.summary.clone();
                            let pid = shared.next_id();
                            shared.plans.lock().unwrap().insert(pid, Arc::new(Mutex::new(plan)));
                            shared.event(id, "migrate", "done", json!({"plan": pid, "summary": summary}))
                        }
                        Err(e) => job_error(&shared, id, "migrate", e),
                    }
                })}))
            }
            "drc_migrate_apply" => {
                ctx(&shared)?;
                let plan = shared.plan(request)?;
                Ok(json!({"job": self.start("migrate-write", move |shared, id, stop| {
                    let c = shared.team.as_ref().unwrap();
                    c.wait_visits();
                    let mut progress = progress_event(&shared, id, "migrate-write");
                    let mut changed = |rule: usize| {
                        shared.dirty.lock().unwrap().insert(rule);
                    };
                    let mut p = plan.lock().unwrap();
                    let result = migrate::apply(&own(&shared, c), &mut p, &stop, &mut progress, &mut changed);
                    let dirty: Vec<usize> = shared.dirty.lock().unwrap().iter().copied().collect();
                    shared.team_soon();
                    match result {
                        Ok(a) => shared.event(id, "migrate-write", "done", json!({"result": a, "summary": p.summary, "rules": dirty})),
                        Err(e) => shared.event(id, "migrate-write", "error", json!({"message": e.message, "rules": dirty, "cancelled": e.kind == ErrorKind::Cancelled})),
                    }
                })}))
            }
            "drc_plan_counts" => {
                let plan = shared.plan(request)?;
                let p = plan.lock().map_err(|_| Error::input("migration plan is busy"))?;
                let check = request.get("check").and_then(Value::as_u64).map(|c| c as usize);
                Ok(json!(p.rule_counts(check)?))
            }
            "drc_plan_summary" => {
                let plan = shared.plan(request)?;
                let p = plan.lock().unwrap();
                Ok(Value::Object(p.summary.clone()))
            }
            "drc_plan_page" => {
                let plan = shared.plan(request)?;
                let p = plan.lock().unwrap();
                let m = p.members(number(request, "check")? as usize, text(request, "kind")?)?;
                let ids = m.page(number(request, "start")?, number(request, "limit")?.min(1 << 20))?;
                Ok(json!({"count": m.total, "ids": ids}))
            }
            "drc_plan_contains" => {
                let plan = shared.plan(request)?;
                let p = plan.lock().unwrap();
                let m = p.members(number(request, "check")? as usize, text(request, "kind")?)?;
                Ok(json!(m.contains_many(&errors_of(request)?)?))
            }
            "drc_plan_get" => {
                let plan = shared.plan(request)?;
                let p = plan.lock().unwrap();
                let check = number(request, "check")? as usize;
                let rows = errors_of(request)?
                    .into_iter()
                    .map(|e| {
                        p.get(check, e).map(|v| match v {
                            None => Value::Null,
                            Some(v) => json!([v.visited, v.waived, v.not_required]),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(json!(rows))
            }
            "drc_plan_drop" => {
                let id = number(request, "plan")?;
                shared.plans.lock().unwrap().remove(&id);
                Ok(Value::Null)
            }
            "drc_team_waives" => {
                let c = ctx(&shared)?;
                let check = number(request, "check")? as usize;
                rule_size(c, check)?;
                // (`show`, not `kind`: a job's kind is the events' word)
                let kind = waives::Kind::parse(text(request, "show")?)?;
                let (start, limit) = (number(request, "start")?, number(request, "limit")?.min(1 << 16));
                let refresh = request.get("refresh").and_then(Value::as_bool).unwrap_or(false);
                Ok(json!({"job": self.start("team-waives", move |shared, id, stop| {
                    let c = shared.team.as_ref().unwrap();
                    let result = (|| -> Result<Value> {
                        let mut slot = shared.waives.lock().unwrap();
                        if refresh || slot.is_none() {
                            *slot = Some(TeamWaives::open(&c.id.pack, &c.id)?);
                        }
                        let view = slot.as_mut().expect("opened");
                        let (count, ids) = view.page(check, kind, start, limit, &stop)?;
                        let rows = view.rows(check, &ids)?;
                        let (rule, all) = (view.counts(Some(check), &stop)?, view.counts(None, &stop)?);
                        let rules: Vec<Value> = view
                            .rules(&stop)?
                            .into_iter()
                            .map(|(ci, k)| json!({"check": ci, "waived": k.waived, "confirmed": k.confirmed, "unconfirmed": k.unconfirmed}))
                            .collect();
                        let skipped: Vec<Value> = view
                            .skipped
                            .iter()
                            .map(|(who, file, why)| json!({"reviewer": who, "file": file, "reason": why}))
                            .collect();
                        Ok(json!({
                            "check": check, "start": start, "count": count, "rows": rows,
                            "rule": rule, "all": all, "rules": rules,
                            "reviewers": view.reviewers(), "confirmers": view.confirmers(),
                            "skipped": skipped,
                        }))
                    })();
                    match result {
                        Ok(v) => shared.event(id, "team-waives", "done", v),
                        Err(e) => job_error(&shared, id, "team-waives", e),
                    }
                })}))
            }
            "drc_team_waive_get" => {
                let c = ctx(&shared)?;
                let check = number(request, "check")? as usize;
                let error = number(request, "error")?;
                if error >= rule_size(c, check)? {
                    return Err(Error::input("DRC error index out of range"));
                }
                // one error, as every reviewer's files say now - found and
                // read by the job: listing the folders the files are in (the
                // temp folder among them) took 8 ms of a temp folder of
                // 19,000 names, and the viewer asks at every selection
                Ok(json!({"job": self.start("team-row", move |shared, id, _stop| {
                    let c = shared.team.as_ref().unwrap();
                    let result = (|| -> Result<Value> {
                        let mut view = TeamWaives::open(&c.id.pack, &c.id)?;
                        let row = view.rows(check, &[error])?.remove(0);
                        let confirmer = super::review(&mut shared.opened.lock().unwrap())?.confirmer();
                        Ok(json!({
                            "check": check, "error": error,
                            "waived_by": row.waived_by, "confirmed_by": row.confirmed_by,
                            "reasons": row.reasons, "confirmer": confirmer, "reviewer": c.id.reviewer,
                        }))
                    })();
                    match result {
                        Ok(v) => shared.event(id, "team-row", "done", v),
                        Err(e) => job_error(&shared, id, "team-row", e),
                    }
                })}))
            }
            "drc_team_confirm" => {
                let c = ctx(&shared)?;
                let check = number(request, "check")? as usize;
                let size = rule_size(c, check)?;
                let on = request.get("on").and_then(Value::as_bool).unwrap_or(true);
                // (refused before a job is made: the role, a job still running)
                if !super::review(&mut shared.opened.lock().unwrap())?.confirmer() {
                    return Err(Error::new(
                        ErrorKind::Unsupported,
                        "confirming waives is the confirmer's: start the viewer with --confirmer",
                    ));
                }
                if self.running("confirm") {
                    return Err(Error::new(ErrorKind::Busy, "a confirmation is still running"));
                }
                let named = match request.get("errors").filter(|v| !v.is_null()) {
                    Some(_) => {
                        let errors = errors_of(request)?;
                        if errors.iter().any(|&e| e >= size) {
                            return Err(Error::input("DRC error index out of range"));
                        }
                        Some(errors)
                    }
                    None => None,
                };
                Ok(json!({"job": self.start("confirm", move |shared, id, stop| {
                    let c = shared.team.as_ref().unwrap();
                    let (mut changed, mut skipped) = (0u64, 0u64);
                    let result = (|| -> Result<()> {
                        // the team's waives as their files stand now
                        let view = TeamWaives::open(&c.id.pack, &c.id)?;
                        let chunks: Vec<u64> = match &named {
                            Some(errors) => {
                                let mut ks: Vec<u64> = errors.iter().map(|e| e / CHUNK).collect();
                                ks.sort_unstable();
                                ks.dedup();
                                ks
                            }
                            None => (0..size.div_ceil(CHUNK)).collect(),
                        };
                        let members = named.as_ref().map(|errors| Selection::from_indices(size, errors.clone()));
                        let members = members.as_ref().map(|m| m as &dyn floe_app_core::drc::analysis::scope::Membership);
                        let (mut said, mut settled) = (std::time::Instant::now(), std::time::Instant::now());
                        for (done, &k) in chunks.iter().enumerate() {
                            // (what a stopped job wrote stays, and is flushed below)
                            floe_app_core::check_cancelled(&stop)?;
                            let mut o = shared.opened.lock().unwrap();
                            let r = super::review(&mut o)?;
                            let (c1, s1) = waives::confirm_chunk(r, &view, check, k, members, on)?;
                            changed += c1;
                            skipped += s1;
                            if settled.elapsed().as_millis() >= 1000 {
                                r.sync_waives()?;
                                settled = std::time::Instant::now();
                            }
                            drop(o);
                            if said.elapsed().as_millis() >= 500 {
                                said = std::time::Instant::now();
                                shared.event(id, "confirm", "progress", json!({"done": done + 1, "total": chunks.len()}));
                            }
                        }
                        Ok(())
                    })();
                    // whatever became of the job, what it wrote reaches the
                    // disk and the change log, and the list reads anew
                    let flushed = super::review(&mut shared.opened.lock().unwrap()).and_then(|r| r.sync_waives());
                    *shared.waives.lock().unwrap() = None;
                    if changed >= super::TEAM_SOON {
                        shared.team_soon();
                    }
                    match result.and(flushed) {
                        Ok(()) => shared.event(id, "confirm", "done", json!({"check": check, "on": on, "changed": changed, "skipped": skipped})),
                        Err(e) => shared.event(id, "confirm", "error", json!({"message": e.message, "changed": changed, "cancelled": e.kind == ErrorKind::Cancelled})),
                    }
                })}))
            }
            other => Err(Error::new(ErrorKind::Unsupported, format!("unknown request: {other}"))),
        }
    }
}
