//! Authenticated owner catalog/operations. All handlers are bounded in-memory
//! lookups/DTO conversion; the service thread performs actual filesystem work.
use crate::{
    service::OperationDto,
    transport::{self, Gate},
    view,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use floe_app_core::view::{CellFailure, CellFailureCode, CellReply, CellRequest};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// A cell query blocks a worker thread on the daemon's hier thread; past
/// this the browser gets 504 and the ticket is withdrawn.
const CELLS_TIMEOUT: Duration = Duration::from_secs(15);
const CELL_PATTERN_CHARS: usize = 256;
pub(crate) fn routes() -> Router<Gate> {
    Router::new()
        .merge(crate::settings::routes())
        .route("/api/v1/catalog", get(catalog))
        .route("/api/v1/catalog/{id}/levels/{start}", get(levels))
        .route("/api/v1/operations", get(operations).post(submit))
        .route("/api/v1/operations/{seq}", get(operation))
        .route(
            "/api/v1/operations/{seq}/index-open",
            get(index_open_preview),
        )
        .route("/api/v1/operations/{seq}/cancel", post(cancel))
        .route("/api/v1/views/{id}", delete(close_view))
        .route("/api/v1/views/{id}/layers/{start}", get(layers))
        .route("/api/v1/views/{id}/palette", post(palette))
        .route("/api/v1/views/{id}/cells", post(cells))
        .route("/api/v1/views/{id}/minimap/{base}", get(minimap))
}
fn failure(code: &'static str) -> Response {
    let status = match code {
        "busy" => StatusCode::TOO_MANY_REQUESTS,
        "operation_conflict" | "operation_sequence" => StatusCode::CONFLICT,
        "operation_expired" | "closed" => StatusCode::GONE,
        "source_unavailable" | "view_unavailable" => StatusCode::NOT_FOUND,
        "timeout" => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(json!({"error":code}))).into_response()
}
/// One cell-tree question (rust/web/ui/cells.js). Coordinates are DBU
/// numbers here, unlike the string world coordinates of a view patch: the
/// panel hands back what the snapshot gave it and draws boxes in pixels.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CellsDto {
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
impl CellsDto {
    /// `root` is the displayed view root: extents and instance walks count
    /// under it, as the frame does.
    fn core(self, id: &str, root: Option<u32>) -> Result<CellRequest, &'static str> {
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
                if pattern.chars().count() > CELL_PATTERN_CHARS
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
fn cell_reply(reply: CellReply) -> serde_json::Value {
    match reply {
        CellReply::Sources(sources) => json!({"sources":sources.iter().map(|s|json!({
            "src":s.source,"placements":s.placements,"name":source_name(&s.path)
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
fn cell_refusal(f: CellFailure) -> Response {
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
async fn cells(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<CellsDto>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(v) = gate.active_view().filter(|v| v.id == id) else {
        return failure("view_unavailable");
    };
    let Ok(Json(request)) = body else {
        return failure("invalid_request");
    };
    let root = v.controller.snapshot().state.root.map(|r| r.cell);
    let request = match request.core(&id, root) {
        Ok(r) => r,
        Err(code) => return failure(code),
    };
    let controller = std::sync::Arc::clone(&v.controller);
    let outcome =
        tokio::task::spawn_blocking(move || controller.cell_query(request, CELLS_TIMEOUT)).await;
    match outcome {
        Ok(Ok(Ok(reply))) => Json(cell_reply(reply)).into_response(),
        Ok(Ok(Err(refusal))) => cell_refusal(refusal),
        Ok(Err(e)) => failure(view::edit_code(e)),
        Err(_) => failure("worker_failed"),
    }
}
async fn minimap(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, base)): Path<(String, String)>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(v) = gate.active_view().filter(|v| v.id == id) else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let model = &v.controller.model;
    match model.minimap.base(&base) {
        Some(pixels) => Json(json!({"view_id":id,"dataset_revision":model.dataset_revision.to_string(),"base":base,"size":180,"pixels":pixels})).into_response(),
        None => transport::error(StatusCode::NOT_FOUND),
    }
}
async fn catalog(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    gate.service.as_ref().map_or_else(
        || transport::error(StatusCode::NOT_FOUND),
        |s| Json(s.catalog()).into_response(),
    )
}
async fn levels(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, start)): Path<(String, usize)>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    gate.service
        .as_ref()
        .and_then(|s| s.levels(&id, start))
        .map_or_else(
            || transport::error(StatusCode::NOT_FOUND),
            |v| Json(v).into_response(),
        )
}
async fn layers(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path((id, start)): Path<(String, usize)>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    gate.active_view()
        .filter(|v| v.id == id)
        .and_then(|v| v.rows.page(&v.controller.snapshot(), start))
        .map_or_else(
            || transport::error(StatusCode::NOT_FOUND),
            |v| Json(v).into_response(),
        )
}
async fn palette(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Result<Json<crate::layer_catalog::PaletteRead>, axum::extract::rejection::JsonRejection>,
) -> Response {
    // POST carries bounded fold/range arguments; it does not submit a view
    // edit, query native geometry, or change the palette stored by another tab.
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(v) = gate.active_view().filter(|v| v.id == id) else {
        return failure("view_unavailable");
    };
    let Ok(Json(request)) = body else {
        return failure("invalid_request");
    };
    match v.rows.read(&v.controller.snapshot(), request) {
        Ok(page) => Json(page).into_response(),
        Err(code) => failure(code),
    }
}
async fn operations(State(gate): State<Gate>, headers: HeaderMap) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    gate.service.as_ref().map_or_else(
        || transport::error(StatusCode::NOT_FOUND),
        |s| Json(s.operations()).into_response(),
    )
}
async fn submit(
    State(gate): State<Gate>,
    headers: HeaderMap,
    body: std::result::Result<Json<OperationDto>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Some(service) = &gate.service else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    let Ok(Json(body)) = body else {
        return failure("invalid_request");
    };
    match service.submit(body) {
        Ok(state) => (StatusCode::ACCEPTED, Json(state)).into_response(),
        Err(code) => failure(code),
    }
}
async fn operation(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(seq): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Ok(seq) = view::counter(&seq) else {
        return failure("invalid_request");
    };
    gate.service
        .as_ref()
        .and_then(|s| s.operation(seq))
        .map_or_else(|| failure("operation_expired"), |v| Json(v).into_response())
}
async fn index_open_preview(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(seq): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Ok(seq) = view::counter(&seq) else {
        return failure("invalid_request");
    };
    let Some(service) = &gate.service else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    match service.index_open_preview(seq) {
        Ok(v) => Json(v).into_response(),
        Err(code) => failure(code),
    }
}
async fn cancel(State(gate): State<Gate>, headers: HeaderMap, Path(seq): Path<String>) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    let Ok(seq) = view::counter(&seq) else {
        return failure("invalid_request");
    };
    let Some(service) = &gate.service else {
        return transport::error(StatusCode::NOT_FOUND);
    };
    match service.cancel(seq) {
        Ok(v) => (StatusCode::ACCEPTED, Json(v)).into_response(),
        Err(code) => failure(code),
    }
}
async fn close_view(
    State(gate): State<Gate>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(e) = transport::http_session(&gate, &headers) {
        return transport::error(e);
    }
    if let Some(service) = &gate.service {
        return match service.close_view(&id) {
            Ok(()) => StatusCode::ACCEPTED.into_response(),
            Err(code) => failure(code),
        };
    }
    match gate.active_view().filter(|v| v.id == id) {
        Some(v) => {
            v.controller.request_close();
            StatusCode::ACCEPTED.into_response()
        }
        None => failure("view_unavailable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_app_core::view::{CellChild, CellMatch, CellSource};
    use serde_json::{json, Value};
    fn core(body: Value, root: Option<u32>) -> Result<CellRequest, &'static str> {
        serde_json::from_value::<CellsDto>(body)
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
        let sources = cell_reply(CellReply::Sources(vec![
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
        let children = cell_reply(CellReply::Children {
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
            cell_reply(CellReply::Find {
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
            cell_reply(CellReply::Bbox {
                source: 0,
                cell: 9,
                insts: 0,
                approx: false,
                bbox: None
            }),
            json!({"insts":0,"approx":false,"bbox":null})
        );
        assert_eq!(
            cell_reply(CellReply::Insts {
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
        let r = cell_refusal(CellFailure {
            code: CellFailureCode::NoHier,
            message: "no hierarchy summary (/private/cache/design.ovh)".into(),
        });
        assert_eq!(r.status(), StatusCode::CONFLICT);
        assert_eq!(failure("timeout").status(), StatusCode::GATEWAY_TIMEOUT);
    }
}
