use floe_app_core::{
    svrf::parse::{self, Options},
    Error, Result,
};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicUsize, Arc},
};

const HELP: &str = "Usage: floe2-web svrf DECK [OPTIONS]

Convert the existing SVRF subset to per-check metadata, without Python.
  -o, --out FILE          Sidecar (default DECK.rules.json); atomic replacement
  --scan                 Inventory both IFDEF branches; no output file written
  -D, --define NAME[=VAL] Switch; repeatable, overrides tested environment names
  -I, --include-dir DIR   INCLUDE search directory; repeatable
  --follow-verbatim      Follow INCLUDEs in Tcl wrappers (does NOT run Tcl)
  --no-env-switches      Disable IFDEF environment fallback (not INCLUDE $VAR/~)
  -h, --help             Show help; -- ends options; -DNAME/-IDIR/-oFILE supported

DMACRO/CMACRO are NOT expanded; unknown statements and unclosed blocks warn.
This is rule metadata, NOT a Calibre replacement or a signoff verdict.
Local CLI only: reads relative/absolute INCLUDE paths and tested environment
names. No web endpoint, Tcl interpreter, shell command or native worker.
Limits: 256 MiB expanded input, 4096 file visits, 64 INCLUDE levels, 64 KiB
line/text, 64 MiB metadata/expansion work, 16M graph steps, 16 MiB sidecar.
Limits/nonfinite numbers fail, never publish truncated metadata. Missing root
fails; missing/unreadable INCLUDEs warn. Regular files only; UTF-8 errors replace.
Source/include/output aliases and cache paths are protected. SIGINT/SIGTERM or
write failure preserves the old sidecar; inputs are rechecked before commit.
JSON schema/values match floe; native generated_by and JSON whitespace differ.";

#[derive(Debug)]
pub enum Command {
    Help,
    Parse {
        deck: PathBuf,
        out: Option<PathBuf>,
        options: Options,
    },
}
pub fn parse(args: &[String]) -> Result<Command> {
    let (mut options, mut deck, mut out) = (Options::default(), None, None);
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
            if deck.replace(PathBuf::from(arg)).is_some() {
                return Err(Error::input("svrf accepts exactly one deck"));
            }
            continue;
        }
        let (flag, inline) =
            if arg.len() > 2 && ["-D", "-I", "-o"].iter().any(|p| arg.starts_with(p)) {
                (&arg[..2], Some(&arg[2..]))
            } else {
                arg.split_once('=')
                    .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)))
            };
        if matches!(
            flag,
            "--help" | "-h" | "--scan" | "--follow-verbatim" | "--no-env-switches"
        ) {
            if inline.is_some() {
                return Err(Error::input(format!("{flag} takes no value")));
            }
            match flag {
                "--help" | "-h" => return Ok(Command::Help),
                "--scan" => options.scan_all = true,
                "--follow-verbatim" => options.follow_verbatim = true,
                _ => options.env_switches = false,
            }
            continue;
        }
        if !matches!(
            flag,
            "-D" | "--define" | "-I" | "--include-dir" | "-o" | "--out"
        ) {
            return Err(Error::input(format!("unknown svrf option: {flag}")));
        }
        let value = if let Some(v) = inline {
            v
        } else {
            let v = args
                .get(i)
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| Error::input(format!("{flag} requires a value")))?;
            i += 1;
            v
        };
        match flag {
            "-D" | "--define" => {
                let (name, v) = value.split_once('=').unwrap_or((value, ""));
                if !name.is_empty() {
                    options
                        .defines
                        .insert(name.into(), (!v.is_empty()).then(|| v.into()));
                }
                if options.defines.len() > 65536 {
                    return Err(Error::input("too many defines"));
                }
            }
            "-I" | "--include-dir" => {
                if options.include_dirs.len() == 4096 {
                    return Err(Error::input("too many include directories"));
                }
                options.include_dirs.push(value.into());
            }
            _ => {
                if value.is_empty() {
                    return Err(Error::input("--out must not be empty"));
                }
                out = Some(value.into());
            }
        }
    }
    Ok(Command::Parse {
        deck: deck.ok_or_else(|| Error::input("svrf requires DECK"))?,
        out,
        options,
    })
}
pub fn run(command: Command, stop: &Arc<AtomicUsize>) -> Result<i32> {
    let Command::Parse { deck, out, options } = command else {
        println!("{HELP}");
        return Ok(0);
    };
    let parsed = parse::parse_deck(&deck, &options, stop)?;
    if options.scan_all {
        println!("{}", parsed.format_scan(stop)?);
        return Ok(0);
    }
    let out = out.unwrap_or_else(|| {
        let mut p = deck.as_os_str().to_owned();
        p.push(".rules.json");
        p.into()
    });
    parsed.write_json(&out, stop)?;
    println!(
        "{}: {} checks, {} derivations, {} layers -> {}",
        deck.display(),
        parsed.check_count(),
        parsed.derivation_count(),
        parsed.layer_count(),
        out.display()
    );
    if parsed.stat("cmacro") > 0 {
        eprintln!("[floe][warn] {} CMACRO calls NOT expanded - metadata is incomplete for macro-generated rules",parsed.stat("cmacro"));
    }
    if parsed.stat("unknown") > 0 {
        eprintln!(
            "[floe] {} unrecognized statements skipped (--scan lists them)",
            parsed.stat("unknown")
        );
    }
    for w in parsed.warnings().iter().take(10) {
        eprintln!("[floe][warn] {w}");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| (*s).into()).collect()
    }
    #[test]
    fn cli_option_spellings_and_scan() {
        let Command::Parse { deck, out, options } = parse(&args(&[
            "svrf",
            "-DA=2",
            "--define=A=3",
            "-I한 글",
            "-oout",
            "--scan",
            "--no-env-switches",
            "--follow-verbatim",
            "--",
            "-deck",
        ]))
        .unwrap() else {
            panic!()
        };
        assert_eq!(deck, PathBuf::from("-deck"));
        assert_eq!(out, Some("out".into()));
        assert_eq!(options.defines["A"], Some("3".into()));
        assert_eq!(options.include_dirs, vec![PathBuf::from("한 글")]);
        assert!(options.scan_all && options.follow_verbatim && !options.env_switches);
    }
    #[test]
    fn cli_bad_arguments() {
        for a in [
            &["svrf"][..],
            &["svrf", "a", "b"],
            &["svrf", "a", "--scan=1"],
            &["svrf", "a", "--out"],
            &["svrf", "a", "--out="],
            &["svrf", "a", "--unknown"],
        ] {
            assert!(parse(&args(a)).is_err(), "{a:?}");
        }
    }
}
