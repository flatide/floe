//! Exact layout export, independent of viewport, styles, cut, LOD and summary.
use crate::{
    artifact, catalog::Layout, check_cancelled, native::Discovery, render::env_number, Error,
    ErrorKind, Result,
};
use floe_worker_client::{ClipArtifact, ClipRequest, Config, Fields, Layers, Source, WorkerClient};
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicUsize, Arc};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ClipOptions {
    pub binary: PathBuf,
    pub budget_mb: u64,
    pub jobs: u16,
    pub open_timeout_s: u64,
    pub clip_timeout_s: u64,
}
impl ClipOptions {
    pub fn local() -> Result<Self> {
        let jobs = std::thread::available_parallelism().map_or(1, |n| n.get().min(8) as u64);
        Ok(Self {
            binary: Discovery::renderer()?.renderd_path()?,
            budget_mb: env_number("FLOE_RUST_BUDGET_MB", 1024, 1, 1 << 20)?,
            jobs: env_number("FLOE_RUST_JOBS", jobs, 1, 256)? as u16,
            open_timeout_s: env_number("FLOE_RUST_OPEN_TIMEOUT_S", 300, 1, 86400)?,
            clip_timeout_s: env_number("FLOE_RUST_CLIP_TIMEOUT_S", 300, 1, 86400)?,
        })
    }
}

/// Python clip's nearest DBU (ties-even), including reversed corners. Reject
/// overflow explicitly instead of Rust's saturating float-to-integer cast.
pub fn bbox_dbu(bbox: [f64; 4], dbu: f64) -> Result<[i64; 4]> {
    if !dbu.is_finite() || dbu <= 0. {
        return Err(Error::input("invalid clip DBU"));
    }
    let mut b = [0; 4];
    for (out, um) in b.iter_mut().zip(bbox) {
        let rounded = (um / dbu).round_ties_even();
        if !rounded.is_finite() || rounded < i64::MIN as f64 || rounded >= -(i64::MIN as f64) {
            return Err(Error::input(
                "clip coordinate is outside signed 64-bit DBU range",
            ));
        }
        *out = rounded as i64;
    }
    if b[0] > b[2] {
        b.swap(0, 2);
    }
    if b[1] > b[3] {
        b.swap(1, 3);
    }
    if b[0] == b[2] || b[1] == b[3] {
        return Err(Error::input(
            "--bbox has zero width or height after DBU rounding",
        ));
    }
    Ok(b)
}

pub struct ClipReport {
    pub output: PathBuf,
    pub size_bytes: u64,
    pub fields: Fields,
}
pub fn export(
    layout: &Layout,
    bbox_um: [f64; 4],
    layers: Option<&str>,
    cell_name: &str,
    output: &Path,
    options: &ClipOptions,
    cancelled: &Arc<AtomicUsize>,
) -> Result<ClipReport> {
    check_cancelled(cancelled)?;
    let request = ClipRequest {
        bbox: bbox_dbu(bbox_um, layout.metadata.dbu)?,
        // Legacy clip shares the query layer contract: an empty token list
        // means all, unlike render's explicit empty selection. Keep CLI parity;
        // the lower-level ClipRequest can still express Layers::None.
        layers: match layout.resolve_layers(layers)? {
            Layers::None => Layers::All,
            selected => selected,
        },
        jobs: options.jobs,
        cell_name: cell_name.into(),
    };
    request.validate()?;
    let output = artifact::output_path(output, layout)?;
    let mut clip = collect(layout, &request, options, cancelled, |_, _| {})?;
    // Recheck the destination after a potentially long export. The worker
    // wrote only to its private slot and is already reaped before publication.
    let output = artifact::output_path(&output, layout)?;
    artifact::publish_reader(&output, &mut clip.file, clip.size_bytes, cancelled)?;
    Ok(ClipReport {
        output,
        size_bytes: clip.size_bytes,
        fields: clip.fields,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectPhase {
    Opening,
    Clipping,
    Closing,
}
/// Return a validated, unlinked descriptor only after worker reap. This is
/// shared by the CLI publisher and managed artifact ownership; it does not
/// reinterpret Layers::None as All or round already-integer DBU coordinates.
pub fn collect(
    layout: &Layout,
    request: &ClipRequest,
    options: &ClipOptions,
    cancelled: &Arc<AtomicUsize>,
    mut progress: impl FnMut(CollectPhase, Option<u32>),
) -> Result<ClipArtifact> {
    check_cancelled(cancelled)?;
    request.validate()?;
    if request.jobs != options.jobs
        || options.budget_mb == 0
        || !(1..=86400).contains(&options.open_timeout_s)
        || !(1..=86400).contains(&options.clip_timeout_s)
    {
        return Err(Error::input("invalid clip collection options"));
    }
    let mut config = Config::new(&options.binary);
    config.open_timeout = Duration::from_secs(options.open_timeout_s);
    config.clip_timeout = Duration::from_secs(options.clip_timeout_s);
    config.shutdown_requested = Some(Arc::clone(cancelled));
    progress(CollectPhase::Opening, None);
    let mut worker = WorkerClient::spawn(config)?;
    progress(CollectPhase::Opening, worker.pid());
    let opened = worker.open(
        Source::Layout(layout.directory.clone()),
        options.budget_mb,
        options.jobs,
    )?;
    if (opened.unit * layout.metadata.dbu - 1.).abs() > 1e-12 {
        return Err(Error::new(
            ErrorKind::Cache,
            "metadata and worker units differ",
        ));
    }
    progress(CollectPhase::Clipping, worker.pid());
    let result = worker.clip(request);
    progress(CollectPhase::Closing, worker.pid());
    let closed = worker.close();
    let clip = result?;
    closed?;
    check_cancelled(cancelled)?;
    Ok(clip)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rounding_normalization_and_range() {
        assert_eq!(bbox_dbu([2.5, 3.5, -1.5, -0.5], 1.).unwrap(), [-2, 0, 2, 4]);
        assert_eq!(
            bbox_dbu([-0.0005, -0.0015, 0.0025, 0.0035], 0.001).unwrap(),
            [0, -2, 2, 4]
        );
        assert!(bbox_dbu([0., 0., 0.5, 1.], 1.).is_err());
        for n in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -(i64::MIN as f64),
            -1e30,
        ] {
            assert!(bbox_dbu([n, 0., 1., 1.], 1.).is_err());
        }
        assert_eq!(
            bbox_dbu([i64::MIN as f64, 0., 1., 1.], 1.).unwrap()[0],
            i64::MIN
        );
        assert!(bbox_dbu([1., 1., 2., 2.], 0.).is_err());
    }
}
