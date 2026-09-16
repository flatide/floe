//! Guest transport is separate from owner Control dispatch. Follow reuses
//! owner pixels; explore owns its independently admitted view. Neither is an
//! owner subscriber. One packet/write/ACK credit per guest connection.
use super::{explore, http, Lease, Mode, Scope};
use crate::{
    auth::public_id,
    origin, query,
    transport::{self, Gate, BUNDLE, PROTOCOL},
    view,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use floe_app_core::view::{DisplayFrame, Phase, Purpose, Snapshot, ViewController};
use futures_util::{Sink, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    future::poll_fn,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::Poll,
    time::{Duration, Instant},
};
use tokio::{sync::OwnedSemaphorePermit, task::JoinHandle, time::timeout};

const CONTROL_BYTES: usize = 8 * 1024;
const ACK_TIMEOUT: Duration = Duration::from_secs(10);
struct Target {
    id: String,
    controller: Arc<ViewController>,
    rows: Arc<crate::layer_catalog::LayerCatalog>,
}

pub(super) async fn upgrade(
    State(gate): State<Gate>,
    Path(id): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if origin::single(&headers, "origin") != Some(gate.origin()) {
        return transport::error(StatusCode::FORBIDDEN);
    }
    let Some(protocols) = origin::single(&headers, "sec-websocket-protocol") else {
        return transport::error(StatusCode::UPGRADE_REQUIRED);
    };
    let protocols: Vec<_> = protocols.split(',').map(str::trim).collect();
    if protocols.len() != 3
        || protocols.iter().filter(|&&p| p == PROTOCOL).count() != 1
        || !protocols.contains(&format!("bundle.{BUNDLE}").as_str())
    {
        return transport::error(StatusCode::UPGRADE_REQUIRED);
    }
    let Some(csrf) = protocols.iter().find_map(|p| p.strip_prefix("guest-csrf.")) else {
        return transport::error(StatusCode::UNAUTHORIZED);
    };
    let Some(cookie) = origin::cookie(&headers, &http::cookie_name(&id)) else {
        return transport::error(StatusCode::UNAUTHORIZED);
    };
    let lease = match http::with_shares(&gate, |shares, now| {
        let guest = shares
            .authenticate(&id, cookie, csrf, now)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        shares
            .lease(guest, now)
            .map_err(|_| StatusCode::UNAUTHORIZED)
    }) {
        Ok(v) => v,
        Err(e) => return transport::error(e),
    };
    let Some(transport) = &gate.share_transport else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let Ok(permit) = Arc::clone(&transport.sockets).try_acquire_owned() else {
        return transport::error(StatusCode::TOO_MANY_REQUESTS);
    };
    let Ok(slot) = Arc::clone(&lease.socket).try_acquire_owned() else {
        return transport::error(StatusCode::TOO_MANY_REQUESTS);
    };
    let Some(owner) = gate.active_view().filter(|v| v.id == lease.scope.view_id) else {
        return transport::error(StatusCode::CONFLICT);
    };
    let target = if lease.guest.mode == Mode::Follow {
        Target {
            id: owner.id.clone(),
            controller: Arc::clone(&owner.controller),
            rows: Arc::clone(&owner.rows),
        }
    } else {
        match http::with_shares(&gate, |shares, now| {
            shares.explorer(&lease, &owner.controller, now)
        }) {
            Ok(view) => Target {
                id: view.id.clone(),
                controller: Arc::clone(&view.controller),
                rows: Arc::clone(&owner.rows),
            },
            Err(status) => return transport::error(status),
        }
    };
    ws.protocols([PROTOCOL])
        .max_message_size(CONTROL_BYTES)
        .max_frame_size(CONTROL_BYTES)
        .read_buffer_size(CONTROL_BYTES)
        .write_buffer_size(0)
        .max_write_buffer_size(view::PACKET_BYTES + 2 * CONTROL_BYTES)
        .on_upgrade(move |ws| async move {
            let (_permit, _slot) = (permit, slot);
            socket(ws, gate, lease, target).await;
        })
        .into_response()
}

fn valid(gate: &Gate, lease: &Lease) -> bool {
    !*gate.stopping.borrow()
        && http::with_shares(gate, |s, now| Ok(s.valid(lease, now))).unwrap_or(false)
}
fn allowed(frame: &DisplayFrame, lease: &Lease, snapshot: &Snapshot) -> bool {
    frame_scope(frame, &lease.scope, lease.guest.mode) && frame.matches(snapshot)
}
fn frame_scope(frame: &DisplayFrame, scope: &Scope, mode: Mode) -> bool {
    frame.dataset_revision == scope.dataset_revision
        && match mode {
            Mode::Follow => frame.frame.request.layers == scope.layers,
            Mode::Explore => explore::layers_within(&scope.layers, &frame.frame.request.layers),
        }
}
fn state(s: &Snapshot, target: &Target, mode: Mode, epoch: &str) -> Value {
    let mut out = json!({"type":"share.state","view_id":target.id,"connection_epoch":epoch,
        "dataset_revision":target.controller.model.dataset_revision.to_string(),"state_rev":s.state_rev.to_string(),
        "render_rev":s.render_rev.to_string(),"render_key":s.render_key.to_string(),
        "worker_epoch":s.worker_epoch.to_string(),"bbox_dbu":s.state.viewport.bbox.map(|v|v.to_string()),
        "dbu_um":target.controller.model.dbu.to_string(),
        "camera_um":view::camera_um(s.state.viewport.bbox,target.controller.model.dbu),
        "pixels":[s.state.viewport.width,s.state.viewport.height],
        "rendering":matches!(s.phase, Phase::Opening | Phase::Rendering | Phase::Cancelling),
        "margin":s.margin.map(|m|json!({"frame_id":m.frame_id.to_string(),"origin_px":m.origin_px,"crop_safe":m.crop_safe})),
        "margin_working":s.margin_working});
    if mode == Mode::Explore {
        use floe_app_core::shots::Detail;
        out["selection"] = match &s.state.layers {
            floe_worker_client::Layers::All => json!({"mode":"all"}),
            floe_worker_client::Layers::None => json!({"mode":"none"}),
            floe_worker_client::Layers::Only(pairs) => json!({"mode":"only","pairs":pairs}),
        };
        out["depth"] = json!(s.state.depth.map_or("full".into(), |v| v.to_string()));
        out["max_depth"] = json!(s.max_depth.map(|v| v.to_string()));
        out["detail"] = json!(match s.state.detail {
            Detail::Exact => "exact",
            Detail::Low => "low",
            Detail::Medium => "medium",
            Detail::High => "high",
        });
        out["thin"] = json!(s.state.thin.name());
        out["frames"] = json!(s.state.frames);
        out["labels"] = json!(s.state.labels);
        out["font_px"] = json!(s.state.font_px);
        out["mono"] = json!(s.state.mono);
        out["failure"] = json!(s.failure.as_ref().map(|(kind, _)| view::safe_error(*kind)));
    }
    out
}
fn header(frame: &DisplayFrame, view_id: &str, epoch: &str, mode: Mode) -> Result<Vec<u8>, ()> {
    // Reuse strict raw/PNG validation, but expose an allowlist so additions to
    // owner metadata (catalog, paths, query receipts, telemetry) cannot leak.
    let bytes = view::frame_header(frame, view_id, epoch).map_err(|_| ())?;
    let mut value: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    value.as_object_mut().ok_or(())?.retain(|key, _| {
        [
            "type",
            "protocol",
            "view_id",
            "connection_epoch",
            "frame_id",
            "dataset_revision",
            "state_rev",
            "render_rev",
            "render_key",
            "worker_epoch",
            "generation",
            "round",
            "purpose",
            "bbox_dbu",
            "width",
            "height",
            "row0",
            "format",
            "payload_length",
            "final",
            "partial",
            "deferred",
            "labels_truncated",
            "deck_skipped",
            "complete",
            "approximate",
            "query",
            "query_scene",
        ]
        .contains(&key.as_str())
    });
    if mode == Mode::Follow {
        value["query"] = json!(false);
        value["query_scene"] =
            json!({"generation":null,"round":null,"complete":false,"summary_layers":"0"});
    }
    serde_json::to_vec(&value).map_err(|_| ())
}

/// Check scope at each actual sink poll, including a backpressured flush.
/// No mutex is held across await. Explicit revoke serializes with start_send;
/// controller scope is observed again every poll/tick. Bytes already handed
/// to the socket before revocation cannot be recalled.
async fn send(ws: &mut WebSocket, gate: &Gate, lease: &Lease, message: Message) -> Result<(), ()> {
    if matches!(&message, Message::Text(t) if t.len() > view::CONTROL_REPLY_BYTES) {
        return Err(());
    }
    let mut message = Some(message);
    let mut revoked = lease.revoked.clone();
    let mut stop = gate.stopping.subscribe();
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let write = poll_fn(|cx| {
        http::with_shares(gate, |shares, now| {
            if *gate.stopping.borrow() || !shares.valid(lease, now) {
                return Err(StatusCode::UNAUTHORIZED);
            }
            let mut sink = Pin::new(&mut *ws);
            if message.is_some() {
                match sink.as_mut().poll_ready(cx) {
                    Poll::Pending => return Ok(Poll::Pending),
                    Poll::Ready(Err(_)) => return Err(StatusCode::GONE),
                    Poll::Ready(Ok(())) => (),
                }
                sink.as_mut()
                    .start_send(message.take().unwrap())
                    .map_err(|_| StatusCode::GONE)?;
            }
            Ok(sink.poll_flush(cx).map(|result| result.map_err(|_| ())))
        })
        .unwrap_or(Poll::Ready(Err(())))
    });
    tokio::pin!(write);
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            _ = revoked.changed() => return Err(()),
            _ = stop.changed() => return Err(()),
            _ = &mut deadline => return Err(()),
            _ = tick.tick() => if !valid(gate, lease) { return Err(()); },
            result = &mut write => return result,
        }
    }
}
fn text(value: Value) -> Message {
    Message::Text(value.to_string().into())
}

struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
struct Packet {
    id: u64,
    margin: bool,
    frame: Arc<DisplayFrame>,
    bytes: Bytes,
    reservation: OwnedSemaphorePermit,
}
struct Flight {
    id: u64,
    since: Instant,
    receipt: query::Receipt,
    _reservation: OwnedSemaphorePermit,
}
fn copy_packet(header: &[u8], body: &[u8], cancelled: impl Fn() -> bool) -> Result<Bytes, ()> {
    if header.len() > view::HEADER_BYTES || body.len() > view::PAYLOAD_BYTES || cancelled() {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(4 + header.len() + body.len());
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    bytes.extend_from_slice(header);
    for chunk in body.chunks(1024 * 1024) {
        if cancelled() {
            return Err(());
        }
        bytes.extend_from_slice(chunk);
    }
    if cancelled() {
        return Err(());
    }
    Ok(Bytes::from(bytes))
}
#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Control {
    #[serde(rename = "ping")]
    Ping { seq: String },
    #[serde(rename = "explore.set")]
    Set {
        seq: String,
        connection_epoch: String,
        view_id: String,
        base_state_rev: String,
        body: Box<explore::DisplayPatch>,
    },
    #[serde(rename = "explore.query")]
    Query {
        seq: String,
        connection_epoch: String,
        view_id: String,
        body: Box<query::Request>,
    },
    #[serde(rename = "explore.query.cancel")]
    CancelQuery {
        seq: String,
        connection_epoch: String,
        view_id: String,
        kind: query::Kind,
    },
    #[serde(rename = "explore.measure")]
    Measure {
        seq: String,
        connection_epoch: String,
        view_id: String,
        body: Box<query::MeasureRequest>,
    },
    #[serde(rename = "explore.measure_selection")]
    MeasureSelection {
        seq: String,
        connection_epoch: String,
        view_id: String,
        body: Box<query::MeasureSelectionRequest>,
    },
    #[serde(rename = "frame.ack")]
    Ack {
        seq: String,
        connection_epoch: String,
        frame_id: String,
        disposition: String,
    },
}
impl Control {
    fn seq(&self) -> &str {
        match self {
            Self::Ping { seq }
            | Self::Ack { seq, .. }
            | Self::Set { seq, .. }
            | Self::Query { seq, .. }
            | Self::CancelQuery { seq, .. }
            | Self::Measure { seq, .. }
            | Self::MeasureSelection { seq, .. } => seq,
        }
    }
}

async fn socket(mut ws: WebSocket, gate: Gate, lease: Lease, target: Target) {
    let Ok(epoch) = public_id() else {
        return;
    };
    if !valid(&gate, &lease) {
        return;
    }
    // Deliberately no Attachment::subscribe(): guests must not keep an absent
    // owner's view alive beyond the existing disconnect grace period.
    let hello = json!({"type":"share.hello","protocol":1,"bundle":BUNDLE,"share_id":lease.guest.share_id,
        "view_id":target.id,"connection_epoch":epoch,"mode":lease.guest.mode.name(),"read_only":true,
        "frame_credit":1,"pending_frames":1,"query":lease.guest.mode == Mode::Explore && !target.controller.model.deck,
        "measure":lease.guest.mode == Mode::Explore});
    if send(&mut ws, &gate, &lease, text(hello)).await.is_err() {
        return;
    }
    let transport = gate
        .share_transport
        .as_ref()
        .expect("enabled share transport");
    let mut revoked = lease.revoked.clone();
    let mut stop = gate.stopping.subscribe();
    let cancel = Cancel(Arc::new(AtomicBool::new(false)));
    let mut encoding: Option<JoinHandle<Result<Packet, ()>>> = None;
    let mut flight: Option<Flight> = None;
    // Follow must never own query tickets on the owner's controller.
    let mut queries = (lease.guest.mode == Mode::Explore)
        .then(|| super::query::Queries::new(&target.controller, &lease.scope));
    let mut last_frames = [0, 0];
    let mut last_state = Value::Null;
    let mut seq = 0;
    let mut received = Instant::now();
    let mut window = received;
    let mut messages = 0;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    heartbeat.tick().await;
    'socket: loop {
        tokio::select! {
            biased;
            _ = revoked.changed() => break,
            _ = stop.changed() => break,
            result = async { encoding.as_mut().unwrap().await }, if encoding.is_some() => {
                encoding = None;
                let Ok(Ok(packet)) = result else { break; };
                let snapshot = target.controller.snapshot();
                if !valid(&gate, &lease) { break; }
                if !allowed(&packet.frame, &lease, &snapshot) { continue; }
                let next_state = state(&snapshot, &target, lease.guest.mode, &epoch);
                if next_state != last_state {
                    if send(&mut ws, &gate, &lease, text(next_state.clone())).await.is_err() { break; }
                    last_state = next_state;
                }
                if send(&mut ws, &gate, &lease, Message::Binary(packet.bytes)).await.is_err() { break; }
                last_frames[usize::from(packet.margin)] = packet.id;
                // Only enter ACK state after the write completes. An early or
                // guessed ACK cannot free bytes while the sink still owns them.
                flight = Some(Flight { id: packet.id, since: Instant::now(), receipt:query::Receipt::of(&packet.frame), _reservation: packet.reservation });
            },
            input = ws.next() => {
                if !valid(&gate, &lease) { break; }
                let Some(Ok(message)) = input else { break; };
                let now = Instant::now();
                if now.duration_since(window) >= Duration::from_secs(1) { window=now; messages=0; }
                messages += 1;
                if messages > 60 { break; }
                received = now;
                let control = match message {
                    Message::Text(t) => match serde_json::from_str::<Control>(&t) { Ok(c) => c, Err(_) => break },
                    Message::Ping(p) => {
                        if send(&mut ws, &gate, &lease, Message::Pong(p)).await.is_err() { break; }
                        continue;
                    },
                    Message::Pong(_) => continue,
                    _ => break,
                };
                let Ok(next) = view::counter(control.seq()) else { break; };
                if next <= seq { break; }
                seq = next;
                match control {
                    Control::Ping { seq } => {
                        if send(&mut ws, &gate, &lease, text(json!({"type":"pong","seq":seq}))).await.is_err() { break; }
                    },
                    Control::Ack { connection_epoch, frame_id, disposition, .. } => {
                        if connection_epoch != epoch || !["displayed", "discarded"].contains(&disposition.as_str())
                            || flight.as_ref().is_none_or(|f| view::counter(&frame_id) != Ok(f.id)) { break; }
                        let completed = flight.take().unwrap();
                        if disposition == "displayed" {
                            if let Some(queries) = queries.as_mut() { queries.displayed(completed.receipt); }
                        }
                    },
                    Control::Query { seq, connection_epoch, view_id, body } => {
                        if lease.guest.mode != Mode::Explore || connection_epoch != epoch || view_id != target.id { break; }
                        let reply = http::with_shares(&gate, |shares, now| {
                            if !shares.valid(&lease, now) { return Err(StatusCode::UNAUTHORIZED); }
                            Ok(match queries.as_mut().unwrap().submit(seq.clone(), *body) {
                                Ok(id) => json!({"type":"query.accepted","seq":seq,"view_id":target.id,"connection_epoch":epoch,"query_id":id.to_string()}),
                                Err(code) => json!({"type":"error","seq":seq,"code":code}),
                            })
                        });
                        let Ok(reply) = reply else { break; };
                        if send(&mut ws, &gate, &lease, text(reply)).await.is_err() { break; }
                    },
                    Control::CancelQuery { seq, connection_epoch, view_id, kind } => {
                        if lease.guest.mode != Mode::Explore || connection_epoch != epoch || view_id != target.id { break; }
                        queries.as_mut().unwrap().cancel(kind);
                        let reply = json!({"type":"query.cancelled","seq":seq,"view_id":target.id,"connection_epoch":epoch,"kind":kind.name()});
                        if send(&mut ws, &gate, &lease, text(reply)).await.is_err() { break; }
                    },
                    Control::Measure { seq, connection_epoch, view_id, body } => {
                        if lease.guest.mode != Mode::Explore || connection_epoch != epoch || view_id != target.id { break; }
                        let reply = queries.as_ref().unwrap().measure(&seq, *body, &target.id, &epoch)
                            .unwrap_or_else(|code|json!({"type":"error","seq":seq,"code":code}));
                        if send(&mut ws, &gate, &lease, text(reply)).await.is_err() { break; }
                    },
                    Control::MeasureSelection { seq, connection_epoch, view_id, body } => {
                        if lease.guest.mode != Mode::Explore || connection_epoch != epoch || view_id != target.id { break; }
                        let reply = queries.as_ref().unwrap().measure_selection(&seq, *body, &target.id, &epoch)
                            .unwrap_or_else(|code|json!({"type":"error","seq":seq,"code":code}));
                        if send(&mut ws, &gate, &lease, text(reply)).await.is_err() { break; }
                    },
                    Control::Set { seq, connection_epoch, view_id, base_state_rev, body } => {
                        if lease.guest.mode != Mode::Explore || connection_epoch != epoch || view_id != target.id { break; }
                        let reply = http::with_shares(&gate, |shares, now| {
                            if !shares.valid(&lease, now) { return Err(StatusCode::UNAUTHORIZED); }
                            let applied = view::counter(&base_state_rev).map_err(|_| "invalid_request")
                                .and_then(|rev| body.core(&target.controller, &lease.scope, &target.rows)
                                    .and_then(|patch| target.controller.edit(rev, patch).map_err(|e| match e.kind {
                                        floe_app_core::ErrorKind::Busy => "conflict", _ => view::safe_error(e.kind)
                                    })));
                            Ok(match applied {
                                Ok(s) => json!({"type":"accepted","seq":seq,"view_id":target.id,"connection_epoch":epoch,
                                    "state_rev":s.state_rev.to_string(),"render_rev":s.render_rev.to_string()}),
                                Err(code) => json!({"type":"error","seq":seq,"code":code}),
                            })
                        });
                        let Ok(reply) = reply else { break; };
                        if send(&mut ws, &gate, &lease, text(reply)).await.is_err() { break; }
                    },
                }
            },
            _ = tick.tick() => {
                if !valid(&gate, &lease) || received.elapsed() >= Duration::from_secs(30)
                    || flight.as_ref().is_some_and(|f| f.since.elapsed() >= ACK_TIMEOUT) { break; }
                let snapshot = target.controller.snapshot();
                let next_state = state(&snapshot, &target, lease.guest.mode, &epoch);
                if next_state != last_state {
                    if send(&mut ws, &gate, &lease, text(next_state.clone())).await.is_err() { break; }
                    last_state = next_state;
                }
                // A waiting image ACK never blocks a small query result.
                if let Some(queries) = queries.as_mut() {
                    for index in 0..2 {
                        if let Some(reply) = queries.ready(index, &target.id, &epoch) {
                            if send(&mut ws, &gate, &lease, text(reply)).await.is_err() { break 'socket; }
                            queries.sent(index);
                        }
                    }
                }
                if encoding.is_some() || flight.is_some() { continue; }
                let candidate = [target.controller.latest(), target.controller.margin()].into_iter().flatten()
                    .find(|f| last_frames[usize::from(f.purpose == Purpose::Margin)] != f.id
                        && allowed(f, &lease, &snapshot));
                let Some(frame) = candidate else { continue; };
                let Ok(header) = header(&frame, &target.id, &epoch, lease.guest.mode) else { break; };
                let cost = 3 * frame.frame.bytes.len() + 2 * (header.len() + 4);
                let Ok(encoder) = Arc::clone(&transport.encoders).try_acquire_owned() else { continue; };
                let Ok(reservation) = Arc::clone(&transport.bytes).try_acquire_many_owned(cost as u32) else { continue; };
                let cancelled = Arc::clone(&cancel.0);
                let revoked = lease.revoked.clone();
                encoding = Some(tokio::task::spawn_blocking(move || {
                    let _encoder = encoder;
                    let bytes = copy_packet(&header, &frame.frame.bytes, ||
                        cancelled.load(Ordering::Relaxed) || *revoked.borrow())?;
                    Ok(Packet { id: frame.id, margin: frame.purpose == Purpose::Margin, frame, bytes, reservation })
                }));
            },
            _ = heartbeat.tick() => {
                if send(&mut ws, &gate, &lease, Message::Ping(Bytes::new())).await.is_err() { break; }
            },
        }
    }
    drop(cancel);
    if let Some(task) = encoding {
        task.abort();
        // A running blocking copy owns its permits until it really exits.
        // Cancellation is polled every MiB; a scheduler delay is not credit.
        let _ = timeout(Duration::from_secs(1), task).await;
    }
    // Drop the socket, do not flush buffered bytes after authorization ended.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn copy_checks_revocation_between_chunks_and_preserves_packet_bytes() {
        let body = vec![17; 3 * 1024 * 1024];
        let calls = Cell::new(0);
        assert!(copy_packet(b"header", &body, || {
            calls.set(calls.get() + 1);
            calls.get() == 3
        })
        .is_err());
        assert_eq!(calls.get(), 3);
        assert_eq!(
            copy_packet(b"header", &body, || false).unwrap().as_ref(),
            view::packet(b"header", &body).unwrap()
        );
    }
    #[test]
    fn follow_control_allowlist_is_not_owner_dispatch() {
        // The AST inventory gate binds this table to every owner command,
        // including multiline attributes. Missing fields are not a denial:
        // the guest enum must reject the owner namespace itself.
        let policy: serde_json::Value =
            serde_json::from_str(include_str!("../../../../tools/web_permissions.json")).unwrap();
        for kind in policy["wire"]
            .as_object()
            .unwrap()
            .keys()
            .filter_map(|key| key.strip_prefix("stream.rs/Control/"))
            .filter(|kind| !["ping", "frame.ack"].contains(kind))
        {
            let error = serde_json::from_value::<Control>(json!({"type":kind,"seq":"1"}))
                .err()
                .expect("owner command admitted to guest enum");
            assert!(
                error.to_string().starts_with("unknown variant"),
                "{kind}: {error}"
            );
        }
        assert!(
            serde_json::from_value::<Control>(json!({"type":"ping","seq":"1","body":{}})).is_err()
        );
    }

    #[test]
    fn native_frame_scope_and_metadata_do_not_inherit_current_owner_authority() {
        use floe_worker_client::{Fields, Frame, FrameFormat, Layers, RenderRequest};
        let mut bytes = b"FLOERAW1".to_vec();
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend([255; 4]);
        let mut frame = DisplayFrame {
            id: 1,
            dataset_revision: 2,
            state_rev: 3,
            render_rev: 4,
            render_key: 5,
            worker_epoch: 6,
            deck_skipped: 0,
            purpose: Purpose::Foreground,
            frame: Frame {
                generation: 1,
                round: 1,
                final_frame: true,
                partial: false,
                deferred: 0,
                labels_truncated: false,
                request: RenderRequest {
                    width: 1,
                    height: 1,
                    view: [0., 0., 1., 1.],
                    layers: Layers::Only(vec![(7, 0)]),
                    format: FrameFormat::Raw,
                    ..Default::default()
                },
                bytes,
                fields: Fields(std::collections::BTreeMap::from([
                    ("scene_gen".into(), "1".into()),
                    ("scene_round".into(), "1".into()),
                    ("scene_complete".into(), "1".into()),
                    ("scene_summary".into(), "0".into()),
                    ("png".into(), "/secret/design".into()),
                    ("raster_us".into(), "200".into()),
                ])),
            },
        };
        let scope = Scope {
            drc: None,
            view_id: "view".into(),
            dataset_revision: 2,
            layers: Layers::Only(vec![(7, 0)]),
        };
        assert!(frame_scope(&frame, &scope, Mode::Follow));
        assert!(frame_scope(&frame, &scope, Mode::Explore));
        let h: Value =
            serde_json::from_slice(&header(&frame, &scope.view_id, "epoch", Mode::Follow).unwrap())
                .unwrap();
        assert_eq!(h["query"], false);
        assert_eq!(
            h["query_scene"],
            json!({"generation":null,"round":null,"complete":false,"summary_layers":"0"})
        );
        assert!(h.get("perf").is_none());
        assert!(!h.to_string().contains("/secret"));
        let e: Value =
            serde_json::from_slice(&header(&frame, "explorer", "epoch", Mode::Explore).unwrap())
                .unwrap();
        assert_eq!(e["query"], true);
        assert_eq!(e["query_scene"]["generation"], "1");
        assert_eq!(e["view_id"], "explorer");
        assert!(e.get("perf").is_none());
        assert!(!e.to_string().contains("/secret"));
        frame.frame.request.layers = Layers::All;
        assert!(!frame_scope(&frame, &scope, Mode::Explore));
        assert!(
            !frame_scope(&frame, &scope, Mode::Follow),
            "stale all-layer frame must not inherit a narrow snapshot"
        );
        frame.frame.request.layers = scope.layers.clone();
        frame.dataset_revision += 1;
        assert!(!frame_scope(&frame, &scope, Mode::Follow));
        assert!(!frame_scope(&frame, &scope, Mode::Explore));
    }

    #[tokio::test]
    async fn cancelled_running_copy_keeps_credit_until_actual_exit() {
        use tokio::sync::{oneshot, Semaphore};
        let budget = Arc::new(Semaphore::new(100));
        let credit = Arc::clone(&budget).try_acquire_many_owned(100).unwrap();
        let cancel = Cancel(Arc::new(AtomicBool::new(false)));
        let flag = Arc::clone(&cancel.0);
        let (entered, ready) = oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let task = tokio::task::spawn_blocking(move || {
            let _credit = credit;
            entered.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(2)).unwrap();
            copy_packet(b"header", &[1; 64], || flag.load(Ordering::Relaxed))
        });
        ready.await.unwrap();
        drop(cancel);
        task.abort();
        assert_eq!(budget.available_permits(), 0);
        release.send(()).unwrap();
        assert!(task.await.unwrap().is_err());
        assert_eq!(budget.available_permits(), 100);
    }
}
