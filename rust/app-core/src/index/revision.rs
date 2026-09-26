//! Explicit full-build lane for immutable generations. The web UI opts in;
//! ordinary mutable indexing remains separate.
use super::{arguments, Action, IndexJob, IndexOptions};
use crate::{
    cache::revision::{Candidate, Publication, Snapshot, Store},
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
    _permit: Option<Permit>,
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
        let store = Store::new(source)?;
        let permit = resources.index([store.path().to_owned()], options.jobs)?;
        let mut build = Self::start_admitted(source, options, indexer, stop)?;
        build._permit = Some(permit);
        Ok(build)
    }
    /// The managed set supervisor holds one admission across every source.
    pub(crate) fn start_admitted(
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
            _permit: None,
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
        self.require_success(stop)?;
        self.candidate.publish(stop)
    }
    /// A sealed source does not advance its individual current pointer. A set
    /// publisher makes all selected sources visible with one manifest commit.
    pub(crate) fn seal(
        mut self,
        owner: crate::cache::revision::SetOwner,
        stop: &AtomicUsize,
    ) -> Result<Snapshot> {
        self.require_success(stop)?;
        self.candidate.seal_owned(Some(owner), stop)
    }
    fn require_success(&mut self, stop: &AtomicUsize) -> Result<()> {
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
        Ok(())
    }
}
