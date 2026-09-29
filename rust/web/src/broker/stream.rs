//! Restricted server view wire: no owner Service, file, query or export commands.
//! One image credit, bounded independent writer/copy and session-scoped auth.
use super::{runtime::Runtime, Access, Error, Result};
use crate::{
    auth, origin,
    transport::BUNDLE,
    view::{self, PatchDto},
};
use axum::{
    extract::ws::{Message, WebSocket},
    http::HeaderMap,
};
use bytes::Bytes;
use floe_app_core::view::{Model, Snapshot};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::timeout,
};

pub const PROTOCOL: &str = "floe-server-v1";
const ACK_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) struct Transport {
    active: Mutex<BTreeSet<String>>,
    slots: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
    encoders: Arc<Semaphore>,
}
impl Default for Transport {
    fn default() -> Self {
        Self {
            active: Mutex::new(BTreeSet::new()),
            slots: Arc::new(Semaphore::new(32)),
            bytes: Arc::new(Semaphore::new(view::OUTPUT_BUDGET)),
            encoders: Arc::new(Semaphore::new(2)),
        }
    }
}
pub(super) struct Slot {
    transport: Arc<Transport>,
    id: String,
    _permit: OwnedSemaphorePermit,
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.transport
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}
impl Transport {
    pub(super) async fn drain(&self) -> Result<()> {
        timeout(Duration::from_secs(2), async {
            let _slots = Arc::clone(&self.slots)
                .acquire_many_owned(32)
                .await
                .map_err(|_| Error::Unavailable)?;
            let _encoders = Arc::clone(&self.encoders)
                .acquire_many_owned(2)
                .await
                .map_err(|_| Error::Unavailable)?;
            let _bytes = Arc::clone(&self.bytes)
                .acquire_many_owned(view::OUTPUT_BUDGET as u32)
                .await
                .map_err(|_| Error::Unavailable)?;
            Ok(())
        })
        .await
        .map_err(|_| Error::Unavailable)?
    }
    pub(super) fn claim(self: &Arc<Self>, id: &str) -> Result<Slot> {
        let permit = Arc::clone(&self.slots)
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        if !self
            .active
            .lock()
            .map_err(|_| Error::Unavailable)?
            .insert(id.to_owned())
        {
            return Err(Error::Busy);
        }
        Ok(Slot {
            transport: Arc::clone(self),
            id: id.into(),
            _permit: permit,
        })
    }
}
pub(super) fn csrf(headers: &HeaderMap) -> Option<String> {
    let protocols: Vec<_> = origin::single(headers, "sec-websocket-protocol")?
        .split(',')
        .map(str::trim)
        .collect();
    if protocols.len() != 3
        || !protocols.contains(&PROTOCOL)
        || !protocols.contains(&format!("bundle.{BUNDLE}").as_str())
    {
        return None;
    }
    let csrf = protocols.iter().find_map(|p| p.strip_prefix("csrf."))?;
    auth::Secret::parse(csrf)?;
    Some(csrf.to_owned())
}
fn snapshot(s: &Snapshot, m: &Model, id: &str, epoch: &str) -> Value {
    let mut value = view::snapshot(s, m, id, epoch);
    // DTO reuse must not advertise the standalone owner's unmounted APIs.
    for name in ["query", "clip", "mode"] {
        value["capabilities"][name] = json!(false);
    }
    value
}
pub(super) fn state(runtime: &Runtime, access: &Access, epoch: &str) -> Result<Value> {
    Ok(match runtime.sample(access)? {
        Some((s, m)) => snapshot(&s, &m, access.id(), epoch),
        None => json!({"type":"opening","view_id":access.id(),"connection_epoch":epoch}),
    })
}
#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Control {
    #[serde(rename = "ping")]
    Ping { seq: String },
    #[serde(rename = "view.set")]
    Set {
        seq: String,
        connection_epoch: String,
        view_id: String,
        base_state_rev: String,
        body: Box<PatchDto>,
    },
    #[serde(rename = "frame.ack")]
    Ack {
        seq: String,
        connection_epoch: String,
        frame_id: String,
    },
}
struct Packet {
    id: u64,
    render_rev: u64,
    bytes: Bytes,
    _bytes: Arc<OwnedSemaphorePermit>,
}
enum Out {
    Control(Message),
    Frame(Packet),
}
struct Flight {
    id: u64,
    written: bool,
    acked: bool,
    since: Instant,
    _bytes: Arc<OwnedSemaphorePermit>,
}
impl Flight {
    fn acknowledge(&mut self, id: u64) -> std::result::Result<(), ()> {
        if id != self.id || self.acked {
            return Err(());
        }
        self.acked = true;
        Ok(())
    }
    fn written(&mut self, id: u64) -> std::result::Result<(), ()> {
        if id != self.id || self.written {
            return Err(());
        }
        self.written = true;
        Ok(())
    }
    fn complete(&self) -> bool {
        self.written && self.acked
    }
}
struct Task<T>(JoinHandle<T>);
impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
fn reply(tx: &mpsc::Sender<Out>, value: Value) -> std::result::Result<(), ()> {
    let text = value.to_string();
    if text.len() > view::CONTROL_REPLY_BYTES {
        return Err(());
    }
    tx.try_send(Out::Control(Message::Text(text.into())))
        .map_err(|_| ())
}
fn offer(tx: &mpsc::Sender<Out>, p: Packet) -> std::result::Result<Option<Packet>, ()> {
    match tx.try_send(Out::Frame(p)) {
        Ok(()) => Ok(None),
        Err(mpsc::error::TrySendError::Full(Out::Frame(p))) => Ok(Some(p)),
        _ => Err(()),
    }
}
fn marker(s: &Snapshot) -> (u64, floe_app_core::view::Phase, u64, u64) {
    (s.state_rev, s.phase, s.submitted, s.consumed)
}
pub(super) async fn socket(
    ws: WebSocket,
    runtime: Arc<Runtime>,
    access: Access,
    transport: Arc<Transport>,
    _slot: Slot,
) {
    let Ok(epoch) = auth::public_id() else { return };
    let Ok(mut stop) = runtime.broker.subscribe(&access) else {
        return;
    };
    if *stop.borrow() {
        return;
    }
    let mut writer_stop = stop.clone();
    let (mut sink, mut input) = ws.split();
    let (tx, mut output) = mpsc::channel::<Out>(4);
    let (written_tx, mut written_rx) = mpsc::channel(1);
    let writer_runtime = Arc::clone(&runtime);
    let writer_access = access.clone();
    let mut writer = Task(tokio::spawn(async move {
        loop {
            let item = tokio::select! {biased;
                _=writer_stop.changed()=>break,
                item=output.recv()=>match item {Some(item)=>item,None=>break},
            };
            if *writer_stop.borrow()
                || writer_runtime
                    .broker
                    .with_access(&writer_access, || Ok(()))
                    .is_err()
            {
                break;
            }
            let (message, id, _charge) = match item {
                Out::Control(m) => (m, None, None),
                Out::Frame(p) => (Message::Binary(p.bytes), Some(p.id), Some(p._bytes)),
            };
            let sent = tokio::select! {biased;
                _=writer_stop.changed()=>false,
                sent=timeout(Duration::from_secs(5),sink.send(message))=>matches!(sent,Ok(Ok(()))),
            };
            if !sent {
                break;
            }
            if let Some(id) = id {
                if written_tx.try_send(id).is_err() {
                    break;
                }
            }
        }
        let _ = timeout(Duration::from_millis(100), sink.close()).await;
    }));
    if reply(&tx,json!({"type":"hello","protocol":1,"bundle":BUNDLE,"connection_epoch":epoch,
        "view_id":access.id(),"frame_credit":1,"pending_frames":1,"capabilities":{"view":true,"index":false,"review":false,"export":false,"query":false}})).is_err() {return}
    let Ok(initial) = state(&runtime, &access, &epoch) else {
        return;
    };
    if reply(&tx, initial).is_err() {
        return;
    }
    let (mut seq, mut last_frame) = (0u64, 0u64);
    let mut last_state = None;
    let mut flight: Option<Flight> = None;
    let mut pending: Option<Packet> = None;
    let mut encoding: Option<Task<std::result::Result<Packet, &'static str>>> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (mut received, mut window, mut heartbeat) =
        (Instant::now(), Instant::now(), Instant::now());
    let mut count = 0u32;
    loop {
        tokio::select! {
            _=stop.changed()=>break,
            _=&mut writer.0=>break,
            Some(id)=written_rx.recv()=>{
                let Some(f)=flight.as_mut() else {break};
                if f.written(id).is_err() {break}
                if f.complete() {flight=None;}
            },
            result=async {(&mut encoding.as_mut().unwrap().0).await},if encoding.is_some()=>{
                encoding=None;
                let Ok(Ok(p))=result else {break};
                let Ok(Some((s,_)))=runtime.sample(&access) else {break};
                if s.render_rev!=p.render_rev {flight=None;continue}
                match offer(&tx,p) {Ok(p)=>pending=p,Err(())=>break}
            },
            _=tick.tick()=>{
                if received.elapsed()>Duration::from_secs(30) || flight.as_ref().is_some_and(|f|f.since.elapsed()>ACK_TIMEOUT) {break}
                if heartbeat.elapsed()>Duration::from_secs(10) {
                    if tx.try_send(Out::Control(Message::Ping(Bytes::new()))).is_err() {break}
                    heartbeat=Instant::now();
                }
                let sample=match runtime.sample(&access) {Ok(s)=>s,Err(_)=>break};
                let Some((s,m))=sample else {continue};
                if last_state!=Some(marker(&s)) {
                    if reply(&tx,snapshot(&s,&m,access.id(),&epoch)).is_err() {break}
                    last_state=Some(marker(&s));
                }
                if let Some(p)=pending.take() {
                    if p.render_rev!=s.render_rev {flight=None;}else{match offer(&tx,p) {Ok(p)=>pending=p,Err(())=>break}}
                }
                if flight.is_none() {
                    let Ok(Some(frame))=runtime.latest(&access) else {continue};
                    if frame.id<=last_frame || !frame.matches(&s) {continue}
                    let Ok(header)=view::frame_header(&frame,access.id(),&epoch) else {break};
                    let mut value:Value=serde_json::from_slice(&header).expect("encoded frame header");
                    value["query"]=json!(false);
                    value["query_scene"]["generation"]=Value::Null;value["query_scene"]["round"]=Value::Null;value["query_scene"]["complete"]=json!(false);
                    let Ok(header)=serde_json::to_vec(&value) else {break};
                    let bytes=(4+header.len()+frame.frame.bytes.len())*2;
                    let Ok(bytes)=u32::try_from(bytes) else {break};
                    let Ok(charge)=Arc::clone(&transport.bytes).try_acquire_many_owned(bytes) else {continue};
                    let Ok(encoder)=Arc::clone(&transport.encoders).try_acquire_owned() else {continue};
                    let charge=Arc::new(charge);let owned=Arc::clone(&charge);
                    last_frame=frame.id;
                    flight=Some(Flight {id:frame.id,written:false,acked:false,since:Instant::now(),_bytes:charge});
                    encoding=Some(Task(tokio::task::spawn_blocking(move || {
                        let _encoder=encoder;
                        let bytes=view::packet(&header,&frame.frame.bytes)?;
                        Ok(Packet {id:frame.id,render_rev:frame.render_rev,bytes:Bytes::from(bytes),_bytes:owned})
                    })));
                }
            },
            incoming=input.next()=>{
                if runtime.broker.with_access(&access,||Ok(())).is_err() {break}
                // Writer notification and an early ACK can race in select!.
                if let Ok(id)=written_rx.try_recv() {
                    let Some(f)=flight.as_mut() else {break};
                    if f.written(id).is_err() {break}
                    if f.complete() {flight=None;}
                }
                if window.elapsed()>=Duration::from_secs(1) {window=Instant::now();count=0;}
                count+=1;if count>60 {break} received=Instant::now();
                let text=match incoming {
                    Some(Ok(Message::Text(t)))=>t,
                    Some(Ok(Message::Ping(p)))=>{if tx.try_send(Out::Control(Message::Pong(p))).is_err() {break}continue},
                    Some(Ok(Message::Pong(_)))=>continue,
                    _=>break,
                };
                let Ok(control)=serde_json::from_str::<Control>(&text) else {break};
                let n=match &control {Control::Ping{seq}|Control::Set{seq,..}|Control::Ack{seq,..}=>view::counter(seq)};
                let Ok(n)=n else {break};if n<=seq {break} seq=n;
                match control {
                    Control::Ping{seq}=>{if reply(&tx,json!({"type":"pong","seq":seq})).is_err() {break}}
                    Control::Ack{connection_epoch,frame_id,..}=>{
                        if connection_epoch!=epoch {break}
                        let Ok(id)=view::counter(&frame_id) else {break};
                        let Some(f)=flight.as_mut() else {break};
                        if f.acknowledge(id).is_err() {break}
                        if f.complete() {flight=None;}
                    },
                    Control::Set{seq,connection_epoch,view_id,base_state_rev,body}=>{
                        if connection_epoch!=epoch || view_id!=access.id() {break}
                        let Ok(base)=view::counter(&base_state_rev) else {break};
                        if tx.capacity()<2 {break}
                        let result=body.core().map_err(|_|Error::Invalid).and_then(|p|runtime.edit(&access,base,p));
                        let Ok(next)=state(&runtime,&access,&epoch) else {break};
                        let event=match result {
                            Ok(())=>json!({"type":"accepted","seq":seq,"state_rev":next["state_rev"],"render_rev":next["render_rev"]}),
                            Err(e)=>json!({"type":"error","seq":seq,"code":match e {Error::Busy=>"stale_state",Error::Invalid=>"invalid_request",_=>"unavailable"}}),
                        };
                        if reply(&tx,event).is_err() || reply(&tx,next).is_err() {break}
                    },
                }
            }
        }
    }
    // Task guards abort on normal return, panic and cancellation. A started
    // blocking copy retains both reservations until it finishes; it does no IO.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credit_requires_both_ack_and_completed_write() {
        let bytes = Arc::new(Semaphore::new(1));
        let charge = Arc::new(bytes.try_acquire_owned().unwrap());
        let mut f = Flight {
            id: 7,
            written: false,
            acked: false,
            since: Instant::now(),
            _bytes: charge,
        };
        f.acknowledge(7).unwrap();
        assert!(!f.complete());
        assert!(f.acknowledge(7).is_err());
        assert!(f.written(8).is_err());
        f.written(7).unwrap();
        assert!(f.complete());
    }
    #[test]
    fn protocol_is_exact_and_owner_operations_are_not_decoded() {
        let mut h = HeaderMap::new();
        let secret = "1".repeat(64);
        h.insert(
            "sec-websocket-protocol",
            format!("{PROTOCOL}, bundle.{BUNDLE}, csrf.{secret}")
                .parse()
                .unwrap(),
        );
        assert_eq!(csrf(&h), Some(secret));
        h.append("sec-websocket-protocol", "duplicate".parse().unwrap());
        assert!(csrf(&h).is_none());
        for text in [
            r#"{"type":"view.query","seq":"1"}"#,
            r#"{"type":"view.set","seq":"1","source":"secret"}"#,
            r#"{"type":"ping","seq":"1","user_id":"other"}"#,
        ] {
            assert!(serde_json::from_str::<Control>(text).is_err());
        }
    }
    #[test]
    fn subscriber_slot_is_exclusive_and_returns_on_drop() {
        let transport = Arc::new(Transport::default());
        let slot = transport.claim("a").unwrap();
        assert!(matches!(transport.claim("a"), Err(Error::Busy)));
        let _b = transport.claim("b").unwrap();
        drop(slot);
        assert!(transport.claim("a").is_ok());
    }
    #[tokio::test]
    async fn shutdown_waits_for_socket_copy_and_packet_reservations() {
        let transport = Arc::new(Transport::default());
        let slot = transport.claim("a").unwrap();
        let encoder = Arc::clone(&transport.encoders).try_acquire_owned().unwrap();
        let bytes = Arc::clone(&transport.bytes).try_acquire_owned().unwrap();
        let owned = Arc::clone(&transport);
        let mut drained = tokio::spawn(async move { owned.drain().await });
        assert!(timeout(Duration::from_millis(10), &mut drained)
            .await
            .is_err());
        drop(slot);
        assert!(timeout(Duration::from_millis(10), &mut drained)
            .await
            .is_err());
        drop(encoder);
        assert!(timeout(Duration::from_millis(10), &mut drained)
            .await
            .is_err());
        drop(bytes);
        assert!(drained.await.unwrap().is_ok());
    }
}
