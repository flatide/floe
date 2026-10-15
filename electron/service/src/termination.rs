//! Install before any pipe handshake; keep handlers through cleanup/join.
use std::sync::{atomic::AtomicUsize, Arc};

pub struct Termination {
    pub flag: Arc<AtomicUsize>,
    ids: Vec<signal_hook::SigId>,
}
impl Termination {
    pub fn install() -> std::io::Result<Self> {
        let mut this = Self {
            flag: Arc::new(AtomicUsize::new(0)),
            ids: Vec::new(),
        };
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            this.ids.push(signal_hook::flag::register_usize(
                signal,
                Arc::clone(&this.flag),
                signal as usize,
            )?);
        }
        Ok(this)
    }
}
impl Drop for Termination {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

/// A blocked initial/folder message is also cancellable. Returning a non-
/// Interrupted error avoids read_exact silently retrying a cancellation.
pub struct Input {
    pub file: std::fs::File,
    pub stop: Arc<AtomicUsize>,
}
impl std::io::Read for Input {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        use std::{os::fd::AsRawFd, sync::atomic::Ordering};
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            if self.stop.load(Ordering::Relaxed) != 0 {
                return Err(std::io::Error::other("pipe read cancelled"));
            }
            let mut poll = libc::pollfd {
                fd: self.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: the descriptor is borrowed from the live owned file.
            let ready = unsafe { libc::poll(&mut poll, 1, 50) };
            if ready < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if ready > 0 {
                return self.file.read(bytes);
            }
        }
    }
}
