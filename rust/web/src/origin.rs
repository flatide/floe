//! Fixed listener/public origin. Request forwarding headers never select it.
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
    /// An administrator-selected HTTPS origin behind a same-host proxy.
    /// Keep the Rust socket on loopback; TLS termination is the proxy's job.
    pub fn for_https_proxy(addr: SocketAddr, public: &str) -> Result<Self, &'static str> {
        Self::for_listener(addr)?;
        let canonical = floe_app_core::server::HttpsOrigin::parse(public)?;
        Ok(Self {
            authority: canonical.authority().into(),
            url: public.to_owned(),
        })
    }
    pub fn is_https(&self) -> bool {
        self.url.starts_with("https://")
    }
    pub fn websocket_url(&self) -> String {
        self.url.replacen("http", "ws", 1)
    }
    /// Only the explicit demo-test policy may call this. Exact Host/Origin and
    /// loopback checks remain mandatory; only transport encryption is omitted.
    pub(crate) fn for_http_test_proxy(
        addr: SocketAddr,
        public: &str,
    ) -> Result<Self, &'static str> {
        Self::for_listener(addr)?;
        let canonical = floe_app_core::server::HttpTestOrigin::parse(public)?;
        Ok(Self {
            authority: canonical.authority().into(),
            url: public.to_owned(),
        })
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
    fn http_test_proxy_never_accepts_forwarding_headers_as_authority() {
        let addr = "127.0.0.1:58080".parse().unwrap();
        let o = Origin::for_http_test_proxy(addr, "http://10.0.0.10:8080").unwrap();
        assert!(!o.is_https());
        assert_eq!(o.websocket_url(), "ws://10.0.0.10:8080");
        let mut h = HeaderMap::new();
        h.insert("host", "10.0.0.10:8080".parse().unwrap());
        h.insert("origin", "http://10.0.0.10:8080".parse().unwrap());
        assert!(o.host_matches(&h) && o.origin_matches(&h, true));
        h.insert("origin", "https://10.0.0.10:8080".parse().unwrap());
        assert!(!o.origin_matches(&h, true));
        h.insert("host", "evil.test".parse().unwrap());
        h.insert("x-forwarded-host", "10.0.0.10:8080".parse().unwrap());
        assert!(!o.host_matches(&h));
        assert!(Origin::for_http_test_proxy(
            "0.0.0.0:58080".parse().unwrap(),
            "http://10.0.0.10:8080"
        )
        .is_err());
        assert!(Origin::for_https_proxy(addr, "http://10.0.0.10:8080").is_err());
    }
    #[test]
    fn proxy_origin_is_explicit_canonical_https_and_never_a_public_listener() {
        let local = "127.0.0.1:58080".parse().unwrap();
        for value in [
            "https://192.0.2.10",
            "https://192.0.2.10:8443",
            "https://floe.example",
            "https://[2001:db8::1]:8443",
        ] {
            let o = Origin::for_https_proxy(local, value).unwrap();
            assert!(o.is_https());
            assert!(o.websocket_url().starts_with("wss://"));
            let mut h = HeaderMap::new();
            h.insert(header::HOST, o.authority().parse().unwrap());
            h.insert(header::ORIGIN, value.parse().unwrap());
            assert!(o.host_matches(&h) && o.origin_matches(&h, true));
            h.insert(
                header::ORIGIN,
                value.replacen("https", "http", 1).parse().unwrap(),
            );
            assert!(!o.origin_matches(&h, true));
        }
        for value in [
            "http://192.0.2.10",
            "https://user:pass@host",
            "https://host/",
            "https://host/path",
            "https://host?x",
            "https://host#x",
            "https://host:0",
            "https://host:65536",
            "https://host:0443",
            "https://host:443",
            "https://HOST",
            "https://host.",
            "https://127.1",
            "https://2130706433",
            "https://0x7f000001",
            "https://192.000.2.1",
            "https://0.0.0.0",
            "https://[::]",
            "https://host%0d",
            "https://a_b",
            "https://-host",
            "https://host:",
            "https://host\r\nx: y",
        ] {
            assert!(Origin::for_https_proxy(local, value).is_err(), "{value:?}");
        }
        assert!(
            Origin::for_https_proxy("0.0.0.0:58080".parse().unwrap(), "https://192.0.2.10")
                .is_err()
        );
    }
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
