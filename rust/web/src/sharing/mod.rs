//! Opt-in local guest identities. Never insert these credentials into owner
//! Auth: every existing HTTP/WS handler must continue to reject guest proofs.
mod explore;
mod http;
mod stream;
pub(crate) use http::maintenance;
pub(crate) use http::routes;

use crate::auth::{public_id, Auth, AuthError, Credentials, Secret, SessionId};
use floe_app_core::view::{ViewController, ViewState};
use floe_worker_client::Layers;
use serde::Deserialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{watch, Semaphore};

pub(crate) const INVITE_TTL: Duration = Duration::from_secs(120);
pub(crate) const SESSION_TTL: Duration = Duration::from_secs(1800);
const MAX_GRANTS: usize = 4;
pub(crate) const OUTPUT_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const SOCKETS: usize = 4;
const EXPLORE_IDLE: Duration = Duration::from_secs(60);

fn idle_expired(since: &mut Option<Instant>, connected: bool, now: Instant) -> bool {
    if connected {
        *since = None;
        false
    } else {
        now.saturating_duration_since(*since.get_or_insert(now)) >= EXPLORE_IDLE
    }
}

/// Separate admission from owner output: a slow guest cannot consume its
/// packet/encoder credits. This is transport accounting, not a process RSS cap.
pub(crate) struct Transport {
    pub bytes: Arc<Semaphore>,
    pub encoders: Arc<Semaphore>,
    pub sockets: Arc<Semaphore>,
}
impl Default for Transport {
    fn default() -> Self {
        Self {
            bytes: Arc::new(Semaphore::new(OUTPUT_BYTES)),
            encoders: Arc::new(Semaphore::new(1)),
            sockets: Arc::new(Semaphore::new(SOCKETS)),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(try_from = "String")]
pub(crate) enum Mode {
    Follow,
    Explore,
}
impl TryFrom<String> for Mode {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "follow" => Ok(Self::Follow),
            "explore" => Ok(Self::Explore),
            _ => Err("invalid share mode"),
        }
    }
}
impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Explore => "explore",
        }
    }
}

/// Server-captured binding, not a browser-selected path or unbounded catalog.
/// Frame consumers also check the actual native request, not just the latest
/// controller snapshot: a stale frame may belong to an earlier layer scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Scope {
    pub view_id: String,
    pub dataset_revision: u64,
    pub layers: Layers,
}
struct Entry {
    id: String,
    owner: SessionId,
    scope: Scope,
    mode: Mode,
    auth: Auth,
    revoked: watch::Sender<bool>,
    socket: Arc<Semaphore>,
    explore: Option<Arc<explore::Explorer>>,
    saved: Option<ViewState>,
    disconnected_since: Option<Instant>,
}
impl Drop for Entry {
    fn drop(&mut self) {
        self.revoked.send_replace(true);
    }
}
pub(crate) struct Invitation {
    pub id: String,
    pub token: Secret,
    pub mode: Mode,
}
/// A distinct type from an owner SessionId; possession of public IDs is never
/// a grant. Re-authenticate on every HTTP request / streaming publication.
#[derive(Clone)]
pub(crate) struct Guest {
    pub share_id: String,
    pub session: SessionId,
    pub mode: Mode,
}
struct Lease {
    guest: Guest,
    scope: Scope,
    revoked: watch::Receiver<bool>,
    socket: Arc<Semaphore>,
}
#[derive(Default)]
pub(crate) struct Shares {
    entries: Vec<Entry>,
    // Never drop the last controller Arc on an HTTP handler while its native
    // thread is still running: ViewController::drop joins the thread.
    retired: Vec<Arc<explore::Explorer>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Unauthorized,
    Full,
    Entropy,
}
impl From<AuthError> for Failure {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::Entropy => Self::Entropy,
            _ => Self::Unauthorized,
        }
    }
}
impl Shares {
    fn maintain(&mut self, now: Instant, valid: impl Fn(&SessionId, &Scope) -> bool) {
        self.retired.retain(|v| !v.controller.is_finished());
        self.remove_if(|entry| entry.auth.expired(now) || !valid(&entry.owner, &entry.scope));
        for entry in &mut self.entries {
            if entry.explore.is_none() {
                continue;
            }
            if idle_expired(
                &mut entry.disconnected_since,
                entry.socket.available_permits() == 0,
                now,
            ) {
                let view = entry.explore.take().unwrap();
                entry.saved = Some(view.controller.snapshot().state);
                view.controller.request_close();
                self.retired.push(view);
                entry.disconnected_since = None;
            }
        }
    }
    fn remove_if(&mut self, remove: impl Fn(&Entry) -> bool) {
        let mut i = 0;
        while i < self.entries.len() {
            if remove(&self.entries[i]) {
                let mut entry = self.entries.remove(i);
                if let Some(view) = entry.explore.take() {
                    view.controller.request_close();
                    self.retired.push(view);
                }
                // Entry's Drop wakes all authenticated sockets/encoders.
            } else {
                i += 1;
            }
        }
    }
    fn issue(
        &mut self,
        owner: SessionId,
        scope: Scope,
        mode: Mode,
        now: Instant,
    ) -> Result<Invitation, Failure> {
        if self.entries.len() >= MAX_GRANTS {
            return Err(Failure::Full);
        }
        // Do not consume a slot on entropy failure. IDs are public and use
        // independent entropy; invite/cookie/CSRF are never reused as IDs.
        let id = public_id()?;
        let (auth, token) = Auth::new(now, INVITE_TTL, SESSION_TTL)?;
        let (revoked, _) = watch::channel(false);
        self.entries.push(Entry {
            id: id.clone(),
            owner,
            scope,
            mode,
            auth,
            revoked,
            socket: Arc::new(Semaphore::new(1)),
            explore: None,
            saved: None,
            disconnected_since: None,
        });
        Ok(Invitation { id, token, mode })
    }
    fn exchange(&mut self, id: &str, token: &str, now: Instant) -> Result<Credentials, Failure> {
        self.entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or(Failure::Unauthorized)?
            .auth
            .exchange(token, now)
            .map_err(Into::into)
    }
    fn authenticate(
        &self,
        id: &str,
        cookie: &str,
        csrf: &str,
        now: Instant,
    ) -> Result<Guest, Failure> {
        let entry = self
            .entries
            .iter()
            .find(|e| e.id == id)
            .ok_or(Failure::Unauthorized)?;
        Ok(Guest {
            share_id: entry.id.clone(),
            session: entry.auth.authenticate(cookie, csrf, now)?,
            mode: entry.mode,
        })
    }
    fn revoke_owner(&mut self, owner: &SessionId, id: &str) -> bool {
        let before = self.entries.len();
        self.remove_if(|e| &e.owner == owner && e.id == id);
        before != self.entries.len()
    }
    fn lease(&self, guest: Guest, now: Instant) -> Result<Lease, Failure> {
        let entry = self
            .entries
            .iter()
            .find(|e| {
                e.id == guest.share_id && e.mode == guest.mode && e.auth.alive(&guest.session, now)
            })
            .ok_or(Failure::Unauthorized)?;
        Ok(Lease {
            guest,
            scope: entry.scope.clone(),
            revoked: entry.revoked.subscribe(),
            socket: Arc::clone(&entry.socket),
        })
    }
    fn valid(&self, lease: &Lease, now: Instant) -> bool {
        !*lease.revoked.borrow()
            && self.entries.iter().any(|e| {
                e.id == lease.guest.share_id
                    && e.scope == lease.scope
                    && e.mode == lease.guest.mode
                    && e.auth.alive(&lease.guest.session, now)
            })
    }
    fn logout(&mut self, guest: &Guest, now: Instant) {
        self.remove_if(|e| e.id == guest.share_id && e.auth.alive(&guest.session, now));
    }
    pub(crate) fn stop(&mut self) {
        self.remove_if(|_| true);
        self.retired.retain(|v| !v.controller.is_finished());
    }
    pub(crate) fn finished(&self) -> bool {
        self.entries.iter().all(|e| e.explore.is_none())
            && self.retired.iter().all(|v| v.controller.is_finished())
    }
    fn explorer(
        &mut self,
        lease: &Lease,
        owner: &ViewController,
        now: Instant,
    ) -> Result<Arc<explore::Explorer>, axum::http::StatusCode> {
        use axum::http::StatusCode as S;
        if !self.valid(lease, now) || lease.guest.mode != Mode::Explore {
            return Err(S::UNAUTHORIZED);
        }
        let index = self
            .entries
            .iter()
            .position(|e| e.id == lease.guest.share_id)
            .ok_or(S::UNAUTHORIZED)?;
        if let Some(view) = &self.entries[index].explore {
            return Ok(Arc::clone(view));
        }
        if self.retired.len() + self.entries.iter().filter(|e| e.explore.is_some()).count()
            >= MAX_GRANTS
        {
            return Err(S::TOO_MANY_REQUESTS);
        }
        let initial = self.entries[index]
            .saved
            .clone()
            .unwrap_or_else(|| owner.snapshot().state);
        if owner.model.dataset_revision != lease.scope.dataset_revision
            || !explore::layers_within(&lease.scope.layers, &initial.layers)
        {
            return Err(S::CONFLICT);
        }
        let id = public_id().map_err(|_| S::SERVICE_UNAVAILABLE)?;
        let controller = owner
            .fork_view(initial, 1, 1, 128)
            .map_err(|e| match e.kind {
                floe_app_core::ErrorKind::Busy => S::TOO_MANY_REQUESTS,
                _ => S::SERVICE_UNAVAILABLE,
            })?;
        let view = Arc::new(explore::Explorer {
            id,
            controller: Arc::new(controller),
        });
        self.entries[index].saved = None;
        self.entries[index].explore = Some(Arc::clone(&view));
        self.entries[index].disconnected_since = None;
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explore_idle_uses_continuous_disconnection_not_creation_age() {
        let now = Instant::now();
        let mut since = None;
        assert!(!idle_expired(&mut since, false, now));
        assert!(!idle_expired(
            &mut since,
            false,
            now + EXPLORE_IDLE - Duration::from_nanos(1)
        ));
        assert!(idle_expired(&mut since, false, now + EXPLORE_IDLE));
        assert!(!idle_expired(&mut since, true, now + EXPLORE_IDLE));
        assert!(since.is_none());
        assert!(!idle_expired(&mut since, false, now + EXPLORE_IDLE));
    }
    #[test]
    fn live_leases_are_revoked_on_owner_scope_logout_and_expiry() {
        let now = Instant::now();
        let (_, o) = owner(now);
        for action in 0..5 {
            let mut shares = Shares::default();
            let a = shares
                .issue(o.id.clone(), scope(), Mode::Follow, now)
                .unwrap();
            let c = shares.exchange(&a.id, &a.token.expose(), now).unwrap();
            let guest = shares
                .authenticate(&a.id, &c.cookie.expose(), &c.csrf.expose(), now)
                .unwrap();
            let lease = shares.lease(guest, now).unwrap();
            assert!(shares.valid(&lease, now));
            match action {
                0 => {
                    shares.revoke_owner(&o.id, &a.id);
                }
                1 => shares.logout(&lease.guest, now),
                2 => shares.maintain(now, |_, _| false),
                3 => shares.maintain(now + SESSION_TTL, |_, _| true),
                _ => shares.stop(),
            }
            assert!(*lease.revoked.borrow());
            assert!(!shares.valid(&lease, now));
        }
    }
    #[test]
    fn modes_are_strict_readonly_strings() {
        for mode in ["follow", "explore"] {
            assert!(serde_json::from_value::<Mode>(serde_json::json!(mode)).is_ok());
        }
        for value in [
            serde_json::json!("owner"),
            serde_json::json!({"follow":null}),
            serde_json::Value::Null,
            serde_json::json!(false),
        ] {
            assert!(serde_json::from_value::<Mode>(value).is_err());
        }
    }
    fn owner(now: Instant) -> (Auth, Credentials) {
        let (mut auth, token) = Auth::new(now, INVITE_TTL, SESSION_TTL).unwrap();
        let credentials = auth.exchange(&token.expose(), now).unwrap();
        (auth, credentials)
    }
    fn scope() -> Scope {
        Scope {
            view_id: "synthetic-view".into(),
            dataset_revision: 1,
            layers: Layers::Only(vec![(7, 0)]),
        }
    }
    #[test]
    fn owner_and_guest_proofs_never_authenticate_each_other() {
        let now = Instant::now();
        let (auth, owner) = owner(now);
        let mut shares = Shares::default();
        let invite = shares
            .issue(owner.id.clone(), scope(), Mode::Follow, now)
            .unwrap();
        assert_ne!(invite.id, invite.token.expose());
        assert!(shares
            .exchange(&invite.id, &owner.cookie.expose(), now)
            .is_err());
        let c = shares
            .exchange(&invite.id, &invite.token.expose(), now)
            .unwrap();
        assert!(shares
            .exchange(&invite.id, &invite.token.expose(), now)
            .is_err());
        assert!(auth
            .authenticate(&c.cookie.expose(), &c.csrf.expose(), now)
            .is_err());
        assert!(shares
            .authenticate(
                &invite.id,
                &owner.cookie.expose(),
                &owner.csrf.expose(),
                now
            )
            .is_err());
        for (cookie, csrf) in [
            (c.cookie.expose(), owner.csrf.expose()),
            (owner.cookie.expose(), c.csrf.expose()),
            (c.cookie.expose(), String::new()),
        ] {
            assert!(shares
                .authenticate(&invite.id, &cookie, &csrf, now)
                .is_err());
        }
        assert!(auth
            .authenticate(&owner.cookie.expose(), &owner.csrf.expose(), now)
            .is_ok());
    }
    #[test]
    fn guest_logout_and_owner_revoke_are_local_and_do_not_reopen_invites() {
        let now = Instant::now();
        let (auth, o) = owner(now);
        let (_, other) = owner(now);
        let mut shares = Shares::default();
        let a = shares
            .issue(o.id.clone(), scope(), Mode::Follow, now)
            .unwrap();
        let b = shares
            .issue(o.id.clone(), scope(), Mode::Explore, now)
            .unwrap();
        let ac = shares.exchange(&a.id, &a.token.expose(), now).unwrap();
        let bc = shares.exchange(&b.id, &b.token.expose(), now).unwrap();
        assert!(shares.exchange(&a.id, &b.token.expose(), now).is_err());
        assert!(shares
            .authenticate(&a.id, &bc.cookie.expose(), &bc.csrf.expose(), now)
            .is_err());
        let guest = shares
            .authenticate(&a.id, &ac.cookie.expose(), &ac.csrf.expose(), now)
            .unwrap();
        assert!(!shares.revoke_owner(&other.id, &b.id));
        shares.logout(&guest, now);
        assert!(shares.exchange(&a.id, &a.token.expose(), now).is_err());
        assert!(shares
            .authenticate(&a.id, &ac.cookie.expose(), &ac.csrf.expose(), now)
            .is_err());
        assert!(shares
            .authenticate(&b.id, &bc.cookie.expose(), &bc.csrf.expose(), now)
            .is_ok());
        assert!(auth.alive(&o.id, now));
        assert!(shares.revoke_owner(&o.id, &b.id));
        assert!(!shares.revoke_owner(&o.id, &b.id));
    }
    #[test]
    fn bounded_slots_expire_and_owner_or_scope_change_revokes() {
        let now = Instant::now();
        let (mut auth, o) = owner(now);
        let mut shares = Shares::default();
        for _ in 0..MAX_GRANTS {
            shares
                .issue(o.id.clone(), scope(), Mode::Follow, now)
                .unwrap();
        }
        assert!(matches!(
            shares.issue(o.id.clone(), scope(), Mode::Follow, now),
            Err(Failure::Full)
        ));
        shares.maintain(now + INVITE_TTL, |owner, _| auth.alive(owner, now));
        assert!(shares.entries.is_empty());
        let a = shares
            .issue(o.id.clone(), scope(), Mode::Follow, now)
            .unwrap();
        let ac = shares.exchange(&a.id, &a.token.expose(), now).unwrap();
        assert!(shares
            .authenticate(
                &a.id,
                &ac.cookie.expose(),
                &ac.csrf.expose(),
                now + SESSION_TTL
            )
            .is_err());
        shares.maintain(now, |owner, s| {
            auth.alive(owner, now) && s.dataset_revision == 2
        });
        assert!(shares.entries.is_empty());
        shares
            .issue(o.id.clone(), scope(), Mode::Explore, now)
            .unwrap();
        auth.revoke(&o.id);
        shares.maintain(now, |owner, _| auth.alive(owner, now));
        assert!(shares.entries.is_empty());
        shares.issue(o.id, scope(), Mode::Explore, now).unwrap();
        shares.stop();
        assert!(shares.entries.is_empty());
    }
}
