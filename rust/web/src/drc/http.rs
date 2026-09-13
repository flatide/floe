use super::Request;
use crate::transport::{self, Gate};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
pub(crate) fn routes() -> Router<Gate> {
    Router::new()
        .route("/api/v1/drc", get(catalog))
        .route("/api/v1/drc/{id}/read", post(read))
}
fn failure(code: super::Failure) -> Response {
    let status = match code {
        "drc_busy" => StatusCode::TOO_MANY_REQUESTS,
        "drc_closed" => StatusCode::GONE,
        "drc_unavailable" => StatusCode::NOT_FOUND,
        "drc_changed_or_corrupt" | "drc_read_error" => StatusCode::UNPROCESSABLE_ENTITY,
        "drc_read_limit" => StatusCode::PAYLOAD_TOO_LARGE,
        "drc_context_changed" => StatusCode::CONFLICT,
        "drc_cancelled" => StatusCode::REQUEST_TIMEOUT,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(json!({"error":code}))).into_response()
}
async fn catalog(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    Json(json!({"drc":gate.drc.as_ref().map(|d|d.catalog())})).into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    view_id: String,
    revision: String,
    body: Request,
}
async fn read(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<Read>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let session = match transport::http_session(&gate, &headers) {
        Ok(s) => s,
        Err(e) => return transport::error(e),
    };
    let Ok(Json(body)) = body else {
        return failure("invalid_drc_request");
    };
    let Some(drc) = gate.drc.as_ref().filter(|d| d.id == id) else {
        return failure("drc_unavailable");
    };
    let matches = || {
        gate.active_view().is_some_and(|v| {
            v.id == body.view_id && v.source_id == drc.source_id && !v.controller.is_finished()
        }) && body.revision == drc.revision
    };
    if !matches() {
        return failure("drc_context_changed");
    }
    let mut ticket = match drc.submit(body.body) {
        Ok(t) => t,
        Err(e) => return failure(e),
    };
    let result = ticket.result().await;
    if !gate.alive(&session) {
        return transport::error(StatusCode::UNAUTHORIZED);
    }
    if !matches() {
        return failure("drc_context_changed");
    }
    match result {
        Ok(bytes) => (
            [
                ("content-type", "application/json"),
                ("x-floe-drc-revision", drc.revision.as_str()),
                ("x-floe-view-id", body.view_id.as_str()),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => failure(e),
    }
}
