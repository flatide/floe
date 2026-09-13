//! Owner socket queries: displayed receipts and outstanding results are local
//! to one authenticated connection. No native diagnostic/path is serialized.
use crate::view::{self, Selection};
use floe_app_core::view::{
    DisplayFrame, Purpose, QueryAnchor, ViewController, ViewQuery, ViewQueryResult,
};
use floe_worker_client::{
    Layers, QueryHit, QueryKind, QueryOperation, QueryScene, QueryStatus, SnapKind,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(try_from = "String")]
pub(crate) enum Kind {
    Snap,
    Pick,
}
impl TryFrom<String> for Kind {
    type Error = &'static str;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        match s.as_str() {
            "snap" => Ok(Self::Snap),
            "pick" => Ok(Self::Pick),
            _ => Err("invalid query kind"),
        }
    }
}
impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Snap => "snap",
            Self::Pick => "pick",
        }
    }
    fn core(self) -> QueryKind {
        match self {
            Self::Snap => QueryKind::Snap,
            Self::Pick => QueryKind::Pick,
        }
    }
    fn index(self) -> usize {
        match self {
            Self::Snap => 0,
            Self::Pick => 1,
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Snap {},
    Pick { nth: String },
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Anchor {
    dataset_revision: String,
    worker_epoch: String,
    frame_id: String,
    state_rev: String,
    render_rev: String,
    render_key: String,
}
impl Anchor {
    fn core(self) -> Result<QueryAnchor, &'static str> {
        Ok(QueryAnchor {
            dataset_revision: view::counter(&self.dataset_revision)?,
            worker_epoch: view::counter(&self.worker_epoch)?,
            frame_id: view::counter(&self.frame_id)?,
            state_rev: view::counter(&self.state_rev)?,
            render_rev: view::counter(&self.render_rev)?,
            render_key: view::counter(&self.render_key)?,
        })
    }
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    anchor: Anchor,
    operation: Operation,
    position: [f64; 2],
    radius_px: f64,
    layers: Selection,
}
impl Request {
    fn core(self) -> Result<ViewQuery, &'static str> {
        let operation = match self.operation {
            Operation::Snap {} => QueryOperation::Snap,
            Operation::Pick { nth } => {
                let n = nth.parse::<i64>().map_err(|_| "invalid query cycle")?;
                if n.to_string() != nth {
                    return Err("invalid query cycle");
                }
                QueryOperation::Pick { nth: n }
            }
        };
        if !self
            .position
            .iter()
            .all(|n| n.is_finite() && (0.0..=1.0).contains(n))
            || !self.radius_px.is_finite()
            || !(0.0..=64.0).contains(&self.radius_px)
        {
            return Err("invalid query position or radius");
        }
        let layers = match self.layers {
            Selection::All {} => Layers::All,
            Selection::None {} => Layers::None,
            Selection::Only { pairs } => {
                if pairs.is_empty() || pairs.len() > 4096 {
                    return Err("invalid query layers");
                }
                Layers::Only(pairs)
            }
        };
        Ok(ViewQuery {
            anchor: self.anchor.core()?,
            operation,
            position: self.position,
            radius_px: self.radius_px,
            layers,
        })
    }
}

/// Only immutable identity, never a frame Arc/payload/scene history. A margin
/// receipt remains useful across state/render revisions for valid crop pans.
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Default))]
pub(crate) struct Receipt {
    frame_id: u64,
    dataset_revision: u64,
    worker_epoch: u64,
    render_key: u64,
    margin: bool,
}
impl Receipt {
    pub fn of(f: &DisplayFrame) -> Self {
        Self {
            frame_id: f.id,
            dataset_revision: f.dataset_revision,
            worker_epoch: f.worker_epoch,
            render_key: f.render_key,
            margin: f.purpose == Purpose::Margin,
        }
    }
    fn matches(self, a: QueryAnchor) -> bool {
        self.frame_id == a.frame_id
            && self.dataset_revision == a.dataset_revision
            && self.worker_epoch == a.worker_epoch
            && self.render_key == a.render_key
    }
}
struct Ticket {
    sequence: String,
    id: u64,
    anchor: QueryAnchor,
    sent: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MeasureRequest {
    anchor: Anchor,
    position: [f64; 2],
    start_dbu: Option<[String; 2]>,
    free_angle: bool,
    snap_query: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MeasureSelectionRequest {
    anchor: Anchor,
    boxes_dbu: Vec<[String; 4]>,
}
impl MeasureSelectionRequest {
    fn core(self) -> Result<(QueryAnchor, Vec<[i64; 4]>), &'static str> {
        if self.boxes_dbu.len() > 64 {
            return Err("invalid_request");
        }
        let mut boxes = Vec::with_capacity(self.boxes_dbu.len());
        for b in self.boxes_dbu {
            let mut bounds = [0; 4];
            for (i, s) in b.into_iter().enumerate() {
                bounds[i] = s.parse::<i64>().map_err(|_| "invalid_request")?;
                if bounds[i].to_string() != s {
                    return Err("invalid_request");
                }
            }
            if bounds[0] > bounds[2] || bounds[1] > bounds[3] {
                return Err("invalid_request");
            }
            boxes.push(bounds);
        }
        Ok((self.anchor.core().map_err(|_| "invalid_request")?, boxes))
    }
}
#[derive(Default)]
struct Receipts([Option<Receipt>; 2]);
impl Receipts {
    fn displayed(&mut self, r: Receipt) {
        self.0[usize::from(r.margin)] = Some(r);
    }
    fn accepts(&self, a: QueryAnchor) -> bool {
        self.0.iter().flatten().any(|r| r.matches(a))
    }
}
pub(crate) struct Queries<'a> {
    controller: &'a ViewController,
    displayed: Receipts,
    tickets: [Option<Ticket>; 2],
}
impl<'a> Queries<'a> {
    pub fn new(controller: &'a ViewController) -> Self {
        Self {
            controller,
            displayed: Receipts::default(),
            tickets: [None, None],
        }
    }
    /// Caller must have both the matching displayed ACK and writer completion.
    pub fn displayed(&mut self, r: Receipt) {
        self.displayed.displayed(r);
    }
    pub fn submit(&mut self, sequence: String, request: Request) -> Result<u64, &'static str> {
        let input = request.core().map_err(|_| "invalid_request")?;
        if !self.displayed.accepts(input.anchor) {
            return Err("frame_not_displayed");
        }
        let index = match input.operation.kind() {
            QueryKind::Snap => 0,
            QueryKind::Pick => 1,
        };
        let anchor = input.anchor;
        let id = self.controller.query(input).map_err(|e| {
            if e.kind == floe_app_core::ErrorKind::Busy {
                "stale_frame"
            } else {
                view::safe_error(e.kind)
            }
        })?;
        self.tickets[index] = Some(Ticket {
            sequence,
            id,
            anchor,
            sent: false,
        });
        Ok(id)
    }
    pub fn cancel(&mut self, kind: Kind) {
        if let Some(ticket) = self.tickets[kind.index()].take() {
            self.controller
                .cancel_query_if_current(kind.core(), ticket.id);
        }
    }
    /// Constant-size, read-only arithmetic on the same owner/display receipt.
    /// A snap reference can only name a result sent on this connection.
    pub fn measure(
        &self,
        sequence: &str,
        request: MeasureRequest,
        view_id: &str,
        epoch: &str,
    ) -> Result<Value, &'static str> {
        let a = request.anchor.core().map_err(|_| "invalid_request")?;
        if !self.displayed.accepts(a) {
            return Err("frame_not_displayed");
        }
        let snap_id = request
            .snap_query
            .map(|id| view::counter(&id))
            .transpose()
            .map_err(|_| "invalid_request")?;
        if let Some(id) = snap_id {
            if !self.tickets[0]
                .as_ref()
                .is_some_and(|t| t.id == id && t.anchor == a && t.sent)
            {
                return Err("stale_snap");
            }
        }
        let start = request
            .start_dbu
            .map(|p| floe_app_core::view::RulerPoint::parse([&p[0], &p[1]]))
            .transpose()
            .map_err(|_| "invalid_request")?;
        let r = self
            .controller
            .measure(a, request.position, start, request.free_angle, snap_id)
            .map_err(|e| {
                if e.kind == floe_app_core::ErrorKind::Busy {
                    "stale_frame"
                } else {
                    view::safe_error(e.kind)
                }
            })?;
        Ok(
            json!({"type":"measure.result","seq":sequence,"view_id":view_id,"connection_epoch":epoch,"anchor":anchor(a),
            "point_dbu":r.point.strings(),"snap":r.snap.map(|s|match s {SnapKind::Vertex=>"vertex",SnapKind::Edge=>"edge"}),
            "segment":r.segment.map(|s|json!({"endpoints_dbu":s.endpoints.map(|p|p.strings()),
                "delta_um":s.delta_strings(),"distance_um":s.distance_string()}))}),
        )
    }
    pub fn measure_selection(
        &self,
        sequence: &str,
        request: MeasureSelectionRequest,
        view_id: &str,
        epoch: &str,
    ) -> Result<Value, &'static str> {
        let (a, boxes) = request.core()?;
        if !self.displayed.accepts(a) {
            return Err("frame_not_displayed");
        }
        let segments = self.controller.measure_selection(a, &boxes).map_err(|e| {
            if e.kind == floe_app_core::ErrorKind::Busy {
                "stale_frame"
            } else {
                view::safe_error(e.kind)
            }
        })?;
        Ok(
            json!({"type":"measure_selection.result","seq":sequence,"view_id":view_id,
            "connection_epoch":epoch,"anchor":anchor(a),"segments":segments.iter().map(segment).collect::<Vec<_>>()}),
        )
    }
    /// Latest-only results, at most one per kind. A full socket queue leaves
    /// the ticket unsent for retry, never accumulates another response queue.
    pub fn ready(&self, index: usize, view_id: &str, epoch: &str) -> Option<Value> {
        let ticket = self.tickets[index].as_ref().filter(|t| !t.sent)?;
        let current = self.controller.query_snapshot();
        let (id, result) = if index == 0 {
            (current.snap_id, current.snap)
        } else {
            (current.pick_id, current.pick)
        };
        let stale =
            self.controller.query_anchor(ticket.anchor.frame_id).ok() != Some(ticket.anchor);
        if stale || id != Some(ticket.id) {
            return Some(envelope(
                ticket,
                view_id,
                epoch,
                if stale { "stale_frame" } else { "superseded" },
                None,
            ));
        }
        result
            .filter(|r| r.id == ticket.id && r.anchor == ticket.anchor)
            .map(|r| response(ticket, &r, view_id, epoch))
    }
    pub fn sent(&mut self, index: usize) {
        if let Some(t) = &mut self.tickets[index] {
            t.sent = true;
        }
    }
}
impl Drop for Queries<'_> {
    fn drop(&mut self) {
        self.cancel(Kind::Snap);
        self.cancel(Kind::Pick);
    }
}
fn anchor(a: QueryAnchor) -> Value {
    json!({"dataset_revision":a.dataset_revision.to_string(),"worker_epoch":a.worker_epoch.to_string(),
        "frame_id":a.frame_id.to_string(),"state_rev":a.state_rev.to_string(),
        "render_rev":a.render_rev.to_string(),"render_key":a.render_key.to_string()})
}
fn segment(s: &floe_app_core::view::RulerSegment) -> Value {
    json!({"endpoints_dbu":s.endpoints.map(|p|p.strings()),
        "delta_um":s.delta_strings(),"distance_um":s.distance_string()})
}
pub(crate) fn scene(s: QueryScene) -> Value {
    json!({"generation":s.id.map(|id|id.generation.to_string()),"round":s.id.map(|id|id.round.to_string()),
        "complete":s.complete,"summary_layers":s.summary_layers.to_string()})
}
fn envelope(t: &Ticket, view_id: &str, epoch: &str, status: &str, hit: Option<Value>) -> Value {
    json!({"type":"query.result","seq":t.sequence,"view_id":view_id,"connection_epoch":epoch,
        "query_id":t.id.to_string(),"anchor":anchor(t.anchor),"status":status,"hit":hit})
}
fn response(t: &Ticket, r: &ViewQueryResult, view_id: &str, epoch: &str) -> Value {
    let q = &r.reply;
    let status = match q.status {
        QueryStatus::Ok => "ok",
        QueryStatus::Unavailable => "scene_unavailable",
        QueryStatus::Mismatch => "scene_mismatch",
        QueryStatus::Incomplete => "scene_incomplete",
        QueryStatus::Summary => "scene_summary",
        QueryStatus::Superseded => "superseded",
        QueryStatus::Error => "query_failed",
    };
    let hit = q.hit.as_ref().map(|h| match h {
        QueryHit::Snap(s) => json!({"kind":"snap","point_dbu":[s.x.to_string(),s.y.to_string()],
            "snap":match s.kind {SnapKind::Vertex=>"vertex",SnapKind::Edge=>"edge"}}),
        QueryHit::Pick(p) => json!({"kind":"pick","count":p.count.to_string(),"index":p.index.to_string(),
            "pair":[p.layer.0,p.layer.1],"layer_name":p.layer_name,"cell_name":p.cell_name,
            "area_dbu2":p.area.to_string(),"bbox_dbu":p.bbox.map(|n|n.to_string()),
            "points_dbu":p.points.iter().map(|(x,y)|[x.to_string(),y.to_string()]).collect::<Vec<_>>(),
            "points_truncated":p.points_truncated}),
    });
    let mut out = envelope(t, view_id, epoch, status, hit);
    out["scene"] = scene(q.scene);
    out["requested_summary_layers"] = json!(q.summary_layers.to_string());
    out
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
