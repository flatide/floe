//! Loaded-level changes bind the current server camera, not a browser bbox.
use super::*;
use floe_app_core::view::{Phase, Viewport};

#[derive(Clone)]
pub(super) struct Camera {
    pub viewport: Viewport,
    pub dbu: f64,
}

pub(super) fn prepare(
    service: &Service,
    view_id: String,
    base_state_rev: String,
    levels: LevelSelection,
) -> std::result::Result<OpenCommand, &'static str> {
    index_open::identity(&view_id)?;
    let rev = view::counter(&base_state_rev)?;
    let previous = service
        .inner
        .state
        .lock()
        .unwrap()
        .view
        .as_ref()
        .filter(|v| v.id == view_id)
        .cloned()
        .ok_or("view_unavailable")?;
    let snapshot = previous.controller.snapshot();
    if snapshot.state_rev != rev || !matches!(snapshot.phase, Phase::Idle | Phase::Rendering) {
        return Err("busy");
    }
    if !previous.controller.model.deck {
        return Err("invalid_request");
    }
    let source = service
        .source(&previous.source_id)
        .ok_or("source_unavailable")?;
    let levels = levels.core().map_err(|_| "invalid_request")?;
    source
        .validate_levels(levels.as_ref())
        .map_err(|_| "invalid_request")?;
    let viewport = snapshot.state.viewport;
    Ok(OpenCommand {
        source,
        source_id: previous.source_id.clone(),
        levels,
        mode: Mode::parse(previous.mode).map_err(|_| "invalid_request")?,
        patch: Box::new(Patch {
            pixels: Some((viewport.width, viewport.height)),
            ..Default::default()
        }),
        replace: Some((view_id, rev)),
        display_policy: OpenDisplay::Window,
        label_preference: None,
        reselect: Some(Camera {
            viewport,
            dbu: previous.controller.model.dbu,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_wire_does_not_admit_camera_source_mode_or_write_authority() {
        let request = json!({"kind":"reselect_levels","seq":"1","view_id":"a".repeat(64),
            "base_state_rev":"1","levels":{"mode":"only","ids":["1","2"]}});
        assert!(serde_json::from_value::<OperationDto>(request.clone()).is_ok());
        for key in ["view_id", "base_state_rev", "levels"] {
            let mut missing = request.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<OperationDto>(missing).is_err());
            let mut null = request.clone();
            null[key] = Value::Null;
            assert!(serde_json::from_value::<OperationDto>(null).is_err());
        }
        for key in [
            "body",
            "source_id",
            "mode",
            "pixels",
            "bbox",
            "approved",
            "options",
            "force",
        ] {
            let mut extra = request.clone();
            extra[key] = json!({});
            assert!(
                serde_json::from_value::<OperationDto>(extra).is_err(),
                "{key}"
            );
        }
    }
}
