//! Authenticated owner catalog/operations. All handlers are bounded in-memory
//! lookups/DTO conversion; the service thread performs actual filesystem work.
use crate::{
    service::OperationDto,
    transport::{self, Gate},
    view,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::json;
pub(crate) fn routes() -> Router<Gate> {
    Router::new()
        .merge(crate::settings::routes())
        .route("/api/v1/catalog", get(catalog))
        .route("/api/v1/catalog/{id}/levels/{start}", get(levels))
        .route("/api/v1/operations", get(operations).post(submit))
        .route("/api/v1/operations/{seq}", get(operation))
        .route(
            "/api/v1/operations/{seq}/index-open",
            get(index_open_preview),
        )
        .route("/api/v1/operations/{seq}/cancel", post(cancel))
        .route("/api/v1/views/{id}", delete(close_view))
        .route("/api/v1/views/{id}/layers/{start}", get(layers))
        .route("/api/v1/views/{id}/minimap/{base}", get(minimap))
}
fn failure(code: &'static str) -> Response {
    let status = match code {
        "busy" => StatusCode::TOO_MANY_REQUESTS,
        "operation_conflict" | "operation_sequence" => StatusCode::CONFLICT,
        "operation_expired" | "closed" => StatusCode::GONE,
        "source_unavailable" | "view_unavailable" => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(json!({"error":code}))).into_response()
}
async fn minimap(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, base)): Path<(String, String)>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(v) = gate.active_view().filter(|v| v.id == id) else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let model = &v.controller.model;
    match model.minimap.base(&base) {
        Some(pixels) => Json(json!({"view_id":id,"dataset_revision":model.dataset_revision.to_string(),"base":base,"size":180,"pixels":pixels})).into_response(),
        None => transport::error(StatusCode::NOT_FOUND),
    }
}
async fn catalog(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    gate.service.as_ref().map_or_else(
        || transport::error(StatusCode::NOT_FOUND),
        |s| Json(s.catalog()).into_response(),
    )
}
async fn levels(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, start)): Path<(String, usize)>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    gate.service
        .as_ref()
        .and_then(|s| s.levels(&id, start))
        .map_or_else(
            || transport::error(StatusCode::NOT_FOUND),
            |v| Json(v).into_response(),
        )
}
async fn layers(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, start)): Path<(String, usize)>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    gate.active_view()
        .filter(|v| v.id == id)
        .and_then(|v| v.rows.page(&v.controller.snapshot(), start))
        .map_or_else(
            || transport::error(StatusCode::NOT_FOUND),
            |v| Json(v).into_response(),
        )
}
async fn operations(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    gate.service.as_ref().map_or_else(
        || transport::error(StatusCode::NOT_FOUND),
        |s| Json(s.operations()).into_response(),
    )
}
async fn submit(
    State(gate): State<Gate>,
    headers: HeaderMap,
    body: std::result::Result<Json<OperationDto>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(service) = &gate.service else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let Ok(Json(body)) = body else {
        return failure("invalid_request");
    };
    match service.submit(body) {
        Ok(state) => (StatusCode::ACCEPTED, Json(state)).into_response(),
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
    let Ok(seq) = view::counter(&seq) else {
        return failure("invalid_request");
    };
    gate.service
        .as_ref()
        .and_then(|s| s.operation(seq))
        .map_or_else(|| failure("operation_expired"), |v| Json(v).into_response())
}
async fn index_open_preview(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(seq): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Ok(seq) = view::counter(&seq) else {
        return failure("invalid_request");
    };
    let Some(service) = &gate.service else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    match service.index_open_preview(seq) {
        Ok(v) => Json(v).into_response(),
        Err(code) => failure(code),
    }
}
async fn cancel(State(gate): State<Gate>, headers: HeaderMap, Path(seq): Path<String>) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Ok(seq) = view::counter(&seq) else {
        return failure("invalid_request");
    };
    let Some(service) = &gate.service else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    match service.cancel(seq) {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(code) => failure(code),
    }
}
async fn close_view(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    if let Some(service) = &gate.service {
        return match service.close_view(&id) {
            Ok(()) => StatusCode::ACCEPTED.into_response(),
            Err(code) => failure(code),
        };
    }
    match gate.active_view().filter(|v| v.id == id) {
        Some(v) => {
            v.controller.request_close();
            StatusCode::ACCEPTED.into_response()
        }
        None => failure("view_unavailable"),
    }
}
