//! Canonical administrator-selected origin shared by preflight and transport.
//! This validates spelling, not DNS ownership, TLS certificates or reachability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpsOrigin {
    authority: String,
}
impl HttpsOrigin {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        Self::parse_scheme(value, "https", 443)
    }
    fn parse_scheme(value: &str, scheme: &str, default_port: u16) -> Result<Self, &'static str> {
        let authority = value
            .strip_prefix(&format!("{scheme}://"))
            .ok_or("public origin scheme does not match the configured transport policy")?;
        if authority.is_empty()
            || authority.len() > 300
            || !authority.is_ascii()
            || authority
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
            || authority.contains(['/', '\\', '@', '?', '#', '%'])
        {
            return Err("public origin must contain only a canonical host and optional port");
        }
        let (host, port) = if authority.starts_with('[') {
            let end = authority.find(']').ok_or("invalid public IPv6 authority")?;
            let tail = &authority[end + 1..];
            let port = if tail.is_empty() {
                None
            } else {
                Some(tail.strip_prefix(':').ok_or("invalid public IPv6 port")?)
            };
            (&authority[..=end], port)
        } else if let Some((host, port)) = authority.split_once(':') {
            (host, Some(port))
        } else {
            (authority, None)
        };
        let canonical_host = if host.starts_with('[') && host.ends_with(']') {
            let ip: std::net::Ipv6Addr = host[1..host.len() - 1]
                .parse()
                .map_err(|_| "invalid public IPv6 address")?;
            if ip.is_unspecified() || ip.is_multicast() {
                return Err("invalid public address");
            }
            format!("[{ip}]")
        } else if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
            if ip.is_unspecified() || ip.is_multicast() {
                return Err("invalid public address");
            }
            ip.to_string()
        } else {
            // Reject WHATWG numeric-IP aliases, empty/trailing labels and IDN
            // ambiguity. International names must already be ASCII punycode.
            if host.len() > 253
                || host.split('.').any(|label| {
                    label.is_empty()
                        || label.len() > 63
                        || !label.as_bytes()[0].is_ascii_alphanumeric()
                        || !label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                        || !label
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
                || host.rsplit('.').next().is_some_and(|last| {
                    last.bytes().all(|b| b.is_ascii_digit())
                        || last.to_ascii_lowercase().starts_with("0x")
                })
            {
                return Err("invalid or ambiguous public host");
            }
            host.to_ascii_lowercase()
        };
        let port = port
            .map(|p| p.parse::<u16>())
            .transpose()
            .map_err(|_| "invalid public port")?;
        if port == Some(0) {
            return Err("public port must be nonzero");
        }
        let canonical = match port {
            Some(n) if n != default_port => format!("{canonical_host}:{n}"),
            _ => canonical_host,
        };
        if authority != canonical {
            return Err(
                "public origin must use canonical lowercase host and port (omit the default port)",
            );
        }
        Ok(Self {
            authority: canonical,
        })
    }
    pub fn authority(&self) -> &str {
        &self.authority
    }
    pub fn url(&self) -> String {
        format!("https://{}", self.authority)
    }
}

/// Explicit unencrypted demo-test origin. This validates spelling, not whether
/// a hostname/address is on a trusted LAN. The operator must restrict ingress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpTestOrigin(HttpsOrigin);
impl HttpTestOrigin {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        HttpsOrigin::parse_scheme(value, "http", 80).map(Self)
    }
    pub fn authority(&self) -> &str {
        self.0.authority()
    }
    pub fn url(&self) -> String {
        format!("http://{}", self.authority())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn http_test_is_explicit_and_canonical_with_its_own_default_port() {
        for value in [
            "http://10.0.0.10:8080",
            "http://localhost",
            "http://[::1]:8080",
            "http://test.example:443",
        ] {
            assert_eq!(HttpTestOrigin::parse(value).unwrap().url(), value);
            assert!(HttpsOrigin::parse(value).is_err());
        }
        for value in [
            "https://test.example",
            "http://test.example:80",
            "http://test.example:080",
            "http://TEST",
            "http://user@test",
            "http://127.1",
            "http://0.0.0.0",
            "http://test/",
            "http://test?x",
            "http://test#x",
            "http://test:0",
        ] {
            assert!(HttpTestOrigin::parse(value).is_err(), "{value}");
        }
    }
    #[test]
    fn canonical_origin_not_a_request_url() {
        for value in [
            "https://192.0.2.1",
            "https://example.test:8443",
            "https://[2001:db8::1]",
        ] {
            assert_eq!(HttpsOrigin::parse(value).unwrap().url(), value);
        }
        for value in [
            "http://example.test",
            "https://USER@host",
            "https://host/",
            "https://host?x",
            "https://host#x",
            "https://HOST",
            "https://host:443",
            "https://host:0443",
            "https://host:0",
            "https://host:65536",
            "https://host:",
            "https://host:+80",
            "https://host:80:90",
            "https://host.",
            "https://127.1",
            "https://2130706433",
            "https://0x7f000001",
            "https://192.000.2.1",
            "https://0.0.0.0",
            "https://[::]",
            "https://[::1]x",
            "https://[::1]:",
            "https://::1",
            "https://a_b",
            "https://host%0a",
            "https://한글",
            "https://\nlocalhost",
            "https://",
        ] {
            assert!(HttpsOrigin::parse(value).is_err(), "{value:?}");
        }
    }
}
