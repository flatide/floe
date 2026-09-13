//! HTTP idle timeout, not an absolute transfer duration. Upgrade completion
//! disables this timer: the WebSocket protocol owns its separate deadlines.
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    time::{Instant, Sleep},
};
pub(crate) struct IdleIo<T> {
    inner: T,
    delay: Duration,
    timer: Pin<Box<Sleep>>,
    enabled: Arc<AtomicBool>,
}
impl<T> IdleIo<T> {
    pub fn new(inner: T, delay: Duration) -> (Self, Arc<AtomicBool>) {
        let enabled = Arc::new(AtomicBool::new(true));
        (
            Self {
                inner,
                delay,
                timer: Box::pin(tokio::time::sleep(delay)),
                enabled: Arc::clone(&enabled),
            },
            enabled,
        )
    }
    fn timed_out(&mut self, cx: &mut Context<'_>) -> bool {
        self.enabled.load(Ordering::Relaxed) && self.timer.as_mut().poll(cx).is_ready()
    }
    fn touch(&mut self) {
        self.timer.as_mut().reset(Instant::now() + self.delay);
    }
}
fn timeout() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "HTTP idle deadline exceeded")
}
impl<T: AsyncRead + Unpin> AsyncRead for IdleIo<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let s = self.get_mut();
        if s.timed_out(cx) {
            return Poll::Ready(Err(timeout()));
        }
        let before = buf.filled().len();
        let p = Pin::new(&mut s.inner).poll_read(cx, buf);
        if matches!(p, Poll::Ready(Ok(()))) && buf.filled().len() > before {
            s.touch();
        }
        p
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for IdleIo<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let s = self.get_mut();
        if s.timed_out(cx) {
            return Poll::Ready(Err(timeout()));
        }
        let p = Pin::new(&mut s.inner).poll_write(cx, buf);
        if matches!(p,Poll::Ready(Ok(n)) if n>0) {
            s.touch();
        }
        p
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let s = self.get_mut();
        if s.timed_out(cx) {
            return Poll::Ready(Err(timeout()));
        }
        Pin::new(&mut s.inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn active_hyper_body_outlives_idle_deadline() {
        use axum::{
            body::{Body, Bytes},
            routing::get,
            Router,
        };
        use hyper_util::{
            rt::{TokioIo, TokioTimer},
            service::TowerToHyperService,
        };
        let app = Router::new().route(
            "/",
            get(|| async {
                let chunks = futures_util::stream::unfold(0u8, |n| async move {
                    if n == 15 {
                        return None;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Some((
                        Ok::<_, io::Error>(Bytes::from_static(b"transfer-chunk\n")),
                        n + 1,
                    ))
                });
                Body::from_stream(chunks)
            }),
        );
        let (server, mut client) = tokio::io::duplex(1024);
        let (server, _) = IdleIo::new(server, Duration::from_secs(1));
        let task = tokio::spawn(async move {
            hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .serve_connection(TokioIo::new(server), TowerToHyperService::new(app))
                .await
        });
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(8), client.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));
        assert_eq!(
            response
                .windows(15)
                .filter(|b| *b == b"transfer-chunk\n")
                .count(),
            15
        );
        task.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn active_io_refreshes_but_idle_io_expires_and_upgrade_disables_timer() {
        let (a, mut b) = tokio::io::duplex(16);
        let (mut a, http) = IdleIo::new(a, Duration::from_secs(60));
        b.write_all(b"a").await.unwrap();
        assert_eq!(a.read_u8().await.unwrap(), b'a');
        assert!(a.timer.deadline() > Instant::now() + Duration::from_secs(59));
        a.timer.as_mut().reset(Instant::now());
        assert_eq!(
            a.read_u8().await.unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        http.store(false, Ordering::Relaxed);
        b.write_all(b"b").await.unwrap();
        assert_eq!(a.read_u8().await.unwrap(), b'b');
        a.write_all(b"c").await.unwrap();
        assert_eq!(b.read_u8().await.unwrap(), b'c');
    }
}
