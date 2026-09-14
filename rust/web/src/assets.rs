//! Embedded, content-identified assets only. Never map a URL to the filesystem.
use crate::transport::{self, BUNDLE};
use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
pub(crate) fn routes() -> Router<transport::Gate> {
    Router::new()
        .route("/", get(index))
        .route("/assets/{bundle}/{name}", get(asset))
}
async fn index() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!(concat!(env!("OUT_DIR"), "/index.html")),
    )
        .into_response()
}
async fn asset(Path((bundle, name)): Path<(String, String)>) -> Response {
    if bundle != BUNDLE {
        return transport::error(StatusCode::NOT_FOUND);
    }
    let (mime, body) = match name.as_str() {
        "minimap.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/minimap.js"),
        ),
        "notices.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/notices.js"),
        ),
        "about.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/about.js"),
        ),
        "app.css" => ("text/css; charset=utf-8", include_str!("../ui/app.css")),
        "protocol.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/protocol.js"),
        ),
        "app.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/app.js"),
        ),
        "settings.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/settings.js"),
        ),
        "defaults.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/defaults.js"),
        ),
        "gestures.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/gestures.js"),
        ),
        "query.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/query.js"),
        ),
        "inspect.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/inspect.js"),
        ),
        "measure.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/measure.js"),
        ),
        "clip.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/clip.js"),
        ),
        "snapshot.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/snapshot.js"),
        ),
        "drc.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc.js"),
        ),
        "drc-build.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc-build.js"),
        ),
        "drc-notes.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc-notes.js"),
        ),
        "drc-note-display.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc-note-display.js"),
        ),
        "drc-transfer.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc-transfer.js"),
        ),
        "drc-waives.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc-waives.js"),
        ),
        "rulers.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/rulers.js"),
        ),
        "drc-groups.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc-groups.js"),
        ),
        "panel-state.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/panel-state.js"),
        ),
        _ => return transport::error(StatusCode::NOT_FOUND),
    };
    ([(header::CONTENT_TYPE, mime)], body).into_response()
}
