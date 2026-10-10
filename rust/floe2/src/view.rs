//! The GTK viewer's view channel (docs/SHARED_APP_LAYER.ko.md §7, P4c): a
//! source viewed through the shared `floe_app_core::view::ViewController` -
//! the render policy (what to draw, when, a frame superseded or cancelled,
//! the margin) is the controller's; the viewer sends edits and shows the
//! frames it is given.
//!
//!   view_open {source, levels?, mode?, width, height, patch?, margin?,
//!              frame_cache?}           -> {view, snapshot, model}
//!   view_edit {view, patch, base?}     -> snapshot (base: the state_rev the
//!                                         edit is relative to; default the
//!                                         current one - the viewer is the
//!                                         view's only editor)
//!   view_cancel {view}                 -> snapshot (Esc: the frame in
//!                                         progress stops)
//!   view_snapshot {view}               -> snapshot
//!   view_close {view}                  -> null
//!   view_query {view, frame, kind: snap|pick, x, y, r_px, nth?, layers?}
//!                                      -> {id}; the answer is a `query` event
//!                                         (x, y in viewport px from its
//!                                         top-left; frame = the frame shown)
//!   view_cells {view, seq, kind: cell_sources|cells|cell_find|cell_bbox|
//!               cell_insts, src?, cell?, pattern?, limit?, box?, cap?, root?}
//!                                      -> null; the answer is a `cells` event
//!   view_minimap {view, depth?, bbox?} -> {size, key, base, bbox, die, scale}:
//!                                         the overview's base image (palette
//!                                         digits, size x size) and the die's
//!                                         place in it; bbox = a view root's
//!   view_clip {view, seq, bbox (dbu, the root's coordinates), layers?,
//!              cell_name?, out}       -> null; a `clip` event when written
//!
//! Events, between replies, one JSON object per line:
//!   {"event": "view", "view": N, "snapshot": {...}}   the state, phase,
//!       counters or a failure changed
//!   {"event": "frame", "view": N, "frame": {...}}     a frame to show: its
//!       pixels in `path` (FLOERAW1 header + RGBA, or a PNG) - the viewer
//!       reads and removes it; the service removes the files it is not given
//!       time for; `report` (its generation's rounds added up, the dict
//!       floe/rust_render.py's `_emit_frame` gave) and `perf` ([the log
//!       line, the lower bar's], view::perf::perf_status) - the perf line
//!   {"event": "closed", "view": N}
//!   {"event": "query" | "cells" | "clip", "view": N, "result": {...}} - the
//!       answers in the dicts floe/rust_render.py gave the viewer
use floe_app_core::{
    artifact,
    clip::{self, ClipOptions},
    dataset::Dataset,
    jobdeck::color::Mode,
    managed::{Limits, ManagedDataset, Resources},
    render::RenderOptions,
    shots::{Detail, Thin},
    view::{
        minimap, perf, CellOutcome, CellReply, CellRequest, ControllerOptions, Depth,
        DesktopPolicy, DisplayFrame, Model, Navigation, Patch, Phase, Purpose, QueryOperation,
        RootEdit, Snapshot, StyleDelta, ViewController, ViewQuery, ViewQueryResult, ViewState,
    },
    Error, ErrorKind, Result,
};
use floe_worker_client::{ClipRequest, Fill, FrameFormat, Layers, QueryHit, SnapKind};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The service's stdout: replies and events, a whole line at a time.
pub struct Out(Mutex<std::io::Stdout>);
impl Out {
    pub fn new() -> Self {
        Self(Mutex::new(std::io::stdout()))
    }
    pub fn line(&self, value: &Value) {
        let mut out = self.0.lock().unwrap();
        let _ = writeln!(out, "{value}");
        let _ = out.flush();
    }
}

/// How often a view's pump looks at its controller.
const PUMP: Duration = Duration::from_millis(5);
/// Frame files given to the viewer and not yet taken: the oldest go.
const KEPT_FRAMES: usize = 4;

struct View {
    controller: Arc<ViewController>,
    stop: Arc<AtomicBool>,
    pump: Option<JoinHandle<()>>,
    folder: PathBuf,
}
impl Drop for View {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.controller.request_close();
        if let Some(p) = self.pump.take() {
            let _ = p.join();
        }
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// The views one service holds (the viewer has one open at a time; a
/// switch closes the old one first).
pub struct Views {
    resources: Option<Arc<Resources>>,
    next: u64,
    views: BTreeMap<u64, View>,
    out: Arc<Out>,
}
impl Views {
    pub fn new(out: Arc<Out>) -> Self {
        Self {
            resources: None,
            next: 0,
            views: BTreeMap::new(),
            out,
        }
    }
    /// One worker's CPU and memory, as the viewer's render options ask -
    /// the desktop has no other tenants to share them with.
    fn resources(&mut self, options: &RenderOptions) -> Result<Arc<Resources>> {
        if let Some(r) = &self.resources {
            return Ok(Arc::clone(r));
        }
        let slots = u32::from(options.decode_jobs) + u32::from(options.raster_jobs);
        let r = Resources::new(Limits {
            cpu_slots: slots * 2,
            foreground_reserve: 0,
            workers: 2,
            decoded_mb: options.budget_mb * 2,
        })?;
        self.resources = Some(Arc::clone(&r));
        Ok(r)
    }
    pub fn handle(&mut self, op: &str, request: &Value, cancelled: &AtomicUsize) -> Result<Value> {
        match op {
            "view_open" => self.open(request, cancelled),
            "view_edit" => {
                let view = self.view(request)?;
                let patch = patch(request.get("patch").unwrap_or(&Value::Null))?;
                let base = match request.get("base").and_then(Value::as_u64) {
                    Some(b) => b,
                    None => view.controller.snapshot().state_rev,
                };
                Ok(snapshot_json(&view.controller.edit(base, patch)?))
            }
            "view_cancel" => Ok(snapshot_json(
                &self.view(request)?.controller.cancel_render(),
            )),
            "view_snapshot" => Ok(snapshot_json(&self.view(request)?.controller.snapshot())),
            "view_minimap" => {
                let view = self.view(request)?;
                minimap_json(&view.controller, request)
            }
            "view_close" => {
                let id = view_id(request)?;
                self.views.remove(&id);
                Ok(Value::Null)
            }
            "view_query" => {
                let view = self.view(request)?;
                let frame = request
                    .get("frame")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Error::input("frame must be a number"))?;
                let anchor = view.controller.query_anchor(frame)?;
                let viewport = view.controller.snapshot().state.viewport;
                let num = |k: &str| {
                    request
                        .get(k)
                        .and_then(Value::as_f64)
                        .ok_or_else(|| Error::input(format!("{k} must be a number")))
                };
                let operation = match request.get("kind").and_then(Value::as_str) {
                    Some("snap") => QueryOperation::Snap,
                    Some("pick") => QueryOperation::Pick {
                        nth: request.get("nth").and_then(Value::as_i64).unwrap_or(0),
                    },
                    _ => return Err(Error::input("kind must be snap or pick")),
                };
                let layers = match request.get("layers") {
                    None | Some(Value::Null) => Layers::All,
                    Some(Value::Array(a)) if a.is_empty() => Layers::All,
                    Some(Value::Array(a)) => {
                        Layers::Only(a.iter().map(pair).collect::<Result<_>>()?)
                    }
                    Some(_) => return Err(Error::input("layers must be a list")),
                };
                let id = view.controller.query(ViewQuery {
                    anchor,
                    operation,
                    position: [
                        num("x")? / f64::from(viewport.width),
                        num("y")? / f64::from(viewport.height),
                    ],
                    radius_px: num("r_px")?,
                    layers,
                })?;
                Ok(json!({"id": id}))
            }
            "view_cells" => {
                let view = self.view(request)?;
                let (kind, cell_request, src) = cell_request(request)?;
                let seq = request.get("seq").and_then(Value::as_i64).unwrap_or(-1);
                let (out, id) = (Arc::clone(&self.out), view_id(request)?);
                let (controller, stop) = (Arc::clone(&view.controller), Arc::clone(&view.stop));
                thread::Builder::new()
                    .name("floe2-view-cells".into())
                    .spawn(move || {
                        let outcome = cell_ticket(&controller, &stop, cell_request)
                            .and_then(|wait| wait.wait(CELL_WAIT));
                        out.line(&json!({"event": "cells", "view": id,
                                         "result": cells_json(kind, seq, src, outcome)}));
                    })?;
                Ok(Value::Null)
            }
            "view_clip" => {
                let view = self.view(request)?;
                let dataset = view.controller.pin_dataset()?;
                let root = view.controller.snapshot().state.root.map(|r| r.cell);
                let bbox = request
                    .get("bbox")
                    .and_then(Value::as_array)
                    .filter(|a| a.len() == 4)
                    .and_then(|a| a.iter().map(Value::as_i64).collect::<Option<Vec<_>>>())
                    .ok_or_else(|| Error::input("bbox must be four integers"))?;
                let layers = match request.get("layers") {
                    None | Some(Value::Null) => Layers::All,
                    Some(Value::Array(a)) if a.is_empty() => Layers::All,
                    Some(Value::Array(a)) => {
                        Layers::Only(a.iter().map(pair).collect::<Result<_>>()?)
                    }
                    Some(_) => return Err(Error::input("layers must be a list")),
                };
                let cell_name = request
                    .get("cell_name")
                    .and_then(Value::as_str)
                    .unwrap_or("FLOE_CLIP")
                    .to_string();
                let out_path = PathBuf::from(
                    request
                        .get("out")
                        .and_then(Value::as_str)
                        .ok_or_else(|| Error::input("out must be a path"))?,
                );
                let seq = request.get("seq").and_then(Value::as_i64).unwrap_or(-1);
                let (out, id) = (Arc::clone(&self.out), view_id(request)?);
                thread::Builder::new()
                    .name("floe2-view-clip".into())
                    .spawn(move || {
                        let started = Instant::now();
                        let cancelled = Arc::new(AtomicUsize::new(0));
                        let result = (|| -> Result<(PathBuf, u64)> {
                            let Dataset::Layout(layout) = &dataset.dataset else {
                                return Err(Error::new(
                                    ErrorKind::Unsupported,
                                    "a jobdeck has no clip",
                                ));
                            };
                            let options = ClipOptions::local()?;
                            let request = ClipRequest {
                                bbox: [bbox[0], bbox[1], bbox[2], bbox[3]],
                                layers,
                                jobs: options.jobs,
                                cell_name,
                                root,
                            };
                            request.validate()?;
                            let output = artifact::output_path(&out_path, layout)?;
                            let mut clip =
                                clip::collect(layout, &request, &options, &cancelled, |_, _| {})?;
                            artifact::publish_reader(
                                &output,
                                &mut clip.file,
                                clip.size_bytes,
                                &cancelled,
                            )?;
                            Ok((output, clip.size_bytes))
                        })();
                        let result = match result {
                            Ok((path, size)) => json!({"kind": "clip", "seq": seq, "path": path,
                                "size_mb": size as f64 / 1e6,
                                "ms": started.elapsed().as_millis() as u64}),
                            Err(e) => json!({"kind": "error", "seq": seq,
                                "msg": format!("clip: {}", e.message)}),
                        };
                        out.line(&json!({"event": "clip", "view": id, "result": result}));
                    })?;
                Ok(Value::Null)
            }
            other => Err(Error::new(
                ErrorKind::Unsupported,
                format!("unknown request: {other}"),
            )),
        }
    }
    fn view(&self, request: &Value) -> Result<&View> {
        self.views
            .get(&view_id(request)?)
            .ok_or_else(|| Error::input("no such view"))
    }
    fn open(&mut self, request: &Value, cancelled: &AtomicUsize) -> Result<Value> {
        let source = PathBuf::from(
            request
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::input("source must be a string"))?,
        );
        let levels: Option<BTreeSet<i64>> = match request.get("levels") {
            None | Some(Value::Null) => None,
            Some(Value::Array(a)) => Some(
                a.iter()
                    .map(|v| v.as_i64().ok_or_else(|| Error::input("levels are numbers")))
                    .collect::<Result<_>>()?,
            ),
            Some(_) => return Err(Error::input("levels must be a list")),
        };
        let mode = match request.get("mode").and_then(Value::as_str) {
            Some(m) => Mode::parse(m)?,
            None => Mode::Level,
        };
        let width = dimension(request, "width")?;
        let height = dimension(request, "height")?;
        let frame_cache = request
            .get("frame_cache")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let margin = request
            .get("margin")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let options = RenderOptions::local()?;
        let resources = self.resources(&options)?;
        let dataset = ManagedDataset::open(&resources, &source, levels, mode, cancelled)?;
        let model = Model::new(&dataset)?;
        let mut state = ViewState::initial(&model, width, height)?;
        if let Some(p) = request.get("patch").filter(|p| !p.is_null()) {
            state = state.edit(&model, patch(p)?)?;
        }
        // the desktop's bounds from the first view on (DesktopPolicy::clamp)
        state.viewport = state.viewport.clamped(state.die(&model))?;
        let model_json = json!({
            "dbu": model.dbu,
            "bbox": model.bbox,
            "deck": model.deck,
            "skipped": model.skipped,
            "source_stale": model.source_stale,
        });
        let controller = Arc::new(ViewController::start_desktop(
            &resources,
            dataset,
            options,
            state,
            ControllerOptions {
                margin_prefetch: margin && frame_cache,
                frame_cache,
            },
            DesktopPolicy::desktop(),
        )?);
        self.next += 1;
        let id = self.next;
        let folder = std::env::temp_dir().join(format!("floe2-view-{}-{id}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&folder)?;
        let stop = Arc::new(AtomicBool::new(false));
        let pump = {
            let (controller, stop, folder, out) = (
                Arc::clone(&controller),
                Arc::clone(&stop),
                folder.clone(),
                Arc::clone(&self.out),
            );
            thread::Builder::new()
                .name(format!("floe2-view-{id}"))
                .spawn(move || pump(id, &controller, &stop, &folder, &out))?
        };
        let snapshot = snapshot_json(&controller.snapshot());
        self.views.insert(
            id,
            View {
                controller,
                stop,
                pump: Some(pump),
                folder,
            },
        );
        Ok(json!({"view": id, "snapshot": snapshot, "model": model_json}))
    }
}

fn view_id(request: &Value) -> Result<u64> {
    request
        .get("view")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::input("view must be a number"))
}

fn dimension(request: &Value, key: &str) -> Result<u32> {
    request
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 0)
        .ok_or_else(|| Error::input(format!("{key} must be a positive number")))
}

/// Watch one view: say what changed, hand over each new frame.
fn pump(id: u64, controller: &ViewController, stop: &AtomicBool, folder: &PathBuf, out: &Out) {
    let mut said: Option<Value> = None;
    let mut shown = (0u64, 0u64);
    let mut answered = (0u64, 0u64);
    let mut files: VecDeque<PathBuf> = VecDeque::new();
    while !stop.load(Ordering::Relaxed) {
        let snapshot = controller.snapshot();
        let ended = matches!(snapshot.phase, Phase::Closed | Phase::Failed);
        // frames first: a settled phase arrives with its frame shown
        for frame in [controller.latest(), controller.margin()]
            .into_iter()
            .flatten()
        {
            let last = match frame.purpose {
                Purpose::Foreground => &mut shown.0,
                Purpose::Margin => &mut shown.1,
            };
            if frame.id == *last || !frame.matches(&snapshot) {
                continue;
            }
            *last = frame.id;
            match publish(folder, &frame) {
                Ok(path) => {
                    files.push_back(path.clone());
                    while files.len() > KEPT_FRAMES {
                        let _ = std::fs::remove_file(files.pop_front().unwrap());
                    }
                    let mut json = frame_json(&frame, &path);
                    if let Some(report) = controller.frame_report(frame.id) {
                        // the perf line (P4b): the log line and tooltip, the
                        // lower bar's brief one
                        let note = frame
                            .frame
                            .request
                            .depth
                            .map_or(String::new(), |d| format!(", depth {d}"));
                        let (line, brief) = perf::perf_status(&report, &note);
                        json["perf"] = json!([line, brief]);
                        json["report"] = report;
                    }
                    out.line(&json!({"event": "frame", "view": id, "frame": json}));
                }
                Err(e) => out.line(&json!({"event": "frame_error", "view": id,
                                           "message": e.to_string()})),
            }
        }
        let queries = controller.query_snapshot();
        for (kind, result, last) in [
            ("snap", queries.snap, &mut answered.0),
            ("pick", queries.pick, &mut answered.1),
        ] {
            if let Some(r) = result.filter(|r| r.id != *last) {
                *last = r.id;
                out.line(&json!({"event": "query", "view": id, "result": query_json(kind, &r)}));
            }
        }
        let now = snapshot_json(&snapshot);
        if said.as_ref() != Some(&now) {
            out.line(&json!({"event": "view", "view": id, "snapshot": now}));
            said = Some(now);
        }
        if ended {
            break;
        }
        thread::sleep(PUMP);
    }
    out.line(&json!({"event": "closed", "view": id}));
}

/// The overview's base image for a depth and the die's place in it (P4e,
/// app-core `view::minimap` - the bake the web shows too): `bbox` given (a
/// view root's, its coordinates) the plain base of that die - the baked
/// frontiers are the top's -, else the top's with its frontier at `depth`
/// (none, or past the baked depths: the plain one). The viewer draws the
/// live view box on it itself.
fn minimap_json(controller: &ViewController, request: &Value) -> Result<Value> {
    let model = &controller.model;
    let root = request
        .get("bbox")
        .filter(|b| !b.is_null())
        .map(|b| f64s::<4>(b, "minimap bbox"))
        .transpose()?;
    let depth = match request.get("depth") {
        None | Some(Value::Null) => None,
        Some(d) => Some(
            d.as_u64()
                .and_then(|d| u32::try_from(d).ok())
                .ok_or_else(|| Error::input("minimap depth"))?,
        ),
    };
    let (overview, bbox, depth) = match root {
        Some(b) => (Arc::new(minimap::Minimap::plain(b)), b, None),
        None => (Arc::clone(&model.minimap), model.bbox, depth),
    };
    let key = overview
        .projection(bbox, controller.snapshot().state.viewport, depth)
        .base;
    let placement = minimap::placement(bbox);
    Ok(json!({
        "size": minimap::SIZE,
        "key": key,
        "base": overview.base(&key),
        "bbox": bbox,
        "die": placement.map(|(die, _)| die),
        "scale": placement.map(|(_, scale)| scale),
    }))
}

/// The frame's bytes as a file the viewer takes: written under a temporary
/// name, then renamed - never seen half-written.
fn publish(folder: &PathBuf, frame: &DisplayFrame) -> Result<PathBuf> {
    let ext = match frame.frame.request.format {
        FrameFormat::Raw => "raw",
        FrameFormat::Png => "png",
    };
    let path = folder.join(format!("frame-{}.{ext}", frame.id));
    let tmp = folder.join(format!(".frame-{}.tmp", frame.id));
    std::fs::write(&tmp, &frame.frame.bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn frame_json(frame: &DisplayFrame, path: &PathBuf) -> Value {
    let f = &frame.frame;
    json!({
        "id": frame.id,
        "purpose": match frame.purpose {
            Purpose::Foreground => "foreground",
            Purpose::Margin => "margin",
        },
        "state_rev": frame.state_rev,
        "render_rev": frame.render_rev,
        "render_key": frame.render_key,
        "generation": f.generation,
        "round": f.round,
        "final": f.final_frame,
        "partial": f.partial,
        "deferred": f.deferred,
        "labels_truncated": f.labels_truncated,
        "complete": f.complete(),
        "bbox": f.request.view,
        "depth": f.request.depth,
        "width": f.request.width,
        "height": f.request.height,
        "viewport": f.request.viewport.map(|(w, h)| [w, h]),
        "format": match f.request.format {
            FrameFormat::Raw => "raw",
            FrameFormat::Png => "png",
        },
        "path": path,
        "deck_skipped": frame.deck_skipped,
        "fields": f.fields.0,
    })
}

/// A snap or pick answer as floe/rust_render.py gave it (`id` is the
/// controller's query id; `seq` stays the viewer's to set).
fn query_json(kind: &str, r: &ViewQueryResult) -> Value {
    use floe_worker_client::QueryStatus;
    let mut o = Map::new();
    o.insert("kind".into(), json!(kind));
    o.insert("id".into(), json!(r.id));
    o.insert("found".into(), json!(false));
    // the world point and radius the controller asked (dbu, the root's)
    o.insert(
        "request".into(),
        json!({"x": r.reply.request.x, "y": r.reply.request.y, "r": r.reply.request.radius}),
    );
    if r.reply.status != QueryStatus::Ok {
        o.insert(
            "err".into(),
            json!(r
                .reply
                .error
                .clone()
                .unwrap_or_else(|| format!("{:?}", r.reply.status).to_lowercase())),
        );
    }
    match &r.reply.hit {
        Some(QueryHit::Snap(h)) => {
            o.insert("found".into(), json!(true));
            o.insert("x".into(), json!(h.x));
            o.insert("y".into(), json!(h.y));
            o.insert(
                "snap".into(),
                json!(match h.kind {
                    SnapKind::Vertex => "vertex",
                    SnapKind::Edge => "edge",
                }),
            );
        }
        Some(QueryHit::Pick(p)) => {
            o.insert("found".into(), json!(true));
            o.insert("count".into(), json!(p.count));
            o.insert("index".into(), json!(p.index));
            o.insert("layer".into(), json!(p.layer.0));
            o.insert("datatype".into(), json!(p.layer.1));
            o.insert("lname".into(), json!(p.layer_name));
            o.insert("cell".into(), json!(p.cell_name));
            o.insert("area".into(), json!(p.area));
            o.insert("bbox".into(), json!(p.bbox));
            o.insert(
                "points".into(),
                json!(p.points.iter().map(|(x, y)| [*x, *y]).collect::<Vec<_>>()),
            );
            o.insert("points_truncated".into(), json!(p.points_truncated));
        }
        None if kind == "snap" => {
            o.insert("x".into(), json!(0));
            o.insert("y".into(), json!(0));
            o.insert("snap".into(), json!(""));
        }
        None => {
            o.insert("count".into(), json!(0));
        }
    }
    Value::Object(o)
}

/// The viewer's cell-tree request (floe/rust_render.py's cell jobs): its
/// kind, the controller's request, and the source it names (echoed).
fn cell_request(r: &Value) -> Result<(&'static str, CellRequest, Option<i64>)> {
    let kind = r.get("kind").and_then(Value::as_str).unwrap_or("");
    let src = r.get("src").and_then(Value::as_i64);
    let source = || -> Result<usize> {
        usize::try_from(src.unwrap_or(0)).map_err(|_| Error::input("src must be a source"))
    };
    let cell = |k: &str| -> Result<Option<u32>> {
        match r.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .and_then(|c| u32::try_from(c).ok())
                .map(Some)
                .ok_or_else(|| Error::input(format!("{k} must be a cell"))),
        }
    };
    Ok(match kind {
        "cell_sources" => ("cell_sources", CellRequest::Sources, src),
        "cells" => (
            "cells",
            CellRequest::Children {
                source: source()?,
                cell: cell("cell")?,
            },
            src,
        ),
        "cell_find" => (
            "cell_find",
            CellRequest::Find {
                source: match src {
                    Some(s) if s >= 0 => Some(s as usize),
                    _ => None,
                },
                pattern: r
                    .get("pattern")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                limit: r.get("limit").and_then(Value::as_u64).unwrap_or(200) as usize,
            },
            src,
        ),
        "cell_bbox" => (
            "cell_bbox",
            CellRequest::Bbox {
                source: source()?,
                cell: cell("cell")?.ok_or_else(|| Error::input("cell_bbox needs a cell"))?,
                root: cell("root")?,
            },
            src,
        ),
        "cell_insts" => (
            "cell_insts",
            CellRequest::Insts {
                source: source()?,
                cell: cell("cell")?.ok_or_else(|| Error::input("cell_insts needs a cell"))?,
                view: f64s(r.get("box").unwrap_or(&Value::Null), "box")?,
                cap: r.get("cap").and_then(Value::as_u64).unwrap_or(4096) as usize,
                root: cell("root")?,
            },
            src,
        ),
        _ => return Err(Error::input("unknown cell query kind")),
    })
}

/// A cell-tree answer as floe/rust_render.py `_emit_cell_query` gave it.
/// How long a cell question may wait for its answer, and for a place in the
/// controller's queue (MAX_PENDING_CELLS) - a panel expanding rows quickly.
const CELL_WAIT: Duration = Duration::from_secs(60);

/// The cell question's ticket, taken when the controller can take it: the
/// view's worker still opening (renderd opening a big cache - main01 took
/// seconds, 2026-10-10: the panel's first question was refused as `view is
/// still opening` and the tree stayed at "loading...") is waited for as long
/// as it takes, a full queue up to CELL_WAIT; a closed view, or any other
/// refusal, is the answer (a `cells` event the panel shows).
fn cell_ticket(
    controller: &ViewController,
    stop: &AtomicBool,
    request: CellRequest,
) -> Result<floe_app_core::view::CellWait> {
    let started = std::time::Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(Error::new(ErrorKind::Worker, "the view closed"));
        }
        let opening = controller.snapshot().phase == Phase::Opening;
        match controller.cell_ticket(request.clone()) {
            Err(e) if e.kind == ErrorKind::Busy && (opening || started.elapsed() < CELL_WAIT) => {
                thread::sleep(Duration::from_millis(20));
            }
            other => return other,
        }
    }
}

fn cells_json(kind: &str, seq: i64, src: Option<i64>, outcome: Result<CellOutcome>) -> Value {
    let mut o = Map::new();
    o.insert("kind".into(), json!(kind));
    o.insert("seq".into(), json!(seq));
    let reply = match outcome {
        Ok(Ok(reply)) => reply,
        Ok(Err(failure)) => {
            o.insert("found".into(), json!(false));
            o.insert("code".into(), json!(failure.code.wire()));
            o.insert("err".into(), json!(failure.message));
            return Value::Object(o);
        }
        Err(e) => {
            o.insert("found".into(), json!(false));
            o.insert(
                "code".into(),
                json!(if e.kind == ErrorKind::Busy {
                    "superseded"
                } else {
                    "state"
                }),
            );
            o.insert("err".into(), json!(e.message));
            return Value::Object(o);
        }
    };
    o.insert("found".into(), json!(true));
    match reply {
        CellReply::Sources(sources) => {
            o.insert(
                "sources".into(),
                json!(sources
                    .iter()
                    .map(|s| json!({"src": s.source, "placements": s.placements, "path": s.path}))
                    .collect::<Vec<_>>()),
            );
        }
        CellReply::Children {
            source,
            cell,
            name,
            insts,
            height,
            unit,
            bbox,
            total,
            children,
        } => {
            o.insert("src".into(), json!(source));
            o.insert("cell".into(), json!(cell));
            o.insert("name".into(), json!(name));
            o.insert("insts".into(), json!(insts));
            o.insert("height".into(), json!(height));
            o.insert("unit".into(), json!(unit));
            o.insert("bbox".into(), json!(bbox));
            o.insert("total".into(), json!(total));
            o.insert(
                "children".into(),
                json!(children
                    .iter()
                    .map(
                        |c| json!({"cell": c.cell, "members": c.members, "leaf": c.leaf,
                                    "name": c.name})
                    )
                    .collect::<Vec<_>>()),
            );
        }
        CellReply::Find { total, matches } => {
            o.insert("src".into(), json!(src.unwrap_or(-1)));
            o.insert("total".into(), json!(total));
            o.insert(
                "matches".into(),
                json!(matches
                    .iter()
                    .map(
                        |m| json!({"src": m.source, "cell": m.cell, "insts": m.insts,
                                    "name": m.name})
                    )
                    .collect::<Vec<_>>()),
            );
        }
        CellReply::Bbox {
            source,
            cell,
            insts,
            approx,
            bbox,
        } => {
            o.insert("src".into(), json!(source));
            o.insert("cell".into(), json!(cell));
            o.insert("insts".into(), json!(insts));
            o.insert("approx".into(), json!(approx));
            o.insert("bbox".into(), json!(bbox));
        }
        CellReply::Insts {
            source,
            cell,
            more,
            visited,
            boxes,
        } => {
            o.insert("src".into(), json!(source));
            o.insert("cell".into(), json!(cell));
            o.insert("more".into(), json!(more));
            o.insert("visited".into(), json!(visited));
            o.insert("boxes".into(), json!(boxes));
        }
    }
    Value::Object(o)
}

fn detail_name(d: Detail) -> &'static str {
    match d {
        Detail::Exact => "exact",
        Detail::Low => "low",
        Detail::Medium => "medium",
        Detail::High => "high",
    }
}
fn thin_name(t: Thin) -> &'static str {
    match t {
        Thin::Auto => "auto",
        Thin::Keep => "keep",
        Thin::Cull => "cull",
    }
}
fn layers_json(layers: &Layers) -> Value {
    match layers {
        Layers::All => json!("all"),
        Layers::None => json!("none"),
        Layers::Only(pairs) => json!(pairs),
    }
}

pub fn snapshot_json(s: &Snapshot) -> Value {
    let st = &s.state;
    json!({
        "state_rev": s.state_rev,
        "render_rev": s.render_rev,
        "render_key": s.render_key,
        "phase": format!("{:?}", s.phase).to_lowercase(),
        "max_depth": s.max_depth,
        "submitted": s.submitted,
        "consumed": s.consumed,
        "discarded": s.discarded,
        "margin_enabled": s.margin_enabled,
        "margin_working": s.margin_working,
        "crop_hits": s.crop_hits,
        "margin": s.margin.map(|m| json!({
            "frame_id": m.frame_id, "origin_px": m.origin_px, "crop_safe": m.crop_safe})),
        "margin_failure": s.margin_failure.as_ref().map(|(_, m)| m),
        "failure": s.failure.as_ref().map(|(k, m)| json!({
            "kind": format!("{k:?}").to_lowercase(), "message": m})),
        "render_failure": s.render_failure.as_ref().map(|(rev, m)| json!({
            "render_rev": rev, "message": m})),
        "cancelled_rev": s.cancelled_rev,
        "state": {
            "viewport": {"bbox": st.viewport.bbox, "width": st.viewport.width,
                         "height": st.viewport.height},
            "depth": st.depth,
            "detail": detail_name(st.detail),
            "thin": thin_name(st.thin),
            "layers": layers_json(&st.layers),
            "layers_isolated": st.layers_isolated(),
            "frames": st.frames,
            "labels": st.labels,
            "font_px": st.font_px,
            "mono": st.mono,
            "density": st.density,
            "root": st.root.as_ref().map(|r| json!({
                "cell": r.cell, "name": r.name, "bbox": r.bbox})),
        },
    })
}

fn bad(what: &str) -> Error {
    Error::input(format!("patch: {what}"))
}

fn pair(v: &Value) -> Result<(u32, u32)> {
    let a = v
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or_else(|| bad("a layer is [layer, datatype]"))?;
    let n = |v: &Value| {
        v.as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| bad("a layer number"))
    };
    Ok((n(&a[0])?, n(&a[1])?))
}

fn fill(v: &Value) -> Result<Fill> {
    let kind = v
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("fill kind"))?;
    Ok(match kind {
        "solid" => Fill::Solid,
        "clear" => Fill::Clear,
        "speckle" => Fill::Speckle,
        "pattern" => {
            let rows = v
                .get("rows")
                .and_then(Value::as_array)
                .filter(|r| r.len() == 16)
                .ok_or_else(|| bad("16 pattern rows"))?;
            let mut out = [0u16; 16];
            for (o, r) in out.iter_mut().zip(rows) {
                *o = r
                    .as_u64()
                    .and_then(|r| u16::try_from(r).ok())
                    .ok_or_else(|| bad("a pattern row"))?;
            }
            Fill::Pattern(out)
        }
        _ => return Err(bad("fill kind")),
    })
}

fn f64s<const N: usize>(v: &Value, what: &str) -> Result<[f64; N]> {
    let a = v
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or_else(|| bad(what))?;
    let mut out = [0.; N];
    for (o, x) in out.iter_mut().zip(a) {
        *o = x.as_f64().ok_or_else(|| bad(what))?;
    }
    Ok(out)
}

fn navigation(v: &Value) -> Result<Navigation> {
    let kind = v
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("navigation kind"))?;
    let num = |k: &str| v.get(k).and_then(Value::as_f64).ok_or_else(|| bad(k));
    Ok(match kind {
        "fit" => Navigation::Fit,
        "minimap" => Navigation::Minimap {
            point: f64s(v.get("point").unwrap_or(&Value::Null), "minimap point")?,
        },
        "goto" => Navigation::Goto {
            center_um: f64s(v.get("center_um").unwrap_or(&Value::Null), "goto center")?,
            width_um: v.get("width_um").and_then(Value::as_f64),
        },
        "pan" => Navigation::Pan {
            x: num("x")?,
            y: num("y")?,
            snap: v.get("snap").and_then(Value::as_bool).unwrap_or(false),
        },
        "zoom" => Navigation::Zoom {
            factor: num("factor")?,
            anchor: f64s(v.get("anchor").unwrap_or(&Value::Null), "zoom anchor")?,
        },
        "band" => {
            let axes = v
                .get("axes")
                .and_then(Value::as_array)
                .filter(|a| a.len() == 2)
                .ok_or_else(|| bad("band axes"))?;
            Navigation::Band {
                start: f64s(v.get("start").unwrap_or(&Value::Null), "band start")?,
                end: f64s(v.get("end").unwrap_or(&Value::Null), "band end")?,
                axes: [
                    axes[0].as_bool().ok_or_else(|| bad("band axes"))?,
                    axes[1].as_bool().ok_or_else(|| bad("band axes"))?,
                ],
                outward: v.get("outward").and_then(Value::as_bool).unwrap_or(false),
            }
        }
        _ => return Err(bad("navigation kind")),
    })
}

/// The viewer's edit as the controller's Patch (the web's PatchDto names,
/// rust/web/src/view.rs on feature/webui, plus `density`).
pub fn patch(v: &Value) -> Result<Patch> {
    let empty = Map::new();
    let o = match v {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return Err(bad("an object")),
    };
    const KNOWN: &[&str] = &[
        "navigation",
        "pixels",
        "depth",
        "depth_step",
        "detail",
        "thin",
        "layers",
        "layer_change",
        "frames",
        "labels",
        "font_px",
        "mono",
        "density",
        "style_deltas",
        "root",
    ];
    if let Some(k) = o.keys().find(|k| !KNOWN.contains(&k.as_str())) {
        return Err(bad(&format!("unknown field {k}")));
    }
    let flag = |k: &str| -> Result<Option<bool>> {
        o.get(k)
            .map(|v| v.as_bool().ok_or_else(|| bad(k)))
            .transpose()
    };
    let mut p = Patch {
        navigation: o.get("navigation").map(navigation).transpose()?,
        frames: flag("frames")?,
        labels: flag("labels")?,
        mono: flag("mono")?,
        density: flag("density")?,
        ..Default::default()
    };
    if let Some(px) = o.get("pixels") {
        let [w, h] = f64s::<2>(px, "pixels")?;
        if w < 1. || h < 1. || w.fract() != 0. || h.fract() != 0. || w > 1e6 || h > 1e6 {
            return Err(bad("pixels"));
        }
        p.pixels = Some((w as u32, h as u32));
    }
    if let Some(d) = o.get("depth") {
        p.depth = Some(match d {
            Value::String(s) if s == "full" => Depth::Full,
            Value::Null => Depth::Full,
            v => Depth::Levels(
                v.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| bad("depth"))?,
            ),
        });
    }
    if let Some(step) = o.get("depth_step") {
        let step = step
            .as_i64()
            .filter(|s| *s == 1 || *s == -1)
            .ok_or_else(|| bad("depth_step is 1 or -1"))?;
        if p.depth.is_some() {
            return Err(bad("depth_step without depth"));
        }
        p.depth = Some(Depth::Step(step as i8));
    }
    if let Some(d) = o.get("detail") {
        p.detail = Some(match d.as_str() {
            Some("exact") => Detail::Exact,
            Some("low") => Detail::Low,
            Some("medium") => Detail::Medium,
            Some("high") => Detail::High,
            _ => return Err(bad("detail")),
        });
    }
    if let Some(t) = o.get("thin") {
        p.thin = Some(match t.as_str() {
            Some("auto") => Thin::Auto,
            Some("keep") => Thin::Keep,
            Some("cull") => Thin::Cull,
            _ => return Err(bad("thin")),
        });
    }
    if let Some(l) = o.get("layers") {
        p.layers = Some(match l {
            Value::String(s) if s == "all" => Layers::All,
            Value::String(s) if s == "none" => Layers::None,
            Value::Array(a) => Layers::Only(a.iter().map(pair).collect::<Result<_>>()?),
            _ => return Err(bad("layers")),
        });
    }
    if let Some(c) = o.get("layer_change") {
        let visible = c
            .get("visible")
            .and_then(Value::as_bool)
            .ok_or_else(|| bad("layer_change visible"))?;
        p.layer_change = Some((pair(c.get("pair").unwrap_or(&Value::Null))?, visible));
    }
    if let Some(f) = o.get("font_px") {
        p.font_px = Some(
            f.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| bad("font_px"))?,
        );
    }
    if let Some(deltas) = o.get("style_deltas") {
        let deltas = deltas.as_array().ok_or_else(|| bad("style_deltas"))?;
        for d in deltas {
            let color = match d.get("color").and_then(Value::as_str) {
                Some(c) if c.len() == 7 && c.starts_with('#') => {
                    Some(floe_app_core::styles::color(c).ok_or_else(|| bad("color"))?)
                }
                Some(_) => return Err(bad("color is #RRGGBB")),
                None => None,
            };
            p.style_deltas.push(StyleDelta {
                layer: pair(d.get("pair").unwrap_or(&Value::Null))?,
                color,
                fill: d.get("fill").map(fill).transpose()?,
                width: d
                    .get("width")
                    .map(|w| {
                        w.as_u64()
                            .and_then(|w| u8::try_from(w).ok())
                            .ok_or_else(|| bad("width"))
                    })
                    .transpose()?,
            });
        }
    }
    if let Some(r) = o.get("root") {
        p.root = Some(match r {
            Value::Null => RootEdit::Clear,
            r => RootEdit::Cell {
                source: r
                    .get("src")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| bad("root src"))? as usize,
                cell: r
                    .get("cell")
                    .and_then(Value::as_u64)
                    .and_then(|c| u32::try_from(c).ok())
                    .ok_or_else(|| bad("root cell"))?,
            },
        });
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_reads_the_web_names_and_refuses_the_unknown() {
        let p = patch(&json!({
            "navigation": {"kind": "pan", "x": 0.25, "y": -0.5, "snap": true},
            "depth": 3, "detail": "high", "thin": "keep", "density": true,
            "layers": [[1, 0], [2, 0]], "font_px": 18, "mono": false,
            "style_deltas": [{"pair": [1, 0], "color": "#ff0000",
                              "fill": {"kind": "speckle"}, "width": 2}],
        }))
        .unwrap();
        assert!(
            matches!(p.navigation, Some(Navigation::Pan { x, y, snap: true }) if x == 0.25 && y == -0.5)
        );
        assert!(matches!(p.depth, Some(Depth::Levels(3))));
        assert_eq!(p.density, Some(true));
        assert_eq!(p.layers, Some(Layers::Only(vec![(1, 0), (2, 0)])));
        assert_eq!(p.style_deltas.len(), 1);
        assert_eq!(p.style_deltas[0].color, Some([255, 0, 0, 255]));
        assert!(patch(&json!({"zoomies": 1})).is_err());
        assert!(patch(&json!({"depth_step": 2})).is_err());
        assert!(matches!(
            patch(&json!({"depth": "full"})).unwrap().depth,
            Some(Depth::Full)
        ));
        assert!(matches!(
            patch(&json!({"root": null})).unwrap().root,
            Some(RootEdit::Clear)
        ));
    }
}
