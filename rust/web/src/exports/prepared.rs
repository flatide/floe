use crate::query::Anchor;
use floe_app_core::view::QueryAnchor;
use floe_worker_client::{ClipRequest, Layers};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Bounds {
    Viewport {},
    Dbu { bbox: [String; 4] },
}

#[cfg(test)]
mod tests {
    use super::*;
    fn anchor() -> QueryAnchor {
        QueryAnchor {
            dataset_revision: 1,
            worker_epoch: 2,
            frame_id: 3,
            state_rev: 4,
            render_rev: 5,
            render_key: 6,
        }
    }
    fn request() -> ClipRequest {
        ClipRequest {
            bbox: [0, 0, 10, 20],
            layers: Layers::All,
            jobs: 1,
            cell_name: "CLIP".into(),
        }
    }
    #[test]
    fn drafts_are_single_use_expiring_and_connection_scoped() {
        let mut d = Drafts::default();
        let a = d.prepare(anchor(), request(), "a").unwrap();
        let b = d.prepare(anchor(), request(), "b").unwrap();
        assert!(d.check(a["token"].as_str().unwrap()).is_err());
        let token = b["token"].as_str().unwrap();
        d.revoke("a");
        assert!(d.check(token).is_ok());
        assert!(Drafts::default().check(token).is_err());
        d.ready.as_mut().unwrap().expires = Instant::now();
        assert!(d.check(token).is_err());
        let c = d.prepare(anchor(), request(), "c").unwrap();
        d.consume();
        assert!(d.check(c["token"].as_str().unwrap()).is_err());
        let c = d.prepare(anchor(), request(), "c").unwrap();
        d.revoke("c");
        assert!(d.check(c["token"].as_str().unwrap()).is_err());
    }
    #[test]
    fn input_is_canonical_and_never_accepts_paths_or_display_cut_options() {
        let doc = json!({"anchor":{"dataset_revision":"1","worker_epoch":"2","frame_id":"3","state_rev":"4","render_rev":"5","render_key":"6"},"bounds":{"kind":"dbu","bbox":["-9223372036854775808","0","9223372036854775807","1"]},"layers":"none","jobs":4,"cell_name":"字"});
        let parsed = serde_json::from_value::<PrepareDto>(doc.clone())
            .unwrap()
            .core()
            .unwrap();
        assert_eq!(parsed.request.bbox, [i64::MIN, 0, i64::MAX, 1]);
        assert!(matches!(parsed.request.layers, Layers::None));
        for (k, v) in [
            ("out", json!("/tmp/x.oas")),
            ("cut", json!(1)),
            ("jobs", json!(0)),
            ("layers", json!("unknown")),
            ("cell_name", json!("bad\nname")),
        ] {
            let mut bad = doc.clone();
            bad[k] = v;
            assert!(serde_json::from_value::<PrepareDto>(bad)
                .map_err(|_| "parse")
                .and_then(PrepareDto::core)
                .is_err());
        }
        for v in [
            json!(1),
            json!("01"),
            json!("1.0"),
            json!("9223372036854775808"),
        ] {
            let mut bad = doc.clone();
            bad["bounds"]["bbox"][0] = v;
            assert!(serde_json::from_value::<PrepareDto>(bad)
                .map_err(|_| "parse")
                .and_then(PrepareDto::core)
                .is_err());
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrepareDto {
    pub anchor: Anchor,
    pub bounds: Bounds,
    pub layers: String,
    pub jobs: u16,
    pub cell_name: String,
}
impl PrepareDto {
    pub fn core(self) -> Result<PrepareInput, &'static str> {
        if !(1..=16).contains(&self.jobs) {
            return Err("invalid_request");
        }
        let bbox = match self.bounds {
            Bounds::Viewport {} => None,
            Bounds::Dbu { bbox } => {
                let mut out = [0; 4];
                for (n, s) in out.iter_mut().zip(bbox) {
                    *n = s.parse().map_err(|_| "invalid_request")?;
                    if n.to_string() != s {
                        return Err("invalid_request");
                    }
                }
                Some(out)
            }
        };
        let (visible, layers) = match self.layers.as_str() {
            "visible" => (true, Layers::All),
            "all" => (false, Layers::All),
            "none" => (false, Layers::None),
            _ => return Err("invalid_request"),
        };
        let request = ClipRequest {
            bbox: bbox.unwrap_or([0, 0, 1, 1]),
            layers,
            jobs: self.jobs,
            cell_name: self.cell_name,
        };
        request.validate().map_err(|_| "invalid_request")?;
        Ok(PrepareInput {
            anchor: self.anchor.core()?,
            bbox,
            visible,
            request,
        })
    }
}
pub(crate) struct PrepareInput {
    pub anchor: QueryAnchor,
    pub bbox: Option<[i64; 4]>,
    pub visible: bool,
    pub request: ClipRequest,
}
pub(crate) struct Draft {
    pub anchor: QueryAnchor,
    pub request: ClipRequest,
    token: String,
    connection: String,
    expires: Instant,
}
#[derive(Default)]
pub(crate) struct Drafts {
    ready: Option<Draft>,
}
impl Drafts {
    pub fn prepare(
        &mut self,
        anchor: QueryAnchor,
        request: ClipRequest,
        connection: &str,
    ) -> Result<Value, &'static str> {
        let token = crate::auth::public_id().map_err(|_| "export_unavailable")?;
        let selection = match &request.layers {
            Layers::All => json!({"mode":"all"}),
            Layers::None => json!({"mode":"none"}),
            Layers::Only(p) => json!({"mode":"only","count":p.len()}),
        };
        let value = json!({"token":token,"dataset_revision":anchor.dataset_revision.to_string(),"bbox_dbu":request.bbox.map(|n|n.to_string()),"layers":selection,"jobs":request.jobs,"cell_name":request.cell_name,"expires_in_ms":"30000"});
        self.ready = Some(Draft {
            anchor,
            request,
            token,
            connection: connection.into(),
            expires: Instant::now() + Duration::from_secs(30),
        });
        Ok(value)
    }
    pub fn check(&self, token: &str) -> Result<&Draft, &'static str> {
        self.ready
            .as_ref()
            .filter(|d| d.token == token && Instant::now() < d.expires)
            .ok_or("export_draft_expired")
    }
    pub fn consume(&mut self) {
        self.ready = None;
    }
    pub fn revoke(&mut self, connection: &str) {
        if self
            .ready
            .as_ref()
            .is_some_and(|d| d.connection == connection)
        {
            self.ready = None;
        }
    }
}
