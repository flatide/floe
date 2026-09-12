//! Synchronous controller API for CLI and a future bounded server controller.
//! No raster implementation lives here; only policy and worker lifecycle.
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
}
fn env_number(name: &str, default: u64, min: u64, max: u64) -> Result<u64> {
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
        })
    }
}
pub struct RenderSession {
    worker: WorkerClient,
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
            options,
            cancelled,
        })
    }
    pub fn pid(&self) -> Option<u32> {
        self.worker.pid()
    }
    pub fn close(&mut self) -> Result<()> {
        self.worker.close().map_err(Into::into)
    }

    pub fn capture(&mut self, request: RenderRequest) -> Result<Frame> {
        let result = self.capture_inner(request);
        if result.is_err() {
            let _ = self.close();
        }
        result
    }
    fn capture_inner(&mut self, request: RenderRequest) -> Result<Frame> {
        check_cancelled(&self.cancelled)?;
        let generation = self.worker.render(request)?;
        loop {
            check_cancelled(&self.cancelled)?;
            match self.worker.poll(Duration::from_millis(20))? {
                Some(Event::Frame(frame))
                    if frame.generation == generation && frame.final_frame =>
                {
                    return Ok(frame)
                }
                Some(Event::Failed { code, message, .. }) => {
                    return Err(Error::new(ErrorKind::Worker, format!("{code}: {message}")))
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
pub fn require_complete(frame: &Frame) -> Result<()> {
    if !frame.complete() {
        return Err(Error::new(ErrorKind::Incomplete,
            format!("incomplete frame (final={}, partial={}, deferred={}, labels_truncated={}); output not replaced",
                frame.final_frame, frame.partial, frame.deferred, frame.labels_truncated)));
    }
    Ok(())
}
