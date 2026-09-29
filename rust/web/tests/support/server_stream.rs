//! HTTPS proof/cookie/protocol are simulated over loopback. Native pixels and
//! actual HTTP/WebSocket transports are real; no public TLS or browser test.
use super::*;
use floe_web::{broker, transport::BUNDLE};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
const ROOT: &str = "/api/v1/server/sessions";
struct Server {
    addr: std::net::SocketAddr,
    broker: Arc<Broker>,
    resources: Arc<Resources>,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    _fixture: Fixture,
}
struct Session {
    id: String,
    cookie: String,
    csrf: String,
}
impl Session {
    fn headers(&self) -> HeaderMap {
        let mut h = headers();
        h.remove("x-floe-delegation-key");
        h.remove("x-floe-service-client");
        h.insert("cookie", self.cookie.parse().unwrap());
        h.insert("x-floe-csrf", self.csrf.parse().unwrap());
        h
    }
    fn path(&self, tail: &str) -> String {
        format!("{ROOT}/{}{tail}", self.id)
    }
}
impl Server {
    async fn start(ttl: Duration) -> Self {
        let fixture = Fixture::new();
        let resources = resources();
        indexed(fixture.index(&resources, "a.oas"));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let broker = fixture.broker_at(
            addr,
            Lifetimes {
                bootstrap: Duration::from_secs(120),
                session: ttl,
            },
        );
        let runtime = Runtime::new(Arc::clone(&broker), Arc::clone(&resources), options()).unwrap();
        let (stop, done) = oneshot::channel();
        let task = tokio::spawn(broker::serve_runtime(listener, runtime, async {
            let _ = done.await;
        }));
        Self {
            addr,
            broker,
            resources,
            stop: Some(stop),
            task,
            _fixture: fixture,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        h: HeaderMap,
        body: Value,
    ) -> (u16, HeaderMap, Value) {
        Self::request_at(self.addr, &self._fixture.0, method, path, h, body).await
    }
    async fn request_at(
        addr: std::net::SocketAddr,
        fixture: &Path,
        method: &str,
        path: &str,
        h: HeaderMap,
        body: Value,
    ) -> (u16, HeaderMap, Value) {
        let body = if body.is_null() {
            String::new()
        } else {
            body.to_string()
        };
        let mut wire=format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",body.len());
        for (k, v) in &h {
            wire.push_str(&format!("{k}: {}\r\n", v.to_str().unwrap()));
        }
        wire.push_str("\r\n");
        wire.push_str(&body);
        let mut tcp = TcpStream::connect(addr).await.unwrap();
        tcp.write_all(wire.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        timeout(Duration::from_secs(5), tcp.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let split = bytes.windows(4).position(|p| p == b"\r\n\r\n").unwrap();
        let mut lines = std::str::from_utf8(&bytes[..split]).unwrap().lines();
        let status = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let mut headers = HeaderMap::new();
        for line in lines {
            let (k, v) = line.split_once(':').unwrap();
            headers.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.trim().parse().unwrap(),
            );
        }
        let body = serde_json::from_slice(&bytes[split + 4..]).unwrap_or(Value::Null);
        assert!(!String::from_utf8_lossy(&bytes[split + 4..]).contains(fixture.to_str().unwrap()));
        (status, headers, body)
    }
    async fn login(&self, user: &str) -> Session {
        let (status, _, l) = self
            .request(
                "POST",
                "/api/v1/server/launches",
                headers(),
                json!({"user_id":user,"source":"a.oas"}),
            )
            .await;
        assert_eq!(status, 201);
        let id = l["launch_id"].as_str().unwrap().to_owned();
        let (status, h, v) = self
            .request(
                "POST",
                &format!("{ROOT}/{id}/exchange"),
                headers(),
                json!({"bootstrap":l["bootstrap"]}),
            )
            .await;
        assert_eq!(status, 200);
        assert_eq!(v["render_transport"], true);
        assert_eq!(v["viewer_ready"], true);
        Session {
            id,
            cookie: h["set-cookie"]
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .into(),
            csrf: v["csrf"].as_str().unwrap().into(),
        }
    }
    async fn open(&self, s: &Session) {
        let (status, _, view) = self
            .request("GET", &s.path("/view"), s.headers(), Value::Null)
            .await;
        assert_eq!(status, 200);
        assert_eq!(view, json!({"type":"unopened","view_id":s.id}));
        assert_eq!(
            self.request(
                "POST",
                &s.path("/view"),
                s.headers(),
                json!({"width":96,"height":64})
            )
            .await
            .0,
            202
        );
    }
    async fn connect(&self, s: &Session) -> (Socket, Value) {
        let deadline = Instant::now() + Duration::from_secs(3);
        let (mut ws, response) = loop {
            match connect_async(self.ws_request(s)).await {
                Ok(pair) => break pair,
                Err(tungstenite::Error::Http(r))
                    if r.status().as_u16() == 429 && Instant::now() < deadline =>
                {
                    sleep(Duration::from_millis(10)).await
                }
                Err(_) => panic!("valid server stream handshake failed"),
            }
        };
        assert_eq!(
            response.headers()["sec-websocket-protocol"],
            broker::PROTOCOL
        );
        let message = next(&mut ws).await;
        let Message::Text(text) = message else {
            panic!("hello required")
        };
        let hello: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["view_id"], s.id);
        (ws, hello)
    }
    fn ws_request(&self, s: &Session) -> axum::http::Request<()> {
        let mut req = format!("ws://{}{}", self.addr, s.path("/stream"))
            .into_client_request()
            .unwrap();
        for (k, v) in s.headers() {
            if let Some(k) = k {
                if k != "x-floe-csrf" {
                    req.headers_mut().insert(k, v);
                }
            }
        }
        req.headers_mut().insert(
            "sec-websocket-protocol",
            format!("{}, bundle.{BUNDLE}, csrf.{}", broker::PROTOCOL, s.csrf)
                .parse()
                .unwrap(),
        );
        req
    }
    async fn shutdown(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        timeout(Duration::from_secs(8), &mut self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(self.broker.pending_workers().unwrap(), 0);
        assert_eq!(self.resources.usage(), Usage::default());
    }
}

#[tokio::test]
#[ignore = "synthetic native server_runtime gate"]
async fn public_demo_cli_prepares_renders_enforces_caps_and_reaps_on_hup() {
    demo_cli_transport(false).await;
}
#[tokio::test]
#[ignore = "synthetic native server_runtime gate"]
async fn http_demo_cli_prepares_renders_enforces_caps_and_reaps_on_hup() {
    demo_cli_transport(true).await;
}
async fn demo_cli_transport(http_test: bool) {
    use std::{
        os::unix::fs::PermissionsExt,
        process::{Command, Stdio},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let f = Fixture::new();
    let origin = if http_test {
        "http://10.0.0.10:8080"
    } else {
        "https://service.example.test"
    };
    let authority = if http_test {
        "10.0.0.10:8080"
    } else {
        "service.example.test"
    };
    let headers = || {
        let mut h = super::headers();
        h.insert("host", authority.parse().unwrap());
        h.insert("origin", origin.parse().unwrap());
        h
    };
    let config = f.0.join("demo.json");
    let key = f.0.join("proxy.key");
    fs::write(
        &config,
        serde_json::to_vec(&json!({"version":1,"public_origin":origin,
        "runtime_root":f.0.join("runtime"),"max_sessions":2,"deployment":{"mode":"public_demo",
        "allow_insecure_http":http_test,
        "data_root":f.0.join("data"),"samples":[{"id":"demo1","source":"a.oas"}]}}))
        .unwrap(),
    )
    .unwrap();
    fs::write(&key, PROXY).unwrap();
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
    let app = binary().with_file_name("floe2-web");
    let command = || {
        let mut c = Command::new(&app);
        c.env("FLOE_INDEX_BIN", binary())
            .env("FLOE_RENDERD_BIN", binary().with_file_name("floe-renderd"));
        c
    };
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = reservation.local_addr().unwrap();
    let port = addr.port().to_string();
    let mut start = command();
    start
        .args(["server", "--demo"])
        .arg(&config)
        .arg("--proxy-key-file")
        .arg(&key)
        .args(["--port", &port]);
    let before = files(&f.0.join("data"));
    let missing = start.output().unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--prepare-demo"));
    assert_eq!(
        files(&f.0.join("data")),
        before,
        "serving cannot build missing indexes"
    );
    let prepared = command()
        .args(["server", "--prepare-demo"])
        .arg(&config)
        .args(["--jobs", "1"])
        .output()
        .unwrap();
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let before = files(&f.0.join("data"));
    let log = f.0.join("server.log");
    let output = fs::File::create(&log).unwrap();
    drop(reservation);
    let mut child = Child(start.stdout(Stdio::null()).stderr(output).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "{}",
            fs::read_to_string(&log).unwrap()
        );
        if TcpStream::connect(addr).await.is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "demo CLI startup timeout");
        sleep(Duration::from_millis(20)).await;
    }
    let (code, _, list) = Server::request_at(
        addr,
        &f.0,
        "GET",
        "/api/v1/demo/samples",
        headers(),
        Value::Null,
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(list, json!({"samples":["demo1"]}));
    let (code, _, launch) = Server::request_at(
        addr,
        &f.0,
        "POST",
        "/api/v1/demo/launches",
        headers(),
        json!({"sample_id":"demo1"}),
    )
    .await;
    assert_eq!(code, 201);
    let id = launch["launch_id"].as_str().unwrap().to_string();
    let (code, h, v) = Server::request_at(
        addr,
        &f.0,
        "POST",
        &format!("{ROOT}/{id}/exchange"),
        headers(),
        json!({"bootstrap":launch["bootstrap"]}),
    )
    .await;
    assert_eq!(code, 200);
    let set_cookie = h["set-cookie"].to_str().unwrap();
    assert_eq!(set_cookie.contains("; Secure;"), !http_test);
    assert_eq!(set_cookie.starts_with("floe_http_test_"), http_test);
    let s = Session {
        id,
        cookie: h["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .into(),
        csrf: v["csrf"].as_str().unwrap().into(),
    };
    let session_headers = || {
        let mut h = headers();
        h.insert("cookie", s.cookie.parse().unwrap());
        h.insert("x-floe-csrf", s.csrf.parse().unwrap());
        h
    };
    assert_eq!(
        Server::request_at(
            addr,
            &f.0,
            "POST",
            &s.path("/view"),
            session_headers(),
            json!({"width":2048,"height":2048})
        )
        .await
        .0,
        400
    );
    assert_eq!(
        Server::request_at(
            addr,
            &f.0,
            "POST",
            &s.path("/view"),
            session_headers(),
            json!({"width":96,"height":64})
        )
        .await
        .0,
        202
    );
    let mut req = format!("ws://{addr}{}", s.path("/stream"))
        .into_client_request()
        .unwrap();
    for (k, v) in session_headers() {
        if let Some(k) = k {
            if k != "x-floe-csrf" {
                req.headers_mut().insert(k, v);
            }
        }
    }
    req.headers_mut().insert(
        "sec-websocket-protocol",
        format!("{}, bundle.{BUNDLE}, csrf.{}", broker::PROTOCOL, s.csrf)
            .parse()
            .unwrap(),
    );
    let (mut ws, _) = connect_async(req).await.unwrap();
    let Message::Text(t) = next(&mut ws).await else {
        panic!("hello required")
    };
    let hello: Value = serde_json::from_str(&t).unwrap();
    assert_eq!(hello["type"], "hello");
    let (f1, png) = frame(&mut ws).await;
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(f1["complete"], true);
    ack(&mut ws, &hello, &f1, 1).await;
    send(&mut ws,json!({"type":"view.set","seq":"2","connection_epoch":hello["connection_epoch"],"view_id":s.id,
        "base_state_rev":f1["state_rev"],"body":{"pixels":[2048,2048]}})).await;
    loop {
        let Message::Text(t) = next(&mut ws).await else {
            panic!("error required")
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        if v["type"] == "error" {
            break;
        }
        assert_eq!(v["type"], "snapshot");
    }
    // Exercise the exact read-only UI band command through the real public
    // transport and native renderer, not just the standalone navigation DTO.
    let span = |f: &Value| {
        f["bbox_dbu"][2].as_str().unwrap().parse::<f64>().unwrap()
            - f["bbox_dbu"][0].as_str().unwrap().parse::<f64>().unwrap()
    };
    let mut previous = f1;
    for (i, outward) in [false, true].into_iter().enumerate() {
        let seq = (3 + 2 * i).to_string();
        let end_x = if outward { 0.25 } else { 0.75 };
        send(&mut ws,json!({"type":"view.set","seq":seq,"connection_epoch":hello["connection_epoch"],"view_id":s.id,
            "base_state_rev":previous["state_rev"],"body":{"navigation":{"kind":"band","start":[0.5,0.25],
            "end":[end_x,0.75],"axes":[true,true],"outward":outward}}})).await;
        loop {
            let Message::Text(t) = next(&mut ws).await else {
                panic!("band acceptance required")
            };
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["type"] == "accepted" {
                assert_eq!(v["seq"], seq);
                break;
            }
            assert_eq!(v["type"], "snapshot", "{v}");
        }
        let (next, png) = frame(&mut ws).await;
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(next["complete"], true);
        assert_eq!(next["render_key"], previous["render_key"]);
        assert_ne!(next["render_rev"], previous["render_rev"]);
        assert_eq!(span(&next) > span(&previous), outward);
        assert_ne!(span(&next), span(&previous));
        ack(&mut ws, &hello, &next, (4 + 2 * i) as u32).await;
        previous = next;
    }
    assert_eq!(
        files(&f.0.join("data")),
        before,
        "demo rendering does not mutate source/index bytes"
    );
    assert!(Command::new("kill")
        .args(["-HUP", &child.0.id().to_string()])
        .status()
        .unwrap()
        .success());
    ended(&mut ws).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert_eq!(status.code(), Some(129));
            break;
        }
        assert!(Instant::now() < deadline, "demo shutdown did not reap");
        sleep(Duration::from_millis(20)).await;
    }
    let text = fs::read_to_string(log).unwrap();
    assert_eq!(
        text.contains("WARNING: HTTP demo test mode is unencrypted"),
        http_test
    );
    assert!(
        !text.contains(PROXY)
            && !text.contains(s.csrf.as_str())
            && !text.contains(launch["bootstrap"].as_str().unwrap())
    );
    assert_eq!(files(&f.0.join("data")), before);
}
async fn next(ws: &mut Socket) -> Message {
    loop {
        let m = timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match m {
            Message::Ping(p) => {
                ws.send(Message::Pong(p)).await.unwrap();
            }
            _ => return m,
        }
    }
}
async fn frame(ws: &mut Socket) -> (Value, Vec<u8>) {
    loop {
        match next(ws).await {
            Message::Binary(b) => {
                let n = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
                let h: Value = serde_json::from_slice(&b[4..4 + n]).unwrap();
                assert_eq!(h["query"], false);
                assert_eq!(h["query_scene"]["complete"], false);
                return (h, b[4 + n..].to_vec());
            }
            Message::Text(t) => {
                let v: Value = serde_json::from_str(&t).unwrap();
                assert!(
                    matches!(v["type"].as_str(), Some("snapshot" | "opening")),
                    "{v}"
                );
            }
            _ => panic!("stream ended before frame"),
        }
    }
}
async fn send(ws: &mut Socket, value: Value) {
    ws.send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}
async fn ack(ws: &mut Socket, h: &Value, f: &Value, seq: u32) {
    send(ws,json!({"type":"frame.ack","seq":seq.to_string(),"connection_epoch":h["connection_epoch"],"frame_id":f["frame_id"]})).await;
}
async fn ended(ws: &mut Socket) {
    timeout(Duration::from_secs(4), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(Message::Ping(p))) => {
                    let _ = ws.send(Message::Pong(p)).await;
                }
                _ => (),
            }
        }
    })
    .await
    .unwrap();
}
async fn denied(req: axum::http::Request<()>, status: u16) {
    match connect_async(req).await {
        Err(tungstenite::Error::Http(r)) => assert_eq!(r.status().as_u16(), status),
        _ => panic!("expected denied upgrade {status}"),
    }
}

#[tokio::test]
#[ignore = "synthetic native server_runtime gate"]
async fn sessions_stream_independent_frames_reconnect_and_reject_cross_user_input() {
    let s = Server::start(Duration::from_secs(60)).await;
    let a = s.login("alice").await;
    let b = s.login("bob").await;
    s.open(&a).await;
    s.open(&b).await;
    let (mut aw, ah) = s.connect(&a).await;
    let (af, ab) = frame(&mut aw).await;
    let (mut bw, bh) = s.connect(&b).await;
    let (bf, bb) = frame(&mut bw).await;
    assert_eq!(ab, bb);
    assert_ne!(af["worker_epoch"], bf["worker_epoch"]);
    denied(s.ws_request(&a), 429).await;
    ack(&mut aw, &ah, &af, 1).await;
    ack(&mut bw, &bh, &bf, 1).await;
    send(&mut aw,json!({"type":"view.set","seq":"2","connection_epoch":ah["connection_epoch"],"view_id":a.id,"base_state_rev":af["state_rev"],"body":{"navigation":{"kind":"pan","x":0.5,"y":0.,"snap":false}}})).await;
    loop {
        let Message::Text(t) = next(&mut aw).await else {
            panic!("edit ACK precedes image")
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        if v["type"] == "accepted" {
            break;
        }
        assert_eq!(v["type"], "snapshot");
    }
    let (moved, _) = frame(&mut aw).await;
    assert_ne!(moved["bbox_dbu"], af["bbox_dbu"]);
    assert_eq!(
        s.request("GET", &b.path("/view"), b.headers(), Value::Null)
            .await
            .2["bbox_dbu"],
        bf["bbox_dbu"]
    );
    ack(&mut aw, &ah, &moved, 3).await;
    aw.close(None).await.unwrap();
    drop(aw);
    sleep(Duration::from_millis(80)).await;
    let (mut aw, new_hello) = s.connect(&a).await;
    assert_ne!(new_hello["connection_epoch"], ah["connection_epoch"]);
    let (again, _) = frame(&mut aw).await;
    assert_eq!(again["worker_epoch"], af["worker_epoch"]);
    assert_eq!(again["bbox_dbu"], moved["bbox_dbu"]);
    ack(&mut aw, &new_hello, &again, 1).await;
    // New socket cannot replay an old epoch or target Bob, even with Alice's
    // otherwise valid cookie and freshly sequenced commands.
    send(&mut aw,json!({"type":"view.set","seq":"2","connection_epoch":ah["connection_epoch"],"view_id":a.id,"base_state_rev":again["state_rev"],"body":{"navigation":{"kind":"fit"}}})).await;
    ended(&mut aw).await;
    let (mut aw, new_hello) = s.connect(&a).await;
    let (again, _) = frame(&mut aw).await;
    assert_eq!(again["bbox_dbu"], moved["bbox_dbu"]);
    ack(&mut aw, &new_hello, &again, 1).await;
    send(&mut aw,json!({"type":"view.set","seq":"2","connection_epoch":new_hello["connection_epoch"],"view_id":b.id,"base_state_rev":again["state_rev"],"body":{"navigation":{"kind":"fit"}}})).await;
    ended(&mut aw).await;
    assert_eq!(
        s.request("GET", &b.path("/view"), b.headers(), Value::Null)
            .await
            .2["bbox_dbu"],
        bf["bbox_dbu"]
    );
    assert_eq!(
        s.request("DELETE", &a.path(""), a.headers(), Value::Null)
            .await
            .0,
        204
    );
    send(&mut bw, json!({"type":"ping","seq":"2"})).await;
    loop {
        let Message::Text(t) = next(&mut bw).await else {
            panic!("expected pong")
        };
        if serde_json::from_str::<Value>(&t).unwrap()["type"] == "pong" {
            break;
        }
    }
    assert_eq!(
        s.request("GET", &a.path("/view"), a.headers(), Value::Null)
            .await
            .0,
        401
    );
    s.shutdown().await;
    ended(&mut bw).await;
}

#[tokio::test]
#[ignore = "synthetic native server_runtime gate"]
async fn stream_auth_origin_csrf_bundle_and_http_bodies_are_strict() {
    let s = Server::start(Duration::from_secs(60)).await;
    let a = s.login("alice").await;
    let b = s.login("bob").await;
    assert_eq!(
        s.request(
            "POST",
            &a.path("/view"),
            a.headers(),
            json!({"width":96,"height":64,"source":"b.oas"})
        )
        .await
        .0,
        400
    );
    let mut h = a.headers();
    h.remove("origin");
    assert_eq!(
        s.request("POST", &a.path("/view"), h, json!({"width":96,"height":64}))
            .await
            .0,
        401
    );
    assert_eq!(
        s.request(
            "POST",
            &b.path("/view"),
            a.headers(),
            json!({"width":96,"height":64})
        )
        .await
        .0,
        401
    );
    s.open(&a).await;
    for field in ["origin", "cookie", "x-floe-proxy-key"] {
        let mut req = s.ws_request(&a);
        req.headers_mut().remove(field);
        denied(
            req,
            if field == "x-floe-proxy-key" {
                403
            } else {
                401
            },
        )
        .await;
    }
    for secret in ["0".repeat(64), b.csrf.clone()] {
        let mut req = s.ws_request(&a);
        req.headers_mut().insert(
            "sec-websocket-protocol",
            format!("{}, bundle.{BUNDLE}, csrf.{secret}", broker::PROTOCOL)
                .parse()
                .unwrap(),
        );
        denied(req, 401).await;
    }
    let mut req = s.ws_request(&a);
    req.headers_mut().insert(
        "sec-websocket-protocol",
        format!("{}, bundle.old, csrf.{}", broker::PROTOCOL, a.csrf)
            .parse()
            .unwrap(),
    );
    denied(req, 401).await;
    let mut req = s.ws_request(&a);
    req.headers_mut()
        .insert("origin", "https://evil.example.test".parse().unwrap());
    denied(req, 403).await;
    let (mut ws, h) = s.connect(&a).await;
    let (f, _) = frame(&mut ws).await;
    ack(&mut ws, &h, &f, 1).await;
    send(&mut ws,json!({"type":"view.clip.prepare","seq":"2","connection_epoch":h["connection_epoch"],"view_id":a.id,"body":{}})).await;
    ended(&mut ws).await;
    // Owner filesystem/indexing/review endpoints were not mounted here.
    for path in [
        "/api/v1/operations",
        "/api/v1/catalog",
        "/api/v1/drc/review/notes",
        "/api/v1/exports",
    ] {
        assert_eq!(
            s.request("GET", path, a.headers(), Value::Null).await.0,
            404
        );
    }
    s.shutdown().await;
}

#[tokio::test]
#[ignore = "synthetic native server_runtime gate"]
async fn frame_credit_does_not_block_edits_and_revocation_closes_open_socket() {
    let s = Server::start(Duration::from_secs(60)).await;
    let a = s.login("alice").await;
    s.open(&a).await;
    let (mut ws, h) = s.connect(&a).await;
    let (f, _) = frame(&mut ws).await;
    send(&mut ws,json!({"type":"view.set","seq":"1","connection_epoch":h["connection_epoch"],"view_id":a.id,"base_state_rev":f["state_rev"],"body":{"navigation":{"kind":"zoom","factor":0.8,"anchor":[0.5,0.5]}}})).await;
    let deadline = Instant::now() + Duration::from_millis(200);
    let mut accepted = false;
    while Instant::now() < deadline {
        match timeout(Duration::from_millis(30), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                accepted |= serde_json::from_str::<Value>(&t).unwrap()["type"] == "accepted"
            }
            Err(_) => (),
            other => panic!("unexpected frame before credit return: {other:?}"),
        }
    }
    assert!(accepted);
    ack(&mut ws, &h, &f, 2).await;
    let (new, _) = frame(&mut ws).await;
    assert_ne!(new["frame_id"], f["frame_id"]);
    assert_eq!(
        s.request("DELETE", &a.path(""), a.headers(), Value::Null)
            .await
            .0,
        204
    );
    ended(&mut ws).await;
    s.shutdown().await;
}

#[tokio::test]
#[ignore = "synthetic native server_runtime gate"]
async fn aborting_server_future_still_reaps_native_workers() {
    let s = Server::start(Duration::from_secs(60)).await;
    let a = s.login("alice").await;
    s.open(&a).await;
    let (mut ws, _) = s.connect(&a).await;
    frame(&mut ws).await;
    s.task.abort();
    assert!(s.task.await.unwrap_err().is_cancelled());
    ended(&mut ws).await;
    timeout(Duration::from_secs(8), async {
        while s.broker.pending_workers().unwrap() != 0 || s.resources.usage() != Usage::default() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "synthetic native server_runtime gate"]
async fn session_expiry_closes_live_stream_without_renewal_by_heartbeats() {
    let s = Server::start(Duration::from_secs(3)).await;
    let a = s.login("alice").await;
    s.open(&a).await;
    let (mut ws, h) = s.connect(&a).await;
    let (f, _) = frame(&mut ws).await;
    ack(&mut ws, &h, &f, 1).await;
    send(&mut ws, json!({"type":"ping","seq":"2"})).await;
    ended(&mut ws).await;
    assert_eq!(
        s.request("GET", &a.path("/view"), a.headers(), Value::Null)
            .await
            .0,
        401
    );
    s.shutdown().await;
}

#[tokio::test]
#[ignore = "synthetic native server_runtime gate; Node 18+ development test"]
async fn server_client_displays_actual_native_packet_and_snapshot() {
    let s = Server::start(Duration::from_secs(60)).await;
    let a = s.login("alice").await;
    s.open(&a).await;
    let (mut ws, hello) = s.connect(&a).await;
    let (header, bytes) = frame(&mut ws).await;
    let (_, _, mut state) = s
        .request("GET", &a.path("/view"), a.headers(), Value::Null)
        .await;
    state["connection_epoch"] = hello["connection_epoch"].clone();
    let encoded = serde_json::to_vec(&header).unwrap();
    let mut packet = (encoded.len() as u32).to_le_bytes().to_vec();
    packet.extend(encoded);
    packet.extend(bytes);
    let input = serde_json::to_vec(&json!({"hello":hello,"state":state,"packet":packet})).unwrap();
    let result = tokio::task::spawn_blocking(move || {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let mut child = Command::new("node")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/server-native.test.cjs"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        child.wait_with_output().unwrap()
    })
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("WEB SERVER NATIVE DISPLAY: ALL OK"));
    s.shutdown().await;
}
