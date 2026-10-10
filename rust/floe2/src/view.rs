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
//!
//! Events, between replies, one JSON object per line:
//!   {"event": "view", "view": N, "snapshot": {...}}   the state, phase,
//!       counters or a failure changed
//!   {"event": "frame", "view": N, "frame": {...}}     a frame to show: its
//!       pixels in `path` (FLOERAW1 header + RGBA, or a PNG) - the viewer
//!       reads and removes it; the service removes the files it is not given
//!       time for
//!   {"event": "closed", "view": N}
use floe_app_core::{
    jobdeck::color::Mode,
    managed::{Limits, ManagedDataset, Resources},
    render::RenderOptions,
    shots::{Detail, Thin},
    view::{
        ControllerOptions, Depth, DesktopPolicy, DisplayFrame, Model, Navigation, Patch, Phase,
        Purpose, RootEdit, Snapshot, StyleDelta, ViewController, ViewState,
    },
    Error, ErrorKind, Result,
};
use floe_worker_client::{Fill, FrameFormat, Layers};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

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
            "view_close" => {
                let id = view_id(request)?;
                self.views.remove(&id);
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
                    out.line(&json!({"event": "frame", "view": id,
                                     "frame": frame_json(&frame, &path)}));
                }
                Err(e) => out.line(&json!({"event": "frame_error", "view": id,
                                           "message": e.to_string()})),
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
