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
        .route(
            "/api/v1/drc/{id}/views/{view}/panel",
            get(panel_get).post(panel_set),
        )
        .merge(super::selection::routes())
}
pub(super) fn failure(code: super::Failure) -> Response {
    let status = match code {
        "drc_busy" => StatusCode::TOO_MANY_REQUESTS,
        "drc_closed" => StatusCode::GONE,
        "drc_unavailable" => StatusCode::NOT_FOUND,
        "drc_changed_or_corrupt" | "drc_read_error" => StatusCode::UNPROCESSABLE_ENTITY,
        "drc_read_limit" | "drc_selection_limit" => StatusCode::PAYLOAD_TOO_LARGE,
        "drc_context_changed" | "drc_panel_conflict" | "drc_selection_conflict" => {
            StatusCode::CONFLICT
        }
        "drc_cancelled" => StatusCode::REQUEST_TIMEOUT,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(json!({"error":code}))).into_response()
}
async fn panel_get(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, view)): Path<(String, String)>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(drc) = gate.drc.as_ref().filter(|d| d.id == id) else {
        return failure("drc_unavailable");
    };
    let Some(v) = gate
        .active_view()
        .filter(|v| v.id == view && v.source_id == drc.source_id && !v.controller.is_finished())
    else {
        return failure("drc_context_changed");
    };
    let state = v.drc_panel.lock().unwrap().snapshot();
    Json(json!({"revision":drc.revision,"view_id":view,"state":state})).into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PanelSet {
    revision: String,
    base_panel_rev: String,
    body: super::panel::Data,
}
async fn panel_set(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, view)): Path<(String, String)>,
    body: Result<Json<PanelSet>, axum::extract::rejection::JsonRejection>,
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
        gate.active_view().filter(|v| {
            v.id == view
                && v.source_id == drc.source_id
                && !v.controller.is_finished()
                && body.revision == drc.revision
        })
    };
    let Some(v) = matches() else {
        return failure("drc_context_changed");
    };
    let Ok(base) = crate::view::counter(&body.base_panel_rev) else {
        return failure("invalid_drc_request");
    };
    if let Err(e) = body
        .body
        .validate()
        .and_then(|()| v.drc_panel.lock().unwrap().check(base, &body.body))
    {
        return failure(e);
    }
    if body.body.query.as_ref().is_some_and(|q| {
        crate::view::counter(&q.state_rev).is_ok_and(|rev| rev > v.controller.snapshot().state_rev)
    }) {
        return failure("invalid_drc_request");
    }
    // Validate references on the existing bounded actor; no coordinate decode,
    // implicit goto, file write or new worker is needed for panel state.
    let mut ticket = match drc.enqueue(super::dto::Command::ValidatePanel(Box::new(
        body.body.clone(),
    ))) {
        Ok(t) => t,
        Err(e) => return failure(e),
    };
    let validated = ticket.result().await;
    if !gate.alive(&session) {
        return transport::error(StatusCode::UNAUTHORIZED);
    }
    if matches().is_none() {
        return failure("drc_context_changed");
    }
    if let Err(e) = validated {
        return failure(e);
    }
    let result = v.drc_panel.lock().unwrap().set(base, body.body);
    match result {
        Ok(state) => {
            Json(json!({"revision":drc.revision,"view_id":view,"state":state})).into_response()
        }
        Err(e) => failure(e),
    }
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
    state_rev: Option<String>,
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
    let focused = matches!(
        &body.body,
        Request::Focus { .. }
            | Request::InView { .. }
            | Request::List { in_view: true, .. }
            | Request::FilteredStep { in_view: true, .. }
    );
    let selection_filter = match body.body.selection_filter() {
        Ok(v) => v,
        Err(e) => return failure(e),
    };
    if body
        .state_rev
        .as_ref()
        .is_some_and(|s| crate::view::counter(s).is_err())
        || focused && body.state_rev.is_none()
    {
        return failure("invalid_drc_request");
    }
    let matches = || {
        gate.active_view().is_some_and(|v| {
            v.id == body.view_id
                && v.source_id == drc.source_id
                && !v.controller.is_finished()
                && body
                    .state_rev
                    .as_ref()
                    .is_none_or(|rev| v.controller.snapshot().state_rev.to_string() == *rev)
        }) && body.revision == drc.revision
    };
    if !matches() {
        return failure("drc_context_changed");
    }
    let context = if focused {
        let Some(v) = gate.active_view() else {
            return failure("drc_context_changed");
        };
        let s = v.controller.snapshot();
        if body.state_rev.as_deref() != Some(s.state_rev.to_string().as_str()) {
            return failure("drc_context_changed");
        }
        Some(super::dto::FocusContext {
            bbox_dbu: s.state.viewport.bbox,
            dbu: v.controller.model.dbu,
            pixels: [s.state.viewport.width, s.state.viewport.height],
        })
    } else {
        None
    };
    let selected = if let Some((revision, check)) = selection_filter {
        let Some(v) = gate.active_view().filter(|v| v.id == body.view_id) else {
            return failure("drc_context_changed");
        };
        let ids = v.drc_panel.lock().unwrap().groups.ids(revision, check);
        match ids {
            Ok(ids) => Some(ids),
            Err(e) => return failure(e),
        }
    } else {
        None
    };
    let mut ticket = match drc.submit_filters(body.body, context, selected) {
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
    if let Some((revision, _)) = selection_filter {
        let Some(v) = gate.active_view().filter(|v| v.id == body.view_id) else {
            return failure("drc_context_changed");
        };
        let check = v.drc_panel.lock().unwrap().groups.check(revision);
        if let Err(e) = check {
            return failure(e);
        }
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
