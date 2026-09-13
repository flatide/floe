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
        "app.css" => ("text/css; charset=utf-8", include_str!("../ui/app.css")),
        "protocol.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/protocol.js"),
        ),
        "app.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/app.js"),
        ),
        "gestures.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/gestures.js"),
        ),
        "drc.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc.js"),
        ),
        "drc-build.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc-build.js"),
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
