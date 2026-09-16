//! Picker-owned preparation; publication is under picker -> registry -> view
//! locks. Opening never retires the previous reader or changes layout state.
use super::*;
use floe_app_core::{browse::SelectedFile, check_cancelled, registered, Error};
use serde::Serialize;
use std::time::Instant;

const RETIRED_READERS: usize = 2;
const OPEN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenContext {
    pub view_id: String,
    pub drc_id: Option<String>,
    pub revision: Option<String>,
}
impl OpenContext {
    pub(crate) fn validate(&self) -> std::result::Result<(), Failure> {
        if self.drc_id.is_some() != self.revision.is_some()
            || std::iter::once(&self.view_id)
                .chain(self.drc_id.iter())
                .chain(self.revision.iter())
                .any(|id| {
                    id.len() != 64
                        || !id
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
        {
            return Err("invalid_drc_request");
        }
        Ok(())
    }
    fn matches(&self, s: &State) -> bool {
        match (&s.current, &self.drc_id, &self.revision) {
            (None, None, None) => s.registration.is_none(),
            (Some(r), Some(id), Some(rev)) => {
                // A failed/closed reader must be replaceable too. This is an
                // identity CAS, not permission to query the retired geometry.
                let (revision, changing) = r.inner.revision.snapshot();
                r.id == *id && revision == *rev && !changing
            }
            _ => false,
        }
    }
}
pub(crate) struct PreparedOpen {
    registry: Arc<Registry>,
    owner: Arc<crate::service::Service>,
    context: OpenContext,
    candidate: Option<Arc<Service>>,
    registration: Option<Registration>,
    protection: Option<registered::Registration>,
}
fn fail(code: Failure) -> Error {
    Error::new(ErrorKind::Busy, code)
}
impl Registry {
    /// Blocking picker actor only: selected inode witness and roots never come
    /// from a client-supplied path. One candidate plus at most two draining readers.
    pub(crate) fn prepare_open(
        self: &Arc<Self>,
        owner: Arc<crate::service::Service>,
        selected: SelectedFile,
        context: OpenContext,
        stop: &AtomicUsize,
    ) -> Result<PreparedOpen> {
        context.validate().map_err(fail)?;
        {
            let mut s = self.inner.state.lock().unwrap();
            s.retired.retain(|r| !r.is_finished());
            if s.closed
                || s.replacing
                || s.ledger.active().is_some()
                || s.retired.len() >= RETIRED_READERS
                || !context.matches(&s)
            {
                return Err(fail("drc_busy_or_context_changed"));
            }
            s.replacing = true;
        }
        let mut pending = PreparedOpen {
            registry: Arc::clone(self),
            owner,
            context,
            candidate: None,
            registration: None,
            protection: None,
        };
        let source_id = pending
            .owner
            .with_current(&pending.context.view_id, |v| {
                if v.controller.is_finished() {
                    return Err("view_unavailable");
                }
                Ok(v.source_id.clone())
            })
            .map_err(fail)?;
        selected.validate(stop)?;
        let scope = selected.scope();
        let path = selected.path().to_owned();
        pending.protection = Some(pending.owner.source_set().begin(stop)?);
        let mut cache = path.as_os_str().to_owned();
        cache.push(".ice");
        let cache = std::path::PathBuf::from(cache);
        let packed = floe_app_core::drc::is_packed_source(&path)?;
        if !packed {
            scope.check(&cache)?;
        }
        let (resources, _) = pending.owner.drc_resources();
        // Read leases also exclude managed index/export writers during cache
        // selection; the candidate retains its own leases before this is dropped.
        let files: Vec<_> = std::iter::once(path.clone())
            .chain((!packed).then(|| cache.clone()))
            .collect();
        let admission = resources.drc(files.clone())?;
        let chosen = floe_app_core::drc::select_current_source(&path, stop)?;
        scope.check(&chosen.path)?;
        let protection = pending.protection.as_mut().unwrap();
        protection.protect_inputs(&files, &files, stop)?;
        let reader =
            Service::start_selected_source(&resources, Arc::clone(&scope), chosen, &source_id)?;
        pending.candidate = Some(Arc::clone(&reader));
        drop(admission);
        pending.registration = Some(Registration {
            resources,
            scope,
            path,
            waives: None,
            rules: None,
            readonly: None,
            source_id,
        });
        let end = Instant::now() + OPEN_TIMEOUT;
        loop {
            check_cancelled(stop)?;
            if self.inner.state.lock().unwrap().closed {
                return Err(Error::new(ErrorKind::Cancelled, "DRC workspace closed"));
            }
            match reader.catalog()["phase"].as_str() {
                Some("ready") => break,
                Some("error" | "closed") => {
                    return Err(Error::new(ErrorKind::Cache, "DRC open failed"))
                }
                _ => (),
            }
            if Instant::now() >= end {
                return Err(Error::new(ErrorKind::Busy, "DRC open deadline exceeded"));
            }
            thread::sleep(Duration::from_millis(10));
        }
        selected.validate(stop)?;
        Ok(pending)
    }
}
impl PreparedOpen {
    /// Call under the picker's cancellation/receipt lock. All I/O has finished;
    /// no cancellation after this commit can relabel it as a cancelled open.
    pub(crate) fn commit(mut self, stop: &AtomicUsize) -> Result<Value> {
        let registry = Arc::clone(&self.registry);
        let mut s = registry.inner.state.lock().unwrap();
        if s.closed || !self.context.matches(&s) || s.ledger.active().is_some() {
            return Err(fail("drc_context_changed"));
        }
        let notes = registry.notes();
        let waives = registry.review(floe_app_core::drc::review::store::Kind::Waives);
        let owner = Arc::clone(&self.owner);
        let view_id = self.context.view_id.clone();
        owner
            .with_current(&view_id, |view| {
                let mut commit = || {
                    check_cancelled(stop).map_err(|_| "drc_cancelled")?;
                    if view.controller.is_finished() {
                        return Err("view_unavailable");
                    }
                    let candidate = self.candidate.as_ref().unwrap();
                    if candidate.source_id != view.source_id {
                        return Err("drc_context_changed");
                    }
                    self.protection
                        .take()
                        .unwrap()
                        .commit(stop)
                        .map_err(|_| "drc_cancelled")?;
                    let reader = self.candidate.take().unwrap();
                    let result = reader.catalog();
                    if let Some(old) = s.current.replace(reader) {
                        old.request_stop();
                        s.retired.push(old);
                    }
                    s.registration = self.registration.take();
                    view.prepared.lock().unwrap().invalidate();
                    *view.drc_panel.lock().unwrap() = super::super::panel::Panel::default();
                    Ok(json!({"drc":result,"view_id":view_id,"review_registration_required":true}))
                };
                let waive = || {
                    if let Some(w) = &waives {
                        w.admit_detach(commit)
                    } else {
                        commit()
                    }
                };
                if let Some(n) = &notes {
                    n.admit_detach(waive)
                } else {
                    waive()
                }
            })
            .map_err(fail)
    }
}
impl Drop for PreparedOpen {
    fn drop(&mut self) {
        let mut s = self.registry.inner.state.lock().unwrap();
        if let Some(reader) = self.candidate.take() {
            reader.request_stop();
            s.retired.push(reader);
        }
        s.replacing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_app_core::{
        managed::{Limits, Resources},
        registered::AccessScope,
    };
    #[test]
    fn replacement_identity_allows_failed_reader_but_not_stale_or_updating() {
        let reader = Service::unavailable(
            Registration {
                resources: Resources::new(Limits::default()).unwrap(),
                scope: AccessScope::new(&[std::env::temp_dir()]).unwrap(),
                path: std::env::temp_dir().join("runtime-drc-test-not-opened.db"),
                waives: None,
                rules: None,
                readonly: None,
                source_id: "source".into(),
            },
            "drc_read_error",
        )
        .unwrap();
        let registry = Registry::read_only(Arc::clone(&reader));
        let mut context = OpenContext {
            view_id: "a".repeat(64),
            drc_id: Some(reader.id.clone()),
            revision: Some(reader.revision()),
        };
        context.validate().unwrap();
        assert!(context.matches(&registry.inner.state.lock().unwrap()));
        let change = reader.inner.revision.begin().unwrap();
        context.revision = Some(reader.revision());
        assert!(!context.matches(&registry.inner.state.lock().unwrap()));
        drop(change);
        reader.request_stop();
        assert!(
            context.matches(&registry.inner.state.lock().unwrap()),
            "closed reader is still replaceable"
        );
        context.revision = Some("b".repeat(64));
        assert!(!context.matches(&registry.inner.state.lock().unwrap()));
        context.drc_id = None;
        assert!(context.validate().is_err());
        context.revision = None;
        context.validate().unwrap();
        assert!(!context.matches(&registry.inner.state.lock().unwrap()));
        let empty = Registry::initial(None, None).unwrap();
        assert!(context.matches(&empty.inner.state.lock().unwrap()));
        assert!(
            empty.catalog()["build"].is_null(),
            "no null source_id in a build capability"
        );
        context.view_id = "../file".into();
        assert!(context.validate().is_err());
        assert!(serde_json::from_value::<OpenContext>(
            json!({"view_id":"a".repeat(64),"path":"/tmp/file"})
        )
        .is_err());
    }
}
