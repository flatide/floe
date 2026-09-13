//! Owner-only settings text, never a path or layout upload. Preparation is
//! read-only; the existing one-use view.apply token performs the atomic CAS.
use crate::{
    transport::{self, Gate},
    view,
};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Extension, Path, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use floe_app_core::{layerprops, view::Patch};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::OwnedSemaphorePermit;

pub(crate) fn is_import(method: &Method, path: &str) -> bool {
    let p: Vec<_> = path.split('/').collect();
    *method == Method::POST
        && p.len() == 8
        && p[1..4] == ["api", "v1", "views"]
        && p[5] == "settings"
        && matches!(p[7], "calibre" | "native")
        && view::counter(p[6]).is_ok()
        && p[4].len() == 64
        && p[4].bytes().all(|c| c.is_ascii_hexdigit())
}
pub(crate) fn routes() -> Router<Gate> {
    Router::new()
        .route(
            "/api/v1/views/{id}/settings/{rev}/{format}",
            get(export).post(prepare),
        )
        .layer(DefaultBodyLimit::max(layerprops::MAX_BYTES))
}
fn fail(code: &'static str, status: StatusCode) -> Response {
    (status, Json(json!({"error":code}))).into_response()
}
fn context(
    gate: &Gate,
    headers: &HeaderMap,
    id: &str,
    rev: &str,
    format: &str,
) -> Result<(Arc<crate::transport::Attachment>, u64), (StatusCode, &'static str)> {
    transport::http_session(gate, headers).map_err(|status| (status, "unauthorized"))?;
    if !matches!(format, "calibre" | "native") {
        return Err((StatusCode::BAD_REQUEST, "invalid_request"));
    }
    let rev = view::counter(rev).map_err(|_| (StatusCode::BAD_REQUEST, "invalid_request"))?;
    let v = gate
        .active_view()
        .filter(|v| v.id == id)
        .ok_or((StatusCode::NOT_FOUND, "view_unavailable"))?;
    if v.controller.snapshot().state_rev != rev {
        return Err((StatusCode::CONFLICT, "stale_state"));
    }
    Ok((v, rev))
}
async fn prepare(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, rev, format)): Path<(String, String, String)>,
    permit: Option<Extension<Arc<OwnedSemaphorePermit>>>,
    body: Bytes,
) -> Response {
    let (v, rev) = match context(&gate, &headers, &id, &rev, &format) {
        Ok(v) => v,
        Err((status, code)) => return fail(code, status),
    };
    if crate::origin::single(&headers, header::CONTENT_TYPE.as_str())
        != Some("text/plain; charset=utf-8")
        || permit.is_none()
    {
        return fail("invalid_request", StatusCode::BAD_REQUEST);
    }
    let login = match transport::http_session(&gate, &headers) {
        Ok(id) => id,
        Err(e) => return transport::error(e),
    };
    let stamp = match v.prepared.lock().unwrap().begin(rev) {
        Ok(s) => s,
        Err(e) => return fail(e, StatusCode::CONFLICT),
    };
    let state = v.controller.snapshot().state;
    let model = Arc::clone(&v.controller.model);
    let output = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let text = std::str::from_utf8(&body)
            .map_err(|_| floe_app_core::Error::input("settings must be UTF-8"))?;
        let mut patch = Patch::default();
        let (rows, malformed) = if format == "calibre" {
            let doc = layerprops::parse(text)?;
            let counts = (doc.rows.len(), doc.malformed);
            patch.properties = Some(doc);
            counts
        } else {
            let doc = layerprops::parse_settings(text)?;
            let count = doc.rows.len();
            patch.settings = Some(doc);
            (count, 0)
        };
        let prepared_layers = state.prepare_layers(&model, patch)?;
        Ok::<_, floe_app_core::Error>((
            Patch {
                prepared_layers: Some(prepared_layers),
                ..Default::default()
            },
            rows,
            malformed,
        ))
    })
    .await;
    if !gate.alive(&login)
        || gate
            .active_view()
            .is_none_or(|active| !Arc::ptr_eq(&active, &v))
    {
        return transport::error(StatusCode::GONE);
    }
    if v.controller.snapshot().state_rev != rev {
        return fail("stale_state", StatusCode::CONFLICT);
    }
    match output {
        Ok(Ok((patch, rows, malformed))) => match v.prepared.lock().unwrap().finish(stamp, patch) {
            Ok(token) => Json(json!({"view_id":id,"state_rev":rev.to_string(),"prepared_token":token,"rows":rows,"malformed":malformed})).into_response(),
            Err(code) => fail(code, StatusCode::CONFLICT),
        },
        Ok(Err(e)) => fail(view::safe_error(e.kind), StatusCode::BAD_REQUEST),
        Err(_) => fail("settings_failed", StatusCode::INTERNAL_SERVER_ERROR),
    }
}
async fn export(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, rev, format)): Path<(String, String, String)>,
) -> Response {
    let (v, rev) = match context(&gate, &headers, &id, &rev, &format) {
        Ok(v) => v,
        Err((status, code)) => return fail(code, status),
    };
    let login = match transport::http_session(&gate, &headers) {
        Ok(id) => id,
        Err(e) => return transport::error(e),
    };
    let permit = match Arc::clone(&gate.settings_ops).try_acquire_owned() {
        Ok(p) => p,
        Err(_) => return transport::error(StatusCode::TOO_MANY_REQUESTS),
    };
    let state = v.controller.snapshot().state;
    let model = Arc::clone(&v.controller.model);
    let output = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if format == "calibre" {
            state.layerprops(&model)
        } else {
            state.settings(&model).text()
        }
    })
    .await;
    if !gate.alive(&login)
        || gate
            .active_view()
            .is_none_or(|active| !Arc::ptr_eq(&active, &v))
    {
        return transport::error(StatusCode::GONE);
    }
    if v.controller.snapshot().state_rev != rev {
        return fail("stale_state", StatusCode::CONFLICT);
    }
    match output {
        Ok(Ok(text)) => {
            ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
        }
        Ok(Err(e)) => fail(view::safe_error(e.kind), StatusCode::BAD_REQUEST),
        Err(_) => fail("settings_failed", StatusCode::INTERNAL_SERVER_ERROR),
    }
}
