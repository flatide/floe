//! Synthetic control plane over loopback with simulated HTTPS proxy headers.
//! No real identity, renderer, TLS endpoint or design file is involved.
use floe_app_core::server::Config;
use floe_web::broker::{self, Broker, Lifetimes};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
    time::timeout,
};

const PUBLIC: &str = "https://service.example.test";
const PROXY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const DELEGATE: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const LAUNCH: &str = "/api/v1/server/launches";
fn headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Host", "service.example.test"),
        ("Origin", PUBLIC),
        ("X-Floe-Proxy-Key", PROXY),
        ("Content-Type", "application/json"),
    ]
}
fn delegated() -> Vec<(&'static str, &'static str)> {
    let mut h = headers();
    h.push(("X-Floe-Delegation-Key", DELEGATE));
    h.push(("X-Floe-Service-Client", "teebox-service"));
    h
}
fn path(id: &str) -> String {
    format!("/api/v1/server/sessions/{id}")
}
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let id: String = random.iter().map(|n| format!("{n:02x}")).collect();
        let p = std::env::temp_dir().join(format!("floe-broker-http-{id}"));
        fs::create_dir(&p).unwrap();
        let p = fs::canonicalize(p).unwrap();
        for d in ["data", "work", "runtime"] {
            fs::create_dir(p.join(d)).unwrap();
        }
        fs::write(
            p.join("data/synthetic.oas"),
            b"synthetic metadata-only fixture",
        )
        .unwrap();
        Self(p)
    }
    fn config(&self) -> Config {
        serde_json::from_value(json!({"version":1,"public_origin":PUBLIC,"runtime_root":self.0.join("runtime"),"max_sessions":2,
            "deployment":{"mode":"teebox","client_id":"teebox-service","user_namespace":"teebox",
                "shared_root":self.0.join("data"),"work_root":self.0.join("work"),
                "index":{"max_running":1,"max_entries":32,"jobs":8}}})).unwrap()
    }
    fn unchanged(&self) {
        assert_eq!(fs::read_dir(self.0.join("data")).unwrap().count(), 1);
        assert_eq!(fs::read_dir(self.0.join("work")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(self.0.join("runtime")).unwrap().count(), 0);
        assert_eq!(
            fs::read(self.0.join("data/synthetic.oas")).unwrap(),
            b"synthetic metadata-only fixture"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Server {
    addr: SocketAddr,
    broker: Arc<Broker>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
    fixture: Fixture,
}
struct Reply {
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
}
struct Session {
    id: String,
    cookie: String,
    csrf: String,
}
impl Session {
    fn headers(&self) -> Vec<(&str, &str)> {
        let mut h = headers();
        h.push(("Cookie", &self.cookie));
        h.push(("X-Floe-CSRF", &self.csrf));
        h
    }
}
impl Server {
    async fn start() -> Self {
        Self::start_mode(false).await
    }
    async fn start_mode(demo: bool) -> Self {
        Self::start_transport(demo, false).await
    }
    async fn start_transport(demo: bool, http_test: bool) -> Self {
        let fixture = Fixture::new();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut config = fixture.config();
        if demo {
            config.deployment = floe_app_core::server::Deployment::PublicDemo {
                allow_insecure_http: http_test,
                data_root: fixture.0.join("data"),
                samples: vec![floe_app_core::server::Sample {
                    id: "sample1".into(),
                    source: "synthetic.oas".into(),
                }],
            };
        }
        if http_test {
            config.public_origin = "http://10.0.0.10:8080".into();
        }
        let policy = config.validate().unwrap();
        let broker = Arc::new(if demo {
            Broker::public_demo(addr, policy, PROXY).unwrap()
        } else {
            Broker::new(addr, policy, PROXY, DELEGATE, Lifetimes::default()).unwrap()
        });
        let (stop, done) = oneshot::channel();
        let b = Arc::clone(&broker);
        let task = tokio::spawn(broker::serve(listener, b, async {
            let _ = done.await;
        }));
        Self {
            addr,
            broker,
            stop,
            task,
            fixture,
        }
    }
    async fn request(&self, method: &str, path: &str, h: &[(&str, &str)], body: &str) -> Reply {
        Self::request_at(self.addr, method, path, h, body).await
    }
    async fn request_at(
        addr: SocketAddr,
        method: &str,
        path: &str,
        h: &[(&str, &str)],
        body: &str,
    ) -> Reply {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let mut wire = format!(
            "{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (k, v) in h {
            wire.push_str(&format!("{k}: {v}\r\n"));
        }
        wire.push_str("\r\n");
        wire.push_str(body);
        stream.write_all(wire.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        timeout(Duration::from_secs(12), stream.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = std::str::from_utf8(&bytes[..split]).unwrap();
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
                (k.to_ascii_lowercase(), v.trim().to_owned())
            })
            .collect();
        Reply {
            status,
            headers,
            body: String::from_utf8(bytes[split + 4..].to_vec()).unwrap(),
        }
    }
    async fn launch(&self, user: &str) -> Value {
        let r = self
            .request(
                "POST",
                LAUNCH,
                &delegated(),
                &json!({"user_id":user,"source":"synthetic.oas"}).to_string(),
            )
            .await;
        assert_eq!(r.status, 201);
        assert_eq!(r.headers["cache-control"], "no-store");
        assert!(!r.headers.contains_key("set-cookie"));
        assert!(!r.body.contains(self.fixture.0.to_str().unwrap()));
        serde_json::from_str(&r.body).unwrap()
    }
    async fn open(&self, user: &str) -> Session {
        let launch = self.launch(user).await;
        let id = launch["launch_id"].as_str().unwrap().to_string();
        let body = json!({"bootstrap":launch["bootstrap"]}).to_string();
        let exchange = format!("{}/exchange", path(&id));
        let r = self.request("POST", &exchange, &headers(), &body).await;
        assert_eq!(r.status, 200);
        let v: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["viewer_ready"], false);
        let cookie = &r.headers["set-cookie"];
        assert!(cookie.starts_with("__Secure-floe_server_"));
        assert!(cookie.contains(&format!("Path={};", path(&id))));
        assert!(!cookie.contains("Domain="));
        for flag in ["Secure", "HttpOnly", "SameSite=Strict"] {
            assert!(cookie.contains(flag));
        }
        assert_eq!(
            self.request("POST", &exchange, &headers(), &body)
                .await
                .status,
            401
        );
        Session {
            id,
            cookie: cookie.split(';').next().unwrap().into(),
            csrf: v["csrf"].as_str().unwrap().into(),
        }
    }
    async fn shutdown(self) {
        let _ = self.stop.send(());
        timeout(Duration::from_secs(5), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        self.fixture.unchanged();
    }
}

#[tokio::test]
async fn http_demo_keeps_proxy_origin_csrf_and_read_only_boundaries() {
    let s = Server::start_transport(true, true).await;
    let h = vec![
        ("Host", "10.0.0.10:8080"),
        ("Origin", "http://10.0.0.10:8080"),
        ("X-Floe-Proxy-Key", PROXY),
        ("Content-Type", "application/json"),
    ];
    for route in ["/demo", &format!("/server/{}", "a".repeat(64))] {
        let r = s.request("GET", route, &h, "").await;
        assert_eq!(r.status, 200);
        assert!(r.body.contains("name=\"floe-http-test\" content=\"true\""));
        assert!(!r.body.contains("@@HTTP_TEST@@"));
        assert!(
            r.headers["content-security-policy"].contains("connect-src 'self' ws://10.0.0.10:8080")
        );
    }
    for (name, value) in [
        ("Host", "evil.test"),
        ("Origin", "https://10.0.0.10:8080"),
        ("X-Floe-Proxy-Key", "wrong"),
    ] {
        let mut bad = h.clone();
        bad.iter_mut().find(|(k, _)| *k == name).unwrap().1 = value;
        assert_eq!(
            s.request(
                "POST",
                "/api/v1/demo/launches",
                &bad,
                r#"{"sample_id":"sample1"}"#
            )
            .await
            .status,
            403
        );
    }
    let mut direct = h.clone();
    direct.retain(|(k, _)| *k != "X-Floe-Proxy-Key");
    assert_eq!(s.request("GET", "/demo", &direct, "").await.status, 403);
    let r = s
        .request(
            "POST",
            "/api/v1/demo/launches",
            &h,
            r#"{"sample_id":"sample1"}"#,
        )
        .await;
    assert_eq!(r.status, 201);
    let launch: Value = serde_json::from_str(&r.body).unwrap();
    let id = launch["launch_id"].as_str().unwrap();
    let exchange = format!("{}/exchange", path(id));
    let body = json!({"bootstrap":launch["bootstrap"]}).to_string();
    let r = s.request("POST", &exchange, &h, &body).await;
    assert_eq!(r.status, 200);
    let credentials: Value = serde_json::from_str(&r.body).unwrap();
    let cookie = &r.headers["set-cookie"];
    assert!(cookie.starts_with("floe_http_test_"));
    assert!(!cookie.contains("Secure") && !cookie.contains("Domain="));
    assert!(cookie.contains("HttpOnly; SameSite=Strict"));
    assert!(cookie.contains(&format!("Path={};", path(id))));
    assert_eq!(s.request("POST", &exchange, &h, &body).await.status, 401);
    let mut auth = h.clone();
    auth.push(("Cookie", cookie.split(';').next().unwrap()));
    assert_eq!(s.request("GET", &path(id), &auth, "").await.status, 401);
    auth.push(("X-Floe-CSRF", credentials["csrf"].as_str().unwrap()));
    assert_eq!(s.request("GET", &path(id), &auth, "").await.status, 200);
    let renamed =
        cookie
            .split(';')
            .next()
            .unwrap()
            .replacen("floe_http_test_", "__Secure-floe_server_", 1);
    let mut wrong = auth.clone();
    wrong.iter_mut().find(|(k, _)| *k == "Cookie").unwrap().1 = &renamed;
    assert_eq!(s.request("GET", &path(id), &wrong, "").await.status, 401);
    for route in [
        "/api/v1/catalog",
        "/api/v1/operations",
        "/api/v1/exports",
        "/api/v1/drc/review/notes",
    ] {
        assert_eq!(s.request("GET", route, &auth, "").await.status, 404);
    }
    assert_eq!(s.request("POST", LAUNCH, &auth, "{}").await.status, 401);
    let logout = s.request("DELETE", &path(id), &auth, "").await;
    assert_eq!(logout.status, 204);
    let expired = &logout.headers["set-cookie"];
    assert!(expired.starts_with(&format!("floe_http_test_{id}=;")));
    assert!(expired.contains("Max-Age=0") && !expired.contains("Secure"));
    assert_eq!(s.request("GET", &path(id), &auth, "").await.status, 401);
    s.shutdown().await;
}

#[tokio::test]
async fn public_demo_only_accepts_ids_and_keeps_session_authority_isolated() {
    let s = Server::start_mode(true).await;
    let r = s.request("GET", "/demo", &headers(), "").await;
    assert_eq!(r.status, 200);
    assert!(r.body.contains("name=\"floe-http-test\" content=\"false\""));
    assert!(r.body.contains("demo.js") && !r.body.contains("@@BUNDLE@@"));
    assert!(r.headers["content-security-policy"].contains("frame-ancestors 'none'"));
    let r = s
        .request("GET", "/api/v1/demo/samples", &headers(), "")
        .await;
    assert_eq!(
        serde_json::from_str::<Value>(&r.body).unwrap(),
        json!({"samples":["sample1"]})
    );
    assert_eq!(
        s.request("POST", LAUNCH, &delegated(), "{}").await.status,
        401
    );
    let launch_path = "/api/v1/demo/launches";
    let mut h = headers();
    h.retain(|(k, _)| *k != "Origin");
    assert_eq!(
        s.request("POST", launch_path, &h, r#"{"sample_id":"sample1"}"#)
            .await
            .status,
        401
    );
    for body in [
        r#"{"sample_id":"../synthetic.oas"}"#,
        r#"{"sample_id":"synthetic.oas"}"#,
        r#"{"sample_id":"sample1","source":"synthetic.oas"}"#,
    ] {
        assert_eq!(
            s.request("POST", launch_path, &headers(), body)
                .await
                .status,
            400
        );
    }
    let mut sessions = Vec::new();
    for _ in 0..2 {
        let r = s
            .request(
                "POST",
                launch_path,
                &headers(),
                r#"{"sample_id":"sample1"}"#,
            )
            .await;
        assert_eq!(r.status, 201);
        let launch: Value = serde_json::from_str(&r.body).unwrap();
        let id = launch["launch_id"].as_str().unwrap().to_string();
        let r = s
            .request(
                "POST",
                &format!("{}/exchange", path(&id)),
                &headers(),
                &json!({"bootstrap":launch["bootstrap"]}).to_string(),
            )
            .await;
        assert_eq!(r.status, 200);
        let v: Value = serde_json::from_str(&r.body).unwrap();
        sessions.push(Session {
            id,
            cookie: r.headers["set-cookie"].split(';').next().unwrap().into(),
            csrf: v["csrf"].as_str().unwrap().into(),
        });
    }
    let a = &sessions[0];
    let b = &sessions[1];
    assert_ne!(a.id, b.id);
    assert_eq!(
        s.request("GET", &path(&b.id), &a.headers(), "")
            .await
            .status,
        401
    );
    let r = s.request("GET", &path(&a.id), &a.headers(), "").await;
    assert_eq!(
        serde_json::from_str::<Value>(&r.body).unwrap()["public_demo"],
        true
    );
    for path in [
        "/api/v1/catalog",
        "/api/v1/browse",
        "/api/v1/index",
        "/api/v1/exports",
        "/api/v1/drc/review/notes",
        "/api/v1/defaults",
    ] {
        assert_eq!(
            s.request("POST", path, &a.headers(), "{}").await.status,
            404
        );
    }
    assert_eq!(
        s.request("DELETE", &path(&a.id), &a.headers(), "")
            .await
            .status,
        204
    );
    assert_eq!(
        s.request("GET", &path(&b.id), &b.headers(), "")
            .await
            .status,
        200
    );
    // Even after freeing a slot, the process-wide launch rate is exhausted.
    assert_eq!(
        s.request(
            "POST",
            launch_path,
            &headers(),
            r#"{"sample_id":"sample1"}"#
        )
        .await
        .status,
        429
    );
    s.shutdown().await;
    let ordinary = Server::start().await;
    assert_eq!(
        ordinary
            .request("GET", "/demo", &headers(), "")
            .await
            .status,
        404
    );
    assert_eq!(
        ordinary
            .request("GET", "/api/v1/demo/samples", &headers(), "")
            .await
            .status,
        404
    );
    ordinary.shutdown().await;
}

#[tokio::test]
async fn server_shell_is_static_bounded_and_cannot_load_owner_assets() {
    let s = Server::start().await;
    let route = format!("/server/{}", "a".repeat(64));
    let mut h = headers();
    h.retain(|(k, _)| *k != "Origin"); // ordinary top-level HTTPS navigation
    let r = s.request("GET", &route, &h, "").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["content-type"], "text/html; charset=utf-8");
    assert_eq!(r.headers["cache-control"], "no-store");
    assert_eq!(r.headers["referrer-policy"], "no-referrer");
    assert_eq!(r.headers["x-frame-options"], "DENY");
    let csp = &r.headers["content-security-policy"];
    assert!(csp.contains("script-src 'self'") && !csp.contains("unsafe-eval"));
    assert!(csp.contains("connect-src 'self' wss://service.example.test"));
    assert!(csp.contains("frame-ancestors 'none'") && csp.contains("base-uri 'none'"));
    assert!(r.body.contains("server.js") && !r.body.contains("@@BUNDLE@@"));
    assert!(!r.body.contains("/app.js") && !r.body.contains("synthetic.oas"));
    assert!(!r.body.contains("@@VIEW_") && r.body.contains("/viewer.js"));
    let owner = include_str!("../ui/index.html");
    for name in ["controls", "toolbar"] {
        let fragment = owner
            .split_once(&format!("<!-- floe-view-{name}:start -->"))
            .unwrap()
            .1
            .split_once(&format!("<!-- floe-view-{name}:end -->"))
            .unwrap()
            .0
            .replace("id=\"", "id=\"server-")
            .replace("for=\"", "for=\"server-");
        assert!(r.body.contains(&fragment), "same canonical {name}");
    }
    assert!(!r.headers.contains_key("set-cookie"));
    assert!(s.request("HEAD", &route, &h, "").await.body.is_empty());
    let bundle = floe_web::transport::BUNDLE;
    for name in [
        "server.js",
        "server.css",
        "app.css",
        "protocol.js",
        "image-decode.js",
        "gestures.js",
        "viewer.js",
        "viewer.css",
        "minimap.js",
        "fill-editor.js",
        "presets.js",
        "palette.js",
        "menubar.js",
        "panes.js",
    ] {
        let r = s
            .request("GET", &format!("/server-assets/{bundle}/{name}"), &h, "")
            .await;
        assert_eq!(r.status, 200);
        assert!(!r.body.is_empty() && r.body.len() < 256 * 1024);
    }
    for route in [
        format!("/server-assets/{bundle}/app.js"),
        format!("/server-assets/{bundle}/guest.js"),
        format!("/server-assets/{bundle}/defaults.js"),
        "/server-assets/old/server.js".into(),
        "/server/invalid".into(),
        "/".into(),
    ] {
        assert_eq!(s.request("GET", &route, &h, "").await.status, 404);
    }
    h.retain(|(k, _)| *k != "X-Floe-Proxy-Key");
    assert_eq!(s.request("GET", &route, &h, "").await.status, 403);
    assert_eq!(
        s.request("GET", &format!("/server-assets/{bundle}/server.js"), &h, "")
            .await
            .status,
        403
    );
    assert_eq!(s.broker.pending_workers().unwrap(), 0);
    s.shutdown().await;
}
#[tokio::test]
async fn only_the_authenticated_teebox_backchannel_can_delegate() {
    let s = Server::start().await;
    let body = json!({"user_id":"alice","source":"synthetic.oas"}).to_string();
    for name in ["X-Floe-Delegation-Key", "X-Floe-Service-Client"] {
        let mut h = delegated();
        h.retain(|(k, _)| *k != name);
        assert_eq!(s.request("POST", LAUNCH, &h, &body).await.status, 401);
        let mut h = delegated();
        h.push((name, "forged"));
        assert_eq!(s.request("POST", LAUNCH, &h, &body).await.status, 401);
    }
    for name in ["Host", "X-Floe-Proxy-Key"] {
        let mut h = delegated();
        h.retain(|(k, _)| *k != name);
        assert_eq!(s.request("POST", LAUNCH, &h, &body).await.status, 403);
        let mut h = delegated();
        h.push((name, "forged"));
        assert_eq!(s.request("POST", LAUNCH, &h, &body).await.status, 403);
    }
    for (field, value) in [
        ("X-Floe-Delegation-Key", "invalid"),
        ("X-Floe-Service-Client", "alice"),
    ] {
        let mut h = delegated();
        h.retain(|(k, _)| *k != field);
        h.push((field, value));
        assert_eq!(s.request("POST", LAUNCH, &h, &body).await.status, 401);
    }
    // Forwarded attributes neither authenticate nor choose a different origin.
    let h = [
        ("Host", "evil.test"),
        ("X-Forwarded-Host", "service.example.test"),
        ("X-Forwarded-Proto", "https"),
        ("X-Floe-Proxy-Key", PROXY),
    ];
    assert_eq!(s.request("POST", LAUNCH, &h, &body).await.status, 403);
    assert_eq!(
        s.request(
            "POST",
            &format!("{LAUNCH}?user_id=bob"),
            &delegated(),
            &body
        )
        .await
        .status,
        403
    );
    let mut h = delegated();
    h.retain(|(k, _)| *k != "Origin"); // server backchannel has no browser Origin
                                       // The proxy consumes account/password Authorization independently; it is
                                       // not a substitute for the dedicated TeeBox delegation proof downstream.
    h.push(("Authorization", "Basic c3ludGhldGljOnRlc3Q="));
    assert_eq!(s.request("POST", LAUNCH, &h, &body).await.status, 201);
    s.shutdown().await;
}
#[tokio::test]
async fn cookie_csrf_and_route_target_isolate_users_and_cannot_create_sessions() {
    let s = Server::start().await;
    let a = s.open("alice").await;
    let b = s.open("bob").await;
    let own = s.request("GET", &path(&a.id), &a.headers(), "").await;
    assert_eq!(own.status, 200);
    let v: Value = serde_json::from_str(&own.body).unwrap();
    assert_eq!(
        v["principal"],
        json!({"namespace":"teebox","subject":"alice"})
    );
    assert!(!own.body.contains("synthetic.oas") && !own.body.contains(&a.csrf));
    for method in ["GET", "HEAD", "DELETE"] {
        assert_eq!(
            s.request(method, &path(&b.id), &a.headers(), "")
                .await
                .status,
            401
        );
        assert_eq!(
            s.request(method, &path(&a.id), &headers(), "").await.status,
            401
        );
    }
    let mut h = a.headers();
    h.retain(|(k, _)| *k != "X-Floe-CSRF");
    assert_eq!(s.request("GET", &path(&a.id), &h, "").await.status, 401);
    h.push(("X-Floe-CSRF", &b.csrf));
    assert_eq!(s.request("GET", &path(&a.id), &h, "").await.status, 401);
    let mut h = a.headers();
    h.push(("Cookie", &a.cookie));
    assert_eq!(s.request("GET", &path(&a.id), &h, "").await.status, 401);
    // Rename A's cookie into B's namespace as well: the name is not the fence.
    let renamed = format!(
        "{}={}",
        b.cookie.split_once('=').unwrap().0,
        a.cookie.split_once('=').unwrap().1
    );
    let mut h = a.headers();
    h.retain(|(k, _)| *k != "Cookie");
    h.push(("Cookie", &renamed));
    assert_eq!(s.request("GET", &path(&b.id), &h, "").await.status, 401);
    assert_eq!(
        s.request(
            "POST",
            LAUNCH,
            &a.headers(),
            &json!({"user_id":"bob","source":"synthetic.oas"}).to_string()
        )
        .await
        .status,
        401
    );
    assert_eq!(
        s.request("DELETE", &format!("{LAUNCH}/{}", b.id), &a.headers(), "")
            .await
            .status,
        401
    );
    // A valid owner session does not expose local Gateway/file/write APIs here.
    for route in [
        "/api/v1/catalog",
        "/api/v1/browse",
        "/api/v1/defaults",
        "/api/v1/drc/review/notes",
    ] {
        assert_eq!(s.request("GET", route, &a.headers(), "").await.status, 404);
    }
    let logout = s.request("DELETE", &path(&a.id), &a.headers(), "").await;
    assert_eq!(logout.status, 204);
    assert!(logout.headers["set-cookie"].contains("Max-Age=0"));
    assert!(logout.headers["set-cookie"].contains(&format!("Path={};", path(&a.id))));
    assert!(logout.headers["set-cookie"].contains("; Secure"));
    assert_eq!(
        s.request("GET", &path(&a.id), &a.headers(), "")
            .await
            .status,
        401
    );
    assert_eq!(
        s.request("GET", &path(&b.id), &b.headers(), "")
            .await
            .status,
        200
    );
    s.shutdown().await;
}
#[tokio::test]
async fn launch_tokens_have_one_target_expiry_revocation_and_capacity() {
    let s = Server::start().await;
    let a = s.launch("alice").await;
    let b = s.launch("bob").await;
    let aid = a["launch_id"].as_str().unwrap();
    let bid = b["launch_id"].as_str().unwrap();
    let exchange = format!("{}/exchange", path(aid));
    let wrong = json!({"bootstrap":b["bootstrap"]}).to_string();
    assert_eq!(
        s.request("POST", &exchange, &headers(), &wrong)
            .await
            .status,
        401
    );
    let body = json!({"bootstrap":a["bootstrap"]}).to_string();
    let mut h = headers();
    h.retain(|(k, _)| *k != "Origin");
    assert_eq!(s.request("POST", &exchange, &h, &body).await.status, 401);
    h.push(("Origin", "https://evil.test"));
    assert_eq!(s.request("POST", &exchange, &h, &body).await.status, 403);
    assert_eq!(
        s.request(
            "POST",
            LAUNCH,
            &delegated(),
            &json!({"user_id":"carol","source":"synthetic.oas"}).to_string()
        )
        .await
        .status,
        429
    );
    assert_eq!(
        s.request("DELETE", &format!("{LAUNCH}/{aid}"), &delegated(), "")
            .await
            .status,
        204
    );
    assert_eq!(
        s.request("POST", &exchange, &headers(), &body).await.status,
        401
    );
    let c = s.open("carol").await;
    s.broker
        .maintain(Instant::now() + Duration::from_secs(86400))
        .unwrap();
    assert_eq!(
        s.request("GET", &path(&c.id), &c.headers(), "")
            .await
            .status,
        401
    );
    assert_eq!(
        s.request(
            "POST",
            &format!("{}/exchange", path(bid)),
            &headers(),
            &wrong
        )
        .await
        .status,
        401
    );
    s.shutdown().await;
}
#[tokio::test]
async fn strict_bodies_scope_and_denials_do_not_echo_secrets_or_mutate_files() {
    let s = Server::start().await;
    for body in [
        json!({"user_id":"alice","source":"../outside.oas"}),
        json!({"user_id":"alice","source":"synthetic.oas","root":"/"}),
        json!({"user_id":"alice","source":"synthetic.oas","reviewer":"bob"}),
        json!({"user_id":"alice","source":"synthetic.oas","password":"synthetic-no-echo"}),
        json!({"user_id":"alice","source":"x".repeat(5000)}),
    ] {
        let r = s
            .request("POST", LAUNCH, &delegated(), &body.to_string())
            .await;
        assert_eq!(r.status, 400);
        assert_eq!(r.headers["cache-control"], "no-store");
        assert_eq!(r.headers["referrer-policy"], "no-referrer");
        assert!(!r.headers.contains_key("access-control-allow-origin"));
        for private in [DELEGATE, PROXY, "synthetic-no-echo", "outside.oas"] {
            assert!(!r.body.contains(private));
        }
    }
    // Bad authentication wins over JSON parsing and filesystem errors.
    assert_eq!(
        s.request("POST", LAUNCH, &headers(), "{not json")
            .await
            .status,
        401
    );
    let s2 = Server::start().await;
    let a = s.open("alice").await;
    assert_eq!(
        s2.request("GET", &path(&a.id), &a.headers(), "")
            .await
            .status,
        401
    );
    let mut h = a.headers();
    h.retain(|(k, _)| *k != "X-Floe-Proxy-Key");
    assert_eq!(s.request("GET", &path(&a.id), &h, "").await.status, 403);
    s2.shutdown().await;
    s.shutdown().await;
}

#[tokio::test]
async fn standalone_gateway_keeps_its_routes_and_separate_authentication() {
    use floe_web::transport::{self, Gateway, BUNDLE};
    let s = Server::start().await;
    let user = s.open("alice").await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (mut gate, bootstrap) = Gateway::new(addr).unwrap();
    Gateway::enable_https_proxy(&mut gate, PUBLIC, PROXY).unwrap();
    let (stop, done) = oneshot::channel();
    let task = tokio::spawn(transport::serve(listener, gate, async {
        let _ = done.await;
    }));
    let response = Server::request_at(
        addr,
        "POST",
        "/api/v1/session/exchange",
        &headers(),
        &json!({"bootstrap":bootstrap.expose(),"protocol":1,"bundle":BUNDLE}).to_string(),
    )
    .await;
    assert_eq!(response.status, 200);
    let owner_cookie = response.headers["set-cookie"]
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let body: Value = serde_json::from_str(&response.body).unwrap();
    let owner_csrf = body["csrf"].as_str().unwrap();
    let mut owner_headers = headers();
    owner_headers.push(("Cookie", &owner_cookie));
    owner_headers.push(("X-Floe-CSRF", owner_csrf));
    assert_eq!(
        Server::request_at(addr, "GET", "/api/v1/capabilities", &owner_headers, "")
            .await
            .status,
        200
    );
    assert_eq!(
        Server::request_at(addr, "POST", LAUNCH, &delegated(), "{}")
            .await
            .status,
        404
    );
    let renamed_user = format!(
        "{}={}",
        owner_cookie.split_once('=').unwrap().0,
        user.cookie.split_once('=').unwrap().1
    );
    let mut wrong = user.headers();
    wrong.retain(|(k, _)| *k != "Cookie");
    wrong.push(("Cookie", &renamed_user));
    assert_eq!(
        Server::request_at(addr, "GET", "/api/v1/capabilities", &wrong, "")
            .await
            .status,
        401
    );
    assert_eq!(
        s.request("POST", LAUNCH, &owner_headers, "{}").await.status,
        401
    );
    let renamed_owner = format!(
        "{}={}",
        user.cookie.split_once('=').unwrap().0,
        owner_cookie.split_once('=').unwrap().1
    );
    owner_headers.retain(|(k, _)| *k != "Cookie");
    owner_headers.push(("Cookie", &renamed_owner));
    assert_eq!(
        s.request("GET", &path(&user.id), &owner_headers, "")
            .await
            .status,
        401
    );
    let _ = stop.send(());
    timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    s.shutdown().await;
}
