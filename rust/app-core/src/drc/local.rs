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

/// How a review's reviewer id was found (`reviewer`'s order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewerSource {
    /// `--userid` / `--floe-reviewer`, or a caller's own
    Given,
    /// FLOE_REVIEWER (the viewer's `--userid` sets it for its service)
    Environment,
    /// the DISPLAY's host - everyone shown from that host shares it
    Display,
    /// the ssh client's address
    Ssh,
    /// the account - shared where a launcher runs every user's viewer
    Account,
}
impl ReviewerSource {
    /// As the viewer and the refusal say it.
    pub fn text(self) -> &'static str {
        match self {
            Self::Given => "given",
            Self::Environment => "FLOE_REVIEWER",
            Self::Display => "DISPLAY host",
            Self::Ssh => "ssh client",
            Self::Account => "account",
        }
    }
    /// Said by the user or the launcher, not guessed from where the viewer
    /// runs.
    pub fn stated(self) -> bool {
        matches!(self, Self::Given | Self::Environment)
    }
}

/// A review's reviewer: the tag its sidecars are named by, and how it was
/// found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reviewer {
    pub tag: String,
    pub source: ReviewerSource,
}

/// The reviewer of this process's DRC review: `explicit`, else
/// FLOE_REVIEWER, else the DISPLAY's host (not a local one), else the ssh
/// client, else the account - a stable name where nothing was said, but one
/// several people share when a launcher runs the viewer for them (an ETX
/// node's display host, a service account; user 2026-11-10: TeeBox). The
/// tag keeps letters, digits, `.`, `_` and `-`; anything else becomes `_`.
pub fn reviewer(explicit: Option<&str>) -> Reviewer {
    // an id given - even a blank one - keeps FLOE_REVIEWER out, as it always
    // did; a blank one then falls to where the viewer runs
    let (configured, source) = match explicit {
        Some(given) => (given.to_owned(), ReviewerSource::Given),
        None => (
            env::var("FLOE_REVIEWER").unwrap_or_default(),
            ReviewerSource::Environment,
        ),
    };
    let display = env::var("DISPLAY").unwrap_or_default();
    // no colon: the whole value is the host, as floe/drc.py read it
    let host = display
        .rsplit_once(':')
        .map_or(display.as_str(), |(h, _)| h);
    let ssh = env::var("SSH_CONNECTION")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| env::var("SSH_CLIENT").ok())
        .unwrap_or_default();
    let (tag, source) = if !configured.trim().is_empty() {
        (configured.trim().to_owned(), source)
    } else if !host.is_empty()
        && !host.starts_with('/')
        && !["localhost", "127.0.0.1", "::1", "unix"].contains(&host)
    {
        (host.to_owned(), ReviewerSource::Display)
    } else if let Some(ip) = ssh.split_whitespace().next() {
        (ip.to_owned(), ReviewerSource::Ssh)
    } else {
        (
            ["LOGNAME", "USER", "LNAME", "USERNAME"]
                .into_iter()
                .find_map(|k| env::var(k).ok().filter(|s| !s.is_empty()))
                .unwrap_or_else(account_name),
            ReviewerSource::Account,
        )
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
    Reviewer {
        tag: if s.is_empty() { "user".into() } else { s },
        source,
    }
}

/// `reviewer`'s tag.
pub fn reviewer_tag(explicit: Option<&str>) -> String {
    reviewer(explicit).tag
}

/// FLOE_REVIEWER_REQUIRED (`1`, `on`, `yes`, `true`): a DRC review, and a
/// command that reads a reviewer's waives, needs a stated reviewer id -
/// `--userid` / `--floe-reviewer` or FLOE_REVIEWER (user 2026-11-10: a
/// launcher that runs everyone's viewer under one account on shared display
/// hosts; without an id two people's reviews are one file).
pub fn reviewer_required() -> bool {
    required_of(env::var("FLOE_REVIEWER_REQUIRED").ok().as_deref())
}

fn required_of(value: Option<&str>) -> bool {
    matches!(
        value.unwrap_or_default().trim().to_ascii_lowercase().as_str(),
        "1" | "on" | "yes" | "true"
    )
}

/// `reviewer`, refused when a stated id is required here and none was given
/// (`reviewer_required`): the refusal says what the id would have been and
/// why that is not one person's.
pub fn required_reviewer(explicit: Option<&str>) -> Result<Reviewer> {
    let found = reviewer(explicit);
    if reviewer_required() && !found.source.stated() {
        return Err(unstated(&found));
    }
    Ok(found)
}

fn unstated(found: &Reviewer) -> Error {
    Error::new(
        ErrorKind::Admission,
        format!(
            "a reviewer ID is required for DRC review here (FLOE_REVIEWER_REQUIRED): start the viewer with --userid ID, or give --floe-reviewer ID or FLOE_REVIEWER - without one the review would be kept as \"{}\" (the {}), which is not one person's",
            found.tag,
            found.source.text()
        ),
    )
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
/// Where a review file of another result is moved: `<path>.stale-<epoch>`,
/// the first such name not taken. Two files moved in one second must not
/// share a name - the rename would replace the review kept first - so a
/// taken name gives way to the next number; the name stays `stale-<digits>`,
/// what the statistics skip as a leftover (drc::team::stats).
pub(crate) fn aside_path(path: &Path) -> PathBuf {
    let mut stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    loop {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".stale-{stamp}"));
        let candidate = PathBuf::from(name);
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
        stamp += 1;
    }
}
/// The pack's reader lock (floe_vfs::lock, kind Pack): refused - as
/// ErrorKind::Busy, with who - while a run re-packs it.
pub(crate) fn pack_reader(path: &Path) -> Result<floe_vfs::lock::ReaderGuard> {
    let key = floe_vfs::lock::key(floe_vfs::lock::Kind::Pack, &path.to_string_lossy());
    floe_vfs::lock::readers(
        &[key],
        &floe_vfs::lock::reader_label(&format!("{} drc", crate::program())),
    )
    .map_err(|busy| Error::new(ErrorKind::Busy, busy.to_string()))
}
fn current_pack(source: &Path, cancelled: &AtomicUsize) -> Result<(Option<Pack>, Option<String>)> {
    crate::check_cancelled(cancelled)?;
    let packed = is_packed_source(source)?;
    let pack = if packed {
        let reader = pack_reader(source)?;
        let mut pack = Pack::open(source, cancelled)?;
        pack.hold(reader);
        pack
    } else {
        let path = cache::pack_path(source)?;
        let candidate = match fs::metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // no pack: one being made whole is said, not parsed around
                let key = floe_vfs::lock::key(floe_vfs::lock::Kind::Pack, &path.to_string_lossy());
                if let Some(busy) = floe_vfs::lock::opening_refusal(&key) {
                    return Err(Error::new(ErrorKind::Busy, busy.to_string()));
                }
                return Ok((None, None));
            }
            Err(e) => Err(e.into()),
            Ok(_) => pack_reader(&path)
                .and_then(|reader| {
                    let mut p = Pack::open(&path, cancelled)?;
                    p.hold(reader);
                    Ok(p)
                })
                .and_then(|p| {
                    // (the ASCII file would be read whole otherwise: a
                    // source whose time changed is compared whatever its size)
                    if p.source_current(source, cancelled)? {
                        Ok(p)
                    } else {
                        Err(Error::new(ErrorKind::Cache, "stale DRC pack"))
                    }
                }),
        };
        match candidate {
            Ok(p) => p,
            Err(e) if matches!(e.kind, ErrorKind::Cancelled | ErrorKind::Busy) => return Err(e),
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
/// The current pack of `source` (a .db or its pack), held as a reader, for a
/// command that must not answer from the ASCII file - the review statistics:
/// no pack, or one of an older .db, is an error that says how to make it
/// (a pack being made is busy, as for every reader).
pub fn open_pack(source: &Path, cancelled: &AtomicUsize) -> Result<Pack> {
    crate::check_cancelled(cancelled)?;
    let source = cache::absolute(source)?;
    if is_packed_source(&source)? {
        let reader = pack_reader(&source)?;
        let mut pack = Pack::open(&source, cancelled)?;
        pack.hold(reader);
        return Ok(pack);
    }
    let path = cache::pack_path(&source)?;
    let build = format!(
        "{} drc {} --build (or floe-index drc)",
        crate::program(),
        source.display()
    );
    match fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = floe_vfs::lock::key(floe_vfs::lock::Kind::Pack, &path.to_string_lossy());
            if let Some(busy) = floe_vfs::lock::opening_refusal(&key) {
                return Err(Error::new(ErrorKind::Busy, busy.to_string()));
            }
            return Err(Error::new(
                ErrorKind::Cache,
                format!(
                    "{}: no DRC pack - the review statistics need it; make it first: {build}",
                    source.display()
                ),
            ));
        }
        Err(e) => return Err(e.into()),
        Ok(_) => (),
    }
    let reader = pack_reader(&path)?;
    let mut pack = Pack::open(&path, cancelled)?;
    pack.hold(reader);
    if !pack.source_current(&source, cancelled)? {
        return Err(Error::new(
            ErrorKind::Cache,
            format!(
                "{}: stale DRC pack - {} is not the file that was indexed (its size or its content changed). Index it again: {} drc {} --build --force (or floe-index drc)",
                path.display(),
                source.display(),
                crate::program(),
                source.display()
            ),
        ));
    }
    Ok(pack)
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
    for path in waive_paths(&pack.path, &required_reviewer(reviewer)?.tag)? {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stated_id_is_required_only_where_the_switch_says() {
        for on in ["1", "on", "ON", " yes ", "true"] {
            assert!(required_of(Some(on)), "{on}");
        }
        for off in ["", "0", "off", "no", "false", "maybe"] {
            assert!(!required_of(Some(off)), "{off}");
        }
        assert!(!required_of(None));
        // the refusal says what the id would have been and whose it is not
        let e = unstated(&Reviewer {
            tag: "etx01".into(),
            source: ReviewerSource::Display,
        });
        assert!(e.kind == ErrorKind::Admission);
        assert!(
            e.message.contains("FLOE_REVIEWER_REQUIRED")
                && e.message.contains("\"etx01\" (the DISPLAY host)")
                && e.message.contains("--userid"),
            "{}",
            e.message
        );
    }
}
