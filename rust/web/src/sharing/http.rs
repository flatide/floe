//! Guest routes are an explicit allowlist and do not
//! dispatch owner handlers, native work, catalog/DRC reads or filesystem IO.
use super::*;
use crate::{
    origin,
    transport::{self, Gate, Gateway, BUNDLE},
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::json;

pub(crate) fn routes() -> Router<Gate> {
    Router::new()
        .route("/api/v1/shares", get(list).post(issue))
        .route("/api/v1/shares/{id}", delete(revoke))
        .route("/api/v1/guest/{id}/exchange", post(exchange))
        .route("/api/v1/guest/{id}/session", get(session).delete(logout))
        .route("/api/v1/guest/{id}/events", get(super::stream::upgrade))
}
fn failure(error: Failure) -> StatusCode {
    match error {
        Failure::Unauthorized => StatusCode::UNAUTHORIZED,
        Failure::Full => StatusCode::TOO_MANY_REQUESTS,
        Failure::Entropy => StatusCode::SERVICE_UNAVAILABLE,
    }
}
fn current_scope(gate: &Gateway) -> Option<Scope> {
    let view = gate.active_view()?;
    let snapshot = view.controller.snapshot();
    if matches!(
        snapshot.phase,
        floe_app_core::view::Phase::Closed | floe_app_core::view::Phase::Failed
    ) {
        return None;
    }
    Some(Scope {
        view_id: view.id.clone(),
        dataset_revision: view.controller.model.dataset_revision,
        layers: snapshot.state.layers,
    })
}
pub(super) fn with_shares<T>(
    gate: &Gateway,
    op: impl FnOnce(&mut Shares, Instant) -> Result<T, StatusCode>,
) -> Result<T, StatusCode> {
    let sharing = gate.shares.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let now = Instant::now();
    let scope = current_scope(gate);
    let mut shares = sharing
        .lock()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    shares.maintain(now, |owner, s| {
        gate.alive(owner) && scope.as_ref() == Some(s)
    });
    op(&mut shares, now)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Issue {
    view_id: String,
    base_state_rev: String,
    mode: Mode,
    approve: bool,
}
async fn issue(
    State(gate): State<Gate>,
    headers: HeaderMap,
    body: Result<Json<Issue>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&gate, &headers) {
        Ok(id) => id,
        Err(e) => return transport::error(e),
    };
    if gate.shares.is_none() {
        return transport::error(StatusCode::NOT_FOUND);
    }
    let Ok(Json(body)) = body else {
        return transport::error(StatusCode::BAD_REQUEST);
    };
    if !body.approve {
        return transport::error(StatusCode::FORBIDDEN);
    }
    let Some(view) = gate.active_view().filter(|v| v.id == body.view_id) else {
        return transport::error(StatusCode::CONFLICT);
    };
    let snapshot = view.controller.snapshot();
    if crate::view::counter(&body.base_state_rev) != Ok(snapshot.state_rev) {
        return transport::error(StatusCode::CONFLICT);
    }
    let scope = Scope {
        view_id: view.id.clone(),
        dataset_revision: view.controller.model.dataset_revision,
        layers: snapshot.state.layers,
    };
    let result = with_shares(&gate, |shares, now| {
        if !gate.alive(&owner) || current_scope(&gate).as_ref() != Some(&scope) {
            return Err(StatusCode::CONFLICT);
        }
        shares.issue(owner, scope, body.mode, now).map_err(failure)
    });
    match result {
        Ok(invite) => Json(json!({"share_id":invite.id,"invite":invite.token.expose(),
            "mode":invite.mode.name(),"invite_seconds":INVITE_TTL.as_secs(),
            "session_seconds":SESSION_TTL.as_secs(),"delivery":delivery(invite.mode)}))
        .into_response(),
        Err(e) => transport::error(e),
    }
}
async fn list(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    let owner = match transport::http_session(&gate, &headers) {
        Ok(id) => id,
        Err(e) => return transport::error(e),
    };
    match with_shares(&gate, |shares, _| {
        Ok(shares
            .entries
            .iter()
            .filter(|e| e.owner == owner)
            .map(|e| json!({"share_id":e.id,"mode":e.mode.name()}))
            .collect::<Vec<_>>())
    }) {
        Ok(entries) => Json(json!({"shares":entries,"max_grants":MAX_GRANTS})).into_response(),
        Err(e) => transport::error(e),
    }
}
async fn revoke(State(gate): State<Gate>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let owner = match transport::http_session(&gate, &headers) {
        Ok(id) => id,
        Err(e) => return transport::error(e),
    };
    match with_shares(&gate, |shares, _| Ok(shares.revoke_owner(&owner, &id))) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => transport::error(StatusCode::NOT_FOUND),
        Err(e) => transport::error(e),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exchange {
    invite: String,
    protocol: u32,
    bundle: String,
}
pub(super) fn cookie_name(id: &str) -> String {
    format!("floe_guest_{id}")
}
fn set_cookie(response: &mut Response, id: &str, value: &str, seconds: u64) {
    // id/value are validated server-generated hex; never accept a user path.
    let cookie = format!(
        "{}={value}; Path=/api/v1/guest/{id}; HttpOnly; SameSite=Strict; Max-Age={seconds}",
        cookie_name(id)
    );
    response.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&cookie).expect("hex guest cookie"),
    );
}
async fn exchange(
    State(gate): State<Gate>,
    Path(id): Path<String>,
    body: Result<Json<Exchange>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if gate.shares.is_none() {
        return transport::error(StatusCode::NOT_FOUND);
    }
    let Ok(Json(body)) = body else {
        return transport::error(StatusCode::BAD_REQUEST);
    };
    if body.protocol != 1 || body.bundle != BUNDLE {
        return transport::error(StatusCode::UPGRADE_REQUIRED);
    }
    match with_shares(&gate, |shares, now| {
        shares.exchange(&id, &body.invite, now).map_err(failure)
    }) {
        Ok(c) => {
            let mut response = Json(
                json!({"protocol":1,"bundle":BUNDLE,"session_id":c.id.as_str(),
                "csrf":c.csrf.expose(),"share_id":id,"session_seconds":SESSION_TTL.as_secs()}),
            )
            .into_response();
            set_cookie(
                &mut response,
                &id,
                &c.cookie.expose(),
                SESSION_TTL.as_secs(),
            );
            response
        }
        Err(e) => transport::error(e),
    }
}
fn authenticate(
    shares: &Shares,
    headers: &HeaderMap,
    id: &str,
    now: Instant,
) -> Result<Guest, StatusCode> {
    let csrf = origin::single(headers, "x-floe-guest-csrf").ok_or(StatusCode::UNAUTHORIZED)?;
    let cookie = origin::cookie(headers, &cookie_name(id)).ok_or(StatusCode::UNAUTHORIZED)?;
    shares.authenticate(id, cookie, csrf, now).map_err(failure)
}
async fn session(State(gate): State<Gate>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    match with_shares(&gate, |shares, now| {
        authenticate(shares, &headers, &id, now)
    }) {
        Ok(guest) => Json(json!({"share_id":guest.share_id,"mode":guest.mode.name(),
            "read_only":true,"delivery":delivery(guest.mode)}))
        .into_response(),
        Err(e) => transport::error(e),
    }
}
fn delivery(mode: Mode) -> &'static str {
    match mode {
        Mode::Follow => "follow_frames",
        Mode::Explore => "not_connected",
    }
}
async fn logout(State(gate): State<Gate>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    match with_shares(&gate, |shares, now| {
        let guest = authenticate(shares, &headers, &id, now)?;
        shares.logout(&guest, now);
        Ok(())
    }) {
        Ok(()) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            set_cookie(&mut response, &id, "", 0);
            response
        }
        Err(e) => transport::error(e),
    }
}
