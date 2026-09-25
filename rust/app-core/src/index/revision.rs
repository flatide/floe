//! Explicit full-build lane for immutable generations. Not yet wired to the
//! ordinary index command or web UI: readers/cutover must migrate together.
use super::{arguments, Action, IndexJob, IndexOptions};
use crate::{
    cache::revision::{Candidate, Publication, Store},
    check_cancelled,
    index_progress::Progress,
    managed::{Permit, Resources},
    native::Indexer,
    Error, ErrorKind, Result,
};
use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc},
};

pub struct Build {
    // Drop/reap the native child before releasing either writer lease.
    job: IndexJob,
    candidate: Candidate,
    _permit: Permit,
}
impl Build {
    /// Trusted local source only. Web callers must first validate their registered
    /// source capability; no browser-chosen destination or binary is accepted.
    pub fn start(
        resources: &Arc<Resources>,
        source: &Path,
        options: &IndexOptions,
        indexer: Indexer,
        stop: &AtomicUsize,
    ) -> Result<Self> {
        options.validate()?;
        if options.profile_cell.is_some()
            || options.occupancy_only
            || options.representatives_only
            || crate::jobdeck::index::is_deck(source)
        {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "revision build requires one layout and a full build",
            ));
        }
        check_cancelled(stop)?;
        let store = Store::new(source)?;
        let permit = resources.index([store.path().to_owned()], options.jobs)?;
        indexer.verify(stop)?;
        let candidate = store.begin(stop)?;
        let args = arguments(
            &crate::cache::absolute(source)?,
            &candidate.directory(),
            options,
            &Action::Build,
        )?;
        check_cancelled(stop)?;
        let job = IndexJob::captured(&indexer, &args)?;
        Ok(Self {
            job,
            candidate,
            _permit: permit,
        })
    }
    pub fn directory(&self) -> PathBuf {
        self.candidate.directory()
    }
    pub fn poll(&mut self) -> Result<Option<i32>> {
        self.job.poll()
    }
    pub fn progress(&self) -> Option<&Progress> {
        self.job.progress()
    }
    pub fn cancel(&mut self) -> Result<i32> {
        self.job.cancel(libc::SIGTERM)
    }
    /// Publication is separate from native exit: callers can cancel after a
    /// complete build but before changing the current revision. Failure/cancel
    /// preserves all previous generations and keeps the candidate for diagnosis.
    pub fn publish(mut self, stop: &AtomicUsize) -> Result<Publication> {
        check_cancelled(stop)?;
        match self.job.poll()? {
            Some(0) => (),
            Some(_) => return Err(Error::new(ErrorKind::Worker, "revision indexer failed")),
            None => {
                return Err(Error::new(
                    ErrorKind::Busy,
                    "revision indexer is still running",
                ))
            }
        }
        self.candidate.publish(stop)
    }
}
