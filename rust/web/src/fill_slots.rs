//! Session palette metadata and the memory-only developer edit boundary.
//! No filesystem paths, renderer commands or shared-default publications.
use crate::transport::{self, Gate};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use floe_app_core::view::{FillSlotEdit, Patch, ViewState};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Edit {
    name: String,
    rows: [u16; 16],
}
impl Edit {
    pub(crate) fn patch(self, enabled: bool) -> Result<Patch, &'static str> {
        if !enabled {
            return Err("fill_edit_disabled");
        }
        let edit = FillSlotEdit {
            name: self.name,
            rows: self.rows,
        };
        edit.validate().map_err(|_| "invalid_request")?;
        Ok(Patch {
            fill_slot_edit: Some(edit),
            ..Default::default()
        })
    }
}
fn data(state: &ViewState, id: &str, enabled: bool) -> Value {
    // Compiled defaults remain on the immutable presets endpoint. Only the
    // live table is returned here, including unused slots for future assignment.
    json!({"version":1,"view_id":id,"fill_slots_key":state.fill_slots_key(),
        "editable":enabled,"fills":state.fill_slots()})
}
pub(crate) async fn read(
    State(g): State<Gate>,
    Path((id, key)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = transport::http_session(&g, &headers) {
        return transport::error(e);
    }
    if key.len() != 40
        || !key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return transport::error(StatusCode::BAD_REQUEST);
    }
    let Some(view) = g.active_view().filter(|v| v.id == id) else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let state = view.controller.snapshot();
    if state.state.fill_slots_key() != key {
        return transport::error(StatusCode::CONFLICT);
    }
    Json(data(&state.state, &id, g.fill_slot_edit)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edit_requires_launcher_opt_in_and_exact_schema() {
        let body = json!({"name":"brick","rows":vec![3;16]});
        assert!(matches!(
            serde_json::from_value::<Edit>(body.clone())
                .unwrap()
                .patch(false),
            Err("fill_edit_disabled")
        ));
        let patch = serde_json::from_value::<Edit>(body)
            .unwrap()
            .patch(true)
            .unwrap();
        assert_eq!(patch.fill_slot_edit.unwrap().rows, [3; 16]);
        for name in ["solid", "clear", "SOLID", "unknown", "한글", "../brick"] {
            assert!(
                serde_json::from_value::<Edit>(json!({"name":name,"rows":vec![0;16]}))
                    .unwrap()
                    .patch(true)
                    .is_err()
            );
        }
        for body in [
            json!({"name":"brick","rows":[1]}),
            json!({"name":"brick","rows":vec![65536;16]}),
            json!({"name":"brick","rows":vec![-1;16]}),
            json!({"name":"brick","rows":vec![0;16],"path":"out"}),
            json!({"name":"brick","rows":null}),
        ] {
            assert!(serde_json::from_value::<Edit>(body).is_err());
        }
        assert!(serde_json::from_str::<Edit>(
            r#"{"name":"brick","name":"solid","rows":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}"#
        )
        .is_err());
    }
}
