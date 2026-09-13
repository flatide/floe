use floe_app_core::{
    captures::{self, ExportOptions},
    check_cancelled,
    dataset::Dataset,
    jobdeck::color::Mode,
    render::RenderOptions,
    shots::batch::{self, Capture, NamedCapture},
    Error, Result,
};
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{atomic::AtomicUsize, mpsc, Arc},
    time::Duration,
};

fn stdin_text(cancelled: &Arc<AtomicUsize>) -> Result<String> {
    let flag = Arc::clone(cancelled);
    let (tx, rx) = mpsc::sync_channel(1);
    // CLI only: at most one bounded reader. On cancellation main exits the
    // process, so an EOF-less stdin cannot keep shutdown blocked by join().
    std::thread::Builder::new()
        .name("floe-batch-stdin".into())
        .spawn(move || {
            let _ = tx.send(batch::read_from(std::io::stdin().lock(), &flag));
        })?;
    loop {
        check_cancelled(cancelled)?;
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(_) => return Err(Error::input("batch stdin reader stopped")),
        }
    }
}
pub fn run(
    source: PathBuf,
    capture: Capture,
    out: PathBuf,
    report: Option<PathBuf>,
    batch: Option<String>,
    levels: Option<BTreeSet<i64>>,
    cancelled: &Arc<AtomicUsize>,
) -> Result<i32> {
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
    let dataset = Dataset::open(&source, levels, Mode::Level, cancelled)?;
    if dataset.source_stale() {
        eprintln!("[floe2-web][warn] source changed; displaying cached geometry. Rebuild with index --force, then reopen.");
    }
    let options = ExportOptions {
        out,
        report,
        batch: batch.is_some(),
        input,
    };
    let complete = captures::run(
        &dataset,
        &shots,
        &options,
        RenderOptions::local()?,
        cancelled,
        |line| println!("{line}"),
    )?;
    if !complete {
        for r in dataset.skipped() {
            eprintln!(
                "[jobdeck] skipped: CHIP {} ${} {}: {} ({})",
                r.chip, r.idx, r.tc, r.reason, r.detail
            );
        }
        eprintln!(
            "[floe2-web] rendered INCOMPLETE; see per-shot skipped/over-budget counts [exit 3]"
        );
    }
    Ok(if complete { 0 } else { 3 })
}
