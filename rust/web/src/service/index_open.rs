//! Explicit owner consent binds indexing and its follow-up to a failed open.
//! A browser reload only observes the ledger; it never allocates the second job.
use super::*;

/// Only a verified cache problem offers an index retry. Dataset readers also
/// report missing metadata as I/O and an entirely unindexed deck as Input; do
/// not turn arbitrary source/format/registration failures into write proposals.
/// This error-only probe stays off the HTTP reactor and uses selected read leases.
pub(super) fn retryable(inner: &Inner, open: &OpenCommand, stop: &AtomicUsize) -> Result<bool> {
    needs_index(inner, &open.source, open.levels.as_ref(), stop)
}

pub(super) fn needs_index(
    inner: &Inner,
    registered: &RegisteredSource,
    levels: Option<&BTreeSet<i64>>,
    stop: &AtomicUsize,
) -> Result<bool> {
    use floe_app_core::{
        cache,
        jobdeck::{parser::JobDeck, sources::SourceCatalog},
    };
    registered.validate(stop)?;
    let source = registered.path();
    let names: BTreeSet<String> = if registered.deck {
        JobDeck::read(source, true, stop)?
            .sources(levels)
            .into_iter()
            .map(str::to_owned)
            .collect()
    } else {
        [source
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error::input("source name requires UTF-8"))?
            .to_owned()]
        .into()
    };
    let mut catalog = SourceCatalog::new(source.parent().expect("absolute registered source"))?;
    let _read = inner.resources.read(
        names
            .iter()
            .map(|tc| cache::cache_path(&catalog.resolve(tc)))
            .collect::<Result<Vec<_>>>()?,
    )?;
    for tc in names {
        let info = catalog.probe(&tc, stop)?;
        if info.ok() && !info.indexed {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn identity(id: &str) -> std::result::Result<(), &'static str> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("invalid_request");
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum IndexTarget {
    Empty {},
    Replace { view_id: String, state_rev: String },
}
impl IndexTarget {
    pub(super) fn core(self) -> std::result::Result<Option<(String, u64)>, &'static str> {
        match self {
            Self::Empty {} => Ok(None),
            Self::Replace { view_id, state_rev } => {
                if view_id.len() != 64
                    || !view_id
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                {
                    return Err("invalid_request");
                }
                Ok(Some((view_id, view::counter(&state_rev)?)))
            }
        }
    }
}

pub(super) fn proposal(seq: u64, open: &OpenCommand) -> Value {
    let mode = match open.mode {
        Mode::Level => "level",
        Mode::Chip => "chip",
        Mode::Layer => "layer",
    };
    let levels = match &open.levels {
        Some(ids) => json!({"mode":"only","count":ids.len()}),
        None => json!({"mode":"all"}),
    };
    let mut out = json!({"open_seq":seq.to_string(),"source_id":open.source_id,"title":open.source.title,
        "mode":mode,"levels":levels,"display_policy":match open.display_policy {OpenDisplay::Window=>"window",OpenDisplay::Explicit=>"explicit"}});
    if let Some(camera) = &open.reselect {
        let (id, rev) = open.replace.as_ref().expect("reselection anchor");
        out["reselect"] = json!({"target":{"kind":"replace","view_id":id,"state_rev":rev.to_string()},
            "pixels":[camera.viewport.width,camera.viewport.height]});
    }
    out
}

pub(super) fn open_state(mut state: Value, index: Option<&Value>) -> Value {
    if let Some(index) = index {
        state["kind"] = json!("index_open");
        state["request_id"] = index["request_id"].clone();
        state["open_seq"] = index["open_seq"].clone();
        state["stage"] = json!("open");
        state["index"] = index.clone();
    }
    state
}
fn failure(seq: u64, kind: &str, error: Error) -> Value {
    json!({"seq":seq.to_string(),"kind":kind,
        "phase":if error.kind==ErrorKind::Cancelled {"cancelled"}else{"failed"},
        "error":view::safe_error(error.kind)})
}
fn index_envelope(seq: u64, index: &Value) -> Value {
    json!({"seq":seq.to_string(),"kind":"index_open","stage":"index",
        "phase":index["phase"],"error":index["error"],"index":index,"request_id":index["request_id"],"open_seq":index["open_seq"]})
}

pub(super) fn execute(
    inner: &Inner,
    seq: u64,
    open: OpenCommand,
    options: IndexOptions,
    stop: Arc<AtomicUsize>,
    request_id: &str,
    open_seq: u64,
) -> Result<Value> {
    let identified = |mut state: Value| {
        state["request_id"] = json!(request_id);
        state["open_seq"] = json!(open_seq.to_string());
        state
    };
    let index = identified(
        (|| {
            floe_app_core::check_cancelled(&stop)?;
            // Reject an obsolete approval before *any* native/index filesystem work.
            // Check again in open::execute after indexing, and at cutover commit.
            open::anchor(inner, &open.replace)?;
            let mut job = ManagedIndex::start(
                &inner.resources,
                Arc::clone(&open.source),
                open.levels.clone(),
                options,
                inner.indexer.clone(),
            )?;
            loop {
                if stop.load(Ordering::Relaxed) != 0 {
                    job.cancel();
                }
                if job.is_finished() {
                    job.close()?; // releases writer/CPU permits and reaps native children
                    return Ok(index_state(seq, &job.snapshot()));
                }
                let state = index_envelope(seq, &identified(index_state(seq, &job.snapshot())));
                inner.state.lock().unwrap().ledger.update(seq, state, false);
                thread::sleep(Duration::from_millis(20));
            }
        })()
        .unwrap_or_else(|e| failure(seq, "index", e)),
    );
    if index["phase"] != "succeeded" {
        // Partial decks, failed native work and cancellation never silently open.
        return Ok(index_envelope(seq, &index));
    }
    let result = floe_app_core::check_cancelled(&stop)
        .and_then(|()| open::execute(inner, seq, open, stop, Some(&index)))
        .unwrap_or_else(|e| failure(seq, "open", e));
    // A late cancel/error cannot undo already committed index files. Report the
    // successful index separately from the failed/skipped viewport cutover.
    Ok(open_state(result, Some(&index)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_and_target_are_strict_and_not_implicit_defaults() {
        assert!(identity(&"a".repeat(64)).is_ok());
        for value in [
            "".to_owned(),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            "x".repeat(64),
            "한".repeat(64),
        ] {
            assert!(identity(&value).is_err());
        }
        let request = json!({"kind":"index_open","seq":"2","open_seq":"1","approved":true,
            "request_id":"a".repeat(64),"target":{"kind":"empty"},"pixels":[137,103],"options":{"jobs":2}});
        assert!(serde_json::from_value::<OperationDto>(request.clone()).is_ok());
        for key in [
            "approved",
            "target",
            "open_seq",
            "pixels",
            "options",
            "request_id",
        ] {
            let mut missing = request.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(
                serde_json::from_value::<OperationDto>(missing).is_err(),
                "{key}"
            );
            let mut null = request.clone();
            null[key] = Value::Null;
            assert!(
                serde_json::from_value::<OperationDto>(null).is_err(),
                "{key}"
            );
        }
        for target in [
            json!({"kind":"current"}),
            json!({"kind":"empty","view_id":"x"}),
            json!({"kind":"replace","view_id":"x"}),
        ] {
            assert!(serde_json::from_value::<IndexTarget>(target).is_err());
        }
        assert!(IndexTarget::Replace {
            view_id: "x".into(),
            state_rev: "1".into()
        }
        .core()
        .is_err());
        assert!(IndexTarget::Replace {
            view_id: "a".repeat(64),
            state_rev: "01".into()
        }
        .core()
        .is_err());
    }
}
