//! Opt-in local guest identities. Never insert these credentials into owner
//! Auth: every existing HTTP/WS handler must continue to reject guest proofs.
mod http;
pub(crate) use http::routes;

use crate::auth::{public_id, Auth, AuthError, Credentials, Secret, SessionId};
use floe_worker_client::Layers;
use serde::Deserialize;
use std::time::{Duration, Instant};

pub(crate) const INVITE_TTL: Duration = Duration::from_secs(120);
pub(crate) const SESSION_TTL: Duration = Duration::from_secs(1800);
const MAX_GRANTS: usize = 4;

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
/// In this first slice no geometry/DRC endpoint is granted. Follow/explore
/// consumers must check this binding again before publishing any data.
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
}
pub(crate) struct Invitation {
    pub id: String,
    pub token: Secret,
    pub mode: Mode,
}
/// A distinct type from an owner SessionId; possession of public IDs is never
/// a grant. Re-authenticate on every HTTP request / streaming publication.
pub(crate) struct Guest {
    pub share_id: String,
    pub session: SessionId,
    pub mode: Mode,
}
#[derive(Default)]
pub(crate) struct Shares {
    entries: Vec<Entry>,
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
        self.entries
            .retain(|entry| !entry.auth.expired(now) && valid(&entry.owner, &entry.scope));
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
        self.entries.push(Entry {
            id: id.clone(),
            owner,
            scope,
            mode,
            auth,
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
        self.entries.retain(|e| &e.owner != owner || e.id != id);
        before != self.entries.len()
    }
    fn logout(&mut self, guest: &Guest, now: Instant) {
        self.entries
            .retain(|e| e.id != guest.share_id || !e.auth.alive(&guest.session, now));
    }
    pub(crate) fn stop(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
