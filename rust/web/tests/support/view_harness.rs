// Common real owner HTTP/WS/native harness. Only private synthetic fixtures.
use floe_app_core::{
    jobdeck::color::Mode,
    managed::{Limits, ManagedDataset, Resources, Usage},
    render::RenderOptions,
    shots::Detail,
    view::{Model, ViewController, ViewState},
};
use floe_web::{
    auth::Secret,
    transport::{self, Gateway, BUNDLE, PROTOCOL},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{atomic::AtomicUsize, Arc},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
struct Harness {
    addr: SocketAddr,
    gate: Arc<Gateway>,
    bootstrap: Secret,
    controller: Arc<ViewController>,
    resources: Arc<Resources>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}
struct Login {
    cookie: String,
    csrf: String,
}
impl Harness {
    async fn start(raw: bool) -> Self {
        Self::start_configured(raw, false, false).await
    }
    async fn start_configured(raw: bool, margin: bool, labels: bool) -> Self {
        let resources = Resources::new(Limits::default()).unwrap();
        let source =
            PathBuf::from(std::env::var_os("FLOE_VIEW_FIXTURE").expect("private fixture required"));
        let data =
            ManagedDataset::open(&resources, &source, None, Mode::Level, &AtomicUsize::new(0))
                .unwrap();
        let model = Model::new(&data).unwrap();
        let mut state = ViewState::initial(&model, 257, 191).unwrap();
        state.labels = labels;
        state.detail = Detail::High;
        let mut options = RenderOptions::local().unwrap();
        options.decode_jobs = 1;
        options.raster_jobs = 1;
        options.budget_mb = 64;
        options.raw = raw;
        let controller = Arc::new(
            ViewController::start_configured(
                &resources,
                data,
                options,
                state,
                floe_app_core::view::ControllerOptions {
                    margin_prefetch: margin,
                    frame_cache: true,
                },
            )
            .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (gate, bootstrap) =
            Gateway::with_view(addr, Arc::clone(&controller), "synthetic <valmini>").unwrap();
        let (stop, rx) = oneshot::channel();
        let task = tokio::spawn(transport::serve(listener, Arc::clone(&gate), async {
            let _ = rx.await;
        }));
        Self {
            addr,
            gate,
            bootstrap,
            controller,
            resources,
            stop,
            task,
        }
    }
    async fn http(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, String) {
        let mut s = TcpStream::connect(self.addr).await.unwrap();
        let mut req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.addr,
            body.len()
        );
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str("\r\n");
        req.push_str(body);
        s.write_all(req.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        timeout(Duration::from_secs(7), s.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            head.into(),
            body.into(),
        )
    }
    async fn login(&self) -> Login {
        let (status, headers, body) = self
            .http(
                "POST",
                "/api/v1/session/exchange",
                &[
                    ("Origin", &format!("http://{}", self.addr)),
                    ("Content-Type", "application/json"),
                ],
                &json!({"bootstrap":self.bootstrap.expose(),"protocol":1,"bundle":BUNDLE})
                    .to_string(),
            )
            .await;
        assert_eq!(status, 200);
        let body: Value = serde_json::from_str(&body).unwrap();
        Login {
            cookie: headers
                .lines()
                .find_map(|l| l.strip_prefix("set-cookie: "))
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .into(),
            csrf: body["csrf"].as_str().unwrap().into(),
        }
    }
    async fn connect(&self, login: &Login) -> (Socket, Value, Value) {
        let mut r = format!("ws://{}/api/v1/events", self.addr)
            .into_client_request()
            .unwrap();
        r.headers_mut()
            .insert("origin", format!("http://{}", self.addr).parse().unwrap());
        r.headers_mut()
            .insert("cookie", login.cookie.parse().unwrap());
        r.headers_mut().insert(
            "sec-websocket-protocol",
            format!("{PROTOCOL}, bundle.{BUNDLE}, csrf.{}", login.csrf)
                .parse()
                .unwrap(),
        );
        let (mut ws, _) = connect_async(r).await.unwrap();
        let hello = next_json(&mut ws).await;
        let state = next_json(&mut ws).await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(state["type"], "snapshot");
        assert_eq!(hello["connection_epoch"], state["connection_epoch"]);
        (ws, hello, state)
    }
    async fn shutdown(self) {
        let _ = self.stop.send(());
        timeout(Duration::from_secs(6), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(self.controller.is_finished());
        assert_eq!(self.resources.usage(), Usage::default());
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.gate.transport_usage().reserved_output_bytes != 0
            || self.gate.transport_usage().encoders != 0
        {
            assert!(
                Instant::now() < deadline,
                "packet reservation leak {:?}",
                self.gate.transport_usage()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(self.gate.transport_usage().sockets, 0);
    }
}
async fn next(ws: &mut Socket) -> Message {
    loop {
        match timeout(Duration::from_secs(12), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            Message::Ping(p) => {
                ws.send(Message::Pong(p)).await.unwrap();
            }
            Message::Pong(_) => (),
            m => return m,
        }
    }
}
async fn next_json(ws: &mut Socket) -> Value {
    match next(ws).await {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected JSON, {other:?}"),
    }
}
async fn frame(ws: &mut Socket) -> (Value, Vec<u8>) {
    loop {
        match next(ws).await {
            Message::Binary(b) => {
                assert!(b.len() >= 4);
                let n = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
                assert!(n <= floe_web::view::HEADER_BYTES);
                assert!(b.len() >= 4 + n);
                let h: Value = serde_json::from_slice(&b[4..4 + n]).unwrap();
                let body = b[4 + n..].to_vec();
                assert_eq!(body.len().to_string(), h["payload_length"]);
                assert_eq!(h["row0"], "top");
                assert_eq!(h["query"], true);
                assert_eq!(h["query_scene"]["complete"], true);
                return (h, body);
            }
            Message::Text(t) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                assert_ne!(v["type"], "error", "{v}");
            }
            other => panic!("expected frame, {other:?}"),
        }
    }
}
async fn ack(ws: &mut Socket, hello: &Value, seq: u64, header: &Value) {
    ws.send(Message::Text(json!({"type":"frame.ack","seq":seq.to_string(),"connection_epoch":hello["connection_epoch"],
        "frame_id":header["frame_id"],"disposition":"displayed"}).to_string().into())).await.unwrap();
}
async fn until_reply(ws: &mut Socket, seq: u64, kind: &str) -> Value {
    let expected = seq.to_string();
    loop {
        let v = next_json(ws).await;
        if v["seq"] == expected {
            assert_eq!(v["type"], kind, "{v}");
            return v;
        }
    }
}
async fn closed(ws: &mut Socket) {
    loop {
        match timeout(Duration::from_secs(12), ws.next()).await.unwrap() {
            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
            Some(Ok(Message::Ping(p))) => {
                let _ = ws.send(Message::Pong(p)).await;
            }
            _ => (),
        }
    }
}
