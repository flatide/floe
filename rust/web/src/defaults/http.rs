use super::{Prepare, Submit};
use crate::{
    transport::{self, Gate},
    view,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use floe_app_core::jobdeck::color::Mode;
use serde::Deserialize;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

pub(crate) fn routes() -> Router<Gate> {
    Router::new()
        .route("/api/v1/defaults", get(status).post(submit))
        .route("/api/v1/defaults/prepare", post(prepare))
        .route("/api/v1/defaults/revoke", post(revoke))
        .route("/api/v1/defaults/{seq}", get(operation))
        .route("/api/v1/defaults/{seq}/cancel", post(cancel))
}
fn fail(code: &str) -> Response {
    let status = match code {
        "busy" => StatusCode::TOO_MANY_REQUESTS,
        "stale_state" | "default_changed" | "operation_conflict" | "operation_sequence" => {
            StatusCode::CONFLICT
        }
        "closed" | "operation_expired" | "default_draft_expired" => StatusCode::GONE,
        "view_unavailable" | "source_unavailable" => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(json!({"error":code}))).into_response()
}
struct CancelPreparation(Arc<AtomicUsize>);
impl Drop for CancelPreparation {
    fn drop(&mut self) {
        self.0.store(1, Ordering::Relaxed);
    }
}
async fn prepare(
    State(g): State<Gate>,
    headers: HeaderMap,
    body: Result<Json<Prepare>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(o) => o,
        Err(e) => return transport::error(e),
    };
    let (Some(defaults), Some(views)) = (&g.defaults, &g.service) else {
        return transport::error(StatusCode::FORBIDDEN);
    };
    let Ok(Json(req)) = body else {
        return fail("invalid_request");
    };
    let rev = match view::counter(&req.state_rev) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let context = views.with_current(&req.view_id, |v| {
        let snapshot = v.controller.snapshot();
        if snapshot.state_rev != rev || v.controller.is_finished() {
            return Err("stale_state");
        }
        Ok((Arc::clone(v), snapshot.state))
    });
    let (attached, state) = match context {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let Some(source) = views.source(&attached.source_id) else {
        return fail("source_unavailable");
    };
    let permit = match Arc::clone(&defaults.preparations).try_acquire_owned() {
        Ok(p) => p,
        Err(_) => return fail("busy"),
    };
    let serial = match defaults.begin() {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let cancel = CancelPreparation(Arc::new(AtomicUsize::new(0)));
    let stop = Arc::clone(&cancel.0);
    let publisher = Arc::clone(&defaults.publisher);
    let model = Arc::clone(&attached.controller.model);
    let mode = match Mode::parse(attached.mode) {
        Ok(m) => m,
        Err(_) => return fail("invalid_request"),
    };
    let output = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        publisher.prepare(source, mode, &state.layerprops(&model)?, &stop)
    })
    .await;
    if !g.alive(&owner) || *g.stopping.borrow() {
        return transport::error(StatusCode::GONE);
    }
    let current = views.with_current(&req.view_id, |v| {
        if !Arc::ptr_eq(v, &attached)
            || v.controller.snapshot().state_rev != rev
            || v.controller.is_finished()
        {
            return Err("stale_state");
        }
        Ok(())
    });
    if let Err(e) = current {
        return fail(e);
    }
    match output {
        Ok(Ok(draft)) => match defaults.finish(serial, owner, req, draft) {
            Ok(mut v) => {
                v["mode"] = json!(attached.mode);
                v["levels"] = json!(attached.levels);
                v["title"] = json!(attached.title);
                v["rows"] = json!(attached.controller.model.styles.len());
                Json(v).into_response()
            }
            Err(e) => fail(e),
        },
        Ok(Err(e)) => fail(super::safe(e.kind)),
        Err(_) => fail("default_unavailable"),
    }
}
async fn submit(
    State(g): State<Gate>,
    headers: HeaderMap,
    body: Result<Json<Submit>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(o) => o,
        Err(e) => return transport::error(e),
    };
    let (Some(defaults), Some(views)) = (&g.defaults, &g.service) else {
        return transport::error(StatusCode::FORBIDDEN);
    };
    let Ok(Json(req)) = body else {
        return fail("invalid_request");
    };
    match defaults.submit(&owner, req, views) {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => fail(e),
    }
}
async fn status(State(g): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    g.defaults.as_ref().map_or_else(
        || transport::error(StatusCode::FORBIDDEN),
        |s| Json(s.status()).into_response(),
    )
}
async fn operation(State(g): State<Gate>, headers: HeaderMap, Path(seq): Path<String>) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let Some(s) = &g.defaults else {
        return transport::error(StatusCode::FORBIDDEN);
    };
    let seq = match view::counter(&seq) {
        Ok(n) => n,
        Err(e) => return fail(e),
    };
    s.operation(seq)
        .map_or_else(|| fail("operation_expired"), |v| Json(v).into_response())
}
async fn cancel(State(g): State<Gate>, headers: HeaderMap, Path(seq): Path<String>) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let Some(s) = &g.defaults else {
        return transport::error(StatusCode::FORBIDDEN);
    };
    let seq = match view::counter(&seq) {
        Ok(n) => n,
        Err(e) => return fail(e),
    };
    match s.cancel(seq) {
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
    headers: HeaderMap,
    body: Result<Json<Revoke>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(o) => o,
        Err(e) => return transport::error(e),
    };
    let Some(s) = &g.defaults else {
        return transport::error(StatusCode::FORBIDDEN);
    };
    let Ok(Json(req)) = body else {
        return fail("invalid_request");
    };
    if req.token.len() != 64 {
        return fail("invalid_request");
    }
    s.revoke(&owner, &req.token);
    StatusCode::NO_CONTENT.into_response()
}
