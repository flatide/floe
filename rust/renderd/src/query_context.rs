//! Query geometry identity may precede the latest label-only/cropped frame.
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SceneId {
    pub generation: u64,
    pub round: u64,
}
impl SceneId {
    pub fn parse(fields: &BTreeMap<String, String>) -> Result<Option<Self>, String> {
        match (fields.get("scene_gen"), fields.get("scene_round")) {
            (None, None) => Ok(None), // Legacy local clients remain compatible.
            (Some(g), Some(r)) => {
                let positive = |s: &str| -> Result<u64, String> {
                    let n = s
                        .parse::<u64>()
                        .map_err(|_| "invalid query scene identity")?;
                    if n == 0 || n.to_string() != s {
                        return Err("invalid query scene identity".into());
                    }
                    Ok(n)
                };
                Ok(Some(Self {
                    generation: positive(g)?,
                    round: positive(r)?,
                }))
            }
            _ => Err("query requires both scene_gen and scene_round".into()),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct QueryContext {
    pub id: Option<SceneId>,
    /// Completeness of selected geometry, not draw-only labels/frames.
    pub complete: bool,
    pub summary_layers: usize,
}
impl QueryContext {
    pub fn rejection(&self, expected: SceneId, requested_summaries: usize) -> Option<&'static str> {
        if self.id.is_none() {
            Some("scene_unavailable")
        } else if self.id != Some(expected) {
            Some("scene_mismatch")
        } else if !self.complete {
            Some("scene_incomplete")
        } else if requested_summaries != 0 {
            Some("scene_summary")
        } else {
            None
        }
    }
    pub fn wire(self) -> String {
        format!(
            "scene_gen={} scene_round={} scene_complete={} scene_summary={}",
            self.id.map_or(0, |i| i.generation),
            self.id.map_or(0, |i| i.round),
            self.complete as u8,
            self.summary_layers
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_requires_both_positive_canonical_counters() {
        assert_eq!(SceneId::parse(&BTreeMap::new()).unwrap(), None);
        for (g, r) in [
            ("0", "1"),
            ("1", "0"),
            ("01", "1"),
            ("+1", "1"),
            ("한글", "1"),
            ("1", "18446744073709551616"),
        ] {
            let fields = [
                ("scene_gen".into(), g.into()),
                ("scene_round".into(), r.into()),
            ]
            .into();
            assert!(SceneId::parse(&fields).is_err());
        }
        assert!(SceneId::parse(&[("scene_gen".into(), "1".into())].into()).is_err());
        let fields = [
            ("scene_gen".into(), u64::MAX.to_string()),
            ("scene_round".into(), "2".into()),
        ]
        .into();
        assert_eq!(
            SceneId::parse(&fields).unwrap(),
            Some(SceneId {
                generation: u64::MAX,
                round: 2
            })
        );
    }
    #[test]
    fn unavailable_mismatch_partial_and_summary_are_not_empty_hits() {
        let id = SceneId {
            generation: 8,
            round: 2,
        };
        let mut c = QueryContext::default();
        assert_eq!(c.rejection(id, 0), Some("scene_unavailable"));
        c.id = Some(SceneId {
            generation: 8,
            round: 1,
        });
        assert_eq!(c.rejection(id, 0), Some("scene_mismatch"));
        c.id = Some(id);
        assert_eq!(c.rejection(id, 0), Some("scene_incomplete"));
        c.complete = true;
        c.summary_layers = 1;
        assert_eq!(c.rejection(id, 1), Some("scene_summary"));
        assert_eq!(
            c.rejection(id, 0),
            None,
            "exact-only selection remains queryable"
        );
        c.summary_layers = 0;
        assert_eq!(c.rejection(id, 0), None);
        assert_eq!(
            c.wire(),
            "scene_gen=8 scene_round=2 scene_complete=1 scene_summary=0"
        );
    }
}
