//! Follow is a separate, read-only protocol, never owner Control dispatch.
//! Native pixels are reused; no renderer, dataset lease, file IO or owner
//! subscriber is created. One packet/write/ACK credit per guest connection.
use super::{http, Lease, Mode, Scope};
use crate::{
    auth::public_id,
    origin,
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
use floe_app_core::view::{DisplayFrame, Phase, Purpose, Snapshot};
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
        if guest.mode != Mode::Follow {
            return Err(StatusCode::CONFLICT);
        }
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
    ws.protocols([PROTOCOL])
        .max_message_size(CONTROL_BYTES)
        .max_frame_size(CONTROL_BYTES)
        .read_buffer_size(CONTROL_BYTES)
        .write_buffer_size(0)
        .max_write_buffer_size(view::PACKET_BYTES + 2 * CONTROL_BYTES)
        .on_upgrade(move |ws| async move {
            let (_permit, _slot) = (permit, slot);
            socket(ws, gate, lease).await;
        })
        .into_response()
}

fn valid(gate: &Gate, lease: &Lease) -> bool {
    !*gate.stopping.borrow()
        && http::with_shares(gate, |s, now| Ok(s.valid(lease, now))).unwrap_or(false)
}
fn allowed(frame: &DisplayFrame, scope: &Scope, snapshot: &Snapshot) -> bool {
    frame_scope(frame, scope) && frame.matches(snapshot)
}
fn frame_scope(frame: &DisplayFrame, scope: &Scope) -> bool {
    frame.dataset_revision == scope.dataset_revision && frame.frame.request.layers == scope.layers
}
fn state(s: &Snapshot, scope: &Scope, epoch: &str) -> Value {
    json!({"type":"share.state","view_id":scope.view_id,"connection_epoch":epoch,
        "dataset_revision":scope.dataset_revision.to_string(),"state_rev":s.state_rev.to_string(),
        "render_rev":s.render_rev.to_string(),"render_key":s.render_key.to_string(),
        "worker_epoch":s.worker_epoch.to_string(),"bbox_dbu":s.state.viewport.bbox.map(|v|v.to_string()),
        "pixels":[s.state.viewport.width,s.state.viewport.height],
        "rendering":matches!(s.phase, Phase::Opening | Phase::Rendering | Phase::Cancelling),
        "margin":s.margin.map(|m|json!({"frame_id":m.frame_id.to_string(),"origin_px":m.origin_px,"crop_safe":m.crop_safe})),
        "margin_working":s.margin_working})
}
fn header(frame: &DisplayFrame, scope: &Scope, epoch: &str) -> Result<Vec<u8>, ()> {
    // Reuse strict raw/PNG validation, but expose an allowlist so additions to
    // owner metadata (catalog, paths, query receipts, telemetry) cannot leak.
    let bytes = view::frame_header(frame, &scope.view_id, epoch).map_err(|_| ())?;
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
        ]
        .contains(&key.as_str())
    });
    value["query"] = json!(false);
    value["query_scene"] =
        json!({"generation":null,"round":null,"complete":false,"summary_layers":"0"});
    serde_json::to_vec(&value).map_err(|_| ())
}

/// Check scope at each actual sink poll, including a backpressured flush.
/// No mutex is held across await. Explicit revoke serializes with start_send;
/// controller scope is observed again every poll/tick. Bytes already handed
/// to the socket before revocation cannot be recalled.
async fn send(ws: &mut WebSocket, gate: &Gate, lease: &Lease, message: Message) -> Result<(), ()> {
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
            Self::Ping { seq } | Self::Ack { seq, .. } => seq,
        }
    }
}

async fn socket(mut ws: WebSocket, gate: Gate, lease: Lease) {
    let Ok(epoch) = public_id() else {
        return;
    };
    let Some(attached) = gate.active_view().filter(|v| v.id == lease.scope.view_id) else {
        return;
    };
    if !valid(&gate, &lease) {
        return;
    }
    // Deliberately no Attachment::subscribe(): guests must not keep an absent
    // owner's view alive beyond the existing disconnect grace period.
    let hello = json!({"type":"share.hello","protocol":1,"bundle":BUNDLE,"share_id":lease.guest.share_id,
        "view_id":lease.scope.view_id,"connection_epoch":epoch,"mode":"follow","read_only":true,
        "frame_credit":1,"pending_frames":1});
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
    let mut last_frames = [0, 0];
    let mut last_state = Value::Null;
    let mut seq = 0;
    let mut received = Instant::now();
    let mut window = received;
    let mut messages = 0;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    heartbeat.tick().await;
    loop {
        tokio::select! {
            biased;
            _ = revoked.changed() => break,
            _ = stop.changed() => break,
            result = async { encoding.as_mut().unwrap().await }, if encoding.is_some() => {
                encoding = None;
                let Ok(Ok(packet)) = result else { break; };
                let snapshot = attached.controller.snapshot();
                if !valid(&gate, &lease) { break; }
                if !allowed(&packet.frame, &lease.scope, &snapshot) { continue; }
                let next_state = state(&snapshot, &lease.scope, &epoch);
                if next_state != last_state {
                    if send(&mut ws, &gate, &lease, text(next_state.clone())).await.is_err() { break; }
                    last_state = next_state;
                }
                if send(&mut ws, &gate, &lease, Message::Binary(packet.bytes)).await.is_err() { break; }
                last_frames[usize::from(packet.margin)] = packet.id;
                // Only enter ACK state after the write completes. An early or
                // guessed ACK cannot free bytes while the sink still owns them.
                flight = Some(Flight { id: packet.id, since: Instant::now(), _reservation: packet.reservation });
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
                        flight = None;
                    },
                }
            },
            _ = tick.tick() => {
                if !valid(&gate, &lease) || received.elapsed() >= Duration::from_secs(30)
                    || flight.as_ref().is_some_and(|f| f.since.elapsed() >= ACK_TIMEOUT) { break; }
                let snapshot = attached.controller.snapshot();
                let next_state = state(&snapshot, &lease.scope, &epoch);
                if next_state != last_state {
                    if send(&mut ws, &gate, &lease, text(next_state.clone())).await.is_err() { break; }
                    last_state = next_state;
                }
                if encoding.is_some() || flight.is_some() { continue; }
                let candidate = [attached.controller.latest(), attached.controller.margin()].into_iter().flatten()
                    .find(|f| last_frames[usize::from(f.purpose == Purpose::Margin)] != f.id
                        && allowed(f, &lease.scope, &snapshot));
                let Some(frame) = candidate else { continue; };
                let Ok(header) = header(&frame, &lease.scope, &epoch) else { break; };
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
        // New owner commands remain denied without a second hand-copied list.
        for kind in include_str!("../stream.rs")
            .lines()
            .filter_map(|line| {
                line.trim()
                    .strip_prefix("#[serde(rename = \"")?
                    .strip_suffix("\")]")
            })
            .filter(|kind| !["ping", "frame.ack"].contains(kind))
        {
            assert!(serde_json::from_value::<Control>(json!({"type":kind,"seq":"1"})).is_err());
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
            view_id: "view".into(),
            dataset_revision: 2,
            layers: Layers::Only(vec![(7, 0)]),
        };
        assert!(frame_scope(&frame, &scope));
        let h: Value = serde_json::from_slice(&header(&frame, &scope, "epoch").unwrap()).unwrap();
        assert_eq!(h["query"], false);
        assert_eq!(
            h["query_scene"],
            json!({"generation":null,"round":null,"complete":false,"summary_layers":"0"})
        );
        assert!(h.get("perf").is_none());
        assert!(!h.to_string().contains("/secret"));
        frame.frame.request.layers = Layers::All;
        assert!(
            !frame_scope(&frame, &scope),
            "stale all-layer frame must not inherit a narrow snapshot"
        );
        frame.frame.request.layers = scope.layers.clone();
        frame.dataset_revision += 1;
        assert!(!frame_scope(&frame, &scope));
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
