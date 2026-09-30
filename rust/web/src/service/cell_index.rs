//! The open view's cell index (design.ovh), added to its own mutable caches
//! on explicit owner request. The view stays open: the summary is renamed
//! into place and its renderer picks it up on the next cell query.
use super::*;
use floe_app_core::cell_index::{self, Report, Stage};

/// What admission needs of the current view, without the attachment.
pub(super) struct Facts<'a> {
    pub id: &'a str,
    pub source_id: &'a str,
    pub index_revision: Option<&'a str>,
    pub levels: Option<&'a [String]>,
}
impl<'a> Facts<'a> {
    pub fn of(view: &'a Attachment) -> Self {
        Self {
            id: &view.id,
            source_id: &view.source_id,
            index_revision: view.index_revision.as_deref(),
            levels: view.levels.as_deref(),
        }
    }
}

/// The source and loaded levels of the named view. A sealed revision is
/// never amended; a new revision already includes design.ovh.
pub(super) fn admit(
    view: Option<Facts<'_>>,
    view_id: &str,
) -> std::result::Result<(String, Option<BTreeSet<i64>>), &'static str> {
    index_open::identity(view_id)?;
    let view = view.filter(|v| v.id == view_id).ok_or("view_unavailable")?;
    if view.index_revision.is_some() {
        return Err("index_revision_sealed");
    }
    let levels = view
        .levels
        .map(|ids| {
            ids.iter()
                .map(|s| match s.parse::<i64>() {
                    Ok(n) if *s == n.to_string() => Ok(n),
                    _ => Err("invalid_request"),
                })
                .collect::<std::result::Result<BTreeSet<_>, _>>()
        })
        .transpose()?;
    Ok((view.source_id.to_owned(), levels))
}

fn state(seq: u64, view_id: &str, phase: &str, r: &Report) -> Value {
    let stage = match r.stage {
        Stage::Preparing => "preparing",
        Stage::Checking => "checking",
        Stage::Building => "building",
        Stage::Done => "done",
    };
    json!({"seq":seq.to_string(),"kind":"cell_index","phase":phase,"view_id":view_id,
        "stage":stage,"current":r.current,"total":r.total,"built":r.built,"kept":r.kept,
        "skipped":r.skipped,"failed":r.failed,"error":r.failure.map(view::safe_error)})
}

pub(super) fn execute(
    inner: &Inner,
    seq: u64,
    view_id: String,
    source: Arc<RegisteredSource>,
    levels: Option<BTreeSet<i64>>,
    stop: &AtomicUsize,
) -> Result<Value> {
    {
        let s = inner.state.lock().unwrap();
        let view = s
            .view
            .as_ref()
            .filter(|v| v.id == view_id)
            .ok_or_else(|| Error::new(ErrorKind::Busy, "view changed"))?;
        if view.index_revision.is_some() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "index revisions are sealed",
            ));
        }
    }
    let mut last = Report::default();
    let result = cell_index::run(
        &inner.resources,
        &source,
        levels.as_ref(),
        &inner.indexer,
        stop,
        &mut |r| {
            last = *r;
            let running = state(seq, &view_id, "running", r);
            inner
                .state
                .lock()
                .unwrap()
                .ledger
                .update(seq, running, false);
        },
    );
    Ok(match result {
        Ok(r) => state(
            seq,
            &view_id,
            if r.skipped + r.failed > 0 {
                "incomplete"
            } else {
                "succeeded"
            },
            &r,
        ),
        // Deck sources already summarized stay summarized: report them.
        Err(e) => {
            let mut v = state(
                seq,
                &view_id,
                if e.kind == ErrorKind::Cancelled {
                    "cancelled"
                } else {
                    "failed"
                },
                &last,
            );
            v["error"] = json!(view::safe_error(e.kind));
            v
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn facts<'a>(
        id: &'a str,
        revision: Option<&'a str>,
        levels: Option<&'a [String]>,
    ) -> Option<Facts<'a>> {
        Some(Facts {
            id,
            source_id: "s",
            index_revision: revision,
            levels,
        })
    }
    #[test]
    fn wire_names_only_the_open_view() {
        let id = "a".repeat(64);
        let request = json!({"kind":"cell_index","seq":"1","view_id":id});
        assert!(matches!(
            serde_json::from_value::<OperationDto>(request.clone()).unwrap(),
            OperationDto::CellIndex { seq, view_id } if seq == "1" && view_id == id
        ));
        for key in ["view_id", "seq"] {
            let mut missing = request.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<OperationDto>(missing).is_err());
            let mut null = request.clone();
            null[key] = Value::Null;
            assert!(serde_json::from_value::<OperationDto>(null).is_err());
        }
        // No path, source, level, revision or option authority on the wire.
        for key in [
            "source_id",
            "levels",
            "path",
            "revision",
            "options",
            "force",
            "approved",
        ] {
            let mut extra = request.clone();
            extra[key] = json!("x");
            assert!(
                serde_json::from_value::<OperationDto>(extra).is_err(),
                "{key}"
            );
        }
    }
    #[test]
    fn admission_takes_the_named_mutable_view_and_its_levels() {
        let id = "a".repeat(64);
        assert_eq!(
            admit(facts(&id, None, None), &id).unwrap(),
            ("s".to_string(), None)
        );
        let ids = ["2".to_string(), "1".to_string()];
        assert_eq!(
            admit(facts(&id, None, Some(&ids)), &id).unwrap().1,
            Some(BTreeSet::from([1, 2]))
        );
        let other = "b".repeat(64);
        assert_eq!(
            admit(facts(&id, None, None), &other),
            Err("view_unavailable")
        );
        assert_eq!(admit(None, &id), Err("view_unavailable"));
        for bad in ["A".repeat(64), "a".repeat(63), String::new()] {
            assert_eq!(admit(facts(&bad, None, None), &bad), Err("invalid_request"));
        }
        // Refused at admission; the worker checks the live view again.
        assert_eq!(
            admit(facts(&id, Some("7"), None), &id),
            Err("index_revision_sealed")
        );
        let odd = ["01".to_string()];
        assert_eq!(
            admit(facts(&id, None, Some(&odd)), &id),
            Err("invalid_request")
        );
    }
    #[test]
    fn progress_and_terminal_states_carry_counts_not_paths() {
        let r = Report {
            stage: Stage::Building,
            current: 2,
            total: 3,
            built: 1,
            kept: 0,
            skipped: 1,
            failed: 0,
            failure: None,
        };
        let v = state(9, "v", "running", &r);
        assert_eq!(
            v,
            json!({"seq":"9","kind":"cell_index","phase":"running","view_id":"v","stage":"building",
                "current":2,"total":3,"built":1,"kept":0,"skipped":1,"failed":0,"error":null})
        );
        let failed = Report {
            failed: 1,
            failure: Some(ErrorKind::Worker),
            ..r
        };
        assert_eq!(
            state(9, "v", "incomplete", &failed)["error"],
            "worker_failed"
        );
    }
}
