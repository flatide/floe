//! Authenticated bounded HTTP/1 + RFC6455 transport. M1b foundation only:
//! no browser-provided path, worker command, upload or render endpoint.
use crate::{
    auth::{Auth, Secret, SessionId},
    origin::{self, Origin},
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Request, State,
    },
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    sync::{watch, Semaphore},
    task::JoinSet,
    time::timeout,
};

pub const PROTOCOL: &str = "floe.v1";
pub const BUNDLE: &str = "m1b-transport-1";
const BODY_BYTES: usize = 16 * 1024;
const CONTROL_BYTES: usize = 8 * 1024;
const CONNECTIONS: u32 = 32;
const SOCKETS: u32 = 8;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
type Gate = Arc<Gateway>;

pub struct Gateway {
    addr: SocketAddr,
    origin: Origin,
    cookie_name: String,
    auth: Mutex<Auth>,
    sockets: Arc<Semaphore>,
    stopping: watch::Sender<bool>,
}
impl Gateway {
    pub fn new(addr: SocketAddr) -> Result<(Gate, Secret), String> {
        let origin = Origin::for_listener(addr)?;
        let (auth, secret) = Auth::new(
            Instant::now(),
            Duration::from_secs(120),
            Duration::from_secs(8 * 3600),
        )
        .map_err(|e| e.to_string())?;
        let (stopping, _) = watch::channel(false);
        Ok((
            Arc::new(Self {
                addr,
                origin,
                cookie_name: format!("floe_session_{}", addr.port()),
                auth: Mutex::new(auth),
                sockets: Arc::new(Semaphore::new(SOCKETS as usize)),
                stopping,
            }),
            secret,
        ))
    }
    pub fn origin(&self) -> &str {
        self.origin.url()
    }
    fn authenticate(&self, headers: &HeaderMap, csrf: &str) -> Result<SessionId, StatusCode> {
        let cookie = origin::cookie(headers, &self.cookie_name).ok_or(StatusCode::UNAUTHORIZED)?;
        self.auth
            .lock()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .authenticate(cookie, csrf, Instant::now())
            .map_err(|_| StatusCode::UNAUTHORIZED)
    }
    fn alive(&self, id: &SessionId) -> bool {
        self.auth.lock().is_ok_and(|a| a.alive(id, Instant::now()))
    }
}
fn error(status: StatusCode) -> Response {
    (status, Json(json!({"error":status.as_u16()}))).into_response()
}
pub fn router(gate: Gate) -> Router {
    Router::new()
        .route("/api/v1/session/exchange", post(exchange))
        .route("/api/v1/session", delete(logout))
        .route("/api/v1/capabilities", get(capabilities))
        .route("/api/v1/events", get(upgrade))
        .fallback(|| async { error(StatusCode::NOT_FOUND) })
        .layer(DefaultBodyLimit::max(BODY_BYTES))
        .layer(middleware::from_fn_with_state(Arc::clone(&gate), guard))
        .with_state(gate)
}
async fn guard(State(gate): State<Gate>, request: Request, next: Next) -> Response {
    let mut response = if *gate.stopping.borrow() {
        error(StatusCode::SERVICE_UNAVAILABLE)
    } else if !gate.origin.host_matches(request.headers())
        || !gate.origin.origin_matches(
            request.headers(),
            !matches!(*request.method(), Method::GET | Method::HEAD),
        )
        || request.uri().scheme().is_some()
        || request.uri().query().is_some()
    {
        error(StatusCode::FORBIDDEN)
    } else {
        timeout(IO_TIMEOUT, next.run(request))
            .await
            .unwrap_or_else(|_| error(StatusCode::REQUEST_TIMEOUT))
    };
    let headers = response.headers_mut();
    for (key, value) in [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        (
            "content-security-policy",
            "default-src 'none'; frame-ancestors 'none'; base-uri 'none'",
        ),
    ] {
        headers.insert(key, HeaderValue::from_static(value));
    }
    response
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exchange {
    bootstrap: String,
    protocol: u32,
    bundle: String,
}
async fn exchange(
    State(gate): State<Gate>,
    body: Result<Json<Exchange>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = match body {
        Ok(Json(b)) => b,
        Err(e) => return error(e.status()),
    };
    if body.protocol != 1 || body.bundle != BUNDLE {
        return error(StatusCode::UPGRADE_REQUIRED);
    }
    let credentials = match gate.auth.lock() {
        Ok(mut a) => match a.exchange(&body.bootstrap, Instant::now()) {
            Ok(c) => c,
            Err(crate::auth::AuthError::Unauthorized) => return error(StatusCode::UNAUTHORIZED),
            Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE),
        },
        Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE),
    };
    let cookie = format!(
        "{}={}; Path=/api/v1; HttpOnly; SameSite=Strict; Max-Age=28800",
        gate.cookie_name,
        credentials.cookie.expose()
    );
    let mut response = Json(
        json!({"protocol":1,"bundle":BUNDLE,"session_id":credentials.id.as_str(),
        "csrf":credentials.csrf.expose()}),
    )
    .into_response();
    // Only random lowercase hex and the server-generated cookie name enter
    // this header. HttpOnly is supplemented with CSRF even for GET and WS.
    response.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&cookie).expect("hex cookie"),
    );
    response
}
fn http_session(gate: &Gateway, headers: &HeaderMap) -> Result<SessionId, StatusCode> {
    gate.authenticate(
        headers,
        origin::single(headers, "x-floe-csrf").ok_or(StatusCode::UNAUTHORIZED)?,
    )
}
async fn capabilities(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = http_session(&gate, &headers) {
        return error(e);
    }
    Json(json!({"protocol":1,"bundle":BUNDLE,"stage":"transport",
        "render":false,"shares":false,"uploads":false,"control_bytes":CONTROL_BYTES}))
    .into_response()
}
async fn logout(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    let id = match http_session(&gate, &headers) {
        Ok(id) => id,
        Err(e) => return error(e),
    };
    match gate.auth.lock() {
        Ok(mut a) => a.revoke(&id),
        Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE),
    }
    let mut r = StatusCode::NO_CONTENT.into_response();
    r.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&format!(
            "{}=; Path=/api/v1; HttpOnly; SameSite=Strict; Max-Age=0",
            gate.cookie_name
        ))
        .unwrap(),
    );
    r
}
async fn upgrade(State(gate): State<Gate>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    if !gate.origin.origin_matches(&headers, true) {
        return error(StatusCode::FORBIDDEN);
    }
    let Some(protocols) = origin::single(&headers, "sec-websocket-protocol") else {
        return error(StatusCode::UPGRADE_REQUIRED);
    };
    let protocols: Vec<_> = protocols.split(',').map(str::trim).collect();
    if protocols.len() != 3
        || protocols.iter().filter(|&&p| p == PROTOCOL).count() != 1
        || !protocols.contains(&format!("bundle.{BUNDLE}").as_str())
    {
        return error(StatusCode::UPGRADE_REQUIRED);
    }
    let Some(csrf) = protocols.iter().find_map(|p| p.strip_prefix("csrf.")) else {
        return error(StatusCode::UNAUTHORIZED);
    };
    let id = match gate.authenticate(&headers, csrf) {
        Ok(id) => id,
        Err(e) => return error(e),
    };
    let permit = match Arc::clone(&gate.sockets).try_acquire_owned() {
        Ok(p) => p,
        Err(_) => return error(StatusCode::TOO_MANY_REQUESTS),
    };
    ws.protocols([PROTOCOL])
        .max_message_size(CONTROL_BYTES)
        .max_frame_size(CONTROL_BYTES)
        .read_buffer_size(CONTROL_BYTES)
        .write_buffer_size(0)
        .max_write_buffer_size(2 * CONTROL_BYTES)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            control_socket(socket, gate, id).await;
        })
}
#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Control {
    #[serde(rename = "ping")]
    Ping { seq: String },
}
async fn control_socket(mut ws: WebSocket, gate: Gate, id: SessionId) {
    let mut stop = gate.stopping.subscribe();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut last = 0u64;
    let mut received = Instant::now();
    let mut window = Instant::now();
    let mut messages = 0u32;
    if !gate.alive(&id) || *stop.borrow() {
        return;
    }
    if !matches!(
        timeout(
            Duration::from_secs(1),
            ws.send(Message::Text(
                json!({"type":"hello","protocol":1,"bundle":BUNDLE,"session_id":id.as_str()})
                    .to_string()
                    .into()
            ))
        )
        .await,
        Ok(Ok(()))
    ) {
        return;
    }
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = tick.tick() => {
                if !gate.alive(&id) || received.elapsed() >= Duration::from_secs(30) {break;}
            }
            incoming = ws.recv() => {
                if !gate.alive(&id) {break;}
                if window.elapsed() >= Duration::from_secs(1) {window=Instant::now();messages=0;}
                messages += 1;
                if messages > 60 {break;}
                received=Instant::now();
                let text = match incoming {Some(Ok(Message::Text(t)))=>t,
                    Some(Ok(Message::Ping(_) | Message::Pong(_)))=>continue,_=>break};
                let Ok(Control::Ping{seq}) = serde_json::from_str(&text) else {break;};
                let Ok(value) = seq.parse::<u64>() else {break;};
                if value <= last || seq != value.to_string() {break;}
                last=value;
                if !matches!(timeout(Duration::from_secs(1),ws.send(Message::Text(json!({"type":"pong","seq":seq}).to_string().into()))).await,Ok(Ok(()))) {break;}
            }
        }
    }
    let _ = timeout(Duration::from_millis(100), ws.send(Message::Close(None))).await;
}
/// Bound connection count, header size/read deadline and handler deadline.
/// WebSocket tasks have a separate slot budget, stop watch and send timeout.
pub async fn serve(
    listener: TcpListener,
    gate: Gate,
    shutdown: impl std::future::Future<Output = ()>,
) -> io::Result<()> {
    if listener.local_addr()? != gate.addr {
        return Err(io::Error::other("listener origin mismatch"));
    }
    let slots = Arc::new(Semaphore::new(CONNECTIONS as usize));
    let app = router(Arc::clone(&gate));
    let mut tasks = JoinSet::new();
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! {
            _ = &mut shutdown => break Ok(()),
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            incoming = listener.accept() => {
                let (stream,_) = match incoming {Ok(pair)=>pair,Err(e)=>break Err(e)};
                let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {drop(stream);continue;};
                let service = TowerToHyperService::new(app.clone());
                tasks.spawn(async move {
                    let _permit = permit;
                    let mut builder = hyper::server::conn::http1::Builder::new();
                    builder.timer(TokioTimer::new()).header_read_timeout(IO_TIMEOUT)
                        .max_headers(64).max_buf_size(16 * 1024);
                    // Also bound unused-body drain/keep-alive, outside the
                    // handler's timeout. Upgraded WS has its own lifecycle.
                    let _ = timeout(Duration::from_secs(10),builder.serve_connection(TokioIo::new(stream),service).with_upgrades()).await;
                });
            }
        }
    };
    gate.stopping.send_replace(true);
    drop(listener);
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    let drained = timeout(
        Duration::from_secs(2),
        Arc::clone(&gate.sockets).acquire_many_owned(SOCKETS),
    )
    .await;
    if drained.is_err() {
        return Err(io::Error::other("WebSocket shutdown deadline exceeded"));
    }
    result
}
