//! Best-effort, allowlisted telemetry, never the native completion oracle.
//! Pipes are drained on the job's control thread in bounded nonblocking turns;
//! neither log volume nor a slow HTTP subscriber creates a queue of strings.
use crate::Result;
use std::{
    io::{self, Read},
    os::fd::AsRawFd,
    process::{Child, ChildStderr, ChildStdout},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NativePhase {
    #[default]
    Starting,
    Reading,
    Parsing,
    Preparing,
    Building,
    Publishing,
    Occupancy,
}
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub phase: NativePhase,
    pub output_bytes: u64,
    pub dropped_lines: u64,
    pub cells: Option<u64>,
    pub total_cells: Option<u64>,
    pub planned_pages: Option<u64>,
    pub encoded_pages: Option<u64>,
    pub drc_checks: Option<u64>,
    pub drc_total_checks: Option<u64>,
    pub drc_errors: Option<u64>,
    pub drc_noninteger: bool,
}
#[derive(Default)]
struct Line {
    bytes: Vec<u8>,
    dropping: bool,
}
impl Line {
    fn feed(&mut self, data: &[u8], progress: &mut Progress, parse: bool) {
        progress.output_bytes = progress.output_bytes.saturating_add(data.len() as u64);
        if !parse {
            return;
        }
        for &b in data {
            if b == b'\n' || b == b'\r' {
                if !self.dropping {
                    self.finish(progress);
                }
                self.bytes.clear();
                self.dropping = false;
            } else if !self.dropping {
                if self.bytes.len() == 4096 {
                    progress.dropped_lines = progress.dropped_lines.saturating_add(1);
                    self.bytes.clear();
                    self.dropping = true;
                } else {
                    self.bytes.push(b);
                }
            }
        }
    }
    fn finish(&self, p: &mut Progress) {
        let Ok(s) = std::str::from_utf8(&self.bytes) else {
            return;
        };
        if s.starts_with("[vfs] reading ") {
            p.phase = NativePhase::Reading;
        } else if s.starts_with("[vfs] parsing...") {
            p.phase = NativePhase::Parsing;
        } else if s.starts_with("[vfs] parsed ") || s.starts_with("[vfs] build: recursive bbox") {
            p.phase = NativePhase::Preparing;
        } else if s.starts_with("[vfs] build: streaming pipeline") {
            p.phase = NativePhase::Building;
        } else if s.starts_with("[vfs] build: pipeline complete") {
            p.phase = NativePhase::Publishing;
        } else if s.starts_with("[occupancy]") {
            p.phase = NativePhase::Occupancy;
        }
        if s.starts_with("[drc-pack w") && s.contains("G scanned, ") {
            p.phase = NativePhase::Parsing;
        }
        if let Some(s) = s.strip_prefix("[drc-pack enc] ") {
            let mut words = s.split_whitespace();
            if let Some((done, total)) = words
                .next()
                .and_then(|s| s.split_once('/'))
                .and_then(|(a, b)| Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?)))
                .filter(|(a, b)| a <= b)
            {
                if words.next() == Some("checks,") {
                    if let Some(errors) = words.next().and_then(|s| s.parse::<u64>().ok()) {
                        if words.next() == Some("errors,") {
                            p.phase = NativePhase::Building;
                            p.drc_checks = Some(done);
                            p.drc_total_checks = Some(total);
                            p.drc_errors = Some(errors);
                        }
                    }
                }
            }
        }
        if s.contains("non-integer coordinate token: --pack") {
            p.drc_noninteger = true;
        }
        if let Some(s) = s.strip_prefix("[vfs] build: pipeline cells ") {
            let mut tokens = s.split_whitespace();
            let counts = tokens
                .next()
                .and_then(|v| v.split_once('/'))
                .and_then(|(a, b)| Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?)));
            if let Some((done, total)) = counts.filter(|(a, b)| a <= b) {
                p.phase = NativePhase::Building;
                p.cells = Some(done);
                p.total_cells = Some(total);
                for token in tokens {
                    if let Some(value) = token.strip_prefix("planned=").and_then(|s| s.parse().ok())
                    {
                        p.planned_pages = Some(value);
                    }
                    if let Some(value) = token.strip_prefix("encoded=").and_then(|s| s.parse().ok())
                    {
                        p.encoded_pages = Some(value);
                    }
                }
            }
        }
    }
}
pub(crate) struct Capture {
    stdout: ChildStdout,
    stderr: ChildStderr,
    line: Line,
    pub progress: Progress,
}
pub(crate) fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    // SAFETY: this descriptor is borrowed from a live, exclusively owned pipe.
    // fcntl reads/sets only its status flags and does not transfer ownership.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
impl Capture {
    pub fn take(child: &mut Child) -> Result<Self> {
        let stdout = child.stdout.take().expect("captured stdout");
        let stderr = child.stderr.take().expect("captured stderr");
        nonblocking(&stdout)?;
        nonblocking(&stderr)?;
        Ok(Self {
            stdout,
            stderr,
            line: Line::default(),
            progress: Progress::default(),
        })
    }
    pub fn drain(&mut self) -> Result<()> {
        drain(&mut self.stdout, &mut self.line, &mut self.progress, false)?;
        drain(&mut self.stderr, &mut self.line, &mut self.progress, true)?;
        Ok(())
    }
    pub fn finish(&mut self) -> Result<()> {
        // Bounded even if a broken native child left a descendant with pipes.
        for _ in 0..8 {
            self.drain()?;
        }
        if !self.line.dropping {
            self.line.finish(&mut self.progress);
        }
        Ok(())
    }
}
fn drain(
    reader: &mut impl Read,
    line: &mut Line,
    progress: &mut Progress,
    parse: bool,
) -> io::Result<()> {
    let mut buf = [0; 8192];
    for _ in 0..8 {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => line.feed(&buf[..n], progress, parse),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn telemetry_is_bounded_incremental_and_never_retains_names_or_paths() {
        let mut line = Line::default();
        let mut p = Progress::default();
        line.feed(b"[vfs] build: pipe", &mut p, true);
        line.feed(
            b"line cells 5/9 pages planned=100 encoded=90 active_cells=2 (5s, rss private)\n",
            &mut p,
            true,
        );
        assert_eq!(
            (p.cells, p.total_cells, p.planned_pages, p.encoded_pages),
            (Some(5), Some(9), Some(100), Some(90))
        );
        assert_eq!(p.phase, NativePhase::Building);
        line.feed(&vec![b'x'; 200_000], &mut p, true);
        assert!(line.bytes.len() <= 4096);
        assert_eq!(p.dropped_lines, 1);
        line.feed(b"\n[vfs] parsing... (private name)\n", &mut p, true);
        assert_eq!(p.phase, NativePhase::Parsing);
        assert!(line.bytes.is_empty());
        line.feed(
            b"[vfs] build: pipeline cells 99/1 pages planned=123\n",
            &mut p,
            true,
        );
        assert_eq!(p.planned_pages, Some(100));
        line.feed(b"\xff\n[vfs] reading /private/a.oas\n", &mut p, false);
        assert_eq!(p.phase, NativePhase::Parsing);
        line.feed(b"[vfs] build: pipeline complete", &mut p, true);
        line.finish(&mut p);
        assert_eq!(p.phase, NativePhase::Publishing);
    }
    #[test]
    fn drc_progress_is_allowlisted_and_partial_worker_scans_are_not_global_counts() {
        let mut line = Line::default();
        let mut p = Progress::default();
        line.feed(b"[drc-pack w0] 1.0G scanned, 200 checks\n", &mut p, true);
        assert_eq!(p.phase, NativePhase::Parsing);
        assert_eq!(p.drc_checks, None);
        line.feed(
            b"[drc-pack enc] 40/120 checks, 8000 errors, 0.1G blob\n",
            &mut p,
            true,
        );
        assert_eq!(
            (p.drc_checks, p.drc_total_checks, p.drc_errors),
            (Some(40), Some(120), Some(8000))
        );
        line.feed(
            b"[drc-pack enc] 50/2 checks, 99 errors, invalid\n",
            &mut p,
            true,
        );
        assert_eq!(p.drc_errors, Some(8000));
        line.feed(
            b"drc private.db: non-integer coordinate token: --pack stores dbu integers\n",
            &mut p,
            true,
        );
        assert!(p.drc_noninteger);
        assert!(line.bytes.is_empty());
    }
}
