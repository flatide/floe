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
//!
//! DRC review (P2b; app-core drc::desktop - floe/drc.py IcePack's files and
//! autosave), by the handle `drc_open` gives:
//!   drc_busy {db}                        who re-packs its pack, or null
//!   drc_find {db}                        its pack, or null
//!   drc_open {path, source?, reviewer?, mode: pack|load}
//!                                        checks, counts, notes, notices
//!   drc_errors {handle, check, start, count}
//!   drc_status {handle, check, start, count}   status bytes, hex
//!   drc_set_status {handle, check, errors, status}
//!   drc_status_page {handle, check, waived, start, limit}
//!   drc_status_rank {handle, check, waived, error}
//!   drc_query {handle, bbox, cap, checks?, waived?}
//!   drc_set_note / drc_clear_note {handle, gids[, text]}
//!   drc_note_export / drc_note_import / drc_waive_export / drc_waive_import {handle, path}
//!   drc_cd {handle, check, error}       CD ruler segments, um
//!   drc_close {handle}
//!   svrf_rules {path}, svrf_operands {rhs}
use floe_app_core::{
    dataset::Dataset,
    desktop,
    drc::desktop::{query_rows, ErrorRow, Opened, Review},
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
/// and when the service ends; and the DRC databases it holds open.
#[derive(Default)]
struct Specs {
    next: u64,
    folders: BTreeMap<u64, PathBuf>,
    drcs: BTreeMap<u64, Opened>,
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

fn number(request: &Value, key: &str) -> Result<u64> {
    request
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::input(format!("{key} must be a number")))
}

fn numbers(request: &Value, key: &str) -> Result<Vec<u64>> {
    request
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| Error::input(format!("{key} must be a list")))?
        .iter()
        .map(|v| {
            v.as_u64()
                .ok_or_else(|| Error::input(format!("{key} must hold numbers")))
        })
        .collect()
}

fn rows(rows: &[ErrorRow]) -> Value {
    Value::from(
        rows.iter()
            .map(|r| json!([r.kind.to_string(), r.number, r.points]))
            .collect::<Vec<_>>(),
    )
}

fn drc<'a>(specs: &'a mut Specs, request: &Value) -> Result<&'a mut Opened> {
    let handle = number(request, "handle")?;
    specs
        .drcs
        .get_mut(&handle)
        .ok_or_else(|| Error::input("no such DRC handle"))
}

fn review<'a>(specs: &'a mut Specs, request: &Value) -> Result<&'a mut Review> {
    match drc(specs, request)? {
        Opened::Pack(r) => Ok(r),
        Opened::Ascii(_) => Err(Error::new(
            ErrorKind::Unsupported,
            "an ASCII results file has no review state",
        )),
    }
}

fn notes(review: &mut Review) -> Value {
    json!({
        "notes": review.notes().iter().map(|n| json!({"text": n.text, "members": n.members})).collect::<Vec<_>>(),
        "notices": std::mem::take(&mut review.notices),
    })
}

fn counts(review: &Review) -> Result<Value> {
    Ok(Value::from(
        (0..review.pack.checks.len())
            .map(|c| review.status_counts(c).map(|(w, t)| json!([w, t])))
            .collect::<Result<Vec<_>>>()?,
    ))
}

fn drc_handle(
    op: &str,
    request: &Value,
    specs: &mut Specs,
    cancelled: &AtomicUsize,
) -> Result<Value> {
    match op {
        "drc_busy" => {
            let pack = floe_app_core::cache::pack_path(Path::new(text(request, "db")?))?;
            let key = floe_vfs::lock::key(floe_vfs::lock::Kind::Pack, &pack.to_string_lossy());
            Ok(floe_vfs::lock::opening_refusal(&key)
                .map_or(Value::Null, |b| Value::from(b.to_string())))
        }
        "drc_find" => {
            let pack = floe_app_core::cache::pack_path(Path::new(text(request, "db")?))?;
            Ok(if pack.exists() {
                json!(pack)
            } else {
                Value::Null
            })
        }
        "drc_open" => {
            let path = PathBuf::from(text(request, "path")?);
            let reviewer = request.get("reviewer").and_then(Value::as_str);
            let (mut opened, mut notices) = match request.get("mode").and_then(Value::as_str) {
                Some("pack") => {
                    let source = request.get("source").and_then(Value::as_str).map(Path::new);
                    (
                        Opened::Pack(Box::new(Review::open(&path, source, reviewer, cancelled)?)),
                        Vec::new(),
                    )
                }
                _ => Opened::load(&path, reviewer, cancelled)?,
            };
            let mut result = json!({
                "packed": matches!(opened, Opened::Pack(_)),
                "path": opened.path(),
                "cell": opened.cell(),
                "total": opened.total(),
                "precision": opened.precision(),
                "checks": opened.checks(),
            });
            if let Opened::Pack(r) = &mut opened {
                result["counts"] = counts(r)?;
                result["waive_path"] = json!(r.waive_path);
                result["note_path"] = json!(r.note_path);
                let n = notes(r);
                result["notes"] = n["notes"].clone();
                notices.extend(
                    n["notices"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_string)),
                );
            }
            result["notices"] = json!(notices);
            specs.next += 1;
            result["handle"] = json!(specs.next);
            specs.drcs.insert(specs.next, opened);
            Ok(result)
        }
        "drc_close" => {
            specs.drcs.remove(&number(request, "handle")?);
            Ok(Value::Null)
        }
        "drc_errors" => {
            let (check, start, count) = (
                number(request, "check")? as usize,
                number(request, "start")?,
                number(request, "count")?,
            );
            Ok(rows(&drc(specs, request)?.errors(
                check,
                start,
                count.min(4096),
                cancelled,
            )?))
        }
        "drc_cd" => {
            let (check, error) = (
                number(request, "check")? as usize,
                number(request, "error")?,
            );
            Ok(json!(drc(specs, request)?.cd(check, error, cancelled)?))
        }
        "drc_status" => {
            let (check, start, count) = (
                number(request, "check")? as usize,
                number(request, "start")?,
                number(request, "count")?,
            );
            let bytes = review(specs, request)?.statuses(check, start, count.min(1 << 22))?;
            Ok(Value::from(
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            ))
        }
        "drc_set_status" => {
            let check = number(request, "check")? as usize;
            let status = u8::try_from(number(request, "status")?)
                .map_err(|_| Error::input("status is a byte"))?;
            let errors = numbers(request, "errors")?;
            let r = review(specs, request)?;
            r.set_status(check, &errors, status)?;
            let (w, t) = r.status_counts(check)?;
            Ok(json!({"counts": [w, t]}))
        }
        "drc_status_page" => {
            let waived = request
                .get("waived")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let (check, start, limit) = (
                number(request, "check")? as usize,
                number(request, "start")?,
                number(request, "limit")?,
            );
            Ok(json!(review(specs, request)?.status_page(
                check,
                waived,
                start,
                limit.min(1 << 20)
            )?))
        }
        "drc_status_rank" => {
            let waived = request
                .get("waived")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let (check, error) = (
                number(request, "check")? as usize,
                number(request, "error")?,
            );
            Ok(json!(
                review(specs, request)?.status_rank(check, waived, error)?
            ))
        }
        "drc_query" => {
            let b = request
                .get("bbox")
                .and_then(Value::as_array)
                .filter(|b| b.len() == 4)
                .ok_or_else(|| Error::input("bbox must be four numbers"))?
                .iter()
                .map(|v| {
                    v.as_f64()
                        .ok_or_else(|| Error::input("bbox must be four numbers"))
                })
                .collect::<Result<Vec<f64>>>()?;
            let cap = request.get("cap").and_then(Value::as_u64).unwrap_or(2000) as usize;
            let checks = match request.get("checks") {
                None | Some(Value::Null) => None,
                Some(_) => Some(
                    numbers(request, "checks")?
                        .into_iter()
                        .map(|c| c as usize)
                        .collect::<BTreeSet<_>>(),
                ),
            };
            let waived = request.get("waived").and_then(Value::as_bool);
            let r = review(specs, request)?;
            let hits = r.query(
                [b[0], b[1], b[2], b[3]],
                cap,
                checks.as_ref(),
                waived,
                cancelled,
            )?;
            Ok(Value::from(
                query_rows(r, hits)
                    .into_iter()
                    .map(|(c, e, row)| json!([c, e, row.kind.to_string(), row.number, row.points]))
                    .collect::<Vec<_>>(),
            ))
        }
        "drc_set_note" | "drc_clear_note" => {
            let gids = numbers(request, "gids")?;
            let r = review(specs, request)?;
            if op == "drc_set_note" {
                r.set_note(&gids, text(request, "text")?, cancelled)?;
            } else {
                r.clear_note(&gids, cancelled)?;
            }
            Ok(notes(r))
        }
        "drc_note_export" => {
            let path = PathBuf::from(text(request, "path")?);
            review(specs, request)?.note_export(&path, cancelled)?;
            Ok(Value::Null)
        }
        "drc_note_import" => {
            let path = PathBuf::from(text(request, "path")?);
            let r = review(specs, request)?;
            let count = r.note_import(&path, cancelled)?;
            let mut n = notes(r);
            n["count"] = json!(count);
            Ok(n)
        }
        "drc_waive_export" => {
            let path = PathBuf::from(text(request, "path")?);
            review(specs, request)?.waive_export(&path)?;
            Ok(Value::Null)
        }
        "drc_waive_import" => {
            let path = PathBuf::from(text(request, "path")?);
            let r = review(specs, request)?;
            let waived = r.waive_import(&path)?;
            Ok(json!({"waived": waived, "counts": counts(r)?}))
        }
        "svrf_rules" => {
            let (rules, warning) = desktop::rules_sidecar(Path::new(text(request, "path")?))?;
            Ok(json!({"rules": rules, "warning": warning}))
        }
        "svrf_operands" => Ok(json!(floe_app_core::svrf::rhs_operands(text(
            request, "rhs"
        )?))),
        other => Err(Error::new(
            ErrorKind::Unsupported,
            format!("unknown request: {other}"),
        )),
    }
}

fn handle(request: &Value, specs: &mut Specs, cancelled: &AtomicUsize) -> Result<Value> {
    let op = text(request, "op")?;
    if op.starts_with("drc_") || op.starts_with("svrf_") {
        return drc_handle(op, request, specs, cancelled);
    }
    match op {
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
