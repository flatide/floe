use floe_render_core::{
    pick_scene, render_geometry_occupancy_cancellable, render_geometry_styled_cancellable,
    snap_scene, validate_font_px, Cache, CacheLayer, ClipGeometry, DecodedPageCache, FrameScene,
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
const SNAP_SHAPE_CAP: usize = 400;
const PICK_CANDIDATE_CAP: usize = 64;
const QUERY_MEMBER_CAP: usize = 400;

struct PublishedScene {
    scene: Arc<FrameScene>,
    layers: Arc<[CacheLayer]>,
    cell_names: Arc<BTreeMap<u32, String>>,
}

type SharedPublishedScene = Arc<RwLock<Option<Arc<PublishedScene>>>>;

fn main() -> ExitCode {
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
        format!("ready version={}", env!("CARGO_PKG_VERSION")),
    );

    let cancellation = RenderCancellation::new();
    let worker_cancellation = cancellation.clone();
    let worker_responses = response_tx.clone();
    let published_scene = Arc::new(RwLock::new(None));
    let worker_scene = Arc::clone(&published_scene);
    let (command_tx, command_rx) = mpsc::channel::<WorkerCommand>();
    let worker = thread::spawn(move || {
        render_worker(
            command_rx,
            worker_responses,
            worker_cancellation,
            worker_scene,
        )
    });

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
            InputCommand::Worker(command) => {
                if let WorkerCommand::Render(render) = &command {
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
            InputCommand::Snap(command) => handle_snap(&published_scene, command, &response_tx),
            InputCommand::Pick(command) => handle_pick(&published_scene, command, &response_tx),
            InputCommand::Quit => break,
        }
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
    Quit,
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
    cache: String,
    budget_mb: u64,
    jobs: u16,
}

#[derive(Debug, PartialEq, Eq)]
struct StyleCommand {
    epoch: u64,
    path: String,
}

#[derive(Debug, PartialEq)]
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
    /// Raster workers. Legacy `jobs` continues to set both phases when
    /// `decode_jobs` is absent.
    jobs: Option<u16>,
    decode_jobs: Option<u16>,
    tile_size: u16,
    decode_pages: Option<usize>,
    round_pages: usize,
    unique_round_paths: bool,
    style_epoch: Option<u64>,
    out: String,
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
            reject_unknown(&fields, &["cache", "budget_mb", "jobs"])?;
            let jobs = optional_parse(&fields, "jobs")?.unwrap_or(DEFAULT_JOBS);
            validate_jobs(jobs)?;
            Ok(Some(InputCommand::Worker(WorkerCommand::Open(
                OpenCommand {
                    cache: required(&fields, "cache")?.to_string(),
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
        "render" => {
            reject_unknown(
                &fields,
                &[
                    "gen",
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
                    "jobs",
                    "decode_jobs",
                    "tile_px",
                    "decode_pages",
                    "round_pages",
                    "round_paths",
                    "style_epoch",
                    "out",
                ],
            )?;
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
                    jobs,
                    decode_jobs,
                    tile_size,
                    decode_pages: optional_parse(&fields, "decode_pages")?,
                    round_pages,
                    unique_round_paths: optional_bool(&fields, "round_paths")?.unwrap_or(false),
                    style_epoch: optional_parse(&fields, "style_epoch")?,
                    out: required(&fields, "out")?.to_string(),
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
                &["seq", "box", "layers", "jobs", "cell_hex", "out"],
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
                },
            ))))
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

struct WorkerState {
    cache: Option<Cache>,
    page_cache: DecodedPageCache,
    jobs: u16,
    styles: Vec<LayerStyle>,
    style_epoch: Option<u64>,
}

impl Default for WorkerState {
    fn default() -> Self {
        Self {
            cache: None,
            page_cache: DecodedPageCache::new(DEFAULT_BUDGET_MB * 1024 * 1024),
            jobs: DEFAULT_JOBS,
            styles: Vec::new(),
            style_epoch: None,
        }
    }
}

fn render_worker(
    commands: Receiver<WorkerCommand>,
    responses: Sender<String>,
    cancellation: RenderCancellation,
    published_scene: SharedPublishedScene,
) {
    let mut state = WorkerState::default();
    for command in commands {
        match command {
            WorkerCommand::Open(command) => {
                handle_open(&mut state, command, &responses, &published_scene)
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
) {
    if state.cache.is_some() {
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
    match Cache::open(&command.cache) {
        Ok(cache) => {
            let info = cache.info();
            state.cache = Some(cache);
            state.page_cache = DecodedPageCache::new(budget_bytes);
            state.jobs = command.jobs;
            state.styles.clear();
            state.style_epoch = None;
            if let Ok(mut published) = published_scene.write() {
                *published = None;
            }
            respond(
                responses,
                format!(
                    "opened unit={} top={} layers={} cells={} pages={} ovp_bytes={} budget_bytes={} jobs={}",
                    info.unit,
                    info.top_cell,
                    info.layers,
                    info.cells,
                    info.pages,
                    info.ovp_bytes,
                    budget_bytes,
                    command.jobs
                ),
            );
        }
        Err(error) => respond(
            responses,
            format!("error code=open message={}", wire_escape(&error)),
        ),
    }
}

fn handle_style(state: &mut WorkerState, command: StyleCommand, responses: &Sender<String>) {
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

fn handle_snap(shared: &SharedPublishedScene, command: SnapCommand, responses: &Sender<String>) {
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
        snap_scene(&published.scene, &request)
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

fn handle_pick(shared: &SharedPublishedScene, command: PickCommand, responses: &Sender<String>) {
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
        let pick = pick_scene(&published.scene, &request, command.nth)?;
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
    let result = run_render(state, &command, responses, cancellation, published_scene);
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

fn run_render(
    state: &mut WorkerState,
    command: &RenderCommand,
    responses: &Sender<String>,
    cancellation: &RenderCancellation,
    published_scene: &SharedPublishedScene,
) -> Result<(), String> {
    let cache = state
        .cache
        .as_ref()
        .ok_or_else(|| "cache not open".to_string())?;
    check_generation(cancellation, command.generation)?;
    let request = make_plan_request(cache, command)?;
    let planned = cache.plan(&request)?;
    check_generation(cancellation, command.generation)?;
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
        tile_size: command.tile_size,
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
    let plan = Arc::new(planned.plan);
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
    let rounds = refinement_batches(&selected, command.round_pages, |page_id| {
        state.page_cache.contains(page_id)
    })?;
    let mut decoded_pages = Vec::with_capacity(selected.len());
    let mut generation_bytes = 0u64;
    for (round_index, round_page_ids) in rounds.iter().enumerate() {
        check_generation(cancellation, command.generation)?;
        let (mut round_pages, decode_stats) = state.page_cache.load_cancellable(
            cache,
            round_page_ids,
            decode_workers,
            command.generation,
            cancellation,
        )?;
        let round_bytes = round_pages.iter().try_fold(0u64, |total, page| {
            total
                .checked_add(page.estimated_bytes())
                .ok_or_else(|| "decoded generation byte charge overflow".to_string())
        })?;
        generation_bytes = checked_generation_bytes(
            generation_bytes,
            round_bytes,
            state.page_cache.budget_bytes(),
        )?;
        decoded_pages.append(&mut round_pages);
        check_generation(cancellation, command.generation)?;
        let scene_started = Instant::now();
        let scene = Arc::new(FrameScene::new_shared_with_labels(
            cache,
            Arc::clone(&plan),
            decoded_pages.clone(),
            Arc::clone(&labels),
            command.label_font_px,
        )?);
        let scene_us = elapsed_us(scene_started);
        check_generation(cancellation, command.generation)?;

        let report = if styles.is_empty() && !command.frames {
            render_geometry_occupancy_cancellable(
                &scene,
                &raster_request,
                command.generation,
                cancellation,
            )?
        } else {
            render_geometry_styled_cancellable(
                &scene,
                &StyledGeometryRasterRequest {
                    raster: raster_request,
                    layers: styles.clone(),
                    hierarchy_frames: command.frames,
                    mono: command.mono,
                },
                command.generation,
                cancellation,
            )?
        };
        check_generation(cancellation, command.generation)?;
        let png_started = Instant::now();
        let png = report.frame.png_bytes()?;
        let png_us = elapsed_us(png_started);
        check_generation(cancellation, command.generation)?;
        let final_round = round_index + 1 == rounds.len();
        let published_output = if command.unique_round_paths && !final_round {
            format!(
                "{}.gen-{}.round-{}.partial.png",
                command.out,
                command.generation,
                round_index + 1
            )
        } else {
            command.out.clone()
        };
        let publish_stats =
            publish_png(&published_output, command.generation, &png, cancellation)?;
        // A successful rename is the generation's linearization point. A
        // later cancellation must not turn an already-published frame into a
        // cancelled response or leave a reported-less partial file behind.
        {
            let mut published = published_scene
                .write()
                .map_err(|_| "published scene lock poisoned".to_string())?;
            *published = Some(Arc::new(PublishedScene {
                scene: Arc::clone(&scene),
                layers: Arc::clone(&query_layers),
                cell_names: Arc::clone(&query_cell_names),
            }));
        }

        respond(
            responses,
            format!(
                "frame gen={} round={} final={} png={} partial={} deferred={} style_epoch={} plan_us={} text_plan_us={} labels={} labels_truncated={} text_place_records={} read_us={} decode_us={} decode_workers={} scene_us={} raster_us={} png_us={} publish_write_us={} publish_sync_us={} publish_rename_us={} workers={} tiles={} tile_px={} pages={} plan_pages={} cache_hit={} cache_miss={} resident_bytes={} wc_cells={} inst_edges={} frame_rects={} rect_paints={} polygon_paints={} path_paints={} frame_paints={} label_tile_paints={} label_pixel_paints={}",
                command.generation,
                round_index + 1,
                final_round as u8,
                published_output,
                report.partial as u8,
                scene.deferred_pages().len(),
                state
                    .style_epoch
                    .map(|epoch| epoch.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                planned.stats.plan_us,
                planned_labels.as_ref().map(|p| p.plan_us).unwrap_or(0),
                labels.len(),
                (report.labels_truncated
                    || planned_labels
                        .as_ref()
                        .is_some_and(|p| p.stats.truncated)) as u8,
                planned_labels
                    .as_ref()
                    .map(|p| p.stats.place_records_scanned)
                    .unwrap_or(0),
                decode_stats.page_read_us,
                decode_stats.page_decode_us,
                decode_stats.decode_workers_used,
                scene_us,
                report.stats.raster_us,
                png_us,
                publish_stats.write_us,
                publish_stats.sync_us,
                publish_stats.rename_us,
                report.stats.workers_used,
                report.stats.tiles,
                command.tile_size,
                scene.available_pages(),
                planned.summary.pages,
                decode_stats.decoded_cache_hit,
                decode_stats.decoded_cache_miss,
                state.page_cache.resident_bytes(),
                planned.summary.wc_cells,
                planned.summary.inst_edges,
                planned.summary.frame_rects,
                report.rectangle_member_paints,
                report.polygon_member_paints,
                report.path_member_paints,
                report.frame_member_paints,
                report.label_tile_paints,
                report.label_pixel_paints,
            ),
        );
    }
    Ok(())
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

    let mut batches: Vec<Vec<u32>> = missing
        .chunks(round_pages)
        .map(<[u32]>::to_vec)
        .collect();
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

fn make_plan_request(cache: &Cache, command: &RenderCommand) -> Result<PlanRequest, String> {
    let [x0, y0, x1, y1] = command.view;
    let planner_x0 = checked_bound(x0.floor(), "view x0")?;
    let planner_y0 = checked_bound(y0.floor(), "view y0")?;
    let planner_x1 = checked_bound(x1.ceil(), "view x1")?;
    let planner_y1 = checked_bound(y1.ceil(), "view y1")?;
    let span_x = x1 - x0;
    let span_y = y1 - y0;
    let px_per_dbu = (command.width as f64 / span_x).min(command.height as f64 / span_y);
    let cut_dbu = if command.exact || command.cut_px == 0.0 {
        0
    } else {
        checked_bound((command.cut_px / px_per_dbu).ceil(), "cut dbu")?
    };
    let request = PlanRequest {
        view: ViewBox::new(planner_x0, planner_y0, planner_x1, planner_y1)?,
        cut_dbu,
        visible_layers: command.visible_layers.clone(),
        depth: command.depth,
        px_per_dbu,
        exact: command.exact,
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

fn publish_png(
    output: &str,
    generation: u64,
    bytes: &[u8],
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
    if let Err(error) = file.write_all(bytes) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("write {}: {}", temporary.display(), error));
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

    fn render(command: InputCommand) -> RenderCommand {
        match command {
            InputCommand::Worker(WorkerCommand::Render(render)) => render,
            _ => panic!("expected render command"),
        }
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
                        cache: "/tmp/a.floe".to_string(),
                        budget_mb: 64,
                        jobs: 8,
                    }
                );
            }
            _ => panic!("expected open command"),
        }

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
                "render gen=11 view=0,0,1,1 w=1 h=1 jobs=3 decode_jobs=8 round_pages=17 round_paths=1 frames=off labels=on font_px=18 out=/tmp/f.png",
            )
            .unwrap()
            .unwrap(),
        );
        assert_eq!(progressive.round_pages, 17);
        assert_eq!(progressive.jobs, Some(3));
        assert_eq!(progressive.decode_jobs, Some(8));
        assert!(progressive.unique_round_paths);
        assert!(progressive.labels);
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
    fn refinement_batches_are_cache_aware_and_cover_pages_once() {
        let empty = refinement_batches(&[], 64, |_| false).unwrap();
        assert_eq!(empty.len(), 1);
        assert!(empty[0].is_empty());

        let selected: Vec<u32> = (0..130).collect();
        let cold = refinement_batches(&selected, 64, |_| false).unwrap();
        assert_eq!(cold.iter().map(Vec::len).collect::<Vec<_>>(), [64, 66]);
        assert_eq!(cold.concat(), selected);

        let warm = refinement_batches(&selected, 64, |_| true).unwrap();
        assert_eq!(warm, [selected.clone()]);

        let mixed = refinement_batches(&selected, 32, |page_id| page_id % 2 == 0).unwrap();
        assert_eq!(mixed.len(), 2);
        assert_eq!(mixed[0].len(), 65 + 32);
        let mut covered = mixed.concat();
        covered.sort_unstable();
        assert_eq!(covered, selected);
        assert!(refinement_batches(&[1], 0, |_| false).is_err());
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
        publish_png(output.to_str().unwrap(), 4, b"png-bytes", &cancellation).unwrap();
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
        assert!(publish_png(
            dir.join("stale.png").to_str().unwrap(),
            4,
            b"must-not-publish",
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
