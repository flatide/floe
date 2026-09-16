use floe_app_core::{
    artifact, check_cancelled,
    jobdeck::{
        color::{ColorScheme, Mode},
        index::parse_levels,
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
            _ => return Err(Error::input(format!("unknown jobdeck option: {flag}"))),
        }
    }
    Ok(Command::Run(Box::new(Args {
        source: source.ok_or_else(|| Error::input("jobdeck requires a deck"))?,
        options,
        colors,
        placements,
        report,
        spec,
    })))
}
pub fn run(command: Command, cancelled: &AtomicUsize) -> Result<i32> {
    let Command::Run(mut a) = command else {
        println!("{HELP}");
        return Ok(0);
    };
    if let Some(path) = &a.colors {
        a.options.scheme = Some(ColorScheme::load(path)?);
    }
    let analysis = Analysis::open(&a.source, &a.options, cancelled)?;
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
                "[jobdeck] {n} source(s) have no VFS cache yet; run: floe2-web index {}",
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
}
