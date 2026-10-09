use super::*;
use std::{
    ffi::{CStr, CString},
    fs::{self, Metadata, OpenOptions},
    io::{BufRead, BufReader},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};

#[derive(PartialEq, Eq)]
pub(super) struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl From<Metadata> for Stamp {
    fn from(m: Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            size: m.len(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        }
    }
}
pub(super) fn check_destination(out: &Path) -> Result<()> {
    for part in out.components().filter_map(|c| c.as_os_str().to_str()) {
        if part.ends_with(".floe") || part.ends_with(".ice") || part.ends_with(".index.lock") {
            return Err(Error::input(
                "SVRF output must be outside source caches/locks",
            ));
        }
    }
    match fs::symlink_metadata(out) {
        Ok(m) if !m.is_file() || m.nlink() != 1 => Err(Error::input(
            "SVRF output cannot be a symlink, hardlink or nonregular file",
        )),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

// Python text iteration's CR, LF and CRLF handling, including a final line
// without a newline. No read_line unbounded allocation and no FIFO blocking.
fn line(
    reader: &mut impl BufRead,
    skip_lf: &mut bool,
    total: &mut usize,
    maximum: usize,
    stop: &AtomicUsize,
) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        check_cancelled(stop)?;
        let buf = match reader.fill_buf() {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            r => r?,
        };
        if buf.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                decoded(&bytes).map(Some)
            };
        }
        if *skip_lf {
            *skip_lf = false;
            if buf[0] == b'\n' {
                *total += 1;
                bounded(*total, maximum, "expanded input exceeds 256 MiB")?;
                reader.consume(1);
                continue;
            }
        }
        let end = buf.iter().position(|b| b"\r\n".contains(b));
        let count = end.unwrap_or(buf.len());
        let consumed = count + usize::from(end.is_some());
        *total = total
            .checked_add(consumed)
            .ok_or_else(|| limit("input byte count overflow"))?;
        bounded(*total, maximum, "expanded input exceeds 256 MiB")?;
        bounded(
            bytes.len() + count,
            MAX_TEXT,
            "physical line exceeds 64 KiB",
        )?;
        bytes.extend_from_slice(&buf[..count]);
        *skip_lf = end.is_some_and(|i| buf[i] == b'\r');
        reader.consume(consumed);
        if end.is_some() {
            return decoded(&bytes).map(Some);
        }
    }
}
fn decoded(bytes: &[u8]) -> Result<String> {
    // Same errors=replace policy as the Python subset parser; valid Unicode
    // descriptions and paths are never indexed with ASCII byte boundaries.
    let s = String::from_utf8_lossy(bytes);
    bounded(s.len(), MAX_TEXT, "decoded line exceeds 64 KiB")?;
    Ok(s.into_owned())
}

impl Parser<'_> {
    fn protect(&mut self, path: &Path) -> Result<()> {
        let path = path.to_owned();
        if !self.d.protected.contains(&path) {
            self.text(cache::utf8(&path)?)?;
            bounded(
                self.d.protected.len() + 1,
                MAX_ENTRIES,
                "too many include search candidates",
            )?;
            self.d.protected.insert(path);
        }
        Ok(())
    }
    pub(super) fn feed_file(&mut self, path: &Path, chain: &mut Vec<PathBuf>) -> Result<()> {
        check_cancelled(self.stop)?;
        self.protect(path)?;
        let real = match fs::canonicalize(path) {
            Ok(p) => p,
            Err(e) if chain.is_empty() => return Err(e.into()),
            Err(e) => return self.warn(format!("INCLUDE unreadable: {} ({e})", path.display())),
        };
        if chain.contains(&real) {
            return self.warn(format!("INCLUDE cycle: {}", path.display()));
        }
        bounded(
            chain.len() + 1,
            self.limits.depth,
            "INCLUDE depth exceeds 64",
        )?;
        bounded(
            self.d.stat("files") + 1,
            self.limits.files,
            "file visits exceed 4096",
        )?;
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(f) => f,
            Err(e) if chain.is_empty() => return Err(e.into()),
            Err(e) => return self.warn(format!("INCLUDE unreadable: {} ({e})", path.display())),
        };
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(Error::input("SVRF input must be a regular file"));
        }
        if meta.len() > self.limits.input as u64 {
            return Err(limit("input file exceeds 256 MiB"));
        }
        let stamp = Stamp::from(meta);
        if self.d.inputs.get(path).is_some_and(|old| old != &stamp) {
            return Err(limit("input changed between INCLUDE visits"));
        }
        self.protect(&real)?;
        self.d.inc("files");
        let mut reader = BufReader::with_capacity(65536, file);
        let mut skip_lf = false;
        chain.push(real);
        while let Some(s) = line(
            &mut reader,
            &mut skip_lf,
            &mut self.input_bytes,
            self.limits.input,
            self.stop,
        )? {
            self.d.inc("lines");
            self.feed_line(&s, path, chain)?;
        }
        chain.pop();
        if Stamp::from(reader.get_ref().metadata()?) != stamp
            || Stamp::from(fs::metadata(path)?) != stamp
        {
            return Err(limit("input changed during conversion"));
        }
        self.d.inputs.insert(path.into(), stamp);
        let basename = path.file_name().unwrap_or_default().to_string_lossy();
        if let Some(name) = &self.cur {
            self.warn(format!("unclosed block {name} at end of {basename}"))?;
        }
        if self.macro_depth > 0 || self.macro_pending {
            self.warn(format!(
                "unclosed DMACRO body at end of {basename} (brace drift?)"
            ))?;
        }
        if self.verbatim_depth > 0 {
            self.warn(format!(
                "unclosed VERBATIM/Tcl block at end of {basename} (brace drift?)"
            ))?;
            self.verbatim_depth = 0;
        }
        if self.in_comment {
            self.warn(format!("unclosed /* comment at end of {basename}"))?;
            self.in_comment = false;
        }
        if !self.cond.is_empty() && chain.is_empty() {
            self.warn("unbalanced #IFDEF at end of deck".into())?;
        }
        Ok(())
    }
    fn feed_line(&mut self, raw: &str, path: &Path, chain: &mut Vec<PathBuf>) -> Result<()> {
        let mut s = trim(raw).to_owned();
        if s.is_empty() {
            return Ok(());
        }
        if self.in_comment {
            let Some((_, rest)) = s.split_once("*/") else {
                return Ok(());
            };
            s = trim(rest).into();
            self.in_comment = false;
        }
        while !s.starts_with('@') {
            let Some(i) = s.find("/*") else {
                break;
            };
            if let Some(j) = s[i + 2..].find("*/") {
                s = trim(&format!("{} {}", &s[..i], &s[i + 2 + j + 2..])).into();
            } else {
                s = s[..i].trim_end_matches(whitespace).into();
                self.in_comment = true;
                break;
            }
        }
        if s.is_empty() {
            return Ok(());
        }
        if s.starts_with('#') {
            return self.directive(&s);
        }
        if !self.active() {
            return Ok(());
        }
        if !s.starts_with('@') {
            s = trim(s.split_once("//").map_or(s.as_str(), |(a, _)| a)).into();
            if s.is_empty() {
                return Ok(());
            }
            s = self.substitution.apply(&s)?;
        }
        if head_rest(&s).0.eq_ignore_ascii_case("INCLUDE") {
            let target = trim(head_rest(&s).1).trim_matches(['\'', '"']);
            let verbatim = self.verbatim_depth > 0;
            if verbatim {
                if !target.is_empty() && !self.d.verbatim_includes.iter().any(|s| s == target) {
                    self.text(target)?;
                    self.d.verbatim_includes.push(target.into());
                }
                if target.is_empty() || !(self.options.scan_all || self.options.follow_verbatim) {
                    return Ok(());
                }
            } else if self.cur.is_some() || self.macro_depth > 0 || self.macro_pending {
                let context = if self.macro_depth > 0 || self.macro_pending {
                    "a DMACRO body".into()
                } else {
                    format!("open block {}", self.cur.as_ref().unwrap())
                };
                self.warn(format!("INCLUDE swallowed by {context}: {s}"))?;
                return self.statement(&s);
            }
            let expanded = self.expand(target)?;
            if let Some(inc) = self.find_include(&expanded, path)? {
                self.d.includes.push(inc.clone());
                let saved = self.verbatim_depth;
                if verbatim {
                    self.verbatim_depth = 0;
                }
                self.feed_file(&inc, chain)?;
                if verbatim {
                    self.verbatim_depth = saved;
                }
            } else {
                let mut message = format!(
                    "INCLUDE{} not found: {target}",
                    if verbatim { " (in VERBATIM)" } else { "" }
                );
                if expanded != target {
                    message.push_str(&format!(" -> {expanded}"));
                }
                if expanded.contains('$') {
                    message.push_str(" (env var unset in this shell?)");
                }
                self.warn(message)?;
            }
            return Ok(());
        }
        self.statement(&s)
    }
    fn find_include(&mut self, target: &str, source: &Path) -> Result<Option<PathBuf>> {
        if target.is_empty() {
            return Ok(None);
        }
        let target = Path::new(target);
        if target.is_absolute() {
            self.protect(target)?;
            return Ok(target.is_file().then(|| target.to_owned()));
        }
        // Keep lexical paths (including ../) in diagnostic/provenance output.
        // Canonical paths are used only for cycles and output protection.
        let first = source.parent().unwrap_or(Path::new("")).join(target);
        self.protect(&first)?;
        if first.is_file() {
            return Ok(Some(first));
        }
        for dir in &self.options.include_dirs {
            let candidate = dir.join(target);
            self.protect(&candidate)?;
            if candidate.is_file() {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }
    fn expand(&mut self, target: &str) -> Result<String> {
        let mut out = String::new();
        let mut pos = 0;
        while pos < target.len() {
            let c = target[pos..].chars().next().unwrap();
            let (end, value) = if c == '$' {
                let rest = &target[pos + 1..];
                let found = if let Some(rest) = rest.strip_prefix('{') {
                    rest.find('}').map(|n| (&rest[..n], pos + n + 3))
                } else {
                    let n = rest
                        .bytes()
                        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                        .count();
                    (n > 0).then(|| (&rest[..n], pos + n + 1))
                };
                if let Some((name, end)) = found {
                    (
                        end,
                        (self.env)(name).unwrap_or_else(|| target[pos..end].into()),
                    )
                } else {
                    (pos + 1, "$".into())
                }
            } else {
                (pos + c.len_utf8(), c.to_string())
            };
            bounded(
                out.len() + value.len(),
                MAX_TEXT,
                "expanded INCLUDE path exceeds 64 KiB",
            )?;
            out.push_str(&value);
            pos = end;
        }
        if out.starts_with('~') {
            let end = out.find('/').unwrap_or(out.len());
            let home = if end == 1 {
                (self.env)("HOME").or_else(|| account_home(None))
            } else {
                account_home(Some(&out[1..end]))
            };
            if let Some(home) = home {
                bounded(
                    home.len() + out.len() - end,
                    MAX_TEXT,
                    "expanded home path exceeds 64 KiB",
                )?;
                out = format!("{}{}", home.trim_end_matches('/'), &out[end..]);
                if out.is_empty() {
                    out = "/".into();
                }
            }
        }
        self.text(&out)?;
        Ok(out)
    }
}
fn account_home(name: Option<&str>) -> Option<String> {
    let name = name.map(CString::new).transpose().ok()?;
    // SAFETY: reentrant passwd lookup only writes to this bounded live buffer
    // and passwd/result. Copy pw_dir before the backing allocation is dropped.
    unsafe {
        let mut buf = vec![0u8; 16384];
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut result = std::ptr::null_mut();
        let status = if let Some(name) = name {
            libc::getpwnam_r(
                name.as_ptr(),
                &mut pw,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut result,
            )
        } else {
            libc::getpwuid_r(
                libc::getuid(),
                &mut pw,
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut result,
            )
        };
        if status != 0 || result.is_null() || pw.pw_dir.is_null() {
            None
        } else {
            Some(CStr::from_ptr(pw.pw_dir).to_string_lossy().into_owned())
        }
    }
}
