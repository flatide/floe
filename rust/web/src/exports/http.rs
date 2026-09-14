use super::Submit;
use crate::{
    auth::SessionId,
    origin,
    transport::{self, Gate},
    view,
};
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use std::{
    io,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, OwnedSemaphorePermit};

const CHUNK: usize = 64 * 1024;
const IDLE: Duration = Duration::from_secs(10);
pub(crate) fn routes() -> Router<Gate> {
    Router::new()
        .route("/api/v1/exports", get(status).post(submit))
        .route("/api/v1/exports/{seq}", get(operation))
        .route("/api/v1/exports/{seq}/cancel", post(cancel))
        .route("/api/v1/artifacts/{id}", get(info).delete(release))
        .route(
            "/api/v1/artifacts/{id}/download",
            get(download_get).post(download_post),
        )
}
fn failure(code: &str) -> Response {
    let status = match code {
        "busy" => StatusCode::TOO_MANY_REQUESTS,
        "operation_conflict" | "operation_sequence" | "stale_frame" => StatusCode::CONFLICT,
        "operation_expired" | "export_draft_expired" | "closed" | "artifact_unavailable" => {
            StatusCode::GONE
        }
        "source_unavailable" | "view_unavailable" => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(json!({"error":code}))).into_response()
}
async fn status(State(g): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    match &g.service {
        Some(s) => Json(s.exports().status()).into_response(),
        None => transport::error(StatusCode::NOT_FOUND),
    }
}
async fn submit(
    State(g): State<Gate>,
    headers: HeaderMap,
    body: Result<Json<Submit>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let Some(service) = &g.service else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let Ok(Json(req)) = body else {
        return failure("invalid_request");
    };
    let attached = g.active_view();
    let source = attached.as_ref().and_then(|v| service.source(&v.source_id));
    match service.exports().submit(req, attached, source) {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => failure(e),
    }
}
async fn operation(State(g): State<Gate>, headers: HeaderMap, Path(seq): Path<String>) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let Ok(seq) = view::counter(&seq) else {
        return failure("invalid_request");
    };
    g.service
        .as_ref()
        .and_then(|s| s.exports().operation(seq))
        .map_or_else(|| failure("operation_expired"), |v| Json(v).into_response())
}
async fn cancel(State(g): State<Gate>, headers: HeaderMap, Path(seq): Path<String>) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let Ok(seq) = view::counter(&seq) else {
        return failure("invalid_request");
    };
    let Some(s) = &g.service else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    match s.exports().cancel(seq) {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => failure(e),
    }
}
async fn info(State(g): State<Gate>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let Ok(id) = view::counter(&id) else {
        return failure("invalid_request");
    };
    g.service.as_ref().and_then(|s|s.exports().store().info(id)).map_or_else(||failure("artifact_unavailable"),|i|Json(json!({"id":id.to_string(),"bytes":i.size_bytes.to_string(),"expires_in_ms":i.expires_in_ms.to_string(),"name":format!("floe-clip-{id}.oas")})).into_response())
}
async fn release(State(g): State<Gate>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let Ok(id) = view::counter(&id) else {
        return failure("invalid_request");
    };
    if let Some(s) = &g.service {
        s.exports().store().request_release(id);
    }
    StatusCode::NO_CONTENT.into_response()
}
async fn download_get(
    State(g): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match transport::http_session(&g, &headers) {
        Ok(s) => s,
        Err(e) => return transport::error(e),
    };
    download(g, session, headers, id)
}
/// A native browser download via a same-origin form POST. Its ONE fixed field
/// is lowercase hex, so form encoding is byte-identical: no general form/URL
/// decoder and no credential in URL, referrer, logs or filesystem paths.
async fn download_post(
    State(g): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let token = std::str::from_utf8(&body)
        .ok()
        .and_then(|s| s.strip_prefix("csrf="));
    let valid = token.is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    if !valid
        || origin::single(&headers, "content-type") != Some("application/x-www-form-urlencoded")
    {
        return transport::error(StatusCode::UNAUTHORIZED);
    }
    let session = match transport::download_session(&g, &headers, token.unwrap()) {
        Ok(s) => s,
        Err(e) => return transport::error(e),
    };
    download(g, session, headers, id)
}
struct Charged {
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}
impl AsRef<[u8]> for Charged {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
fn download(g: Gate, session: SessionId, headers: HeaderMap, id: String) -> Response {
    let Ok(id) = view::counter(&id) else {
        return failure("invalid_request");
    };
    if headers.contains_key("range") {
        return transport::error(StatusCode::RANGE_NOT_SATISFIABLE);
    }
    let Some(service) = &g.service else {
        return failure("artifact_unavailable");
    };
    let reader = match service.exports().store().open(id) {
        Ok(r) => r,
        Err(e) => {
            return failure(if e.kind == floe_app_core::ErrorKind::Busy {
                "busy"
            } else {
                "artifact_unavailable"
            })
        }
    };
    stream_download(g, session, reader, format!("floe-clip-{id}.oas"), |_| true)
}
/// Shared bounded download transport. The caller supplies a fixed server
/// filename and a memory-only context fence; never a client-selected path.
pub(crate) fn stream_download(
    g: Gate,
    session: SessionId,
    mut reader: floe_app_core::exports::artifacts::Download,
    name: String,
    valid: impl Fn(&Gate) -> bool + Send + 'static,
) -> Response {
    let size = reader.size_bytes();
    let (tx, rx) = mpsc::channel::<Result<Bytes, io::Error>>(1);
    // At most Store::readers producers. Every live chunk retains its byte
    // permit through Hyper (Bytes::from_owner), not only until channel send.
    tokio::task::spawn_blocking(move || {
        let mut active = Instant::now();
        loop {
            let alive =
                || g.alive(&session) && !*g.stopping.borrow() && reader.is_available() && valid(&g);
            let permit = loop {
                if !alive() || active.elapsed() >= IDLE {
                    return;
                }
                if let Ok(p) = Arc::clone(&g.output_bytes).try_acquire_many_owned(CHUNK as u32) {
                    break p;
                }
                thread::sleep(Duration::from_millis(5));
            };
            let bytes = reader.read_chunk(CHUNK);
            if matches!(&bytes,Ok(b) if b.is_empty()) {
                return;
            }
            let failed = bytes.is_err();
            let mut item = bytes
                .map(|bytes| {
                    Bytes::from_owner(Charged {
                        bytes,
                        _permit: permit,
                    })
                })
                .map_err(|_| io::Error::other("artifact unavailable"));
            loop {
                if !g.alive(&session)
                    || *g.stopping.borrow()
                    || !reader.is_available()
                    || !valid(&g)
                    || active.elapsed() >= IDLE
                {
                    return;
                }
                match tx.try_send(item) {
                    Ok(()) => {
                        active = Instant::now();
                        break;
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => return,
                    Err(mpsc::error::TrySendError::Full(v)) => item = v,
                }
                thread::sleep(Duration::from_millis(5));
            }
            if failed {
                return;
            }
        }
    });
    let stream =
        futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|b| (b, rx)) });
    let mut response = Body::from_stream(stream).into_response();
    let h = response.headers_mut();
    h.insert(
        "content-type",
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(
        "content-length",
        HeaderValue::from_str(&size.to_string()).unwrap(),
    );
    h.insert(
        "content-disposition",
        HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")).unwrap(),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn bytes_hold_credit_through_all_hyper_chunk_clones() {
        let pool = Arc::new(tokio::sync::Semaphore::new(17));
        let permit = Arc::clone(&pool).try_acquire_many_owned(17).unwrap();
        let bytes = Bytes::from_owner(Charged {
            bytes: vec![7; 17],
            _permit: permit,
        });
        let clone = bytes.slice(1..2);
        drop(bytes);
        assert_eq!(pool.available_permits(), 0);
        drop(clone);
        assert_eq!(pool.available_permits(), 17);
    }
}
