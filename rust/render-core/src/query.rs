use crate::raster::{checked_path_outline, SeenPlanes};
use crate::repetition::for_each_visible_offset_query;
use crate::scene::FrameScene;
use crate::transform::OrthoTransform;
use crate::RenderCancellation;
use floe_ovm::BBox;
use floe_vfs::hier::WsKey;
use std::cell::Cell;
use std::collections::BTreeSet;
use std::sync::Arc;

const QUERY_STOP: &str = "__floe_query_cap__";

/// §3.19 second iteration: the member budget is a SAFETY VALVE, not a
/// work limiter - enumeration is naturally bounded by the query box
/// (grid ranges, Pts chunks, bbox culling), matching floe's
/// RecursiveShapeIterator model where only the candidate count is
/// capped. Exhausting it therefore surfaces as a real error (the GUI
/// shows it) instead of the silent empty result that made dense chips
/// "no object here" everywhere.
const QUERY_MEMBER_LIMIT: &str = "query member limit exceeded: overly dense query region";

#[derive(Clone, Debug, PartialEq)]
pub struct SceneQueryRequest {
    pub x: i64,
    pub y: i64,
    pub radius: i64,
    /// Stable OVM layer order. Snap ties retain this traversal order while
    /// pick candidates are sorted by the numeric layer/datatype pair.
    pub layers: Vec<SceneQueryLayer>,
    /// Snap: maximum shapes examined. Pick: the candidates kept - the
    /// smallest of all the shapes that hold the point.
    pub shape_cap: usize,
    /// Maximum repetition members examined, including members outside the
    /// query box. This bounds sparse explicit-point repetitions.
    pub member_cap: usize,
    /// What shows of each layer around the point in the frame on the screen
    /// (`raster::seen_planes_around`). Pick: a shape whose layer shows no
    /// pixel within the shape there is no candidate - it is on, and under
    /// others. Snap: a vertex or a point of an edge where its layer shows no
    /// pixel is no answer - the corner under another shape is not snapped
    /// to (0.12.457). None: every shape.
    pub seen: Option<Arc<SeenPlanes>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneQueryLayer {
    pub index: u32,
    pub layer: u32,
    pub datatype: u32,
}

impl SceneQueryRequest {
    fn validate(&self) -> Result<(), String> {
        if self.radius < 0 {
            return Err(format!(
                "query radius must be non-negative: {}",
                self.radius
            ));
        }
        if self.shape_cap == 0 {
            return Err("query shape_cap must be positive".to_string());
        }
        if self.member_cap == 0 {
            return Err("query member_cap must be positive".to_string());
        }
        let unique: BTreeSet<_> = self.layers.iter().map(|layer| layer.index).collect();
        if unique.len() != self.layers.len() {
            return Err("query layers contain duplicate indexes".to_string());
        }
        Ok(())
    }

    fn view(&self) -> BBox {
        BBox {
            x0: self.x.saturating_sub(self.radius),
            y0: self.y.saturating_sub(self.radius),
            // BBox is half-open; one extra DBU preserves KLayout's inclusive
            // touching query at the positive radius boundary.
            x1: self.x.saturating_add(self.radius).saturating_add(1),
            y1: self.y.saturating_add(self.radius).saturating_add(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneSnapKind {
    Vertex,
    Edge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneSnap {
    pub x: i64,
    pub y: i64,
    pub kind: SceneSnapKind,
    pub shapes_tested: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScenePickCandidate {
    pub layer_idx: u32,
    pub cell_id: u32,
    pub area: f64,
    pub bbox: BBox,
    /// KLayout-compatible clockwise, lexicographically anchored hull.
    pub points: Vec<(i64, i64)>,
    /// A truncated outline is a prefix; never close it as a full polygon.
    pub points_truncated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScenePick {
    pub count: usize,
    pub index: usize,
    pub candidate: Option<ScenePickCandidate>,
    pub shapes_tested: usize,
    /// Shapes that hold the point and do not show there
    /// (`SceneQueryRequest::seen`).
    pub hidden: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SceneShapeKind {
    Rectangle,
    Polygon,
}

pub(crate) struct SceneShape {
    pub kind: SceneShapeKind,
    pub layer_idx: u32,
    pub layer: u32,
    pub datatype: u32,
    pub cell_id: u32,
    pub points: Vec<(i64, i64)>,
}

pub fn snap_scene(
    scene: &FrameScene,
    request: &SceneQueryRequest,
) -> Result<Option<SceneSnap>, String> {
    snap_scene_impl(scene, request, None)
}

/// §F2R-20b: a snap that a newer snap (higher sequence) supersedes
/// aborts at its next member heartbeat instead of running to the
/// member cap - the daemon's query thread stays responsive.
pub fn snap_scene_cancellable(
    scene: &FrameScene,
    request: &SceneQueryRequest,
    sequence: u64,
    cancellation: &RenderCancellation,
) -> Result<Option<SceneSnap>, String> {
    snap_scene_impl(scene, request, Some((sequence, cancellation)))
}

fn snap_scene_impl(
    scene: &FrameScene,
    request: &SceneQueryRequest,
    cancellation: Option<(u64, &RenderCancellation)>,
) -> Result<Option<SceneSnap>, String> {
    request.validate()?;
    let radius2 = request.radius as f64 * request.radius as f64;
    let mut best_vertex = None::<(f64, i64, i64)>;
    let mut best_edge = None::<(f64, f64, f64)>;
    let mut tested = 0usize;
    visit_query_shapes(scene, request, cancellation, |shape| {
        if tested >= request.shape_cap {
            return Err(QUERY_STOP.to_string());
        }
        tested += 1;
        // what is snapped to is what is seen: a corner or a point of an
        // edge that the layers above cover is passed by (the nearer of
        // those that show is taken, not the nearest and then nothing)
        let shows = |x: f64, y: f64| {
            request
                .seen
                .as_ref()
                .is_none_or(|seen| seen.shows_at(shape.layer_idx, x, y))
        };
        for &(x, y) in &shape.points {
            let dx = (x as i128 - request.x as i128) as f64;
            let dy = (y as i128 - request.y as i128) as f64;
            let distance2 = dx * dx + dy * dy;
            if distance2 <= radius2
                && best_vertex.is_none_or(|current| distance2 < current.0)
                && shows(x as f64, y as f64)
            {
                best_vertex = Some((distance2, x, y));
            }
        }
        for index in 0..shape.points.len() {
            let (x1, y1) = shape.points[index];
            let (x2, y2) = shape.points[(index + 1) % shape.points.len()];
            let vx = (x2 as i128 - x1 as i128) as f64;
            let vy = (y2 as i128 - y1 as i128) as f64;
            let length2 = vx * vx + vy * vy;
            if length2 == 0.0 {
                continue;
            }
            let along = ((((request.x as i128 - x1 as i128) as f64) * vx
                + ((request.y as i128 - y1 as i128) as f64) * vy)
                / length2)
                .clamp(0.0, 1.0);
            let qx = x1 as f64 + along * vx;
            let qy = y1 as f64 + along * vy;
            let dx = qx - request.x as f64;
            let dy = qy - request.y as f64;
            let distance2 = dx * dx + dy * dy;
            if distance2 <= radius2
                && best_edge.is_none_or(|current| distance2 < current.0)
                && shows(qx, qy)
            {
                best_edge = Some((distance2, qx, qy));
            }
        }
        Ok(())
    })?;
    let result = if let Some((_, x, y)) = best_vertex {
        Some(SceneSnap {
            x,
            y,
            kind: SceneSnapKind::Vertex,
            shapes_tested: tested,
        })
    } else if let Some((_, x, y)) = best_edge {
        Some(SceneSnap {
            x: round_ties_even_i64(x)?,
            y: round_ties_even_i64(y)?,
            kind: SceneSnapKind::Edge,
            shapes_tested: tested,
        })
    } else {
        None
    };
    Ok(result)
}

pub fn pick_scene(
    scene: &FrameScene,
    request: &SceneQueryRequest,
    nth: i64,
) -> Result<ScenePick, String> {
    pick_scene_impl(scene, request, nth, None)
}

/// §F2R-20b: see `snap_scene_cancellable`.
pub fn pick_scene_cancellable(
    scene: &FrameScene,
    request: &SceneQueryRequest,
    nth: i64,
    sequence: u64,
    cancellation: &RenderCancellation,
) -> Result<ScenePick, String> {
    pick_scene_impl(scene, request, nth, Some((sequence, cancellation)))
}

fn pick_scene_impl(
    scene: &FrameScene,
    request: &SceneQueryRequest,
    nth: i64,
    cancellation: Option<(u64, &RenderCancellation)>,
) -> Result<ScenePick, String> {
    request.validate()?;
    // The smallest first; equal areas by their layer numbers, then as the
    // walk found them (the ordinal makes a stable sort's order explicit).
    type Rank = (f64, u32, u32, usize);
    fn before(left: &Rank, right: &Rank) -> bool {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.3.cmp(&right.3))
            .is_lt()
    }
    // Every shape that holds the point is weighed and the smallest
    // `shape_cap` are kept, in their order. (Until 0.12.454 the walk stopped
    // at the first `shape_cap` it met, in layer order: where more shapes
    // than that lay over a point - the plates of a few hundred layers - the
    // small shape clicked on was never a candidate and the "smallest" was
    // the smallest plate of the first layers.)
    let mut kept = Vec::<(Rank, ScenePickCandidate)>::new();
    let (mut tested, mut held, mut hidden) = (0usize, 0usize, 0usize);
    visit_query_shapes(scene, request, cancellation, |shape| {
        tested += 1;
        if !point_inclusive(&shape.points, request.x, request.y)? {
            return Ok(());
        }
        let bbox = polygon_bbox(&shape.points).expect("query polygon has a bbox");
        // what is picked is what is seen (0.12.456): a shape whose layer
        // shows nothing of it around the point - the layers above cover it -
        // is no candidate, however small
        if let Some(seen) = &request.seen {
            if !seen.shows(shape.layer_idx, bbox) {
                hidden += 1;
                return Ok(());
            }
        }
        let area = (polygon_area2(&shape.points)? / 2) as f64;
        let rank = (area, shape.layer, shape.datatype, held);
        held += 1;
        if kept.len() >= request.shape_cap && kept.last().is_some_and(|last| !before(&rank, &last.0))
        {
            return Ok(());
        }
        let at = kept.partition_point(|(other, _)| before(other, &rank));
        kept.insert(
            at,
            (
                rank,
                ScenePickCandidate {
                    layer_idx: shape.layer_idx,
                    cell_id: shape.cell_id,
                    area,
                    bbox,
                    points_truncated: shape.points.len() > 512,
                    points: shape.points.into_iter().take(512).collect(),
                },
            ),
        );
        kept.truncate(request.shape_cap);
        Ok(())
    })?;
    if kept.is_empty() {
        return Ok(ScenePick {
            count: 0,
            index: 0,
            candidate: None,
            shapes_tested: tested,
            hidden,
        });
    }
    let count = kept.len();
    let index = nth.rem_euclid(count as i64) as usize;
    let candidate = kept.swap_remove(index).1;
    Ok(ScenePick {
        count,
        index,
        candidate: Some(candidate),
        shapes_tested: tested,
        hidden,
    })
}

fn visit_query_shapes(
    scene: &FrameScene,
    request: &SceneQueryRequest,
    cancellation: Option<(u64, &RenderCancellation)>,
    visit: impl FnMut(SceneShape) -> Result<(), String>,
) -> Result<(), String> {
    let member_budget = Cell::new(request.member_cap);
    match visit_scene_shapes_impl(
        scene,
        request.view(),
        &request.layers,
        Some(&member_budget),
        cancellation,
        visit,
    ) {
        Err(error) if error == QUERY_STOP => Ok(()),
        other => other,
    }
}

pub(crate) fn visit_scene_shapes(
    scene: &FrameScene,
    view: BBox,
    layers: &[SceneQueryLayer],
    visit: impl FnMut(SceneShape) -> Result<(), String>,
) -> Result<(), String> {
    visit_scene_shapes_impl(scene, view, layers, None, None, visit)
}

pub(crate) fn visit_scene_shapes_cancellable(
    scene: &FrameScene,
    view: BBox,
    layers: &[SceneQueryLayer],
    generation: u64,
    cancellation: &RenderCancellation,
    visit: impl FnMut(SceneShape) -> Result<(), String>,
) -> Result<(), String> {
    visit_scene_shapes_impl(
        scene,
        view,
        layers,
        None,
        Some((generation, cancellation)),
        visit,
    )
}

fn visit_scene_shapes_impl(
    scene: &FrameScene,
    view: BBox,
    layers: &[SceneQueryLayer],
    member_budget: Option<&Cell<usize>>,
    cancellation: Option<(u64, &RenderCancellation)>,
    mut visit: impl FnMut(SceneShape) -> Result<(), String>,
) -> Result<(), String> {
    check_visit_cancelled(cancellation)?;
    for &layer in layers {
        check_visit_cancelled(cancellation)?;
        // 2b subtree mask (§3.19): skip whole subtrees that cannot
        // hold this layer, exactly like the raster's per-plane gate.
        let layer_bit = scene.layer_mask_bit(layer.index);
        visit_cell_layer(
            scene,
            view,
            layer,
            layer_bit,
            scene.top(),
            OrthoTransform::identity(),
            &mut Vec::new(),
            member_budget,
            cancellation,
            &mut visit,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn visit_cell_layer(
    scene: &FrameScene,
    view: BBox,
    layer: SceneQueryLayer,
    layer_bit: Option<usize>,
    key: WsKey,
    world_transform: OrthoTransform,
    path: &mut Vec<WsKey>,
    member_budget: Option<&Cell<usize>>,
    cancellation: Option<(u64, &RenderCancellation)>,
    visit: &mut impl FnMut(SceneShape) -> Result<(), String>,
) -> Result<(), String> {
    if path.contains(&key) {
        return Err(format!("invalid plan: hierarchy cycle at {:?}", key));
    }
    let cell = scene
        .cell(key)
        .ok_or_else(|| format!("invalid plan: missing working cell {:?}", key))?;
    path.push(key);
    let local_view = world_transform.invert()?.apply_bbox(view)?;

    {
        let mut emit = |kind: SceneShapeKind, points: Vec<(i64, i64)>| -> Result<(), String> {
            check_visit_cancelled(cancellation)?;
            let points = canonical_polygon(points)?;
            visit(SceneShape {
                kind,
                layer_idx: layer.index,
                layer: layer.layer,
                datatype: layer.datatype,
                cell_id: key.0,
                points,
            })
        };

        for &page_id in &cell.pages {
            let Some(page) = scene.page(page_id) else {
                continue;
            };
            if page.layer_idx != layer.index {
                continue;
            }
            if !page.bbox.intersects(&local_view) {
                continue;
            }
            let geometry = page
                .doc
                .cells
                .get(page.doc.top)
                .ok_or_else(|| format!("corrupt page {}: invalid top cell", page_id))?;
            if !geometry.places.is_empty() || !geometry.texts.is_empty() {
                return Err(format!(
                    "corrupt page {}: geometry page contains placements or text",
                    page_id
                ));
            }
            for rect in &geometry.rects {
                if rect.w <= 0 || rect.h <= 0 {
                    if rect.w < 0 || rect.h < 0 {
                        return Err(format!(
                            "corrupt page {}: negative rectangle size {}x{}",
                            page_id, rect.w, rect.h
                        ));
                    }
                    continue;
                }
                let x1 = checked_add(rect.x, rect.w, "rectangle x")?;
                let y1 = checked_add(rect.y, rect.h, "rectangle y")?;
                let base = BBox {
                    x0: rect.x,
                    y0: rect.y,
                    x1,
                    y1,
                };
                for_each_query_offset(
                    &rect.rep,
                    page.index.pts_chunks(&rect.rep),
                    base,
                    local_view,
                    member_budget,
                    cancellation,
                    |ox, oy| {
                        emit(
                            SceneShapeKind::Rectangle,
                            transform_points(
                                &world_transform,
                                &[
                                    (
                                        checked_add(base.x0, ox, "rectangle x0")?,
                                        checked_add(base.y0, oy, "rectangle y0")?,
                                    ),
                                    (
                                        checked_add(base.x1, ox, "rectangle x1")?,
                                        checked_add(base.y0, oy, "rectangle y0")?,
                                    ),
                                    (
                                        checked_add(base.x1, ox, "rectangle x1")?,
                                        checked_add(base.y1, oy, "rectangle y1")?,
                                    ),
                                    (
                                        checked_add(base.x0, ox, "rectangle x0")?,
                                        checked_add(base.y1, oy, "rectangle y1")?,
                                    ),
                                ],
                            )?,
                        )
                    },
                )?;
            }
            for polygon in &geometry.polys {
                let base = polygon_bbox(&polygon.pts).ok_or_else(|| {
                    format!(
                        "corrupt page {}: polygon has fewer than 3 vertices",
                        page_id
                    )
                })?;
                for_each_query_offset(
                    &polygon.rep,
                    page.index.pts_chunks(&polygon.rep),
                    base,
                    local_view,
                    member_budget,
                    cancellation,
                    |ox, oy| {
                        let local: Result<Vec<_>, String> = polygon
                            .pts
                            .iter()
                            .map(|&(x, y)| {
                                Ok((
                                    checked_add(x, ox, "polygon x")?,
                                    checked_add(y, oy, "polygon y")?,
                                ))
                            })
                            .collect();
                        emit(
                            SceneShapeKind::Polygon,
                            transform_points(&world_transform, &local?)?,
                        )
                    },
                )?;
            }
            for path_record in &geometry.paths {
                let outline = checked_path_outline(
                    &path_record.pts,
                    path_record.hw,
                    path_record.es,
                    path_record.ee,
                )
                .map_err(|error| format!("page {}: {}", page_id, error))?;
                let base = polygon_bbox(&outline).ok_or_else(|| {
                    format!("corrupt page {}: path outline is degenerate", page_id)
                })?;
                for_each_query_offset(
                    &path_record.rep,
                    page.index.pts_chunks(&path_record.rep),
                    base,
                    local_view,
                    member_budget,
                    cancellation,
                    |ox, oy| {
                        let local: Result<Vec<_>, String> = outline
                            .iter()
                            .map(|&(x, y)| {
                                Ok((checked_add(x, ox, "path x")?, checked_add(y, oy, "path y")?))
                            })
                            .collect();
                        emit(
                            SceneShapeKind::Polygon,
                            transform_points(&world_transform, &local?)?,
                        )
                    },
                )?;
            }
        }

        for &(wash_layer_idx, wash) in &cell.washes {
            // Zero-area washes encode OVR display points, never query shapes.
            if wash.x0 == wash.x1 && wash.y0 == wash.y1 {
                continue;
            }
            if wash_layer_idx != layer.index {
                continue;
            }
            let world = world_transform.apply_bbox(wash)?;
            if world.intersects(&view) {
                emit(SceneShapeKind::Rectangle, canonical_rect(world))?;
            }
        }
    }
    for instance in &cell.insts {
        let child_bbox = scene
            .cell_bbox(instance.child)
            .ok_or_else(|| format!("invalid plan: missing bbox for child {:?}", instance.child))?;
        if child_bbox.is_empty() {
            continue;
        }
        if !scene.subtree_paints(instance.child, layer_bit) {
            continue;
        }
        let base_place =
            OrthoTransform::place(instance.x, instance.y, instance.rot, instance.flip)?;
        let base_bbox = base_place.apply_bbox(child_bbox)?;
        for_each_query_offset(
            &instance.rep,
            None,
            base_bbox,
            local_view,
            member_budget,
            cancellation,
            |ox, oy| {
                let x = checked_add(instance.x, ox, "instance x")?;
                let y = checked_add(instance.y, oy, "instance y")?;
                let local = OrthoTransform::place(x, y, instance.rot, instance.flip)?;
                visit_cell_layer(
                    scene,
                    view,
                    layer,
                    layer_bit,
                    instance.child,
                    world_transform.compose(&local)?,
                    path,
                    member_budget,
                    cancellation,
                    visit,
                )
            },
        )?;
    }
    path.pop();
    Ok(())
}

fn check_visit_cancelled(cancellation: Option<(u64, &RenderCancellation)>) -> Result<(), String> {
    if let Some((generation, cancellation)) = cancellation {
        cancellation.check(generation)?;
    }
    Ok(())
}

fn for_each_query_offset(
    rep: &floe_oasis::doc::Rep,
    pts_chunks: Option<&[BBox]>,
    base_bbox: BBox,
    local_view: BBox,
    member_budget: Option<&Cell<usize>>,
    cancellation: Option<(u64, &RenderCancellation)>,
    visit: impl FnMut(i64, i64) -> Result<(), String>,
) -> Result<(), String> {
    for_each_visible_offset_query(
        rep,
        pts_chunks,
        base_bbox,
        local_view,
        member_budget.map(|remaining| (remaining, QUERY_MEMBER_LIMIT)),
        cancellation,
        visit,
    )?;
    Ok(())
}

fn transform_points(
    transform: &OrthoTransform,
    points: &[(i64, i64)],
) -> Result<Vec<(i64, i64)>, String> {
    points.iter().map(|&(x, y)| transform.apply(x, y)).collect()
}

fn checked_add(a: i64, b: i64, field: &str) -> Result<i64, String> {
    a.checked_add(b)
        .ok_or_else(|| format!("coordinate overflow: {field}"))
}

fn canonical_rect(bbox: BBox) -> Vec<(i64, i64)> {
    vec![
        (bbox.x0, bbox.y0),
        (bbox.x0, bbox.y1),
        (bbox.x1, bbox.y1),
        (bbox.x1, bbox.y0),
    ]
}

pub(crate) fn polygon_bbox(points: &[(i64, i64)]) -> Option<BBox> {
    let &(first_x, first_y) = points.first()?;
    let mut bbox = BBox {
        x0: first_x,
        y0: first_y,
        x1: first_x,
        y1: first_y,
    };
    for &(x, y) in &points[1..] {
        bbox.x0 = bbox.x0.min(x);
        bbox.y0 = bbox.y0.min(y);
        bbox.x1 = bbox.x1.max(x);
        bbox.y1 = bbox.y1.max(y);
    }
    (points.len() >= 3).then_some(bbox)
}

pub(crate) fn canonical_polygon(mut points: Vec<(i64, i64)>) -> Result<Vec<(i64, i64)>, String> {
    points.dedup();
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    loop {
        if points.len() < 3 {
            return Err("query polygon has fewer than 3 distinct vertices".to_string());
        }
        let mut remove = None;
        for index in 0..points.len() {
            let previous = points[(index + points.len() - 1) % points.len()];
            let current = points[index];
            let next = points[(index + 1) % points.len()];
            if cross(previous, current, next)? == 0 && between(previous, current, next)? {
                remove = Some(index);
                break;
            }
        }
        let Some(index) = remove else {
            break;
        };
        points.remove(index);
    }
    if signed_area2(&points)? > 0 {
        points.reverse();
    }
    let start = points
        .iter()
        .enumerate()
        .min_by_key(|(_, point)| **point)
        .map(|(index, _)| index)
        .unwrap_or(0);
    points.rotate_left(start);
    Ok(points)
}

fn between(a: (i64, i64), b: (i64, i64), c: (i64, i64)) -> Result<bool, String> {
    let bax = b.0 as i128 - a.0 as i128;
    let bay = b.1 as i128 - a.1 as i128;
    let bcx = b.0 as i128 - c.0 as i128;
    let bcy = b.1 as i128 - c.1 as i128;
    let x = bax
        .checked_mul(bcx)
        .ok_or_else(|| "coordinate overflow: query collinear x".to_string())?;
    let y = bay
        .checked_mul(bcy)
        .ok_or_else(|| "coordinate overflow: query collinear y".to_string())?;
    Ok(x.checked_add(y)
        .ok_or_else(|| "coordinate overflow: query collinear sum".to_string())?
        <= 0)
}

fn cross(a: (i64, i64), b: (i64, i64), c: (i64, i64)) -> Result<i128, String> {
    let bax = b.0 as i128 - a.0 as i128;
    let bay = b.1 as i128 - a.1 as i128;
    let cax = c.0 as i128 - a.0 as i128;
    let cay = c.1 as i128 - a.1 as i128;
    let first = bax
        .checked_mul(cay)
        .ok_or_else(|| "coordinate overflow: query cross first".to_string())?;
    let second = bay
        .checked_mul(cax)
        .ok_or_else(|| "coordinate overflow: query cross second".to_string())?;
    first
        .checked_sub(second)
        .ok_or_else(|| "coordinate overflow: query cross result".to_string())
}

pub(crate) fn signed_area2(points: &[(i64, i64)]) -> Result<i128, String> {
    let mut area = 0i128;
    for (&(x1, y1), &(x2, y2)) in points.iter().zip(points.iter().cycle().skip(1)) {
        let first = (x1 as i128)
            .checked_mul(y2 as i128)
            .ok_or_else(|| "coordinate overflow: query area first".to_string())?;
        let second = (x2 as i128)
            .checked_mul(y1 as i128)
            .ok_or_else(|| "coordinate overflow: query area second".to_string())?;
        let term = first
            .checked_sub(second)
            .ok_or_else(|| "coordinate overflow: query area term".to_string())?;
        area = area
            .checked_add(term)
            .ok_or_else(|| "coordinate overflow: query area sum".to_string())?;
    }
    Ok(area)
}

fn polygon_area2(points: &[(i64, i64)]) -> Result<u128, String> {
    Ok(signed_area2(points)?.unsigned_abs())
}

fn point_inclusive(points: &[(i64, i64)], x: i64, y: i64) -> Result<bool, String> {
    let point = (x, y);
    for (&a, &b) in points.iter().zip(points.iter().cycle().skip(1)) {
        if cross(a, point, b)? == 0 && between(a, point, b)? {
            return Ok(true);
        }
    }
    let mut inside = false;
    for (&a, &b) in points.iter().zip(points.iter().cycle().skip(1)) {
        if (a.1 > y) == (b.1 > y) {
            continue;
        }
        let dy = b.1 as i128 - a.1 as i128;
        let lhs = (x as i128 - a.0 as i128)
            .checked_mul(dy)
            .ok_or_else(|| "coordinate overflow: query inside lhs".to_string())?;
        let rhs = (b.0 as i128 - a.0 as i128)
            .checked_mul(y as i128 - a.1 as i128)
            .ok_or_else(|| "coordinate overflow: query inside rhs".to_string())?;
        if (dy > 0 && lhs < rhs) || (dy < 0 && lhs > rhs) {
            inside = !inside;
        }
    }
    Ok(inside)
}

fn round_ties_even_i64(value: f64) -> Result<i64, String> {
    let rounded = value.round_ties_even();
    if !rounded.is_finite() || rounded < i64::MIN as f64 || rounded > i64::MAX as f64 {
        return Err(format!("query coordinate overflow: {value}"));
    }
    Ok(rounded as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DecodedPage;
    use floe_oasis::doc::{Cell, Doc, RectRec, Rep};
    use floe_vfs::hier::{HierPlan, HierStats, WsCell, REM_FULL};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    fn page(page_id: u32, layer_idx: u32) -> Arc<DecodedPage> {
        page_with_rect(
            page_id,
            layer_idx,
            BBox {
                x0: 0,
                y0: 0,
                x1: 10,
                y1: 10,
            },
            (0, 0),
            Rep::One,
        )
    }

    fn page_with_rect(
        page_id: u32,
        layer_idx: u32,
        bbox: BBox,
        origin: (i64, i64),
        rep: Rep,
    ) -> Arc<DecodedPage> {
        let members = rep.members();
        let doc = Doc {
            unit: 1.0,
            cells: vec![Cell {
                name: format!("P{page_id}"),
                rects: vec![RectRec {
                    layer: layer_idx,
                    dt: 0,
                    x: origin.0,
                    y: origin.1,
                    w: 10,
                    h: 10,
                    rep,
                }],
                ..Cell::default()
            }],
            top: 0,
            layer_order: vec![(layer_idx, 0)],
            norm_s: 0.0,
            layer_names: HashMap::new(),
            layer_aliases: HashMap::new(),
        };
        Arc::new(DecodedPage {
            page_id,
            layer_idx,
            bbox,
            encoded_bytes: 1,
            records: 1,
            members,
            index: crate::PageIndex::build(&doc),
            doc,
            grown_bytes: 0,
        })
    }

    fn single_page_scene(page: Arc<DecodedPage>) -> FrameScene {
        let top = (0, REM_FULL);
        let page_id = page.page_id;
        let plan = HierPlan {
            top,
            wcells: vec![WsCell {
                key: top,
                pages: vec![page_id],
                page_levels: Vec::new(),
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
                dot_counts: Vec::new(),
                reps: Vec::new(),
            }],
            pages: vec![page_id],
            page_prio: vec![0],
            stats: HierStats::default(),
            explain: Vec::new(),
        };
        let mut bounds = BTreeMap::new();
        bounds.insert(top, page.bbox);
        FrameScene::from_test_parts(plan, vec![page], bounds).unwrap()
    }

    fn query_scene() -> FrameScene {
        let top = (0, REM_FULL);
        let plan = HierPlan {
            top,
            wcells: vec![WsCell {
                key: top,
                pages: vec![0, 1],
                page_levels: Vec::new(),
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
                dot_counts: Vec::new(),
                reps: Vec::new(),
            }],
            pages: vec![0, 1],
            page_prio: vec![0, 1],
            stats: HierStats::default(),
            explain: Vec::new(),
        };
        let mut bounds = BTreeMap::new();
        bounds.insert(
            top,
            BBox {
                x0: 0,
                y0: 0,
                x1: 10,
                y1: 10,
            },
        );
        FrameScene::from_test_parts(plan, vec![page(0, 0), page(1, 1)], bounds).unwrap()
    }

    fn request(x: i64, y: i64) -> SceneQueryRequest {
        SceneQueryRequest {
            x,
            y,
            radius: 2,
            // Deliberately not numeric layer order: pick must sort 5/1
            // before 10/0 for equal-area candidates.
            layers: vec![
                SceneQueryLayer {
                    index: 0,
                    layer: 10,
                    datatype: 0,
                },
                SceneQueryLayer {
                    index: 1,
                    layer: 5,
                    datatype: 1,
                },
            ],
            shape_cap: 400,
            member_cap: 400,
            seen: None,
        }
    }

    #[test]
    fn canonical_hull_matches_klayout_order_and_drops_collinear_points() {
        assert_eq!(
            canonical_polygon(vec![(5, 0), (0, 0), (2, 3)]).unwrap(),
            vec![(0, 0), (2, 3), (5, 0)]
        );
        assert_eq!(
            canonical_polygon(vec![(0, 0), (5, 0), (10, 0), (10, 10), (0, 10)]).unwrap(),
            vec![(0, 0), (0, 10), (10, 10), (10, 0)]
        );
    }

    #[test]
    fn inside_is_boundary_inclusive_and_area_matches_integer_klayout() {
        let square = canonical_rect(BBox {
            x0: 0,
            y0: 0,
            x1: 10,
            y1: 10,
        });
        for point in [(0, 0), (0, 5), (5, 5), (10, 5), (10, 10)] {
            assert!(point_inclusive(&square, point.0, point.1).unwrap());
        }
        assert!(!point_inclusive(&square, -1, 5).unwrap());
        assert_eq!(polygon_area2(&[(0, 0), (1, 0), (0, 1)]).unwrap() / 2, 0);
        assert_eq!(polygon_area2(&square).unwrap() / 2, 100);
    }

    #[test]
    fn extreme_coordinates_fail_instead_of_wrapping() {
        let extreme = vec![
            (i64::MIN, i64::MIN),
            (i64::MIN, i64::MAX),
            (i64::MAX, i64::MAX),
            (i64::MAX, i64::MIN),
        ];
        assert!(canonical_polygon(extreme).unwrap_err().contains("overflow"));
    }

    #[test]
    fn edge_rounding_uses_python_ties_to_even() {
        assert_eq!(round_ties_even_i64(2.5).unwrap(), 2);
        assert_eq!(round_ties_even_i64(3.5).unwrap(), 4);
        assert_eq!(round_ties_even_i64(-2.5).unwrap(), -2);
    }

    #[test]
    fn scene_snap_prefers_any_vertex_then_uses_edges() {
        let scene = query_scene();
        let vertex = snap_scene(&scene, &request(1, 1)).unwrap().unwrap();
        assert_eq!(
            (vertex.x, vertex.y, vertex.kind),
            (0, 0, SceneSnapKind::Vertex)
        );

        let edge = snap_scene(&scene, &request(5, 1)).unwrap().unwrap();
        assert_eq!((edge.x, edge.y, edge.kind), (5, 0, SceneSnapKind::Edge));
        assert_eq!(edge.shapes_tested, 2);
    }

    #[test]
    fn cancellable_shape_visit_rejects_a_stale_clip_generation() {
        let scene = query_scene();
        let cancellation = RenderCancellation::new();
        cancellation.cancel_before(2);
        let mut visits = 0;
        let error = visit_scene_shapes_cancellable(
            &scene,
            BBox {
                x0: 0,
                y0: 0,
                x1: 10,
                y1: 10,
            },
            &request(5, 5).layers,
            1,
            &cancellation,
            |_| {
                visits += 1;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.contains("render cancelled"), "{error}");
        assert_eq!(visits, 0);
    }

    #[test]
    fn scene_pick_is_boundary_inclusive_and_sorts_by_layer_numbers() {
        let scene = query_scene();
        let first = pick_scene(&scene, &request(10, 5), 0).unwrap();
        // §F2R-20b: a superseded pick aborts before touching members;
        // the current one answers exactly like the plain entry point
        let frontier = RenderCancellation::new();
        frontier.cancel_before(2);
        let stale = pick_scene_cancellable(&scene, &request(10, 5), 0, 1, &frontier).unwrap_err();
        assert!(stale.contains("cancelled"), "{stale}");
        let current = pick_scene_cancellable(&scene, &request(10, 5), 0, 2, &frontier).unwrap();
        assert_eq!(current.count, first.count);
        assert_eq!(current.candidate, first.candidate);
        assert!(snap_scene_cancellable(&scene, &request(1, 1), 1, &frontier)
            .unwrap_err()
            .contains("cancelled"));
        assert_eq!((first.count, first.index), (2, 0));
        let first = first.candidate.unwrap();
        assert_eq!((first.layer_idx, first.cell_id, first.area), (1, 0, 100.0));
        assert_eq!(first.points, vec![(0, 0), (0, 10), (10, 10), (10, 0)]);

        let second = pick_scene(&scene, &request(10, 5), 3).unwrap();
        assert_eq!((second.count, second.index), (2, 1));
        assert_eq!(second.candidate.unwrap().layer_idx, 0);
    }

    #[test]
    fn pick_cap_keeps_the_smallest_candidates_not_the_first_met() {
        let scene = query_scene();
        let mut capped = request(5, 5);
        capped.shape_cap = 1;
        let picked = pick_scene(&scene, &capped, 0).unwrap();
        // Both shapes are weighed, and of two as large the numerically
        // lower layer is kept - the walk meets it second. (The parent
        // KLayout loop stopped at the cap and never saw it.)
        assert_eq!((picked.count, picked.shapes_tested), (1, 2));
        assert_eq!(picked.candidate.unwrap().layer_idx, 1);
    }

    /// One layer a rectangle `[x0, y0, x1, y1]`, in the order given: layer
    /// index n is layer number n + 1.
    fn stack(rects: &[[i64; 4]]) -> (FrameScene, SceneQueryRequest) {
        let top = (0, REM_FULL);
        let mut pages = Vec::new();
        let mut around = BBox {
            x0: 0,
            y0: 0,
            x1: 1,
            y1: 1,
        };
        for (n, r) in rects.iter().enumerate() {
            let bbox = BBox {
                x0: r[0],
                y0: r[1],
                x1: r[2],
                y1: r[3],
            };
            around = BBox {
                x0: around.x0.min(bbox.x0),
                y0: around.y0.min(bbox.y0),
                x1: around.x1.max(bbox.x1),
                y1: around.y1.max(bbox.y1),
            };
            let doc = Doc {
                unit: 1.0,
                cells: vec![Cell {
                    name: format!("P{n}"),
                    rects: vec![RectRec {
                        layer: n as u32,
                        dt: 0,
                        x: r[0],
                        y: r[1],
                        w: r[2] - r[0],
                        h: r[3] - r[1],
                        rep: Rep::One,
                    }],
                    ..Cell::default()
                }],
                top: 0,
                layer_order: vec![(n as u32, 0)],
                norm_s: 0.0,
                layer_names: HashMap::new(),
                layer_aliases: HashMap::new(),
            };
            pages.push(Arc::new(DecodedPage {
                page_id: n as u32,
                layer_idx: n as u32,
                bbox,
                encoded_bytes: 1,
                records: 1,
                members: 1,
                index: crate::PageIndex::build(&doc),
                doc,
                grown_bytes: 0,
            }));
        }
        let ids: Vec<u32> = (0..rects.len() as u32).collect();
        let plan = HierPlan {
            top,
            wcells: vec![WsCell {
                key: top,
                pages: ids.clone(),
                page_levels: Vec::new(),
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
                dot_counts: Vec::new(),
                reps: Vec::new(),
            }],
            page_prio: vec![0; ids.len()],
            pages: ids.clone(),
            stats: HierStats::default(),
            explain: Vec::new(),
        };
        let mut bounds = BTreeMap::new();
        bounds.insert(top, around);
        let scene = FrameScene::from_test_parts(plan, pages, bounds).unwrap();
        let request = SceneQueryRequest {
            x: 0,
            y: 0,
            radius: 2,
            layers: ids
                .iter()
                .map(|&index| SceneQueryLayer {
                    index,
                    layer: index + 1,
                    datatype: 0,
                })
                .collect(),
            shape_cap: 64,
            member_cap: 4096,
            seen: None,
        };
        (scene, request)
    }

    #[test]
    fn pick_cycles_from_the_smallest_of_every_shape_over_the_point() {
        // Plates on the first layers, the small shapes on the last ones: the
        // walk meets the plates first, and more of them than a pick keeps.
        let mut rects: Vec<[i64; 4]> = (0..70).map(|n| [-900 - n, -900, 900, 900]).collect();
        rects.extend([[-30, -30, 30, 30], [-10, -10, 10, 10], [-2, -2, 2, 2]]);
        let (scene, request) = stack(&rects);
        let picks: Vec<ScenePick> = (0..65)
            .map(|nth| pick_scene(&scene, &request, nth).unwrap())
            .collect();
        assert!(picks
            .iter()
            .all(|p| (p.count, p.shapes_tested, p.hidden) == (64, 73, 0)));
        let layer = |p: &ScenePick| p.candidate.as_ref().unwrap().layer_idx;
        let area = |p: &ScenePick| p.candidate.as_ref().unwrap().area;
        // the smallest first, then the next ones; the plates after them
        // from the narrowest, and the cycle ends where it began
        assert_eq!(
            picks[..4].iter().map(layer).collect::<Vec<_>>(),
            [72, 71, 70, 0]
        );
        assert_eq!(
            picks[..3].iter().map(area).collect::<Vec<_>>(),
            [16., 400., 3600.]
        );
        assert_eq!(
            picks.iter().map(|p| p.index).collect::<Vec<_>>(),
            (0..64).chain([0]).collect::<Vec<_>>()
        );
        assert!(picks.windows(2).take(63).all(|w| area(&w[0]) <= area(&w[1])));
        assert_eq!(layer(&picks[63]), 60);
        assert_eq!(layer(&picks[64]), 72);
        assert_eq!(pick_scene(&scene, &request, -1).unwrap().index, 63);
        // of two as large the lower layer number, whichever the walk met
        let (scene, mut request) = stack(&[[-5, -5, 5, 5], [-5, -5, 5, 5], [-5, -5, 5, 5]]);
        request.layers[0].layer = 9;
        let order: Vec<u32> = (0..3)
            .map(|nth| layer(&pick_scene(&scene, &request, nth).unwrap()))
            .collect();
        assert_eq!(order, [1, 2, 0]);
    }

    /// A frame of the `stack` scene, a dbu a pixel: `fills` by layer index,
    /// in paint order (the last on top), each a colour of its own.
    fn frame_of(fills: &[(u32, crate::LayerFill)]) -> crate::StyledGeometryRasterRequest {
        crate::StyledGeometryRasterRequest {
            raster: crate::GeometryRasterRequest {
                view: crate::RasterViewBox::new(-256.0, -256.0, 256.0, 256.0).unwrap(),
                width: 512,
                height: 512,
                background: [0, 0, 0, 255],
                foreground: [255, 255, 255, 255],
                workers: 1,
                tile_size: crate::DEFAULT_TILE_SIZE,
                area_true: true,
                width_c: 1.0,
                survivor_list: true,
                place_lattice: false,
                density_stack: false,
                density_claim_lit: false,
                density_top_planes: 1,
                density_shapes_first: false,
                density_bright: 0.0,
                density_pattern: false,
            },
            layers: fills
                .iter()
                .map(|&(layer_idx, fill)| crate::LayerStyle {
                    layer_idx,
                    color: [40 + layer_idx as u8 * 30, 200, 255 - layer_idx as u8 * 30, 255],
                    fill,
                    outline_width: 1,
                })
                .collect(),
            hierarchy_frames: false,
            mono: false,
        }
    }

    #[test]
    fn pick_takes_the_shapes_that_show() {
        use crate::LayerFill::{Clear, Solid, Speckle};
        // from the bottom: two plates, a solid square, a small square on it,
        // and over them all a large square that is its outline alone
        let rects = [
            [-200, -200, 200, 200],
            [-200, -200, 200, 200],
            [-40, -40, 40, 40],
            [-10, -10, 10, 10],
            [-150, -150, 150, 150],
        ];
        let (scene, base) = stack(&rects);
        let fills = [(0, Speckle), (1, Speckle), (2, Solid), (3, Speckle), (4, Clear)];
        let picks = |styled: &crate::StyledGeometryRasterRequest, x: i64, y: i64, seen: bool| {
            let mut request = SceneQueryRequest { x, y, ..base.clone() };
            request
                .layers
                .retain(|layer| styled.layers.iter().any(|style| style.layer_idx == layer.index));
            if seen {
                let around = crate::seen_planes_around(&scene, styled, (x, y), crate::SEEN_REACH_PX, None)
                    .unwrap()
                    .unwrap();
                request.seen = Some(Arc::new(around));
            }
            let first = pick_scene(&scene, &request, 0).unwrap();
            let order: Vec<u32> = (0..first.count as i64)
                .map(|nth| {
                    pick_scene(&scene, &request, nth)
                        .unwrap()
                        .candidate
                        .unwrap()
                        .layer_idx
                })
                .collect();
            (order, first.hidden)
        };
        let all = frame_of(&fills);
        // every shape over the point, the smallest first: the rule without
        // the frame (of two plates as large the lower layer number)
        assert_eq!(picks(&all, 0, 0, false), (vec![3, 2, 4, 0, 1], 0));
        // at the middle the small square shows, and between its speckles
        // the solid square under it; the outline of the large square is far
        // away and the plates are under the solid one
        assert_eq!(picks(&all, 0, 0, true), (vec![3, 2], 3));
        // on the solid square beside the small one: it alone
        assert_eq!(picks(&all, 30, 0, true), (vec![2], 3));
        // off the solid square: the upper plate - the lower one has the
        // same speckle and lights no pixel the upper one left
        assert_eq!(picks(&all, 100, 0, true), (vec![1], 2));
        // on the large square's outline: it, and the plate around it
        assert_eq!(picks(&all, 150, 0, true), (vec![4, 1], 1));
        assert_eq!(picks(&all, 149, 60, true), (vec![4, 1], 1));
        // a few pixels inside the outline it is still in the window
        assert_eq!(picks(&all, 144, 0, true), (vec![4, 1], 1));
        assert_eq!(picks(&all, 130, 0, true), (vec![1], 2));
        // the upper plate off: the lower one shows
        let lower = frame_of(&[fills[0], fills[2], fills[3], fills[4]]);
        assert_eq!(picks(&lower, 100, 0, true), (vec![0], 1));
        // the small square solid: nothing of the square under it shows at
        // its middle; near its edge the square under it does
        let solid = frame_of(&[fills[0], fills[1], fills[2], (3, Solid), fills[4]]);
        assert_eq!(picks(&solid, 0, 0, true), (vec![3], 4));
        assert_eq!(picks(&solid, 8, 0, true), (vec![3, 2], 3));
        // the large square filled: it is over everything
        let over = frame_of(&[fills[0], fills[1], fills[2], fills[3], (4, Solid)]);
        assert_eq!(picks(&over, 0, 0, true), (vec![4], 4));
        // those that do not show take no place among the candidates kept
        let mut capped = SceneQueryRequest { x: 0, y: 0, shape_cap: 1, ..base.clone() };
        capped.seen = Some(Arc::new(
            crate::seen_planes_around(&scene, &all, (0, 0), crate::SEEN_REACH_PX, None)
                .unwrap()
                .unwrap(),
        ));
        let one = pick_scene(&scene, &capped, 0).unwrap();
        assert_eq!((one.count, one.hidden), (1, 3));
        assert_eq!(one.candidate.unwrap().layer_idx, 3);
        // nothing shows: nothing is picked, and the holders are counted
        let bare = frame_of(&[(4, Clear)]);
        let none = picks(&bare, 0, 0, true);
        assert_eq!(none, (vec![], 1));
    }

    #[test]
    fn snap_passes_by_what_does_not_show() {
        use crate::LayerFill::{Clear, Solid, Speckle};
        // from the bottom: a plate, a small square, a solid square over it
        // that reaches further right, and a large outline over them all
        let rects = [
            [-200, -200, 200, 200],
            [-10, -10, 10, 10],
            [-40, -40, 60, 40],
            [-150, -150, 150, 150],
        ];
        let (scene, base) = stack(&rects);
        let snap = |styled: &crate::StyledGeometryRasterRequest, x: i64, y: i64, seen: bool| {
            let mut request = SceneQueryRequest {
                x,
                y,
                radius: 6,
                ..base.clone()
            };
            request
                .layers
                .retain(|layer| styled.layers.iter().any(|style| style.layer_idx == layer.index));
            if seen {
                request.seen = Some(Arc::new(
                    crate::seen_planes_around(&scene, styled, (x, y), 6 + 3, None)
                        .unwrap()
                        .unwrap(),
                ));
            }
            snap_scene(&scene, &request)
                .unwrap()
                .map(|hit| (hit.x, hit.y, hit.kind))
        };
        let covered = frame_of(&[(0, Speckle), (1, Speckle), (2, Solid), (3, Clear)]);
        // the small square's corner is under the solid square: every shape's
        // corners are snapped to, the ones that show alone are not
        assert_eq!(snap(&covered, 12, 12, false), Some((10, 10, SceneSnapKind::Vertex)));
        assert_eq!(snap(&covered, 12, 12, true), None);
        // its edge likewise
        assert_eq!(snap(&covered, 0, 12, false), Some((0, 10, SceneSnapKind::Edge)));
        assert_eq!(snap(&covered, 0, 12, true), None);
        // the solid square's own corner and edge show
        assert_eq!(snap(&covered, 58, 38, true), Some((60, 40, SceneSnapKind::Vertex)));
        assert_eq!(snap(&covered, 20, 38, true), Some((20, 40, SceneSnapKind::Edge)));
        // of two corners in reach the nearer one that shows: the covered
        // corner at (10, 10) is nearer to (13, 13) than nothing else is,
        // and with the solid square off it shows again
        let open = frame_of(&[(0, Speckle), (1, Speckle), (3, Clear)]);
        assert_eq!(snap(&open, 12, 12, true), Some((10, 10, SceneSnapKind::Vertex)));
        assert_eq!(snap(&open, 0, 12, true), Some((0, 10, SceneSnapKind::Edge)));
        // a speckle above leaves every other pixel of an outline: the small
        // square under a speckled one is still snapped to
        let speckled = frame_of(&[(0, Speckle), (1, Speckle), (2, Speckle), (3, Clear)]);
        assert_eq!(snap(&speckled, 12, 12, true), Some((10, 10, SceneSnapKind::Vertex)));
        assert_eq!(snap(&speckled, 0, 12, true), Some((0, 10, SceneSnapKind::Edge)));
        // the outline over everything shows, and the plate's corner under
        // nothing does
        assert_eq!(snap(&covered, 148, 148, true), Some((150, 150, SceneSnapKind::Vertex)));
        assert_eq!(snap(&covered, 198, 198, true), Some((200, 200, SceneSnapKind::Vertex)));
        // a covered corner nearer than one that shows: the one that shows,
        // where every shape gives the covered one
        let near = [[-200, -200, 200, 200], [-10, -10, 10, 10], [-40, -40, 14, 14]];
        let (scene, base) = stack(&near);
        let request = |seen: bool| {
            let styled = frame_of(&[(0, Speckle), (1, Speckle), (2, Solid)]);
            let mut request = SceneQueryRequest {
                x: 12,
                y: 12,
                radius: 6,
                ..base.clone()
            };
            if seen {
                request.seen = Some(Arc::new(
                    crate::seen_planes_around(&scene, &styled, (12, 12), 9, None)
                        .unwrap()
                        .unwrap(),
                ));
            }
            snap_scene(&scene, &request)
                .unwrap()
                .map(|hit| (hit.x, hit.y, hit.kind))
        };
        assert_eq!(request(false), Some((10, 10, SceneSnapKind::Vertex)));
        assert_eq!(request(true), Some((14, 14, SceneSnapKind::Vertex)));
    }

    #[test]
    fn pick_member_budget_charges_only_visible_members() {
        // §3.19: members outside the query region are free. Charging
        // every tested member let dense pages starve the shared budget
        // before the walk reached the clicked shape (field: bars
        // unpickable with all layers on, pickable with three).
        let scene = single_page_scene(page_with_rect(
            0,
            0,
            BBox {
                x0: 0,
                y0: 0,
                x1: 510,
                y1: 510,
            },
            (0, 0),
            Rep::Pts(Arc::from([(100, 100), (200, 200), (500, 500)])),
        ));
        let mut capped = request(505, 505);
        capped.member_cap = 1;
        let found = pick_scene(&scene, &capped, 0).unwrap();
        assert_eq!((found.count, found.shapes_tested), (1, 1));
    }

    #[test]
    fn member_limit_exhaustion_is_an_error_not_an_empty_result() {
        // §3.19: the safety valve must be VISIBLE - a silent empty
        // result reads as "no object here" in the GUI and hides the
        // defect class this section documents.
        let scene = single_page_scene(page_with_rect(
            0,
            0,
            BBox {
                x0: 0,
                y0: 0,
                x1: 20,
                y1: 20,
            },
            (0, 0),
            Rep::Pts(Arc::from([(0, 0), (1, 1), (2, 2)])),
        ));
        let mut capped = request(5, 5);
        capped.member_cap = 2;
        let error = pick_scene(&scene, &capped, 0).unwrap_err();
        assert!(error.contains("member limit"), "{error}");
    }

    #[test]
    fn dense_far_layer_does_not_starve_the_pick_budget() {
        // Walk order visits the dense layer first: its 600 members sit
        // far from the click, so under visible-only charging the
        // second layer's bar at the click must still be found with the
        // default 400 budget (the old tested-member charge starved the
        // whole query on the first layer).
        let far: Vec<(i64, i64)> = (0..600).map(|index| (index * 3, 0)).collect();
        let top = (0, REM_FULL);
        let dense = page_with_rect(
            0,
            0,
            BBox {
                x0: 0,
                y0: 0,
                x1: 1810,
                y1: 10,
            },
            (0, 0),
            Rep::Pts(Arc::from(far.as_slice())),
        );
        let bar = page_with_rect(
            1,
            1,
            BBox {
                x0: 1000,
                y0: 500,
                x1: 1010,
                y1: 510,
            },
            (1000, 500),
            Rep::One,
        );
        let plan = HierPlan {
            top,
            wcells: vec![WsCell {
                key: top,
                pages: vec![0, 1],
                page_levels: Vec::new(),
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
                dot_counts: Vec::new(),
                reps: Vec::new(),
            }],
            pages: vec![0, 1],
            page_prio: vec![0, 1],
            stats: HierStats::default(),
            explain: Vec::new(),
        };
        let mut bounds = BTreeMap::new();
        bounds.insert(
            top,
            BBox {
                x0: 0,
                y0: 0,
                x1: 1810,
                y1: 510,
            },
        );
        let scene = FrameScene::from_test_parts(plan, vec![dense, bar], bounds).unwrap();
        let query = request(1005, 505);
        assert_eq!(query.member_cap, 400, "test rides the product default");
        let found = pick_scene(&scene, &query, 0).unwrap();
        let candidate = found.candidate.expect("bar must be reachable");
        assert_eq!(candidate.layer_idx, 1);
        assert_eq!(found.count, 1);
    }

    #[test]
    fn page_bbox_prunes_before_repetition_validation() {
        let scene = single_page_scene(page_with_rect(
            0,
            0,
            BBox {
                x0: 1000,
                y0: 1000,
                x1: 2020,
                y1: 2020,
            },
            (1000, 1000),
            Rep::Grid {
                na: 1 << 31,
                nb: 1 << 31,
                va: (1, 1),
                vb: (2, 2),
            },
        ));
        let picked = pick_scene(&scene, &request(0, 0), 0).unwrap();
        assert_eq!((picked.count, picked.shapes_tested), (0, 0));
    }
}
