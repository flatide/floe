//! One in-flight image, latest available controller frame as the pending slot.
//! Reader/control, bounded packet encoding and socket writer are independent.
use crate::{
    auth::{public_id, SessionId},
    query,
    transport::{Gate, BUNDLE},
    view::{self, PatchDto},
};
use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, OwnedSemaphorePermit},
    task::JoinHandle,
    time::timeout,
};

const ACK_TIMEOUT: Duration = Duration::from_secs(10);
struct Packet {
    id: u64,
    render_rev: u64,
    margin: bool,
    bytes: Bytes,
    _reservation: Arc<OwnedSemaphorePermit>,
}
enum Out {
    Control(Message),
    Frame(Packet),
}
struct Flight {
    id: u64,
    receipt: query::Receipt,
    displayed: bool,
    since: Instant,
    written: bool,
    acked: bool,
    _reservation: Arc<OwnedSemaphorePermit>,
}
impl Flight {
    fn acknowledge(&mut self, id: u64) -> Result<(), ()> {
        if id != self.id || self.acked {
            return Err(());
        }
        self.acked = true;
        Ok(())
    }
    fn written(&mut self, id: u64) -> Result<(), ()> {
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
#[derive(Deserialize)]
#[serde(try_from = "String")]
enum Disposition {
    Displayed,
    Discarded,
}
impl TryFrom<String> for Disposition {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "displayed" => Ok(Self::Displayed),
            "discarded" => Ok(Self::Discarded),
            _ => Err("invalid disposition"),
        }
    }
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
    #[serde(rename = "view.apply")]
    Apply {
        seq: String,
        connection_epoch: String,
        view_id: String,
        base_state_rev: String,
        token: String,
    },
    #[serde(rename = "frame.ack")]
    Ack {
        seq: String,
        connection_epoch: String,
        frame_id: String,
        disposition: Disposition,
    },
    #[serde(rename = "view.query")]
    Query {
        seq: String,
        connection_epoch: String,
        view_id: String,
        body: Box<query::Request>,
    },
    #[serde(rename = "view.query.cancel")]
    CancelQuery {
        seq: String,
        connection_epoch: String,
        view_id: String,
        kind: query::Kind,
    },
}
fn try_reply(tx: &mpsc::Sender<Out>, value: Value) -> Result<bool, ()> {
    let text = value.to_string();
    if text.len() > view::CONTROL_REPLY_BYTES {
        return Err(());
    }
    match tx.try_send(Out::Control(Message::Text(text.into()))) {
        Ok(()) => Ok(true),
        Err(mpsc::error::TrySendError::Full(_)) => Ok(false),
        Err(mpsc::error::TrySendError::Closed(_)) => Err(()),
    }
}
fn reply(tx: &mpsc::Sender<Out>, value: Value) -> Result<(), ()> {
    try_reply(tx, value)?.then_some(()).ok_or(())
}
fn offer_packet(tx: &mpsc::Sender<Out>, packet: Packet) -> Result<Option<Packet>, ()> {
    match tx.try_send(Out::Frame(packet)) {
        Ok(()) => Ok(None),
        Err(mpsc::error::TrySendError::Full(Out::Frame(p))) => Ok(Some(p)),
        _ => Err(()),
    }
}
fn state_marker(
    s: &floe_app_core::view::Snapshot,
) -> (u64, floe_app_core::view::Phase, u64, u64, bool) {
    (
        s.state_rev,
        s.phase,
        s.submitted,
        s.consumed,
        s.margin_working,
    )
}

pub(crate) async fn socket(
    ws: WebSocket,
    gate: Gate,
    id: SessionId,
    attached: Arc<crate::transport::Attachment>,
) {
    let Ok(epoch) = public_id() else {
        return;
    };
    let mut stop = gate.stopping.subscribe();
    if !gate.alive(&id) || *stop.borrow() {
        return;
    }
    let _subscriber = attached.subscribe();
    let (mut sink, mut input) = ws.split();
    let (tx, mut output) = mpsc::channel::<Out>(4);
    let (sent_tx, mut sent_rx) = mpsc::channel::<u64>(1);
    let mut writer = tokio::spawn(async move {
        while let Some(item) = output.recv().await {
            match item {
                Out::Control(m) => {
                    if !matches!(
                        timeout(Duration::from_secs(5), sink.send(m)).await,
                        Ok(Ok(()))
                    ) {
                        break;
                    }
                }
                Out::Frame(p) => {
                    if !matches!(
                        timeout(Duration::from_secs(5), sink.send(Message::Binary(p.bytes))).await,
                        Ok(Ok(()))
                    ) {
                        break;
                    }
                    if sent_tx.send(p.id).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = timeout(Duration::from_millis(100), sink.close()).await;
    });
    let controller = &attached.controller;
    let initial = controller.snapshot();
    let hello = json!({"type":"hello","protocol":1,"bundle":BUNDLE,"session_id":id.as_str(),"connection_epoch":epoch,
        "view_id":attached.id,"title":attached.title,"frame_credit":1,"pending_frames":1});
    if reply(&tx, hello).is_err()
        || reply(
            &tx,
            view::snapshot(&initial, &controller.model, &attached.id, &epoch),
        )
        .is_err()
    {
        writer.abort();
        let _ = writer.await;
        return;
    }
    let mut last_state = state_marker(&initial);
    let (mut seq, mut last_frame, mut last_margin) = (0u64, 0u64, 0u64);
    let mut flight: Option<Flight> = None;
    let mut pending_packet: Option<Packet> = None;
    let mut queries = query::Queries::new(controller);
    let mut encoding: Option<JoinHandle<Result<Packet, &'static str>>> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (mut received, mut window, mut heartbeat) =
        (Instant::now(), Instant::now(), Instant::now());
    let mut messages = 0u32;
    let mut writer_finished = false;
    let mut closed_since: Option<Instant> = None;
    'socket: loop {
        tokio::select! {
            _=stop.changed()=>break,
            _=&mut writer=>{writer_finished=true;break;},
            Some(written)=sent_rx.recv()=>{
                let Some(f)=flight.as_mut() else {break;};if f.written(written).is_err(){break;}
                if f.complete(){if f.displayed{queries.displayed(f.receipt);}flight=None;}
            }
            result=async {encoding.as_mut().expect("guarded encoder").await}, if encoding.is_some()=>{
                encoding=None;
                match result {
                    Ok(Ok(packet))=>{
                        let state=controller.snapshot();
                        let current=if packet.margin {state.margin.is_some_and(|m|m.frame_id==packet.id)} else {packet.render_rev==state.render_rev};
                        if !current {flight=None;continue;}
                        // No added tick latency for the normal, writable case.
                        match offer_packet(&tx,packet) {Ok(p)=>pending_packet=p,Err(())=>break}
                    }
                    _=>break,
                }
            }
            _=tick.tick()=>{
                if !gate.alive(&id) || received.elapsed()>Duration::from_secs(30) || flight.as_ref().is_some_and(|f|f.since.elapsed()>ACK_TIMEOUT){break;}
                if heartbeat.elapsed()>Duration::from_secs(10) {
                    if tx.try_send(Out::Control(Message::Ping(Bytes::new()))).is_err(){break;}
                    heartbeat=Instant::now();
                }
                let state=controller.snapshot();
                if matches!(state.phase,floe_app_core::view::Phase::Closed|floe_app_core::view::Phase::Failed) {
                    // Let the last snapshot flush, then release this view's
                    // metadata instead of keeping closed views alive forever.
                    if closed_since.get_or_insert_with(Instant::now).elapsed()>Duration::from_millis(100){break;}
                }
                let marker=state_marker(&state);
                if marker!=last_state {
                    if reply(&tx,view::snapshot(&state,&controller.model,&attached.id,&epoch)).is_err(){break;}
                    last_state=marker;
                }
                if let Some(packet)=pending_packet.take() {
                    let current=if packet.margin {state.margin.is_some_and(|m|m.frame_id==packet.id)} else {packet.render_rev==state.render_rev};
                    if !current {flight=None;} else {
                        match offer_packet(&tx,packet) {Ok(p)=>pending_packet=p,Err(())=>break}
                    }
                }
                // Query replies do not wait for image encoder/byte admission.
                // Leave two slots for a revision edit's ACK+snapshot.
                for index in 0..2 {
                    if tx.capacity()<3 {break;}
                    if let Some(value)=queries.ready(index,&attached.id,&epoch) {
                        match try_reply(&tx,value) {
                            Ok(true)=>queries.sent(index),
                            Ok(false)=>break,
                            Err(())=>break 'socket,
                        }
                    }
                }
                if flight.is_none() {
                    if let Some(frame)=controller.latest().filter(|f|f.id!=last_frame && f.matches(&state))
                        .or_else(||controller.margin().filter(|f|f.id!=last_margin && f.matches(&state))) {
                        // All admission is try-only: there is no accumulating
                        // waiter/encode queue when another browser is slow.
                        let Ok(encoder)=Arc::clone(&gate.encoders).try_acquire_owned() else {continue;};
                        let header=match view::frame_header(&frame,&attached.id,&epoch) {Ok(h)=>h,Err(_)=>break};
                        // Account conservatively for native payload ownership,
                        // contiguous packet and tungstenite's write buffering.
                        let cost=frame.frame.bytes.len()*3+(header.len()+4)*2;
                        let Ok(reservation)=Arc::clone(&gate.output_bytes).try_acquire_many_owned(cost as u32) else {continue;};
                        let reservation=Arc::new(reservation);let owned=Arc::clone(&reservation);let frame_id=frame.id;
                        flight=Some(Flight{id:frame_id,receipt:query::Receipt::of(&frame),displayed:false,since:Instant::now(),written:false,acked:false,_reservation:reservation});
                        let margin=frame.purpose==floe_app_core::view::Purpose::Margin;
                        if margin {last_margin=frame_id;} else {last_frame=frame_id;}
                        encoding=Some(tokio::task::spawn_blocking(move || {
                            let _encoder=encoder;
                            let bytes=view::packet(&header,&frame.frame.bytes)?;
                            Ok(Packet{id:frame_id,render_rev:frame.render_rev,margin,bytes:Bytes::from(bytes),_reservation:owned})
                        }));
                    }
                }
            }
            incoming=input.next()=>{
                if !gate.alive(&id){break;}
                // A displayed ACK/query can win select! over a ready writer
                // notification. Consume that proof before testing its receipt.
                if let Ok(written)=sent_rx.try_recv() {
                    let Some(f)=flight.as_mut() else {break;};if f.written(written).is_err(){break;}
                    if f.complete(){if f.displayed{queries.displayed(f.receipt);}flight=None;}
                }
                if window.elapsed()>=Duration::from_secs(1){window=Instant::now();messages=0;}
                messages+=1;if messages>60{break;}received=Instant::now();
                let text=match incoming {
                    Some(Ok(Message::Text(t)))=>t,
                    Some(Ok(Message::Ping(p)))=>{if tx.try_send(Out::Control(Message::Pong(p))).is_err(){break;}continue;},
                    Some(Ok(Message::Pong(_)))=>continue,
                    _=>break,
                };
                let control=match serde_json::from_str::<Control>(&text){Ok(c)=>c,Err(_)=>break};
                let value=match &control {Control::Ping{seq}|Control::Set{seq,..}|Control::Apply{seq,..}|Control::Ack{seq,..}|Control::Query{seq,..}|Control::CancelQuery{seq,..}=>view::counter(seq)};
                let Ok(n)=value else {break;};if n<=seq{break;}seq=n;
                match control {
                    Control::Ping{seq}=>{if reply(&tx,json!({"type":"pong","seq":seq})).is_err(){break;}}
                    Control::Ack{connection_epoch,frame_id,disposition,..}=>{
                        if connection_epoch!=epoch{break;}
                        let Ok(frame_id)=view::counter(&frame_id) else {break;};
                        let Some(f)=flight.as_mut() else {break;};if f.acknowledge(frame_id).is_err(){break;}
                        f.displayed=matches!(disposition,Disposition::Displayed);
                        // A guessed early ACK cannot release byte credit before
                        // the independent writer confirms its write finished.
                        if f.complete(){if f.displayed{queries.displayed(f.receipt);}flight=None;}
                    }
                    Control::Query{seq,connection_epoch,view_id,body}=>{
                        if connection_epoch!=epoch||view_id!=attached.id{break;}
                        let event=match queries.submit(seq.clone(),*body) {
                            Ok(query_id)=>json!({"type":"query.accepted","seq":seq,"view_id":attached.id,"connection_epoch":epoch,"query_id":query_id.to_string()}),
                            Err(code)=>json!({"type":"error","seq":seq,"code":code}),
                        };
                        if reply(&tx,event).is_err(){break;}
                    }
                    Control::CancelQuery{seq,connection_epoch,view_id,kind}=>{
                        if connection_epoch!=epoch||view_id!=attached.id{break;}
                        queries.cancel(kind);
                        if reply(&tx,json!({"type":"query.cancelled","seq":seq,"view_id":attached.id,"connection_epoch":epoch,"kind":kind.name()})).is_err(){break;}
                    }
                    Control::Set{seq,connection_epoch,view_id,base_state_rev,body}=>{
                        if connection_epoch!=epoch||view_id!=attached.id{break;}
                        let Ok(base)=view::counter(&base_state_rev) else {break;};
                        let outcome=body.core().map_err(|_|"invalid_request").and_then(|patch|
                            controller.edit(base,patch).map_err(|e|if e.kind==floe_app_core::ErrorKind::Busy{"stale_state"}else{view::safe_error(e.kind)}));
                        let state=controller.snapshot();
                        let event=match outcome {
                            Ok(accepted)=>json!({"type":"accepted","seq":seq,"state_rev":accepted.state_rev.to_string(),"render_rev":accepted.render_rev.to_string()}),
                            Err(code)=>json!({"type":"error","seq":seq,"code":code}),
                        };
                        if reply(&tx,event).is_err() || reply(&tx,view::snapshot(&state,&controller.model,&attached.id,&epoch)).is_err(){break;}
                        last_state=state_marker(&state);
                    }
                    Control::Apply{seq,connection_epoch,view_id,base_state_rev,token}=>{
                        if connection_epoch!=epoch||view_id!=attached.id{break;}
                        let Ok(base)=view::counter(&base_state_rev) else {break;};
                        let outcome=attached.prepared.lock().unwrap().apply(&token,base,controller);
                        let state=controller.snapshot();
                        let event=match outcome {
                            Ok(accepted)=>json!({"type":"accepted","seq":seq,"state_rev":accepted.state_rev.to_string(),"render_rev":accepted.render_rev.to_string()}),
                            Err(code)=>json!({"type":"error","seq":seq,"code":code}),
                        };
                        if reply(&tx,event).is_err() || reply(&tx,view::snapshot(&state,&controller.model,&attached.id,&epoch)).is_err(){break;}
                        last_state=state_marker(&state);
                    }
                }
            }
        }
    }
    if let Some(task) = encoding {
        task.abort();
    }
    if !writer_finished {
        writer.abort();
        let _ = writer.await;
    }
    // An already-running blocking copy is bounded by the 2 encoder + byte
    // reservations, retains its reservation until it drops, and does no IO.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn full_writer_preserves_packet_and_credit_without_copy_or_extra_queue() {
        let budget = Arc::new(tokio::sync::Semaphore::new(100));
        let permit = Arc::new(Arc::clone(&budget).try_acquire_many_owned(100).unwrap());
        let packet = Packet {
            id: 1,
            render_rev: 1,
            margin: false,
            bytes: Bytes::from_static(b"pixels"),
            _reservation: permit,
        };
        let address = packet.bytes.as_ptr();
        let (tx, mut rx) = mpsc::channel(1);
        assert_eq!(try_reply(&tx, json!({"type":"test"})), Ok(true));
        assert_eq!(try_reply(&tx, json!({"type":"another"})), Ok(false));
        assert!(try_reply(&tx, json!("x".repeat(view::CONTROL_REPLY_BYTES))).is_err());
        let pending = offer_packet(&tx, packet).unwrap().unwrap();
        assert_eq!(pending.bytes.as_ptr(), address);
        assert_eq!(budget.available_permits(), 0);
        assert!(matches!(rx.try_recv().unwrap(), Out::Control(_)));
        assert!(offer_packet(&tx, pending).unwrap().is_none());
        assert_eq!(budget.available_permits(), 0);
        let Out::Frame(sent) = rx.try_recv().unwrap() else {
            panic!("frame expected")
        };
        assert_eq!(sent.bytes.as_ptr(), address);
        drop(sent);
        assert_eq!(budget.available_permits(), 100);
    }
    #[test]
    fn disposition_is_a_string_not_an_externally_tagged_object() {
        for value in [r#"{"displayed":null}"#, "null", "0", r#""other""#] {
            assert!(serde_json::from_str::<Disposition>(value).is_err());
        }
        assert!(serde_json::from_str::<Disposition>(r#""displayed""#).is_ok());
        assert!(serde_json::from_str::<Disposition>(r#""discarded""#).is_ok());
    }
    #[tokio::test]
    async fn ack_and_write_are_both_required_for_credit() {
        let budget = Arc::new(tokio::sync::Semaphore::new(100));
        for early_ack in [true, false] {
            let reservation = Arc::new(Arc::clone(&budget).try_acquire_many_owned(100).unwrap());
            let mut f = Flight {
                id: 5,
                receipt: query::Receipt::default(),
                displayed: false,
                since: Instant::now(),
                written: false,
                acked: false,
                _reservation: reservation,
            };
            assert!(f.acknowledge(6).is_err());
            if early_ack {
                f.acknowledge(5).unwrap();
            } else {
                f.written(5).unwrap();
            }
            assert!(!f.complete());
            assert_eq!(budget.available_permits(), 0);
            if early_ack {
                f.written(5).unwrap();
            } else {
                f.acknowledge(5).unwrap();
            }
            assert!(f.complete());
            assert!(f.acknowledge(5).is_err());
            assert!(f.written(5).is_err());
            drop(f);
            assert_eq!(budget.available_permits(), 100);
        }
    }
}
