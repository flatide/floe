//! Embedded, content-identified assets only. Never map a URL to the filesystem.
use crate::transport::{self, BUNDLE};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
pub(crate) fn routes() -> Router<transport::Gate> {
    Router::new()
        .route("/", get(index))
        .route("/guest/{id}", get(guest))
        .route("/assets/{bundle}/{name}", get(asset))
}
async fn guest(State(gate): State<transport::Gate>, Path(id): Path<String>) -> Response {
    if gate.shares.is_none()
        || id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return transport::error(StatusCode::NOT_FOUND);
    }
    // Static shell only. No owner bootstrap, catalogue or data is embedded.
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!(concat!(env!("OUT_DIR"), "/guest.html")),
    )
        .into_response()
}
async fn index(State(gate): State<transport::Gate>) -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        if gate.display_only {
            include_str!(concat!(env!("OUT_DIR"), "/display.html"))
        } else {
            include_str!(concat!(env!("OUT_DIR"), "/index.html"))
        },
    )
        .into_response()
}
async fn asset(Path((bundle, name)): Path<(String, String)>) -> Response {
    if bundle != BUNDLE {
        return transport::error(StatusCode::NOT_FOUND);
    }
    let (mime, body) = match name.as_str() {
        "guest-display.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/guest-display.js"),
        ),
        "guest-query-wire.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/guest-query-wire.js"),
        ),
        "guest-tools.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/guest-tools.js"),
        ),
        "guest-layers.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/guest-layers.js"),
        ),
        "guest-drc.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/guest-drc.js"),
        ),
        "drc-geometry.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc-geometry.js"),
        ),
        "sharing.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/sharing.js"),
        ),
        "guest.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/guest.js"),
        ),
        "guest.css" => ("text/css; charset=utf-8", include_str!("../ui/guest.css")),
        "display-input.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/display-input.js"),
        ),
        "display-page.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/display-page.js"),
        ),
        "display-test.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/display-test.js"),
        ),
        "image-decode.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/image-decode.js"),
        ),
        "fill-editor.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/fill-editor.js"),
        ),
        "presets.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/presets.js"),
        ),
        "palette.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/palette.js"),
        ),
        "index-open.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/index-open.js"),
        ),
        "browse.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/browse.js"),
        ),
        "launcher.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/launcher.js"),
        ),
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
        "session-exit.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/session-exit.js"),
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
        "display-dump.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/display-dump.js"),
        ),
        "drc.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc.js"),
        ),
        "drc-build.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/drc-build.js"),
        ),
        "review-save-mode.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/review-save-mode.js"),
        ),
        "hangul.js" => (
            "text/javascript; charset=utf-8",
            include_str!("../ui/hangul.js"),
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
