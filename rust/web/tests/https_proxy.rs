//! Transport-only, simulated HTTPS termination over a synthetic loopback socket.
//! This deliberately retains bootstrap + cookie + CSRF; it is not user login.
use floe_web::transport::{self, Gateway, BUNDLE, PROTOCOL};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{collections::BTreeMap, net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

const PUBLIC: &str = "https://192.0.2.10:8443";
const HOST: &str = "192.0.2.10:8443";
// Synthetic fixtures only; never deployment credentials.
const KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const WRONG: &str = "2222222222222222222222222222222222222222222222222222222222222222";

struct Server {
    addr: SocketAddr,
    bootstrap: String,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}
struct Reply {
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
}
impl Server {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (mut gate, bootstrap) = Gateway::new(addr).unwrap();
        Gateway::enable_https_proxy(&mut gate, PUBLIC, KEY).unwrap();
        assert_eq!(gate.origin(), PUBLIC);
        assert!(Gateway::enable_https_proxy(&mut gate, PUBLIC, KEY).is_err());
        let (stop, rx) = oneshot::channel();
        let task = tokio::spawn(transport::serve(listener, gate, async {
            let _ = rx.await;
        }));
        Self {
            addr,
            bootstrap: bootstrap.expose(),
            stop,
            task,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Reply {
        let mut stream = TcpStream::connect(self.addr).await.unwrap();
        let mut bytes = format!(
            "{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (key, value) in headers {
            bytes.push_str(&format!("{key}: {value}\r\n"));
        }
        bytes.push_str("\r\n");
        bytes.push_str(body);
        stream.write_all(bytes.as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        timeout(Duration::from_secs(7), stream.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let split = bytes.windows(4).position(|s| s == b"\r\n\r\n").unwrap();
        let head = std::str::from_utf8(&bytes[..split]).unwrap();
        let mut lines = head.lines();
        let status = lines
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let headers = lines
            .map(|line| {
                let (key, value) = line.split_once(':').unwrap();
                (key.to_ascii_lowercase(), value.trim().to_owned())
            })
            .collect();
        Reply {
            status,
            headers,
            body: String::from_utf8(bytes[split + 4..].to_vec()).unwrap(),
        }
    }
    async fn login(&self) -> (String, String) {
        let body =
            json!({"bootstrap": self.bootstrap, "protocol": 1, "bundle": BUNDLE}).to_string();
        let headers = [
            ("Host", HOST),
            ("Origin", PUBLIC),
            ("X-Floe-Proxy-Key", KEY),
            ("Content-Type", "application/json"),
        ];
        let reply = self
            .request("POST", "/api/v1/session/exchange", &headers, &body)
            .await;
        assert_eq!(reply.status, 200);
        let cookie = &reply.headers["set-cookie"];
        assert!(
            cookie.contains("; Secure")
                && cookie.contains("; HttpOnly")
                && cookie.contains("SameSite=Strict")
        );
        assert_eq!(reply.headers["cache-control"], "no-store");
        let value: Value = serde_json::from_str(&reply.body).unwrap();
        (
            cookie.split(';').next().unwrap().into(),
            value["csrf"].as_str().unwrap().into(),
        )
    }
    async fn shutdown(self) {
        let _ = self.stop.send(());
        timeout(Duration::from_secs(5), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn proxy_proof_is_additional_not_a_replacement_for_owner_authentication() {
    let s = Server::start().await;
    for headers in [
        vec![("Host", HOST)],
        vec![("Host", HOST), ("X-Floe-Proxy-Key", WRONG)],
        vec![
            ("Host", HOST),
            ("X-Floe-Proxy-Key", KEY),
            ("X-Floe-Proxy-Key", KEY),
        ],
        vec![
            ("Host", "evil.example"),
            ("X-Floe-Proxy-Key", KEY),
            ("X-Forwarded-Host", HOST),
        ],
        vec![
            ("Host", HOST),
            ("X-Floe-Proxy-Key", KEY),
            ("Origin", "http://192.0.2.10:8443"),
        ],
        vec![
            ("Host", HOST),
            ("X-Floe-Proxy-Key", KEY),
            ("Origin", PUBLIC),
            ("Origin", PUBLIC),
        ],
        vec![
            ("Host", HOST),
            ("X-Forwarded-Proto", "https"),
            ("X-Forwarded-For", "127.0.0.1"),
        ],
    ] {
        assert_eq!(
            s.request("GET", "/api/v1/capabilities", &headers, "")
                .await
                .status,
            403
        );
    }
    let proxy = [("Host", HOST), ("X-Floe-Proxy-Key", KEY)];
    assert_eq!(
        s.request("GET", "/api/v1/capabilities", &proxy, "")
            .await
            .status,
        401
    );
    let shell = s.request("GET", "/", &proxy, "").await;
    assert_eq!(shell.status, 200);
    assert!(shell.headers["content-security-policy"]
        .contains("https://192.0.2.10:8443 wss://192.0.2.10:8443"));
    assert!(!shell.body.contains(&s.bootstrap));
    let body = json!({"bootstrap": s.bootstrap, "protocol": 1, "bundle": BUNDLE}).to_string();
    let mut headers = proxy.to_vec();
    headers.push(("Content-Type", "application/json"));
    assert_eq!(
        s.request("POST", "/api/v1/session/exchange", &headers, &body)
            .await
            .status,
        403
    );
    let (cookie, csrf) = s.login().await;
    headers.extend([("Cookie", cookie.as_str()), ("X-Floe-CSRF", csrf.as_str())]);
    assert_eq!(
        s.request("GET", "/api/v1/capabilities", &headers, "")
            .await
            .status,
        200
    );
    assert_eq!(
        s.request("GET", "/api/v1/capabilities?user=other", &headers, "")
            .await
            .status,
        403
    );
    // Cookies/CSRF cannot move between independent gateways, even with the
    // same public origin and proxy proof (neither is an owner credential).
    let other = Server::start().await;
    assert_eq!(
        other
            .request("GET", "/api/v1/capabilities", &headers, "")
            .await
            .status,
        401
    );
    other.shutdown().await;
    headers.push(("Origin", PUBLIC));
    let logout = s.request("DELETE", "/api/v1/session", &headers, "").await;
    assert_eq!(logout.status, 204);
    assert!(logout.headers["set-cookie"].contains("Max-Age=0; Secure"));
    assert_eq!(
        s.request("GET", "/api/v1/capabilities", &headers, "")
            .await
            .status,
        401
    );
    s.shutdown().await;
}

#[tokio::test]
async fn websocket_upgrade_requires_proxy_proof_https_origin_cookie_and_csrf() {
    let s = Server::start().await;
    let (cookie, csrf) = s.login().await;
    for (proof, origin, secret, expected) in [
        (None, Some(PUBLIC), csrf.as_str(), 403),
        (Some(WRONG), Some(PUBLIC), csrf.as_str(), 403),
        (Some(KEY), None, csrf.as_str(), 403),
        (
            Some(KEY),
            Some("http://192.0.2.10:8443"),
            csrf.as_str(),
            403,
        ),
        (Some(KEY), Some(PUBLIC), WRONG, 401),
        (Some(KEY), Some(PUBLIC), csrf.as_str(), 101),
    ] {
        let mut request = format!("ws://{}/api/v1/events", s.addr)
            .into_client_request()
            .unwrap();
        let headers = request.headers_mut();
        headers.insert("host", HOST.parse().unwrap());
        headers.insert("cookie", cookie.parse().unwrap());
        headers.insert(
            "sec-websocket-protocol",
            format!("{PROTOCOL}, bundle.{BUNDLE}, csrf.{secret}")
                .parse()
                .unwrap(),
        );
        if let Some(p) = proof {
            headers.insert("x-floe-proxy-key", p.parse().unwrap());
        }
        if let Some(o) = origin {
            headers.insert("origin", o.parse().unwrap());
        }
        let result = timeout(Duration::from_secs(5), connect_async(request))
            .await
            .unwrap();
        if expected == 101 {
            let (mut socket, response) = result.unwrap();
            assert_eq!(response.status(), 101);
            let message = timeout(Duration::from_secs(5), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let value: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
            assert_eq!(value["type"], "hello");
            socket.close(None).await.unwrap();
        } else {
            match result {
                Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                    assert_eq!(response.status(), expected)
                }
                _ => panic!("unexpected websocket result"),
            }
        }
    }
    s.shutdown().await;
}

#[test]
fn invalid_proxy_configuration_is_atomic() {
    let (mut gate, _) = Gateway::new("127.0.0.1:58080".parse().unwrap()).unwrap();
    for (url, key) in [
        ("http://192.0.2.10", KEY),
        (PUBLIC, "short"),
        (PUBLIC, &"A".repeat(64)),
    ] {
        assert!(Gateway::enable_https_proxy(&mut gate, url, key).is_err());
        assert_eq!(gate.origin(), "http://127.0.0.1:58080");
    }
    let shared = std::sync::Arc::clone(&gate);
    assert!(Gateway::enable_https_proxy(&mut gate, PUBLIC, KEY).is_err());
    assert_eq!(shared.origin(), "http://127.0.0.1:58080");
}
