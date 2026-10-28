//! `drc-prepare`: a DRC pack's analysis caches made ahead of a review
//! (feature/jobdeck-drc's `floe-index drc-prepare`, Python then; app-core
//! drc::analysis now): per rule, the exact centres the viewer's markers
//! read and the CD measurement table with its absolute and ratio groups,
//! in `.<db>.analysis/` (or FLOE_DRC_ANALYSIS_ROOT). The pack and the
//! review files are only read; finished rules keep their caches when a
//! later one is cancelled.
use floe_app_core::{
    check_cancelled,
    drc::analysis::{
        delta::{Basis, Cache, Groups, Table},
        points,
        source::{worker_count, PackSource, Source},
    },
    drc::Pack,
    Error, ErrorKind, Result,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicUsize, Arc};

const HELP: &str = "Usage: floe2-web drc-prepare RESULTS.db|.RESULTS.db.tray [OPTIONS]

Precompute the analysis caches of a DRC pack for large rules: per rule the
exact marker centres (16 bytes an error) and the CD measurement table with
its absolute and ratio delta groups, in .RESULTS.db.analysis/ beside the
database (or FLOE_DRC_ANALYSIS_ROOT/<hash>, e.g. a local SSD for a
read-only results folder; set it for the viewer too). The pack, the waive
and note files are not changed. Rules without errors are skipped.

  --svrf RULES_JSON   the deck's floe-index svrf sidecar (required for the
                      CD/delta caches; not the SVRF text itself)
  --rule NAME         prepare this rule only (repeatable; default: all)
  --jobs N            workers within a rule, 1..256 (default min(4, CPUs),
                      or FLOE_DRC_JOBS)
  --spatial-only      the marker centres only
  --delta-only        the CD/delta caches only
  -h, --help          this help

Progress goes to stderr; Ctrl+C stops (exit 130) and keeps finished rules.";

pub struct Command {
    source: PathBuf,
    rules_json: Option<PathBuf>,
    rules: Vec<String>,
    jobs: Option<usize>,
    spatial: bool,
    delta: bool,
}

pub fn parse(args: &[String]) -> Result<Command> {
    let mut c = Command {
        source: PathBuf::new(),
        rules_json: None,
        rules: Vec::new(),
        jobs: None,
        spatial: true,
        delta: true,
    };
    let mut source = None;
    let mut i = 1;
    let (mut spatial_only, mut delta_only) = (false, false);
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        let mut value = |flag: &str| -> Result<String> {
            let v = args
                .get(i)
                .cloned()
                .ok_or_else(|| Error::input(format!("{flag} needs a value")))?;
            i += 1;
            Ok(v)
        };
        match a {
            "-h" | "--help" => {
                println!("{}", crate::named(HELP));
                std::process::exit(0);
            }
            "--svrf" => c.rules_json = Some(PathBuf::from(value(a)?)),
            "--rule" => c.rules.push(value(a)?),
            "--jobs" => {
                let v = value(a)?;
                c.jobs = Some(v.parse().map_err(|_| {
                    Error::input("DRC worker count must be an integer from 1 to 256")
                })?)
            }
            // the Rust engine is the only one now (the branch's Python backend is gone)
            "--backend" => match value(a)?.as_str() {
                "auto" | "rust" => (),
                other => {
                    return Err(Error::input(format!(
                        "--backend {other}: the Python backend is gone; CD measurement is Rust"
                    )))
                }
            },
            "--spatial-only" => spatial_only = true,
            "--delta-only" => delta_only = true,
            flag if flag.starts_with('-') && flag != "-" => {
                return Err(Error::input(format!("unknown option {flag} (see --help)")))
            }
            path => {
                if source.replace(PathBuf::from(path)).is_some() {
                    return Err(Error::input("one DRC database at a time"));
                }
            }
        }
    }
    if spatial_only && delta_only {
        return Err(Error::input("--spatial-only and --delta-only exclude each other"));
    }
    c.spatial = !delta_only;
    c.delta = !spatial_only;
    c.source = source.ok_or_else(|| Error::input("a DRC .db or pack is required (see --help)"))?;
    if c.delta && c.rules_json.is_none() {
        return Err(Error::input(
            "--svrf <rules.json> is required for CD/delta preprocessing; use --spatial-only for the marker centres alone",
        ));
    }
    worker_count(c.jobs)?;
    Ok(c)
}

/// The pack a .db or pack path names, fresh against its .db.
fn pack_of(source: &Path) -> Result<PathBuf> {
    let mut magic = [0u8; 8];
    let packed = std::fs::File::open(source)
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut magic))
        .is_ok()
        && &magic == floe_app_core::drc::MAGIC;
    if packed {
        return Ok(source.to_path_buf());
    }
    let pack = floe_app_core::cache::pack_path(source)?;
    let stop = AtomicUsize::new(0);
    let fresh = Pack::open(&pack, &stop).and_then(|p| p.source_matches(source));
    match fresh {
        Ok(true) => Ok(pack),
        _ => Err(Error::input(format!(
            "build the DRC pack first: floe-index drc {}",
            source.display()
        ))),
    }
}

pub fn run(c: Command, stop: &Arc<AtomicUsize>) -> Result<i32> {
    let report = |s: String| eprintln!("{s}");
    match prepare(&c, stop, &report) {
        Ok(()) => Ok(0),
        Err(e) if e.kind == ErrorKind::Cancelled => {
            eprintln!("DRC preprocessing cancelled; completed rule caches are retained.");
            Ok(130)
        }
        Err(e) => {
            eprintln!("DRC preprocessing failed: {}", e.message);
            Ok(1)
        }
    }
}

fn prepare(c: &Command, stop: &AtomicUsize, report: &(dyn Fn(String) + Sync)) -> Result<()> {
    let jobs = worker_count(c.jobs)?;
    let metadata: Option<Value> = match &c.rules_json {
        Some(p) if c.delta => Some(floe_app_core::desktop::rules_sidecar(p)?.0),
        _ => None,
    };
    let pack_path = pack_of(&c.source)?;
    // a reader of the pack (a re-pack waits for it, as for a review)
    let _reader = floe_app_core::drc::pack_reader_lock(&pack_path)?;
    let pack = Pack::open(&pack_path, stop)?;
    let names: Vec<&str> = pack.checks.iter().map(|ch| ch.name.as_str()).collect();
    let missing: Vec<&str> = c
        .rules
        .iter()
        .map(String::as_str)
        .filter(|r| !names.contains(r))
        .collect();
    if !missing.is_empty() {
        return Err(Error::input(format!("unknown DRC rules: {}", missing.join(", "))));
    }
    let cache = Cache::of_pack(&pack_path)?;
    report(format!("Analysis cache: {}", cache.dir.display()));
    let source = PackSource(pack.blocks()?);
    let wanted: Vec<usize> = (0..pack.checks.len())
        .filter(|&ci| c.rules.is_empty() || c.rules.iter().any(|r| r == names[ci]))
        .collect();
    let nonempty: Vec<usize> = wanted
        .iter()
        .copied()
        .filter(|&ci| source.count(ci) > 0)
        .collect();
    let skipped = wanted.len() - nonempty.len();
    if skipped > 0 {
        report(format!("Skipping {skipped} empty rules"));
    }
    for (number, &ci) in nonempty.iter().enumerate() {
        check_cancelled(stop)?;
        let n = source.count(ci);
        report(format!("[{}/{}] {}: {n} errors", number + 1, nonempty.len(), names[ci]));
        if c.spatial {
            points::visit_centers(&source, ci, Some(&cache), jobs, stop, &|_, _| Ok(()))?;
            if points::cached_centers(&cache, ci, n, source.precision()).is_some() {
                report("  marker centres ready".into());
            } else {
                report("  marker centres: the analysis folder could not be written".into());
            }
        }
        if c.delta {
            let constraints: Vec<Value> = metadata
                .as_ref()
                .and_then(|m| m.get("checks"))
                .and_then(|checks| checks.get(names[ci]))
                .and_then(|r| r.get("constraints"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if constraints.is_empty() {
                report("  CD skipped: no matching measurement constraints".into());
                continue;
            }
            let table = Arc::new(Table::measure(
                &source,
                ci,
                &constraints,
                Some(&cache),
                true,
                jobs,
                stop,
                &|text| report(format!("  {text}")),
            )?);
            if !table.measurable {
                report("  CD skipped: the rule's constraints measure nothing".into());
                continue;
            }
            for basis in [Basis::Absolute, Basis::Percent] {
                let groups = Groups::build(Arc::clone(&table), None, basis, None, "all", stop)?;
                report(format!("  {}: {} groups ready", basis.name(), groups.len()));
            }
        }
    }
    report(format!(
        "DRC analysis ready: {} rules prepared; {skipped} empty rules skipped",
        nonempty.len()
    ));
    Ok(())
}
