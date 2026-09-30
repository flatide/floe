//! A view root is another cell's frame while DRC positions and guest
//! navigation are top-cell coordinates. Guests never inherit a root: an
//! Explore fork starts at the top, and a Follow guest's viewport reads
//! refuse while the owner is under one. Real HTTP/WS + native daemon on a
//! private copy of valmini (FLOE_OWNER_FIXTURE, FLOE_INDEX_BIN, FLOE_RENDERD_BIN).
use floe_app_core::{
    cache,
    managed::{Limits, Resources},
    native::{Discovery, Indexer},
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
    bootstrap: Secret,
    service: Arc<Service>,
    drc: Arc<floe_web::drc::Service>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}
/// Cookie + CSRF; `guest` picks the guest CSRF header.
struct Login {
    cookie: String,
    csrf: String,
    guest: bool,
}
impl Harness {
    async fn start(source: &Path, pack: &Path) -> Self {
        let resources = Resources::new(Limits {
            workers: 4,
            ..Limits::default()
        })
        .unwrap();
        let scope = AccessScope::new(&[source.parent().unwrap().to_owned()]).unwrap();
        let sources =
            vec![RegisteredSource::register(scope, source, &AtomicUsize::new(0)).unwrap()];
        let mut options = RenderOptions::local().unwrap();
        options.decode_jobs = 1;
        options.raster_jobs = 1;
        options.budget_mb = 64;
        options.raw = false;
        let indexer = Indexer::discover(&Discovery::local().unwrap()).unwrap();
        let service = Service::start(sources, Arc::clone(&resources), options, indexer).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (mut gate, bootstrap) = Gateway::with_service(addr, Arc::clone(&service)).unwrap();
        let source_id = service.catalog()["sources"][0]["source_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let drc = floe_web::drc::Service::start_with_rules(
            &resources,
            AccessScope::new(&[pack.parent().unwrap().to_owned()]).unwrap(),
            pack,
            None,
            None,
            &source_id,
        )
        .unwrap();
        Gateway::attach_drc(&mut gate, Arc::clone(&drc)).unwrap();
        Gateway::enable_local_sharing(&mut gate).unwrap();
        let (stop, rx) = oneshot::channel();
        let task = tokio::spawn(transport::serve(listener, gate, async {
            let _ = rx.await;
        }));
        Self {
            addr,
            bootstrap,
            service,
            drc,
            stop,
            task,
        }
    }
    async fn http(
        &self,
        method: &str,
        path: &str,
        login: Option<&Login>,
        body: Value,
    ) -> (u16, String, Value) {
        let body = if body.is_null() {
            String::new()
        } else {
            body.to_string()
        };
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {0}\r\nConnection: close\r\nOrigin: http://{0}\r\nContent-Type: application/json\r\nContent-Length: {1}\r\n",
            self.addr,
            body.len()
        );
        if let Some(l) = login {
            let csrf = if l.guest {
                "X-Floe-Guest-CSRF"
            } else {
                "X-Floe-CSRF"
            };
            request.push_str(&format!("Cookie: {}\r\n{csrf}: {}\r\n", l.cookie, l.csrf));
        }
        request.push_str("\r\n");
        request.push_str(&body);
        let mut stream = TcpStream::connect(self.addr).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        timeout(Duration::from_secs(40), stream.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        (
            head.split_whitespace().nth(1).unwrap().parse().unwrap(),
            head.into(),
            serde_json::from_str(body).unwrap_or(Value::Null),
        )
    }
    async fn call(&self, login: &Login, method: &str, path: &str, body: Value) -> (u16, Value) {
        let (status, _, value) = self.http(method, path, Some(login), body).await;
        (status, value)
    }
    async fn login(&self) -> Login {
        let body = json!({"bootstrap":self.bootstrap.expose(),"protocol":1,"bundle":BUNDLE});
        let (status, head, reply) = self
            .http("POST", "/api/v1/session/exchange", None, body)
            .await;
        assert_eq!(status, 200);
        Login {
            cookie: cookie(&head),
            csrf: reply["csrf"].as_str().unwrap().into(),
            guest: false,
        }
    }
    async fn view(&self, owner: &Login) -> Value {
        self.call(owner, "GET", "/api/v1/view", Value::Null).await.1["view"].clone()
    }
    async fn peer(&self, path: &str, login: &Login) -> Peer {
        let mut request = format!("ws://{}{path}", self.addr)
            .into_client_request()
            .unwrap();
        let prefix = if login.guest { "guest-csrf" } else { "csrf" };
        let headers = request.headers_mut();
        headers.insert("Origin", format!("http://{}", self.addr).parse().unwrap());
        headers.insert("Cookie", login.cookie.parse().unwrap());
        headers.insert(
            "Sec-WebSocket-Protocol",
            format!("{PROTOCOL}, bundle.{BUNDLE}, {prefix}.{}", login.csrf)
                .parse()
                .unwrap(),
        );
        let mut peer = Peer {
            ws: connect_async(request).await.unwrap().0,
            hello: Value::Null,
            seq: 0,
        };
        peer.hello = peer.next().await;
        assert!(["hello", "share.hello"].contains(&peer.hello["type"].as_str().unwrap()));
        peer
    }
    /// Issue a share of the owner's current state and redeem its invitation.
    async fn share(&self, owner: &Login, mode: &str, approval: &Value) -> (String, Login) {
        let view = self.view(owner).await;
        let issue = json!({"view_id":view["view_id"],"base_state_rev":view["state_rev"],
            "mode":mode,"approve":true,"drc":approval});
        let (code, invite) = self.call(owner, "POST", "/api/v1/shares", issue).await;
        assert_eq!(code, 200, "{invite}");
        let id = invite["share_id"].as_str().unwrap().to_owned();
        let redeem = json!({"invite":invite["invite"],"protocol":1,"bundle":BUNDLE});
        let (code, head, reply) = self
            .http(
                "POST",
                &format!("/api/v1/guest/{id}/exchange"),
                None,
                redeem,
            )
            .await;
        assert_eq!(code, 200);
        let guest = Login {
            cookie: cookie(&head),
            csrf: reply["csrf"].as_str().unwrap().into(),
            guest: true,
        };
        (id, guest)
    }
    async fn shutdown(self) {
        self.stop.send(()).unwrap();
        timeout(Duration::from_secs(10), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(self.service.is_finished());
    }
}
fn cookie(head: &str) -> String {
    head.lines()
        .find_map(|s| s.strip_prefix("set-cookie: "))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into()
}
/// One socket; every image is acknowledged as displayed so frame credit
/// and the ACK timeout never end the connection mid-test.
struct Peer {
    ws: Socket,
    hello: Value,
    seq: u64,
}
impl Peer {
    async fn next(&mut self) -> Value {
        loop {
            match timeout(Duration::from_secs(20), self.ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
            {
                Message::Text(t) => return serde_json::from_str(&t).unwrap(),
                Message::Ping(p) => self.ws.send(Message::Pong(p)).await.unwrap(),
                Message::Binary(b) => {
                    let n = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
                    let h: Value = serde_json::from_slice(&b[4..4 + n]).unwrap();
                    self.send(json!({"type":"frame.ack","frame_id":h["frame_id"],"disposition":"displayed"}))
                        .await;
                }
                m => panic!("unexpected message {m:?}"),
            }
        }
    }
    async fn until(&mut self, found: impl Fn(&Value) -> bool) -> Value {
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < end, "expected message never came");
            let v = self.next().await;
            if found(&v) {
                return v;
            }
        }
    }
    async fn send(&mut self, mut v: Value) -> String {
        self.seq += 1;
        v["seq"] = json!(self.seq.to_string());
        v["connection_epoch"] = self.hello["connection_epoch"].clone();
        self.ws
            .send(Message::Text(v.to_string().into()))
            .await
            .unwrap();
        self.seq.to_string()
    }
    /// Owner root edit; the reply is the accepted state_rev.
    async fn root(&mut self, base: &Value, root: Value) -> Value {
        let view = self.hello["view_id"].clone();
        let seq = self
            .send(json!({"type":"view.set","view_id":view,"base_state_rev":base,"body":{"root":root}}))
            .await;
        let reply = self
            .until(|v| {
                v["seq"] == seq && ["accepted", "error"].contains(&v["type"].as_str().unwrap_or(""))
            })
            .await;
        assert_eq!(reply["type"], "accepted", "{reply}");
        reply["state_rev"].clone()
    }
}
/// The same viewport up to float rounding (an open may size the camera by
/// another path than the fit a root clear or a fork computes).
fn same_box(a: &Value, b: &Value) -> bool {
    let n = |v: &Value| -> Vec<f64> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().parse().unwrap())
            .collect()
    };
    let (a, b) = (n(a), n(b));
    let span = (a[2] - a[0]).abs().max((a[3] - a[1]).abs());
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= span * 1e-9)
}
fn bodies() -> [(&'static str, Value); 4] {
    [
        (
            "in_view",
            json!({"kind":"in_view","cursor":{"check":"0","error":"0"},"waived":null,"limit":64}),
        ),
        (
            "list in_view",
            json!({"kind":"list","check":"0","start":"0","waived":null,"limit":64,"in_view":true,"selection_rev":null}),
        ),
        (
            "filtered_step in_view",
            json!({"kind":"filtered_step","check":"0","backwards":false,"after":null,"cursor":null,"waived":null,"in_view":true,"selection_rev":null}),
        ),
        (
            "focus",
            json!({"kind":"focus","check":"0","error":"0","fit":true,"isolate":false}),
        ),
    ]
}
async fn guest_read(
    h: &Harness,
    id: &str,
    guest: &Login,
    context: (&Value, &Value, &Value),
    body: Value,
) -> (u16, Value) {
    let (view, revision, state_rev) = context;
    let read = json!({"view_id":view,"revision":revision,"state_rev":state_rev,"body":body});
    h.call(guest, "POST", &format!("/api/v1/guest/{id}/drc/read"), read)
        .await
}

#[tokio::test]
#[ignore = "private valmini: FLOE_OWNER_FIXTURE, FLOE_INDEX_BIN, FLOE_RENDERD_BIN"]
async fn guests_never_inherit_a_view_root_and_refuse_viewport_reads_under_one() {
    let fixture = PathBuf::from(std::env::var_os("FLOE_OWNER_FIXTURE").unwrap());
    let dir = fixture.parent().unwrap().join("guest-root");
    fs::create_dir(&dir).unwrap();
    let source = dir.join("design.oas");
    fs::copy(&fixture, &source).unwrap();
    let index = std::env::var_os("FLOE_INDEX_BIN").unwrap();
    let built = std::process::Command::new(&index)
        .arg("vfs")
        .arg(&source)
        .arg(cache::cache_path(&source).unwrap())
        .args(["--jobs", "2"])
        .output()
        .unwrap();
    assert!(built.status.success(), "{built:?}");
    let db = dir.join("review.db");
    fs::write(&db, "TOP 1000\nWIDTH\n3 3 0\np 1 4\n30000 30000\n31000 30000\n31000 31000\n30000 31000\np 2 4\n200000 200000\n201000 200000\n201000 201000\n200000 201000\np 3 4\n380000 420000\n381000 420000\n381000 421000\n380000 421000\n").unwrap();
    let packed = std::process::Command::new(&index)
        .arg("drc")
        .arg(&db)
        .args(["--jobs", "2"])
        .output()
        .unwrap();
    assert!(packed.status.success(), "{packed:?}");
    let h = Harness::start(&source, &db.with_file_name(".review.db.tray")).await;
    h.drc
        .submit(serde_json::from_value(json!({"kind":"rule","check":"0"})).unwrap())
        .unwrap()
        .result()
        .await
        .unwrap();
    let owner = h.login().await;
    let source_id = h.service.catalog()["sources"][0]["source_id"].clone();
    let open = json!({"kind":"open","seq":"1","source_id":source_id,"mode":"level","levels":{"mode":"all"},
        "body":{"pixels":[257,191],"labels":false,"depth":"full","detail":"high","thin":"keep"}});
    assert_eq!(
        h.call(&owner, "POST", "/api/v1/operations", open).await.0,
        202
    );
    let end = Instant::now() + Duration::from_secs(30);
    while h.view(&owner).await["view_id"].is_null() {
        assert!(Instant::now() < end, "open did not publish a view");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut ow = h.peer("/api/v1/events", &owner).await;
    let view_id = ow.hello["view_id"].clone();
    let top = h.view(&owner).await;
    assert_eq!(top["root_name"], "");
    let catalog = h.call(&owner, "GET", "/api/v1/drc", Value::Null).await.1["drc"].clone();
    let revision = catalog["revision"].clone();
    let approval = json!({"id":catalog["id"],"revision":revision,"approve":true});

    // The owner roots a placed child cell: its viewport is now that cell's frame.
    let children = h
        .call(
            &owner,
            "POST",
            &format!("/api/v1/views/{}/cells", view_id.as_str().unwrap()),
            json!({"kind":"children","src":0}),
        )
        .await
        .1;
    let child = children["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["leaf"] == false)
        .expect("a non-leaf child cell")["ci"]
        .clone();
    let rooted_rev = ow
        .root(&top["state_rev"], json!({"src":0,"cell":child}))
        .await;
    let rooted = h.view(&owner).await;
    assert_ne!(rooted["root_name"], "");
    assert_ne!(rooted["bbox_dbu"], top["bbox_dbu"]);
    let (code, refused) = h
        .call(
            &owner,
            "POST",
            &format!("/api/v1/drc/{}/read", catalog["id"].as_str().unwrap()),
            json!({"view_id":view_id,"revision":revision,"state_rev":rooted_rev,"body":bodies()[0].1}),
        )
        .await;
    assert_eq!(
        (code, refused["error"].as_str()),
        (409, Some("drc_view_root"))
    );

    // Follow shows the owner's rooted pixels: share.state flags the root and
    // every viewport read refuses, Focus included (a guest cannot leave the
    // root in the same edit). Reads that do not use the viewport still work.
    let (fid, fl) = h.share(&owner, "follow", &approval).await;
    let mut fw = h.peer(&format!("/api/v1/guest/{fid}/events"), &fl).await;
    let state = fw.until(|v| v["type"] == "share.state").await;
    assert_eq!(state["root"], true, "{state}");
    assert_eq!(state["bbox_dbu"], rooted["bbox_dbu"]);
    let under = (&view_id, &revision, &rooted_rev);
    for (name, body) in bodies() {
        let (code, reply) = guest_read(&h, &fid, &fl, under, body).await;
        assert_eq!(code, 409, "follow {name} under a root: {reply}");
    }
    let plain = json!({"kind":"list","check":"0","start":"0","waived":null,"limit":64,"in_view":false,"selection_rev":null});
    let (code, rows) = guest_read(&h, &fid, &fl, under, plain).await;
    assert_eq!(code, 200, "{rows}");
    assert_eq!(rows["data"]["rows"].as_array().unwrap().len(), 3);
    let selection = format!("/api/v1/guest/{fid}/drc/selection");
    let boxed = json!({"view_id":view_id,"revision":revision,"base_selection_rev":"1","state_rev":rooted_rev,
        "body":{"kind":"apply","check":"0","errors":["0","1","2"],"mode":"replace","bbox_um":["0","0","100","100"],"waived":null}});
    assert_eq!(h.call(&fl, "POST", &selection, boxed).await.0, 409);
    let listed = json!({"view_id":view_id,"revision":revision,"base_selection_rev":"1",
        "body":{"kind":"apply","check":"0","errors":["0"],"mode":"replace","bbox_um":null,"waived":null}});
    let (code, picked) = h.call(&fl, "POST", &selection, listed).await;
    assert_eq!(code, 200, "{picked}");

    // An Explore fork of the rooted owner starts at the top, fitted to the
    // file's die at the owner's pixels, and serves its own viewport reads.
    let (eid, el) = h.share(&owner, "explore", &approval).await;
    let mut ew = h.peer(&format!("/api/v1/guest/{eid}/events"), &el).await;
    let fork = ew.until(|v| v["type"] == "share.state").await;
    assert_eq!(fork["root"], false, "{fork}");
    assert!(
        same_box(&fork["bbox_dbu"], &top["bbox_dbu"]),
        "{fork} {top}"
    );
    assert!(!same_box(&fork["bbox_dbu"], &rooted["bbox_dbu"]));
    let fork_view = ew.hello["view_id"].clone();
    for (name, body) in bodies() {
        let at_top = (&fork_view, &revision, &fork["state_rev"]);
        let (code, reply) = guest_read(&h, &eid, &el, at_top, body).await;
        assert_eq!(code, 200, "explore {name} at the top: {reply}");
    }

    // Back at the top the Follow guest's flag clears and its reads resume.
    let top_rev = ow.root(&rooted_rev, Value::Null).await;
    let state = fw
        .until(|v| v["type"] == "share.state" && v["state_rev"] == top_rev)
        .await;
    assert_eq!(state["root"], false, "{state}");
    // Leaving the root refits the file's die exactly as the fork did.
    assert_eq!(state["bbox_dbu"], fork["bbox_dbu"]);
    for (name, body) in bodies() {
        let (code, reply) = guest_read(&h, &fid, &fl, (&view_id, &revision, &top_rev), body).await;
        assert_eq!(code, 200, "follow {name} at the top: {reply}");
    }
    drop((fw, ew, ow));
    h.shutdown().await;
    println!("RUST GUEST VIEW ROOT: ALL OK (explore forks at the top, follow flag, viewport reads/box refused under a root, resumed at the top)");
}
