//! Trusted local launcher; browser requests never choose paths/binaries.
use floe_app_core::{
    cache,
    jobdeck::index::parse_levels,
    managed::{Limits, Resources},
    native::{Discovery, Indexer},
    registered::{AccessScope, RegisteredSource},
    render::RenderOptions,
    shots,
    view::ControllerOptions,
    Error, Result,
};
use floe_web::{
    service::Service,
    transport::{self, Gateway, BUNDLE},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    net::{Ipv4Addr, TcpListener},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const HELP: &str = "Usage: floe2-web view SOURCE [SOURCE ...] [OPTIONS]

  --goto X,Y,WIDTH          Initial centre and view width in um (one render)
  --depth full|N            Initial depth (default full)
  --detail low|medium|high|exact  Initial detail (default medium)
  --thin auto|keep|cull     Thin-page policy (default auto)
  --mode level|chip        Jobdeck view mode (default level)
  --level N,N,...          Initial jobdeck levels (default all)
  --no-labels / --frames   Initial display switches
  --labels / --no-frames   Explicit display overrides
  --label-font-px N        Label size (6..96 device px, default 14)
  --mono                   Initial monochrome display
  --jobs N                 Decode workers (environment/default up to 8)
  --raster-jobs N          Raster workers (environment/default up to 4)
  --budget-mb N            Decoded page budget (default 1024)
  --png / --raw            Frame transfer (default raw)
  --frame-cache on|off      Retained frame reuse + layout margin (default on)
  --root DIRECTORY         Additional approved dependency root, repeatable
  --drc RESULTS.db|PACK.ice Register DRC on first source; pack build needs owner approval
  --drc-waives FILE        Explicit existing waive sidecar (requires --drc)
  --drc-rules FILE         Explicit existing SVRF rules.json (requires --drc)
  --drc-reviewer TAG       Enable owner note publication for this fixed tag (requires --drc)
  --port N                 Loopback port (default random)
  --no-open                Do not launch a browser; use the private session file
  --firefox PATH           Explicit Firefox binary (or FLOE_FIREFOX_BIN)
  --session-file FILE      New 0600 session JSON; never overwrite
  --help                   Show this help

This development command does not replace the GTK floe2 launcher.
No automatic indexing or Python fallback. Paths are local-launcher inputs only.
Binds only 127.0.0.1; stops on Ctrl+C or End session.
Managed capacity: 16 CPU slots, 4 reserved for foreground; index jobs <=12.
Decode+raster reservation must fit 16 slots (DRC reserves 1 extra slot + 256 MiB;
SVRF metadata reserves another 256 MiB, with no extra CPU worker).
DRC reads the explicit ASCII or ICE file; no adjacent-pack/reviewer discovery or implicit indexing.
Refinement off; deck margin unsupported.
FLOE_FILL_EDIT (nonempty) explicitly enables approved shared design-default publication.
The session link is a one-time credential; do not share or log it.";

#[derive(Debug)]
pub struct Command {
    help: bool,
    sources: Vec<PathBuf>,
    roots: Vec<PathBuf>,
    initial: Value,
    mode: String,
    levels: Option<BTreeSet<i64>>,
    jobs: Option<u16>,
    raster: Option<u16>,
    budget: Option<u64>,
    raw: Option<bool>,
    frame_cache: bool,
    port: u16,
    no_open: bool,
    session_file: Option<PathBuf>,
    firefox: Option<PathBuf>,
    drc: Option<PathBuf>,
    drc_waives: Option<PathBuf>,
    drc_rules: Option<PathBuf>,
    drc_reviewer: Option<String>,
}
pub fn parse(args: &[String]) -> Result<Command> {
    let mut c = Command {
        help: false,
        sources: Vec::new(),
        roots: Vec::new(),
        initial: json!({}),
        mode: "level".into(),
        levels: None,
        jobs: None,
        raster: None,
        budget: None,
        raw: None,
        frame_cache: true,
        port: 0,
        no_open: false,
        session_file: None,
        firefox: None,
        drc: None,
        drc_waives: None,
        drc_rules: None,
        drc_reviewer: None,
    };
    let mut i = 1;
    let mut positional = false;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if !positional && arg == "--" {
            positional = true;
            continue;
        }
        if positional || !arg.starts_with('-') {
            c.sources.push(PathBuf::from(arg));
            continue;
        }
        let (key, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)));
        let mut value = || -> Result<&str> {
            if let Some(v) = inline {
                return Ok(v);
            }
            let v = args
                .get(i)
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| Error::input(format!("{key} requires a value")))?;
            i += 1;
            Ok(v)
        };
        let flag = || -> Result<()> {
            if inline.is_some() {
                Err(Error::input(format!("{key} takes no value")))
            } else {
                Ok(())
            }
        };
        match key {
            "--help" | "-h" => {
                flag()?;
                c.help = true;
                return Ok(c);
            }
            "--no-open" => {
                flag()?;
                c.no_open = true;
            }
            "--no-labels" => {
                flag()?;
                c.initial["labels"] = json!(false);
            }
            "--labels" => {
                flag()?;
                c.initial["labels"] = json!(true);
            }
            "--no-frames" => {
                flag()?;
                c.initial["frames"] = json!(false);
            }
            "--label-font-px" => {
                c.initial["font_px"] = json!(number(value()?, 6, 96, key)?);
            }
            "--frames" => {
                flag()?;
                c.initial["frames"] = json!(true);
            }
            "--mono" => {
                flag()?;
                c.initial["mono"] = json!(true);
            }
            "--png" => {
                flag()?;
                c.raw = Some(false);
            }
            "--raw" => {
                flag()?;
                c.raw = Some(true);
            }
            "--goto" => {
                let [x, y, w] = shots::lengths::<3>(value()?)?;
                if w <= 0. {
                    return Err(Error::input("goto width must be positive"));
                }
                c.initial["navigation"] = json!({"kind":"goto","center_um":[x.to_string(),y.to_string()],"width_um":w.to_string()});
            }
            "--frame-cache" => {
                c.frame_cache = match value()? {
                    "on" => true,
                    "off" => false,
                    _ => return Err(Error::input("frame-cache must be on or off")),
                };
            }
            "--depth" => {
                let v = value()?;
                if v != "full" && v.parse::<u32>().map_or(true, |n| n.to_string() != v) {
                    return Err(Error::input("depth must be full or a nonnegative integer"));
                }
                c.initial["depth"] = json!(v);
            }
            "--detail" => {
                let v = value()?;
                if !["low", "medium", "high", "exact"].contains(&v) {
                    return Err(Error::input("invalid detail"));
                }
                c.initial["detail"] = json!(v);
            }
            "--thin" => {
                let v = value()?;
                if !["auto", "keep", "cull"].contains(&v) {
                    return Err(Error::input("invalid thin policy"));
                }
                c.initial["thin"] = json!(v);
            }
            "--mode" => {
                let v = value()?;
                if !["level", "chip"].contains(&v) {
                    return Err(Error::input("mode must be level or chip"));
                }
                c.mode = v.into();
            }
            "--level" => c.levels = Some(parse_levels(value()?)?),
            "--jobs" => c.jobs = Some(number(value()?, 1, 16, key)? as u16),
            "--raster-jobs" => c.raster = Some(number(value()?, 1, 16, key)? as u16),
            "--budget-mb" => c.budget = Some(number(value()?, 1, 2048, key)?),
            "--port" => c.port = number(value()?, 0, 65535, key)? as u16,
            "--root" => c.roots.push(PathBuf::from(value()?)),
            "--session-file" => c.session_file = Some(PathBuf::from(value()?)),
            "--firefox" => c.firefox = Some(PathBuf::from(value()?)),
            "--drc" => c.drc = Some(PathBuf::from(value()?)),
            "--drc-waives" => c.drc_waives = Some(PathBuf::from(value()?)),
            "--drc-rules" => c.drc_rules = Some(PathBuf::from(value()?)),
            "--drc-reviewer" => c.drc_reviewer = Some(value()?.to_owned()),
            _ => return Err(Error::input(format!("unsupported view option: {key}"))),
        }
    }
    if (c.drc_waives.is_some() || c.drc_rules.is_some() || c.drc_reviewer.is_some())
        && c.drc.is_none()
    {
        return Err(Error::input(
            "--drc-waives / --drc-rules / --drc-reviewer require --drc",
        ));
    }
    if let Some(tag) = &c.drc_reviewer {
        floe_app_core::drc::waive_paths(c.drc.as_ref().unwrap(), tag)?;
    }
    if c.sources.is_empty() || c.sources.len() > 32 {
        return Err(Error::input("view requires 1..32 registered sources"));
    }
    if c.roots.len() > 32 {
        return Err(Error::input("too many approved roots"));
    }
    Ok(c)
}
fn number(s: &str, min: u64, max: u64, key: &str) -> Result<u64> {
    s.parse::<u64>()
        .ok()
        .filter(|v| *v >= min && *v <= max)
        .ok_or_else(|| Error::input(format!("{key} must be {min}..{max}")))
}

struct SessionFile {
    directory: PathBuf,
    path: PathBuf,
    identity: Option<(u64, u64)>,
}
impl SessionFile {
    fn create(explicit: Option<PathBuf>, value: &Value) -> Result<Self> {
        let root = fs::canonicalize(std::env::temp_dir())?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::input("invalid clock"))?
            .as_nanos();
        let directory = root.join(format!("floe-web-{}-{stamp}", std::process::id()));
        DirBuilder::new().mode(0o700).create(&directory)?;
        let mut owned = Self {
            path: directory.join("session.json"),
            directory,
            identity: None,
        };
        if let Some(p) = explicit {
            owned.path = cache::absolute(&p)?;
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&owned.path)?;
        let meta = file.metadata()?;
        owned.identity = Some((meta.dev(), meta.ino()));
        file.write_all(serde_json::to_string(value).unwrap().as_bytes())?;
        file.sync_all()?;
        Ok(owned)
    }
}
impl Drop for SessionFile {
    fn drop(&mut self) {
        // Remove only this created inode, never a user replacement/symlink.
        if fs::symlink_metadata(&self.path).is_ok_and(|m| Some((m.dev(), m.ino())) == self.identity)
        {
            let _ = fs::remove_file(&self.path);
        }
        let _ = fs::remove_dir(&self.directory);
    }
}
pub fn run(c: Command, cancelled: &Arc<AtomicUsize>) -> Result<i32> {
    if c.help {
        println!("{HELP}");
        return Ok(0);
    }
    let firefox = if c.no_open {
        None
    } else {
        Some(floe_app_core::browser::discover(c.firefox.as_deref())?)
    };
    let mut options = RenderOptions::local()?;
    if let Some(n) = c.jobs {
        options.decode_jobs = n;
    }
    if let Some(n) = c.raster {
        options.raster_jobs = n;
    }
    if let Some(n) = c.budget {
        options.budget_mb = n;
    }
    if let Some(v) = c.raw {
        options.raw = v;
    }
    let resources = Resources::new(Limits::default())?;
    drop(resources.render(&options)?);
    let mut drc_roots = c.roots.clone();
    let mut roots = c.roots;
    for source in &c.sources {
        roots.push(
            cache::absolute(source)?
                .parent()
                .ok_or_else(|| Error::input("source has no parent"))?
                .to_owned(),
        );
    }
    for path in c
        .drc
        .iter()
        .chain(c.drc_waives.iter())
        .chain(c.drc_rules.iter())
    {
        drc_roots.push(
            cache::absolute(path)?
                .parent()
                .ok_or_else(|| Error::input("DRC path has no parent"))?
                .to_owned(),
        );
    }
    roots.sort();
    roots.dedup();
    let scope = AccessScope::new(&roots)?;
    // An explicit review pack must not broaden a deck's TC dependency roots.
    // Only --root grants extra roots to both registrations.
    let drc_scope = if c.drc.is_some() {
        drc_roots.sort();
        drc_roots.dedup();
        Some(AccessScope::new(&drc_roots)?)
    } else {
        None
    };
    let sources = c
        .sources
        .iter()
        .map(|p| RegisteredSource::register(Arc::clone(&scope), p, cancelled))
        .collect::<Result<Vec<_>>>()?;
    sources[0].validate_levels(c.levels.as_ref())?;
    if !sources[0].deck && c.mode != "level" {
        return Err(Error::input("chip mode requires a jobdeck"));
    }
    let indexer = Indexer::discover(&Discovery::local()?)?;
    let service = Service::start_configured(
        sources,
        Arc::clone(&resources),
        options.clone(),
        indexer.clone(),
        ControllerOptions {
            margin_prefetch: c.frame_cache,
            frame_cache: c.frame_cache,
        },
    )?;
    let request = json!({"kind":"open","seq":"1","source_id":service.catalog()["sources"][0]["source_id"],"mode":c.mode,
        "levels":c.levels.map_or_else(||json!({"mode":"all"}),|ids|json!({"mode":"only","ids":ids.iter().map(i64::to_string).collect::<Vec<_>>()})),"body":c.initial});
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, c.port))?;
    listener.set_nonblocking(true)?;
    let (mut gate, secret) =
        Gateway::with_startup(listener.local_addr()?, Arc::clone(&service), request)
            .map_err(Error::input)?;
    if let Some(path) = &c.drc {
        let drc = floe_web::drc::Service::start_with_rules(
            &resources,
            drc_scope.expect("DRC-specific registration scope"),
            path,
            c.drc_waives.as_deref(),
            c.drc_rules.as_deref(),
            service.catalog()["sources"][0]["source_id"]
                .as_str()
                .unwrap(),
        )?;
        // Check combined capacity before publishing a URL or starting a browser.
        // The actual view acquires its own reservation when opened by the UI.
        drop(resources.render(&options)?);
        let registry = floe_web::drc::Registry::with_builds(drc, indexer)?;
        Gateway::attach_drc_registry(&mut gate, registry).map_err(Error::input)?;
    }
    let url = format!("{}/#bootstrap={}", gate.origin(), secret.expose());
    let session = SessionFile::create(
        c.session_file,
        &json!({"url":url,"origin":gate.origin(),"bundle":BUNDLE,"pid":std::process::id()}),
    )?;
    if let Some(tag) = &c.drc_reviewer {
        Gateway::enable_drc_notes(
            &mut gate,
            tag,
            std::slice::from_ref(&session.path),
            std::slice::from_ref(&session.directory),
        )
        .map_err(Error::input)?;
    }
    if std::env::var_os("FLOE_FILL_EDIT").is_some_and(|v| !v.is_empty()) {
        Gateway::enable_design_defaults(
            &mut gate,
            std::slice::from_ref(&session.path),
            std::slice::from_ref(&session.directory),
        )
        .map_err(Error::input)?;
    }
    let mut browser = firefox
        .map(|path| floe_app_core::browser::Browser::start(&path, &session.directory, &url))
        .transpose()?;
    eprintln!("[floe2-web] local workspace: {}", gate.origin());
    eprintln!(
        "[floe2-web] private session link: {} (one use, expires in 120s)",
        session.path.display()
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut browser_error = None;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        transport::serve(listener, gate, async {
            while cancelled.load(Ordering::Relaxed) == 0 && !service.is_finished() {
                if let Some(browser) = &browser {
                    match browser.exited() {
                        Ok(true) => break,
                        Err(e) => {
                            browser_error = Some(e);
                            break;
                        }
                        Ok(false) => (),
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
    })?;
    if let Some(browser) = &mut browser {
        browser.close()?;
    }
    if let Some(error) = browser_error {
        return Err(error);
    }
    drop(session);
    let signal = cancelled.load(Ordering::Relaxed);
    Ok(if signal == 0 { 0 } else { 128 + signal as i32 })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }
    #[test]
    fn initial_settings_form_one_bounded_patch() {
        let c=parse(&args("view source.oas --goto -10.9375,20,700 --depth 99 --detail high --thin keep --jobs 8 --raster-jobs 4 --no-open --label-font-px 18 --no-frames --labels")).unwrap();
        assert_eq!(c.initial["navigation"]["center_um"][0], "-10.9375");
        assert_eq!(c.initial["navigation"]["width_um"], "700");
        assert_eq!(c.initial["depth"], "99");
        assert_eq!(c.initial["detail"], "high");
        assert!(c.no_open);
        assert_eq!(c.initial["font_px"], 18);
        assert_eq!(c.initial["labels"], true);
        assert_eq!(c.initial["frames"], false);
        for s in [
            "view",
            "view a --jobs 0",
            "view a --raster-jobs 17",
            "view a --goto 0,0,-1",
            "view a --depth -1",
            "view a --thin bad",
            "view a --listen 0.0.0.0",
            "view a --no-open=yes",
            "view a --label-font-px 97",
            "view a --label-font-px 5",
            "view a --drc-waives review",
            "view a --drc-rules rules.json",
            "view a --drc-reviewer test",
            "view a --drc b --drc-reviewer ../escape",
        ] {
            assert!(parse(&args(s)).is_err(), "{s}");
        }
        assert!(parse(&args(
            "view a --drc results.ice --drc-rules rules.json --no-open"
        ))
        .unwrap()
        .drc_rules
        .is_some());
        assert!(parse(&args("view --help")).unwrap().help);
        let c = parse(&args("view a --drc b.ice --drc-waives side")).unwrap();
        assert!(c.drc_reviewer.is_none());
        assert_eq!(
            parse(&args("view a --drc b.ice --drc-reviewer fixed-owner"))
                .unwrap()
                .drc_reviewer
                .as_deref(),
            Some("fixed-owner")
        );
        assert_eq!(c.drc, Some(PathBuf::from("b.ice")));
        assert_eq!(c.drc_waives, Some(PathBuf::from("side")));
    }
}
