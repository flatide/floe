use floe_app_core::{
    artifact, cache, check_cancelled,
    jobdeck::{
        chips::tokens,
        chipset,
        color::{ColorScheme, Mode},
        index::{parse_levels, validate_levels},
        parser::JobDeck,
        plan::{Analysis, AnalysisOptions},
        spec,
    },
    Error, Result,
};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;

const HELP: &str = "Usage: floe2-web jobdeck DECK.jb [OPTIONS]
  --sources DIR              TC base directory (default deck directory)
  --level N,N,... / --id ...  Analysis placement selection; whole-deck grid
  --mode level|chip|layer     Color policy (identifier aliases level)
  --colors FILE              Saved palette/pins JSON; overrides --ly-dt
  --ly-dt cross|zip           Multi-valued LY/DT combination (default cross)
  --on-missing skip|fail      Skip with ledger/exit 3, or fail/exit 2
  --lenient                  Preserve structural errors; exclude bad entries
  --placements               Print all placements and their analysis colors
  --report FILE              JSON report; atomic individual artifact
  --spec FILE                Composite spec from indexed sources; skip ledger
  --chipset NAME[,NAME...]   The chip set of a source: every CHIP block that
                             places it, with the CHIP line's text, the levels,
                             placements and the line in the deck. NAME is the
                             source's file name, its TC path or a path to the
                             file; wildcards * ? [..]; '*' lists every source.
                             Reads the deck's text alone (no source needed);
                             takes --level, --sources and --lenient only
No indexing or rendering is performed. Inputs/caches cannot be output targets.
Report/spec are individually atomic, not a two-file transaction.
This command's CHIP-block colors differ from the viewer's source-chip tree.";
pub struct Args {
    source: PathBuf,
    options: AnalysisOptions,
    colors: Option<PathBuf>,
    placements: bool,
    report: Option<PathBuf>,
    spec: Option<PathBuf>,
    /// --chipset: the sources whose CHIP blocks to show
    chipset: Option<Vec<String>>,
}
pub enum Command {
    Help,
    Run(Box<Args>),
}
pub fn parse(args: &[String]) -> Result<Command> {
    let mut options = AnalysisOptions {
        skip_missing: true,
        ..Default::default()
    };
    let mut source = None;
    let mut colors = None;
    let mut report = None;
    let mut spec = None;
    let mut placements = false;
    let mut chipset = None;
    // the analysis's own options, which a --chipset lookup does not take
    let mut analysis_flags: Vec<String> = Vec::new();
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
                return Err(Error::input("jobdeck accepts one deck"));
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
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| Error::input(format!("{flag} requires a value")))?;
            i += 1;
            Ok(v)
        };
        let boolean = || {
            if inline.is_some() {
                Err(Error::input(format!("{flag} takes no value")))
            } else {
                Ok(())
            }
        };
        if matches!(
            flag,
            "--mode" | "--colors" | "--ly-dt" | "--on-missing" | "--placements" | "--report" | "--spec"
        ) && !analysis_flags.iter().any(|f| f == flag)
        {
            analysis_flags.push(flag.into());
        }
        match flag {
            "-h" | "--help" => {
                boolean()?;
                return Ok(Command::Help);
            }
            "--sources" => options.sources = Some(value()?.into()),
            "--level" | "--id" => options.ids = Some(parse_levels(value()?)?),
            "--mode" => options.mode = Mode::parse(value()?)?,
            "--colors" => colors = Some(PathBuf::from(value()?)),
            "--ly-dt" => {
                options.cross = match value()? {
                    "cross" => true,
                    "zip" => false,
                    _ => return Err(Error::input("--ly-dt must be cross or zip")),
                }
            }
            "--on-missing" => {
                options.skip_missing = match value()? {
                    "skip" => true,
                    "fail" => false,
                    _ => return Err(Error::input("--on-missing must be skip or fail")),
                }
            }
            "--lenient" => {
                boolean()?;
                options.strict = false;
            }
            "--placements" => {
                boolean()?;
                placements = true;
            }
            "--report" => report = Some(PathBuf::from(value()?)),
            "--spec" => spec = Some(PathBuf::from(value()?)),
            "--chipset" => {
                let names: Vec<String> = tokens(value()?).into_iter().map(String::from).collect();
                if names.is_empty() {
                    return Err(Error::input(
                        "--chipset takes NAME[,NAME...]: a source's file name, TC path or a path to it",
                    ));
                }
                chipset = Some(names);
            }
            _ => return Err(Error::input(format!("unknown jobdeck option: {flag}"))),
        }
    }
    if chipset.is_some() && !analysis_flags.is_empty() {
        return Err(Error::input(format!(
            "--chipset looks the sources up in the deck's text; {} {} the analysis's \
             (--chipset takes --level, --sources and --lenient)",
            analysis_flags.join(", "),
            if analysis_flags.len() == 1 { "is" } else { "are" }
        )));
    }
    Ok(Command::Run(Box::new(Args {
        source: source.ok_or_else(|| Error::input("jobdeck requires a deck"))?,
        options,
        colors,
        placements,
        report,
        spec,
        chipset,
    })))
}

/// `--chipset`: every CHIP block that places each source named, from the
/// deck's text alone (floe_app_core::jobdeck::chipset) - no source is
/// opened, so one that is missing or not indexed is looked up as well.
fn chipset_lookup(a: &Args, names: &[String], cancelled: &AtomicUsize) -> Result<i32> {
    let deck = JobDeck::read(&a.source, a.options.strict, cancelled)?;
    validate_levels(&deck, a.options.ids.as_ref())?;
    // the TC paths' base, as the analysis resolves them (SourceCatalog)
    let base = match &a.options.sources {
        Some(dir) => cache::absolute(dir)?,
        None => cache::absolute(&a.source)?
            .parent()
            .expect("absolute path")
            .to_path_buf(),
    };
    let cwd = std::env::current_dir()?;
    let found = chipset::lookup(&deck, names, &base, &cwd, a.options.ids.as_ref())?;
    for line in chipset::lines(&deck, &found) {
        check_cancelled(cancelled)?;
        println!("{line}");
    }
    Ok(0)
}

pub fn run(command: Command, cancelled: &AtomicUsize) -> Result<i32> {
    let Command::Run(mut a) = command else {
        println!("{}", crate::named(HELP));
        return Ok(0);
    };
    if let Some(names) = &a.chipset {
        return chipset_lookup(&a, names, cancelled);
    }
    if let Some(path) = &a.colors {
        a.options.scheme = Some(ColorScheme::load(path)?);
    }
    let analysis = match Analysis::open(&a.source, &a.options, cancelled) {
        // --on-missing fail: a selected entry names a source without a dbu
        // - said, and 2 (floe/cli.py cmd_jobdeck), not an input error
        Err(e) if !a.options.skip_missing && e.message.starts_with("no source dbu") => {
            eprintln!(
                "{}: {} (a selected entry names a source without a dbu; --on-missing skip lists it instead)  [exit 2]",
                floe_app_core::program(),
                e.message
            );
            return Ok(2);
        }
        result => result?,
    };
    let inputs: Vec<_> = a.colors.into_iter().collect();
    let report = a
        .report
        .as_ref()
        .map(|p| analysis.output_path(p, &inputs))
        .transpose()?;
    let spec_path = a
        .spec
        .as_ref()
        .map(|p| analysis.output_path(p, &inputs))
        .transpose()?;
    if report.is_some() && report == spec_path {
        return Err(Error::input("report and spec must be different files"));
    }
    // Validate every requested artifact before publishing either. Still not
    // a multi-file transaction: an I/O failure after the first commit is shown.
    let spec = spec_path
        .as_ref()
        .map(|_| spec::compose(&analysis, cancelled))
        .transpose()?;
    if let Some(s) = &spec {
        s.require_drawable()?;
    }
    let report_bytes = report
        .as_ref()
        .map(|_| {
            analysis
                .report()
                .and_then(|v| artifact::json_bytes(&v, cancelled))
        })
        .transpose()?;
    for line in analysis.summary()? {
        check_cancelled(cancelled)?;
        println!("[jobdeck] {line}");
    }
    if a.placements {
        println!(
            "{:<8} {:<4} {:<4} {:<24} {:<8} {:<9} {:>16} {:>16} {:>8}",
            "chip", "idx", "row", "tc", "ly/dt", "mag", "dx_um", "dy_um", "colour"
        );
        for p in &analysis.model.placements {
            check_cancelled(cancelled)?;
            println!("{}", analysis.placement_line(p));
        }
    }
    if let (Some(path), Some(bytes)) = (&report, &report_bytes) {
        artifact::publish(path, bytes, cancelled)?;
        println!("[jobdeck] report    : {}", path.display());
    }
    let mut code = if analysis.model.stats.skipped.is_empty() {
        0
    } else {
        3
    };
    if code == 3 {
        println!(
            "[jobdeck] {} selected entries could not be placed [exit 3]",
            analysis.model.stats.skipped.len()
        );
    }
    if let (Some(path), Some(s)) = (&spec_path, spec) {
        if let Err(e) = artifact::publish(path, s.text.as_bytes(), cancelled) {
            return Err(Error::new(
                e.kind,
                format!(
                    "{}{}",
                    if report.is_some() {
                        "report was published; spec failed: "
                    } else {
                        "spec failed: "
                    },
                    e
                ),
            ));
        }
        for r in &s.skipped {
            println!(
                "[jobdeck] skipped   : CHIP {} ${} {}: {} ({})",
                r.chip, r.idx, r.tc, r.reason, r.detail
            );
        }
        println!(
            "[jobdeck] spec      : {} ({} placement(s), {} skipped)",
            path.display(),
            s.placements,
            s.skipped.len()
        );
        if !s.skipped.is_empty() {
            code = 3;
        }
    } else {
        let n = analysis
            .catalog
            .infos
            .values()
            .filter(|i| i.ok() && !i.indexed)
            .count();
        if n > 0 {
            println!(
                "[jobdeck] {n} source(s) have no index yet; run: {} index {}",
                floe_app_core::program(),
                a.source.display()
            );
        }
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_and_invalid_options() {
        let parse = |s: &[&str]| super::parse(&s.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(matches!(
            parse(&["jobdeck", "--help"]).unwrap(),
            Command::Help
        ));
        let Command::Run(a) = parse(&[
            "jobdeck",
            "x.jb",
            "--id=3,1,3",
            "--mode=identifier",
            "--placements",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            a.options.ids.unwrap().into_iter().collect::<Vec<_>>(),
            [1, 3]
        );
        assert!(a.placements && a.options.skip_missing);
        assert!(a.chipset.is_none());
        for s in [
            &["jobdeck"][..],
            &["jobdeck", "x", "y"],
            &["jobdeck", "x", "--lenient=yes"],
            &["jobdeck", "x", "--level="],
            &["jobdeck", "x", "--ly-dt=guess"],
            &["jobdeck", "x", "--report"],
        ] {
            assert!(parse(s).is_err());
        }
    }

    #[test]
    fn chipset_names_sources_and_takes_no_analysis_option() {
        let parse = |s: &[&str]| super::parse(&s.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let Command::Run(a) = parse(&[
            "jobdeck",
            "x.jb",
            "--chipset",
            " a.oas, ,b*.oas ",
            "--level=2",
            "--sources",
            "src",
            "--lenient",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(a.chipset.unwrap(), ["a.oas", "b*.oas"]);
        assert!(!a.options.strict);
        let Command::Run(a) = parse(&["jobdeck", "--chipset=*", "x.jb"]).unwrap() else {
            panic!()
        };
        assert_eq!(a.chipset.unwrap(), ["*"]);
        for s in [
            &["jobdeck", "x", "--chipset"][..],
            &["jobdeck", "x", "--chipset", ","],
            &["jobdeck", "x", "--chipset="],
            &["jobdeck", "--chipset", "a"],
        ] {
            assert!(parse(s).is_err(), "{s:?}");
        }
        for flag in [
            &["--report", "r.json"][..],
            &["--spec", "s.spec"],
            &["--placements"],
            &["--mode", "chip"],
            &["--colors", "c.json"],
            &["--ly-dt", "zip"],
            &["--on-missing", "fail"],
        ] {
            let mut s = vec!["jobdeck", "x.jb", "--chipset", "a.oas"];
            s.extend_from_slice(flag);
            let Err(e) = parse(&s) else {
                panic!("{s:?} parsed")
            };
            assert!(e.message.contains(flag[0]), "{}", e.message);
        }
    }
}
