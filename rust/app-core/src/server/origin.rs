//! Canonical administrator-selected origin shared by preflight and transport.
//! This validates spelling, not DNS ownership, TLS certificates or reachability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpsOrigin {
    authority: String,
}
impl HttpsOrigin {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        let authority = value
            .strip_prefix("https://")
            .ok_or("public origin requires https://")?;
        if authority.is_empty()
            || authority.len() > 300
            || !authority.is_ascii()
            || authority
                .bytes()
                .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
            || authority.contains(['/', '\\', '@', '?', '#', '%'])
        {
            return Err("public origin must contain only a canonical HTTPS host and optional port");
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
            Some(n) if n != 443 => format!("{canonical_host}:{n}"),
            _ => canonical_host,
        };
        if authority != canonical {
            return Err("public origin must use canonical lowercase host and port (omit :443)");
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

#[cfg(test)]
mod tests {
    use super::*;
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
