//! Strict literal loopback origin. Proxy/TLS/public binding requires a
//! separate deployment policy, not X-Forwarded-* headers from the caller.
use axum::http::{header, HeaderMap};
use std::net::SocketAddr;
#[derive(Debug, Clone)]
pub struct Origin {
    authority: String,
    url: String,
}
impl Origin {
    pub fn for_listener(addr: SocketAddr) -> Result<Self, &'static str> {
        if !addr.ip().is_loopback() || addr.port() == 0 {
            return Err("gateway requires a bound loopback listener");
        }
        let authority = addr.to_string();
        Ok(Self {
            url: format!("http://{authority}"),
            authority,
        })
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn authority(&self) -> &str {
        &self.authority
    }
    pub fn host_matches(&self, headers: &HeaderMap) -> bool {
        single(headers, header::HOST.as_str()) == Some(self.authority.as_str())
    }
    /// GET with no Origin still requires the separate CSRF secret. Mutations
    /// and WebSocket handshakes additionally require an exact Origin header.
    pub fn origin_matches(&self, headers: &HeaderMap, required: bool) -> bool {
        if !headers.contains_key(header::ORIGIN) {
            return !required;
        }
        single(headers, header::ORIGIN.as_str()) == Some(self.url.as_str())
    }
}
pub fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(first)
}
/// Cookie-name match, not a substring; duplicate credentials are ambiguous
/// and rejected. Unknown cookies may coexist on the same loopback host.
pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut result = None;
    for header in headers.get_all(header::COOKIE) {
        for item in header.to_str().ok()?.split(';') {
            let (key, value) = item.trim().split_once('=')?;
            if key == name {
                if result.is_some() {
                    return None;
                }
                result = Some(value);
            }
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_authority_and_origin_cannot_be_rebound_or_forwarded() {
        for value in ["0.0.0.0:9000", "192.0.2.1:9000", "127.0.0.1:0"] {
            assert!(Origin::for_listener(value.parse().unwrap()).is_err());
        }
        for addr in ["127.0.0.1:9000", "[::1]:9000"] {
            let o = Origin::for_listener(addr.parse().unwrap()).unwrap();
            let mut h = HeaderMap::new();
            h.insert(header::HOST, addr.parse().unwrap());
            h.insert(header::ORIGIN, o.url().parse().unwrap());
            assert!(o.host_matches(&h) && o.origin_matches(&h, true));
            for bad in [
                "http://evil.example",
                "null",
                "http://127.0.0.1:9001",
                "http://localhost:9000",
            ] {
                h.insert(header::ORIGIN, bad.parse().unwrap());
                assert!(!o.origin_matches(&h, true));
            }
            h.remove(header::ORIGIN);
            assert!(!o.origin_matches(&h, true) && o.origin_matches(&h, false));
            h.insert("x-forwarded-host", addr.parse().unwrap());
            h.insert(header::HOST, "evil.example".parse().unwrap());
            assert!(!o.host_matches(&h));
            h.insert(header::HOST, addr.parse().unwrap());
            h.append(header::HOST, addr.parse().unwrap());
            assert!(!o.host_matches(&h));
        }
    }
    #[test]
    fn cookies_are_exact_and_duplicates_fail() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, "other=abc; floe=def".parse().unwrap());
        assert_eq!(cookie(&h, "floe"), Some("def"));
        assert_eq!(cookie(&h, "flo"), None);
        h.append(header::COOKIE, "floe=ghi".parse().unwrap());
        assert_eq!(cookie(&h, "floe"), None);
    }
}
