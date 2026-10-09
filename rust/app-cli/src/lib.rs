//! The application CLI shared by feature/jobdeck's `floe2` and feature/webui's
//! `floe2-web` (docs/SHARED_APP_LAYER.ko.md): index, info, render, probe,
//! clip, jobdeck, drc, svrf, fe-embed and selfcheck over app-core - no web
//! server, no Python. A host binary names itself and adds its own commands
//! (jobdeck's `view` starts the GTK viewer; webui's starts the web viewer).
#![forbid(unsafe_code)]
mod capture;
mod clip;
mod deck_analysis;
mod deck_index;
mod drc;
mod fe_embed;
mod read;
pub mod selfcheck;
mod svrf;
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

/// The binary the CLI runs in: its name (the messages', `floe_app_core::
/// program`), version, help lines for its own commands and the metadata
/// selfcheck adds.
pub struct Host {
    pub name: &'static str,
    pub version: &'static str,
    /// usage lines of the host's own commands, before the shared ones
    pub usage: &'static str,
    pub metadata: fn() -> Vec<(&'static str, serde_json::Value)>,
}

static HOST: std::sync::OnceLock<&'static Host> = std::sync::OnceLock::new();
static DEFAULT_HOST: Host = Host {
    name: "floe2-web",
    version: env!("CARGO_PKG_VERSION"),
    usage: "",
    metadata: Vec::new,
};

/// The running host (`main`'s; floe2-web's defaults until then).
pub fn host() -> &'static Host {
    HOST.get().copied().unwrap_or(&DEFAULT_HOST)
}

/// A help text written for `floe2-web`, in the running program's name.
pub fn named(text: &str) -> String {
    text.replace("floe2-web", floe_app_core::program())
}

/// The shared commands: a host passes anything else to its own handler
/// (`main` refuses an unknown command).
pub const COMMANDS: &[&str] = &[
    "index",
    "info",
    "render",
    "probe",
    "clip",
    "jobdeck",
    "drc",
    "fe-embed",
    "svrf",
    "selfcheck",
];

const HELP: &str = "floe2-web — the Rust application CLI

Usage: floe2-web index SOURCE [OPTIONS]
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
DRC: read-only ICE/ASCII queries; explicit --build [--force] for atomic packs.
Clip: full-depth exact layout OASIS export; jobdeck clip remains unsupported.
Render: batch/mosaic + JSON reports, DRC marker/CD/legend captures; no Python runtime.
Annotations: fe-embed CLI writes flateyes PNG metadata without changing pixels.
SVRF: local subset parser/scan with diagnostics; no Tcl or macro execution.
Run floe2-web index --help for indexing options.";
const INDEX_HELP: &str = "Usage: floe2-web index SOURCE [OPTIONS]

  --force                    Allow replacement of stale/existing cache
  --level N,N,...            Jobdeck: index sources of these mask levels only
  --jobs N                   Native parser/planner workers (default 12)
  --page-target-mb N          Encoded page target MiB (native default 1)
  --lod / --no-lod            LOD generation opt-in / default off
  --occupancy                Add summary (default on for decks, off for layouts)
  --no-occupancy             Leave/build the cache without adding a summary
  --occupancy-only           Rebuild only summary on a current cache
  --occupancy-um UM          Positive base cell; default chip-size adaptive
  --occupancy-balance 0|1    Marking work split; default 1, byte-neutral diagnostic
  --occupancy-prune 0|1      Sub-cell bbox marking; default 1, 0 walks exact geometry
  --representatives          Add bounded representative points (plain layout only)
  --representatives-only     Rebuild only points on a current cache
  --representatives-points N Group sample cap, 1..4194304; implies representatives
  --representatives-format 1|2 Points or shape/tree samples; implies rebuild of OVR
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
Explicit Index migrates a legacy cache name to the hidden .ice directory.
Info/open/probe and cell profiling never rename. Completed renames remain
if a later rebuild fails or is cancelled; new-name destinations are not overwritten.
Close viewers in other processes before indexing; reader leases are app-local.
The source path may contain spaces/Unicode. Use -- for a leading dash.";

enum Cli {
    Help(bool),
    Version,
    Index(PathBuf, Box<IndexOptions>, Option<BTreeSet<i64>>),
    Read(Box<read::Command>),
    Clip(Box<clip::Command>),
    Jobdeck(Box<deck_analysis::Command>),
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
        return Ok(Cli::Help(false));
    }
    match args[0].as_str() {
        "--help" | "-h" if args.len() == 1 => return Ok(Cli::Help(false)),
        "--version" if args.len() == 1 => return Ok(Cli::Version),
        "selfcheck" => return selfcheck::parse(&args).map(Cli::SelfCheck),
        "index" => (),
        "info" | "render" | "probe" => return read::parse(&args).map(|c| Cli::Read(Box::new(c))),
        "jobdeck" => return deck_analysis::parse(&args).map(|c| Cli::Jobdeck(Box::new(c))),
        "drc" => return drc::parse(&args).map(|c| Cli::Drc(Box::new(c))),
        "clip" => return clip::parse(&args).map(|c| Cli::Clip(Box::new(c))),
        "fe-embed" => return fe_embed::parse(&args).map(|c| Cli::FeEmbed(Box::new(c))),
        "svrf" => return svrf::parse(&args).map(|c| Cli::Svrf(Box::new(c))),
        "--help" | "-h" | "--version" => {
            return Err(Error::input("global help/version must be used alone"))
        }
        other => {
            return Err(Error::input(format!(
                "unknown command: {other}; run {} --help",
                floe_app_core::program()
            )))
        }
    }
    let mut options = IndexOptions::default();
    let mut occupancy_mode = None;
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
            "--occupancy" | "--no-occupancy" | "--occupancy-only" => {
                no_value()?;
                if occupancy_mode.is_some_and(|previous| previous != flag) {
                    return Err(Error::input("occupancy mode flags are mutually exclusive"));
                }
                occupancy_mode = Some(flag);
                options.occupancy = Some(flag != "--no-occupancy");
                options.occupancy_only = flag == "--occupancy-only";
            }
            "--profile-snapshot-refresh" => {
                no_value()?;
                options.profile_snapshot_refresh = true;
            }
            "--representatives" => {
                no_value()?;
                options.representatives = true;
            }
            "--representatives-only" => {
                no_value()?;
                options.representatives_only = true;
            }
            "--representatives-points" => {
                options.representatives_points = Some(number(value()?, flag)?)
            }
            "--representatives-format" => {
                options.representatives_format = Some(number(value()?, flag)?)
            }
            "--jobs" => options.jobs = number(value()?, flag)?,
            "--page-target-mb" => options.page_target_mb = Some(number(value()?, flag)?),
            "--occupancy-um" => options.occupancy_um = Some(number(value()?, flag)?),
            "--occupancy-balance" => {
                options.occupancy_balance = Some(match number::<u8>(value()?, flag)? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::input("occupancy-balance must be 0 or 1")),
                });
            }
            "--occupancy-prune" => {
                options.occupancy_prune = Some(match number::<u8>(value()?, flag)? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::input("occupancy-prune must be 0 or 1")),
                });
            }
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
                    "unsupported index option: {flag}; run {} index --help",
                    floe_app_core::program()
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
        for n in [
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGHUP,
        ] {
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
        Cli::Drc(command) => return drc::run(*command, cancelled),
        Cli::FeEmbed(command) => return fe_embed::run(*command, cancelled),
        Cli::Svrf(command) => return svrf::run(*command, cancelled),
        Cli::Read(command) => return read::run(*command, cancelled),
        Cli::Clip(command) => return clip::run(*command, cancelled),
        Cli::Jobdeck(command) => return deck_analysis::run(*command, cancelled),
        Cli::Help(index) => {
            if index {
                println!("{}", named(INDEX_HELP));
            } else {
                let help = named(HELP);
                let (head, rest) = help.split_once('\n').unwrap_or((&help, ""));
                // the host's own commands first, under one Usage
                let rest = if host().usage.is_empty() {
                    rest.to_string()
                } else {
                    rest.replacen("Usage: ", "       ", 1)
                };
                println!("{head}\n{}{rest}", named(host().usage));
            }
        }
        Cli::Version => {
            let mut extra = String::new();
            for (key, value) in (host().metadata)() {
                extra.push_str(&format!(
                    "; {key} {}",
                    value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string())
                ));
            }
            println!(
                "{} {} (revision {}; target {}{extra}; floe-index {}; floe-renderd {})",
                floe_app_core::program(),
                host().version,
                env!("FLOE_APP_REVISION"),
                env!("FLOE_APP_TARGET"),
                floe_app_core::native::INDEX_VERSION,
                floe_worker_client::EXPECTED_RENDERD_VERSION
            )
        }
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
                "[{}] cache up to date: {} (use --force to rebuild with new options)",
                floe_app_core::program(),
                prepared.directory().display()
            );
            return Ok((0, action));
        }
        Action::OccupancyPresent => {
            println!(
                "[{}] cache up to date: {} (occupancy already present; use --force to rebuild, --occupancy-only to rebuild the summary)",
                floe_app_core::program(),
                prepared.directory().join("design.ovo").display()
            );
            return Ok((0, action));
        }
        _ => (),
    }
    // Stderr only: profile stdout must remain native JSON.
    eprintln!(
        "[{}] {:?}: {}",
        floe_app_core::program(),
        action,
        prepared.source().display()
    );
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
/// Run the shared command in `args` (the program's arguments, its name
/// left out) as `host`, and exit with its status.
pub fn main(host: &'static Host, args: Vec<OsString>) -> ! {
    let _ = HOST.set(host);
    floe_app_core::set_program(host.name);
    let program = floe_app_core::program();
    let cli = match parse(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{program}: {e}");
            std::process::exit(2);
        }
    };
    let signals = match Signals::install() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{program}: cannot install signal handlers: {e}");
            std::process::exit(1);
        }
    };
    let code = match run(cli, &signals.flag) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{program}: {e}");
            match e.kind {
                ErrorKind::InvalidInput | ErrorKind::Unsupported => 2,
                ErrorKind::Cancelled => {
                    let signal = signals.flag.load(Ordering::Relaxed);
                    128 + if signal == 0 { 2 } else { signal as i32 }
                }
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
        assert_eq!(o.occupancy, None);
        let Cli::Index(_, o, levels) =
            parsed(&["index", "x.JB", "--level", "2,1,2", "--lod"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(levels, Some(BTreeSet::from([1, 2])));
        assert!(o.lod);
    }
    #[test]
    fn occupancy_default_optout_only_and_profile_parse_without_writes() {
        for (flags, enabled, only) in [
            (vec![], None, false),
            (vec!["--no-occupancy"], Some(false), false),
            (vec!["--occupancy-only"], Some(true), true),
            (vec!["--profile-cell", "TOP"], None, false),
        ] {
            let mut args = vec!["index", "x.oas"];
            args.extend(flags);
            let Cli::Index(_, o, _) = parsed(&args).unwrap() else {
                panic!()
            };
            assert_eq!((o.occupancy, o.occupancy_only), (enabled, only));
        }
        assert!(parsed(&["index", "x", "--occupancy-only", "--no-occupancy"]).is_err());
        assert!(parsed(&["index", "x", "--no-occupancy", "--occupancy"]).is_err());
    }
    #[test]
    fn invalid_or_unported_requests_never_launch_native() {
        for format in ["1", "2"] {
            let Cli::Index(_, o, _) = parsed(&[
                "index",
                "x",
                "--representatives-format",
                format,
                "--occupancy-prune",
                "0",
            ])
            .unwrap() else {
                panic!("index")
            };
            assert!(o.wants_representatives());
            assert_eq!(o.occupancy_prune, Some(false));
        }
        for args in [
            ["index", "x", "--representatives-format", "0"],
            ["index", "x", "--representatives-format", "3"],
            ["index", "x", "--occupancy-prune", "2"],
        ] {
            assert!(parsed(&args).is_err());
        }
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
        ] {
            assert!(parsed(args).is_err(), "{args:?}");
        }
        assert!(matches!(parsed(&[]).unwrap(), Cli::Help(false)));
        assert!(parsed(&["view"]).is_err());
    }
}
