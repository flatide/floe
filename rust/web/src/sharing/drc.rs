//! Fixed DRC grant + private guest panel. Never dispatch an owner HTTP route.
use super::{http, Lease, Mode};
use crate::{
    drc::{self, shared},
    transport::{self, Gate, Gateway},
};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use floe_app_core::view::{Phase, ViewController};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    id: String,
    revision: String,
    source: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Approval {
    id: String,
    revision: String,
    approve: bool,
}
impl Approval {
    pub fn capture(self, gate: &Gateway, source: &str) -> Result<Binding, StatusCode> {
        if !self.approve {
            return Err(StatusCode::FORBIDDEN);
        }
        let registry = gate.drc.as_ref().ok_or(StatusCode::CONFLICT)?;
        let reader = registry.current(&self.id).ok_or(StatusCode::CONFLICT)?;
        reader.shared_catalog().map_err(|_| StatusCode::CONFLICT)?;
        if reader.source_id != source || !registry.is_revision(&reader, &self.revision) {
            return Err(StatusCode::CONFLICT);
        }
        Ok(Binding {
            id: self.id,
            revision: self.revision,
            source: source.into(),
        })
    }
}
impl Binding {
    pub fn describe(&self) -> Value {
        json!({"id":self.id,"revision":self.revision})
    }
    pub fn valid(&self, gate: &Gateway) -> bool {
        let Some(registry) = &gate.drc else {
            return false;
        };
        let Some(reader) = registry.current(&self.id) else {
            return false;
        };
        reader.source_id == self.source
            && gate
                .active_view()
                .is_some_and(|v| v.source_id == self.source)
            && registry.is_revision(&reader, &self.revision)
    }
}
pub(super) fn routes() -> Router<Gate> {
    Router::new()
        .route("/api/v1/guest/{id}/drc", get(metadata))
        .route("/api/v1/guest/{id}/drc/read", post(read))
        .route(
            "/api/v1/guest/{id}/drc/panel",
            get(panel_get).post(panel_set),
        )
        .route(
            "/api/v1/guest/{id}/drc/selection",
            get(selection_get).post(selection_set),
        )
}
struct Target {
    lease: Lease,
    view: String,
    controller: Arc<ViewController>,
    reader: Arc<drc::Service>,
}
impl Target {
    fn binding(&self) -> &Binding {
        self.lease.scope.drc.as_ref().unwrap()
    }
    fn identity(&self, view: &str, revision: &str) -> bool {
        view == self.view && revision == self.binding().revision
    }
}
fn target(gate: &Gateway, headers: &HeaderMap, id: &str) -> Result<Target, StatusCode> {
    http::with_shares(gate, |shares, now| {
        let guest = http::authenticate(shares, headers, id, now)?;
        let lease = shares
            .lease(guest, now)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let binding = lease.scope.drc.as_ref().ok_or(StatusCode::FORBIDDEN)?;
        let reader = gate
            .drc
            .as_ref()
            .and_then(|r| r.current(&binding.id))
            .ok_or(StatusCode::CONFLICT)?;
        let (view, controller) = if lease.guest.mode == Mode::Explore {
            // HTTP reads do not implicitly reserve/reopen a native worker.
            let v = shares
                .entries
                .iter()
                .find(|e| e.id == id)
                .and_then(|e| e.explore.as_ref())
                .ok_or(StatusCode::CONFLICT)?;
            (v.id.clone(), Arc::clone(&v.controller))
        } else {
            let v = gate
                .active_view()
                .filter(|v| v.id == lease.scope.view_id)
                .ok_or(StatusCode::CONFLICT)?;
            (v.id.clone(), Arc::clone(&v.controller))
        };
        shared::bind(&mut lease.panel.lock().unwrap(), &binding.revision);
        Ok(Target {
            lease,
            view,
            controller,
            reader,
        })
    })
}
fn valid(gate: &Gateway, t: &Target) -> bool {
    http::with_shares(gate, |s, n| {
        Ok(!*gate.stopping.borrow() && target_current(s, t, n))
    })
    .unwrap_or(false)
}
fn target_current(s: &super::Shares, t: &Target, now: std::time::Instant) -> bool {
    s.valid(&t.lease, now)
        && (t.lease.guest.mode == Mode::Follow
            || s.entries.iter().any(|e| {
                e.id == t.lease.guest.share_id && e.explore.as_ref().is_some_and(|v| v.id == t.view)
            }))
}
// Lock order: shares -> registry -> read revision -> panel/controller. Actor
// submission is outside the revision guard (enqueue captures its own fence).
fn fenced<T>(
    gate: &Gateway,
    t: &Target,
    state: Option<u64>,
    f: impl FnOnce() -> Result<T, drc::Failure>,
) -> Result<T, StatusCode> {
    http::with_shares(gate, |shares, now| {
        if *gate.stopping.borrow() || !target_current(shares, t, now) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        gate.drc
            .as_ref()
            .ok_or(StatusCode::CONFLICT)?
            .with_revision(&t.reader, &t.binding().revision, || {
                let s = t.controller.snapshot();
                if matches!(s.phase, Phase::Closed | Phase::Failed)
                    || state.is_some_and(|n| n != s.state_rev)
                {
                    return Err("drc_context_changed");
                }
                f()
            })
            .map_err(status)
    })
}
fn status(code: drc::Failure) -> StatusCode {
    match code {
        "forbidden" => StatusCode::FORBIDDEN,
        "drc_busy" => StatusCode::TOO_MANY_REQUESTS,
        "drc_context_changed" | "drc_panel_conflict" | "drc_selection_conflict" => {
            StatusCode::CONFLICT
        }
        "drc_read_limit" | "drc_selection_limit" => StatusCode::PAYLOAD_TOO_LARGE,
        "invalid_drc_request" => StatusCode::BAD_REQUEST,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}
fn response(gate: Gate, t: Target, state: Option<u64>, bytes: Vec<u8>) -> Response {
    let length = bytes.len();
    // Recheck at the actual body poll, not just before returning the handler.
    // Once handed to hyper/OS, a bounded chunk cannot be recalled.
    let body = guarded_body(bytes, move || fenced(&gate, &t, state, || Ok(())));
    let mut response = ([("content-type", "application/json")], body).into_response();
    response
        .headers_mut()
        .insert("content-length", length.to_string().parse().unwrap());
    response
}
fn guarded_body(
    bytes: Vec<u8>,
    authorize: impl FnOnce() -> Result<(), StatusCode> + Send + 'static,
) -> Body {
    Body::from_stream(futures_util::stream::once(async move {
        authorize()
            .map(|()| bytes::Bytes::from(bytes))
            .map_err(|_| std::io::Error::other("shared DRC response revoked"))
    }))
}
fn encode(t: &Target, value: Value) -> Result<Vec<u8>, StatusCode> {
    let bytes =
        serde_json::to_vec(&json!({"view_id":t.view,"revision":t.binding().revision,"data":value}))
            .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    if bytes.len() > drc::RESPONSE_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    Ok(bytes)
}
async fn wait(gate: &Gate, t: &Target, mut ticket: drc::Ticket) -> Result<Vec<u8>, StatusCode> {
    let mut revoked = t.lease.revoked.clone();
    let mut stop = gate.stopping.subscribe();
    let deadline = tokio::time::sleep(Duration::from_secs(30));
    tokio::pin!(deadline);
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    loop {
        tokio::select! { biased;
            _=revoked.changed()=>return Err(StatusCode::UNAUTHORIZED),
            _=stop.changed()=>return Err(StatusCode::UNAUTHORIZED),
            _=&mut deadline=>return Err(StatusCode::REQUEST_TIMEOUT),
            _=tick.tick()=>if !valid(gate,t) { return Err(StatusCode::UNAUTHORIZED); },
            result=ticket.result()=>return result.map_err(status),
        }
    }
}
fn permit(gate: &Gateway) -> Result<tokio::sync::OwnedSemaphorePermit, StatusCode> {
    Arc::clone(
        &gate
            .share_transport
            .as_ref()
            .ok_or(StatusCode::NOT_FOUND)?
            .drc_work,
    )
    .try_acquire_owned()
    .map_err(|_| StatusCode::TOO_MANY_REQUESTS)
}
fn admit(
    gate: &Gateway,
    t: &Target,
    submit: impl FnOnce(tokio::sync::OwnedSemaphorePermit) -> Result<drc::Ticket, drc::Failure>,
) -> Result<drc::Ticket, StatusCode> {
    let permit = permit(gate)?;
    http::with_shares(gate, |s, n| {
        if *gate.stopping.borrow() || !target_current(s, t, n) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        gate.drc
            .as_ref()
            .ok_or(StatusCode::CONFLICT)?
            .with_current(&t.reader, || submit(permit))
            .map_err(status)
    })
}
async fn metadata(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let result = (|| {
        let t = target(&gate, &headers, &id)?;
        // Actor metadata lock is not taken inside the read-revision lock.
        let value = t.reader.shared_catalog().map_err(status)?;
        fenced(&gate, &t, None, || Ok(()))?;
        let bytes = encode(&t, value)?;
        Ok::<_, StatusCode>((t, bytes))
    })();
    match result {
        Ok((t, b)) => response(gate, t, None, b),
        Err(e) => transport::error(e),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    view_id: String,
    revision: String,
    state_rev: Option<String>,
    body: shared::Read,
}
async fn read(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<Read>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let t = match target(&gate, &headers, &id) {
        Ok(t) => t,
        Err(e) => return transport::error(e),
    };
    let Ok(Json(body)) = body else {
        return transport::error(StatusCode::BAD_REQUEST);
    };
    let result = async {
        if !t.identity(&body.view_id, &body.revision) {
            return Err(StatusCode::CONFLICT);
        };
        let state = body
            .state_rev
            .as_deref()
            .map(crate::view::counter)
            .transpose()
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        if body.body.needs_view() && state.is_none() {
            return Err(StatusCode::BAD_REQUEST);
        };
        let selected = body.body.selected().map_err(status)?;
        fenced(&gate, &t, state, || {
            shared::check_selection(&t.lease.panel.lock().unwrap(), selected)
        })?;
        let ticket = admit(&gate, &t, |permit| {
            body.body.submit(
                &t.reader,
                &t.binding().revision,
                &t.controller.snapshot(),
                t.controller.model.dbu,
                &t.lease.panel,
                permit,
            )
        })?;
        let bytes = wait(&gate, &t, ticket).await?;
        let bytes = fenced(&gate, &t, state, || {
            shared::check_selection(&t.lease.panel.lock().unwrap(), selected)?;
            shared::project(&bytes)
        })?;
        let value = serde_json::from_slice(&bytes).map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        Ok::<_, StatusCode>((state, encode(&t, value)?))
    }
    .await;
    match result {
        Ok((state, b)) => response(gate, t, state, b),
        Err(e) => transport::error(e),
    }
}
async fn panel_get(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    snapshot(gate, headers, id, false)
}
async fn selection_get(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    snapshot(gate, headers, id, true)
}
fn snapshot(gate: Gate, headers: HeaderMap, id: String, selection: bool) -> Response {
    let result = (|| {
        let t = target(&gate, &headers, &id)?;
        let v = fenced(&gate, &t, None, || {
            let p = t.lease.panel.lock().unwrap();
            Ok(if selection {
                shared::selection(&p)
            } else {
                p.snapshot()
            })
        })?;
        let b = encode(&t, v)?;
        Ok::<_, StatusCode>((t, b))
    })();
    match result {
        Ok((t, b)) => response(gate, t, None, b),
        Err(e) => transport::error(e),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PanelSet {
    view_id: String,
    revision: String,
    base_panel_rev: String,
    body: shared::PanelData,
}
async fn panel_set(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<PanelSet>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let t = match target(&gate, &headers, &id) {
        Ok(t) => t,
        Err(e) => return transport::error(e),
    };
    let Ok(Json(body)) = body else {
        return transport::error(StatusCode::BAD_REQUEST);
    };
    let result = async {
        if !t.identity(&body.view_id, &body.revision) {
            return Err(StatusCode::CONFLICT);
        };
        let base =
            crate::view::counter(&body.base_panel_rev).map_err(|_| StatusCode::BAD_REQUEST)?;
        fenced(&gate, &t, None, || {
            body.body.check(
                &t.lease.panel.lock().unwrap(),
                base,
                t.controller.snapshot().state_rev,
            )
        })?;
        let ticket = admit(&gate, &t, |permit| {
            body.body.submit(&t.reader, &t.binding().revision, permit)
        })?;
        wait(&gate, &t, ticket).await?;
        let value = fenced(&gate, &t, None, || {
            body.body.apply(&mut t.lease.panel.lock().unwrap(), base)
        })?;
        encode(&t, value)
    }
    .await;
    match result {
        Ok(b) => response(gate, t, None, b),
        Err(e) => transport::error(e),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionSet {
    view_id: String,
    revision: String,
    base_selection_rev: String,
    state_rev: Option<String>,
    body: shared::SelectionEdit,
}
async fn selection_set(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<SelectionSet>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let t = match target(&gate, &headers, &id) {
        Ok(t) => t,
        Err(e) => return transport::error(e),
    };
    let Ok(Json(body)) = body else {
        return transport::error(StatusCode::BAD_REQUEST);
    };
    let result = async {
        if !t.identity(&body.view_id, &body.revision) {
            return Err(StatusCode::CONFLICT);
        };
        let base =
            crate::view::counter(&body.base_selection_rev).map_err(|_| StatusCode::BAD_REQUEST)?;
        let edit = body.body.prepare().map_err(status)?;
        let state = body
            .state_rev
            .as_deref()
            .map(crate::view::counter)
            .transpose()
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        if edit.needs_view() && state.is_none() {
            return Err(StatusCode::BAD_REQUEST);
        }
        fenced(&gate, &t, state, || {
            shared::selection_base(&t.lease.panel.lock().unwrap(), base)
        })?;
        let ticket = admit(&gate, &t, |permit| {
            edit.submit(&t.reader, &t.binding().revision, permit)
        })?;
        let bytes = wait(&gate, &t, ticket).await?;
        let value = fenced(&gate, &t, state, || {
            edit.apply(&mut t.lease.panel.lock().unwrap(), base, &bytes)
        })?;
        Ok::<_, StatusCode>((state, encode(&t, value)?))
    }
    .await;
    match result {
        Ok((state, b)) => response(gate, t, state, b),
        Err(e) => transport::error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    #[tokio::test]
    async fn body_authorizes_at_poll_not_when_handler_returns() {
        let allowed = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(AtomicUsize::new(0));
        let flag = Arc::clone(&allowed);
        let counted = Arc::clone(&calls);
        let body = guarded_body(b"private DRC".to_vec(), move || {
            counted.fetch_add(1, Ordering::Relaxed);
            if flag.load(Ordering::Relaxed) {
                Ok(())
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        });
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        allowed.store(false, Ordering::Relaxed);
        assert!(axum::body::to_bytes(body, 100).await.is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            axum::body::to_bytes(guarded_body(b"allowed".to_vec(), || Ok(())), 100)
                .await
                .unwrap()
                .as_ref(),
            b"allowed"
        );
    }
}
