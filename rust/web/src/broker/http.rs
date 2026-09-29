//! Session-scoped control/render protocol; never mounted into standalone.
use super::*;
use axum::{
    extract::{ws::WebSocketUpgrade, DefaultBodyLimit, Path as RoutePath, Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use serde_json::json;
use std::io;
use tokio::{net::TcpListener, task::JoinSet};

#[derive(Clone)]
struct Host {
    broker: Arc<Broker>,
    runtime: Option<Arc<runtime::Runtime>>,
    streams: Arc<stream::Transport>,
}
fn router(host: Host) -> Router {
    Router::new()
        .route("/demo", get(demo_page))
        .route("/api/v1/demo/samples", get(demo_samples))
        .route("/api/v1/demo/launches", post(demo_launch))
        .route("/server/{id}", get(server_page))
        .route("/server-assets/{bundle}/{name}", get(server_asset))
        .route("/api/v1/server/launches", post(launch))
        .route("/api/v1/server/launches/{id}", delete(revoke))
        .route("/api/v1/server/sessions/{id}/exchange", post(exchange))
        .route("/api/v1/server/sessions/{id}", get(inspect).delete(logout))
        .route(
            "/api/v1/server/sessions/{id}/view",
            get(view_state).post(open_view),
        )
        .route("/api/v1/server/sessions/{id}/stream", get(upgrade))
        .layer(DefaultBodyLimit::max(4096))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&host.broker),
            boundary,
        ))
        .with_state(host)
}
async fn demo_page(State(host): State<Host>) -> Response {
    if !host.broker.is_public_demo() {
        return StatusCode::NOT_FOUND.into_response();
    }
    super::assets::demo()
}
async fn demo_samples(State(host): State<Host>) -> Response {
    match host.broker.demo_samples() {
        Ok(samples) => Json(json!({"samples": samples})).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DemoLaunch {
    sample_id: String,
}
async fn demo_launch(
    State(host): State<Host>,
    headers: HeaderMap,
    body: Input<DemoLaunch>,
) -> Response {
    if !host.broker.is_public_demo() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(Json(body)) = body else {
        return error(Error::Invalid);
    };
    let b = host.broker;
    let Ok(permit) = Arc::clone(&b.lookups).try_acquire_owned() else {
        return error(Error::Busy);
    };
    let viewer_ready = host.runtime.is_some();
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        b.launch_demo(&headers, body.sample_id, Instant::now())
    })
    .await
    {
        Ok(Ok(launch)) => (
            StatusCode::CREATED,
            Json(json!({"launch_id":launch.id,
            "bootstrap":launch.bootstrap.expose(), "viewer_ready":viewer_ready})),
        )
            .into_response(),
        Ok(Err(e)) => error(e),
        Err(_) => error(Error::Unavailable),
    }
}
async fn server_page(RoutePath(id): RoutePath<String>) -> Response {
    super::assets::page(&id)
}
async fn server_asset(RoutePath((bundle, name)): RoutePath<(String, String)>) -> Response {
    super::assets::asset(&bundle, &name)
}
fn error(error: Error) -> Response {
    let (status, code) = match error {
        Error::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
        Error::Invalid => (StatusCode::BAD_REQUEST, "invalid_request"),
        Error::Busy => (StatusCode::TOO_MANY_REQUESTS, "capacity"),
        Error::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    };
    (status, Json(json!({"error":code}))).into_response()
}
async fn boundary(State(b): State<Arc<Broker>>, req: Request, next: Next) -> Response {
    let allowed = b.boundary(req.headers(), false).is_ok() && req.uri().query().is_none();
    // Authenticate the backchannel BEFORE extracting a body or touching paths.
    let launch_route = req.uri().path() == "/api/v1/server/launches"
        || req.uri().path().starts_with("/api/v1/server/launches/");
    let unauthorized = if launch_route {
        b.delegation(req.headers()).is_err()
    } else {
        !matches!(*req.method(), Method::GET | Method::HEAD)
            && !b.origin.origin_matches(req.headers(), true)
    };
    let mut response = if !allowed {
        (StatusCode::FORBIDDEN, Json(json!({"error":"forbidden"}))).into_response()
    } else if unauthorized {
        error(Error::Unauthorized)
    } else {
        next.run(req).await
    };
    for (name, value) in [
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    let csp = format!("default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' blob:; connect-src 'self' {}; frame-ancestors 'none'; base-uri 'none'; form-action 'none'", b.origin.websocket_url());
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_str(&csp).expect("fixed canonical origin"),
    );
    // Hyper's global keep_alive(false) also replaces Upgrade's Connection
    // header. Close ordinary responses explicitly, preserving the WS 101.
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        response
            .headers_mut()
            .insert("connection", HeaderValue::from_static("close"));
    }
    response
}
type Input<T> = std::result::Result<Json<T>, axum::extract::rejection::JsonRejection>;
async fn launch(
    State(host): State<Host>,
    headers: HeaderMap,
    body: Input<LaunchRequest>,
) -> Response {
    let b = host.broker;
    let Ok(Json(body)) = body else {
        return error(Error::Invalid);
    };
    let Ok(permit) = Arc::clone(&b.lookups).try_acquire_owned() else {
        return error(Error::Busy);
    };
    let ttl = b.lifetimes.bootstrap.as_secs();
    let viewer_ready = host.runtime.is_some();
    // Disconnect/HTTP timeout does not spawn unbounded abandoned NFS lookups.
    // The permit remains with the blocking task until its metadata calls end.
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        b.launch(&headers, body, Instant::now())
    })
    .await
    {
        Ok(Ok(launch)) => (
            StatusCode::CREATED,
            Json(json!({"launch_id":launch.id,
            "bootstrap":launch.bootstrap.expose(),"expires_in":ttl,"viewer_ready":viewer_ready})),
        )
            .into_response(),
        Ok(Err(e)) => error(e),
        Err(_) => error(Error::Unavailable),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exchange {
    bootstrap: String,
}
async fn exchange(
    State(host): State<Host>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
    body: Input<Exchange>,
) -> Response {
    let b = host.broker;
    let Ok(Json(body)) = body else {
        return error(Error::Invalid);
    };
    match b.exchange(&id, &body.bootstrap, &headers, Instant::now()) {
        Ok(credentials) => {
            let mut response =
                Json(json!({"launch_id":id,"csrf":credentials.csrf.expose(),"viewer_ready":host.runtime.is_some(),
                    "render_transport":host.runtime.is_some(),"protocol":stream::PROTOCOL,"bundle":crate::transport::BUNDLE}))
                    .into_response();
            let cookie = format!(
                "{}={}; Path=/api/v1/server/sessions/{id}; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
                cookie_name(&id).expect("issued ID"),
                credentials.cookie.expose(),
                b.lifetimes.session.as_secs()
            );
            response.headers_mut().insert(
                "set-cookie",
                HeaderValue::from_str(&cookie).expect("fixed hex credentials"),
            );
            response
        }
        Err(e) => error(e),
    }
}
async fn inspect(
    State(host): State<Host>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    let b = host.broker;
    match b.authorize(&id, &headers, Instant::now()) {
        Ok(access) => Json(json!({"launch_id":access.id(),"viewer_ready":host.runtime.is_some(),"render_transport":host.runtime.is_some(),"public_demo":b.is_public_demo(),
            "principal":{"namespace":access.binding().principal().namespace(),"subject":access.binding().principal().subject()}})).into_response(),
        Err(e) => error(e),
    }
}
async fn logout(
    State(host): State<Host>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    let b = host.broker;
    match b.logout(&id, &headers, Instant::now()) {
        Ok(()) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            response.headers_mut().insert(
                "set-cookie",
                HeaderValue::from_str(&format!(
                    "{}=; Path=/api/v1/server/sessions/{id}; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
                    cookie_name(&id).expect("authenticated ID")
                ))
                .unwrap(),
            );
            response
        }
        Err(e) => error(e),
    }
}
async fn revoke(
    State(host): State<Host>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    let b = host.broker;
    match b.revoke(&id, &headers, Instant::now()) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenView {
    width: u32,
    height: u32,
}
async fn open_view(
    State(host): State<Host>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
    body: Input<OpenView>,
) -> Response {
    let access = match host.broker.authorize(&id, &headers, Instant::now()) {
        Ok(a) => a,
        Err(e) => return error(e),
    };
    let Some(runtime) = host.runtime else {
        return error(Error::Unavailable);
    };
    let Ok(Json(body)) = body else {
        return error(Error::Invalid);
    };
    match runtime.open(&access, body.width, body.height) {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({"status":"opening","view_id":id})),
        )
            .into_response(),
        Err(e) => error(e),
    }
}
async fn view_state(
    State(host): State<Host>,
    RoutePath(id): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    let access = match host.broker.authorize(&id, &headers, Instant::now()) {
        Ok(a) => a,
        Err(e) => return error(e),
    };
    let Some(runtime) = host.runtime else {
        return error(Error::Unavailable);
    };
    match stream::state(&runtime, &access, "") {
        Ok(v) => Json(v).into_response(),
        Err(Error::Invalid) => {
            Json(json!({"type":"unopened","view_id":access.id()})).into_response()
        }
        Err(e) => error(e),
    }
}
async fn upgrade(
    State(host): State<Host>,
    RoutePath(id): RoutePath<String>,
    mut headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !host.broker.origin.origin_matches(&headers, true) || headers.contains_key("x-floe-csrf") {
        return error(Error::Unauthorized);
    }
    let Some(csrf) = stream::csrf(&headers) else {
        return error(Error::Unauthorized);
    };
    headers.insert(
        "x-floe-csrf",
        HeaderValue::from_str(&csrf).expect("hex CSRF"),
    );
    let access = match host.broker.authorize(&id, &headers, Instant::now()) {
        Ok(a) => a,
        Err(e) => return error(e),
    };
    let Some(runtime) = host.runtime else {
        return error(Error::Unavailable);
    };
    if let Err(e) = runtime.snapshot(&access) {
        return error(e);
    }
    let slot = match host.streams.claim(&id) {
        Ok(s) => s,
        Err(e) => return error(e),
    };
    ws.protocols([stream::PROTOCOL])
        .max_message_size(8192)
        .max_frame_size(8192)
        .read_buffer_size(8192)
        .write_buffer_size(0)
        .max_write_buffer_size(crate::view::PACKET_BYTES + 16384)
        .on_upgrade(move |ws| stream::socket(ws, runtime, access, host.streams, slot))
}

/// Synthetic/local integration entry point pending a configured service runtime.
/// Fixed loopback address, bounded connections/body/header/total request time;
/// no public bind, CORS or file API. Static UI is available but render routes
/// return unavailable when
/// this control-only entry point is used; serve_runtime attaches the workers.
pub async fn serve(
    listener: TcpListener,
    broker: Arc<Broker>,
    shutdown: impl std::future::Future<Output = ()>,
) -> io::Result<()> {
    serve_inner(listener, broker, None, shutdown).await
}
/// Local/native integration with graceful reap. This does not configure public
/// TLS, keys, an Electron launcher or any indexing/write API.
pub async fn serve_runtime(
    listener: TcpListener,
    runtime: runtime::Runtime,
    shutdown: impl std::future::Future<Output = ()>,
) -> io::Result<()> {
    let mut guard = RuntimeGuard(Some(Arc::new(runtime)));
    let runtime = Arc::clone(guard.0.as_ref().unwrap());
    let result = serve_inner(
        listener,
        Arc::clone(&runtime.broker),
        Some(runtime),
        shutdown,
    )
    .await;
    let runtime = guard.0.take().unwrap();
    runtime.request_stop();
    let drained = tokio::task::spawn_blocking(move || runtime.drain()).await;
    if !matches!(drained, Ok(Ok(()))) {
        return Err(io::Error::other("broker renderer cleanup failed"));
    }
    result
}
struct RuntimeGuard(Option<Arc<runtime::Runtime>>);
impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.request_stop();
            // Cancellation of the server future must not orphan children or
            // make the async reactor join an NFS-bound preparation thread.
            tokio::task::spawn_blocking(move || runtime.drain());
        }
    }
}
async fn serve_inner(
    listener: TcpListener,
    broker: Arc<Broker>,
    runtime: Option<Arc<runtime::Runtime>>,
    shutdown: impl std::future::Future<Output = ()>,
) -> io::Result<()> {
    if listener.local_addr()? != broker.addr || !broker.addr.ip().is_loopback() {
        return Err(io::Error::other("broker listener mismatch"));
    }
    let host = Host {
        broker: Arc::clone(&broker),
        runtime: runtime.clone(),
        streams: Arc::new(stream::Transport::default()),
    };
    let streams = Arc::clone(&host.streams);
    let app = router(host);
    let slots = Arc::new(Semaphore::new(16));
    let mut tasks = JoinSet::new();
    let mut maintenance = tokio::time::interval(Duration::from_millis(250));
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! {
            _ = &mut shutdown => break Ok(()),
            _ = maintenance.tick() => {
                let check=if let Some(r)=&runtime {r.maintain()}else{broker.maintain(Instant::now())};
                if check.is_err() { break Err(io::Error::other("broker unavailable")); }
            },
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            incoming = listener.accept() => {
                let (stream, _) = match incoming { Ok(pair) => pair, Err(e) => break Err(e) };
                let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else { drop(stream); continue; };
                let service = TowerToHyperService::new(app.clone());
                tasks.spawn(async move {
                    let _permit = permit;
                    let mut builder = hyper::server::conn::http1::Builder::new();
                    builder.timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(5))
                        .max_headers(32).max_buf_size(16 * 1024);
                    let _ = tokio::time::timeout(Duration::from_secs(10), builder.serve_connection(TokioIo::new(stream), service).with_upgrades()).await;
                });
            }
        }
    };
    let stopped = broker.stop();
    drop(listener);
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    if stopped.is_err() {
        return Err(io::Error::other("broker shutdown failed"));
    }
    streams
        .drain()
        .await
        .map_err(|_| io::Error::other("broker stream cleanup failed"))?;
    // A caller that attaches native workers must retain their runner handles
    // and reap them. Dropping the broker does not pretend they have exited.
    if runtime.is_none()
        && broker
            .pending_workers()
            .map_err(|_| io::Error::other("broker shutdown failed"))?
            != 0
    {
        return Err(io::Error::other("broker workers still require reap"));
    }
    result
}
