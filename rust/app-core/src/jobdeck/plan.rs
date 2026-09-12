//! Shared analysis service. Analysis selections and load selections are
//! deliberately different: only the former splice historical CHIP colors.
use super::{
    color::{ColorMap, ColorScheme, Mode},
    geom::{self, PlanOptions, PlannedDeck},
    index::validate_levels,
    parser::{general, JobDeck},
    sources::SourceCatalog,
    view::ViewRows,
};
use crate::{artifact, cache, check_cancelled, Result};
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
}
impl Analysis {
    pub fn open(path: &Path, options: &AnalysisOptions, cancelled: &AtomicUsize) -> Result<Self> {
        check_cancelled(cancelled)?;
        let deck = JobDeck::read(path, options.strict, cancelled)?;
        let path = cache::absolute(path)?;
        let mut catalog = SourceCatalog::new(
            options
                .sources
                .as_deref()
                .unwrap_or(path.parent().expect("absolute path")),
        )?;
        // Empty LOAD means all, as in the GTK load dialog. Empty ANALYSIS
        // selection means no placements and must not be silently widened.
        let load = options.load_ids.as_ref().filter(|ids| !ids.is_empty());
        validate_levels(&deck, load)?;
        catalog.probe_all(deck.sources(load), cancelled)?;
        let mut dbus = catalog.dbus();
        if load.is_some() {
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
                skip_missing: options.skip_missing,
                ..Default::default()
            },
            cancelled,
        )?;
        let colors = scheme.build(&deck, options.ids.as_ref())?;
        check_cancelled(cancelled)?;
        Ok(Self {
            deck,
            catalog,
            model,
            scheme,
            colors,
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
        let mut files = vec![PathBuf::from(&self.deck.path)];
        files.extend_from_slice(extra_inputs);
        let mut trees = Vec::new();
        for tc in self.deck.sources(None) {
            let source = self.catalog.resolve(tc);
            let directory = cache::cache_path(&source)?;
            let mut lock = directory.as_os_str().to_owned();
            lock.push(".index.lock");
            files.push(source);
            files.push(lock.into());
            trees.push(directory);
        }
        artifact::protected_output(path, &files, &trees)
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
                "sources   : {} probed, {} ok, {} indexed (.floe)  dir {}",
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
                        r.key,
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
                "skipped   : CHIP {} ${} {}: {} ({}, {} row(s))",
                r.chip, r.idx, r.tc, r.reason, r.detail, r.rows
            ));
        }
        for r in &stats.deck_issues {
            lines.push(format!(
                "outside   : CHIP {} ${} {}: {} (not selected)",
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
            "{:<8} ${:<3} {:<4} {:<24} {:<8} {:<9} {:16.4} {:16.4} {:>8}",
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
