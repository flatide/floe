//! Metadata-only replacement: reuse the open database, prepare off-reactor,
//! then publish the snapshot and its read revision in one in-memory commit.
use super::super::{dto::Command, metadata};
use super::*;
use floe_app_core::{browse::SelectedFile, check_cancelled, registered, Error};

pub(crate) struct PreparedRules {
    registry: Arc<Registry>,
    owner: Arc<crate::service::Service>,
    reader: Arc<Service>,
    context: OpenContext,
    next_revision: String,
    snapshot: Option<Arc<metadata::Snapshot>>,
    protection: Option<registered::Registration>,
}
fn fail(code: Failure) -> Error {
    Error::new(ErrorKind::Busy, code)
}
impl Registry {
    pub(crate) fn prepare_rules(
        self: &Arc<Self>,
        owner: Arc<crate::service::Service>,
        selected: SelectedFile,
        context: OpenContext,
        stop: &AtomicUsize,
    ) -> Result<PreparedRules> {
        context.validate().map_err(fail)?;
        let reader = {
            let mut s = self.inner.state.lock().unwrap();
            if s.closed || s.replacing || s.ledger.active().is_some() || !context.matches(&s) {
                return Err(fail("drc_busy_or_context_changed"));
            }
            let reader = s
                .current
                .as_ref()
                .ok_or_else(|| fail("drc_context_changed"))?
                .clone();
            if reader.catalog()["phase"] != "ready" {
                return Err(fail("drc_busy"));
            }
            s.replacing = true;
            reader
        };
        // Install the rollback guard before any fallible preparation.
        let mut pending = PreparedRules {
            registry: self.clone(),
            owner,
            reader,
            context,
            next_revision: String::new(),
            snapshot: None,
            protection: None,
        };
        pending
            .owner
            .with_current(&pending.context.view_id, |v| {
                if v.controller.is_finished() || v.source_id != pending.reader.source_id {
                    Err("drc_context_changed")
                } else {
                    Ok(())
                }
            })
            .map_err(fail)?;
        selected.validate(stop)?;
        let input = metadata::Input {
            path: selected.path().to_owned(),
            scope: selected.scope(),
        };
        let mut protection = pending.owner.source_set().begin(stop)?;
        protection.protect_inputs(
            std::slice::from_ref(&input.path),
            std::slice::from_ref(&input.path),
            stop,
        )?;
        pending.protection = Some(protection);
        let permit = pending
            .reader
            .registration
            .resources
            .drc_metadata(&input.path)?;
        let candidate = metadata::Candidate::load(input, permit, stop)?;
        let mut ticket = pending
            .reader
            .enqueue(Command::PrepareMetadata(candidate.clone()))
            .map_err(fail)?;
        ticket
            .blocking_result(stop, Duration::from_secs(300))
            .map_err(fail)?;
        selected.validate(stop)?;
        pending.snapshot = Some(candidate.lock().unwrap().take()?);
        pending.next_revision = crate::auth::public_id()
            .map_err(|_| Error::new(ErrorKind::Io, "entropy unavailable"))?;
        Ok(pending)
    }
}
impl PreparedRules {
    /// Picker cancellation lock -> registry -> view -> reviews -> reader state
    /// -> revision. No I/O, no pack reparse, and no authority expansion here.
    pub(crate) fn commit(mut self, stop: &AtomicUsize) -> Result<Value> {
        let registry = self.registry.clone();
        let mut s = registry.inner.state.lock().unwrap();
        if s.closed
            || s.ledger.active().is_some()
            || !self.context.matches(&s)
            || s.registration.is_none()
        {
            return Err(fail("drc_context_changed"));
        }
        let notes = registry.notes();
        let waives = registry.review(floe_app_core::drc::review::store::Kind::Waives);
        let owner = self.owner.clone();
        let reader = self.reader.clone();
        let view_id = self.context.view_id.clone();
        owner
            .with_current(&view_id, |view| {
                let mut commit = || {
                    check_cancelled(stop).map_err(|_| "drc_cancelled")?;
                    if view.controller.is_finished() || view.source_id != reader.source_id {
                        return Err("drc_context_changed");
                    }
                    let mut state = reader.inner.state.lock().unwrap();
                    if state.closed || state.failure.is_some() || state.metadata.is_none() {
                        return Err("drc_context_changed");
                    }
                    reader.inner.revision.replace_at(
                        self.context.revision.as_deref().unwrap(),
                        self.next_revision.clone(),
                        || {
                            self.protection
                                .take()
                                .unwrap()
                                .commit(stop)
                                .map_err(|_| "drc_cancelled")?;
                            let snapshot = self.snapshot.take().unwrap();
                            let registration = s.registration.as_mut().unwrap();
                            registration.rules = Some(snapshot.input.path.clone());
                            registration.rules_scope = Some(snapshot.input.scope.clone());
                            state.metadata.as_mut().unwrap()["svrf"] =
                                snapshot.data.summary().clone();
                            state.rules = Some(snapshot);
                            view.prepared.lock().unwrap().invalidate();
                            *view.drc_panel.lock().unwrap() = super::super::panel::Panel::default();
                            Ok(())
                        },
                    )
                };
                let waive = || {
                    if let Some(w) = &waives {
                        w.admit_build(commit)
                    } else {
                        commit()
                    }
                };
                if let Some(n) = &notes {
                    n.admit_build(waive)
                } else {
                    waive()
                }
            })
            .map_err(fail)?;
        Ok(json!({"drc":reader.catalog(),"view_id":view_id,"metadata_replaced":true}))
    }
}
impl Drop for PreparedRules {
    fn drop(&mut self) {
        self.registry.inner.state.lock().unwrap().replacing = false;
    }
}
