use crate::ready::{self, Ready, Waiter};
use floe_app_core::{
    dataset::Dataset,
    jobdeck::{
        index::{is_deck, parse_levels, ChipSelection},
        window::DeckReads,
    },
    render::{require_complete, RenderOptions, RenderSession},
    shots::{self, batch::Capture, Anchor, Detail, Shot, Thin},
    styles::LayerColors,
    Error, ErrorKind, Result,
};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{atomic::AtomicUsize, Arc};
use std::time::Instant;

const RENDER_HELP: &str = "Usage: floe2-web render SOURCE [OPTIONS]
Draws what the viewer shows at the same view (floe2 view --goto: full depth):
detail medium, thin keep, the density under the cut on, the viewer's fills
and layer properties, cell frames and labels. --archival: the export look
(cut 0, solid fills, no density, frames or labels; FLOE_RENDER_DEFAULTS=archival
makes it the default, as before 0.12.350).
  --bbox X0,Y0,X1,Y1          Region (default whole source)
  --at X,Y --size W,H         Alternative region form
  --anchor center|lb          Aspect/position anchor (default center)
  --px W|WxH                 Default 1200; height follows aspect
  --stretch                  Do not expand bbox to explicit WxH aspect
  --layers NAME,L/D,...       Layer names/aliases or pairs (default all); a
                            jobdeck's level by its name (the MTITLE; LEVELn
                            without one), a chip by its file name. --layer:
                            the same
  --layerid ID,...            Layers by the layer panel's number: N or N.D; a
                            jobdeck's N is level N (every chip of it). With
                            --layers, both
  --level N,N,...             Jobdeck: load these mask levels only
  --depth N                  0=top, 999/omit=full
  --detail exact|low|medium|high  Size cut 0/5/3/1 px (default medium, the
                            viewer's; --archival: exact)
  --thin auto|keep|cull       Auto=keep for both layouts and jobdecks
  --density on|off           The density under the cut, as the viewer's: the
                            shapes the --detail cut drops, drawn by the area
                            they cover; needs --detail low|medium|high and the
                            cache's design.ovs (on without it is refused: a
                            light cache has none, `floe-index ovs CACHE` adds
                            it to a full index); layouts (default on where the
                            file is, as the viewer starts; off for --archival;
                            FLOE_RUST_DENSITY_STACK=off makes it off)
  --occupancy on|off         The occupancy summary (design.ovo) at wide views,
                            as the viewer's View > occupancy summary: on draws
                            a layout's wide views from it as a jobdeck's are
                            drawn, and has a source without the file get it
                            first (the auto-index; a light cache's by the
                            light command); off none for a jobdeck either;
                            needs --detail low|medium|high. Unsaid: a jobdeck
                            draws from the file where it is and builds none
                            (a capture is drawn once: a wide pass of a light
                            source without it stays empty, exit 3), a layout
                            off; FLOE_RUST_OCCUPANCY=off|on where unsaid
  --frames[=on|off] --labels[=on|off]  Cell frames, design text (default on,
                            as the viewer; jobdeck labels unsupported)
  --layer-colors row|group  A layout's default layer colours: row = every row
                            of the layer panel its own colour, neighbours
                            differ (default); group = one colour per layer
                            number, its datatypes alike (before 0.12.352);
                            FLOE_LAYER_COLORS sets the default; layouts
  --legend                  Each image carries the legend of the layers it
                            draws (colour, fill, name L/D; every layer on: the
                            layer panel's rows) as flateyes metadata - the
                            pixels stay as drawn; fe-embed --dump prints it
  --archival                The export look; the options given still win
  --label-font-px N          6..96 (default 14)
  --out FILE                Default view.png; atomic PNG publication
  --report FILE             JSON single-shot report
  --batch FILE|-            Named captures; --out is a directory (also for one shot)
  --mosaic-at X,Y;X,Y;X,Y;X,Y  Four points clockwise TL,TR,BR,BL; needs --size
  --corners X0,Y0,X1,Y1     Four --size rectangles INSIDE this region; alone
                            (or fit): the default region's (the chips' extent)
  --chip NAME[,NAME...]     Jobdeck: these chips alone on (a NAME is the chip
                            view's file name, its TC path or a CHIP id; N:NAME
                            level N alone; wildcards * ? [..]); the region is
                            then their extent
  --chip-off NAME[,...]     These chips off, the others on
  --fit-chip NAME[#K][,...] The region is these chips' extent (on or not); #K
                            their K-th placement alone, deck order (info --chips)
  --chipid ID,ID,...        Jobdeck: these CHIP blocks' sources and placements
                            alone are opened, indexed and drawn (ids exact, or
                            * ? [..] patterns); the other blocks place nothing
  --chipnumber N,N-M,...    The same by block number, 1.. in deck order (ranges
                            N-M); with --chipid a block in either list is in
  --line W --line-color RGB  Separator width (default 2), color (default #ffffff)
  --keep-tiles              Keep mosaic _tl/_tr/_bl/_br PNGs after all renders succeed
  --drc DB --drc-rule NAME   Square per-error PNGs, live style + flateyes metadata
  --drc-err N|A-B|all        1-based local selection (default all)
  --drc-cap N               Limit all only (default 200); explicit ranges uncapped
  --drc-frac F              Error/frame span (default .3, clamped .02..1)
  --drc-rules FILE          SVRF isolation sidecar; explicit --layers wins
  --floe-reviewer TAG       Read existing waive status; no review writes
  --no-index                A source with no index yet is refused, not indexed
                            first (FLOE_AUTO_INDEX=off: the default)
  --temp-index[=DIR]        A source with no index is indexed in a folder of
                            this run's own under DIR ($TMPDIR unless given),
                            read from there, and the folder is removed when
                            the command ends: nothing is written beside the
                            source (a folder that cannot be written, a one-off
                            capture). The kind is --light / --full or the
                            rule's; a cache beside the source is opened as it
                            is. A jobdeck capture whose wide passes wait for
                            their occupancy summary builds the missing
                            summaries there and draws again (--occupancy off:
                            the pages instead, no summary). Not with --no-index
  --light | --full          The cache the auto-index builds for a source with
                            none: the light cache (design.inv + design.seg, the
                            geometry read from the source - an EBEAM file
                            captured on its own) or the full index; unsaid, a
                            jobdeck's sources light and a layout full
                            (FLOE_INDEX_LIGHT=on|off). A cache there is opened
                            as it is
  --wait[=SECONDS]          A source another run indexes: wait for it instead of
                            exit 75 (SECONDS bounds it; FLOE_LOCK_WAIT=on|SECONDS)
  --no-wait                 Refuse at once (exit 75) whatever FLOE_LOCK_WAIT says
  --workdir DIR             Read and write the caches (a jobdeck: every source's)
                            under DIR at the source's absolute path
                            (DIR/a/b/.x.oas.ice), not beside the source; DIR is
                            made when it is not there (FLOE_WORKDIR; every
                            command's, anywhere on the line)
A source with no index (a jobdeck: its sources of the levels loaded) is indexed
first, as `floe2-web index [--light|--full]` builds it - without the occupancy
summary unless --occupancy on asks for it -, its output on stderr; a changed
source keeps its cache (index --force rebuilds it).
A jobdeck capture with a region (--bbox, --at, --mosaic-at, --corners X0,.., or
the chips' extent of --chip / --fit-chip) opens, indexes and draws the sources
whose placements - the deck's BX..UY extents - the region reaches; the rest of
the levels loaded is left alone (FLOE_DECK_WINDOW=off: every source, as before
0.12.421). A region that reaches no placement is refused.
Lengths: bare/um/µm/μm, nm, mm, cm, m. Fractional DBU is preserved.
Batch: NAME key=value ...; quotes supported, full-line # comments. Region fields
override the CLI region. Keys: bbox at size anchor px stretch layers (layer)
layerid depth mosaic corners line linecolor keep_tiles chip chip_off fit_chip;
corners=fit. Limit 16 MiB/4096 shots; no shell expansion.
DRC uses the width from --px even for WxH; frames/labels are on (deck labels off),
the viewer's fills. --detail/--thin/--density/--occupancy/--label-font-px are honored.
No batch/mosaic/region/report with DRC. Local/global/path TSV.
Only complete DRC frames are saved; all-layer shots omit the layer legend
unless --legend.
Annotate existing PNGs with fe-embed. Jobdeck labels unsupported.
Frames exceeding 16 Mpx are rejected, not silently rescaled.
Mosaic final image is four tiles (up to 64 Mpx). Outputs must not collide.
Cancelled/failed frames never replace an existing PNG. Jobdeck known skipped
placements/over-budget pages may publish a flagged incomplete PNG (exit 3).
Other incomplete frames preserve the previous output.";
const INFO_HELP: &str = "Usage: floe2-web info SOURCE [--level N,N,...] [--json] [--chips] [--no-index] [--temp-index[=DIR]] [--light|--full] [--wait[=SECONDS]] [--workdir DIR]
--chips (jobdeck): every chip of the levels loaded - its CHIP blocks,
placements (#K) and extents, the names render --chip/--fit-chip take.
Read layout/cache summary. --json emits metadata plus source_stale.
A source with no index is indexed first (its output on stderr; --no-index or
FLOE_AUTO_INDEX=off: refused instead; --temp-index[=DIR]: in a temporary
folder of this run's own, removed at its end, as render's); one another run
indexes is refused (75) unless --wait[=SECONDS] (FLOE_LOCK_WAIT=on|SECONDS;
--no-wait). Jobdeck info includes the skip ledger and virtual layers;
--level is a jobdeck load selection. --json uses cache:null for composites.
--workdir DIR (FLOE_WORKDIR): the caches under DIR at the source's absolute
path, as render's.";
const PROBE_HELP: &str = "Usage: floe2-web probe SOURCE [--no-index] [--temp-index[=DIR]] [--light|--full] [--wait[=SECONDS]] [--workdir DIR]
Headless ready/open/style + fit/center frame checks; no output files.
This validates the worker, not browser display or ETX performance.
A source with no index is indexed first (--no-index: refused; --temp-index[=DIR]:
in a temporary folder of this run's own, removed at its end; --light / --full the
kind, as render's; no occupancy summary is built); --wait and --workdir as render's.";

pub enum Command {
    Help(&'static str),
    Info {
        source: PathBuf,
        json: bool,
        levels: Option<BTreeSet<i64>>,
        chips: bool,
        ready: Ready,
    },
    Render {
        source: PathBuf,
        capture: Box<Capture>,
        out: PathBuf,
        report: Option<PathBuf>,
        batch: Option<String>,
        levels: Option<BTreeSet<i64>>,
        drc: Option<Box<floe_app_core::drc::capture::Options>>,
        layer_colors: Option<LayerColors>,
        legend: bool,
        /// `--chipid` / `--chipnumber`: a jobdeck's CHIP blocks to read
        /// (jobdeck::window)
        chips: Option<ChipSelection>,
        ready: Ready,
    },
    Probe {
        source: PathBuf,
        ready: Ready,
    },
}
pub fn parse(args: &[String]) -> Result<Command> {
    let kind = args[0].as_str();
    let mut capture = Capture::default();
    // what the viewer draws (Shot::viewer); FLOE_RENDER_DEFAULTS=archival:
    // the export look, as before 0.12.350
    capture.shot = if std::env::var("FLOE_RENDER_DEFAULTS").as_deref() == Ok("archival") {
        Shot::default()
    } else {
        Shot::viewer()
    };
    let shot = &mut capture.shot;
    // the options given, which --archival leaves as they are
    let (mut detail_given, mut frames_given, mut labels_given, mut archival) =
        (false, false, false, false);
    let mut source = None;
    let mut out = PathBuf::from("view.png");
    let mut report = None;
    let mut batch = None;
    let mut drc = floe_app_core::drc::capture::Options::default();
    let mut drc_requested = false;
    let mut capture_only = false;
    let mut layers_explicit = false;
    // --layers / --layer (names) and --layerid (the panel's numbers)
    let mut layers = shots::LayerChoice::default();
    let mut json = false;
    let mut chips = false;
    // --chipid / --chipnumber: the CHIP blocks a deck's capture reads
    let mut chip_blocks = ChipSelection::default();
    let mut levels = None;
    // --layer-colors (a layout's default layer colours; the open's
    // FLOE_LAYER_COLORS without it)
    let mut layer_colors = None;
    // --legend (each image's layer legend as flateyes metadata)
    let mut legend = false;
    // --no-index / --wait / --no-wait over FLOE_AUTO_INDEX / FLOE_LOCK_WAIT
    let mut ready = Ready::from_env()?;
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
        if kind == "info" && flag == "--chips" {
            no_value()?;
            chips = true;
            continue;
        }
        if kind != "probe" && flag == "--level" {
            levels = Some(parse_levels(value()?)?);
            continue;
        }
        if ready.take(flag, inline)? {
            continue;
        }
        if kind != "render" {
            return Err(Error::input(format!("unsupported {kind} option: {flag}")));
        }
        if matches!(
            flag,
            "--bbox"
                | "--at"
                | "--size"
                | "--anchor"
                | "--stretch"
                | "--report"
                | "--batch"
                | "--mosaic-at"
                | "--corners"
                | "--chip"
                | "--chip-off"
                | "--fit-chip"
                | "--chipid"
                | "--chipnumber"
                | "--line"
                | "--line-color"
                | "--keep-tiles"
        ) {
            capture_only = true;
        }
        match flag {
            "--drc" | "--drc-rule" | "--drc-err" | "--drc-cap" | "--drc-frac" | "--drc-rules"
            | "--floe-reviewer" => {
                drc_requested = true;
                let v = value()?;
                match flag {
                    "--drc" => drc.database = v.into(),
                    "--drc-rule" => drc.rule = v.into(),
                    "--drc-err" => drc.errors = v.into(),
                    "--drc-cap" => drc.cap = super::number(v, flag)?,
                    "--drc-frac" => drc.fraction = super::number(v, flag)?,
                    "--drc-rules" => drc.rules = Some(v.into()),
                    _ => drc.reviewer = Some(v.trim().into()),
                }
            }
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
                detail_given = true;
                shot.detail = match value()? {
                    "exact" => Detail::Exact,
                    "low" => Detail::Low,
                    "medium" => Detail::Medium,
                    "high" => Detail::High,
                    _ => return Err(Error::input("invalid detail")),
                }
            }
            "--density" => {
                shot.density = Some(match value()? {
                    "on" => true,
                    "off" => false,
                    _ => return Err(Error::input("--density must be on or off")),
                })
            }
            "--occupancy" => {
                shot.occupancy = Some(match value()? {
                    "on" => true,
                    "off" => false,
                    _ => return Err(Error::input("--occupancy must be on or off")),
                })
            }
            "--thin" => {
                shot.thin = match value()? {
                    "auto" => Thin::Auto,
                    "keep" => Thin::Keep,
                    "cull" => Thin::Cull,
                    _ => return Err(Error::input("invalid thin policy")),
                }
            }
            "--layers" | "--layer" => {
                layers_explicit = true;
                layers.names(value()?);
            }
            "--layerid" => {
                layers_explicit = true;
                layers.ids(value()?)?;
            }
            "--label-font-px" => shot.font_px = super::number(value()?, flag)?,
            "--out" => out = value()?.into(),
            "--report" => report = Some(PathBuf::from(value()?)),
            "--batch" => {
                let v = value()?;
                batch = if v.is_empty() { None } else { Some(v.into()) };
            }
            "--mosaic-at" => capture.mosaic = Some(shots::batch::points(value()?)?),
            // alone (or `fit`): the corners of the default region - the
            // chips' extent with chips named, else the whole source
            "--corners" => {
                let given = match inline {
                    Some(v) => Some(v),
                    None => match args.get(i) {
                        Some(v) if !v.starts_with('-') => {
                            i += 1;
                            Some(v.as_str())
                        }
                        _ => None,
                    },
                };
                match given {
                    Some(v) if !shots::batch::is_fit(v) => {
                        capture.corners = Some(shots::lengths(v)?);
                        capture.corners_fit = false;
                    }
                    _ => {
                        capture.corners = None;
                        capture.corners_fit = true;
                    }
                }
            }
            "--chip" => capture.chip = Some(value()?.to_string()).filter(|v| !v.is_empty()),
            "--chip-off" => capture.chip_off = Some(value()?.to_string()).filter(|v| !v.is_empty()),
            "--fit-chip" => capture.fit_chip = Some(value()?.to_string()).filter(|v| !v.is_empty()),
            // the CHIP blocks to read (2026-10-29), as `index DECK` takes them
            "--chipid" => chip_blocks.add_ids(value()?)?,
            "--chipnumber" => chip_blocks.add_numbers(value()?)?,
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
            // `--frames` alone is on; `--frames off` / `--frames=off` (the
            // viewer's --frames on|off) - a following on|off is the value
            "--frames" | "--labels" => {
                let given = match inline {
                    Some(v) => Some(v),
                    None => match args.get(i).map(String::as_str) {
                        Some(v @ ("on" | "off")) => {
                            i += 1;
                            Some(v)
                        }
                        _ => None,
                    },
                };
                let on = match given {
                    None | Some("on") => true,
                    Some("off") => false,
                    Some(_) => return Err(Error::input(format!("{flag} takes on or off"))),
                };
                if flag == "--frames" {
                    frames_given = true;
                    shot.frames = on;
                } else {
                    labels_given = true;
                    shot.labels = on;
                }
            }
            "--archival" => {
                no_value()?;
                archival = true;
            }
            "--legend" => {
                no_value()?;
                legend = true;
            }
            "--layer-colors" => {
                layer_colors = Some(LayerColors::parse(value()?).ok_or_else(|| {
                    Error::input("--layer-colors must be row or group")
                })?);
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
    let chip_blocks = (!chip_blocks.is_empty()).then_some(chip_blocks);
    if chip_blocks.is_some() && !is_deck(&source) {
        return Err(Error::input("--chipid / --chipnumber require a .jb source"));
    }
    if layers.said() {
        shot.layers = layers.spec();
    }
    // the export look: cut 0, solid fills, no overlays - what the options
    // given do not say otherwise
    if archival {
        shot.archival = true;
        if !detail_given {
            shot.detail = Detail::Exact;
        }
        if !frames_given {
            shot.frames = false;
        }
        if !labels_given {
            shot.labels = false;
        }
    }
    // a jobdeck draws no labels: off unless asked for (refused below)
    if is_deck(&source) && !labels_given {
        shot.labels = false;
    }
    let drc = if drc_requested {
        drc.validate()?;
        if capture_only {
            return Err(Error::input(
                "DRC captures cannot be combined with region/batch/mosaic/report options",
            ));
        }
        if layers_explicit {
            drc.layers = Some(shot.layers.clone().unwrap_or_else(|| "all".into()));
        }
        drc.legend = legend;
        Some(Box::new(drc))
    } else {
        None
    };
    if levels.is_some() && !is_deck(&source) {
        return Err(Error::input("--level requires a jobdeck"));
    }
    if is_deck(&source) && shot.labels {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "jobdeck labels are not supported",
        ));
    }
    // the density is of what a cut drops: none at exact, and a jobdeck's
    // frames draw none (renderd's deck pass; its wide views come from the
    // sources' occupancy summary)
    if shot.density == Some(true) && shot.detail == Detail::Exact {
        return Err(Error::input(
            "--density on draws the shapes the size cut drops: give --detail low, medium or high \
             (exact drops none)",
        ));
    }
    if shot.density == Some(true) && is_deck(&source) {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "--density is for layouts: a jobdeck's frames draw no density under the cut",
        ));
    }
    // the occupancy summary stands in for pages at a cut: at exact (cut 0)
    // nothing is summarized (render-core summary_selection, reason exact)
    if shot.occupancy == Some(true) && shot.detail == Detail::Exact {
        return Err(Error::input(
            "--occupancy on draws the wide view from the occupancy summary in place of the pages a \
             size cut drops: give --detail low, medium or high (exact draws every page)",
        ));
    }
    if layer_colors.is_some() && is_deck(&source) {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "--layer-colors is for layouts: a jobdeck's colours are its colour scheme's",
        ));
    }
    match kind {
        "info" => Ok(Command::Info {
            source,
            json,
            levels,
            chips,
            ready,
        }),
        "probe" => Ok(Command::Probe { source, ready }),
        _ => {
            // the capture's own rules (one region form, --fit-chip against
            // a region of its own, --layers against --chip) are checked
            // when it runs: a run its input stops, not a malformed command
            Ok(Command::Render {
                source,
                capture: Box::new(capture),
                out,
                report,
                batch,
                levels,
                drc,
                layer_colors,
                legend,
                chips: chip_blocks,
                ready,
            })
        }
    }
}
/// `source` opened (ready.rs: indexed first when it has no index, waited
/// for when `waiter` waits), its staleness said.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dataset(
    source: &std::path::Path,
    levels: Option<BTreeSet<i64>>,
    index: bool,
    light: Option<bool>,
    summaries: bool,
    temp: Option<&std::path::Path>,
    reads: Option<&DeckReads>,
    waiter: &mut Waiter,
    cancelled: &AtomicUsize,
) -> Result<Dataset> {
    // (`light`: the kind the auto-index builds - --light / --full;
    // `summaries`: a render with --occupancy on has a source's missing
    // occupancy summary added too - DIRECT_READ_PLAN §5.5 / §8.11; by
    // default a capture builds none, and info reads the caches as they are;
    // `temp`: the run's temporary index - `--temp-index`, ready.rs;
    // `reads`: what a capture reads of a deck - jobdeck::window)
    let l = ready::open(
        source, levels, index, light, summaries, temp, reads, waiter, cancelled,
    )?;
    if l.source_stale() {
        eprintln!("[{}][warn] source changed; displaying cached geometry. Rebuild with index --force, then reopen.", floe_app_core::program());
    }
    Ok(l)
}
pub fn run(command: Command, cancelled: &Arc<AtomicUsize>) -> Result<i32> {
    match command {
        Command::Help(help) => {
            println!("{}", crate::named(help));
            Ok(0)
        }
        Command::Info {
            source,
            json,
            levels,
            chips,
            ready,
        } => {
            if chips && !is_deck(&source) {
                return Err(Error::input(format!(
                    "--chips lists a jobdeck's chips; {} is a layout",
                    source.display()
                )));
            }
            // the run's temporary index (--temp-index), gone after the
            // command - the dataset first
            let temp = ready.temp_index()?;
            let mut waiter = Waiter::new(ready.wait, cancelled);
            again(&source, &mut waiter, |waiter| {
                info(
                    &source,
                    json,
                    levels.clone(),
                    chips,
                    ready.index,
                    ready.light,
                    temp.as_ref().map(|t| t.root()),
                    waiter,
                    cancelled,
                )
            })
        }
        Command::Render {
            source,
            capture,
            out,
            levels,
            drc: Some(drc),
            layer_colors,
            ready,
            ..
        } => {
            let temp = ready.temp_index()?;
            let mut waiter = Waiter::new(ready.wait, cancelled);
            again(&source, &mut waiter, |waiter| {
                let mut dataset = dataset(
                    &source,
                    levels.clone(),
                    ready.index,
                    ready.light,
                    capture.shot.occupancy == Some(true),
                    temp.as_ref().map(|t| t.root()),
                    None,
                    waiter,
                    cancelled,
                )?;
                if let Some(mode) = layer_colors {
                    dataset.set_layer_colors(mode);
                }
                floe_app_core::drc::capture::run(
                    &dataset,
                    &capture.shot,
                    &out,
                    &drc,
                    RenderOptions::local()?,
                    cancelled,
                    |line| println!("{line}"),
                )?;
                Ok(0)
            })
        }
        Command::Render {
            source,
            capture,
            out,
            report,
            batch,
            levels,
            drc: None,
            layer_colors,
            legend,
            chips,
            ready,
        } => {
            // its shots read once (`--batch -` is stdin), the captures run
            // again there
            let temp = ready.temp_index()?;
            let mut waiter = Waiter::new(ready.wait, cancelled);
            crate::capture::run(
                source,
                *capture,
                out,
                report,
                batch,
                levels,
                chips,
                layer_colors,
                legend,
                ready.index,
                ready.light,
                temp.as_ref().map(|t| t.root()),
                &mut waiter,
                cancelled,
            )
        }
        Command::Probe { source, ready } => {
            let temp = ready.temp_index()?;
            let mut waiter = Waiter::new(ready.wait, cancelled);
            again(&source, &mut waiter, |waiter| {
                probe(
                    &source,
                    ready.index,
                    ready.light,
                    temp.as_ref().map(|t| t.root()),
                    waiter,
                    cancelled,
                )
            })
        }
    }
}
/// `body` again once its source is free (--wait) when it was refused for
/// it after the open: a rebuild that began as renderd took its locks.
pub(crate) fn again(
    source: &std::path::Path,
    waiter: &mut Waiter,
    mut body: impl FnMut(&mut Waiter) -> Result<i32>,
) -> Result<i32> {
    loop {
        match body(waiter) {
            Err(e) if waiter.again(&e, source)? => continue,
            other => return other,
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn info(
    source: &std::path::Path,
    json: bool,
    levels: Option<BTreeSet<i64>>,
    chips: bool,
    index: bool,
    light: Option<bool>,
    temp: Option<&std::path::Path>,
    waiter: &mut Waiter,
    cancelled: &Arc<AtomicUsize>,
) -> Result<i32> {
    let l = dataset(
        source, levels, index, light, false, temp, None, waiter, cancelled,
    )?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&l.info())
                .map_err(|e| Error::input(e.to_string()))?
        );
    } else {
        print!("{}", l.summary(cancelled)?);
    }
    if let (true, Dataset::Deck(deck)) = (chips, &l) {
        // what `render --chip / --chip-off / --fit-chip` name: the
        // chip view's rows, their CHIP blocks and placements
        let table = floe_app_core::jobdeck::chips::ChipTable::new(
            &deck.analysis.deck,
            &deck.analysis.model.placements,
            &deck.metadata.layers,
        );
        let levels = table.levels();
        println!(
            "[jobdeck] chip rows : {} in level{} {} - the chips render --chip / --chip-off / --fit-chip NAME[#K] name",
            table.rows.len(),
            if levels.len() == 1 { "" } else { "s" },
            levels.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
        );
        for line in table.listing() {
            println!("  {line}");
        }
    }
    Ok(0)
}
fn probe(
    source: &std::path::Path,
    index: bool,
    light: Option<bool>,
    temp: Option<&std::path::Path>,
    waiter: &mut Waiter,
    cancelled: &Arc<AtomicUsize>,
) -> Result<i32> {
    // (a probe builds no summary: its frames are a check, drawn once)
    let l = dataset(
        source, None, index, light, false, temp, None, waiter, cancelled,
    )?;
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
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parsed(args: &[&str]) -> Result<Command> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn layers_by_name_or_by_the_panels_number() {
        // user 2026-10-26: --layer is --layers; --layerid N[.D] is the
        // layer panel's number (a jobdeck's level N); both in the order given
        for (args, want) in [
            (&["render", "a.jb", "--layer", "METAL1"][..], Some("METAL1")),
            (
                &["render", "a.jb", "--layers=METAL1,VIA1"][..],
                Some("METAL1,VIA1"),
            ),
            (&["render", "a.jb", "--layerid", "1,3"][..], Some("1/0,3/0")),
            (
                &["render", "a.jb", "--layerid=2.1", "--layer", "ALIGN"][..],
                Some("2/1,ALIGN"),
            ),
            (
                &["render", "a.jb", "--layer", "METAL1", "--layerid", "3"][..],
                Some("METAL1,3/0"),
            ),
            (
                &["render", "a.jb", "--layers", "all", "--layerid", "2"][..],
                Some("2/0"),
            ),
            (&["render", "a.jb", "--layer", "all"][..], None),
            (&["render", "a.jb"][..], None),
        ] {
            let Command::Render { capture, .. } = parsed(args).unwrap() else {
                panic!()
            };
            assert_eq!(capture.shot.layers.as_deref(), want, "{args:?}");
        }
        assert!(parsed(&["render", "a.jb", "--layerid", "METAL1"]).is_err());
        assert!(parsed(&["render", "a.jb", "--layerid"]).is_err());
        // a DRC capture's explicit layers, by id too
        let Command::Render { drc: Some(drc), .. } = parsed(&[
            "render",
            "a.oas",
            "--drc=db",
            "--drc-rule=R",
            "--layerid",
            "7.1",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(drc.layers.as_deref(), Some("7/1"));
    }
    #[test]
    fn render_reads_the_chip_blocks_named() {
        // render --chipid / --chipnumber (2026-10-29): a deck's capture
        // opens those blocks' sources alone, as `index DECK` takes them; a
        // layout and a malformed list are argument errors, DRC excludes them
        let Command::Render { chips, .. } = parsed(&[
            "render",
            "a.jb",
            "--chipid",
            "ID001,ID00[23]",
            "--chipnumber=2,4-5",
            "--bbox=0,0,1,1",
        ])
        .unwrap() else {
            panic!()
        };
        let chips = chips.unwrap();
        assert_eq!(chips.ids, ["ID001", "ID00[23]"]);
        assert_eq!(chips.numbers, [(2, 2), (4, 5)]);
        let Command::Render { chips, .. } = parsed(&["render", "a.jb"]).unwrap() else {
            panic!()
        };
        assert!(chips.is_none());
        assert!(parsed(&["render", "a.oas", "--chipid", "ID001"]).is_err());
        assert!(parsed(&["render", "a.jb", "--chipnumber", "0"]).is_err());
        assert!(parsed(&["render", "a.jb", "--chipid", ","]).is_err());
        assert!(parsed(&[
            "render",
            "a.jb",
            "--chipid",
            "ID001",
            "--drc=db",
            "--drc-rule=R"
        ])
        .is_err());
    }
    #[test]
    fn read_commands_are_strict() {
        // two region forms: a run its input stops (exit 1 for floe2), checked
        // when it runs, not a malformed command line
        let Command::Render { capture, .. } = parsed(&[
            "render",
            "a.oas",
            "--bbox=0,0,4,4",
            "--at=1,1",
            "--size=2,2",
        ])
        .unwrap() else {
            panic!()
        };
        assert!(capture.validate().is_err());
        let Command::Render { capture, .. } = parsed(&[
            "render",
            "a.jb",
            "--chip",
            "x",
            "--corners",
            "--size",
            "5,5",
        ])
        .unwrap() else {
            panic!()
        };
        assert!(capture.corners_fit && capture.corners.is_none());
        assert_eq!(capture.chip.as_deref(), Some("x"));
        let Command::Render { capture, .. } =
            parsed(&["render", "a.jb", "--corners", "1,2,3,4", "--size", "1,1"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(capture.corners, Some([1., 2., 3., 4.]));
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
    #[test]
    fn drc_captures_preserve_explicit_all_and_reject_ignored_modes() {
        let args = ["render", "a.oas", "--drc=db", "--drc-rule=R"];
        for layers in [None, Some("all"), Some("1/0")] {
            let mut a = args.to_vec();
            if let Some(s) = layers {
                a.extend(["--layers", s]);
            }
            let Command::Render { drc: Some(drc), .. } = parsed(&a).unwrap() else {
                panic!()
            };
            assert_eq!(drc.layers.as_deref(), layers);
        }
        for flags in [
            ["--batch", "-"],
            ["--report", "report.json"],
            ["--bbox", "0,0,1,1"],
            ["--floe-reviewer", " "],
            ["--drc-frac", "nan"],
            ["--drc-cap", "0"],
        ] {
            let mut a = args.to_vec();
            a.extend(flags);
            assert!(parsed(&a).is_err(), "{a:?}");
        }
        assert!(parsed(&["render", "a.oas", "--drc-rule=R"]).is_err());
    }
    #[test]
    fn density_is_asked_with_a_cut_on_a_layout() {
        let density = |args: &[&str]| match parsed(args) {
            Ok(Command::Render { capture, .. }) => Ok(capture.shot.density),
            Ok(_) => panic!(),
            Err(e) => Err(e.to_string()),
        };
        assert_eq!(density(&["render", "a.oas", "--detail", "high"]), Ok(None));
        assert_eq!(density(&["render", "a.oas", "--detail=high", "--density=on"]), Ok(Some(true)));
        assert_eq!(density(&["render", "a.oas", "--density", "off"]), Ok(Some(false)));
        // a DRC capture asks it the same way
        assert_eq!(
            density(&["render", "a.oas", "--drc=db", "--drc-rule=R", "--detail=low", "--density=on"]),
            Ok(Some(true))
        );
        assert!(density(&["render", "a.oas", "--detail=high", "--density=yes"]).is_err());
        let exact = density(&["render", "a.oas", "--detail=exact", "--density=on"]).unwrap_err();
        assert!(exact.contains("--detail low, medium or high"), "{exact}");
        let deck = density(&["render", "a.jb", "--detail=high", "--density=on"]).unwrap_err();
        assert!(deck.contains("for layouts"), "{deck}");
        assert_eq!(density(&["render", "a.jb", "--density=off"]), Ok(Some(false)));
        // the viewer's detail is the default: on asks for no more
        assert_eq!(density(&["render", "a.oas", "--density=on"]), Ok(Some(true)));
        let archival = density(&["render", "a.oas", "--archival", "--density=on"]).unwrap_err();
        assert!(archival.contains("--detail low, medium or high"), "{archival}");
    }
    /// `--light` / `--full` (user 2026-10-22): the kind of cache the
    /// auto-index builds, on render, probe and info alike; exclusive
    #[test]
    fn the_auto_index_takes_the_cache_kind() {
        let light = |args: &[&str]| match parsed(args) {
            Ok(Command::Render { ready, .. }) | Ok(Command::Probe { ready, .. }) | Ok(Command::Info { ready, .. }) => {
                Ok(ready.light)
            }
            Ok(_) => panic!(),
            Err(e) => Err(e.to_string()),
        };
        assert_eq!(light(&["render", "a.oas"]), Ok(None));
        assert_eq!(light(&["render", "a.oas", "--light", "--px", "64"]), Ok(Some(true)));
        assert_eq!(light(&["render", "a.jb", "--full"]), Ok(Some(false)));
        assert_eq!(light(&["probe", "a.oas", "--light"]), Ok(Some(true)));
        assert_eq!(light(&["info", "a.oas", "--full"]), Ok(Some(false)));
        assert!(light(&["render", "a.oas", "--light", "--full"]).is_err());
        assert!(light(&["render", "a.oas", "--light=1"]).is_err());
    }
    /// `--occupancy on|off` (user 2026-10-20): the occupancy summary at wide
    /// views for a layout or a jobdeck, unsaid = renderd's rule; it stands in
    /// for pages a cut drops, so exact (and the archival look) refuse `on`.
    #[test]
    fn occupancy_is_asked_with_a_cut_for_layouts_and_jobdecks() {
        let occupancy = |args: &[&str]| match parsed(args) {
            Ok(Command::Render { capture, .. }) => Ok(capture.shot.occupancy),
            Ok(_) => panic!(),
            Err(e) => Err(e.to_string()),
        };
        assert_eq!(occupancy(&["render", "a.oas", "--detail", "high"]), Ok(None));
        assert_eq!(occupancy(&["render", "a.oas", "--detail=high", "--occupancy=on"]), Ok(Some(true)));
        assert_eq!(occupancy(&["render", "a.oas", "--occupancy", "off"]), Ok(Some(false)));
        // the viewer's detail is the default: on asks for no more
        assert_eq!(occupancy(&["render", "a.oas", "--occupancy=on"]), Ok(Some(true)));
        // a jobdeck's passes: off, or on against the kill switch
        assert_eq!(occupancy(&["render", "a.jb", "--occupancy=off"]), Ok(Some(false)));
        assert_eq!(occupancy(&["render", "a.jb", "--detail=high", "--occupancy=on"]), Ok(Some(true)));
        // a DRC capture asks it the same way
        assert_eq!(
            occupancy(&["render", "a.oas", "--drc=db", "--drc-rule=R", "--detail=low", "--occupancy=on"]),
            Ok(Some(true))
        );
        assert!(occupancy(&["render", "a.oas", "--occupancy=maybe"]).is_err());
        let exact = occupancy(&["render", "a.oas", "--detail=exact", "--occupancy=on"]).unwrap_err();
        assert!(exact.contains("--detail low, medium or high"), "{exact}");
        assert_eq!(occupancy(&["render", "a.oas", "--detail=exact", "--occupancy=off"]), Ok(Some(false)));
        let archival = occupancy(&["render", "a.oas", "--archival", "--occupancy=on"]).unwrap_err();
        assert!(archival.contains("--detail low, medium or high"), "{archival}");
    }
    #[test]
    fn captures_start_as_the_viewer_and_archival_is_the_export_look() {
        let shot = |args: &[&str]| match parsed(args) {
            Ok(Command::Render { capture, .. }) => capture.shot,
            _ => panic!("{args:?}"),
        };
        let s = shot(&["render", "a.oas"]);
        assert_eq!(
            (s.detail, s.frames, s.labels, s.archival, s.density, s.thin, s.depth),
            (Detail::Medium, true, true, false, None, Thin::Auto, None)
        );
        // a jobdeck has no labels to draw
        let s = shot(&["render", "a.jb"]);
        assert!(s.frames && !s.labels);
        // the export look, where the options given do not say otherwise
        let s = shot(&["render", "a.oas", "--archival"]);
        assert_eq!(
            (s.detail, s.frames, s.labels, s.archival),
            (Detail::Exact, false, false, true)
        );
        let s = shot(&["render", "--archival", "a.oas", "--detail", "high", "--labels"]);
        assert_eq!(
            (s.detail, s.frames, s.labels, s.archival),
            (Detail::High, false, true, true)
        );
        // the viewer's --frames/--labels on|off; a bare flag is on
        let s = shot(&["render", "a.oas", "--frames", "off", "--labels=off"]);
        assert!(!s.frames && !s.labels);
        let s = shot(&["render", "a.oas", "--frames", "--labels", "on"]);
        assert!(s.frames && s.labels);
        // a source named after the flag's word is still the source
        let Command::Render { source, capture, .. } =
            parsed(&["render", "--frames", "--", "off"]).unwrap()
        else {
            panic!()
        };
        assert!(capture.shot.frames && source == PathBuf::from("off"));
        assert!(parsed(&["render", "a.oas", "--frames=yes"]).is_err());
        assert!(parsed(&["render", "a.oas", "--archival=on"]).is_err());
    }
    #[test]
    fn layer_colors_are_asked_for_a_layout() {
        let colors = |args: &[&str]| match parsed(args) {
            Ok(Command::Render { layer_colors, .. }) => Ok(layer_colors),
            Ok(_) => panic!(),
            Err(e) => Err(e.to_string()),
        };
        assert_eq!(colors(&["render", "a.oas"]), Ok(None));
        assert_eq!(
            colors(&["render", "a.oas", "--layer-colors", "group"]),
            Ok(Some(LayerColors::Group))
        );
        assert_eq!(
            colors(&["render", "a.oas", "--layer-colors=row"]),
            Ok(Some(LayerColors::Row))
        );
        // a DRC capture asks it the same way
        assert_eq!(
            colors(&["render", "a.oas", "--drc=db", "--drc-rule=R", "--layer-colors=group"]),
            Ok(Some(LayerColors::Group))
        );
        let bad = colors(&["render", "a.oas", "--layer-colors=groups"]).unwrap_err();
        assert!(bad.contains("row or group"), "{bad}");
        let deck = colors(&["render", "a.jb", "--layer-colors=group"]).unwrap_err();
        assert!(deck.contains("for layouts"), "{deck}");
    }
    #[test]
    fn a_legend_is_asked_with_a_flag() {
        let legend = |args: &[&str]| match parsed(args) {
            Ok(Command::Render { legend, drc, .. }) => Ok((legend, drc.map(|d| d.legend))),
            Ok(_) => panic!(),
            Err(e) => Err(e.to_string()),
        };
        assert_eq!(legend(&["render", "a.oas"]), Ok((false, None)));
        assert_eq!(legend(&["render", "a.oas", "--legend"]), Ok((true, None)));
        // a jobdeck's levels and a batch take it too
        assert_eq!(legend(&["render", "a.jb", "--legend"]), Ok((true, None)));
        assert_eq!(
            legend(&["render", "a.oas", "--legend", "--batch", "-", "--out", "d"]),
            Ok((true, None))
        );
        // a DRC capture: every layer's legend too
        assert_eq!(
            legend(&["render", "a.oas", "--drc=db", "--drc-rule=R", "--legend"]),
            Ok((true, Some(true)))
        );
        assert_eq!(
            legend(&["render", "a.oas", "--drc=db", "--drc-rule=R"]),
            Ok((false, Some(false)))
        );
        assert!(legend(&["render", "a.oas", "--legend=on"]).is_err());
        assert!(parsed(&["info", "a.oas", "--legend"]).is_err());
    }
}
