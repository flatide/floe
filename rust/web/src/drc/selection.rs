//! Read-only review groups: mutations affect this open view's memory only.
use super::{dto, http::failure, Failure};
use crate::transport::{self, Gate};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use floe_app_core::drc::{SelectionMode, Selections, SELECTION_INPUT, SELECTION_ITEMS};
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) struct Groups {
    revision: u64,
    selected: Selections,
}
impl Default for Groups {
    fn default() -> Self {
        Self {
            revision: 1,
            selected: Selections::default(),
        }
    }
}
impl Groups {
    pub(super) fn check(&self, base: u64) -> Result<(), Failure> {
        if base == self.revision {
            Ok(())
        } else {
            Err("drc_selection_conflict")
        }
    }
    pub(super) fn ids(
        &self,
        base: u64,
        check: usize,
    ) -> Result<std::collections::BTreeSet<u64>, Failure> {
        self.check(base)?;
        Ok(self.selected.ids(check))
    }
    pub(super) fn snapshot(&self) -> Value {
        let rules = self.selected.rules().map(|(check, ids)| json!({"check":check.to_string(), "errors":ids.iter().map(u64::to_string).collect::<Vec<_>>() })).collect::<Vec<_>>();
        json!({"selection_rev":self.revision.to_string(),"total":self.selected.total().to_string(),"limit":SELECTION_ITEMS,"rules":rules})
    }
    pub(super) fn apply(
        &mut self,
        base: u64,
        check: Option<usize>,
        mode: SelectionMode,
        hits: &[u64],
    ) -> Result<Value, Failure> {
        self.check(base)?;
        let next = self.revision.checked_add(1).ok_or("drc_selection_limit")?;
        if let Some(ci) = check {
            self.selected
                .apply(ci, hits, mode)
                .map_err(|_| "drc_selection_limit")?;
        } else {
            self.selected.clear();
        }
        // Consume even a no-op. Add/toggle are commands, not full snapshots;
        // an uncertain retry must conflict, never apply a toggle twice.
        self.revision = next;
        Ok(self.snapshot())
    }
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Replace,
    Add,
    Toggle,
}
impl From<Mode> for SelectionMode {
    fn from(v: Mode) -> Self {
        match v {
            Mode::Replace => Self::Replace,
            Mode::Add => Self::Add,
            Mode::Toggle => Self::Toggle,
        }
    }
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Edit {
    Apply {
        check: String,
        errors: Vec<String>,
        mode: Mode,
        bbox_um: Option<[String; 4]>,
        waived: Option<bool>,
    },
    ClearAll {},
}
struct Prepared {
    check: Option<usize>,
    mode: SelectionMode,
    errors: Vec<u64>,
    bbox: Option<[f64; 4]>,
    waived: Option<bool>,
}
impl Edit {
    fn prepare(self) -> Result<Prepared, Failure> {
        Ok(match self {
            Self::ClearAll {} => Prepared {
                check: None,
                mode: SelectionMode::Replace,
                errors: Vec::new(),
                bbox: None,
                waived: None,
            },
            Self::Apply {
                check,
                errors,
                mode,
                bbox_um,
                waived,
            } => {
                if errors.len() > SELECTION_INPUT {
                    return Err("invalid_drc_request");
                }
                Prepared {
                    check: Some(dto::index(&check)?),
                    mode: mode.into(),
                    errors: errors
                        .iter()
                        .map(|s| dto::number(s))
                        .collect::<Result<_, _>>()?,
                    bbox: bbox_um.map(dto::bbox).transpose()?,
                    waived,
                }
            }
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    revision: String,
    base_selection_rev: String,
    state_rev: Option<String>,
    body: Edit,
}
pub(super) fn routes() -> Router<Gate> {
    Router::new().route(
        "/api/v1/drc/{id}/views/{view}/selection",
        get(snapshot).post(change),
    )
}
async fn snapshot(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, view)): Path<(String, String)>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(drc) = gate.drc.as_ref().and_then(|r| r.current(&id)) else {
        return failure("drc_unavailable");
    };
    let Some(v) = gate
        .active_view()
        .filter(|v| v.id == view && v.source_id == drc.source_id && !v.controller.is_finished())
    else {
        return failure("drc_context_changed");
    };
    let revision = drc.revision();
    match gate
        .drc
        .as_ref()
        .unwrap()
        .with_panel(&drc, &revision, &v, |p| Ok(p.groups.snapshot()))
    {
        Ok(state) => {
            Json(json!({"revision":revision,"view_id":view,"state":state})).into_response()
        }
        Err(e) => failure(e),
    }
}
async fn change(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, view)): Path<(String, String)>,
    body: Result<Json<Set>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let session = match transport::http_session(&gate, &headers) {
        Ok(s) => s,
        Err(e) => return transport::error(e),
    };
    let Ok(Json(body)) = body else {
        return failure("invalid_drc_request");
    };
    let Ok(base) = crate::view::counter(&body.base_selection_rev) else {
        return failure("invalid_drc_request");
    };
    let Ok(edit) = body.body.prepare() else {
        return failure("invalid_drc_request");
    };
    if body
        .state_rev
        .as_ref()
        .is_some_and(|r| crate::view::counter(r).is_err())
        || edit.bbox.is_some() && body.state_rev.is_none()
    {
        return failure("invalid_drc_request");
    }
    let Some(drc) = gate.drc.as_ref().and_then(|r| r.current(&id)) else {
        return failure("drc_unavailable");
    };
    let matches = || {
        gate.active_view().filter(|v| {
            v.id == view
                && v.source_id == drc.source_id
                && !v.controller.is_finished()
                && gate.drc.as_ref().unwrap().is_revision(&drc, &body.revision)
                && body
                    .state_rev
                    .as_ref()
                    .is_none_or(|rev| v.controller.snapshot().state_rev.to_string() == *rev)
        })
    };
    let Some(v) = matches() else {
        return failure("drc_context_changed");
    };
    if let Err(e) = gate
        .drc
        .as_ref()
        .unwrap()
        .with_panel(&drc, &body.revision, &v, |p| p.groups.check(base))
    {
        return failure(e);
    }
    let mut ticket = match drc.enqueue(dto::Command::SelectionCandidates {
        check: edit.check,
        errors: edit.errors,
        bbox_um: edit.bbox,
        waived: edit.waived,
    }) {
        Ok(t) => t,
        Err(e) => return failure(e),
    };
    let result = ticket.result().await;
    if !gate.alive(&session) {
        return transport::error(StatusCode::UNAUTHORIZED);
    }
    if matches().is_none() {
        return failure("drc_context_changed");
    }
    let bytes = match result {
        Ok(v) => v,
        Err(e) => return failure(e),
    };
    // Small metadata response from our actor; never deserialize point arrays
    // or retain them in per-view state. Only the canonical local IDs survive.
    #[derive(Deserialize)]
    struct Candidate {
        local: String,
    }
    #[derive(Deserialize)]
    struct Candidates {
        rows: Vec<Candidate>,
    }
    let hits = serde_json::from_slice::<Candidates>(&bytes)
        .map_err(|_| "drc_read_error")
        .and_then(|v| {
            v.rows
                .iter()
                .map(|r| dto::number(&r.local))
                .collect::<Result<Vec<_>, _>>()
        });
    let hits = match hits {
        Ok(v) => v,
        Err(e) => return failure(e),
    };
    let updated = gate
        .drc
        .as_ref()
        .unwrap()
        .with_panel(&drc, &body.revision, &v, |p| {
            p.groups.apply(base, edit.check, edit.mode, &hits)
        });
    match updated {
        Ok(state) => {
            Json(json!({"revision":body.revision,"view_id":view,"state":state})).into_response()
        }
        Err(e) => failure(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commands_consume_revisions_and_uncertain_toggle_is_never_replayed() {
        let mut g = Groups::default();
        assert_eq!(
            g.apply(1, Some(0), SelectionMode::Toggle, &[7]).unwrap()["selection_rev"],
            "2"
        );
        let before = g.snapshot();
        assert_eq!(
            g.apply(1, Some(0), SelectionMode::Toggle, &[7])
                .unwrap_err(),
            "drc_selection_conflict"
        );
        assert_eq!(g.snapshot(), before);
        assert_eq!(
            g.apply(2, Some(0), SelectionMode::Add, &[]).unwrap()["selection_rev"],
            "3"
        );
        assert_eq!(
            g.apply(3, None, SelectionMode::Replace, &[]).unwrap()["total"],
            "0"
        );
        g.revision = u64::MAX;
        assert_eq!(
            g.apply(u64::MAX, Some(1), SelectionMode::Add, &[1])
                .unwrap_err(),
            "drc_selection_limit"
        );
        assert_eq!(g.snapshot()["total"], "0");
    }
    #[test]
    fn scoped_state_bound_and_conflicts_preserve_old_groups() {
        let mut g = Groups::default();
        for ci in 0..SELECTION_ITEMS {
            g.selected
                .apply(usize::MAX - ci, &[u64::MAX], SelectionMode::Add)
                .unwrap();
        }
        g.revision = SELECTION_ITEMS as u64 + 1;
        let before = g.snapshot();
        assert_eq!(
            g.apply(
                SELECTION_ITEMS as u64 + 1,
                Some(0),
                SelectionMode::Add,
                &[8]
            )
            .unwrap_err(),
            "drc_selection_limit"
        );
        assert_eq!(g.snapshot(), before);
        assert!(serde_json::to_vec(&before).unwrap().len() < super::super::RESPONSE_BYTES);
    }
    #[test]
    fn two_concurrent_same_revision_mutations_commit_only_one() {
        use std::sync::{Arc, Barrier, Mutex};
        let state = Arc::new(Mutex::new(Groups::default()));
        let barrier = Arc::new(Barrier::new(2));
        let handles = [1, 2].map(|id| {
            let s = Arc::clone(&state);
            let b = Arc::clone(&barrier);
            std::thread::spawn(move || {
                b.wait();
                s.lock()
                    .unwrap()
                    .apply(1, Some(0), SelectionMode::Toggle, &[id])
            })
        });
        let values = handles.map(|h| h.join().unwrap());
        assert_eq!(values.iter().filter(|v| v.is_ok()).count(), 1);
        assert_eq!(
            values
                .iter()
                .filter(|v| matches!(v, Err("drc_selection_conflict")))
                .count(),
            1
        );
        assert_eq!(state.lock().unwrap().snapshot()["total"], "1");
    }
    #[test]
    fn invalid_ids_bounds_modes_and_fields_are_rejected() {
        for value in [
            json!({"kind":"apply","check":"00","errors":[],"mode":"replace"}),
            json!({"kind":"apply","check":"0","errors":["-1"],"mode":"add"}),
            json!({"kind":"apply","check":"0","errors":vec!["0";65],"mode":"toggle"}),
            json!({"kind":"apply","check":"0","errors":[],"mode":"add","bbox_um":["0","NaN","1","1"]}),
        ] {
            assert!(serde_json::from_value::<Edit>(value)
                .unwrap()
                .prepare()
                .is_err());
        }
        for value in [
            json!({"kind":"clear_all","path":"/etc/passwd"}),
            json!({"kind":"apply","check":"0","errors":[0],"mode":"replace"}),
            json!({"kind":"apply","check":"0","errors":[],"mode":"unknown"}),
        ] {
            assert!(serde_json::from_value::<Edit>(value).is_err());
        }
    }
}
