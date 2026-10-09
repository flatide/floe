//! `floe2 gtk-service`: the GTK viewer's non-UI answers (docs/
//! SHARED_APP_LAYER.ko.md P2). One JSON object per line on stdin
//! (`{"id": N, "op": "...", ...}`), one per line on stdout (`{"id": N,
//! "result": ...}` or `{"id": N, "error": {"kind": "...", "message":
//! "..."}}`), in order, until stdin ends. The decisions are the shared
//! app-core's (`desktop`, `dataset`, `jobdeck`); this is their wire.
//!
//! Requests:
//!   version                              the app, renderd and floe-index versions
//!   ready {source, levels?}              whether it opens as it is (desktop::ready)
//!   open {source, levels?, mode?}        a layout's or a deck's meta, cache folder or
//!                                        composite spec, layer properties
//!   close {handle}                       a deck's spec folder goes
//!   level_rows {source}                  the deck's load dialog rows
use floe_app_core::{
    dataset::Dataset,
    desktop,
    jobdeck::{color::Mode, dataset::props_source, parser::JobDeck, view::level_rows},
    Error, ErrorKind, Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

/// The composite specs this service wrote, by handle: removed on `close`
/// and when the service ends.
#[derive(Default)]
struct Specs {
    next: u64,
    folders: BTreeMap<u64, PathBuf>,
}
impl Drop for Specs {
    fn drop(&mut self) {
        for folder in self.folders.values() {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

fn kind_name(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::InvalidInput => "input",
        ErrorKind::Unsupported => "unsupported",
        ErrorKind::Io => "io",
        ErrorKind::PublicationUnknown => "publication",
        ErrorKind::Cache => "cache",
        ErrorKind::Busy => "busy",
        ErrorKind::Admission => "admission",
        ErrorKind::Version => "version",
        ErrorKind::Worker => "worker",
        ErrorKind::Cancelled => "cancelled",
        ErrorKind::Incomplete => "incomplete",
    }
}

fn text<'a>(request: &'a Value, key: &str) -> Result<&'a str> {
    request
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::input(format!("{key} must be a string")))
}

fn levels(request: &Value) -> Result<Option<BTreeSet<i64>>> {
    match request.get("levels") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let set = items
                .iter()
                .map(|v| {
                    v.as_i64()
                        .ok_or_else(|| Error::input("levels must be integers"))
                })
                .collect::<Result<BTreeSet<_>>>()?;
            Ok((!set.is_empty()).then_some(set))
        }
        Some(_) => Err(Error::input("levels must be a list")),
    }
}

fn props(source: &Path) -> Result<Value> {
    Ok(serde_json::to_value(desktop::props_rows(source)?).expect("layerprops rows serialize"))
}

fn open(request: &Value, specs: &mut Specs, cancelled: &AtomicUsize) -> Result<Value> {
    let source = floe_app_core::cache::absolute(Path::new(text(request, "source")?))?;
    let mode = match request.get("mode").and_then(Value::as_str) {
        Some(m) => Mode::parse(m)?,
        None => Mode::Level,
    };
    match Dataset::open(&source, levels(request)?, mode, cancelled)? {
        Dataset::Layout(layout) => {
            let mut meta = desktop::layout_meta(&layout)?;
            let left = desktop::list_layers(&mut meta);
            Ok(json!({
                "kind": "layout",
                "src": source,
                "dir": layout.directory,
                "meta": meta,
                "unlisted": desktop::unlisted_note(&left),
                "props_src": source,
                "props": props(&source)?,
                "stale": layout.source_stale,
            }))
        }
        Dataset::Deck(deck) => {
            // the spec renderd opens (`open deck=`), in a folder of its own
            specs.next += 1;
            let handle = specs.next;
            let folder =
                std::env::temp_dir().join(format!("floe-jobdeck-{}-{handle}", std::process::id()));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::DirBuilder::new().mode(0o700).create(&folder)?;
            specs.folders.insert(handle, folder.clone());
            let mode_name = serde_json::to_value(mode).expect("mode serializes");
            let spec = folder.join(format!(
                "deck-{}.spec",
                mode_name.as_str().unwrap_or("level")
            ));
            std::fs::write(&spec, deck.spec.text.as_bytes())?;
            let props_src = props_source(&source, mode)?;
            let dirs: BTreeMap<&str, &Path> = deck
                .analysis
                .catalog
                .infos
                .iter()
                .map(|(tc, info)| (tc.as_str(), info.cache_dir.as_path()))
                .collect();
            Ok(json!({
                "kind": "deck",
                "handle": handle,
                "src": source,
                "dir": spec,
                "meta": deck.metadata,
                "props_src": props_src,
                "props": props(&props_src)?,
                "source_dirs": dirs,
                "stale": false,
            }))
        }
    }
}

fn handle(request: &Value, specs: &mut Specs, cancelled: &AtomicUsize) -> Result<Value> {
    match text(request, "op")? {
        "version" => Ok(json!({
            "version": env!("FLOE2_VERSION"),
            "renderd": floe_worker_client::EXPECTED_RENDERD_VERSION,
            "index": floe_app_core::native::INDEX_VERSION,
        })),
        "ready" => {
            let source = PathBuf::from(text(request, "source")?);
            Ok(serde_json::to_value(desktop::ready(
                &source,
                levels(request)?.as_ref(),
                cancelled,
            )?)
            .expect("readiness serializes"))
        }
        "open" => open(request, specs, cancelled),
        "close" => {
            let handle = request
                .get("handle")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::input("handle must be a number"))?;
            if let Some(folder) = specs.folders.remove(&handle) {
                let _ = std::fs::remove_dir_all(folder);
            }
            Ok(Value::Null)
        }
        "level_rows" => {
            let deck = JobDeck::read(Path::new(text(request, "source")?), true, cancelled)?;
            Ok(serde_json::to_value(level_rows(&deck, cancelled)?).expect("level rows serialize"))
        }
        other => Err(Error::new(
            ErrorKind::Unsupported,
            format!("unknown request: {other}"),
        )),
    }
}

/// Serve stdin until it ends; the exit status.
pub fn run() -> i32 {
    floe_app_core::set_program("floe2");
    let cancelled = AtomicUsize::new(0);
    let mut specs = Specs::default();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(request) => {
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                match handle(&request, &mut specs, &cancelled) {
                    Ok(result) => json!({"id": id, "result": result}),
                    Err(e) => {
                        json!({"id": id, "error": {"kind": kind_name(e.kind), "message": e.message}})
                    }
                }
            }
            Err(e) => {
                json!({"id": null, "error": {"kind": "input", "message": format!("not a JSON request: {e}")}})
            }
        };
        if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
    0
}
