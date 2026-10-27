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
mod drc_match;
mod drc_prepare;
mod fe_embed;
mod read;
mod ready;
pub mod selfcheck;
mod svrf;
use floe_app_core::{
    index::{Action, IndexMode, IndexOptions, PreparedIndex, ProfileCell},
    jobdeck::index::{is_deck, parse_levels, ChipSelection},
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
    /// The exit status of a run stopped by its input or an unsupported
    /// request once its arguments parsed (a malformed command line is 2
    /// whatever this says): floe2-web 2, floe2 1 - the Python CLI's (its
    /// `SystemExit("...")`), which field scripts and the gates read.
    pub input_error_exit: i32,
}

static HOST: std::sync::OnceLock<&'static Host> = std::sync::OnceLock::new();
static DEFAULT_HOST: Host = Host {
    name: "floe2-web",
    version: env!("CARGO_PKG_VERSION"),
    usage: "",
    metadata: Vec::new,
    input_error_exit: 2,
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
    "drc-prepare",
    "drc-match",
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
       floe2-web drc-prepare RESULTS.db|PACK [--svrf RULES_JSON] [OPTIONS]
       floe2-web drc-match OLD NEW --plan PAIRS.json --states FLAGS.u8 --out RECORDS.bin --work-dir DIR
       floe2-web fe-embed [OPTIONS] PNG...
       floe2-web svrf DECK [OPTIONS]
       floe2-web selfcheck [--adjacent] [--metadata-only]
       floe2-web --version

Implemented: layout/jobdeck index/info/render/probe, occupancy, profiling,
and jobdeck analysis/spec + source indexing with level selection.
info/render/probe/clip index a source that has no index first (--no-index or
FLOE_AUTO_INDEX=off: refuse instead; --light / --full the kind of cache, else the
rule; a capture builds no occupancy summary unless render --occupancy on asks); a
source another run indexes is refused (exit 75) unless --wait[=SECONDS]
(FLOE_LOCK_WAIT=on|SECONDS) waits for it.
Indexing streams the source by default (0.12.370: an inventory pass, then the cells in file
order behind --mem GiB; --in-memory or FLOE_INDEX_STREAM=off is the whole-parse build). A
jobdeck's sources get the light cache (design.inv + design.seg + design.ovo, the geometry
read from the source; index --light / --full, FLOE_INDEX_LIGHT=on|off), a layout the full
index. A
build that outgrows the host stops at the memory ceiling (FLOE_RSS_CEILING: a percent of
MemTotal, default 90, or bytes such as 300G, or off; exit 71) and says what to do.
DRC: read-only ICE/ASCII queries; explicit --build [--force] for atomic packs.
Clip: full-depth exact layout OASIS export; jobdeck clip remains unsupported.
Render: batch/mosaic + JSON reports, DRC marker/CD/legend captures; no Python runtime.
Annotations: fe-embed CLI writes flateyes PNG metadata without changing pixels.
SVRF: local subset parser/scan with diagnostics; no Tcl or macro execution.
Run floe2-web index --help for indexing options.";
const INDEX_HELP: &str = "Usage: floe2-web index SOURCE [OPTIONS]

  --force                    Allow replacement of stale/existing cache
  --level N,N,...            Jobdeck: index sources of these mask levels only
  --chipid ID,ID,...         Jobdeck: the sources these CHIP blocks place only
                            (ids exact, or * ? [..] patterns as render --chip)
  --chipnumber N,N-M,...     Jobdeck: the sources of these CHIP blocks, numbered
                            1.. in deck order (ranges N-M); with --chipid a chip
                            in either list is in; --level cuts across both
  --jobs N                   Native parser/planner workers (default 12)
  --page-target-mb N          Encoded page target MiB (native default 1)
  --lod / --no-lod            LOD generation opt-in / default off
  --occupancy                Add summary (default on for decks, off for layouts)
  --no-occupancy             Leave/build the cache without adding a summary
  --occupancy-only           Rebuild only summary on a current cache
  --hier-only                Add/rebuild only the cell tree index (design.ovh)
  --no-page-occupancy        Leave out the pages' occupancy grids (design.ovb)
  --no-ovs                   Leave out the occupancy density (design.ovs)
  --occupancy-um UM          Positive base cell; default chip-size adaptive
  --occupancy-balance 0|1    Marking work split; default 1, byte-neutral diagnostic
  --occupancy-prune 0|1      Sub-cell bbox marking; default 1, 0 walks exact geometry
  --occupancy-max-work N     Marks per layer before it has no summary (none:work);
                             default 4 per byte of the file, at least 2^36
  --occupancy-max-bytes N    Bitmap bytes per file before the rest has none (none:size);
                             default 4G; K/M/G suffixes
  --representatives          Add bounded representative points (plain layout only)
  --representatives-only     Rebuild only points on a current cache
  --representatives-points N Group sample cap, 1..4194304; implies representatives
  --representatives-format 1|2 Points or shape/tree samples; implies rebuild of OVR
  --slow-cell-s S            Nonnegative slow-cell threshold
  --p2-shard-limit-mb N       Nonnegative shard-copy ceiling
  --stream                   Insist on the streaming build (the default: inventory pass,
                             then cells read in file order behind a byte budget,
                             design.ovo from one more pass); refuses representatives,
                             profiling and hier-only, which read the whole parse
  --in-memory                The whole-parse build (the default before 0.12.370; also
                             FLOE_INDEX_STREAM=off); the default falls back to it when
                             the streaming build declines a source
  --mem GiB                  Streaming budget for resident cells (default
                             min(MemAvailable/2, 32), raised to the source's largest
                             cell when the host has it); not with --in-memory
  --light                    The light cache (floe-index light): design.inv + design.seg
                             from one inventory pass, design.ovo with --occupancy; the
                             viewer reads the geometry from the source. The default for
                             a jobdeck's sources (FLOE_INDEX_LIGHT=on|off sets it for
                             every source). Not with --lod, --page-target-mb,
                             --representatives*, --hier-only, --profile-*, --stream,
                             --in-memory, --slow-cell-s, --p2-shard-limit-mb
  --full                     The full index (the default for a layout; also when one
                             of the options above is given). A current light cache is
                             built over; a full index is kept under --light
  --profile-cell NAME        Profile one cell without writing a normal cache
  --profile-cell-ci N        Profile by zero-based cell index (exclusive)
  --profile-jobs N,N,...     Reuse parse/prepare across ordered job counts
  --profile-repeat N         Repeat each job count (default 1)
  --profile-snapshot PATH    Explicit reusable parse/prepare snapshot
  --profile-snapshot-refresh Replace an explicit snapshot
  --wait[=SECONDS]           Another run indexing it (or, to rebuild it, a viewer
                            with it open): wait for it instead of exit 75; SECONDS
                            bounds the wait (FLOE_LOCK_WAIT=on|SECONDS: the default)
  --no-wait                  Refuse at once (exit 75) whatever FLOE_LOCK_WAIT says
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
    Index(
        PathBuf,
        Box<IndexOptions>,
        Option<BTreeSet<i64>>,
        Option<ChipSelection>,
        ready::Wait,
    ),
    Read(Box<read::Command>),
    Clip(Box<clip::Command>),
    Jobdeck(Box<deck_analysis::Command>),
    Drc(Box<drc::Command>),
    DrcPrepare(Box<drc_prepare::Command>),
    DrcMatch(Box<drc_match::Command>),
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
        "drc-prepare" => return drc_prepare::parse(&args).map(|c| Cli::DrcPrepare(Box::new(c))),
        "drc-match" => return drc_match::parse(&args).map(|c| Cli::DrcMatch(Box::new(c))),
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
    let mut chips = ChipSelection::default();
    // --wait / --no-wait over FLOE_LOCK_WAIT (ready.rs)
    let mut ready = ready::Ready::from_env()?;
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
        if matches!(flag, "--wait" | "--no-wait") {
            ready.take(flag, inline)?;
            continue;
        }
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
            "--hier-only" => {
                no_value()?;
                options.hier_only = true;
            }
            "--no-page-occupancy" => {
                no_value()?;
                options.page_occupancy = false;
            }
            "--no-ovs" => {
                no_value()?;
                options.ovs = false;
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
            "--occupancy-max-work" => {
                options.occupancy_max_work = Some(number(value()?, flag)?);
            }
            "--occupancy-max-bytes" => {
                options.occupancy_max_bytes = Some(byte_count(value()?, flag)?);
            }
            "--slow-cell-s" => options.slow_cell_s = Some(number(value()?, flag)?),
            "--p2-shard-limit-mb" => options.p2_shard_limit_mb = Some(number(value()?, flag)?),
            "--stream" => {
                no_value()?;
                if options.mode == IndexMode::InMemory {
                    return Err(Error::input("--stream and --in-memory are exclusive"));
                }
                options.mode = IndexMode::Stream;
            }
            "--in-memory" => {
                no_value()?;
                if options.mode == IndexMode::Stream {
                    return Err(Error::input("--stream and --in-memory are exclusive"));
                }
                options.mode = IndexMode::InMemory;
            }
            "--mem" => options.mem_gib = Some(number(value()?, flag)?),
            "--light" | "--full" => {
                no_value()?;
                let light = flag == "--light";
                if options.light.is_some_and(|l| l != light) {
                    return Err(Error::input("--light and --full are exclusive"));
                }
                options.light = Some(light);
            }
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
            // the CHIP blocks to index (0.12.402, user 2026-10-25: a deck's
            // index split by chips, the parts run at once)
            "--chipid" => chips.add_ids(value()?)?,
            "--chipnumber" => chips.add_numbers(value()?)?,
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
    let chips = (!chips.is_empty()).then_some(chips);
    if chips.is_some() && !is_deck(&source) {
        return Err(Error::input("--chipid / --chipnumber require a .jb source"));
    }
    if is_deck(&source) && options.profile_cell.is_some() {
        return Err(Error::input("profile a jobdeck source OASIS directly"));
    }
    Ok(Cli::Index(source, Box::new(options), levels, chips, ready.wait))
}

/// Where a command's own lines go: stdout, or stderr for an index a read
/// command runs first (its stdout is the command's: `info --json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Out {
    Stdout,
    Stderr,
}
impl Out {
    pub(crate) fn line(self, text: &str) {
        match self {
            Out::Stdout => println!("{text}"),
            Out::Stderr => eprintln!("{text}"),
        }
    }
    pub(crate) fn flush(self) -> std::io::Result<()> {
        use std::io::Write;
        match self {
            Out::Stdout => std::io::stdout().flush(),
            Out::Stderr => std::io::stderr().flush(),
        }
    }
}
/// Words as a POSIX shell reads them back (Python's `shlex.join`).
pub(crate) fn shell_join<S: AsRef<std::ffi::OsStr>>(words: &[S]) -> String {
    words
        .iter()
        .map(|w| {
            let w = w.as_ref().to_string_lossy();
            let safe = !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./_-".contains(c));
            if safe {
                w.into_owned()
            } else {
                format!("'{}'", w.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
/// a byte count with an optional K/M/G/T suffix (as floe-index reads
/// `--occupancy-max-bytes` and `--seg-bytes`), to the number floe-index gets
fn byte_count(s: &str, flag: &str) -> Result<u64> {
    let t = s.trim();
    let t = t.strip_suffix("iB").or_else(|| t.strip_suffix('B')).unwrap_or(t);
    let (digits, mult) = match t.chars().last() {
        Some('k' | 'K') => (&t[..t.len() - 1], 1u64 << 10),
        Some('m' | 'M') => (&t[..t.len() - 1], 1u64 << 20),
        Some('g' | 'G') => (&t[..t.len() - 1], 1u64 << 30),
        Some('t' | 'T') => (&t[..t.len() - 1], 1u64 << 40),
        _ => (t, 1u64),
    };
    digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(mult))
        .filter(|&n| n > 0)
        .ok_or_else(|| Error::input(format!("{flag} needs a byte count (e.g. 4G)")))
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
        Cli::DrcPrepare(command) => return drc_prepare::run(*command, cancelled),
        Cli::DrcMatch(command) => return drc_match::run(*command, cancelled),
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
        Cli::Index(source, options, levels, chips, wait) => {
            let mut waiter = ready::Waiter::new(wait, cancelled);
            if is_deck(&source) {
                return deck_index::run(&source, levels, chips, &options, &mut waiter, cancelled);
            }
            // a cache with readers in the way of this run: a rebuild or a
            // replacement of what they map - it is there already
            let readers = options.force
                || floe_app_core::cache::cache_path(&source).is_ok_and(|d| d.exists());
            loop {
                match execute_index(&source, &options, Out::Stdout, cancelled) {
                    // refused (floe-index's `[lock]` line, or the probe's,
                    // which the waiting line says again): wait, then again
                    Ok((floe_vfs::lock::BUSY_EXIT, _)) if waiter.on() => (),
                    Err(e) if e.kind == ErrorKind::Busy && waiter.on() => (),
                    other => return other.map(|(code, _)| code),
                }
                waiter.wait_to_write(&ready::layout_key(&source)?, readers)?;
            }
        }
    }
    Ok(0)
}
fn execute_index(
    source: &Path,
    options: &IndexOptions,
    out: Out,
    cancelled: &AtomicUsize,
) -> Result<(i32, Action)> {
    let indexer = Indexer::discover(&Discovery::local()?)?;
    let mut prepared = PreparedIndex::prepare(source, options, indexer, cancelled)?;
    if out == Out::Stderr {
        prepared = prepared.on_stderr();
    }
    let action = prepared.action().clone();
    if let Some(note) = prepared.note() {
        out.line(&format!("[{}] {}", floe_app_core::program(), note));
    }
    match action {
        Action::Reuse => {
            out.line(&format!(
                "[{}] cache up to date: {} (use --force to rebuild with new options)",
                floe_app_core::program(),
                prepared.directory().display()
            ));
            return Ok((0, action));
        }
        Action::OccupancyPresent => {
            out.line(&format!(
                "[{}] cache up to date: {} (occupancy already present; use --force to rebuild, --occupancy-only to rebuild the summary)",
                floe_app_core::program(),
                prepared.directory().join("design.ovo").display()
            ));
            return Ok((0, action));
        }
        _ => (),
    }
    // the indexer's command line, as the Python CLI showed it - stderr when
    // profiling: its stdout is the native JSON
    for command in prepared.commands() {
        let line = format!("[{}] {}", floe_app_core::program(), shell_join(&command));
        if action == Action::Profile {
            eprintln!("{line}");
        } else {
            out.line(&line);
        }
    }
    let mut job = prepared.start(cancelled)?;
    loop {
        let signal = cancelled.load(Ordering::Relaxed) as i32;
        if signal != 0 {
            return job.cancel(signal).map(|code| (code, action));
        }
        if let Some(code) = job.poll()? {
            if job.fell_back() {
                out.line(&format!(
                    "[{}] {}: the light cache cannot read it (a cell is defined inside a CBLOCK) - the full index was built in its place",
                    floe_app_core::program(),
                    source.display()
                ));
            }
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
    let command = args.first().and_then(|a| a.to_str()).map(str::to_string);
    let cli = match parse(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{program}: {e}");
            std::process::exit(2);
        }
    };
    // what this run's reader locks say it runs (renderd's registrations:
    // "floe2 render"), unless the caller named one (floe/cli.py)
    if std::env::var_os("FLOE_LOCK_WHAT").is_none() {
        if let Some(command) = &command {
            std::env::set_var("FLOE_LOCK_WHAT", format!("{program} {command}"));
        }
    }
    let signals = match Signals::install() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{program}: cannot install signal handlers: {e}");
            std::process::exit(1);
        }
    };
    let code = match run(cli, &signals.flag) {
        Ok(code) => code,
        // a busy target (floe_vfs::lock): one line, as floe-index says it,
        // and 75 - the jobdeck index and the viewer read it as busy; none
        // when floe-index said it already (an index run first, ready.rs)
        Err(e) if e.kind == ErrorKind::Busy => {
            if !e.message.is_empty() {
                eprintln!("[lock] {}", e.message);
            }
            floe_vfs::lock::BUSY_EXIT
        }
        Err(e) => {
            eprintln!("{program}: {e}");
            match e.kind {
                ErrorKind::InvalidInput | ErrorKind::Unsupported => host.input_error_exit,
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
    #[test]
    fn the_summary_caps_are_options_of_index() {
        let Cli::Index(_, o, _, _, _) = parsed(&[
            "index",
            "x",
            "--occupancy-max-work",
            "68719476736",
            "--occupancy-max-bytes",
            "4G",
        ])
        .unwrap() else {
            panic!("index")
        };
        assert_eq!(o.occupancy_max_work, Some(1 << 36));
        assert_eq!(o.occupancy_max_bytes, Some(4 << 30));
        for (text, bytes) in [("512", 512u64), ("64K", 64 << 10), ("3m", 3 << 20), ("2GiB", 2 << 30), ("1T", 1 << 40)] {
            assert_eq!(byte_count(text, "--occupancy-max-bytes").unwrap(), bytes, "{text}");
        }
        for args in [
            &["index", "x", "--occupancy-max-bytes", "0"][..],
            &["index", "x", "--occupancy-max-bytes", "4X"],
            &["index", "x", "--occupancy-max-work", "-1"],
            &["index", "x", "--occupancy-max-work"],
        ] {
            assert!(parsed(args).is_err(), "{args:?}");
        }
    }

    fn parsed(args: &[&str]) -> Result<Cli> {
        parse(args.iter().map(OsString::from))
    }
    #[test]
    fn parser_keeps_values_and_defaults() {
        let Cli::Index(p, o, _, _, _) = parsed(&[
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
        let Cli::Index(p, o, _, _, _) = parsed(&["index", "--", "-source.oas"]).unwrap() else {
            panic!()
        };
        assert_eq!(p, PathBuf::from("-source.oas"));
        assert_eq!(o.jobs, 12);
        assert!(!o.lod);
        assert_eq!(o.occupancy, None);
        let Cli::Index(_, o, levels, _, _) =
            parsed(&["index", "x.JB", "--level", "2,1,2", "--lod"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(levels, Some(BTreeSet::from([1, 2])));
        assert!(o.lod);
    }
    #[test]
    fn chip_selectors_parse_and_need_a_deck() {
        let Cli::Index(_, _, levels, chips, _) = parsed(&[
            "index",
            "x.jb",
            "--level",
            "1",
            "--chipid",
            "ID001,ID00[23]",
            "--chipnumber",
            "2,5-7",
            "--chipnumber=9",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(levels, Some(BTreeSet::from([1])));
        let chips = chips.unwrap();
        assert_eq!(chips.ids, ["ID001", "ID00[23]"]);
        assert_eq!(chips.numbers, [(2, 2), (5, 7), (9, 9)]);
        let Cli::Index(_, _, _, none, _) = parsed(&["index", "x.jb"]).unwrap() else {
            panic!()
        };
        assert!(none.is_none());
        assert!(parsed(&["index", "x.oas", "--chipid", "A"]).is_err());
        assert!(parsed(&["index", "x.jb", "--chipnumber", "0"]).is_err());
        assert!(parsed(&["index", "x.jb", "--chipnumber", "3-2"]).is_err());
        assert!(parsed(&["index", "x.jb", "--chipid", ","]).is_err());
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
            let Cli::Index(_, o, _, _, _) = parsed(&args).unwrap() else {
                panic!()
            };
            assert_eq!((o.occupancy, o.occupancy_only), (enabled, only));
        }
        assert!(parsed(&["index", "x", "--occupancy-only", "--no-occupancy"]).is_err());
        assert!(parsed(&["index", "x", "--no-occupancy", "--occupancy"]).is_err());
    }
    #[test]
    fn the_light_cache_is_a_mode_of_its_own() {
        // (DIRECT_READ_PLAN L5): --light / --full, exclusive; --light with an
        // option of the full index's is refused; nothing given leaves the rule
        let Cli::Index(_, o, _, _, _) = parsed(&["index", "x.oas", "--light"]).unwrap() else {
            panic!()
        };
        assert_eq!(o.light, Some(true));
        assert!(o.light_for(false) && o.light_for(true));
        let Cli::Index(_, o, _, _, _) = parsed(&["index", "x.jb", "--full", "--level", "1"]).unwrap() else {
            panic!()
        };
        assert_eq!(o.light, Some(false));
        assert!(!o.light_for(true));
        let Cli::Index(_, o, _, _, _) = parsed(&["index", "x.jb", "--lod"]).unwrap() else {
            panic!()
        };
        assert_eq!(o.light, None);
        assert_eq!(o.full_only(), Some("--lod"));
        assert!(!o.light_for(true), "an option of the full index's picks it");
        assert!(parsed(&["index", "x", "--light", "--full"]).is_err());
        for flag in ["--lod", "--hier-only", "--in-memory", "--stream", "--representatives"] {
            assert!(parsed(&["index", "x", "--light", flag]).is_err(), "{flag}");
        }
        // leaving a file of the full index out is moot under light, not refused
        // (a deck's sources come with --no-ovs)
        assert!(parsed(&["index", "x", "--light", "--no-ovs", "--no-page-occupancy"]).is_ok());
        assert!(parsed(&["index", "x", "--light", "--occupancy-only"]).is_ok());
        assert!(parsed(&["index", "x", "--light", "--occupancy", "--occupancy-um", "2", "--mem", "1"]).is_ok());
    }
    #[test]
    fn invalid_or_unported_requests_never_launch_native() {
        for format in ["1", "2"] {
            let Cli::Index(_, o, _, _, _) = parsed(&[
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
