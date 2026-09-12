//! One in-flight image, latest available controller frame as the pending slot.
//! Reader/control, bounded packet encoding and socket writer are independent.
use crate::{
    auth::{public_id, SessionId},
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
    bytes: Bytes,
    _reservation: Arc<OwnedSemaphorePermit>,
}
enum Out {
    Control(Message),
    Frame(Packet),
}
struct Flight {
    id: u64,
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
    #[serde(rename = "frame.ack")]
    Ack {
        seq: String,
        connection_epoch: String,
        frame_id: String,
        disposition: Disposition,
    },
}
fn reply(tx: &mpsc::Sender<Out>, value: Value) -> Result<(), ()> {
    let text = value.to_string();
    if text.len() > view::CONTROL_REPLY_BYTES {
        return Err(());
    }
    tx.try_send(Out::Control(Message::Text(text.into())))
        .map_err(|_| ())
}
fn state_marker(s: &floe_app_core::view::Snapshot) -> (u64, floe_app_core::view::Phase, u64, u64) {
    (s.state_rev, s.phase, s.submitted, s.consumed)
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
    let (mut seq, mut last_frame) = (0u64, 0u64);
    let mut flight: Option<Flight> = None;
    let mut encoding: Option<JoinHandle<Result<Packet, &'static str>>> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (mut received, mut window, mut heartbeat) =
        (Instant::now(), Instant::now(), Instant::now());
    let mut messages = 0u32;
    let mut writer_finished = false;
    let mut closed_since: Option<Instant> = None;
    loop {
        tokio::select! {
            _=stop.changed()=>break,
            _=&mut writer=>{writer_finished=true;break;},
            Some(written)=sent_rx.recv()=>{
                let Some(f)=flight.as_mut() else {break;};if f.written(written).is_err(){break;}
                if f.complete(){flight=None;}
            }
            result=async {encoding.as_mut().expect("guarded encoder").await}, if encoding.is_some()=>{
                encoding=None;
                match result {
                    Ok(Ok(packet))=>{
                        if packet.render_rev!=controller.snapshot().render_rev {flight=None;continue;}
                        if tx.try_send(Out::Frame(packet)).is_err(){break;}
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
                if flight.is_none() {
                    if let Some(frame)=controller.latest().filter(|f|f.id!=last_frame && f.render_rev==state.render_rev) {
                        // All admission is try-only: there is no accumulating
                        // waiter/encode queue when another browser is slow.
                        let Ok(encoder)=Arc::clone(&gate.encoders).try_acquire_owned() else {continue;};
                        let header=match view::frame_header(&frame,&attached.id,&epoch) {Ok(h)=>h,Err(_)=>break};
                        // Account conservatively for native payload ownership,
                        // contiguous packet and tungstenite's write buffering.
                        let cost=frame.frame.bytes.len()*3+(header.len()+4)*2;
                        let Ok(reservation)=Arc::clone(&gate.output_bytes).try_acquire_many_owned(cost as u32) else {continue;};
                        let reservation=Arc::new(reservation);let owned=Arc::clone(&reservation);let frame_id=frame.id;
                        flight=Some(Flight{id:frame_id,since:Instant::now(),written:false,acked:false,_reservation:reservation});
                        last_frame=frame_id;
                        encoding=Some(tokio::task::spawn_blocking(move || {
                            let _encoder=encoder;
                            let bytes=view::packet(&header,&frame.frame.bytes)?;
                            Ok(Packet{id:frame_id,render_rev:frame.render_rev,bytes:Bytes::from(bytes),_reservation:owned})
                        }));
                    }
                }
            }
            incoming=input.next()=>{
                if !gate.alive(&id){break;}
                if window.elapsed()>=Duration::from_secs(1){window=Instant::now();messages=0;}
                messages+=1;if messages>60{break;}received=Instant::now();
                let text=match incoming {
                    Some(Ok(Message::Text(t)))=>t,
                    Some(Ok(Message::Ping(p)))=>{if tx.try_send(Out::Control(Message::Pong(p))).is_err(){break;}continue;},
                    Some(Ok(Message::Pong(_)))=>continue,
                    _=>break,
                };
                let control=match serde_json::from_str::<Control>(&text){Ok(c)=>c,Err(_)=>break};
                let value=match &control {Control::Ping{seq}|Control::Set{seq,..}|Control::Ack{seq,..}=>view::counter(seq)};
                let Ok(n)=value else {break;};if n<=seq{break;}seq=n;
                match control {
                    Control::Ping{seq}=>{if reply(&tx,json!({"type":"pong","seq":seq})).is_err(){break;}}
                    Control::Ack{connection_epoch,frame_id,disposition,..}=>{
                        let _=disposition;
                        if connection_epoch!=epoch{break;}
                        let Ok(frame_id)=view::counter(&frame_id) else {break;};
                        let Some(f)=flight.as_mut() else {break;};if f.acknowledge(frame_id).is_err(){break;}
                        // A guessed early ACK cannot release byte credit before
                        // the independent writer confirms its write finished.
                        if f.complete(){flight=None;}
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
