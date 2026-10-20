//! Synchronous application API for the CLI and dedicated view-control thread.
//! No raster implementation lives here; only policy and worker lifecycle.
mod diagnostics;
use crate::{
    artifact, check_cancelled,
    dataset::Dataset,
    native::Discovery,
    shots::{Shot, MAX_PIXELS},
    Error, ErrorKind, Result,
};
use floe_worker_client::{Config, Event, Frame, FrameFormat, RenderRequest, Source, WorkerClient};
use std::path::PathBuf;
use std::sync::{atomic::AtomicUsize, Arc};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub binary: PathBuf,
    pub budget_mb: u64,
    pub decode_jobs: u16,
    pub raster_jobs: u16,
    pub tile_px: u32,
    pub round_pages: u32,
    pub open_timeout_s: u64,
    pub label_font_px: u32,
    pub raw: bool,
    /// Opt-in local numeric diagnostics. Never relay raw worker lines or paths.
    pub debug: bool,
}
pub(crate) fn env_number(name: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(default);
    };
    let n = value
        .to_str()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n >= min && *n <= max)
        .ok_or_else(|| Error::input(format!("{name} must be in {min}..{max}")))?;
    Ok(n)
}
impl RenderOptions {
    pub fn local() -> Result<Self> {
        let jobs = std::thread::available_parallelism().map_or(1, |n| n.get().min(8) as u64);
        let jobs = env_number("FLOE_RUST_JOBS", jobs, 1, 256)?;
        Ok(Self {
            binary: Discovery::renderer()?.renderd_path()?,
            budget_mb: env_number("FLOE_RUST_BUDGET_MB", 1024, 1, 1 << 20)?,
            decode_jobs: jobs as u16,
            raster_jobs: env_number("FLOE_RUST_RASTER_JOBS", jobs.min(4), 1, 256)? as u16,
            tile_px: env_number("FLOE_RUST_TILE_PX", 384, 1, 4096)? as u32,
            round_pages: env_number("FLOE_RUST_ROUND_PAGES", 1 << 30, 1, 1 << 30)? as u32,
            open_timeout_s: env_number("FLOE_RUST_OPEN_TIMEOUT_S", 300, 1, 86400)?,
            label_font_px: env_number("FLOE_RUST_LABEL_PX", 14, 6, 96)? as u32,
            raw: std::env::var_os("FLOE_RUST_RAW_FRAME").is_none_or(|s| s != "off"),
            debug: false,
        })
    }
}
pub struct RenderSession {
    // Drop/reap worker before releasing pins, even if the caller drops Dataset.
    worker: WorkerClient,
    _layout_pin: Option<crate::cache::revision::Snapshot>,
    _deck_pin: Option<Arc<crate::cache::revision::set::Snapshot>>,
    max_depth: u64,
    options: RenderOptions,
    cancelled: Arc<AtomicUsize>,
}
impl RenderSession {
    pub fn open(
        dataset: &Dataset,
        options: RenderOptions,
        archival: bool,
        cancelled: Arc<AtomicUsize>,
    ) -> Result<Self> {
        check_cancelled(&cancelled)?;
        let styles = dataset.styles(archival)?;
        let mut config = Config::new(&options.binary);
        config.open_timeout = Duration::from_secs(options.open_timeout_s);
        config.max_pixels = MAX_PIXELS;
        config.shutdown_requested = Some(Arc::clone(&cancelled));
        let mut worker = WorkerClient::spawn(config)?;
        let source = match dataset {
            Dataset::Layout(layout) => Source::Layout(layout.directory.clone()),
            Dataset::Deck(deck) => {
                let path = worker.work_dir().join("deck.spec");
                artifact::publish(&path, deck.spec.text.as_bytes(), &cancelled)?;
                Source::Deck(path)
            }
        };
        let opened = worker.open(source, options.budget_mb, options.decode_jobs)?;
        // Native layout replies use DBU/um; deck replies use um/deck-DBU.
        // Preserve this existing wire distinction without changing coordinates.
        let unit_ratio = if dataset.is_deck() {
            opened.unit / dataset.dbu()
        } else {
            opened.unit * dataset.dbu()
        };
        if (unit_ratio - 1.).abs() > 1e-12 {
            return Err(Error::new(
                ErrorKind::Cache,
                "metadata and worker units differ",
            ));
        }
        worker.set_styles(&styles)?;
        Ok(Self {
            worker,
            _layout_pin: match dataset {
                Dataset::Layout(layout) => layout.revision_pin.clone(),
                _ => None,
            },
            _deck_pin: match dataset {
                Dataset::Deck(deck) => deck.analysis.catalog.pinned.clone(),
                _ => None,
            },
            max_depth: opened.max_depth,
            options,
            cancelled,
        })
    }
    pub fn pid(&self) -> Option<u32> {
        self.worker.pid()
    }
    pub fn max_depth(&self) -> u64 {
        self.max_depth
    }
    pub fn submit(&mut self, request: RenderRequest) -> Result<u64> {
        self.worker.render(request).map_err(Into::into)
    }
    pub fn cancel(&mut self) -> Result<u64> {
        self.worker.cancel().map_err(Into::into)
    }
    pub fn pending_generations(&self) -> usize {
        self.worker.pending_generations()
    }
    /// floe_worker_client::WorkerClient::set_timeouts.
    pub fn set_timeouts(&mut self, render: Duration, query: Duration) {
        self.worker.set_timeouts(render, query)
    }
    /// floe_worker_client::WorkerClient::set_incomplete_queries.
    pub fn set_incomplete_queries(&mut self, on: bool) {
        self.worker.set_incomplete_queries(on)
    }
    pub fn query(&mut self, request: floe_worker_client::QueryRequest) -> Result<u64> {
        self.worker.query(request).map_err(Into::into)
    }
    pub fn cancel_queries(&mut self, kind: floe_worker_client::QueryKind) -> Result<u64> {
        self.worker.cancel_queries(kind).map_err(Into::into)
    }
    pub fn pending_queries(&self) -> usize {
        self.worker.pending_queries()
    }
    pub fn cell_query(&mut self, request: floe_worker_client::CellRequest) -> Result<u64> {
        self.worker.cell_query(request).map_err(Into::into)
    }
    pub fn pending_cell_queries(&self) -> usize {
        self.worker.pending_cell_queries()
    }
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<Event>> {
        let event = self.worker.poll(timeout)?;
        if self.options.debug {
            if let Some(Event::Frame(frame)) = &event {
                use std::io::Write;
                // Explicit debug only: a slow stderr reader can delay polling.
                // Broken diagnostics must not turn a good render into an error.
                let _ = std::io::stderr()
                    .lock()
                    .write_all(diagnostics::frame_line(self.worker.pid(), frame).as_bytes());
            }
        }
        Ok(event)
    }
    pub fn set_styles(&mut self, styles: &[floe_worker_client::Style]) -> Result<()> {
        self.worker
            .set_styles(styles)
            .map(|_| ())
            .map_err(Into::into)
    }
    pub fn close(&mut self) -> Result<()> {
        self.worker.close().map_err(Into::into)
    }

    pub fn capture(&mut self, request: RenderRequest) -> Result<Frame> {
        if self.pending_queries() != 0 {
            return Err(Error::new(
                ErrorKind::Busy,
                "drain queries before synchronous capture",
            ));
        }
        let result = self.capture_inner(request);
        if result.is_err() {
            let _ = self.close();
        }
        result
    }
    fn capture_inner(&mut self, request: RenderRequest) -> Result<Frame> {
        check_cancelled(&self.cancelled)?;
        let cut_px = request.cut_px;
        let generation = self.worker.render(request)?;
        loop {
            check_cancelled(&self.cancelled)?;
            match self.poll(Duration::from_millis(20))? {
                Some(Event::Frame(frame))
                    if frame.generation == generation && frame.final_frame =>
                {
                    return Ok(frame)
                }
                Some(Event::Failed { code, message, .. }) => {
                    return Err(Error::new(
                        ErrorKind::Worker,
                        failure_text(&code, &message, cut_px),
                    ))
                }
                Some(Event::Cancelled { generation: gen }) if gen == generation => {
                    return Err(Error::new(ErrorKind::Cancelled, "render cancelled"))
                }
                _ => (),
            }
        }
    }
    pub fn shot_request(&self, dataset: &Dataset, shot: &Shot) -> Result<RenderRequest> {
        if dataset.is_deck() && shot.labels {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "jobdeck labels are not supported",
            ));
        }
        let (box_um, w, h) = shot.fitted(dataset.bbox_um())?;
        let view = box_um.map(|v| v / dataset.dbu());
        if !view.iter().all(|v| v.is_finite()) {
            return Err(Error::input("DBU conversion overflow"));
        }
        Ok(RenderRequest {
            view,
            width: w,
            height: h,
            depth: shot.depth,
            cut_px: shot.detail.cut_px(),
            layers: dataset.resolve_layers(shot.layers.as_deref())?,
            frames: shot.frames,
            labels: shot.labels,
            font_px: shot.font_px,
            thin: shot.thin.effective(dataset.is_deck()),
            // a jobdeck's frames draw no density (renderd's deck pass): off
            density: Some(!dataset.is_deck() && shot.density_on()),
            // the occupancy summary as asked (--occupancy; None = renderd's rule)
            occupancy: shot.occupancy,
            format: FrameFormat::Png,
            ..self.base_request()
        })
    }
    pub fn base_request(&self) -> RenderRequest {
        RenderRequest {
            decode_jobs: self.options.decode_jobs,
            raster_jobs: self.options.raster_jobs,
            round_pages: self.options.round_pages,
            tile_px: self.options.tile_px,
            font_px: self.options.label_font_px,
            format: if self.options.raw {
                FrameFormat::Raw
            } else {
                FrameFormat::Png
            },
            ..Default::default()
        }
    }
}
/// A worker's failure as a capture's error. renderd's `decoded generation
/// budget exceeded: NEXT > BUDGET bytes` (its wire turns spaces into `_`) is
/// said in words, with the ways out (field 2026-10-12, a whole-chip capture:
/// `render: decoded_generation_budget_exceeded:_3967202112_>_1073741824_bytes`).
/// A frame at cut 0 - `--detail exact`, the capture's default - is every
/// page or none: an export must not come out short of shapes, where a cut
/// lets the planner thin the frame to the budget (renderd run_render_attempt).
fn failure_text(code: &str, message: &str, cut_px: f64) -> String {
    let Some((need, budget)) = budget_exceeded(message) else {
        return format!("{code}: {message}");
    };
    let size = |bytes: u64| match bytes >> 30 {
        0 => format!("{} MiB", bytes.div_ceil(1 << 20)),
        _ => format!("{:.2} GiB", bytes as f64 / f64::from(1u32 << 30)),
    };
    // the charge when the pages passed the budget: the least the frame needs;
    // a quarter more, in 512 MB steps
    let suggest = (need.saturating_add(need / 4) >> 20)
        .div_ceil(512)
        .saturating_mul(512)
        .max(1024);
    let ways = if cut_px == 0.0 {
        "--detail exact (no size cut) draws every shape or fails: use --detail high \
         (the viewer's 1 px cut, thinned to fit the budget when needed), a smaller \
         region or fewer --layers, or"
    } else {
        "use a smaller region or fewer --layers, or"
    };
    format!(
        "{code}: decoded generation budget exceeded - the region needs at least {} \
         of decoded shapes, the render budget is {}; {ways} \
         FLOE_RUST_BUDGET_MB={suggest} if this host has the memory",
        size(need),
        size(budget)
    )
}

/// renderd checked_generation_bytes's message off the wire: (charge, budget).
fn budget_exceeded(message: &str) -> Option<(u64, u64)> {
    let rest = message.strip_prefix("decoded_generation_budget_exceeded:_")?;
    let (need, rest) = rest.split_once("_>_")?;
    let budget = rest.strip_suffix("_bytes")?;
    Some((need.parse().ok()?, budget.parse().ok()?))
}

/// What a frame's budget fit did to it - renderd's `fit_*` frame fields, the
/// viewer's `... to fit budget` - or None when it drew the plan as asked. A
/// capture thinned to the budget says so: `--detail high` is the way past an
/// exact capture's budget error (failure_text), and on the synthetic MAIN01
/// 1/10 its whole chip at 1 GiB drew 206 of the 449 layers, with nothing
/// said (2026-10-12).
pub fn budget_fit(frame: &Frame) -> Option<serde_json::Value> {
    const KEYS: [&str; 12] = [
        "fit_pct",
        "fit_thin",
        "fit_full_pct",
        "fit_none_pct",
        "fit_cull",
        "fit_over",
        "fit_ranked",
        "fit_layers_whole",
        "fit_layer_edge",
        "fit_layers_out",
        "fit_scale",
        "fit_refits",
    ];
    let set = |key: &str| frame.fields.get(key).is_some_and(|v| !v.is_empty() && v != "0");
    if !["fit_pct", "fit_thin", "fit_cull", "fit_over"].iter().any(|k| set(k)) {
        return None;
    }
    let mut fit = serde_json::Map::new();
    for key in KEYS {
        if let Some(value) = frame.fields.get(key) {
            let value = value
                .parse::<u64>()
                .map_or_else(|_| serde_json::Value::from(value), serde_json::Value::from);
            fit.insert(key.to_string(), value);
        }
    }
    Some(fit.into())
}

/// budget_fit in words, as the viewer's status says it (view::perf):
/// `206 layer(s) whole, 243 layer(s) left out`. With the density under the
/// cut on, the layers the fit leaves out of its shapes are still in the
/// density (pass 2 keeps its own order: on the routing chip's four metal
/// layers under 160 MB, the three left out drew 36,146 density pixels where
/// they had none, 2026-10-12): `as density only`.
pub fn budget_fit_brief(fit: &serde_json::Value, density: bool) -> String {
    let n = |key: &str| fit.get(key).and_then(serde_json::Value::as_u64).unwrap_or(0);
    let times = |pct: u64| format!("{:.2}", pct as f64 / 100.0);
    let mut parts = Vec::new();
    if n("fit_pct") > 100 {
        parts.push(format!("cut x{}", times(n("fit_pct"))));
    }
    // the size classes the fit thinned: one shape in 2^thin below `full`
    // times the cut, none below `none` times it
    let mut classes = Vec::new();
    if n("fit_thin") != 0 {
        classes.push(format!(
            "1 in {} shapes{}",
            1u64 << n("fit_thin").min(30),
            match n("fit_full_pct") {
                0 => String::new(),
                full => format!(" under x{} the cut", times(full)),
            }
        ));
    }
    if n("fit_none_pct") != 0 {
        classes.push(format!("none under x{} the cut", times(n("fit_none_pct"))));
    }
    if n("fit_ranked") != 0 {
        // top planes first: these layers whole, one at the edge thinned, the
        // rest left out
        if n("fit_layers_whole") != 0 {
            parts.push(format!("{} layer(s) whole", n("fit_layers_whole")));
        }
        let edge = fit.get("fit_layer_edge").and_then(serde_json::Value::as_str);
        if let Some(edge) = edge.filter(|e| !e.is_empty() && *e != "-") {
            parts.push(if classes.is_empty() {
                format!("layer {edge} thinned")
            } else {
                format!("layer {edge} thinned ({})", classes.join(", "))
            });
        }
        if n("fit_layers_out") != 0 {
            parts.push(format!(
                "{} layer(s) {}",
                n("fit_layers_out"),
                if density { "as density only" } else { "left out" }
            ));
        }
    } else {
        parts.extend(classes);
    }
    if n("fit_cull") != 0 {
        parts.push("hairlines culled".to_string());
    }
    if n("fit_over") != 0 {
        parts.push("still over".to_string());
    }
    if parts.is_empty() {
        parts.push("fitted".to_string());
    }
    parts.join(", ")
}

pub fn require_complete(frame: &Frame) -> Result<()> {
    if !frame.complete() {
        return Err(Error::new(ErrorKind::Incomplete,
            format!("incomplete frame (final={}, partial={}, deferred={}, labels_truncated={}); output not replaced",
                frame.final_frame, frame.partial, frame.deferred, frame.labels_truncated)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_failure_is_said_with_the_ways_out() {
        let wire = "decoded_generation_budget_exceeded:_3967202112_>_1073741824_bytes";
        let exact = failure_text("render", wire, 0.0);
        assert!(
            exact.starts_with(
                "render: decoded generation budget exceeded - the region needs at least 3.69 GiB"
            ),
            "{exact}"
        );
        assert!(exact.contains("the render budget is 1.00 GiB"), "{exact}");
        assert!(exact.contains("--detail high"), "{exact}");
        assert!(exact.ends_with("FLOE_RUST_BUDGET_MB=5120 if this host has the memory"), "{exact}");
        let cut = failure_text("render", wire, 1.0);
        assert!(!cut.contains("--detail") && cut.contains("FLOE_RUST_BUDGET_MB=5120"), "{cut}");
        // a small charge still suggests no less than the default budget
        let small = failure_text("render", "decoded_generation_budget_exceeded:_49297004_>_1048576_bytes", 0.0);
        assert!(
            small.contains("needs at least 48 MiB of decoded shapes, the render budget is 1 MiB;")
                && small.contains("FLOE_RUST_BUDGET_MB=1024"),
            "{small}"
        );
        // anything else as it came
        assert_eq!(failure_text("open", "no_such_cache", 0.0), "open: no_such_cache");
        let odd = "decoded_generation_budget_exceeded:_x_>_1_bytes";
        assert_eq!(failure_text("render", odd, 0.0), format!("render: {odd}"));
    }

    #[test]
    fn a_thinned_frame_says_how() {
        let frame = |pairs: &[(&str, &str)]| Frame {
            generation: 1,
            round: 0,
            final_frame: true,
            partial: false,
            deferred: 0,
            labels_truncated: false,
            request: RenderRequest::default(),
            bytes: Vec::new(),
            fields: floe_worker_client::Fields(
                pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ),
        };
        let asked = frame(&[("fit_pct", "0"), ("fit_thin", "0"), ("fit_cull", "0"), ("fit_over", "0")]);
        assert!(budget_fit(&asked).is_none());
        let thinned = frame(&[
            ("fit_pct", "0"),
            ("fit_thin", "2"),
            ("fit_full_pct", "250"),
            ("fit_cull", "0"),
            ("fit_over", "0"),
            ("fit_layer_edge", "M1"),
            ("pages", "7"),
        ]);
        let fit = budget_fit(&thinned).unwrap();
        assert_eq!(fit["fit_thin"], 2);
        assert_eq!(fit["fit_layer_edge"], "M1");
        assert!(fit.get("pages").is_none());
        assert_eq!(budget_fit_brief(&fit, false), "1 in 4 shapes under x2.50 the cut");
        let ranked = frame(&[
            ("fit_pct", "100"),
            ("fit_ranked", "1"),
            ("fit_layers_whole", "38"),
            ("fit_layer_edge", "103/2"),
            ("fit_thin", "1"),
            ("fit_full_pct", "216"),
            ("fit_none_pct", "108"),
            ("fit_layers_out", "410"),
        ]);
        assert_eq!(
            budget_fit_brief(&budget_fit(&ranked).unwrap(), false),
            "38 layer(s) whole, layer 103/2 thinned (1 in 2 shapes under x2.16 the cut, \
             none under x1.08 the cut), 410 layer(s) left out"
        );
        let out = frame(&[
            ("fit_pct", "100"),
            ("fit_ranked", "1"),
            ("fit_layers_whole", "206"),
            ("fit_layer_edge", "-"),
            ("fit_layers_out", "243"),
        ]);
        assert_eq!(
            budget_fit_brief(&budget_fit(&out).unwrap(), false),
            "206 layer(s) whole, 243 layer(s) left out"
        );
        assert_eq!(
            budget_fit_brief(&budget_fit(&out).unwrap(), true),
            "206 layer(s) whole, 243 layer(s) as density only"
        );
        let cut = frame(&[("fit_pct", "141"), ("fit_cull", "1")]);
        assert_eq!(budget_fit_brief(&budget_fit(&cut).unwrap(), false), "cut x1.41, hairlines culled");
    }
}
