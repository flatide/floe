//! Development CLI: deliberately distinct from the Python floe2 launcher.
#![forbid(unsafe_code)]
mod capture;
mod clip;
mod deck_analysis;
mod deck_index;
mod drc;
mod fe_embed;
mod read;
mod selfcheck;
mod svrf;
mod web_view;
use floe_app_core::{
    index::{Action, IndexOptions, PreparedIndex, ProfileCell},
    jobdeck::index::{is_deck, parse_levels},
    native::{Discovery, Indexer},
    Error, ErrorKind, Result,
};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

const HELP: &str = "floe2-web — Rust application migration CLI (web preview)

Usage: floe2-web index SOURCE [OPTIONS]
       floe2-web view SOURCE [OPTIONS]
       floe2-web info SOURCE [--json]
       floe2-web render SOURCE [OPTIONS]
       floe2-web clip SOURCE --bbox X0,Y0,X1,Y1 [OPTIONS]
       floe2-web probe SOURCE
       floe2-web jobdeck DECK.jb [OPTIONS]
       floe2-web drc RESULTS.db|PACK.ice [OPTIONS]
       floe2-web fe-embed [OPTIONS] PNG...
       floe2-web svrf DECK [OPTIONS]
       floe2-web selfcheck [--adjacent] [--metadata-only]
       floe2-web --version

Implemented: layout/jobdeck index/info/render/probe, occupancy, profiling,
and jobdeck analysis/spec + source indexing with level selection.
Web preview: isolated Firefox or --no-open; no GTK launcher replacement yet.
DRC: read-only ICE/ASCII queries; explicit --build [--force] for atomic packs.
Clip: full-depth exact layout OASIS export; jobdeck clip remains unsupported.
Render: batch/mosaic + JSON reports, DRC marker/CD/legend captures; no Python runtime.
Annotations: fe-embed CLI writes flateyes PNG metadata without changing pixels.
SVRF: local subset parser/scan with diagnostics; no Tcl or macro execution.
Web: owner notes/waives and whole-review transfers require explicit opt-ins.
GTK-only gtktest is not ported. Full interaction/field acceptance remains open.
The existing floe2/GTK launcher is unchanged; there is no Python fallback here.
Run floe2-web index --help for indexing options.";
const INDEX_HELP: &str = "Usage: floe2-web index SOURCE [OPTIONS]

  --force                    Allow replacement of stale/existing cache
  --level N,N,...            Jobdeck: index sources of these mask levels only
  --jobs N                   Native parser/planner workers (default 12)
  --page-target-mb N          Encoded page target MiB (native default 1)
  --lod / --no-lod            LOD generation opt-in / default off
  --occupancy                Add summary, or include it in a new build
  --occupancy-only           Rebuild only summary on a current cache
  --occupancy-um UM          Positive base cell; implies occupancy
  --slow-cell-s S            Nonnegative slow-cell threshold
  --p2-shard-limit-mb N       Nonnegative shard-copy ceiling
  --profile-cell NAME        Profile one cell without writing a normal cache
  --profile-cell-ci N        Profile by zero-based cell index (exclusive)
  --profile-jobs N,N,...     Reuse parse/prepare across ordered job counts
  --profile-repeat N         Repeat each job count (default 1)
  --profile-snapshot PATH    Explicit reusable parse/prepare snapshot
  --profile-snapshot-refresh Replace an explicit snapshot
  -h, --help                Show this help

.jb sources are indexed sequentially; each source uses --jobs workers.
Deck profiling is unsupported; profile its source OASIS directly.
Legacy/KLayout and retired coverage options are rejected.
--force authorizes native replacement, not a transactional backup.
Current caches keep their build options; use --force to change LOD.
The source path may contain spaces/Unicode. Use -- for a leading dash.";

enum Cli {
    Help(bool),
    Version,
    Index(PathBuf, Box<IndexOptions>, Option<BTreeSet<i64>>),
    Read(Box<read::Command>),
    Clip(Box<clip::Command>),
    Jobdeck(Box<deck_analysis::Command>),
    View(Box<web_view::Command>),
    Drc(Box<drc::Command>),
    FeEmbed(Box<fe_embed::Command>),
    Svrf(Box<svrf::Command>),
    SelfCheck(selfcheck::Options),
}
fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Cli> {
    let args: Vec<String> = args
        .into_iter()
        .map(|a| {
            a.into_string()
                .map_err(|_| Error::input("arguments must be UTF-8"))
        })
        .collect::<Result<_>>()?;
    if args.is_empty() {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "a command/source is required; run floe2-web --help",
        ));
    }
    match args[0].as_str() {
        "--help" | "-h" if args.len() == 1 => return Ok(Cli::Help(false)),
        "--version" if args.len() == 1 => return Ok(Cli::Version),
        "selfcheck" => return selfcheck::parse(&args).map(Cli::SelfCheck),
        "index" => (),
        "info" | "render" | "probe" => return read::parse(&args).map(|c| Cli::Read(Box::new(c))),
        "jobdeck" => return deck_analysis::parse(&args).map(|c| Cli::Jobdeck(Box::new(c))),
        "view" => return web_view::parse(&args).map(|c| Cli::View(Box::new(c))),
        "drc" => return drc::parse(&args).map(|c| Cli::Drc(Box::new(c))),
        "clip" => return clip::parse(&args).map(|c| Cli::Clip(Box::new(c))),
        "fe-embed" => return fe_embed::parse(&args).map(|c| Cli::FeEmbed(Box::new(c))),
        "svrf" => return svrf::parse(&args).map(|c| Cli::Svrf(Box::new(c))),
        "gtktest" => {
            return Err(Error::new(
                ErrorKind::Unsupported,
                format!(
                    "{} is not yet ported; use the existing floe2 (no Python fallback)",
                    args[0]
                ),
            ))
        }
        _ => return Err(Error::input("unknown command; run floe2-web --help")),
    }
    let mut options = IndexOptions::default();
    let mut source = None;
    let mut levels = None;
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
                return Err(Error::input("index accepts exactly one source"));
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
                .ok_or_else(|| Error::input(format!("{flag} requires a value")))?;
            if v.starts_with("--") {
                return Err(Error::input(format!("{flag} requires a value")));
            }
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
        match flag {
            "--help" | "-h" => {
                no_value()?;
                return Ok(Cli::Help(true));
            }
            "--force" => {
                no_value()?;
                options.force = true;
            }
            "--lod" => {
                no_value()?;
                options.lod = true;
            }
            // Existing Python precedence: explicit --lod wins if both occur.
            "--no-lod" => {
                no_value()?;
            }
            "--occupancy" => {
                no_value()?;
                options.occupancy = true;
            }
            "--occupancy-only" => {
                no_value()?;
                options.occupancy_only = true;
            }
            "--profile-snapshot-refresh" => {
                no_value()?;
                options.profile_snapshot_refresh = true;
            }
            "--jobs" => options.jobs = number(value()?, flag)?,
            "--page-target-mb" => options.page_target_mb = Some(number(value()?, flag)?),
            "--occupancy-um" => options.occupancy_um = Some(number(value()?, flag)?),
            "--slow-cell-s" => options.slow_cell_s = Some(number(value()?, flag)?),
            "--p2-shard-limit-mb" => options.p2_shard_limit_mb = Some(number(value()?, flag)?),
            "--profile-cell" => {
                if matches!(options.profile_cell, Some(ProfileCell::Index(_))) {
                    return Err(Error::input(
                        "profile cell selectors are mutually exclusive",
                    ));
                }
                options.profile_cell = Some(ProfileCell::Name(value()?.into()));
            }
            "--profile-cell-ci" => {
                if matches!(options.profile_cell, Some(ProfileCell::Name(_))) {
                    return Err(Error::input(
                        "profile cell selectors are mutually exclusive",
                    ));
                }
                options.profile_cell = Some(ProfileCell::Index(number(value()?, flag)?));
            }
            "--profile-jobs" => {
                options.profile_jobs = Some(
                    value()?
                        .split(',')
                        .map(|s| number(s.trim(), flag))
                        .collect::<Result<_>>()?,
                )
            }
            "--profile-repeat" => options.profile_repeat = number(value()?, flag)?,
            "--profile-snapshot" => options.profile_snapshot = Some(PathBuf::from(value()?)),
            "--level" => levels = Some(parse_levels(value()?)?),
            _ => {
                return Err(Error::input(format!(
                    "unsupported index option: {flag}; run floe2-web index --help"
                )))
            }
        }
    }
    options.validate()?;
    let source = source.ok_or_else(|| Error::input("index requires SOURCE"))?;
    if levels.is_some() && !is_deck(&source) {
        return Err(Error::input("--level requires a .jb source"));
    }
    if is_deck(&source) && options.profile_cell.is_some() {
        return Err(Error::input("profile a jobdeck source OASIS directly"));
    }
    Ok(Cli::Index(source, Box::new(options), levels))
}
fn number<T: std::str::FromStr>(s: &str, flag: &str) -> Result<T> {
    s.parse()
        .map_err(|_| Error::input(format!("invalid numeric value for {flag}: {s}")))
}

struct Signals {
    flag: Arc<AtomicUsize>,
    ids: Vec<signal_hook::SigId>,
}
impl Signals {
    fn install() -> Result<Self> {
        let mut s = Self {
            flag: Arc::new(AtomicUsize::new(0)),
            ids: Vec::new(),
        };
        for n in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            s.ids.push(signal_hook::flag::register_usize(
                n,
                Arc::clone(&s.flag),
                n as usize,
            )?);
        }
        Ok(s)
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        for &id in &self.ids {
            signal_hook::low_level::unregister(id);
        }
    }
}
fn run(cli: Cli, cancelled: &Arc<AtomicUsize>) -> Result<i32> {
    match cli {
        Cli::SelfCheck(options) => return selfcheck::run(options, cancelled),
        Cli::View(command) => return web_view::run(*command, cancelled),
        Cli::Drc(command) => return drc::run(*command, cancelled),
        Cli::FeEmbed(command) => return fe_embed::run(*command, cancelled),
        Cli::Svrf(command) => return svrf::run(*command, cancelled),
        Cli::Read(command) => return read::run(*command, cancelled),
        Cli::Clip(command) => return clip::run(*command, cancelled),
        Cli::Jobdeck(command) => return deck_analysis::run(*command, cancelled),
        Cli::Help(index) => println!("{}", if index { INDEX_HELP } else { HELP }),
        Cli::Version => println!(
            "floe2-web {} (preview; revision {}; target {}; web {}; floe-index {}; floe-renderd {})",
            env!("CARGO_PKG_VERSION"),
            env!("FLOE_APP_REVISION"), env!("FLOE_APP_TARGET"), floe_web::transport::BUNDLE,
            floe_app_core::native::INDEX_VERSION, floe_worker_client::EXPECTED_RENDERD_VERSION
        ),
        Cli::Index(source, options, levels) => {
            if is_deck(&source) {
                return deck_index::run(&source, levels, &options, cancelled);
            }
            return execute_index(&source, &options, cancelled).map(|(code, _)| code);
        }
    }
    Ok(0)
}
fn execute_index(
    source: &Path,
    options: &IndexOptions,
    cancelled: &AtomicUsize,
) -> Result<(i32, Action)> {
    let indexer = Indexer::discover(&Discovery::local()?)?;
    let prepared = PreparedIndex::prepare(source, options, indexer, cancelled)?;
    let action = prepared.action().clone();
    match action {
        Action::Reuse => {
            println!(
                "[floe2-web] cache up to date: {} (use --force to rebuild with new options)",
                prepared.directory().display()
            );
            return Ok((0, action));
        }
        Action::OccupancyPresent => {
            println!(
                "[floe2-web] occupancy already present: {} (use --occupancy-only to rebuild it)",
                prepared.directory().join("design.ovo").display()
            );
            return Ok((0, action));
        }
        _ => (),
    }
    // Stderr only: profile stdout must remain native JSON.
    eprintln!("[floe2-web] {:?}: {}", action, prepared.source().display());
    let mut job = prepared.start(cancelled)?;
    loop {
        let signal = cancelled.load(Ordering::Relaxed) as i32;
        if signal != 0 {
            return job.cancel(signal).map(|code| (code, action));
        }
        if let Some(code) = job.poll()? {
            return Ok((code, action));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn main() {
    let cli = match parse(std::env::args_os().skip(1)) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("floe2-web: {e}");
            std::process::exit(2);
        }
    };
    let signals = match Signals::install() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("floe2-web: cannot install signal handlers: {e}");
            std::process::exit(1);
        }
    };
    let code = match run(cli, &signals.flag) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("floe2-web: {e}");
            match e.kind {
                ErrorKind::InvalidInput | ErrorKind::Unsupported => 2,
                ErrorKind::Cancelled => 128 + signals.flag.load(Ordering::Relaxed).max(2) as i32,
                ErrorKind::Incomplete => 3,
                _ => 1,
            }
        }
    };
    drop(signals);
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parsed(args: &[&str]) -> Result<Cli> {
        parse(args.iter().map(OsString::from))
    }
    #[test]
    fn parser_keeps_values_and_defaults() {
        let Cli::Index(p, o, _) = parsed(&[
            "index",
            "한 글.oas",
            "--jobs=16",
            "--lod",
            "--occupancy-um",
            "4",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(p, PathBuf::from("한 글.oas"));
        assert_eq!(o.jobs, 16);
        assert!(o.lod);
        let Cli::Index(p, o, _) = parsed(&["index", "--", "-source.oas"]).unwrap() else {
            panic!()
        };
        assert_eq!(p, PathBuf::from("-source.oas"));
        assert_eq!(o.jobs, 12);
        assert!(!o.lod);
        let Cli::Index(_, o, levels) =
            parsed(&["index", "x.JB", "--level", "2,1,2", "--lod"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(levels, Some(BTreeSet::from([1, 2])));
        assert!(o.lod);
    }
    #[test]
    fn invalid_or_unported_requests_never_launch_native() {
        for args in [
            &["index"][..],
            &["index", "x", "--jobs"],
            &["index", "x", "--jobs", "0"],
            &["index", "x", "--occupancy-um", "NaN"],
            &["index", "x", "--legacy"],
            &["index", "x", "--coverage"],
            &["index", "x", "--level", "1"],
            &[
                "index",
                "x",
                "--profile-cell",
                "T",
                "--profile-cell-ci",
                "0",
            ],
            &["index", "x", "--occupancy", "--occupancy-only"],
            &["view"],
            &[],
        ] {
            assert!(parsed(args).is_err(), "{args:?}");
        }
    }
}
