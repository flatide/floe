//! Read-only guest facade. No registry catalog, reviewer, note or publication
//! methods are exposed. A guest panel is never an owner's Attachment panel.
use super::{dto, panel, Failure, Service, Ticket};
use floe_app_core::{drc::SelectionMode, view::Snapshot};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::OwnedSemaphorePermit;

pub(crate) use super::panel::Panel;
#[derive(Deserialize)]
#[serde(transparent)]
pub(crate) struct Read(pub super::Request);
impl Read {
    pub fn needs_view(&self) -> bool {
        matches!(
            &self.0,
            super::Request::Focus { .. }
                | super::Request::InView { .. }
                | super::Request::List { in_view: true, .. }
                | super::Request::FilteredStep { in_view: true, .. }
        )
    }
    pub fn selected(&self) -> Result<Option<(u64, usize)>, Failure> {
        self.0.selection_filter()
    }
    pub fn submit(
        self,
        reader: &Service,
        revision: &str,
        state: &Snapshot,
        dbu: f64,
        panel: &std::sync::Mutex<Panel>,
        permit: OwnedSemaphorePermit,
    ) -> Result<Ticket, Failure> {
        use super::Request::*;
        // Exhaustive classification: new owner reads do not silently become
        // guest capabilities. SVRF metadata and layer isolation are not granted.
        match &self.0 {
            Rules {
                metric: Some(_), ..
            }
            | Types { .. }
            | Comparison { .. }
            | Focus { isolate: true, .. } => return Err("forbidden"),
            Rules { .. }
            | Rule { .. }
            | Errors { .. }
            | Geometry { .. }
            | Measurements { .. }
            | Records { .. }
            | Focus { .. }
            | Step { .. }
            | InView { .. }
            | Query { .. }
            | List { .. }
            | FilteredStep { .. } => (),
        }
        let selected = self
            .selected()?
            .map(|(r, c)| panel.lock().unwrap().groups.ids(r, c))
            .transpose()?;
        let mut command = self.0.core()?;
        let context = dto::FocusContext {
            bbox_dbu: state.state.viewport.bbox,
            dbu,
            pixels: [state.state.viewport.width, state.state.viewport.height],
        };
        match &mut command {
            dto::Command::Focus {
                context: target, ..
            }
            | dto::Command::InView {
                context: target, ..
            } => *target = Some(context),
            dto::Command::List { filters, .. } | dto::Command::FilteredStep { filters, .. } => {
                filters.context = Some(context);
                filters.selected = selected;
            }
            _ => (),
        }
        reader.enqueue_reserved(command, None, Some(permit), Some(revision))
    }
}
pub(crate) fn check_selection(
    panel: &Panel,
    revision: Option<(u64, usize)>,
) -> Result<(), Failure> {
    if let Some((r, _)) = revision {
        panel.groups.check(r)?;
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(transparent)]
pub(crate) struct PanelData(panel::Data);
impl PanelData {
    pub fn check(&self, panel: &Panel, base: u64, state_rev: u64) -> Result<(), Failure> {
        self.0.validate()?;
        if self.0.note_target.is_some() || self.0.metric.is_some() {
            return Err("forbidden");
        }
        if self
            .0
            .query
            .as_ref()
            .is_some_and(|q| crate::view::counter(&q.state_rev).is_ok_and(|v| v > state_rev))
        {
            return Err("invalid_drc_request");
        }
        panel.check(base, &self.0)
    }
    pub fn submit(
        &self,
        reader: &Service,
        revision: &str,
        permit: OwnedSemaphorePermit,
    ) -> Result<Ticket, Failure> {
        reader.enqueue_reserved(
            dto::Command::ValidatePanel(Box::new(self.0.clone())),
            None,
            Some(permit),
            Some(revision),
        )
    }
    pub fn apply(self, panel: &mut Panel, base: u64) -> Result<Value, Failure> {
        panel.set(base, self.0)
    }
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum SelectionEdit {
    Apply {
        check: String,
        errors: Vec<String>,
        mode: SelectionAction,
        bbox_um: Option<[String; 4]>,
        waived: Option<bool>,
    },
    ClearAll {},
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SelectionAction {
    Replace,
    Add,
    Toggle,
}
pub(crate) struct SelectionChange {
    check: Option<usize>,
    ids: Vec<u64>,
    mode: SelectionMode,
    bbox: Option<[f64; 4]>,
    waived: Option<bool>,
}
impl SelectionEdit {
    pub fn prepare(self) -> Result<SelectionChange, Failure> {
        match self {
            Self::ClearAll {} => Ok(SelectionChange {
                check: None,
                ids: vec![],
                mode: SelectionMode::Replace,
                bbox: None,
                waived: None,
            }),
            Self::Apply {
                check,
                errors,
                mode,
                bbox_um,
                waived,
            } => {
                if errors.len() > floe_app_core::drc::SELECTION_INPUT {
                    return Err("invalid_drc_request");
                }
                Ok(SelectionChange {
                    check: Some(dto::index(&check)?),
                    ids: errors
                        .iter()
                        .map(|s| dto::number(s))
                        .collect::<Result<_, _>>()?,
                    mode: match mode {
                        SelectionAction::Replace => SelectionMode::Replace,
                        SelectionAction::Add => SelectionMode::Add,
                        SelectionAction::Toggle => SelectionMode::Toggle,
                    },
                    bbox: bbox_um.map(dto::bbox).transpose()?,
                    waived,
                })
            }
        }
    }
}
impl SelectionChange {
    pub fn needs_view(&self) -> bool {
        self.bbox.is_some()
    }
    pub fn submit(
        &self,
        reader: &Service,
        revision: &str,
        permit: OwnedSemaphorePermit,
    ) -> Result<Ticket, Failure> {
        reader.enqueue_reserved(
            dto::Command::SelectionCandidates {
                check: self.check,
                errors: self.ids.clone(),
                bbox_um: self.bbox,
                waived: self.waived,
            },
            None,
            Some(permit),
            Some(revision),
        )
    }
    pub fn apply(self, panel: &mut Panel, base: u64, bytes: &[u8]) -> Result<Value, Failure> {
        let value: Value = serde_json::from_slice(bytes).map_err(|_| "drc_read_error")?;
        let rows = value["rows"].as_array().ok_or("drc_read_error")?;
        let ids = rows
            .iter()
            .map(|r| dto::number(r["local"].as_str().ok_or("drc_read_error")?))
            .collect::<Result<Vec<_>, _>>()?;
        panel.groups.apply(base, self.check, self.mode, &ids)
    }
}
pub(crate) fn selection(panel: &Panel) -> Value {
    panel.groups.snapshot()
}
pub(crate) fn selection_base(panel: &Panel, base: u64) -> Result<(), Failure> {
    panel.groups.check(base)
}
pub(crate) fn bind(panel: &mut Panel, revision: &str) {
    panel.bind_revision(revision);
}

impl Service {
    pub(crate) fn shared_catalog(&self) -> Result<Value, Failure> {
        let state = self.inner.state.lock().unwrap();
        if state.closed || state.failure.is_some() {
            return Err("drc_unavailable");
        }
        let data = state.metadata.as_ref().ok_or("drc_busy")?;
        // Not even transiently serialize the broad owner catalog.
        Ok(
            json!({"checks":data["checks"],"errors":data["errors"],"precision":data["precision"],
            "format":data["format"],"truncated_records":data["truncated_records"],"read_only":true}),
        )
    }
}
pub(crate) fn project(bytes: &[u8]) -> Result<Vec<u8>, Failure> {
    let mut v: Value = serde_json::from_slice(bytes).map_err(|_| "drc_read_error")?;
    project_value(&mut v);
    serde_json::to_vec(&v).map_err(|_| "drc_read_error")
}
fn project_value(v: &mut Value) {
    match v {
        Value::Object(m) => {
            // Explicit output vocabulary, recursively. Future owner metadata
            // additions (including inside rows) do not inherit a guest grant.
            m.retain(|k, _| {
                [
                    "rows",
                    "next",
                    "scanned",
                    "metric",
                    "waived",
                    "check",
                    "local",
                    "global",
                    "kind",
                    "status",
                    "bbox_um",
                    "points",
                    "name",
                    "name_truncated",
                    "description",
                    "errors",
                    "declared",
                    "original",
                    "hit",
                    "remaining",
                    "selection_rev",
                    "precision",
                    "start",
                    "total",
                    "points_dbu",
                    "points_um",
                    "segments",
                    "endpoints_um",
                    "distance_um",
                    "offset",
                    "navigation",
                    "center_um",
                    "width_um",
                ]
                .contains(&k.as_str())
            });
            for value in m.values_mut() {
                project_value(value);
            }
        }
        Value::Array(a) => {
            for value in a {
                project_value(value);
            }
        }
        _ => (),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_box_candidates_are_bounded_and_need_a_view() {
        let value = json!({"kind":"apply","check":"0","errors":["9007199254740993"],"mode":"toggle","bbox_um":["-1","2","3","4"],"waived":false});
        let edit = serde_json::from_value::<SelectionEdit>(value.clone())
            .unwrap()
            .prepare()
            .unwrap();
        assert!(edit.needs_view());
        assert_eq!(edit.bbox, Some([-1., 2., 3., 4.]));
        assert_eq!(edit.waived, Some(false));
        assert_eq!(edit.ids, [9007199254740993]);
        let mut invalid = value.clone();
        invalid["bbox_um"] = json!(["3", "2", "-1", "4"]);
        assert!(serde_json::from_value::<SelectionEdit>(invalid)
            .unwrap()
            .prepare()
            .is_err());
        let mut huge = value.clone();
        huge["errors"] = json!(vec!["0"; 65]);
        assert!(serde_json::from_value::<SelectionEdit>(huge)
            .unwrap()
            .prepare()
            .is_err());
        let mut plain = value;
        plain.as_object_mut().unwrap().remove("bbox_um");
        assert!(!serde_json::from_value::<SelectionEdit>(plain)
            .unwrap()
            .prepare()
            .unwrap()
            .needs_view());
    }
    #[test]
    fn projection_never_inherits_owner_metadata_or_nested_notes() {
        let source = json!({"name":"WIDTH","description":"width rule","svrf":{"source":"private"},"reviewer":"hidden",
            "rows":[{"local":"9007199254740994","bbox_um":["0","1","2","3"],"status":1,"note":"secret","review":{"body":"hidden"}}],"unknown":"hidden"});
        let projected: Value =
            serde_json::from_slice(&project(&serde_json::to_vec(&source).unwrap()).unwrap())
                .unwrap();
        assert_eq!(
            projected,
            json!({"name":"WIDTH","description":"width rule","rows":[{"local":"9007199254740994","bbox_um":["0","1","2","3"],"status":1}]})
        );
    }
    #[test]
    fn guest_panel_keeps_filters_but_never_a_note_target_or_svrf_filter() {
        let base = json!({"search":"x","rule_start":"0","check":null,"error_start":"0","query":null,"waived":false,"selected":null,"markers":true,"shown":true,"jump_scale":null,"zoom_lock":false,"jump_active":false,"focus_visible":false});
        let p = Panel::default();
        serde_json::from_value::<PanelData>(base.clone())
            .unwrap()
            .check(&p, 1, 1)
            .unwrap();
        let mut metric = base.clone();
        metric["metric"] = json!("WIDTH");
        assert_eq!(
            serde_json::from_value::<PanelData>(metric)
                .unwrap()
                .check(&p, 1, 1),
            Err("forbidden")
        );
        let mut note = base;
        note["jump_active"] = json!(true);
        note["focus_visible"] = json!(true);
        note["selected"] = json!({"check":"0","error":"0"});
        note["note_target"] = note["selected"].clone();
        assert_eq!(
            serde_json::from_value::<PanelData>(note)
                .unwrap()
                .check(&p, 1, 1),
            Err("forbidden")
        );
    }
}
