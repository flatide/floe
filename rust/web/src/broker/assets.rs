//! Server-only static shell. No owner modules or filesystem URL resolution.
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
fn transport(html: &str, http_test: bool) -> String {
    html.replace("@@HTTP_TEST@@", if http_test { "true" } else { "false" })
}
pub(super) fn demo(http_test: bool) -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        transport(
            include_str!(concat!(env!("OUT_DIR"), "/demo.html")),
            http_test,
        ),
    )
        .into_response()
}
pub(super) fn page(id: &str, http_test: bool) -> Response {
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Contains neither dataset metadata nor a credential; API auth is separate.
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        transport(
            include_str!(concat!(env!("OUT_DIR"), "/server.html")),
            http_test,
        ),
    )
        .into_response()
}
pub(super) fn asset(bundle: &str, name: &str) -> Response {
    if bundle != crate::transport::BUNDLE {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (mime, body) = match name {
        "demo.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/demo.js"),
        ),
        "server.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/server.js"),
        ),
        "protocol.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/protocol.js"),
        ),
        "image-decode.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/image-decode.js"),
        ),
        "gestures.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/gestures.js"),
        ),
        "app.css" => ("text/css; charset=utf-8", include_str!("../../ui/app.css")),
        "viewer.css" => (
            "text/css; charset=utf-8",
            include_str!("../../ui/viewer.css"),
        ),
        "viewer.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/viewer.js"),
        ),
        "server.css" => (
            "text/css; charset=utf-8",
            include_str!("../../ui/server.css"),
        ),
        "minimap.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/minimap.js"),
        ),
        "fill-editor.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/fill-editor.js"),
        ),
        "presets.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/presets.js"),
        ),
        "palette.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/palette.js"),
        ),
        "menubar.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/menubar.js"),
        ),
        "panes.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../../ui/panes.js"),
        ),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, mime)], body).into_response()
}
