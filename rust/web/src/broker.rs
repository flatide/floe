//! TeeBox or public-demo sessions. Separate from the standalone Gateway.
//! Optional native runtime is separate from HTTP; no index writer/file API.
mod assets;
mod http;
pub mod runtime;
mod stream;
use crate::{
    auth::{Auth, Credentials, Secret, SessionId},
    origin::{self, Origin},
};
use axum::http::HeaderMap;
use floe_app_core::server::{Deployment, Principal, ValidatedConfig};
pub use http::{serve, serve_runtime};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    net::SocketAddr,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};
pub use stream::PROTOCOL;
use tokio::sync::{watch, Semaphore};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unauthorized,
    Invalid,
    Busy,
    Unavailable,
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy)]
pub struct Lifetimes {
    pub bootstrap: Duration,
    pub session: Duration,
}
impl Default for Lifetimes {
    fn default() -> Self {
        Self {
            bootstrap: Duration::from_secs(120),
            session: Duration::from_secs(8 * 3600),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    pub user_id: String,
    pub source: String,
}

/// Deliver through the authorized launch response, never logs or process argv.
/// The chosen client exchanges this bootstrap once for session credentials.
#[derive(Debug)]
pub struct Launch {
    pub id: String,
    pub bootstrap: Secret,
}

/// Immutable trusted context, not deserializable from a browser or a path ID.
pub struct Binding {
    principal: Principal,
    selection: String,
    source: PathBuf,
    stamp: [u64; 7],
}
impl Binding {
    pub fn principal(&self) -> &Principal {
        &self.principal
    }
    pub fn source(&self) -> &Path {
        &self.source
    }
}
/// A snapshot is not lasting authority. Revalidate on every request/worker
/// acquisition, and observe the cancellation receiver throughout worker use.
#[derive(Clone)]
pub struct Access {
    id: String,
    session: SessionId,
    binding: Arc<Binding>,
}
impl Access {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn binding(&self) -> &Binding {
        &self.binding
    }
}
/// A server-owned runner holds this until its native child is reaped. Drop
/// deliberately does not free capacity: a child might still be running.
/// This is a session renderer, never the shared indexing job whose lifetime
/// must survive this particular user's logout.
pub struct WorkerLease {
    id: String,
    claim: String,
    binding: Arc<Binding>,
    cancelled: watch::Receiver<bool>,
    stop: Arc<AtomicUsize>,
}
impl WorkerLease {
    pub fn binding(&self) -> &Binding {
        &self.binding
    }
    pub fn cancellation(&self) -> watch::Receiver<bool> {
        self.cancelled.clone()
    }
}
struct Entry {
    auth: Option<Auth>,
    binding: Arc<Binding>,
    worker: Option<String>,
    cancelled: watch::Sender<bool>,
    stop: Arc<AtomicUsize>,
}
impl Entry {
    fn close(&mut self) {
        self.auth = None;
        self.stop.store(1, Ordering::Relaxed);
        self.cancelled.send_replace(true);
    }
}
struct State {
    stopping: bool,
    runtime_attached: bool,
    entries: BTreeMap<String, Entry>,
    demo_tokens: u8,
    demo_refill: Instant,
}
pub struct Broker {
    addr: SocketAddr,
    origin: Origin,
    policy: ValidatedConfig,
    client_id: String,
    proxy: Secret,
    delegator: Option<Secret>,
    lifetimes: Lifetimes,
    state: Mutex<State>,
    lookups: Arc<Semaphore>,
}
impl Broker {
    /// Trusted runtime construction only. Proxy proof and TeeBox's independent
    /// delegation credential are distinct secrets; neither enters browser UI.
    /// The proxy must ALSO authenticate client_id using its account/password.
    pub fn new(
        addr: SocketAddr,
        policy: ValidatedConfig,
        proxy_key: &str,
        delegation_key: &str,
        lifetimes: Lifetimes,
    ) -> Result<Self> {
        let Deployment::TeeBox { client_id, .. } = &policy.config().deployment else {
            return Err(Error::Invalid);
        };
        let client_id = client_id.clone();
        let delegator = Secret::parse(delegation_key).ok_or(Error::Invalid)?;
        Self::configured(
            addr,
            policy,
            proxy_key,
            Some(delegator),
            client_id,
            lifetimes,
        )
    }
    /// Anonymous visitors may choose ONLY configured public sample IDs. This
    /// mode has no TeeBox delegation, owner Service or file/index/write APIs.
    pub fn public_demo(addr: SocketAddr, policy: ValidatedConfig, proxy_key: &str) -> Result<Self> {
        if !matches!(policy.config().deployment, Deployment::PublicDemo { .. })
            || policy.config().max_sessions > 4
        {
            return Err(Error::Invalid);
        }
        Self::configured(
            addr,
            policy,
            proxy_key,
            None,
            String::new(),
            Lifetimes {
                bootstrap: Duration::from_secs(30),
                session: Duration::from_secs(15 * 60),
            },
        )
    }
    fn configured(
        addr: SocketAddr,
        policy: ValidatedConfig,
        proxy_key: &str,
        delegator: Option<Secret>,
        client_id: String,
        lifetimes: Lifetimes,
    ) -> Result<Self> {
        let origin =
            Origin::for_https_proxy(addr, policy.public_origin()).map_err(|_| Error::Invalid)?;
        let proxy = Secret::parse(proxy_key).ok_or(Error::Invalid)?;
        if delegator.as_ref().is_some_and(|key| proxy.matches(key))
            || lifetimes.bootstrap.is_zero()
            || lifetimes.bootstrap.subsec_nanos() != 0
            || lifetimes.bootstrap > Duration::from_secs(600)
            || lifetimes.session.is_zero()
            || lifetimes.session.subsec_nanos() != 0
            || lifetimes.session > Duration::from_secs(86400)
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            addr,
            origin,
            policy,
            client_id,
            proxy,
            delegator,
            lifetimes,
            state: Mutex::new(State {
                stopping: false,
                runtime_attached: false,
                entries: BTreeMap::new(),
                demo_tokens: 4,
                demo_refill: Instant::now(),
            }),
            lookups: Arc::new(Semaphore::new(2)),
        })
    }
    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| Error::Unavailable)
    }
    fn boundary(&self, headers: &HeaderMap, require_origin: bool) -> Result<()> {
        let proxy = origin::single(headers, "x-floe-proxy-key")
            .and_then(Secret::parse)
            .ok_or(Error::Unauthorized)?;
        if !self.proxy.matches(&proxy)
            || !self.origin.host_matches(headers)
            || !self.origin.origin_matches(headers, require_origin)
        {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    fn delegation(&self, headers: &HeaderMap) -> Result<()> {
        self.boundary(headers, false)?;
        let delegator = self.delegator.as_ref().ok_or(Error::Unauthorized)?;
        // Authorization remains available for the proxy's account/password
        // authentication. The independent launcher proof has its own header.
        let key = origin::single(headers, "x-floe-delegation-key")
            .and_then(Secret::parse)
            .ok_or(Error::Unauthorized)?;
        if !delegator.matches(&key)
            || origin::single(headers, "x-floe-service-client") != Some(&self.client_id)
        {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    /// May touch configured filesystem metadata; HTTP calls this on a bounded
    /// blocking executor, never on the async I/O thread. No source is decoded.
    pub fn launch(
        &self,
        headers: &HeaderMap,
        request: LaunchRequest,
        now: Instant,
    ) -> Result<Launch> {
        self.delegation(headers)?;
        let principal = self
            .policy
            .principal(&self.client_id, &request.user_id)
            .map_err(|_| Error::Invalid)?;
        self.issue(request.source, principal, now)
    }
    pub fn is_public_demo(&self) -> bool {
        self.delegator.is_none()
    }
    pub fn demo_samples(&self) -> Result<Vec<String>> {
        match &self.policy.config().deployment {
            Deployment::PublicDemo { samples, .. } => {
                Ok(samples.iter().map(|s| s.id.clone()).collect())
            }
            _ => Err(Error::Unauthorized),
        }
    }
    pub fn launch_demo(&self, headers: &HeaderMap, sample: String, now: Instant) -> Result<Launch> {
        self.boundary(headers, true)?;
        if !self.is_public_demo() {
            return Err(Error::Unauthorized);
        }
        {
            let mut state = self.lock()?;
            let added = now.saturating_duration_since(state.demo_refill).as_secs() / 5;
            if added > 0 {
                state.demo_tokens = (u64::from(state.demo_tokens) + added.min(4)).min(4) as u8;
                state.demo_refill = now;
            }
            if state.demo_tokens == 0 {
                return Err(Error::Busy);
            }
            state.demo_tokens -= 1;
        }
        self.issue(sample, Principal::anonymous_demo(), now)
    }
    fn issue(&self, selection: String, principal: Principal, now: Instant) -> Result<Launch> {
        // Cheap admission before a possibly slow NFS lookup; recheck atomically
        // below because another launch can claim the last slot meanwhile.
        {
            let mut state = self.lock()?;
            sweep(&mut state, now);
            self.admit(&state)?;
        }
        let source = self
            .policy
            .resolve_source(&selection)
            .map_err(|_| Error::Invalid)?;
        let stamp = source_stamp(&source)?;
        let binding = Arc::new(Binding {
            principal,
            selection,
            source,
            stamp,
        });
        let id = crate::auth::public_id().map_err(|_| Error::Unavailable)?;
        let (auth, bootstrap) = Auth::new(now, self.lifetimes.bootstrap, self.lifetimes.session)
            .map_err(|_| Error::Unavailable)?;
        let (cancelled, _) = watch::channel(false);
        let mut state = self.lock()?;
        sweep(&mut state, now);
        self.admit(&state)?;
        if state.entries.contains_key(&id) {
            return Err(Error::Unavailable);
        }
        state.entries.insert(
            id.clone(),
            Entry {
                auth: Some(auth),
                binding,
                worker: None,
                cancelled,
                stop: Arc::new(AtomicUsize::new(0)),
            },
        );
        Ok(Launch { id, bootstrap })
    }
    fn admit(&self, state: &State) -> Result<()> {
        if state.stopping {
            return Err(Error::Unavailable);
        }
        if state.entries.len() >= usize::from(self.policy.config().max_sessions) {
            return Err(Error::Busy);
        }
        Ok(())
    }
    pub fn exchange(
        &self,
        id: &str,
        token: &str,
        headers: &HeaderMap,
        now: Instant,
    ) -> Result<Credentials> {
        self.boundary(headers, true)?;
        let mut state = self.lock()?;
        sweep(&mut state, now);
        let entry = state.entries.get_mut(id).ok_or(Error::Unauthorized)?;
        entry
            .auth
            .as_mut()
            .ok_or(Error::Unauthorized)?
            .exchange(token, now)
            .map_err(|_| Error::Unauthorized)
    }
    pub fn authorize(&self, id: &str, headers: &HeaderMap, now: Instant) -> Result<Access> {
        self.boundary(headers, false)?;
        let cookie = origin::cookie(headers, &cookie_name(id)?).ok_or(Error::Unauthorized)?;
        let csrf = origin::single(headers, "x-floe-csrf").ok_or(Error::Unauthorized)?;
        let mut state = self.lock()?;
        sweep(&mut state, now);
        let entry = state.entries.get(id).ok_or(Error::Unauthorized)?;
        let session = entry
            .auth
            .as_ref()
            .ok_or(Error::Unauthorized)?
            .authenticate(cookie, csrf, now)
            .map_err(|_| Error::Unauthorized)?;
        Ok(Access {
            id: id.into(),
            session,
            binding: Arc::clone(&entry.binding),
        })
    }
    pub fn logout(&self, id: &str, headers: &HeaderMap, now: Instant) -> Result<()> {
        self.boundary(headers, true)?;
        let access = self.authorize(id, headers, now)?;
        let mut state = self.lock()?;
        let entry = state
            .entries
            .get_mut(&access.id)
            .ok_or(Error::Unauthorized)?;
        if !entry
            .auth
            .as_ref()
            .is_some_and(|auth| auth.alive(&access.session, now))
        {
            return Err(Error::Unauthorized);
        }
        entry.close();
        sweep(&mut state, now);
        Ok(())
    }
    /// The trusted TeeBox can revoke even an unredeemed launch. This operation
    /// never accepts owner cookies in place of delegation credentials.
    pub fn revoke(&self, id: &str, headers: &HeaderMap, now: Instant) -> Result<()> {
        self.delegation(headers)?;
        let mut state = self.lock()?;
        if let Some(entry) = state.entries.get_mut(id) {
            entry.close();
        }
        sweep(&mut state, now);
        Ok(())
    }
    /// Called by the trusted native adapter, not exposed as an HTTP operation.
    /// Source registration/dependency checks and revision leases remain required.
    pub fn claim_worker(&self, access: &Access, now: Instant) -> Result<WorkerLease> {
        let started = Instant::now();
        {
            let mut state = self.lock()?;
            sweep(&mut state, now);
            let entry = state.entries.get(&access.id).ok_or(Error::Unauthorized)?;
            if !Arc::ptr_eq(&entry.binding, &access.binding)
                || !entry
                    .auth
                    .as_ref()
                    .is_some_and(|auth| auth.alive(&access.session, now))
            {
                return Err(Error::Unauthorized);
            }
            if entry.worker.is_some() {
                return Err(Error::Busy);
            }
        }
        // Scope/stamp changes between grant and worker start must not silently
        // widen the session. Never perform filesystem I/O while holding state.
        let source = self
            .policy
            .resolve_source(&access.binding.selection)
            .map_err(|_| Error::Invalid)?;
        if source != access.binding.source || source_stamp(&source)? != access.binding.stamp {
            return Err(Error::Invalid);
        }
        let claim = crate::auth::public_id().map_err(|_| Error::Unavailable)?;
        // A slow metadata lookup must not extend the authenticated lifetime.
        let now = now
            .checked_add(started.elapsed())
            .ok_or(Error::Unavailable)?;
        let mut state = self.lock()?;
        sweep(&mut state, now);
        let entry = state
            .entries
            .get_mut(&access.id)
            .ok_or(Error::Unauthorized)?;
        if !entry
            .auth
            .as_ref()
            .is_some_and(|auth| auth.alive(&access.session, now))
        {
            return Err(Error::Unauthorized);
        }
        if entry.worker.is_some() {
            return Err(Error::Busy);
        }
        entry.worker = Some(claim.clone());
        Ok(WorkerLease {
            id: access.id.clone(),
            claim,
            binding: Arc::clone(&entry.binding),
            cancelled: entry.cancelled.subscribe(),
            stop: Arc::clone(&entry.stop),
        })
    }
    // Short in-memory operations only. This linearizes native dispatch with
    // logout; no filesystem lookup, child wait or join may run under this lock.
    fn with_access<T>(&self, access: &Access, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let now = Instant::now();
        let mut state = self.lock()?;
        sweep(&mut state, now);
        let entry = state.entries.get(&access.id).ok_or(Error::Unauthorized)?;
        if !Arc::ptr_eq(&entry.binding, &access.binding)
            || !entry
                .auth
                .as_ref()
                .is_some_and(|a| a.alive(&access.session, now))
        {
            return Err(Error::Unauthorized);
        }
        f()
    }
    /// Failed preparation before claim: close only this exact unclaimed
    /// session, never a concurrently owned worker or a different binding.
    fn abandon_unclaimed(&self, access: &Access) -> Result<()> {
        let mut state = self.lock()?;
        if let Some(entry) = state.entries.get_mut(&access.id) {
            if Arc::ptr_eq(&entry.binding, &access.binding) && entry.worker.is_none() {
                entry.close();
            }
        }
        sweep(&mut state, Instant::now());
        Ok(())
    }
    fn subscribe(&self, access: &Access) -> Result<watch::Receiver<bool>> {
        let now = Instant::now();
        let mut state = self.lock()?;
        sweep(&mut state, now);
        let entry = state.entries.get(&access.id).ok_or(Error::Unauthorized)?;
        if !Arc::ptr_eq(&entry.binding, &access.binding)
            || !entry
                .auth
                .as_ref()
                .is_some_and(|a| a.alive(&access.session, now))
        {
            return Err(Error::Unauthorized);
        }
        Ok(entry.cancelled.subscribe())
    }
    /// Only after child reap. Unplanned worker death terminates the session too;
    /// it does not silently rebind old credentials to a replacement worker.
    pub fn worker_reaped(&self, lease: &WorkerLease) -> Result<()> {
        let mut state = self.lock()?;
        let entry = state
            .entries
            .get_mut(&lease.id)
            .ok_or(Error::Unauthorized)?;
        if entry.worker.as_ref() != Some(&lease.claim) {
            return Err(Error::Unauthorized);
        }
        entry.close();
        state.entries.remove(&lease.id);
        Ok(())
    }
    pub fn maintain(&self, now: Instant) -> Result<()> {
        let mut state = self.lock()?;
        sweep(&mut state, now);
        Ok(())
    }
    pub fn stop(&self) -> Result<()> {
        let mut state = self.lock()?;
        state.stopping = true;
        for entry in state.entries.values_mut() {
            entry.close();
        }
        sweep(&mut state, Instant::now());
        Ok(())
    }
    pub fn pending_workers(&self) -> Result<usize> {
        Ok(self
            .lock()?
            .entries
            .values()
            .filter(|entry| entry.worker.is_some())
            .count())
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        // A runner must treat sender closure as cancellation too. Publish the
        // explicit value even on an in-process early-return/drop path.
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        for entry in state.entries.values_mut() {
            entry.close();
        }
    }
}
fn sweep(state: &mut State, now: Instant) {
    state.entries.retain(|_, entry| {
        if entry.auth.as_ref().is_none_or(|auth| auth.expired(now)) {
            entry.close();
        }
        entry.auth.is_some() || entry.worker.is_some()
    });
}
fn cookie_name(id: &str) -> Result<String> {
    Secret::parse(id).ok_or(Error::Unauthorized)?;
    Ok(format!("__Secure-floe_server_{id}"))
}
fn source_stamp(path: &Path) -> Result<[u64; 7]> {
    let m = fs::metadata(path).map_err(|_| Error::Invalid)?;
    if !m.is_file() {
        return Err(Error::Invalid);
    }
    Ok([
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime() as u64,
        m.mtime_nsec() as u64,
        m.ctime() as u64,
        m.ctime_nsec() as u64,
    ])
}

#[cfg(test)]
mod tests;
