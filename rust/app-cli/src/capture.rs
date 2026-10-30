use crate::ready::Waiter;
use floe_app_core::{
    captures::{self, ExportOptions},
    check_cancelled,
    dataset::Dataset,
    jobdeck::{
        color::Mode,
        index::{is_deck, ChipSelection},
        plan::{Analysis, AnalysisOptions},
        window::{window_enabled, DeckReads},
    },
    render::RenderOptions,
    shots::batch::{self, Capture, NamedCapture},
    styles::LayerColors,
    Error, Result,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, mpsc, Arc},
    time::Duration,
};

pub(crate) fn stdin_text(cancelled: &Arc<AtomicUsize>) -> Result<String> {
    let flag = Arc::clone(cancelled);
    let (tx, rx) = mpsc::sync_channel(1);
    // CLI only: at most one bounded reader. On cancellation main exits the
    // process, so an EOF-less stdin cannot keep shutdown blocked by join().
    std::thread::Builder::new()
        .name("floe-text-stdin".into())
        .spawn(move || {
            let _ = tx.send(batch::read_from(std::io::stdin().lock(), &flag));
        })?;
    loop {
        check_cancelled(cancelled)?;
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(_) => return Err(Error::input("stdin reader stopped")),
        }
    }
}
#[allow(clippy::too_many_arguments)]
pub fn run(
    source: PathBuf,
    capture: Capture,
    out: PathBuf,
    report: Option<PathBuf>,
    batch: Option<String>,
    levels: Option<BTreeSet<i64>>,
    chips: Option<ChipSelection>,
    layer_colors: Option<LayerColors>,
    legend: bool,
    index: bool,
    light: Option<bool>,
    temp: Option<&std::path::Path>,
    waiter: &mut Waiter,
    cancelled: &Arc<AtomicUsize>,
) -> Result<i32> {
    if batch.is_none() {
        capture.validate()?;
    }
    let (shots, input) = if let Some(batch) = &batch {
        let text = if batch == "-" {
            stdin_text(cancelled)?
        } else {
            batch::read_file(std::path::Path::new(batch), cancelled)?
        };
        (
            batch::parse(&text, &capture)?,
            if batch == "-" {
                None
            } else {
                Some(PathBuf::from(batch))
            },
        )
    } else {
        let name = out
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("view")
            .to_string();
        (vec![NamedCapture { name, capture }], None)
    };
    // the shots read once; the open and the captures again (--wait) when
    // the source was refused after its open
    crate::read::again(&source, waiter, |waiter| {
        render(
            &source,
            &shots,
            ExportOptions {
                out: out.clone(),
                report: report.clone(),
                batch: batch.is_some(),
                input: input.clone(),
                legend,
            },
            levels.clone(),
            chips.as_ref(),
            layer_colors,
            index,
            light,
            temp,
            waiter,
            cancelled,
        )
    })
}

/// What a capture reads of a deck (jobdeck::window; user 2026-10-29: a
/// location's capture reads the EBEAM files there alone): the CHIP blocks
/// `--chipid` / `--chipnumber` name, and the window of the shots' regions -
/// so the open, the auto-index and the temporary index's summaries take the
/// sources those regions reach, not every source of the levels loaded. None
/// for a layout, for a shot of the whole deck (no region, no chips) and
/// under FLOE_DECK_WINDOW=off (the chips named still count). The regions of
/// the shots that name chips come from the deck's placements, read from the
/// deck and the sources' headers as the open reads them (no geometry).
fn deck_reads(
    source: &Path,
    shots: &[NamedCapture],
    levels: Option<&BTreeSet<i64>>,
    chips: Option<&ChipSelection>,
    temp: Option<&Path>,
    cancelled: &AtomicUsize,
) -> Result<Option<DeckReads>> {
    if !is_deck(source) {
        return Ok(None);
    }
    let chips = chips.filter(|c| !c.is_empty()).cloned();
    let windowed = window_enabled()
        && shots
            .iter()
            .all(|s| s.capture.explicit_region() || s.capture.names_chips());
    let window = if windowed {
        let analysis = Analysis::open(
            source,
            &AnalysisOptions {
                mode: Mode::Level,
                load_ids: levels.cloned(),
                skip_missing: true,
                temp: temp.map(Path::to_path_buf),
                chips: chips.clone(),
                ..Default::default()
            },
            cancelled,
        )?;
        captures::deck_window(&analysis, shots, cancelled)?
    } else {
        None
    };
    if chips.is_none() && window.is_none() {
        return Ok(None);
    }
    Ok(Some(DeckReads { chips, window }))
}

#[allow(clippy::too_many_arguments)]
fn render(
    source: &std::path::Path,
    shots: &[NamedCapture],
    options: ExportOptions,
    levels: Option<BTreeSet<i64>>,
    chips: Option<&ChipSelection>,
    layer_colors: Option<LayerColors>,
    index: bool,
    light: Option<bool>,
    temp: Option<&std::path::Path>,
    waiter: &mut Waiter,
    cancelled: &Arc<AtomicUsize>,
) -> Result<i32> {
    // indexed first when it has none (`light`: the kind), its staleness
    // said (read::dataset); a shot with --occupancy on has the missing
    // summaries built first, the default capture none (DIRECT_READ_PLAN
    // §8.11: a capture is drawn once); a deck's sources those the shots'
    // regions reach (`reads`)
    let summaries = shots.iter().any(|s| s.capture.shot.occupancy == Some(true));
    let reads = deck_reads(source, shots, levels.as_ref(), chips, temp, cancelled)?;
    let mut dataset = crate::read::dataset(
        source,
        levels.clone(),
        index,
        light,
        summaries,
        temp,
        reads.as_ref(),
        waiter,
        cancelled,
    )?;
    // --layer-colors over FLOE_LAYER_COLORS (the open's)
    if let Some(mode) = layer_colors {
        dataset.set_layer_colors(mode);
    }
    // what the capture reads of the deck: the CHIP blocks placed, and the
    // sources and placements of the levels loaded its regions reach
    if let Dataset::Deck(deck) = &dataset {
        if let Some(labels) = &deck.metadata.jobdeck.chip_blocks {
            let shown: Vec<&str> = labels.iter().take(32).map(String::as_str).collect();
            println!(
                "[jobdeck] blocks    : {} of {} CHIP block(s) placed (--chipid / --chipnumber): {}{}",
                labels.len(),
                deck.metadata.jobdeck.chips,
                shown.join(","),
                if labels.len() > shown.len() {
                    format!(",... (+{})", labels.len() - shown.len())
                } else {
                    String::new()
                }
            );
        }
        if let Some(w) = &deck.metadata.jobdeck.window {
            println!(
                "[jobdeck] window    : {} of {} source(s) read - {} of {} placement(s) in the capture's region(s) (FLOE_DECK_WINDOW=off reads them all)",
                w.sources, w.sources_total, w.placements, w.placements_total
            );
        }
    }
    // what the deck asked for that will not be drawn (a source without
    // dbu, a cache without the entry's LY/DT): said at every open, and
    // again by the exit status (floe/cli.py open_cache)
    for r in dataset.skipped() {
        println!(
            "[jobdeck] skipped   : CHIP {} level {} {}: {} ({})",
            r.chip, r.idx, r.tc, r.reason, r.detail
        );
    }
    let draw = |dataset: &Dataset, cancelled: &Arc<AtomicUsize>| {
        captures::run_summary(
            dataset,
            shots,
            &options,
            RenderOptions::local()?,
            cancelled,
            |line| println!("{line}"),
        )
    };
    let mut summary = draw(&dataset, cancelled)?;
    // a temporary index (--temp-index): no later `floe2 index DECK.jb` adds
    // the summaries a deck's wide passes wait for (DIRECT_READ_PLAN §5.5),
    // so the capture builds the missing ones of the levels loaded there -
    // the renderer's own judgment of which passes wait, a near capture
    // builds none - and draws again (with --occupancy off on every shot no
    // pass uses a summary: the pages instead, nothing waits)
    if temp.is_some()
        && summary.summary_building > 0
        && dataset.is_deck()
        && shots
            .iter()
            .any(|s| s.capture.shot.occupancy != Some(false))
    {
        eprintln!(
            "[jobdeck] {} pass(es) of light source(s) wait for their occupancy summary - building the missing summaries in the temporary index and drawing again (--occupancy off: the pages instead)",
            summary.summary_building
        );
        dataset = crate::read::dataset(
            source,
            levels,
            index,
            light,
            true,
            temp,
            reads.as_ref(),
            waiter,
            cancelled,
        )?;
        if let Some(mode) = layer_colors {
            dataset.set_layer_colors(mode);
        }
        summary = draw(&dataset, cancelled)?;
    }
    // what the images lack: the skipped placements of the chips on, the
    // pages over the decode budget, the passes of light sources whose
    // occupancy summary is not built yet (DIRECT_READ_PLAN §5.5)
    if summary.lacking > 0 || summary.over_budget > 0 || summary.summary_building > 0 {
        eprintln!(
            "{}: rendered incomplete - {} jobdeck placement(s) missing, {} page(s) over the decode budget{} (see above)  [exit 3]",
            floe_app_core::program(),
            summary.lacking,
            summary.over_budget,
            if summary.summary_building > 0 {
                format!(
                    ", summary_building={} pass(es) of light source(s) whose occupancy summary is not built yet (`{} index DECK.jb` adds it, or `render --occupancy on` builds it first)",
                    summary.summary_building,
                    floe_app_core::program()
                )
            } else {
                String::new()
            }
        );
    }
    Ok(if summary.complete { 0 } else { 3 })
}
