//! Bounded at-most-once acceptance for owner mutations. High-water survives
//! history eviction: an old sequence is never interpreted as a new command.
use serde_json::{json, Value};
use std::collections::VecDeque;

const HISTORY: usize = 32;
const SIGNATURE_BYTES: usize = 128 * 1024;
pub(crate) struct Record {
    seq: u64,
    signature: String,
    scope_id: Option<String>,
    pub state: Value,
}
#[derive(Default)]
pub(crate) struct Ledger {
    high_water: u64,
    active: Option<u64>,
    history: VecDeque<Record>,
}
pub(crate) enum Admission {
    New,
    Replay(Value),
}
impl Ledger {
    /// Check retries before validating a mutable registration. A successful
    /// build changes that registration, but an uncertain retry is still a replay.
    pub fn replay(&self, seq: u64, signature: &str) -> Result<Option<Value>, &'static str> {
        if let Some(record) = self.history.iter().find(|r| r.seq == seq) {
            return if record.signature == signature {
                Ok(Some(record.state.clone()))
            } else {
                Err("operation_conflict")
            };
        }
        if seq <= self.high_water {
            return Err("operation_expired");
        }
        Ok(None)
    }
    pub fn admit(
        &mut self,
        seq: u64,
        signature: String,
        kind: &str,
    ) -> Result<Admission, &'static str> {
        self.admit_scoped(seq, signature, kind, None)
    }
    /// The coordinator supplies this opaque registration epoch, not the client.
    /// It belongs to the receipt forever, including after a reviewer reconnect.
    pub fn admit_scoped(
        &mut self,
        seq: u64,
        signature: String,
        kind: &str,
        scope_id: Option<&str>,
    ) -> Result<Admission, &'static str> {
        if scope_id.is_some_and(|v| {
            v.len() != 64
                || !v
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            return Err("invalid_request");
        }
        if signature.len() > SIGNATURE_BYTES {
            return Err("request_too_large");
        }
        if let Some(state) = self.replay(seq, &signature)? {
            return Ok(Admission::Replay(state));
        }
        if Some(seq) != self.high_water.checked_add(1) {
            return Err("operation_sequence");
        }
        if self.active.is_some() {
            return Err("busy");
        }
        self.high_water = seq;
        self.active = Some(seq);
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        let mut state = json!({"seq":seq.to_string(),"kind":kind,"phase":"queued"});
        if let Some(id) = scope_id {
            state["scope_id"] = json!(id);
        }
        self.history.push_back(Record {
            seq,
            signature,
            scope_id: scope_id.map(str::to_owned),
            state,
        });
        Ok(Admission::New)
    }
    pub fn update(&mut self, seq: u64, mut state: Value, terminal: bool) {
        // Internal caller emits bounded schema state, not an arbitrary native
        // JSON/log object. A completed operation cannot mutate a newer one.
        if self.active != Some(seq) {
            return;
        }
        let record = self.history.back_mut().expect("active record");
        let Some(fields) = state.as_object_mut() else {
            return;
        };
        fields.remove("scope_id");
        if let Some(id) = &record.scope_id {
            fields.insert("scope_id".into(), json!(id));
        }
        record.state = state;
        if terminal {
            self.active = None;
        }
    }
    pub fn get(&self, seq: u64) -> Option<Value> {
        self.history
            .iter()
            .find(|r| r.seq == seq)
            .map(|r| r.state.clone())
    }
    pub fn active(&self) -> Option<u64> {
        self.active
    }
    pub fn snapshot(&self) -> Value {
        json!({"last_seq":self.high_water.to_string(),"active":self.active.map(|n|n.to_string()),"history":self.history.iter().map(|r|r.state.clone()).collect::<Vec<_>>()})
    }
    pub fn cursor(&self) -> Value {
        json!({"last_seq":self.high_water.to_string(),"active":self.active.map(|n|n.to_string())})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_scope_cannot_be_rewritten_by_progress_or_a_new_registration() {
        let mut ledger = Ledger::default();
        let old = "a".repeat(64);
        let new = "b".repeat(64);
        ledger
            .admit_scoped(1, "approved".into(), "review", Some(&old))
            .unwrap();
        assert_eq!(ledger.get(1).unwrap()["scope_id"], old);
        ledger.update(
            1,
            json!({"seq":"1","phase":"succeeded","scope_id":new}),
            true,
        );
        let saved = ledger.get(1).unwrap();
        assert_eq!(saved["scope_id"], old);
        match ledger
            .admit_scoped(1, "approved".into(), "review", Some(&new))
            .unwrap()
        {
            Admission::Replay(value) => assert_eq!(value, saved),
            Admission::New => panic!("repeated approval became a new operation"),
        }
        ledger
            .admit_scoped(2, "new approval".into(), "review", Some(&new))
            .unwrap();
        ledger.update(1, json!({"phase":"failed"}), true);
        assert_eq!(ledger.get(1).unwrap(), saved);
        assert_eq!(ledger.get(2).unwrap()["scope_id"], new);
    }
    #[test]
    fn duplicates_conflicts_busy_and_eviction_never_reexecute() {
        let mut l = Ledger::default();
        assert!(matches!(
            l.admit(1, "index force".into(), "index"),
            Ok(Admission::New)
        ));
        assert!(matches!(
            l.admit(1, "index force".into(), "index"),
            Ok(Admission::Replay(_))
        ));
        assert!(matches!(
            l.admit(1, "index keep".into(), "index"),
            Err("operation_conflict")
        ));
        assert!(matches!(l.admit(2, "open".into(), "open"), Err("busy")));
        l.update(1, json!({"seq":"1","phase":"succeeded"}), true);
        assert_eq!(l.active(), None);
        assert!(matches!(
            l.admit(3, "open".into(), "open"),
            Err("operation_sequence")
        ));
        for n in 2..=40 {
            assert!(matches!(
                l.admit(n, "open".into(), "open"),
                Ok(Admission::New)
            ));
            l.update(n, json!({"seq":n.to_string(),"phase":"succeeded"}), true);
        }
        assert!(l.get(1).is_none());
        assert_eq!(l.snapshot()["history"].as_array().unwrap().len(), HISTORY);
        assert!(matches!(
            l.admit(1, "index force".into(), "index"),
            Err("operation_expired")
        ));
        let before = l.snapshot();
        l.update(1, json!("late failure"), true);
        assert_eq!(before, l.snapshot());
        l.high_water = u64::MAX;
        assert!(matches!(
            l.admit(u64::MAX, "open".into(), "open"),
            Err("operation_expired")
        ));
        assert!(matches!(
            l.admit(0, "open".into(), "open"),
            Err("operation_expired")
        ));
    }
}
