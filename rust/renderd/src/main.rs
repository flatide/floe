use floe_render_core::{
    pick_scene, pick_scene_cancellable, render_geometry_occupancy_cancellable,
    render_geometry_styled_cancellable, HierPlan, LayerProbeReport, LayerRasterSession, ProbeMode,
    RenderLabel, SummarySelection,
    render_geometry_styled_cancellable_reuse,
    render_geometry_styled_unbinned_cancellable, FrameReuse,
    snap_scene, snap_scene_cancellable, validate_font_px, Cache, CacheLayer, ClipGeometry, Deck,
    DeckRenderRequest, DeckSpec, DeckXf, DecodedPageCache, FrameScene, HierError, HierHandle,
    GeometryRasterRequest, LayerFill, LayerStyle, PlanRequest, RasterViewBox, RenderCancellation,
    SceneQueryLayer, SceneQueryRequest, SceneSnapKind, StyledGeometryRasterRequest, ViewBox,
    DEFAULT_LABEL_FONT_PX, DEFAULT_TILE_SIZE, FULL_DEPTH, MAX_TILE_SIZE,
};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Instant;

const DEFAULT_BUDGET_MB: u64 = 1024;
const DEFAULT_JOBS: u16 = 1;
const DEFAULT_ROUND_PAGES: usize = 128;
const MAX_JOBS: u16 = 256;
/// §F2R-18: retained label-free geometry frames (one per render
/// state x scale) - they serve exact revisits, pans, and margins.
const RETAINED_FRAMES: usize = 3;
/// §F2R-20: retained geometry frames are bounded in BYTES as well as
/// count - a 4K margin frame is ~130 MiB, three of them ~400 MiB on a
/// shared host outside the decoded page budget. FLOE_RUST_RETAINED_MB
/// overrides (0 retains nothing = pan reuse off).
const RETAINED_BUDGET_MB_DEFAULT: u64 = 256;
/// A refinement round whose raster ran past this stops the stream:
/// the remaining page batches merge into one final round (§3.15 —
/// five ~2.2s intermediate rasters were the draw time of a large
/// cold view). Cheap rounds keep streaming below it.
const REFINEMENT_RASTER_BUDGET_US: u64 = 500_000;
const SNAP_SHAPE_CAP: usize = 1_048_576;
const PICK_CANDIDATE_CAP: usize = 64;
const QUERY_MEMBER_CAP: usize = 4_194_304;

struct PublishedScene {
    scene: Arc<FrameScene>,
    layers: Arc<[CacheLayer]>,
    cell_names: Arc<BTreeMap<u32, String>>,
    /// §F2R-21 review: the render state and view this scene was
    /// planned for - a label-only render (geometry reused in full)
    /// may leave it published only when it still serves the request.
    key: RetainedKey,
    view: [f64; 4],
}

type SharedPublishedScene = Arc<RwLock<Option<Arc<PublishedScene>>>>;

/// The hierarchy the cell tree's queries read (docs/SPEC-VIEWER.ko.md
/// §8c): one source for a cache, the deck's sources in spec order for a
/// jobdeck, each with its geometric placements (deck dbu). Published by
/// the render worker at open, read by the hier query thread.
struct HierSource {
    /// the source's cache folder (a jobdeck's source identity)
    path: String,
    handle: Arc<HierHandle>,
    placements: Vec<DeckXf>,
}

struct PublishedHier {
    sources: Vec<HierSource>,
}

type SharedHier = Arc<RwLock<Option<Arc<PublishedHier>>>>;

/// Children rows one `cells` answer carries at most (the count of all
/// children travels beside them).
const CELLS_CHILD_CAP: usize = 20_000;
/// Name-search matches one `cell_find` answer carries at most.
const CELL_FIND_CAP: usize = 5_000;
/// Instance boxes one `cell_insts` answer carries at most, and the walk
/// budget (placement records, members and BVH nodes looked at) behind
/// them: a whole-chip view of a cell with millions of instances answers
/// partially (more=1) instead of running for seconds.
const CELL_INSTS_CAP: usize = 4_096;
const CELL_INSTS_BUDGET: u64 = 2_000_000;

// which target this binary was built for, mirrored from floe-index:
// multiple builds circulate on the closed-network hosts and "which
// one is this" keeps coming up
const BUILD_FLAVOR: &str = if cfg!(target_env = "musl") {
    "musl-static"
} else if cfg!(target_os = "linux") {
    "gnu"
} else {
    "native"
};

fn version() -> String {
    let git = env!("FLOE_GIT");
    format!(
        "floe-renderd {}{} ({})",
        env!("CARGO_PKG_VERSION"),
        if git == "unknown" {
            String::new()
        } else {
            format!(" {git}")
        },
        BUILD_FLAVOR
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && (args[1] == "--version" || args[1] == "-V") {
        println!("{}", version());
        return ExitCode::SUCCESS;
    }
    // every run states which build it is (stderr - stdout carries the
    // wire protocol)
    eprintln!("[{}]", version());
    if let Err(error) = serve() {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn serve() -> Result<(), String> {
    let (response_tx, response_rx) = mpsc::channel::<String>();
    let writer = thread::spawn(move || response_writer(response_rx));
    respond(
        &response_tx,
        format!(
            "ready version={} git={} flavor={}",
            env!("CARGO_PKG_VERSION"),
            env!("FLOE_GIT"),
            BUILD_FLAVOR
        ),
    );

    let cancellation = RenderCancellation::new();
    let worker_cancellation = cancellation.clone();
    let worker_responses = response_tx.clone();
    let published_scene = Arc::new(RwLock::new(None));
    let worker_scene = Arc::clone(&published_scene);
    let published_hier: SharedHier = Arc::new(RwLock::new(None));
    let worker_hier = Arc::clone(&published_hier);
    let (command_tx, command_rx) = mpsc::channel::<WorkerCommand>();
    let worker = thread::spawn(move || {
        render_worker(
            command_rx,
            worker_responses,
            worker_cancellation,
            worker_scene,
            worker_hier,
        )
    });

    // the cell tree's queries (cells, cell_find, cell_bbox, cell_insts,
    // cell_sources) run on their own thread: the first one may build or
    // open the hierarchy summary, and an instance walk over a wide view
    // runs to its budget - neither may delay a pick or a render
    let (hier_tx, hier_rx) = mpsc::channel::<HierCommand>();
    let hier = {
        let responses = response_tx.clone();
        let hier = Arc::clone(&published_hier);
        thread::spawn(move || hier_worker(hier_rx, responses, hier))
    };

    let query_inline = std::env::var("FLOE_RUST_QUERY_INLINE").as_deref() == Ok("1");
    let snap_frontier = RenderCancellation::new();
    let pick_frontier = RenderCancellation::new();
    let (query_tx, query_rx) = mpsc::channel::<QueryCommand>();
    let query = {
        let responses = response_tx.clone();
        let scene = Arc::clone(&published_scene);
        let (snap_frontier, pick_frontier) = (snap_frontier.clone(), pick_frontier.clone());
        thread::spawn(move || query_worker(query_rx, responses, scene, snap_frontier, pick_frontier))
    };

    let stdin = io::stdin();
    let mut latest_generation = None;
    let mut main_error = None;
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                main_error = Some(error.to_string());
                break;
            }
        };
        let parsed = match parse_command(&line) {
            Ok(Some(command)) => command,
            Ok(None) => continue,
            Err(error) => {
                respond(
                    &response_tx,
                    format!("error code=command message={}", wire_escape(&error)),
                );
                continue;
            }
        };
        match parsed {
            InputCommand::Worker(mut command) => {
                if let WorkerCommand::Render(render) = &mut command {
                    render.received = Some(Instant::now());
                    if latest_generation.is_some_and(|latest| render.generation <= latest) {
                        respond(
                            &response_tx,
                            format!("dropped gen={} reason=stale", render.generation),
                        );
                        continue;
                    }
                    cancellation.cancel_before(render.generation);
                    latest_generation = Some(render.generation);
                }
                if command_tx.send(command).is_err() {
                    main_error = Some("render worker stopped".to_string());
                    break;
                }
            }
            InputCommand::Cancel(before_generation) => {
                let frontier = cancellation.cancel_before(before_generation);
                respond(&response_tx, format!("cancelled before_gen={frontier}"));
            }
            InputCommand::Snap(command) if query_inline => {
                handle_snap(&published_scene, command, &response_tx, None)
            }
            InputCommand::Pick(command) if query_inline => {
                handle_pick(&published_scene, command, &response_tx, None)
            }
            InputCommand::Snap(command) => {
                snap_frontier.cancel_before(query_generation(command.sequence));
                if query_tx.send(QueryCommand::Snap(command)).is_err() {
                    main_error = Some("query worker stopped".to_string());
                    break;
                }
            }
            InputCommand::Pick(command) => {
                pick_frontier.cancel_before(query_generation(command.sequence));
                if query_tx.send(QueryCommand::Pick(command)).is_err() {
                    main_error = Some("query worker stopped".to_string());
                    break;
                }
            }
            InputCommand::Hier(command) => {
                if hier_tx.send(command).is_err() {
                    main_error = Some("hier worker stopped".to_string());
                    break;
                }
            }
            InputCommand::Quit => break,
        }
    }
    snap_frontier.cancel_before(u64::MAX);
    pick_frontier.cancel_before(u64::MAX);
    let _ = query_tx.send(QueryCommand::Shutdown);
    drop(query_tx);
    if query.join().is_err() {
        main_error.get_or_insert_with(|| "query worker panicked".to_string());
    }
    let _ = hier_tx.send(HierCommand::Shutdown);
    drop(hier_tx);
    if hier.join().is_err() {
        main_error.get_or_insert_with(|| "hier worker panicked".to_string());
    }

    // EOF and stdin read failures are process shutdown requests just like
    // `quit`: do not let an in-flight render run to completion after its
    // client has disappeared.
    cancellation.cancel_before(u64::MAX);
    let _ = command_tx.send(WorkerCommand::Shutdown);
    drop(command_tx);
    if worker.join().is_err() {
        main_error.get_or_insert_with(|| "render worker panicked".to_string());
    }
    respond(&response_tx, "bye".to_string());
    drop(response_tx);
    match writer.join() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            main_error.get_or_insert(error);
        }
        Err(_) => {
            main_error.get_or_insert_with(|| "response writer panicked".to_string());
        }
    };
    match main_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn response_writer(responses: Receiver<String>) -> Result<(), String> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    for response in responses {
        writeln!(stdout, "{response}").map_err(|error| error.to_string())?;
        stdout.flush().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn respond(responses: &Sender<String>, response: String) {
    let _ = responses.send(response);
}

enum InputCommand {
    Worker(WorkerCommand),
    Cancel(u64),
    Snap(SnapCommand),
    Pick(PickCommand),
    Hier(HierCommand),
    Quit,
}

/// The cell tree's queries (docs/SPEC-VIEWER.ko.md §8c), answered on
/// the hier thread from the published hierarchy.
#[derive(Debug, PartialEq)]
enum HierCommand {
    /// the sources of the open cache or deck
    Sources { sequence: i64 },
    /// a cell's children (cell None = the source's top)
    Cells { sequence: i64, source: usize, cell: Option<u32> },
    /// cells whose name matches (source None = every source)
    Find { sequence: i64, source: Option<usize>, pattern: String, limit: usize },
    /// the extent of a cell's instances (view coordinates)
    Bbox { sequence: i64, source: usize, cell: u32, root: Option<u32> },
    /// a cell's instances inside a view (view coordinates)
    Insts { sequence: i64, source: usize, cell: u32, view: [f64; 4], cap: usize, root: Option<u32> },
    Shutdown,
}

impl HierCommand {
    fn sequence(&self) -> i64 {
        match self {
            HierCommand::Sources { sequence }
            | HierCommand::Cells { sequence, .. }
            | HierCommand::Find { sequence, .. }
            | HierCommand::Bbox { sequence, .. }
            | HierCommand::Insts { sequence, .. } => *sequence,
            HierCommand::Shutdown => -1,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            HierCommand::Sources { .. } => "cell_sources",
            HierCommand::Cells { .. } => "cells",
            HierCommand::Find { .. } => "cell_find",
            HierCommand::Bbox { .. } => "cell_bbox",
            HierCommand::Insts { .. } => "cell_insts",
            HierCommand::Shutdown => "shutdown",
        }
    }
}

/// §F2R-20b: snap/pick run on their own thread so a worst-case dense
/// query (up to QUERY_MEMBER_CAP members) never blocks the stdin
/// dispatcher - render generations keep arriving and the render
/// cancellation frontier keeps moving. A newer query of the same kind
/// supersedes an older one in flight through that kind's own
/// sequence frontier. FLOE_RUST_QUERY_INLINE=1 restores the inline
/// dispatch (field kill switch).
enum QueryCommand {
    Snap(SnapCommand),
    Pick(PickCommand),
    Shutdown,
}

fn query_generation(sequence: i64) -> u64 {
    sequence.max(0) as u64
}

fn query_worker(
    commands: Receiver<QueryCommand>,
    responses: Sender<String>,
    scene: SharedPublishedScene,
    snap_frontier: RenderCancellation,
    pick_frontier: RenderCancellation,
) {
    for command in commands {
        match command {
            QueryCommand::Snap(command) => {
                let sequence = query_generation(command.sequence);
                handle_snap(&scene, command, &responses, Some((sequence, &snap_frontier)));
            }
            QueryCommand::Pick(command) => {
                let sequence = query_generation(command.sequence);
                handle_pick(&scene, command, &responses, Some((sequence, &pick_frontier)));
            }
            QueryCommand::Shutdown => break,
        }
    }
}

enum WorkerCommand {
    Open(OpenCommand),
    Style(StyleCommand),
    Render(RenderCommand),
    Clip(ClipCommand),
    Info,
    Shutdown,
}

#[derive(Debug, PartialEq, Eq)]
struct OpenCommand {
    /// One VFS cache folder (the viewer's normal open) ...
    cache: Option<String>,
    /// ... or a jobdeck spec (docs/JOBDECK.ko.md M2): several caches
    /// composited through `floe_render_core::Deck`.
    deck: Option<String>,
    budget_mb: u64,
    jobs: u16,
}

#[derive(Debug, PartialEq, Eq)]
struct StyleCommand {
    epoch: u64,
    path: String,
}

#[derive(Clone, Debug, PartialEq)]
struct RenderCommand {
    generation: u64,
    view: [f64; 4],
    width: u32,
    height: u32,
    depth: u32,
    cut_px: f64,
    exact: bool,
    visible_layers: Option<Vec<String>>,
    frames: bool,
    labels: bool,
    label_font_px: f32,
    mono: bool,
    /// Allow exact settled PNG reuse. Page/decode caching is independent.
    frame_cache: bool,
    /// Raster workers. Legacy `jobs` continues to set both phases when
    /// `decode_jobs` is absent.
    jobs: Option<u16>,
    decode_jobs: Option<u16>,
    tile_size: u16,
    decode_pages: Option<usize>,
    round_pages: usize,
    unique_round_paths: bool,
    /// Publish the frame as raw RGBA (`FLOERAW1` header) instead of PNG.
    /// The interactive GUI path skips both the encode here and the decode
    /// on its side; PNG stays the default for export/oracle consumers.
    raw_frame: bool,
    style_epoch: Option<u64>,
    out: String,
    /// When the command line was read (set by the input loop, not the
    /// parser): the frame's `queue_us` is how long it then waited behind the
    /// commands before it - a plan cannot be cancelled, so a superseded one
    /// still runs to its end (2026-09-21: the field's status line showed
    /// 15.5 s that no phase accounted for, and the adapter's wait was a
    /// constant 0).
    received: Option<Instant>,
    /// `render_probe` only (docs/LAYER_DECODE_PROBE_PLAN.ko.md): paint this
    /// frame the probe's way instead of the normal one and answer with a
    /// `probe_frame`. The published scene, the retained frame and the
    /// refinement rounds are not touched.
    probe: Option<ProbeMode>,
    /// `render_probe` only: how many consecutive passes a raster worker paints
    /// into a tile before the workers meet and the driver looks at the masks
    /// again. 1 stops at every layer; a block is conservative (it decides with
    /// the mask of the block's first layer) but meets N times less often.
    probe_block: usize,
    /// `bg=on`: the viewer's margin prefetch (§F2R-17), drawn behind a
    /// settled viewport frame and swapped in when it lands. It must look as
    /// the viewport already does: one the scale's budget fit does not hold
    /// is dropped instead of decided anew (2026-09-27).
    background: bool,
    /// `thin=keep|cull`: the page hairline policy of this frame's
    /// plans - keep (mask / jobdeck: all-thin pages stay and raster
    /// as 1 px hairlines) or cull (plain layout: dropped whole, the
    /// performance policy). Absent = cull.
    thin_keep: bool,
    /// The view root (floe_vfs::ViewReq::root): the plan starts from this
    /// cell in its own coordinates (SPEC-VIEWER §8c). None = the top.
    root: Option<u32>,
    /// `vw=`/`vh=`: the viewer's viewport, px, when the frame is not it (a
    /// margin's 2W x 2H): the fit view the dots thin past is the viewport's
    /// (density_zoom_gain). None: the frame's own size.
    viewport: Option<(u32, u32)>,
    /// `density=on|off`: the density stack of this frame - the viewer's
    /// toggle (user 2026-10-05: "add a density on/off option to the
    /// viewer"): on draws pass 2 with the sub-cut dots (density_dots_on),
    /// off none. None (absent): FLOE_RUST_DENSITY_STACK and
    /// FLOE_RUST_DENSITY_DOTS decide, as before.
    density: Option<bool>,
}

#[derive(Debug, PartialEq, Eq)]
struct SnapCommand {
    sequence: i64,
    x: i64,
    y: i64,
    radius: i64,
    visible_layers: Option<Vec<String>>,
}

#[derive(Debug, PartialEq, Eq)]
struct PickCommand {
    sequence: i64,
    x: i64,
    y: i64,
    radius: i64,
    nth: i64,
    visible_layers: Option<Vec<String>>,
}

#[derive(Debug, PartialEq, Eq)]
struct ClipCommand {
    sequence: i64,
    bbox: [i64; 4],
    visible_layers: Option<Vec<String>>,
    jobs: Option<u16>,
    cell_name: String,
    out: String,
    /// the view root the clip is cut from (as a render's); None = the top
    root: Option<u32>,
}

struct PickWireResponse {
    count: usize,
    index: usize,
    candidate: floe_render_core::ScenePickCandidate,
    layer: u32,
    datatype: u32,
    layer_name: String,
    cell_name: String,
}

fn parse_command(line: &str) -> Result<Option<InputCommand>, String> {
    let mut tokens = line.split_whitespace();
    let Some(command) = tokens.next() else {
        return Ok(None);
    };
    if command.starts_with('#') {
        return Ok(None);
    }
    let fields = parse_fields(tokens)?;
    match command {
        "open" => {
            reject_unknown(&fields, &["cache", "deck", "budget_mb", "jobs"])?;
            let jobs = optional_parse(&fields, "jobs")?.unwrap_or(DEFAULT_JOBS);
            validate_jobs(jobs)?;
            let cache = fields.get("cache").cloned();
            let deck = fields.get("deck").cloned();
            if cache.is_some() == deck.is_some() {
                return Err("open requires exactly one of cache= or deck=".to_string());
            }
            Ok(Some(InputCommand::Worker(WorkerCommand::Open(
                OpenCommand {
                    cache,
                    deck,
                    budget_mb: optional_parse(&fields, "budget_mb")?.unwrap_or(DEFAULT_BUDGET_MB),
                    jobs,
                },
            ))))
        }
        "style" => {
            reject_unknown(&fields, &["epoch", "path"])?;
            Ok(Some(InputCommand::Worker(WorkerCommand::Style(
                StyleCommand {
                    epoch: required_parse(&fields, "epoch")?,
                    path: required(&fields, "path")?.to_string(),
                },
            ))))
        }
        "render" | "render_probe" => {
            let probe = if command == "render_probe" {
                Some(ProbeMode::parse(required(&fields, "probe")?)?)
            } else {
                None
            };
            let probe_block: usize = if probe.is_some() {
                let block = optional_parse(&fields, "block")?.unwrap_or(1usize);
                if block == 0 {
                    return Err("probe block must be positive".to_string());
                }
                block
            } else {
                1
            };
            let mut allowed: Vec<&str> = Vec::new();
            if probe.is_some() {
                allowed.extend(["probe", "block"]);
            }
            allowed.extend([
                "gen",
                "root",
                "view",
                "w",
                "h",
                "depth",
                "cut",
                "exact",
                "layers",
                "frames",
                "labels",
                "font_px",
                "mono",
                "frame_cache",
                "jobs",
                "decode_jobs",
                "tile_px",
                "decode_pages",
                "round_pages",
                "round_paths",
                "frame_format",
                "style_epoch",
                "out",
                "thin",
                "bg",
                "vw",
                "vh",
                "density",
            ]);
            reject_unknown(&fields, &allowed)?;
            let thin_keep = match fields.get("thin").map(|s| s.as_str()) {
                None | Some("cull") => false,
                Some("keep") => true,
                Some(other) => return Err(format!("thin must be keep or cull: {other}")),
            };
            let jobs = optional_parse(&fields, "jobs")?;
            if let Some(jobs) = jobs {
                validate_jobs(jobs)?;
            }
            let decode_jobs = optional_parse(&fields, "decode_jobs")?;
            if let Some(decode_jobs) = decode_jobs {
                validate_jobs(decode_jobs)?;
            }
            let tile_size = optional_parse(&fields, "tile_px")?.unwrap_or(DEFAULT_TILE_SIZE);
            if tile_size == 0 || tile_size > MAX_TILE_SIZE {
                return Err(format!(
                    "tile_px must be in 1..={MAX_TILE_SIZE}: {tile_size}"
                ));
            }
            let view = parse_view(required(&fields, "view")?)?;
            let width = required_parse(&fields, "w")?;
            let height = required_parse(&fields, "h")?;
            if width == 0 || height == 0 {
                return Err("render width and height must be positive".to_string());
            }
            let depth = match fields.get("depth").map(String::as_str).unwrap_or("full") {
                "full" => u32::MAX,
                value => parse_value(value, "depth")?,
            };
            let cut_px: f64 = optional_parse(&fields, "cut")?.unwrap_or(0.0);
            if !cut_px.is_finite() || cut_px < 0.0 {
                return Err(format!("invalid cut: {cut_px}"));
            }
            let exact = optional_bool(&fields, "exact")?.unwrap_or(false);
            let frames = optional_bool(&fields, "frames")?.unwrap_or(true);
            let labels = optional_bool(&fields, "labels")?.unwrap_or(false);
            let label_font_px =
                optional_parse(&fields, "font_px")?.unwrap_or(DEFAULT_LABEL_FONT_PX);
            validate_font_px(label_font_px)?;
            if exact && (cut_px != 0.0 || depth != u32::MAX || frames) {
                return Err("exact render requires cut=0 depth=full frames=off".to_string());
            }
            let round_pages =
                optional_parse(&fields, "round_pages")?.unwrap_or(DEFAULT_ROUND_PAGES);
            if round_pages == 0 {
                return Err("round_pages must be positive".to_string());
            }
            let raw_frame = match fields.get("frame_format").map(String::as_str) {
                None | Some("png") => false,
                Some("raw") => true,
                Some(other) => {
                    return Err(format!("frame_format must be png or raw: {other}"));
                }
            };
            Ok(Some(InputCommand::Worker(WorkerCommand::Render(
                RenderCommand {
                    generation: required_parse(&fields, "gen")?,
                    view,
                    width,
                    height,
                    depth,
                    cut_px,
                    exact,
                    visible_layers: parse_layers(fields.get("layers"))?,
                    frames,
                    labels,
                    label_font_px,
                    mono: optional_bool(&fields, "mono")?.unwrap_or(false),
                    frame_cache: optional_bool(&fields, "frame_cache")?.unwrap_or(true),
                    jobs,
                    decode_jobs,
                    tile_size,
                    decode_pages: optional_parse(&fields, "decode_pages")?,
                    round_pages,
                    unique_round_paths: optional_bool(&fields, "round_paths")?.unwrap_or(false),
                    raw_frame,
                    style_epoch: optional_parse(&fields, "style_epoch")?,
                    out: required(&fields, "out")?.to_string(),
                    received: None,
                    probe,
                    probe_block,
                    background: optional_bool(&fields, "bg")?.unwrap_or(false),
                    thin_keep,
                    root: optional_parse(&fields, "root")?,
                    viewport: match (optional_parse::<u32>(&fields, "vw")?, optional_parse::<u32>(&fields, "vh")?) {
                        (Some(vw), Some(vh)) if vw > 0 && vh > 0 => Some((vw, vh)),
                        _ => None,
                    },
                    density: optional_bool(&fields, "density")?,
                },
            ))))
        }
        "snap" => {
            reject_unknown(&fields, &["seq", "x", "y", "r", "layers"])?;
            Ok(Some(InputCommand::Snap(SnapCommand {
                sequence: optional_parse(&fields, "seq")?.unwrap_or(-1),
                x: required_parse(&fields, "x")?,
                y: required_parse(&fields, "y")?,
                radius: required_parse::<i64>(&fields, "r")?.max(1),
                visible_layers: parse_layers(fields.get("layers"))?,
            })))
        }
        "pick" => {
            reject_unknown(&fields, &["seq", "x", "y", "r", "nth", "layers"])?;
            Ok(Some(InputCommand::Pick(PickCommand {
                sequence: optional_parse(&fields, "seq")?.unwrap_or(-1),
                x: required_parse(&fields, "x")?,
                y: required_parse(&fields, "y")?,
                radius: required_parse::<i64>(&fields, "r")?.max(1),
                nth: optional_parse(&fields, "nth")?.unwrap_or(0),
                visible_layers: parse_layers(fields.get("layers"))?,
            })))
        }
        "clip" => {
            reject_unknown(
                &fields,
                &["seq", "box", "layers", "jobs", "cell_hex", "out", "root"],
            )?;
            let jobs = optional_parse(&fields, "jobs")?;
            if let Some(jobs) = jobs {
                validate_jobs(jobs)?;
            }
            Ok(Some(InputCommand::Worker(WorkerCommand::Clip(
                ClipCommand {
                    sequence: optional_parse(&fields, "seq")?.unwrap_or(-1),
                    bbox: parse_i64_box(required(&fields, "box")?)?,
                    visible_layers: parse_layers(fields.get("layers"))?,
                    jobs,
                    cell_name: fields
                        .get("cell_hex")
                        .map(|value| wire_unhex(value, "cell_hex"))
                        .transpose()?
                        .unwrap_or_else(|| "FLOE_CLIP".to_string()),
                    out: required(&fields, "out")?.to_string(),
                    root: optional_parse(&fields, "root")?,
                },
            ))))
        }
        "cell_sources" => {
            reject_unknown(&fields, &["seq"])?;
            Ok(Some(InputCommand::Hier(HierCommand::Sources {
                sequence: optional_parse(&fields, "seq")?.unwrap_or(-1),
            })))
        }
        "cells" => {
            reject_unknown(&fields, &["seq", "src", "cell"])?;
            Ok(Some(InputCommand::Hier(HierCommand::Cells {
                sequence: optional_parse(&fields, "seq")?.unwrap_or(-1),
                source: optional_parse(&fields, "src")?.unwrap_or(0),
                cell: optional_parse(&fields, "cell")?,
            })))
        }
        "cell_find" => {
            reject_unknown(&fields, &["seq", "src", "pat_hex", "limit"])?;
            let source: i64 = optional_parse(&fields, "src")?.unwrap_or(-1);
            let limit: usize = optional_parse(&fields, "limit")?.unwrap_or(CELL_FIND_CAP);
            Ok(Some(InputCommand::Hier(HierCommand::Find {
                sequence: optional_parse(&fields, "seq")?.unwrap_or(-1),
                source: usize::try_from(source).ok(),
                pattern: fields
                    .get("pat_hex")
                    .map(|value| wire_unhex(value, "pat_hex"))
                    .transpose()?
                    .unwrap_or_default(),
                limit: limit.clamp(1, CELL_FIND_CAP),
            })))
        }
        "cell_bbox" => {
            reject_unknown(&fields, &["seq", "src", "cell", "root"])?;
            Ok(Some(InputCommand::Hier(HierCommand::Bbox {
                sequence: optional_parse(&fields, "seq")?.unwrap_or(-1),
                source: optional_parse(&fields, "src")?.unwrap_or(0),
                cell: required_parse(&fields, "cell")?,
                root: optional_parse(&fields, "root")?,
            })))
        }
        "cell_insts" => {
            reject_unknown(&fields, &["seq", "src", "cell", "view", "cap", "root"])?;
            let cap: usize = optional_parse(&fields, "cap")?.unwrap_or(CELL_INSTS_CAP);
            Ok(Some(InputCommand::Hier(HierCommand::Insts {
                sequence: optional_parse(&fields, "seq")?.unwrap_or(-1),
                source: optional_parse(&fields, "src")?.unwrap_or(0),
                cell: required_parse(&fields, "cell")?,
                view: parse_view(required(&fields, "view")?)?,
                cap: cap.clamp(1, CELL_INSTS_CAP),
                root: optional_parse(&fields, "root")?,
            })))
        }
        "cancel" => {
            reject_unknown(&fields, &["before_gen"])?;
            Ok(Some(InputCommand::Cancel(required_parse(
                &fields,
                "before_gen",
            )?)))
        }
        "info" => {
            reject_unknown(&fields, &[])?;
            Ok(Some(InputCommand::Worker(WorkerCommand::Info)))
        }
        "quit" => {
            reject_unknown(&fields, &[])?;
            Ok(Some(InputCommand::Quit))
        }
        _ => Err(format!("unknown command: {command}")),
    }
}

fn parse_fields<'a>(
    tokens: impl Iterator<Item = &'a str>,
) -> Result<BTreeMap<String, String>, String> {
    let mut fields = BTreeMap::new();
    for token in tokens {
        let (key, value) = token
            .split_once('=')
            .ok_or_else(|| format!("expected key=value field: {token}"))?;
        if key.is_empty() || value.is_empty() {
            return Err(format!("empty key or value: {token}"));
        }
        if fields.insert(key.to_string(), value.to_string()).is_some() {
            return Err(format!("duplicate field: {key}"));
        }
    }
    Ok(fields)
}

fn reject_unknown(fields: &BTreeMap<String, String>, allowed: &[&str]) -> Result<(), String> {
    for key in fields.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unknown field: {key}"));
        }
    }
    Ok(())
}

fn required<'a>(fields: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    fields
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("missing field: {name}"))
}

fn required_parse<T>(fields: &BTreeMap<String, String>, name: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    parse_value(required(fields, name)?, name)
}

fn optional_parse<T>(fields: &BTreeMap<String, String>, name: &str) -> Result<Option<T>, String>
where
    T: std::str::FromStr,
{
    fields
        .get(name)
        .map(|value| parse_value(value, name))
        .transpose()
}

fn parse_value<T>(value: &str, name: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    value
        .parse()
        .map_err(|_| format!("invalid {name}: {value}"))
}

fn optional_bool(fields: &BTreeMap<String, String>, name: &str) -> Result<Option<bool>, String> {
    fields
        .get(name)
        .map(|value| parse_bool(value, name))
        .transpose()
}

fn parse_bool(value: &str, name: &str) -> Result<bool, String> {
    match value {
        "1" | "on" | "true" => Ok(true),
        "0" | "off" | "false" => Ok(false),
        _ => Err(format!("invalid {name}: {value}")),
    }
}

fn parse_view(value: &str) -> Result<[f64; 4], String> {
    let values = value
        .split(',')
        .map(|part| parse_value(part, "view"))
        .collect::<Result<Vec<f64>, _>>()?;
    let view: [f64; 4] = values
        .try_into()
        .map_err(|_| "view requires four coordinates".to_string())?;
    RasterViewBox::new(view[0], view[1], view[2], view[3])?;
    Ok(view)
}

fn parse_i64_box(value: &str) -> Result<[i64; 4], String> {
    let values = value
        .split(',')
        .map(|part| parse_value(part, "box"))
        .collect::<Result<Vec<i64>, _>>()?;
    let bbox: [i64; 4] = values
        .try_into()
        .map_err(|_| "box requires four coordinates".to_string())?;
    ViewBox::new(bbox[0], bbox[1], bbox[2], bbox[3])?;
    Ok(bbox)
}

fn parse_layers(value: Option<&String>) -> Result<Option<Vec<String>>, String> {
    match value.map(String::as_str) {
        None | Some("all") => Ok(None),
        Some("none") => Ok(Some(Vec::new())),
        Some(value) => {
            let layers: Vec<String> = value.split(',').map(str::to_string).collect();
            if layers.iter().any(String::is_empty) {
                return Err("layers contains an empty entry".to_string());
            }
            Ok(Some(layers))
        }
    }
}

fn validate_jobs(jobs: u16) -> Result<(), String> {
    if jobs == 0 || jobs > MAX_JOBS {
        return Err(format!("jobs must be in 1..={MAX_JOBS}: {jobs}"));
    }
    Ok(())
}

/// §F2R-16: identity of a retained geometry frame. Labels are drawn
/// on top per render, so label state is deliberately absent.
#[derive(Clone, Debug, PartialEq)]
struct RetainedKey {
    // §F2R-17: frame sizes are NOT part of the key - a margin frame
    // serves viewport-sized pans and vice versa; scale equality is
    // checked per axis in prepare_pan_reuse.
    depth: u32,
    cut_px: u64,
    visible_layers: Option<Vec<String>>,
    frames: bool,
    mono: bool,
    decode_pages: Option<usize>,
    style_epoch: Option<u64>,
    /// the page hairline policy the frame was planned under (review
    /// 2026-09-11 P1-2: a keep frame was reused for a cull request
    /// and vice versa - 16 tiles reused, the thin lines stayed or
    /// stayed missing; the published query scene shares this key)
    thin_keep: bool,
    /// the view root the frame was planned from (another root is another
    /// picture in other coordinates; SPEC-VIEWER §8c)
    root: Option<u32>,
    /// the density stack and its dots the frame was drawn with (the viewer's
    /// toggle, 2026-10-05: a frame drawn with the density must not serve a
    /// pan without it, nor the other way round)
    density: (bool, bool),
    /// the occupancy summary the frame was drawn with (M2): its level
    /// and the file's identity, so a frame drawn from an older
    /// design.ovo (or without one) is never reused after a rebuild
    summary: SummaryKey,
}

/// What of the summary decision a retained frame depends on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SummaryKey {
    level: Option<u32>,
    stamp: (u64, u64),
}

impl SummaryKey {
    fn of(selection: &floe_render_core::SummarySelection) -> Self {
        SummaryKey {
            level: selection.is_active().then_some(selection.level),
            stamp: selection.stamp,
        }
    }
}

impl RetainedKey {
    fn new(command: &RenderCommand, style_epoch: Option<u64>) -> Self {
        Self::with_summary(command, style_epoch, SummaryKey::default())
    }

    fn with_summary(command: &RenderCommand, style_epoch: Option<u64>, summary: SummaryKey) -> Self {
        Self {
            depth: command.depth,
            cut_px: command.cut_px.to_bits(),
            visible_layers: command.visible_layers.clone(),
            frames: command.frames,
            mono: command.mono,
            decode_pages: command.decode_pages,
            style_epoch,
            thin_keep: command.thin_keep,
            root: command.root,
            density: (density_stack_on(command), density_dots_on(command)),
            summary,
        }
    }
}

/// The last final render's label-free frame, for §F2R-16 pan reuse.
struct RetainedFrame {
    key: RetainedKey,
    view: [f64; 4],
    frame: floe_render_core::RgbaFrame,
    /// The budget fit the frame was planned under (WorkerState::fit_memory;
    /// None: no budget fit). A request under another decision must not
    /// reuse it (review 2026-09-28: after a redecision the frame mixed the
    /// old selection's tiles with the new one's - 240 tiles, 16,178 px off
    /// a fresh render).
    fit: Option<floe_render_core::FixedFit>,
}

impl RetainedFrame {
    fn bytes(&self) -> usize {
        self.frame.pixels().len()
    }
}

fn retained_bytes(retained: &[RetainedFrame]) -> usize {
    retained.iter().map(RetainedFrame::bytes).sum()
}

/// FLOE_RUST_RETAINED_MB: the byte budget shared by every retained
/// geometry frame (§F2R-20). Unparseable values fall back to the
/// default; 0 disables retention.
fn retained_budget_bytes() -> usize {
    let mb = std::env::var("FLOE_RUST_RETAINED_MB")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(RETAINED_BUDGET_MB_DEFAULT);
    mb.saturating_mul(1024 * 1024)
        .try_into()
        .unwrap_or(usize::MAX)
}

struct WorkerState {
    cache: Option<Cache>,
    /// A jobdeck composite (M2) opened instead of a single cache. The
    /// deck owns its per-source page LRUs under the shared budget.
    deck: Option<Deck>,
    page_cache: DecodedPageCache,
    /// §F2R-18: up to RETAINED_FRAMES label-free geometry frames,
    /// newest last, one per (render state, scale) - they serve exact
    /// revisits (k=0 full reuse), pans, and margins alike, replacing
    /// the retired exact frame cache.
    retained: Vec<RetainedFrame>,
    jobs: u16,
    styles: Vec<LayerStyle>,
    style_epoch: Option<u64>,
    /// The budget fit decided per render state and scale (fit_memory_key):
    /// every later frame there that has to thin - the viewport's, its
    /// margin, a pan's - is planned under the same decision, so the picture
    /// does not change when one replaces another (SPEC-PLANNER, 2026-09-27);
    /// a frame the decision does not fit decides anew and replaces it. A
    /// frame its budget holds whole keeps every page whatever the decision
    /// (floe_vfs plan_hier_fixed, user 2026-10-01: the key has no place, the
    /// viewer's zoom steps recur everywhere, and a dense view's decision
    /// emptied a sparse one).
    fit_memory: BTreeMap<String, floe_render_core::FixedFit>,
    /// The render states and scales whose last viewport frame its budget
    /// held whole: a margin there that has to thin would show less than the
    /// viewport did when it lands - dropped, as one that must decide anew.
    fit_whole: BTreeSet<String>,
    /// The same for the density stack's pass 2, per side (the top plane's
    /// plan, the other planes'): its plans are budget-fitted to a reserve of
    /// their own and must thin alike in every frame at a scale too.
    density_fit_memory: BTreeMap<String, floe_render_core::FixedFit>,
    /// The sub-cut dots' page floor per scale and side (the rung of
    /// DOT_PAGE_FLOORS the last viewport frame's pass 2 planned at; its
    /// length = the density cut with the planner's fit): a margin must plan
    /// at it or is dropped. A viewport frame probes from the lowest rung
    /// again (user 2026-10-01: a probe that failed in a dense view kept
    /// every later view at that zoom step off the floor).
    density_floor_memory: BTreeMap<String, u8>,
    /// fit_whole for the density stack's pass 2, per scale and side.
    density_whole: BTreeSet<String>,
    /// How far the planner's page estimates fell short of the pages' decoded
    /// charge, per layer set, depth, root and the density stack on or off
    /// (budget_scale_key): the charge of a frame's pages over their estimate
    /// when it passed the generation budget (run_render: the frame is planned
    /// again under the budget less that much, not failed). Every later frame
    /// of the key plans its pages to the budget over it.
    budget_scale: BTreeMap<String, f64>,
}

impl Default for WorkerState {
    fn default() -> Self {
        Self {
            cache: None,
            deck: None,
            page_cache: DecodedPageCache::new(DEFAULT_BUDGET_MB * 1024 * 1024),
            retained: Vec::new(),
            jobs: DEFAULT_JOBS,
            styles: Vec::new(),
            style_epoch: None,
            fit_memory: BTreeMap::new(),
            fit_whole: BTreeSet::new(),
            density_fit_memory: BTreeMap::new(),
            density_floor_memory: BTreeMap::new(),
            density_whole: BTreeSet::new(),
            budget_scale: BTreeMap::new(),
        }
    }
}

/// The key a budget fit is remembered under: the layers, depth, cut, thin
/// mode and the scale (a pan keeps it, a zoom changes it).
fn fit_memory_key(command: &RenderCommand, request: &PlanRequest) -> String {
    let (head, tail) = fit_memory_scope(command);
    format!("{head}{}|{}|{}|{}|{}{tail}", request.cut_dbu, scale_token(request.px_per_dbu), command.thin_keep, request.page_hairline, command.frames)
}

/// What a fit_memory_key begins and ends with: the layers and the depth, and
/// the root - every key of a layer set between them (forget_fits).
fn fit_memory_scope(command: &RenderCommand) -> (String, String) {
    (format!("{:?}|{}|", command.visible_layers, command.depth), format!("|{:?}", command.root))
}

/// The scale as the fit memory keys it: nine significant digits. The exact
/// bits (0.12.231) split frames the viewer draws at one scale - the viewport
/// frame, its margin, a pan's - since each derives px_per_dbu from its own
/// box (field 2026-09-27: viewport `461/230` beside margin `3686/1843`, each
/// under its own decision, 139,565 px of the overlap different).
fn scale_token(px_per_dbu: f64) -> String {
    format!("{px_per_dbu:.8e}")
}

struct FramePixels {
    /// PNG bytes (headless/export jobs); raw frames publish straight
    /// from `frame` - header and pixels are two writes, no concatenated
    /// payload copy (§F2R-20).
    png: Option<Vec<u8>>,
    /// The rendered frame. Raw publish source; after publish it is the
    /// retained geometry itself when `geometry_is_frame` (no copy).
    frame: Option<floe_render_core::RgbaFrame>,
    /// The label-free copy when a label pass painted over `frame`.
    geometry: Option<floe_render_core::RgbaFrame>,
    geometry_is_frame: bool,
    frame_cache_hit: bool,
    raster_us: u64,
    raster_tile_max_us: u64,
    work_bin_items: u64,
    work_bin_overflow_items: u64,
    work_bin_defer_rep: u64,
    work_bin_defer_single: u64,
    work_bin_defer_weight_max: u64,
    tiles_reused: u32,
    png_us: u64,
    workers_used: u16,
    tiles: u32,
    partial: bool,
    labels_truncated: bool,
    rectangle_member_paints: u64,
    polygon_member_paints: u64,
    path_member_paints: u64,
    frame_member_paints: u64,
    label_tile_paints: u64,
    label_pixel_paints: u64,
    rep_members_tested: u64,
    rep_members_drawn: u64,
    representative_spans: u64,
    representative_pixels: u64,
    hier_cells_visited: u64,
    subtrees_pruned: u64,
    once_full_tiles: u32,
    once_passes_skipped: u64,
    once_items_skipped: u64,
    summary_cells: u64,
    summary_pixels: u64,
    /// placement survivor walks by outcome (RenderStats::place_walks)
    place_walks: [(u64, u64); 32],
    /// the density stack's counts (RenderStats::density_stack) when the
    /// frame stacked its density
    density_stack: Option<[u64; 5]>,
    /// pass 2's pages: planned, in hand from pass 1, decoded, over the budget
    density_pages: Option<[u64; 4]>,
    /// pass 2's time (us): the finer plans, their scenes, the bins and minis,
    /// the regions, the decode
    density_us: Option<[u64; 6]>,
    /// pass 2's bin: items, deferred edges, overflow items
    density_bin: Option<[u64; 3]>,
    /// pass 2's sub-cut dot items (FLOE_RUST_DENSITY_DOTS=on): planned, and
    /// the plans' items past the cap (dropped)
    density_dots: Option<[u64; 2]>,
    /// the records' cut pass 2 planned at (px, the highest over its sides):
    /// the dots' floor (FLOE_RUST_DENSITY_FLOOR_PX), the density cut, or
    /// what a budget fit raised it to
    density_floor: Option<f64>,
    /// pass 2's plans, summed over its sides (diagnostic, 2026-10-01: on the
    /// field chip the plan took 6.9 s whatever the dot block): the floor
    /// probes' and the fitted plans' wall time (us), the probes, the fitted
    /// plans' passes (HierStats::fit_passes), the regions planned over, and
    /// the final plans' child-BVH nodes, page-BVH nodes and page candidates,
    /// the threads the regions were planned apart on (1: one plan), and the
    /// final plans' placement reads (a dot node's layers and count) and dot
    /// items; then the floor probes past the reserve and the sides whose plan
    /// the budget fit thinned (2026-10-01); then the dot items by where they
    /// came from - child-BVH nodes, placements, arrays, point-list members one
    /// by one, point-list chunks at once and their members, array members one
    /// by one, pages - and the dot block updates the hash map took
    /// (2026-10-02); then pass 2's reserve in MB (density_frame_reserve,
    /// 2026-10-02); then the pages placed by their occupancy grids
    /// (design.ovb, HierOpts::dot_page_occ, 2026-10-02); then the pages under
    /// the floor decoded, their occupancy cells too coarse on screen
    /// (HierOpts::dot_occ_decode, 2026-10-03); ...; then the dot blocks left
    /// out as too sparse and the dots a block needed (HierOpts::dot_gate,
    /// 2026-10-04); then the brightness's gain in thousandths
    /// (density_bright_gain, 2026-10-05; 0: the dots as lit pixels); then the
    /// pages a budget left out that their occupancy records stand in for
    /// (floe_vfs HierOpts::dot_stand_in, 2026-10-05); then whether a sub-cut
    /// cell stands for its cover (1; 0: for its box - floe_render_core
    /// Cache::cell_cover), the cells whose cover the plans worked out, and
    /// the nodes that counted what their placements hold (HierOpts::
    /// dot_node_sample, 2026-10-05)
    density_plan2: Option<[u64; 39]>,
}

fn render_worker(
    commands: Receiver<WorkerCommand>,
    responses: Sender<String>,
    cancellation: RenderCancellation,
    published_scene: SharedPublishedScene,
    published_hier: SharedHier,
) {
    let mut state = WorkerState::default();
    for command in commands {
        match command {
            WorkerCommand::Open(command) => {
                handle_open(&mut state, command, &responses, &published_scene, &published_hier)
            }
            WorkerCommand::Style(command) => handle_style(&mut state, command, &responses),
            WorkerCommand::Render(command) => handle_render(
                &mut state,
                command,
                &responses,
                &cancellation,
                &published_scene,
            ),
            WorkerCommand::Clip(command) => {
                handle_clip(&mut state, command, &responses, &cancellation)
            }
            WorkerCommand::Info => handle_info(&state, &responses),
            WorkerCommand::Shutdown => break,
        }
    }
}

fn handle_clip(
    state: &mut WorkerState,
    command: ClipCommand,
    responses: &Sender<String>,
    cancellation: &RenderCancellation,
) {
    let started = Instant::now();
    if state.deck.is_some() {
        respond(
            responses,
            format!(
                "error code=clip seq={} message=deck_clip_unsupported",
                command.sequence
            ),
        );
        return;
    }
    let generation = cancellation.before_generation();
    let result = run_clip(state, &command, generation, cancellation);
    match result {
        Ok((geometry, bytes, plan_us, read_us, decode_us, clip_us, write_us)) => respond(
            responses,
            format!(
                "clip seq={} size_bytes={} ms={} records={} rects={} polys={} plan_us={} read_us={} decode_us={} clip_us={} write_us={}",
                command.sequence,
                bytes,
                elapsed_us(started).saturating_add(500) / 1000,
                geometry.records(),
                geometry.rects.len(),
                geometry.polys.len(),
                plan_us,
                read_us,
                decode_us,
                clip_us,
                write_us,
            ),
        ),
        Err(error) => respond(
            responses,
            format!(
                "error code=clip seq={} message={}",
                command.sequence,
                wire_escape(&error)
            ),
        ),
    }
}

#[allow(clippy::type_complexity)]
fn run_clip(
    state: &mut WorkerState,
    command: &ClipCommand,
    generation: u64,
    cancellation: &RenderCancellation,
) -> Result<(ClipGeometry, u64, u64, u64, u64, u64, u64), String> {
    let cache = state
        .cache
        .as_ref()
        .ok_or_else(|| "cache not open".to_string())?;
    let view = ViewBox::new(
        command.bbox[0],
        command.bbox[1],
        command.bbox[2],
        command.bbox[3],
    )?;
    let request = PlanRequest {
        view,
        cut_dbu: 0,
        visible_layers: command.visible_layers.clone(),
        depth: FULL_DEPTH,
        px_per_dbu: 0.0,
        exact: true,
        sub_cut_wash: false,
        page_reps: false,
        decode_budget: 0,
        page_hairline: true,
        summary_layers: Vec::new(),
        prune_summary: false,
        sub_cut_box: false,
        shape_cut: false,
        shape_cut_max: false,
        frames: true,
        // px_per_dbu 0 above: no wash could fire either way
        page_wash: true,
        lod_swap: true,
        regions: Vec::new(),
        visible_indices: None,
        fixed_fit: None,
        root: command.root,
        sub_cut_dots: None,
        dot_records: None,
        probe_limit: 0,
        free_pages: None,
        empty_top: true,
        dot_bright: None,
        dot_occ_first: None,
    };
    let plan_started = Instant::now();
    let planned = cache.plan(&request)?;
    check_generation(cancellation, generation)?;
    let plan_us = elapsed_us(plan_started);
    let layers = selected_scene_layers(cache, command.visible_layers.as_deref())?;
    let page_ids = planned.plan.pages.clone();
    let plan = Arc::new(planned.plan);
    let workers = command.jobs.unwrap_or(state.jobs);
    let mut geometry = ClipGeometry::default();
    let mut read_us = 0u64;
    let mut decode_us = 0u64;
    let clip_started = Instant::now();
    for page_chunk in page_ids.chunks(DEFAULT_ROUND_PAGES) {
        let (pages, stats) = state.page_cache.load_cancellable(
            cache,
            page_chunk,
            workers,
            generation,
            cancellation,
        )?;
        read_us = read_us.saturating_add(stats.page_read_us);
        decode_us = decode_us.saturating_add(stats.page_decode_us);
        let scene = FrameScene::new_shared(cache, Arc::clone(&plan), pages)?;
        geometry.append_scene_cancellable(
            &scene,
            view.as_bbox(),
            &layers,
            generation,
            cancellation,
        )?;
    }
    let clip_us = elapsed_us(clip_started)
        .saturating_sub(read_us)
        .saturating_sub(decode_us);
    check_generation(cancellation, generation)?;
    let oasis = geometry.oasis_bytes_named(cache.unit(), &command.cell_name)?;
    check_generation(cancellation, generation)?;
    let bytes = u64::try_from(oasis.len()).unwrap_or(u64::MAX);
    let write_started = Instant::now();
    publish_bytes(
        &command.out,
        command.sequence,
        generation,
        &oasis,
        cancellation,
    )?;
    let write_us = elapsed_us(write_started);
    Ok((
        geometry, bytes, plan_us, read_us, decode_us, clip_us, write_us,
    ))
}

fn selected_scene_layers(
    cache: &Cache,
    visible_layers: Option<&[String]>,
) -> Result<Vec<SceneQueryLayer>, String> {
    let cache_layers = cache.layers();
    let selected: Option<BTreeSet<u32>> = visible_layers
        .map(|specs| {
            specs
                .iter()
                .map(|spec| {
                    resolve_layer(spec, &cache_layers)
                        .map(|layer| layer.index)
                        .ok_or_else(|| format!("clip layer not found: {spec}"))
                })
                .collect()
        })
        .transpose()?;
    Ok(cache_layers
        .into_iter()
        .filter(|layer| {
            selected
                .as_ref()
                .is_none_or(|selected| selected.contains(&layer.index))
        })
        .map(|layer| SceneQueryLayer {
            index: layer.index,
            layer: layer.layer,
            datatype: layer.datatype,
        })
        .collect())
}

fn handle_open(
    state: &mut WorkerState,
    command: OpenCommand,
    responses: &Sender<String>,
    published_scene: &SharedPublishedScene,
    published_hier: &SharedHier,
) {
    if state.cache.is_some() || state.deck.is_some() {
        respond(
            responses,
            "error code=state message=cache_already_open".to_string(),
        );
        return;
    }
    let budget_bytes = match command.budget_mb.checked_mul(1024 * 1024) {
        Some(bytes) => bytes,
        None => {
            respond(
                responses,
                "error code=limit message=budget_mb_overflow".to_string(),
            );
            return;
        }
    };
    // the open's own time, reported on `opened` (open_us) so a client can
    // tell the cache open from the process start and the first frame
    let open_started = Instant::now();
    if let Some(spec_path) = command.deck.as_deref() {
        let opened = std::fs::read_to_string(spec_path)
            .map_err(|error| format!("read deck spec {spec_path}: {error}"))
            .and_then(|text| DeckSpec::parse(&text))
            .and_then(|spec| Deck::open(spec, budget_bytes));
        match opened {
            Ok(deck) => {
                let info = deck.info();
                if let Ok(mut published) = published_hier.write() {
                    *published = Some(Arc::new(PublishedHier {
                        sources: deck
                            .hier_sources()
                            .into_iter()
                            .map(|source| HierSource {
                                path: source.path,
                                handle: source.handle,
                                placements: source.placements,
                            })
                            .collect(),
                    }));
                }
                state.deck = Some(deck);
                state.cache = None;
                state.page_cache = DecodedPageCache::new(0);
                state.retained.clear();
                state.fit_memory.clear();
                state.density_fit_memory.clear();
                state.density_floor_memory.clear();
                state.fit_whole.clear();
                state.density_whole.clear();
                state.budget_scale.clear();
                state.jobs = command.jobs;
                state.styles.clear();
                state.style_epoch = None;
                if let Ok(mut published) = published_scene.write() {
                    *published = None;
                }
                let bbox = info
                    .bbox
                    .map(|b| format!("{},{},{},{}", b[0], b[1], b[2], b[3]))
                    .unwrap_or_else(|| "none".to_string());
                respond(
                    responses,
                    format!(
                        "opened unit={} top=0 layers={} cells={} pages={} ovp_bytes=0 max_depth={} budget_bytes={} jobs={} deck=1 sources={} placements={} bbox={} open_us={}",
                        info.unit,
                        info.layers,
                        info.sources,
                        info.placements,
                        info.max_depth,
                        budget_bytes,
                        command.jobs,
                        info.sources,
                        info.placements,
                        bbox,
                        elapsed_us(open_started)
                    ),
                );
            }
            Err(error) => respond(
                responses,
                format!("error code=open message={}", wire_escape(&error)),
            ),
        }
        return;
    }
    let cache_path = command.cache.as_deref().unwrap_or_default();
    match Cache::open(cache_path) {
        Ok(cache) => {
            let info = cache.info();
            if let Ok(mut published) = published_hier.write() {
                *published = Some(Arc::new(PublishedHier {
                    sources: vec![HierSource {
                        path: cache_path.to_string(),
                        handle: cache.hier(),
                        placements: vec![DeckXf::IDENTITY],
                    }],
                }));
            }
            state.cache = Some(cache);
            state.page_cache = DecodedPageCache::new(budget_bytes);
            state.retained.clear();
                state.fit_memory.clear();
                state.density_fit_memory.clear();
                state.density_floor_memory.clear();
                state.fit_whole.clear();
                state.density_whole.clear();
                state.budget_scale.clear();
            state.jobs = command.jobs;
            state.styles.clear();
            state.style_epoch = None;
            if let Ok(mut published) = published_scene.write() {
                *published = None;
            }
            respond(
                responses,
                format!(
                    "opened unit={} top={} layers={} cells={} pages={} ovp_bytes={} max_depth={} budget_bytes={} jobs={} open_us={}",
                    info.unit,
                    info.top_cell,
                    info.layers,
                    info.cells,
                    info.pages,
                    info.ovp_bytes,
                    info.max_depth,
                    budget_bytes,
                    command.jobs,
                    elapsed_us(open_started)
                ),
            );
        }
        Err(error) => respond(
            responses,
            format!("error code=open message={}", wire_escape(&error)),
        ),
    }
}

/// The hier thread: answers the cell tree's queries in order, except
/// that an instance query with a newer one already queued behind it is
/// answered `superseded` at once (the viewer reads its latest sequence
/// only; a wide view's walk runs to its budget and must not queue up
/// behind a pan).
fn hier_worker(commands: Receiver<HierCommand>, responses: Sender<String>, hier: SharedHier) {
    let mut queue: std::collections::VecDeque<HierCommand> = std::collections::VecDeque::new();
    loop {
        if queue.is_empty() {
            match commands.recv() {
                Ok(command) => queue.push_back(command),
                Err(_) => break,
            }
        }
        while let Ok(command) = commands.try_recv() {
            queue.push_back(command);
        }
        let Some(command) = queue.pop_front() else {
            continue;
        };
        if matches!(command, HierCommand::Shutdown) {
            break;
        }
        if matches!(command, HierCommand::Insts { .. })
            && queue.iter().any(|later| matches!(later, HierCommand::Insts { .. }))
        {
            respond(
                &responses,
                hier_error_line(command.kind(), command.sequence(), "superseded", QUERY_SUPERSEDED),
            );
            continue;
        }
        handle_hier(&hier, command, &responses);
    }
}

fn hier_error_line(kind: &str, sequence: i64, code: &str, message: &str) -> String {
    format!(
        "{kind} seq={sequence} found=0 code={code} err_hex={}",
        wire_hex(message)
    )
}

fn hier_error(kind: &str, sequence: i64, error: &HierError) -> String {
    match error {
        HierError::NoSummary(message) => hier_error_line(kind, sequence, "nohier", message),
        HierError::Other(message) => hier_error_line(kind, sequence, "query", message),
    }
}

fn wire_f64_box(b: &[f64; 4]) -> String {
    format!("{},{},{},{}", b[0], b[1], b[2], b[3])
}

fn handle_hier(hier: &SharedHier, command: HierCommand, responses: &Sender<String>) {
    let kind = command.kind();
    let sequence = command.sequence();
    let published = match hier.read() {
        Ok(guard) => guard.clone(),
        Err(_) => None,
    };
    let Some(published) = published else {
        respond(responses, hier_error_line(kind, sequence, "state", "cache not open"));
        return;
    };
    let source_of = |index: usize| -> Result<&HierSource, String> {
        published
            .sources
            .get(index)
            .ok_or_else(|| format!("source {} of {}", index, published.sources.len()))
    };
    let line: Result<String, String> = (|| match &command {
        HierCommand::Sources { .. } => {
            let sources: Vec<String> = published
                .sources
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}:{}:{}", i, s.placements.len(), wire_hex(&s.path)))
                .collect();
            Ok(format!(
                "cell_sources seq={} found=1 n={} sources={}",
                sequence,
                sources.len(),
                if sources.is_empty() { "-".to_string() } else { sources.join(",") }
            ))
        }
        HierCommand::Cells { source, cell, .. } => {
            let src = source_of(*source)?;
            let answer = floe_render_core::cell_children(&src.handle, *cell, CELLS_CHILD_CAP)
                .map_err(|e| hier_error(kind, sequence, &e))
                .map_err(HierWire)?;
            let children: Vec<String> = answer
                .rows
                .iter()
                .map(|r| format!("{}:{}:{}:{}", r.cell, r.members, u8::from(r.leaf), wire_hex(&r.name)))
                .collect();
            let bbox = if answer.rbbox.is_empty() {
                "-".to_string()
            } else {
                format!("{},{},{},{}", answer.rbbox.x0, answer.rbbox.y0, answer.rbbox.x1, answer.rbbox.y1)
            };
            Ok(format!(
                "cells seq={} src={} found=1 cell={} name_hex={} insts={} height={} unit={} bbox={} n={} total={} children={}",
                sequence,
                source,
                answer.cell,
                wire_hex(&answer.name),
                answer.insts,
                answer.height,
                src.handle.unit(),
                bbox,
                children.len(),
                answer.total,
                if children.is_empty() { "-".to_string() } else { children.join(",") }
            ))
        }
        HierCommand::Find { source, pattern, limit, .. } => {
            let indices: Vec<usize> = match source {
                Some(index) => {
                    source_of(*index)?;
                    vec![*index]
                }
                None => (0..published.sources.len()).collect(),
            };
            let mut total = 0usize;
            let mut rows: Vec<(usize, floe_render_core::FindRow)> = Vec::new();
            for index in indices {
                let src = &published.sources[index];
                let found = floe_render_core::cell_find(&src.handle, pattern, *limit)
                    .map_err(|e| hier_error(kind, sequence, &e))
                    .map_err(HierWire)?;
                total += found.total;
                rows.extend(found.rows.into_iter().map(|r| (index, r)));
            }
            rows.sort_by_cached_key(|(index, r)| (r.name.to_lowercase(), r.name.clone(), *index, r.cell));
            rows.truncate(*limit);
            let matches: Vec<String> = rows
                .iter()
                .map(|(index, r)| format!("{}:{}:{}:{}", index, r.cell, r.insts, wire_hex(&r.name)))
                .collect();
            Ok(format!(
                "cell_find seq={} src={} found=1 total={} n={} matches={}",
                sequence,
                source.map(|s| s as i64).unwrap_or(-1),
                total,
                matches.len(),
                if matches.is_empty() { "-".to_string() } else { matches.join(",") }
            ))
        }
        HierCommand::Bbox { source, cell, root, .. } => {
            let src = source_of(*source)?;
            let extent = floe_render_core::cell_extent(&src.handle, *root, *cell)
                .map_err(|e| hier_error(kind, sequence, &e))
                .map_err(HierWire)?;
            let mut union: Option<[f64; 4]> = None;
            if let Some(b) = extent.bbox {
                for xf in &src.placements {
                    let t = xf.apply(&b);
                    union = Some(match union {
                        None => t,
                        Some(u) => [u[0].min(t[0]), u[1].min(t[1]), u[2].max(t[2]), u[3].max(t[3])],
                    });
                }
            }
            Ok(format!(
                "cell_bbox seq={} src={} cell={} found=1 insts={} approx={} bbox={}",
                sequence,
                source,
                cell,
                extent.insts,
                u8::from(extent.approx),
                union.map(|b| wire_f64_box(&b)).unwrap_or_else(|| "-".to_string())
            ))
        }
        HierCommand::Insts { source, cell, view, cap, root, .. } => {
            let src = source_of(*source)?;
            let mut boxes: Vec<String> = Vec::new();
            let mut more = false;
            let mut visited = 0u64;
            for xf in &src.placements {
                if boxes.len() >= *cap {
                    more = true;
                    break;
                }
                let Some(local) = xf.source_view(*view) else {
                    continue;
                };
                let found = floe_render_core::cell_instances(
                    &src.handle,
                    *root,
                    *cell,
                    local,
                    *cap - boxes.len(),
                    CELL_INSTS_BUDGET,
                )
                .map_err(|e| hier_error(kind, sequence, &e))
                .map_err(HierWire)?;
                more |= found.more;
                visited = visited.saturating_add(found.visited);
                boxes.extend(found.boxes.iter().map(|b| wire_f64_box(&xf.apply(b))));
            }
            Ok(format!(
                "cell_insts seq={} src={} cell={} found=1 n={} more={} visited={} boxes={}",
                sequence,
                source,
                cell,
                boxes.len(),
                u8::from(more),
                visited,
                if boxes.is_empty() { "-".to_string() } else { boxes.join(";") }
            ))
        }
        HierCommand::Shutdown => Ok(String::new()),
    })()
    .map_err(|error: HierWire| error.0);
    match line {
        Ok(line) if line.is_empty() => {}
        Ok(line) => respond(responses, line),
        Err(line) if line.starts_with(kind) => respond(responses, line),
        Err(message) => respond(responses, hier_error_line(kind, sequence, "query", &message)),
    }
}

/// A finished error line for the wire (distinguished from a bare message).
struct HierWire(String);

impl From<String> for HierWire {
    fn from(message: String) -> Self {
        HierWire(message)
    }
}

fn handle_style(state: &mut WorkerState, command: StyleCommand, responses: &Sender<String>) {
    if let Some(deck) = state.deck.as_mut() {
        let applied = load_styles(&command.path, &deck.style_layers())
            .and_then(|styles| deck.set_styles(&styles).map(|_| styles.len()));
        match applied {
            Ok(count) => {
                state.style_epoch = Some(command.epoch);
                respond(
                    responses,
                    format!("styled epoch={} layers={}", command.epoch, count),
                );
            }
            Err(error) => respond(
                responses,
                format!(
                    "error code=style epoch={} message={}",
                    command.epoch,
                    wire_escape(&error)
                ),
            ),
        }
        return;
    }
    let Some(cache) = state.cache.as_ref() else {
        respond(
            responses,
            "error code=state message=cache_not_open".to_string(),
        );
        return;
    };
    match load_styles(&command.path, &cache.layers()) {
        Ok(styles) => {
            state.styles = styles;
            state.style_epoch = Some(command.epoch);
            state.retained.clear();
                state.fit_memory.clear();
                state.density_fit_memory.clear();
                state.density_floor_memory.clear();
                state.fit_whole.clear();
                state.density_whole.clear();
            respond(
                responses,
                format!(
                    "styled epoch={} layers={}",
                    command.epoch,
                    state.styles.len()
                ),
            );
        }
        Err(error) => respond(
            responses,
            format!(
                "error code=style epoch={} message={}",
                command.epoch,
                wire_escape(&error)
            ),
        ),
    }
}

fn handle_info(state: &WorkerState, responses: &Sender<String>) {
    if let Some(deck) = state.deck.as_ref() {
        let info = deck.info();
        respond(
            responses,
            format!(
                "info unit={} top=0 layers={} cells={} pages={} ovp_bytes=0 resident_bytes={} style_epoch={} deck=1 sources={} placements={} budget_bytes={}",
                info.unit,
                info.layers,
                info.sources,
                info.placements,
                deck.resident_bytes(),
                state
                    .style_epoch
                    .map(|epoch| epoch.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                info.sources,
                info.placements,
                deck.budget_bytes()
            ),
        );
        return;
    }
    match state.cache.as_ref() {
        Some(cache) => {
            let info = cache.info();
            respond(
                responses,
                format!(
                    "info unit={} top={} layers={} cells={} pages={} ovp_bytes={} resident_bytes={} style_epoch={}",
                    info.unit,
                    info.top_cell,
                    info.layers,
                    info.cells,
                    info.pages,
                    info.ovp_bytes,
                    state.page_cache.resident_bytes(),
                    state
                        .style_epoch
                        .map(|epoch| epoch.to_string())
                        .unwrap_or_else(|| "none".to_string())
                ),
            );
        }
        None => respond(
            responses,
            "error code=state message=cache_not_open".to_string(),
        ),
    }
}

/// A query superseded by a newer one of its kind answers with this
/// error; the GUI only reads the response of its latest sequence.
const QUERY_SUPERSEDED: &str = "superseded by a newer query";

fn superseded(error: String) -> String {
    if error.starts_with("render cancelled") {
        QUERY_SUPERSEDED.to_string()
    } else {
        error
    }
}

fn handle_snap(
    shared: &SharedPublishedScene,
    command: SnapCommand,
    responses: &Sender<String>,
    cancellation: Option<(u64, &RenderCancellation)>,
) {
    let result: Result<Option<floe_render_core::SceneSnap>, String> = (|| {
        let Some(published) = current_scene(shared)? else {
            return Ok(None);
        };
        let request = scene_query_request(
            &published,
            command.x,
            command.y,
            command.radius,
            command.visible_layers.as_deref(),
            SNAP_SHAPE_CAP,
            QUERY_MEMBER_CAP,
        )?;
        match cancellation {
            Some((sequence, frontier)) => {
                snap_scene_cancellable(&published.scene, &request, sequence, frontier)
                    .map_err(superseded)
            }
            None => snap_scene(&published.scene, &request),
        }
    })();
    match result {
        Ok(Some(snap)) => respond(
            responses,
            format!(
                "snap seq={} found=1 x={} y={} snap={}",
                command.sequence,
                snap.x,
                snap.y,
                match snap.kind {
                    SceneSnapKind::Vertex => "vertex",
                    SceneSnapKind::Edge => "edge",
                }
            ),
        ),
        Ok(None) => respond(
            responses,
            format!(
                "snap seq={} found=0 x={} y={} snap=-",
                command.sequence, command.x, command.y
            ),
        ),
        Err(error) => respond(
            responses,
            format!(
                "snap seq={} found=0 x={} y={} snap=- err_hex={}",
                command.sequence,
                command.x,
                command.y,
                wire_hex(&error)
            ),
        ),
    }
}

fn handle_pick(
    shared: &SharedPublishedScene,
    command: PickCommand,
    responses: &Sender<String>,
    cancellation: Option<(u64, &RenderCancellation)>,
) {
    let result: Result<Option<PickWireResponse>, String> = (|| {
        let Some(published) = current_scene(shared)? else {
            return Ok(None);
        };
        let request = scene_query_request(
            &published,
            command.x,
            command.y,
            command.radius,
            command.visible_layers.as_deref(),
            PICK_CANDIDATE_CAP,
            QUERY_MEMBER_CAP,
        )?;
        let pick = match cancellation {
            Some((sequence, frontier)) => pick_scene_cancellable(
                &published.scene,
                &request,
                command.nth,
                sequence,
                frontier,
            )
            .map_err(superseded)?,
            None => pick_scene(&published.scene, &request, command.nth)?,
        };
        let Some(candidate) = pick.candidate else {
            return Ok(None);
        };
        let layer = published
            .layers
            .iter()
            .find(|layer| layer.index == candidate.layer_idx)
            .ok_or_else(|| format!("query layer index {} not found", candidate.layer_idx))?;
        let layer_name = if layer.name.is_empty() {
            format!("{}/{}", layer.layer, layer.datatype)
        } else {
            layer.name.clone()
        };
        let cell_name = published
            .cell_names
            .get(&candidate.cell_id)
            .ok_or_else(|| format!("query cell index {} not found", candidate.cell_id))?;
        Ok(Some(PickWireResponse {
            count: pick.count,
            index: pick.index,
            candidate,
            layer: layer.layer,
            datatype: layer.datatype,
            layer_name,
            cell_name: cell_name.to_string(),
        }))
    })();
    match result {
        Ok(Some(pick)) => respond(
            responses,
            format!(
                "pick seq={} found=1 count={} index={} layer={} datatype={} lname_hex={} cell_hex={} area={} bbox={},{},{},{} points={}",
                command.sequence,
                pick.count,
                pick.index,
                pick.layer,
                pick.datatype,
                wire_hex(&pick.layer_name),
                wire_hex(&pick.cell_name),
                pick.candidate.area,
                pick.candidate.bbox.x0,
                pick.candidate.bbox.y0,
                pick.candidate.bbox.x1,
                pick.candidate.bbox.y1,
                wire_points(&pick.candidate.points),
            ),
        ),
        Ok(None) => respond(
            responses,
            format!("pick seq={} found=0 count=0", command.sequence),
        ),
        Err(error) => respond(
            responses,
            format!(
                "pick seq={} found=0 count=0 err_hex={}",
                command.sequence,
                wire_hex(&error)
            ),
        ),
    }
}

fn current_scene(shared: &SharedPublishedScene) -> Result<Option<Arc<PublishedScene>>, String> {
    shared
        .read()
        .map(|published| published.clone())
        .map_err(|_| "published scene lock poisoned".to_string())
}

fn scene_query_request(
    published: &PublishedScene,
    x: i64,
    y: i64,
    radius: i64,
    visible_layers: Option<&[String]>,
    shape_cap: usize,
    member_cap: usize,
) -> Result<SceneQueryRequest, String> {
    let selected: Option<BTreeSet<u32>> = visible_layers
        .map(|specs| {
            specs
                .iter()
                .map(|spec| {
                    resolve_layer(spec, &published.layers)
                        .map(|layer| layer.index)
                        .ok_or_else(|| format!("query layer not found: {spec}"))
                })
                .collect()
        })
        .transpose()?;
    let layers = published
        .layers
        .iter()
        .filter(|layer| {
            selected
                .as_ref()
                .is_none_or(|selected| selected.contains(&layer.index))
        })
        .map(|layer| SceneQueryLayer {
            index: layer.index,
            layer: layer.layer,
            datatype: layer.datatype,
        })
        .collect();
    Ok(SceneQueryRequest {
        x,
        y,
        radius,
        layers,
        shape_cap,
        member_cap,
    })
}

fn wire_hex(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn wire_unhex(value: &str, field: &str) -> Result<String, String> {
    if !value.is_ascii() {
        return Err(format!("invalid {field}: non-hex byte"));
    }
    if !value.len().is_multiple_of(2) {
        return Err(format!("invalid {field}: odd hex length"));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for offset in (0..value.len()).step_by(2) {
        bytes.push(
            u8::from_str_radix(&value[offset..offset + 2], 16)
                .map_err(|_| format!("invalid {field}: non-hex byte"))?,
        );
    }
    String::from_utf8(bytes).map_err(|_| format!("invalid {field}: not UTF-8"))
}

fn wire_points(points: &[(i64, i64)]) -> String {
    points
        .iter()
        .map(|(x, y)| format!("{x},{y}"))
        .collect::<Vec<_>>()
        .join(";")
}

fn handle_render(
    state: &mut WorkerState,
    command: RenderCommand,
    responses: &Sender<String>,
    cancellation: &RenderCancellation,
    published_scene: &SharedPublishedScene,
) {
    let generation = command.generation;
    if cancellation.is_cancelled(generation) {
        cancelled_response(responses, generation, "queued");
        return;
    }
    if command.style_epoch.is_some() && command.style_epoch != state.style_epoch {
        respond(
            responses,
            format!(
                "error gen={} code=style message=style_epoch_mismatch",
                generation
            ),
        );
        return;
    }
    let result = if state.deck.is_some() {
        run_deck_render(state, &command, responses, cancellation)
    } else {
        run_render(state, &command, responses, cancellation, published_scene)
    };
    match result {
        Ok(()) => {}
        Err(error)
            if cancellation.is_cancelled(generation) && is_render_cancelled_error(&error) =>
        {
            cancelled_response(responses, generation, "render")
        }
        Err(error) => respond(
            responses,
            format!(
                "error gen={} code=render message={}",
                generation,
                wire_escape(&error)
            ),
        ),
    }
}

/// Jobdeck composite frame (docs/JOBDECK.ko.md M2): one pass per
/// visible placement inside the view, overlaid in deck layer order by
/// `floe_render_core::Deck`. No refinement rounds, labels, hierarchy
/// frames, pan reuse or published query scene yet - the frame line
/// keeps every field of the single-cache path (zeros where a phase
/// does not exist) so the adapter parses it unchanged, plus
/// `passes=`/`passes_skipped=`.
fn run_deck_render(
    state: &mut WorkerState,
    command: &RenderCommand,
    responses: &Sender<String>,
    cancellation: &RenderCancellation,
) -> Result<(), String> {
    check_generation(cancellation, command.generation)?;
    if command.root.is_some() {
        return Err("a jobdeck has no view root (root=): the sources' tops are the deck's cells".to_string());
    }
    let deck = state
        .deck
        .as_mut()
        .ok_or_else(|| "deck not open".to_string())?;
    let visible = match command.visible_layers.as_deref() {
        None => None,
        Some(specs) => {
            let layers = deck.style_layers();
            let mut outs = BTreeSet::new();
            for spec in specs {
                let layer = resolve_layer(spec, &layers)
                    .ok_or_else(|| format!("deck layer not found: {spec}"))?;
                outs.insert(layer.index);
            }
            Some(outs)
        }
    };
    let request = DeckRenderRequest {
        view: RasterViewBox::new(
            command.view[0],
            command.view[1],
            command.view[2],
            command.view[3],
        )?,
        width: command.width,
        height: command.height,
        depth: command.depth,
        cut_px: command.cut_px,
        exact: command.exact,
        visible,
        frames: command.frames,
        mono: command.mono,
        // FLOE_RUST_DECK_SUBWINDOW=off: field kill switch back to the
        // full-frame pass per placement (identical pixels)
        subwindow: std::env::var("FLOE_RUST_DECK_SUBWINDOW").as_deref() != Ok("off"),
        workers: command.jobs.unwrap_or(state.jobs),
        decode_workers: command.decode_jobs.or(command.jobs).unwrap_or(state.jobs),
        tile_size: command.tile_size,
        decode_pages: command.decode_pages,
        // FLOE_RUST_DECK_WIDE=off: field kill switch back to the plain
        // size cut (sub-cut content silently omitted)
        // the deck wide-view washes are OFF by default (user decision
        // 2026-09-16: deck sources carry an occupancy summary, which
        // draws a wide view instead); FLOE_RUST_DECK_WIDE=on enables
        wide: std::env::var("FLOE_RUST_DECK_WIDE").as_deref() == Ok("on"),
        // FLOE_RUST_DECK_STREAM=off: stop a pass at the budget again
        // (partial frame, pages "over budget (not drawn)")
        stream: std::env::var("FLOE_RUST_DECK_STREAM").as_deref() != Ok("off"),
        thin_keep: command.thin_keep,
    };
    let report = deck.render(&request, command.generation, cancellation)?;
    check_generation(cancellation, command.generation)?;
    let png_started = Instant::now();
    let png = if command.raw_frame {
        None
    } else {
        Some(report.frame.png_bytes()?)
    };
    let png_us = elapsed_us(png_started);
    let raw_header = command
        .raw_frame
        .then(|| raw_frame_header(report.frame.width(), report.frame.height()));
    let parts: Vec<&[u8]> = match (&raw_header, &png) {
        (Some(header), _) => vec![header.as_slice(), report.frame.pixels()],
        (None, Some(png)) => vec![png.as_slice()],
        _ => return Err("frame has neither raw pixels nor PNG bytes".to_string()),
    };
    let publish_stats = publish_frame(&command.out, command.generation, &parts, cancellation)?;
    let stats = &report.stats;
    respond(
        responses,
        format!(
            "frame gen={} round=1 final=1 png={} format={} partial={} deferred={} frame_cache_hit=0 style_epoch={} plan_us={} text_plan_us=0 labels=0 labels_truncated=0 text_place_records=0 read_us={} decode_us={} decode_sum_us={} decode_max_us={} index_us={} decode_workers={} scene_us={} mask_bytes=0 raster_us={} raster_tile_max_us={} tiles_reused=0 bin_items={} bin_overflow={} bin_defer_rep={} bin_defer_single={} bin_defer_wmax={} png_us={} publish_write_us={} publish_sync_us={} publish_rename_us={} workers={} tiles={} tile_px={} pages={} plan_pages={} cache_hit={} cache_miss={} cache_evict={} resident_bytes={} wc_cells=0 inst_edges=0 frame_rects=0 rect_paints={} polygon_paints={} path_paints={} frame_paints={} label_tile_paints=0 label_pixel_paints=0 rep_tested={} rep_drawn={} hier_cells={} subtree_prunes={} retained_bytes=0 passes={} passes_skipped={} pass_bytes_max={} frame_passes={} unique_pages={} frame_raster_us={} composite_us={} scene_reuses={} raster_wall_us={} pass_workers={} batches={} batch_bytes_max={} streamed_passes={} slices={} wide_washes={} cull_pages={} cull_pbvh={} cull_cbvh={} cull_children={} cull_layer={} washed={} lod_swapped={} thin_frames={} thin_pages={} sub_cut_sparse={} sub_cut_sparse_over={} sub_cut_wash_over={} rep_kept={} rep_washed={} rep_children={} rep_page_level={} rep_level={} summary_passes={} summary_none_passes={} summary_cells={} once_tiles={} once_passes={} once_items={}",
            command.generation,
            command.out,
            if command.raw_frame { "raw" } else { "png" },
            report.partial as u8,
            report.deferred,
            state
                .style_epoch
                .map(|epoch| epoch.to_string())
                .unwrap_or_else(|| "none".to_string()),
            stats.plan_us,
            stats.page_read_us,
            stats.page_decode_us,
            stats.page_decode_sum_us,
            stats.page_decode_max_us,
            stats.page_index_us,
            stats.decode_workers_used,
            report.scene_us,
            stats.raster_us,
            stats.raster_tile_max_us,
            stats.work_bin_items,
            stats.work_bin_overflow_items,
            stats.work_bin_defer_rep,
            stats.work_bin_defer_single,
            stats.work_bin_defer_weight_max,
            png_us,
            publish_stats.write_us,
            publish_stats.sync_us,
            publish_stats.rename_us,
            stats.workers_used,
            stats.tiles,
            command.tile_size,
            report.pages,
            report.plan_pages,
            stats.decoded_cache_hit,
            stats.decoded_cache_miss,
            stats.decoded_cache_evicted,
            report.resident_bytes,
            report.rectangle_member_paints,
            report.polygon_member_paints,
            report.path_member_paints,
            report.frame_member_paints,
            stats.rep_members_tested,
            stats.rep_members_drawn,
            stats.hier_cells_visited,
            stats.subtrees_pruned,
            report.passes,
            report.passes_skipped,
            report.pass_bytes_max,
            report.frame_passes,
            report.unique_pages,
            report.frame_raster_us,
            report.composite_us,
            report.scene_reuses,
            report.raster_wall_us,
            report.pass_workers,
            report.batches,
            report.batch_bytes_max,
            report.streamed_passes,
            report.slices,
            report.wide_washes,
            report.culls.pages_size,
            report.culls.page_bvh,
            report.culls.child_bvh,
            report.culls.children_size,
            report.culls.layer,
            report.culls.washed,
            report.culls.lod_swapped,
            report.culls.thin_frames,
            report.culls.thin_pages,
            report.culls.sub_cut_sparse,
            report.culls.sub_cut_sparse_over,
            report.culls.sub_cut_wash_over,
            report.culls.rep_kept,
            report.culls.rep_washed,
            report.culls.rep_children,
            report.culls.rep_page_level,
            report.culls.rep_level,
            report.summary_passes,
            report.summary_none_passes,
            report.summary_cells,
            stats.once_full_tiles,
            stats.once_passes_skipped,
            stats.once_items_skipped,
        ),
    );
    Ok(())
}

/// Whether a plain layout's plan keeps its sub-cut pages as washes or
/// sparse pixels like a deck pass. OFF by default (user decision
/// 2026-09-16: the rules slowed mid-zoom draws on a 150 MB chip and
/// still did not show everything; presence at a wide view is the
/// occupancy summary's job); FLOE_RUST_SUB_CUT_WASH=on enables them
/// for a diagnosis.
fn sub_cut_wash_enabled() -> bool {
    std::env::var("FLOE_RUST_SUB_CUT_WASH").as_deref() == Ok("on")
}

/// Sub-cut boxes on a plain layout's `thin keep` frames
/// (floe_vfs::ViewReq::sub_cut_box). OFF by default since 0.12.182 (user
/// decision 2026-09-21): with few layers visible the box plan walks every
/// size-cut subtree to its placements and replans the frame past its cap -
/// 20 s near the fit view of a 449-layer chip with ten layers on - and what
/// lies below the cut is to be shown by a density representation instead.
/// FLOE_RUST_SUB_CUT_BOX=on turns them on for a diagnosis.
fn sub_cut_box_enabled() -> bool {
    std::env::var("FLOE_RUST_SUB_CUT_BOX").as_deref() == Ok("on")
}

/// The per-shape cut on a plain layout's `thin keep` frames, by
/// FLOE_RUST_SHAPE_CUT (CUT_DENSITY_DESIGN §10.6).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShapeCut {
    /// The default since 0.12.214 (user decision 2026-09-25), also `max`: the
    /// hairline-keeping cut (floe_vfs::ViewReq::shape_cut_max) - pages,
    /// child cells and records are cut only when their LARGER side is under
    /// the cut, so a thin shape longer than the cut stays and the width-first
    /// drawing thins it by its width.
    Larger,
    /// `min`, the kill switch - the default of 0.12.173..0.12.213
    /// (floe_vfs::ViewReq::shape_cut): every shape judged by its smaller side.
    Smaller,
    /// `off`: no per-shape cut - pages are cut by their largest shape and
    /// every record of a kept page is drawn, as before 0.12.173.
    Off,
}

fn shape_cut_mode() -> ShapeCut {
    match std::env::var("FLOE_RUST_SHAPE_CUT").as_deref() {
        Ok("min") => ShapeCut::Smaller,
        Ok("off") => ShapeCut::Off,
        _ => ShapeCut::Larger,
    }
}

/// Area-true drawing (floe_render_core::GeometryRasterRequest::area_true,
/// user decision 2026-09-22): a drawn shape lights the pixels whose centres it
/// covers with its outline on their rim, and a shape under a pixel on a side is
/// kept with the chance its area fills its pixels - the KLayout rule grew every
/// shape by about a pixel per axis, closed the gaps up to ~1.5 px and lit 0.1 px
/// wires 1 px apart as a solid block. Exact frames keep the KLayout rule;
/// FLOE_RUST_AREA_TRUE=off is the kill switch.
fn area_true_enabled() -> bool {
    std::env::var("FLOE_RUST_AREA_TRUE").as_deref() != Ok("off")
}

/// The M7-C page wash (floe_vfs::ViewReq::page_wash) on a plain layout's
/// frames. OFF by default (user decision 2026-09-22): a page whose whole image
/// fits 2 x 2 px was shipped as one bbox rect - a marker drawn by the KLayout
/// rule - so the small pages of a wide view never showed the area-true
/// drawing that is being checked; they are now decoded and drawn.
/// FLOE_RUST_PAGE_WASH=on turns the wash back on.
/// The width-first rule's extra-sparsening strength
/// (floe_render_core::GeometryRasterRequest::width_c, ADAPTIVE_CUT_DENSITY_PLAN
/// §4.2 candidate 1): FLOE_RUST_WIDTH_C=c with c >= 1, diagnostic only -
/// unset, empty or out of range means 1 (the plain rule).
fn width_c() -> f64 {
    std::env::var("FLOE_RUST_WIDTH_C")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|c| c.is_finite() && *c >= 1.0)
        .unwrap_or(1.0)
}

/// The density stack (floe_render_core::GeometryRasterRequest::density_stack,
/// CUT_DENSITY_DESIGN §10.10; user direction 2026-09-26: the top layer's
/// density, and the others' only in the empty space): FLOE_RUST_DENSITY_STACK=top,
/// diagnostic and off by default. It needs the area-true drawing (not on an
/// exact frame or under FLOE_RUST_AREA_TRUE=off) and the write-once tiles
/// (not under FLOE_RUST_WRITE_ONCE=off).
fn density_stack_enabled() -> bool {
    std::env::var("FLOE_RUST_DENSITY_STACK").as_deref() == Ok("top")
}

/// The sub-cut dots' one walk (CUT_DENSITY_DESIGN §10.12): pass 2 plans once,
/// its pages at the cells' cut (pass 1's, in hand - nothing more is decoded),
/// its records at dot_record_floor_px, a page all under the cut a dot item -
/// no floor probe, no budget fit. The default for a day (0.12.259, field: pass
/// 2 planned 6.9 s whatever the dot block, the probe and the fit's ladder
/// walking the view up to five times on the first frame at a scale), opt-in
/// since (0.12.261, field: a root's first view drew 13.8 s against 7.9 s -
/// every record under the cut of pass 1's pages drawn, the small-shape pages
/// as dots far denser than their shapes: x16 lit 0.26 against 0.058 drawn
/// cut-free, 0.080 under the fit). FLOE_RUST_DENSITY_ONE_WALK=on, diagnostic.
/// The density stack's planes without the visible layers above the topmost
/// one the plan's top cell shows shapes of within its depth
/// (Cache::layer_held) that hold no text below it either: the top plane is
/// the topmost visible layer with shapes (user 2026-10-03: "the topmost of
/// the layers on that has shapes" - the routing chip's BOUNDARY 100/0, named
/// and empty, on top of every layer on, left the top plane's density empty).
/// Such a layer draws nothing in either pass. None of them with shapes: as
/// they are.
fn density_held_top(cache: &Cache, top: (u32, u32), mut styled: StyledGeometryRasterRequest) -> StyledGeometryRasterRequest {
    let texted = cache.layers_texted(top.0);
    let has = |bits: &[u8], idx: u32| bits.get(idx as usize / 8).is_some_and(|byte| (byte >> (idx % 8)) & 1 == 1);
    // the topmost one with shapes, from the top; none: as they are
    let Some(keep) = styled
        .layers
        .iter()
        .rposition(|layer| has(&texted, layer.layer_idx) || cache.layer_held(top.0, top.1, layer.layer_idx))
    else {
        return styled;
    };
    styled.layers.truncate(keep + 1);
    styled
}

/// density_held_top default: on; FLOE_RUST_DENSITY_TOP_HELD=off (the kill
/// switch) keeps the topmost visible layer the top plane, shapes or not.
fn density_top_held() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FLOE_RUST_DENSITY_TOP_HELD").as_deref() != Ok("off"))
}

/// The density stack's top planes are the topmost DENSITY_TOP_PLANES of the
/// visible layers by the drawing order - (layer, datatype) ascending, the last
/// on top (user 2026-10-04: "787.0, 787.20, 787.55, 789.0, 789.20, 789.55 -
/// the top layer is 789.55 ... we draw from the top, filling what is empty:
/// 789.55 first, then 789.20, 789.0, 787.55, 787.20, 787.0"): each is planned
/// on its own and draws its density over the originals below it, the top
/// first (render-core GeometryRasterRequest::density_top_planes); the layers
/// under them in one walk where no original is. FLOE_RUST_DENSITY_TOP_PLANES
/// sets how many (diagnostic); FLOE_RUST_DENSITY_TOP_GROUP=off is the kill
/// switch: the topmost one alone.
const DENSITY_TOP_PLANES: usize = 8;

fn density_top_group(_cache: &Cache, mut styled: StyledGeometryRasterRequest) -> StyledGeometryRasterRequest {
    let count = if std::env::var("FLOE_RUST_DENSITY_TOP_GROUP").as_deref() == Ok("off") {
        1
    } else {
        std::env::var("FLOE_RUST_DENSITY_TOP_PLANES").ok().and_then(|v| v.trim().parse::<usize>().ok()).filter(|&k| k > 0).unwrap_or(DENSITY_TOP_PLANES)
    };
    styled.raster.density_top_planes = count.clamp(1, styled.layers.len().max(1)).min(u16::MAX as usize) as u16;
    styled
}

/// Pass 1's shapes come first (user 2026-10-04, the real chip: 787 and 789
/// draw nothing in pass 1 - in pass 2 787 covers 789 or some of 789's density
/// goes; "if pass 1 drew the shapes past the cut, density drawn only in the
/// space left will hardly jar"; then "go on with the simplification too"): no
/// plane's density, the top planes' neither, shows where pass 1 wrote or
/// covers (render-core GeometryRasterRequest::density_shapes_first) - the top
/// planes, each planned on its own (density_top_group), fill the space left
/// top first, the planes under them what is left after. FLOE_RUST_DENSITY_SHAPES_FIRST=off
/// is the kill switch: each top plane's density over the originals of the
/// planes below it (0.12.293).
fn density_shapes_first() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FLOE_RUST_DENSITY_SHAPES_FIRST").as_deref() != Ok("off"))
}

/// The density stack's pass 2 alone (user 2026-10-04: "789's dots still go
/// when 787 is on - it may be pass 1's budget; an option to pass the shapes
/// by and draw the density alone, to compare"): pass 1 decodes no page and
/// draws no shape (its plan keeps the hierarchy), so pass 2 has the whole
/// budget as its reserve (density_frame_reserve: nothing left by pass 1).
/// FLOE_RUST_DENSITY_ONLY=on, diagnostic, off by default; the viewer's density
/// tag says `density only`.
fn density_only() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FLOE_RUST_DENSITY_ONLY").as_deref() == Ok("on"))
}

/// Pass 2 serves the top plane first (user 2026-10-04: the dots 789 lit alone
/// went, many of them, with 787 on - "the density draws 789 first, then 787,
/// so 789's dots should stay"): the pages to decode are the top plane's side's
/// before the others' (they were in one list by distance from the view's
/// centre, under one reserve: the routing chip's M8 over M1 under a 256 MB
/// budget kept 76 % of M8's pixels); the others take what is left, as they
/// planned - since 0.12.292 each plane's pages in the drawing order down (789's
/// lower datatypes before 787's, in the others' side). FLOE_RUST_DENSITY_TOP_FIRST=off
/// is the kill switch.
fn density_top_first() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FLOE_RUST_DENSITY_TOP_FIRST").as_deref() != Ok("off"))
}

/// The viewer's fit view: the die and this margin (floe gui._fit_spp).
const VIEWER_FIT_MARGIN: f64 = 1.05;

/// density_zoom_gain's power: FLOE_RUST_DENSITY_ZOOM_OUT=off is the kill
/// switch (None: no thinning); FLOE_RUST_DENSITY_ZOOM_OUT_POWER, diagnostic,
/// the power (1 by default, 0..=4).
fn density_zoom_out() -> Option<f64> {
    static POWER: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *POWER.get_or_init(|| {
        if std::env::var("FLOE_RUST_DENSITY_ZOOM_OUT").as_deref() == Ok("off") {
            return None;
        }
        Some(
            std::env::var("FLOE_RUST_DENSITY_ZOOM_OUT_POWER")
                .ok()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| (0.0..=4.0).contains(v))
                .unwrap_or(1.0),
        )
    })
}

/// The sub-cut dots' gain of a frame zoomed out past the viewer's fit view
/// of its die - the plan's top cell's box, the view root or the layout's top
/// (gui._die_bbox) - in its viewport (the frame's own size, or a margin's
/// `vw`/`vh`): (the fit's scale / the frame's)^power, 1 at the fit view and
/// within it (user 2026-10-04: zooming out, more shapes fall under a pixel
/// as the die's part of the screen shrinks - "how about drawing it sparser
/// the more it is zoomed out"; the routing chip at depth 0, two steps out
/// from fit, its wire pieces all under the 1 px floor at once: dots over the
/// die where the step before drew lines).
fn density_zoom_gain(cache: &Cache, command: &RenderCommand, top: u32) -> f64 {
    let Some(power) = density_zoom_out() else {
        return 1.0;
    };
    let Some(die) = cache.cell_rbbox(top) else {
        return 1.0;
    };
    let [x0, y0, x1, y1] = command.view;
    let spp = ((x1 - x0) / command.width as f64).max((y1 - y0) / command.height as f64);
    let (vw, vh) = command.viewport.unwrap_or((command.width, command.height));
    let fit = ((die.x1 - die.x0).max(0) as f64 / vw as f64).max((die.y1 - die.y0).max(0) as f64 / vh as f64) * VIEWER_FIT_MARGIN;
    if !(spp > 0.0 && fit > 0.0) || spp <= fit {
        return 1.0;
    }
    (fit / spp).powf(power).clamp(0.0, 1.0)
}

/// A gain under 1 (density_zoom_gain) on a pass 2 plan's dot items: a
/// counted one keeps floor(count x gain + its dither) dots - the dither of
/// its cell, layer and box, the same in every frame and plan - and goes when
/// none is left; a wash without a count (a page wash) stays.
fn thin_dot_items(plan: &mut HierPlan, gain: f64) {
    if !(gain < 1.0) {
        return;
    }
    for cell in &mut plan.wcells {
        if !cell.dot_counts.iter().any(|&count| count > 0) {
            continue;
        }
        let mut washes = Vec::with_capacity(cell.washes.len());
        let mut counts = Vec::with_capacity(cell.washes.len());
        for (at, &(layer, b)) in cell.washes.iter().enumerate() {
            let count = cell.dot_counts.get(at).copied().unwrap_or(0);
            if count == 0 {
                washes.push((layer, b));
                counts.push(0);
                continue;
            }
            let mut z = (cell.key.0 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ (cell.key.1 as u64).rotate_left(17)
                ^ (layer as u64).rotate_left(31)
                ^ (b.x0 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
                ^ (b.y0 as u64).rotate_left(23)
                ^ (b.x1 as u64).rotate_left(41)
                ^ (b.y1 as u64).rotate_left(53);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            let dither = (z >> 11) as f64 / (1u64 << 53) as f64;
            let kept = (count as f64 * gain + dither).floor();
            if kept < 1.0 {
                continue;
            }
            washes.push((layer, b));
            counts.push(kept.min(u16::MAX as f64) as u16);
        }
        cell.washes = washes;
        cell.dot_counts = counts;
    }
}

/// Pass 2's regions by cells of the free space (2026-10-03, a reviewer: "per
/// tile the bounding box of its free pixels is nearly the tile when 1 % of
/// it is free, and the joint plan was decided by those boxes' area"): (cell
/// px, the others' least free share of a cell). FLOE_RUST_DENSITY_FREE_CELLS=
/// off is the kill switch (a tile's bounding box, by area);
/// FLOE_RUST_DENSITY_FREE_CELL_PX and FLOE_RUST_DENSITY_OTHERS_MIN are
/// diagnostics.
fn density_free_cells() -> Option<(usize, f64)> {
    static CELLS: std::sync::OnceLock<Option<(usize, f64)>> = std::sync::OnceLock::new();
    *CELLS.get_or_init(|| {
        if std::env::var("FLOE_RUST_DENSITY_FREE_CELLS").as_deref() == Ok("off") {
            return None;
        }
        let cell = std::env::var("FLOE_RUST_DENSITY_FREE_CELL_PX").ok().and_then(|v| v.trim().parse::<usize>().ok()).filter(|v| (4..=1024).contains(v)).unwrap_or(DENSITY_FREE_CELL_PX);
        let min = std::env::var("FLOE_RUST_DENSITY_OTHERS_MIN").ok().and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| (0.0..=1.0).contains(v)).unwrap_or(DENSITY_OTHERS_MIN);
        Some((cell, min))
    })
}

/// density_free_cells: the cell, px, and the least share of a cell's pixels
/// free for the other planes' density to plan it (fewer: the cell is shown
/// enough by the originals)
const DENSITY_FREE_CELL_PX: usize = 32;
const DENSITY_OTHERS_MIN: f64 = 0.125;

/// The threads pass 2 of the sub-cut dots plans its regions on (2026-10-01,
/// field: one plan of a 5.2 mm view walked 4.0 M nodes in 3.5 s on one core
/// while the raster workers waited): the regions dealt round robin to that
/// many bands, each planned as asked on its own thread, the plans merged
/// (Cache::merge_plans: a block's item the one of the most dots) and the
/// merge fitted to the reserve as the one plan would be (Cache::fit_plan:
/// held whole, under the scale's decision, or decided anew - since 0.12.267;
/// before, only a view the reserve held whole took the threads, and the slow
/// ones thin: user 2026-10-01, a 4.3 s pass 2 of a root's fit view, `thinned`).
/// A fit that would plan again (a decision at a coarser cut, pages past its
/// overshoot) plans as one. The work is uneven - on the synthetic chip's
/// H01_00001 fit view (796 x 798 px) one of nine tiles held over half of it,
/// 785 ms on one thread, 517 / 439 / 487 ms on 2 / 4 / 8 (bands 376 ms and a
/// single-threaded merge of 68-122 ms at 4-8), frames byte for byte the one
/// plan's; a cell's view boxes, merged per plan, differed on another view by
/// 27 px. On by default since 0.12.268 (DENSITY_PLAN_THREADS, field
/// 2026-10-02). FLOE_RUST_DENSITY_PLAN_THREADS=N (diagnostic) sets the threads,
/// 1 one plan (the kill switch); the decode workers bound nothing here.
fn density_plan_threads(_decode_workers: u16) -> usize {
    std::env::var("FLOE_RUST_DENSITY_PLAN_THREADS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&threads| threads >= 1)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |cores| cores.get()).min(DENSITY_PLAN_THREADS))
        .min(16)
}

/// density_plan_threads' default: four, or the cores there are (user
/// 2026-10-02: on four threads MAIN01's `ltv_top_RTG` fit view went from over
/// 6 s to under 4, pass 2 planned 2,922 ms; on eight 2,847 ms, the picture
/// alike to the eye).
const DENSITY_PLAN_THREADS: usize = 4;

/// Pass 2's floor probe on the threads its fit plans on (density_plan_threads;
/// user 2026-10-04, the field chip, 789.0 alone at depth 1 with a floor under
/// the density cut: `pass 2 plan 7499 ms` with the probe over a 128 MB
/// reserve, 7258 ms with it held by 1 GB - the probe planned as one, and a
/// probe that fits is the plan, so the threads never ran): the regions dealt
/// round robin, each band a probe to the reserve, the bands merged
/// (Cache::merge_plans) - over when a band is or the merge's pages
/// (Cache::plan_page_cost) pass the reserve; one that fits is the plan.
/// FLOE_RUST_DENSITY_PROBE_THREADS=off is the kill switch: the probe as one
/// plan.
fn density_probe_threads() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FLOE_RUST_DENSITY_PROBE_THREADS").as_deref() != Ok("off"))
}

fn density_one_walk_enabled() -> bool {
    std::env::var("FLOE_RUST_DENSITY_ONE_WALK").as_deref() == Ok("on")
}

/// The sub-cut dots' floor (px, the larger side): FLOE_RUST_DENSITY_FLOOR_PX
/// (diagnostic), unset, empty or invalid DOT_FLOOR_PX. The one walk's record
/// floor, and the page floor dot_page_floors probes under it.
fn dot_record_floor_px() -> f64 {
    std::env::var("FLOE_RUST_DENSITY_FLOOR_PX")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(DOT_FLOOR_PX)
}

/// The sub-cut dots' default floor: 1 px (user 2026-10-01: "isn't it the
/// 0 px floor? what about fixing the floor at 1 px"). 0 px before: on the
/// field chip its probe overflowed the reserve on the first frame at a scale
/// and the fit walked again at 1 px; at the density cut there is no probe.
/// Shapes under it in the cells above the cut are not drawn (nor dotted,
/// HierOpts::dot_pages): the synthetic chip's first 10 layers x16 light 0.076
/// against 0.058 drawn without a cut (0.080 at 0 px).
const DOT_FLOOR_PX: f64 = 1.0;

/// The page floor (px, the larger side) pass 2 of the sub-cut dots tries
/// first: taken per scale and side when its pages fit the density reserve
/// (render_density_frame), else the density cut (density_cut_px) with the
/// planner's budget fit. One rung: each that does not fit costs a probe on the
/// first frame at a scale (about 0.4 s on the synthetic chip's all-layer fit
/// view). FLOE_RUST_DENSITY_FLOOR_PX, diagnostic (user 2026-09-30: compare
/// 0.5 and 0.25 px); unset, empty or out of range means DOT_FLOOR_PX (1 px);
/// a floor at or above the density cut goes straight to it - no probe.
fn dot_page_floors() -> Vec<f64> {
    let floor = dot_record_floor_px();
    if floor < density_cut_px() { vec![floor] } else { Vec::new() }
}

/// The sub-cut dots show pass 1 as a first round (final=0) before pass 2
/// (CUT_DENSITY_DESIGN §10.12 step 3; the viewer's refining path): on with the
/// dots, FLOE_RUST_DENSITY_PROGRESSIVE=off the kill switch. The last frame is
/// the one a single round draws.
fn density_progressive_enabled() -> bool {
    std::env::var("FLOE_RUST_DENSITY_PROGRESSIVE").as_deref() != Ok("off")
}

/// The density stack's sub-cut dots (floe_vfs HierOpts::sub_cut_dots,
/// CUT_DENSITY_DESIGN §10.12; user 2026-09-30: "a cell of 3 x 3 px or less
/// is one dot, no descent"): pass 2 plans the cells at pass 1's cut - a cell
/// under it stands as dots, never walked into or decoded - and the pages at
/// the density cut. FLOE_RUST_DENSITY_DOTS=on, diagnostic and off by default
/// (with the stack only).
fn density_dots_enabled() -> bool {
    std::env::var("FLOE_RUST_DENSITY_DOTS").as_deref() == Ok("on")
}

/// The density stack of this frame: the command's `density=` (the viewer's
/// toggle, 2026-10-05), or without it FLOE_RUST_DENSITY_STACK=top
/// (density_stack_enabled).
fn density_stack_on(command: &RenderCommand) -> bool {
    command.density.unwrap_or_else(density_stack_enabled)
}

/// The sub-cut dots of this frame: with the command's `density=on` on unless
/// FLOE_RUST_DENSITY_DOTS=off - the viewer's density is the dots' - with
/// `density=off` none, without it FLOE_RUST_DENSITY_DOTS=on
/// (density_dots_enabled).
fn density_dots_on(command: &RenderCommand) -> bool {
    match command.density {
        Some(true) => std::env::var("FLOE_RUST_DENSITY_DOTS").as_deref() != Ok("off"),
        Some(false) => false,
        None => density_dots_enabled(),
    }
}

/// The density stack's brightness (user 2026-10-05: "the brightness of a
/// pixel by the shapes' size that gathers on it", "never brighter than the
/// original colour", then "go on with g = 1, 2, 4"): with the sub-cut dots,
/// pass 2 counts the area its shapes cover (floe_vfs HierOpts::dot_bright)
/// and a pixel shows min(1, g x its covered share) of its plane's colour, the
/// top plane over the others (render-core GeometryRasterRequest::
/// density_bright) - in place of the dots' gate (HierOpts::dot_gate) and the
/// zoomed-out gain (density_zoom_gain). g by the detail, 2^((5 - cut) / 2):
/// low (5 px) 1, medium (3 px) 2, high (1 px) 4. None: no brightness - the
/// dots as lit pixels (FLOE_RUST_DENSITY_BRIGHT=off is the kill switch, as
/// 0.12.296), or no dots. FLOE_RUST_DENSITY_BRIGHT_GAIN, diagnostic: g itself
/// (1..=64).
fn density_bright_gain(command: &RenderCommand) -> Option<f64> {
    let cut_px = command.cut_px;
    if std::env::var("FLOE_RUST_DENSITY_BRIGHT").as_deref() == Ok("off") || !density_dots_on(command) || !density_stack_on(command) || !(cut_px > 0.0) {
        return None;
    }
    let fixed = std::env::var("FLOE_RUST_DENSITY_BRIGHT_GAIN")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|g| g.is_finite() && (1.0..=64.0).contains(g));
    Some(fixed.unwrap_or_else(|| 2f64.powf((5.0 - cut_px) / 2.0).clamp(1.0, 4.0)))
}

/// Pass 2's cut (px, the larger side): the shapes under pass 1's cut down to
/// this size are density. 1 px (user decision 2026-09-27: 0.5 px first, so
/// the count stays in bounds; then 1 px, which looks fine at detail medium
/// and costs a fraction - last-10-layer fit 2.3 s -> 47 ms on the synthetic
/// chip). FLOE_RUST_DENSITY_CUT_PX, diagnostic; unset, empty or out of range
/// means 1.
fn density_cut_px() -> f64 {
    std::env::var("FLOE_RUST_DENSITY_CUT_PX")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|c| c.is_finite() && *c > 0.0)
        .unwrap_or(1.0)
}

/// With the page spread (floe_vfs HierOpts::dot_page_spread) pass 2 draws a
/// decoded page's shapes under the records' floor, by the area they cover as
/// every shape under a pixel, rather than leaving them out: the spread pages
/// under the floor stand for theirs by area, and a page holding shapes on
/// both sides of the floor was left blank between them (user 2026-10-03, the
/// routing chip's fill: 0.3 um squares paged with 1 um array squares were
/// blank where their page was decoded and dense where it was spread).
/// The page spread in effect: on by default where the index has design.ovb,
/// anywhere with FLOE_RUST_DENSITY_PAGE_SPREAD=on (floe_vfs Hier's page_spread).
/// FLOE_RUST_DENSITY_UNDER_FLOOR=drop is the kill switch.
fn density_under_floor_drawn(cache: &Cache) -> bool {
    let spread = floe_render_core::dot_page_spread()
        && (floe_render_core::dot_page_spread_boxes() || (floe_render_core::dot_page_occ() && cache.has_page_occ()));
    spread && std::env::var("FLOE_RUST_DENSITY_UNDER_FLOOR").as_deref() != Ok("drop")
}

/// The occupancy first (floe_vfs HierOpts::dot_occ_first; a reviewer
/// 2026-10-05, then the exact cover: on the routing chip's fit view the pages
/// pass 2 decoded from 1 px up were 277, 110 of them within 1 GB - x0.49 of
/// what all of them draw - where their occupancy grids draw x1.01 in a third
/// of the time): under the brightness, on an index with design.ovb, pass 2's
/// pages are cut at pass 1's cut, not at the density cut - a page all under
/// it is spread by its occupancy grid while the grid's cells show no larger
/// than FLOE_RUST_DENSITY_OCC_CELL_PX (4), decoded past that or with no grid
/// to go by; no floor is probed. FLOE_RUST_DENSITY_OVB_FIRST=off is the kill
/// switch: the pages from the density cut up decoded, as 0.12.298.
fn density_ovb_first(cache: &Cache, command: &RenderCommand) -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FLOE_RUST_DENSITY_OVB_FIRST").as_deref() != Ok("off"))
        && density_bright_gain(command).is_some()
        && floe_render_core::dot_page_occ()
        && floe_render_core::dot_occ_decode()
        && cache.has_page_occ()
        && density_under_floor_drawn(cache)
}

/// What pass 2 may decode on top of pass 1 (bytes, encoded x 2 as the
/// estimate): FLOE_RUST_DENSITY_BUDGET_MB, diagnostic, default 256 MB - and
/// never more than half of what the generation budget has left.
fn density_budget_bytes() -> u64 {
    std::env::var("FLOE_RUST_DENSITY_BUDGET_MB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|mb| *mb > 0)
        .unwrap_or(256)
        .saturating_mul(1 << 20)
}

/// Pass 1's decode budget: the generation's, less pass 2's reserve when the
/// density stack is on (never under half of it), so pass 2 always has the
/// reserve whatever pass 1 decoded (2026-09-27: planned to half of what was
/// left, the margin's pass 2 decoded 23 pages where the viewport's decoded
/// 2,208 and the fit view changed when the margin landed).
fn pass1_decode_budget(budget: u64, command: &RenderCommand) -> u64 {
    if budget > 0 && density_stack_on(command) && !command.exact {
        budget.saturating_sub(density_reserve(budget))
    } else {
        budget
    }
}

/// Pass 2's reserve of the generation budget: FLOE_RUST_DENSITY_BUDGET_MB
/// (256) but never over an eighth of the generation (128 MB of the default
/// 1024), so pass 1 keeps most of the budget for the originals.
fn density_reserve(budget: u64) -> u64 {
    density_budget_bytes().min(budget / 8).max(1)
}

/// Pass 2's reserve in a frame: density_reserve - pass 1 plans to leave it -
/// or, when larger, what pass 1 left of the generation budget (its pages'
/// bytes, `pass1_bytes`), so a frame whose pass 1 is light gives pass 2 the
/// rest (user 2026-10-02, the field chip at depth 0: a root's own shapes under
/// a pixel - in a test, a million boxes, 192 MB by estimate - failed the 0 px
/// floor's probe of the 128 MB reserve while pass 1 held a few pages of the
/// 1024 MB, and the view drew nothing under the cut). The budget stays the
/// generation's. FLOE_RUST_DENSITY_RESERVE_LEFT=off (the kill switch): the
/// fixed reserve, as before.
fn density_frame_reserve(budget: u64, pass1_bytes: u64) -> u64 {
    let fixed = density_reserve(budget);
    if std::env::var("FLOE_RUST_DENSITY_RESERVE_LEFT").as_deref() == Ok("off") {
        fixed
    } else {
        fixed.max(budget.saturating_sub(pass1_bytes))
    }
}

/// The density stack counts its pages as the parser read them
/// (DecodedPage::grown_charge), their record lists holding the room they
/// grew to: on. Since 0.12.301 a decoded page's lists are cut to their
/// length (floe_render_core decode_shrink) and a page is charged a fifth to
/// a third less; pass 2 has what pass 1's pages leave of the budget
/// (density_frame_reserve), so counted as held pass 2 would have more and
/// decode more - the synthetic MAIN01 1/10 at full depth 8-17 % slower at
/// fit and 14-20 % two steps out for the same picture, the routing chip two
/// steps in with 133 pages decoded for 86 (2026-10-05).
/// Counted as read, pass 2's reserve and its last check are what they were
/// and a density frame is the same picture in the same time; the pages
/// themselves are held smaller. FLOE_RUST_DENSITY_AS_READ=off counts them as
/// held: pass 2 has what the cut lists free - the user's to choose, with
/// pass 2's reserve.
fn density_as_read() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("FLOE_RUST_DENSITY_AS_READ").as_deref() != Ok("off"))
}

/// The error a margin's pass 2 raises when its budget fit does not hold the
/// scale's decision: the frame is dropped like a pass-1 refit of a margin.
const DROPPED_FIT: &str = "dropped:fit";

fn page_wash_enabled() -> bool {
    std::env::var("FLOE_RUST_PAGE_WASH").as_deref() == Ok("on")
}

/// The M7 LOD swap (floe_vfs::ViewReq::lod_swap) on a plain layout's frames.
/// OFF by default (user decision 2026-09-22: LOD is not in use; the viewer's
/// toggle is gone, and it never reached renderd anyway). The index builds no
/// merged variants unless `floe2 index --lod`; FLOE_RUST_LOD=on swaps them in
/// again for a cache that has them.
fn lod_enabled() -> bool {
    std::env::var("FLOE_RUST_LOD").as_deref() == Ok("on")
}

/// The page frontier (floe_vfs::ViewReq::page_reps) on a plain
/// layout's frames. DEACTIVATED (user decision 2026-09-17: the field
/// still saw boxes and a 60 s full-depth plan on 0.12.152, and the
/// answer moves to representative data built at index time into a
/// file of its own); FLOE_RUST_PAGE_REPS=on turns the planner-side
/// representatives on for a diagnosis.
fn page_reps_enabled() -> bool {
    std::env::var("FLOE_RUST_PAGE_REPS").as_deref() == Ok("on")
}

/// docs/LAYER_DECODE_PROBE_PLAN.ko.md: the `render_probe` command. One plan,
/// painted the way `mode` asks, published as a `probe_frame` so a normal
/// render's answer can never be confused with a diagnostic one.
///
/// `baseline` decodes the whole selection and renders it in one call.
/// `ordered` and `occlusion` build a metadata scene - it knows every page of
/// the plan before one is read - and decode block by block: `ordered` asks for
/// every page of the block's layers, `occlusion` only for those a pass could
/// still paint into an open pixel. The two of the same block size are the
/// pair to compare, and all three must reach the same pixels.
#[allow(clippy::too_many_arguments)]
/// Microseconds between reading the command and starting on it.
fn queued_us(command: &RenderCommand, started: Instant) -> u64 {
    command
        .received
        .map(|received| {
            started
                .saturating_duration_since(received)
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX)
        })
        .unwrap_or(0)
}

fn run_layer_probe(
    run_started: Instant,
    queue_us: u64,
    page_cache: &mut DecodedPageCache,
    cache: &Cache,
    command: &RenderCommand,
    responses: &Sender<String>,
    cancellation: &RenderCancellation,
    mode: ProbeMode,
    plan: Arc<HierPlan>,
    selected: &[u32],
    styles: &[LayerStyle],
    raster_request: GeometryRasterRequest,
    labels: Arc<[RenderLabel]>,
    summary: &SummarySelection,
    decode_workers: u16,
) -> Result<(), String> {
    let started = Instant::now();
    if styles.is_empty() {
        return Err("a layer-decode probe needs styled layers".to_string());
    }
    let mut probe = LayerProbeReport::new(mode);
    probe.planned_pages = plan.pages.len() as u64;
    probe.selected_pages = selected.len() as u64;
    let styled = StyledGeometryRasterRequest {
        raster: raster_request,
        layers: styles.to_vec(),
        hierarchy_frames: command.frames,
        mono: command.mono,
    };
    let report = if mode == ProbeMode::Baseline {
        let decode_started = Instant::now();
        let (pages, decode_stats) = page_cache.load_cancellable(
            cache,
            selected,
            decode_workers,
            command.generation,
            cancellation,
        )?;
        probe.decode_us = elapsed_us(decode_started);
        probe.requested_pages = selected.len() as u64;
        probe.decoded_pages = pages.len() as u64;
        probe.decoded_bytes = pages.iter().map(|page| page.estimated_bytes()).sum();
        probe.cache_hits = u64::from(decode_stats.decoded_cache_hit);
        probe.cache_misses = u64::from(decode_stats.decoded_cache_miss);
        probe.read_us = decode_stats.page_read_us;
        probe.decode_sum_us = decode_stats.page_decode_sum_us;
        check_generation(cancellation, command.generation)?;
        let scene_started = Instant::now();
        let mut scene = FrameScene::new_shared_with_labels(
            cache,
            Arc::clone(&plan),
            pages,
            Arc::clone(&labels),
            command.label_font_px,
        )?;
        if summary.is_active() {
            scene.set_summaries(summary.planes.clone());
        }
        probe.scene_us = elapsed_us(scene_started);
        let paint_started = Instant::now();
        let report =
            render_geometry_styled_cancellable(&scene, &styled, command.generation, cancellation)?;
        probe.paint_us = elapsed_us(paint_started);
        report
    } else {
        // FLOE_RUST_WORK_BIN=off keeps its meaning here: the per-tile walk.
        // The demand reads the collection, so occlusion needs it, and it can
        // only show coverage with the write-once masks.
        let work_bin = std::env::var("FLOE_RUST_WORK_BIN").as_deref() != Ok("off");
        let occlusion = mode == ProbeMode::Occlusion;
        if occlusion && !work_bin {
            return Err("probe mode occlusion needs the work bin".to_string());
        }
        if occlusion && std::env::var("FLOE_RUST_WRITE_ONCE").as_deref() == Ok("off") {
            return Err("probe mode occlusion needs the write-once masks".to_string());
        }
        // the pages arrive block by block, so the collection and the masks
        // come from the plan's page metadata (LAYER_DECODE_PROBE_PLAN §4)
        let scene_started = Instant::now();
        let mut scene = FrameScene::new_metadata(
            cache,
            Arc::clone(&plan),
            Arc::clone(&labels),
            command.label_font_px,
        )?;
        if summary.is_active() {
            scene.set_summaries(summary.planes.clone());
        }
        let scene = scene;
        probe.scene_us = elapsed_us(scene_started);
        let prepare_started = Instant::now();
        let session = LayerRasterSession::begin_cancellable(
            &scene,
            &styled,
            work_bin,
            command.generation,
            cancellation,
        )?;
        probe.passes = session.passes() as u64;
        probe.prepare_us = elapsed_us(prepare_started);
        let paint_started = Instant::now();
        let mut loaded: BTreeSet<u32> = BTreeSet::new();
        let mut generation_bytes = 0u64;
        let mut wanted: Vec<u32> = Vec::new();
        let mut failed: Option<String> = None;
        // the decode workers stay up for the frame: one worker set per block
        // was most of a layer-ordered frame's decode (0.12.178)
        let (report, pool_us) = cache.with_decode_pool(
            decode_workers,
            Some((command.generation, cancellation)),
            |pool| session.render_layered_cancellable(
            &scene,
            &styled,
            command.generation,
            cancellation,
            command.probe_block,
            |planes, demand| {
                probe.blocks += 1;
                probe.layer_passes += planes.len() as u64;
                let demand_started = Instant::now();
                wanted.clear();
                for &plane in planes {
                    let stats = demand.pages_for_plane(plane, occlusion, &mut wanted);
                    probe.demand_candidates += stats.candidates;
                    probe.demand_out_of_view += stats.out_of_view;
                    probe.demand_occluded += stats.occluded;
                    probe.demand_unsure += stats.unsure;
                }
                wanted.sort_unstable();
                wanted.dedup();
                wanted.retain(|page_id| !loaded.contains(page_id));
                probe.demand_us += elapsed_us(demand_started);
                if wanted.is_empty() {
                    return Ok(());
                }
                let decode_started = Instant::now();
                let (pages, decode_stats) = page_cache.load_pooled(pool, &wanted)?;
                probe.decode_us += elapsed_us(decode_started);
                probe.requested_pages += wanted.len() as u64;
                probe.cache_hits += u64::from(decode_stats.decoded_cache_hit);
                probe.cache_misses += u64::from(decode_stats.decoded_cache_miss);
                probe.read_us += decode_stats.page_read_us;
                probe.decode_sum_us += decode_stats.page_decode_sum_us;
                let block_bytes = pages.iter().try_fold(0u64, |total, page| {
                    total
                        .checked_add(page.estimated_bytes())
                        .ok_or_else(|| "decoded generation byte charge overflow".to_string())
                })?;
                probe.decoded_bytes += block_bytes;
                generation_bytes = checked_generation_bytes(
                    generation_bytes,
                    block_bytes,
                    page_cache.budget_bytes(),
                )?;
                probe.decoded_pages += pages.len() as u64;
                for page in pages {
                    loaded.insert(page.page_id);
                    if let Err(error) = scene.set_decoded_page(page) {
                        failed.get_or_insert(error);
                    }
                }
                Ok(())
            },
        ),
        )?;
        let report = report?;
        probe.pool_us = pool_us;
        if let Some(error) = failed {
            return Err(error);
        }
        // the raster alone: reading and asking happen on this thread between
        // blocks and must not be counted as painting
        // the raster alone: reading, asking and the decode pool itself happen
        // on this thread, between blocks
        probe.paint_us = elapsed_us(paint_started)
            .saturating_sub(probe.decode_us)
            .saturating_sub(probe.demand_us)
            .saturating_sub(probe.pool_us);
        probe.skipped_pages = probe.selected_pages.saturating_sub(loaded.len() as u64);
        probe.skipped_bytes = selected
            .iter()
            .filter(|page_id| !loaded.contains(page_id))
            .map(|&page_id| cache.page_encoded_bytes(page_id))
            .sum();
        report
    };
    check_generation(cancellation, command.generation)?;
    let frame = report.frame;
    let png = if command.raw_frame {
        None
    } else {
        Some(frame.png_bytes()?)
    };
    let raw_header = command
        .raw_frame
        .then(|| raw_frame_header(frame.width(), frame.height()));
    let parts: Vec<&[u8]> = match (&raw_header, &png) {
        (Some(header), _) => vec![header.as_slice(), frame.pixels()],
        (None, Some(png)) => vec![png.as_slice()],
        _ => return Err("probe frame has neither raw pixels nor PNG bytes".to_string()),
    };
    let publish = publish_frame(&command.out, command.generation, &parts, cancellation)?;
    probe.total_us = elapsed_us(started);
    respond(
        responses,
        format!(
            "probe_frame gen={} mode={} block={} format={} out={} partial={} planned_pages={} selected_pages={} requested_pages={} decoded_pages={} cache_hits={} cache_misses={} skipped_pages={} skipped_bytes={} decoded_bytes={} demand_candidates={} demand_out_of_view={} demand_occluded={} demand_unsure={} passes={} blocks={} layer_passes={} decode_us={} read_us={} decode_sum_us={} demand_us={} pool_us={} scene_us={} prepare_us={} paint_us={} total_us={} raster_us={} raster_tile_max_us={} tiles={} workers={} bin_items={} once_tiles={} once_passes={} once_items={} publish_write_us={} publish_sync_us={} publish_rename_us={} queue_us={} wall_us={}",
            command.generation,
            probe.mode,
            command.probe_block,
            if command.raw_frame { "raw" } else { "png" },
            command.out,
            report.partial as u8,
            probe.planned_pages,
            probe.selected_pages,
            probe.requested_pages,
            probe.decoded_pages,
            probe.cache_hits,
            probe.cache_misses,
            probe.skipped_pages,
            probe.skipped_bytes,
            probe.decoded_bytes,
            probe.demand_candidates,
            probe.demand_out_of_view,
            probe.demand_occluded,
            probe.demand_unsure,
            probe.passes,
            probe.blocks,
            probe.layer_passes,
            probe.decode_us,
            probe.read_us,
            probe.decode_sum_us,
            probe.demand_us,
            probe.pool_us,
            probe.scene_us,
            probe.prepare_us,
            probe.paint_us,
            probe.total_us,
            report.stats.raster_us,
            report.stats.raster_tile_max_us,
            report.stats.tiles,
            report.stats.workers_used,
            report.stats.work_bin_items,
            report.stats.once_full_tiles,
            report.stats.once_passes_skipped,
            report.stats.once_items_skipped,
            publish.write_us,
            publish.sync_us,
            publish.rename_us,
            queue_us,
            elapsed_us(run_started),
        ),
    );
    Ok(())
}

fn run_render(
    state: &mut WorkerState,
    command: &RenderCommand,
    responses: &Sender<String>,
    cancellation: &RenderCancellation,
    published_scene: &SharedPublishedScene,
) -> Result<(), String> {
    // wall_us counts from here, queue_us up to here (see RenderCommand::received)
    let run_started = Instant::now();
    let queue_us = queued_us(command, run_started);
    // a frame whose pages pass the generation budget is planned anew under
    // less of it (budget_refit_enabled)
    let mut refits = 0u32;
    loop {
        match run_render_attempt(state, command, responses, cancellation, published_scene, run_started, queue_us, refits) {
            Err(error) if error == REFIT_BUDGET => refits += 1,
            result => return result,
        }
    }
}

/// run_render once: `refits` plans of this frame before it passed the budget.
#[allow(clippy::too_many_arguments)]
fn run_render_attempt(
    state: &mut WorkerState,
    command: &RenderCommand,
    responses: &Sender<String>,
    cancellation: &RenderCancellation,
    published_scene: &SharedPublishedScene,
    run_started: Instant,
    queue_us: u64,
    refits: u32,
) -> Result<(), String> {
    // the layers' pages decode larger than estimated by this much
    // (WorkerState::budget_scale): planned to the budget over it
    let scale_key = budget_scale_key(command);
    let budget_scale = state.budget_scale.get(&scale_key).copied();
    let mut command = command.clone();
    let cache = state
        .cache
        .as_ref()
        .ok_or_else(|| "cache not open".to_string())?;
    // the cells' cover (floe_render_core Cache::cell_cover) is asked for as
    // the frame begins: its table is worked out on a thread of its own
    // while pass 1 draws, ahead of pass 2's plans that read it
    if density_bright_gain(&command).is_some() {
        let _ = cache.cell_cover();
    }
    // occupancy summary (docs/OCCUPANCY_PLAN.ko.md M2): decided per
    // request before any reuse, since the retained-frame and published-
    // scene keys carry it; FLOE_RUST_OCCUPANCY=off is the kill switch
    // the policy condition: keep, or cull (2026-09-18) unless
    // FLOE_RUST_OCCUPANCY_CULL=off (the kill switch back to keep-only)
    let policy_allows = command.thin_keep || floe_render_core::summary_cull_allowed();
    let summary = cache.summary_selection(
        &make_plan_request(cache, &command, state.page_cache.budget_bytes())?,
        policy_allows,
        // design.ovo flattens the TOP cell: under another view root it is
        // not this picture's summary
        std::env::var("FLOE_RUST_OCCUPANCY").as_deref() == Ok("off") || command.root.is_some(),
        // a plain layout draws no summary unless FLOE_RUST_OCCUPANCY=on
        // (user decision 2026-09-24; jobdeck passes keep it)
        !floe_render_core::summary_layout_allowed(),
    )?;
    let summary_key = SummaryKey::of(&summary);
    // the budget fit a retained frame must have been planned under to serve
    // this request: the scale's remembered decision (the fit key does not
    // depend on the view's position, so the request before the snap gives it)
    // ... or every page at the asked cut: a frame its budget holds whole keeps
    // them all whatever the decision (floe_vfs plan_hier_fixed, user
    // 2026-10-01) - which of the two this frame is, its plan says, and a
    // reuse drawn under the other is dropped then
    let (expected_fit, expected_whole) = if command.exact {
        (None, None)
    } else {
        let pre = make_plan_request(cache, &command, scaled_decode_budget(state.page_cache.budget_bytes(), &command, budget_scale))?;
        if pre.decode_budget > 0 {
            (state.fit_memory.get(&fit_memory_key(&command, &pre)).copied(), Some(floe_render_core::FixedFit::everything(pre.cut_dbu)))
        } else {
            (None, None)
        }
    };
    let mut reuse_fit = expected_fit;
    let mut pan_reuse = prepare_pan_reuse(state, &mut command, &summary_key, expected_fit);
    if pan_reuse.is_none() && expected_fit.is_some() && expected_whole != expected_fit {
        pan_reuse = prepare_pan_reuse(state, &mut command, &summary_key, expected_whole);
        if pan_reuse.is_some() {
            reuse_fit = expected_whole;
        }
    }
    let command = &command;
    check_generation(cancellation, command.generation)?;
    // the density stack's pass 2 decodes within a reserve of its own
    // (density_budget_bytes): pass 1 plans to what the generation has left
    let request = make_plan_request(cache, command, scaled_decode_budget(state.page_cache.budget_bytes(), command, budget_scale))?;
    // the summarized layers leave the page plan (§6 step 3): no page
    // selection, page BVH or child walk for them
    let mut page_request = cache.page_plan_request(&request, &summary, !command.frames)?;
    // §F2R-21 label re-synthesis: when a retained frame (the margin
    // prefetch) covers the WHOLE request, its geometry is a pure
    // memcpy - skip the page plan and decode entirely, plan only the
    // labels of THIS viewport and paint them over the reused geometry.
    // The GUI never crops a margin while labels are on (an off-frame
    // label's tail could overwrite in-view labels), so this is what a
    // pan inside the margin costs with labels: text plan + label pass.
    // §F2R-21 review (HIGH): the fast path leaves the published query
    // scene untouched, so it may run only when that scene already
    // serves this request (same render state - layer set, depth, cut,
    // decode pages, style - and a view containing this one). A layer
    // toggle A -> B -> A reuses A's retained frame in full but must
    // replan and republish, or picks on A-only layers would fail.
    // A retained frame drawn under a thinning decision serves only after the
    // plan: this view may be one its budget holds whole (2026-10-01), which
    // only planning tells; a frame drawn whole holds every page of any view
    // inside it.
    let label_only = pan_reuse
        .as_ref()
        .is_some_and(|reuse| reuse.valid == [0, 0, command.width, command.height])
        && (reuse_fit.is_none() || reuse_fit == expected_whole)
        && published_scene_serves(published_scene, command, state.style_epoch, &summary_key)?;
    // the budget fit decided at this scale before, if any (WorkerState::
    // fit_memory); the first frame at a scale decides it over the extent the
    // viewer's margin frame will have - twice the view per axis - so the
    // margin, when it lands, thins as the viewport did (no flip after a zoom)
    let fit_key = fit_memory_key(command, &request);
    if !command.exact && !label_only && page_request.decode_budget > 0 {
        if let Some(decision) = state.fit_memory.get(&fit_key).copied() {
            page_request.fixed_fit = Some(decision);
        } else {
            // the margin's extension per side is half the view snapped to
            // 16 px (gui._submit_margin): up to 8 px more than half - probe
            // 16 px beyond it, so the margin never plans what the probe
            // did not
            let [x0, y0, x1, y1] = command.view;
            let slack = 16.0 / page_request.px_per_dbu.max(f64::MIN_POSITIVE);
            let (dx, dy) = ((x1 - x0) / 2.0 + slack, (y1 - y0) / 2.0 + slack);
            let probe = PlanRequest {
                view: ViewBox::new(
                    checked_bound((x0 - dx).floor(), "probe x0")?,
                    checked_bound((y0 - dy).floor(), "probe y0")?,
                    checked_bound((x1 + dx).ceil(), "probe x1")?,
                    checked_bound((y1 + dy).ceil(), "probe y1")?,
                )?,
                ..page_request.clone()
            };
            let decided = cache.plan_cancellable(&probe, command.generation, cancellation)?.plan.stats.fit_decision;
            check_generation(cancellation, command.generation)?;
            if let Some(decision) = decided {
                state.fit_memory.insert(fit_key.clone(), decision);
                page_request.fixed_fit = Some(decision);
            }
        }
    }
    let mut representative_options = floe_render_core::RepresentativeOptions::default();
    representative_options.max_px_per_dbu = Some(
        (command.width as f64 / (command.view[2] - command.view[0]))
            .max(command.height as f64 / (command.view[3] - command.view[1])));
    representative_options.halo_px = state.styles.iter().map(|s| s.outline_width)
        .max().unwrap_or(1) as f64 + 1.;
    representative_options.direct = std::env::var("FLOE_RUST_REPRESENTATIVES_MERGE").as_deref() == Ok("off")
        || std::env::var("FLOE_RUST_REPRESENTATIVES_DIRECT").as_deref() == Ok("on");
    if !state.styles.is_empty() {
        representative_options.solid_layers = Some(state.styles.iter().filter(|s| s.outline_width == 1
            && (matches!(s.fill, LayerFill::Solid) || matches!(s.fill, LayerFill::Pattern(rows) if rows.iter().all(|&r| r == u16::MAX))))
            .map(|s| s.layer_idx).collect());
        representative_options.hairline_layers = Some(state.styles.iter().filter(|s| s.outline_width == 1)
            .map(|s| s.layer_idx).collect());
    }
    // Diagnostic for the resumable query gate (also useful for IO/paint tuning).
    if let Ok(n) = std::env::var("FLOE_RUST_REPRESENTATIVES_BATCH").unwrap_or_default().parse::<usize>() {
        representative_options.output_per_batch = n.clamp(1, 262144);
    }
    let mut planned = if label_only {
        cache.empty_plan()
    } else {
        if std::env::var("FLOE_RUST_REPRESENTATIVES").as_deref() == Ok("off") || command.thin_keep {
            // a newer generation ends the walk (floe_vfs HierOpts::stop): a
            // chip's plan is seconds, and the next view waited for it
            cache.plan_cancellable(&page_request, command.generation, cancellation)?
        } else {
            cache.plan_with_representatives_options(&page_request, representative_options,
                || cancellation.is_cancelled(command.generation))?
        }
    };
    check_generation(cancellation, command.generation)?;
    // remember the fit this plan decided (a redecision replaces the old one)
    if !label_only && !command.exact {
        // every page as asked (or no budget fit at all)
        let whole = planned.plan.stats.fit_whole || planned.plan.stats.fit_decision.is_none();
        if command.background && (planned.plan.stats.fit_redecided || (!whole && state.fit_whole.contains(&fit_key))) {
            // the viewer's margin does not fit under the scale's decision
            // (denser content than the frame that decided it): drawn under
            // another it would change the picture when it lands (field
            // 2026-09-27: 50% pans into the chip, 1,772 px of the viewport)
            // - dropped; the decision and the viewport stay, pans render.
            // So is one that has to thin where the viewport was held whole
            // (2026-10-01): it would show fewer pages than the viewport did
            respond(responses, format!("dropped gen={} reason=fit", command.generation));
            return Ok(());
        }
        if let Some(decision) = planned.plan.stats.fit_decision {
            if planned.plan.stats.fit_redecided || !state.fit_memory.contains_key(&fit_key) {
                state.fit_memory.insert(fit_key.clone(), decision);
            }
        }
        if !command.background {
            if whole {
                state.fit_whole.insert(fit_key.clone());
            } else {
                state.fit_whole.remove(&fit_key);
            }
        }
        if pan_reuse.is_some() && reuse_fit != planned.plan.stats.fit_decision {
            // the reused tiles were drawn under another selection - the
            // decision this frame replaced (review 2026-09-28), or every page
            // where this frame has to thin, or the reverse (2026-10-01): the
            // pages one drops or adds would sit beside them - the whole
            // frame is drawn anew
            pan_reuse = None;
        }
    }
    // the fit this frame is drawn under, for the retained frame it leaves
    let frame_fit = if label_only { reuse_fit } else { planned.plan.stats.fit_decision };
    let planned_labels = if command.labels {
        Some(cache.plan_labels(&request, command.frames, command.label_font_px)?)
    } else {
        None
    };
    check_generation(cancellation, command.generation)?;

    let mut prioritized: Vec<(u64, u32)> = planned
        .plan
        .page_prio
        .iter()
        .copied()
        .zip(planned.plan.pages.iter().copied())
        .collect();
    prioritized.sort_unstable();
    let selected: Vec<u32> = prioritized
        .into_iter()
        .take(command.decode_pages.unwrap_or(usize::MAX))
        .map(|(_, page_id)| page_id)
        .collect();
    let raster_workers = command.jobs.unwrap_or(state.jobs);
    let decode_workers = command.decode_jobs.or(command.jobs).unwrap_or(state.jobs);
    let raster_request = GeometryRasterRequest {
        view: RasterViewBox::new(
            command.view[0],
            command.view[1],
            command.view[2],
            command.view[3],
        )?,
        width: command.width,
        height: command.height,
        background: [0, 0, 0, 255],
        foreground: [255, 255, 255, 255],
        workers: raster_workers,
        // §F2R-16: pixels are tile-size invariant (a pinned oracle), so
        // a reusing render drops to 64px tiles - at 384px almost no
        // tile sits fully inside the shifted overlap and the reuse
        // would be nominal.
        tile_size: if pan_reuse.is_some() {
            command.tile_size.min(64)
        } else {
            command.tile_size
        },
        // area-true drawing (GeometryRasterRequest::area_true): every frame
        // but an exact one, which keeps the KLayout rule
        area_true: !command.exact && area_true_enabled(),
        width_c: width_c(),
        survivor_list: std::env::var("FLOE_RUST_SURVIVOR_LIST").as_deref() != Ok("off"),
        place_lattice: std::env::var("FLOE_RUST_PLACE_LATTICE").as_deref() == Ok("on"),
        // the density stack sorts the area-true drawing: none without it
        density_stack: !command.exact && area_true_enabled() && density_stack_on(command),
        // the sub-cut dots' composition: a density shape claims what it lights
        density_claim_lit: density_dots_on(command),
        density_top_planes: 1,
        // pass 1's shapes first: every plane's density in the space they left
        density_shapes_first: density_shapes_first(),
        // the density's brightness by the area it covers (density_bright_gain)
        density_bright: if !command.exact && area_true_enabled() && density_stack_on(command) {
            density_bright_gain(command).unwrap_or(0.0) as f32
        } else {
            0.0
        },
    };
    let styles = if state.styles.is_empty() && (command.frames || command.labels) {
        cache
            .layers()
            .into_iter()
            .map(|layer| LayerStyle {
                layer_idx: layer.index,
                color: [255, 255, 255, 255],
                fill: LayerFill::Solid,
                outline_width: 1,
            })
            .collect()
    } else {
        state.styles.clone()
    };
    let mut plan = Arc::new(std::mem::replace(&mut planned.plan, cache.empty_plan().plan));
    let query_layers: Arc<[CacheLayer]> = Arc::from(cache.layers());
    let mut query_cell_names = BTreeMap::new();
    for cell in &plan.wcells {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            query_cell_names.entry(cell.key.0)
        {
            entry.insert(cache.cell_name(cell.key.0)?);
        }
    }
    let query_cell_names = Arc::new(query_cell_names);
    let labels: Arc<[floe_render_core::RenderLabel]> = planned_labels
        .as_ref()
        .map(|planned| Arc::from(planned.rows.clone()))
        .unwrap_or_else(|| Arc::from([]));
    // the density stack's pass 2 (floe_render_core DensityStack,
    // CUT_DENSITY_DESIGN §10.10): the same view planned with the finer cut
    // (density_cut_px); its pages are read only where pass 1 left room, in
    // render_density_frame. A reused frame, an occupancy frame and a probe
    // have no pass 2, nor a frame without the write-once tiles.
    // the density stack's pass 2 (floe_render_core DensityStack,
    // CUT_DENSITY_DESIGN §10.10): planned at the block boundary, over the
    // space pass 1 left (render_density_frame). A reused frame, an occupancy
    // frame and a probe have no pass 2, nor a frame without the write-once
    // tiles.
    let density_plan: Option<()> = (raster_request.density_stack
        && !label_only
        && !(styles.is_empty() && !command.frames)
        && command.probe.is_none()
        && std::env::var("FLOE_RUST_WRITE_ONCE").as_deref() != Ok("off"))
    .then_some(());
    // the density alone (density_only, diagnostic): pass 1 reads no page and
    // draws no shape - its plan keeps the hierarchy only - so pass 2 has the
    // whole budget
    let selected = if density_plan.is_some() && density_only() {
        let pass1 = Arc::make_mut(&mut plan);
        for cell in &mut pass1.wcells {
            cell.pages.clear();
            cell.page_levels.clear();
            cell.washes.clear();
            cell.dot_counts.clear();
            cell.reps.clear();
        }
        pass1.pages.clear();
        pass1.page_prio.clear();
        Vec::new()
    } else {
        selected
    };
    // pass 2 plans by plane, so its planes are the VISIBLE layers only (the
    // style list holds every layer; pass 1 leaves the plan to pick the pages
    // - field 2026-09-27: with one layer on, the others' density showed and
    // the top plane was the style list's last layer, not the visible one)
    let density_layers: Option<BTreeSet<u32>> = match (density_plan, command.visible_layers.as_deref()) {
        (Some(()), Some(specs)) => {
            let cache_layers = cache.layers();
            Some(
                specs
                    .iter()
                    .filter_map(|spec| resolve_layer(spec, &cache_layers).map(|layer| layer.index))
                    .collect(),
            )
        }
        _ => None,
    };
    if let Some(mode) = command.probe {
        // docs/LAYER_DECODE_PROBE_PLAN.ko.md: the same plan and selection, a
        // different way of painting them. Everything a normal render does
        // around the frame - refinement rounds, published scene, retained
        // frame, pan reuse - is deliberately skipped.
        return run_layer_probe(
            run_started,
            queue_us,
            &mut state.page_cache,
            cache,
            &command,
            responses,
            cancellation,
            mode,
            Arc::clone(&plan),
            &selected,
            &styles,
            raster_request,
            Arc::clone(&labels),
            &summary,
            decode_workers,
        );
    }
    // pass 2 is planned once a frame: a density frame takes its pages in one round
    let round_pages = if density_plan.is_some() { usize::MAX } else { command.round_pages };
    let mut rounds = refinement_batches(&selected, round_pages, |page_id| {
        state.page_cache.contains(page_id)
    })?;
    let mut decoded_pages = Vec::with_capacity(selected.len());
    let mut generation_bytes = 0u64;
    // the same pages as the parser read them (DecodedPage::grown_charge):
    // what the density stack counts pass 1 by (density_as_read)
    let mut generation_grown = 0u64;
    let mut round_index = 0usize;
    let mut drain_representatives = false;
    // the budget's last resort (budget_refit_enabled): the frame draws the
    // pages the budget holds and reports the rest as over it
    let mut cut_short = false;
    while round_index < rounds.len() {
        if round_index > 0 && planned.representative_stream.is_some() {
            // COW preserves the previously published query scene. Resume the
            // saved spatial cursor; a budget never silently drops the tail.
            std::mem::swap(&mut planned.plan, Arc::make_mut(&mut plan));
            loop {
                planned.advance_representatives(|| cancellation.is_cancelled(command.generation))?;
                if !drain_representatives || planned.representative_stream.is_none() { break; }
            }
            std::mem::swap(&mut planned.plan, Arc::make_mut(&mut plan));
        }
        if round_index + 1 == rounds.len() && planned.representative_stream.is_some() {
            rounds.push(Vec::new());
        }
        let round_page_ids = std::mem::take(&mut rounds[round_index]);
        check_generation(cancellation, command.generation)?;
        let (mut round_pages, decode_stats) = state.page_cache.load_cancellable(
            cache,
            &round_page_ids,
            decode_workers,
            command.generation,
            cancellation,
        )?;
        let round_bytes = round_pages.iter().try_fold(0u64, |total, page| {
            total
                .checked_add(page.estimated_bytes())
                .ok_or_else(|| "decoded generation byte charge overflow".to_string())
        })?;
        generation_bytes = match checked_generation_bytes(generation_bytes, round_bytes, state.page_cache.budget_bytes()) {
            Ok(bytes) => bytes,
            // the pages read so far pass the budget: their charge over their
            // estimate is what the planner's fit was short by - remembered
            // for these layers, and the frame planned anew (budget_refit_enabled)
            Err(error) => {
                // (an exact frame is every page or none; a frame the planner
                // does not fit - no budget, no cut, or its fit switched off,
                // FLOE_RUST_FIT_BUDGET=off - is as it was: an export at cut 0
                // must not come out short of pages)
                let fitted = request.decode_budget > 0 && request.cut_dbu > 0 && std::env::var("FLOE_RUST_FIT_BUDGET").as_deref() != Ok("off");
                if !budget_refit_enabled() || command.exact || !fitted {
                    return Err(error);
                }
                if !plan.stats.fit_over {
                    // (a plan over by its own estimate, `STILL OVER`, says
                    // nothing of the pages' charge)
                    let read: Vec<u32> = decoded_pages.iter().chain(round_pages.iter()).map(|page| page.page_id).collect();
                    let estimate = cache.pages_memory(&read).max(1);
                    let charged = generation_bytes.saturating_add(round_bytes);
                    let short = charged as f64 / estimate as f64;
                    // (at least 5 % past the scale it was planned under: a next
                    // plan is a smaller one)
                    let scale = (short * 1.03).max(budget_scale.unwrap_or(1.0) * 1.05);
                    eprintln!(
                        "[renderd] gen {}: {} pages charged {} bytes for {} estimated ({short:.3}x) pass the budget {}: the layers are planned at 1/{scale:.3} of it",
                        command.generation,
                        read.len(),
                        charged,
                        estimate,
                        state.page_cache.budget_bytes(),
                    );
                    state.budget_scale.insert(scale_key.clone(), scale);
                    // the fits decided under the budget as it was go with it
                    forget_fits(&mut state.fit_memory, &mut state.fit_whole, command);
                }
                if command.background {
                    // the viewer's margin: its viewport is on the screen as
                    // planned under the budget as it was, and a margin
                    // planned under less - or cut short - would change the
                    // picture when it lands. Dropped, as one its scale's fit
                    // does not hold; the next viewport frame decides anew
                    // over its margin's extent
                    respond(responses, format!("dropped gen={} reason=budget", command.generation));
                    return Ok(());
                }
                if plan.stats.fit_over || refits >= BUDGET_REFITS {
                    // no plan under less of the budget holds either - the
                    // fit's reach is spent (its plan was over by its own
                    // estimate, `STILL OVER`): the pages the budget holds are
                    // drawn, nearest the view's centre first as they were
                    // read, and the rest are the frame's pages over the
                    // budget (`deferred`, the viewer's `N pages over budget
                    // (not drawn)`)
                    let budget = state.page_cache.budget_bytes();
                    let mut held = generation_bytes;
                    let asked = round_pages.len();
                    round_pages.retain(|page| {
                        let next = held.saturating_add(page.estimated_bytes());
                        let fits = next <= budget;
                        if fits {
                            held = next;
                        }
                        fits
                    });
                    eprintln!(
                        "[renderd] gen {}: the pages pass the budget {} after {} plans: {} of this round's {} pages drawn, the rest left out",
                        command.generation,
                        budget,
                        refits + 1,
                        round_pages.len(),
                        asked,
                    );
                    rounds.truncate(round_index + 1);
                    cut_short = true;
                    held
                } else {
                    return Err(REFIT_BUDGET.to_string());
                }
            }
        };
        generation_grown = round_pages.iter().fold(generation_grown, |total, page| total.saturating_add(page.grown_charge()));
        decoded_pages.append(&mut round_pages);
        check_generation(cancellation, command.generation)?;
        let scene_started = Instant::now();
        let mut scene = FrameScene::new_shared_with_labels(
            cache,
            Arc::clone(&plan),
            decoded_pages.clone(),
            Arc::clone(&labels),
            command.label_font_px,
        )?;
        if summary.is_active() {
            scene.set_summaries(summary.planes.clone());
        }
        let scene = Arc::new(scene);
        let scene_us = elapsed_us(scene_started);
        check_generation(cancellation, command.generation)?;

        let mut density_pages: Option<[u64; 4]> = None;
        let mut density_us: Option<[u64; 6]> = None;
        let mut density_dots: Option<[u64; 2]> = None;
        let mut density_floor: Option<f64> = None;
        let mut density_plan2: Option<[u64; 39]> = None;
        let mut pixels = {
            let report = if styles.is_empty() && !command.frames {
                render_geometry_occupancy_cancellable(
                    &scene,
                    &raster_request,
                    command.generation,
                    cancellation,
                )?
            } else {
                let styled = StyledGeometryRasterRequest {
                    raster: raster_request,
                    layers: styles.clone(),
                    hierarchy_frames: command.frames,
                    mono: command.mono,
                };
                // FLOE_RUST_WORK_BIN=off: field kill switch back to the
                // per-tile walk (identical pixels, F2R-03b 2c).
                if density_plan.is_some() {
                    let density_styled = match &density_layers {
                        Some(visible) => StyledGeometryRasterRequest {
                            layers: styled.layers.iter().filter(|layer| visible.contains(&layer.layer_idx)).copied().collect(),
                            ..styled.clone()
                        },
                        None => styled.clone(),
                    };
                    let density_styled = if density_top_held() { density_held_top(cache, plan.top, density_styled) } else { density_styled };
                    // every visible datatype of the topmost layer number a top plane
                    let density_styled = density_top_group(cache, density_styled);
                    // the sub-cut dots show pass 1 first (CUT_DENSITY_DESIGN §10.12
                    // step 3): a round of its own, final=0, then the frame with the
                    // density - never for a margin (it is not shown before it lands)
                    let progressive = density_dots_on(command) && density_progressive_enabled() && command.unique_round_paths && !command.background;
                    let mut publish_first = |frame: &floe_render_core::RgbaFrame| -> Result<(), String> {
                        check_generation(cancellation, command.generation)?;
                        let format = if command.raw_frame { "raw" } else { "png" };
                        let path = format!("{}.gen-{}.round-1.partial.{}", command.out, command.generation, format);
                        let (header, png);
                        let parts: Vec<&[u8]> = if command.raw_frame {
                            header = raw_frame_header(frame.width(), frame.height());
                            vec![header.as_slice(), frame.pixels()]
                        } else {
                            png = frame.png_bytes()?;
                            vec![png.as_slice()]
                        };
                        publish_frame(&path, command.generation, &parts, cancellation)?;
                        respond(responses, format!("frame gen={} round=1 final=0 png={} format={} partial=1 deferred=1 density_round=1", command.generation, path, format));
                        Ok(())
                    };
                    let (report, counts, times, floor, plan2) = match render_density_frame(
                        cache,
                        &mut state.page_cache,
                        command,
                        cancellation,
                        &summary,
                        &scene,
                        &density_styled,
                        &plan,
                        &decoded_pages,
                        decode_workers,
                        if density_as_read() { &mut generation_grown } else { &mut generation_bytes },
                        &fit_key,
                        &mut state.density_fit_memory,
                        &mut state.density_floor_memory,
                        &mut state.density_whole,
                        command.background,
                        if progressive { Some(&mut publish_first) } else { None },
                    ) {
                        Ok(rendered) => rendered,
                        Err(error) if error == DROPPED_FIT => {
                            // the margin's pass 2 would thin otherwise than the
                            // viewport's: dropped, as a margin's pass-1 refit is
                            respond(responses, format!("dropped gen={} reason=fit", command.generation));
                            return Ok(());
                        }
                        Err(error) => return Err(error),
                    };
                    density_pages = Some([counts[0], counts[1], counts[2], counts[3]]);
                    density_floor = floor;
                    density_plan2 = Some(plan2);
                    density_dots = density_dots_on(command).then_some([counts[4], counts[5]]);
                    // both plans / the scenes / the collection / the regions / the decode
                    density_us = Some([times[0], times[1], report.stats.density_collect_us, times[2], times[3], report.stats.density_raster_us]);
                    report
                } else if std::env::var("FLOE_RUST_WORK_BIN").as_deref() == Ok("off") {
                    render_geometry_styled_unbinned_cancellable(
                        &scene,
                        &styled,
                        command.generation,
                        cancellation,
                    )?
                } else {
                    // §F2R-16: a snapped pan reuses the previous
                    // geometry frame's overlap; only the final round
                    // keeps its own geometry for the next pan - and
                    // only when retention is on at all (§F2R-20
                    // review: frame_cache=0 must neither clone nor
                    // retain, decided BEFORE the raster runs).
                    let is_final = round_index + 1 == rounds.len();
                    let keep_geometry = is_final && retention_enabled(command);
                    render_geometry_styled_cancellable_reuse(
                        &scene,
                        &styled,
                        command.generation,
                        cancellation,
                        if is_final { pan_reuse.as_ref() } else { None },
                        keep_geometry,
                    )?
                }
            };
            check_generation(cancellation, command.generation)?;
            let mut report = report;
            let geometry = report.geometry_frame.take();
            let geometry_is_frame = report.geometry_is_frame;
            let png_started = Instant::now();
            let png = if command.raw_frame {
                None
            } else {
                Some(report.frame.png_bytes()?)
            };
            let png_us = elapsed_us(png_started);
            // a PNG job keeps its frame only when it is the retained
            // geometry; the raw path publishes from it either way
            let frame = (command.raw_frame || geometry_is_frame).then_some(report.frame);
            FramePixels {
                png,
                frame,
                geometry,
                geometry_is_frame,
                // §F2R-18: the payload cache is retired; the wire
                // field stays 0 for adapter compatibility.
                frame_cache_hit: false,
                raster_us: report.stats.raster_us,
                raster_tile_max_us: report.stats.raster_tile_max_us,
                work_bin_items: report.stats.work_bin_items,
                work_bin_overflow_items: report.stats.work_bin_overflow_items,
                work_bin_defer_rep: report.stats.work_bin_defer_rep,
                work_bin_defer_single: report.stats.work_bin_defer_single,
                work_bin_defer_weight_max: report
                    .stats
                    .work_bin_defer_weight_max,
                tiles_reused: report.stats.tiles_reused,
                png_us,
                workers_used: report.stats.workers_used,
                tiles: report.stats.tiles,
                partial: report.partial,
                labels_truncated: report.labels_truncated,
                rectangle_member_paints: report.rectangle_member_paints,
                polygon_member_paints: report.polygon_member_paints,
                path_member_paints: report.path_member_paints,
                frame_member_paints: report.frame_member_paints,
                label_tile_paints: report.label_tile_paints,
                label_pixel_paints: report.label_pixel_paints,
                rep_members_tested: report.stats.rep_members_tested,
                rep_members_drawn: report.stats.rep_members_drawn,
                representative_spans: report.stats.representative_spans,
                representative_pixels: report.stats.representative_pixels,
                hier_cells_visited: report.stats.hier_cells_visited,
                subtrees_pruned: report.stats.subtrees_pruned,
                once_full_tiles: report.stats.once_full_tiles,
                once_passes_skipped: report.stats.once_passes_skipped,
                once_items_skipped: report.stats.once_items_skipped,
                summary_cells: report.summary_cell_paints,
                summary_pixels: report.summary_pixel_paints,
                place_walks: report.stats.place_walks,
                // the stack lives in the styled write-once tiles
                density_stack: density_plan.is_some().then_some(report.stats.density_stack),
                density_pages,
                density_us,
                density_bin: density_plan.is_some().then_some(report.stats.density_bin),
                density_dots,
                density_floor,
                density_plan2,
            }
        };
        check_generation(cancellation, command.generation)?;
        let final_round = round_index + 1 == rounds.len();
        let published_output = if command.unique_round_paths && !final_round {
            format!(
                "{}.gen-{}.round-{}.partial.{}",
                command.out,
                command.generation,
                round_index + 1,
                if command.raw_frame { "raw" } else { "png" }
            )
        } else {
            command.out.clone()
        };
        let raw_header = pixels
            .frame
            .as_ref()
            .filter(|_| command.raw_frame)
            .map(|frame| raw_frame_header(frame.width(), frame.height()));
        let parts: Vec<&[u8]> = match (&raw_header, &pixels.frame, &pixels.png) {
            (Some(header), Some(frame), _) => vec![header.as_slice(), frame.pixels()],
            (None, _, Some(png)) => vec![png.as_slice()],
            _ => return Err("frame has neither raw pixels nor PNG bytes".to_string()),
        };
        let publish_stats = publish_frame(
            &published_output,
            command.generation,
            &parts,
            cancellation,
        )?;
        // A successful rename is the generation's linearization point. A
        // later cancellation must not turn an already-published frame into a
        // cancelled response or leave a reported-less partial file behind.
        // a full-cover label re-synthesis carries no working set: the
        // covering render's published scene spans this view and stays
        if !label_only {
            let mut published = published_scene
                .write()
                .map_err(|_| "published scene lock poisoned".to_string())?;
            *published = Some(Arc::new(PublishedScene {
                scene: Arc::clone(&scene),
                layers: Arc::clone(&query_layers),
                cell_names: Arc::clone(&query_cell_names),
                key: RetainedKey::with_summary(command, state.style_epoch, summary_key.clone()),
                view: command.view,
            }));
        }
        if final_round {
            // §F2R-20: the label-free copy when labels painted, else
            // the published frame itself (no copy was made)
            let geometry = pixels.geometry.take().or_else(|| {
                if pixels.geometry_is_frame {
                    pixels.frame.take()
                } else {
                    None
                }
            });
            // (a frame cut short at the budget serves no other: its pages
            // are not its plan's)
            if let Some(frame) = geometry.filter(|_| !cut_short) {
                store_retained(
                    &mut state.retained,
                    RetainedFrame {
                        key: RetainedKey::with_summary(command, state.style_epoch, summary_key.clone()),
                        view: command.view,
                        frame,
                        fit: frame_fit,
                    },
                    retained_budget_bytes(),
                );
            }
        }
        // the published frame's buffers are done: free them before the
        // status line so a large margin does not linger past its use
        pixels.frame = None;
        pixels.png = None;

        respond(
            responses,
            format!(
                "frame gen={} round={} final={} png={} format={} partial={} deferred={} frame_cache_hit={} style_epoch={} plan_us={} text_plan_us={} labels={} labels_truncated={} text_place_records={} read_us={} decode_us={} decode_sum_us={} decode_max_us={} index_us={} decode_workers={} scene_us={} mask_bytes={} raster_us={} raster_tile_max_us={} tiles_reused={} bin_items={} bin_overflow={} bin_defer_rep={} bin_defer_single={} bin_defer_wmax={} png_us={} publish_write_us={} publish_sync_us={} publish_rename_us={} workers={} tiles={} tile_px={} pages={} plan_pages={} cache_hit={} cache_miss={} cache_evict={} resident_bytes={} wc_cells={} inst_edges={} frame_rects={} rect_paints={} polygon_paints={} path_paints={} frame_paints={} label_tile_paints={} label_pixel_paints={} rep_tested={} rep_drawn={} hier_cells={} subtree_prunes={} retained_bytes={} cull_pages={} cull_pbvh={} cull_cbvh={} cull_children={} cull_layer={} washed={} lod_swapped={} thin_frames={} thin_pages={} sub_cut_washes={} sub_cut_sparse={} sub_cut_sparse_over={} sub_cut_wash_over={} rep_kept={} rep_washed={} rep_children={} rep_page_level={} rep_level={} fit_pct={} fit_cull={} fit_over={} fit_thin={} fit_full_pct={} fit_none_pct={} fit_fixed={} fit_redecided={} sub_cut_boxes={} sub_cut_box_over={} sub_cut_box_level={} sub_cut_box_unsure={} shape_cut={} shape_cut_max={} summary_layers={} summary_cells={} summary_pixels={} summary_level={} summary_cell_um={} summary_none={} summary_pages={} stored_rep_points={} stored_rep_tested={} stored_rep_limited={} stored_rep_nodes={} stored_rep_proxies={} stored_rep_bytes={} stored_rep_pixels={} stored_rep_spans={} stored_rep_painted_pixels={} once_tiles={} once_passes={} once_items={} place_walks={} density_stack={} density_pages={} density_us={} density_bin={} density_dots={} density_floor={} density_block={} density_plan2={} queue_us={} wall_us={} fit_scale={} fit_refits={}",
                command.generation,
                round_index + 1,
                final_round as u8,
                published_output,
                if command.raw_frame { "raw" } else { "png" },
                (pixels.partial || planned.representative_stream.is_some()) as u8,
                scene.deferred_pages().len(),
                pixels.frame_cache_hit as u8,
                state
                    .style_epoch
                    .map(|epoch| epoch.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                planned.stats.plan_us,
                planned_labels.as_ref().map(|p| p.plan_us).unwrap_or(0),
                labels.len(),
                (pixels.labels_truncated
                    || planned_labels
                        .as_ref()
                        .is_some_and(|p| p.stats.truncated)) as u8,
                planned_labels
                    .as_ref()
                    .map(|p| p.stats.place_records_scanned)
                    .unwrap_or(0),
                decode_stats.page_read_us,
                decode_stats.page_decode_us,
                decode_stats.page_decode_sum_us,
                decode_stats.page_decode_max_us,
                decode_stats.page_index_us,
                decode_stats.decode_workers_used,
                scene_us,
                scene.mask_bytes(),
                pixels.raster_us,
                pixels.raster_tile_max_us,
                pixels.tiles_reused,
                pixels.work_bin_items,
                pixels.work_bin_overflow_items,
                pixels.work_bin_defer_rep,
                pixels.work_bin_defer_single,
                pixels.work_bin_defer_weight_max,
                pixels.png_us,
                publish_stats.write_us,
                publish_stats.sync_us,
                publish_stats.rename_us,
                pixels.workers_used,
                pixels.tiles,
                command.tile_size,
                scene.available_pages(),
                planned.summary.pages,
                decode_stats.decoded_cache_hit,
                decode_stats.decoded_cache_miss,
                decode_stats.decoded_cache_evicted,
                state.page_cache.resident_bytes(),
                planned.summary.wc_cells,
                planned.summary.inst_edges,
                planned.summary.frame_rects,
                pixels.rectangle_member_paints,
                pixels.polygon_member_paints,
                pixels.path_member_paints,
                pixels.frame_member_paints,
                pixels.label_tile_paints,
                pixels.label_pixel_paints,
                pixels.rep_members_tested,
                pixels.rep_members_drawn,
                pixels.hier_cells_visited,
                pixels.subtrees_pruned,
                retained_bytes(&state.retained),
                planned.summary.culls.pages_size,
                planned.summary.culls.page_bvh,
                planned.summary.culls.child_bvh,
                planned.summary.culls.children_size,
                planned.summary.culls.layer,
                planned.summary.culls.washed,
                planned.summary.culls.lod_swapped,
                planned.summary.culls.thin_frames,
                planned.summary.culls.thin_pages,
                planned.summary.culls.sub_cut_washes,
                planned.summary.culls.sub_cut_sparse,
                planned.summary.culls.sub_cut_sparse_over,
                planned.summary.culls.sub_cut_wash_over,
                planned.summary.culls.rep_kept,
                planned.summary.culls.rep_washed,
                planned.summary.culls.rep_children,
                planned.summary.culls.rep_page_level,
                planned.summary.culls.rep_level,
                planned.summary.culls.fit_pct,
                planned.summary.culls.fit_cull,
                planned.summary.culls.fit_over,
                planned.summary.culls.fit_thin,
                planned.summary.culls.fit_full_pct,
                planned.summary.culls.fit_none_pct,
                planned.summary.culls.fit_fixed,
                planned.summary.culls.fit_redecided,
                planned.summary.culls.sub_cut_boxes,
                planned.summary.culls.sub_cut_box_over,
                planned.summary.culls.sub_cut_box_level,
                planned.summary.culls.sub_cut_box_unsure,
                planned.summary.culls.shape_cut,
                planned.summary.culls.shape_cut_max as u8,
                summary.planes.len(),
                pixels.summary_cells,
                pixels.summary_pixels,
                summary.level,
                summary.cell_um(),
                summary.none.unwrap_or("-"),
                planned.summary.culls.summary_pages,
                planned.summary.representative_points,
                planned.summary.representative_tested,
                planned.summary.representative_limited as u8,
                planned.summary.representative_nodes,
                planned.summary.representative_proxies,
                planned.summary.representative_bytes,
                planned.summary.representative_pixels,
                pixels.representative_spans,
                pixels.representative_pixels,
                pixels.once_full_tiles,
                pixels.once_passes_skipped,
                pixels.once_items_skipped,
                floe_render_core::place_walks_wire(&pixels.place_walks),
                // lit/top/lower/covered/claimed (DENSITY_STACK_COUNTS), `-`
                // when the frame did not stack its density
                pixels
                    .density_stack
                    .map_or_else(|| "-".to_string(), |counts| counts.map(|count| count.to_string()).join("/")),
                // planned/in_hand/decoded/over_budget, `-` without pass 2
                pixels
                    .density_pages
                    .map_or_else(|| "-".to_string(), |counts| counts.map(|count| count.to_string()).join("/")),
                // plan2/scene2/collect/regions/decode2 us, and the pass-2 bins'
                // items/deferred/overflow
                pixels
                    .density_us
                    .map_or_else(|| "-".to_string(), |counts| counts.map(|count| count.to_string()).join("/")),
                pixels
                    .density_bin
                    .map_or_else(|| "-".to_string(), |counts| counts.map(|count| count.to_string()).join("/")),
                // the sub-cut dots' items/over, `-` without them
                pixels
                    .density_dots
                    .map_or_else(|| "-".to_string(), |counts| counts.map(|count| count.to_string()).join("/")),
                pixels.density_floor.map_or_else(|| "-".to_string(), |floor| format!("{floor:.3}")),
                // the sub-cut dots' block (FLOE_RUST_DENSITY_BLOCK_PX), px
                // (64 px at most under the brightness: floe_vfs dot_block_px_of)
                if pixels.density_dots.is_some() {
                    format!("{}", floe_render_core::dot_block_px_of(floe_render_core::dot_block_px(), pixels.density_plan2.is_some_and(|plan2| plan2[34] > 0)))
                } else {
                    "-".to_string()
                },
                // pass 2's plans: probe_us/fit_us/probes/passes/regions/nodes/page_nodes/page_candidates/threads/reads/items
                pixels
                    .density_plan2
                    .map_or_else(|| "-".to_string(), |counts| counts.map(|count| count.to_string()).join("/")),
                queue_us,
                // up to this frame's response: the phases above account for
                // part of it, the rest is time no phase timer covers
                elapsed_us(run_started),
                // the layers' pages decode larger than estimated: the scale
                // the plan's budget was cut by, thousandths (0: none), and
                // the plans of this frame that passed the budget before it
                budget_scale.map_or(0, |scale| (scale * 1000.0).round() as u64),
                refits,
            ),
        );
        // Representative batches bound query work, not the number of full
        // scene rasters: publish one preview, then drain the resumable cursor
        // for the next scene. Otherwise tiny/IO-limited batches would paint an
        // ever-growing prefix repeatedly (quadratic refinement cost).
        if planned.representative_stream.is_some() {
            drain_representatives = true;
        }
        // Cost-aware refinement (F2R-09 REOPEN, §3.15): every round
        // re-rasterizes the whole accumulated scene, so on a large
        // cold view the intermediate frames themselves became the
        // draw time (measured rounds 5 -> ~3x a single draw). Once a
        // round's raster exceeds the budget the remaining batches
        // merge into one final round: the user keeps the frames that
        // were cheap enough to stream, and pays exactly one more
        // raster for the settled frame. The dual of the F2R-01 rule
        // that sub-500ms jobs get no refinement at all.
        if round_index + 1 < rounds.len() && pixels.raster_us > refinement_raster_budget_us() {
            collapse_refinement_tail(&mut rounds, round_index);
            // As for page refinement, pay at most one further expensive
            // raster. Queries still check cancellation at every node/batch.
            drain_representatives = true;
        }
        round_index += 1;
    }
    Ok(())
}

/// Merges every batch after `round_index` into one final round.
fn collapse_refinement_tail(rounds: &mut Vec<Vec<u32>>, round_index: usize) {
    let merged: Vec<u32> = rounds[round_index + 1..].concat();
    rounds.truncate(round_index + 1);
    rounds.push(merged);
}

/// FLOE_RUST_REFINE_BUDGET_US overrides the collapse threshold —
/// primarily so tests can force the collapse on a small fixture.
fn refinement_raster_budget_us() -> u64 {
    std::env::var("FLOE_RUST_REFINE_BUDGET_US")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(REFINEMENT_RASTER_BUDGET_US)
}

fn refinement_batches(
    selected: &[u32],
    round_pages: usize,
    mut is_cached: impl FnMut(u32) -> bool,
) -> Result<Vec<Vec<u32>>, String> {
    if round_pages == 0 {
        return Err("round_pages must be positive".to_string());
    }
    let mut cached = Vec::new();
    let mut missing = Vec::new();
    for &page_id in selected {
        if is_cached(page_id) {
            cached.push(page_id);
        } else {
            missing.push(page_id);
        }
    }

    let mut batches: Vec<Vec<u32>> = missing.chunks(round_pages).map(<[u32]>::to_vec).collect();
    // A tiny final chunk makes first paint barely earlier, then repeats the
    // entire raster and PNG for the accumulated scene. Coalesce a <=50% tail
    // with its predecessor; sample9's 146-page view becomes one 128+18 batch
    // instead of doing almost all work twice.
    if batches.len() >= 2
        && batches
            .last()
            .is_some_and(|tail| tail.len() <= round_pages / 2)
    {
        let tail = batches.pop().expect("checked non-empty refinement tail");
        batches
            .last_mut()
            .expect("checked refinement predecessor")
            .extend(tail);
    }
    if batches.is_empty() {
        // Empty plans still publish a background frame; an all-hit plan must
        // settle in one frame regardless of its page count.
        batches.push(cached);
    } else if !cached.is_empty() {
        // Resident pages cost no read/decode and belong in the first scene.
        // Only actual misses are eligible for progressive splitting.
        let mut first = cached;
        first.append(&mut batches[0]);
        batches[0] = first;
    }
    Ok(batches)
}

/// A frame whose pages' decoded charge passes the generation budget is
/// planned again under less of it, not failed (field 2026-10-05, 787 and 789
/// alone with the density off: "most frames fail with `decoded generation
/// budget exceeded: 1093017130 > 1073741824 bytes`; with every layer on
/// hardly ever"). The planner fits a frame's pages to the budget by an
/// estimate of their decoded size (floe_vfs page_memory: its record count
/// and stored bytes); pages that decode larger than it - with every layer
/// on the others' generous estimates covered them - passed the budget, and
/// the frame was an error. Now the charge of the pages read so far over
/// their estimate is remembered for the layer set (WorkerState::
/// budget_scale, 3 % added) and the frame is planned anew to the budget over
/// it - a fitted plan under what its pages really take, not the first one
/// cut short - up to BUDGET_REFITS times; every later frame of the layers
/// starts there. FLOE_RUST_BUDGET_REFIT=off is the kill switch: the error,
/// as 0.12.300.
fn budget_refit_enabled() -> bool {
    std::env::var("FLOE_RUST_BUDGET_REFIT").as_deref() != Ok("off")
}

/// budget_refit_enabled: the most a frame is planned anew
const BUDGET_REFITS: u32 = 4;

/// run_render_attempt's answer when its pages passed the budget and the
/// frame is to be planned anew (WorkerState::budget_scale raised)
const REFIT_BUDGET: &str = "refit:budget";

/// The key of WorkerState::budget_scale: what decides which pages a frame
/// reads - the layers, the depth and the root - whatever the view, and
/// whether the density stack is on: with it pass 1 plans to the budget less
/// pass 2's reserve (pass1_decode_budget), which holds what its pages take
/// past their estimates - a scale the plain frames of the layers needed
/// would only take pages from it.
fn budget_scale_key(command: &RenderCommand) -> String {
    format!("{:?}|{}|{:?}|{}", command.visible_layers, command.depth, command.root, density_stack_on(command) as u8)
}

/// The budget fits remembered for a command's layers, depth and root, at
/// every scale and cut (WorkerState::fit_memory, fit_whole; fit_memory_key),
/// are forgotten when their budget scale is raised (budget_refit_enabled):
/// they were decided under the budget as it was. A viewport frame a decision
/// still held for would keep it, and its margin - wider - be dropped under
/// it every time (the two layers of the synthetic MAIN01 1/10 two steps in
/// under 24 MB: the viewport drew, each margin was dropped). The next
/// viewport frame decides anew, over its margin's extent.
fn forget_fits(fits: &mut BTreeMap<String, floe_render_core::FixedFit>, whole: &mut BTreeSet<String>, command: &RenderCommand) {
    let (head, tail) = fit_memory_scope(command);
    fits.retain(|key, _| !(key.starts_with(&head) && key.ends_with(&tail)));
    whole.retain(|key| !(key.starts_with(&head) && key.ends_with(&tail)));
}

/// Pass 1's decode budget (pass1_decode_budget) over the layers' remembered
/// scale (WorkerState::budget_scale): what the planner fits the pages'
/// estimates to, so their charge stays within the budget.
fn scaled_decode_budget(budget: u64, command: &RenderCommand, scale: Option<f64>) -> u64 {
    let pass1 = pass1_decode_budget(budget, command);
    match scale {
        Some(scale) if scale > 1.0 && pass1 > 0 => ((pass1 as f64 / scale) as u64).max(1),
        _ => pass1,
    }
}

fn checked_generation_bytes(current: u64, incoming: u64, budget: u64) -> Result<u64, String> {
    let next = current
        .checked_add(incoming)
        .ok_or_else(|| "decoded generation byte charge overflow".to_string())?;
    if next > budget {
        return Err(format!(
            "decoded generation budget exceeded: {next} > {budget} bytes"
        ));
    }
    Ok(next)
}

/// §F2R-18: keep one retained frame per (render state, scale),
/// newest last, bounded at RETAINED_FRAMES - zoom round-trips find
/// their scale again while pans keep reusing the current one.
/// `outer` contains `inner` (world boxes), within float slack.
fn view_contains(outer: &[f64; 4], inner: &[f64; 4]) -> bool {
    let eps = 1e-9 * (inner[2] - inner[0]).abs().max((inner[3] - inner[1]).abs());
    outer[0] <= inner[0] + eps
        && outer[1] <= inner[1] + eps
        && outer[2] >= inner[2] - eps
        && outer[3] >= inner[3] - eps
}

/// §F2R-21 review (HIGH): whether the published query scene already
/// serves `command` - same render state and a view containing the
/// request - so a label-only render may leave it in place.
fn published_scene_serves(
    shared: &SharedPublishedScene,
    command: &RenderCommand,
    style_epoch: Option<u64>,
    summary: &SummaryKey,
) -> Result<bool, String> {
    let Some(published) = current_scene(shared)? else {
        return Ok(false);
    };
    Ok(published.key == RetainedKey::with_summary(command, style_epoch, summary.clone())
        && view_contains(&published.view, &command.view))
}

fn store_retained(retained: &mut Vec<RetainedFrame>, entry: RetainedFrame, budget_bytes: usize) {
    let same_scale = |a: &RetainedFrame| {
        let span = |frame: &RetainedFrame| {
            (
                (frame.view[2] - frame.view[0]) / f64::from(frame.frame.width()),
                (frame.view[3] - frame.view[1]) / f64::from(frame.frame.height()),
            )
        };
        let (ax, ay) = span(a);
        let (bx, by) = span(&entry);
        a.key == entry.key
            && (ax - bx).abs() <= bx.abs() * 1e-9
            && (ay - by).abs() <= by.abs() * 1e-9
    };
    // §F2R-21 review (MEDIUM): a same-state, same-scale frame that
    // already CONTAINS the new one - the margin around a viewport pan
    // - stays as it is: replacing it with the smaller frame ended the
    // pan series' full reuse after one step while the GUI, remembering
    // the margin, did not prefetch again. Same state means identical
    // pixels over the overlap, so nothing is lost by not storing.
    // ... and planned under the same budget fit (review 2026-09-28: a frame
    // under another decision stayed in place, so nothing at that scale was
    // ever reusable again and every revisit drew anew)
    if let Some(index) = retained
        .iter()
        .position(|candidate| same_scale(candidate) && candidate.fit == entry.fit && view_contains(&candidate.view, &entry.view))
    {
        // it just served this render: touch it to newest (LRU order)
        let kept = retained.remove(index);
        retained.push(kept);
        return;
    }
    retained.retain(|candidate| !same_scale(candidate));
    if entry.bytes() > budget_bytes {
        // §F2R-20: a frame that alone exceeds the byte budget is not
        // retained at all (its pan reuse is not worth the residency)
        return;
    }
    retained.push(entry);
    while retained.len() > RETAINED_FRAMES || retained_bytes(retained) > budget_bytes {
        retained.remove(0);
    }
}

/// §F2R-16/§F2R-17: when the request shares the retained frame's
/// render state and scale and sits on its 16-device-px grid (the fill
/// phase period), snap the request onto that exact grid - eliminating
/// client float drift - and hand back the overlap of the retained
/// geometry frame. Sizes may differ: a 2wx2h margin frame (§F2R-17)
/// serves viewport pans and a viewport frame seeds the margin ring.
/// Any mismatch falls back to the full raster;
/// FLOE_RUST_PAN_REUSE=off is the kill switch.
/// Whether this render takes part in geometry retention at all: the
/// FLOE_RUST_PAN_REUSE kill switch, exact (archival) renders, the
/// frame_cache flag (perf baselines turn it off for backend-neutral
/// timing) and a zero retained budget all switch BOTH the reuse lookup
/// and the keep/clone/store of the new frame off (§F2R-20 review).
fn retention_enabled(command: &RenderCommand) -> bool {
    std::env::var("FLOE_RUST_PAN_REUSE").as_deref() != Ok("off")
        && !command.exact
        && command.frame_cache
        && retained_budget_bytes() > 0
}

fn prepare_pan_reuse(
    state: &WorkerState,
    command: &mut RenderCommand,
    summary: &SummaryKey,
    fit: Option<floe_render_core::FixedFit>,
) -> Option<FrameReuse> {
    if !retention_enabled(command) {
        return None;
    }
    let key = RetainedKey::with_summary(command, state.style_epoch, summary.clone());
    // newest first, every frame of this render state and budget fit: the
    // first whose scale and grid the request meets serves (review 2026-09-28:
    // the newest frame alone was tried, so with A -> B -> A the frame at A
    // went unused although it was still retained)
    let (reuse, view) = state
        .retained
        .iter()
        .rev()
        .filter(|candidate| candidate.key == key && candidate.fit == fit)
        .find_map(|candidate| reuse_from(candidate, command))?;
    command.view = view;
    Some(reuse)
}

/// The overlap of `retained` this request can take, and the request's view
/// snapped onto the retained grid, when the scale and the 16 px grid agree.
fn reuse_from(retained: &RetainedFrame, command: &RenderCommand) -> Option<(FrameReuse, [f64; 4])> {
    let [ox0, oy0, ox1, oy1] = retained.view;
    let [nx0, ny0, nx1, ny1] = command.view;
    let rw = f64::from(retained.frame.width());
    let rh = f64::from(retained.frame.height());
    let width = f64::from(command.width);
    let height = f64::from(command.height);
    let span_rx = ox1 - ox0;
    let span_ry = oy1 - oy0;
    if span_rx <= 0.0 || span_ry <= 0.0 {
        return None;
    }
    let sppx = span_rx / rw;
    let sppy = span_ry / rh;
    // Same device scale on both axes (relative epsilon).
    let same_scale = |new_span: f64, pixels: f64, spp: f64| {
        (new_span - pixels * spp).abs() <= (pixels * spp).abs() * 1e-9
    };
    if !same_scale(nx1 - nx0, width, sppx) || !same_scale(ny1 - ny0, height, sppy) {
        return None;
    }
    // Columns anchor at x0 (row 0 is the TOP of the frame, world y1):
    // request column c is retained column c + kx, request ROW r is
    // retained row r + k_row with k_row derived from the TOP edges.
    // Deriving rows from y0 flips the sign on pans (and only cancels
    // out on symmetric margins) - the §3.24 vertical-pan corruption.
    let dx = (nx0 - ox0) / sppx;
    let d_row = (oy1 - ny1) / sppy;
    if (dx - dx.round()).abs() > 1e-6 || (d_row - d_row.round()).abs() > 1e-6 {
        return None;
    }
    let kx = dx.round() as i64;
    let ky = d_row.round() as i64;
    if kx % 16 != 0 || ky % 16 != 0 {
        return None;
    }
    let rw_px = i64::from(retained.frame.width());
    let rh_px = i64::from(retained.frame.height());
    let w = i64::from(command.width);
    let h = i64::from(command.height);
    // The stipple row phase carries the frame HEIGHT (row + height - 1),
    // so reuse is byte-exact only when the heights agree modulo the
    // 16px pattern period (margins differ by 2 x 16-multiples; a window
    // resize does not).
    if (h - rh_px) % 16 != 0 {
        return None;
    }
    // kx == ky == 0 with equal sizes is the exact revisit: with the
    // payload cache retired (§F2R-18) it reuses every tile here.
    // request pixel (x, y) = retained pixel (x + kx, y + ky).
    let valid_x0 = (-kx).max(0);
    let valid_y0 = (-ky).max(0);
    let valid_x1 = (rw_px - kx).min(w);
    let valid_y1 = (rh_px - ky).min(h);
    if valid_x0 >= valid_x1 || valid_y0 >= valid_y1 {
        return None;
    }
    // Snap the request onto the retained grid so world->pixel sampling
    // is the exact translate of the retained frame (x from the left
    // edge, y from the TOP edge).
    let vx0 = ox0 + kx as f64 * sppx;
    let vy1 = oy1 - ky as f64 * sppy;
    let view = [vx0, vy1 - height * sppy, vx0 + width * sppx, vy1];
    let mut pixels = vec![0u8; (w as usize) * (h as usize) * 4];
    let old = retained.frame.pixels();
    let src_row_bytes = (rw_px as usize) * 4;
    let dst_row_bytes = (w as usize) * 4;
    let len = ((valid_x1 - valid_x0) as usize) * 4;
    for y in valid_y0..valid_y1 {
        let src = ((y + ky) as usize) * src_row_bytes + ((valid_x0 + kx) as usize) * 4;
        let dst = (y as usize) * dst_row_bytes + (valid_x0 as usize) * 4;
        pixels[dst..dst + len].copy_from_slice(&old[src..src + len]);
    }
    let base = floe_render_core::RgbaFrame::from_pixels(command.width, command.height, pixels)
        .ok()?;
    Some((
        FrameReuse {
            base,
            valid: [
                valid_x0 as u32,
                valid_y0 as u32,
                valid_x1 as u32,
                valid_y1 as u32,
            ],
        },
        view,
    ))
}

/// A frame with the density stack's pass 2 (floe_render_core DensityStack,
/// CUT_DENSITY_DESIGN §10.10): pass 1 paints `scene` as always over tiles
/// that stay alive (LayerRasterSession); at the block boundary the tiles say
/// where density may still go (`BlockDemand::eligible_regions`: per tile the
/// box of the pixels the top plane's density may take, and the box of those
/// any other plane's may), and the same view is planned with the finer cut
/// (density_cut_px) over those regions only - the top plane's layer over its
/// regions, the other layers over theirs - so the planner culls what the
/// originals cover before anything is collected or read. The plans' pages
/// (pass 1's own are in hand) are decoded in the plan's order within
/// density_budget_bytes and the generation's remaining budget; pass 2 then
/// draws the records under pass 1's cut (`plan.stats.shape_cut`) from them.
/// No pan reuse and no retained geometry (as the layer probe).
/// (the report, [pages planned, in hand, decoded, over the budget],
/// [both plans, the scenes, the regions, the decode] in us)
#[allow(clippy::too_many_arguments)]
fn render_density_frame(
    cache: &Cache,
    page_cache: &mut DecodedPageCache,
    command: &RenderCommand,
    cancellation: &RenderCancellation,
    summary: &SummarySelection,
    scene: &FrameScene,
    styled: &StyledGeometryRasterRequest,
    plan: &HierPlan,
    decoded_pages: &[Arc<floe_render_core::DecodedPage>],
    decode_workers: u16,
    generation_bytes: &mut u64,
    fit_key: &str,
    density_memory: &mut BTreeMap<String, floe_render_core::FixedFit>,
    floor_memory: &mut BTreeMap<String, u8>,
    whole_memory: &mut BTreeSet<String>,
    background: bool,
    mut first_round: Option<&mut dyn FnMut(&floe_render_core::RgbaFrame) -> Result<(), String>>,
) -> Result<(floe_render_core::GeometryRasterReport, [u64; 6], [u64; 4], Option<f64>, [u64; 39]), String> {
    let work_bin = std::env::var("FLOE_RUST_WORK_BIN").as_deref() != Ok("off");
    let upper_cut = plan.stats.shape_cut.min(i64::MAX as u64) as i64;
    let session = LayerRasterSession::begin_with_density_cancellable(
        scene,
        styled,
        work_bin,
        Some(upper_cut),
        command.generation,
        cancellation,
    )?;
    let block = session.density_block();
    let budget_bytes = page_cache.budget_bytes();
    // pass 2's reserve: the fixed one or what pass 1 left (density_frame_reserve)
    let reserve_bytes = density_frame_reserve(budget_bytes, *generation_bytes);
    // the top planes (density_top_group): their layers plan as the top side
    let top_planes = (styled.raster.density_top_planes as usize).clamp(1, styled.layers.len().max(1));
    let top_layers: Vec<u32> = styled.layers[styled.layers.len().saturating_sub(top_planes)..].iter().map(|layer| layer.layer_idx).collect();
    // each layer's place from the top (0 the top plane): pass 2 decodes the
    // upper planes' pages first (density_top_first)
    let from_top: BTreeMap<u32, u16> = styled.layers.iter().rev().enumerate().map(|(at, layer)| (layer.layer_idx, at.min(u16::MAX as usize) as u16)).collect();
    // zoomed out past the viewer's fit view, the dots thin (density_zoom_gain);
    // under the brightness (density_bright_gain) they keep their cover
    let dot_gain = if density_bright_gain(command).is_some() { 1.0 } else { density_zoom_gain(cache, command, plan.top.0) };
    let other_layers: Vec<u32> = styled.layers.iter().take(styled.layers.len().saturating_sub(top_planes)).map(|layer| layer.layer_idx).collect();
    // pages planned/in hand/decoded/over the budget, dot items/over the cap
    let mut counts = [0u64; 6];
    // the records' cut pass 2 planned at (px), and the frame's scale
    let mut floor_px: Option<f64> = None;
    let px_per_dbu = {
        let [x0, y0, x1, y1] = command.view;
        (command.width as f64 / (x1 - x0)).min(command.height as f64 / (y1 - y0))
    };
    let mut times = [0u64; 4];
    // the plans' breakdown (RenderPixels::density_plan2)
    let mut plan2 = [0u64; 39];
    plan2[22] = reserve_bytes >> 20;
    // a sub-cut cell stands for the area its shapes cover, not its box
    // (floe_render_core Cache::cell_cover: design.ovb and a hierarchy summary)
    plan2[36] = u64::from(density_bright_gain(command).is_some() && cache.cell_cover().is_some());
    // the dots' gain past the fit view, in thousandths (density_zoom_gain)
    plan2[31] = (dot_gain * 1000.0).round() as u64;
    // the brightness's gain, in thousandths (density_bright_gain; 0: off)
    plan2[34] = density_bright_gain(command).map_or(0, |g| (g * 1000.0).round() as u64);
    // the dot block the plans take and merge by (64 px at most under the
    // brightness: floe_vfs dot_block_px_of)
    let dot_block = floe_render_core::dot_block_px_of(floe_render_core::dot_block_px(), plan2[34] > 0);
    // the cut pass 2's dots plans take their pages at, px: the density cut, or
    // pass 1's under the occupancy first (density_ovb_first)
    let page_cut_px = if density_ovb_first(cache, command) { command.cut_px } else { density_cut_px() };
    // pass 1's pages: pass 2 holds them already, so they cost its reserve
    // nothing (floe_vfs HierOpts::free_pages; user 2026-10-01: 37 pages of
    // pass 1's, 201 MB by estimate, failed the 0 px floor's probe of a 128 MB
    // reserve with nothing new to decode, and the view drew nothing under the
    // cut until a pan took them out of it)
    let held: Arc<[u32]> = {
        let mut pages: Vec<u32> = decoded_pages.iter().map(|page| page.page_id).collect();
        pages.sort_unstable();
        pages.dedup();
        Arc::from(pages)
    };
    let mut failed: Option<String> = None;
    let (report, _pool_us) = cache.with_decode_pool(
        decode_workers,
        Some((command.generation, cancellation)),
        |pool| {
            session.render_layered_cancellable_with(
                scene,
                styled,
                command.generation,
                cancellation,
                block,
                |_, demand| {
                    if !demand.density_block() {
                        return Ok(None);
                    }
                    // pass 1 is painted: shown first, while pass 2 plans, decodes
                    // and draws (FLOE_RUST_DENSITY_PROGRESSIVE)
                    if let Some(publish) = first_round.as_mut() {
                        publish(&demand.snapshot()?)?;
                    }
                    // between pass 2's steps a newer generation stops this one:
                    // its plans end at their next look, its decode is pooled
                    // under the guard, its passes check per tile
                    check_generation(cancellation, command.generation)?;
                    let regions_started = Instant::now();
                    // by cells (density_free_cells): the top plane's where any
                    // pixel is free, the others' where enough is to add to; else
                    // a tile's bounding box of its free pixels, by area
                    let cells = density_free_cells();
                    let by_cells = cells.and_then(|(cell, others_min)| {
                        let (top, top_free) = demand.eligible_cells(true, cell, 0.0)?;
                        let (others, others_free) = demand.eligible_cells(false, cell, others_min)?;
                        Some((top, others, top_free as f64, others_free as f64))
                    });
                    let cells = cells.filter(|_| by_cells.is_some());
                    let (regions_top, regions_others, top_free, others_free) = match by_cells {
                        Some(found) => found,
                        None => {
                            let (top, others) = (demand.eligible_regions(true), demand.eligible_regions(false));
                            let (top_area, others_area): (f64, f64) = (
                                top.iter().map(|b| (b.x1 - b.x0).max(0) as f64 * (b.y1 - b.y0).max(0) as f64).sum(),
                                others.iter().map(|b| (b.x1 - b.x0).max(0) as f64 * (b.y1 - b.y0).max(0) as f64).sum(),
                            );
                            (top, others, top_area, others_area)
                        }
                    };
                    times[2] += elapsed_us(regions_started);
                    if cells.is_some() {
                        // the free pixels of the cells planned: the top plane's,
                        // the others'
                        plan2[29] = top_free as u64;
                        plan2[30] = others_free as u64;
                    }
                    // the finer plans: the top plane's layer over its regions,
                    // the other layers over theirs: each side's plan is kept until
                    // the decode below is settled - a page it leaves out is drawn
                    // by its occupancy record instead (Cache::stand_in_pages) - and
                    // its scene made then
                    let mut side_plans: Vec<(usize, HierPlan)> = Vec::new();
                    // the pages to decode: the top plane's first (density_top_first),
                    // then by the plan's priority
                    let mut wanted: Vec<(u16, u64, u32)> = Vec::new();
                    let top_first = density_top_first();
                    // the sub-cut dots (FLOE_RUST_DENSITY_DOTS=on) plan both sides at
                    // once when the others' space is much of the top plane's: the walk
                    // that finds the dots is the same for every layer, and the top
                    // plane's regions hold the others' (it may also take what lower
                    // originals drew); the top plane's side reads that plan's top layer
                    // alone. When the originals left the others little (the top layer
                    // is sparse and they cover the rest - an all-layer view), a joint
                    // plan would count every layer over the whole top space for
                    // nothing: the two sides plan apart, as without the dots.
                    let dots = density_dots_on(command) && command.cut_px > 0.0;
                    let one_walk = dots && density_one_walk_enabled();
                    // by cells (density_free_cells) the sides plan apart: the top
                    // plane's layer over its cells, the others over theirs - a joint
                    // plan walked every layer over the whole top space, where the
                    // originals may have left the others little (a reviewer,
                    // 2026-10-03: an all-layer fit view at full depth)
                    let joint = dots && cells.is_none() && !regions_top.is_empty() && 2.0 * others_free >= top_free;
                    // the jobs: (side, regions, layers) - the top planes each on its own
                    // (a plan counts a sub-cut item for its topmost layer alone: the
                    // top planes planned as one lost the lower ones' dots), merged
                    // into the top side; the others in one plan
                    let jobs: Vec<(usize, _, Vec<u32>)> = if joint {
                        vec![(0, regions_top, styled.layers.iter().map(|layer| layer.layer_idx).collect::<Vec<u32>>())]
                    } else {
                        let mut jobs: Vec<(usize, _, Vec<u32>)> = top_layers.iter().rev().map(|&layer| (0, regions_top.clone(), vec![layer])).collect();
                        jobs.push((1, regions_others, other_layers.clone()));
                        jobs
                    };
                    let top_jobs = if joint { 1 } else { top_layers.len() };
                    let mut top_plans: Vec<HierPlan> = Vec::new();
                    for (side, regions, layers) in jobs {
                        if regions.is_empty() || layers.is_empty() {
                            continue;
                        }
                        plan2[4] += regions.len() as u64;
                        let plan_started = Instant::now();
                        let region_boxes = regions
                            .iter()
                            .map(|b| ViewBox::new(b.x0, b.y0, b.x1, b.y1))
                            .collect::<Result<Vec<_>, _>>()?;
                        let side_key = if side == 0 && top_jobs > 1 { format!("{fit_key}|density0|{}", layers[0]) } else { format!("{fit_key}|density{side}") };
                        let reserve = reserve_bytes;
                        // one plan of this side: the sub-cut dots' (the cells at pass 1's
                        // cut - a cell under it is a dot item, never walked into or
                        // decoded - and the pages at `floor` px) or the plain finer cut;
                        // with `fit` to the reserve under the scale's decision
                        // (a floor is a probe: abandoned once its pages pass the reserve)
                        let plan_at = |floor: Option<f64>, fit: bool| -> Result<HierPlan, String> {
                            let budget = if fit { reserve } else { 0 };
                            let mut fine = match floor {
                                Some(floor) => {
                                    let mut fine = make_plan_request_cut(cache, command, budget, command.cut_px)?;
                                    fine.sub_cut_dots = Some((floor / command.cut_px).clamp(0.0, 1.0));
                                    fine
                                }
                                None => make_plan_request_cut(cache, command, budget, density_cut_px())?,
                            };
                            fine.regions = region_boxes.clone();
                            fine.visible_indices = Some(layers.clone());
                            fine.free_pages = Some(Arc::clone(&held));
                            if fit {
                                fine.fixed_fit = density_memory.get(&side_key).copied();
                            } else {
                                fine.probe_limit = reserve;
                            }
                            let fine_pages = cache.page_plan_request(&fine, summary, !command.frames)?;
                            Ok(cache.plan_cancellable(&fine_pages, command.generation, cancellation)?.plan)
                        };
                        // the dots' pages go as low as the reserve holds (step 2 of
                        // CUT_DENSITY_DESIGN §10.12: a floor by the work, not a fixed
                        // 1 px): the lowest of DOT_PAGE_FLOORS whose pages fit, from the
                        // scale's remembered rung up - never down, so every frame at a
                        // scale reads the same pages; a margin that would have to go up
                        // is dropped. Past the floors, the density cut with the
                        // planner's budget fit (as without the dots).
                        // the one walk (density_one_walk_enabled): no probe, no fit
                        let mut floored = None;
                        if one_walk {
                            let walk_started = Instant::now();
                            let mut fine = make_plan_request_cut(cache, command, 0, command.cut_px)?;
                            fine.sub_cut_dots = Some(1.0);
                            fine.dot_records = Some((dot_record_floor_px() / command.cut_px).clamp(0.0, 1.0));
                            fine.regions = region_boxes.clone();
                            fine.visible_indices = Some(layers.clone());
                            let fine = cache.page_plan_request(&fine, summary, !command.frames)?;
                            floored = Some(cache.plan_cancellable(&fine, command.generation, cancellation)?.plan);
                            plan2[1] += elapsed_us(walk_started);
                            plan2[3] += 1;
                        }
                        // a floor's probe planned in bands on threads and merged
                        // (density_probe_threads; None: as one plan)
                        let probe_apart = |floor: f64| -> Result<Option<(HierPlan, u64)>, String> {
                            let threads = density_plan_threads(decode_workers).min(region_boxes.len());
                            if !density_probe_threads() || threads <= 1 {
                                return Ok(None);
                            }
                            let dealt: Vec<Vec<ViewBox>> = (0..threads).map(|t| region_boxes.iter().skip(t).step_by(threads).cloned().collect()).collect();
                            let plans = std::thread::scope(|scope| {
                                let handles: Vec<_> = dealt
                                    .iter()
                                    .map(|regions| {
                                        let (regions, layers, held) = (regions.clone(), layers.clone(), Arc::clone(&held));
                                        scope.spawn(move || -> Result<HierPlan, String> {
                                            let mut fine = make_plan_request_cut(cache, command, 0, command.cut_px)?;
                                            fine.sub_cut_dots = Some((floor / command.cut_px).clamp(0.0, 1.0));
                                            fine.regions = regions;
                                            fine.visible_indices = Some(layers);
                                            fine.free_pages = Some(held);
                                            fine.probe_limit = reserve;
                                            let fine = cache.page_plan_request(&fine, summary, !command.frames)?;
                                            Ok(cache.plan_cancellable(&fine, command.generation, cancellation)?.plan)
                                        })
                                    })
                                    .collect();
                                handles
                                    .into_iter()
                                    .map(|handle| handle.join().unwrap_or_else(|_| Err("pass 2 probe thread panicked".to_string())))
                                    .collect::<Result<Vec<HierPlan>, String>>()
                            })?;
                            let groups = plans.len() as u64;
                            // over: a band past the reserve, or the merge's pages
                            let over = plans.iter().any(|plan| plan.stats.fit_over);
                            let mut merged = floe_render_core::Cache::merge_plans(plans, dot_block / px_per_dbu);
                            merged.stats.fit_bytes = cache.plan_page_cost(&merged, &held);
                            merged.stats.fit_over = over;
                            Ok(Some((merged, groups)))
                        };
                        // (the occupancy first probes no floor: its pages are spread
                        // or decoded by their grids' cells, whatever fits)
                        let floors = if density_ovb_first(cache, command) { Vec::new() } else { dot_page_floors() };
                        if dots && !one_walk {
                            // the last viewport's rung: a margin must plan at it (it
                            // starts there and is dropped past it); a viewport probes
                            // from the lowest rung, as its own budget decides (user
                            // 2026-10-01: a probe that failed in a dense view kept every
                            // later view at the zoom step off the floor)
                            let known = floor_memory.get(&side_key).copied();
                            let first = if background { usize::from(known.unwrap_or(0)) } else { 0 };
                            for rung in first..floors.len() {
                                let probe_started = Instant::now();
                                let (plan, groups) = match probe_apart(floors[rung])? {
                                    Some(apart) => apart,
                                    None => (plan_at(Some(floors[rung]), false)?, 1),
                                };
                                plan2[0] += elapsed_us(probe_started);
                                plan2[2] += 1;
                                plan2[8] = plan2[8].max(groups);
                                let fits = !plan.stats.fit_over && plan.stats.fit_bytes <= reserve;
                                if !fits {
                                    plan2[11] += 1;
                                }
                                if !fits || known != Some(rung as u8) {
                                    if background && known.is_some() {
                                        return Err(DROPPED_FIT.to_string());
                                    }
                                }
                                if fits {
                                    floor_memory.insert(side_key.clone(), rung as u8);
                                    floored = Some(plan);
                                    break;
                                }
                            }
                        }
                        check_generation(cancellation, command.generation)?;
                        let planned_fine = match floored {
                            Some(plan) => plan,
                            None => {
                                if dots {
                                    floor_memory.insert(side_key.clone(), floors.len() as u8);
                                }
                                // pass 2 plans to a reserve of its own (density_budget_bytes;
                                // pass 1 plans to the rest) and its budget fit is remembered per
                                // scale and side as pass 1's is: the same pages whatever the frame
                                let fit_started = Instant::now();
                                // the regions planned apart on threads (density_plan_threads): as
                                // asked in bands, merged, and fitted to the reserve as the one plan
                                // would be (Cache::fit_plan: held whole, under the scale's decision,
                                // or decided anew; user 2026-10-01: the threads served views the
                                // reserve held whole only, and the slow ones thin). A fit that would
                                // plan again - a decision at a coarser cut, pages past its overshoot -
                                // plans as one.
                                let threads = density_plan_threads(decode_workers).min(region_boxes.len());
                                let apart = if dots && threads > 1 {
                                    // the regions dealt round robin (tile order): the view's work is
                                    // seldom even - on the synthetic chip's H01_00001 fit view one of
                                    // nine tiles held over half of it, and contiguous bands of two
                                    // threads planned in 806 ms against one plan's 785, dealt in 517
                                    let dealt: Vec<Vec<ViewBox>> = (0..threads).map(|t| region_boxes.iter().skip(t).step_by(threads).cloned().collect()).collect();
                                    let plans = std::thread::scope(|scope| {
                                        let handles: Vec<_> = dealt
                                            .iter()
                                            .map(|regions| {
                                                let (regions, layers) = (regions.clone(), layers.clone());
                                                scope.spawn(move || -> Result<HierPlan, String> {
                                                    let mut fine = make_plan_request_cut(cache, command, 0, command.cut_px)?;
                                                    fine.sub_cut_dots = Some((page_cut_px / command.cut_px).clamp(0.0, 1.0));
                                                    fine.regions = regions;
                                                    fine.visible_indices = Some(layers);
                                                    let fine = cache.page_plan_request(&fine, summary, !command.frames)?;
                                                    Ok(cache.plan_cancellable(&fine, command.generation, cancellation)?.plan)
                                                })
                                            })
                                            .collect();
                                        handles
                                            .into_iter()
                                            .map(|handle| handle.join().unwrap_or_else(|_| Err("pass 2 plan thread panicked".to_string())))
                                            .collect::<Result<Vec<HierPlan>, String>>()
                                    })?;
                                    let groups = plans.len() as u64;
                                    let merged = floe_render_core::Cache::merge_plans(plans, dot_block / px_per_dbu);
                                    let mut fit = make_plan_request_cut(cache, command, reserve, command.cut_px)?;
                                    fit.sub_cut_dots = Some((page_cut_px / command.cut_px).clamp(0.0, 1.0));
                                    fit.regions = region_boxes.clone();
                                    fit.visible_indices = Some(layers.clone());
                                    fit.free_pages = Some(Arc::clone(&held));
                                    fit.fixed_fit = density_memory.get(&side_key).copied();
                                    let fit = cache.page_plan_request(&fit, summary, !command.frames)?;
                                    cache.fit_plan(&fit, merged)?.map(|plan| (plan, groups))
                                } else {
                                    None
                                };
                                let planned_fine = match apart {
                                    Some((plan, groups)) => {
                                        plan2[8] = plan2[8].max(groups);
                                        plan
                                    }
                                    None => plan_at(dots.then_some(page_cut_px), true)?,
                                };
                                plan2[3] += u64::from(planned_fine.stats.fit_passes.max(1));
                                let whole = planned_fine.stats.fit_whole || planned_fine.stats.fit_decision.is_none();
                                if background && (planned_fine.stats.fit_redecided || (!whole && whole_memory.contains(&side_key))) {
                                    // the margin's pass 2 does not fit under the scale's decision,
                                    // or has to thin where the viewport's was held whole: drawn
                                    // otherwise it would change the picture when it lands
                                    return Err(DROPPED_FIT.to_string());
                                }
                                if let Some(decision) = planned_fine.stats.fit_decision {
                                    if planned_fine.stats.fit_redecided || !density_memory.contains_key(&side_key) {
                                        density_memory.insert(side_key.clone(), decision);
                                    }
                                }
                                plan2[1] += elapsed_us(fit_started);
                                planned_fine
                            }
                        };
                        if !background {
                            // whether this viewport's pass 2 kept every page as asked (a
                            // floor's probe and the bands' merge plan as asked): a margin
                            // that has to thin here is dropped
                            if planned_fine.stats.fit_whole || planned_fine.stats.fit_decision.is_none() {
                                whole_memory.insert(side_key.clone());
                            } else {
                                whole_memory.remove(&side_key);
                            }
                        }
                        if (planned_fine.stats.fit_decision.is_some() && !planned_fine.stats.fit_whole) || planned_fine.stats.fit_dropped {
                            // the budget fit dropped pages or raised the cut (or, at
                            // the asked cut, kept none that cost: fit_dropped)
                            plan2[12] += 1;
                        }
                        plan2[5] += planned_fine.stats.visited_bvh;
                        plan2[6] += planned_fine.stats.visited_page_bvh;
                        plan2[7] += planned_fine.stats.page_candidates;
                        plan2[8] = plan2[8].max(1);
                        plan2[9] += planned_fine.stats.sub_cut_box_reads;
                        plan2[10] += planned_fine.stats.sub_cut_dot_items;
                        for (at, count) in planned_fine.stats.dot_by.iter().enumerate() {
                            plan2[13 + at] += count;
                        }
                        // the pages placed by their occupancy grids (design.ovb)
                        plan2[23] += planned_fine.stats.dot_occ_pages;
                        // the pages under the floor decoded, their cells too coarse
                        plan2[24] += planned_fine.stats.dot_occ_decoded;
                        // the point-list chunks passed over in full blocks, their members
                        plan2[25] += planned_fine.stats.dot_full_chunks;
                        plan2[26] += planned_fine.stats.dot_full_members;
                        // the point-list chunks read at a step, their members
                        plan2[27] += planned_fine.stats.dot_sampled_chunks;
                        plan2[28] += planned_fine.stats.dot_sampled_members;
                        // the dot blocks too sparse to draw, the dots a block needed
                        // (HierOpts::dot_gate)
                        plan2[32] += planned_fine.stats.dot_gated;
                        plan2[33] = plan2[33].max(u64::from(planned_fine.stats.dot_gate_min));
                        counts[4] += planned_fine.stats.sub_cut_boxes;
                        counts[5] += planned_fine.stats.sub_cut_box_over;
                        // the records' cut this side planned at, px (the floor, or
                        // the density cut, or what a budget fit raised it to)
                        let side_floor = planned_fine.stats.shape_cut as f64 * px_per_dbu;
                        floor_px = Some(floor_px.map_or(side_floor, |known: f64| known.max(side_floor)));
                        let mut planned_fine = planned_fine;
                        thin_dot_items(&mut planned_fine, dot_gain);
                        if density_under_floor_drawn(cache) {
                            // a decoded page's shapes under the floor are drawn,
                            // by area, as the spread pages under it are
                            planned_fine.stats.shape_cut = 0;
                        }
                        if side == 0 && top_jobs > 1 {
                            // a top plane's plan: the top side is all of them, merged
                            top_plans.push(planned_fine);
                            if top_plans.len() < top_jobs {
                                times[0] += elapsed_us(plan_started);
                                continue;
                            }
                            planned_fine = floe_render_core::Cache::merge_plans(std::mem::take(&mut top_plans), dot_block / px_per_dbu);
                        }
                        // the pages the planner's own fit left out and their occupancy
                        // records stand in for (floe_vfs HierOpts::dot_stand_in)
                        plan2[35] += planned_fine.stats.dot_stood_in;
                        plan2[37] += planned_fine.stats.dot_cover_cells;
                        plan2[38] += planned_fine.stats.dot_node_sampled;
                        let density_plan = planned_fine;
                        times[0] += elapsed_us(plan_started);
                        counts[0] += density_plan.pages.len() as u64;
                        for (&page_id, &prio) in density_plan.pages.iter().zip(density_plan.page_prio.iter()) {
                            // pass 1's pages the finer plan holds too are in hand
                            if held.binary_search(&page_id).is_ok() {
                                counts[1] += 1;
                            } else if one_walk {
                                // the one walk reads no page: one pass 1 left
                                // out (its fit) stays out, counted as over
                                counts[3] += 1;
                            } else {
                                // the upper planes' pages first: the top plane's, then
                                // each lower plane's in the drawing order down
                                let rank = if top_first {
                                    cache.page_layer(page_id).and_then(|layer| from_top.get(&layer).copied()).unwrap_or(u16::MAX)
                                } else {
                                    0
                                };
                                wanted.push((rank, prio, page_id));
                            }
                        }
                        // (a joint plan is both sides': side 2 marks it)
                        side_plans.push((if joint { 2 } else { side }, density_plan));
                    }
                    // the plans are within the reserve by the planner's estimate; the
                    // decode takes them in their order (the priority) under the reserve
                    // as twice the encoded bytes, the generation check below the net
                    check_generation(cancellation, command.generation)?;
                    wanted.sort_unstable();
                    let mut seen = std::collections::HashSet::with_capacity(wanted.len());
                    wanted.retain(|entry| seen.insert(entry.2));
                    let limit = reserve_bytes;
                    let mut estimate = 0u64;
                    let mut take = Vec::with_capacity(wanted.len());
                    // the pages this budget leaves out after all
                    let mut left_out: Vec<u32> = Vec::new();
                    for (_, _, page_id) in wanted {
                        let bytes = cache.page_encoded_bytes(page_id).saturating_mul(2).max(1);
                        if estimate.saturating_add(bytes) > limit {
                            counts[3] += 1;
                            left_out.push(page_id);
                            continue;
                        }
                        estimate += bytes;
                        take.push(page_id);
                    }
                    let mut decoded_fine: Vec<Arc<floe_render_core::DecodedPage>> = Vec::new();
                    if !take.is_empty() {
                        let decode_started = Instant::now();
                        let (pages, _decode_stats) = page_cache.load_pooled(pool, &take)?;
                        times[3] += elapsed_us(decode_started);
                        for page in pages {
                            // (as the parser read it or as it is held: the
                            // count pass 1's pages came in by, density_as_read)
                            let bytes = if density_as_read() { page.grown_charge() } else { page.estimated_bytes() };
                            if generation_bytes.saturating_add(bytes) > budget_bytes {
                                counts[3] += 1;
                                left_out.push(page.page_id);
                                continue;
                            }
                            *generation_bytes += bytes;
                            counts[2] += 1;
                            decoded_fine.push(page);
                        }
                    }
                    // the sides' scenes: a page left out drawn by its occupancy
                    // record (floe_vfs HierOpts::dot_stand_in), pass 1's pages the
                    // finer plan holds too and those decoded here set
                    let scene_started = Instant::now();
                    left_out.sort_unstable();
                    left_out.dedup();
                    let stand_in_request = if left_out.is_empty() { None } else { Some(make_plan_request_cut(cache, command, 0, command.cut_px)?) };
                    let mut sides: [Option<Arc<FrameScene>>; 2] = [None, None];
                    let scene_of = |plan: Arc<HierPlan>, failed: &mut Option<String>| -> Result<Arc<FrameScene>, String> {
                        let scene = Arc::new(FrameScene::new_metadata(cache, plan, Arc::from([]), command.label_font_px)?);
                        for page in decoded_pages {
                            // pass 1's pages the finer plan holds too; the rest are not its
                            let _ = scene.set_decoded_page(Arc::clone(page));
                        }
                        for page in &decoded_fine {
                            // a page of the other side's plan is not this one's
                            if let Err(error) = scene.set_decoded_page(Arc::clone(page)) {
                                if !error.contains("outside the plan") && !error.contains("already in the scene") {
                                    failed.get_or_insert(error);
                                }
                            }
                        }
                        Ok(scene)
                    };
                    for (side, mut density_plan) in side_plans {
                        if let Some(request) = &stand_in_request {
                            let stood = cache.stand_in_pages(request, &mut density_plan, &left_out)?;
                            plan2[35] += stood;
                            // (pages placed by their occupancy records, as the others)
                            plan2[20] += stood;
                            plan2[23] += stood;
                        }
                        let density_plan = Arc::new(density_plan);
                        if side == 2 {
                            // the lower planes' walk reads the whole plan (its plane
                            // table leaves the top layer out), the top plane's side
                            // that layer's pages and dots (Cache::plan_layer_only)
                            if !top_layers.is_empty() {
                                let top_plan = Arc::new(cache.plan_layers_only(&density_plan, &top_layers));
                                sides[0] = Some(scene_of(top_plan, &mut failed)?);
                            }
                            sides[1] = Some(scene_of(density_plan, &mut failed)?);
                        } else {
                            sides[side] = Some(scene_of(density_plan, &mut failed)?);
                        }
                    }
                    times[1] += elapsed_us(scene_started);
                    let [top, others] = sides;
                    Ok(Some(floe_render_core::DensityScenes { top, others }))
                },
            )
        },
    )?;
    if let Some(error) = failed {
        return Err(error);
    }
    Ok((report?, counts, times, floor_px, plan2))
}

fn make_plan_request(cache: &Cache, command: &RenderCommand, decode_budget: u64) -> Result<PlanRequest, String> {
    make_plan_request_cut(cache, command, decode_budget, command.cut_px)
}

/// `make_plan_request` at another cut (px): the density stack's pass 2 plans
/// the same view with the finer cut (density_cut_px).
fn make_plan_request_cut(cache: &Cache, command: &RenderCommand, decode_budget: u64, cut_px: f64) -> Result<PlanRequest, String> {
    let [x0, y0, x1, y1] = command.view;
    let planner_x0 = checked_bound(x0.floor(), "view x0")?;
    let planner_y0 = checked_bound(y0.floor(), "view y0")?;
    let planner_x1 = checked_bound(x1.ceil(), "view x1")?;
    let planner_y1 = checked_bound(y1.ceil(), "view y1")?;
    let span_x = x1 - x0;
    let span_y = y1 - y0;
    let px_per_dbu = (command.width as f64 / span_x).min(command.height as f64 / span_y);
    let cut_dbu = if command.exact || cut_px == 0.0 {
        0
    } else {
        checked_bound((cut_px / px_per_dbu).ceil(), "cut dbu")?
    };
    let request = PlanRequest {
        view: ViewBox::new(planner_x0, planner_y0, planner_x1, planner_y1)?,
        cut_dbu,
        visible_layers: command.visible_layers.clone(),
        depth: command.depth,
        px_per_dbu,
        exact: command.exact,
        // the deck's sub-cut rules on a plain layout too (field
        // 2026-09-16: a 9.8 GB design layout showed far less than
        // Calibre at detail high - pages whose every shape is below
        // the cut were dropped whole; now a dense one is a footprint
        // wash and a sparse one is kept and drawn as pixels).
        // FLOE_RUST_SUB_CUT_WASH=off is the kill switch.
        sub_cut_wash: !command.exact && sub_cut_wash_enabled(),
        // the page frontier (user design 2026-09-17): what the cut
        // drops is thinned to representatives - one in 4^k of the
        // pages and placements k octaves below their cut, nested
        // across zooms - instead of vanishing. FLOE_RUST_PAGE_REPS=off
        // is the kill switch.
        page_reps: !command.exact && page_reps_enabled(),
        decode_budget,
        page_hairline: !command.thin_keep,
        summary_layers: Vec::new(),
        prune_summary: false,
        // sub-cut boxes (hier.rs SUB_CUT_BOX_PX): off unless
        // FLOE_RUST_SUB_CUT_BOX=on - the density representation below the
        // cut replaces them (see sub_cut_box_enabled)
        sub_cut_box: !command.exact && command.thin_keep && sub_cut_box_enabled(),
        shape_cut: !command.exact && command.thin_keep && shape_cut_mode() == ShapeCut::Smaller,
        shape_cut_max: !command.exact && command.thin_keep && shape_cut_mode() == ShapeCut::Larger,
        // the viewer's frames switch reaches the planner (review 2026-09-20: it
        // only reached the raster, so a frames-off view still planned - and
        // walked for - every depth-boundary outline)
        frames: command.frames,
        // the M7-C page wash (floe_vfs::ViewReq::page_wash): off unless
        // FLOE_RUST_PAGE_WASH=on (see page_wash_enabled); an exact frame
        // keeps what it had
        page_wash: command.exact || page_wash_enabled(),
        // the M7 LOD swap (floe_vfs::ViewReq::lod_swap): off unless
        // FLOE_RUST_LOD=on (see lod_enabled)
        lod_swap: lod_enabled(),
        regions: Vec::new(),
        visible_indices: None,
        fixed_fit: None,
        root: command.root,
        sub_cut_dots: None,
        dot_records: None,
        probe_limit: 0,
        free_pages: None,
        // a view whose visible layers no cell holds is an empty picture
        empty_top: true,
        // the dots' brightness by the detail (density_bright_gain)
        dot_bright: density_bright_gain(command),
        // the occupancy first (density_ovb_first): in a dots plan at the cells'
        // cut, a page with no grid to spread it by is decoded from the
        // density cut up, as before
        dot_occ_first: density_ovb_first(cache, command).then(|| (density_cut_px() / command.cut_px).clamp(0.0, 1.0)),
    };
    request.validate()?;
    if cache.unit() <= 0.0 {
        return Err("invalid cache unit".to_string());
    }
    Ok(request)
}

fn checked_bound(value: f64, name: &str) -> Result<i64, String> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(format!("coordinate overflow: {name} = {value}"));
    }
    Ok(value as i64)
}

fn check_generation(cancellation: &RenderCancellation, generation: u64) -> Result<(), String> {
    if cancellation.is_cancelled(generation) {
        Err("render cancelled".to_string())
    } else {
        Ok(())
    }
}

fn is_render_cancelled_error(error: &str) -> bool {
    error == "render cancelled" || error.starts_with("render cancelled:")
}

fn cancelled_response(responses: &Sender<String>, generation: u64, phase: &str) {
    respond(
        responses,
        format!("cancelled gen={generation} phase={phase}"),
    );
}

fn publish_bytes(
    output: &str,
    sequence: i64,
    generation: u64,
    bytes: &[u8],
    cancellation: &RenderCancellation,
) -> Result<(), String> {
    check_generation(cancellation, generation)?;
    let output = Path::new(output);
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid output path: {}", output.display()))?;
    let temporary = parent.join(format!(
        ".{name}.floe-renderd-{}-clip-{sequence}.tmp",
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("create {}: {}", temporary.display(), error))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("write {}: {}", temporary.display(), error));
    }
    drop(file);
    let published = cancellation.commit_if_current(generation, || {
        std::fs::rename(&temporary, output).map_err(|error| {
            format!(
                "publish {} -> {}: {}",
                temporary.display(),
                output.display(),
                error
            )
        })
    });
    match published {
        Ok(Some(())) => Ok(()),
        Ok(None) => {
            let _ = std::fs::remove_file(&temporary);
            Err("render cancelled".to_string())
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

struct PublishStats {
    write_us: u64,
    sync_us: u64,
    rename_us: u64,
}

/// Wire header of a raw published frame: magic, then width and height as
/// little-endian u32. The pixels follow as tightly packed RGBA rows.
const RAW_FRAME_MAGIC: &[u8; 8] = b"FLOERAW1";

fn raw_frame_header(width: u32, height: u32) -> [u8; 16] {
    let mut header = [0u8; 16];
    header[..8].copy_from_slice(RAW_FRAME_MAGIC);
    header[8..12].copy_from_slice(&width.to_le_bytes());
    header[12..16].copy_from_slice(&height.to_le_bytes());
    header
}

#[cfg(test)]
fn raw_frame_payload(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    debug_assert_eq!(pixels.len(), width as usize * height as usize * 4);
    let mut payload = raw_frame_header(width, height).to_vec();
    payload.extend_from_slice(pixels);
    payload
}

/// Writes `parts` back to back into a private temporary file and renames
/// it into place at the generation's linearization point. A raw frame
/// arrives as [header, pixels] so no concatenated copy is ever built.
fn publish_frame(
    output: &str,
    generation: u64,
    parts: &[&[u8]],
    cancellation: &RenderCancellation,
) -> Result<PublishStats, String> {
    check_generation(cancellation, generation)?;
    let output = Path::new(output);
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid output path: {}", output.display()))?;
    let temporary = parent.join(format!(
        ".{name}.floe-renderd-{}-{generation}.tmp",
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let write_started = Instant::now();
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("create {}: {}", temporary.display(), error))?;
    for part in parts {
        if let Err(error) = file.write_all(part) {
            let _ = std::fs::remove_file(&temporary);
            return Err(format!("write {}: {}", temporary.display(), error));
        }
    }
    let write_us = elapsed_us(write_started);
    let sync_started = Instant::now();
    if let Err(error) = file.sync_all() {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("sync {}: {}", temporary.display(), error));
    }
    let sync_us = elapsed_us(sync_started);
    let rename_started = Instant::now();
    let published = cancellation.commit_if_current(generation, || {
        std::fs::rename(&temporary, output).map_err(|error| {
            format!(
                "publish {} -> {}: {}",
                temporary.display(),
                output.display(),
                error
            )
        })
    });
    let rename_us = elapsed_us(rename_started);
    match published {
        Ok(Some(())) => Ok(PublishStats {
            write_us,
            sync_us,
            rename_us,
        }),
        Ok(None) => {
            let _ = std::fs::remove_file(&temporary);
            Err("render cancelled".to_string())
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn load_styles(path: &str, layers: &[CacheLayer]) -> Result<Vec<LayerStyle>, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("read {path}: {error}"))?;
    parse_styles(&text, layers)
}

fn parse_styles(text: &str, layers: &[CacheLayer]) -> Result<Vec<LayerStyle>, String> {
    let mut styles = Vec::new();
    let mut seen = BTreeSet::new();
    for (line_index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() != 4 {
            return Err(format!(
                "style line {} requires: L/D COLOR FILL WIDTH",
                line_index + 1
            ));
        }
        let layer = resolve_layer(fields[0], layers)
            .ok_or_else(|| format!("style line {}: layer not found", line_index + 1))?;
        if !seen.insert(layer.index) {
            return Err(format!(
                "style line {}: duplicate layer {}",
                line_index + 1,
                fields[0]
            ));
        }
        let width: u8 = parse_value(fields[3], "style width")?;
        if !(1..=8).contains(&width) {
            return Err(format!("style width must be in 1..=8: {width}"));
        }
        styles.push(LayerStyle {
            layer_idx: layer.index,
            color: parse_color(fields[1])?,
            fill: parse_fill(fields[2])?,
            outline_width: width,
        });
    }
    if styles.is_empty() {
        return Err("style file contains no layer styles".to_string());
    }
    Ok(styles)
}

fn resolve_layer<'a>(spec: &str, layers: &'a [CacheLayer]) -> Option<&'a CacheLayer> {
    layers.iter().find(|layer| {
        spec == layer.name
            || spec == format!("{}/{}", layer.layer, layer.datatype)
            || spec == format!("idx:{}", layer.index)
    })
}

fn parse_color(value: &str) -> Result<[u8; 4], String> {
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| format!("color must start with #: {value}"))?;
    if !hex.is_ascii() {
        return Err(format!("invalid color: {value}"));
    }
    if hex.len() != 6 && hex.len() != 8 {
        return Err(format!("color must have 6 or 8 hex digits: {value}"));
    }
    let byte = |offset: usize| {
        u8::from_str_radix(&hex[offset..offset + 2], 16)
            .map_err(|_| format!("invalid color: {value}"))
    };
    Ok([
        byte(0)?,
        byte(2)?,
        byte(4)?,
        if hex.len() == 8 { byte(6)? } else { 255 },
    ])
}

fn parse_fill(value: &str) -> Result<LayerFill, String> {
    match value {
        "solid" => Ok(LayerFill::Solid),
        "speckle" => Ok(LayerFill::Speckle),
        "clear" => Ok(LayerFill::Clear),
        _ => {
            let hex = value
                .strip_prefix("pat:")
                .ok_or_else(|| format!("unknown fill: {value}"))?;
            if !hex.is_ascii() {
                return Err(format!("invalid pat fill: {value}"));
            }
            if hex.len() != 64 {
                return Err("pat fill requires exactly 64 hex digits".to_string());
            }
            let mut rows = [0u16; 16];
            for (index, row) in rows.iter_mut().enumerate() {
                *row = u16::from_str_radix(&hex[index * 4..index * 4 + 4], 16)
                    .map_err(|_| format!("invalid pat fill: {value}"))?;
            }
            Ok(LayerFill::Pattern(rows))
        }
    }
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn wire_escape(message: &str) -> String {
    message
        .chars()
        .map(|ch| if ch.is_whitespace() { '_' } else { ch })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_is_decided_before_the_raster() {
        // §F2R-20 review: frame_cache=0 and exact renders take no part
        // in retention (no keep, no clone, no store, no lookup).
        let on = render(
            parse_command("render gen=1 view=0,0,320,320 w=32 h=32 out=/tmp/a.raw")
                .unwrap()
                .unwrap(),
        );
        let off = render(
            parse_command("render gen=1 view=0,0,320,320 w=32 h=32 frame_cache=0 out=/tmp/a.raw")
                .unwrap()
                .unwrap(),
        );
        let exact = render(
            parse_command(
                "render gen=1 view=0,0,320,320 w=32 h=32 exact=1 frames=off out=/tmp/a.raw",
            )
            .unwrap()
            .unwrap(),
        );
        assert!(retention_enabled(&on));
        assert!(!retention_enabled(&off));
        assert!(!retention_enabled(&exact));
    }

    #[test]
    fn query_worker_answers_off_the_input_thread_and_shuts_down() {
        // §F2R-20b: queries flow through their own thread; without a
        // published scene they answer found=0, and Shutdown ends it.
        let (tx, rx) = mpsc::channel();
        let (responses, answers) = mpsc::channel();
        let scene: SharedPublishedScene = Arc::new(RwLock::new(None));
        let worker = thread::spawn(move || {
            query_worker(
                rx,
                responses,
                scene,
                RenderCancellation::new(),
                RenderCancellation::new(),
            )
        });
        tx.send(QueryCommand::Pick(PickCommand {
            sequence: 5,
            x: 1,
            y: 2,
            radius: 3,
            nth: 0,
            visible_layers: None,
        }))
        .unwrap();
        tx.send(QueryCommand::Snap(SnapCommand {
            sequence: 6,
            x: 1,
            y: 2,
            radius: 3,
            visible_layers: None,
        }))
        .unwrap();
        let pick = answers.recv().unwrap();
        assert!(pick.starts_with("pick seq=5 found=0"), "{pick}");
        let snap = answers.recv().unwrap();
        assert!(snap.starts_with("snap seq=6 found=0"), "{snap}");
        tx.send(QueryCommand::Shutdown).unwrap();
        worker.join().unwrap();
        assert_eq!(query_generation(-7), 0);
        assert_eq!(superseded("render cancelled: generation 1 is before 2".into()), QUERY_SUPERSEDED);
        assert_eq!(superseded("other".into()), "other");
    }

    fn render(command: InputCommand) -> RenderCommand {
        match command {
            InputCommand::Worker(WorkerCommand::Render(render)) => render,
            _ => panic!("expected render command"),
        }
    }

    /// Review 2026-09-11 P1-2: a retained frame (and the published
    /// query scene, which shares the key) is never reused across the
    /// thin policy - a keep frame served a cull request and vice versa.
    #[test]
    fn retained_key_tracks_the_thin_policy() {
        let parse = |thin: &str| {
            render(
                parse_command(&format!(
                    "render gen=1 view=0,0,320,320 w=32 h=32 frames=off thin={thin} out=/tmp/a.raw"
                ))
                .unwrap()
                .unwrap(),
            )
        };
        let keep = parse("keep");
        let cull = parse("cull");
        assert!(keep.thin_keep && !cull.thin_keep);
        assert_ne!(RetainedKey::new(&keep, Some(1)), RetainedKey::new(&cull, Some(1)));
        assert_eq!(RetainedKey::new(&keep, Some(1)), RetainedKey::new(&parse("keep"), Some(1)));
        // absent = cull, the plain layout's policy
        let absent = render(
            parse_command("render gen=1 view=0,0,320,320 w=32 h=32 frames=off out=/tmp/a.raw")
                .unwrap()
                .unwrap(),
        );
        assert_eq!(RetainedKey::new(&absent, Some(1)), RetainedKey::new(&cull, Some(1)));
        assert!(parse_command(
            "render gen=1 view=0,0,320,320 w=32 h=32 frames=off thin=maybe out=/tmp/a.raw"
        )
        .is_err());
    }

    /// The viewer's density toggle (2026-10-05): `density=on|off` is the
    /// frame's own and a retained frame never serves the other setting;
    /// absent, the environment's (here: no stack).
    #[test]
    fn retained_key_tracks_the_density_toggle() {
        let parse = |extra: &str| {
            render(
                parse_command(&format!("render gen=1 view=0,0,320,320 w=32 h=32 frames=off {extra} out=/tmp/a.raw"))
                    .unwrap()
                    .unwrap(),
            )
        };
        let (on, off, absent) = (parse("density=on"), parse("density=off"), parse(""));
        assert_eq!((on.density, off.density, absent.density), (Some(true), Some(false), None));
        assert!(density_stack_on(&on) && density_dots_on(&on));
        assert!(!density_stack_on(&off) && !density_dots_on(&off));
        assert_ne!(RetainedKey::new(&on, Some(1)), RetainedKey::new(&off, Some(1)));
        if std::env::var("FLOE_RUST_DENSITY_STACK").is_err() {
            assert_eq!(RetainedKey::new(&absent, Some(1)), RetainedKey::new(&off, Some(1)));
        }
        assert!(parse_command("render gen=1 view=0,0,320,320 w=32 h=32 frames=off density=maybe out=/tmp/a.raw").is_err());
    }

    /// budget_refit_enabled: pass 1's decode budget over what the layers'
    /// pages took past their estimates, and the key it is remembered by.
    #[test]
    fn a_layer_sets_budget_scale_cuts_the_plans_budget() {
        let parse = |extra: &str| {
            render(
                parse_command(&format!("render gen=1 view=0,0,320,320 w=32 h=32 frames=off {extra} out=/tmp/a.raw"))
                    .unwrap()
                    .unwrap(),
            )
        };
        let (off, on) = (parse("density=off"), parse("density=on"));
        let budget = 1024u64 << 20;
        // no scale, or one that is none: pass 1's budget as it is
        assert_eq!(scaled_decode_budget(budget, &off, None), budget);
        assert_eq!(scaled_decode_budget(budget, &off, Some(0.8)), budget);
        assert_eq!(scaled_decode_budget(budget, &off, Some(1.25)), (budget as f64 / 1.25) as u64);
        // with the density stack, of what pass 1 has: the budget less pass 2's reserve
        let pass1 = pass1_decode_budget(budget, &on);
        assert!(pass1 < budget);
        assert_eq!(scaled_decode_budget(budget, &on, Some(2.0)), pass1 / 2);
        assert_eq!(scaled_decode_budget(0, &off, Some(2.0)), 0);
        // the key: the layers, the depth, the root and the density stack on
        // or off (its pass 1 has pass 2's reserve to spare) - not the view
        let key = |extra: &str| budget_scale_key(&parse(extra));
        assert_ne!(key("density=off"), key("density=on"));
        assert_eq!(key("density=off"), key("density=off"));
        assert_eq!(budget_scale_key(&parse("")), budget_scale_key(&render(parse_command("render gen=2 view=50,50,90,90 w=64 h=64 frames=off out=/tmp/b.raw").unwrap().unwrap())));
        assert_ne!(key("layers=1/0"), key("layers=2/0"));
        assert_ne!(key("depth=1"), key("depth=2"));
        assert_ne!(key("root=3"), key(""));
    }

    fn snap(command: InputCommand) -> SnapCommand {
        match command {
            InputCommand::Snap(snap) => snap,
            _ => panic!("expected snap command"),
        }
    }

    fn pick(command: InputCommand) -> PickCommand {
        match command {
            InputCommand::Pick(pick) => pick,
            _ => panic!("expected pick command"),
        }
    }

    fn clip(command: InputCommand) -> ClipCommand {
        match command {
            InputCommand::Worker(WorkerCommand::Clip(clip)) => clip,
            _ => panic!("expected clip command"),
        }
    }

    #[test]
    fn parses_open_and_exact_render_contract() {
        let open = parse_command("open cache=/tmp/a.floe budget_mb=64 jobs=8")
            .unwrap()
            .unwrap();
        match open {
            InputCommand::Worker(WorkerCommand::Open(open)) => {
                assert_eq!(
                    open,
                    OpenCommand {
                        cache: Some("/tmp/a.floe".to_string()),
                        deck: None,
                        budget_mb: 64,
                        jobs: 8,
                    }
                );
            }
            _ => panic!("expected open command"),
        }
        match parse_command("open deck=/tmp/d.spec").unwrap().unwrap() {
            InputCommand::Worker(WorkerCommand::Open(open)) => {
                assert_eq!(open.deck.as_deref(), Some("/tmp/d.spec"));
                assert_eq!(open.cache, None);
            }
            _ => panic!("expected deck open command"),
        }
        assert!(parse_command("open budget_mb=1").is_err());
        assert!(parse_command("open cache=/a deck=/b").is_err());

        let parsed_render = render(
            parse_command(
                "render gen=7 view=-0.5,-0.5,9.5,9.5 w=10 h=10 depth=full cut=0 exact=1 frames=off layers=1/0,2/0 out=/tmp/f.png",
            )
            .unwrap()
            .unwrap(),
        );
        assert_eq!(parsed_render.generation, 7);
        assert_eq!(parsed_render.visible_layers.unwrap(), ["1/0", "2/0"]);
        assert_eq!(parsed_render.tile_size, DEFAULT_TILE_SIZE);
        assert_eq!(parsed_render.round_pages, DEFAULT_ROUND_PAGES);
        assert!(!parsed_render.unique_round_paths);
        assert!(parsed_render.exact);
        assert!(!parsed_render.frames);
        assert!(!parsed_render.labels);
        assert!(parsed_render.frame_cache);
        assert_eq!(parsed_render.label_font_px, DEFAULT_LABEL_FONT_PX);
        assert!(parse_command(
            "render gen=8 view=0,0,1,1 w=1 h=1 depth=0 cut=0 exact=1 frames=off out=/tmp/f.png"
        )
        .is_err());
        assert!(parse_command(
            "render gen=9 view=0,0,1,1 w=1 h=1 tile_px=0 frames=off out=/tmp/f.png"
        )
        .is_err());
        assert!(parse_command(
            "render gen=10 view=0,0,1,1 w=1 h=1 round_pages=0 frames=off out=/tmp/f.png"
        )
        .is_err());

        let progressive = render(
            parse_command(
                "render gen=11 view=0,0,1,1 w=1 h=1 jobs=3 decode_jobs=8 round_pages=17 round_paths=1 frames=off labels=on font_px=18 frame_cache=0 out=/tmp/f.png",
            )
            .unwrap()
            .unwrap(),
        );
        assert_eq!(progressive.round_pages, 17);
        assert_eq!(progressive.jobs, Some(3));
        assert_eq!(progressive.decode_jobs, Some(8));
        assert!(progressive.unique_round_paths);
        assert!(progressive.labels);
        assert!(!progressive.frame_cache);
        assert_eq!(progressive.label_font_px, 18.0);
        assert!(parse_command(
            "render gen=12 view=0,0,1,1 w=1 h=1 frames=off font_px=100 out=/tmp/f.png"
        )
        .is_err());

        assert_eq!(
            clip(
                parse_command(
                    "clip seq=13 box=-10,-20,30,40 layers=1/0,2/0 jobs=6 cell_hex=544f5020ed959ceab880 out=/tmp/c.oas",
                )
                .unwrap()
                .unwrap(),
            ),
            ClipCommand {
                sequence: 13,
                bbox: [-10, -20, 30, 40],
                visible_layers: Some(vec!["1/0".to_string(), "2/0".to_string()]),
                jobs: Some(6),
                cell_name: "TOP 한글".to_string(),
                out: "/tmp/c.oas".to_string(),
                root: None,
            }
        );
        assert!(parse_command("clip box=4,0,3,1 out=/tmp/c.oas").is_err());
        assert!(parse_command("clip box=0,0,1 out=/tmp/c.oas").is_err());
        assert!(parse_command("clip box=0,0,1,1 cell_hex=f out=/tmp/c.oas").is_err());
    }

    #[test]
    fn rejects_duplicate_and_unknown_protocol_fields() {
        assert!(parse_command("open cache=a cache=b").is_err());
        assert!(parse_command("info surprise=1").is_err());
        assert!(parse_command("cancel before_gen=9").is_ok());
    }

    #[test]
    fn parses_bounded_scene_queries() {
        assert_eq!(
            snap(
                parse_command("snap seq=4 x=-2 y=7 r=0 layers=2/0,1/3")
                    .unwrap()
                    .unwrap()
            ),
            SnapCommand {
                sequence: 4,
                x: -2,
                y: 7,
                radius: 1,
                visible_layers: Some(vec!["2/0".to_string(), "1/3".to_string()]),
            }
        );
        assert_eq!(
            pick(
                parse_command("pick seq=5 x=1 y=2 r=3 nth=-1 layers=all")
                    .unwrap()
                    .unwrap()
            ),
            PickCommand {
                sequence: 5,
                x: 1,
                y: 2,
                radius: 3,
                nth: -1,
                visible_layers: None,
            }
        );
        assert!(parse_command("snap x=1 y=2 r=3 unknown=4").is_err());
        assert_eq!(wire_hex("TOP 한글"), "544f5020ed959ceab880");
        assert_eq!(
            wire_unhex("544f5020ed959ceab880", "cell_hex").unwrap(),
            "TOP 한글"
        );
        assert_eq!(wire_points(&[(0, 1), (-2, 3)]), "0,1;-2,3");
    }

    #[test]
    fn parses_the_cell_tree_queries() {
        let hier = |line: &str| match parse_command(line).unwrap().unwrap() {
            InputCommand::Hier(command) => command,
            _ => panic!("expected a hier command"),
        };
        assert_eq!(hier("cell_sources seq=3"), HierCommand::Sources { sequence: 3 });
        assert_eq!(hier("cells seq=4"), HierCommand::Cells { sequence: 4, source: 0, cell: None });
        assert_eq!(
            hier("cells seq=5 src=2 cell=17"),
            HierCommand::Cells { sequence: 5, source: 2, cell: Some(17) }
        );
        assert_eq!(
            hier(&format!("cell_find seq=6 pat_hex={} limit=10", wire_hex("*inv?"))),
            HierCommand::Find { sequence: 6, source: None, pattern: "*inv?".to_string(), limit: 10 }
        );
        // a limit over the cap is the cap; an empty pattern is allowed
        assert_eq!(
            hier("cell_find seq=7 src=1 limit=999999"),
            HierCommand::Find { sequence: 7, source: Some(1), pattern: String::new(), limit: CELL_FIND_CAP }
        );
        assert_eq!(hier("cell_bbox seq=8 cell=9"), HierCommand::Bbox { sequence: 8, source: 0, cell: 9, root: None });
        assert_eq!(
            hier("cell_insts seq=9 src=1 cell=9 view=0,-5,10.5,20 cap=7"),
            HierCommand::Insts { sequence: 9, source: 1, cell: 9, view: [0.0, -5.0, 10.5, 20.0], cap: 7, root: None }
        );
        assert!(parse_command("cell_bbox seq=8").is_err());
        assert!(parse_command("cell_insts seq=9 cell=1 view=0,0,1,1 extra=1").is_err());
        assert!(parse_command("cells seq=1 cell=x").is_err());
        assert_eq!(
            hier_error_line("cells", 4, "nohier", "no summary"),
            format!("cells seq=4 found=0 code=nohier err_hex={}", wire_hex("no summary"))
        );
        assert_eq!(wire_f64_box(&[1.0, 2.5, -3.0, 4e9]), "1,2.5,-3,4000000000");
    }

    /// A plan request of a render command for the fit memory's keys.
    fn fit_request(command: &RenderCommand) -> PlanRequest {
        PlanRequest {
            view: ViewBox::new(0, 0, 320, 320).unwrap(),
            cut_dbu: 3,
            visible_layers: None,
            depth: FULL_DEPTH,
            px_per_dbu: 0.1,
            exact: false,
            sub_cut_wash: false,
            page_reps: false,
            decode_budget: 0,
            page_hairline: true,
            summary_layers: Vec::new(),
            prune_summary: false,
            sub_cut_box: false,
            shape_cut: false,
            shape_cut_max: false,
            frames: false,
            page_wash: false,
            lod_swap: false,
            regions: Vec::new(),
            visible_indices: None,
            fixed_fit: None,
            root: command.root,
            sub_cut_dots: None,
            dot_records: None,
            probe_limit: 0,
            free_pages: None,
            empty_top: true,
            dot_bright: None,
            dot_occ_first: None,
        }
    }

    /// forget_fits (budget_refit_enabled): when a layer set's budget scale is
    /// raised its fits go, at every cut and scale; another layer set's,
    /// depth's or root's stay.
    #[test]
    fn a_raised_budget_scale_forgets_the_layer_sets_fits() {
        let render = |extra: &str| match parse_command(&format!("render gen=1 view=0,0,320,320 w=32 h=32 frames=off {extra} out=/tmp/a.raw")).unwrap().unwrap() {
            InputCommand::Worker(WorkerCommand::Render(command)) => command,
            _ => panic!("expected a render command"),
        };
        let (top, layered, shallow, rooted) = (render(""), render("layers=1/0"), render("depth=1"), render("root=17"));
        let fine = |command: &RenderCommand| PlanRequest { cut_dbu: 1, px_per_dbu: 0.4, ..fit_request(command) };
        let mut fits = BTreeMap::new();
        let mut whole = BTreeSet::new();
        for command in [&top, &layered, &shallow, &rooted] {
            for key in [fit_memory_key(command, &fit_request(command)), fit_memory_key(command, &fine(command))] {
                fits.insert(key.clone(), floe_render_core::FixedFit::everything(3));
                whole.insert(key);
            }
        }
        assert_eq!((fits.len(), whole.len()), (8, 8));
        forget_fits(&mut fits, &mut whole, &top);
        let kept: BTreeSet<String> = [&layered, &shallow, &rooted]
            .into_iter()
            .flat_map(|command| [fit_memory_key(command, &fit_request(command)), fit_memory_key(command, &fine(command))])
            .collect();
        assert_eq!(fits.keys().cloned().collect::<BTreeSet<_>>(), kept);
        assert_eq!(whole, kept);
        // the margin's key is its viewport's: the same layers at the same scale
        let margin = render("bg=on");
        assert_eq!(fit_memory_key(&margin, &fit_request(&margin)), fit_memory_key(&top, &fit_request(&top)));
    }

    #[test]
    fn a_view_root_is_part_of_the_render_state() {
        let render = |line: &str| match parse_command(line).unwrap().unwrap() {
            InputCommand::Worker(WorkerCommand::Render(command)) => command,
            _ => panic!("expected a render command"),
        };
        let top = render("render gen=1 view=0,0,320,320 w=32 h=32 frames=off out=/tmp/a.raw");
        let rooted = render("render gen=1 view=0,0,320,320 w=32 h=32 frames=off root=17 out=/tmp/a.raw");
        assert_eq!(top.root, None);
        assert_eq!(rooted.root, Some(17));
        // another root is another retained state and another fit memory
        assert_ne!(RetainedKey::new(&top, Some(1)), RetainedKey::new(&rooted, Some(1)));
        assert_eq!(RetainedKey::new(&rooted, Some(1)), RetainedKey::new(&render(
            "render gen=2 view=5,5,325,325 w=32 h=32 frames=off root=17 out=/tmp/b.raw"), Some(1)));
        assert_ne!(fit_memory_key(&top, &fit_request(&top)), fit_memory_key(&rooted, &fit_request(&rooted)));
        // the clip and the cell queries carry it too
        match parse_command("clip seq=1 box=0,0,10,10 root=17 out=/tmp/c.oas").unwrap().unwrap() {
            InputCommand::Worker(WorkerCommand::Clip(clip)) => assert_eq!(clip.root, Some(17)),
            _ => panic!("expected a clip command"),
        }
        match parse_command("cell_bbox seq=8 cell=9 root=17").unwrap().unwrap() {
            InputCommand::Hier(HierCommand::Bbox { root, .. }) => assert_eq!(root, Some(17)),
            _ => panic!("expected a cell_bbox command"),
        }
        match parse_command("cell_insts seq=9 cell=9 view=0,0,1,1 root=17").unwrap().unwrap() {
            InputCommand::Hier(HierCommand::Insts { root, .. }) => assert_eq!(root, Some(17)),
            _ => panic!("expected a cell_insts command"),
        }
        assert!(parse_command("render gen=1 view=0,0,320,320 w=32 h=32 root=x out=/tmp/a.raw").is_err());
    }

    #[test]
    fn hier_worker_answers_without_a_cache_and_supersedes_queued_instance_walks() {
        let (tx, rx) = mpsc::channel();
        let (responses, answers) = mpsc::channel();
        let hier: SharedHier = Arc::new(RwLock::new(None));
        // queue three commands before the worker runs: the older
        // instance query is superseded by the newer one behind it, the
        // rest answer "cache not open"
        tx.send(HierCommand::Cells { sequence: 1, source: 0, cell: None }).unwrap();
        tx.send(HierCommand::Insts { sequence: 2, source: 0, cell: 1, view: [0.0, 0.0, 1.0, 1.0], cap: 4, root: None })
            .unwrap();
        tx.send(HierCommand::Insts { sequence: 3, source: 0, cell: 1, view: [0.0, 0.0, 2.0, 2.0], cap: 4, root: None })
            .unwrap();
        tx.send(HierCommand::Shutdown).unwrap();
        let worker = thread::spawn(move || hier_worker(rx, responses, hier));
        worker.join().unwrap();
        let lines: Vec<String> = answers.try_iter().collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines[0].starts_with("cells seq=1 found=0 code=state"), "{}", lines[0]);
        assert!(lines[1].starts_with("cell_insts seq=2 found=0 code=superseded"), "{}", lines[1]);
        assert!(lines[2].starts_with("cell_insts seq=3 found=0 code=state"), "{}", lines[2]);
    }

    #[test]
    fn non_ascii_wire_fields_return_errors_instead_of_panicking() {
        assert!(wire_unhex("한글", "cell_hex").is_err());
        // These have an accepted byte length but invalid UTF-8 character
        // boundaries for the old byte-offset string slicing.
        assert!(parse_color("#한글").is_err());
        let pattern = format!("pat:{}x", "한".repeat(21));
        assert_eq!(pattern[4..].len(), 64);
        assert!(parse_fill(&pattern).is_err());
    }

    #[test]
    fn expensive_round_collapses_the_refinement_tail() {
        let mut rounds = vec![vec![1, 2], vec![3, 4], vec![5], vec![6, 7]];
        collapse_refinement_tail(&mut rounds, 1);
        assert_eq!(rounds, vec![vec![1, 2], vec![3, 4], vec![5, 6, 7]]);
        // right before the final round the collapse changes nothing
        let mut two = vec![vec![1], vec![2, 3]];
        collapse_refinement_tail(&mut two, 0);
        assert_eq!(two, vec![vec![1], vec![2, 3]]);
    }

    #[test]
    fn the_fit_memory_keys_one_scale_whatever_bits_a_frame_derives() {
        // the viewport frame and its margin compute px_per_dbu from their own
        // boxes: the last bits differ, the scale is one
        let scale = 1922.0 / (18_732_000.0f64 + 1922.0 * 600.0 - 18_732_000.0);
        let margin = 3842.0 / (18_731_000.0f64 + 3842.0 * 600.0 - 18_731_000.0);
        assert_eq!(scale_token(scale), scale_token(margin));
        assert_eq!(scale_token(0.1), scale_token(0.1 + 4.0 * f64::EPSILON));
        // a zoom step is another scale
        assert_ne!(scale_token(0.1), scale_token(0.1 * 1.01));
        assert_ne!(scale_token(1e-6), scale_token(1e-6 * 1.0001));
    }

    #[test]
    fn a_margin_render_says_so_on_the_wire() {
        let render = |line: &str| match parse_command(line).unwrap().unwrap() {
            InputCommand::Worker(WorkerCommand::Render(command)) => command,
            _ => panic!("expected a render command"),
        };
        let margin = render("render gen=3 view=0,0,4,4 w=4 h=4 frames=off bg=on out=/tmp/m.png");
        assert!(margin.background);
        let viewport = render("render gen=4 view=1,1,3,3 w=2 h=2 frames=off out=/tmp/v.png");
        assert!(!viewport.background);
    }

    #[test]
    fn refinement_batches_are_cache_aware_and_cover_pages_once() {
        let empty = refinement_batches(&[], 64, |_| false).unwrap();
        assert_eq!(empty.len(), 1);
        assert!(empty[0].is_empty());

        let selected: Vec<u32> = (0..130).collect();
        let cold = refinement_batches(&selected, 64, |_| false).unwrap();
        assert_eq!(cold.iter().map(Vec::len).collect::<Vec<_>>(), [64, 66]);
        assert_eq!(cold.concat(), selected);

        let warm = refinement_batches(&selected, 64, |_| true).unwrap();
        assert_eq!(warm.as_slice(), std::slice::from_ref(&selected));

        let mixed = refinement_batches(&selected, 32, |page_id| page_id % 2 == 0).unwrap();
        assert_eq!(mixed.len(), 2);
        assert_eq!(mixed[0].len(), 65 + 32);
        let mut covered = mixed.concat();
        covered.sort_unstable();
        assert_eq!(covered, selected);
        assert!(refinement_batches(&[1], 0, |_| false).is_err());
    }

    #[test]
    fn retained_frames_are_bounded_per_state_and_scale() {
        // §F2R-18: one retained frame per (render state, scale),
        // newest last, capped - a zoom round-trip finds its scale
        // again while re-renders at one scale replace in place.
        let command = render(
            parse_command(
                "render gen=1 view=0,0,320,320 w=32 h=32 frames=off out=/tmp/a.raw",
            )
            .unwrap()
            .unwrap(),
        );
        let frame = |scale: f64| RetainedFrame {
            key: RetainedKey::new(&command, Some(7)),
            view: [0.0, 0.0, 320.0 * scale, 320.0 * scale],
            frame: floe_render_core::RgbaFrame::from_pixels(
                32,
                32,
                vec![0u8; 32 * 32 * 4],
            )
            .unwrap(),
            fit: None,
        };
        let roomy = usize::MAX;
        let mut retained = Vec::new();
        store_retained(&mut retained, frame(1.0), roomy);
        store_retained(&mut retained, frame(2.0), roomy);
        store_retained(&mut retained, frame(1.0), roomy);
        assert_eq!(retained.len(), 2, "same scale replaces in place");
        store_retained(&mut retained, frame(4.0), roomy);
        store_retained(&mut retained, frame(8.0), roomy);
        assert_eq!(retained.len(), RETAINED_FRAMES, "bounded");
        // the oldest scale (2.0) was evicted; 1.0 was refreshed later
        let spans: Vec<f64> = retained
            .iter()
            .map(|entry| entry.view[2] - entry.view[0])
            .collect();
        assert_eq!(spans, vec![320.0, 320.0 * 4.0, 320.0 * 8.0]);

        // A different style epoch never matches.
        let mut other = frame(1.0);
        other.key = RetainedKey::new(&command, Some(8));
        store_retained(&mut retained, other, roomy);
        assert_eq!(retained.len(), RETAINED_FRAMES);

        // §F2R-20: the byte budget evicts oldest-first past the count
        // cap, and a frame that alone exceeds it is never retained.
        let one = 32 * 32 * 4;
        let mut retained = Vec::new();
        store_retained(&mut retained, frame(1.0), 2 * one + 1);
        store_retained(&mut retained, frame(2.0), 2 * one + 1);
        store_retained(&mut retained, frame(4.0), 2 * one + 1);
        assert_eq!(retained.len(), 2, "two frames fit the byte budget");
        assert_eq!(retained_bytes(&retained), 2 * one);
        let spans: Vec<f64> = retained
            .iter()
            .map(|entry| entry.view[2] - entry.view[0])
            .collect();
        assert_eq!(spans, vec![320.0 * 2.0, 320.0 * 4.0], "oldest evicted");
        store_retained(&mut retained, frame(8.0), one - 1);
        assert_eq!(retained.len(), 2, "an over-budget frame is dropped, not stored");
        assert!(!retained.iter().any(|entry| entry.view[2] == 320.0 * 8.0));
        let mut none = Vec::new();
        store_retained(&mut none, frame(1.0), 0);
        assert!(none.is_empty(), "FLOE_RUST_RETAINED_MB=0 retains nothing");

        // §F2R-21 review: a contained same-scale frame (a viewport pan
        // inside the retained margin) leaves the margin in place; a
        // containing one (a new margin) replaces the viewport frame.
        let sized = |px: u32, view: [f64; 4]| RetainedFrame {
            key: RetainedKey::new(&command, Some(7)),
            view,
            frame: floe_render_core::RgbaFrame::from_pixels(
                px,
                px,
                vec![0u8; (px * px * 4) as usize],
            )
            .unwrap(),
            fit: None,
        };
        let mut retained = Vec::new();
        store_retained(&mut retained, sized(64, [0.0, 0.0, 640.0, 640.0]), roomy);
        store_retained(&mut retained, sized(32, [160.0, 160.0, 480.0, 480.0]), roomy);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].frame.width(), 64, "the containing margin stays");
        store_retained(&mut retained, sized(128, [-320.0, -320.0, 960.0, 960.0]), roomy);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].frame.width(), 128, "a containing frame replaces");
        assert!(view_contains(&[0.0, 0.0, 10.0, 10.0], &[0.0, 0.0, 10.0, 10.0]));
        assert!(!view_contains(&[0.0, 0.0, 10.0, 10.0], &[0.0, 0.0, 10.0, 11.0]));
        let shared: SharedPublishedScene = Arc::new(RwLock::new(None));
        assert!(!published_scene_serves(&shared, &command, Some(7), &SummaryKey::default()).unwrap());
    }

    #[test]
    fn frame_format_selects_raw_payloads_and_rejects_unknown_values() {
        let command = render(
            parse_command(
                "render gen=1 view=0,0,1,1 w=4 h=3 frames=off frame_format=raw out=/tmp/a.raw",
            )
            .unwrap()
            .unwrap(),
        );
        assert!(command.raw_frame);
        let default_command = render(
            parse_command("render gen=1 view=0,0,1,1 w=4 h=3 frames=off out=/tmp/a.png")
                .unwrap()
                .unwrap(),
        );
        assert!(!default_command.raw_frame);
        assert!(parse_command(
            "render gen=1 view=0,0,1,1 w=4 h=3 frames=off frame_format=bmp out=/tmp/a.bmp"
        )
        .is_err());
    }

    #[test]
    fn raw_frame_payload_is_header_plus_packed_rgba() {
        let pixels: Vec<u8> = (0..4u32 * 3 * 4).map(|value| value as u8).collect();
        let payload = raw_frame_payload(4, 3, &pixels);
        assert_eq!(&payload[..8], RAW_FRAME_MAGIC);
        assert_eq!(u32::from_le_bytes(payload[8..12].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(payload[12..16].try_into().unwrap()), 3);
        assert_eq!(&payload[16..], pixels.as_slice());
        assert_eq!(payload.len(), 16 + 4 * 3 * 4);
    }

    #[test]
    fn pan_reuse_maps_between_viewport_and_margin_frames() {
        // §F2R-17: sizes differ but the scale and the 16px grid match,
        // so a margin request maps the retained viewport into its
        // center, and a viewport request inside a margin frame maps
        // out fully covered.
        let mut state = WorkerState::default();
        let retained_cmd = render(
            parse_command(
                "render gen=0 view=0,0,320,320 w=32 h=32 frames=off out=/tmp/b.raw",
            )
            .unwrap()
            .unwrap(),
        );
        let pixels: Vec<u8> = (0..32u32 * 32)
            .flat_map(|index| [(index % 251) as u8, 1, 2, 255])
            .collect();
        state.retained = vec![RetainedFrame {
            key: RetainedKey::new(&retained_cmd, None),
            view: [0.0, 0.0, 320.0, 320.0],
            frame: floe_render_core::RgbaFrame::from_pixels(32, 32, pixels.clone()).unwrap(),
            fit: None,
        }];
        let mut margin = render(
            parse_command(
                "render gen=1 view=-160,-160,480,480 w=64 h=64 frames=off out=/tmp/a.raw",
            )
            .unwrap()
            .unwrap(),
        );
        let reuse = prepare_pan_reuse(&state, &mut margin, &SummaryKey::default(), None).expect("margin maps the center");
        assert_eq!(reuse.valid, [16, 16, 48, 48]);
        // margin pixel (16,16) is retained pixel (0,0)
        assert_eq!(&reuse.base.pixels()[(16 * 64 + 16) * 4..][..4], &pixels[..4]);
        assert_eq!(margin.view, [-160.0, -160.0, 480.0, 480.0]);

        let margin_pixels = vec![7u8; 64 * 64 * 4];
        state.retained = vec![RetainedFrame {
            key: RetainedKey::new(&retained_cmd, None),
            view: [-160.0, -160.0, 480.0, 480.0],
            frame: floe_render_core::RgbaFrame::from_pixels(64, 64, margin_pixels).unwrap(),
            fit: None,
        }];
        let mut inside = render(
            parse_command(
                "render gen=2 view=0,0,320,320 w=32 h=32 frames=off out=/tmp/c.raw",
            )
            .unwrap()
            .unwrap(),
        );
        let reuse = prepare_pan_reuse(&state, &mut inside, &SummaryKey::default(), None).expect("viewport maps out");
        assert_eq!(reuse.valid, [0, 0, 32, 32], "fully covered by the margin");

        // Vertical pan: row 0 is the TOP (world y1), so a pan UP in
        // world coordinates pulls retained rows DOWN in the base -
        // §3.24 pinned a sign flip here that corrupted every pan with
        // a y component.
        let row_coded: Vec<u8> = (0..32u32 * 32)
            .flat_map(|index| [(index / 32) as u8, 0, 0, 255])
            .collect();
        state.retained = vec![RetainedFrame {
            key: RetainedKey::new(&retained_cmd, None),
            view: [0.0, 0.0, 320.0, 320.0],
            frame: floe_render_core::RgbaFrame::from_pixels(32, 32, row_coded).unwrap(),
            fit: None,
        }];
        let mut panned_up = render(
            parse_command(
                "render gen=3 view=0,160,320,480 w=32 h=32 frames=off out=/tmp/d.raw",
            )
            .unwrap()
            .unwrap(),
        );
        let reuse = prepare_pan_reuse(&state, &mut panned_up, &SummaryKey::default(), None).expect("vertical pan maps");
        // request y1 = 480 sits 16 rows above retained y1 = 320:
        // request rows 16..32 hold retained rows 0..16.
        assert_eq!(reuse.valid, [0, 16, 32, 32]);
        assert_eq!(reuse.base.pixels()[16 * 32 * 4], 0, "row 16 = old row 0");
        assert_eq!(reuse.base.pixels()[31 * 32 * 4], 15, "row 31 = old row 15");
    }

    #[test]
    fn a_containing_frame_under_another_fit_does_not_keep_its_place() {
        // review 2026-09-28: the containing same-scale frame stayed whatever
        // its fit, so after a redecision no frame at the scale could serve
        let command = render(parse_command("render gen=1 view=0,0,320,320 w=32 h=32 frames=off out=/tmp/a.raw").unwrap().unwrap());
        let frame = |px: u32, view: [f64; 4], fit: Option<floe_render_core::FixedFit>| RetainedFrame {
            key: RetainedKey::new(&command, Some(7)),
            view,
            frame: floe_render_core::RgbaFrame::from_pixels(px, px, vec![0u8; (px * px * 4) as usize]).unwrap(),
            fit,
        };
        let (a, b) = (
            Some(floe_render_core::FixedFit { cut_dbu: 5, class: 3, phase: 7, page: 9 }),
            Some(floe_render_core::FixedFit { cut_dbu: 5, class: 4, phase: 7, page: 9 }),
        );
        let mut retained = vec![frame(64, [-160.0, -160.0, 480.0, 480.0], a)];
        // the same fit: the containing margin stays
        store_retained(&mut retained, frame(32, [0.0, 0.0, 320.0, 320.0], a), usize::MAX);
        assert_eq!((retained.len(), retained[0].frame.width()), (1, 64));
        // another fit: the new frame replaces it
        store_retained(&mut retained, frame(32, [0.0, 0.0, 320.0, 320.0], b), usize::MAX);
        assert_eq!((retained.len(), retained[0].frame.width(), retained[0].fit), (1, 32, b));
    }

    #[test]
    fn pan_reuse_finds_an_older_frame_at_the_scale_and_none_under_another_fit() {
        // review 2026-09-28: A -> B -> A found nothing, since only the newest
        // frame of the render state was tried; and a frame planned under
        // another budget fit must not serve
        let mut state = WorkerState::default();
        let cmd = |line: &str| render(parse_command(line).unwrap().unwrap());
        let a_cmd = cmd("render gen=0 view=0,0,320,320 w=32 h=32 frames=off out=/tmp/a.raw");
        let b_cmd = cmd("render gen=1 view=0,0,640,640 w=32 h=32 frames=off out=/tmp/b.raw");
        let frame = |px: u32| floe_render_core::RgbaFrame::from_pixels(px, px, vec![3u8; (px * px * 4) as usize]).unwrap();
        let fit = floe_render_core::FixedFit { cut_dbu: 5, class: 3, phase: 7, page: 9 };
        state.retained = vec![
            RetainedFrame { key: RetainedKey::new(&a_cmd, None), view: [0.0, 0.0, 320.0, 320.0], frame: frame(32), fit: Some(fit) },
            RetainedFrame { key: RetainedKey::new(&b_cmd, None), view: [0.0, 0.0, 640.0, 640.0], frame: frame(32), fit: Some(fit) },
        ];
        let mut again = cmd("render gen=2 view=0,0,320,320 w=32 h=32 frames=off out=/tmp/c.raw");
        let reuse = prepare_pan_reuse(&state, &mut again, &SummaryKey::default(), Some(fit)).expect("the older frame at this scale serves");
        assert_eq!(reuse.valid, [0, 0, 32, 32]);
        let other = floe_render_core::FixedFit { cut_dbu: 5, class: 4, phase: 7, page: 9 };
        assert!(prepare_pan_reuse(&state, &mut again, &SummaryKey::default(), Some(other)).is_none(), "another decision: nothing to reuse");
        assert!(prepare_pan_reuse(&state, &mut again, &SummaryKey::default(), None).is_none());
    }

    #[test]
    fn generation_page_charge_is_bounded_by_open_budget() {
        assert_eq!(checked_generation_bytes(40, 24, 64).unwrap(), 64);
        let error = checked_generation_bytes(40, 25, 64).unwrap_err();
        assert!(error.contains("decoded generation budget exceeded"));
        assert!(checked_generation_bytes(u64::MAX, 1, u64::MAX).is_err());
    }

    #[test]
    fn cancellation_errors_are_distinct_from_render_failures() {
        assert!(is_render_cancelled_error("render cancelled"));
        assert!(is_render_cancelled_error(
            "render cancelled: generation 4 is before 5"
        ));
        assert!(!is_render_cancelled_error("write frame.png: no space left"));
    }

    #[test]
    fn parses_style_file_in_bottom_to_top_order() {
        let layers = [
            CacheLayer {
                index: 2,
                layer: 10,
                datatype: 0,
                name: "M1".to_string(),
            },
            CacheLayer {
                index: 4,
                layer: 20,
                datatype: 1,
                name: "M2".to_string(),
            },
        ];
        let pattern = "8000".repeat(16);
        let styles = parse_styles(
            &format!("M1 #ff0000 speckle 1\n20/1 #00ff0080 pat:{pattern} 4\n"),
            &layers,
        )
        .unwrap();
        assert_eq!(styles[0].layer_idx, 2);
        assert_eq!(styles[1].layer_idx, 4);
        assert_eq!(styles[1].color, [0, 255, 0, 128]);
        assert_eq!(styles[1].fill, LayerFill::Pattern([0x8000; 16]));
        assert_eq!(styles[1].outline_width, 4);
        assert!(parse_styles("# comments only\n", &layers).is_err());
    }

    #[test]
    fn atomic_publish_does_not_leave_temporary_file() {
        let dir = std::env::temp_dir().join(format!(
            "floe-renderd-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("publish")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let output = dir.join("frame.png");
        let cancellation = RenderCancellation::new();
        publish_frame(
            output.to_str().unwrap(),
            4,
            &[b"png-".as_slice(), b"bytes".as_slice()],
            &cancellation,
        )
        .unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), b"png-bytes");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_file(output).unwrap();

        let clip_output = dir.join("clip.oas");
        publish_bytes(
            clip_output.to_str().unwrap(),
            8,
            4,
            b"oasis-bytes",
            &cancellation,
        )
        .unwrap();
        assert_eq!(std::fs::read(&clip_output).unwrap(), b"oasis-bytes");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_file(clip_output).unwrap();

        cancellation.cancel_before(5);
        assert!(publish_frame(
            dir.join("stale.png").to_str().unwrap(),
            4,
            &[b"must-not-publish".as_slice()],
            &cancellation,
        )
        .is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

        assert!(publish_bytes(
            dir.join("stale.oas").to_str().unwrap(),
            9,
            4,
            b"must-not-publish",
            &cancellation,
        )
        .is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir(dir).unwrap();
    }
}
