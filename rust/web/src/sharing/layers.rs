//! A scoped palette read, not an owner catalogue alias. No filesystem access,
//! implicit worker creation or style/settings authority.
use super::{http, Lease, Mode};
use crate::{
    layer_catalog::{LayerCatalog, ScopedPage},
    transport::{self, Gate, Gateway},
    view,
};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use floe_app_core::view::{Phase, Snapshot, ViewController};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub(super) fn routes() -> Router<Gate> {
    Router::new()
        .route("/api/v1/guest/{id}/layers", post(read))
        // 4096 u32 fold-exception pairs fit; metadata output has a separate
        // 256KiB cap. This is not a generic upload or owner route allowance.
        .layer(DefaultBodyLimit::max(128 * 1024))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    view_id: String,
    state_rev: String,
    body: ScopedPage,
}
struct Target {
    lease: Lease,
    view: String,
    controller: Arc<ViewController>,
    rows: Arc<LayerCatalog>,
}
fn target(gate: &Gateway, headers: &HeaderMap, id: &str) -> Result<Target, StatusCode> {
    http::with_shares(gate, |shares, now| {
        let guest = http::authenticate(shares, headers, id, now)?;
        let lease = shares
            .lease(guest, now)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let owner = gate
            .active_view()
            .filter(|v| v.id == lease.scope.view_id)
            .ok_or(StatusCode::CONFLICT)?;
        let (view, controller) = if lease.guest.mode == Mode::Explore {
            let v = shares
                .entries
                .iter()
                .find(|e| e.id == id)
                .and_then(|e| e.explore.as_ref())
                .ok_or(StatusCode::CONFLICT)?;
            (v.id.clone(), Arc::clone(&v.controller))
        } else {
            (owner.id.clone(), Arc::clone(&owner.controller))
        };
        Ok(Target {
            lease,
            view,
            controller,
            rows: Arc::clone(&owner.rows),
        })
    })
}
fn fenced(gate: &Gateway, target: &Target, revision: u64) -> Result<Snapshot, StatusCode> {
    http::with_shares(gate, |shares, now| {
        if *gate.stopping.borrow() || !shares.valid(&target.lease, now) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        if target.lease.guest.mode == Mode::Explore
            && !shares.entries.iter().any(|e| {
                e.id == target.lease.guest.share_id
                    && e.explore.as_ref().is_some_and(|v| v.id == target.view)
            })
        {
            return Err(StatusCode::CONFLICT);
        }
        let s = target.controller.snapshot();
        if s.state_rev != revision || matches!(s.phase, Phase::Closed | Phase::Failed) {
            return Err(StatusCode::CONFLICT);
        }
        Ok(s)
    })
}
fn guarded_body(
    bytes: Vec<u8>,
    authorize: impl FnOnce() -> Result<(), StatusCode> + Send + 'static,
) -> Body {
    Body::from_stream(futures_util::stream::once(async move {
        authorize()
            .map(|()| bytes::Bytes::from(bytes))
            .map_err(|_| std::io::Error::other("shared palette response revoked"))
    }))
}
async fn read(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<Read>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let result = (|| {
        let target = target(&gate, &headers, &id)?;
        let Json(body) = body.map_err(|_| StatusCode::BAD_REQUEST)?;
        if body.view_id != target.view {
            return Err(StatusCode::CONFLICT);
        }
        let revision = view::counter(&body.state_rev).map_err(|_| StatusCode::BAD_REQUEST)?;
        let snapshot = fenced(&gate, &target, revision)?;
        // Catalogue scanning/serialization happens outside the shares lock.
        let data = target
            .rows
            .scoped_page(
                &target.controller.model,
                &snapshot,
                &target.lease.scope.layers,
                body.body,
            )
            .map_err(|_| StatusCode::BAD_REQUEST)?;
        let bytes = serde_json::to_vec(&json!({"view_id":target.view,"data":data}))
            .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        if bytes.len() > view::CONTROL_REPLY_BYTES {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        fenced(&gate, &target, revision)?;
        Ok((target, revision, bytes))
    })();
    match result {
        Err(status) => transport::error(status),
        Ok((target, revision, bytes)) => {
            let len = bytes.len();
            let body = guarded_body(bytes, move || fenced(&gate, &target, revision).map(|_| ()));
            let mut response = ([("content-type", "application/json")], body).into_response();
            response
                .headers_mut()
                .insert("content-length", len.to_string().parse().unwrap());
            response
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn delayed_body_rechecks_authority_before_any_metadata_is_returned() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let live = Arc::new(AtomicBool::new(true));
        let check = Arc::clone(&live);
        let body = guarded_body(b"private names".to_vec(), move || {
            if check.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        });
        live.store(false, Ordering::SeqCst);
        assert!(axum::body::to_bytes(body, 1024).await.is_err());
        let body = guarded_body(b"approved".to_vec(), || Ok(()));
        assert_eq!(
            axum::body::to_bytes(body, 1024).await.unwrap(),
            &b"approved"[..]
        );
    }
}
