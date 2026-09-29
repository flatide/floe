//! Website demo operator entry point. No browser-controlled paths or indexing.
use floe_app_core::{
    check_cancelled,
    index::IndexOptions,
    jobdeck::color::Mode,
    managed::{Limits, ManagedDataset, Resources},
    managed_index::{ManagedIndex, Phase},
    native::{Discovery, Indexer},
    render::RenderOptions,
    server::{secret::read_proxy_key, Config, Deployment, ValidatedConfig},
    Error, ErrorKind, Result,
};
use floe_web::broker::{self, runtime::Runtime, Broker};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

fn policy(path: &Path) -> Result<ValidatedConfig> {
    let config = Config::read(path)?.validate()?;
    if !matches!(config.config().deployment, Deployment::PublicDemo { .. })
        || config.config().max_sessions > 4
    {
        return Err(Error::input(
            "demo commands require public_demo and max_sessions 1..4",
        ));
    }
    Ok(config)
}
fn samples(config: &ValidatedConfig) -> Vec<String> {
    match &config.config().deployment {
        Deployment::PublicDemo { samples, .. } => samples.iter().map(|s| s.id.clone()).collect(),
        _ => unreachable!("validated demo policy"),
    }
}
fn resources() -> Result<Arc<Resources>> {
    Resources::new(Limits {
        cpu_slots: 16,
        foreground_reserve: 4,
        workers: 4,
        decoded_mb: 1024,
    })
}
pub(super) fn prepare(path: &Path, jobs: usize, lod: bool, stop: &Arc<AtomicUsize>) -> Result<()> {
    let config = policy(path)?;
    let resources = resources()?;
    let indexer = Indexer::discover(&Discovery::local()?)?;
    // Every configured source is scope-checked before the first explicit write.
    let sources = samples(&config)
        .into_iter()
        .map(|id| config.register_source(&id, stop).map(|source| (id, source)))
        .collect::<Result<Vec<_>>>()?;
    for (id, source) in sources {
        check_cancelled(stop)?;
        eprintln!("demo sample {id}: preparing immutable index revision");
        let mut job = ManagedIndex::start_revisions(
            &resources,
            source,
            None,
            IndexOptions {
                jobs,
                lod,
                ..Default::default()
            },
            indexer.clone(),
        )?;
        let mut last = std::time::Instant::now();
        while !job.is_finished() {
            if stop.load(Ordering::Relaxed) != 0 {
                job.cancel();
            }
            if last.elapsed() >= Duration::from_secs(5) {
                let s = job.snapshot();
                eprintln!(
                    "demo sample {id}: {:?} ({}/{})",
                    s.phase, s.completed, s.total
                );
                last = std::time::Instant::now();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        job.close()?;
        check_cancelled(stop)?;
        let s = job.snapshot();
        if s.phase != Phase::Succeeded || s.revision_sync_warning {
            return Err(Error::new(ErrorKind::Incomplete,
                format!("demo sample {id}: revision preparation incomplete or durability unconfirmed; inspect before serving")));
        }
        eprintln!("demo sample {id}: prepared");
    }
    Ok(())
}
pub(super) fn serve(path: &Path, key: &Path, port: u16, stop: &Arc<AtomicUsize>) -> Result<()> {
    let config = policy(path)?;
    let http_test = config.http_test();
    if http_test {
        eprintln!("WARNING: HTTP demo test mode is unencrypted. Restrict the proxy to a trusted internal network; use approved samples only.");
    }
    let key = read_proxy_key(key)?;
    let resources = resources()?;
    for id in samples(&config) {
        check_cancelled(stop)?;
        let source = config.register_source(&id, stop)?;
        let _dataset = ManagedDataset::open_revisions(&resources, &source, None, Mode::Level, stop)
            .map_err(|error| {
                if error.kind == ErrorKind::Cancelled { error } else {
                    Error::new(ErrorKind::Cache,
                        format!("demo sample {id}: no usable immutable index; run server --prepare-demo first"))
                }
            })?;
    }
    let options = RenderOptions {
        binary: Discovery::renderer()?.renderd_path()?,
        budget_mb: 256,
        decode_jobs: 2,
        raster_jobs: 2,
        tile_px: 384,
        round_pages: 1 << 30,
        open_timeout_s: 60,
        label_font_px: 14,
        raw: false,
        debug: false,
    };
    check_cancelled(stop)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(4)
        .build()?;
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
        let addr = listener.local_addr()?;
        let url = format!("{}/demo", config.public_origin());
        let broker = Arc::new(
            Broker::public_demo(addr, config, &key)
                .map_err(|_| Error::input("cannot configure public demo broker"))?,
        );
        let runtime = Runtime::new(broker, resources, options)
            .map_err(|_| Error::input("cannot configure public demo renderer"))?;
        let transport = if http_test { "HTTP test" } else { "HTTPS" };
        eprintln!("public demo ready: {url} (loopback {addr}; {transport} proxy required)");
        broker::serve_runtime(listener, runtime, async {
            while stop.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await?;
        check_cancelled(stop)
    })
}
