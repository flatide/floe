//! Connection-local receipts and read-only arithmetic for ONE explorer.
//! No owner dispatcher, filesystem/export methods or history of frame bytes.
use super::{
    explore::{layers_within, scoped_layers},
    Scope,
};
use crate::query;
use floe_app_core::view::{ViewController, ViewQueryResult};
use floe_worker_client::{Layers, QueryHit};
use serde_json::Value;

pub(super) struct Queries<'a> {
    inner: query::Queries<'a>,
    controller: &'a ViewController,
    scope: &'a Scope,
}
impl<'a> Queries<'a> {
    pub fn new(controller: &'a ViewController, scope: &'a Scope) -> Self {
        Self {
            inner: query::Queries::new(controller),
            controller,
            scope,
        }
    }
    pub fn displayed(&mut self, receipt: query::Receipt) {
        self.inner.displayed(receipt);
    }
    pub fn submit(
        &mut self,
        sequence: String,
        request: query::Request,
    ) -> Result<u64, &'static str> {
        let mut input = request.core().map_err(|_| "invalid_request")?;
        // Query All follows the current VISIBLE subset, unlike display All.
        let layers = if input.layers == Layers::All {
            self.controller.snapshot().state.layers
        } else {
            input.layers
        };
        input.layers = scoped_layers(self.controller, self.scope, &layers)?;
        self.inner.submit_core(sequence, input)
    }
    pub fn cancel(&mut self, kind: query::Kind) {
        self.inner.cancel(kind);
    }
    pub fn measure(
        &self,
        sequence: &str,
        request: query::MeasureRequest,
        view: &str,
        epoch: &str,
    ) -> Result<Value, &'static str> {
        self.inner.measure(sequence, request, view, epoch)
    }
    pub fn measure_selection(
        &self,
        sequence: &str,
        request: query::MeasureSelectionRequest,
        view: &str,
        epoch: &str,
    ) -> Result<Value, &'static str> {
        self.inner.measure_selection(sequence, request, view, epoch)
    }
    pub fn ready(&mut self, index: usize, view: &str, epoch: &str) -> Option<Value> {
        let visible = self.controller.snapshot().state.layers;
        let mut denied = false;
        let reply = self.inner.ready_checked(index, view, epoch, |r| {
            denied = !result_within(&self.scope.layers, &visible, r);
            !denied
        });
        if denied {
            // A filtered snap must not later become a ruler authority merely
            // because its query_failed envelope was delivered on this socket.
            self.inner.cancel(if index == 0 {
                query::Kind::Snap
            } else {
                query::Kind::Pick
            });
        }
        reply
    }
    pub fn sent(&mut self, index: usize) {
        self.inner.sent(index);
    }
}

fn result_within(granted: &Layers, visible: &Layers, result: &ViewQueryResult) -> bool {
    // The native scene and request enforce snap's layer scope (a snap has no
    // plane field). Explicit pick planes can also be checked before encoding.
    if !layers_within(granted, &result.reply.request.layers)
        || !layers_within(visible, &result.reply.request.layers)
    {
        return false;
    }
    match &result.reply.hit {
        None => true,
        Some(QueryHit::Pick(hit)) => {
            let plane = Layers::Only(vec![hit.layer]);
            layers_within(granted, &plane)
                && layers_within(visible, &plane)
                && layers_within(&result.reply.request.layers, &plane)
        }
        Some(QueryHit::Snap(_)) => result.reply.request.layers != Layers::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use floe_app_core::view::QueryAnchor;
    use floe_worker_client::{
        QueryOperation, QueryReply, QueryRequest, QueryScene, QueryStatus, SceneId, SnapHit,
        SnapKind,
    };
    #[test]
    fn result_requires_scoped_visible_request_even_when_native_reports_success() {
        let id = SceneId {
            generation: 1,
            round: 1,
        };
        let mut result = ViewQueryResult {
            id: 1,
            anchor: QueryAnchor {
                dataset_revision: 1,
                worker_epoch: 1,
                frame_id: 1,
                state_rev: 1,
                render_rev: 1,
                render_key: 1,
            },
            reply: QueryReply {
                request: QueryRequest {
                    scene: id,
                    operation: QueryOperation::Snap,
                    x: 0,
                    y: 0,
                    radius: 1,
                    layers: Layers::Only(vec![(7, 0)]),
                },
                scene: QueryScene {
                    id: Some(id),
                    complete: true,
                    summary_layers: 0,
                },
                status: QueryStatus::Ok,
                summary_layers: 0,
                hit: Some(QueryHit::Snap(SnapHit {
                    x: 0,
                    y: 0,
                    kind: SnapKind::Vertex,
                })),
                sequence: 1,
                error: None,
            },
        };
        let allowed = Layers::Only(vec![(7, 0)]);
        assert!(result_within(&allowed, &allowed, &result));
        assert!(!result_within(&allowed, &Layers::None, &result));
        result.reply.request.layers = Layers::All;
        assert!(!result_within(&allowed, &Layers::All, &result));
        result.reply.request.layers = Layers::None;
        assert!(!result_within(&allowed, &allowed, &result));
        result.reply.hit = None;
        assert!(result_within(&allowed, &allowed, &result));
        result.reply.request.layers = allowed.clone();
        result.reply.request.operation = QueryOperation::Pick { nth: 0 };
        result.reply.hit = Some(QueryHit::Pick(floe_worker_client::PickHit {
            count: 1,
            index: 0,
            layer: (8, 0),
            layer_name: "hidden".into(),
            cell_name: "private".into(),
            area: 1.,
            bbox: [0, 0, 1, 1],
            points: Vec::new(),
            points_truncated: false,
        }));
        assert!(!result_within(&allowed, &allowed, &result));
        let Some(QueryHit::Pick(hit)) = result.reply.hit.as_mut() else {
            unreachable!()
        };
        hit.layer = (7, 0);
        assert!(result_within(&allowed, &allowed, &result));
    }
}
