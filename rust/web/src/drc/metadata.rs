//! Owned immutable metadata snapshot for one DRC actor/revision. Browser DTOs
//! contain no recorded deck/include paths and all floating values are strings.
use floe_app_core::{drc::Database, svrf::Rules, Error, Result};
use serde_json::{json, Value};
use std::{path::Path, sync::atomic::AtomicUsize};

pub(super) struct Metadata {
    pub rules: Rules,
    summary: Value,
    types: Vec<(String, usize)>,
}
impl Metadata {
    pub fn load(path: &Path, pack: &Database, stop: &AtomicUsize) -> Result<Self> {
        let rules = Rules::load(path, stop)?;
        let catalog = rules.catalog(pack.names(), stop)?;
        let summary = json!({"matched":catalog.matched.to_string(),"checks":catalog.checks.to_string(),"type_count":catalog.types.len().to_string()});
        let types = catalog
            .types
            .into_iter()
            .map(|t| (t.metric.to_owned(), t.checks))
            .collect();
        Ok(Self {
            rules,
            summary,
            types,
        })
    }
    pub fn summary(&self) -> &Value {
        &self.summary
    }
    pub fn has_type(&self, metric: &str) -> bool {
        self.types.iter().any(|(name, _)| name == metric)
    }
    pub fn matches(&self, name: &str, metric: &str) -> bool {
        // Empty metadata deliberately offers no types, not an "other" match
        // for every check. An unmatched check in nonempty metadata is other.
        if self.types.is_empty() {
            return false;
        }
        self.rules
            .rule(name)
            .map_or(metric == "other", |r| r.metrics().contains(&metric))
    }
    pub fn types(metadata: Option<&Self>, start: usize, limit: usize) -> Result<Value> {
        let types = metadata.map_or(&[][..], |m| m.types.as_slice());
        if start > types.len() {
            return Err(Error::input("DRC type cursor"));
        }
        let end = start.saturating_add(limit).min(types.len());
        Ok(
            json!({"available":metadata.is_some(),"rows":types[start..end].iter().map(|(metric,count)|json!({"metric":metric,"checks":count.to_string()})).collect::<Vec<_>>(),
            "next":(end<types.len()).then(||end.to_string()),"total":types.len().to_string()}),
        )
    }
    pub fn detail(&self, name: &str, stop: &AtomicUsize) -> Result<Value> {
        let Some(d) = self.rules.detail(name, stop)? else {
            return Ok(Value::Null);
        };
        let r = d.rule;
        Ok(
            json!({"rule":{"desc":r.desc,"constraints":r.constraints.iter().map(|c|json!({"metric":c.metric,"op":c.op,"value":c.value.map(|v|v.to_string()),"text":c.text,"raw":c.raw})).collect::<Vec<_>>(),
            "layers":r.layers,"source_gds":r.source_gds.iter().map(|(l,dt)|(l.to_string(),dt.map(|d|d.to_string()))).collect::<Vec<_>>(),"unresolved":r.unresolved},
            "metrics":d.metrics,"derivations":d.derivations,"derivations_more":d.derivations_more}),
        )
    }
}
