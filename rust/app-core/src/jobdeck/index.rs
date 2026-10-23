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
    /// light caches an explicit --full rebuilds as the full index (0.12.390;
    /// `prepare` - the auto-index's `prepare_for_open` rebuilds none)
    pub full_over_light: usize,
}
impl DeckIndexPlan {
    /// The plan of `floe2 index DECK` (and the viewer's and the service's
    /// index of a deck): the sources of the levels selected, each as
    /// `options` ask - a source without a cache built, one with a current
    /// cache kept, a light cache under an explicit `--full` built over as
    /// the full index (0.12.390), a missing occupancy summary added.
    pub fn prepare(
        path: &Path,
        selected: Option<BTreeSet<i64>>,
        options: &IndexOptions,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        Self::plan(path, selected, options, true, cancelled)
    }

    /// The auto-index's plan (app-cli ready.rs: a read command that opens a
    /// deck - render, probe, info): a cache there is opened as it is
    /// whatever its kind - `--light` / `--full` name the cache built for a
    /// source without one (render's help) - so a light cache under `--full`
    /// is kept, and with the occupancy on a current cache without its
    /// summary gets the occupancy-only entry (run as its own kind's
    /// command, whatever `light` says). `prepare`'s rebuild of a light
    /// cache under `--full` made such entries builds, which the auto-index
    /// skips (a cache is there): `render --full --occupancy on` over light
    /// caches built no summary and the capture ended incomplete (exit 3,
    /// `summary_building=`) where the same command without `--full`
    /// succeeded (review 2026-10-23, 0.12.392).
    pub fn prepare_for_open(
        path: &Path,
        selected: Option<BTreeSet<i64>>,
        options: &IndexOptions,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        Self::plan(path, selected, options, false, cancelled)
    }

    fn plan(
        path: &Path,
        selected: Option<BTreeSet<i64>>,
        options: &IndexOptions,
        rebuild_light_under_full: bool,
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
        // a deck's sources take the light cache unless told otherwise
        // (DIRECT_READ_PLAN §10 decision 2; --full, FLOE_INDEX_LIGHT=off, or
        // an option of the full index's): `deck_source` gives the rule to
        // PreparedIndex::prepare, which keeps `light` as given - a light
        // cache the rule chose gives way to the full index when floe-index
        // light cannot read the stream (a cell defined inside a CBLOCK)
        options.deck_source = true;
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
        let (mut todo, mut kept, mut aliases, mut full_over_light) = (Vec::new(), 0, 0, 0);
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
            // an explicit --full over a light cache: the full index is built
            // in its place (decide() builds over a light cache under a full
            // request); the plan kept the source as indexed before, so
            // `floe2 index deck.jb --full` ended with "kept" and the caches
            // light (review 2026-10-23, 0.12.390). Not for the auto-index
            // (`prepare_for_open`): it opens a cache as it is
            let light_under_full = rebuild_light_under_full
                && options.light == Some(false)
                && info.indexed
                && cache::cache_kind(&info.cache_dir) == Some(cache::CacheKind::Light);
            if light_under_full {
                full_over_light += 1;
            }
            if options.force || !info.indexed || light_under_full {
                // Legacy deck occupancy-only also builds unindexed sources.
                current.occupancy =
                    Some(options.occupancy.unwrap_or(true) || options.occupancy_only);
                current.occupancy_only = false;
                // a deck's sources need no design.ovs: the deck view never
                // reads it (a layout's own index makes it; `floe-index ovs`
                // adds it to a source opened alone)
                current.ovs = false;
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
            full_over_light,
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
