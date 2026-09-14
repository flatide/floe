//! One bounded, single-use server-side edit per view. HTTP can prepare a large
//! layer selection without reflecting it through the 8 KiB control channel.
use floe_app_core::view::{Patch, Snapshot, ViewController};
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct PreparedEdits {
    serial: u64,
    ready: Option<Edit>,
    exhausted: bool,
}
pub(crate) struct Stamp {
    serial: u64,
    base: u64,
    token: String,
}
struct Edit {
    stamp: Stamp,
    patch: Patch,
    fence: Option<crate::drc::revision::Fence>,
}
impl PreparedEdits {
    /// Revocation must also defeat a preparation still awaiting the DRC actor.
    /// Do not reset the serial: an old finish could otherwise match a new begin.
    pub fn invalidate(&mut self) {
        self.ready = None;
        match self.serial.checked_add(1) {
            Some(next) => self.serial = next,
            None => self.exhausted = true,
        }
    }
    pub fn begin(&mut self, base: u64) -> Result<Stamp, &'static str> {
        let serial = self.serial.checked_add(1).ok_or("prepared_edit_limit")?;
        let token = crate::auth::public_id().map_err(|_| "prepared_edit_unavailable")?;
        self.serial = serial;
        self.ready = None;
        Ok(Stamp {
            serial,
            base,
            token,
        })
    }
    pub fn finish(&mut self, stamp: Stamp, patch: Patch) -> Result<String, &'static str> {
        self.finish_with(stamp, patch, None)
    }
    pub fn finish_drc(
        &mut self,
        stamp: Stamp,
        patch: Patch,
        fence: crate::drc::revision::Fence,
    ) -> Result<String, &'static str> {
        self.finish_with(stamp, patch, Some(fence))
    }
    fn finish_with(
        &mut self,
        stamp: Stamp,
        patch: Patch,
        fence: Option<crate::drc::revision::Fence>,
    ) -> Result<String, &'static str> {
        if self.exhausted || self.serial != stamp.serial {
            return Err("prepared_edit_expired");
        }
        let token = stamp.token.clone();
        self.ready = Some(Edit {
            stamp,
            patch,
            fence,
        });
        Ok(token)
    }
    fn take(&mut self, token: &str, base: u64) -> Result<Patch, &'static str> {
        let edit = self.ready.as_ref().ok_or("prepared_edit_expired")?;
        if token != edit.stamp.token {
            return Err("prepared_edit_expired");
        }
        let edit = self.ready.take().unwrap();
        // A recognized token is consumed on an attempt, including a conflict.
        // Retrying a stale focus must re-read the current authoritative view.
        if base != edit.stamp.base {
            return Err("stale_state");
        }
        Ok(edit.patch)
    }
    fn apply(
        &mut self,
        token: &str,
        base: u64,
        controller: &ViewController,
    ) -> Result<Snapshot, &'static str> {
        controller.edit(base, self.take(token, base)?).map_err(|e| {
            if e.kind == floe_app_core::ErrorKind::Busy {
                "stale_state"
            } else {
                crate::view::safe_error(e.kind)
            }
        })
    }
    pub fn apply_current(
        plans: &Mutex<Self>,
        token: &str,
        base: u64,
        controller: &ViewController,
    ) -> Result<Snapshot, &'static str> {
        let fence = plans
            .lock()
            .unwrap()
            .ready
            .as_ref()
            .filter(|edit| edit.stamp.token == token)
            .and_then(|edit| edit.fence.clone());
        let apply = || plans.lock().unwrap().apply(token, base, controller);
        // Recheck token under the prepared lock after acquiring revision. This
        // preserves revision -> prepared -> controller order and defeats a
        // concurrent begin/invalidate without keeping any guard across await.
        match fence {
            Some(f) => f.with_current(apply),
            None => apply(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revocation_cancels_ready_and_inflight_preparations_even_at_exhaustion() {
        let mut p = PreparedEdits::default();
        let pending = p.begin(1).unwrap();
        p.invalidate();
        assert!(p.finish(pending, Patch::default()).is_err());
        let stamp = p.begin(1).unwrap();
        let token = p.finish(stamp, Patch::default()).unwrap();
        p.invalidate();
        assert!(p.take(&token, 1).is_err());
        p.serial = u64::MAX - 1;
        let pending = p.begin(1).unwrap();
        p.invalidate();
        assert!(p.finish(pending, Patch::default()).is_err());
        assert!(p.begin(1).is_err());
    }
    #[test]
    fn newest_preparation_wins_and_tokens_are_scoped_single_use() {
        let mut plans = PreparedEdits::default();
        let first = plans.begin(1).unwrap();
        let second = plans.begin(1).unwrap();
        let token = plans.finish(second, Patch::default()).unwrap();
        assert!(plans.finish(first, Patch::default()).is_err());
        assert!(PreparedEdits::default().take(&token, 1).is_err());
        assert!(plans.take("wrong", 1).is_err());
        plans.take(&token, 1).unwrap();
        assert!(plans.take(&token, 1).is_err());
        let stamp = plans.begin(2).unwrap();
        let token = plans.finish(stamp, Patch::default()).unwrap();
        assert_eq!(plans.take(&token, 3).unwrap_err(), "stale_state");
        assert!(plans.take(&token, 2).is_err());
        let stamp = plans.begin(4).unwrap();
        let token = plans.finish(stamp, Patch::default()).unwrap();
        let _cancelled = plans.begin(4).unwrap();
        assert!(plans.take(&token, 4).is_err());
        plans.serial = u64::MAX;
        assert!(plans.begin(5).is_err());
    }
}
