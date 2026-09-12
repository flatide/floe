//! Development CLI: deliberately distinct from the Python floe2 launcher.
#![forbid(unsafe_code)]
use floe_app_core::{
    index::{Action, IndexOptions, PreparedIndex, ProfileCell},
    native::{Discovery, Indexer},
    Error, ErrorKind, Result,
};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

const HELP: &str = "floe2-web — Rust application migration CLI (M1a, not yet a web server)

Usage: floe2-web index SOURCE [OPTIONS]
       floe2-web --version

Implemented: ordinary OASIS layout indexing, occupancy and cell profiling.
Not yet ported: view, info, render, probe, clip, jobdeck, drc, svrf, gtktest.
Use the existing floe2 for those commands; there is no Python fallback.
Run floe2-web index --help for indexing options.";
const INDEX_HELP: &str = "Usage: floe2-web index SOURCE [OPTIONS]

  --force                    Allow replacement of stale/existing cache
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

--level and .jb sources require the later jobdeck port (M1a-3).
Legacy/KLayout and retired coverage options are rejected.
--force authorizes native replacement, not a transactional backup.
Current caches keep their build options; use --force to change LOD.
The source path may contain spaces/Unicode. Use -- for a leading dash.";

enum Cli {
    Help(bool),
    Version,
    Index(PathBuf, Box<IndexOptions>),
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
            "view is not yet ported; run existing floe2, or floe2-web --help",
        ));
    }
    match args[0].as_str() {
        "--help" | "-h" if args.len() == 1 => return Ok(Cli::Help(false)),
        "--version" if args.len() == 1 => return Ok(Cli::Version),
        "index" => (),
        "view" | "info" | "render" | "probe" | "clip" | "jobdeck" | "drc" | "svrf" | "gtktest" => {
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
            "--level" | "--id" => {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "jobdeck level selection is not yet ported (M1a-3)",
                ))
            }
            _ => {
                return Err(Error::input(format!(
                    "unsupported index option: {flag}; run floe2-web index --help"
                )))
            }
        }
    }
    options.validate()?;
    let source = source.ok_or_else(|| Error::input("index requires SOURCE"))?;
    Ok(Cli::Index(source, Box::new(options)))
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
fn run(cli: Cli, cancelled: &AtomicUsize) -> Result<i32> {
    match cli {
        Cli::Help(index) => println!("{}", if index { INDEX_HELP } else { HELP }),
        Cli::Version => println!(
            "floe2-web {} (development M1a; floe-index {})",
            env!("CARGO_PKG_VERSION"),
            floe_app_core::native::INDEX_VERSION
        ),
        Cli::Index(source, options) => {
            let indexer = Indexer::discover(&Discovery::local()?)?;
            let prepared = PreparedIndex::prepare(&source, &options, indexer, cancelled)?;
            match prepared.action() {
                Action::Reuse => {
                    println!("[floe2-web] cache up to date: {} (use --force to rebuild with new options)", prepared.directory().display());
                    return Ok(0);
                }
                Action::OccupancyPresent => {
                    println!("[floe2-web] occupancy already present: {} (use --occupancy-only to rebuild it)", prepared.directory().join("design.ovo").display());
                    return Ok(0);
                }
                _ => (),
            }
            // Always stderr: profile stdout must remain a single native JSON
            // object/array. This line is diagnostic, never executed by a shell.
            eprintln!(
                "[floe2-web] {:?}: {}",
                prepared.action(),
                prepared.source().display()
            );
            let mut job = prepared.start(cancelled)?;
            loop {
                let signal = cancelled.load(Ordering::Relaxed) as i32;
                if signal != 0 {
                    return job.cancel(signal);
                }
                if let Some(code) = job.poll()? {
                    return Ok(code);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    Ok(0)
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
        let Cli::Index(p, o) = parsed(&[
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
        let Cli::Index(p, o) = parsed(&["index", "--", "-source.oas"]).unwrap() else {
            panic!()
        };
        assert_eq!(p, PathBuf::from("-source.oas"));
        assert_eq!(o.jobs, 12);
        assert!(!o.lod);
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
            &["view", "x"],
            &[],
        ] {
            assert!(parsed(args).is_err(), "{args:?}");
        }
    }
}
