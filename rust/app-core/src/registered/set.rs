//! Append-only trusted registrations and sidecar-publication exclusion.
//! The mutex protects counters only, never filesystem I/O. Reads can continue
//! while a registration is inspected; competing mutations fail with Busy.
use super::{AccessScope, RegisteredSource, MAX_SOURCES};
use crate::{check_cancelled, Error, ErrorKind, Result};
use std::{
    path::Path,
    sync::{atomic::AtomicUsize, Arc, Mutex},
};

pub struct SourceSet {
    state: Mutex<State>,
}
struct State {
    sources: Vec<Arc<RegisteredSource>>,
    registering: bool,
    publications: usize,
}
impl SourceSet {
    pub fn new(sources: Vec<Arc<RegisteredSource>>) -> Result<Arc<Self>> {
        if sources.len() > MAX_SOURCES {
            return Err(Error::input("too many registered sources"));
        }
        Ok(Arc::new(Self {
            state: Mutex::new(State {
                sources,
                registering: false,
                publications: 0,
            }),
        }))
    }
    pub fn snapshot(&self) -> Vec<Arc<RegisteredSource>> {
        self.state.lock().unwrap().sources.clone()
    }
    /// Trusted launcher only. Holding this reservation grants no indexing or
    /// publication authority, and dropping it rolls back all staged additions.
    pub fn begin(self: &Arc<Self>, stop: &AtomicUsize) -> Result<Registration> {
        check_cancelled(stop)?;
        let mut state = self.state.lock().unwrap();
        if state.registering || state.publications != 0 {
            return Err(busy());
        }
        state.registering = true;
        Ok(Registration {
            owner: Arc::clone(self),
            existing: state.sources.clone(),
            staged: Vec::new(),
        })
    }
    /// Hold from before any lock/stage creation through commit and cleanup.
    /// Other publishers may proceed; source registration cannot overlap them.
    pub(crate) fn publication(self: &Arc<Self>, stop: &AtomicUsize) -> Result<Publication> {
        check_cancelled(stop)?;
        let mut state = self.state.lock().unwrap();
        if state.registering {
            return Err(busy());
        }
        state.publications = state.publications.checked_add(1).ok_or_else(busy)?;
        Ok(Publication(Arc::clone(self)))
    }
}
fn busy() -> Error {
    Error::new(
        ErrorKind::Busy,
        "source registration or publication is active",
    )
}
pub struct Registration {
    owner: Arc<SourceSet>,
    existing: Vec<Arc<RegisteredSource>>,
    staged: Vec<Arc<RegisteredSource>>,
}
impl Registration {
    /// All header/dependency/scope checks run outside the short state mutex,
    /// while the reservation excludes cooperating sidecar writers.
    pub fn register(
        &mut self,
        scope: Arc<AccessScope>,
        path: &Path,
        stop: &AtomicUsize,
    ) -> Result<Arc<RegisteredSource>> {
        check_cancelled(stop)?;
        let path = scope.check(path)?;
        if let Some(source) = self
            .existing
            .iter()
            .chain(&self.staged)
            .find(|s| s.path() == path)
        {
            // Do not silently adopt an externally replaced source or broaden
            // the dependency scope using a previous registration's roots.
            for dependency in &source.dependencies {
                scope.check(dependency)?;
            }
            source.validate(stop)?;
            return Ok(Arc::clone(source));
        }
        if self.existing.len() + self.staged.len() >= MAX_SOURCES {
            return Err(Error::input("too many registered sources"));
        }
        let source = RegisteredSource::register(scope, &path, stop)?;
        self.staged.push(Arc::clone(&source));
        Ok(source)
    }
    /// No filesystem work. Call under the application's catalogue commit lock
    /// so a newly visible handle is never missing from publication protection.
    pub fn commit(mut self, stop: &AtomicUsize) -> Result<()> {
        check_cancelled(stop)?;
        self.owner
            .state
            .lock()
            .unwrap()
            .sources
            .append(&mut self.staged);
        Ok(())
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.owner.state.lock().unwrap().registering = false;
    }
}
pub(crate) struct Publication(Arc<SourceSet>);
impl Drop for Publication {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().publications -= 1;
    }
}

#[cfg(test)]
mod tests;
