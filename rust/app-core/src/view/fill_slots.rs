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
impl FillSlotEdit {
    pub fn validate(&self) -> Result<()> {
        styles::pattern_slot(&self.name)
            .filter(|name| !matches!(*name, "solid" | "clear"))
            .map(|_| ())
            .ok_or_else(|| Error::input("unknown or fixed fill slot"))
    }
}
impl ViewState {
    /// Cache hint only, never authority for edits. View id + full state CAS
    /// remain mandatory. Cost depends on at most 18 overrides, not layer count;
    /// pan, assignments and direct styles leave this fingerprint unchanged.
    pub fn fill_slots_key(&self) -> String {
        use sha1::{Digest, Sha1};
        let mut hash = Sha1::new();
        hash.update(b"floe.fill-slots.v1\0");
        for (name, rows) in &self.assignments.slots {
            hash.update(name.as_bytes());
            hash.update([0]);
            for row in rows {
                hash.update(row.to_le_bytes());
            }
        }
        format!("{:x}", hash.finalize())
    }
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
        edit.validate()?;
        let name = styles::pattern_slot(&edit.name).expect("validated fill slot");
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
