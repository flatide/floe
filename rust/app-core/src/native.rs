use crate::{cache, Error, ErrorKind, Result};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub const INDEX_VERSION: &str = env!("FLOE_INDEX_VERSION");

/// Trusted local discovery input. A future gateway never accepts these paths
/// or the search PATH from a browser.
pub struct Discovery {
    pub override_path: Option<PathBuf>,
    pub development_root: Option<PathBuf>,
    pub executable: PathBuf,
    pub search_path: Option<OsString>,
}
impl Discovery {
    pub fn local() -> Result<Self> {
        let executable = std::env::current_exe()?;
        // Discover a checkout from the running executable, not an arbitrary
        // current working directory nor the build machine's absolute path.
        let development_root = executable
            .ancestors()
            .nth(4)
            .filter(|root| executable.starts_with(root.join("rust/target")))
            .map(Path::to_owned);
        Ok(Self {
            override_path: std::env::var_os("FLOE_INDEX_BIN").map(PathBuf::from),
            development_root,
            executable,
            search_path: std::env::var_os("PATH"),
        })
    }
    pub fn renderer() -> Result<Self> {
        let mut d = Self::local()?;
        d.override_path = std::env::var_os("FLOE_RENDERD_BIN").map(PathBuf::from);
        Ok(d)
    }
    pub fn renderd_path(&self) -> Result<PathBuf> {
        self.find("floe-renderd", "FLOE_RENDERD_BIN")
    }
    fn find(&self, name: &str, variable: &str) -> Result<PathBuf> {
        if let Some(p) = &self.override_path {
            if p.as_os_str().is_empty() || !executable(p) {
                return Err(Error::input(format!(
                    "{variable} is set but is not an executable file: {}",
                    p.display()
                )));
            }
            return Ok(fs::canonicalize(p)?);
        }
        let mut candidates = Vec::new();
        if let Some(root) = &self.development_root {
            candidates.push(root.join("rust/target/release").join(name));
        }
        if let Some(parent) = self.executable.parent() {
            candidates.push(parent.join(name));
        }
        if let Some(path) = &self.search_path {
            candidates.extend(std::env::split_paths(path).map(|p| p.join(name)));
        }
        let p = candidates.into_iter().find(|p| executable(p)).ok_or_else(|| Error::input(
            format!("{name} not found; set {variable}, build the release binary, or install it beside floe2-web/on PATH")
        ))?;
        Ok(fs::canonicalize(p)?)
    }
}
fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[derive(Clone, Debug)]
pub struct Indexer {
    binary: PathBuf,
}
impl Indexer {
    pub fn discover(d: &Discovery) -> Result<Self> {
        Ok(Self {
            binary: d.find("floe-index", "FLOE_INDEX_BIN")?,
        })
    }
    pub fn path(&self) -> &Path {
        &self.binary
    }

    pub fn verify(&self, cancelled: &AtomicUsize) -> Result<()> {
        let mut child = Command::new(&self.binary)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        let mut stdout = child.stdout.take().expect("version stdout");
        let mut bytes = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        // Leader exit is not pipe EOF: a wrapper may leave stdout inherited by
        // a descendant. Bound both waits without joining a blocked reader.
        let result = (|| -> Result<()> {
            crate::index_progress::nonblocking(&stdout)?;
            let mut status = None;
            let mut eof = false;
            loop {
                if cancelled.load(Ordering::Relaxed) != 0 {
                    return Err(Error::new(
                        ErrorKind::Cancelled,
                        "indexer validation cancelled",
                    ));
                }
                if !eof {
                    let mut block = [0u8; 4097];
                    match stdout.read(&mut block[..4097 - bytes.len()]) {
                        Ok(0) => eof = true,
                        Ok(n) => bytes.extend_from_slice(&block[..n]),
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                            ) => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                if bytes.len() > 4096 {
                    return Err(Error::new(
                        ErrorKind::Version,
                        "oversized indexer version response",
                    ));
                }
                if status.is_none() {
                    status = child.try_wait()?;
                }
                if let Some(status) = status {
                    if !status.success() {
                        return Err(Error::new(
                            ErrorKind::Version,
                            "floe-index --version failed",
                        ));
                    }
                    if eof {
                        return Ok(());
                    }
                }
                if Instant::now() >= deadline {
                    return Err(Error::new(
                        ErrorKind::Version,
                        "floe-index --version timed out",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })();
        drop(stdout);
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| Error::new(ErrorKind::Version, "invalid indexer version UTF-8"))?;
        let words: Vec<_> = text.split_whitespace().collect();
        if words.first() != Some(&"floe-index") || words.get(1) != Some(&INDEX_VERSION) {
            return Err(Error::new(
                ErrorKind::Version,
                format!("expected floe-index {INDEX_VERSION}; rebuild the matched indexer"),
            ));
        }
        Ok(())
    }
    pub(crate) fn spawn(&self, args: &[OsString], capture: bool) -> Result<Child> {
        cache::utf8(&self.binary)?;
        // CLI output still streams to its descriptors. Managed jobs use two
        // bounded nonblocking pipes, never output() or a shell/Python fallback.
        Ok(Command::new(&self.binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(if capture {
                Stdio::piped()
            } else {
                Stdio::inherit()
            })
            .stderr(if capture {
                Stdio::piped()
            } else {
                Stdio::inherit()
            })
            .process_group(0)
            .spawn()?)
    }
}

pub(crate) fn signal_child(child: &Child, signal: i32) -> Result<()> {
    let pid = i32::try_from(child.id()).map_err(|_| Error::input("child PID out of range"))?;
    // SAFETY: pid is positive and belongs to this still-owned, unreaped Child.
    // It cannot be recycled until wait/reap; never signal a caller-supplied PID
    // or a negative process-group ID. No handler runs Rust allocation code.
    if unsafe { libc::kill(pid, signal) } == -1 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ESRCH) {
            return Err(e.into());
        }
    }
    Ok(())
}
