//! Owned service lifetime; never detach render/index workers on host exit.
use floe_app::embedded::{Ready, Session};
use floe_app_core::{Error, Result};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc,
};
use std::thread::{self, JoinHandle};

pub struct Service {
    pub ready: mpsc::Receiver<Ready>,
    stop: Arc<AtomicUsize>,
    thread: Option<JoinHandle<Result<i32>>>,
    signals: Vec<signal_hook::SigId>,
}
impl Service {
    pub fn start(session: Session) -> Result<Self> {
        let stop = Arc::new(AtomicUsize::new(0));
        let (tx, ready) = mpsc::sync_channel(1);
        let mut service = Self {
            ready,
            stop,
            thread: None,
            signals: Vec::new(),
        };
        for n in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            service.signals.push(
                signal_hook::flag::register_usize(n, Arc::clone(&service.stop), n as usize)
                    .map_err(|_| Error::input("cannot register desktop termination handler"))?,
            );
        }
        let flag = Arc::clone(&service.stop);
        service.thread = Some(
            thread::Builder::new()
                .name("floe-desktop-service".into())
                .spawn(move || {
                    session.run(&flag, move |ready| {
                        tx.send(ready)
                            .map_err(|_| Error::input("desktop closed before startup"))
                    })
                })
                .map_err(|_| Error::input("cannot start desktop service"))?,
        );
        Ok(service)
    }
    pub fn cancel(&self) {
        self.stop.store(15, Ordering::Relaxed);
    }
    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }
    pub fn join(&mut self) -> Result<i32> {
        self.thread.take().map_or(Ok(0), |t| {
            t.join()
                .map_err(|_| Error::input("desktop service panicked"))?
        })
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.cancel();
        let _ = self.join();
        for id in self.signals.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_the_host_cancels_and_joins_its_worker() {
        let stop = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));
        let flag = Arc::clone(&stop);
        let joined = Arc::clone(&done);
        let (_, ready) = mpsc::channel();
        let thread = thread::spawn(move || {
            while flag.load(Ordering::Relaxed) == 0 {
                thread::yield_now();
            }
            joined.store(1, Ordering::Release);
            Ok(0)
        });
        drop(Service {
            ready,
            stop,
            thread: Some(thread),
            signals: Vec::new(),
        });
        assert_eq!(done.load(Ordering::Acquire), 1);
    }

    #[test]
    fn worker_failure_is_reported_and_join_is_once_only() {
        let (_, ready) = mpsc::channel();
        let mut service = Service {
            ready,
            stop: Arc::new(AtomicUsize::new(0)),
            thread: Some(thread::spawn(|| {
                Err(Error::input("synthetic worker failure"))
            })),
            signals: Vec::new(),
        };
        assert!(service.join().is_err());
        assert!(service.finished());
        assert_eq!(service.join().unwrap(), 0);
    }
}
