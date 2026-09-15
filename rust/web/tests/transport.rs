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
async fn preset_catalogue_is_authenticated_readonly_and_works_without_a_view() {
    let s = Server::start().await;
    let path = "/api/v1/palette/presets";
    assert_eq!(s.request("GET", path, &[], "").await.status, 401);
    let a = s.login().await;
    let headers = [
        ("Cookie", a.cookie.as_str()),
        ("X-Floe-CSRF", a.csrf.as_str()),
    ];
    assert_eq!(s.request("GET", path, &headers[..1], "").await.status, 401);
    assert_eq!(
        s.request(
            "GET",
            path,
            &[("Cookie", &a.cookie), ("X-Floe-CSRF", "wrong")],
            ""
        )
        .await
        .status,
        401
    );
    let mut cross = headers.to_vec();
    cross.push(("Origin", "http://wrong.invalid"));
    assert_eq!(s.request("GET", path, &cross, "").await.status, 403);
    let r = s.request("GET", path, &headers, "").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["cache-control"], "no-store");
    assert!(r.body.len() < 16 * 1024);
    let p: Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(p["colors"].as_array().unwrap().len(), 49);
    assert_eq!(p["fills"].as_array().unwrap().len(), 20);
    assert_eq!(p["colors"][7]["name"], "yellow");
    assert_eq!(p["colors"][8]["name"], "yellow1");
    assert_eq!(p["colors"][7]["color"], p["colors"][8]["color"]);
    assert_eq!(p["fills"][18]["fill"], json!({"kind":"solid"}));
    assert_eq!(p["fills"][19]["fill"], json!({"kind":"clear"}));
    assert_eq!(s.request("GET", path, &headers, "").await.body, r.body);
    assert_eq!(
        s.request("GET", "/api/v1/view", &headers, "").await.status,
        404
    );
    let origin = format!("http://{}", s.addr);
    let mut same = headers.to_vec();
    same.push(("Origin", &origin));
    assert_eq!(s.request("POST", path, &same, "{}").await.status, 405);
    s.shutdown().await;
}
#[tokio::test]
async fn launcher_routes_require_trusted_attachment_and_owner_credentials() {
    let (mut gate, _) = Gateway::new("127.0.0.1:23456".parse().unwrap()).unwrap();
    assert!(Gateway::attach_launches(&mut gate, floe_web::launch::Launches::new()).is_err());
    let s = Server::start().await;
    assert_eq!(
        s.request("GET", "/api/v1/launch", &[], "").await.status,
        401
    );
    let a = s.login().await;
    let h = [
        ("Cookie", a.cookie.as_str()),
        ("X-Floe-CSRF", a.csrf.as_str()),
    ];
    let caps = s.request("GET", "/api/v1/capabilities", &h, "").await;
    assert_eq!(
        serde_json::from_str::<Value>(&caps.body).unwrap()["launcher"],
        false
    );
    for path in ["/api/v1/launch", "/api/v1/launch/poll/0"] {
        assert_eq!(s.request("GET", path, &h, "").await.status, 404);
    }
    s.shutdown().await;
}
#[tokio::test]
async fn portable_notice_catalogue_is_authenticated_bounded_and_fail_closed() {
    use std::{
        fs,
        os::unix::fs::DirBuilderExt,
        sync::{atomic::AtomicUsize, Arc},
    };
    let dir = std::env::temp_dir().join(format!("floe-http-notices-{}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    fs::create_dir(dir.join("NOTICES")).unwrap();
    let original = "x".repeat(65535) + "한";
    fs::write(dir.join("NOTICES/test.html"), &original).unwrap();
    fs::write(dir.join("NOTICES/binary"), [0, 255]).unwrap();
    let index = floe_notices::build_index(
        &dir,
        &["NOTICES/test.html".into(), "NOTICES/binary".into()],
        "r",
        "t",
        &AtomicUsize::new(0),
    )
    .unwrap();
    fs::write(dir.join(floe_notices::INDEX_NAME), &index).unwrap();
    let digest = floe_notices::digest(&index);
    let cat = floe_notices::Catalog::open(&dir, &digest, "r", "t", &AtomicUsize::new(0)).unwrap();
    let s = Server::configured(None, floe_web::about::Notices::Ready(Arc::new(cat))).await;
    for path in ["/api/v1/about/notices/0", "/api/v1/about/notices/0/0"] {
        assert_eq!(s.request("GET", path, &[], "").await.status, 401);
    }
    let a = s.login().await;
    let h = [
        ("Cookie", a.cookie.as_str()),
        ("X-Floe-CSRF", a.csrf.as_str()),
    ];
    let about = s.request("GET", "/api/v1/about", &h, "").await;
    let about: Value = serde_json::from_str(&about.body).unwrap();
    assert_eq!(about["notice_scope"], "portable_manifest");
    assert_eq!(about["notices"]["files"], 2);
    assert_eq!(about["notices"]["index_id"], digest);
    let list = s.request("GET", "/api/v1/about/notices/0", &h, "").await;
    assert_eq!(list.status, 200);
    assert_eq!(list.headers["cache-control"], "no-store");
    let list: Value = serde_json::from_str(&list.body).unwrap();
    assert_eq!(list["files"][0]["pages"], 2);
    let page = s.request("GET", "/api/v1/about/notices/0/0", &h, "").await;
    assert_eq!(page.status, 200);
    assert!(page.body.len() < 1024 * 1024);
    let page: Value = serde_json::from_str(&page.body).unwrap();
    assert_eq!(page["text"].as_str().unwrap().len(), 65535);
    let next = s.request("GET", "/api/v1/about/notices/0/1", &h, "").await;
    let next: Value = serde_json::from_str(&next.body).unwrap();
    assert_eq!(next["text"], "한");
    assert_eq!(next["offset"], 65535);
    let binary = s.request("GET", "/api/v1/about/notices/1/0", &h, "").await;
    let binary: Value = serde_json::from_str(&binary.body).unwrap();
    assert_eq!(binary["text"], "00 ff ");
    for path in [
        "/api/v1/about/notices/01",
        "/api/v1/about/notices/64",
        "/api/v1/about/notices/1/x",
        "/api/v1/about/notices/99/0",
    ] {
        assert!((400..500).contains(&s.request("GET", path, &h, "").await.status));
    }
    assert_eq!(
        s.request("GET", "/api/v1/about/notices/0?path=/etc/passwd", &h, "")
            .await
            .status,
        403
    );
    fs::write(dir.join("NOTICES/test.html"), "z".repeat(original.len())).unwrap();
    let bad = s.request("GET", "/api/v1/about/notices/0/0", &h, "").await;
    assert_eq!(bad.status, 409);
    assert!(!bad.body.contains("text"));
    assert!(!bad.body.contains(dir.to_str().unwrap()));
    let origin = format!("http://{}", s.addr);
    let post = [
        ("Cookie", a.cookie.as_str()),
        ("X-Floe-CSRF", a.csrf.as_str()),
        ("Origin", origin.as_str()),
    ];
    assert_eq!(
        s.request("POST", "/api/v1/about/notices/0/0", &post, "")
            .await
            .status,
        405
    );
    assert_eq!(
        s.request("DELETE", "/api/v1/session", &post, "")
            .await
            .status,
        204
    );
    assert_eq!(
        s.request("GET", "/api/v1/about/notices/0/0", &h, "")
            .await
            .status,
        401
    );
    s.shutdown().await;
    let s = Server::configured(None, floe_web::about::Notices::Unavailable).await;
    let a = s.login().await;
    let h = [
        ("Cookie", a.cookie.as_str()),
        ("X-Floe-CSRF", a.csrf.as_str()),
    ];
    assert_eq!(
        s.request("GET", "/api/v1/about/notices/0", &h, "")
            .await
            .status,
        503
    );
    s.shutdown().await;
}
#[tokio::test]
async fn about_is_authenticated_readonly_and_reports_expected_not_running_versions() {
    for configured in [false, true] {
        let server = Server::start_build(configured.then_some(floe_web::about::BuildInfo {
            app_version: "0.1.0",
            source_revision: "unknown",
            target: "synthetic",
            index_compatibility: "test-index",
            renderd_compatibility: "test-renderd",
        }))
        .await;
        assert_eq!(
            server.request("GET", "/api/v1/about", &[], "").await.status,
            401
        );
        let auth = server.login().await;
        assert_eq!(
            server
                .request("GET", "/api/v1/about", &[("Cookie", &auth.cookie)], "")
                .await
                .status,
            401
        );
        let h = [
            ("Cookie", auth.cookie.as_str()),
            ("X-Floe-CSRF", auth.csrf.as_str()),
        ];
        let reply = server.request("GET", "/api/v1/about", &h, "").await;
        assert_eq!(reply.status, 200);
        assert_eq!(reply.headers["cache-control"], "no-store");
        assert!(reply.body.len() < 16384);
        let value: Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(value["bundle"], BUNDLE);
        assert_eq!(value["desktop_acceptance"], "unverified");
        assert_eq!(value["notice_scope"], "embedded_font_only");
        assert_eq!(
            value["font_notice"],
            include_str!("../../render-core/assets/NotoSansMono-OFL.txt")
        );
        if configured {
            assert_eq!(value["build"]["renderd_compatibility"], "test-renderd");
        } else {
            assert!(value["build"].is_null());
        }
        assert_eq!(
            server.request("GET", "/api/v1/view", &h, "").await.status,
            404
        );
        assert_eq!(
            server
                .request("GET", "/api/v1/about?path=/etc/passwd", &h, "")
                .await
                .status,
            403
        );
        let origin = format!("http://{}", server.addr);
        let post = [
            ("Cookie", auth.cookie.as_str()),
            ("X-Floe-CSRF", auth.csrf.as_str()),
            ("Origin", origin.as_str()),
        ];
        assert_eq!(
            server
                .request("POST", "/api/v1/about", &post, "")
                .await
                .status,
            405
        );
        assert_eq!(
            server
                .request("DELETE", "/api/v1/session", &post, "")
                .await
                .status,
            204
        );
        assert_eq!(
            server.request("GET", "/api/v1/about", &h, "").await.status,
            401
        );
        server.shutdown().await;
    }
}
#[tokio::test]
async fn embedded_assets_are_content_identified_and_never_serve_files() {
    let server = Server::start().await;
    let page = server.request("GET", "/", &[], "").await;
    assert_eq!(page.status, 200);
    assert!(page.body.contains(&format!("/assets/{BUNDLE}/app.js")));
    assert!(page.body.contains(&format!("/assets/{BUNDLE}/palette.js")));
    assert!(page.body.contains(&format!("/assets/{BUNDLE}/presets.js")));
    assert!(!page.body.contains("@@BUNDLE@@"));
    assert!(page.headers["content-security-policy"].contains("script-src 'self'"));
    assert!(page.headers["content-security-policy"].contains(&format!("ws://{}", server.addr)));
    assert!(!page.headers["content-security-policy"].contains("unsafe-inline"));
    for (name, mime) in [
        ("app.js", "text/javascript"),
        ("palette.js", "text/javascript"),
        ("presets.js", "text/javascript"),
        ("about.js", "text/javascript"),
        ("session-exit.js", "text/javascript"),
        ("notices.js", "text/javascript"),
        ("protocol.js", "text/javascript"),
        ("query.js", "text/javascript"),
        ("inspect.js", "text/javascript"),
        ("measure.js", "text/javascript"),
        ("clip.js", "text/javascript"),
        ("snapshot.js", "text/javascript"),
        ("drc.js", "text/javascript"),
        ("drc-notes.js", "text/javascript"),
        ("drc-note-display.js", "text/javascript"),
        ("drc-waives.js", "text/javascript"),
        ("drc-transfer.js", "text/javascript"),
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
        Self::start_build(None).await
    }
    async fn start_build(info: Option<floe_web::about::BuildInfo>) -> Self {
        Self::configured(info, floe_web::about::Notices::NotPackaged).await
    }
    async fn configured(
        info: Option<floe_web::about::BuildInfo>,
        notices: floe_web::about::Notices,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (mut gateway, bootstrap) = Gateway::new(addr).unwrap();
        if let Some(info) = info {
            Gateway::attach_build(&mut gateway, info.clone()).unwrap();
            assert!(Gateway::attach_build(&mut gateway, info).is_err());
        }
        Gateway::attach_notices(&mut gateway, notices).unwrap();
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
    assert_eq!(
        serde_json::from_str::<Value>(&r.body).unwrap()["snapshot_png"],
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
