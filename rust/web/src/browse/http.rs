use super::Request;
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
use serde_json::json;
pub(crate) fn routes() -> Router<Gate> {
    Router::new()
        .route("/api/v1/browse", get(snapshot).post(submit))
        .route("/api/v1/browse/{seq}", get(operation))
        .route("/api/v1/browse/{seq}/cancel", post(cancel))
}
fn failure(code: &'static str) -> Response {
    (
        match code {
            "busy" => StatusCode::TOO_MANY_REQUESTS,
            "closed" | "operation_expired" => StatusCode::GONE,
            _ => StatusCode::CONFLICT,
        },
        Json(json!({"error":code})),
    )
        .into_response()
}
async fn snapshot(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    gate.browse.as_ref().map_or_else(
        || transport::error(StatusCode::NOT_FOUND),
        |b| Json(b.snapshot()).into_response(),
    )
}
async fn submit(
    State(gate): State<Gate>,
    headers: HeaderMap,
    input: std::result::Result<Json<Request>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(b) = &gate.browse else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let Ok(Json(input)) = input else {
        return transport::error(StatusCode::BAD_REQUEST);
    };
    match b.submit(input) {
        Ok(value) => (StatusCode::ACCEPTED, Json(value)).into_response(),
        Err(code) => failure(code),
    }
}
async fn operation(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(seq): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(b) = &gate.browse else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let Ok(seq) = view::counter(&seq) else {
        return transport::error(StatusCode::BAD_REQUEST);
    };
    b.operation(seq).map_or_else(
        || transport::error(StatusCode::NOT_FOUND),
        |v| Json(v).into_response(),
    )
}
async fn cancel(State(gate): State<Gate>, headers: HeaderMap, Path(seq): Path<String>) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(b) = &gate.browse else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let Ok(seq) = view::counter(&seq) else {
        return transport::error(StatusCode::BAD_REQUEST);
    };
    match b.cancel(seq) {
        Ok(v) => Json(v).into_response(),
        Err(code) => failure(code),
    }
}
