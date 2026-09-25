//! Separate owner approval/ledger: ordinary saves and autosave tokens cannot
//! execute repairs. A failed reader still has a trusted registration/context.
use super::http::{alive, current, fail, reader, review};
use super::*;
use crate::transport::{self, Gate};
use axum::{
    extract::{Extension, State as HttpState},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};

#[derive(Default)]
pub(super) struct State {
    pub(super) ledger: Ledger,
    pub(super) pending: Option<Task>,
    pub(super) held: Option<Task>,
}
pub(super) struct Task {
    seq: u64,
    ready: Ready,
    check_only: bool,
    read_stop: Option<Arc<AtomicUsize>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    context: Context,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    seq: String,
    context: Context,
    token: String,
    approve_recovery: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Check {
    seq: String,
}
pub(super) fn routes(root: &str) -> Router<Gate> {
    Router::new()
        .route(&format!("{root}/recovery"), get(status).post(approve))
        .route(&format!("{root}/recovery/prepare"), post(prepare))
        .route(&format!("{root}/recovery/reconcile"), post(reconcile))
}
fn signature(owner: &SessionId, req: &Approval) -> String {
    format!("{} {req:?}", owner.as_str())
}
fn snapshot(service: &Service) -> Value {
    let s = service.inner.state.lock().unwrap();
    json!({"kind":"review_recovery","review_kind":service.kind(),
        "available":!s.closed && !s.detached && service.inner.config.editable,
        "reviewer":service.inner.config.reviewer,"binding_id":s.binding.id,
        "operations":s.recovery.ledger.snapshot()})
}
async fn status(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    match review(&g, kind) {
        Ok(s) => Json(snapshot(&s)).into_response(),
        Err(e) => fail(e),
    }
}
async fn prepare(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Extension(body): Extension<Arc<OwnedSemaphorePermit>>,
    req: std::result::Result<Json<Preview>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(o) => o,
        Err(e) => return transport::error(e),
    };
    let service = match review(&g, kind) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let Ok(Json(req)) = req else {
        return fail("invalid_drc_request");
    };
    let r = match reader(&g, &req.context) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let op = match current(&g, &r, &req.context, || service.begin(body)) {
        Ok(o) => o,
        Err(e) => return fail(e),
    };
    let mut cancel = super::http::Cancel(Some(Arc::clone(&op)));
    let task = Arc::clone(&op);
    let registered = Arc::clone(&r);
    let value = tokio::task::spawn_blocking(move || {
        let store = task.service.open(&registered, &task.stop)?;
        store.prepare_recovery(Arc::clone(&task.stop))
    })
    .await;
    let proof = match value {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return fail(safe(e.kind)),
        Err(_) => return fail("review_unavailable"),
    };
    if !alive(&g, &owner) {
        return fail("review_expired");
    }
    let value = json!({"kind":"review_recovery","phase":"prepared","review_kind":service.kind(),
        "name":proof.target().file_name().unwrap_or_default().to_string_lossy(),
        "bytes":proof.bytes().to_string(),"scope":"exact_staging_link_only"});
    match current(&g, &r, &req.context, || {
        service.finish(
            &op,
            owner,
            Arc::clone(&r),
            req.context.clone(),
            Model::Recovery(proof),
            value,
        )
    }) {
        Ok(v) => {
            cancel.0 = None;
            Json(v).into_response()
        }
        Err(e) => fail(e),
    }
}
fn submit(
    service: &Service,
    owner: &SessionId,
    req: Approval,
) -> std::result::Result<Value, Failure> {
    service.require_editor()?;
    let seq = crate::view::counter(&req.seq)?;
    if !req.approve_recovery {
        return Err("review_approval_required");
    }
    let sig = signature(owner, &req);
    let mut s = service.inner.state.lock().unwrap();
    if s.closed || s.detached {
        return Err("review_disabled");
    }
    if let Some(v) = s.recovery.ledger.replay(seq, &sig)? {
        return Ok(v);
    }
    if s.preparing.is_some() || s.ledger.active().is_some() || s.transfer.ledger.active().is_some()
    {
        return Err("drc_busy");
    }
    let ready = s
        .ready
        .as_ref()
        .filter(|r| {
            r.owner == *owner
                && r.token == req.token
                && r.context == req.context
                && r.expires > Instant::now()
                && matches!(&r.model, Model::Recovery(_))
        })
        .ok_or("review_expired")?;
    if ready.stop.load(Ordering::Relaxed) != 0 {
        return Err("review_expired");
    }
    if s.review_rev == u64::MAX {
        return Err("review_limit");
    }
    let binding = s.binding.id.clone();
    match s
        .recovery
        .ledger
        .admit_scoped(seq, sig, "review_recovery", Some(&binding))?
    {
        Admission::Replay(v) => return Ok(v),
        Admission::New => (),
    }
    let ready = s.ready.take().unwrap();
    s.stop = Some(Arc::clone(&ready.stop));
    s.display = None;
    s.recovery.pending = Some(Task {
        seq,
        ready,
        check_only: false,
        read_stop: None,
    });
    service.inner.wake.notify_one();
    Ok(s.recovery.ledger.get(seq).unwrap())
}
async fn approve(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    req: std::result::Result<Json<Approval>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(o) => o,
        Err(e) => return transport::error(e),
    };
    let service = match review(&g, kind) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let Ok(Json(req)) = req else {
        return fail("invalid_drc_request");
    };
    let seq = match crate::view::counter(&req.seq) {
        Ok(n) => n,
        Err(e) => return fail(e),
    };
    match service
        .inner
        .state
        .lock()
        .unwrap()
        .recovery
        .ledger
        .replay(seq, &signature(&owner, &req))
    {
        Ok(Some(v)) => return (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => return fail(e),
        _ => (),
    }
    let r = match reader(&g, &req.context) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let context = req.context.clone();
    match current(&g, &r, &context, || submit(&service, &owner, req)) {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => fail(e),
    }
}
async fn reconcile(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    req: std::result::Result<Json<Check>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(o) => o,
        Err(e) => return transport::error(e),
    };
    let service = match review(&g, kind) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let Ok(Json(req)) = req else {
        return fail("invalid_drc_request");
    };
    let seq = match crate::view::counter(&req.seq) {
        Ok(n) => n,
        Err(e) => return fail(e),
    };
    let mut s = service.inner.state.lock().unwrap();
    if s.closed || s.detached {
        return fail("review_disabled");
    }
    let Some(value) = s.recovery.ledger.get(seq) else {
        return fail("operation_expired");
    };
    if s.recovery.ledger.active() != Some(seq) {
        return (StatusCode::ACCEPTED, Json(value)).into_response();
    }
    if let Some(held) = &s.recovery.held {
        if held.seq != seq || held.ready.owner != owner {
            return fail("review_expired");
        }
        let mut task = s.recovery.held.take().unwrap();
        task.check_only = true;
        let stop = Arc::new(AtomicUsize::new(0));
        task.read_stop = Some(Arc::clone(&stop));
        s.stop = Some(stop);
        s.recovery.pending = Some(task);
        s.recovery.ledger.update(
            seq,
            json!({"kind":"review_recovery","seq":seq.to_string(),"phase":"reconciling"}),
            false,
        );
        service.inner.wake.notify_one();
    }
    (
        StatusCode::ACCEPTED,
        Json(s.recovery.ledger.get(seq).unwrap()),
    )
        .into_response()
}
pub(super) fn run(inner: &Inner, task: Task) {
    use store::{RecoveryResult as R, RecoveryState as S};
    let seq = task.seq;
    let r = &task.ready.reader;
    // Recovery must work even when metadata failed. Retire queries, never
    // reinstall an old reader or pretend it observed the repaired file.
    let change = if task.check_only {
        None
    } else {
        r.inner
            .revision
            .begin_at(Some(&task.ready.context.revision))
            .ok()
    };
    let retired = change.is_some() || task.check_only;
    if change.is_some() {
        // A new revision must not re-legitimize cached waive statuses. Keep
        // the registration for explicit reopen/recovery, but disable queries.
        r.inner.state.lock().unwrap().failure = Some("drc_reopen_required");
    }
    let result = if !task.check_only && change.is_none() {
        Ok(Err(floe_app_core::Error::new(
            ErrorKind::Busy,
            "review context changed",
        )))
    } else {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Result<(Option<bool>, Option<bool>)> {
                let Model::Recovery(proof) = &task.ready.model else {
                    unreachable!()
                };
                if task.check_only {
                    return Ok(match proof.reconcile(task.read_stop.as_ref().unwrap())? {
                        S::Pending => (Some(false), None),
                        S::Completed => (Some(true), None),
                    });
                }
                Ok(match proof.recover()? {
                    R::Recovered {
                        directory_synced, ..
                    } => (Some(true), Some(directory_synced)),
                    R::Uncertain => (None, None),
                })
            },
        ))
    };
    drop(change);
    let (recovered, synced, error) = match result {
        Ok(Ok((done, sync))) => (done, sync, None),
        Ok(Err(e)) if !task.check_only => (Some(false), None, Some(safe(e.kind))),
        Ok(Err(e)) => (None, None, Some(safe(e.kind))),
        Err(_) => (None, None, Some("review_unavailable")),
    };
    let mut s = inner.state.lock().unwrap();
    if !task.check_only {
        s.review_rev += 1;
        for id in s.transfer.artifacts.keys() {
            inner.artifacts.request_release(*id);
        }
        s.transfer.artifacts.clear();
    }
    let value = json!({"kind":"review_recovery","seq":seq.to_string(),
        "phase":if recovered.is_none(){"uncertain"}else if recovered==Some(true){"succeeded"}else{"failed"},
        "context":task.ready.context,"reader_revision":r.revision(),"review_rev":s.review_rev.to_string(),
        "recovered":recovered,"directory_synced":synced,"error":error,"reopen_required":retired});
    s.recovery.ledger.update(seq, value, recovered.is_some());
    if recovered.is_none() && !s.closed {
        s.recovery.held = Some(task);
    } else {
        s.stop = None;
        drop(s);
        drop(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unresolved_repair_blocks_new_reads_saves_and_registration_changes() {
        let service = super::super::tests::service();
        let actor = super::super::tests::owner();
        let save = super::super::tests::request();
        {
            let mut s = service.inner.state.lock().unwrap();
            s.recovery
                .ledger
                .admit(1, "approved repair".into(), "review_recovery")
                .unwrap();
            s.recovery
                .ledger
                .update(1, json!({"seq":"1","phase":"uncertain"}), false);
        }
        let body = Arc::new(Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap());
        assert!(matches!(service.begin(body), Err("drc_busy")));
        assert!(matches!(service.submit(&actor, save), Err("drc_busy")));
        assert!(matches!(service.admit_build(|| Ok(())), Err("drc_busy")));
        assert!(matches!(service.admit_detach(|| Ok(())), Err("drc_busy")));
        assert!(matches!(
            service.admit_reconnect(Binding::new(None, None).unwrap(), || Ok(())),
            Err("review_registration_changed")
        ));
        let mut s = service.inner.state.lock().unwrap();
        s.recovery
            .ledger
            .update(1, json!({"seq":"1","phase":"succeeded"}), true);
        drop(s);
        service.admit_detach(|| Ok(())).unwrap();
        super::super::tests::stop(&service);
    }
}
