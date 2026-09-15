//! Session-owned named fill slots; the renderer still receives resolved bits.
#[cfg(test)]
#[path = "fill_slots_tests.rs"]
mod tests;
use super::{properties::AssignedFill, Model, ViewState};
use crate::{layerprops::FillSlot, styles, Error, Result};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Debug)]
pub struct FillSlotEdit {
    pub name: String,
    pub rows: [u16; 16],
}
impl ViewState {
    /// Includes unused slots so saving a palette does not silently lose edits.
    pub fn fill_slots(&self) -> Vec<FillSlot> {
        styles::presets::bundled()
            .expect("validated bundled palette")
            .fills
            .into_iter()
            .map(|p| FillSlot {
                name: p.name.into(),
                rows: self.assignments.slot_rows(p.name),
            })
            .collect()
    }
    pub(super) fn apply_fill_slot_edit(&mut self, model: &Model, edit: FillSlotEdit) -> Result<()> {
        let name = styles::pattern_slot(&edit.name)
            .filter(|name| !matches!(*name, "solid" | "clear"))
            .ok_or_else(|| Error::input("unknown or fixed fill slot"))?;
        // The fan-out is independent of the selected rows. Include inherited
        // children, but never detach explicit (even bitmap-equal) assignments.
        let mut affected = BTreeSet::new();
        for (&pair, fill) in &self.assignments.fills {
            if fill != &AssignedFill::Slot(name) {
                continue;
            }
            affected.insert(pair);
            if let Some(children) = model.groups.get(&pair) {
                for child in children {
                    if !self.assignments.fills.contains_key(child) {
                        affected.insert(*child);
                        if affected.len() > 4096 {
                            return Err(Error::input("fill slot affects more than 4096 rows"));
                        }
                    }
                }
            }
            if affected.len() > 4096 {
                return Err(Error::input("fill slot affects more than 4096 rows"));
            }
        }
        let assignments = Arc::make_mut(&mut self.assignments);
        if styles::pattern(name) == Some(floe_worker_client::Fill::Pattern(edit.rows)) {
            assignments.slots.remove(name);
        } else {
            assignments.slots.insert(name, edit.rows);
        }
        if !affected.is_empty() {
            self.styles = Arc::new(
                self.styles
                    .iter()
                    .map(|old| {
                        if !affected.contains(&old.layer) {
                            return old.clone();
                        }
                        floe_worker_client::Style {
                            fill: floe_worker_client::Fill::Pattern(edit.rows),
                            ..old.clone()
                        }
                    })
                    .collect(),
            );
        }
        Ok(())
    }
}
