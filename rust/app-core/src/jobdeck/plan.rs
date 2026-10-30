//! Shared analysis service. Analysis selections and load selections are
//! deliberately different: only the former splice historical CHIP colors.
use super::{
    color::{ColorMap, ColorScheme, Mode},
    geom::{self, Placement, PlanOptions, PlannedDeck},
    index::{validate_levels, ChipPick, ChipSelection},
    parser::JobDeck,
    sources::SourceCatalog,
    view::ViewRows,
    window::{DeckWindow, WindowInfo},
};
use crate::{artifact, cache, check_cancelled, format_general as general, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

#[derive(Clone, Debug)]
pub struct AnalysisOptions {
    pub sources: Option<PathBuf>,
    pub ids: Option<BTreeSet<i64>>,
    pub load_ids: Option<BTreeSet<i64>>,
    pub mode: Mode,
    pub scheme: Option<ColorScheme>,
    pub cross: bool,
    pub skip_missing: bool,
    pub strict: bool,
    /// the run's temporary index (cache::TempIndex root): the sources'
    /// caches resolved under it when none is beside them
    pub temp: Option<PathBuf>,
    /// `render --chipid` / `--chipnumber` (jobdeck::window): these CHIP
    /// blocks' sources and placements alone; the other blocks are visited
    /// for the deck's grid and extent, as levels not loaded are
    pub chips: Option<ChipSelection>,
    /// a capture's window (jobdeck::window): the placements it reaches are
    /// read - the spec, the index plans, the summaries - and the rest of
    /// the load is left alone; the model holds them all
    pub window: Option<DeckWindow>,
}
impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            sources: None,
            ids: None,
            load_ids: None,
            mode: Mode::Level,
            scheme: None,
            cross: true,
            skip_missing: false,
            strict: true,
            temp: None,
            chips: None,
            window: None,
        }
    }
}
#[derive(Debug)]
pub struct Analysis {
    pub deck: JobDeck,
    pub catalog: SourceCatalog,
    pub model: PlannedDeck,
    pub scheme: ColorScheme,
    pub colors: ColorMap,
    /// the CHIP blocks `--chipid` / `--chipnumber` picked (None: every block)
    pub chips: Option<ChipPick>,
    /// the capture's window, and the sources of the placements it reaches
    /// (None: the whole load is read)
    pub window: Option<DeckWindow>,
    pub window_sources: Option<BTreeSet<String>>,
}
impl Analysis {
    pub fn open(path: &Path, options: &AnalysisOptions, cancelled: &AtomicUsize) -> Result<Self> {
        Self::open_pinned(path, options, None, cancelled)
    }
    /// Whether a placement of the load is read (the spec, renderd): in the
    /// capture's window, or no window.
    pub fn reads(&self, p: &Placement) -> bool {
        self.window.as_ref().is_none_or(|w| w.reaches(&p.bbox_um))
    }
    /// Whether a source of the load is read: one of a placement the window
    /// reaches, or no window.
    pub fn reads_source(&self, tc: &str) -> bool {
        self.window_sources.as_ref().is_none_or(|s| s.contains(tc))
    }
    /// What the window came to (None: no window).
    pub fn window_info(&self) -> Option<WindowInfo> {
        let (window, sources) = (self.window.as_ref()?, self.window_sources.as_ref()?);
        let total: BTreeSet<&str> = self
            .model
            .placements
            .iter()
            .map(|p| p.tc.as_str())
            .collect();
        Some(WindowInfo {
            rects_um: window.rects.clone(),
            sources: sources.len(),
            sources_total: total.len(),
            placements: self
                .model
                .placements
                .iter()
                .filter(|p| self.reads(p))
                .count(),
            placements_total: self.model.placements.len(),
        })
    }
    pub(crate) fn open_pinned(
        path: &Path,
        options: &AnalysisOptions,
        pinned: Option<std::sync::Arc<cache::revision::set::Snapshot>>,
        cancelled: &AtomicUsize,
    ) -> Result<Self> {
        check_cancelled(cancelled)?;
        let deck = JobDeck::read(path, options.strict, cancelled)?;
        let path = cache::absolute(path)?;
        let mut catalog = SourceCatalog::new(
            options
                .sources
                .as_deref()
                .unwrap_or(path.parent().expect("absolute path")),
        )?;
        catalog.pinned = pinned;
        catalog.temp = options.temp.clone();
        // Empty LOAD means all, as in the GTK load dialog. Empty ANALYSIS
        // selection means no placements and must not be silently widened.
        let load = options.load_ids.as_ref().filter(|ids| !ids.is_empty());
        validate_levels(&deck, load)?;
        // the CHIP blocks --chipid / --chipnumber pick (every block unsaid):
        // their sources are probed, the others' headers read for the grid
        let chips = options
            .chips
            .as_ref()
            .filter(|c| !c.is_empty())
            .map(|c| c.resolve(&deck))
            .transpose()?;
        let blocks = chips.as_ref().map(|p| &p.blocks);
        catalog.probe_all(deck.sources_in(load, blocks), cancelled)?;
        let mut dbus = catalog.dbus();
        if load.is_some() || blocks.is_some() {
            for tc in deck.sources(None) {
                if !catalog.infos.contains_key(tc) {
                    if let Some(dbu) = catalog.header_dbu(tc, cancelled)? {
                        dbus.insert(tc.into(), dbu);
                    }
                }
            }
        }
        let mut scheme = options.scheme.clone().unwrap_or_else(|| ColorScheme {
            mode: options.mode,
            cross_ly_dt: options.cross,
            ..Default::default()
        });
        scheme.mode = options.mode;
        scheme.validate()?;
        let model = geom::plan(
            &deck,
            &dbus,
            &catalog.bad(),
            &PlanOptions {
                cross: scheme.cross_ly_dt,
                by_chip: scheme.mode == Mode::Chip,
                selected: load.or(options.ids.as_ref()).cloned(),
                chips: blocks.cloned(),
                skip_missing: options.skip_missing,
                ..Default::default()
            },
            cancelled,
        )?;
        let colors = scheme.build(&deck, options.ids.as_ref())?;
        // the capture's window: the sources of the placements it reaches
        let window_sources = options
            .window
            .as_ref()
            .map(|w| w.sources(&model.placements));
        check_cancelled(cancelled)?;
        Ok(Self {
            deck,
            catalog,
            model,
            scheme,
            colors,
            chips,
            window: options.window.clone(),
            window_sources,
        })
    }
    pub fn view_rows(&self, cancelled: &AtomicUsize) -> Result<ViewRows> {
        ViewRows::build(
            &self.deck,
            &self.model.stats,
            &self.scheme,
            &self.colors,
            cancelled,
        )
    }
    pub fn report(&self) -> Result<Value> {
        let mut stats = serde_json::to_value(&self.model.stats).expect("finite plan statistics");
        stats["colors"] = self.colors.report(self.scheme.mode);
        stats["sources"] = self.catalog.report();
        Ok(json!({"deck":self.deck.report()?,"plan":stats,"placements":self.model.placements}))
    }
    /// Local export preflight, including unselected/missing source names.
    /// A report must never replace the deck, an OASIS or a cache/lock through
    /// an alias. No read lease/hot-reload guarantee is implied here.
    pub fn output_path(&self, path: &Path, extra_inputs: &[PathBuf]) -> Result<PathBuf> {
        self.output_path_mode(path, extra_inputs, false)
    }
    pub(crate) fn output_path_mode(
        &self,
        path: &Path,
        extra_inputs: &[PathBuf],
        planned: bool,
    ) -> Result<PathBuf> {
        let mut files = vec![PathBuf::from(&self.deck.path)];
        files.extend_from_slice(extra_inputs);
        let mut trees = Vec::new();
        trees.push(
            cache::revision::set::Store::new(Path::new(&self.deck.path))?
                .path()
                .to_owned(),
        );
        for tc in self.deck.sources(None) {
            let source = self.catalog.resolve(tc);
            let mut source_trees = cache::cache_paths(&source)?.to_vec();
            source_trees.push(cache::revision::Store::new(&source)?.path().to_owned());
            source_trees.push(cache::revision::set::Store::new(&source)?.path().to_owned());
            for directory in source_trees {
                let mut lock = directory.as_os_str().to_owned();
                lock.push(".index.lock");
                files.push(lock.into());
                trees.push(directory);
            }
            files.push(source);
        }
        let mut set_lock = cache::revision::set::Store::new(Path::new(&self.deck.path))?
            .path()
            .as_os_str()
            .to_owned();
        set_lock.push(".index.lock");
        files.push(set_lock.into());
        artifact::protected_output_mode(path, &files, &trees, planned)
    }
    pub fn summary(&self) -> Result<Vec<String>> {
        let stats = &self.model.stats;
        let src = self.catalog.report();
        let coverage = self.deck.coverage()?;
        let levels: Vec<_> = self.deck.levels().into_iter().collect();
        let selection = stats
            .selection
            .as_ref()
            .map(|s| s.iter().copied().collect::<Vec<_>>());
        let mut lines = vec![
            format!(
                "deck      : {}{}",
                self.deck.path,
                if self.deck.name().is_empty() {
                    String::new()
                } else {
                    format!("  ({})", self.deck.name())
                }
            ),
            format!(
                "chips     : {}  levels {:?}  ({} complete, {} partial)",
                self.deck.chips.len(),
                levels,
                coverage["complete"],
                coverage["partial"]
            ),
            format!(
                "sources   : {} probed, {} ok, {} indexed  dir {}",
                src["probed"],
                src["ok"],
                src["indexed"],
                self.catalog.directory.display()
            ),
            format!(
                "instances : {} placed{}",
                stats.instances,
                selection.map_or_else(String::new, |s| format!(
                    " of {} (selection {s:?})",
                    stats.instances_total
                ))
            ),
            format!(
                "grid      : dbu {} um ({})",
                general(stats.dbu),
                stats.dbu_choice.why
            ),
            format!(
                "residual  : max {} um, {} non-zero, {} over half a dbu",
                general(stats.residual_max_um),
                stats.residual_nonzero,
                stats.residual_over_half_dbu
            ),
            format!("mag       : {:?}", stats.mags),
            format!(
                "bbox um   : {}",
                stats.bbox_um.map_or_else(
                    || "none".into(),
                    |b| format!("{:.4} {:.4} {:.4} {:.4}", b[0], b[1], b[2], b[3])
                )
            ),
            format!(
                "view      : {} -> {}",
                match self.scheme.mode {
                    Mode::Level => "level view (by mask level)",
                    Mode::Chip => "chip view (by CHIP block)",
                    Mode::Layer => "source layer view (LY/DT)",
                },
                self.colors
                    .order
                    .iter()
                    .map(|r| format!(
                        "{} {}{}",
                        r.label(),
                        r.name,
                        if r.source == "pinned" {
                            " (pinned)"
                        } else {
                            ""
                        }
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ];
        if let Some(pick) = &self.chips {
            lines.push(format!(
                "blocks    : {} of {} CHIP block(s) placed (--chipid / --chipnumber): {}",
                pick.blocks.len(),
                pick.total,
                pick.labels
                    .iter()
                    .take(32)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if let Some(w) = self.window_info() {
            lines.push(format!(
                "window    : {} of {} source(s) read - {} of {} placement(s) in the capture's region(s)",
                w.sources, w.sources_total, w.placements, w.placements_total
            ));
        }
        for i in self.catalog.infos.values().filter(|i| i.status != "ok") {
            lines.push(format!(
                "source    : {} {} ({})",
                i.tc,
                i.status.to_ascii_uppercase(),
                i.error
            ));
        }
        for r in &stats.skipped {
            lines.push(format!(
                "skipped   : CHIP {} level {} {}: {} ({}, {} row(s))",
                r.chip, r.idx, r.tc, r.reason, r.detail, r.rows
            ));
        }
        for r in &stats.deck_issues {
            lines.push(format!(
                "outside   : CHIP {} level {} {}: {} (not selected)",
                r.chip, r.idx, r.tc, r.reason
            ));
        }
        for (line, msg) in &self.deck.errors {
            lines.push(format!("error     : line {line}: {msg}"));
        }
        for (line, msg) in &self.deck.warnings {
            lines.push(format!("warning   : line {line}: {msg}"));
        }
        let report = self.deck.report()?;
        let extras = report["undocumented_entry_fields"]
            .as_object()
            .expect("extras object");
        if !extras.is_empty() {
            lines.push(format!(
                "unknown $ : {}",
                extras
                    .iter()
                    .map(|(k, v)| format!("{k} ({})", v.as_array().expect("extras rows").len()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some((n, s)) = self.deck.unknown.first() {
            lines.push(format!(
                "unparsed  : {} line(s), first: {n}: {s}",
                self.deck.unknown.len()
            ));
        }
        Ok(lines)
    }
    pub fn placement_line(&self, p: &geom::Placement) -> String {
        format!(
            "{:<8} {:<5} {:<4} {:<24} {:<8} {:<9} {:16.4} {:16.4} {:>8}",
            p.chip,
            p.idx,
            p.row,
            p.tc,
            format!("{}/{}", p.ly, p.dt),
            general(p.mag),
            p.dx_um,
            p.dy_um,
            self.scheme.color_of(&self.colors, p)
        )
    }
}
