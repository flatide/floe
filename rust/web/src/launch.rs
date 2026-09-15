//! Trusted launcher proposals. Paths never cross this owner HTTP boundary.
//! A proposal is accepted at most once into the existing operation ledger.
use crate::{
    auth::public_id,
    service::{LevelSelection, OperationDto},
    transport, view,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use floe_app_core::{Error, ErrorKind, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{watch, Semaphore};

pub struct Launches {
    state: Mutex<History>,
    changed: watch::Sender<u64>,
    readers: Semaphore,
}
#[derive(Default)]
struct History {
    revision: u64,
    closed: bool,
    entries: VecDeque<Entry>,
}
struct Entry {
    id: String,
    stop: Arc<AtomicUsize>,
    phase: &'static str,
    request: Option<Value>,
    confirm_levels: bool,
    error: Option<&'static str>,
    receipt: Option<(String, Value)>,
}
impl Entry {
    fn pending(&self) -> bool {
        matches!(self.phase, "preparing" | "ready" | "failed")
    }
    fn json(&self) -> Value {
        json!({"id":self.id,"phase":self.phase,"request":self.request,
            "confirm_levels":self.confirm_levels,"error":self.error})
    }
}
impl Launches {
    pub fn new() -> Arc<Self> {
        let (changed, _) = watch::channel(0);
        Arc::new(Self {
            state: Mutex::new(History::default()),
            changed,
            readers: Semaphore::new(2),
        })
    }
    fn notify(&self, state: &mut History) {
        state.revision = state.revision.saturating_add(1);
        self.changed.send_replace(state.revision);
    }
    /// One outstanding CLI request. Caller enqueues bounded preparation work;
    /// this receipt is not registration/open/frame completion.
    pub fn reserve(&self) -> Result<(String, Arc<AtomicUsize>)> {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(Error::new(ErrorKind::Cancelled, "launcher closed"));
        }
        if state.entries.iter().any(Entry::pending) || state.revision > u64::MAX - 4 {
            return Err(Error::new(
                ErrorKind::Busy,
                "finish or dismiss the pending launcher request",
            ));
        }
        let id = public_id().map_err(|_| Error::new(ErrorKind::Io, "entropy unavailable"))?;
        let stop = Arc::new(AtomicUsize::new(0));
        if state.entries.len() == 16 {
            state.entries.pop_front();
        }
        state.entries.push_back(Entry {
            id: id.clone(),
            stop: Arc::clone(&stop),
            phase: "preparing",
            request: None,
            confirm_levels: false,
            error: None,
            receipt: None,
        });
        self.notify(&mut state);
        Ok((id, stop))
    }
    pub fn ready(&self, id: &str, request: Option<Value>, confirm_levels: bool) -> Result<()> {
        if let Some(value) = &request {
            if value.to_string().len() > 16 * 1024 {
                return Err(Error::input("launcher proposal limit"));
            }
            let operation: OperationDto = serde_json::from_value(value.clone())
                .map_err(|_| Error::input("invalid launcher proposal"))?;
            match operation {
                OperationDto::Open { seq, body, .. } if seq == "1" => {
                    body.core().map_err(Error::input)?;
                }
                _ => return Err(Error::input("launcher proposal must open, never index")),
            }
        } else if confirm_levels {
            return Err(Error::input("present request cannot select levels"));
        }
        let mut state = self.state.lock().unwrap();
        let entry = state
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| Error::input("launch expired"))?;
        if entry.phase != "preparing" || entry.stop.load(Ordering::Relaxed) != 0 {
            return Err(Error::new(ErrorKind::Cancelled, "launch cancelled"));
        }
        entry.request = request;
        entry.confirm_levels = confirm_levels;
        entry.phase = "ready";
        self.notify(&mut state);
        Ok(())
    }
    pub fn failed(&self, id: &str, kind: ErrorKind) {
        let mut state = self.state.lock().unwrap();
        if let Some(entry) = state
            .entries
            .iter_mut()
            .find(|e| e.id == id && e.phase == "preparing")
        {
            entry.phase = "failed";
            entry.error = Some(match kind {
                ErrorKind::Busy => "busy",
                ErrorKind::Cancelled => "cancelled",
                ErrorKind::Cache => "source_changed",
                ErrorKind::Version => "worker_version",
                ErrorKind::InvalidInput | ErrorKind::Unsupported => "invalid_request",
                _ => "io_error",
            });
            self.notify(&mut state);
        }
    }
    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return;
        }
        state.closed = true;
        for entry in &mut state.entries {
            entry.stop.store(1, Ordering::Relaxed);
            if entry.pending() {
                entry.phase = "dismissed";
            }
        }
        self.notify(&mut state);
    }
    fn snapshot(&self) -> Value {
        let state = self.state.lock().unwrap();
        json!({"revision":state.revision.to_string(),"pending":state.entries.iter().find(|e|e.pending()).map(Entry::json)})
    }
    fn submit(
        &self,
        id: &str,
        input: Action,
        service: &crate::service::Service,
    ) -> std::result::Result<Value, &'static str> {
        let signature = serde_json::to_string(&input).map_err(|_| "invalid_request")?;
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err("closed");
        }
        let entry = state
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or("launch_expired")?;
        if let Some((old, receipt)) = &entry.receipt {
            return if *old == signature {
                Ok(receipt.clone())
            } else {
                Err("launch_conflict")
            };
        }
        let receipt = match input {
            Action::Dismiss {} if entry.pending() => {
                entry.stop.store(1, Ordering::Relaxed);
                entry.phase = "dismissed";
                json!({"phase":"dismissed"})
            }
            Action::Present {} if entry.phase == "ready" && entry.request.is_none() => {
                entry.phase = "presented";
                json!({"phase":"presented"})
            }
            Action::Open {
                seq,
                pixels,
                levels,
                view_id,
                state_rev,
            } if entry.phase == "ready" => {
                view::counter(&seq)?;
                if view_id.is_some() != state_rev.is_some() {
                    return Err("invalid_request");
                }
                if let Some(rev) = &state_rev {
                    view::counter(rev)?;
                }
                let mut request = entry.request.clone().ok_or("invalid_request")?;
                request["seq"] = json!(seq);
                request["body"]["pixels"] = json!(pixels);
                request["levels"] = serde_json::to_value(levels).map_err(|_| "invalid_request")?;
                let request = serde_json::from_value(request).map_err(|_| "invalid_request")?;
                let result = service.submit_launch(request, view_id.zip(state_rev))?;
                entry.phase = "submitted";
                json!({"phase":"submitted","operation":result})
            }
            _ => return Err("launch_unavailable"),
        };
        entry.receipt = Some((signature, receipt.clone()));
        self.notify(&mut state);
        Ok(receipt)
    }
}
#[derive(Deserialize, serde::Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Dismiss {},
    Present {},
    Open {
        seq: String,
        pixels: [u32; 2],
        levels: LevelSelection,
        #[serde(default)]
        view_id: Option<String>,
        #[serde(default)]
        state_rev: Option<String>,
    },
}
pub(crate) fn routes() -> Router<transport::Gate> {
    Router::new()
        .route("/api/v1/launch", get(read))
        .route("/api/v1/launch/poll/{after}", get(poll))
        .route("/api/v1/launch/{id}", get(receipt).post(submit))
}
async fn receipt(
    State(gate): State<transport::Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(launch) = &gate.launch else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let state = launch.state.lock().unwrap();
    match state.entries.iter().find(|e|e.id==id) {
        Some(entry)=>Json(json!({"id":entry.id,"phase":entry.phase,"receipt":entry.receipt.as_ref().map(|(_,v)|v)})).into_response(),
        None=>transport::error(StatusCode::NOT_FOUND),
    }
}
async fn read(State(gate): State<transport::Gate>, headers: HeaderMap) -> Response {
    read_after(gate, headers, None).await
}
async fn poll(
    State(gate): State<transport::Gate>,
    headers: HeaderMap,
    Path(after): Path<String>,
) -> Response {
    read_after(gate, headers, Some(after)).await
}
async fn read_after(gate: transport::Gate, headers: HeaderMap, after: Option<String>) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(launch) = &gate.launch else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let Ok(_permit) = launch.readers.try_acquire() else {
        return transport::error(StatusCode::TOO_MANY_REQUESTS);
    };
    if let Some(after) = after {
        let Ok(after) = (if after == "0" {
            Ok(0)
        } else {
            view::counter(&after)
        }) else {
            return transport::error(StatusCode::BAD_REQUEST);
        };
        let mut receiver = launch.changed.subscribe();
        if *receiver.borrow_and_update() == after {
            // Below the transport's five-second handler deadline. Query strings
            // remain forbidden; no relaxation of the global origin guard.
            let _ = tokio::time::timeout(Duration::from_secs(4), receiver.changed()).await;
        }
    }
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    Json(launch.snapshot()).into_response()
}
async fn submit(
    State(gate): State<transport::Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
    input: std::result::Result<Json<Action>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let (Some(launch), Some(service)) = (&gate.launch, &gate.service) else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let input = match input {
        Ok(Json(input)) => input,
        Err(_) => return transport::error(StatusCode::BAD_REQUEST),
    };
    match launch.submit(&id, input, service) {
        Ok(value) => Json(value).into_response(),
        Err(code) => (StatusCode::CONFLICT, Json(json!({"error":code}))).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pending_reservation_and_terminal_shutdown() {
        let l = Launches::new();
        assert_eq!(l.snapshot(), json!({"revision":"0","pending":null}));
        let (id, stop) = l.reserve().unwrap();
        assert_eq!(l.reserve().unwrap_err().kind, ErrorKind::Busy);
        assert_eq!(l.snapshot()["pending"]["phase"], "preparing");
        l.ready(&id, None, false).unwrap();
        assert_eq!(l.snapshot()["pending"]["phase"], "ready");
        assert!(l.ready(&id, None, false).is_err());
        l.failed(&id, ErrorKind::Io);
        assert_eq!(l.snapshot()["pending"]["phase"], "ready");
        l.stop();
        let revision = l.snapshot()["revision"].clone();
        assert_eq!(stop.load(Ordering::Relaxed), 1);
        assert!(l.snapshot()["pending"].is_null());
        assert_eq!(l.reserve().unwrap_err().kind, ErrorKind::Cancelled);
        assert!(l.ready(&id, None, false).is_err());
        l.stop();
        assert_eq!(l.snapshot()["revision"], revision);
    }

    #[test]
    fn proposal_schema_never_grants_index_or_path_authority() {
        let l = Launches::new();
        let (id, _) = l.reserve().unwrap();
        let valid = json!({"kind":"open","seq":"1","source_id":"opaque","mode":"level","body":{}});
        let mut cases = vec![json!({"kind":"index","seq":"1","source_id":"opaque","options":{}})];
        for (key, value) in [
            ("path", json!("/private/design.oas")),
            ("seq", json!("2")),
            ("body", json!({"path":"secret"})),
            ("body", json!({"depth":null})),
            ("source_id", json!("x".repeat(16384))),
        ] {
            let mut bad = valid.clone();
            bad[key] = value;
            cases.push(bad);
        }
        for bad in cases {
            assert!(l.ready(&id, Some(bad.clone()), false).is_err(), "{bad}");
            assert_eq!(l.snapshot()["pending"]["phase"], "preparing");
        }
        assert!(l.ready(&id, None, true).is_err());
        l.ready(&id, Some(valid.clone()), true).unwrap();
        assert_eq!(l.snapshot()["pending"]["request"], valid);
        assert_eq!(l.snapshot()["pending"]["confirm_levels"], true);
        for input in [
            json!({"action":"index"}),
            json!({"action":"dismiss","path":"secret"}),
            json!({"action":"open","seq":"1","pixels":[100,100],"levels":{"mode":"all"},"body":{}}),
        ] {
            assert!(serde_json::from_value::<Action>(input).is_err());
        }
    }

    #[test]
    fn cancelled_preparation_cannot_become_ready() {
        let l = Launches::new();
        let (id, flag) = l.reserve().unwrap();
        flag.store(1, Ordering::Relaxed);
        assert_eq!(
            l.ready(&id, None, false).unwrap_err().kind,
            ErrorKind::Cancelled
        );
        l.failed(&id, ErrorKind::Cancelled);
        assert_eq!(l.snapshot()["pending"]["error"], "cancelled");
        l.failed(&id, ErrorKind::Io);
        assert_eq!(l.snapshot()["pending"]["error"], "cancelled");
    }

    #[test]
    fn concurrent_claims_have_one_winner() {
        let l = Launches::new();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let l = Arc::clone(&l);
                let b = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    b.wait();
                    l.reserve().is_ok()
                })
            })
            .collect();
        assert_eq!(
            threads
                .into_iter()
                .map(|t| usize::from(t.join().unwrap()))
                .sum::<usize>(),
            1
        );
    }

    #[test]
    fn history_and_revision_are_bounded() {
        let l = Launches::new();
        let first = l.reserve().unwrap().0;
        for _ in 0..32 {
            l.state.lock().unwrap().entries.back_mut().unwrap().phase = "dismissed";
            l.reserve().unwrap();
        }
        let mut s = l.state.lock().unwrap();
        assert_eq!(s.entries.len(), 16);
        assert!(!s.entries.iter().any(|e| e.id == first));
        s.entries.back_mut().unwrap().phase = "dismissed";
        s.revision = u64::MAX - 3;
        drop(s);
        assert_eq!(l.reserve().unwrap_err().kind, ErrorKind::Busy);
    }
}
