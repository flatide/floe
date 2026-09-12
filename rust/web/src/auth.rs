//! Local bootstrap exchange. Secrets never implement Serialize or unredacted
//! Debug. A session needs both an HttpOnly cookie and an origin-bound CSRF
//! secret; cookies alone are shared across ports on a loopback host.
use std::fmt;
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct Secret([u8; 32]);
impl Secret {
    fn generate() -> Result<Self, AuthError> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|_| AuthError::Entropy)?;
        Ok(Self(bytes))
    }
    fn parse(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        if bytes.len() != 64 || !bytes.is_ascii() {
            return None;
        }
        let hex = |b| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        };
        let mut out = [0; 32];
        for (out, pair) in out.iter_mut().zip(bytes.chunks_exact(2)) {
            *out = hex(pair[0])? * 16 + hex(pair[1])?;
        }
        Some(Self(out))
    }
    fn matches(&self, other: &Self) -> bool {
        bool::from(self.0.ct_eq(&other.0))
    }
    /// Only call for the launcher fragment, bootstrap response or Set-Cookie.
    /// Never put this value in a request path, log or error message.
    pub fn expose(&self) -> String {
        self.0.iter().map(|v| format!("{v:02x}")).collect()
    }
}
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionId(String);
/// Public, non-credential identities use independent entropy, never a cookie
/// or bootstrap value reused as a view/connection identifier.
pub(crate) fn public_id() -> Result<String, AuthError> {
    Ok(Secret::generate()?.expose())
}
impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Debug)]
pub struct Credentials {
    pub id: SessionId,
    pub cookie: Secret,
    pub csrf: Secret,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthError {
    Unauthorized,
    Entropy,
    InvalidLifetime,
}
impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unauthorized => "session expired or unauthorized",
            Self::Entropy => "operating system entropy unavailable",
            Self::InvalidLifetime => "invalid session lifetime",
        })
    }
}
impl std::error::Error for AuthError {}
struct Grant {
    credentials: Credentials,
    expires: Instant,
}
pub struct Auth {
    bootstrap: Option<(Secret, Instant)>,
    session: Option<Grant>,
    session_ttl: Duration,
}
impl Auth {
    pub(crate) fn expired(&self, now: Instant) -> bool {
        if let Some(session) = &self.session {
            now >= session.expires
        } else {
            self.bootstrap
                .as_ref()
                .is_none_or(|(_, deadline)| now >= *deadline)
        }
    }
    /// M1 local owner session only. Shares and independent identities are a
    /// later service; do not infer guest grants from possession of a view ID.
    pub fn new(
        now: Instant,
        bootstrap_ttl: Duration,
        session_ttl: Duration,
    ) -> Result<(Self, Secret), AuthError> {
        if bootstrap_ttl.is_zero()
            || bootstrap_ttl > Duration::from_secs(600)
            || session_ttl.is_zero()
            || session_ttl > Duration::from_secs(86400)
        {
            return Err(AuthError::InvalidLifetime);
        }
        let token = Secret::generate()?;
        Ok((
            Self {
                bootstrap: Some((token.clone(), now + bootstrap_ttl)),
                session: None,
                session_ttl,
            },
            token,
        ))
    }
    pub fn exchange(&mut self, token: &str, now: Instant) -> Result<Credentials, AuthError> {
        let input = Secret::parse(token).ok_or(AuthError::Unauthorized)?;
        let (expected, deadline) = self.bootstrap.as_ref().ok_or(AuthError::Unauthorized)?;
        if now >= *deadline || !expected.matches(&input) {
            return Err(AuthError::Unauthorized);
        }
        // Allocate every secret before consuming the one-use grant. An OS
        // entropy failure must not produce a partially authenticated session.
        let credentials = Credentials {
            id: SessionId(Secret::generate()?.expose()),
            cookie: Secret::generate()?,
            csrf: Secret::generate()?,
        };
        self.session = Some(Grant {
            credentials: Credentials {
                id: credentials.id.clone(),
                cookie: credentials.cookie.clone(),
                csrf: credentials.csrf.clone(),
            },
            expires: now + self.session_ttl,
        });
        self.bootstrap = None;
        Ok(credentials)
    }
    pub fn authenticate(
        &self,
        cookie: &str,
        csrf: &str,
        now: Instant,
    ) -> Result<SessionId, AuthError> {
        let cookie = Secret::parse(cookie).ok_or(AuthError::Unauthorized)?;
        let csrf = Secret::parse(csrf).ok_or(AuthError::Unauthorized)?;
        let grant = self.session.as_ref().ok_or(AuthError::Unauthorized)?;
        // Non-short-circuit comparison of both fixed-size secrets.
        let matches =
            grant.credentials.cookie.matches(&cookie) & grant.credentials.csrf.matches(&csrf);
        if now >= grant.expires || !matches {
            return Err(AuthError::Unauthorized);
        }
        Ok(grant.credentials.id.clone())
    }
    pub fn alive(&self, session: &SessionId, now: Instant) -> bool {
        self.session
            .as_ref()
            .is_some_and(|g| &g.credentials.id == session && now < g.expires)
    }
    pub fn revoke(&mut self, session: &SessionId) {
        if self
            .session
            .as_ref()
            .is_some_and(|g| &g.credentials.id == session)
        {
            self.session = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_is_one_use_and_has_a_deadline() {
        let now = Instant::now();
        let (mut a, b) = Auth::new(now, Duration::from_secs(2), Duration::from_secs(5)).unwrap();
        assert!(!a.expired(now));
        assert!(a.expired(now + Duration::from_secs(2)));
        assert!(a.exchange(&"0".repeat(64), now).is_err());
        let c = a.exchange(&b.expose(), now).unwrap();
        assert!(!a.expired(now + Duration::from_secs(2)));
        assert!(a.expired(now + Duration::from_secs(5)));
        assert!(a.exchange(&b.expose(), now).is_err());
        assert_eq!(
            a.authenticate(&c.cookie.expose(), &c.csrf.expose(), now)
                .unwrap(),
            c.id
        );
        let (mut a, b) = Auth::new(now, Duration::from_secs(2), Duration::from_secs(5)).unwrap();
        assert!(a
            .exchange(&b.expose(), now + Duration::from_secs(2))
            .is_err());
    }
    #[test]
    fn cookie_alone_wrong_csrf_expiry_and_revocation_fail_closed() {
        let now = Instant::now();
        let (mut a, b) = Auth::new(now, Duration::from_secs(2), Duration::from_secs(5)).unwrap();
        let c = a.exchange(&b.expose(), now).unwrap();
        for (cookie, csrf) in [
            (c.cookie.expose(), String::new()),
            (String::new(), c.csrf.expose()),
            (c.csrf.expose(), c.cookie.expose()),
            (c.cookie.expose(), "0".repeat(64)),
        ] {
            assert!(a.authenticate(&cookie, &csrf, now).is_err());
        }
        assert!(a.alive(&c.id, now));
        assert!(!a.alive(&c.id, now + Duration::from_secs(5)));
        assert!(a
            .authenticate(
                &c.cookie.expose(),
                &c.csrf.expose(),
                now + Duration::from_secs(5)
            )
            .is_err());
        a.revoke(&c.id);
        assert!(a.expired(now));
        assert!(!a.alive(&c.id, now));
        assert!(a
            .authenticate(&c.cookie.expose(), &c.csrf.expose(), now)
            .is_err());
        assert!(a.exchange(&b.expose(), now).is_err());
    }
    #[test]
    fn secret_parser_and_debug_never_expose_secrets() {
        for bad in [
            "한글".repeat(32),
            "g".repeat(64),
            "a".repeat(63),
            "A".repeat(64),
            "1".repeat(65),
        ] {
            assert!(Secret::parse(&bad).is_none());
        }
        let s = Secret::generate().unwrap();
        assert!(s.matches(&Secret::parse(&s.expose()).unwrap()));
        assert_eq!(format!("{s:?}"), "Secret([REDACTED])");
        assert!(!format!("{:?}", AuthError::Unauthorized).contains(&s.expose()));
    }
    #[test]
    fn lifetime_bounds_are_checked() {
        let now = Instant::now();
        for (boot, session) in [(0, 10), (10, 0), (601, 10), (10, 86401)] {
            assert!(matches!(
                Auth::new(now, Duration::from_secs(boot), Duration::from_secs(session)),
                Err(AuthError::InvalidLifetime)
            ));
        }
    }
}
