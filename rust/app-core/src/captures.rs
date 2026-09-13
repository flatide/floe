//! Headless named captures, one native worker, per-artifact atomic commit.
use crate::{
    artifact::{self, StagedArtifact},
    check_cancelled,
    dataset::Dataset,
    render::{require_complete, RenderOptions, RenderSession},
    shots::{
        batch::{NamedCapture, TILE_NAMES},
        mosaic::{self, Mosaic},
    },
    Error, ErrorKind, Result,
};
use floe_worker_client::FrameFormat;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Arc},
    time::Instant,
};

pub struct ExportOptions {
    pub out: PathBuf,
    pub report: Option<PathBuf>,
    pub batch: bool,
    pub input: Option<PathBuf>,
}
struct Plan<'a> {
    named: &'a NamedCapture,
    tiles: Vec<crate::shots::Shot>,
    display: PathBuf,
    target: PathBuf,
    keep: Vec<PathBuf>,
}
fn sidecars(path: &Path) -> Result<Vec<PathBuf>> {
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::input("output filename must be UTF-8"))?;
    let stem = if filename.to_ascii_lowercase().ends_with(".png") {
        &filename[..filename.len() - 4]
    } else {
        filename
    };
    Ok(TILE_NAMES
        .iter()
        .map(|tag| path.with_file_name(format!("{stem}_{tag}.png")))
        .collect())
}

pub fn run(
    dataset: &Dataset,
    shots: &[NamedCapture],
    options: &ExportOptions,
    render: RenderOptions,
    cancelled: &Arc<AtomicUsize>,
    mut log: impl FnMut(&str),
) -> Result<bool> {
    check_cancelled(cancelled)?;
    if shots.is_empty() || (!options.batch && shots.len() != 1) {
        return Err(Error::input("invalid capture count"));
    }
    if options.batch
        && options
            .out
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".png")
    {
        return Err(Error::input("with --batch, --out is a directory"));
    }
    let input = options.input.as_ref().map(fs::canonicalize).transpose()?;
    let mut targets = BTreeSet::new();
    let mut check_target = |path: &Path| -> Result<PathBuf> {
        let target = dataset.output_path_mode(path, options.batch)?;
        if input
            .as_ref()
            .is_some_and(|p| p == &target || fs::canonicalize(&target).ok().as_ref() == Some(p))
        {
            return Err(Error::input("output must not replace the batch input"));
        }
        // A mosaic sidecar can collide with another shot/report. Reject also
        // case-only names so the same plan is safe on macOS and Linux.
        if !targets.insert(target.to_string_lossy().to_lowercase()) {
            return Err(Error::input(
                "output/report/kept tile paths collide (including case-only names)",
            ));
        }
        Ok(target)
    };
    let mut plans = Vec::new();
    for named in shots {
        let tiles = named.capture.tiles(dataset.bbox_um())?;
        if dataset.is_deck() && named.capture.shot.labels {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "jobdeck labels are not supported",
            ));
        }
        dataset.resolve_layers(named.capture.shot.layers.as_deref())?;
        for tile in &tiles {
            if tile
                .bbox
                .unwrap()
                .iter()
                .any(|n| !(n / dataset.dbu()).is_finite())
            {
                return Err(Error::input("DBU conversion overflow"));
            }
        }
        let display = if options.batch {
            options.out.join(format!("{}.png", named.name))
        } else {
            options.out.clone()
        };
        let target = check_target(&display)?;
        let keep = if named.capture.is_mosaic() && named.capture.keep_tiles {
            sidecars(&display)?
                .iter()
                .map(|p| check_target(p))
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        plans.push(Plan {
            named,
            tiles,
            display,
            target,
            keep,
        });
    }
    let report = options
        .report
        .as_ref()
        .map(|p| check_target(p))
        .transpose()?;
    for path in &targets {
        if Path::new(path)
            .ancestors()
            .skip(1)
            .any(|p| targets.contains(p.to_string_lossy().as_ref()))
        {
            return Err(Error::input(
                "an output file is also another output's parent directory",
            ));
        }
    }
    if let Some(path) = &report {
        // Only the explicit batch directory will be created below. A report
        // elsewhere must already have a valid parent, before any mkdir/worker.
        if !options.batch
            || path.parent() != Some(artifact::resolve_prefix(&options.out)?.as_path())
        {
            dataset.output_path(path)?;
        }
    }
    if options.batch {
        // All destinations/values were checked with missing-parent resolution
        // before creating the explicit output directory. Never create parents
        // for an unrelated report path.
        fs::create_dir_all(&options.out)?;
    }
    for p in &plans {
        for target in std::iter::once(&p.target).chain(&p.keep) {
            dataset.output_path(target)?;
        }
    }
    if let Some(r) = &report {
        dataset.output_path(r)?;
    }
    let mut session = RenderSession::open(dataset, render, true, Arc::clone(cancelled))?;
    let mut rows = Vec::new();
    let mut artifact_count = 0usize;
    for (index, plan) in plans.iter().enumerate() {
        let result = (|| -> Result<serde_json::Value> {
            let started = Instant::now();
            let capture = &plan.named.capture;
            let (width, height) = (plan.tiles[0].pixels.0, plan.tiles[0].pixels.1.unwrap());
            let mut canvas = capture
                .is_mosaic()
                .then(|| Mosaic::new(width, height))
                .transpose()?;
            let mut kept = Vec::new();
            let mut png = None;
            let mut deferred = 0u64;
            let mut all_complete = dataset.skipped().is_empty();
            for (i, tile) in plan.tiles.iter().enumerate() {
                let mut request = session.shot_request(dataset, tile)?;
                if canvas.is_some() {
                    request.format = FrameFormat::Raw;
                }
                let frame = session.capture(request)?;
                if !dataset.is_deck()
                    || frame.labels_truncated
                    || (frame.partial && frame.deferred == 0)
                {
                    require_complete(&frame)?;
                }
                all_complete &= frame.complete();
                deferred = deferred
                    .checked_add(frame.deferred)
                    .ok_or_else(|| Error::input("deferred-page count overflow"))?;
                if let Some(canvas) = &mut canvas {
                    let rgba = &frame.bytes[16..]; // worker client validated FLOERAW1/size
                    canvas.insert(rgba, cancelled)?;
                    if !plan.keep.is_empty() {
                        kept.push(StagedArtifact::write(&plan.keep[i], cancelled, |out| {
                            mosaic::encode_png(out, width, height, rgba, cancelled)
                        })?);
                    }
                } else {
                    png = Some(frame.bytes);
                }
            }
            if index + 1 == plans.len() {
                session.close()?;
            }
            let mut row = serde_json::json!({"name":plan.named.name,"out":plan.display,"pixel":[width,height],
                "layers":capture.shot.layers,"depth":capture.shot.depth,"over_budget_pages":deferred,
                "skipped_placements":dataset.skipped().len(),"complete":all_complete});
            let staged = if let Some(canvas) = canvas {
                let (rgba, info) = canvas.finish(capture.line, &capture.line_color, cancelled)?;
                row["pixel"] = serde_json::json!([width * 2, height * 2]);
                row["mosaic"] = info;
                row["tiles"] = TILE_NAMES
                    .iter()
                    .zip(&plan.tiles)
                    .map(|(tag, tile)| (tag.to_string(), serde_json::json!(tile.bbox.unwrap())))
                    .collect::<serde_json::Map<_, _>>()
                    .into();
                StagedArtifact::write(&plan.target, cancelled, |out| {
                    mosaic::encode_png(out, width * 2, height * 2, &rgba, cancelled)
                })?
            } else {
                row["bbox_um"] = serde_json::json!(plan.tiles[0].bbox.unwrap());
                StagedArtifact::write(&plan.target, cancelled, |out| {
                    use std::io::Write;
                    for chunk in png.as_ref().unwrap().chunks(1024 * 1024) {
                        check_cancelled(cancelled)?;
                        out.write_all(chunk)?;
                    }
                    Ok(())
                })?
            };
            for target in std::iter::once(&plan.target).chain(&plan.keep) {
                dataset.output_path(target)?;
            }
            for stage in std::iter::once(staged).chain(kept) {
                stage.commit(cancelled)?;
                artifact_count += 1;
            }
            row["ms"] = serde_json::json!(
                (started.elapsed().as_secs_f64() * 1000.).round_ties_even() as u64
            );
            log(&format!(
                "[floe2-web] rendered {} ({}x{}, {} ms{})",
                plan.display.display(),
                row["pixel"][0],
                row["pixel"][1],
                row["ms"],
                if all_complete { "" } else { ", INCOMPLETE" }
            ));
            if deferred != 0 {
                log(&format!(
                    "[floe2-web] WARNING: {} has {deferred} over-budget page(s)",
                    plan.display.display()
                ));
            }
            Ok(row)
        })();
        let row = result.map_err(|e| {
            Error::new(
                e.kind,
                format!(
                    "shot {} failed; {artifact_count} artifact(s) already saved: {e}",
                    plan.named.name
                ),
            )
        })?;
        rows.push(row);
    }
    let complete = rows.iter().all(|r| r["complete"] == true);
    if let Some(path) = report {
        let mut doc = serde_json::json!({"source":dataset.source(),"dbu":dataset.dbu(),"shots":rows,
            "cut_px":shots[0].capture.shot.detail.cut_px(),"thin":shots[0].capture.shot.thin.name(),"complete":complete});
        if let Dataset::Deck(deck) = dataset {
            let over_budget = doc["shots"]
                .as_array()
                .unwrap()
                .iter()
                .try_fold(0u64, |n, r| {
                    n.checked_add(r["over_budget_pages"].as_u64().unwrap())
                })
                .ok_or_else(|| Error::input("report page count overflow"))?;
            doc["jobdeck"] = serde_json::json!({"complete":complete,"skipped":dataset.skipped(),"over_budget_pages":over_budget,
                "view":deck.metadata.jobdeck.mode,"levels":deck.metadata.jobdeck.levels});
        }
        let result = (|| {
            let bytes = artifact::json_bytes(&doc, cancelled)?;
            dataset.output_path(&path)?;
            artifact::publish(&path, &bytes, cancelled)
        })();
        result.map_err(|e| Error::new(e.kind, format!("PNG was saved; report failed: {e}")))?;
        log(&format!(
            "[floe2-web] report {} ({} shot(s))",
            path.display(),
            shots.len()
        ));
    }
    if !complete {
        log(&format!("[floe2-web] rendered INCOMPLETE: {} skipped placement(s), see per-shot over-budget counts [exit 3]",dataset.skipped().len()));
    }
    Ok(complete)
}
