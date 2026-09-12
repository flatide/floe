//! Existing scripting output remains streamed; no browser/GTK dependencies.
use floe_app_core::{check_cancelled, drc::open_current, Error, Result};
use std::{
    io::{self, Write},
    path::PathBuf,
    sync::atomic::AtomicUsize,
};
const HELP: &str = "Usage: floe2-web drc RESULTS.db|RESULTS.ice [OPTIONS]
  --list                  Also list every error's centre and size
  --rules                 Rule JSON [{name,errors,waived},...]
  --errs RULE             Stream one rule's error JSON (first duplicate)
  --floe-reviewer TAG     Existing per-reviewer waive sidecar selection

Read-only: no automatic indexing, autosave creation, or in-pack writes.
This stage requires an existing layout-4 .ice pack (fresh beside a .db).
ASCII fallback and DRC editing/notes are not ported yet.";
pub struct Command {
    source: Option<PathBuf>,
    list: bool,
    rules: bool,
    errs: Option<String>,
    reviewer: Option<String>,
}
pub fn parse(args: &[String]) -> Result<Command> {
    let mut c = Command {
        source: None,
        list: false,
        rules: false,
        errs: None,
        reviewer: None,
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
            "--list" | "--rules" if inline.is_none() => {
                if flag == "--list" {
                    c.list = true
                } else {
                    c.rules = true
                }
            }
            "--errs" | "--floe-reviewer" => {
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
                if flag == "--errs" {
                    c.errs = Some(value.into())
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
        println!("{HELP}");
        return Ok(0);
    };
    let mut p = open_current(&source, command.reviewer.as_deref(), cancelled)?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    if command.rules {
        writeln!(out, "[")?;
        for (i, c) in p.checks.iter().enumerate() {
            check_cancelled(cancelled)?;
            if i > 0 {
                writeln!(out, ",")?;
            }
            json(
                &mut out,
                &serde_json::json!({"name":c.name,"errors":c.count,"waived":p.waived_count(i)?}),
            )?;
        }
        writeln!(out, "\n]")?;
    } else if let Some(rule) = command.errs {
        let hits: Vec<_> = p
            .checks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.name == rule)
            .map(|(i, _)| i)
            .collect();
        let ci = *hits
            .first()
            .ok_or_else(|| Error::input(format!("no such rule {rule:?} (see --rules)")))?;
        if hits.len() > 1 {
            eprintln!(
                "[floe2-web][warn] rule {rule:?} appears {} times - using the first",
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
                json(
                    &mut out,
                    &serde_json::json!({"local":hit.local+1,"global":e.number,"kind":e.kind.to_string(),"status":hit.status,"bbox":p.bbox_um(e.bbox)?.map(rounded)}),
                )?;
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
            p.path.display(),
            p.cell,
            p.precision
        )?;
        writeln!(out, "{} checks, {} errors", p.checks.len(), p.total)?;
        for ci in 0..p.checks.len() {
            check_cancelled(cancelled)?;
            let c = &p.checks[ci];
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
                        let b = p.bbox_um(e.bbox)?;
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
    }
}
