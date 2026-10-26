use crate::ready::Waiter;
use floe_app_core::{
    captures::{self, ExportOptions},
    check_cancelled,
    render::RenderOptions,
    shots::batch::{self, Capture, NamedCapture},
    styles::LayerColors,
    Error, Result,
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
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
    layer_colors: Option<LayerColors>,
    legend: bool,
    index: bool,
    light: Option<bool>,
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
            layer_colors,
            index,
            light,
            waiter,
            cancelled,
        )
    })
}
#[allow(clippy::too_many_arguments)]
fn render(
    source: &std::path::Path,
    shots: &[NamedCapture],
    options: ExportOptions,
    levels: Option<BTreeSet<i64>>,
    layer_colors: Option<LayerColors>,
    index: bool,
    light: Option<bool>,
    waiter: &mut Waiter,
    cancelled: &Arc<AtomicUsize>,
) -> Result<i32> {
    // indexed first when it has none (`light`: the kind), its staleness
    // said (read::dataset); a shot with --occupancy on has the missing
    // summaries built first, the default capture none (DIRECT_READ_PLAN
    // §8.11: a capture is drawn once)
    let summaries = shots.iter().any(|s| s.capture.shot.occupancy == Some(true));
    let mut dataset = crate::read::dataset(source, levels, index, light, summaries, waiter, cancelled)?;
    // --layer-colors over FLOE_LAYER_COLORS (the open's)
    if let Some(mode) = layer_colors {
        dataset.set_layer_colors(mode);
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
    let summary = captures::run_summary(
        &dataset,
        shots,
        &options,
        RenderOptions::local()?,
        cancelled,
        |line| println!("{line}"),
    )?;
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
