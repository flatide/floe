//! A short in-memory commit fence. Never hold this lock during native I/O or
//! across await. Reader replacement keeps its separate registry identity lock.
use super::Failure;
use std::sync::{Arc, Mutex};

struct State {
    token: String,
    changing: bool,
    closed: bool,
}
pub(crate) struct Revision(Arc<Mutex<State>>);
#[derive(Clone)]
pub(crate) struct Fence {
    state: Arc<Mutex<State>>,
    token: String,
}
pub(super) struct Change(Arc<Mutex<State>>);
impl Revision {
    pub(super) fn new(token: String) -> Self {
        Self(Arc::new(Mutex::new(State {
            token,
            changing: false,
            closed: false,
        })))
    }
    pub(super) fn snapshot(&self) -> (String, bool) {
        let s = self.0.lock().unwrap();
        (s.token.clone(), s.changing)
    }
    pub(super) fn capture(&self, expected: &str) -> Result<Fence, Failure> {
        let fence = Fence {
            state: Arc::clone(&self.0),
            token: expected.into(),
        };
        fence.with_current(|| Ok(()))?;
        Ok(fence)
    }
    pub(super) fn begin(&self) -> Result<Change, Failure> {
        self.begin_at(None)
    }
    pub(super) fn begin_at(&self, expected: Option<&str>) -> Result<Change, Failure> {
        let next = crate::auth::public_id().map_err(|_| "drc_read_error")?;
        let mut s = self.0.lock().unwrap();
        if s.closed {
            return Err("drc_closed");
        }
        if s.changing {
            return Err("drc_busy");
        }
        if expected.is_some_and(|v| v != s.token) {
            return Err("drc_context_changed");
        }
        s.token = next;
        s.changing = true;
        Ok(Change(Arc::clone(&self.0)))
    }
    pub(super) fn owns(&self, change: &Change) -> bool {
        Arc::ptr_eq(&self.0, &change.0)
    }
    /// A fully prepared in-memory replacement. A failed precommit leaves even
    /// the old revision intact; no native I/O or reentrant revision lookup in f.
    pub(super) fn replace_at<T>(
        &self,
        expected: &str,
        next: String,
        f: impl FnOnce() -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        let mut s = self.0.lock().unwrap();
        if s.closed || s.changing || s.token != expected {
            return Err("drc_context_changed");
        }
        let value = f()?;
        s.token = next;
        Ok(value)
    }
    pub(super) fn close(&self) {
        self.0.lock().unwrap().closed = true;
    }
}
impl Fence {
    pub(crate) fn with_current<T>(
        &self,
        f: impl FnOnce() -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        let s = self.state.lock().unwrap();
        if s.closed || s.changing || s.token != self.token {
            return Err("drc_context_changed");
        }
        f()
    }
}
impl Drop for Change {
    fn drop(&mut self) {
        // Cancellation/failure still retires the old token. An uncertain ACK
        // must not revive old filter cursors or prepared actions.
        self.0.lock().unwrap().changing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prepared_replacement_only_retires_revision_after_success() {
        let r = Revision::new("old".into());
        let old = r.capture("old").unwrap();
        assert_eq!(
            r.replace_at("old", "bad".into(), || Err::<(), _>("drc_cancelled")),
            Err("drc_cancelled")
        );
        old.with_current(|| Ok(())).unwrap();
        assert_eq!(r.snapshot().0, "old");
        assert_eq!(
            r.replace_at("stale", "bad".into(), || panic!("stale commit")),
            Err::<(), _>("drc_context_changed")
        );
        r.replace_at("old", "new".into(), || Ok(())).unwrap();
        assert_eq!(old.with_current(|| Ok(())), Err("drc_context_changed"));
        r.capture("new").unwrap();
        let change = r.begin().unwrap();
        assert_eq!(
            r.replace_at(&r.snapshot().0, "bad".into(), || panic!("busy commit")),
            Err::<(), _>("drc_context_changed")
        );
        drop(change);
        r.close();
        assert_eq!(
            r.replace_at(&r.snapshot().0, "bad".into(), || panic!("closed commit")),
            Err::<(), _>("drc_context_changed")
        );
    }
    #[test]
    fn changing_cancelled_and_closed_revisions_never_revive() {
        let r = Revision::new("a".repeat(64));
        let old = r.capture(&r.snapshot().0).unwrap();
        old.with_current(|| {
            assert!(r.0.try_lock().is_err());
            Ok(())
        })
        .unwrap();
        let change = r.begin().unwrap();
        let next = r.snapshot().0;
        assert_ne!(next, old.token);
        assert_eq!(old.with_current(|| Ok(())), Err("drc_context_changed"));
        assert!(r.capture(&next).is_err());
        assert!(matches!(r.begin(), Err("drc_busy")));
        drop(change);
        let fresh = r.capture(&next).unwrap();
        fresh.with_current(|| Ok(())).unwrap();
        assert!(old.with_current(|| Ok(())).is_err());
        let change = r.begin().unwrap();
        r.close();
        drop(change);
        assert!(r.capture(&r.snapshot().0).is_err());
        assert!(matches!(r.begin(), Err("drc_closed")));
    }
}
