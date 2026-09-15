//! Explicit local registration, not a browser filesystem API. A gateway only
//! exposes opaque handles for these sources. All deck dependencies (including
//! unselected levels, whose headers can be read) must stay in approved roots.
mod set;
use crate::{
    cache, check_cancelled,
    jobdeck::{index::is_deck, parser::JobDeck, sources::file_header},
    Error, ErrorKind, Result,
};
pub use set::{Registration, SourceSet};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc},
    time::SystemTime,
};

pub const MAX_SOURCES: usize = 32;
const MAX_DEPENDENCIES: usize = 65536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelInfo {
    pub id: i64,
    pub title: String,
}
#[derive(Debug)]
pub struct AccessScope {
    roots: Vec<PathBuf>,
}
impl AccessScope {
    /// Only the trusted launcher supplies roots, never a browser command.
    pub fn new(roots: &[PathBuf]) -> Result<Arc<Self>> {
        if roots.is_empty() || roots.len() > MAX_SOURCES {
            return Err(Error::input("registration requires 1..32 approved roots"));
        }
        let mut resolved = BTreeSet::new();
        for root in roots {
            let p = fs::canonicalize(root)?;
            if !p.is_dir() || p.parent().is_none() {
                return Err(Error::input("approved root must be a non-root directory"));
            }
            resolved.insert(p);
        }
        Ok(Arc::new(Self {
            roots: resolved.into_iter().collect(),
        }))
    }
    pub fn check(&self, path: &Path) -> Result<PathBuf> {
        let original = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()?.join(path)
        };
        let path = cache::absolute(path)?;
        // Native cache paths use lexical abspath, while source/header readers
        // may follow the original spelling. Validate BOTH: symlink/../x can
        // resolve outside a root even though its lexical normalization is in it.
        for candidate in [&original, &path] {
            let resolved = resolve_existing_parent(candidate)?;
            if !self.roots.iter().any(|root| resolved.starts_with(root)) {
                return Err(Error::input(
                    "source or dependency is outside approved roots",
                ));
            }
        }
        Ok(path)
    }
}
fn resolve_existing_parent(path: &Path) -> Result<PathBuf> {
    // Resolve the nearest existing ancestor so missing TCs and future
    // caches cannot bypass scope through a parent-directory symlink.
    let mut ancestor = path;
    let mut missing = Vec::new();
    let resolved = loop {
        match fs::canonicalize(ancestor) {
            Ok(mut p) => {
                for name in missing.iter().rev() {
                    p.push(name);
                }
                break p;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(
                    ancestor
                        .file_name()
                        .ok_or_else(|| Error::input("missing root"))?,
                );
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| Error::input("missing root"))?;
            }
            Err(e) => return Err(e.into()),
        }
    };
    cache::absolute(&resolved)
}

#[derive(Debug)]
pub struct RegisteredSource {
    pub title: String,
    pub deck: bool,
    pub levels: Vec<LevelInfo>,
    path: PathBuf,
    scope: Arc<AccessScope>,
    stamp: (u64, SystemTime),
    dependencies: Vec<PathBuf>,
}
impl RegisteredSource {
    pub fn register(
        scope: Arc<AccessScope>,
        path: &Path,
        cancelled: &AtomicUsize,
    ) -> Result<Arc<Self>> {
        let path = scope.check(path)?;
        let stamp = stamp(&path)?;
        let deck = is_deck(&path);
        let (levels, dependencies) = inspect(&scope, &path, cancelled)?;
        if stamp != self::stamp(&path)? {
            return Err(Error::new(
                ErrorKind::Cache,
                "source changed during registration",
            ));
        }
        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::input("source name requires UTF-8"))?
            .chars()
            .take(256)
            .collect();
        Ok(Arc::new(Self {
            title,
            deck,
            levels,
            path,
            scope,
            stamp,
            dependencies,
        }))
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Revalidate before an operation on its bounded service thread, never on
    /// the HTTP reactor. External concurrent file/symlink replacement remains
    /// unsupported: these path checks are not an OS sandbox/immutable revision.
    pub fn validate(&self, cancelled: &AtomicUsize) -> Result<()> {
        check_cancelled(cancelled)?;
        self.scope.check(&self.path)?;
        if stamp(&self.path)? != self.stamp {
            return Err(Error::new(
                ErrorKind::Cache,
                "registered source changed; register it again",
            ));
        }
        let (levels, dependencies) = inspect(&self.scope, &self.path, cancelled)?;
        if levels != self.levels
            || dependencies != self.dependencies
            || stamp(&self.path)? != self.stamp
        {
            return Err(Error::new(
                ErrorKind::Cache,
                "registered source/dependencies changed",
            ));
        }
        Ok(())
    }
    pub fn cache_paths(&self) -> Result<Vec<PathBuf>> {
        self.dependencies
            .iter()
            .map(|p| cache::cache_path(p))
            .collect()
    }
    pub(crate) fn scoped_output(&self, path: &Path) -> Result<PathBuf> {
        self.scope.check(path)
    }
    /// A shared sidecar writer must protect every registered source, not only
    /// the current one: another source may have a sidecar-shaped filename.
    pub(crate) fn protect_output(&self, path: &Path) -> Result<PathBuf> {
        let trees = self.cache_paths()?;
        let mut files = self.dependencies.clone();
        files.push(self.path.clone());
        for tree in &trees {
            let mut lock = tree.as_os_str().to_owned();
            lock.push(".index.lock");
            files.push(lock.into());
        }
        let out = crate::artifact::protected_output(path, &files, &trees)?;
        crate::layer_defaults::reject_aliases(path, &files)?;
        Ok(out)
    }
    pub fn validate_levels(&self, selected: Option<&BTreeSet<i64>>) -> Result<()> {
        if let Some(ids) = selected {
            if !self.deck
                || ids.is_empty()
                || ids
                    .iter()
                    .any(|id| !self.levels.iter().any(|l| l.id == *id))
            {
                return Err(Error::input("invalid registered level selection"));
            }
        }
        Ok(())
    }
}
fn stamp(path: &Path) -> Result<(u64, SystemTime)> {
    let m = fs::metadata(path)?;
    if !m.is_file() {
        return Err(Error::input("registered source must be a regular file"));
    }
    Ok((m.len(), m.modified()?))
}
fn inspect(
    scope: &AccessScope,
    path: &Path,
    cancelled: &AtomicUsize,
) -> Result<(Vec<LevelInfo>, Vec<PathBuf>)> {
    check_cancelled(cancelled)?;
    let (levels, dependencies) = if is_deck(path) {
        let deck = JobDeck::read(path, true, cancelled)?;
        let names = deck.sources(None);
        let ids = deck.levels();
        if names.len() > MAX_DEPENDENCIES || ids.len() > 4096 {
            return Err(Error::input(
                "registered deck exceeds dependency/level limit",
            ));
        }
        let mut dependencies = BTreeSet::new();
        for tc in names {
            check_cancelled(cancelled)?;
            dependencies.insert(scope.check(&path.parent().expect("absolute source").join(tc))?);
        }
        let levels = ids
            .into_iter()
            .map(|id| LevelInfo {
                id,
                title: deck
                    .mtitles
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| format!("LEVEL-{id}"))
                    .chars()
                    .take(256)
                    .collect(),
            })
            .collect();
        (levels, dependencies.into_iter().collect::<Vec<_>>())
    } else {
        let h = file_header(path, cancelled)?;
        if h.format.as_deref() != Some("oasis") || h.gzipped {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "web sources require OASIS or jobdeck",
            ));
        }
        (Vec::new(), vec![path.to_owned()])
    };
    for source in &dependencies {
        check_cancelled(cancelled)?;
        scope.check(source)?;
        scope.check(&cache::cache_path(source)?)?;
    }
    Ok((levels, dependencies))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_rejects_unselected_escapes_and_rechecks_changed_sources() {
        let dir = std::env::temp_dir().join(format!("floe-register-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        fs::create_dir(dir.join("allowed")).unwrap();
        fs::create_dir(dir.join("outside")).unwrap();
        std::os::unix::fs::symlink(dir.join("outside"), dir.join("allowed/link")).unwrap();
        let root = dir.join("allowed");
        let scope = AccessScope::new(std::slice::from_ref(&root)).unwrap();
        assert!(scope.check(&root.join("link/missing/subdir.oas")).is_err());
        assert!(scope.check(&root.join("link/../outside/leak.oas")).is_err());
        assert!(scope.check(&root.join("../outside/new.oas")).is_err());
        assert!(scope.check(&root.join("new/subdir.oas")).is_ok());
        assert!(AccessScope::new(&[PathBuf::from("/")]).is_err());
        let path = root.join("test.jb");
        let flag = AtomicUsize::new(0);
        fs::write(
            &path,
            "CHIP A\n$ (1,A,TC=missing.oas)\n$ (2,B,TC=link/leak.oas)\n",
        )
        .unwrap();
        assert!(RegisteredSource::register(Arc::clone(&scope), &path, &flag).is_err());
        fs::write(&path, "MTITLE 1,Mask\nCHIP A\n$ (1,A,TC=missing.oas)\n").unwrap();
        let source = RegisteredSource::register(Arc::clone(&scope), &path, &flag).unwrap();
        source.validate(&flag).unwrap();
        assert_eq!(source.title, "test.jb");
        assert_eq!(
            source.levels,
            [LevelInfo {
                id: 1,
                title: "Mask".into()
            }]
        );
        assert_eq!(
            source.cache_paths().unwrap(),
            [root.join("missing.oas.floe")]
        );
        assert!(source.validate_levels(Some(&BTreeSet::from([2]))).is_err());
        fs::write(&path, "CHIP A\n$ (1,A,TC=other.oas)\n").unwrap();
        assert!(source.validate(&flag).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_file(root.join("link")).unwrap();
        fs::remove_dir(root).unwrap();
        fs::remove_dir(dir.join("outside")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
