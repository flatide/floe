use crate::raster::checked_path_outline;
use crate::repetition::{
    for_each_visible_offset, for_each_visible_offset_bounded, for_each_visible_offset_cancellable,
};
use crate::scene::FrameScene;
use crate::transform::OrthoTransform;
use crate::RenderCancellation;
use floe_ovm::BBox;
use floe_vfs::hier::WsKey;
use std::cell::Cell;
use std::collections::BTreeSet;

const QUERY_STOP: &str = "__floe_query_cap__";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneQueryRequest {
    pub x: i64,
    pub y: i64,
    pub radius: i64,
    /// Stable OVM layer order. Snap ties retain this traversal order while
    /// pick candidates are sorted by the numeric layer/datatype pair.
    pub layers: Vec<SceneQueryLayer>,
    /// Snap: maximum shapes examined. Pick: maximum containing candidates.
    pub shape_cap: usize,
    /// Maximum repetition members examined, including members outside the
    /// query box. This bounds sparse explicit-point repetitions.
    pub member_cap: usize,
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScenePick {
    pub count: usize,
    pub index: usize,
    pub candidate: Option<ScenePickCandidate>,
    pub shapes_tested: usize,
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
    request.validate()?;
    let radius2 = request.radius as f64 * request.radius as f64;
    let mut best_vertex = None::<(f64, i64, i64)>;
    let mut best_edge = None::<(f64, f64, f64)>;
    let mut tested = 0usize;
    visit_query_shapes(scene, request, |shape| {
        if tested >= request.shape_cap {
            return Err(QUERY_STOP.to_string());
        }
        tested += 1;
        for &(x, y) in &shape.points {
            let dx = (x as i128 - request.x as i128) as f64;
            let dy = (y as i128 - request.y as i128) as f64;
            let distance2 = dx * dx + dy * dy;
            if distance2 <= radius2 && best_vertex.is_none_or(|current| distance2 < current.0) {
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
            if distance2 <= radius2 && best_edge.is_none_or(|current| distance2 < current.0) {
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
    request.validate()?;
    let mut candidates = Vec::<(f64, u32, u32, usize, ScenePickCandidate)>::new();
    let mut tested = 0usize;
    visit_query_shapes(scene, request, |shape| {
        tested += 1;
        if point_inclusive(&shape.points, request.x, request.y)? {
            let area2 = polygon_area2(&shape.points)?;
            let area = (area2 / 2) as f64;
            candidates.push((
                area,
                shape.layer,
                shape.datatype,
                candidates.len(),
                ScenePickCandidate {
                    layer_idx: shape.layer_idx,
                    cell_id: shape.cell_id,
                    area,
                    bbox: polygon_bbox(&shape.points).expect("query polygon has a bbox"),
                    points: shape.points.into_iter().take(512).collect(),
                },
            ));
            if candidates.len() >= request.shape_cap {
                return Err(QUERY_STOP.to_string());
            }
        }
        Ok(())
    })?;
    // Python's sort is stable. The source ordinal makes that stability
    // explicit and independent of the standard library sort implementation.
    candidates.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.3.cmp(&right.3))
    });
    if candidates.is_empty() {
        return Ok(ScenePick {
            count: 0,
            index: 0,
            candidate: None,
            shapes_tested: tested,
        });
    }
    let count = candidates.len();
    let index = nth.rem_euclid(count as i64) as usize;
    let candidate = candidates.swap_remove(index).4;
    Ok(ScenePick {
        count,
        index,
        candidate: Some(candidate),
        shapes_tested: tested,
    })
}

fn visit_query_shapes(
    scene: &FrameScene,
    request: &SceneQueryRequest,
    visit: impl FnMut(SceneShape) -> Result<(), String>,
) -> Result<(), String> {
    let member_budget = Cell::new(request.member_cap);
    match visit_scene_shapes_impl(
        scene,
        request.view(),
        &request.layers,
        Some(&member_budget),
        None,
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
        visit_cell_layer(
            scene,
            view,
            layer,
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
        let base_place =
            OrthoTransform::place(instance.x, instance.y, instance.rot, instance.flip)?;
        let base_bbox = base_place.apply_bbox(child_bbox)?;
        for_each_query_offset(
            &instance.rep,
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
    base_bbox: BBox,
    local_view: BBox,
    member_budget: Option<&Cell<usize>>,
    cancellation: Option<(u64, &RenderCancellation)>,
    visit: impl FnMut(i64, i64) -> Result<(), String>,
) -> Result<(), String> {
    match (member_budget, cancellation) {
        (Some(remaining), _) => {
            for_each_visible_offset_bounded(
                rep, base_bbox, local_view, remaining, QUERY_STOP, visit,
            )?;
        }
        (None, Some((generation, cancellation))) => {
            for_each_visible_offset_cancellable(
                rep,
                base_bbox,
                local_view,
                generation,
                cancellation,
                visit,
            )?;
        }
        (None, None) => {
            for_each_visible_offset(rep, base_bbox, local_view, visit)?;
        }
    }
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
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
            }],
            pages: vec![page_id],
            page_prio: vec![0],
            stats: HierStats::default(),
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
                insts: Vec::new(),
                frames: Vec::new(),
                washes: Vec::new(),
            }],
            pages: vec![0, 1],
            page_prio: vec![0, 1],
            stats: HierStats::default(),
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
        assert_eq!((first.count, first.index), (2, 0));
        let first = first.candidate.unwrap();
        assert_eq!((first.layer_idx, first.cell_id, first.area), (1, 0, 100.0));
        assert_eq!(first.points, vec![(0, 0), (0, 10), (10, 10), (10, 0)]);

        let second = pick_scene(&scene, &request(10, 5), 3).unwrap();
        assert_eq!((second.count, second.index), (2, 1));
        assert_eq!(second.candidate.unwrap().layer_idx, 0);
    }

    #[test]
    fn pick_cap_counts_candidates_not_examined_shapes() {
        let scene = query_scene();
        let mut capped = request(5, 5);
        capped.shape_cap = 1;
        let picked = pick_scene(&scene, &capped, 0).unwrap();
        assert_eq!((picked.count, picked.shapes_tested), (1, 1));
        // Parent KLayout stops as soon as the cap is reached, before it can
        // discover the later numerically lower layer.
        assert_eq!(picked.candidate.unwrap().layer_idx, 0);
    }

    #[test]
    fn pick_member_cap_counts_non_visible_explicit_offsets() {
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
        capped.member_cap = 2;
        let missed = pick_scene(&scene, &capped, 0).unwrap();
        assert_eq!((missed.count, missed.shapes_tested), (0, 0));

        capped.member_cap = 3;
        let found = pick_scene(&scene, &capped, 0).unwrap();
        assert_eq!((found.count, found.shapes_tested), (1, 1));
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
