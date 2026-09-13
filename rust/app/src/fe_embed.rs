use floe_app_core::{
    annotations::{self, png, Annotation, Document},
    check_cancelled,
    shots::batch,
    Error, ErrorKind, Result,
};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    os::unix::fs::MetadataExt,
    path::PathBuf,
    sync::{atomic::AtomicUsize, Arc},
};

const HELP: &str = "Usage: floe2-web fe-embed [OPTIONS] PNG...

Python-free replacement for python -m floe.fe_embed; pixels remain untouched.
Coordinates: image CENTER-origin pixels, x right/y down. No layout/worker needed.
  --box/--ellipse X1,Y1,X2,Y2[,COLOR[,FILL[,WIDTH[,DASH]]]]
  --line X1,Y1,X2,Y2[,COLOR[,WIDTH[,DASH]]]
  --path X1,Y1,X2,Y2[,X3,Y3...][,COLOR[,WIDTH[,DASH]]]
  --polygon X1,Y1,X2,Y2,X3,Y3[,X4,Y4...][,COLOR[,FILL[,WIDTH[,DASH]]]]
  --ruler X1,Y1,X2,Y2
  --text X,Y[,size=N,color=COLOR,bg=COLOR,]TEXT
              Repeat annotations in draw order; text= escapes style-like text.
  --json FILE|-   JSON annotations/object, appended after option annotations
  --legend FILE   box/line COLOR [STYLE] LABEL; full-line # comments
  --ppu N --unit U  Positive pixels/unit and unit label (default um)
  --note TEXT     Whole-image note; literal \\n starts a new line
  --append        Keep old annotations; new ppu/note/legend replace old fields
  --dump          Print embedded metadata without writing (wins over --strip)
  --strip         Remove flateyes iTXt; no annotation inputs allowed
  --selftest      Native in-memory seven-kind/PNG round-trip (no Python import)
  -h, --help      Show this help; -- ends options

COLOR: black/white/red/orange/green/sky/pink or #RRGGBB. FILL also #RRGGBBAA;
polygon fill may add :PATTERN (floe names or pat:HEX64). Width 1..8, text 6..96.
PNG max 1 GiB/65536 chunks; annotation input/text max 16 MiB/100k annotations.
Nonfinite numbers, invalid PNG envelopes/CRC/iTXt and symlink/hardlink edits are rejected.
Pixel IDAT data are copied, not decompressed or validated as image samples.
Each PNG is staged and atomically replaced; multiple files are not one transaction.
Source identity/metadata are rechecked before rename; no cross-process editing lock.
SIGINT/SIGTERM abort before commit and preserve that PNG. Unix rwx mode retained.
No browser, HTTP endpoint, renderd, indexer, Pillow or Python runtime is used.";

#[derive(Debug, Default)]
pub struct Options {
    files: Vec<PathBuf>,
    doc: Document,
    json: Option<String>,
    legend: Option<String>,
    append: bool,
    dump: bool,
    strip: bool,
}
#[derive(Debug)]
pub enum Command {
    Help,
    Selftest,
    Edit(Box<Options>),
}
pub fn parse(args: &[String]) -> Result<Command> {
    let mut opts = Options::default();
    let mut positional = false;
    let mut selftest = false;
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if !positional && arg == "--" {
            positional = true;
            continue;
        }
        if positional || !arg.starts_with('-') {
            if opts.files.len() == 4096 {
                return Err(Error::input("at most 4096 PNG paths"));
            }
            opts.files.push(arg.into());
            continue;
        }
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
        if matches!(
            flag,
            "--help" | "-h" | "--selftest" | "--append" | "--dump" | "--strip"
        ) {
            if inline.is_some() {
                return Err(Error::input(format!("{flag} takes no value")));
            }
            match flag {
                "--help" | "-h" => return Ok(Command::Help),
                "--selftest" => selftest = true,
                "--append" => opts.append = true,
                "--dump" => opts.dump = true,
                _ => opts.strip = true,
            }
            continue;
        }
        if !matches!(
            flag,
            "--box"
                | "--ellipse"
                | "--line"
                | "--path"
                | "--polygon"
                | "--ruler"
                | "--text"
                | "--json"
                | "--legend"
                | "--note"
                | "--ppu"
                | "--unit"
        ) {
            return Err(Error::input(format!("unknown fe-embed option: {flag}")));
        }
        let value = if let Some(v) = inline {
            v
        } else {
            let v = args
                .get(i)
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| Error::input(format!("{flag} needs a value")))?;
            i += 1;
            v
        };
        match flag {
            "--json" => opts.json = (!value.is_empty()).then(|| value.into()),
            "--legend" => opts.legend = (!value.is_empty()).then(|| value.into()),
            "--note" => opts.doc.note = (!value.is_empty()).then(|| annotations::unescape(value)),
            "--ppu" => opts.doc.ppu = Some(annotations::number(value)?),
            "--unit" => opts.doc.unit = (!value.is_empty()).then(|| value.into()),
            _ => opts
                .doc
                .annotations
                .push(Annotation::option(&flag[2..], value)?),
        }
    }
    if selftest {
        return Ok(Command::Selftest);
    }
    if opts.files.is_empty() {
        return Err(Error::input("no PNG files given"));
    }
    opts.doc.validate()?;
    if !opts.dump
        && opts.strip
        && (!opts.doc.empty() || opts.json.is_some() || opts.legend.is_some())
    {
        return Err(Error::input("--strip takes no annotations"));
    }
    Ok(Command::Edit(Box::new(opts)))
}
fn runtime(error: Error) -> Error {
    if error.kind == ErrorKind::Cancelled {
        error
    } else {
        Error::new(ErrorKind::Io, error.message)
    }
}
pub fn run(command: Command, flag: &Arc<AtomicUsize>) -> Result<i32> {
    let Command::Edit(opts) = command else {
        if matches!(command, Command::Help) {
            println!("{HELP}");
        } else {
            png::selftest(flag)?;
            println!(
                "FLATEYES METADATA: native selftest OK (seven kinds, UTF-8, PNG round-trip/strip)"
            );
        }
        return Ok(0);
    };
    let mut opts = *opts;
    // Pure inspection/strip do not load unrelated JSON/legend inputs (legacy
    // precedence). Validate all destination identities before the first write.
    let mut paths = BTreeSet::new();
    for path in &opts.files {
        check_cancelled(flag)?;
        let m = if opts.dump {
            fs::metadata(path)
        } else {
            fs::symlink_metadata(path)
        }
        .map_err(Error::from)?;
        if !m.is_file() {
            return Err(Error::new(
                ErrorKind::Io,
                "PNG must be a regular file, not a symlink/directory/FIFO",
            ));
        }
        if !opts.dump && !paths.insert((m.dev(), m.ino())) {
            return Err(Error::input("duplicate/aliased PNG paths"));
        }
    }
    if !opts.dump && !opts.strip {
        if let Some(path) = &opts.json {
            let text = if path == "-" {
                crate::capture::stdin_text(flag)
            } else {
                batch::read_file(std::path::Path::new(path), flag)
            }
            .map_err(runtime)?;
            let value = serde_json::from_str(&text)
                .map_err(|e| Error::input(format!("invalid annotations JSON: {e}")))
                .map_err(runtime)?;
            let mut doc = Document::from_json(value, flag).map_err(runtime)?;
            opts.doc.annotations.append(&mut doc.annotations);
            opts.doc.ppu = opts.doc.ppu.or(doc.ppu);
            opts.doc.unit = opts.doc.unit.or(doc.unit);
            opts.doc.note = opts.doc.note.or(doc.note);
            opts.doc.legend = doc.legend;
        }
        if let Some(path) = &opts.legend {
            let text = batch::read_file(std::path::Path::new(path), flag).map_err(runtime)?;
            let lines = text
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.starts_with('#'));
            opts.doc.legend = Some(
                lines
                    .map(annotations::legend_line)
                    .collect::<Result<_>>()
                    .map_err(runtime)?,
            );
        }
        opts.doc.validate().map_err(runtime)?;
        if opts.doc.empty() && !opts.append {
            return Err(Error::input("no annotations given (use --strip to remove)"));
        }
        // Reject text overflow before touching the first PNG, not after a prefix.
        opts.doc.serialize(None, false, flag)?;
    }
    let mut saved = 0;
    let empty = Document::default();
    for path in &opts.files {
        check_cancelled(flag)?;
        let result = if opts.dump {
            png::read(path, flag).and_then(|text| {
                let mut stdout = std::io::stdout().lock();
                if opts.files.len() > 1 {
                    writeln!(stdout, "==> {} <==", path.display())?;
                }
                stdout.write_all(
                    text.as_deref()
                        .unwrap_or("(no embedded metadata)\n")
                        .as_bytes(),
                )?;
                Ok(())
            })
        } else {
            png::edit(
                path,
                if opts.strip { &empty } else { &opts.doc },
                opts.append && !opts.strip,
                flag,
            )
        };
        result.map_err(|e| {
            let e = runtime(e);
            Error::new(
                e.kind,
                format!("{}: {e}; {saved} PNG(s) already updated", path.display()),
            )
        })?;
        if !opts.dump {
            saved += 1;
            if opts.strip {
                println!("{}: metadata removed", path.display());
            } else {
                let n = opts.doc.annotations.len();
                println!(
                    "{}: {n} annotation{} embedded{}",
                    path.display(),
                    if n == 1 { "" } else { "s" },
                    if opts.append { " (appended)" } else { "" }
                );
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(args: &[&str]) -> Result<Command> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn cli_inputs_and_precedence() {
        assert!(matches!(
            p(&["fe-embed", "--selftest"]).unwrap(),
            Command::Selftest
        ));
        assert!(p(&["fe-embed", "--strip", "--box=0,0,1,1", "a.png"]).is_err());
        assert!(p(&["fe-embed", "--ppu=NaN", "a.png"]).is_err());
        let Command::Edit(o) = p(&[
            "fe-embed",
            "--text=0,0,text=size=20,hello",
            "--box=0,0,1,1",
            "--json=x.json",
            "--dump",
            "--strip",
            "--",
            "-한 글.png",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(o.files, [PathBuf::from("-한 글.png")]);
        assert!(o.dump && o.strip);
        assert_eq!(o.doc.annotations.len(), 2);
    }
}
