//! Bounded at-most-once acceptance for owner mutations. High-water survives
//! history eviction: an old sequence is never interpreted as a new command.
use serde_json::{json, Value};
use std::collections::VecDeque;

const HISTORY: usize = 32;
const SIGNATURE_BYTES: usize = 128 * 1024;
pub(crate) struct Record {
    seq: u64,
    signature: String,
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
    pub fn admit(
        &mut self,
        seq: u64,
        signature: String,
        kind: &str,
    ) -> Result<Admission, &'static str> {
        if signature.len() > SIGNATURE_BYTES {
            return Err("request_too_large");
        }
        if let Some(record) = self.history.iter().find(|r| r.seq == seq) {
            return if record.signature == signature {
                Ok(Admission::Replay(record.state.clone()))
            } else {
                Err("operation_conflict")
            };
        }
        if seq <= self.high_water {
            return Err("operation_expired");
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
        self.history.push_back(Record {
            seq,
            signature,
            state: json!({"seq":seq.to_string(),"kind":kind,"phase":"queued"}),
        });
        Ok(Admission::New)
    }
    pub fn update(&mut self, seq: u64, state: Value, terminal: bool) {
        // Internal caller emits bounded schema state, not an arbitrary native
        // JSON/log object. A completed operation cannot mutate a newer one.
        if self.active != Some(seq) {
            return;
        }
        self.history.back_mut().expect("active record").state = state;
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
