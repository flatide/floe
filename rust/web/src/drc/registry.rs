//! Owner-only, explicit DRC replacement. Registry lock linearizes retirement
//! with HTTP panel/selection/prepared-edit commits; native I/O stays off-reactor.
use super::{Failure, Registration, Service};
use crate::{
    operations::{Admission, Ledger},
    transport::Attachment,
};
use floe_app_core::{
    drc::build::{self, Phase},
    native::Indexer,
    ErrorKind, Result,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BuildRequest {
    pub seq: String,
    pub drc_id: String,
    pub revision: String,
    pub view_id: String,
    pub approve: bool,
    pub force: bool,
    pub jobs: u16,
}
struct Work {
    seq: u64,
    reader: Arc<Service>,
    options: build::Options,
    stop: Arc<AtomicUsize>,
}
struct State {
    current: Option<Arc<Service>>,
    ledger: Ledger,
    pending: Option<Work>,
    stop: Option<Arc<AtomicUsize>>,
    closed: bool,
}
struct Inner {
    registration: Registration,
    indexer: Option<Indexer>,
    state: Mutex<State>,
    wake: Condvar,
}
pub struct Registry {
    inner: Arc<Inner>,
    thread: Mutex<Option<JoinHandle<()>>>,
    notes: Mutex<Option<Arc<super::review::Service>>>,
    waives: Mutex<Option<Arc<super::review::Service>>>,
}
impl Registry {
    pub fn read_only(reader: Arc<Service>) -> Arc<Self> {
        Self::new(reader, None).expect("read-only registry has no thread")
    }
    pub fn with_builds(reader: Arc<Service>, indexer: Indexer) -> Result<Arc<Self>> {
        Self::new(reader, Some(indexer))
    }
    fn new(reader: Arc<Service>, indexer: Option<Indexer>) -> Result<Arc<Self>> {
        let inner = Arc::new(Inner {
            registration: reader.registration.clone(),
            indexer,
            state: Mutex::new(State {
                current: Some(reader),
                ledger: Ledger::default(),
                pending: None,
                stop: None,
                closed: false,
            }),
            wake: Condvar::new(),
        });
        let thread = if inner.indexer.is_some() {
            let task = Arc::clone(&inner);
            Some(
                thread::Builder::new()
                    .name("floe-drc-owner".into())
                    .spawn(move || run(task))?,
            )
        } else {
            None
        };
        Ok(Arc::new(Self {
            inner,
            thread: Mutex::new(thread),
            notes: Mutex::new(None),
            waives: Mutex::new(None),
        }))
    }
    pub(super) fn notes(&self) -> Option<Arc<super::review::Service>> {
        self.notes.lock().unwrap().clone()
    }
    pub(super) fn review(
        &self,
        kind: floe_app_core::drc::review::store::Kind,
    ) -> Option<Arc<super::review::Service>> {
        match kind {
            floe_app_core::drc::review::store::Kind::Notes => self.notes(),
            floe_app_core::drc::review::store::Kind::Waives => self.waives.lock().unwrap().clone(),
        }
    }
    fn reviews(&self) -> impl Iterator<Item = Arc<super::review::Service>> {
        self.notes()
            .into_iter()
            .chain(self.waives.lock().unwrap().clone())
    }
    pub(crate) fn enable_notes(
        &self,
        reviewer: &str,
        sources: Arc<floe_app_core::registered::SourceSet>,
        files: &[std::path::PathBuf],
        trees: &[std::path::PathBuf],
        edit_waives: bool,
        editable: bool,
    ) -> Result<()> {
        use floe_app_core::drc::review::store;
        let mut notes = self.notes.lock().unwrap();
        if notes.is_some()
            || edit_waives && !editable
            || files.len() > 120
            || trees.len() > 128
            || sources.snapshot().is_empty()
        {
            return Err(floe_app_core::Error::input(
                "invalid note review registration",
            ));
        }
        let r = &self.inner.registration;
        store::paths(&r.path, reviewer, store::Kind::Notes)?;
        let waive_target = store::paths(&r.path, reviewer, store::Kind::Waives)?;
        if edit_waives && r.waives.as_ref().is_some_and(|p| *p != waive_target[0]) {
            return Err(floe_app_core::Error::input(
                "waive write target must match the registered read sidecar",
            ));
        }
        let note_files = files
            .iter()
            .cloned()
            .chain(std::iter::once(r.path.clone()))
            .chain(r.waives.clone())
            .chain(r.rules.clone())
            .collect();
        *notes = Some(super::review::Service::start(super::review::Config {
            kind: store::Kind::Notes,
            editable,
            reader_id: if editable {
                None
            } else {
                Some(
                    self.inner
                        .state
                        .lock()
                        .unwrap()
                        .current
                        .as_ref()
                        .ok_or_else(|| floe_app_core::Error::input("DRC reader unavailable"))?
                        .id
                        .clone(),
                )
            },
            reviewer: reviewer.into(),
            files: note_files,
            trees: trees.to_vec(),
            sources: sources.clone(),
        })?);
        if edit_waives {
            *self.waives.lock().unwrap() =
                Some(super::review::Service::start(super::review::Config {
                    kind: store::Kind::Waives,
                    editable: true,
                    reader_id: None,
                    reviewer: reviewer.into(),
                    files: files
                        .iter()
                        .cloned()
                        .chain(std::iter::once(r.path.clone()))
                        .chain(r.rules.clone())
                        .collect(),
                    trees: trees.to_vec(),
                    sources,
                })?);
        }
        Ok(())
    }
    pub(crate) fn maintain(&self) {
        for n in self.reviews() {
            n.maintain();
        }
    }
    pub(crate) fn notes_enabled(&self) -> bool {
        self.notes().is_some()
    }
    pub(crate) fn waives_enabled(&self) -> bool {
        self.waives.lock().unwrap().is_some()
    }
    pub(crate) fn source_id(&self) -> &str {
        &self.inner.registration.source_id
    }
    pub(crate) fn protected_paths(
        &self,
    ) -> Result<(Vec<std::path::PathBuf>, Vec<std::path::PathBuf>)> {
        let r = &self.inner.registration;
        let mut files: Vec<_> = std::iter::once(r.path.clone())
            .chain(r.waives.clone())
            .chain(r.rules.clone())
            .collect();
        // Protect both an existing ICE tree and the future build target. A
        // read-only registration must not gain writes through another feature.
        let mut trees = vec![r.path.clone()];
        if self.inner.indexer.is_some() {
            let mut target = r.path.as_os_str().to_owned();
            target.push(".ice");
            trees.push(target.into());
        }
        if let Some(notes) = self.notes() {
            files.extend(notes.protected_targets(&r.path)?);
            if self.inner.indexer.is_some() {
                files.extend(notes.protected_targets(&build::output_path(&r.path)?)?);
            }
        }
        Ok((files, trees))
    }
    pub(crate) fn current(&self, id: &str) -> Option<Arc<Service>> {
        let s = self.inner.state.lock().unwrap();
        s.current
            .as_ref()
            .filter(|d| !s.closed && d.id == id)
            .cloned()
    }
    pub(crate) fn with_current<T>(
        &self,
        reader: &Service,
        f: impl FnOnce() -> std::result::Result<T, Failure>,
    ) -> std::result::Result<T, Failure> {
        let s = self.inner.state.lock().unwrap();
        if s.closed || !s.current.as_ref().is_some_and(|d| d.id == reader.id) {
            return Err("drc_context_changed");
        }
        // Lock order: registry -> panel/prepared -> view controller. No caller
        // may keep these guards across await or reacquire the registry in f.
        f()
    }
    #[cfg(test)]
    pub(crate) fn is_current(&self, reader: &Service) -> bool {
        self.with_current(reader, || Ok(())).is_ok()
    }
    pub(crate) fn with_revision<T>(
        &self,
        reader: &Service,
        expected: &str,
        f: impl FnOnce() -> std::result::Result<T, Failure>,
    ) -> std::result::Result<T, Failure> {
        // Registry -> read revision -> panel/prepared/controller. No I/O and
        // no revision lookup from inside f (the same non-reentrant lock).
        self.with_current(reader, || reader.fence(expected)?.with_current(f))
    }
    pub(crate) fn is_revision(&self, reader: &Service, expected: &str) -> bool {
        self.with_revision(reader, expected, || Ok(())).is_ok()
    }
    pub(super) fn with_panel<T>(
        &self,
        reader: &Service,
        expected: &str,
        view: &Attachment,
        f: impl FnOnce(&mut super::panel::Panel) -> std::result::Result<T, Failure>,
    ) -> std::result::Result<T, Failure> {
        self.with_revision(reader, expected, || {
            let mut panel = view.drc_panel.lock().unwrap();
            panel.bind_revision(expected);
            f(&mut panel)
        })
    }
    fn allowed(&self, s: &State) -> bool {
        self.inner.indexer.is_some()
            && self.inner.registration.waives.is_none()
            && !s.current.as_ref().is_some_and(|d| {
                d.registration.path == self.inner.registration.path
                    && d.catalog()["metadata"]["format"] == "ice"
            })
    }
    pub fn catalog(&self) -> Value {
        let s = self.inner.state.lock().unwrap();
        json!({"drc":s.current.as_ref().map(|d|d.catalog()),
            "notes":self.notes().map(|n|n.status()),
            "waives":self.review(floe_app_core::drc::review::store::Kind::Waives).map(|n|n.status()),
            "build":{"available":!s.closed && self.allowed(&s),"source_id":self.source_id(),"jobs_min":1,"jobs_max":16,"jobs_default":4,"operations":s.ledger.snapshot()}})
    }
    fn submit(
        &self,
        req: BuildRequest,
        view: Option<&Arc<Attachment>>,
    ) -> std::result::Result<Value, Failure> {
        let seq = crate::view::counter(&req.seq)?;
        let signature = format!("{req:?}");
        let options = build::Options {
            jobs: usize::from(req.jobs),
            force: req.force,
        };
        options.validate().map_err(|_| "invalid_drc_request")?;
        if !req.approve {
            return Err("drc_build_approval_required");
        }
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("drc_closed");
        }
        if let Some(replay) = s.ledger.replay(seq, &signature)? {
            return Ok(replay);
        }
        if !self.allowed(&s) {
            return Err("drc_build_unavailable");
        }
        if s.ledger.active().is_some() {
            return Err("busy");
        }
        let view = view.ok_or("drc_context_changed")?;
        let reader = s
            .current
            .as_ref()
            .filter(|d| d.id == req.drc_id)
            .ok_or("drc_context_changed")?
            .clone();
        if view.id != req.view_id
            || view.source_id != reader.source_id
            || view.controller.is_finished()
        {
            return Err("drc_context_changed");
        }
        let admit = || s.ledger.admit(seq, signature, "drc_build");
        let admitted = reader.fence(&req.revision)?.with_current(|| {
            let admit = || {
                if let Some(waives) = self.review(floe_app_core::drc::review::store::Kind::Waives) {
                    waives.admit_build(admit)
                } else {
                    admit()
                }
            };
            if let Some(notes) = self.notes() {
                notes.admit_build(admit)
            } else {
                admit()
            }
        })?;
        match admitted {
            Admission::Replay(v) => return Ok(v),
            Admission::New => (),
        }
        // Invalidate before releasing the identity lock. An in-flight old read
        // cannot refill either panel or prepared moves after this boundary.
        view.prepared.lock().unwrap().invalidate();
        *view.drc_panel.lock().unwrap() = super::panel::Panel::default();
        s.current = None;
        reader.request_stop();
        let stop = Arc::new(AtomicUsize::new(0));
        s.stop = Some(Arc::clone(&stop));
        s.pending = Some(Work {
            seq,
            reader,
            options,
            stop,
        });
        self.inner.wake.notify_one();
        Ok(s.ledger.get(seq).unwrap())
    }
    pub fn operations(&self) -> Value {
        self.inner.state.lock().unwrap().ledger.snapshot()
    }
    pub fn operation(&self, seq: u64) -> Option<Value> {
        self.inner.state.lock().unwrap().ledger.get(seq)
    }
    pub fn cancel(&self, seq: u64) -> std::result::Result<Value, Failure> {
        let s = self.inner.state.lock().unwrap();
        let result = s.ledger.get(seq).ok_or("operation_expired")?;
        if s.ledger.active() == Some(seq) {
            if let Some(stop) = &s.stop {
                stop.store(1, Ordering::Relaxed);
            }
        }
        Ok(result)
    }
    pub fn request_stop(&self) {
        let mut s = self.inner.state.lock().unwrap();
        s.closed = true;
        if let Some(stop) = &s.stop {
            stop.store(1, Ordering::Relaxed);
        }
        if let Some(d) = &s.current {
            d.request_stop();
        }
        for n in self.reviews() {
            n.request_stop();
        }
        self.inner.wake.notify_all();
    }
    pub fn is_finished(&self) -> bool {
        self.reviews().all(|n| n.is_finished())
            && self
                .thread
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(JoinHandle::is_finished)
            && self
                .inner
                .state
                .lock()
                .unwrap()
                .current
                .as_ref()
                .is_none_or(|d| d.is_finished())
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.request_stop();
        // NFS I/O is not killable. The worker retains reservations until exit;
        // transport teardown waits with its explicit deadline, never in Drop.
        if let Some(t) = self.thread.get_mut().unwrap().take() {
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}
fn update(inner: &Inner, seq: u64, value: Value) {
    inner.state.lock().unwrap().ledger.update(seq, value, false);
}
fn progress(seq: u64, s: &build::Snapshot) -> Value {
    let phase = match s.phase {
        Phase::Preparing => "preparing",
        Phase::Running => "running",
        Phase::Validating => "validating",
        Phase::Cancelling => "cancelling",
        Phase::Succeeded => "succeeded",
        Phase::Failed => "failed",
        Phase::Cancelled => "cancelled",
    };
    json!({"seq":seq.to_string(),"kind":"drc_build","phase":phase,"elapsed_ms":s.elapsed_ms.to_string(),
        "error":s.failure.map(crate::view::safe_error),"noninteger":s.native.drc_noninteger,"cleanup_warning":s.cleanup_warning,
        "native":{"checks":s.native.drc_checks.map(|n|n.to_string()),"total_checks":s.native.drc_total_checks.map(|n|n.to_string()),"errors":s.native.drc_errors.map(|n|n.to_string()),"output_bytes":s.native.output_bytes.to_string(),"dropped_lines":s.native.dropped_lines.to_string()},
        "outcome":s.outcome.as_ref().map(|o|json!({"reused":o.reused,"checks":o.checks.to_string(),"errors":o.errors.to_string(),"bytes":o.bytes.to_string(),"directory_synced":o.directory_synced}))})
}
fn protect_inputs(r: &Registration) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let protected = std::iter::once(r.path.clone())
        .chain(r.rules.clone())
        .collect::<Vec<_>>();
    let output =
        floe_app_core::artifact::protected_output(&build::output_path(&r.path)?, &protected, &[])?;
    let metadata = |path: &std::path::Path| match std::fs::metadata(path) {
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(floe_app_core::Error::from(e)),
    };
    if let (Some(rules), Some(output)) = (&r.rules, metadata(&output)?) {
        if metadata(rules)?.is_some_and(|m| (m.dev(), m.ino()) == (output.dev(), output.ino())) {
            // A case-insensitive filesystem can alias different path strings
            // without creating a second hard link. Compare identities too.
            return Err(floe_app_core::Error::input(
                "DRC output aliases registered SVRF input",
            ));
        }
    }
    Ok(())
}
fn execute(inner: &Inner, work: Work) -> Value {
    let Work {
        seq,
        reader,
        options,
        stop,
    } = work;
    update(
        inner,
        seq,
        json!({"seq":seq.to_string(),"kind":"drc_build","phase":"closing_review"}),
    );
    while !reader.is_finished() {
        thread::sleep(Duration::from_millis(10));
    }
    let previous = reader.registration.clone();
    drop(reader);
    let mut pack_path = None;
    let result = (|| -> Result<Value> {
        floe_app_core::check_cancelled(&stop)?;
        let r = &inner.registration;
        // --force approves replacing a pack, never a registered SVRF input
        // which happens to occupy the fixed adjacent output path.
        protect_inputs(r)?;
        let mut job = build::Build::start(
            &r.resources,
            Arc::clone(&r.scope),
            &r.path,
            options,
            inner.indexer.as_ref().unwrap().clone(),
        )?;
        loop {
            if stop.load(Ordering::Relaxed) != 0 {
                job.cancel();
            }
            let snapshot = job.snapshot();
            if snapshot.terminal() || job.is_finished() {
                job.close()?;
                let snapshot = job.snapshot();
                if snapshot.outcome.is_some() {
                    pack_path = Some(job.output().to_owned());
                }
                return Ok(progress(seq, &snapshot));
            }
            update(inner, seq, progress(seq, &snapshot));
            thread::sleep(Duration::from_millis(20));
        }
    })();
    let mut result = result.unwrap_or_else(|e|json!({"seq":seq.to_string(),"kind":"drc_build","phase":if e.kind==ErrorKind::Cancelled{"cancelled"}else{"failed"},"error":crate::view::safe_error(e.kind)}));
    // Once native publication wins, cancellation only stops future work. Do
    // not relabel published success as cancelled due to review reopen latency.
    if inner.state.lock().unwrap().closed {
        result["review"] = json!({"phase":"closed"});
        return result;
    }
    let target = pack_path.as_deref().unwrap_or(&previous.path);
    let mut opening = result.clone();
    opening["phase"] = json!("opening_review");
    opening["build_phase"] = result["phase"].clone();
    update(inner, seq, opening);
    let r = &inner.registration;
    let reopened = Service::start_with_rules(
        &r.resources,
        Arc::clone(&r.scope),
        target,
        None,
        r.rules.as_deref(),
        &r.source_id,
    )
    .or_else(|e| {
        let mut failed = r.clone();
        failed.path = target.to_owned();
        failed.waives = None;
        Service::unavailable(failed, super::code(&e))
    });
    match reopened {
        Ok(reader) => {
            let mut s = inner.state.lock().unwrap();
            if s.closed {
                reader.request_stop();
                drop(s);
                while !reader.is_finished() {
                    thread::sleep(Duration::from_millis(10));
                }
                result["review"] = json!({"phase":"closed"});
            } else {
                result["review"] = reader.catalog();
                s.current = Some(reader);
            }
        }
        Err(e) => {
            result["review"] = json!({"phase":"error","error":super::code(&e)});
        }
    }
    result
}
fn run(inner: Arc<Inner>) {
    loop {
        let work = {
            let mut s = inner.state.lock().unwrap();
            while s.pending.is_none() && !s.closed {
                s = inner.wake.wait(s).unwrap();
            }
            if s.pending.is_none() && s.closed {
                break;
            }
            s.pending.take().unwrap()
        };
        let seq = work.seq;
        let result = execute(&inner, work);
        let mut s = inner.state.lock().unwrap();
        s.ledger.update(seq, result, true);
        s.stop = None;
    }
    let reader = inner.state.lock().unwrap().current.clone();
    if let Some(d) = reader {
        d.request_stop();
        while !d.is_finished() {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

use crate::transport::{self, Gate};
use axum::{
    extract::{Path, State as WebState},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
pub(super) fn routes() -> Router<Gate> {
    Router::new()
        .route("/api/v1/drc/builds", get(operations).post(submit))
        .route("/api/v1/drc/builds/{seq}", get(operation))
        .route("/api/v1/drc/builds/{seq}/cancel", post(cancel))
}
async fn operations(WebState(gate): WebState<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    gate.drc.as_ref().map_or_else(
        || super::http::failure("drc_unavailable"),
        |r| Json(r.operations()).into_response(),
    )
}
async fn submit(
    WebState(gate): WebState<Gate>,
    headers: HeaderMap,
    body: std::result::Result<Json<BuildRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(registry) = &gate.drc else {
        return super::http::failure("drc_unavailable");
    };
    let Ok(Json(body)) = body else {
        return super::http::failure("invalid_drc_request");
    };
    match registry.submit(body, gate.active_view().as_ref()) {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => super::http::failure(e),
    }
}
async fn operation(
    WebState(gate): WebState<Gate>,
    headers: HeaderMap,
    Path(seq): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Ok(seq) = crate::view::counter(&seq) else {
        return super::http::failure("invalid_drc_request");
    };
    gate.drc
        .as_ref()
        .and_then(|r| r.operation(seq))
        .map_or_else(
            || super::http::failure("operation_expired"),
            |v| Json(v).into_response(),
        )
}
async fn cancel(
    WebState(gate): WebState<Gate>,
    headers: HeaderMap,
    Path(seq): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Ok(seq) = crate::view::counter(&seq) else {
        return super::http::failure("invalid_drc_request");
    };
    match gate
        .drc
        .as_ref()
        .ok_or("drc_unavailable")
        .and_then(|r| r.cancel(seq))
    {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => super::http::failure(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_app_core::{
        managed::{Limits, Resources},
        registered::AccessScope,
    };
    #[test]
    fn refreshed_revision_rejects_old_callbacks_without_replacing_reader() {
        let reg = Registration {
            resources: Resources::new(Limits::default()).unwrap(),
            scope: AccessScope::new(&[std::env::temp_dir()]).unwrap(),
            path: std::env::temp_dir().join("revision-test-not-opened.db"),
            waives: None,
            rules: None,
            source_id: "source".into(),
        };
        let reader = Service::unavailable(reg, "drc_read_error").unwrap();
        let registry = Registry::read_only(Arc::clone(&reader));
        let old = reader.revision();
        registry.with_revision(&reader, &old, || Ok(())).unwrap();
        let change = reader.inner.revision.begin().unwrap();
        let next = reader.revision();
        assert!(!registry.is_revision(&reader, &old));
        assert!(!registry.is_revision(&reader, &next));
        drop(change);
        assert_eq!(
            registry.with_revision(&reader, &old, || panic!("stale commit")),
            Err::<(), _>("drc_context_changed")
        );
        assert!(registry.is_revision(&reader, &next));
        assert_eq!(registry.current(&reader.id).unwrap().id, reader.id);
        registry.request_stop();
        assert!(!registry.is_revision(&reader, &next));
    }
    #[test]
    fn retired_read_callbacks_never_commit_to_the_new_review() {
        let reg = Registration {
            resources: Resources::new(Limits::default()).unwrap(),
            scope: AccessScope::new(&[std::env::temp_dir()]).unwrap(),
            path: std::env::temp_dir().join("registry-test-not-opened.db"),
            waives: None,
            rules: None,
            source_id: "source".into(),
        };
        let old = Service::unavailable(reg.clone(), "drc_read_error").unwrap();
        let new = Service::unavailable(reg, "drc_busy").unwrap();
        let registry = Registry::read_only(Arc::clone(&old));
        assert!(!registry.catalog()["build"]["available"].as_bool().unwrap());
        let edits = Arc::new(AtomicUsize::new(0));
        let rendezvous = Arc::new(std::sync::Barrier::new(2));
        let handle = {
            let (r, d, e, b) = (
                Arc::clone(&registry),
                Arc::clone(&old),
                Arc::clone(&edits),
                Arc::clone(&rendezvous),
            );
            thread::spawn(move || {
                r.with_current(&d, || {
                    b.wait();
                    e.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
            })
        };
        rendezvous.wait();
        {
            let mut s = registry.inner.state.lock().unwrap();
            // Retirement serializes after the callback that already owns the
            // guard. A callback arriving after it may not touch fresh state.
            assert_eq!(edits.load(Ordering::Relaxed), 1);
            s.current = Some(Arc::clone(&new));
        }
        handle.join().unwrap().unwrap();
        assert_eq!(
            registry
                .with_current(&old, || {
                    edits.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
                .unwrap_err(),
            "drc_context_changed"
        );
        assert_eq!(edits.load(Ordering::Relaxed), 1);
        assert!(registry.is_current(&new));
        assert!(!registry.is_current(&old));
        registry.request_stop();
        assert!(!registry.is_current(&new));
    }
}
