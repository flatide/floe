//! Per-view UI state only: never an autosave/waive/geometry mutation.
use super::{dto::CursorDto, Failure};
use floe_app_core::{drc::Pack, Error, Result};
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
pub(super) struct Data {
    pub search: String,
    pub rule_start: String,
    pub check: Option<String>,
    pub error_start: String,
    pub query: Option<Query>,
    pub waived: Option<bool>,
    pub selected: Option<CursorDto>,
    pub markers: bool,
    pub shown: bool,
    pub jump_scale: Option<String>,
    pub zoom_lock: bool,
}
impl Data {
    pub fn validate(&self) -> std::result::Result<(), Failure> {
        if self.search.len() > 256 {
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
    pub fn validate_pack(&self, p: &Pack) -> Result<()> {
        let number = |s: &str| super::dto::number(s).map_err(Error::input);
        let count = |s: &str| -> Result<u64> {
            let i = usize::try_from(number(s)?).map_err(|_| Error::input("DRC check index"))?;
            p.checks
                .get(i)
                .map(|c| c.count)
                .ok_or_else(|| Error::input("DRC check index"))
        };
        if number(&self.rule_start)? > p.checks.len() as u64 {
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
        if let Some(q) = &self.query {
            let end =
                number(&q.cursor.check)? == p.checks.len() as u64 && number(&q.cursor.error)? == 0;
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
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            revision: 1,
            data: None,
        }
    }
}
impl Panel {
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
    fn data() -> Data {
        serde_json::from_value(json!({"search":"","rule_start":"0","check":null,"error_start":"0","query":null,"waived":null,"selected":null,"markers":true,"shown":true,"jump_scale":null,"zoom_lock":false})).unwrap()
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
        d.selected = Some(CursorDto {
            check: "0".into(),
            error: "9007199254740993".into(),
        });
        d.validate().unwrap();
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
