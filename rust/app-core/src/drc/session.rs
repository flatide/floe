//! One reviewer's review of one DRC result is open in one session at a time
//! (user 2026-11-10: a launcher starts everyone's viewer under one account,
//! and a viewer given `--userid` is a process of its own - the same reviewer
//! started twice, or two people left with one derived id, wrote one waive
//! sidecar from two sessions: each kept its own waived counters in memory
//! and wrote them over the other's, so the file's counters no longer matched
//! its statuses).
//!
//! The session holds the sidecar's lock for as long as its review is open:
//! floe_vfs::lock's kernel lock (released when the process ends, however it
//! ends - nothing stale to clear) on `.floe-lock/<sidecar>.session.build`
//! beside the sidecar, with who holds it written inside. A second session -
//! another process, host or account - is refused and told who has it.
//! Handles of one process share the hold (a viewer opens the next review of
//! a database before it lets go of the last). A lock that cannot be taken
//! for another reason (no lock support on the mount, a lock folder this
//! account may not write) is said and the review goes on unlocked, as
//! before 0.12.440; `FLOE_REVIEW_SESSION_LOCK=off` (and `FLOE_LOCK=off`)
//! take none.
use crate::{Error, ErrorKind, Result};
use floe_vfs::lock::{self, Busy, Key, Mode, WriterGuard};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// A session's hold on a waive sidecar; dropped with the last review of
/// this process that has it open.
pub struct Session {
    _guard: WriterGuard,
}

fn sessions() -> &'static Mutex<HashMap<PathBuf, Weak<Session>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<PathBuf, Weak<Session>>>> = OnceLock::new();
    SESSIONS.get_or_init(Default::default)
}

/// Whether a review takes its session lock (`FLOE_REVIEW_SESSION_LOCK=off`:
/// no - a second session of a reviewer is opened as before 0.12.440).
pub fn enabled() -> bool {
    enabled_of(std::env::var("FLOE_REVIEW_SESSION_LOCK").ok().as_deref())
}

fn enabled_of(value: Option<&str>) -> bool {
    !matches!(
        value.unwrap_or_default().trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "no"
    )
}

/// The lock of the sidecar at `sidecar` (its real path): beside it, named by
/// it.
fn key(sidecar: &Path, reviewer: &str, database: &str) -> Key {
    let folder = match sidecar.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = sidecar
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Key {
        dir: folder.join(lock::DIR),
        name: format!("{}.session", name.trim_start_matches('.')),
        subject: format!("{reviewer}'s review of {database}"),
    }
}

/// Hold the session of the waive sidecar `sidecar` (it exists) for
/// `reviewer`'s review of `database`, as `what` (the lock's holder line).
/// Ok(None): no lock is taken here - `notice` says why when it is worth
/// saying. Refused (ErrorKind::Busy, with who holds it) when another process
/// has the review open.
pub fn hold(
    sidecar: &Path,
    reviewer: &str,
    database: &str,
    what: &str,
    notices: &mut Vec<String>,
) -> Result<Option<Arc<Session>>> {
    if !enabled() {
        return Ok(None);
    }
    let real = std::fs::canonicalize(sidecar).unwrap_or_else(|_| sidecar.to_path_buf());
    let mut held = sessions().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(session) = held.get(&real).and_then(Weak::upgrade) {
        return Ok(Some(session));
    }
    held.retain(|_, s| s.strong_count() > 0);
    match lock::writer(&key(&real, reviewer, database), Mode::Additive, what) {
        Ok(guard) => {
            let session = Arc::new(Session { _guard: guard });
            held.insert(real, Arc::downgrade(&session));
            Ok(Some(session))
        }
        Err(Busy::Indexing { subject, holder, .. }) => Err(Error::new(
            ErrorKind::Busy,
            format!(
                "{subject} is already open in another session: {} - close it there first, or start with another reviewer ID (--userid)",
                holder.map_or_else(|| "another process".to_string(), |h| h.to_string())
            ),
        )),
        Err(other) => {
            notices.push(format!(
                "[drc] the review's session lock could not be taken ({other}); a second session of this reviewer is not refused"
            ));
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_is_named_by_the_sidecar_and_the_switch_reads_off() {
        let k = key(Path::new("/r/run/.results.db.waive.kim"), "kim", "results.db");
        assert_eq!(k.dir, Path::new("/r/run/.floe-lock"));
        assert_eq!(k.name, "results.db.waive.kim.session");
        assert_eq!(k.subject, "kim's review of results.db");
        // the temp folder's sidecar (a results folder that cannot be written)
        let k = key(Path::new("/tmp/.results.db.waive.kim-0123456789ab"), "kim", "results.db");
        assert_eq!(k.name, "results.db.waive.kim-0123456789ab.session");
        assert!(enabled_of(None) && enabled_of(Some("")) && enabled_of(Some("on")));
        assert!(!enabled_of(Some("off")) && !enabled_of(Some(" OFF ")) && !enabled_of(Some("0")));
    }

    #[test]
    fn handles_of_one_process_share_the_session() {
        let dir = std::env::temp_dir().join(format!("floe-session-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sidecar = dir.join(".x.db.waive.kim");
        std::fs::write(&sidecar, b"x").unwrap();
        let mut notices = Vec::new();
        let a = hold(&sidecar, "kim", "x.db", "test", &mut notices).unwrap();
        let b = hold(&sidecar, "kim", "x.db", "test", &mut notices).unwrap();
        // (FLOE_LOCK=off or the switch in this environment: nothing to compare)
        if let (Some(a), Some(b)) = (&a, &b) {
            assert!(Arc::ptr_eq(a, b));
            // the holder is written where a refused session reads it
            let text = std::fs::read_to_string(dir.join(".floe-lock/x.db.waive.kim.session.build")).unwrap();
            assert!(text.contains(&format!("pid={}", std::process::id())), "{text}");
            assert!(text.contains("what=test"), "{text}");
        }
        assert!(notices.is_empty(), "{notices:?}");
        drop((a, b));
        // let go with the last handle: the next hold is a new session
        assert!(sessions().lock().unwrap().values().all(|s| s.strong_count() == 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
