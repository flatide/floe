//! Real loopback lifecycle through the in-process host API, no browser/GUI.
//! Run with matching native binaries; this is NOT WebView/ETX acceptance.
use floe_app::embedded::{validate_ready, Ready, Session};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};

struct Running {
    stop: Arc<AtomicUsize>,
    thread: Option<thread::JoinHandle<floe_app_core::Result<i32>>>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(15, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn request(
    ready: &Ready,
    method: &str,
    path: &str,
    extra: &str,
    body: &str,
) -> (u16, String, String) {
    let address = ready.origin.strip_prefix("http://").unwrap();
    let mut socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(socket, "{method} {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: {}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", ready.origin, body.len()).unwrap();
    let mut text = String::new();
    socket
        .take(2 * 1024 * 1024)
        .read_to_string(&mut text)
        .unwrap();
    let (headers, body) = text.split_once("\r\n\r\n").expect("HTTP response");
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, headers.to_owned(), body.to_owned())
}

#[test]
#[ignore = "requires matching floe-index/renderd; tools/validate_rust.sh --only embedded_host"]
fn in_process_bootstrap_authentication_and_confirmed_shutdown() {
    let session = Session::parse(&[]).unwrap();
    let stop = Arc::new(AtomicUsize::new(0));
    let flag = Arc::clone(&stop);
    let (send, receive) = mpsc::sync_channel(1);
    let thread = thread::spawn(move || {
        session.run(&flag, move |ready| {
            send.send(ready)
                .map_err(|_| floe_app_core::Error::input("host closed"))
        })
    });
    let mut running = Running {
        stop,
        thread: Some(thread),
    };
    let ready = receive
        .recv_timeout(Duration::from_secs(30))
        .expect("in-process ready callback");
    validate_ready(&ready).unwrap();
    let token = ready.url.split_once("#bootstrap=").unwrap().1;
    let body = serde_json::json!({
        "bootstrap": token, "protocol": 1, "bundle": floe_web::transport::BUNDLE,
    })
    .to_string();
    let (status, headers, body) = request(&ready, "POST", "/api/v1/session/exchange", "", &body);
    assert_eq!(status, 200); // Never echo credential-bearing bodies/headers.
    let cookie = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("set-cookie")
                .then(|| value.trim().split(';').next().unwrap())
        })
        .expect("session cookie");
    let json: serde_json::Value = serde_json::from_str(&body).expect("exchange JSON");
    let csrf = json["csrf"].as_str().expect("CSRF token");
    let extra = format!("Cookie: {cookie}\r\nX-Floe-CSRF: {csrf}\r\n");
    let (status, _, _) = request(&ready, "DELETE", "/api/v1/session", &extra, "");
    assert_eq!(status, 204);
    let result = running
        .thread
        .take()
        .unwrap()
        .join()
        .expect("service thread")
        .unwrap();
    assert_eq!(result, 0);
    assert!(TcpStream::connect(ready.origin.strip_prefix("http://").unwrap()).is_err());
}
