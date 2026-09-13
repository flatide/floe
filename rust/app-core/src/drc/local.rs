//! Trusted CLI selection only. The web registration path supplies explicit
//! pack/sidecar handles within its own scope; it does not accept paths from UI.
use super::{Ascii, Database, Pack, MAGIC};
use crate::{cache, Error, ErrorKind, Result};
use sha1::{Digest, Sha1};
use std::{
    env,
    fs::{self, OpenOptions},
    io::Read,
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::AtomicUsize,
};

pub fn reviewer_tag(explicit: Option<&str>) -> String {
    let configured = explicit
        .map(str::to_owned)
        .or_else(|| env::var("FLOE_REVIEWER").ok())
        .unwrap_or_default();
    let display = env::var("DISPLAY").unwrap_or_default();
    let host = display.rsplit_once(':').map(|(h, _)| h).unwrap_or("");
    let ssh = env::var("SSH_CONNECTION")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| env::var("SSH_CLIENT").ok())
        .unwrap_or_default();
    let tag = if !configured.trim().is_empty() {
        configured.trim().to_owned()
    } else if !host.is_empty()
        && !host.starts_with('/')
        && !["localhost", "127.0.0.1", "::1", "unix"].contains(&host)
    {
        host.to_owned()
    } else if let Some(ip) = ssh.split_whitespace().next() {
        ip.to_owned()
    } else {
        ["LOGNAME", "USER", "LNAME", "USERNAME"]
            .into_iter()
            .find_map(|k| env::var(k).ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(account_name)
    };
    let s: String = tag
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "user".into()
    } else {
        s
    }
}
fn account_name() -> String {
    // SAFETY: getuid has no pointer arguments; getpwuid_r only writes into
    // this live bounded buffer/result, whose returned string is copied here.
    unsafe {
        let uid = libc::getuid();
        let mut buf = vec![0u8; 16384];
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut result = std::ptr::null_mut();
        if libc::getpwuid_r(
            uid,
            &mut pw,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        ) == 0
            && !result.is_null()
            && !pw.pw_name.is_null()
        {
            return std::ffi::CStr::from_ptr(pw.pw_name)
                .to_string_lossy()
                .into_owned();
        }
        format!("uid{uid}")
    }
}
pub fn waive_paths(pack: &Path, reviewer: &str) -> Result<[PathBuf; 2]> {
    if reviewer.is_empty()
        || reviewer.len() > 200
        || !reviewer
            .chars()
            .all(|c| c.is_alphanumeric() || "._-".contains(c))
    {
        return Err(Error::input("invalid or oversized reviewer tag"));
    }
    let path = cache::absolute(pack)?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::input("DRC filename must be UTF-8"))?;
    let name = name.strip_suffix(".ice").unwrap_or(name);
    // Identical legacy path tag, not a security fingerprint or user credential.
    let tag = format!("{:x}", Sha1::digest(path.as_os_str().as_bytes()));
    Ok([
        path.parent()
            .unwrap()
            .join(format!(".{name}.waive.{reviewer}")),
        env::temp_dir().join(format!(".{name}.waive.{reviewer}-{}", &tag[..12])),
    ])
}
pub fn open_current(
    source: &Path,
    reviewer: Option<&str>,
    cancelled: &AtomicUsize,
) -> Result<Database> {
    crate::check_cancelled(cancelled)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(source)?;
    if !file.metadata()?.is_file() {
        return Err(Error::input("DRC source must be a regular file"));
    }
    let mut magic = [0; 8];
    let packed = file.read_exact(&mut magic).is_ok() && &magic == MAGIC;
    let mut pack = if packed {
        Pack::open(source, cancelled)?
    } else {
        let path = PathBuf::from(format!("{}.ice", cache::utf8(source)?));
        let candidate = match fs::metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Database::ascii(Ascii::open(source, cancelled)?, None));
            }
            Err(e) => Err(e.into()),
            Ok(_) => Pack::open(&path, cancelled).and_then(|p| {
                if p.source_matches(source)? {
                    Ok(p)
                } else {
                    Err(Error::new(ErrorKind::Cache, "stale DRC pack"))
                }
            }),
        };
        match candidate {
            Ok(p) => p,
            Err(e) if e.kind == ErrorKind::Cancelled => return Err(e),
            Err(e) => {
                let warning = format!(
                    "{}: {e}; parsing ASCII instead (integral-DBU sources can be indexed explicitly: floe-index drc {})",
                    path.display(),
                    source.display()
                );
                return Ok(Database::ascii(
                    Ascii::open(source, cancelled)?,
                    Some(warning),
                ));
            }
        }
    };
    for path in waive_paths(&pack.path, &reviewer_tag(reviewer))? {
        match fs::metadata(&path) {
            Ok(_) => {
                pack.attach_waives(&path)?;
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    pack.unchanged()?;
    Ok(Database::packed(pack))
}
