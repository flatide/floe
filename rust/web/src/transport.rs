//! Authenticated bounded HTTP/1 + RFC6455 transport. M1b foundation only:
//! no browser-provided path, native command, layout upload or arbitrary file
//! endpoint. Owner settings text has a separate bounded import route.
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
use floe_app_core::view::ViewController;
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
pub const BUNDLE: &str = env!("FLOE_WEB_BUNDLE");
const BODY_BYTES: usize = 16 * 1024;
const CONTROL_BYTES: usize = 8 * 1024;
const CONNECTIONS: u32 = 32;
const SOCKETS: u32 = 8;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) type Gate = Arc<Gateway>;

pub(crate) struct Attachment {
    pub id: String,
    pub title: String,
    pub controller: Arc<ViewController>,
    pub rows: crate::layer_catalog::LayerCatalog,
    pub source_id: String,
    pub mode: &'static str,
    pub levels: Option<Vec<String>>,
    pub mode_memory: floe_app_core::view::deck_mode::DeckModeMemory,
    pub drc_panel: Mutex<crate::drc::panel::Panel>,
    pub prepared: Mutex<crate::prepared::PreparedEdits>,
    pub clips: Mutex<crate::exports::Drafts>,
    activity: Mutex<Activity>,
}
impl Attachment {
    pub(crate) fn new(
        controller: Arc<ViewController>,
        title: &str,
    ) -> Result<Self, crate::auth::AuthError> {
        let rows = crate::layer_catalog::LayerCatalog::model(&controller.model);
        Self::with_rows(controller, title, rows)
    }
    pub(crate) fn with_rows(
        controller: Arc<ViewController>,
        title: &str,
        rows: crate::layer_catalog::LayerCatalog,
    ) -> Result<Self, crate::auth::AuthError> {
        Ok(Self {
            id: crate::auth::public_id()?,
            title: title.chars().take(256).collect(),
            rows,
            controller,
            source_id: String::new(),
            mode: "level",
            levels: None,
            mode_memory: Default::default(),
            drc_panel: Mutex::new(crate::drc::panel::Panel::default()),
            prepared: Mutex::new(crate::prepared::PreparedEdits::default()),
            clips: Mutex::new(crate::exports::Drafts::default()),
            activity: Mutex::new(Activity {
                connected: 0,
                seen: false,
                since: Instant::now(),
            }),
        })
    }
    pub(crate) fn subscribe(self: &Arc<Self>) -> Subscriber {
        let mut a = self.activity.lock().unwrap();
        a.connected += 1;
        a.seen = true;
        Subscriber(Arc::clone(self))
    }
}
struct Activity {
    connected: usize,
    seen: bool,
    since: Instant,
}
impl Activity {
    fn expired(&self, now: Instant) -> bool {
        self.connected == 0
            && now.saturating_duration_since(self.since)
                >= Duration::from_secs(if self.seen { 60 } else { 120 })
    }
}
pub(crate) struct Subscriber(Arc<Attachment>);
impl Drop for Subscriber {
    fn drop(&mut self) {
        let mut a = self.0.activity.lock().unwrap();
        a.connected -= 1;
        if a.connected == 0 {
            a.since = Instant::now();
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct TransportUsage {
    pub reserved_output_bytes: usize,
    pub encoders: usize,
    pub sockets: usize,
    pub guest_reserved_output_bytes: usize,
    pub guest_encoders: usize,
    pub guest_sockets: usize,
}

pub struct Gateway {
    addr: SocketAddr,
    origin: Origin,
    cookie_name: String,
    auth: Mutex<Auth>,
    pub(crate) shares: Option<Mutex<crate::sharing::Shares>>,
    pub(crate) share_transport: Option<crate::sharing::Transport>,
    sockets: Arc<Semaphore>,
    pub(crate) stopping: watch::Sender<bool>,
    pub(crate) view: Option<Arc<Attachment>>,
    pub(crate) service: Option<Arc<crate::service::Service>>,
    pub(crate) launch: Option<Arc<crate::launch::Launches>>,
    cli_owner: bool,
    pub(crate) browse: Option<Arc<crate::browse::Picker>>,
    pub(crate) drc: Option<Arc<crate::drc::Registry>>,
    pub(crate) defaults: Option<Arc<crate::defaults::Service>>,
    pub(crate) fill_slot_edit: bool,
    pub(crate) display_only: bool,
    dump_on_start: bool,
    pub(crate) display_input: Option<crate::display_test::Input>,
    startup: Option<serde_json::Value>,
    startup_confirm_levels: bool,
    pub(crate) build: Option<crate::about::BuildInfo>,
    pub(crate) notices: crate::about::Notices,
    pub(crate) notice_readers: Arc<Semaphore>,
    pub(crate) output_bytes: Arc<Semaphore>,
    pub(crate) encoders: Arc<Semaphore>,
    pub(crate) settings_ops: Arc<Semaphore>,
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
                shares: None,
                share_transport: None,
                sockets: Arc::new(Semaphore::new(SOCKETS as usize)),
                stopping,
                view: None,
                service: None,
                launch: None,
                cli_owner: false,
                browse: None,
                drc: None,
                defaults: None,
                fill_slot_edit: false,
                display_only: false,
                dump_on_start: false,
                display_input: None,
                startup: None,
                startup_confirm_levels: false,
                build: None,
                notices: crate::about::Notices::NotPackaged,
                notice_readers: Arc::new(Semaphore::new(1)),
                output_bytes: Arc::new(Semaphore::new(crate::view::OUTPUT_BUDGET)),
                encoders: Arc::new(Semaphore::new(2)),
                settings_ops: Arc::new(Semaphore::new(1)),
            }),
            secret,
        ))
    }
    pub fn origin(&self) -> &str {
        self.origin.url()
    }
    /// A standalone diagnostic session has no registered sources or native workers.
    pub fn with_display_test(addr: SocketAddr) -> Result<(Gate, Secret), String> {
        let (mut gate, secret) = Self::new(addr)?;
        Arc::get_mut(&mut gate).expect("new gateway").display_only = true;
        Ok((gate, secret))
    }
    /// A trusted CLI has already bounded, read and validated this one PNG.
    pub fn with_display_input(
        addr: SocketAddr,
        png: floe_app_core::annotations::png::DisplayPng,
    ) -> Result<(Gate, Secret), String> {
        let (mut gate, secret) = Self::with_display_test(addr)?;
        Arc::get_mut(&mut gate).expect("new gateway").display_input = Some(png.into());
        Ok((gate, secret))
    }
    /// Compile-time identity from the launcher, not a live native-tool audit.
    pub fn attach_build(gate: &mut Gate, info: crate::about::BuildInfo) -> Result<(), String> {
        let g = Arc::get_mut(gate).ok_or("gateway already published")?;
        if g.build.is_some() {
            return Err("build identity already attached".into());
        }
        g.build = Some(info);
        Ok(())
    }
    pub fn attach_notices(gate: &mut Gate, notices: crate::about::Notices) -> Result<(), String> {
        Arc::get_mut(gate)
            .ok_or("gateway already published")?
            .notices = notices;
        Ok(())
    }
    /// A trusted local launcher supplies the controller, never an HTTP path.
    /// One pre-registered owner view in this slice; creation/catalog is separate.
    pub fn with_view(
        addr: SocketAddr,
        controller: Arc<ViewController>,
        title: &str,
    ) -> Result<(Gate, Secret), String> {
        let (mut gate, secret) = Self::new(addr)?;
        Arc::get_mut(&mut gate).expect("new gateway").view = Some(Arc::new(
            Attachment::new(controller, title).map_err(|e| e.to_string())?,
        ));
        Ok((gate, secret))
    }
    pub fn with_service(
        addr: SocketAddr,
        service: Arc<crate::service::Service>,
    ) -> Result<(Gate, Secret), String> {
        let (mut gate, secret) = Self::new(addr)?;
        Arc::get_mut(&mut gate).expect("new gateway").service = Some(service);
        Ok((gate, secret))
    }
    /// Trusted local launcher only. HTTP receives opaque proposals, not paths.
    pub fn attach_launches(
        gate: &mut Gate,
        launches: Arc<crate::launch::Launches>,
    ) -> Result<(), String> {
        let g = Arc::get_mut(gate).ok_or("gateway already published")?;
        if g.service.is_none() || g.launch.is_some() {
            return Err("launch proposals require an owner service and one broker".into());
        }
        g.launch = Some(launches);
        g.cli_owner = true;
        Ok(())
    }
    /// Approved server roots only. Independent workspaces get the same safe
    /// open-proposal broker, but do not acquire a CLI single-instance endpoint.
    pub fn enable_browse(
        gate: &mut Gate,
        roots: &[std::path::PathBuf],
        resources: &Arc<floe_app_core::managed::Resources>,
    ) -> Result<(), String> {
        let g = Arc::get_mut(gate).ok_or("gateway already published")?;
        if g.browse.is_some() {
            return Err("file picker already attached".into());
        }
        let service = Arc::clone(
            g.service
                .as_ref()
                .ok_or("picker requires an owner service")?,
        );
        let launch = g
            .launch
            .clone()
            .unwrap_or_else(crate::launch::Launches::new);
        let drc = match &g.drc {
            Some(drc) => Arc::clone(drc),
            None => {
                crate::drc::Registry::empty(service.drc_resources().1).map_err(|e| e.to_string())?
            }
        };
        let picker = crate::browse::Picker::start_with_drc(
            roots,
            resources,
            service,
            Arc::clone(&launch),
            Some(Arc::clone(&drc)),
        )
        .map_err(|e| e.to_string())?;
        g.drc = Some(drc);
        g.launch = Some(launch);
        g.browse = Some(picker);
        Ok(())
    }
    pub(crate) fn active_view(&self) -> Option<Arc<Attachment>> {
        self.service
            .as_ref()
            .and_then(|s| s.current())
            .or_else(|| self.view.clone())
    }
    /// Only a local launcher supplies startup preferences. The browser adds its
    /// measured device dimensions before the FIRST open, not a second render.
    pub fn with_startup(
        addr: SocketAddr,
        service: Arc<crate::service::Service>,
        request: serde_json::Value,
    ) -> Result<(Gate, Secret), String> {
        Self::with_startup_options(addr, service, request, false)
    }
    pub fn with_startup_options(
        addr: SocketAddr,
        service: Arc<crate::service::Service>,
        request: serde_json::Value,
        confirm_levels: bool,
    ) -> Result<(Gate, Secret), String> {
        if request.to_string().len() > BODY_BYTES {
            return Err("startup request limit".into());
        }
        let parsed: crate::service::OperationDto =
            serde_json::from_value(request.clone()).map_err(|_| "invalid startup request")?;
        match parsed {
            crate::service::OperationDto::Open {
                seq,
                source_id,
                body,
                ..
            } => {
                if seq != "1"
                    || !service.catalog()["sources"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|r| r["source_id"] == source_id)
                {
                    return Err("invalid startup source/sequence".into());
                }
                body.core().map_err(str::to_owned)?;
            }
            _ => return Err("startup must be an open, never an index".into()),
        }
        let (mut gate, secret) = Self::with_service(addr, service)?;
        let gateway = Arc::get_mut(&mut gate).expect("new gateway");
        gateway.startup = Some(request);
        gateway.startup_confirm_levels = confirm_levels;
        Ok((gate, secret))
    }
    fn stop_services(&self) {
        if let Some(shares) = &self.shares {
            shares.lock().unwrap().stop();
        }
        if let Some(browse) = &self.browse {
            browse.request_stop();
        }
        if let Some(launch) = &self.launch {
            launch.stop();
        }
        if let Some(defaults) = &self.defaults {
            defaults.request_stop();
        }
        if let Some(drc) = &self.drc {
            drc.request_stop();
        }
        if let Some(service) = &self.service {
            service.request_stop();
        }
        if let Some(view) = &self.view {
            view.controller.request_close();
        }
    }
    pub fn transport_usage(&self) -> TransportUsage {
        TransportUsage {
            reserved_output_bytes: crate::view::OUTPUT_BUDGET
                - self.output_bytes.available_permits(),
            encoders: 2 - self.encoders.available_permits(),
            sockets: SOCKETS as usize - self.sockets.available_permits(),
            guest_reserved_output_bytes: self.share_transport.as_ref().map_or(0, |t| {
                crate::sharing::OUTPUT_BYTES - t.bytes.available_permits()
            }),
            guest_encoders: self
                .share_transport
                .as_ref()
                .map_or(0, |t| 1 - t.encoders.available_permits()),
            guest_sockets: self.share_transport.as_ref().map_or(0, |t| {
                crate::sharing::SOCKETS - t.sockets.available_permits()
            }),
        }
    }
    /// Attach a locally authorized read-only pack before publishing the gateway.
    /// Its source binding must be one of the owner's registered sources.
    pub fn attach_drc(gate: &mut Gate, drc: Arc<crate::drc::Service>) -> Result<(), String> {
        Self::attach_drc_registry(gate, crate::drc::Registry::read_only(drc))
    }
    /// Local launcher opts the owner into explicit builds of this registered
    /// source only. Read-only registrations and future shares do not gain writes.
    pub fn attach_drc_registry(
        gate: &mut Gate,
        drc: Arc<crate::drc::Registry>,
    ) -> Result<(), String> {
        let gate = Arc::get_mut(gate).ok_or("gateway already published")?;
        if gate.drc.is_some()
            || gate.defaults.is_some()
            || !gate.service.as_ref().is_some_and(|s| {
                s.catalog()["sources"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|r| r["source_id"] == drc.source_id()))
            })
        {
            return Err("invalid DRC source registration".into());
        }
        drc.protect_publications(&gate.service.as_ref().unwrap().source_set())
            .map_err(|e| e.to_string())?;
        gate.drc = Some(drc);
        Ok(())
    }
    /// Trusted local opt-in for grants and follow/independent native views.
    /// Guest UI is a separate stage; scoped queries/DRC never share owner proofs.
    pub fn enable_local_sharing(gate: &mut Gate) -> Result<(), String> {
        let g = Arc::get_mut(gate).ok_or("gateway already published")?;
        if g.shares.is_some() || g.display_only || (g.service.is_none() && g.view.is_none()) {
            return Err("local sharing requires a view workspace and one opt-in".into());
        }
        g.shares = Some(Mutex::new(crate::sharing::Shares::default()));
        g.share_transport = Some(crate::sharing::Transport::default());
        Ok(())
    }
    /// Memory-only developer tool. This grants no filesystem publication,
    /// source access or reviewer permissions, unlike other launcher opt-ins.
    pub fn enable_fill_slot_edit(gate: &mut Gate) -> Result<(), String> {
        Arc::get_mut(gate)
            .ok_or("gateway already published")?
            .fill_slot_edit = true;
        Ok(())
    }
    /// Browser-local diagnostic pixels only; no file API or server dump writer.
    pub fn enable_display_dump(gate: &mut Gate) -> Result<(), String> {
        let g = Arc::get_mut(gate).ok_or("gateway already published")?;
        if g.service.is_none() || g.display_only {
            return Err("display dump requires a view workspace".into());
        }
        g.dump_on_start = true;
        Ok(())
    }
    /// Explicit trusted-launcher opt-in, after all DRC/input registrations and
    /// before publishing the gateway. Extra paths protect launcher-owned files
    /// (notably the private session credential); they never grant write paths.
    pub fn enable_design_defaults(
        gate: &mut Gate,
        files: &[std::path::PathBuf],
        trees: &[std::path::PathBuf],
    ) -> Result<(), String> {
        let g = Arc::get_mut(gate).ok_or("gateway already published")?;
        if g.defaults.is_some() {
            return Err("design defaults already enabled".into());
        }
        let service = g
            .service
            .as_ref()
            .ok_or("design defaults require registered sources")?;
        let mut files = files.to_vec();
        let mut trees = trees.to_vec();
        if let Some(drc) = &g.drc {
            let (df, dt) = drc.protected_paths().map_err(|e| e.to_string())?;
            files.extend(df);
            trees.extend(dt);
        }
        let publisher = floe_app_core::layer_defaults::Publisher::with_sources(
            service.source_set(),
            files,
            trees,
        )
        .map_err(|e| e.to_string())?;
        g.defaults = Some(crate::defaults::Service::start(publisher).map_err(|e| e.to_string())?);
        Ok(())
    }
    /// Trusted launcher chooses the owner tag. No browser-supplied reviewer or
    /// output path; enable before defaults so those writers protect these names.
    pub fn enable_drc_notes(
        gate: &mut Gate,
        reviewer: &str,
        files: &[std::path::PathBuf],
        trees: &[std::path::PathBuf],
    ) -> Result<(), String> {
        Self::enable_drc_review(gate, reviewer, files, trees, false)
    }
    /// Separate trusted opt-in for waive publication, never implied by a
    /// read-sidecar registration or the existing notes-only setting.
    pub fn enable_drc_review(
        gate: &mut Gate,
        reviewer: &str,
        files: &[std::path::PathBuf],
        trees: &[std::path::PathBuf],
        edit_waives: bool,
    ) -> Result<(), String> {
        Self::register_drc_review(gate, reviewer, files, trees, edit_waives, true)
    }
    /// Display existing notes without granting editor/transfer/publication APIs.
    /// The trusted CLI separately registers that reviewer's read-only waives.
    pub fn enable_drc_readonly_review(
        gate: &mut Gate,
        reviewer: &str,
        files: &[std::path::PathBuf],
        trees: &[std::path::PathBuf],
    ) -> Result<(), String> {
        Self::register_drc_review(gate, reviewer, files, trees, false, false)
    }
    fn register_drc_review(
        gate: &mut Gate,
        reviewer: &str,
        files: &[std::path::PathBuf],
        trees: &[std::path::PathBuf],
        edit_waives: bool,
        editable: bool,
    ) -> Result<(), String> {
        let g = Arc::get_mut(gate).ok_or("gateway already published")?;
        if g.defaults.is_some() {
            return Err("register notes before design defaults".into());
        }
        let drc = g
            .drc
            .as_ref()
            .ok_or("note review requires registered DRC")?;
        let sources = g
            .service
            .as_ref()
            .ok_or("note review requires registered sources")?
            .source_set();
        drc.enable_notes(
            reviewer,
            Arc::clone(&sources),
            files,
            trees,
            edit_waives,
            editable,
        )
        .map_err(|e| e.to_string())?;
        drc.protect_publications(&sources)
            .map_err(|e| e.to_string())
    }
    fn authenticate(&self, headers: &HeaderMap, csrf: &str) -> Result<SessionId, StatusCode> {
        let cookie = origin::cookie(headers, &self.cookie_name).ok_or(StatusCode::UNAUTHORIZED)?;
        self.auth
            .lock()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .authenticate(cookie, csrf, Instant::now())
            .map_err(|_| StatusCode::UNAUTHORIZED)
    }
    pub(crate) fn alive(&self, id: &SessionId) -> bool {
        self.auth.lock().is_ok_and(|a| a.alive(id, Instant::now()))
    }
}
pub(crate) fn error(status: StatusCode) -> Response {
    (status, Json(json!({"error":status.as_u16()}))).into_response()
}
pub fn router(gate: Gate) -> Router {
    Router::new()
        .route("/api/v1/session/exchange", post(exchange))
        .route("/api/v1/session", delete(logout))
        .route("/api/v1/capabilities", get(capabilities))
        .route("/api/v1/about", get(crate::about::read))
        .route(
            "/api/v1/display-test/{format}",
            get(crate::display_test::read),
        )
        .route("/api/v1/palette/presets", get(crate::presets::read))
        .route(
            "/api/v1/views/{id}/fill-slots/{key}",
            get(crate::fill_slots::read),
        )
        .route("/api/v1/about/notices/{start}", get(crate::about::list))
        .route("/api/v1/about/notices/{id}/{page}", get(crate::about::page))
        .route("/api/v1/events", get(upgrade))
        .route("/api/v1/view", get(current_view))
        .route("/api/v1/startup", get(startup))
        .merge(crate::owner::routes())
        .merge(crate::launch::routes())
        .merge(crate::browse::routes())
        .merge(crate::drc::routes())
        .merge(crate::exports::routes())
        .merge(crate::defaults::routes())
        .merge(crate::sharing::routes())
        .merge(crate::assets::routes())
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
        // Bound even bodies an endpoint would ignore. Without the old
        // connection-wide timeout, trickling an unused GET/denied POST body
        // must not hold an HTTP slot indefinitely. Ordinary bodies are 16KiB;
        // authenticated settings (4MiB) and note previews (1MiB) share one
        // large-body/preparation slot; note native work retains it on disconnect.
        // Response bodies (downloads) remain under the idle deadline.
        let gate = Arc::clone(&gate);
        timeout(IO_TIMEOUT, async move {
            let settings_body = crate::settings::is_import(request.method(), request.uri().path());
            let review_body =
                crate::drc::review::is_large_body(request.method(), request.uri().path());
            let permit = if settings_body || review_body {
                if http_session(&gate, request.headers()).is_err() {
                    return error(StatusCode::UNAUTHORIZED);
                }
                match Arc::clone(&gate.settings_ops).try_acquire_owned() {
                    Ok(p) => Some(p),
                    Err(_) => return error(StatusCode::TOO_MANY_REQUESTS),
                }
            } else {
                None
            };
            let limit = if settings_body {
                floe_app_core::layerprops::MAX_BYTES
            } else if review_body {
                crate::drc::RESPONSE_BYTES
            } else {
                BODY_BYTES
            };
            let (parts, body) = request.into_parts();
            match axum::body::to_bytes(body, limit).await {
                Ok(bytes) => {
                    let mut request = Request::from_parts(parts, bytes.into());
                    if let Some(permit) = permit {
                        request.extensions_mut().insert(Arc::new(permit));
                    }
                    next.run(request).await
                }
                Err(_) => error(StatusCode::PAYLOAD_TOO_LARGE),
            }
        })
        .await
        .unwrap_or_else(|_| error(StatusCode::REQUEST_TIMEOUT))
    };
    if response.status().is_client_error() || response.status().is_server_error() {
        // Early origin/auth/body failures cannot leave a keep-alive drain.
        response
            .headers_mut()
            .insert("connection", HeaderValue::from_static("close"));
    }
    let headers = response.headers_mut();
    for (key, value) in [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
    ] {
        headers.insert(key, HeaderValue::from_static(value));
    }
    // Explicit ws origin also covers Firefox versions that don't include a
    // websocket scheme in connect-src 'self'. No inline/eval/third-party code.
    let csp = format!("default-src 'none'; script-src 'self'; style-src 'self'; img-src blob:; connect-src {} {}; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        gate.origin.url(), gate.origin.url().replacen("http:", "ws:", 1));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_str(&csp).expect("literal loopback origin"),
    );
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
pub(crate) fn http_session(gate: &Gateway, headers: &HeaderMap) -> Result<SessionId, StatusCode> {
    gate.authenticate(
        headers,
        origin::single(headers, "x-floe-csrf").ok_or(StatusCode::UNAUTHORIZED)?,
    )
}
/// Only the fixed-field download form endpoint uses body CSRF. The outer
/// guard still requires exact Origin/Host, with the same cookie+CSRF proof.
pub(crate) fn download_session(
    gate: &Gateway,
    headers: &HeaderMap,
    csrf: &str,
) -> Result<SessionId, StatusCode> {
    gate.authenticate(headers, csrf)
}
async fn capabilities(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = http_session(&gate, &headers) {
        return error(e);
    }
    let render = gate.service.is_some() || gate.view.is_some();
    Json(json!({"protocol":1,"bundle":BUNDLE,"stage":if gate.service.is_some(){"owner-service"}else if render{"view-stream"}else{"transport"},
        "render":render,"catalog":gate.service.is_some(),"index":gate.service.is_some(),"index_open":gate.service.is_some(),"launcher":gate.cli_owner,"file_picker":gate.browse.is_some(),"jobdeck_modes":gate.service.is_some(),"jobdeck_levels":gate.service.is_some(),"drc":gate.drc.is_some(),"drc_notes":gate.drc.as_ref().is_some_and(|r|r.notes_enabled()),"drc_waives":gate.drc.as_ref().is_some_and(|r|r.waives_enabled()),"exports":gate.service.is_some(),"snapshot_png":gate.service.is_some(),"layer_settings":true,"design_defaults":gate.defaults.is_some(),"shares":false,"uploads":false,"control_bytes":CONTROL_BYTES,
        "share_grants":gate.shares.is_some(),
        "fill_slot_edit":gate.fill_slot_edit,"display_only":gate.display_only,
        "display_dump":gate.service.is_some(),"dump_on_start":gate.dump_on_start,
        "display_input":gate.display_input.as_ref().map(|p|json!({"width":p.width,"height":p.height,"bytes":p.bytes.len()})),
        "frame_bytes":crate::view::PACKET_BYTES,"frame_credit":1,"pending_frames":1}))
    .into_response()
}
async fn current_view(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = http_session(&gate, &headers) {
        return error(e);
    }
    let Some(view) = gate.active_view() else {
        return error(StatusCode::NOT_FOUND);
    };
    let snapshot = crate::view::snapshot(
        &view.controller.snapshot(),
        &view.controller.model,
        &view.id,
        "",
    );
    Json(json!({"title":view.title,"source_id":view.source_id,"mode":view.mode,"levels":view.levels,"view":snapshot}))
        .into_response()
}
async fn startup(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = http_session(&gate, &headers) {
        return error(e);
    }
    Json(json!({"request":gate.startup,"confirm_levels":gate.startup_confirm_levels}))
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
    gate.stop_services();
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
    // Pin the view along with this handshake's buffer limits. A concurrent
    // open must not turn a control-only upgrade into an 80 MiB frame stream.
    let attached = gate.active_view();
    ws.protocols([PROTOCOL])
        .max_message_size(CONTROL_BYTES)
        .max_frame_size(CONTROL_BYTES)
        .read_buffer_size(CONTROL_BYTES)
        .write_buffer_size(0)
        .max_write_buffer_size(if attached.is_some() {
            crate::view::PACKET_BYTES + 2 * CONTROL_BYTES
        } else {
            2 * CONTROL_BYTES
        })
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            if let Some(attached) = attached {
                crate::stream::socket(socket, gate, id, attached).await;
            } else {
                control_socket(socket, gate, id).await;
            }
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
    let mut maintenance = tokio::time::interval(Duration::from_millis(250));
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! {
            _ = &mut shutdown => break Ok(()),
            _ = maintenance.tick()=>{
                if gate.browse.as_ref().is_some_and(|b|b.has_failed()) {break Err(io::Error::other("file catalogue stopped"));}
                if let Some(defaults)=&gate.defaults {defaults.maintain();}
                if let Some(drc)=&gate.drc {drc.maintain();}
                crate::sharing::maintenance(&gate);
                if gate.auth.lock().is_ok_and(|a|a.expired(Instant::now())) {
                    gate.stop_services();
                    // There is no owner worker whose exit can stop this listener.
                    // After logout/expiry, drain on this tick, not inside the handler.
                    if gate.display_only {break Ok(());}
                }
                if let Some(view)=gate.active_view() {
                    if view.activity.lock().unwrap().expired(Instant::now()) {view.controller.request_close();}
                }
            },
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
                    // Bound idle drain/keep-alive while allowing an active
                    // large download beyond ten seconds. Upgrade gets the
                    // existing WebSocket deadlines, not the HTTP idle timer.
                    let (stream,http)=crate::idle_io::IdleIo::new(stream,Duration::from_secs(10));
                    let _ = builder.serve_connection(TokioIo::new(stream),service).with_upgrades().await;
                    http.store(false,std::sync::atomic::Ordering::Relaxed);
                });
            }
        }
    };
    gate.stopping.send_replace(true);
    gate.stop_services();
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
    if let Some(transport) = &gate.share_transport {
        let drained = timeout(Duration::from_secs(2), async {
            let _sockets = Arc::clone(&transport.sockets)
                .acquire_many_owned(crate::sharing::SOCKETS as u32)
                .await;
            let _encoder = Arc::clone(&transport.encoders).acquire_owned().await;
            let _bytes = Arc::clone(&transport.bytes)
                .acquire_many_owned(crate::sharing::OUTPUT_BYTES as u32)
                .await;
        })
        .await;
        if drained.is_err() {
            return Err(io::Error::other(
                "guest transport shutdown deadline exceeded",
            ));
        }
    }
    if let Some(shares) = &gate.shares {
        let stopped = timeout(Duration::from_secs(4), async {
            loop {
                let finished = {
                    let mut shares = shares.lock().unwrap();
                    shares.stop();
                    shares.finished()
                };
                if finished {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if stopped.is_err() {
            return Err(io::Error::other("guest worker shutdown deadline exceeded"));
        }
    }
    if let Some(view) = gate.active_view() {
        let stopped = timeout(Duration::from_secs(4), async {
            while !view.controller.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if stopped.is_err() {
            return Err(io::Error::other("view shutdown deadline exceeded"));
        }
    }
    if let Some(service) = &gate.service {
        let stopped = timeout(Duration::from_secs(4), async {
            while !service.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if stopped.is_err() {
            return Err(io::Error::other("owner service shutdown deadline exceeded"));
        }
    }
    if let Some(drc) = &gate.drc {
        if timeout(Duration::from_secs(4), async {
            while !drc.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_err()
        {
            return Err(io::Error::other("DRC service shutdown deadline exceeded"));
        }
    }
    if let Some(browse) = &gate.browse {
        if timeout(Duration::from_secs(4), async {
            while !browse.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_err()
        {
            return Err(io::Error::other(
                "file catalogue shutdown deadline exceeded",
            ));
        }
    }
    if let Some(defaults) = &gate.defaults {
        if timeout(Duration::from_secs(4), async {
            while !defaults.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_err()
        {
            return Err(io::Error::other(
                "design-default shutdown deadline exceeded",
            ));
        }
    }
    result
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;
    #[test]
    fn grants_require_a_workspace_and_are_not_enabled_by_diagnostics() {
        let addr = "127.0.0.1:23456".parse().unwrap();
        let (mut gate, _) = Gateway::new(addr).unwrap();
        assert!(gate.shares.is_none());
        assert!(Gateway::enable_local_sharing(&mut gate).is_err());
        let (mut gate, _) = Gateway::with_display_test(addr).unwrap();
        assert!(Gateway::enable_local_sharing(&mut gate).is_err());
        assert!(gate.shares.is_none());
    }
    #[tokio::test]
    async fn standalone_expiry_stops_without_an_owner_worker() {
        for exchanged in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (gate, _) = Gateway::with_display_test(listener.local_addr().unwrap()).unwrap();
            let old = Instant::now() - Duration::from_secs(3);
            let (mut auth, token) =
                Auth::new(old, Duration::from_secs(1), Duration::from_secs(1)).unwrap();
            if exchanged {
                auth.exchange(&token.expose(), old).unwrap();
            }
            *gate.auth.lock().unwrap() = auth;
            assert!(gate.service.is_none() && gate.view.is_none());
            timeout(
                Duration::from_secs(2),
                serve(listener, gate, std::future::pending()),
            )
            .await
            .unwrap()
            .unwrap();
        }
    }
    #[test]
    fn never_connected_and_detached_views_have_distinct_bounded_lifetimes() {
        let now = Instant::now();
        let mut a = Activity {
            connected: 0,
            seen: false,
            since: now,
        };
        assert!(!a.expired(now + Duration::from_secs(119)));
        assert!(a.expired(now + Duration::from_secs(120)));
        a.seen = true;
        assert!(!a.expired(now + Duration::from_secs(59)));
        assert!(a.expired(now + Duration::from_secs(60)));
        a.connected = 1;
        assert!(!a.expired(now + Duration::from_secs(86400)));
    }
}
