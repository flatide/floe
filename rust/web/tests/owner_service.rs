//! Runs through authenticated HTTP and real native processes on private data.
use floe_app_core::{
    cache,
    managed::{Limits, Resources, Usage},
    native::{Discovery, Indexer, INDEX_VERSION},
    registered::{AccessScope, RegisteredSource},
    render::RenderOptions,
};
use floe_web::{
    auth::Secret,
    service::Service,
    transport::{self, Gateway, BUNDLE, PROTOCOL},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
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
    service: Arc<Service>,
    resources: Arc<Resources>,
    launches: Arc<floe_web::launch::Launches>,
    drc_reader: Option<Arc<floe_web::drc::Service>>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}
struct Login {
    cookie: String,
    csrf: String,
}
impl Harness {
    async fn start(paths: &[PathBuf], indexer: Indexer) -> Self {
        Self::start_with_drc(paths, indexer, None).await
    }
    async fn start_with_drc(
        paths: &[PathBuf],
        indexer: Indexer,
        drc: Option<(&Path, Option<&Path>)>,
    ) -> Self {
        Self::start_with_drc_builds(paths, indexer, drc, false).await
    }
    async fn start_with_drc_builds(
        paths: &[PathBuf],
        indexer: Indexer,
        drc: Option<(&Path, Option<&Path>)>,
        builds: bool,
    ) -> Self {
        Self::configured(paths, indexer, drc, builds, false).await
    }
    async fn configured(
        paths: &[PathBuf],
        indexer: Indexer,
        drc: Option<(&Path, Option<&Path>)>,
        builds: bool,
        defaults: bool,
    ) -> Self {
        Self::configured_limits(paths, indexer, drc, builds, defaults, Limits::default()).await
    }
    async fn configured_limits(
        paths: &[PathBuf],
        indexer: Indexer,
        drc: Option<(&Path, Option<&Path>)>,
        builds: bool,
        defaults: bool,
        limits: Limits,
    ) -> Self {
        Self::configured_sharing(paths, indexer, drc, builds, defaults, limits, false).await
    }
    async fn configured_sharing(
        paths: &[PathBuf],
        indexer: Indexer,
        drc: Option<(&Path, Option<&Path>)>,
        builds: bool,
        defaults: bool,
        limits: Limits,
        sharing: bool,
    ) -> Self {
        let resources = Resources::new(limits).unwrap();
        let fixture = PathBuf::from(std::env::var_os("FLOE_OWNER_FIXTURE").unwrap());
        let scope = AccessScope::new(&[paths
            .first()
            .unwrap_or(&fixture)
            .parent()
            .unwrap()
            .to_owned()])
        .unwrap();
        let sources = paths
            .iter()
            .map(|p| {
                RegisteredSource::register(Arc::clone(&scope), p, &AtomicUsize::new(0)).unwrap()
            })
            .collect();
        let mut options = RenderOptions::local().unwrap();
        options.decode_jobs = 1;
        options.raster_jobs = 1;
        options.budget_mb = 64;
        options.raw = false;
        let service =
            Service::start(sources, Arc::clone(&resources), options, indexer.clone()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (mut gate, bootstrap) = Gateway::with_service(addr, Arc::clone(&service)).unwrap();
        let launches = floe_web::launch::Launches::new();
        Gateway::attach_launches(&mut gate, Arc::clone(&launches)).unwrap();
        let mut drc_reader = None;
        if let Some((pack, rules)) = drc {
            let scope = AccessScope::new(&[pack.parent().unwrap().to_owned()]).unwrap();
            let source_id = service.catalog()["sources"][0]["source_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let drc = floe_web::drc::Service::start_with_rules(
                &resources, scope, pack, None, rules, &source_id,
            )
            .unwrap();
            drc_reader = Some(Arc::clone(&drc));
            if builds {
                Gateway::attach_drc_registry(
                    &mut gate,
                    floe_web::drc::Registry::with_builds(drc, indexer).unwrap(),
                )
                .unwrap();
            } else {
                Gateway::attach_drc(&mut gate, drc).unwrap();
            }
        }
        if defaults {
            Gateway::enable_design_defaults(&mut gate, &[], &[]).unwrap();
        }
        if sharing {
            Gateway::enable_local_sharing(&mut gate).unwrap();
        }
        let (stop, rx) = oneshot::channel();
        let task = tokio::spawn(transport::serve(listener, Arc::clone(&gate), async {
            let _ = rx.await;
        }));
        Self {
            addr,
            gate,
            bootstrap,
            service,
            resources,
            launches,
            drc_reader,
            stop,
            task,
        }
    }
    async fn raw(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, String) {
        let (status, headers, body) = self.raw_bytes(method, path, headers, body).await;
        (status, headers, String::from_utf8(body).unwrap())
    }
    async fn raw_bytes(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> (u16, String, Vec<u8>) {
        let mut stream = TcpStream::connect(self.addr).await.unwrap();
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
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        let read = timeout(Duration::from_secs(7), stream.read_to_end(&mut bytes))
            .await
            .unwrap();
        let split = bytes.windows(4).position(|s| s == b"\r\n\r\n").unwrap();
        let header = std::str::from_utf8(&bytes[..split]).unwrap();
        if let Err(e) = read {
            // An early authenticated/body-limit rejection deliberately does
            // not drain the upload. macOS can RST after the complete response.
            assert_eq!(
                e.kind(),
                std::io::ErrorKind::ConnectionReset,
                "{method} {path}: {e}"
            );
            let n = header
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|n| n.trim().parse::<usize>().unwrap())
                })
                .expect("reset requires a complete length-delimited response");
            assert_eq!(
                bytes.len() - split - 4,
                n,
                "reset truncated {method} {path}"
            );
        }
        (
            header.split_whitespace().nth(1).unwrap().parse().unwrap(),
            header.into(),
            bytes[split + 4..].into(),
        )
    }
    async fn login(&self) -> Login {
        let origin = format!("http://{}", self.addr);
        let (status, headers, body) = self
            .raw(
                "POST",
                "/api/v1/session/exchange",
                &[("Origin", &origin), ("Content-Type", "application/json")],
                &json!({"bootstrap":self.bootstrap.expose(),"protocol":1,"bundle":BUNDLE})
                    .to_string(),
            )
            .await;
        assert_eq!(status, 200);
        let cookie = headers
            .lines()
            .find_map(|s| s.strip_prefix("set-cookie: "))
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .into();
        let reply: Value = serde_json::from_str(&body).unwrap();
        Login {
            cookie,
            csrf: reply["csrf"].as_str().unwrap().into(),
        }
    }
    async fn call(&self, login: &Login, method: &str, path: &str, body: Value) -> (u16, Value) {
        let origin = format!("http://{}", self.addr);
        let body = if body.is_null() {
            String::new()
        } else {
            body.to_string()
        };
        let (status, _, body) = self
            .raw(
                method,
                path,
                &[
                    ("Origin", &origin),
                    ("Cookie", &login.cookie),
                    ("X-Floe-CSRF", &login.csrf),
                    ("Content-Type", "application/json"),
                ],
                &body,
            )
            .await;
        let value = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&body).unwrap()
        };
        assert!(
            !body.contains("/private/") && !body.contains("/Users/"),
            "path leaked in {path}"
        );
        (status, value)
    }
    async fn finished(&self, login: &Login, seq: u64) -> Value {
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            let (status, v) = self
                .call(
                    login,
                    "GET",
                    &format!("/api/v1/operations/{seq}"),
                    Value::Null,
                )
                .await;
            assert_eq!(status, 200);
            if matches!(
                v["phase"].as_str(),
                Some("succeeded" | "incomplete" | "failed" | "cancelled")
            ) {
                return v;
            }
            assert!(Instant::now() < end, "{v}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    async fn connect(&self, login: &Login) -> Socket {
        let mut request = format!("ws://{}/api/v1/events", self.addr)
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("Origin", format!("http://{}", self.addr).parse().unwrap());
        request
            .headers_mut()
            .insert("Cookie", login.cookie.parse().unwrap());
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            format!("{PROTOCOL}, bundle.{BUNDLE}, csrf.{}", login.csrf)
                .parse()
                .unwrap(),
        );
        connect_async(request).await.unwrap().0
    }
    async fn shutdown(self) {
        self.stop.send(()).unwrap();
        timeout(Duration::from_secs(10), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(self.service.is_finished());
        assert_eq!(self.resources.usage(), Usage::default());
        assert_eq!(self.gate.transport_usage().sockets, 0);
        assert_eq!(self.gate.transport_usage().reserved_output_bytes, 0);
    }
}
async fn text(ws: &mut Socket) -> Value {
    loop {
        match timeout(Duration::from_secs(10), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            Message::Ping(p) => ws.send(Message::Pong(p)).await.unwrap(),
            other => panic!("expected text, got {other:?}"),
        }
    }
}
async fn frame(ws: &mut Socket) -> (Value, Value) {
    let hello = text(ws).await;
    assert_eq!(hello["type"], "hello");
    loop {
        match timeout(Duration::from_secs(10), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            Message::Text(_) => (),
            Message::Ping(p) => ws.send(Message::Pong(p)).await.unwrap(),
            Message::Binary(bytes) => {
                let n = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
                let h: Value = serde_json::from_slice(&bytes[4..4 + n]).unwrap();
                assert!(bytes[4 + n..].starts_with(b"\x89PNG\r\n\x1a\n"));
                assert_eq!(
                    h["payload_length"]
                        .as_str()
                        .unwrap()
                        .parse::<usize>()
                        .unwrap(),
                    bytes.len() - 4 - n
                );
                ws.send(Message::Text(json!({"type":"frame.ack","seq":"1","connection_epoch":hello["connection_epoch"],"frame_id":h["frame_id"],"disposition":"displayed"}).to_string().into())).await.unwrap();
                return (hello, h);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
async fn closed(ws: &mut Socket) {
    loop {
        match timeout(Duration::from_secs(5), ws.next()).await.unwrap() {
            None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
            Some(Ok(Message::Ping(p))) => {
                let _ = ws.send(Message::Pong(p)).await;
            }
            _ => (),
        }
    }
}
fn native() -> Indexer {
    Indexer::discover(&Discovery::local().unwrap()).unwrap()
}
fn open(seq: &str, id: &Value, mode: &str, levels: Value) -> Value {
    json!({"kind":"open","seq":seq,"source_id":id,"mode":mode,"levels":levels,"body":{"pixels":[257,191],"labels":false,"depth":"full","detail":"high","thin":"keep"}})
}

#[path = "support/deck_levels.rs"]
mod deck_levels;
#[path = "support/deck_modes.rs"]
mod deck_modes;
#[path = "support/defaults.rs"]
mod defaults;
#[path = "support/drc_isolation.rs"]
mod drc_isolation;
#[path = "support/exports.rs"]
mod exports;
#[path = "support/guest_drc.rs"]
mod guest_drc;
#[path = "support/index_open.rs"]
mod index_open;
#[path = "support/launch.rs"]
mod launch;
#[path = "support/settings.rs"]
mod settings;
#[path = "support/window_display.rs"]
mod window_display;

#[tokio::test(flavor = "current_thread")]
#[ignore = "run tools/validate_owner_service.py with private source files"]
async fn authenticated_catalog_index_open_reopen_and_logout() {
    let source = PathBuf::from(std::env::var_os("FLOE_OWNER_FIXTURE").expect("private fixture"));
    let deck = source.parent().unwrap().join("test.jb");
    fs::write(&deck,format!("MTITLE 1,Mask\nCHIP C\n$ (1,PATTERN,TC='{}',AD=0.001,LY={{1}},DT={{0}},UX=100,UY=100)\n$ (2,MISSING,TC=missing.oas,AD=0.001,LY={{1}},DT={{0}},UX=100,UY=100)\nROWS 0/0\n",source.file_name().unwrap().to_str().unwrap())).unwrap();
    let h = Harness::start(&[source.clone(), deck], native()).await;
    assert_eq!(h.raw("GET", "/api/v1/catalog", &[], "").await.0, 401);
    let login = h.login().await;
    let (status, catalog) = h.call(&login, "GET", "/api/v1/catalog", Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(catalog["sources"].as_array().unwrap().len(), 2);
    let layout_id = catalog["sources"][0]["source_id"].clone();
    let deck_id = catalog["sources"][1]["source_id"].clone();
    let (status, levels) = h
        .call(
            &login,
            "GET",
            &format!("/api/v1/catalog/{}/levels/0", deck_id.as_str().unwrap()),
            Value::Null,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(levels["levels"][0]["id"], "1");
    assert_eq!(levels["total"], 2);
    assert_eq!(
        h.call(
            &login,
            "POST",
            "/api/v1/operations",
            json!({"kind":"index","seq":"1","source_id":"/private/arbitrary.oas","options":{}})
        )
        .await
        .0,
        404
    );
    for bad in [
        json!({"kind":"index","seq":"1","source_id":layout_id,"options":{"path":"/anything"}}),
        json!({"kind":"index","seq":"1","source_id":layout_id,"options":null}),
        json!({"kind":"index","seq":"1","source_id":layout_id,"options":{"jobs":17}}),
    ] {
        assert_eq!(
            h.call(&login, "POST", "/api/v1/operations", bad).await.0,
            400
        );
    }
    assert_eq!(
        h.call(&login, "GET", "/api/v1/operations", Value::Null)
            .await
            .1["last_seq"],
        "0"
    );
    let mut control = h.connect(&login).await;
    assert!(text(&mut control).await.get("view_id").is_none());
    assert_eq!(
        h.call(
            &login,
            "POST",
            "/api/v1/operations",
            open("1", &layout_id, "level", json!({"mode":"all"}))
        )
        .await
        .0,
        202
    );
    let failed = h.finished(&login, 1).await;
    assert_eq!(failed["phase"], "failed");
    assert_eq!(failed["error"], "index_unavailable");
    let cache_dir = cache::cache_path(&source).unwrap();
    assert!(!cache_dir.exists());
    let index = json!({"kind":"index","seq":"2","source_id":layout_id,"options":{"jobs":2}});
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", index.clone())
            .await
            .0,
        202
    );
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", index.clone())
            .await
            .0,
        202
    );
    assert_eq!(h.finished(&login, 2).await["phase"], "succeeded");
    let stamp = fs::metadata(cache_dir.join("design.ovm"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        h.call(&login, "POST", "/api/v1/operations", index).await.1["phase"],
        "succeeded"
    );
    assert_eq!(
        fs::metadata(cache_dir.join("design.ovm"))
            .unwrap()
            .modified()
            .unwrap(),
        stamp
    );
    assert_eq!(h.call(&login,"POST","/api/v1/operations",json!({"kind":"index","seq":"2","source_id":layout_id,"options":{"jobs":2,"force":true}})).await.0,409);
    assert_eq!(
        h.call(
            &login,
            "POST",
            "/api/v1/operations",
            open("3", &layout_id, "level", json!({"mode":"all"}))
        )
        .await
        .0,
        202
    );
    let opened = h.finished(&login, 3).await;
    assert_eq!(opened["phase"], "succeeded", "{opened}");
    let first_id = opened["view_id"].clone();
    // The pre-open connection stays control-only, even after a view exists.
    control
        .send(Message::Text(
            json!({"type":"ping","seq":"1"}).to_string().into(),
        ))
        .await
        .unwrap();
    assert_eq!(text(&mut control).await["type"], "pong");
    control.close(None).await.unwrap();
    closed(&mut control).await;
    let mut ws = h.connect(&login).await;
    let (_, first) = frame(&mut ws).await;
    assert_eq!(first["view_id"], first_id);
    assert_eq!(first["state_rev"], "1");
    assert_eq!(first["generation"], "1");
    assert_eq!(first["width"], 257);
    let (status, rows) = h
        .call(
            &login,
            "GET",
            &format!("/api/v1/views/{}/layers/0", first_id.as_str().unwrap()),
            Value::Null,
        )
        .await;
    assert_eq!(status, 200);
    assert!(!rows["rows"].as_array().unwrap().is_empty());
    assert_eq!(rows["render_key"], first["render_key"]);
    assert!(rows["rows"].as_array().unwrap().len() <= 64);
    assert_eq!(h.call(&login,"POST","/api/v1/operations",json!({"kind":"index","seq":"4","source_id":layout_id,"options":{"jobs":2,"force":true}})).await.0,202);
    assert_eq!(h.finished(&login, 4).await["error"], "busy");
    assert_eq!(
        fs::metadata(cache_dir.join("design.ovm"))
            .unwrap()
            .modified()
            .unwrap(),
        stamp
    );
    assert_eq!(
        h.call(
            &login,
            "DELETE",
            &format!("/api/v1/views/{}", first_id.as_str().unwrap()),
            Value::Null
        )
        .await
        .0,
        202
    );
    closed(&mut ws).await;
    assert_eq!(
        h.call(
            &login,
            "POST",
            "/api/v1/operations",
            open("5", &deck_id, "chip", json!({"mode":"only","ids":["1"]}))
        )
        .await
        .0,
        202
    );
    let opened = h.finished(&login, 5).await;
    assert_eq!(opened["phase"], "succeeded", "{opened}");
    assert_ne!(opened["view_id"], first_id);
    let mut ws = h.connect(&login).await;
    let (hello, second) = frame(&mut ws).await;
    assert_ne!(second["dataset_revision"], first["dataset_revision"]);
    assert_eq!(second["state_rev"], "1");
    assert_eq!(
        h.call(
            &login,
            "DELETE",
            &format!("/api/v1/views/{}", first_id.as_str().unwrap()),
            Value::Null
        )
        .await
        .0,
        404
    );
    let current = h.call(&login, "GET", "/api/v1/view", Value::Null).await.1;
    assert_eq!(current["source_id"], deck_id);
    assert_eq!(current["mode"], "chip");
    assert_eq!(current["view"]["effective_thin"], "keep");
    let id = opened["view_id"].as_str().unwrap();
    let rows = h
        .call(
            &login,
            "GET",
            &format!("/api/v1/views/{id}/layers/0"),
            Value::Null,
        )
        .await
        .1;
    assert!(rows["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["head"] == true));
    assert!(rows["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| !r["parent"].is_null()));
    // Level-mode heads must reflect selected hidden render rows as well.
    assert_eq!(
        h.call(
            &login,
            "DELETE",
            &format!("/api/v1/views/{id}"),
            Value::Null
        )
        .await
        .0,
        202
    );
    closed(&mut ws).await;
    assert_eq!(
        h.call(
            &login,
            "POST",
            "/api/v1/operations",
            open("6", &deck_id, "level", json!({"mode":"only","ids":["1"]}))
        )
        .await
        .0,
        202
    );
    let level = h.finished(&login, 6).await;
    assert_eq!(level["phase"], "succeeded");
    let id = level["view_id"].as_str().unwrap();
    let mut ws = h.connect(&login).await;
    let (level_hello, _) = frame(&mut ws).await;
    let rows = h
        .call(
            &login,
            "GET",
            &format!("/api/v1/views/{id}/layers/0"),
            Value::Null,
        )
        .await
        .1;
    assert_eq!(rows["rows"].as_array().unwrap().len(), 1);
    let pair = rows["rows"][0]["pair"].clone();
    ws.send(Message::Text(json!({"type":"view.set","seq":"2","connection_epoch":level_hello["connection_epoch"],"view_id":id,"base_state_rev":"1","body":{"layers":{"mode":"only","pairs":[pair]}}}).to_string().into())).await.unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let state = h.call(&login, "GET", "/api/v1/view", Value::Null).await.1;
        if state["view"]["state_rev"] == "2" {
            break;
        }
        assert!(Instant::now() < end);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        h.call(
            &login,
            "GET",
            &format!("/api/v1/views/{id}/layers/0"),
            Value::Null
        )
        .await
        .1["rows"][0]["visible"],
        true
    );
    // Keep the stream live during logout; it must not require another input.
    ws.send(Message::Text(
        json!({"type":"ping","seq":"3"}).to_string().into(),
    ))
    .await
    .unwrap();
    let _ = hello;
    assert_eq!(
        h.call(&login, "DELETE", "/api/v1/session", Value::Null)
            .await
            .0,
        204
    );
    closed(&mut ws).await;
    assert_eq!(
        h.call(&login, "GET", "/api/v1/catalog", Value::Null)
            .await
            .0,
        401
    );
    h.shutdown().await;

    // Revocation and server stop also terminate in-flight index children.
    for logout in [true, false] {
        let binary = source.parent().unwrap().join("owner-controlled-indexer");
        fs::write(&binary,format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'floe-index {INDEX_VERSION}\\n'; exit 0; fi\nprintf '%s' \"$$\" > \"$2.owner-started\"\nprintf '[vfs] parsing... (5s, rss ?)\\n' >&2\nkill -STOP \"$$\"\nexit 7\n")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let indexer = Indexer::discover(&Discovery {
            override_path: Some(binary),
            development_root: None,
            executable: PathBuf::from("/unused"),
            search_path: None,
        })
        .unwrap();
        let h = Harness::start(std::slice::from_ref(&source), indexer).await;
        let login = h.login().await;
        let catalog = h
            .call(&login, "GET", "/api/v1/catalog", Value::Null)
            .await
            .1;
        let marker = PathBuf::from(format!("{}.owner-started", source.display()));
        if marker.exists() {
            fs::remove_file(&marker).unwrap();
        }
        assert_eq!(h.call(&login,"POST","/api/v1/operations",json!({"kind":"index","seq":"1","source_id":catalog["sources"][0]["source_id"],"options":{"jobs":2,"force":true}})).await.0,202);
        let end = Instant::now() + Duration::from_secs(10);
        while !marker.exists() {
            assert!(Instant::now() < end);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if logout {
            assert_eq!(
                h.call(&login, "DELETE", "/api/v1/session", Value::Null)
                    .await
                    .0,
                204
            );
        }
        let service = Arc::clone(&h.service);
        h.shutdown().await;
        assert_eq!(service.operation(1).unwrap()["phase"], "cancelled");
        let pid = fs::read_to_string(&marker).unwrap();
        // Read-only liveness check of the known child, via /bin/kill (PATH empty).
        let alive = std::process::Command::new(Path::new("/bin/kill"))
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(!alive.success());
    }
    println!("RUST OWNER SERVICE: ALL OK (auth/catalog, index idempotency, startup, layers/deck reopen, logout/shutdown child reap)");
}
