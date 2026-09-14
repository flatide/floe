//! Per-view UI state only: never an autosave/waive/geometry mutation.
use super::{dto::CursorDto, Failure};
use floe_app_core::{drc::Database, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Query {
    pub bbox_um: [String; 4],
    pub state_rev: String,
    pub cursor: CursorDto,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct CdState {
    pub target: CursorDto,
    pub remaining: u8,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Data {
    pub search: String,
    pub metric: Option<String>,
    pub rule_start: String,
    pub check: Option<String>,
    pub error_start: String,
    pub query: Option<Query>,
    #[serde(default)]
    pub in_view: bool,
    #[serde(default)]
    pub selected_only: bool,
    pub waived: Option<bool>,
    pub selected: Option<CursorDto>,
    pub markers: bool,
    pub shown: bool,
    pub jump_scale: Option<String>,
    pub zoom_lock: bool,
    pub jump_active: bool,
    pub focus_visible: bool,
    pub cd: Option<CdState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_target: Option<CursorDto>,
}
impl Data {
    pub fn validate(&self) -> std::result::Result<(), Failure> {
        if self.query.is_some() && (self.in_view || self.selected_only) {
            return Err("invalid_drc_request");
        }
        if self.jump_active && !self.focus_visible
            || (self.jump_active || self.focus_visible) && self.selected.is_none()
        {
            return Err("invalid_drc_request");
        }
        if self.search.len() > 256 {
            return Err("invalid_drc_request");
        }
        if self
            .metric
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 64)
        {
            return Err("invalid_drc_request");
        }
        for s in [&self.rule_start, &self.error_start]
            .into_iter()
            .chain(self.check.iter())
        {
            super::dto::number(s)?;
        }
        if let Some(c) = &self.selected {
            super::dto::number(&c.check)?;
            super::dto::number(&c.error)?;
        }
        if let Some(cd) = &self.cd {
            if !self.jump_active || cd.remaining > 3 {
                return Err("invalid_drc_request");
            }
            super::dto::number(&cd.target.check)?;
            super::dto::number(&cd.target.error)?;
        }
        if let Some(c) = &self.note_target {
            if !self.jump_active {
                return Err("invalid_drc_request");
            }
            super::dto::number(&c.check)?;
            super::dto::number(&c.error)?;
        }
        if let Some(q) = &self.query {
            crate::view::counter(&q.state_rev).map_err(|_| "invalid_drc_request")?;
            super::dto::number(&q.cursor.check)?;
            super::dto::number(&q.cursor.error)?;
            let mut b = [0.; 4];
            for (v, s) in b.iter_mut().zip(&q.bbox_um) {
                if s.len() > 80 {
                    return Err("invalid_drc_request");
                }
                *v = s.parse().map_err(|_| "invalid_drc_request")?;
                if !f64::is_finite(*v) {
                    return Err("invalid_drc_request");
                }
            }
            if b[0] > b[2] || b[1] > b[3] {
                return Err("invalid_drc_request");
            }
        }
        if let Some(s) = &self.jump_scale {
            if s.len() > 80 {
                return Err("invalid_drc_request");
            }
            let n = s.parse::<f64>().map_err(|_| "invalid_drc_request")?;
            if !n.is_finite() || n <= 0. {
                return Err("invalid_drc_request");
            }
        }
        Ok(())
    }
    pub fn validate_pack(&self, p: &Database) -> Result<()> {
        let number = |s: &str| super::dto::number(s).map_err(Error::input);
        let count = |s: &str| -> Result<u64> {
            let i = usize::try_from(number(s)?).map_err(|_| Error::input("DRC check index"))?;
            Ok(p.check(i)?.count)
        };
        if number(&self.rule_start)? > p.check_count() as u64 {
            return Err(Error::input("rule cursor"));
        }
        if number(&self.error_start)?
            > self
                .check
                .as_ref()
                .map(|c| count(c))
                .transpose()?
                .unwrap_or(0)
        {
            return Err(Error::input("error cursor"));
        }
        if let Some(c) = &self.selected {
            if number(&c.error)? >= count(&c.check)? {
                return Err(Error::input("selected error index"));
            }
        }
        if let Some(cd) = &self.cd {
            if number(&cd.target.error)? >= count(&cd.target.check)? {
                return Err(Error::input("CD target error index"));
            }
        }
        if let Some(c) = &self.note_target {
            if number(&c.error)? >= count(&c.check)? {
                return Err(Error::input("note target error index"));
            }
        }
        if let Some(q) = &self.query {
            let end =
                number(&q.cursor.check)? == p.check_count() as u64 && number(&q.cursor.error)? == 0;
            if !end && number(&q.cursor.error)? > count(&q.cursor.check)? {
                return Err(Error::input("query cursor"));
            }
        }
        Ok(())
    }
}
pub(crate) struct Panel {
    revision: u64,
    data: Option<Data>,
    read_revision: Option<String>,
    pub(super) groups: super::selection::Groups,
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            revision: 1,
            data: None,
            read_revision: None,
            groups: super::selection::Groups::default(),
        }
    }
}
impl Panel {
    pub(super) fn bind_revision(&mut self, revision: &str) {
        if self.read_revision.as_deref() != Some(revision) {
            // Filtered cursors and selections were derived under old statuses.
            // Reset lazily under the read revision fence, never from stale GETs.
            *self = Self {
                read_revision: Some(revision.into()),
                ..Self::default()
            };
        }
    }
    pub fn snapshot(&self) -> Value {
        json!({"panel_rev":self.revision.to_string(),"body":self.data})
    }
    pub(super) fn check(&self, base: u64, data: &Data) -> std::result::Result<(), Failure> {
        // An uncertain retry of the same state is harmless; an old *different*
        // selection must never overwrite a newer tab's state.
        if base == self.revision || base < self.revision && self.data.as_ref() == Some(data) {
            Ok(())
        } else {
            Err("drc_panel_conflict")
        }
    }
    pub(super) fn set(&mut self, base: u64, data: Data) -> std::result::Result<Value, Failure> {
        self.check(base, &data)?;
        if self.data.as_ref() != Some(&data) {
            self.revision = self.revision.checked_add(1).ok_or("drc_panel_limit")?;
            self.data = Some(data);
        }
        Ok(self.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_read_revision_discards_saved_filter_cursors_but_idle_poll_keeps_them() {
        let mut p = Panel::default();
        p.bind_revision("old");
        p.set(1, data()).unwrap();
        let before = p.snapshot();
        p.bind_revision("old");
        assert_eq!(p.snapshot(), before);
        p.bind_revision("new");
        assert_eq!(p.snapshot()["body"], Value::Null);
        assert_eq!(p.snapshot()["panel_rev"], "1");
    }
    fn data() -> Data {
        serde_json::from_value(json!({"search":"","rule_start":"0","check":null,"error_start":"0","query":null,"waived":null,"selected":null,"markers":true,"shown":true,"jump_scale":null,"zoom_lock":false,"jump_active":false,"focus_visible":false})).unwrap()
    }
    #[test]
    fn saved_note_target_is_independent_but_requires_a_live_jump() {
        let mut d = data();
        assert!(d.note_target.is_none());
        assert!(serde_json::to_value(&d)
            .unwrap()
            .get("note_target")
            .is_none());
        d.note_target = Some(CursorDto {
            check: "1".into(),
            error: "9007199254740994".into(),
        });
        assert!(d.validate().is_err());
        d.jump_active = true;
        d.focus_visible = true;
        d.selected = Some(CursorDto {
            check: "0".into(),
            error: "0".into(),
        });
        d.validate().unwrap();
        let encoded = serde_json::to_value(&d).unwrap();
        assert_eq!(serde_json::from_value::<Data>(encoded).unwrap(), d);
        d.note_target.as_mut().unwrap().error = "01".into();
        assert!(d.validate().is_err());
    }
    #[test]
    fn revision_retries_and_conflicting_old_tab_are_not_last_writer_wins() {
        let mut p = Panel::default();
        let a = data();
        assert_eq!(p.set(1, a.clone()).unwrap()["panel_rev"], "2");
        assert_eq!(p.set(1, a.clone()).unwrap()["panel_rev"], "2");
        let mut b = a.clone();
        b.markers = false;
        assert_eq!(p.set(1, b.clone()).unwrap_err(), "drc_panel_conflict");
        assert_eq!(p.set(2, b).unwrap()["panel_rev"], "3");
        assert_eq!(p.set(1, a).unwrap_err(), "drc_panel_conflict");
        p.revision = u64::MAX;
        assert_eq!(p.set(u64::MAX, data()).unwrap_err(), "drc_panel_limit");
        assert_eq!(p.snapshot()["body"]["markers"], false);
    }
    #[test]
    fn bounded_typed_state_keeps_u64_and_rejects_nonfinite_coordinates() {
        let mut d = data();
        assert!(!d.in_view && !d.selected_only);
        assert_eq!(d.metric, None);
        for bad in [String::new(), "x".repeat(65), "한".repeat(22)] {
            d.metric = Some(bad);
            assert!(d.validate().is_err());
        }
        d.metric = Some("width".into());
        d.validate().unwrap();
        d.metric = None;
        d.in_view = true;
        d.selected_only = true;
        d.validate().unwrap();
        d.query = Some(Query {
            bbox_um: ["0".into(), "0".into(), "1".into(), "1".into()],
            state_rev: "1".into(),
            cursor: CursorDto {
                check: "0".into(),
                error: "0".into(),
            },
        });
        assert!(d.validate().is_err());
        d.query = None;
        d.jump_active = true;
        assert!(d.validate().is_err());
        d.focus_visible = true;
        assert!(d.validate().is_err());
        d.selected = Some(CursorDto {
            check: "0".into(),
            error: "9007199254740993".into(),
        });
        d.validate().unwrap();
        d.cd = Some(CdState {
            target: CursorDto {
                check: "1".into(),
                error: "9007199254740994".into(),
            },
            remaining: 2,
        });
        d.validate().unwrap();
        d.cd.as_mut().unwrap().remaining = 4;
        assert!(d.validate().is_err());
        d.cd.as_mut().unwrap().remaining = 0;
        d.validate().unwrap();
        d.jump_active = false;
        assert!(d.validate().is_err());
        d.jump_active = true;
        d.cd.as_mut().unwrap().target.error = "01".into();
        assert!(d.validate().is_err());
        d.cd = None;
        d.rule_start = "00".into();
        assert!(d.validate().is_err());
        d.rule_start = "0".into();
        d.jump_scale = Some("NaN".into());
        assert!(d.validate().is_err());
        d.jump_scale = None;
        d.search = "한".repeat(86);
        assert!(d.validate().is_err());
    }
    #[test]
    fn concurrent_same_base_commits_only_one_distinct_state() {
        use std::sync::{Arc, Barrier, Mutex};
        let state = Arc::new(Mutex::new(Panel::default()));
        let barrier = Arc::new(Barrier::new(2));
        let threads = [false, true].map(|shown| {
            let state = Arc::clone(&state);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut d = data();
                d.shown = shown;
                barrier.wait();
                state.lock().unwrap().set(1, d)
            })
        });
        let results = threads.map(|t| t.join().unwrap());
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|r| matches!(r, Err("drc_panel_conflict")))
                .count(),
            1
        );
        assert_eq!(state.lock().unwrap().snapshot()["panel_rev"], "2");
    }
}
