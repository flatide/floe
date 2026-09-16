//! Deny-only, append-only publication protection. No paths are exposed on wire.
use crate::{
    check_cancelled,
    registered::{PublicationKind, RegisteredSource},
    Error, Result,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::atomic::AtomicUsize,
};

const MAX_PATHS: usize = 1024;

#[derive(Clone, Default)]
pub(super) struct Protection {
    files: BTreeSet<PathBuf>,
    trees: BTreeSet<PathBuf>,
    review_targets: BTreeSet<PathBuf>,
}
impl Protection {
    pub(super) fn check_index_targets(
        &self,
        source: &RegisteredSource,
        stop: &AtomicUsize,
    ) -> Result<()> {
        if self.files.is_empty() && self.trees.is_empty() && self.review_targets.is_empty() {
            return Ok(());
        }
        for cache in source.cache_paths()? {
            check_cancelled(stop)?;
            let cache = crate::artifact::resolve_prefix(&cache)?;
            for input in self
                .files
                .iter()
                .chain(&self.review_targets)
                .chain(&self.trees)
            {
                check_cancelled(stop)?;
                let input = crate::artifact::resolve_prefix(input)?;
                if input.starts_with(&cache) {
                    return Err(Error::input(
                        "layout cache would contain a registered review input",
                    ));
                }
            }
            for tree in &self.trees {
                if cache.starts_with(crate::artifact::resolve_prefix(tree)?) {
                    return Err(Error::input(
                        "layout cache would be inside a protected input tree",
                    ));
                }
            }
            let mut lock = cache.into_os_string();
            lock.push(".index.lock");
            self.check(Path::new(&lock), PublicationKind::Defaults)?;
        }
        Ok(())
    }
    fn paths(paths: &[PathBuf], stop: &AtomicUsize) -> Result<BTreeSet<PathBuf>> {
        check_cancelled(stop)?;
        if paths.len() > MAX_PATHS {
            return Err(Error::input("protected path registration limit"));
        }
        paths
            .iter()
            .map(|p| {
                check_cancelled(stop)?;
                if p.as_os_str().is_empty() {
                    return Err(Error::input("empty protected path"));
                }
                let p = crate::cache::absolute(p)?;
                if p.file_name().is_none() {
                    return Err(Error::input("protected path must not be a filesystem root"));
                }
                Ok(p)
            })
            .collect()
    }
    fn install(&mut self, next: Self, stop: &AtomicUsize) -> Result<()> {
        if next.files.len() + next.trees.len() + next.review_targets.len() > MAX_PATHS {
            return Err(Error::input("protected path registration limit"));
        }
        check_cancelled(stop)?;
        *self = next;
        Ok(())
    }
    pub(super) fn inputs(
        &mut self,
        files: &[PathBuf],
        trees: &[PathBuf],
        stop: &AtomicUsize,
    ) -> Result<()> {
        let files = Self::paths(files, stop)?;
        let trees = Self::paths(trees, stop)?;
        let mut next = self.clone();
        next.files.extend(files);
        next.trees.extend(trees);
        self.install(next, stop)
    }
    pub(super) fn review_targets(&mut self, files: &[PathBuf], stop: &AtomicUsize) -> Result<()> {
        let files = Self::paths(files, stop)?;
        let mut next = self.clone();
        next.review_targets.extend(files);
        self.install(next, stop)
    }
    pub(super) fn check(&self, path: &Path, kind: PublicationKind) -> Result<()> {
        let mut files: Vec<_> = self.files.iter().cloned().collect();
        if matches!(kind, PublicationKind::Defaults) {
            files.extend(self.review_targets.iter().cloned());
        }
        // Deny-only metadata also covers a future pack directory. Individual
        // publishers still validate that their actual output parent exists.
        crate::artifact::protected_output_mode(
            path,
            &files,
            &self.trees.iter().cloned().collect::<Vec<_>>(),
            true,
        )?;
        crate::layer_defaults::reject_aliases(path, &files)
    }
}
