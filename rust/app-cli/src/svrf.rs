use floe_app_core::{svrf::parse::Options, Error, Result};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicUsize, Arc},
};

const HELP: &str = "Usage: floe2-web svrf DECK [OPTIONS]

Moved to floe-index (2026-09-29): the tools that build files from inputs live
there (`vfs` the layout cache, `drc` the result pack, `svrf` the rule sidecar),
one SVRF parser for every front end. This command prints the same command for
floe-index and exits 2, so an old script says where it went:
  floe-index svrf DECK [-o FILE] [--scan] [-D NAME[=VAL]]... [-I DIR]...
                       [--follow-verbatim] [--no-env-switches]
Same options, same <deck>.rules.json; floe-index svrf --help lists them.";

#[derive(Debug)]
pub enum Command {
    Help,
    /// the floe-index command the pointer prints: the options as given,
    /// in order (-D/-I values as written), checked as floe-index takes them
    Parse {
        words: Vec<String>,
    },
}
pub fn parse(args: &[String]) -> Result<Command> {
    let (mut options, mut deck, mut out): (Options, Option<PathBuf>, Option<PathBuf>) =
        (Options::default(), None, None);
    let (mut defines, mut includes) = (Vec::new(), Vec::new());
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
                defines.push(value.to_string());
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
                includes.push(value.to_string());
            }
            _ => {
                if value.is_empty() {
                    return Err(Error::input("--out must not be empty"));
                }
                out = Some(value.into());
            }
        }
    }
    let deck: PathBuf = deck.ok_or_else(|| Error::input("svrf requires DECK"))?;
    // floe/cli.py cmd_svrf's order
    let mut words = vec![
        "floe-index".to_string(),
        "svrf".into(),
        deck.to_string_lossy().into_owned(),
    ];
    if let Some(out) = &out {
        words.extend(["-o".into(), out.to_string_lossy().into_owned()]);
    }
    if options.scan_all {
        words.push("--scan".into());
    }
    for d in defines {
        words.extend(["-D".into(), d]);
    }
    for d in includes {
        words.extend(["-I".into(), d]);
    }
    if options.follow_verbatim {
        words.push("--follow-verbatim".into());
    }
    if !options.env_switches {
        words.push("--no-env-switches".into());
    }
    Ok(Command::Parse { words })
}
pub fn run(command: Command, _stop: &Arc<AtomicUsize>) -> Result<i32> {
    let Command::Parse { words } = command else {
        println!("{}", crate::named(HELP));
        return Ok(0);
    };
    eprintln!(
        "svrf moved to floe-index (same options, same <deck>.rules.json) - run:\n  {}",
        crate::shell_join(&words)
    );
    Ok(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| (*s).into()).collect()
    }
    #[test]
    fn cli_option_spellings_and_scan() {
        let Command::Parse { words } = parse(&args(&[
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
        assert_eq!(
            crate::shell_join(&words),
            "floe-index svrf -deck -o out --scan -D A=2 -D A=3 -I '한 글' --follow-verbatim --no-env-switches"
        );
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
