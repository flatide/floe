//! Read-only build identity. No tool discovery, filesystem or renderer work.
use crate::transport::{self, Gate, BUNDLE};
use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::json;

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
        "notice_scope":"embedded_font_only", "font_name":"Noto Sans Mono",
        "font_notice":include_str!("../../render-core/assets/NotoSansMono-OFL.txt")}),
    )
    .into_response()
}
