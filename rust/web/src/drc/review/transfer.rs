//! Bounded owner-only portable review transfers. Async accepted jobs share the
//! existing review worker; uploads never become review writes before approval.
use super::http::{alive, current, fail, reader, review};
use super::*;
use crate::transport::{self, Gate};
use axum::{
    body::Bytes,
    extract::{Extension, Path, State as HttpState},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use floe_app_core::exports::artifacts::{self, Reservation};
use sha1::{Digest, Sha1};
use std::{collections::BTreeMap, fs::File, io::Read as _, os::unix::fs::FileExt};

const CHUNK: usize = 1024 * 1024;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(600);
pub(super) fn limits() -> artifacts::Limits {
    artifacts::Limits {
        entries: 2,
        artifact_bytes: MAX_BYTES,
        total_bytes: 2 * MAX_BYTES,
        readers: 1,
        ttl: TTL,
    }
}
pub(super) struct Upload {
    file: File,
    snapshot: managed::Snapshot,
    charge: Reservation,
    bytes: u64,
    received: u64,
    expires: Instant,
}
#[derive(Clone)]
pub(super) struct Artifact {
    owner: SessionId,
    reader: Arc<Reader>,
    context: Context,
    review_rev: u64,
    name: String,
}
#[derive(Default)]
pub(super) struct State {
    pub(super) ledger: Ledger,
    pub(super) pending: Option<Task>,
    pub(super) artifacts: BTreeMap<u64, Artifact>,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Action {
    Import,
    Export,
    Prepare,
    Chunk,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    seq: String,
    context: Context,
    action: Action,
    bytes: Option<String>,
    token: Option<String>,
}
pub(super) struct Task {
    gate: Gate,
    owner: SessionId,
    reader: Arc<Reader>,
    request: Request,
    upload: Option<Upload>,
    payload: Option<(u64, Bytes)>,
    // Drop last: retained bodies, files and native borrows unwind before the
    // coordination/body permits can admit replacement work.
    op: Arc<Operation>,
}
/// Never close a large unlinked upload under the reactor/registry mutex. At
/// most the artifact entry limit can be charged here; the owner worker drains.
pub(super) fn retire(s: &mut super::State) {
    if let Some(r) = s.ready.take() {
        if matches!(&r.model, Model::Upload(_) | Model::Prepared(_, Some(_))) {
            s.retired.push(r);
        }
    }
}
fn token(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn storage_error(e: floe_app_core::Error) -> Failure {
    if e.kind == ErrorKind::Busy {
        "drc_busy"
    } else {
        safe(e.kind)
    }
}
impl Request {
    fn validate(&self, payload: &Option<(u64, Bytes)>) -> std::result::Result<(), Failure> {
        self.context.validate()?;
        crate::view::counter(&self.seq)?;
        match (self.action, &self.bytes, &self.token, payload) {
            (Action::Import, Some(n), None, None) => {
                let n = crate::drc::dto::number(n)?;
                if n == 0 || n > MAX_BYTES {
                    Err("drc_read_limit")
                } else {
                    Ok(())
                }
            }
            (Action::Export, None, None, None) => Ok(()),
            (Action::Prepare, None, Some(t), None) if token(t) => Ok(()),
            (Action::Chunk, None, Some(t), Some((_, bytes)))
                if token(t) && !bytes.is_empty() && bytes.len() <= CHUNK =>
            {
                Ok(())
            }
            _ => Err("invalid_drc_request"),
        }
    }
    fn signature(&self, owner: &SessionId, payload: &Option<(u64, Bytes)>) -> String {
        // Change/replay detector, not authentication. Owner cookie/CSRF and
        // the captured context grant authority; bytes never enter the ledger.
        let content = payload
            .as_ref()
            .map(|(at, b)| format!("{at} {} {:x}", b.len(), Sha1::digest(b)));
        format!("{} {self:?} {content:?}", owner.as_str())
    }
}
impl Service {
    fn start_transfer(
        self: &Arc<Self>,
        g: Gate,
        owner: SessionId,
        r: Arc<Reader>,
        req: Request,
        payload: Option<(u64, Bytes)>,
        body: Arc<OwnedSemaphorePermit>,
    ) -> std::result::Result<Value, Failure> {
        req.validate(&payload)?;
        let seq = crate::view::counter(&req.seq)?;
        let signature = req.signature(&owner, &payload);
        let permit = Arc::clone(&self.preparations)
            .try_acquire_owned()
            .map_err(|_| "drc_busy")?;
        let mut s = self.inner.state.lock().unwrap();
        if s.closed {
            return Err("drc_closed");
        }
        if let Some(v) = s.transfer.ledger.replay(seq, &signature)? {
            return Ok(v);
        }
        if s.detached {
            return Err("review_disabled");
        }
        if s.ledger.active().is_some() || !s.retired.is_empty() {
            return Err("drc_busy");
        }
        let uses_upload = matches!(req.action, Action::Prepare | Action::Chunk);
        if uses_upload {
            let ready = s
                .ready
                .as_ref()
                .filter(|v| {
                    v.owner == owner
                        && v.context == req.context
                        && Some(&v.token) == req.token.as_ref()
                        && v.expires > Instant::now()
                        && v.stop.load(Ordering::Relaxed) == 0
                })
                .ok_or("review_expired")?;
            let Model::Upload(u) = &ready.model else {
                return Err("review_expired");
            };
            if req.action == Action::Prepare && u.received != u.bytes {
                return Err("transfer_incomplete");
            }
            if let Some((at, bytes)) = &payload {
                if *at != u.received || bytes.len() as u64 > u.bytes - u.received {
                    return Err("transfer_offset");
                }
            }
        }
        let serial = s.serial.checked_add(1).ok_or("review_limit")?;
        match s
            .transfer
            .ledger
            .admit(seq, signature, "drc_review_transfer")?
        {
            Admission::Replay(v) => return Ok(v),
            Admission::New => (),
        }
        let (upload, stop) = if uses_upload {
            let ready = s.ready.take().unwrap();
            let Model::Upload(u) = ready.model else {
                unreachable!()
            };
            (Some(u), ready.stop)
        } else {
            if req.action == Action::Import {
                retire(&mut s);
                s.display = None;
            }
            (None, Arc::new(AtomicUsize::new(0)))
        };
        s.serial = serial;
        s.preparing = Some((serial, Arc::clone(&stop)));
        let op = Arc::new(Operation {
            service: Arc::clone(self),
            serial,
            stop,
            _permit: permit,
            _body: body,
        });
        s.transfer.pending = Some(Task {
            op,
            gate: g,
            owner,
            reader: r,
            request: req,
            upload,
            payload,
        });
        self.inner.wake.notify_one();
        Ok(s.transfer.ledger.get(seq).unwrap())
    }
    fn transfer_replay(
        &self,
        owner: &SessionId,
        req: &Request,
        payload: &Option<(u64, Bytes)>,
    ) -> std::result::Result<Option<Value>, Failure> {
        self.inner.state.lock().unwrap().transfer.ledger.replay(
            crate::view::counter(&req.seq)?,
            &req.signature(owner, payload),
        )
    }
    fn transfer_cancel(&self, seq: u64) -> std::result::Result<Value, Failure> {
        let mut s = self.inner.state.lock().unwrap();
        let value = s.transfer.ledger.get(seq).ok_or("operation_expired")?;
        if s.transfer.ledger.active() == Some(seq) {
            if let Some((_, stop)) = &s.preparing {
                stop.store(1, Ordering::Relaxed);
            }
        }
        if s.ready
            .as_ref()
            .is_some_and(|r| r.stop.load(Ordering::Relaxed) != 0)
        {
            retire(&mut s);
            self.inner.wake.notify_one();
        }
        Ok(value)
    }
    fn artifact(&self, owner: &SessionId, id: u64) -> std::result::Result<Artifact, Failure> {
        let s = self.inner.state.lock().unwrap();
        s.transfer
            .artifacts
            .get(&id)
            .filter(|a| a.owner == *owner && !s.closed && a.review_rev == s.review_rev)
            .cloned()
            .ok_or("review_expired")
    }
}
fn check(task: &Task) -> std::result::Result<(), Failure> {
    if !alive(&task.gate, &task.owner) || task.op.stop.load(Ordering::Relaxed) != 0 {
        return Err("drc_cancelled");
    }
    current(&task.gate, &task.reader, &task.request.context, || Ok(()))
}
fn capture(task: &Task) -> std::result::Result<managed::Snapshot, Failure> {
    let store = task
        .op
        .service
        .open(&task.reader, &task.op.stop)
        .map_err(|e| safe(e.kind))?;
    task.reader
        .validate_review_identity(store.identity())?
        .blocking_review_result(&task.op.stop)?;
    check(task)?;
    store
        .snapshot(Arc::clone(&task.op.stop))
        .map_err(|e| safe(e.kind))
}
fn finish_upload(task: &Task, upload: Upload) -> std::result::Result<Value, Failure> {
    check(task)?;
    let t = task
        .request
        .token
        .clone()
        .map(Ok)
        .unwrap_or_else(|| crate::auth::public_id().map_err(|_| "review_unavailable"))?;
    let value = json!({"token":t,"bytes":upload.bytes.to_string(),"received":upload.received.to_string(),
        "expires_in_ms":upload.expires.saturating_duration_since(Instant::now()).as_millis().to_string()});
    current(&task.gate, &task.reader, &task.request.context, || {
        let mut s = task.op.service.inner.state.lock().unwrap();
        if s.closed || s.serial != task.op.serial || task.op.stop.load(Ordering::Relaxed) != 0 {
            return Err("review_expired");
        }
        let expires = upload.expires;
        s.ready = Some(Ready {
            reader: Arc::clone(&task.reader),
            owner: task.owner.clone(),
            context: task.request.context.clone(),
            token: t,
            model: Model::Upload(upload),
            stop: Arc::clone(&task.op.stop),
            expires,
        });
        Ok(())
    })?;
    Ok(json!({"upload":value}))
}
fn import_begin(task: &Task) -> std::result::Result<Value, Failure> {
    let snapshot = capture(task)?;
    let bytes = crate::drc::dto::number(task.request.bytes.as_deref().unwrap())?;
    if bytes > snapshot.import_bytes()
        || task.op.service.inner.config.kind == store::Kind::Waives
            && bytes != snapshot.import_bytes()
    {
        return Err("drc_read_limit");
    }
    let id = task
        .reader
        .registration
        .resources
        .next_id()
        .map_err(|e| safe(e.kind))?;
    let charge = task
        .op
        .service
        .inner
        .artifacts
        .reserve(id, Arc::clone(&task.op.stop))
        .map_err(storage_error)?;
    let file = artifacts::temporary_file().map_err(|e| safe(e.kind))?;
    finish_upload(
        task,
        Upload {
            snapshot,
            file,
            charge,
            bytes,
            received: 0,
            expires: Instant::now() + TTL,
        },
    )
}
fn upload_chunk(task: &mut Task) -> std::result::Result<Value, Failure> {
    let mut upload = task.upload.take().unwrap();
    let (offset, bytes) = task.payload.take().unwrap();
    if upload.expires <= Instant::now() {
        return Err("review_expired");
    }
    for (i, b) in bytes.chunks(64 * 1024).enumerate() {
        check(task)?;
        upload
            .file
            .write_all_at(b, offset + (i * 64 * 1024) as u64)
            .map_err(|_| "review_io_error")?;
    }
    upload.received += bytes.len() as u64;
    finish_upload(task, upload)
}
fn import_prepare(task: &mut Task) -> std::result::Result<Value, Failure> {
    let upload = task.upload.take().unwrap();
    if upload.received != upload.bytes {
        return Err("transfer_incomplete");
    }
    if upload.expires <= Instant::now() {
        return Err("review_expired");
    }
    upload.file.sync_all().map_err(|_| "review_io_error")?;
    let exists = upload.snapshot.exists();
    let (draft, charge, details) = match task.op.service.inner.config.kind {
        store::Kind::Waives => {
            let (draft, stats) = upload
                .snapshot
                .prepare_waives_import(upload.file)
                .map_err(|e| safe(e.kind))?;
            (
                draft,
                Some(upload.charge),
                json!({"waived_count":stats.waived.to_string()}),
            )
        }
        store::Kind::Notes => {
            let mut text = String::new();
            (&upload.file)
                .take(upload.bytes + 1)
                .read_to_string(&mut text)
                .map_err(|_| "invalid_drc_request")?;
            let (draft, report) = upload
                .snapshot
                .prepare_notes_import(&text)
                .map_err(|e| safe(e.kind))?;
            let (groups, members) = draft.note_counts().ok_or("review_unavailable")?;
            (
                draft,
                None,
                json!({"groups":groups.to_string(),"members":members.to_string(),"clears":members==0,"import_report":{"skipped_lines":report.skipped_lines,"invalid_members":report.invalid_members,"reassigned_members":report.reassigned_members}}),
            )
        }
    };
    check(task)?;
    let mut value = json!({"kind":task.op.service.kind(),"phase":"prepared","action":"replace_all",
        "name":draft.target().file_name().and_then(|s|s.to_str()),"replaces_existing":exists,"legacy_unverified":draft.legacy_unverified(),
        "scope":"registered_reviewer_entire_review","bytes":upload.bytes.to_string()});
    value
        .as_object_mut()
        .unwrap()
        .extend(details.as_object().unwrap().clone());
    let preview = current(&task.gate, &task.reader, &task.request.context, || {
        task.op.service.finish(
            &task.op,
            task.owner.clone(),
            Arc::clone(&task.reader),
            task.request.context.clone(),
            Model::Prepared(draft, charge),
            value,
        )
    })?;
    Ok(json!({"preview":preview}))
}
fn export(task: &Task) -> std::result::Result<Value, Failure> {
    let snapshot = capture(task)?;
    if snapshot.import_bytes() > MAX_BYTES {
        return Err("drc_read_limit");
    }
    let id = task
        .reader
        .registration
        .resources
        .next_id()
        .map_err(|e| safe(e.kind))?;
    let store = &task.op.service.inner.artifacts;
    let reservation = store
        .reserve(id, Arc::clone(&task.op.stop))
        .map_err(storage_error)?;
    let mut file = artifacts::temporary_file().map_err(|e| safe(e.kind))?;
    let info = snapshot.export(&mut file).map_err(|e| safe(e.kind))?;
    file.sync_all().map_err(|_| "review_io_error")?;
    check(task)?;
    reservation
        .commit(file, info.bytes)
        .map_err(|e| safe(e.kind))?;
    struct Unpublished<'a>(&'a artifacts::Store, u64, bool);
    impl Drop for Unpublished<'_> {
        fn drop(&mut self) {
            if self.2 {
                self.0.request_release(self.1);
            }
        }
    }
    let mut unpublished = Unpublished(store, id, true);
    let name = match task.op.service.inner.config.kind {
        store::Kind::Notes => format!("floe-notes-{id}.fe"),
        store::Kind::Waives => format!("floe-waives-{id}.waive"),
    };
    let rev = current(&task.gate, &task.reader, &task.request.context, || {
        let mut s = task.op.service.inner.state.lock().unwrap();
        if s.closed || s.serial != task.op.serial || task.op.stop.load(Ordering::Relaxed) != 0 {
            return Err("review_expired");
        }
        let review_rev = s.review_rev;
        s.transfer.artifacts.insert(
            id,
            Artifact {
                owner: task.owner.clone(),
                reader: Arc::clone(&task.reader),
                context: task.request.context.clone(),
                review_rev,
                name: name.clone(),
            },
        );
        Ok(review_rev)
    })?;
    unpublished.2 = false;
    let contents = match info.contents {
        store::ExportContents::Notes { groups, members } => {
            json!({"groups":groups.to_string(),"members":members.to_string()})
        }
        store::ExportContents::Waives { waived } => json!({"waived_count":waived.to_string()}),
    };
    Ok(
        json!({"artifact":{"id":id.to_string(),"name":name,"bytes":info.bytes.to_string(),"review_rev":rev.to_string(),"contents":contents,
        "legacy_unverified":info.legacy_unverified,"import_report":{"skipped_lines":info.import_report.skipped_lines,"invalid_members":info.import_report.invalid_members,"reassigned_members":info.import_report.reassigned_members}}}),
    )
}
pub(super) fn run(inner: &Inner, mut task: Task) {
    let seq = crate::view::counter(&task.request.seq).unwrap();
    let context = task.request.context.clone();
    let action = task.request.action;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        check(&task)?;
        match action {
            Action::Import => import_begin(&task),
            Action::Export => export(&task),
            Action::Prepare => import_prepare(&mut task),
            Action::Chunk => upload_chunk(&mut task),
        }
    }))
    .unwrap_or(Err("review_unavailable"));
    // Only preparation/artifacts: no review file was published by this worker.
    let mut value = match result {
        Ok(v) => v,
        Err(e) => json!({"error":e}),
    };
    value["seq"] = json!(seq.to_string());
    value["kind"] = json!("drc_review_transfer");
    value["context"] = json!(context);
    value["action"] = json!(action);
    value["phase"] = json!(if value["error"] == "drc_cancelled" {
        "cancelled"
    } else if value.get("error").is_some() {
        "failed"
    } else {
        "succeeded"
    });
    drop(task); // native file/borrow/body quota stay charged until real unwind
    inner
        .state
        .lock()
        .unwrap()
        .transfer
        .ledger
        .update(seq, value, true);
}

pub(super) fn routes(root: &str) -> Router<Gate> {
    Router::new()
        .route(&format!("{root}/transfer"), get(status).post(submit))
        .route(&format!("{root}/transfer/chunk"), post(chunk))
        .route(&format!("{root}/transfer/{{seq}}"), get(operation))
        .route(&format!("{root}/transfer/{{seq}}/cancel"), post(cancel))
        .route(
            &format!("{root}/artifacts/{{id}}"),
            get(artifact_info).delete(release),
        )
        .route(
            &format!("{root}/artifacts/{{id}}/download"),
            get(download_get).post(download_post),
        )
}
async fn submit(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Extension(body): Extension<Arc<OwnedSemaphorePermit>>,
    req: std::result::Result<Json<Request>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    if let Err(e) = review(&g, kind) {
        return fail(e);
    }
    let Ok(Json(req)) = req else {
        return fail("invalid_drc_request");
    };
    start(g, kind, headers, body, req, None)
}
fn start(
    g: Gate,
    kind: store::Kind,
    headers: HeaderMap,
    body: Arc<OwnedSemaphorePermit>,
    req: Request,
    payload: Option<(u64, Bytes)>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(v) => v,
        Err(e) => return transport::error(e),
    };
    let service = match review(&g, kind) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    if let Err(e) = req.validate(&payload) {
        return fail(e);
    }
    match service.transfer_replay(&owner, &req, &payload) {
        Ok(Some(v)) => return (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => return fail(e),
        _ => (),
    }
    let r = match reader(&g, &req.context) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    if r.catalog()["metadata"]["format"] != "ice" {
        return fail("review_pack_required");
    }
    let context = req.context.clone();
    match current(&g, &r, &context, || {
        service.start_transfer(Arc::clone(&g), owner, Arc::clone(&r), req, payload, body)
    }) {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => fail(e),
    }
}
async fn chunk(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Extension(body): Extension<Arc<OwnedSemaphorePermit>>,
    bytes: Bytes,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(v) => v,
        Err(e) => return transport::error(e),
    };
    let service = match review(&g, kind) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let field = |key| {
        crate::origin::single(&headers, key)
            .map(str::to_owned)
            .ok_or("invalid_drc_request")
    };
    let parsed = (|| -> std::result::Result<_, Failure> {
        if crate::origin::single(&headers, "content-type") != Some("application/octet-stream") {
            return Err("invalid_drc_request");
        }
        let t = field("x-floe-transfer-token")?;
        let seq = field("x-floe-transfer-seq")?;
        let offset = crate::drc::dto::number(&field("x-floe-transfer-offset")?)?;
        // Carry context in fixed headers so exact retries remain replays even
        // after the upload is consumed into a prepared whole-review replacement.
        let context = Context {
            drc_id: field("x-floe-drc")?,
            revision: field("x-floe-revision")?,
            view_id: field("x-floe-view")?,
        };
        Ok((
            Request {
                seq,
                context,
                action: Action::Chunk,
                bytes: None,
                token: Some(t),
            },
            offset,
        ))
    })();
    let (req, offset) = match parsed {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    // Owner check above precedes all parsing; start validates the exact slot.
    let _ = (owner, service);
    start(g, kind, headers, body, req, Some((offset, bytes)))
}
async fn status(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(v) => v,
        Err(e) => return transport::error(e),
    };
    let service = match review(&g, kind) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let s = service.inner.state.lock().unwrap();
    let upload = s.ready.as_ref().filter(|r| r.owner == owner && r.expires > Instant::now()).and_then(|r| {
        if let Model::Upload(u) = &r.model { Some(json!({"token":r.token,"context":r.context,"bytes":u.bytes.to_string(),"received":u.received.to_string()})) } else { None }
    });
    let usage = service.inner.artifacts.usage();
    // Inventory is independent of the 32-operation replay window: hundreds
    // of upload chunks must not hide an older still-charged export on reload.
    let artifacts: Vec<_> = s.transfer.artifacts.iter().filter(|(_, a)| a.owner == owner && a.review_rev == s.review_rev)
        .filter_map(|(id, a)| service.inner.artifacts.info(*id).map(|i| json!({"id":id.to_string(),"name":a.name,
            "context":a.context,"review_rev":a.review_rev.to_string(),"bytes":i.size_bytes.to_string(),"expires_in_ms":i.expires_in_ms.to_string()}))).collect();
    Json(json!({"kind":"drc_review_transfer","available":!s.closed && !s.detached,"operations":s.transfer.ledger.snapshot(),"upload":upload,"artifacts":artifacts,
        "limits":{"chunk_bytes":CHUNK,"file_bytes":MAX_BYTES.to_string(),"entries":2,"readers":1,"ttl_seconds":600},
        "usage":{"entries":usage.entries,"bytes":usage.bytes.to_string(),"pending":usage.pending,"readers":usage.readers}})).into_response()
}
async fn operation(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Path(seq): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let service = match review(&g, kind) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let n = match crate::view::counter(&seq) {
        Ok(n) => n,
        Err(e) => return fail(e),
    };
    let result = service.inner.state.lock().unwrap().transfer.ledger.get(n);
    result.map_or_else(|| fail("operation_expired"), |v| Json(v).into_response())
}
async fn cancel(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Path(seq): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let service = match review(&g, kind) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let n = match crate::view::counter(&seq) {
        Ok(n) => n,
        Err(e) => return fail(e),
    };
    match service.transfer_cancel(n) {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(e) => fail(e),
    }
}
fn registered_artifact(
    g: &Gate,
    owner: &SessionId,
    kind: store::Kind,
    id: &str,
) -> std::result::Result<(Arc<Service>, u64, Artifact), Failure> {
    let service = review(g, kind)?;
    let id = crate::view::counter(id)?;
    let a = service.artifact(owner, id)?;
    current(g, &a.reader, &a.context, || Ok(()))?;
    Ok((service, id, a))
}
async fn artifact_info(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let owner = match transport::http_session(&g, &headers) {
        Ok(v) => v,
        Err(e) => return transport::error(e),
    };
    let (s, id, a) = match registered_artifact(&g, &owner, kind, &id) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    s.inner.artifacts.info(id).map_or_else(|| fail("review_expired"), |i| Json(json!({"id":id.to_string(),"context":a.context,"review_rev":a.review_rev.to_string(),"name":a.name,"bytes":i.size_bytes.to_string(),"expires_in_ms":i.expires_in_ms.to_string()})).into_response())
}
async fn release(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    let s = match review(&g, kind) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let id = match crate::view::counter(&id) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    s.inner.artifacts.request_release(id);
    StatusCode::NO_CONTENT.into_response()
}
fn download(
    g: Gate,
    owner: SessionId,
    kind: store::Kind,
    headers: HeaderMap,
    id: String,
) -> Response {
    if headers.contains_key("range") {
        return transport::error(StatusCode::RANGE_NOT_SATISFIABLE);
    }
    let (service, id, a) = match registered_artifact(&g, &owner, kind, &id) {
        Ok(v) => v,
        Err(e) => return fail(e),
    };
    let read = match service.inner.artifacts.open(id) {
        Ok(v) => v,
        Err(e) => return fail(storage_error(e)),
    };
    let who = owner.clone();
    crate::exports::stream_download(g, owner, read, a.name.clone(), move |g| {
        current(g, &a.reader, &a.context, || {
            service.artifact(&who, id).map(|_| ())
        })
        .is_ok()
    })
}
async fn download_get(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    match transport::http_session(&g, &headers) {
        Ok(owner) => download(g, owner, kind, headers, id),
        Err(e) => transport::error(e),
    }
}
async fn download_post(
    HttpState(g): HttpState<Gate>,
    Extension(kind): Extension<store::Kind>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let csrf = std::str::from_utf8(&body)
        .ok()
        .and_then(|s| s.strip_prefix("csrf="))
        .filter(|s| token(s));
    if crate::origin::single(&headers, "content-type") != Some("application/x-www-form-urlencoded")
        || csrf.is_none()
    {
        return transport::error(StatusCode::UNAUTHORIZED);
    }
    match transport::download_session(&g, &headers, csrf.unwrap()) {
        Ok(owner) => download(g, owner, kind, headers, id),
        Err(e) => transport::error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(action: &str) -> Value {
        json!({"seq":"1","context":super::super::tests::request().context,"action":action})
    }
    fn valid(value: Value, payload: Option<(u64, Bytes)>) -> bool {
        serde_json::from_value::<Request>(value).is_ok_and(|r| r.validate(&payload).is_ok())
    }
    #[test]
    fn transfer_schema_has_no_caller_path_and_limits_each_upload_chunk() {
        assert!(valid(request("export"), None));
        for key in ["path", "reviewer", "filename", "approve", "gids", "kind"] {
            let mut r = request("export");
            r[key] = json!("not accepted");
            assert!(!valid(r, None));
        }
        for (bytes, expected) in [
            ("0".into(), false),
            ("01".into(), false),
            ("1".into(), true),
            (MAX_BYTES.to_string(), true),
            ((MAX_BYTES + 1).to_string(), false),
            ("한글".into(), false),
        ] {
            let mut r = request("import");
            r["bytes"] = json!(bytes);
            assert_eq!(valid(r, None), expected);
        }
        let mut r = request("chunk");
        r["token"] = json!("d".repeat(64));
        assert!(!valid(r.clone(), None));
        for (len, expected) in [(0, false), (1, true), (CHUNK, true), (CHUNK + 1, false)] {
            assert_eq!(
                valid(r.clone(), Some((0, Bytes::from(vec![0; len])))),
                expected
            );
        }
        r["action"] = json!("prepare");
        assert!(valid(r.clone(), None));
        r["token"] = json!("한".repeat(64));
        assert!(!valid(r, None));
        for kind in ["notes", "waives"] {
            for suffix in ["transfer", "transfer/chunk"] {
                let path = format!("/api/v1/drc/review/{kind}/{suffix}");
                assert!(super::super::http::is_large_body(
                    &axum::http::Method::POST,
                    &path
                ));
                assert!(!super::super::http::is_large_body(
                    &axum::http::Method::GET,
                    &path
                ));
                assert!(!super::super::http::is_large_body(
                    &axum::http::Method::POST,
                    &(path + "/")
                ));
            }
        }
    }
    #[test]
    fn replay_signature_binds_bytes_offset_context_and_owner_without_retaining_body() {
        let mut auth = crate::auth::Auth::new(Instant::now(), TTL, TTL).unwrap();
        let owner = auth
            .0
            .exchange(&auth.1.expose(), Instant::now())
            .unwrap()
            .id;
        let mut r = request("chunk");
        r["token"] = json!("d".repeat(64));
        let mut r: Request = serde_json::from_value(r).unwrap();
        let payload = Some((0, Bytes::from(vec![b'x'; CHUNK])));
        let sig = r.signature(&owner, &payload);
        assert!(sig.len() < 1024);
        assert_eq!(sig, r.signature(&owner, &payload));
        assert_ne!(
            sig,
            r.signature(&owner, &Some((1, Bytes::from(vec![b'x'; CHUNK]))))
        );
        assert_ne!(
            sig,
            r.signature(&owner, &Some((0, Bytes::from(vec![b'y'; CHUNK]))))
        );
        r.context.view_id = "e".repeat(64);
        assert_ne!(sig, r.signature(&owner, &payload));
        r.context.view_id = "c".repeat(64);
        let mut other = crate::auth::Auth::new(Instant::now(), TTL, TTL).unwrap();
        let other = other
            .0
            .exchange(&other.1.expose(), Instant::now())
            .unwrap()
            .id;
        assert_ne!(sig, r.signature(&other, &payload));
    }
    #[test]
    fn transfer_cancel_retains_admission_until_owner_unwinds() {
        let s = super::super::tests::service();
        let body = Arc::new(Semaphore::new(1));
        let op = s
            .begin(Arc::new(Arc::clone(&body).try_acquire_owned().unwrap()))
            .unwrap();
        s.inner
            .state
            .lock()
            .unwrap()
            .transfer
            .ledger
            .admit(1, "test".into(), "drc_review_transfer")
            .unwrap();
        assert_eq!(s.transfer_cancel(1).unwrap()["phase"], "queued");
        assert_ne!(op.stop.load(Ordering::Relaxed), 0);
        assert_eq!(body.available_permits(), 0);
        assert!(matches!(s.admit_build(|| Ok(())), Err("drc_busy")));
        drop(op);
        assert_eq!(body.available_permits(), 1);
        // Even the handoff between native unwind and its terminal receipt is
        // excluded from edit/read admission.
        assert!(matches!(
            s.begin(Arc::new(Arc::clone(&body).try_acquire_owned().unwrap())),
            Err("drc_busy")
        ));
        let result = json!({"seq":"1","phase":"cancelled"});
        s.inner
            .state
            .lock()
            .unwrap()
            .transfer
            .ledger
            .update(1, result.clone(), true);
        assert_eq!(s.transfer_cancel(1).unwrap(), result);
        s.admit_build(|| Ok(())).unwrap();
        super::super::tests::stop(&s);
    }
}
