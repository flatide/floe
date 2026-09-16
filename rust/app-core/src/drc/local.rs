//! Trusted source selection. CLI reviewer selection is separate from read-only
//! web source selection, whose paths come from approved-root file witnesses.
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
    let path = cache::database_path(pack)?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::input("DRC filename must be UTF-8"))?;
    // Identical legacy path tag, not a security fingerprint or user credential.
    let tag = format!("{:x}", Sha1::digest(path.as_os_str().as_bytes()));
    Ok([
        path.parent()
            .unwrap()
            .join(format!(".{name}.waive.{reviewer}")),
        env::temp_dir().join(format!(".{name}.waive.{reviewer}-{}", &tag[..12])),
    ])
}
/// Trusted launcher selection, not an HTTP path capability. The caller admits
/// pack metadata and checks both source and adjacent cache access scope first.
pub struct ReadSelection {
    pub source: PathBuf,
    pub path: PathBuf,
    pub targets: Option<super::review::store::ReadTargets>,
    pub warning: Option<String>,
}
/// Current adjacent cache selection without reviewer/environment/sidecar lookup.
/// The caller must admit metadata memory and scope-check both possible inputs.
pub struct SourceSelection {
    pub source: PathBuf,
    pub path: PathBuf,
    pub packed: bool,
    pub ignored_cache: bool,
}
pub fn select_current_source(source: &Path, stop: &AtomicUsize) -> Result<SourceSelection> {
    let source = cache::absolute(source)?;
    let (pack, warning) = current_pack(&source, stop)?;
    if let Some(p) = &pack {
        p.unchanged()?;
    }
    Ok(SourceSelection {
        path: pack
            .as_ref()
            .map_or_else(|| source.clone(), |p| p.path.clone()),
        source,
        packed: pack.is_some(),
        ignored_cache: warning.is_some(),
    })
}
pub fn select_review(source: &Path, reviewer: &str, stop: &AtomicUsize) -> Result<ReadSelection> {
    let source = cache::absolute(source)?;
    waive_paths(&source, reviewer)?;
    let (pack, warning) = current_pack(&source, stop)?;
    let path = pack
        .as_ref()
        .map_or_else(|| source.clone(), |p| p.path.clone());
    let targets = if pack.is_some() {
        Some(super::review::store::ReadTargets::select(&path, reviewer)?)
    } else {
        None
    };
    if let Some(pack) = &pack {
        pack.unchanged()?;
    }
    Ok(ReadSelection {
        source,
        path,
        targets,
        warning,
    })
}
/// Bounded, nonblocking type probe for trusted CLI registration. Never parses
/// ASCII, discovers a cache, or creates a review sidecar.
pub fn is_packed_source(source: &Path) -> Result<bool> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(source)?;
    if !file.metadata()?.is_file() {
        return Err(Error::input("DRC source must be a regular file"));
    }
    let mut magic = [0; 8];
    match file.read_exact(&mut magic) {
        Ok(()) => Ok(&magic == MAGIC),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn current_pack(source: &Path, cancelled: &AtomicUsize) -> Result<(Option<Pack>, Option<String>)> {
    crate::check_cancelled(cancelled)?;
    let packed = is_packed_source(source)?;
    let pack = if packed {
        Pack::open(source, cancelled)?
    } else {
        let path = cache::pack_path(source)?;
        let candidate = match fs::metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((None, None));
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
                return Ok((None, Some(warning)));
            }
        }
    };
    Ok((Some(pack), None))
}
pub fn open_current(
    source: &Path,
    reviewer: Option<&str>,
    cancelled: &AtomicUsize,
) -> Result<Database> {
    let (pack, warning) = current_pack(source, cancelled)?;
    let Some(mut pack) = pack else {
        return Ok(Database::ascii(Ascii::open(source, cancelled)?, warning));
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
