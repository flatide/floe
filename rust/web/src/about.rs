//! Read-only identity and registered portable notices; never browser paths.
use crate::transport::{self, Gate, BUNDLE};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

pub enum Notices {
    NotPackaged,
    Unavailable,
    Ready(Arc<floe_notices::Catalog>),
}
impl Notices {
    fn metadata(&self) -> serde_json::Value {
        let (status, catalog) = match self {
            Self::NotPackaged => ("not_packaged", None),
            Self::Unavailable => ("unavailable", None),
            Self::Ready(c) => ("available", Some(c)),
        };
        json!({"status":status,"index_id":catalog.map(|c|c.id()),"files":catalog.map_or(0,|c|c.len()),
            "total_bytes":catalog.map_or(0,|c|c.total_bytes()),"page_bytes":floe_notices::CHUNK_BYTES,"list_size":floe_notices::LIST_SIZE})
    }
}
fn fail(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error":code}))).into_response()
}
type Failure = (StatusCode, &'static str);
fn number(s: &str) -> Result<usize, Failure> {
    if s.is_empty()
        || s.len() > 10
        || s.len() > 1 && s.starts_with('0')
        || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err((StatusCode::BAD_REQUEST, "invalid_notice_page"));
    }
    s.parse()
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid_notice_page"))
}
fn catalog(g: &Gate) -> Result<Arc<floe_notices::Catalog>, Failure> {
    match &g.notices {
        Notices::Ready(c) => Ok(Arc::clone(c)),
        Notices::NotPackaged => Err((StatusCode::NOT_FOUND, "notices_not_packaged")),
        Notices::Unavailable => Err((StatusCode::SERVICE_UNAVAILABLE, "notices_unavailable")),
    }
}
pub(crate) async fn list(
    State(g): State<Gate>,
    headers: HeaderMap,
    Path(start): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let c = match catalog(&g) {
        Ok(c) => c,
        Err(e) => return fail(e.0, e.1),
    };
    let start = match number(&start) {
        Ok(n) => n,
        Err(e) => return fail(e.0, e.1),
    };
    match c.list(start) {
        Ok(v) => Json(v).into_response(),
        Err(_) => fail(StatusCode::BAD_REQUEST, "invalid_notice_page"),
    }
}
struct Cancel(Arc<AtomicUsize>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(1, Ordering::Relaxed);
    }
}
pub(crate) async fn page(
    State(g): State<Gate>,
    headers: HeaderMap,
    Path((id, page)): Path<(String, String)>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(v) => v,
        Err(e) => return transport::error(e),
    };
    let c = match catalog(&g) {
        Ok(c) => c,
        Err(e) => return fail(e.0, e.1),
    };
    let (id, page) = match (number(&id), number(&page)) {
        (Ok(i), Ok(p)) => (i, p),
        (Err(e), _) | (_, Err(e)) => return fail(e.0, e.1),
    };
    let permit = match Arc::clone(&g.notice_readers).try_acquire_owned() {
        Ok(p) => p,
        Err(_) => return fail(StatusCode::TOO_MANY_REQUESTS, "notice_reader_busy"),
    };
    let cancel = Cancel(Arc::new(AtomicUsize::new(0)));
    let flag = Arc::clone(&cancel.0);
    // The existing 5s HTTP guard drops Cancel on timeout/disconnect. The one
    // reader slot stays with the work until its syscall returns. A detached
    // OS thread (not Tokio's blocking pool) cannot hold runtime shutdown open
    // on an uninterruptible filesystem read. At most one exists per gateway.
    let (tx, rx) = tokio::sync::oneshot::channel();
    if std::thread::Builder::new()
        .name("floe-notice-read".into())
        .spawn(move || {
            let _permit = permit;
            let _ = tx.send(c.page(id, page, &flag));
        })
        .is_err()
    {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "notice_reader_unavailable");
    }
    let result = rx.await;
    if !g.alive(&owner) {
        return transport::error(StatusCode::UNAUTHORIZED);
    }
    match result {
        Ok(Ok(v)) => Json(v).into_response(),
        _ => fail(StatusCode::CONFLICT, "notice_changed_or_unreadable"),
    }
}

#[derive(Clone, Serialize)]
pub struct BuildInfo {
    pub app_version: &'static str,
    pub source_revision: &'static str,
    pub target: &'static str,
    pub index_compatibility: &'static str,
    pub renderd_compatibility: &'static str,
}
pub(crate) async fn read(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    Json(
        json!({"product":"floe2-web", "bundle":BUNDLE, "build":gate.build,
        "python_runtime":false, "desktop_acceptance":"unverified",
        "notice_scope":if matches!(&gate.notices,Notices::Ready(_)){"portable_manifest"}else{"embedded_font_only"},
        "notices":gate.notices.metadata(), "font_name":"Noto Sans Mono",
        "font_notice":include_str!("../../render-core/assets/NotoSansMono-OFL.txt")}),
    )
    .into_response()
}
