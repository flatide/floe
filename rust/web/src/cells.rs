//! Cell-tree questions shared by the owner route and the server session
//! route (rust/web/ui/cells.js): request schema, reply DTO and refusals.
use crate::view;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use floe_app_core::view::{CellFailure, CellFailureCode, CellReply, CellRequest};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

pub(crate) const TIMEOUT: Duration = Duration::from_secs(15);
const PATTERN_CHARS: usize = 256;
/// One cell-tree question (rust/web/ui/cells.js). Coordinates are DBU
/// numbers here, unlike the string world coordinates of a view patch: the
/// panel hands back what the snapshot gave it and draws boxes in pixels.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Question {
    kind: String,
    #[serde(default)]
    view_id: view::Field<String>,
    #[serde(default)]
    src: view::Field<i64>,
    #[serde(default)]
    cell: view::Field<u32>,
    #[serde(default)]
    pattern: view::Field<String>,
    #[serde(default)]
    limit: view::Field<usize>,
    #[serde(default)]
    view: view::Field<[f64; 4]>,
    #[serde(default)]
    cap: view::Field<usize>,
}
impl Question {
    /// `root` is the displayed view root: extents and instance walks count
    /// under it, as the frame does.
    pub(crate) fn core(self, id: &str, root: Option<u32>) -> Result<CellRequest, &'static str> {
        if self.view_id.optional().is_some_and(|v| v != id) {
            return Err("invalid_request");
        }
        let source = |s: view::Field<i64>| -> Result<usize, &'static str> {
            usize::try_from(s.optional().ok_or("invalid_request")?).map_err(|_| "invalid_request")
        };
        let cell = |c: view::Field<u32>| c.optional().ok_or("invalid_request");
        let request = match self.kind.as_str() {
            "sources" => CellRequest::Sources,
            "children" => CellRequest::Children {
                source: source(self.src)?,
                cell: self.cell.optional(),
            },
            "find" => {
                let pattern = self.pattern.optional().ok_or("invalid_request")?;
                let limit = self.limit.optional().ok_or("invalid_request")?;
                if pattern.chars().count() > PATTERN_CHARS
                    || pattern.chars().any(char::is_control)
                    || !(1..=floe_worker_client::CELL_FIND_CAP).contains(&limit)
                {
                    return Err("invalid_request");
                }
                let src = self.src.optional().ok_or("invalid_request")?;
                CellRequest::Find {
                    source: if src < 0 {
                        None
                    } else {
                        Some(usize::try_from(src).map_err(|_| "invalid_request")?)
                    },
                    pattern,
                    limit,
                }
            }
            "bbox" => CellRequest::Bbox {
                source: source(self.src)?,
                cell: cell(self.cell)?,
                root,
            },
            "insts" => {
                let view = self.view.optional().ok_or("invalid_request")?;
                let cap = self.cap.optional().ok_or("invalid_request")?;
                if !view.iter().all(|v| v.is_finite())
                    || view[0] >= view[2]
                    || view[1] >= view[3]
                    || !(1..=floe_worker_client::CELL_INSTS_CAP).contains(&cap)
                {
                    return Err("invalid_request");
                }
                CellRequest::Insts {
                    source: source(self.src)?,
                    cell: cell(self.cell)?,
                    view,
                    cap,
                    root,
                }
            }
            _ => return Err("invalid_request"),
        };
        Ok(request)
    }
}
/// A source's display name: the leaf of its cache folder (`.<src>.ice`
/// gives `<src>`); the folder's path stays on the server.
fn source_name(path: &str) -> String {
    let leaf = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    leaf.strip_prefix('.')
        .and_then(|s| s.strip_suffix(".ice"))
        .filter(|s| !s.is_empty())
        .map_or(leaf.clone(), str::to_owned)
}
/// `public`: a public demo names its sources by index, never by the cache
/// folder of a file under the operator's data root.
pub(crate) fn reply(reply: CellReply, public: bool) -> serde_json::Value {
    match reply {
        CellReply::Sources(sources) => json!({"sources":sources.iter().map(|s|json!({
            "src":s.source,"placements":s.placements,
            "name":if public { format!("source {}", s.source) } else { source_name(&s.path) }
        })).collect::<Vec<_>>()}),
        CellReply::Children {
            cell,
            name,
            insts,
            height,
            unit,
            bbox,
            total,
            children,
            ..
        } => json!({"cell":cell,"name":name,"insts":insts,"height":height,
            // The daemon's unit is DBU per micrometre; the panel scales DBU
            // boxes to micrometres, like the snapshot's dbu_um.
            "unit":1./unit,"bbox":bbox,
            "n":children.len(),"total":total,
            "children":children.iter().map(|c|json!({"ci":c.cell,"members":c.members,"leaf":c.leaf,"name":c.name})).collect::<Vec<_>>()}),
        CellReply::Find { total, matches } => json!({"total":total,"n":matches.len(),
            "matches":matches.iter().map(|m|json!({"src":m.source,"ci":m.cell,"insts":m.insts,"name":m.name})).collect::<Vec<_>>()}),
        CellReply::Bbox {
            insts,
            approx,
            bbox,
            ..
        } => json!({"insts":insts,"approx":approx,"bbox":bbox}),
        CellReply::Insts {
            more,
            visited,
            boxes,
            ..
        } => json!({"n":boxes.len(),"more":more,"visited":visited,"boxes":boxes}),
    }
}
/// The daemon's refusal, by code. Its native text can name cache files;
/// the browser gets a fixed sentence per code instead.
pub(crate) fn refusal(f: CellFailure) -> Response {
    let message = match f.code {
        CellFailureCode::NoHier => "No cell index (design.ovh) for this source.",
        CellFailureCode::Superseded => "Superseded by a newer query.",
        CellFailureCode::State => "The layout is not open.",
        CellFailureCode::Query => "The cell query failed.",
        CellFailureCode::Oversize => "The answer exceeds the reply limit.",
    };
    (
        StatusCode::CONFLICT,
        Json(json!({"error":f.code.wire(),"message":message})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_app_core::view::{CellChild, CellMatch, CellSource};
    use serde_json::{json, Value};
    fn reply_owner(r: CellReply) -> Value {
        reply(r, false)
    }
    fn core(body: Value, root: Option<u32>) -> Result<CellRequest, &'static str> {
        serde_json::from_value::<Question>(body)
            .map_err(|_| "invalid_request")
            .and_then(|dto| dto.core("v1", root))
    }
    #[test]
    fn cell_requests_take_the_panel_bodies_and_the_displayed_root() {
        assert_eq!(
            core(json!({"kind":"sources","view_id":"v1"}), None).unwrap(),
            CellRequest::Sources
        );
        assert_eq!(
            core(json!({"kind":"children","src":0,"view_id":"v1"}), Some(9)).unwrap(),
            CellRequest::Children {
                source: 0,
                cell: None
            }
        );
        assert_eq!(
            core(json!({"kind":"children","src":1,"cell":17}), None).unwrap(),
            CellRequest::Children {
                source: 1,
                cell: Some(17)
            }
        );
        assert_eq!(
            core(
                json!({"kind":"find","src":-1,"pattern":"*inv?","limit":2000}),
                None
            )
            .unwrap(),
            CellRequest::Find {
                source: None,
                pattern: "*inv?".into(),
                limit: 2000
            }
        );
        assert_eq!(
            core(json!({"kind":"find","src":2,"pattern":"","limit":1}), None).unwrap(),
            CellRequest::Find {
                source: Some(2),
                pattern: String::new(),
                limit: 1
            }
        );
        // Extents and instance walks count under the displayed root.
        assert_eq!(
            core(json!({"kind":"bbox","src":0,"cell":9}), Some(17)).unwrap(),
            CellRequest::Bbox {
                source: 0,
                cell: 9,
                root: Some(17)
            }
        );
        assert_eq!(
            core(
                json!({"kind":"insts","src":0,"cell":9,"view":[0,-5,10.5,20],"cap":4096}),
                Some(17)
            )
            .unwrap(),
            CellRequest::Insts {
                source: 0,
                cell: 9,
                view: [0., -5., 10.5, 20.],
                cap: 4096,
                root: Some(17)
            }
        );
        for body in [
            json!({"kind":"sources","view_id":"other"}),
            json!({"kind":"sources","out":"/tmp/x"}),
            json!({"kind":"tree"}),
            json!({"kind":"children"}),
            json!({"kind":"children","src":-1}),
            json!({"kind":"children","src":0,"cell":null}),
            json!({"kind":"find","src":-1,"pattern":"a"}),
            json!({"kind":"find","src":-1,"pattern":"a","limit":0}),
            json!({"kind":"find","src":-1,"pattern":"a","limit":5001}),
            json!({"kind":"find","src":-1,"pattern":"x".repeat(257),"limit":1}),
            json!({"kind":"find","src":-1,"pattern":"a\nb","limit":1}),
            json!({"kind":"bbox","src":0}),
            json!({"kind":"bbox","src":0,"cell":"9"}),
            json!({"kind":"insts","src":0,"cell":9,"view":[0,0,1,1],"cap":4097}),
            json!({"kind":"insts","src":0,"cell":9,"view":[0,0,0,1],"cap":1}),
            json!({"kind":"insts","src":0,"cell":9,"view":["0","0","1","1"],"cap":1}),
            json!({"kind":"insts","src":0,"cell":9,"view":[0,0,1],"cap":1}),
        ] {
            assert!(core(body.clone(), None).is_err(), "{body}");
        }
    }
    #[test]
    fn cell_replies_are_the_panel_shapes_with_micrometre_units_and_leaf_names() {
        let sources = reply_owner(CellReply::Sources(vec![
            CellSource {
                source: 0,
                placements: 1,
                path: "/private/data/.chip.oas.ice".into(),
            },
            CellSource {
                source: 1,
                placements: 3,
                path: "/tmp/floe-worker-1/source".into(),
            },
        ]));
        assert_eq!(
            sources,
            json!({"sources":[{"src":0,"placements":1,"name":"chip.oas"},{"src":1,"placements":3,"name":"source"}]})
        );
        assert!(!sources.to_string().contains("/private"));
        // A public demo names sources by index only.
        let public = reply(
            CellReply::Sources(vec![CellSource {
                source: 0,
                placements: 1,
                path: "/srv/samples/.secret-name.oas.ice".into(),
            }]),
            true,
        );
        assert_eq!(
            public,
            json!({"sources":[{"src":0,"placements":1,"name":"source 0"}]})
        );
        let children = reply_owner(CellReply::Children {
            source: 0,
            cell: 6,
            name: "TOP".into(),
            insts: 1,
            height: 2,
            unit: 1000.,
            bbox: Some([23., 214., 403401., 446265.]),
            total: 5,
            children: vec![CellChild {
                cell: 3,
                members: 2,
                leaf: false,
                name: "A".into(),
            }],
        });
        assert_eq!(
            children,
            json!({"cell":6,"name":"TOP","insts":1,"height":2,"unit":0.001,"bbox":[23.,214.,403401.,446265.],"n":1,"total":5,
                "children":[{"ci":3,"members":2,"leaf":false,"name":"A"}]})
        );
        assert_eq!(
            reply_owner(CellReply::Find {
                total: 7,
                matches: vec![CellMatch {
                    source: 0,
                    cell: 4,
                    insts: 10,
                    name: "A".into()
                }]
            }),
            json!({"total":7,"n":1,"matches":[{"src":0,"ci":4,"insts":10,"name":"A"}]})
        );
        assert_eq!(
            reply_owner(CellReply::Bbox {
                source: 0,
                cell: 9,
                insts: 0,
                approx: false,
                bbox: None
            }),
            json!({"insts":0,"approx":false,"bbox":null})
        );
        assert_eq!(
            reply_owner(CellReply::Insts {
                source: 0,
                cell: 9,
                more: true,
                visited: 77,
                boxes: vec![[0., 0., 1., 1.]]
            }),
            json!({"n":1,"more":true,"visited":77,"boxes":[[0.,0.,1.,1.]]})
        );
        assert_eq!(source_name("/a/.x.ice"), "x");
        assert_eq!(source_name(""), "");
    }
    #[test]
    fn a_daemon_refusal_is_a_conflict_with_its_code_and_a_fixed_sentence() {
        let r = refusal(CellFailure {
            code: CellFailureCode::NoHier,
            message: "no hierarchy summary (/private/cache/design.ovh)".into(),
        });
        assert_eq!(r.status(), StatusCode::CONFLICT);
    }
}
