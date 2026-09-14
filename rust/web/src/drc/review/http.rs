use super::*;
use crate::transport::{self, Gate};
use axum::{
    extract::{DefaultBodyLimit, Extension, Path, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::collections::BTreeSet;

pub(crate) fn is_large_body(method: &Method, path: &str) -> bool {
    *method == Method::POST
        && matches!(
            path,
            "/api/v1/drc/review/notes/read"
                | "/api/v1/drc/review/notes/prepare"
                | "/api/v1/drc/review/waives/read"
                | "/api/v1/drc/review/waives/prepare"
        )
}
pub(crate) fn routes() -> Router<Gate> {
    routes_for(store::Kind::Notes, "/api/v1/drc/review/notes")
        .merge(routes_for(store::Kind::Waives, "/api/v1/drc/review/waives"))
}
fn routes_for(kind: store::Kind, root: &str) -> Router<Gate> {
    Router::new()
        .route(root, get(status).post(submit))
        .route(&format!("{root}/read"), post(read))
        .route(&format!("{root}/prepare"), post(prepare))
        .route(&format!("{root}/revoke"), post(revoke))
        .route(&format!("{root}/{{seq}}"), get(operation))
        .route(&format!("{root}/{{seq}}/cancel"), post(cancel))
        .layer(Extension(kind))
        .layer(DefaultBodyLimit::max(crate::drc::RESPONSE_BYTES))
}
fn fail(code: Failure) -> Response {
    crate::drc::http::failure(code)
}
fn review(g: &Gate, kind: store::Kind) -> std::result::Result<Arc<Service>, Failure> {
    g.drc
        .as_ref()
        .and_then(|r| r.review(kind))
        .ok_or("review_disabled")
}
fn reader(g: &Gate, c: &Context) -> std::result::Result<Arc<Reader>, Failure> {
    c.validate()?;
    let reader = g
        .drc
        .as_ref()
        .and_then(|r| r.current(&c.drc_id))
        .ok_or("drc_context_changed")?;
    current(g, &reader, c, || Ok(()))?;
    Ok(reader)
}
/// Lock order registry -> registered owner view -> review. No filesystem I/O.
fn current<T>(
    g: &Gate,
    r: &Reader,
    c: &Context,
    f: impl FnOnce() -> std::result::Result<T, Failure>,
) -> std::result::Result<T, Failure> {
    let registry = g.drc.as_ref().ok_or("drc_context_changed")?;
    let views = g.service.as_ref().ok_or("drc_context_changed")?;
    registry.with_revision(r, &c.revision, || {
        views.with_current(&c.view_id, |v| {
            if r.id != c.drc_id || v.source_id != r.source_id || v.controller.is_finished() {
                return Err("drc_context_changed");
            }
            f()
        })
    })
}
struct Cancel(Option<Arc<Operation>>);
impl Drop for Cancel {
    fn drop(&mut self) {
        if let Some(op) = &self.0 {
            op.stop.store(1, Ordering::Relaxed);
        }
    }
}
fn alive(g: &Gate, owner: &SessionId) -> bool {
    g.alive(owner) && !*g.stopping.borrow()
}
fn refs(errors: &[crate::drc::dto::CursorDto]) -> std::result::Result<Vec<(usize, u64)>, Failure> {
    if errors.is_empty() || errors.len() > floe_app_core::drc::review::EDIT_ITEMS {
        return Err("drc_read_limit");
    }
    errors
        .iter()
        .map(|r| {
            Ok((
                usize::try_from(crate::drc::dto::number(&r.check)?)
                    .map_err(|_| "invalid_drc_request")?,
                crate::drc::dto::number(&r.error)?,
            ))
        })
        .collect()
}
async fn read(
    State(g): State<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Extension(body_permit): Extension<Arc<OwnedSemaphorePermit>>,
    body: std::result::Result<Json<Read>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(o) => o,
        Err(e) => return transport::error(e),
    };
    let service = match review(&g, kind) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let Ok(Json(req)) = body else {
        return fail("invalid_drc_request");
    };
    let references = match refs(&req.errors) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let r = match reader(&g, &req.context) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    if r.catalog()["metadata"]["format"] != "ice" {
        return fail("review_pack_required");
    }
    let op = match current(&g, &r, &req.context, || service.begin(body_permit)) {
        Ok(o) => o,
        Err(e) => return fail(e),
    };
    let mut cancel = Cancel(Some(Arc::clone(&op)));
    let task = Arc::clone(&op);
    let registered = Arc::clone(&r);
    let store =
        match tokio::task::spawn_blocking(move || task.service.open(&registered, &task.stop)).await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return fail(safe(e.kind)),
            Err(_) => return fail("review_unavailable"),
        };
    if !alive(&g, &owner) {
        return transport::error(StatusCode::GONE);
    }
    let mut ticket = match r.enqueue(crate::drc::dto::Command::ReviewTargets {
        identity: store.identity(),
        refs: references,
    }) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let bytes = match ticket.result().await {
        Ok(b) => b,
        Err(e) => return fail(e),
    };
    #[derive(Deserialize)]
    struct Targets {
        gids: Vec<String>,
    }
    let gids = match serde_json::from_slice::<Targets>(&bytes)
        .ok()
        .and_then(|t| {
            t.gids
                .iter()
                .map(|s| crate::drc::dto::number(s))
                .collect::<std::result::Result<BTreeSet<_>, _>>()
                .ok()
        }) {
        Some(ids) if !ids.is_empty() && ids.len() <= floe_app_core::drc::review::EDIT_ITEMS => {
            ids.into_iter().collect::<Vec<_>>()
        }
        _ => return fail("review_unavailable"),
    };
    let task = Arc::clone(&op);
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        let snapshot = store.snapshot(Arc::clone(&task.stop))?;
        if kind == store::Kind::Waives {
            let statuses = snapshot.selected_statuses(&gids)?;
            let value = json!({"kind":"drc_waive","phase":"snapshot","name":store.target().file_name().and_then(|s|s.to_str()),
                "selected_count":gids.len().to_string(),"waived_count":statuses.iter().filter(|&&v|v==1).count().to_string(),
                "reserved_count":statuses.iter().filter(|&&v|v>1).count().to_string(),
                "exists":snapshot.exists(),"legacy_unverified":snapshot.legacy_unverified()});
            return Ok((Model::Snapshot(snapshot, gids), value));
        }
        let notes = snapshot.notes().ok_or_else(|| floe_app_core::Error::input("not a note snapshot"))?;
        let first = notes.get(gids[0]);
        let mixed = gids.iter().any(|&g| notes.get(g) != first);
        let report = snapshot.import_report();
        let value = json!({"kind":"drc_note","phase":"snapshot","name":store.target().file_name().and_then(|s|s.to_str()),
            "selected_count":gids.len().to_string(),"existing_count":gids.iter().filter(|&&g|notes.get(g).is_some()).count().to_string(),
            "mixed":mixed,"text":if mixed {None} else {first},"exists":snapshot.exists(),"legacy_unverified":snapshot.legacy_unverified(),
            "import_report":{"skipped_lines":report.skipped_lines,"invalid_members":report.invalid_members,"reassigned_members":report.reassigned_members}});
        Ok((Model::Snapshot(snapshot, gids), value))
    }).await;
    if !alive(&g, &owner) {
        return transport::error(StatusCode::GONE);
    }
    let (model, value) = match result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return fail(safe(e.kind)),
        Err(_) => return fail("review_unavailable"),
    };
    match current(&g, &r, &req.context, || {
        service.finish(
            &op,
            owner,
            Arc::clone(&r),
            req.context.clone(),
            model,
            value,
        )
    }) {
        Ok(value) => {
            cancel.0 = None;
            Json(value).into_response()
        }
        Err(e) => fail(e),
    }
}
async fn prepare(
    State(g): State<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Extension(body_permit): Extension<Arc<OwnedSemaphorePermit>>,
    body: std::result::Result<Json<Prepare>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(o) => o,
        Err(e) => return transport::error(e),
    };
    let service = match review(&g, kind) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let Ok(Json(req)) = body else {
        return fail("invalid_drc_request");
    };
    let r = match reader(&g, &req.context) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let (op, snapshot, gids) = match current(&g, &r, &req.context, || {
        service.begin_prepare(&owner, &req, body_permit)
    }) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let mut cancel = Cancel(Some(Arc::clone(&op)));
    let task = Arc::clone(&op);
    let context = req.context;
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        floe_app_core::check_cancelled(&task.stop)?;
        let count = gids.len();
        let exists = snapshot.exists();
        if kind == store::Kind::Waives {
            let status = u8::from(req.waived.unwrap());
            let before = snapshot.selected_statuses(&gids)?;
            let changed = before.iter().filter(|&&v|v != status).count();
            let edits = gids.iter().map(|&id|(id, status)).collect::<Vec<_>>();
            let draft = snapshot.prepare_waives(&edits)?;
            let value = json!({"kind":"drc_waive","phase":"prepared","name":draft.target().file_name().and_then(|s|s.to_str()),
                "selected_count":count.to_string(),"changed_count":changed.to_string(),"waived":status==1,
                "reserved_count":before.iter().filter(|&&v|v>1).count().to_string(),
                "legacy_unverified":draft.legacy_unverified(),"replaces_existing":exists,"scope":"registered_reviewer_waives"});
            return Ok((Model::Prepared(draft), value));
        }
        let text = req.text.unwrap();
        let report = snapshot.import_report();
        let report = json!({"skipped_lines":report.skipped_lines,"invalid_members":report.invalid_members,"reassigned_members":report.reassigned_members});
        let draft = snapshot.prepare_note(&gids, &text)?;
        let value = json!({"kind":"drc_note","phase":"prepared","name":draft.target().file_name().and_then(|s|s.to_str()),
            "selected_count":count.to_string(),"text":text.trim(),"clears":text.trim().is_empty(),
            "legacy_unverified":draft.legacy_unverified(),"replaces_existing":exists,"import_report":report,"scope":"registered_reviewer_notes"});
        Ok((Model::Prepared(draft), value))
    }).await;
    if !alive(&g, &owner) {
        return transport::error(StatusCode::GONE);
    }
    let (model, value) = match result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return fail(safe(e.kind)),
        Err(_) => return fail("review_unavailable"),
    };
    match current(&g, &r, &context, || {
        service.finish(&op, owner, Arc::clone(&r), context.clone(), model, value)
    }) {
        Ok(value) => {
            cancel.0 = None;
            Json(value).into_response()
        }
        Err(e) => fail(e),
    }
}
async fn submit(
    State(g): State<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    body: std::result::Result<Json<Submit>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(o) => o,
        Err(e) => return transport::error(e),
    };
    let service = match review(&g, kind) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let Ok(Json(req)) = body else {
        return fail("invalid_drc_request");
    };
    match service.replay(&owner, &req) {
        Ok(Some(v)) => return (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => return fail(e),
        _ => (),
    }
    let r = match reader(&g, &req.context) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let context = req.context.clone();
    match current(&g, &r, &context, || service.submit(&owner, req)) {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => fail(e),
    }
}
async fn status(
    State(g): State<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    match review(&g, kind) {
        Ok(s) => Json(s.status()).into_response(),
        Err(e) => fail(e),
    }
}
async fn operation(
    State(g): State<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Path(seq): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let s = match review(&g, kind) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let n = match crate::view::counter(&seq) {
        Ok(n) => n,
        Err(e) => return fail(e),
    };
    s.operation(n)
        .map_or_else(|| fail("operation_expired"), |v| Json(v).into_response())
}
async fn cancel(
    State(g): State<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Path(seq): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let s = match review(&g, kind) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let n = match crate::view::counter(&seq) {
        Ok(n) => n,
        Err(e) => return fail(e),
    };
    match s.cancel(n) {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => fail(e),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revoke {
    token: String,
}
async fn revoke(
    State(g): State<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    body: std::result::Result<Json<Revoke>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(o) => o,
        Err(e) => return transport::error(e),
    };
    let s = match review(&g, kind) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let Ok(Json(r)) = body else {
        return fail("invalid_drc_request");
    };
    s.revoke(&owner, &r.token);
    StatusCode::NO_CONTENT.into_response()
}
