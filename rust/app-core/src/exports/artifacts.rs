//! Bounded, expiring ownership of unlinked files. No artifact directory or
//! path-based open API: the only inputs are validated worker file descriptors.
//! Retired files with an active reader remain charged until that reader drops.
use crate::{Error, ErrorKind, Result};
use std::{
    collections::BTreeMap,
    fs::File,
    io,
    os::unix::fs::{FileExt, MetadataExt},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const CHUNK_BYTES: usize = 1024 * 1024;
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub entries: usize,
    pub artifact_bytes: u64,
    pub total_bytes: u64,
    pub readers: usize,
    pub ttl: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: 4,
            artifact_bytes: 512 * 1024 * 1024,
            total_bytes: 2 * 1024 * 1024 * 1024,
            readers: 2,
            ttl: Duration::from_secs(600),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub entries: usize,
    pub bytes: u64,
    pub pending: usize,
    pub readers: usize,
}
#[derive(Clone, Copy, Debug)]
pub struct Info {
    pub size_bytes: u64,
    pub expires_in_ms: u64,
}
struct Entry {
    file: Option<Arc<File>>,
    bytes: u64,
    expires: Option<Instant>,
    retired: bool,
    readers: usize,
    stop: Arc<AtomicUsize>,
}
struct State {
    closed: bool,
    rows: BTreeMap<u64, Entry>,
    usage: Usage,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    limits: Limits,
}
pub struct Store {
    shared: Arc<Shared>,
    reaper: Mutex<Option<JoinHandle<()>>>,
}

fn unavailable() -> Error {
    Error::new(
        ErrorKind::Incomplete,
        "export artifact expired or unavailable",
    )
}
/// Remove accounting under the mutex, close the descriptors AFTER releasing
/// it. Closing a large unlinked file can block in the filesystem.
fn sweep(s: &mut State, now: Instant) -> Vec<Arc<File>> {
    let mut retired = Vec::new();
    for entry in s.rows.values_mut() {
        if entry.expires.is_some_and(|t| now >= t) {
            entry.retired = true;
        }
    }
    s.rows.retain(|_, entry| {
        if entry.retired && entry.readers == 0 && entry.file.is_some() {
            s.usage.entries -= 1;
            s.usage.bytes -= entry.bytes;
            retired.push(entry.file.take().unwrap());
            false
        } else {
            true
        }
    });
    retired
}
impl Store {
    pub fn new(limits: Limits) -> Result<Arc<Self>> {
        if !(1..=32).contains(&limits.entries)
            || !(1..=8).contains(&limits.readers)
            || limits.artifact_bytes == 0
            || limits.artifact_bytes > 16 * 1024 * 1024 * 1024
            || limits.total_bytes < limits.artifact_bytes
            || limits.total_bytes > 64 * 1024 * 1024 * 1024
            || limits.ttl.is_zero()
            || limits.ttl > Duration::from_secs(3600)
        {
            return Err(Error::input("invalid artifact storage limits"));
        }
        let shared = Arc::new(Shared {
            limits,
            changed: Condvar::new(),
            state: Mutex::new(State {
                closed: false,
                rows: BTreeMap::new(),
                usage: Usage::default(),
            }),
        });
        let sweep_shared = Arc::clone(&shared);
        let reaper = thread::Builder::new()
            .name("floe-export-expiry".into())
            .spawn(move || {
                let mut s = sweep_shared.state.lock().unwrap();
                loop {
                    let retired = sweep(&mut s, Instant::now());
                    let closed = s.closed;
                    drop(s);
                    drop(retired);
                    if closed {
                        break;
                    }
                    s = sweep_shared.state.lock().unwrap();
                    // A close during descriptor disposal must not lose its wake.
                    if s.closed {
                        continue;
                    }
                    let wait = sweep_shared.limits.ttl.min(Duration::from_secs(1));
                    s = sweep_shared.changed.wait_timeout(s, wait).unwrap().0;
                }
            })?;
        Ok(Arc::new(Self {
            shared,
            reaper: Mutex::new(Some(reaper)),
        }))
    }
    pub fn usage(&self) -> Usage {
        self.shared.state.lock().unwrap().usage
    }
    /// Shutdown must also wait for the expiry thread's descriptor disposal.
    pub fn disposal_finished(&self) -> bool {
        self.reaper
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
    }
    /// Bounded in-memory lookup, without a reader reservation or filesystem I/O.
    pub fn info(&self, id: u64) -> Option<Info> {
        let s = self.shared.state.lock().unwrap();
        let now = Instant::now();
        let e = s.rows.get(&id)?;
        let until = e.expires?;
        (!s.closed && !e.retired && e.file.is_some() && now < until).then(|| Info {
            size_bytes: e.bytes,
            expires_in_ms: until
                .duration_since(now)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        })
    }
    /// Reserve the maximum before native work. Never evict somebody's pending
    /// export or active download to make a new request appear successful.
    pub(super) fn reserve(
        self: &Arc<Self>,
        id: u64,
        stop: Arc<AtomicUsize>,
    ) -> Result<Reservation> {
        self.collect_expired();
        let mut s = self.shared.state.lock().unwrap();
        if s.closed {
            return Err(unavailable());
        }
        if id == 0 || s.rows.contains_key(&id) {
            return Err(Error::input("invalid or duplicate artifact ID"));
        }
        let limits = self.shared.limits;
        if s.usage.entries == limits.entries
            || limits.artifact_bytes > limits.total_bytes - s.usage.bytes
        {
            return Err(Error::new(
                ErrorKind::Busy,
                "artifact capacity is busy; release or wait for existing exports",
            ));
        }
        s.usage.entries += 1;
        s.usage.pending += 1;
        s.usage.bytes += limits.artifact_bytes;
        s.rows.insert(
            id,
            Entry {
                file: None,
                bytes: limits.artifact_bytes,
                expires: None,
                retired: false,
                readers: 0,
                stop,
            },
        );
        Ok(Reservation {
            store: Arc::clone(self),
            id,
            committed: false,
        })
    }
    pub fn open(self: &Arc<Self>, id: u64) -> Result<Download> {
        let now = Instant::now();
        let mut s = self.shared.state.lock().unwrap();
        if s.closed {
            return Err(unavailable());
        }
        if s.usage.readers == self.shared.limits.readers {
            return Err(Error::new(ErrorKind::Busy, "export download limit reached"));
        }
        let e = s
            .rows
            .get_mut(&id)
            .filter(|e| !e.retired && e.file.is_some() && e.expires.is_some_and(|t| now < t))
            .ok_or_else(unavailable)?;
        e.readers += 1;
        let download = Download {
            store: Arc::clone(self),
            id,
            file: Arc::clone(e.file.as_ref().unwrap()),
            size: e.bytes,
            offset: 0,
        };
        s.usage.readers += 1;
        Ok(download)
    }
    /// Explicit result dismissal; active readers are revoked, not silently
    /// detached from accounting. A pending native operation is also stopped.
    pub fn release(&self, id: u64) -> bool {
        let released = self.request_release(id);
        self.collect_expired();
        released
    }
    /// Reactor-safe dismissal. Revoke immediately; the expiry thread owns
    /// descriptor disposal. Ready files remain charged until it collects them.
    pub fn request_release(&self, id: u64) -> bool {
        let mut s = self.shared.state.lock().unwrap();
        let Some(e) = s.rows.get_mut(&id) else {
            return false;
        };
        e.retired = true;
        e.stop.store(1, Ordering::Relaxed);
        self.shared.changed.notify_all();
        true
    }
    /// Call when the owner context logs out/expires/closes. Existing Download
    /// handles cannot continue reading, and pending workers see cancellation.
    /// Join job handles separately to guarantee that their workers are reaped.
    pub fn close(&self) {
        self.request_close();
        self.collect_expired();
    }
    /// Reactor-safe context revocation; no descriptor closes under this call.
    pub fn request_close(&self) {
        let mut s = self.shared.state.lock().unwrap();
        s.closed = true;
        for e in s.rows.values_mut() {
            e.retired = true;
            e.stop.store(1, Ordering::Relaxed);
        }
        self.shared.changed.notify_all();
    }
    fn collect_expired(&self) {
        let retired = {
            let mut s = self.shared.state.lock().unwrap();
            sweep(&mut s, Instant::now())
        };
        drop(retired);
    }
    fn valid(&self, id: u64) -> bool {
        let s = self.shared.state.lock().unwrap();
        !s.closed
            && s.rows
                .get(&id)
                .is_some_and(|e| !e.retired && e.expires.is_some_and(|t| Instant::now() < t))
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        self.close();
        if let Some(t) = self.reaper.get_mut().unwrap().take() {
            let _ = t.join();
        }
    }
}
pub(super) struct Reservation {
    store: Arc<Store>,
    pub id: u64,
    committed: bool,
}
impl Reservation {
    /// The job's cancellation/commit mutex must be held by the caller. Store
    /// revocation and publication are also serialized by the store mutex.
    pub fn commit(mut self, file: File, bytes: u64) -> Result<u64> {
        let m = file.metadata()?;
        if !m.is_file() || m.nlink() != 0 || m.len() != bytes {
            return Err(Error::input(
                "artifact must be an owned, unlinked regular file of the declared size",
            ));
        }
        if bytes > self.store.shared.limits.artifact_bytes {
            return Err(Error::new(
                ErrorKind::Incomplete,
                "export exceeds artifact size limit",
            ));
        }
        let mut s = self.store.shared.state.lock().unwrap();
        if s.closed {
            return Err(Error::new(ErrorKind::Cancelled, "export context closed"));
        }
        let entry = s.rows.get_mut(&self.id).ok_or_else(unavailable)?;
        if entry.retired || entry.stop.load(Ordering::Relaxed) != 0 {
            return Err(Error::new(ErrorKind::Cancelled, "export cancelled"));
        }
        let reserved = entry.bytes;
        entry.file = Some(Arc::new(file));
        entry.bytes = bytes;
        entry.expires = Some(Instant::now() + self.store.shared.limits.ttl);
        s.usage.bytes -= reserved - bytes;
        s.usage.pending -= 1;
        self.committed = true;
        self.store.shared.changed.notify_all();
        Ok(self.id)
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.committed {
            let mut s = self.store.shared.state.lock().unwrap();
            if let Some(entry) = s.rows.remove(&self.id) {
                debug_assert!(entry.file.is_none());
                s.usage.entries -= 1;
                s.usage.pending -= 1;
                s.usage.bytes -= entry.bytes;
            }
        }
    }
}

/// Read one bounded chunk off the HTTP reactor and await transport credit
/// before requesting another. No File::try_clone shared seek offset or Vec of
/// the complete artifact. Expiry/revocation is checked both sides of each read.
pub struct Download {
    store: Arc<Store>,
    id: u64,
    file: Arc<File>,
    size: u64,
    offset: u64,
}
impl Download {
    pub fn is_available(&self) -> bool {
        self.store.valid(self.id)
    }
    pub fn size_bytes(&self) -> u64 {
        self.size
    }
    pub fn read_chunk(&mut self, max: usize) -> Result<Vec<u8>> {
        if !(1..=CHUNK_BYTES).contains(&max) {
            return Err(Error::input("invalid artifact chunk size"));
        }
        if !self.store.valid(self.id) {
            return Err(unavailable());
        }
        let count = (self.size - self.offset).min(max as u64) as usize;
        let mut bytes = vec![0; count];
        let mut used = 0;
        while used < count {
            match self
                .file
                .read_at(&mut bytes[used..], self.offset + used as u64)
            {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    if !self.store.valid(self.id) {
                        return Err(unavailable());
                    }
                }
                Err(e) => return Err(e.into()),
                Ok(0) => return Err(Error::new(ErrorKind::Io, "export artifact became shorter")),
                Ok(n) => used += n,
            }
        }
        if self.file.metadata()?.len() != self.size {
            return Err(Error::new(ErrorKind::Io, "export artifact size changed"));
        }
        if !self.store.valid(self.id) {
            return Err(unavailable());
        }
        self.offset += count as u64;
        Ok(bytes)
    }
}
impl Drop for Download {
    fn drop(&mut self) {
        let mut s = self.store.shared.state.lock().unwrap();
        let entry = s
            .rows
            .get_mut(&self.id)
            .expect("active artifact reader is charged");
        entry.readers -= 1;
        s.usage.readers -= 1;
        let retired = sweep(&mut s, Instant::now());
        self.store.shared.changed.notify_all();
        drop(s);
        drop(retired);
    }
}

#[cfg(test)]
mod tests;
