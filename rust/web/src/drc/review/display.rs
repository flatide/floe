//! Read-only saved-note projection. One cached admitted snapshot; no preview
//! tokens, editor mutation, automatic publication or client-chosen paths.
use super::http::{alive, current, fail, read_review, reader, refs, Cancel};
use super::*;
use crate::transport::{self, Gate};
use axum::{
    extract::{Extension, State as HttpState},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

const ITEMS: usize = 512;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    context: Context,
    errors: Vec<crate::drc::dto::CursorDto>,
    focus: Option<crate::drc::dto::CursorDto>,
}

pub(super) struct Cache {
    context: Context,
    review_rev: u64,
    identity: floe_app_core::drc::review::Identity,
    snapshot: managed::Snapshot,
    name: String,
}
impl Service {
    fn display_cache(
        &self,
        context: &Context,
    ) -> std::result::Result<(u64, Option<Arc<Cache>>), Failure> {
        let mut s = self.inner.state.lock().unwrap();
        if s.closed || s.ledger.active().is_some() {
            return Err("drc_busy");
        }
        // An unknown write is not a safe baseline for an automatic display read.
        if s.ledger.snapshot()["history"]
            .as_array()
            .and_then(|a| a.last())
            .is_some_and(|v| v["outcome_unknown"] == true)
        {
            return Err("review_unavailable");
        }
        if s.display
            .as_ref()
            .is_some_and(|c| c.context != *context || c.review_rev != s.review_rev)
        {
            s.display = None;
        }
        Ok((s.review_rev, s.display.clone()))
    }
}
pub(super) async fn read(
    HttpState(g): HttpState<Gate>,
    headers: HeaderMap,
    Extension(body): Extension<Arc<OwnedSemaphorePermit>>,
    request: std::result::Result<Json<Request>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(o) => o,
        Err(e) => return transport::error(e),
    };
    let service = match read_review(&g, store::Kind::Notes) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let Ok(Json(req)) = request else {
        return fail("invalid_drc_request");
    };
    if req.errors.len() > ITEMS || req.errors.is_empty() && req.focus.is_none() {
        return fail("drc_read_limit");
    }
    let mut all = req.errors.clone();
    if let Some(focus) = &req.focus {
        all.push(focus.clone());
    }
    let references = match refs(&all) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let r = match reader(&g, &req.context) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    if r.catalog()["metadata"]["format"] != "ice" {
        return fail("review_pack_required");
    }
    let op = match current(&g, &r, &req.context, || service.begin_read(body, true)) {
        Ok(o) => o,
        Err(e) => return fail(e),
    };
    let mut cancel = Cancel(Some(Arc::clone(&op)));
    let (revision, cached) = match service.display_cache(&req.context) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let hit = cached.is_some();
    let cache = match cached {
        Some(c) => c,
        None => {
            let task = Arc::clone(&op);
            let registered = Arc::clone(&r);
            let context = req.context.clone();
            match tokio::task::spawn_blocking(move || -> Result<_> {
                let store = task.service.open(&registered, &task.stop)?;
                let identity = store.identity();
                let name = store
                    .target()
                    .file_name()
                    .and_then(|v| v.to_str())
                    .ok_or_else(|| floe_app_core::Error::input("invalid note name"))?
                    .to_owned();
                let snapshot = store.snapshot(Arc::clone(&task.stop))?;
                Ok(Arc::new(Cache {
                    context,
                    review_rev: revision,
                    identity,
                    snapshot,
                    name,
                }))
            })
            .await
            {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => return fail(safe(e.kind)),
                Err(_) => return fail("review_unavailable"),
            }
        }
    };
    if !alive(&g, &owner) {
        return transport::error(StatusCode::GONE);
    }
    let mut ticket = match r.enqueue(crate::drc::dto::Command::ReviewTargets {
        identity: cache.identity.clone(),
        refs: references,
    }) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    let bytes = match ticket.result().await {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    #[derive(Deserialize)]
    struct Targets {
        gids: Vec<String>,
    }
    let gids = match serde_json::from_slice::<Targets>(&bytes)
        .ok()
        .and_then(|t| {
            t.gids
                .iter()
                .map(|v| crate::drc::dto::number(v))
                .collect::<std::result::Result<Vec<_>, _>>()
                .ok()
        }) {
        Some(v) if v.len() == all.len() => v,
        _ => return fail("review_unavailable"),
    };
    let task = Arc::clone(&op);
    let captured = Arc::clone(&cache);
    let context = req.context.clone();
    let value = match tokio::task::spawn_blocking(move || -> Result<_> {
        captured.snapshot.check_note_display(&task.stop)?;
        let notes = captured.snapshot.notes().ok_or_else(|| floe_app_core::Error::input("not a note snapshot"))?;
        let rows: Vec<_> = req.errors.iter().zip(&gids).map(|(r, &gid)|
            json!({"check":r.check,"error":r.error,"noted":notes.get(gid).is_some()})).collect();
        let focus = req.focus.map(|r| json!({"check":r.check,"error":r.error,"text":notes.get(*gids.last().unwrap())}));
        let report = captured.snapshot.import_report();
        let value = json!({"kind":"drc_note_display","context":context,"review_rev":revision.to_string(),
            "reviewer":task.service.inner.config.reviewer,"name":captured.name,"rows":rows,"focus":focus,
            "exists":captured.snapshot.exists(),"legacy_unverified":captured.snapshot.legacy_unverified(),"cache_hit":hit,
            "import_report":{"skipped_lines":report.skipped_lines,"invalid_members":report.invalid_members,"reassigned_members":report.reassigned_members}});
        captured.snapshot.check_note_display(&task.stop)?;
        Ok(value)
    }).await {
        Ok(Ok(v)) => v, Ok(Err(e)) => return fail(safe(e.kind)), Err(_) => return fail("review_unavailable"),
    };
    if !alive(&g, &owner) {
        return transport::error(StatusCode::GONE);
    }
    match current(&g, &r, &req.context, || {
        let mut s = service.inner.state.lock().unwrap();
        if s.closed
            || s.serial != op.serial
            || s.review_rev != revision
            || op.stop.load(Ordering::Relaxed) != 0
        {
            return Err("drc_context_changed");
        }
        s.display = Some(cache);
        Ok(())
    }) {
        Ok(()) => {
            cancel.0 = None;
            Json(value).into_response()
        }
        Err(e) => fail(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_refuses_unknown_publication_and_client_selected_authority() {
        let service = super::super::tests::service();
        let context = super::super::tests::request().context;
        assert!(service.display_cache(&context).is_ok());
        {
            let mut s = service.inner.state.lock().unwrap();
            s.ledger.admit(1, "approved".into(), "drc_note").unwrap();
            s.ledger.update(1, json!({"outcome_unknown":true}), true);
        }
        assert!(matches!(
            service.display_cache(&context),
            Err("review_unavailable")
        ));
        let valid = json!({"context":context,"errors":[],"focus":{"check":"0","error":"0"}});
        assert!(serde_json::from_value::<Request>(valid.clone()).is_ok());
        for field in ["path", "reviewer", "gids", "approve", "token"] {
            let mut body = valid.clone();
            body[field] = json!("untrusted");
            assert!(serde_json::from_value::<Request>(body).is_err());
        }
        super::super::tests::stop(&service);
    }
}
