//! Explicit immutable build/check/use operations. Discovery never writes and
//! only the separate Use operation may replace the owner's current view.
use super::*;

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
