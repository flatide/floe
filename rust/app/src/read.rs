use floe_app_core::{
    dataset::Dataset,
    jobdeck::{
        color::Mode,
        index::{is_deck, parse_levels},
    },
    render::{require_complete, RenderOptions, RenderSession},
    shots::{self, batch::Capture, Anchor, Detail, Thin},
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
  --batch FILE|-            Named captures; --out is a directory (also for one shot)
  --mosaic-at X,Y;X,Y;X,Y;X,Y  Four points clockwise TL,TR,BR,BL; needs --size
  --corners X0,Y0,X1,Y1     Four --size rectangles INSIDE this region
  --line W --line-color RGB  Separator width (default 2), color (default #ffffff)
  --keep-tiles              Keep mosaic _tl/_tr/_bl/_br PNGs after all renders succeed
Lengths: bare/um/µm/μm, nm, mm, cm, m. Fractional DBU is preserved.
Batch: NAME key=value ...; quotes supported, full-line # comments. Region fields
override the CLI region. Keys: bbox at size anchor px stretch layers depth mosaic
corners line linecolor keep_tiles. Limit 16 MiB/4096 shots; no shell expansion.
DRC/annotation metadata exports require later stages. Jobdeck labels unsupported.
Frames exceeding 16 Mpx are rejected, not silently rescaled.
Mosaic final image is four tiles (up to 64 Mpx). Outputs must not collide.
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
        capture: Box<Capture>,
        out: PathBuf,
        report: Option<PathBuf>,
        batch: Option<String>,
        levels: Option<BTreeSet<i64>>,
    },
    Probe(PathBuf),
}
pub fn parse(args: &[String]) -> Result<Command> {
    let kind = args[0].as_str();
    let mut capture = Capture::default();
    let shot = &mut capture.shot;
    let mut source = None;
    let mut out = PathBuf::from("view.png");
    let mut report = None;
    let mut batch = None;
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
            "--batch" => {
                let v = value()?;
                batch = if v.is_empty() { None } else { Some(v.into()) };
            }
            "--mosaic-at" => capture.mosaic = Some(shots::batch::points(value()?)?),
            "--corners" => capture.corners = Some(shots::lengths(value()?)?),
            "--line" => capture.line = super::number(value()?, flag)?,
            "--line-color" => capture.line_color = value()?.into(),
            "--keep-tiles" => {
                no_value()?;
                capture.keep_tiles = true;
            }
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
            if batch.is_none() {
                capture.validate()?;
            }
            Ok(Command::Render {
                source,
                capture: Box::new(capture),
                out,
                report,
                batch,
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
            capture,
            out,
            report,
            batch,
            levels,
        } => {
            return crate::capture::run(source, *capture, out, report, batch, levels, cancelled);
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
        let Command::Render { capture, .. } =
            parsed(&["render", "한 글.oas", "--depth=-2", "--thin=auto"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(capture.shot.depth, Some(0));
        assert_eq!(capture.shot.thin, Thin::Auto);
    }
}
