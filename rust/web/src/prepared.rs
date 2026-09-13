//! One bounded, single-use server-side edit per view. HTTP can prepare a large
//! layer selection without reflecting it through the 8 KiB control channel.
use floe_app_core::view::{Patch, Snapshot, ViewController};

#[derive(Default)]
pub(crate) struct PreparedEdits {
    serial: u64,
    ready: Option<Edit>,
}
pub(crate) struct Stamp {
    serial: u64,
    base: u64,
    token: String,
}
struct Edit {
    stamp: Stamp,
    patch: Patch,
}
impl PreparedEdits {
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
        if self.serial != stamp.serial {
            return Err("prepared_edit_expired");
        }
        let token = stamp.token.clone();
        self.ready = Some(Edit { stamp, patch });
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
    pub fn apply(
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
