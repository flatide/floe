//! `drc-match`: exact geometry matching of review state between two DRC
//! runs (app-core drc::matcher; feature/jobdeck-drc's floe-index drc-match,
//! protocol 2). The viewer's "review from previous run" calls the library
//! directly; this command serves the reference (tools/oracle drc_migrate.py)
//! and scripts.
use floe_app_core::{
    drc::{matcher, Pack},
    Error, Result,
};
use std::collections::HashSet;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicUsize, Arc};

const HELP: &str = "Usage: floe2-web drc-match OLD NEW --plan PAIRS.json --states FLAGS.u8 --out RECORDS.bin --work-dir DIR [OPTIONS]
       floe2-web drc-match --protocol

Match the errors of two DRC packs (OLD: the previous run, NEW: this one) by
exact geometry, rule pairs from PAIRS.json ([[old_rule,new_rule], ...],
same unique names), the previous review from FLAGS.u8 (one byte an OLD
error: V=1, W=2, NR=4). RECORDS.bin gets 16-byte rows sorted by (rule,
error): u32 rule, u8 flags, u8 disposition (1 = hash collision), u8
unknown judgements (W=2, NR=4), u8 0, u64 error.

  --jobs N          hashing workers, 1..256 (default min(4, CPUs))
  --memory-mib N    working memory, 4..65536 (default 64)
  --progress PATH   {\"phase\",\"processed\",\"total\"} rewritten as it goes
  --report PATH     the counts as one JSON line
  --protocol        print the output protocol (2)

Outputs must be new files, distinct from the inputs and each other.";

pub struct Command {
    old: PathBuf,
    new: PathBuf,
    plan: PathBuf,
    states: PathBuf,
    out: PathBuf,
    work_dir: PathBuf,
    progress: Option<PathBuf>,
    report: Option<PathBuf>,
    options: matcher::Options,
    protocol: bool,
}

pub fn parse(args: &[String]) -> Result<Command> {
    let rest = &args[1..];
    let mut c = Command {
        old: PathBuf::new(),
        new: PathBuf::new(),
        plan: PathBuf::new(),
        states: PathBuf::new(),
        out: PathBuf::new(),
        work_dir: PathBuf::new(),
        progress: None,
        report: None,
        options: matcher::Options::default(),
        protocol: false,
    };
    if rest == ["--protocol"] {
        c.protocol = true;
        return Ok(c);
    }
    if rest.iter().any(|a| a == "-h" || a == "--help") {
        return Err(Error::input(crate::named(HELP)));
    }
    if rest.len() < 2 || rest[0].starts_with("--") || rest[1].starts_with("--") {
        return Err(Error::input(crate::named(HELP)));
    }
    c.old = rest[0].clone().into();
    c.new = rest[1].clone().into();
    let (mut plan, mut states, mut out, mut work) = (None, None, None, None);
    let mut i = 2;
    while i < rest.len() {
        let value = rest
            .get(i + 1)
            .ok_or_else(|| Error::input(format!("{} requires a value", rest[i])))?;
        match rest[i].as_str() {
            "--plan" => plan = Some(PathBuf::from(value)),
            "--states" => states = Some(PathBuf::from(value)),
            "--out" => out = Some(PathBuf::from(value)),
            "--work-dir" => work = Some(PathBuf::from(value)),
            "--progress" => c.progress = Some(value.into()),
            "--report" => c.report = Some(value.into()),
            "--jobs" => {
                c.options.jobs = value
                    .parse()
                    .map_err(|_| Error::input("invalid match worker count"))?
            }
            "--memory-mib" => {
                c.options.memory_mib = value
                    .parse()
                    .map_err(|_| Error::input("invalid match memory budget"))?
            }
            option => return Err(Error::input(format!("unknown drc-match option {option}"))),
        }
        i += 2;
    }
    c.plan = plan.ok_or_else(|| Error::input("--plan is required"))?;
    c.states = states.ok_or_else(|| Error::input("--states is required"))?;
    c.out = out.ok_or_else(|| Error::input("--out is required"))?;
    c.work_dir = work.ok_or_else(|| Error::input("--work-dir is required"))?;
    Ok(c)
}

/// A path as it will resolve (an existing one canonical, else its folder).
fn target(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(std::fs::canonicalize(path)?);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(std::fs::canonicalize(parent)?.join(
        path.file_name()
            .ok_or_else(|| Error::input("missing output filename"))?,
    ))
}

fn open(path: &Path, cancelled: &AtomicUsize) -> Result<Pack> {
    Pack::open(path, cancelled).map_err(|e| {
        Error::new(
            e.kind,
            format!("exact migration requires a valid version 4 DRC pack: {}", e.message),
        )
    })
}

pub fn run(c: Command, cancelled: &Arc<AtomicUsize>) -> Result<i32> {
    if c.protocol {
        println!("{}", matcher::PROTOCOL);
        return Ok(0);
    }
    let inputs = [&c.old, &c.new, &c.plan, &c.states];
    let identities = inputs.iter().map(|p| target(p)).collect::<Result<Vec<_>>>()?;
    let mut outputs = HashSet::new();
    for path in [Some(&c.out), c.progress.as_ref(), c.report.as_ref()].into_iter().flatten() {
        let identity = target(path)?;
        if identities.contains(&identity) || !outputs.insert(identity) || path.exists() {
            return Err(Error::input(
                "migration outputs must be fresh files distinct from all inputs and each other",
            ));
        }
    }
    let old = open(&c.old, cancelled)?;
    let new = open(&c.new, cancelled)?;
    let states = std::fs::read(&c.states)?;
    let pairs = matcher::parse_pairs(&std::fs::read(&c.plan)?)?;
    let mut file = match &c.progress {
        Some(p) => Some(std::fs::OpenOptions::new().write(true).create_new(true).open(p)?),
        None => None,
    };
    let mut progress = |phase: &str, processed: u64, total: u64| {
        if let Some(f) = file.as_mut() {
            let text = format!("{{\"phase\":\"{phase}\",\"processed\":{processed},\"total\":{total}}}\n");
            let _ = f
                .seek(SeekFrom::Start(0))
                .and_then(|_| f.write_all(text.as_bytes()))
                .and_then(|_| f.set_len(text.len() as u64));
        }
    };
    let stats = matcher::match_packs(
        &old,
        &new,
        &pairs,
        &states,
        &c.out,
        &c.work_dir,
        c.options,
        cancelled,
        &mut progress,
    )?;
    if let Some(path) = &c.report {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?
            .write_all(stats.json().as_bytes())?;
    }
    eprintln!(
        "[drc-match] {} matched target errors; {} ambiguous; {} invalid old, {} invalid new",
        stats.matched_target, stats.ambiguous_target, stats.invalid_old, stats.invalid_new
    );
    Ok(0)
}
