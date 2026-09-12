//! M1a process client, not an HTTP API. Unix (Linux/macOS) only for now.
//! Blocking waits belong on the application's worker/control thread, never
//! the UI/HTTP event loop. Queued I/O stays bounded; poll() consumes payloads.
#![forbid(unsafe_code)]

#[cfg(not(unix))]
compile_error!("floe-worker-client currently supports Linux/macOS only");

mod files;
mod protocol;
use files::{wire_path, Workspace};
use protocol::{parse_line, style_text, Line, MAX_LINE_BYTES};
pub use protocol::{Fields, Fill, FrameFormat, Layers, RenderRequest, Style, ThinPolicy};
use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::{fs::symlink, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const EXPECTED_RENDERD_VERSION: &str = env!("FLOE_RENDERD_VERSION");
const QUEUE_CAP: usize = 8;
const MAX_IN_FLIGHT: usize = 32;
const STDERR_BYTES: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    State,
    Busy,
    Io,
    Protocol,
    Version,
    Worker,
    Exited,
    Timeout,
}
#[derive(Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}
impl Error {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    fn input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }
    fn protocol(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Protocol, message)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new(ErrorKind::Io, e.to_string())
    }
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug)]
pub struct Config {
    /// Explicit local executable, no shell/PATH fallback. Discovery is M1a-2.
    pub binary: PathBuf,
    pub args: Vec<OsString>,
    pub temp_root: PathBuf,
    pub ready_timeout: Duration,
    pub open_timeout: Duration,
    pub style_timeout: Duration,
    pub render_timeout: Duration,
    pub shutdown_grace: Duration,
    pub max_pixels: u64,
    pub max_frame_bytes: usize,
}
impl Config {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            args: Vec::new(),
            temp_root: std::env::temp_dir(),
            ready_timeout: Duration::from_secs(10),
            open_timeout: Duration::from_secs(300),
            style_timeout: Duration::from_secs(10),
            render_timeout: Duration::from_secs(300),
            shutdown_grace: Duration::from_millis(1500),
            max_pixels: 16 * 1024 * 1024,
            max_frame_bytes: 80 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Source {
    Layout(PathBuf),
    Deck(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Opened {
    /// Native database units per micron (inverse of Python meta["dbu"]).
    pub unit: f64,
    pub max_depth: u64,
    pub is_deck: bool,
    pub fields: Fields,
}

#[derive(Debug)]
pub struct Frame {
    pub generation: u64,
    pub round: u64,
    pub final_frame: bool,
    pub partial: bool,
    pub deferred: u64,
    pub labels_truncated: bool,
    pub request: RenderRequest,
    /// Unmodified PNG or FLOERAW1 payload, never a daemon filesystem path.
    pub bytes: Vec<u8>,
    pub fields: Fields,
}
impl Frame {
    pub fn complete(&self) -> bool {
        self.final_frame && !self.partial && self.deferred == 0 && !self.labels_truncated
    }
}

#[derive(Debug)]
pub enum Event {
    Frame(Frame),
    Cancelled {
        generation: u64,
    },
    CancelAcknowledged {
        before_generation: u64,
    },
    Failed {
        generation: Option<u64>,
        code: String,
        message: String,
    },
}

struct Active {
    gen: u64,
    request: RenderRequest,
    last_round: u64,
    deadline: Instant,
}

pub struct WorkerClient {
    config: Config,
    workspace: Workspace,
    child: Option<Child>,
    commands: Option<mpsc::SyncSender<String>>,
    responses: Option<mpsc::Receiver<Result<Line>>>,
    threads: Vec<JoinHandle<()>>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    pub ready: Fields,
    opened: Option<Opened>,
    style_epoch: u64,
    frontier: u64,
    issued: BTreeSet<u64>,
    active: Option<Active>,
}

impl WorkerClient {
    pub fn spawn(config: Config) -> Result<Self> {
        if config.max_pixels == 0
            || config.max_frame_bytes < 16
            || config.max_frame_bytes == usize::MAX
            || config
                .max_pixels
                .checked_mul(4)
                .and_then(|n| usize::try_from(n).ok())
                .is_none()
        {
            return Err(Error::input("invalid frame limits"));
        }
        for timeout in [
            config.ready_timeout,
            config.open_timeout,
            config.style_timeout,
            config.render_timeout,
            config.shutdown_grace,
        ] {
            if timeout.is_zero() || timeout > Duration::from_secs(86400) {
                return Err(Error::input("timeout must be in (0, 24h]"));
            }
        }
        let workspace = Workspace::create(&config.temp_root)?;
        let binary = std::fs::canonicalize(&config.binary)?;
        if !binary.is_file() {
            return Err(Error::input("renderd must be an explicit executable file"));
        }
        let mut child = Command::new(binary)
            .args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let errors = child.stderr.take().unwrap();
        let (command_tx, command_rx) = mpsc::sync_channel::<String>(QUEUE_CAP);
        let (response_tx, response_rx) = mpsc::sync_channel(QUEUE_CAP);
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        // Construct the RAII owner before spawning threads; partial startup
        // failures still kill/reap the child and remove only its private files.
        let mut client = Self {
            config,
            workspace,
            child: Some(child),
            commands: Some(command_tx),
            responses: Some(response_rx),
            threads: vec![],
            stderr: Arc::clone(&stderr),
            ready: Fields(Default::default()),
            opened: None,
            style_epoch: 0,
            frontier: 0,
            issued: BTreeSet::new(),
            active: None,
        };
        let tx = response_tx.clone();
        client.threads.push(
            thread::Builder::new()
                .name("floe-worker-stdin".into())
                .spawn(move || {
                    let mut input = input;
                    for command in command_rx {
                        if let Err(e) = writeln!(input, "{command}").and_then(|_| input.flush()) {
                            let _ = tx.send(Err(e.into()));
                            break;
                        }
                    }
                })?,
        );
        client.threads.push(
            thread::Builder::new()
                .name("floe-worker-stdout".into())
                .spawn(move || {
                    let mut reader = BufReader::new(output);
                    loop {
                        let result = read_line(&mut reader);
                        let failed = result.is_err();
                        if response_tx.send(result).is_err() || failed {
                            break;
                        }
                    }
                })?,
        );
        client.threads.push(
            thread::Builder::new()
                .name("floe-worker-stderr".into())
                .spawn(move || {
                    let mut errors = errors;
                    let mut buf = [0; 4096];
                    while let Ok(n) = errors.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        let mut tail = stderr.lock().unwrap();
                        tail.extend(&buf[..n]);
                        while tail.len() > STDERR_BYTES {
                            tail.pop_front();
                        }
                    }
                })?,
        );
        let ready = client.wait_ack("ready", client.config.ready_timeout)?;
        if ready.required("version")? != EXPECTED_RENDERD_VERSION {
            return Err(Error::new(
                ErrorKind::Version,
                format!(
                    "expected renderd {EXPECTED_RENDERD_VERSION}, got {}",
                    ready.required("version")?
                ),
            ));
        }
        client.ready = ready;
        Ok(client)
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
    pub fn work_dir(&self) -> &Path {
        &self.workspace.0
    }
    pub fn stderr_tail(&self) -> String {
        String::from_utf8_lossy(
            &self
                .stderr
                .lock()
                .unwrap()
                .iter()
                .copied()
                .collect::<Vec<_>>(),
        )
        .into_owned()
    }

    pub fn open(&mut self, source: Source, budget_mb: u64, jobs: u16) -> Result<Opened> {
        if self.child.is_none() {
            return Err(Error::new(ErrorKind::State, "worker closed"));
        }
        if self.opened.is_some() {
            return Err(Error::new(ErrorKind::State, "reopen requires a new worker"));
        }
        if budget_mb == 0
            || budget_mb.checked_mul(1024 * 1024).is_none()
            || !(1..=256).contains(&jobs)
        {
            return Err(Error::input("invalid open budget/jobs"));
        }
        let (kind, path) = match source {
            Source::Layout(p) => ("cache", p),
            Source::Deck(p) => ("deck", p),
        };
        let source_path = std::fs::canonicalize(path)?;
        if (kind == "cache" && !source_path.is_dir()) || (kind == "deck" && !source_path.is_file())
        {
            return Err(Error::input("invalid source kind"));
        }
        let alias = self.workspace.0.join("source");
        symlink(source_path, &alias)?;
        let result = (|| {
            self.send(format!(
                "open {kind}={} budget_mb={budget_mb} jobs={jobs}",
                wire_path(&alias)?
            ))?;
            let fields = self.wait_ack("opened", self.config.open_timeout)?;
            let unit: f64 = fields
                .required("unit")?
                .parse()
                .map_err(|_| Error::protocol("invalid opened unit"))?;
            if !unit.is_finite() || unit <= 0. {
                return Err(Error::protocol("invalid opened unit"));
            }
            let opened = Opened {
                unit,
                max_depth: fields.u64("max_depth")?,
                is_deck: kind == "deck",
                fields,
            };
            self.opened = Some(opened.clone());
            Ok(opened)
        })();
        self.close_on_error(result)
    }

    /// Synchronous style transaction. Cancel/drain a foreground render first;
    /// async live-style orchestration belongs in the ViewService, not here.
    pub fn set_styles(&mut self, styles: &[Style]) -> Result<u64> {
        if self.child.is_none() || self.opened.is_none() || self.active.is_some() {
            return Err(Error::new(
                ErrorKind::State,
                "styles require an idle open worker",
            ));
        }
        let text = style_text(styles)?;
        let epoch = self
            .style_epoch
            .checked_add(1)
            .ok_or_else(|| Error::input("style epoch exhausted"))?;
        let path = self.workspace.write_style(epoch, &text)?;
        let result = (|| {
            self.send(format!("style epoch={epoch} path={}", wire_path(&path)?))?;
            let fields = self.wait_ack("styled", self.config.style_timeout)?;
            if fields.u64("epoch")? != epoch {
                return Err(Error::protocol("style acknowledgement mismatch"));
            }
            self.style_epoch = epoch;
            Ok(epoch)
        })();
        let removed = std::fs::remove_file(path).map_err(Error::from);
        let result = result.and_then(|epoch| removed.map(|_| epoch));
        self.close_on_error(result)
    }

    pub fn render(&mut self, request: RenderRequest) -> Result<u64> {
        if self.opened.is_none() || self.style_epoch == 0 {
            return Err(Error::new(
                ErrorKind::State,
                "render requires open and style acknowledgement",
            ));
        }
        if self.issued.len() >= MAX_IN_FLIGHT {
            return Err(Error::new(
                ErrorKind::Busy,
                "drain worker events before submitting more renders",
            ));
        }
        if self.opened.as_ref().is_some_and(|o| o.is_deck) && request.labels {
            return Err(Error::input("jobdeck labels are not supported"));
        }
        let gen = self
            .frontier
            .checked_add(1)
            .ok_or_else(|| Error::input("generation exhausted"))?;
        let out = self.workspace.output(gen, request.format);
        let command = request.command(
            gen,
            self.style_epoch,
            wire_path(&out)?,
            self.config.max_pixels,
        )?;
        self.send(command)?;
        self.frontier = gen;
        self.issued.insert(gen);
        self.active = Some(Active {
            gen,
            request,
            last_round: 0,
            deadline: Instant::now() + self.config.render_timeout,
        });
        Ok(gen)
    }

    pub fn cancel(&mut self) -> Result<u64> {
        let frontier = self
            .frontier
            .checked_add(1)
            .ok_or_else(|| Error::input("generation exhausted"))?;
        self.send(format!("cancel before_gen={frontier}"))?;
        self.frontier = frontier;
        self.active = None;
        Ok(frontier)
    }

    /// None means the polling interval expired, not that a render completed.
    /// Render's absolute deadline is not reset by partial frames/other events.
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<Event>> {
        let result = self.poll_inner(timeout);
        self.close_on_error(result)
    }

    fn poll_inner(&mut self, timeout: Duration) -> Result<Option<Event>> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| Error::input("poll timeout overflow"))?;
        loop {
            let now = Instant::now();
            if self.active.as_ref().is_some_and(|a| now >= a.deadline) {
                return Err(Error::new(ErrorKind::Timeout, "render deadline exceeded"));
            }
            let until = self
                .active
                .as_ref()
                .map_or(deadline, |a| deadline.min(a.deadline));
            let Some(line) = self.receive(until.saturating_duration_since(now))? else {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|a| Instant::now() >= a.deadline)
                {
                    return Err(Error::new(ErrorKind::Timeout, "render deadline exceeded"));
                }
                return Ok(None);
            };
            if let Some(event) = self.handle(line)? {
                return Ok(Some(event));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }

    fn send(&self, command: String) -> Result<()> {
        if command.len() > MAX_LINE_BYTES || command.contains(['\n', '\r']) {
            return Err(Error::input("invalid command line"));
        }
        self.commands
            .as_ref()
            .ok_or_else(|| Error::new(ErrorKind::State, "worker closed"))?
            .try_send(command)
            .map_err(|e| match e {
                mpsc::TrySendError::Full(_) => {
                    Error::new(ErrorKind::Busy, "worker command queue full")
                }
                mpsc::TrySendError::Disconnected(_) => {
                    Error::new(ErrorKind::Exited, "worker input closed")
                }
            })
    }

    fn receive(&self, timeout: Duration) -> Result<Option<Line>> {
        match self
            .responses
            .as_ref()
            .ok_or_else(|| Error::new(ErrorKind::State, "worker closed"))?
            .recv_timeout(timeout)
        {
            Ok(line) => line.map(Some),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(Error::new(ErrorKind::Exited, "worker output closed"))
            }
        }
    }

    fn wait_ack(&mut self, kind: &str, timeout: Duration) -> Result<Fields> {
        let deadline = Instant::now() + timeout;
        loop {
            let line = self
                .receive(deadline.saturating_duration_since(Instant::now()))?
                .ok_or_else(|| {
                    Error::new(ErrorKind::Timeout, format!("{kind} deadline exceeded"))
                })?;
            if line.kind == kind {
                return Ok(line.fields);
            }
            if line.kind == "error" {
                return Err(Error::new(
                    ErrorKind::Worker,
                    line.fields.required("message")?,
                ));
            }
            // Cancelled renders may still have a committed frame preceding
            // this ack. Consume/delete it, without reviving its display state.
            if matches!(line.kind.as_str(), "frame" | "cancelled" | "dropped") {
                self.handle(line)?;
            } else {
                return Err(Error::protocol(format!(
                    "expected {kind}, got {}",
                    line.kind
                )));
            }
            if Instant::now() >= deadline {
                return Err(Error::new(
                    ErrorKind::Timeout,
                    format!("{kind} deadline exceeded"),
                ));
            }
        }
    }

    fn handle(&mut self, line: Line) -> Result<Option<Event>> {
        let mut f = line.fields;
        match line.kind.as_str() {
            "frame" => {
                let gen = f.u64("gen")?;
                let round = f.u64("round")?;
                if !self.issued.contains(&gen) || round == 0 {
                    return Err(Error::protocol("unissued generation/invalid round"));
                }
                let final_frame = f.flag("final")?;
                let format = match f.required("format")? {
                    "raw" => FrameFormat::Raw,
                    "png" => FrameFormat::Png,
                    _ => return Err(Error::protocol("invalid frame format")),
                };
                let path = self.workspace.frame_path(
                    f.required("png")?,
                    gen,
                    round,
                    final_frame,
                    format,
                )?;
                let current = self.active.as_ref().filter(|a| a.gen == gen);
                let expected =
                    current.map(|a| (a.request.format, a.request.width, a.request.height));
                if let Some(active) = current {
                    if round <= active.last_round
                        || format != active.request.format
                        || f.u64("style_epoch")? != self.style_epoch
                    {
                        return Err(Error::protocol("frame round/format/style mismatch"));
                    }
                }
                let bytes = files::consume(&path, self.config.max_frame_bytes, expected)?;
                if final_frame {
                    self.issued.remove(&gen);
                }
                let Some(bytes) = bytes else { return Ok(None) };
                let partial = f.flag("partial")?;
                let deferred = f.u64("deferred")?;
                let labels_truncated = f.flag("labels_truncated")?;
                let active = self.active.as_mut().unwrap();
                active.last_round = round;
                f.0.remove("png");
                let frame = Frame {
                    generation: gen,
                    round,
                    final_frame,
                    partial,
                    deferred,
                    labels_truncated,
                    request: active.request.clone(),
                    bytes,
                    fields: f,
                };
                if final_frame {
                    self.active = None;
                }
                Ok(Some(Event::Frame(frame)))
            }
            "cancelled" if f.get("before_gen").is_some() => Ok(Some(Event::CancelAcknowledged {
                before_generation: f.u64("before_gen")?,
            })),
            "cancelled" | "dropped" => {
                let gen = f.u64("gen")?;
                self.issued.remove(&gen);
                if self.active.as_ref().is_some_and(|a| a.gen == gen) {
                    self.active = None;
                }
                Ok(Some(Event::Cancelled { generation: gen }))
            }
            "error" => {
                let gen = f.get("gen").map(|_| f.u64("gen")).transpose()?;
                if let Some(gen) = gen {
                    self.issued.remove(&gen);
                }
                if gen.is_none() || self.active.as_ref().is_some_and(|a| Some(a.gen) == gen) {
                    self.active = None;
                }
                Ok(Some(Event::Failed {
                    generation: gen,
                    code: f.required("code")?.into(),
                    message: f.required("message")?.into(),
                }))
            }
            "bye" => Err(Error::new(ErrorKind::Exited, "worker exited")),
            _ => Err(Error::protocol(format!("unexpected {}", line.kind))),
        }
    }

    fn close_on_error<T>(&mut self, result: Result<T>) -> Result<T> {
        if result.is_err() {
            let _ = self.close();
        }
        result
    }

    pub fn close(&mut self) -> Result<()> {
        if let Some(tx) = self.commands.take() {
            let _ = tx.try_send("quit".into());
        }
        // Release blocked channel senders before joining reader/writer threads.
        self.responses.take();
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + self.config.shutdown_grace;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                    _ => {
                        let _ = child.kill();
                        child.wait()?;
                        break;
                    }
                }
            }
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        self.active = None;
        self.issued.clear();
        self.workspace.cleanup()
    }
}
impl Drop for WorkerClient {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn read_line(reader: &mut impl BufRead) -> Result<Line> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Err(Error::new(
                ErrorKind::Exited,
                "daemon EOF before complete line",
            ));
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |i| i + 1);
        if bytes.len() + count > MAX_LINE_BYTES + 1 {
            return Err(Error::protocol("daemon line exceeds limit"));
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            return parse_line(&bytes);
        }
    }
}
