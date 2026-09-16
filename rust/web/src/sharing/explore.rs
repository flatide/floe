//! Only presentation/navigation of this grant's own view. No owner drafts,
//! settings, source selection, query receipts or filesystem authority.
use super::Scope;
use crate::view::{DetailDto, Field, Nav, PatchDto, Selection, ThinDto};
use floe_app_core::view::{Patch, ViewController};
use floe_worker_client::Layers;
use serde::Deserialize;
use std::{collections::BTreeSet, sync::Arc};

pub(super) struct Explorer {
    pub id: String,
    pub controller: Arc<ViewController>,
}
pub(super) fn layers_within(scope: &Layers, requested: &Layers) -> bool {
    match (scope, requested) {
        (Layers::All, _) | (_, Layers::None) => true,
        (_, Layers::All) => false,
        (Layers::None, Layers::Only(pairs)) => pairs.is_empty(),
        (Layers::Only(allowed), Layers::Only(pairs)) => {
            let allowed: BTreeSet<_> = allowed.iter().copied().collect();
            pairs.iter().all(|p| allowed.contains(p))
        }
    }
}
pub(super) fn scoped_layers(
    controller: &ViewController,
    scope: &Scope,
    layers: &Layers,
) -> Result<Layers, &'static str> {
    // Resolve group aliases BEFORE checking authority. The public All alias
    // means all GRANTED planes for display edits, not all dataset planes.
    let selected = if *layers == Layers::All {
        scope.layers.clone()
    } else {
        controller
            .snapshot()
            .state
            .edit(
                &controller.model,
                Patch {
                    layers: Some(layers.clone()),
                    ..Default::default()
                },
            )
            .map_err(|_| {
                if scope.layers == Layers::All {
                    "invalid_request"
                } else {
                    "forbidden"
                }
            })?
            .layers
    };
    if !layers_within(&scope.layers, &selected) {
        return Err("forbidden");
    }
    Ok(selected)
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct DisplayPatch {
    navigation: Field<Nav>,
    pixels: Field<(u32, u32)>,
    depth: Field<String>,
    depth_step: Field<i8>,
    detail: Field<DetailDto>,
    thin: Field<ThinDto>,
    layers: Field<Selection>,
    frames: Field<bool>,
    labels: Field<bool>,
    font_px: Field<u32>,
    mono: Field<bool>,
}
impl DisplayPatch {
    pub fn core(self, controller: &ViewController, scope: &Scope) -> Result<Patch, &'static str> {
        let mut patch = PatchDto {
            navigation: self.navigation,
            pixels: self.pixels,
            depth: self.depth,
            depth_step: self.depth_step,
            detail: self.detail,
            thin: self.thin,
            layers: self.layers,
            frames: self.frames,
            labels: self.labels,
            font_px: self.font_px,
            mono: self.mono,
            ..Default::default()
        }
        .core()?;
        if let Some(layers) = &patch.layers {
            patch.layers = Some(scoped_layers(controller, scope, layers)?);
        }
        Ok(patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn layer_scope_never_widens_and_display_patch_has_no_owner_edits() {
        let scope = Layers::Only(vec![(7, 0), (8, 0)]);
        assert!(layers_within(&scope, &Layers::Only(vec![(7, 0)])));
        assert!(layers_within(&scope, &Layers::None));
        assert!(!layers_within(&scope, &Layers::All));
        assert!(!layers_within(&scope, &Layers::Only(vec![(9, 0)])));
        assert!(!layers_within(&Layers::None, &scope));
        for key in [
            "styles",
            "style_batch",
            "layer_change",
            "restore_layers",
            "settings",
            "out",
            "source_id",
            "prepared_layers",
            "fill_slot_edit",
        ] {
            assert!(serde_json::from_value::<DisplayPatch>(json!({key:{}})).is_err());
        }
        assert!(serde_json::from_value::<DisplayPatch>(json!({"layers":null})).is_err());
        assert!(serde_json::from_value::<DisplayPatch>(
            json!({"navigation":{"kind":"pan","x":0.1,"y":0.0,"snap":true}})
        )
        .is_ok());
    }
}
