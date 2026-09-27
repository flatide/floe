//! Explicit immutable build/check/use operations. Discovery never writes and
//! only the separate Use operation may replace the owner's current view.
use super::*;
use floe_app_core::cache::revision::reclaim::{self, Prepared};

pub(super) struct Preview {
    source_id: String,
    pub prepared: Prepared,
}
impl Preview {
    pub fn matches(&self, source: &str, revision: &str, token: &str) -> bool {
        self.source_id == source
            && self.prepared.summary().revision == revision
            && self.prepared.summary().token == token
            && !self.prepared.summary().complete
    }
}
pub(super) fn token(value: &str) -> std::result::Result<(), &'static str> {
    if value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err("invalid_request")
    }
}
pub(super) fn prepare(
    inner: &Inner,
    seq: u64,
    source: Arc<RegisteredSource>,
    source_id: String,
    revision: String,
    stop: &AtomicUsize,
) -> Result<Value> {
    // File/lock work is off the HTTP thread and outside the State mutex.
    let prepared = reclaim::prepare(source, &revision, stop)?;
    let value = json!({"seq":seq.to_string(),"kind":"prepare_reclaim","phase":"succeeded",
        "source_id":source_id,"preview":prepared.summary()});
    let mut state = inner.state.lock().unwrap();
    if state.closed {
        return Err(Error::new(ErrorKind::Cancelled, "service closed"));
    }
    floe_app_core::check_cancelled(stop)?;
    if !prepared.summary().complete {
        state.reclaim = Some(Preview {
            source_id,
            prepared,
        });
    }
    Ok(value)
}

pub(super) fn identity(id: &str) -> std::result::Result<(), &'static str> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid_request");
    }
    Ok(())
}
pub(super) fn levels(value: &Option<BTreeSet<i64>>) -> Value {
    match value {
        None => json!({"mode":"all"}),
        Some(ids) => {
            json!({"mode":"only","ids":ids.iter().map(i64::to_string).collect::<Vec<_>>()})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reclamation_schema_requires_separate_consent_and_binds_opaque_preview() {
        let v = json!({"kind":"reclaim_revision","seq":"1","source_id":"a".repeat(64),
            "revision":"b".repeat(32),"token":"c".repeat(64),"approved":true});
        assert!(serde_json::from_value::<OperationDto>(v.clone()).is_ok());
        for key in ["source_id", "revision", "token", "approved"] {
            let mut missing = v.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<OperationDto>(missing).is_err());
        }
        for key in ["path", "force", "files", "levels"] {
            let mut extra = v.clone();
            extra[key] = json!("/outside");
            assert!(serde_json::from_value::<OperationDto>(extra).is_err());
        }
        for value in ["", "../journal", &"C".repeat(64), &"c".repeat(63)] {
            assert!(token(value).is_err());
        }
        assert!(token(&"c".repeat(64)).is_ok());
        let mut preview = json!({"kind":"prepare_reclaim","seq":"1","source_id":"a".repeat(64),"revision":"b".repeat(32)});
        assert!(serde_json::from_value::<OperationDto>(preview.clone()).is_ok());
        preview["approved"] = json!(true);
        assert!(serde_json::from_value::<OperationDto>(preview).is_err());
    }
    #[test]
    fn revision_requests_require_explicit_authority_and_opaque_ids() {
        let use_it = json!({"kind":"use_revision","seq":"1","source_id":"a".repeat(64),
            "levels":{"mode":"all"},"mode":"level","revision":"b".repeat(32),
            "approved":true,"target":{"kind":"empty"},"pixels":[100,100]});
        assert!(serde_json::from_value::<OperationDto>(use_it.clone()).is_ok());
        for key in ["revision", "approved", "target", "levels", "pixels"] {
            let mut v = use_it.clone();
            v.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<OperationDto>(v).is_err());
        }
        for s in ["../current", "", "한글", &"A".repeat(32), &"a".repeat(33)] {
            assert!(identity(s).is_err());
        }
        assert!(identity(&"b".repeat(32)).is_ok());
        let mut v = use_it;
        v["path"] = json!("/tmp/cache");
        assert!(serde_json::from_value::<OperationDto>(v).is_err());
        let usage = json!({"kind":"revision_usage","seq":"1","source_id":"a".repeat(64)});
        assert!(serde_json::from_value::<OperationDto>(usage.clone()).is_ok());
        for key in ["path", "revision", "delete", "approved", "levels"] {
            let mut bad = usage.clone();
            bad[key] = json!(true);
            assert!(serde_json::from_value::<OperationDto>(bad).is_err());
        }
        let v = json!({"kind":"check_revision","seq":"1","source_id":"a".repeat(64),"levels":{"mode":"all"},"force":true});
        assert!(serde_json::from_value::<OperationDto>(v).is_err());
    }
}
