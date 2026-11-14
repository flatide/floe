//! Existing scripting output remains streamed; no browser/GTK dependencies.
use floe_app_core::{
    check_cancelled,
    drc::{build, open_current},
    svrf::Rules,
    Error, ErrorKind, Result,
};
use std::{
    io::{self, Write},
    path::PathBuf,
    sync::atomic::AtomicUsize,
};
const HELP: &str = "Usage: floe2-web drc RESULTS.db|.RESULTS.db.tray [OPTIONS]
  --list                  Also list every error's centre and size
  --rules                 Rule JSON [{name,errors,waived},...]
  --errs RULE             Stream one rule's error JSON (first duplicate)
  --floe-reviewer TAG     Existing per-reviewer waive sidecar selection
                          (required where FLOE_REVIEWER_REQUIRED is set)
  --review-stats          Review statistics JSON of every reviewer found by
                          the working files beside the pack - each one's
                          waived (the statuses counted again, not the stored
                          counters), visited, not for review and pending,
                          whole and by rule - and the team's as the viewer's
                          reviewers window counts them (visits united,
                          conflicts between visited reviewers; the waives a
                          confirmer has confirmed, and the ones that wait
                          for it: `confirmed`, `unconfirmed`; the errors
                          whose waives a confirmer has rejected and no
                          reviewer has waived again since, which are none
                          of the team's waives: `rejected`; where requesters
                          asked for waives (floe2 view --role): the errors
                          asked for and the ones of them nobody has waived,
                          `requested` and `unwaived`, the ones whose requests
                          a waiver declined and no requester has asked again,
                          `declined`, and a requester's own `requested` and
                          `requested_declined`); with
                          --floe-reviewer ID[,ID...] these reviewers alone.
                          Needs the current pack: none or a stale one is an
                          error (exit 1), never an answer of zeros
  --review-counts         The same reviewers' counts for an answer at once
                          (JSON): each one's errors, waived, visited and not
                          for review of the whole result, read from the
                          counters the review files keep - every waive, visit
                          and exclusion brings them up to date as it is
                          written -, so no error is read and the time does
                          not grow with the result. `team`: everyone's review
                          united - visited, waived, not for review, conflicts,
                          pending, confirmed and unconfirmed as
                          --review-stats counts them -, kept
                          in .DB.team beside the pack and brought up to date
                          by what the reviewers changed since (`current` is
                          false while a change is not in it yet). With
                          --rule NAME also the rule's numbers, with
                          --all-rules every rule's; with --floe-reviewer
                          ID[,ID...] these reviewers' rows alone - the team
                          is always everyone's. The reviewers' own counters
                          are not counted again (--review-stats does), and no
                          reasons are counted. FLOE_TEAM_KEEP=off: .DB.team
                          is not written, the team counted for each answer
  --rule NAME             With --review-counts: list this rule. With
                          --team-counts: this rule alone
  --all-rules             With --review-counts: list every rule. With
                          --team-counts: the rules without errors too
  --team-counts           The team's counts in brief, one line of JSON to hand
                          on as it is: `total`, then `rule` - an object of
                          the rules by name -, each with errors, requested,
                          declined, waived, confirmed, rejected, visited,
                          ignored. No blank in
                          it, the keys in this order, the rules in the
                          result's. `total` is the whole result's; `rule` has
                          the rules that have errors - with --all-rules every
                          rule, with --rule NAME that rule alone (a name the
                          result has twice is one entry). Everyone's review
                          united, from the counts
                          --review-counts keeps (.DB.team): requested = the
                          waive asked for by somebody (a requester: floe2
                          view --role), waived since or not - a request a
                          waiver declined is none; declined = the errors
                          whose requests are all declined and nobody has
                          waived; waived = waived by
                          somebody and not rejected, confirmed or not;
                          confirmed = of those, confirmed; rejected = the
                          errors whose waives are all rejected; ignored = set
                          aside from the review (not for review) by
                          somebody. What is not in the counts yet, and the
                          files left out, are said on stderr
  --out FILE              With --team-counts: the line goes into FILE and
                          nothing to stdout - for a caller that takes only so
                          much of a command's output. FILE is replaced whole
                          (written beside it, then renamed: a reader never
                          has part of an answer). It has to be new, empty or
                          an earlier answer: any other file is refused and
                          left as it is. When there is no answer (any exit
                          but 0 and an argument error's 2), an earlier one
                          in FILE is removed: FILE is this call's answer, or
                          it is not there
  --svrf-rules FILE       Explicit rules.json metadata for --rules / --errs
  --build                 Explicitly build/reuse .RESULTS.db.tray, migrate legacy name.
                          A pack is reused when the database is the file that
                          was indexed: its size and time, or - another time (a
                          copy, a touch) - its content, compared here whatever
                          its size (CRC-32); a database that did change is
                          refused without --force
  --force                 With --build: allow atomic replacement of old pack
  --jobs N                With --build: native workers 1..16 (default 12)
  --workdir DIR           Read and write the pack and what follows it (the
                          .analysis folder, the review .sqlite3 / waive / notes
                          sidecars) under DIR at the database's absolute path
                          (DIR/a/.r.db.tray), not beside the database
                          (FLOE_WORKDIR; every command's, anywhere on the line)

Without --build: read-only, no renaming, automatic indexing or autosave creation.
Uses a fresh layout-5 .tray pack (or legacy .ice) when available, otherwise bounded read-only
ASCII parsing (including fractional coordinates). Stale/corrupt adjacent
packs are reported but never overwritten by read commands.
Fresh: the database has the size and time it was indexed with, or the same
content under another time (a copy, a touch) - then it is read once and
compared (CRC-32), and the answer kept as .RESULTS.db.tray.source so the next
open reads nothing. Reviews are bound to the pack's content id (what the
errors are): a pack made again, a copied folder and a re-run with the same
errors keep them.
--review-counts and --team-counts keep one file beside the pack, the team's counts
(.RESULTS.db.team; none where the folder takes no file, or with
FLOE_TEAM_KEEP=off): a kept answer, like .RESULTS.db.tray.source - no review
is changed by it. --team-counts --out FILE writes FILE.
--build uses the existing integer-DBU pack format; fractional coordinates fail
without changing the old pack. It preserves review files and emits JSON.
Name migration is reported separately and is not rolled back if rebuilding fails.
Close readers in other processes before building; reader leases are app-local.
Interactive notes/waives use the web view's explicit reviewer/edit opt-ins;
this drc inspection command does not edit reviews.";
pub struct Command {
    source: Option<PathBuf>,
    list: bool,
    rules: bool,
    errs: Option<String>,
    reviewer: Option<String>,
    svrf_rules: Option<PathBuf>,
    build: bool,
    force: bool,
    jobs: Option<usize>,
    review_stats: bool,
    review_counts: bool,
    team_counts: bool,
    rule: Option<String>,
    all_rules: bool,
    out: Option<PathBuf>,
}
pub fn parse(args: &[String]) -> Result<Command> {
    let mut c = Command {
        source: None,
        list: false,
        rules: false,
        errs: None,
        reviewer: None,
        svrf_rules: None,
        build: false,
        force: false,
        jobs: None,
        review_stats: false,
        review_counts: false,
        team_counts: false,
        rule: None,
        all_rules: false,
        out: None,
    };
    let mut i = 1;
    let mut positional = false;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if !positional && a == "--" {
            positional = true;
            continue;
        }
        if positional || !a.starts_with('-') {
            if c.source.replace(PathBuf::from(a)).is_some() {
                return Err(Error::input("drc accepts exactly one source"));
            }
            continue;
        }
        let (flag, inline) = a
            .split_once('=')
            .map_or((a.as_str(), None), |(f, v)| (f, Some(v)));
        if ["--help", "-h"].contains(&flag) && inline.is_none() {
            return Ok(Command { source: None, ..c });
        }
        match flag {
            "--build" | "--force" if inline.is_none() => {
                if flag == "--build" {
                    c.build = true;
                } else {
                    c.force = true;
                }
            }
            "--list" | "--rules" if inline.is_none() => {
                if flag == "--list" {
                    c.list = true
                } else {
                    c.rules = true
                }
            }
            "--review-stats" if inline.is_none() => c.review_stats = true,
            "--review-counts" if inline.is_none() => c.review_counts = true,
            "--team-counts" if inline.is_none() => c.team_counts = true,
            "--all-rules" if inline.is_none() => c.all_rules = true,
            "--errs" | "--floe-reviewer" | "--svrf-rules" | "--jobs" | "--rule" | "--out" => {
                let value = if let Some(s) = inline {
                    s
                } else {
                    let s = args
                        .get(i)
                        .filter(|s| !s.starts_with("--"))
                        .ok_or_else(|| Error::input(format!("{flag} requires a value")))?;
                    i += 1;
                    s
                };
                if value.is_empty() {
                    return Err(Error::input(format!("{flag} requires a nonempty value")));
                }
                if flag == "--jobs" {
                    c.jobs = Some(
                        value
                            .parse()
                            .ok()
                            .filter(|n| (1..=16).contains(n))
                            .ok_or_else(|| Error::input("DRC pack jobs must be 1..16"))?,
                    );
                } else if flag == "--errs" {
                    c.errs = Some(value.into())
                } else if flag == "--rule" {
                    if c.rule.replace(value.into()).is_some() {
                        return Err(Error::input("--rule names one rule (--all-rules lists every rule)"));
                    }
                } else if flag == "--svrf-rules" {
                    c.svrf_rules = Some(value.into())
                } else if flag == "--out" {
                    if c.out.replace(value.into()).is_some() {
                        return Err(Error::input("--out names one file"));
                    }
                } else {
                    c.reviewer = Some(value.into())
                }
            }
            _ => return Err(Error::input(format!("unsupported drc option {flag}"))),
        }
    }
    if c.source.is_none() {
        return Err(Error::input("drc requires a database"));
    }
    if c.build {
        if c.list
            || c.rules
            || c.errs.is_some()
            || c.reviewer.is_some()
            || c.svrf_rules.is_some()
            || c.review_stats
            || c.review_counts
            || c.team_counts
            || c.rule.is_some()
            || c.all_rules
        {
            return Err(Error::input(
                "--build cannot be combined with DRC read/reviewer options",
            ));
        }
    } else if c.force || c.jobs.is_some() {
        return Err(Error::input("--force/--jobs require explicit --build"));
    }
    if c.review_stats && (c.list || c.rules || c.errs.is_some() || c.svrf_rules.is_some()) {
        return Err(Error::input(
            "--review-stats is a report of its own: without --list / --rules / --errs / --svrf-rules",
        ));
    }
    if c.review_counts
        && (c.review_stats || c.list || c.rules || c.errs.is_some() || c.svrf_rules.is_some())
    {
        return Err(Error::input(
            "--review-counts is a report of its own: without --review-stats / --list / --rules / --errs / --svrf-rules",
        ));
    }
    if c.team_counts
        && (c.review_counts || c.review_stats || c.list || c.rules || c.errs.is_some() || c.svrf_rules.is_some())
    {
        return Err(Error::input(
            "--team-counts is a report of its own: without --review-counts / --review-stats / --list / --rules / --errs / --svrf-rules",
        ));
    }
    if (c.rule.is_some() || c.all_rules) && !(c.review_counts || c.team_counts) {
        return Err(Error::input("--rule / --all-rules require --review-counts or --team-counts"));
    }
    if c.rule.is_some() && c.all_rules {
        return Err(Error::input("--rule and --all-rules: one of the two"));
    }
    if c.out.is_some() && !c.team_counts {
        return Err(Error::input("--out requires --team-counts"));
    }
    if c.svrf_rules.is_some() && (c.list || (c.rules == c.errs.is_some())) {
        return Err(Error::input(
            "--svrf-rules requires exactly one of --rules / --errs (without --list)",
        ));
    }
    Ok(c)
}
fn json(w: &mut impl Write, v: &serde_json::Value) -> Result<()> {
    serde_json::to_writer(w, v).map_err(|e| Error::new(floe_app_core::ErrorKind::Io, e.to_string()))
}
fn rounded(x: f64) -> f64 {
    format!("{x:.4}")
        .parse()
        .expect("formatted finite coordinate")
}
pub fn run(command: Command, cancelled: &AtomicUsize) -> Result<i32> {
    let Some(source) = command.source else {
        println!("{}", crate::named(HELP));
        return Ok(0);
    };
    if command.build {
        return build_pack(
            &source,
            build::Options {
                jobs: command.jobs.unwrap_or(12),
                force: command.force,
            },
            cancelled,
        );
    }
    if command.review_stats {
        return review_stats(&source, command.reviewer.as_deref(), cancelled);
    }
    if command.team_counts {
        return team_counts(
            &source,
            command.rule.as_deref(),
            command.all_rules,
            command.out.as_deref(),
            cancelled,
        );
    }
    if command.review_counts {
        return review_counts(
            &source,
            command.reviewer.as_deref(),
            command.rule.as_deref(),
            command.all_rules,
            cancelled,
        );
    }
    let mut p = open_current(&source, command.reviewer.as_deref(), cancelled)?;
    for warning in &p.warnings {
        eprintln!("[drc] {warning}");
    }
    let metadata = command
        .svrf_rules
        .as_deref()
        .map(|path| Rules::load(path, cancelled))
        .transpose()?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    if command.rules {
        writeln!(out, "[")?;
        for i in 0..p.check_count() {
            check_cancelled(cancelled)?;
            let c = p.check(i)?;
            if i > 0 {
                writeln!(out, ",")?;
            }
            let mut row =
                serde_json::json!({"name":c.name,"errors":c.count,"waived":p.waived_count(i)?});
            if let Some(m) = &metadata {
                row["svrf"] = serde_json::to_value(m.detail(c.name, cancelled)?)
                    .map_err(|e| Error::input(e.to_string()))?;
            }
            json(&mut out, &row)?;
        }
        writeln!(out, "\n]")?;
    } else if let Some(rule) = command.errs {
        let hits: Vec<_> = (0..p.check_count())
            .filter(|&i| p.check(i).is_ok_and(|c| c.name == rule))
            .collect();
        let ci = *hits
            .first()
            .ok_or_else(|| Error::input(format!("no such rule {rule:?} (see --rules)")))?;
        if hits.len() > 1 {
            eprintln!(
                "[{}][warn] rule {rule:?} appears {} times - using the first",
                floe_app_core::program(),
                hits.len()
            );
        }
        writeln!(out, "[")?;
        let mut start = 0;
        loop {
            let page = p.errors(ci, start, 64, cancelled)?;
            for hit in page.hits {
                let e = hit.violation;
                if hit.local > 0 {
                    writeln!(out, ",")?;
                }
                let mut row = serde_json::json!({"local":hit.local+1,"global":e.number,"kind":e.kind.to_string(),"status":hit.status,"bbox":e.bbox_um.map(rounded)});
                if let Some(m) = &metadata {
                    let comparison = m
                        .rule(&rule)
                        .map(|r| e.comparison(r, cancelled))
                        .transpose()?
                        .flatten();
                    row["comparison"] = serde_json::to_value(comparison)
                        .map_err(|e| Error::input(e.to_string()))?;
                }
                json(&mut out, &row)?;
            }
            if let Some(next) = page.next {
                start = next.error;
            } else {
                break;
            }
        }
        writeln!(out, "\n]")?;
    } else {
        writeln!(
            out,
            "{}: cell {}, precision {}",
            p.path().display(),
            p.cell(),
            floe_app_core::format_general(p.precision())
        )?;
        writeln!(out, "{} checks, {} errors", p.check_count(), p.total())?;
        for ci in 0..p.check_count() {
            check_cancelled(cancelled)?;
            let c = p.check(ci)?;
            writeln!(
                out,
                " {:<28} {:6}  {}",
                c.name,
                c.count,
                c.desc.lines().next().unwrap_or("")
            )?;
            if command.list {
                let mut start = 0;
                loop {
                    let page = p.errors(ci, start, 64, cancelled)?;
                    for hit in page.hits {
                        let e = hit.violation;
                        let b = e.bbox_um;
                        writeln!(
                            out,
                            "   #{:<5} {:<4} ({:.3}, {:.3}) um  {:.3} x {:.3}",
                            e.number,
                            if e.kind == 'p' { "poly" } else { "edge" },
                            b[0] / 2. + b[2] / 2.,
                            b[1] / 2. + b[3] / 2.,
                            b[2] - b[0],
                            b[3] - b[1]
                        )?;
                    }
                    if let Some(next) = page.next {
                        start = next.error;
                    } else {
                        break;
                    }
                }
            }
        }
    }
    p.unchanged()?;
    out.flush()?;
    Ok(0)
}
/// `--floe-reviewer ID[,ID...]` of a statistics report: the ids named.
#[cfg(feature = "review-sqlite")]
fn reviewers_named(reviewers: Option<&str>) -> Result<Option<Vec<String>>> {
    let only: Option<Vec<String>> = reviewers.map(|list| {
        list.split(',')
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_owned)
            .collect()
    });
    if only.as_ref().is_some_and(Vec::is_empty) {
        return Err(Error::input("--floe-reviewer names no reviewer"));
    }
    Ok(only)
}
/// `--review-counts`: the counts the reviewers' files keep and the team's
/// kept union, as one JSON document on stdout (app-core drc::team::stats
/// `counts`, drc::team::live) - the answer at once: of the result's errors
/// only the chunks a reviewer changed since the last answer are read.
#[cfg(feature = "review-sqlite")]
fn review_counts(
    source: &std::path::Path,
    reviewers: Option<&str>,
    rule: Option<&str>,
    all_rules: bool,
    cancelled: &AtomicUsize,
) -> Result<i32> {
    use floe_app_core::drc::team::stats::{counts, Listed};
    let only = reviewers_named(reviewers)?;
    let listed = match rule {
        Some(name) => Listed::Named(name),
        None if all_rules => Listed::All,
        None => Listed::Whole,
    };
    let report = counts(source, only.as_deref(), listed, cancelled)?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    serde_json::to_writer_pretty(&mut out, &report)
        .map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
    writeln!(out)?;
    out.flush()?;
    Ok(0)
}
#[cfg(not(feature = "review-sqlite"))]
fn review_counts(
    _source: &std::path::Path,
    _reviewers: Option<&str>,
    _rule: Option<&str>,
    _all_rules: bool,
    _cancelled: &AtomicUsize,
) -> Result<i32> {
    Err(Error::new(
        ErrorKind::Unsupported,
        "--review-counts is not in this build (the reviewers' flags are read by floe2)",
    ))
}
/// `--team-counts`: the team's kept union in brief, one line of JSON on
/// stdout and nothing else there (app-core drc::team::stats `brief`) - a
/// caller hands it on as it is. What there is to say beside it goes to
/// stderr. With `--out FILE` the line goes into the file and stdout has
/// nothing: for a caller that takes only so much of a command's output
/// (`out_held`, `out_put`, `out_retire`: the file is this call's answer or
/// it is not there).
#[cfg(feature = "review-sqlite")]
fn team_counts(
    source: &std::path::Path,
    rule: Option<&str>,
    all_rules: bool,
    out: Option<&std::path::Path>,
    cancelled: &AtomicUsize,
) -> Result<i32> {
    use floe_app_core::drc::team::stats::{brief, BriefRules};
    let listed = match rule {
        Some(name) => BriefRules::Named(name),
        None if all_rules => BriefRules::All,
        None => BriefRules::WithErrors,
    };
    // the file named is looked at before anything is done
    let held = match out {
        Some(path) => Some((path, out_held(path)?)),
        None => None,
    };
    let answered = brief(source, listed, cancelled).and_then(|(text, said)| {
        for line in said {
            eprintln!("[drc] team counts: {line}");
        }
        match held {
            Some((path, _)) => out_put(path, &text),
            None => {
                let mut out = io::stdout().lock();
                out.write_all(text.as_bytes())?;
                out.write_all(b"\n")?;
                Ok(out.flush()?)
            }
        }
    });
    if let (Err(_), Some((path, held))) = (&answered, held) {
        out_retire(path, held);
    }
    answered.map(|()| 0)
}
/// What the file `--team-counts --out` names is before the call.
#[cfg(feature = "review-sqlite")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    /// no file there
    Nothing,
    /// an earlier answer, or an empty file (a caller's `mktemp`): this
    /// call's to replace - and to remove, when it has no answer to give
    Ours,
}
/// Looks at the file `--out` names. It is written over only when it is an
/// earlier answer (it starts as every answer does) or empty: a database, a
/// review file or whatever else was named by mistake is refused and left as
/// it is - the account these commands run under may write everybody's
/// files.
#[cfg(feature = "review-sqlite")]
fn out_held(path: &std::path::Path) -> Result<Held> {
    use floe_app_core::drc::team::stats::BRIEF_HEAD;
    use std::io::Read;
    let refused = |why: String| Error::input(format!("--out {}: {why}", path.display()));
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Held::Nothing),
        Err(e) => return Err(refused(format!("cannot be looked at ({e})"))),
    };
    if meta.is_dir() {
        return Err(refused("is a folder".into()));
    }
    if !meta.is_file() {
        return Err(refused("is not a regular file".into()));
    }
    let mut head = Vec::with_capacity(BRIEF_HEAD.len());
    std::fs::File::open(path)
        .and_then(|f| f.take(BRIEF_HEAD.len() as u64).read_to_end(&mut head))
        .map_err(|e| refused(format!("cannot be read ({e})")))?;
    if head.is_empty() || head == BRIEF_HEAD.as_bytes() {
        return Ok(Held::Ours);
    }
    Err(refused(
        "the file there is no earlier --team-counts answer - it is not written over (name another file, or remove it)"
            .into(),
    ))
}
/// Writes the answer into the file `--out` names: whole into a temporary
/// beside it (`.<name>.tmp-<pid>`), which is then renamed over it - who
/// reads the file has an answer from its first byte to its last, never part
/// of one, however many calls write it at once. Not flushed to the disk: an
/// answer lost with the host is asked for again.
#[cfg(feature = "review-sqlite")]
fn out_put(path: &std::path::Path, text: &str) -> Result<()> {
    let folder = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = folder.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = std::fs::File::create(&tmp)
        .and_then(|mut f| {
            f.write_all(text.as_bytes())?;
            f.write_all(b"\n")
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::new(
            ErrorKind::Io,
            format!("--out {}: cannot be written ({e})", path.display()),
        ));
    }
    Ok(())
}
/// The call has no answer to give: what the file `--out` names held - an
/// earlier answer, an empty file - is removed, and that is said. A caller
/// that reads the file after a failure it did not look at finds no file,
/// not the numbers of another moment - or of another result: a pack that is
/// stale because the database changed is such a failure.
#[cfg(feature = "review-sqlite")]
fn out_retire(path: &std::path::Path, held: Held) {
    if held != Held::Ours {
        return;
    }
    match std::fs::remove_file(path) {
        Ok(()) => eprintln!(
            "[drc] team counts: no answer this time: {} is removed",
            path.display()
        ),
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => eprintln!(
            "[drc] team counts: no answer this time, and {} could not be removed ({e}): what it holds is not this call's",
            path.display()
        ),
    }
}
#[cfg(not(feature = "review-sqlite"))]
fn team_counts(
    _source: &std::path::Path,
    _rule: Option<&str>,
    _all_rules: bool,
    _out: Option<&std::path::Path>,
    _cancelled: &AtomicUsize,
) -> Result<i32> {
    Err(Error::new(
        ErrorKind::Unsupported,
        "--team-counts is not in this build (the reviewers' flags are read by floe2)",
    ))
}
/// `--review-stats`: the reviewers' statistics of the database's current
/// pack as one JSON document on stdout (app-core drc::team::stats).
#[cfg(feature = "review-sqlite")]
fn review_stats(
    source: &std::path::Path,
    reviewers: Option<&str>,
    cancelled: &AtomicUsize,
) -> Result<i32> {
    let only = reviewers_named(reviewers)?;
    let report =
        floe_app_core::drc::team::stats::collect(source, only.as_deref(), cancelled)?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    serde_json::to_writer_pretty(&mut out, &report)
        .map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
    writeln!(out)?;
    out.flush()?;
    Ok(0)
}
#[cfg(not(feature = "review-sqlite"))]
fn review_stats(
    _source: &std::path::Path,
    _reviewers: Option<&str>,
    _cancelled: &AtomicUsize,
) -> Result<i32> {
    Err(Error::new(
        ErrorKind::Unsupported,
        "--review-stats is not in this build (the reviewers' flags are read by floe2)",
    ))
}
fn build_pack(
    source: &std::path::Path,
    options: build::Options,
    cancelled: &AtomicUsize,
) -> Result<i32> {
    use floe_app_core::{
        managed::{Limits, Resources},
        native::{Discovery, Indexer},
        registered::AccessScope,
    };
    use std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    let source = floe_app_core::cache::absolute(source)?;
    let scope = AccessScope::new(&[source
        .parent()
        .ok_or_else(|| Error::input("DRC source needs a parent"))?
        .to_owned()])?;
    let indexer = Indexer::discover(&Discovery::local()?)?;
    // Standalone CLI has no foreground workers; the managed service uses its
    // own shared limits instead. The native build itself never exceeds jobs.
    let resources = Resources::new(Limits {
        cpu_slots: options.jobs as u32 + 4,
        ..Limits::default()
    })?;
    check_cancelled(cancelled)?;
    let mut job = build::Build::start(&resources, scope, &source, options, indexer)?;
    let mut shown = None;
    let mut next_log = Instant::now();
    loop {
        if cancelled.load(Ordering::Relaxed) != 0 {
            job.cancel();
        }
        let s = job.snapshot();
        if shown != Some(s.phase) || Instant::now() >= next_log {
            eprintln!(
                "[{}] DRC pack {:?} ({} ms; checks {:?}/{:?}, errors {:?})",
                floe_app_core::program(),
                s.phase,
                s.elapsed_ms,
                s.native.drc_checks,
                s.native.drc_total_checks,
                s.native.drc_errors
            );
            shown = Some(s.phase);
            next_log = Instant::now() + Duration::from_secs(1);
        }
        if s.terminal() {
            job.close()?;
            if let Some(outcome) = s.outcome {
                println!(
                    "{}",
                    serde_json::json!({"source":source,"pack":job.output(),
                    "reused":outcome.reused,"checks":outcome.checks,"errors":outcome.errors,
                    "bytes":outcome.bytes,"directory_synced":outcome.directory_synced,
                    "cleanup_warning":s.cleanup_warning,"migration":s.migration})
                );
                return Ok(0);
            }
            let message = if s.native.drc_noninteger {
                "DRC pack requires integer DBU coordinate tokens; keep using read-only ASCII for fractional input".to_string()
            } else if matches!(s.failure, Some(ErrorKind::Busy | ErrorKind::Io)) {
                // the pack's lock refused the build (app-core drc/build.rs):
                // the `[lock]` line says who is in the way, as floe-index
                // does; a folder that cannot be written (Io): the lease's
                // words name it and --workdir (cache::write_refusal)
                job.failure_message()
                    .filter(|m| !m.is_empty())
                    .unwrap_or_else(|| "DRC pack build did not publish an output".to_string())
            } else {
                "DRC pack build did not publish an output".to_string()
            };
            return Err(Error::new(s.failure.unwrap_or(ErrorKind::Worker), message));
        }
        if job.is_finished() {
            job.close()?;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn options() {
        let parse_str = |a: &[&str]| parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(parse_str(&["drc", "--help"]).unwrap().source.is_none());
        let c = parse_str(&[
            "drc",
            "한 글.ice",
            "--errs=M1.WIDTH",
            "--floe-reviewer",
            "Kim",
        ])
        .unwrap();
        assert_eq!(c.errs.as_deref(), Some("M1.WIDTH"));
        assert_eq!(c.reviewer.as_deref(), Some("Kim"));
        assert!(parse_str(&["drc", "a", "b"]).is_err());
        assert!(parse_str(&["drc", "a", "--errs"]).is_err());
        assert!(parse_str(&["drc", "a", "--rules=true"]).is_err());
        assert!(parse_str(&["drc", "a", "--svrf-rules", "x"]).is_err());
        assert!(parse_str(&["drc", "a", "--svrf-rules", "x", "--rules", "--errs", "r"]).is_err());
        assert!(parse_str(&["drc", "a", "--svrf-rules", "x", "--rules", "--list"]).is_err());
        assert!(parse_str(&["drc", "a", "--svrf-rules=한 글.rules.json", "--rules"]).is_ok());
        // the reviewers' statistics: a report of its own, of everyone or of
        // the reviewers named
        let c = parse_str(&["drc", "a.db", "--review-stats"]).unwrap();
        assert!(c.review_stats && c.reviewer.is_none());
        let c = parse_str(&["drc", "a.db", "--review-stats", "--floe-reviewer", "kim,lee"]).unwrap();
        assert_eq!(c.reviewer.as_deref(), Some("kim,lee"));
        for with in ["--rules", "--list", "--errs=R", "--build", "--svrf-rules=x"] {
            assert!(parse_str(&["drc", "a.db", "--review-stats", with]).is_err(), "{with}");
        }
        assert!(parse_str(&["drc", "a.db", "--review-stats=on"]).is_err());
        // the kept counts: a report of its own, one rule or every rule with it
        let c = parse_str(&["drc", "a.db", "--review-counts"]).unwrap();
        assert!(c.review_counts && c.rule.is_none() && !c.all_rules && !c.review_stats);
        let c = parse_str(&["drc", "a.db", "--review-counts", "--rule", "M1.S.1", "--floe-reviewer=kim"]).unwrap();
        assert_eq!((c.rule.as_deref(), c.reviewer.as_deref()), (Some("M1.S.1"), Some("kim")));
        let c = parse_str(&["drc", "a.db", "--review-counts", "--rule=이름 둘", "--floe-reviewer", "kim,lee"]).unwrap();
        assert_eq!(c.rule.as_deref(), Some("이름 둘"));
        assert!(parse_str(&["drc", "a.db", "--review-counts", "--all-rules"]).unwrap().all_rules);
        for with in ["--review-stats", "--rules", "--list", "--errs=R", "--build", "--svrf-rules=x"] {
            assert!(parse_str(&["drc", "a.db", "--review-counts", with]).is_err(), "{with}");
        }
        for alone in [vec!["--rule", "R"], vec!["--all-rules"], vec!["--review-stats", "--rule=R"]] {
            let mut args = vec!["drc", "a.db"];
            args.extend(alone.iter().copied());
            assert!(parse_str(&args).is_err(), "{alone:?}");
        }
        for bad in [
            vec!["--review-counts", "--rule", "A", "--rule", "B"],
            vec!["--review-counts", "--rule", "A", "--all-rules"],
            vec!["--review-counts", "--rule"],
            vec!["--review-counts", "--rule="],
            vec!["--review-counts=1"],
            vec!["--review-counts", "--all-rules=1"],
        ] {
            let mut args = vec!["drc", "a.db"];
            args.extend(bad.iter().copied());
            assert!(parse_str(&args).is_err(), "{bad:?}");
        }
        // the team's counts in brief: a report of its own, every rule that
        // has errors or the one named
        let c = parse_str(&["drc", "a.db", "--team-counts"]).unwrap();
        assert!(c.team_counts && c.rule.is_none() && !c.review_counts && !c.review_stats);
        let c = parse_str(&["drc", "a.db", "--team-counts", "--rule=GRX.1"]).unwrap();
        assert_eq!((c.team_counts, c.rule.as_deref()), (true, Some("GRX.1")));
        let c = parse_str(&["drc", "--rule", "이름 둘", "a.db", "--team-counts", "--floe-reviewer", "kim"]).unwrap();
        assert_eq!((c.rule.as_deref(), c.reviewer.as_deref()), (Some("이름 둘"), Some("kim")));
        let c = parse_str(&["drc", "a.db", "--team-counts", "--all-rules"]).unwrap();
        assert!(c.team_counts && c.all_rules && c.rule.is_none());
        for with in ["--review-counts", "--review-stats", "--rules", "--list", "--errs=R", "--build", "--svrf-rules=x"] {
            assert!(parse_str(&["drc", "a.db", "--team-counts", with]).is_err(), "{with}");
        }
        // ... into a file
        let c = parse_str(&["drc", "a.db", "--team-counts", "--out", "counts.json"]).unwrap();
        assert_eq!(c.out.as_deref(), Some(std::path::Path::new("counts.json")));
        let c = parse_str(&["drc", "--out=한 글/c.json", "a.db", "--team-counts", "--all-rules"]).unwrap();
        assert_eq!((c.out.as_deref(), c.all_rules), (Some(std::path::Path::new("한 글/c.json")), true));
        assert!(parse_str(&["drc", "a.db", "--team-counts"]).unwrap().out.is_none());
        for alone in [vec!["--out", "c.json"], vec!["--review-counts", "--out", "c.json"], vec!["--rules", "--out=c.json"]] {
            let mut args = vec!["drc", "a.db"];
            args.extend(alone.iter().copied());
            assert!(parse_str(&args).is_err(), "{alone:?}");
        }
        for bad in [
            vec!["--team-counts=1"],
            vec!["--team-counts", "--rule"],
            vec!["--team-counts", "--rule", "A", "--rule", "B"],
            vec!["--team-counts", "--rule", "A", "--all-rules"],
            vec!["--team-counts", "--out"],
            vec!["--team-counts", "--out="],
            vec!["--team-counts", "--out", "a.json", "--out", "b.json"],
            vec!["--team-counts", "--build", "--out", "a.json"],
        ] {
            let mut args = vec!["drc", "a.db"];
            args.extend(bad.iter().copied());
            assert!(parse_str(&args).is_err(), "{bad:?}");
        }
        assert!(!parse_str(&["drc", "a.db", "--rules"]).unwrap().review_stats);
        let c = parse_str(&["drc", "a.db", "--build", "--force", "--jobs=16"]).unwrap();
        assert!(c.build && c.force);
        assert_eq!(c.jobs, Some(16));
        for flags in [
            vec!["--force"],
            vec!["--jobs=1"],
            vec!["--build", "--jobs=0"],
            vec!["--build", "--jobs=17"],
            vec!["--build", "--list"],
            vec!["--build", "--rules"],
            vec!["--build", "--errs=X"],
            vec!["--build", "--floe-reviewer=Kim"],
            vec!["--build=true"],
        ] {
            let mut args = vec!["drc", "a.db"];
            args.extend(flags);
            assert!(parse_str(&args).is_err(), "{args:?}");
        }
    }
    /// `--team-counts --out FILE`: the file is this call's answer or it is
    /// not there, and a file that is no answer is never written over.
    #[cfg(feature = "review-sqlite")]
    #[test]
    fn the_file_out_names_is_this_calls_answer_or_not_there() {
        use floe_app_core::drc::team::stats::BRIEF_HEAD;
        use std::fs;
        let dir = std::env::temp_dir().join(format!("floe-team-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let names = || {
            let mut v: Vec<String> = fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        let answer = format!("{BRIEF_HEAD}1,\"waived\":0}},\"rule\":{{}}}}");
        let file = dir.join("counts.json");
        // no file there: made whole, with the line's end, no temporary left
        assert_eq!(out_held(&file).unwrap(), Held::Nothing);
        out_put(&file, &answer).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), format!("{answer}\n"));
        assert_eq!(names(), ["counts.json"]);
        // an earlier answer: this call's to replace, whole - a shorter one
        // leaves nothing of the longer
        assert_eq!(out_held(&file).unwrap(), Held::Ours);
        let next = format!("{BRIEF_HEAD}2}},\"rule\":{{}}}}");
        out_put(&file, &next).unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), format!("{next}\n"));
        assert_eq!(names(), ["counts.json"]);
        // ... and to remove when there is no answer; gone already, nothing
        // is done
        out_retire(&file, Held::Ours);
        assert!(!file.exists());
        out_retire(&file, Held::Ours);
        // an empty file - a caller's mktemp - is taken, and removed alike
        fs::write(&file, b"").unwrap();
        assert_eq!(out_held(&file).unwrap(), Held::Ours);
        out_retire(&file, Held::Ours);
        assert!(!file.exists());
        // what was not there before the call is not this call's to remove
        fs::write(&file, b"made meanwhile").unwrap();
        out_retire(&file, Held::Nothing);
        assert_eq!(fs::read(&file).unwrap(), b"made meanwhile");
        // any other file is refused and left as it is: a database, JSON
        // that is no answer, a beginning of one, a blank
        let others: [(&str, &[u8]); 5] = [
            ("results.db", b"TOP 1000\nM1.S.1\n0 0 0 Nov 14 2026\n"),
            ("other.json", b"{\"total\":{}}"),
            ("part.json", b"{\"total\""),
            ("blank.txt", b" "),
            ("counts.json", b"made meanwhile"),
        ];
        for (name, bytes) in others {
            let other = dir.join(name);
            fs::write(&other, bytes).unwrap();
            let e = out_held(&other).unwrap_err();
            assert!(e.message.contains("no earlier --team-counts answer"), "{name}: {}", e.message);
            assert_eq!(e.kind, ErrorKind::InvalidInput, "{name}");
            assert_eq!(fs::read(&other).unwrap(), bytes, "{name}");
        }
        // a folder is none either
        let e = out_held(&dir).unwrap_err();
        assert!(e.message.contains("is a folder"), "{}", e.message);
        // a folder that is not there: nothing to look at, and nothing made
        let nowhere = dir.join("none").join("x.json");
        assert_eq!(out_held(&nowhere).unwrap(), Held::Nothing);
        let e = out_put(&nowhere, &answer).unwrap_err();
        assert!(e.message.contains("cannot be written"), "{}", e.message);
        assert_eq!(e.kind, ErrorKind::Io);
        assert!(!dir.join("none").exists());
        // a link to an earlier answer: the name is this call's, the file it
        // pointed at is left as it was
        let (kept, link) = (dir.join("kept.json"), dir.join("link.json"));
        fs::write(&kept, format!("{answer}\n")).unwrap();
        std::os::unix::fs::symlink(&kept, &link).unwrap();
        assert_eq!(out_held(&link).unwrap(), Held::Ours);
        out_put(&link, &next).unwrap();
        assert!(!fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&link).unwrap(), format!("{next}\n"));
        assert_eq!(fs::read_to_string(&kept).unwrap(), format!("{answer}\n"));
        assert!(names().iter().all(|n| !n.contains(".tmp-")), "{:?}", names());
        fs::remove_dir_all(&dir).unwrap();
    }
}
