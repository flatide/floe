use floe_web::{
    auth::Secret,
    transport::{self, Gateway, BUNDLE, PROTOCOL},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{collections::BTreeMap, net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        protocol::{
            frame::{
                coding::{Data, OpCode},
                Frame,
            },
            Message,
        },
        Error,
    },
    MaybeTlsStream, WebSocketStream,
};
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
#[tokio::test]
async fn embedded_assets_are_content_identified_and_never_serve_files() {
    let server = Server::start().await;
    let page = server.request("GET", "/", &[], "").await;
    assert_eq!(page.status, 200);
    assert!(page.body.contains(&format!("/assets/{BUNDLE}/app.js")));
    assert!(!page.body.contains("@@BUNDLE@@"));
    assert!(page.headers["content-security-policy"].contains("script-src 'self'"));
    assert!(page.headers["content-security-policy"].contains(&format!("ws://{}", server.addr)));
    assert!(!page.headers["content-security-policy"].contains("unsafe-inline"));
    for (name, mime) in [
        ("app.js", "text/javascript"),
        ("protocol.js", "text/javascript"),
        ("query.js", "text/javascript"),
        ("inspect.js", "text/javascript"),
        ("measure.js", "text/javascript"),
        ("drc.js", "text/javascript"),
        ("rulers.js", "text/javascript"),
        ("drc-groups.js", "text/javascript"),
        ("panel-state.js", "text/javascript"),
        ("app.css", "text/css"),
    ] {
        let r = server
            .request("GET", &format!("/assets/{BUNDLE}/{name}"), &[], "")
            .await;
        assert_eq!(r.status, 200);
        assert!(r.headers["content-type"].starts_with(mime));
    }
    for path in [
        "/assets/wrong/app.js",
        "/assets/wrong/../../etc/passwd",
        "/etc/passwd",
    ] {
        assert_eq!(server.request("GET", path, &[], "").await.status, 404);
    }
    assert_eq!(
        server
            .request("GET", "/api/v1/startup", &[], "")
            .await
            .status,
        401
    );
    server.shutdown().await;
}
struct Server {
    addr: SocketAddr,
    bootstrap: Secret,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}
struct Reply {
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
}
struct Auth {
    cookie: String,
    csrf: String,
}
impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (gateway, bootstrap) = Gateway::new(addr).unwrap();
        let (stop, rx) = oneshot::channel();
        let task = tokio::spawn(transport::serve(listener, gateway, async move {
            let _ = rx.await;
        }));
        Self {
            addr,
            bootstrap,
            stop,
            task,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Reply {
        let mut socket = TcpStream::connect(self.addr).await.unwrap();
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\n",
            self.addr,
            body.len()
        );
        for (k, v) in headers {
            request.push_str(&format!("{k}: {v}\r\n"));
        }
        request.push_str("\r\n");
        request.push_str(body);
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut out = vec![];
        timeout(Duration::from_secs(7), socket.read_to_end(&mut out))
            .await
            .unwrap()
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let mut lines = head.lines();
        let status = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .map(|l| {
                let (k, v) = l.split_once(':').unwrap();
                (k.to_lowercase(), v.trim().into())
            })
            .collect();
        Reply {
            status,
            headers,
            body: body.into(),
        }
    }
    async fn exchange(&self, bundle: &str, origin: &str) -> Reply {
        self.request(
            "POST",
            "/api/v1/session/exchange",
            &[("Content-Type", "application/json"), ("Origin", origin)],
            &json!({"bootstrap":self.bootstrap.expose(),"protocol":1,"bundle":bundle}).to_string(),
        )
        .await
    }
    async fn login(&self) -> Auth {
        let r = self
            .exchange(BUNDLE, &format!("http://{}", self.addr))
            .await;
        assert_eq!(r.status, 200);
        assert!(r.headers["set-cookie"].contains("HttpOnly"));
        assert!(r.headers["set-cookie"].contains("SameSite=Strict"));
        assert_eq!(r.headers["cache-control"], "no-store");
        let body: Value = serde_json::from_str(&r.body).unwrap();
        Auth {
            cookie: r.headers["set-cookie"].split(';').next().unwrap().into(),
            csrf: body["csrf"].as_str().unwrap().into(),
        }
    }
    async fn connect(&self, a: &Auth, origin: Option<&str>, bundle: &str) -> Result<Socket, Error> {
        let mut req = format!("ws://{}/api/v1/events", self.addr)
            .into_client_request()
            .unwrap();
        let h = req.headers_mut();
        h.insert("cookie", a.cookie.parse().unwrap());
        if let Some(o) = origin {
            h.insert("origin", o.parse().unwrap());
        }
        h.insert(
            "sec-websocket-protocol",
            format!("{PROTOCOL}, bundle.{bundle}, csrf.{}", a.csrf)
                .parse()
                .unwrap(),
        );
        connect_async(req).await.map(|(s, r)| {
            assert_eq!(r.headers()["sec-websocket-protocol"], PROTOCOL);
            s
        })
    }
    async fn socket(&self, a: &Auth) -> Socket {
        let mut ws = self
            .connect(a, Some(&format!("http://{}", self.addr)), BUNDLE)
            .await
            .unwrap();
        let hello = next_json(&mut ws).await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["bundle"], BUNDLE);
        ws
    }
    async fn shutdown(self) {
        self.stop.send(()).unwrap();
        timeout(Duration::from_secs(3), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
async fn next_json(ws: &mut Socket) -> Value {
    loop {
        match timeout(Duration::from_secs(3), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            Message::Ping(_) | Message::Pong(_) => (),
            _ => panic!("expected a JSON text message"),
        }
    }
}
#[tokio::test]
async fn websocket_upgrade_does_not_inherit_http_idle_deadline() {
    let s = Server::start().await;
    let login = s.login().await;
    let mut ws = s.socket(&login).await;
    // The transport heartbeat deadline is 30s, not HTTP's 10s idle deadline.
    tokio::time::sleep(Duration::from_secs(11)).await;
    ws.send(Message::Text(
        json!({"type":"ping","seq":"1"}).to_string().into(),
    ))
    .await
    .unwrap();
    assert_eq!(next_json(&mut ws).await["type"], "pong");
    ws.close(None).await.unwrap();
    s.shutdown().await;
}
#[tokio::test]
async fn ignored_http_bodies_still_have_size_and_time_limits() {
    let s = Server::start().await;
    // Even a static GET must not evade limits by leaving its body unconsumed.
    assert_eq!(
        s.request("GET", "/", &[], &"x".repeat(16 * 1024 + 1))
            .await
            .status,
        413
    );
    let mut socket = TcpStream::connect(s.addr).await.unwrap();
    socket
        .write_all(
            format!(
                "GET / HTTP/1.1\r\nHost: {}\r\nContent-Length: 4\r\n\r\nx",
                s.addr
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(7), socket.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 408"));
    assert!(std::str::from_utf8(&response)
        .unwrap()
        .contains("connection: close"));
    assert_eq!(s.request("GET", "/", &[], "").await.status, 200);
    s.shutdown().await;
}
async fn closed(ws: &mut Socket) {
    loop {
        match timeout(Duration::from_secs(3), ws.next()).await.unwrap() {
            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => (),
            _ => panic!("expected close"),
        }
    }
}
fn http_error(result: Result<Socket, Error>, status: u16) {
    match result {
        Err(Error::Http(r)) => assert_eq!(r.status().as_u16(), status),
        _ => panic!("expected HTTP rejection"),
    }
}

#[tokio::test]
async fn bootstrap_host_origin_csrf_and_logout() {
    let s = Server::start().await;
    assert_eq!(
        s.request("GET", "/api/v1/capabilities", &[], "")
            .await
            .status,
        401
    );
    assert_eq!(s.exchange(BUNDLE, "null").await.status, 403);
    assert_eq!(s.exchange(BUNDLE, "http://evil.example").await.status, 403);
    assert_eq!(
        s.exchange("old-bundle", &format!("http://{}", s.addr))
            .await
            .status,
        426
    );
    let a = s.login().await;
    assert_eq!(
        s.exchange(BUNDLE, &format!("http://{}", s.addr))
            .await
            .status,
        401
    );
    assert_eq!(
        s.request("GET", "/api/v1/capabilities", &[("Cookie", &a.cookie)], "")
            .await
            .status,
        401
    );
    let h = [
        ("Cookie", a.cookie.as_str()),
        ("X-Floe-CSRF", a.csrf.as_str()),
    ];
    let r = s.request("GET", "/api/v1/capabilities", &h, "").await;
    assert_eq!(r.status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&r.body).unwrap()["render"],
        false
    );
    assert!(!r.headers.contains_key("access-control-allow-origin"));
    assert_eq!(
        serde_json::from_str::<Value>(&r.body).unwrap()["drc"],
        false
    );
    let empty = s.request("GET", "/api/v1/drc", &h, "").await;
    assert_eq!(empty.status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&empty.body).unwrap(),
        json!({"drc":null})
    );
    assert_eq!(
        s.request("POST", "/api/v1/drc/id/read", &h, "{}")
            .await
            .status,
        403
    );
    assert_eq!(
        s.request("GET", "/api/v1/drc", &[h[0]], "").await.status,
        401
    );
    let r = s
        .request(
            "GET",
            "/api/v1/capabilities",
            &[("Host", "evil.example"), h[0], h[1]],
            "",
        )
        .await;
    assert!(matches!(r.status, 400 | 403));
    let r = s
        .request(
            "GET",
            "/api/v1/capabilities",
            &[("Origin", "http://evil.example"), h[0], h[1]],
            "",
        )
        .await;
    assert_eq!(r.status, 403);
    assert_eq!(
        s.request("GET", "/api/v1/capabilities?token=invalid", &h, "")
            .await
            .status,
        403
    );
    assert_eq!(
        s.request("DELETE", "/api/v1/session", &h, "").await.status,
        403
    );
    let mut ws = s.socket(&a).await;
    let r = s
        .request(
            "DELETE",
            "/api/v1/session",
            &[("Origin", &format!("http://{}", s.addr)), h[0], h[1]],
            "",
        )
        .await;
    assert_eq!(r.status, 204);
    assert!(r.headers["set-cookie"].contains("Max-Age=0"));
    closed(&mut ws).await;
    assert_eq!(
        s.request("GET", "/api/v1/capabilities", &h, "")
            .await
            .status,
        401
    );
    http_error(
        s.connect(&a, Some(&format!("http://{}", s.addr)), BUNDLE)
            .await,
        401,
    );
    s.shutdown().await;
}

#[tokio::test]
async fn websocket_auth_protocol_and_fragmentation() {
    let s = Server::start().await;
    let a = s.login().await;
    http_error(s.connect(&a, None, BUNDLE).await, 403);
    http_error(
        s.connect(&a, Some("http://evil.example"), BUNDLE).await,
        403,
    );
    http_error(
        s.connect(&a, Some(&format!("http://{}", s.addr)), "old")
            .await,
        426,
    );
    let wrong = Auth {
        cookie: a.cookie.clone(),
        csrf: "0".repeat(64),
    };
    http_error(
        s.connect(&wrong, Some(&format!("http://{}", s.addr)), BUNDLE)
            .await,
        401,
    );
    let mut ws = s.socket(&a).await;
    ws.send(Message::Text(
        json!({"type":"ping","seq":"1"}).to_string().into(),
    ))
    .await
    .unwrap();
    assert_eq!(next_json(&mut ws).await, json!({"type":"pong","seq":"1"}));
    ws.send(Message::Frame(Frame::message(
        b"{\"type\":\"ping\",".to_vec(),
        OpCode::Data(Data::Text),
        false,
    )))
    .await
    .unwrap();
    ws.send(Message::Ping(b"hi".to_vec().into())).await.unwrap();
    ws.send(Message::Frame(Frame::message(
        b"\"seq\":\"2\"}".to_vec(),
        OpCode::Data(Data::Continue),
        true,
    )))
    .await
    .unwrap();
    assert_eq!(next_json(&mut ws).await, json!({"type":"pong","seq":"2"}));
    // Server sequence checks do not replay a repeated control message.
    ws.send(Message::Text(
        json!({"type":"ping","seq":"2"}).to_string().into(),
    ))
    .await
    .unwrap();
    closed(&mut ws).await;
    s.shutdown().await;
}

#[tokio::test]
async fn malformed_messages_do_not_kill_the_listener() {
    let s = Server::start().await;
    let a = s.login().await;
    for text in [
        "{",
        r#"{"type":"render","out":"/tmp/forbidden"}"#,
        r#"{"type":"ping","seq":"01"}"#,
        r#"{"type":"ping","seq":"-1"}"#,
        r#"{"type":"ping","seq":"1","unknown":1}"#,
    ] {
        let mut ws = s.socket(&a).await;
        ws.send(Message::Text(text.into())).await.unwrap();
        closed(&mut ws).await;
    }
    for message in [
        Message::Binary(vec![0; 1].into()),
        Message::Binary(vec![0; 8193].into()),
        Message::Text("x".repeat(8193).into()),
        Message::Frame(Frame::message(vec![255], OpCode::Data(Data::Text), true)),
    ] {
        let mut ws = s.socket(&a).await;
        ws.send(message).await.unwrap();
        closed(&mut ws).await;
    }
    let mut ws = s.socket(&a).await;
    // Raw test-only RFC6455 violation: a client frame MUST be masked.
    ws.get_mut()
        .write_all(&[0x81, 2, b'{', b'}'])
        .await
        .unwrap();
    closed(&mut ws).await;
    let mut ws = s.socket(&a).await;
    ws.send(Message::Frame(Frame::message(
        vec![b'x'; 4096],
        OpCode::Data(Data::Text),
        false,
    )))
    .await
    .unwrap();
    ws.send(Message::Frame(Frame::message(
        vec![b'x'; 4097],
        OpCode::Data(Data::Continue),
        true,
    )))
    .await
    .unwrap();
    closed(&mut ws).await;
    let mut ws = s.socket(&a).await;
    ws.send(Message::Text(
        json!({"type":"ping","seq":"1"}).to_string().into(),
    ))
    .await
    .unwrap();
    assert_eq!(next_json(&mut ws).await["type"], "pong");
    s.shutdown().await;
    closed(&mut ws).await;
}

#[tokio::test]
async fn socket_slots_and_shutdown_are_bounded() {
    let s = Server::start().await;
    let a = s.login().await;
    let mut sockets = vec![];
    for _ in 0..8 {
        sockets.push(s.socket(&a).await);
    }
    http_error(
        s.connect(&a, Some(&format!("http://{}", s.addr)), BUNDLE)
            .await,
        429,
    );
    s.shutdown().await;
    for ws in &mut sockets {
        closed(ws).await;
    }
}

#[tokio::test]
async fn request_body_and_header_limits_are_applied() {
    let s = Server::start().await;
    let origin = format!("http://{}", s.addr);
    let r = s
        .request(
            "POST",
            "/api/v1/session/exchange",
            &[("Origin", &origin), ("Content-Type", "application/json")],
            &json!({"bootstrap":"x".repeat(17000),"protocol":1,"bundle":BUNDLE}).to_string(),
        )
        .await;
    assert_eq!(r.status, 413);
    let mut ws = TcpStream::connect(s.addr).await.unwrap();
    ws.write_all(
        format!(
            "GET / HTTP/1.1\r\nHost: {}\r\nX-Large: {}\r\n\r\n",
            s.addr,
            "x".repeat(17000)
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut response = vec![];
    let _ = timeout(Duration::from_secs(7), ws.read_to_end(&mut response))
        .await
        .unwrap();
    assert!(response.is_empty() || String::from_utf8_lossy(&response).starts_with("HTTP/1.1 431"));
    let _ = s.login().await;
    s.shutdown().await;
}

#[tokio::test]
async fn slow_headers_and_bodies_have_deadlines() {
    let s = Server::start().await;
    let mut header = TcpStream::connect(s.addr).await.unwrap();
    header.write_all(b"GET /api/v1/").await.unwrap();
    let mut body = TcpStream::connect(s.addr).await.unwrap();
    body.write_all(format!("POST /api/v1/session/exchange HTTP/1.1\r\nHost: {}\r\nOrigin: http://{}\r\nContent-Type: application/json\r\nContent-Length: 99\r\nConnection: close\r\n\r\n{{",s.addr,s.addr).as_bytes()).await.unwrap();
    let mut h = vec![];
    let mut b = vec![];
    timeout(Duration::from_secs(7), header.read_to_end(&mut h))
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(7), body.read_to_end(&mut b))
        .await
        .unwrap()
        .unwrap();
    assert!(h.is_empty() || String::from_utf8_lossy(&h).starts_with("HTTP/1.1 408"));
    assert!(String::from_utf8_lossy(&b).starts_with("HTTP/1.1 408"));
    let _ = s.login().await;
    s.shutdown().await;
}

#[tokio::test]
async fn idle_tcp_connections_cannot_grow_without_bound() {
    let s = Server::start().await;
    let mut connections = vec![];
    for _ in 0..32 {
        let mut c = TcpStream::connect(s.addr).await.unwrap();
        c.write_all(b"GET /").await.unwrap();
        connections.push(c);
    }
    let mut extra = TcpStream::connect(s.addr).await.unwrap();
    let mut out = vec![];
    let _ = timeout(Duration::from_secs(2), extra.read_to_end(&mut out))
        .await
        .unwrap();
    assert!(out.is_empty());
    drop(connections);
    s.shutdown().await;
}
