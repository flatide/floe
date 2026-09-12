use floe_app_core::{
    artifact,
    dataset::Dataset,
    jobdeck::{
        color::Mode,
        index::{is_deck, parse_levels},
    },
    render::{require_complete, RenderOptions, RenderSession},
    shots::{self, Anchor, Detail, Shot, Thin},
    Error, ErrorKind, Result,
};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{atomic::AtomicUsize, Arc};
use std::time::Instant;

const RENDER_HELP: &str = "Usage: floe2-web render SOURCE [OPTIONS]
  --bbox X0,Y0,X1,Y1          Region (default whole source)
  --at X,Y --size W,H         Alternative region form
  --anchor center|lb          Aspect/position anchor (default center)
  --px W|WxH                 Default 1200; height follows aspect
  --stretch                  Do not expand bbox to explicit WxH aspect
  --layers NAME,L/D,...       Layer names/aliases or pairs (default all)
  --level N,N,...             Jobdeck: load these mask levels only
  --depth N                  0=top, 999/omit=full
  --detail exact|low|medium|high  Size cut 0/5/3/1 px (default exact)
  --thin auto|keep|cull       Auto=layout cull, jobdeck keep
  --frames --labels          Enable these overlays (default off)
  --label-font-px N          6..96 (default 14)
  --out FILE                Default view.png; atomic PNG publication
  --report FILE             JSON single-shot report
Lengths: bare/um/µm/μm, nm, mm, cm, m. Fractional DBU is preserved.
Mosaic/batch/DRC/metadata exports require later stages. Jobdeck labels unsupported.
Frames exceeding 16 Mpx are rejected, not silently rescaled.
Cancelled/failed frames never replace an existing PNG. Jobdeck known skipped
placements/over-budget pages may publish a flagged incomplete PNG (exit 3).
Other incomplete frames preserve the previous output.";
const INFO_HELP: &str = "Usage: floe2-web info SOURCE [--level N,N,...] [--json]
Read layout/cache summary. --json emits metadata plus source_stale.
No indexing is performed. Jobdeck info includes the skip ledger and virtual layers;
--level is a jobdeck load selection. --json uses cache:null for composites.";
const PROBE_HELP: &str = "Usage: floe2-web probe SOURCE
Headless ready/open/style + fit/center frame checks; no output files.
This validates the worker, not browser display or ETX performance.";

pub enum Command {
    Help(&'static str),
    Info {
        source: PathBuf,
        json: bool,
        levels: Option<BTreeSet<i64>>,
    },
    Render {
        source: PathBuf,
        shot: Shot,
        out: PathBuf,
        report: Option<PathBuf>,
        levels: Option<BTreeSet<i64>>,
    },
    Probe(PathBuf),
}
pub fn parse(args: &[String]) -> Result<Command> {
    let kind = args[0].as_str();
    let mut shot = Shot::default();
    let mut source = None;
    let mut out = PathBuf::from("view.png");
    let mut report = None;
    let mut json = false;
    let mut levels = None;
    let mut positional = false;
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if !positional && arg == "--" {
            positional = true;
            continue;
        }
        if positional || !arg.starts_with('-') {
            if source.replace(PathBuf::from(arg)).is_some() {
                return Err(Error::input(format!("{kind} accepts one source")));
            }
            continue;
        }
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
        let mut value = || -> Result<&str> {
            if let Some(v) = inline {
                return Ok(v);
            }
            let v = args
                .get(i)
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| Error::input(format!("{flag} requires a value")))?;
            i += 1;
            Ok(v)
        };
        let no_value = || {
            if inline.is_some() {
                Err(Error::input(format!("{flag} takes no value")))
            } else {
                Ok(())
            }
        };
        if matches!(flag, "--help" | "-h") {
            no_value()?;
            return Ok(Command::Help(match kind {
                "info" => INFO_HELP,
                "probe" => PROBE_HELP,
                _ => RENDER_HELP,
            }));
        }
        if kind == "info" && flag == "--json" {
            no_value()?;
            json = true;
            continue;
        }
        if kind != "probe" && flag == "--level" {
            levels = Some(parse_levels(value()?)?);
            continue;
        }
        if kind != "render" {
            return Err(Error::input(format!("unsupported {kind} option: {flag}")));
        }
        match flag {
            "--bbox" => shot.bbox = Some(shots::lengths(value()?)?),
            "--at" => shot.at = Some(shots::lengths(value()?)?),
            "--size" => shot.size = Some(shots::lengths(value()?)?),
            "--px" => shot.pixels = shots::pixels(value()?)?,
            "--anchor" => {
                shot.anchor = match value()? {
                    "center" => Anchor::Center,
                    "lb" => Anchor::LowerLeft,
                    _ => return Err(Error::input("anchor must be center or lb")),
                }
            }
            "--depth" => {
                let n: i64 = super::number(value()?, flag)?;
                shot.depth = if n >= 999 {
                    None
                } else {
                    Some(n.max(0) as u32)
                };
            }
            "--detail" => {
                shot.detail = match value()? {
                    "exact" => Detail::Exact,
                    "low" => Detail::Low,
                    "medium" => Detail::Medium,
                    "high" => Detail::High,
                    _ => return Err(Error::input("invalid detail")),
                }
            }
            "--thin" => {
                shot.thin = match value()? {
                    "auto" => Thin::Auto,
                    "keep" => Thin::Keep,
                    "cull" => Thin::Cull,
                    _ => return Err(Error::input("invalid thin policy")),
                }
            }
            "--layers" => {
                let v = value()?;
                shot.layers = if v.is_empty() || v == "all" {
                    None
                } else {
                    Some(v.into())
                };
            }
            "--label-font-px" => shot.font_px = super::number(value()?, flag)?,
            "--out" => out = value()?.into(),
            "--report" => report = Some(PathBuf::from(value()?)),
            "--stretch" => {
                no_value()?;
                shot.stretch = true;
            }
            "--frames" => {
                no_value()?;
                shot.frames = true;
            }
            "--labels" => {
                no_value()?;
                shot.labels = true;
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    format!(
                    "unsupported render option: {flag}; see render --help for this stage's scope"
                ),
                ))
            }
        }
    }
    let source = source.ok_or_else(|| Error::input(format!("{kind} requires SOURCE")))?;
    if levels.is_some() && !is_deck(&source) {
        return Err(Error::input("--level requires a jobdeck"));
    }
    if is_deck(&source) && shot.labels {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "jobdeck labels are not supported",
        ));
    }
    match kind {
        "info" => Ok(Command::Info {
            source,
            json,
            levels,
        }),
        "probe" => Ok(Command::Probe(source)),
        _ => {
            shot.validate()?;
            Ok(Command::Render {
                source,
                shot,
                out,
                report,
                levels,
            })
        }
    }
}
fn dataset(
    source: &std::path::Path,
    levels: Option<BTreeSet<i64>>,
    cancelled: &AtomicUsize,
) -> Result<Dataset> {
    let l = Dataset::open(source, levels, Mode::Level, cancelled)?;
    if l.source_stale() {
        eprintln!("[floe2-web][warn] source changed; displaying cached geometry. Rebuild with index --force, then reopen.");
    }
    Ok(l)
}
pub fn run(command: Command, cancelled: &Arc<AtomicUsize>) -> Result<i32> {
    match command {
        Command::Help(help) => println!("{help}"),
        Command::Info {
            source,
            json,
            levels,
        } => {
            let l = dataset(&source, levels, cancelled)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&l.info())
                        .map_err(|e| Error::input(e.to_string()))?
                );
            } else {
                print!("{}", l.summary(cancelled)?);
            }
        }
        Command::Render {
            source,
            shot,
            out,
            report,
            levels,
        } => {
            let l = dataset(&source, levels, cancelled)?;
            // All caller values/targets are checked before starting a worker.
            let (bb, width, height) = shot.fitted(l.bbox_um())?;
            l.resolve_layers(shot.layers.as_deref())?;
            let target = l.output_path(&out)?;
            let report_target = report.as_ref().map(|p| l.output_path(p)).transpose()?;
            if report_target.as_ref() == Some(&target) {
                return Err(Error::input("PNG and report paths must differ"));
            }
            let mut session =
                RenderSession::open(&l, RenderOptions::local()?, true, Arc::clone(cancelled))?;
            let request = session.shot_request(&l, &shot)?;
            let started = Instant::now();
            let frame = session.capture(request)?;
            // Preserve the deck CLI contract: known decode-budget deferrals
            // can be exported only as explicitly incomplete (exit 3). Unknown
            // final-partial/glyph failures keep the previous artifact intact.
            if !l.is_deck() || frame.labels_truncated || (frame.partial && frame.deferred == 0) {
                require_complete(&frame)?;
            }
            let complete = frame.complete() && l.skipped().is_empty();
            session.close()?;
            artifact::publish(&target, &frame.bytes, cancelled)?;
            let ms = (started.elapsed().as_secs_f64() * 1000.).round_ties_even() as u64;
            println!(
                "[floe2-web] rendered {} ({}x{}, {:.4},{:.4},{:.4},{:.4} um) in {:.2}s",
                out.display(),
                width,
                height,
                bb[0],
                bb[1],
                bb[2],
                bb[3],
                ms as f64 / 1000.
            );
            if let Some(path) = report_target {
                let name = out
                    .file_stem()
                    .and_then(|p| p.to_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("view");
                let mut doc = serde_json::json!({"source": l.source(), "dbu": l.dbu(), "cut_px": shot.detail.cut_px(), "thin": shot.thin.name(), "complete": complete,
                    "shots": [{"name": name, "out": out, "pixel": [width,height], "layers": shot.layers,
                        "depth": shot.depth, "bbox_um": bb, "ms": ms, "over_budget_pages": frame.deferred, "skipped_placements": l.skipped().len(), "complete": complete}]});
                if let Dataset::Deck(deck) = &l {
                    doc["jobdeck"] = serde_json::json!({"complete":complete,"skipped":l.skipped(),"over_budget_pages":frame.deferred,
                    "view":deck.metadata.jobdeck.mode,"levels":deck.metadata.jobdeck.levels});
                }
                let data = artifact::json_bytes(&doc, cancelled).map_err(|e| {
                    Error::new(e.kind, format!("PNG was saved; report failed: {e}"))
                })?;
                artifact::publish(&path, &data, cancelled).map_err(|e| {
                    Error::new(e.kind, format!("PNG was saved; report failed: {e}"))
                })?;
                println!("[floe2-web] report {} (1 shot)", path.display());
            }
            if !complete {
                for r in l.skipped() {
                    eprintln!(
                        "[jobdeck] skipped: CHIP {} ${} {}: {} ({})",
                        r.chip, r.idx, r.tc, r.reason, r.detail
                    );
                }
                eprintln!("[floe2-web] rendered INCOMPLETE: {} skipped placement(s), {} over-budget page(s) [exit 3]",l.skipped().len(),frame.deferred);
                return Ok(3);
            }
        }
        Command::Probe(source) => {
            let l = dataset(&source, None, cancelled)?;
            let mut session =
                RenderSession::open(&l, RenderOptions::local()?, false, Arc::clone(cancelled))?;
            println!(
                "[probe] service spawned (pid {})",
                session.pid().unwrap_or(0)
            );
            let bb = l.bbox();
            let cx = (i128::from(bb[0]) + i128::from(bb[2])).div_euclid(2) as f64;
            let cy = (i128::from(bb[1]) + i128::from(bb[3])).div_euclid(2) as f64;
            let hw = (l.grid().tile_w / 2).max(1) as f64;
            let hh = (l.grid().tile_h / 2).max(1) as f64;
            for (label, view, depth) in [
                ("fit view (live, depth 0)", bb.map(|n| n as f64), Some(0)),
                (
                    "live (1-tile region at center)",
                    [cx - hw, cy - hh, cx + hw, cy + hh],
                    None,
                ),
            ] {
                let mut request = session.base_request();
                request.view = view;
                request.width = 600;
                request.height = 600;
                request.depth = depth;
                request.frames = true;
                request.labels = !l.is_deck();
                request.thin = Thin::Auto.effective(l.is_deck());
                let started = Instant::now();
                let frame = session.capture(request)?;
                require_complete(&frame)?;
                println!(
                    "[probe] {label}: frame OK {} bytes, {} ms, final={}, partial={}",
                    frame.bytes.len(),
                    started.elapsed().as_millis(),
                    frame.final_frame,
                    frame.partial
                );
            }
            session.close()?;
            if !l.skipped().is_empty() {
                return Err(Error::new(
                    ErrorKind::Incomplete,
                    format!(
                        "probe rendered but jobdeck is incomplete: {} skipped placement(s)",
                        l.skipped().len()
                    ),
                ));
            }
            println!("[probe] OK — worker open/style/frames/shutdown validated; browser display not tested");
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parsed(args: &[&str]) -> Result<Command> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn read_commands_are_strict() {
        assert!(parsed(&[
            "render",
            "a.oas",
            "--bbox=0,0,4,4",
            "--at=1,1",
            "--size=2,2"
        ])
        .is_err());
        assert!(parsed(&["render", "a.oas", "--bbox=0,0,nan,1"]).is_err());
        assert!(parsed(&["info", "a.oas", "--out=a"]).is_err());
        assert!(parsed(&["probe", "a.oas", "b.oas"]).is_err());
        assert!(parsed(&["render", "a.oas", "--frames=yes"]).is_err());
        let Command::Render { shot, .. } =
            parsed(&["render", "한 글.oas", "--depth=-2", "--thin=auto"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(shot.depth, Some(0));
        assert_eq!(shot.thin, Thin::Auto);
    }
}
