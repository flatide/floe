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

#[derive(Debug)]
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
        let stdout = child.stdout.take().expect("version stdout");
        let reader = match std::thread::Builder::new()
            .name("floe-index-version".into())
            .spawn(move || {
                let mut bytes = Vec::new();
                stdout.take(4097).read_to_end(&mut bytes).map(|_| bytes)
            }) {
            Ok(t) => t,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.into());
            }
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        let result = loop {
            if cancelled.load(Ordering::Relaxed) != 0 {
                break Err(Error::new(
                    ErrorKind::Cancelled,
                    "indexer validation cancelled",
                ));
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    break if status.success() {
                        Ok(())
                    } else {
                        Err(Error::new(
                            ErrorKind::Version,
                            "floe-index --version failed",
                        ))
                    }
                }
                Err(e) => break Err(e.into()),
                _ if Instant::now() >= deadline => {
                    break Err(Error::new(
                        ErrorKind::Version,
                        "floe-index --version timed out",
                    ))
                }
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let bytes = reader
            .join()
            .map_err(|_| Error::new(ErrorKind::Worker, "version reader failed"))??;
        result?;
        if bytes.len() > 4096 {
            return Err(Error::new(
                ErrorKind::Version,
                "oversized indexer version response",
            ));
        }
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
    pub(crate) fn spawn(&self, args: &[OsString]) -> Result<Child> {
        cache::utf8(&self.binary)?;
        // Native progress/profile JSON stream directly to the caller's file
        // descriptors. No output() accumulation, no Python/shell fallback.
        Ok(Command::new(&self.binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
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
