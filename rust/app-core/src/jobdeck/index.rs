//! One native index job at a time, each with its own requested worker count.
//! Planning takes no write leases; each source is rechecked under its lease
//! immediately before starting, through the ordinary PreparedIndex service.
use super::{parser::JobDeck, sources::SourceCatalog};
use crate::{cache, check_cancelled, index::IndexOptions, Error, ErrorKind, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

pub fn is_deck(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("jb"))
}
pub fn parse_levels(value: &str) -> Result<BTreeSet<i64>> {
    let ids = value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<i64>()
                .map_err(|_| Error::input("level list must contain integers"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    if ids.is_empty() {
        return Err(Error::input("level list is empty"));
    }
    Ok(ids)
}
pub fn validate_levels(deck: &JobDeck, selected: Option<&BTreeSet<i64>>) -> Result<()> {
    if let Some(ids) = selected {
        if ids.is_empty() {
            return Err(Error::input("level selection is empty"));
        }
        let all = deck.levels();
        let unknown: Vec<_> = ids.difference(&all).map(i64::to_string).collect();
        if !unknown.is_empty() {
            return Err(Error::input(format!(
                "level(s) {} not in the deck (it places {})",
                unknown.join(","),
                all.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
            )));
        }
    }
    Ok(())
}
#[derive(Debug)]
pub struct DeckIndexEntry {
    pub tc: String,
    pub source: PathBuf,
    pub options: IndexOptions,
}
#[derive(Debug)]
pub struct DeckIndexPlan {
    pub catalog: SourceCatalog,
    pub todo: Vec<DeckIndexEntry>,
    pub kept: usize,
    pub levels: BTreeSet<i64>,
    pub selected: Option<BTreeSet<i64>>,
    pub aliases: usize,
}
impl DeckIndexPlan {
    pub fn prepare(
        path: &Path,
        selected: Option<BTreeSet<i64>>,
        options: &IndexOptions,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        options.validate()?;
        if options.wants_representatives() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "representatives require a plain layout source, not a jobdeck",
            ));
        }
        let mut options = options.clone();
        options.occupancy = Some(options.occupancy.unwrap_or(true));
        if options.profile_cell.is_some() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "profile a jobdeck source OASIS directly; deck-wide cell profiling is unsupported",
            ));
        }
        let path = cache::absolute(path)?;
        let deck = JobDeck::read(&path, true, cancelled)?;
        validate_levels(&deck, selected.as_ref())?;
        let mut catalog = SourceCatalog::new(
            path.parent()
                .ok_or_else(|| Error::input("jobdeck has no parent directory"))?,
        )?;
        catalog.probe_all(deck.sources(selected.as_ref()), cancelled)?;
        let (mut todo, mut kept, mut aliases) = (Vec::new(), 0, 0);
        let mut destinations = BTreeSet::new();
        for (tc, info) in &catalog.infos {
            check_cancelled(cancelled)?;
            if !info.ok() {
                continue;
            }
            // Deduplicate lexical aliases for the same cache destination, but
            // not distinct source symlinks: those intentionally own own caches.
            if !destinations.insert(cache::cache_path(&info.path)?) {
                aliases += 1;
                continue;
            }
            let mut current = options.clone();
            if options.force || !info.indexed {
                // Legacy deck occupancy-only also builds unindexed sources.
                current.occupancy =
                    Some(options.occupancy.unwrap_or(true) || options.occupancy_only);
                current.occupancy_only = false;
            } else if options.occupancy_only
                || ((options.occupancy.unwrap_or(true) || options.occupancy_um.is_some())
                    && !info.cache_dir.join("design.ovo").is_file())
            {
                current.occupancy = Some(false);
                current.occupancy_only = true;
            } else if info.cache_dir == cache::default_cache_path(&info.path)? {
                kept += 1;
                continue;
            }
            // An otherwise-current legacy cache is still a planned write:
            // managed admission must promote both aliases before any rename.
            todo.push(DeckIndexEntry {
                tc: tc.clone(),
                source: info.path.clone(),
                options: current,
            });
        }
        Ok(Self {
            catalog,
            todo,
            kept,
            levels: deck.levels(),
            selected,
            aliases,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selectors_are_explicit_and_do_not_touch_sources() {
        assert!(is_deck(Path::new("a 한 글.JB")));
        assert!(!is_deck(Path::new("a.jb.oas")));
        assert_eq!(parse_levels("2,,1,2,").unwrap(), BTreeSet::from([1, 2]));
        assert!(parse_levels(",").is_err());
        assert!(parse_levels("1.0").is_err());
        let deck = JobDeck::parse("x", "CHIP A\n$ (1,A)", true, &AtomicUsize::new(0)).unwrap();
        assert!(validate_levels(&deck, Some(&BTreeSet::from([2]))).is_err());
    }
}
