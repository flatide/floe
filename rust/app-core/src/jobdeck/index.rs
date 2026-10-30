//! One native index job at a time, each with its own requested worker count.
//! Planning takes no write leases; each source is rechecked under its lease
//! immediately before starting, through the ordinary PreparedIndex service.
use super::{
    chips::wildcard,
    geom::{self, PlanOptions},
    parser::JobDeck,
    sources::SourceCatalog,
    window::{DeckReads, DeckWindow},
};
use crate::{cache, check_cancelled, index::IndexOptions, Error, ErrorKind, Result};
use std::collections::{BTreeMap, BTreeSet};
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
/// The CHIP blocks `floe2 index DECK` works on when `--chipid` /
/// `--chipnumber` say (user 2026-10-25: a deck's index split by chips, the
/// parts run at once on several hosts - a jobdeck's EBEAM files are each one
/// chip's, so the parts share no source): ids as the CHIP lines give them -
/// exact, or a pattern with `*`, `?`, `[..]` as `render --chip` takes - and
/// 1-based block numbers in deck order, singly or as `A-B` ranges. A chip in
/// either list is in; `--level` cuts across the result. Every id must name a
/// block and every number must be within the deck (`resolve`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChipSelection {
    pub ids: Vec<String>,
    /// inclusive 1-based ranges, as given
    pub numbers: Vec<(usize, usize)>,
}
impl ChipSelection {
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty() && self.numbers.is_empty()
    }
    /// `--chipid ID,ID,..`
    pub fn add_ids(&mut self, value: &str) -> Result<()> {
        let before = self.ids.len();
        self.ids.extend(
            value
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from),
        );
        if self.ids.len() == before {
            return Err(Error::input("--chipid: the chip id list is empty"));
        }
        Ok(())
    }
    /// `--chipnumber N,N-M,..`
    pub fn add_numbers(&mut self, value: &str) -> Result<()> {
        let bad = || {
            Error::input(
                "--chipnumber: the list takes 1-based CHIP block numbers N or ranges N-M (deck order)",
            )
        };
        let before = self.numbers.len();
        for token in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (lo, hi) = match token.split_once('-') {
                Some((a, b)) => (
                    a.trim().parse::<usize>().map_err(|_| bad())?,
                    b.trim().parse::<usize>().map_err(|_| bad())?,
                ),
                None => {
                    let n = token.parse::<usize>().map_err(|_| bad())?;
                    (n, n)
                }
            };
            if lo == 0 || hi < lo {
                return Err(bad());
            }
            self.numbers.push((lo, hi));
        }
        if self.numbers.len() == before {
            return Err(bad());
        }
        Ok(())
    }
    /// The blocks the selection names in `deck`: 0-based indices in deck
    /// order with `n:ID` labels (n 1-based); an id naming no block, or a
    /// number past the deck's blocks, is an input error naming them.
    pub fn resolve(&self, deck: &JobDeck) -> Result<ChipPick> {
        let total = deck.chips.len();
        let mut blocks = BTreeSet::new();
        let mut unknown = Vec::new();
        for id in &self.ids {
            let wild = id.contains(['*', '?', '[']);
            let mut hit = false;
            for (i, chip) in deck.chips.iter().enumerate() {
                let same = if wild { wildcard(id, &chip.id) } else { chip.id == *id };
                if same {
                    blocks.insert(i);
                    hit = true;
                }
            }
            if !hit {
                unknown.push(id.as_str());
            }
        }
        if !unknown.is_empty() {
            return Err(Error::input(format!(
                "chip id(s) {} not in the deck ({total} CHIP block(s); `{} info DECK --chips` lists them; `*`, `?`, `[..]` match as render --chip)",
                unknown.join(","),
                crate::program()
            )));
        }
        let over: Vec<String> = self
            .numbers
            .iter()
            .filter(|(_, hi)| *hi > total)
            .map(|(lo, hi)| if lo == hi { lo.to_string() } else { format!("{lo}-{hi}") })
            .collect();
        if !over.is_empty() {
            return Err(Error::input(format!(
                "chip number(s) {} not in the deck (it has {total} CHIP block(s), numbered 1..{total} in deck order)",
                over.join(",")
            )));
        }
        for &(lo, hi) in &self.numbers {
            blocks.extend(lo - 1..hi);
        }
        let labels = blocks
            .iter()
            .map(|&i| format!("{}:{}", i + 1, deck.chips[i].id))
            .collect();
        Ok(ChipPick { blocks, total, labels })
    }
}
/// What a `ChipSelection` came to in a deck.
#[derive(Clone, Debug)]
pub struct ChipPick {
    /// 0-based CHIP block indices, deck order
    pub blocks: BTreeSet<usize>,
    /// the deck's CHIP block count
    pub total: usize,
    /// `n:ID` per block picked, deck order
    pub labels: Vec<String>,
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
    /// the CHIP blocks `--chipid` / `--chipnumber` picked (None: every block)
    pub chips: Option<ChipPick>,
    pub aliases: usize,
    /// light caches an explicit --full rebuilds as the full index (0.12.390;
    /// `prepare` - the auto-index's `prepare_for_open` rebuilds none)
    pub full_over_light: usize,
    /// summaries `prepare` builds again because their cell is coarser than
    /// the one the deck's mag asks for (`summary_coarser`; the auto-index
    /// keeps them)
    pub coarse_summaries: usize,
    /// a capture's window (jobdeck::window; `prepare_for_open`): the
    /// sources of the placements it reaches are the plan's, the rest of the
    /// level - `outside` - is left as it is (None: every source)
    pub window_sources: Option<BTreeSet<String>>,
    pub outside: usize,
}
impl DeckIndexPlan {
    /// The plan of `floe2 index DECK` (and the viewer's and the service's
    /// index of a deck): the sources of the levels selected, each as
    /// `options` ask - a source without a cache built, one with a current
    /// cache kept, a light cache under an explicit `--full` built over as
    /// the full index (0.12.390), a missing occupancy summary added, a
    /// summary coarser than the source's mag asks for built again
    /// (2026-10-28). Every entry carries its source's largest mag in the
    /// deck (`source_mags`) for the automatic cell.
    pub fn prepare(
        path: &Path,
        selected: Option<BTreeSet<i64>>,
        chips: Option<&ChipSelection>,
        options: &IndexOptions,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        Self::plan(path, selected, chips, None, options, true, None, cancelled)
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
    /// `temp`: the run's temporary index (cache::TempIndex root; `render
    /// --temp-index`): a source with no cache beside it gets its entry
    /// built under it (`IndexOptions::out_dir`), and is read from there.
    /// `reads`: what the capture reads of the deck (jobdeck::window) - the
    /// CHIP blocks named, and the window whose placements' sources alone
    /// are planned.
    pub fn prepare_for_open(
        path: &Path,
        selected: Option<BTreeSet<i64>>,
        options: &IndexOptions,
        temp: Option<&Path>,
        reads: Option<&DeckReads>,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        Self::plan(
            path,
            selected,
            reads.and_then(|r| r.chips.as_ref()),
            reads.and_then(|r| r.window.as_ref()),
            options,
            false,
            temp,
            cancelled,
        )
    }

    /// `explicit`: `floe2 index DECK` (`prepare`), not the auto-index of an
    /// open (`prepare_for_open`) - a light cache under `--full` is built
    /// over, and a summary coarser than the source's mag asks for is built
    /// again; an open takes the caches as they are.
    #[allow(clippy::too_many_arguments)]
    fn plan(
        path: &Path,
        selected: Option<BTreeSet<i64>>,
        chips: Option<&ChipSelection>,
        window: Option<&DeckWindow>,
        options: &IndexOptions,
        explicit: bool,
        temp: Option<&Path>,
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
        // the CHIP blocks --chipid / --chipnumber pick (every block unsaid)
        let chips = chips
            .filter(|c| !c.is_empty())
            .map(|c| c.resolve(&deck))
            .transpose()?;
        let mut catalog = SourceCatalog::new(
            path.parent()
                .ok_or_else(|| Error::input("jobdeck has no parent directory"))?,
        )?;
        catalog.temp = temp.map(Path::to_path_buf);
        catalog.probe_all(
            deck.sources_in(selected.as_ref(), chips.as_ref().map(|p| &p.blocks)),
            cancelled,
        )?;
        let mags = source_mags(&deck, &catalog);
        // a capture's window (render; jobdeck::window): the sources of the
        // placements it reaches are planned, the rest of the level is left
        // as it is - the placements from the deck and the headers probed,
        // as the open places them (a source without a dbu places nothing
        // and is not planned either way)
        let within = window
            .map(|w| {
                geom::plan(
                    &deck,
                    &catalog.dbus(),
                    &catalog.bad(),
                    &PlanOptions {
                        selected: selected.clone(),
                        chips: chips.as_ref().map(|p| p.blocks.clone()),
                        skip_missing: true,
                        ..Default::default()
                    },
                    cancelled,
                )
                .map(|model| w.sources(&model.placements))
            })
            .transpose()?;
        let (mut todo, mut kept, mut aliases, mut full_over_light, mut coarse_summaries) =
            (Vec::new(), 0, 0, 0, 0);
        let mut outside = 0usize;
        let mut destinations = BTreeSet::new();
        for (tc, info) in &catalog.infos {
            check_cancelled(cancelled)?;
            if !info.ok() {
                continue;
            }
            if within.as_ref().is_some_and(|s| !s.contains(tc)) {
                outside += 1;
                continue;
            }
            // Deduplicate lexical aliases for the same cache destination, but
            // not distinct source symlinks: those intentionally own own caches.
            if !destinations.insert(cache::cache_path(&info.path)?) {
                aliases += 1;
                continue;
            }
            let mut current = options.clone();
            // a source whose cache is the run's temporary index: its entry
            // builds there (and leaves nothing beside the source)
            current.out_dir = info.temp.then(|| info.cache_dir.clone());
            // the source's largest mag in the deck: floe-index's automatic
            // cell is at most 4 um of the deck (user 2026-10-28, a deck at
            // mag 4: "18147x12578um 뷰에서 요약이 하나만 쓰이면서 디테일이
            // 없어져버림"); mag 1 and below change nothing and are not said
            let mag = mags
                .get(tc)
                .copied()
                .filter(|&m| floe_vfs::occupancy::placed_ceiling_um(m).is_some());
            current.occupancy_mag = mag;
            // an explicit --full over a light cache: the full index is built
            // in its place (decide() builds over a light cache under a full
            // request); the plan kept the source as indexed before, so
            // `floe2 index deck.jb --full` ended with "kept" and the caches
            // light (review 2026-10-23, 0.12.390). Not for the auto-index
            // (`prepare_for_open`): it opens a cache as it is
            let light_under_full = explicit
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
            } else if explicit
                && options.occupancy.unwrap_or(true)
                && options.occupancy_um.is_none()
                && mag.is_some_and(|m| summary_coarser(&info.cache_dir, m, &options))
            {
                coarse_summaries += 1;
                current.occupancy = Some(false);
                current.occupancy_only = true;
            } else if info.temp || info.cache_dir == cache::default_cache_path(&info.path)? {
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
            chips,
            aliases,
            full_over_light,
            coarse_summaries,
            window_sources: within,
            outside,
        })
    }
}

/// Each source's largest mag in the whole deck (every level and CHIP block,
/// so one source's cell does not hang on the selection): SF x AD / the
/// source's dbu, as geom::plan places it; an entry without AD (geom refuses
/// it) or a source that did not probe has none.
pub fn source_mags(deck: &JobDeck, catalog: &SourceCatalog) -> BTreeMap<String, f64> {
    let mut mags: BTreeMap<String, f64> = BTreeMap::new();
    for e in deck.chips.iter().flat_map(|c| &c.entries) {
        let Some(dbu) = catalog.infos.get(&e.tc).and_then(|i| i.dbu) else {
            continue;
        };
        let Some(ad) = e.ad else {
            continue;
        };
        let mag = e.sf * ad / dbu;
        if !(mag.is_finite() && mag > 0.) {
            continue;
        }
        let slot = mags.entry(e.tc.clone()).or_insert(mag);
        *slot = slot.max(mag);
    }
    mags
}

/// Whether the design.ovo in `dir` has a cell coarser than the one a build
/// at `mag` picks now - floe-index's automatic choice over the file's own
/// top box and layer count under the caps such a build gets (the default
/// cell cap; `--occupancy-max-bytes` or its default): a summary built
/// before the mag rule, or by an index of the source alone. A file that
/// does not read is left as it is (the renderer says it, the occupancy
/// check names it).
fn summary_coarser(dir: &Path, mag: f64, options: &IndexOptions) -> bool {
    use floe_vfs::occupancy as occ;
    let Some(path) = dir.join("design.ovo").to_str().map(String::from) else {
        return false;
    };
    let Ok(file) = occ::OvoFile::open(&path) else {
        return false;
    };
    let (x0, y0, x1, y1) = file.bbox;
    let (um, _) = occ::auto_base_um_placed(
        (x1 as i128 - x0 as i128).max(0),
        (y1 as i128 - y0 as i128).max(0),
        file.unit,
        file.layers.len(),
        occ::DEFAULT_MAX_CELLS,
        options
            .occupancy_max_bytes
            .unwrap_or(occ::DEFAULT_MAX_BYTES),
        mag,
    );
    file.cell_dbu > occ::cell_dbu_of(um, file.unit)
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
    #[test]
    fn a_source_takes_its_largest_mag_in_the_deck() {
        // a.oas at 1 nm: AD 0.004 / dbu 0.001 = 4 (level 1), SF 2 at AD
        // 0.001 = 2 (level 2, another CHIP); b.oas 1 nm at mag 1; c.oas did
        // not probe; d.oas's entry has no AD
        let deck = JobDeck::parse(
            "x",
            "CHIP A\n$ (1,A,AD=0.004,TC=a.oas)\n$ (3,B,AD=0.001,TC=b.oas)\n\
             CHIP B\n$ (2,A,AD=0.001,SF=2,TC=a.oas)\n$ (1,C,AD=0.004,TC=c.oas)\n$ (1,D,TC=d.oas)",
            true,
            &AtomicUsize::new(0),
        )
        .unwrap();
        let dir = std::env::temp_dir();
        let mut catalog = SourceCatalog::new(&dir).unwrap();
        for (tc, dbu) in [
            ("a.oas", Some(0.001)),
            ("b.oas", Some(0.001)),
            ("c.oas", None),
            ("d.oas", Some(0.001)),
        ] {
            catalog.infos.insert(
                tc.into(),
                super::super::sources::SourceInfo {
                    tc: tc.into(),
                    path: dir.join(tc),
                    status: if dbu.is_some() {
                        "ok".into()
                    } else {
                        "missing".into()
                    },
                    dbu,
                    version: String::new(),
                    format: String::new(),
                    gzipped: false,
                    bytes: 0,
                    probe: String::new(),
                    probe_s: 0.,
                    error: String::new(),
                    indexed: false,
                    cache_dir: dir.join(format!(".{tc}.ice")),
                    busy: String::new(),
                    temp: false,
                },
            );
        }
        let mags = source_mags(&deck, &catalog);
        assert_eq!(mags.len(), 2, "{mags:?}");
        assert!((mags["a.oas"] - 4.0).abs() < 1e-12, "{mags:?}");
        assert!((mags["b.oas"] - 1.0).abs() < 1e-12, "{mags:?}");
        // the plan says a mag past 1 only
        assert_eq!(
            floe_vfs::occupancy::placed_ceiling_um(mags["a.oas"]),
            Some(1.0)
        );
        assert_eq!(floe_vfs::occupancy::placed_ceiling_um(mags["b.oas"]), None);
    }
    #[test]
    fn chip_selectors_pick_blocks_by_id_and_number() {
        let deck = JobDeck::parse(
            "x",
            "CHIP A\n$ (1,A,TC=a.oas)\nCHIP B2\n$ (1,B,TC=b.oas)\n$ (2,B,TC=c.oas)\nCHIP C\n$ (2,C,TC=d.oas)",
            true,
            &AtomicUsize::new(0),
        )
        .unwrap();
        // ids: exact, or a pattern as render --chip's
        let mut sel = ChipSelection::default();
        sel.add_ids("A, B?").unwrap();
        let pick = sel.resolve(&deck).unwrap();
        assert_eq!(
            (pick.blocks.clone(), pick.total, pick.labels.clone()),
            (BTreeSet::from([0, 1]), 3, vec!["1:A".to_string(), "2:B2".to_string()])
        );
        assert_eq!(deck.sources_in(None, Some(&pick.blocks)), ["a.oas", "b.oas", "c.oas"]);
        // --level cuts across the chips
        assert_eq!(
            deck.sources_in(Some(&BTreeSet::from([2])), Some(&pick.blocks)),
            ["c.oas"]
        );
        // numbers: 1-based, deck order, ranges
        let mut sel = ChipSelection::default();
        sel.add_numbers("3, 1-1").unwrap();
        assert_eq!(sel.numbers, [(3, 3), (1, 1)]);
        let pick = sel.resolve(&deck).unwrap();
        assert_eq!(deck.sources_in(None, Some(&pick.blocks)), ["a.oas", "d.oas"]);
        // a chip in either list is in
        sel.add_ids("B2").unwrap();
        assert_eq!(sel.resolve(&deck).unwrap().blocks, BTreeSet::from([0, 1, 2]));
        assert_eq!(deck.sources_in(None, None), deck.sources(None));
        // what the deck has not is said
        let mut bad = ChipSelection::default();
        bad.add_ids("Z*").unwrap();
        let e = bad.resolve(&deck).unwrap_err();
        assert!(e.message.contains("chip id(s) Z* not in the deck (3 CHIP block(s)"), "{}", e.message);
        let mut bad = ChipSelection::default();
        bad.add_numbers("2-4").unwrap();
        let e = bad.resolve(&deck).unwrap_err();
        assert!(e.message.contains("chip number(s) 2-4 not in the deck (it has 3 CHIP block(s)"), "{}", e.message);
        assert!(ChipSelection::default().add_numbers("0").is_err());
        assert!(ChipSelection::default().add_numbers("3-2").is_err());
        assert!(ChipSelection::default().add_numbers("x").is_err());
        assert!(ChipSelection::default().add_ids(" , ").is_err());
        assert!(ChipSelection::default().is_empty());
    }
}
