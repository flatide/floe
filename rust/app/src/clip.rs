use floe_app_core::{
    catalog::Layout,
    clip::{export, ClipOptions},
    jobdeck::index::is_deck,
    Error, ErrorKind, Result,
};
use std::path::PathBuf;
use std::sync::{atomic::AtomicUsize, Arc};
use std::time::Instant;

const HELP: &str = "Usage: floe2-web clip SOURCE --bbox X0,Y0,X1,Y1 [OPTIONS]

  --bbox X0,Y0,X1,Y1   Microns; nearest DBU ties-even, reversed corners accepted
  --layers SPEC        Comma-separated layer/datatype or named layers (default all)
  --out PATH           OASIS output (default clip.oas); publish only on success
  --cell-name NAME     Output top cell (default FLOE_CLIP)
  --exact              Compatibility flag: Rust clip is always exact
  -h, --help           Show this help

Uses the existing cache, full depth, cut=0; display/LOD/summary settings ignored.
An empty layer-token list means all, matching the existing clip CLI (not render).
Jobdeck clip is unsupported; clip a source OASIS layout directly.
FLOE_RUST_JOBS controls decode workers (default min(CPUs,8)).
FLOE_RUST_CLIP_TIMEOUT_S and FLOE_RUST_OPEN_TIMEOUT_S default to 300 (1..86400).
Source/cache aliases cannot be overwritten. Signals/errors preserve old output.
Use -- for a source filename beginning with a dash. No Python fallback.";

#[derive(Debug)]
pub enum Command {
    Help,
    Export {
        source: PathBuf,
        bbox: [f64; 4],
        layers: Option<String>,
        out: PathBuf,
        cell: String,
    },
}
pub fn parse(args: &[String]) -> Result<Command> {
    let mut source = None;
    let mut bbox = None;
    let mut layers = None;
    let mut out = PathBuf::from("clip.oas");
    let mut cell = "FLOE_CLIP".to_string();
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
                return Err(Error::input("clip accepts exactly one source"));
            }
            continue;
        }
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
        if matches!(flag, "--help" | "-h" | "--exact") {
            if inline.is_some() {
                return Err(Error::input(format!("{flag} takes no value")));
            }
            if flag != "--exact" {
                return Ok(Command::Help);
            }
            continue;
        }
        if !matches!(flag, "--bbox" | "--layers" | "--out" | "--cell-name") {
            return Err(Error::input(format!("unsupported clip option: {flag}")));
        }
        let value = if let Some(v) = inline {
            v
        } else {
            let v = args
                .get(i)
                .filter(|s| !s.starts_with("--"))
                .ok_or_else(|| Error::input(format!("{flag} requires a value")))?;
            i += 1;
            v
        };
        match flag {
            "--bbox" => {
                let values = value
                    .split(',')
                    .map(|s| {
                        s.trim()
                            .parse::<f64>()
                            .ok()
                            .filter(|n| n.is_finite())
                            .ok_or_else(|| {
                                Error::input("--bbox requires four finite micron coordinates")
                            })
                    })
                    .collect::<Result<Vec<_>>>()?;
                bbox = Some(
                    values
                        .try_into()
                        .map_err(|_| Error::input("--bbox requires X0,Y0,X1,Y1"))?,
                );
            }
            "--layers" => layers = Some(value.into()),
            "--out" => out = value.into(),
            "--cell-name" => cell = value.into(),
            _ => unreachable!(),
        }
    }
    let source = source.ok_or_else(|| Error::input("clip requires SOURCE"))?;
    if is_deck(&source) {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "jobdeck clip is unsupported; clip a source layout directly",
        ));
    }
    let bbox = bbox.ok_or_else(|| Error::input("clip requires --bbox X0,Y0,X1,Y1"))?;
    Ok(Command::Export {
        source,
        bbox,
        layers,
        out,
        cell,
    })
}
pub fn run(command: Command, cancelled: &Arc<AtomicUsize>) -> Result<i32> {
    let Command::Export {
        source,
        bbox,
        layers,
        out,
        cell,
    } = command
    else {
        println!("{HELP}");
        return Ok(0);
    };
    let start = Instant::now();
    let layout = Layout::open(&source, cancelled)?;
    if layout.source_stale {
        eprintln!("[floe2-web][warn] source changed; clipping cached geometry. Rebuild with index --force first to export the current source.");
    }
    let report = export(
        &layout,
        bbox,
        layers.as_deref(),
        &cell,
        &out,
        &ClipOptions::local()?,
        cancelled,
    )?;
    println!(
        "[floe2-web] clip saved: {} ({:.2} MB) in {:.3}s ({} records)",
        report.output.display(),
        report.size_bytes as f64 / 1e6,
        start.elapsed().as_secs_f64(),
        report.fields.u64("records")?
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn clip_options_are_independent_of_display() {
        let c = parse(&args(&[
            "clip",
            "--bbox=-1,0,1,2",
            "--exact",
            "--cell-name",
            "한 글",
            "--",
            "-source.oas",
        ]))
        .unwrap();
        assert!(matches!(c, Command::Export { source, bbox, cell, .. }
            if source == std::path::Path::new("-source.oas") && bbox == [-1.,0.,1.,2.] && cell == "한 글"));
        for bad in [
            vec!["clip", "x.oas"],
            vec!["clip", "x.jb", "--bbox=0,0,1,1"],
            vec!["clip", "x.oas", "--bbox=NaN,0,1,1"],
            vec!["clip", "x.oas", "--bbox=0,1,2"],
            vec!["clip", "x.oas", "--bbox=0,0,1,1", "--detail=high"],
            vec!["clip", "x.oas", "--bbox=0,0,1,1", "--thin=keep"],
            vec!["clip", "x.oas", "--bbox=0,0,1,1", "--depth=1"],
            vec!["clip", "x.oas", "--bbox=0,0,1,1", "--exact=false"],
        ] {
            assert!(parse(&args(&bad)).is_err(), "{bad:?}");
        }
    }
}
