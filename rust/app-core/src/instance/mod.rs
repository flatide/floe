//! Trusted local launcher rendezvous, not a browser/network authorization API.
//! One cooperating owner holds an inode-stable flock for (product, uid, display).
//! A failed/uncertain forward is never permission to start a replacement owner.
//! The handler must enqueue bounded work, not synchronously index/render a file.
mod unix;
mod wire;

use crate::{check_cancelled, Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    collections::VecDeque,
    fs::{self, DirBuilder, File, OpenOptions},
    io,
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

pub const PROTOCOL: u32 = 1;
pub const BODY_BYTES: usize = 64 * 1024;
pub const REPLY_BYTES: usize = 16 * 1024;
const HISTORY: usize = 32;
const POLL: Duration = Duration::from_millis(10);
const DEADLINE: Duration = Duration::from_secs(3);

/// GTK's screen-suffix normalization, not host-name/DISPLAY authorization.
pub fn normalize_display(display: &str) -> String {
    let text = display.trim();
    let (host, number) = text.rsplit_once(':').unwrap_or(("", text));
    format!("{host}:{}", number.split('.').next().unwrap_or(""))
}
#[derive(Clone, Debug)]
pub struct Key(String);
impl Key {
    pub fn new(product: &str, display: Option<&str>) -> Result<Self> {
        if product.is_empty()
            || product.len() > 32
            || !product
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            return Err(Error::input("invalid instance product"));
        }
        if display.is_some_and(|s| s.len() > 4096 || s.as_bytes().contains(&0)) {
            return Err(Error::input("invalid instance display"));
        }
        let display = display
            .filter(|s| !s.is_empty())
            .map(normalize_display)
            .unwrap_or_else(|| {
                if cfg!(target_os = "macos") {
                    "aqua"
                } else {
                    "headless"
                }
                .to_owned()
            });
        // JSON framing prevents ambiguous concatenations. SHA-1 only names the
        // socket, never authenticates a client or grants a shared web identity.
        let name = serde_json::to_vec(&(product, unix::uid(), display)).unwrap();
        Ok(Self(format!("{:x}", Sha1::digest(name))))
    }
}

#[derive(Clone)]
pub struct Endpoint {
    directory: PathBuf,
    directory_id: (u64, u64),
    socket: PathBuf,
    lock: PathBuf,
}
impl Endpoint {
    /// `base` is supplied only by the local launcher. No existing permissions
    /// are repaired; a hostile/symlink/group-writable entry fails closed.
    pub fn in_directory(base: &Path, key: &Key) -> Result<Self> {
        let base = fs::canonicalize(base)?;
        let m = fs::metadata(&base)?;
        if !m.is_dir()
            || (m.uid() != unix::uid() && m.uid() != 0)
            || (m.mode() & 0o022 != 0 && m.mode() & 0o1000 == 0)
        {
            return Err(Error::input("unsafe instance parent directory"));
        }
        let directory = base.join(format!("floe-launch-{}", unix::uid()));
        match DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
        let m = fs::symlink_metadata(&directory)?;
        if !m.is_dir() || m.uid() != unix::uid() || m.mode() & 0o077 != 0 {
            return Err(Error::input("instance directory must be private and owned"));
        }
        let socket = directory.join(format!("{}.sock", key.0));
        SockAddr::unix(&socket).map_err(|_| Error::input("instance socket path is too long"))?;
        Ok(Self {
            directory_id: identity(&m),
            lock: directory.join(format!("{}.lock", key.0)),
            directory,
            socket,
        })
    }
    fn validate(&self) -> Result<()> {
        let m = fs::symlink_metadata(&self.directory)?;
        if !m.is_dir()
            || identity(&m) != self.directory_id
            || m.uid() != unix::uid()
            || m.mode() & 0o077 != 0
        {
            return Err(Error::input("instance directory changed"));
        }
        Ok(())
    }
    /// A locked owner can be starting, busy, or shutting down. Never remove its
    /// socket on a timeout and never use a persisted PID as authority to kill.
    pub fn claim(&self, build: &str) -> Result<Claim> {
        valid_build(build)?;
        self.validate()?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.lock)?;
        let m = lock.metadata()?;
        if !m.is_file()
            || m.uid() != unix::uid()
            || m.mode() & 0o077 != 0
            || m.nlink() != 1
            || m.len() != 0
        {
            return Err(Error::input("unsafe instance lock file"));
        }
        if !unix::try_lock(&lock)? {
            return Ok(Claim::Running(self.clone()));
        }
        self.validate()?;
        if identity(&fs::symlink_metadata(&self.lock)?) != identity(&m) {
            return Err(Error::input("instance lock changed"));
        }
        let mut random = [0u8; 32];
        getrandom::fill(&mut random)
            .map_err(|_| Error::new(ErrorKind::Io, "instance entropy unavailable"))?;
        let epoch = random.iter().map(|b| format!("{b:02x}")).collect();
        match fs::symlink_metadata(&self.socket) {
            Ok(m) => {
                if !m.file_type().is_socket() || m.uid() != unix::uid() {
                    return Err(Error::input("foreign instance socket entry"));
                }
                // Only ECONNREFUSED proves that an existing socket is stale.
                // Timeout/permission errors do not authorize unlinking it.
                match connect_socket(&self.socket, Duration::from_millis(100)) {
                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => (),
                    _ => {
                        return Err(Error::new(
                            ErrorKind::Busy,
                            "live or unverifiable instance socket",
                        ))
                    }
                }
                self.validate()?;
                if identity(&fs::symlink_metadata(&self.socket)?) != identity(&m) {
                    return Err(Error::input("instance socket changed"));
                }
                fs::remove_file(&self.socket)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let listener = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        listener.bind(&SockAddr::unix(&self.socket)?)?;
        let socket_id = identity(&fs::symlink_metadata(&self.socket)?);
        let owner = Owner {
            endpoint: self.clone(),
            socket_id,
            _lock: lock,
            listener,
            epoch,
            build: build.to_owned(),
        };
        fs::set_permissions(
            &owner.endpoint.socket,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )?;
        owner.listener.listen(8)?;
        owner.listener.set_nonblocking(true)?;
        Ok(Claim::Owner(owner))
    }
    pub fn connect(&self, build: &str, stop: &AtomicUsize) -> Result<Connection> {
        self.connect_until(build, stop, Instant::now() + DEADLINE)
    }
    fn connect_until(
        &self,
        build: &str,
        stop: &AtomicUsize,
        deadline: Instant,
    ) -> Result<Connection> {
        valid_build(build)?;
        loop {
            check_cancelled(stop)?;
            self.validate()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::new(
                    ErrorKind::Busy,
                    "existing instance did not become ready",
                ));
            }
            match connect_socket(&self.socket, remaining.min(Duration::from_millis(50))) {
                Ok(socket) => {
                    unix::same_user(&socket)?;
                    let hello: Hello = wire::read(&socket, deadline, stop)?;
                    if hello.protocol != PROTOCOL || hello.build != build {
                        return Err(Error::new(
                            ErrorKind::Version,
                            "existing instance has a different build/protocol",
                        ));
                    }
                    if !epoch_valid(&hello.epoch) || counter(&hello.next).is_none() {
                        return Err(Error::input("invalid instance handshake"));
                    }
                    return Ok(Connection { socket, hello });
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::NotFound
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                    ) =>
                {
                    thread::sleep(POLL)
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}
fn identity(m: &fs::Metadata) -> (u64, u64) {
    (m.dev(), m.ino())
}
fn valid_build(build: &str) -> Result<()> {
    if build.is_empty()
        || build.len() > 128
        || !build
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
    {
        Err(Error::input("invalid instance build identity"))
    } else {
        Ok(())
    }
}
fn connect_socket(path: &Path, timeout: Duration) -> io::Result<Socket> {
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    socket.connect_timeout(&SockAddr::unix(path)?, timeout)?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}
pub enum Claim {
    Owner(Owner),
    Running(Endpoint),
}
pub struct Owner {
    endpoint: Endpoint,
    socket_id: (u64, u64),
    _lock: File,
    listener: Socket,
    epoch: String,
    build: String,
}
impl Owner {
    /// The application callback must not start work after observing stop. Its
    /// reply can be a queued-operation receipt; `Handled` is NOT a render ACK.
    pub fn serve(
        self,
        stop: &AtomicUsize,
        mut handle: impl FnMut(&Value, &AtomicUsize) -> Result<Value>,
    ) -> Result<()> {
        let mut ledger = Ledger::default();
        while stop.load(Ordering::Relaxed) == 0 {
            self.endpoint.validate()?;
            match self.listener.accept() {
                Ok((socket, _)) => {
                    if unix::same_user(&socket).is_err() {
                        continue;
                    }
                    socket.set_nonblocking(true)?;
                    let deadline = Instant::now() + DEADLINE;
                    let hello = Hello {
                        protocol: PROTOCOL,
                        epoch: self.epoch.clone(),
                        build: self.build.clone(),
                        next: ledger
                            .reserve()
                            .ok_or_else(|| {
                                Error::new(ErrorKind::Busy, "instance sequence exhausted")
                            })?
                            .to_string(),
                    };
                    if wire::write(&socket, &hello, deadline, stop).is_err() {
                        continue;
                    }
                    let request: Request = match wire::read(&socket, deadline, stop) {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    let result = ledger.dispatch(&self.epoch, request, stop, &mut handle);
                    // Keep the receipt even if the client disconnects before ACK.
                    let _ = wire::write(&socket, &result, Instant::now() + DEADLINE, stop);
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) =>
                {
                    thread::sleep(POLL)
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        // Keep the lock inode forever: unlinking an unlocked lock can create
        // two different inodes owned by two concurrent cooperating launchers.
        if self.endpoint.validate().is_ok()
            && fs::symlink_metadata(&self.endpoint.socket)
                .is_ok_and(|m| m.file_type().is_socket() && identity(&m) == self.socket_id)
        {
            let _ = fs::remove_file(&self.endpoint.socket);
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    protocol: u32,
    epoch: String,
    build: String,
    next: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    epoch: String,
    seq: String,
    body: Value,
}
/// Keep this intent after a transport failure. Recover with a new connection
/// to the same epoch/sequence, never by silently issuing a fresh intent.
#[derive(Clone)]
pub struct Intent(Request);
pub struct Connection {
    socket: Socket,
    hello: Hello,
}
impl Connection {
    pub fn intent(&self, body: Value) -> Result<Intent> {
        wire::encode(&body, BODY_BYTES)?;
        Ok(Intent(Request {
            epoch: self.hello.epoch.clone(),
            seq: self.hello.next.clone(),
            body,
        }))
    }
    /// Any I/O failure after this call starts is uncertain; a handler may
    /// already have run. Neither a retry with a fresh seq nor a new owner is safe.
    pub fn submit(self, intent: &Intent, stop: &AtomicUsize) -> Result<Outcome> {
        if intent.0.epoch != self.hello.epoch {
            return Ok(Outcome::Rejected {
                code: Reject::OwnerChanged,
            });
        }
        let attempt = || {
            let deadline = Instant::now() + DEADLINE;
            wire::write(&self.socket, &intent.0, deadline, stop)?;
            let reply: Reply = wire::read(&self.socket, deadline, stop)?;
            if reply.epoch != intent.0.epoch || reply.seq != intent.0.seq {
                return Err(Error::input("invalid instance receipt"));
            }
            if let Outcome::Handled { reply: body } = &reply.outcome {
                wire::encode(body, REPLY_BYTES)?;
            }
            Ok(reply.outcome)
        };
        attempt().map_err(|_| {
            Error::new(
                ErrorKind::Incomplete,
                "launch outcome is unknown; recover the same intent, do not start another instance",
            )
        })
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reject {
    OwnerChanged,
    Sequence,
    Conflict,
    Expired,
    BodyLimit,
    Closing,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    InvalidInput,
    Unsupported,
    Io,
    Cache,
    Busy,
    Version,
    Worker,
    Cancelled,
    Incomplete,
    ReplyLimit,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Handled { reply: Value },
    Failed { code: Failure },
    Rejected { code: Reject },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    epoch: String,
    seq: String,
    outcome: Outcome,
}
#[derive(Default)]
struct Ledger {
    issued: u64,
    history: VecDeque<Reservation>,
}
struct Reservation {
    seq: u64,
    result: Option<(Vec<u8>, Outcome)>,
}
impl Ledger {
    fn reserve(&mut self) -> Option<u64> {
        // Even a disconnected handshake consumes a unique ticket. Reusing its
        // number could mistake a second identical user action for its replay.
        self.issued = self.issued.checked_add(1)?;
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(Reservation {
            seq: self.issued,
            result: None,
        });
        Some(self.issued)
    }
    fn dispatch(
        &mut self,
        epoch: &str,
        request: Request,
        stop: &AtomicUsize,
        handle: &mut impl FnMut(&Value, &AtomicUsize) -> Result<Value>,
    ) -> Reply {
        let reject = |code| Outcome::Rejected { code };
        let outcome = (|| {
            if request.epoch != epoch {
                return reject(Reject::OwnerChanged);
            }
            let Some(seq) = counter(&request.seq) else {
                return reject(Reject::Sequence);
            };
            let signature = match wire::encode(&request.body, BODY_BYTES) {
                Ok(bytes) => bytes,
                Err(_) => return reject(Reject::BodyLimit),
            };
            if seq > self.issued {
                return reject(Reject::Sequence);
            }
            let Some(reservation) = self.history.iter_mut().find(|r| r.seq == seq) else {
                return reject(Reject::Expired);
            };
            if let Some((old, result)) = &reservation.result {
                return if *old == signature {
                    result.clone()
                } else {
                    reject(Reject::Conflict)
                };
            }
            if stop.load(Ordering::Relaxed) != 0 {
                return reject(Reject::Closing);
            }
            let result = match handle(&request.body, stop) {
                Ok(reply) if wire::encode(&reply, REPLY_BYTES).is_ok() => {
                    Outcome::Handled { reply }
                }
                Ok(_) => Outcome::Failed {
                    code: Failure::ReplyLimit,
                },
                Err(e) => Outcome::Failed {
                    code: match e.kind {
                        ErrorKind::InvalidInput => Failure::InvalidInput,
                        ErrorKind::Unsupported => Failure::Unsupported,
                        ErrorKind::Io => Failure::Io,
                        ErrorKind::Cache => Failure::Cache,
                        ErrorKind::Busy => Failure::Busy,
                        ErrorKind::Version => Failure::Version,
                        ErrorKind::Worker => Failure::Worker,
                        ErrorKind::Cancelled => Failure::Cancelled,
                        ErrorKind::Incomplete => Failure::Incomplete,
                    },
                },
            };
            reservation.result = Some((signature, result.clone()));
            result
        })();
        Reply {
            epoch: epoch.to_owned(),
            seq: request.seq,
            outcome,
        }
    }
}
fn epoch_valid(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn counter(text: &str) -> Option<u64> {
    text.parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == text)
}

#[cfg(test)]
mod tests;
