//! Drag sketch vertices and line segments in the viewport while satisfying active constraints.

use crate::constraints::{
    constraint_evaluated_angle, constraint_evaluated_length, find_distance_constraint,
    solve_document_constraints_with_pins,
};
use crate::model::RectEdge;
use crate::construction::point_sketch;
use crate::geometric_constraints::{
    line_direction_uv, line_uv_endpoints, point_uv, set_line_uv_endpoints, set_point_uv,
    translate_line,
};
use crate::hierarchy::SceneElement;
use crate::model::{
    ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, DistanceTarget, Document,
    LineEnd, SketchId,
};
use std::collections::HashMap;

#[derive(Clone)]
pub struct LineDragSession {
    pub target: ConstraintLine,
    pub anchor_uv: (f32, f32),
    pub initial_positions: HashMap<ConstraintPoint, (f32, f32)>,
}

pub fn point_in_sketch(doc: &Document, point: ConstraintPoint, sketch: SketchId) -> bool {
    point_sketch(doc, point) == Some(sketch)
}

pub fn scene_element_for_point(point: ConstraintPoint) -> SceneElement {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => SceneElement::Line(line),
        ConstraintPoint::RectCorner { rect, .. } => SceneElement::Rect(rect),
        ConstraintPoint::CircleCenter(circle) => SceneElement::Circle(circle),
    }
}

pub fn scene_element_for_line(line: ConstraintLine) -> SceneElement {
    match line {
        ConstraintLine::Line(index) => SceneElement::Line(index),
        ConstraintLine::RectEdge { rect, edge } => SceneElement::RectEdge(rect, edge),
    }
}

pub fn line_drag_seed_points(line: ConstraintLine) -> [ConstraintPoint; 2] {
    match line {
        ConstraintLine::Line(index) => [
            ConstraintPoint::LineEndpoint {
                line: index,
                end: LineEnd::Start,
            },
            ConstraintPoint::LineEndpoint {
                line: index,
                end: LineEnd::End,
            },
        ],
        ConstraintLine::RectEdge { rect, edge } => {
            let (c0, c1) = edge.corner_indices();
            [
                ConstraintPoint::RectCorner { rect, corner: c0 },
                ConstraintPoint::RectCorner { rect, corner: c1 },
            ]
        }
    }
}

/// Whether a sketch vertex may be dragged (fully constrained vertices are blocked).
pub fn can_drag_point(doc: &Document, sketch: SketchId, point: ConstraintPoint) -> bool {
    if !point_in_sketch(doc, point, sketch) {
        return false;
    }
    if let ConstraintPoint::LineEndpoint { line, .. } = point {
        return !crate::sketch_solver::sketch_line_vertex_drag_blocked(doc, sketch, line)
            .unwrap_or(false);
    }
    if let ConstraintPoint::RectCorner { rect, .. } = point {
        return !rect_corner_drag_blocked(doc, rect);
    }
    true
}

/// Whether a sketch line may be translated by dragging (fully constrained lines are blocked).
pub fn can_drag_line(doc: &Document, sketch: SketchId, target: ConstraintLine) -> bool {
    match target {
        ConstraintLine::Line(line) => {
            point_in_sketch(doc, line_drag_seed_points(target)[0], sketch)
                && !crate::sketch_solver::sketch_line_vertex_drag_blocked(doc, sketch, line)
                    .unwrap_or(false)
        }
        ConstraintLine::RectEdge { rect, edge } => {
            if !doc
                .rects
                .get(rect)
                .is_some_and(|rect_entity| rect_entity.sketch == sketch)
            {
                return false;
            }
            !rect_edge_drag_blocked(doc, rect, edge)
        }
    }
}

pub fn begin_line_drag_session(
    doc: &Document,
    sketch: SketchId,
    target: ConstraintLine,
    anchor_uv: (f32, f32),
) -> Result<LineDragSession, String> {
    validate_line_drag_target(doc, sketch, target)?;
    let initial_positions = collect_line_drag_positions(doc, sketch, target)?;
    Ok(LineDragSession {
        target,
        anchor_uv,
        initial_positions,
    })
}

pub fn drag_line(
    doc: &mut Document,
    sketch: SketchId,
    session: &LineDragSession,
    current_uv: (f32, f32),
) -> Result<(), String> {
    validate_line_drag_target(doc, sketch, session.target)?;
    let mut du = current_uv.0 - session.anchor_uv.0;
    let mut dv = current_uv.1 - session.anchor_uv.1;
    if let ConstraintLine::RectEdge { rect, edge } = session.target {
        let (locked_w, locked_h) = rect_locked_dimensions(doc, rect);
        if locked_w.is_some() && matches!(edge, RectEdge::Left | RectEdge::Right) {
            du = 0.0;
        }
        if locked_h.is_some() && matches!(edge, RectEdge::Bottom | RectEdge::Top) {
            dv = 0.0;
        }
    }
    let seeds = line_drag_seed_points(session.target);
    if let ConstraintLine::RectEdge { .. } = session.target {
        let [start, end] = seeds;
        let (iu0, iv0) = session.initial_positions[&start];
        let (iu1, iv1) = session.initial_positions[&end];
        let (u0, v0) = project_drag_uv(doc, start, iu0 + du, iv0 + dv)?;
        let (u1, v1) = project_drag_uv(doc, end, iu1 + du, iv1 + dv)?;
        set_line_uv_endpoints(doc, session.target, (u0, v0), (u1, v1))?;
        for (point, (iu, iv)) in &session.initial_positions {
            if seeds.contains(point) || !point_in_sketch(doc, *point, sketch) {
                continue;
            }
            let (pu, pv) = project_drag_uv(doc, *point, iu + du, iv + dv)?;
            set_point_uv(doc, *point, pu, pv)?;
        }
    } else {
        translate_line(doc, session.target, du, dv)?;
        for (point, (iu, iv)) in &session.initial_positions {
            if seeds.contains(point) || !point_in_sketch(doc, *point, sketch) {
                continue;
            }
            set_point_uv(doc, *point, iu + du, iv + dv)?;
        }
    }
    let pins: Vec<(ConstraintPoint, (f32, f32))> = session
        .initial_positions
        .iter()
        .filter(|(point, _)| point_in_sketch(doc, **point, sketch))
        .map(|(point, (iu, iv))| {
            let projected = project_drag_uv(doc, *point, iu + du, iv + dv)
                .unwrap_or((iu + du, iv + dv));
            (*point, projected)
        })
        .collect();
    solve_document_constraints_with_pins(doc, &pins)
}

fn validate_line_drag_target(
    doc: &Document,
    sketch: SketchId,
    target: ConstraintLine,
) -> Result<(), String> {
    match target {
        ConstraintLine::Line(index) => {
            let line = doc
                .lines
                .get(index)
                .ok_or_else(|| format!("Line {index} not found"))?;
            if line.sketch != sketch {
                return Err(format!("Line {index} is not in sketch {sketch}"));
            }
        }
        ConstraintLine::RectEdge { rect, edge: _ } => {
            let rect_entity = doc
                .rects
                .get(rect)
                .ok_or_else(|| format!("Rectangle {rect} not found"))?;
            if rect_entity.sketch != sketch {
                return Err(format!("Rectangle {rect} is not in sketch {sketch}"));
            }
        }
    }
    Ok(())
}

fn collect_line_drag_positions(
    doc: &Document,
    sketch: SketchId,
    target: ConstraintLine,
) -> Result<HashMap<ConstraintPoint, (f32, f32)>, String> {
    let mut points = Vec::new();
    for seed in line_drag_seed_points(target) {
        points.extend(coincident_group(doc, sketch, seed));
    }
    points.sort_by_key(|point| constraint_point_sort_key(*point));
    points.dedup();

    let mut initial_positions = HashMap::new();
    for point in points {
        let uv = point_uv(doc, point)?;
        initial_positions.insert(point, uv);
    }
    Ok(initial_positions)
}

fn constraint_point_sort_key(point: ConstraintPoint) -> (u8, usize, u8, u8) {
    match point {
        ConstraintPoint::LineEndpoint { line, end } => (0, line, end as u8, 0),
        ConstraintPoint::RectCorner { rect, corner } => (1, rect, corner, 0),
        ConstraintPoint::CircleCenter(circle) => (2, circle, 0, 0),
    }
}

/// Move a sketch vertex to `(u, v)` in the sketch plane, updating coincident partners
/// and re-applying distance and geometric constraints that involve the moved geometry.
pub fn drag_point(
    doc: &mut Document,
    sketch: SketchId,
    dragged: ConstraintPoint,
    u: f32,
    v: f32,
) -> Result<(), String> {
    if !point_in_sketch(doc, dragged, sketch) {
        return Err("Point is not in the active sketch".to_string());
    }

    let (u, v) = project_drag_uv(doc, dragged, u, v)?;
    let group = coincident_group(doc, sketch, dragged);
    for point in &group {
        set_point_uv(doc, *point, u, v)?;
    }

    apply_length_constraints_for_drag(doc, dragged, u, v, &group)?;
    let pins: Vec<(ConstraintPoint, (f32, f32))> = group.iter().map(|point| (*point, (u, v))).collect();
    solve_document_constraints_with_pins(doc, &pins)
}

fn rect_locked_dimensions(doc: &Document, rect: usize) -> (Option<f32>, Option<f32>) {
    let width = find_distance_constraint(doc, DistanceTarget::RectWidth(rect))
        .and_then(|id| constraint_evaluated_length(doc, id))
        .filter(|value| *value > 0.0);
    let height = find_distance_constraint(doc, DistanceTarget::RectHeight(rect))
        .and_then(|id| constraint_evaluated_length(doc, id))
        .filter(|value| *value > 0.0);
    (width, height)
}

fn rect_corner_drag_blocked(doc: &Document, rect: usize) -> bool {
    let (locked_w, locked_h) = rect_locked_dimensions(doc, rect);
    locked_w.is_some() && locked_h.is_some()
}

fn rect_edge_drag_blocked(doc: &Document, rect: usize, edge: RectEdge) -> bool {
    let (locked_w, locked_h) = rect_locked_dimensions(doc, rect);
    match (locked_w, locked_h) {
        (Some(_), Some(_)) => true,
        (Some(_), None) => matches!(edge, RectEdge::Left | RectEdge::Right),
        (None, Some(_)) => matches!(edge, RectEdge::Bottom | RectEdge::Top),
        (None, None) => false,
    }
}

fn rect_corner_axis_locks(
    entity: &crate::model::Rect,
    corner: u8,
    locked_w: Option<f32>,
    locked_h: Option<f32>,
) -> (Option<f32>, Option<f32>) {
    let fixed_u = locked_w.map(|width| match corner {
        0 | 3 => entity.x,
        1 | 2 => entity.x + width,
        _ => entity.x,
    });
    let fixed_v = locked_h.map(|height| match corner {
        0 | 1 => entity.y,
        2 | 3 => entity.y + height,
        _ => entity.y,
    });
    (fixed_u, fixed_v)
}

fn project_rect_corner_drag(
    doc: &Document,
    rect: usize,
    corner: u8,
    u: f32,
    v: f32,
) -> Result<(f32, f32), String> {
    let entity = doc
        .rects
        .get(rect)
        .ok_or_else(|| format!("Rectangle {rect} not found"))?;
    let (locked_w, locked_h) = rect_locked_dimensions(doc, rect);
    let (fixed_u, fixed_v) = rect_corner_axis_locks(entity, corner, locked_w, locked_h);
    let point = ConstraintPoint::RectCorner { rect, corner };

    let (pu, pv) = if let Some((pu, pv)) =
        project_point_point_distance_drag(doc, point, u, v, fixed_u, fixed_v)?
    {
        (pu, pv)
    } else {
        (fixed_u.unwrap_or(u), fixed_v.unwrap_or(v))
    };

    // Keep the corner on its own side of the diagonal anchor so the rectangle cannot invert
    // (which would relabel the corners and jump constrained geometry). This must run on the
    // projected position because the drag pins the corner here, overriding `set_point_uv`.
    Ok(clamp_rect_corner_to_anchor(entity, corner, pu, pv))
}

/// Clamp a rect corner's target so it stays on its side of the diagonally opposite corner.
fn clamp_rect_corner_to_anchor(
    entity: &crate::model::Rect,
    corner: u8,
    u: f32,
    v: f32,
) -> (f32, f32) {
    const MIN_EXTENT: f32 = 1e-3;
    let (anchor_u, anchor_v) = match corner {
        0 => (entity.x + entity.w, entity.y + entity.h),
        1 => (entity.x, entity.y + entity.h),
        2 => (entity.x, entity.y),
        3 => (entity.x + entity.w, entity.y),
        _ => (entity.x, entity.y),
    };
    let cu = match corner {
        0 | 3 => u.min(anchor_u - MIN_EXTENT),
        _ => u.max(anchor_u + MIN_EXTENT),
    };
    let cv = match corner {
        0 | 1 => v.min(anchor_v - MIN_EXTENT),
        _ => v.max(anchor_v + MIN_EXTENT),
    };
    (cu, cv)
}

fn project_onto_distance_circle(
    center_u: f32,
    center_v: f32,
    u: f32,
    v: f32,
    distance: f32,
    fallback_dir_u: f32,
    fallback_dir_v: f32,
) -> (f32, f32) {
    let du = u - center_u;
    let dv = v - center_v;
    let len = du.hypot(dv);
    let (dir_u, dir_v) = if len < 1e-6 {
        (fallback_dir_u, fallback_dir_v)
    } else {
        (du / len, dv / len)
    };
    (
        center_u + dir_u * distance,
        center_v + dir_v * distance,
    )
}

fn project_onto_distance_circle_with_axis_locks(
    center_u: f32,
    center_v: f32,
    u: f32,
    v: f32,
    distance: f32,
    fixed_u: Option<f32>,
    fixed_v: Option<f32>,
    fallback_dir_u: f32,
    fallback_dir_v: f32,
) -> (f32, f32) {
    match (fixed_u, fixed_v) {
        (Some(fu), Some(fv)) => (fu, fv),
        (Some(fu), None) => {
            let du = fu - center_u;
            let disc = distance * distance - du * du;
            if disc <= 0.0 {
                return (fu, center_v);
            }
            let span = disc.sqrt();
            let v1 = center_v + span;
            let v2 = center_v - span;
            let fv = if (v1 - v).abs() <= (v2 - v).abs() { v1 } else { v2 };
            (fu, fv)
        }
        (None, Some(fv)) => {
            let dv = fv - center_v;
            let disc = distance * distance - dv * dv;
            if disc <= 0.0 {
                return (center_u, fv);
            }
            let span = disc.sqrt();
            let u1 = center_u + span;
            let u2 = center_u - span;
            let fu = if (u1 - u).abs() <= (u2 - u).abs() { u1 } else { u2 };
            (fu, fv)
        }
        (None, None) => project_onto_distance_circle(
            center_u,
            center_v,
            u,
            v,
            distance,
            fallback_dir_u,
            fallback_dir_v,
        ),
    }
}

fn project_point_point_distance_drag(
    doc: &Document,
    dragged: ConstraintPoint,
    u: f32,
    v: f32,
    fixed_u: Option<f32>,
    fixed_v: Option<f32>,
) -> Result<Option<(f32, f32)>, String> {
    for (index, constraint) in doc.constraints.iter().enumerate() {
        if constraint.deleted {
            continue;
        }
        let ConstraintKind::Distance {
            target:
                DistanceTarget::PointPointDistance {
                    anchor,
                    mover,
                    dir_u,
                    dir_v,
                },
        } = constraint.kind
        else {
            continue;
        };
        let Some(distance) = constraint_evaluated_length(doc, index) else {
            continue;
        };
        if distance <= 0.0 {
            continue;
        }
        if dragged == mover {
            let (au, av) = point_uv(doc, anchor)?;
            return Ok(Some(project_onto_distance_circle_with_axis_locks(
                au,
                av,
                u,
                v,
                distance,
                fixed_u,
                fixed_v,
                dir_u,
                dir_v,
            )));
        }
        if dragged == anchor {
            let (mu, mv) = point_uv(doc, mover)?;
            return Ok(Some(project_onto_distance_circle_with_axis_locks(
                mu,
                mv,
                u,
                v,
                distance,
                fixed_u,
                fixed_v,
                -dir_u,
                -dir_v,
            )));
        }
    }
    Ok(None)
}

fn project_drag_uv(
    doc: &Document,
    dragged: ConstraintPoint,
    u: f32,
    v: f32,
) -> Result<(f32, f32), String> {
    match dragged {
        ConstraintPoint::RectCorner { rect, corner } => {
            return project_rect_corner_drag(doc, rect, corner, u, v);
        }
        ConstraintPoint::LineEndpoint { line: line_index, end } => {
            let line = ConstraintLine::Line(line_index);
            let mut projected_u = u;
            let mut projected_v = v;
            for constraint in &doc.constraints {
                if constraint.deleted {
                    continue;
                }
                match constraint.kind {
                    ConstraintKind::Horizontal { line: constrained } if constrained == line => {
                        let ((_x0, y0), (_x1, y1)) = line_uv_endpoints(doc, line)?;
                        projected_v = match end {
                            LineEnd::Start => y1,
                            LineEnd::End => y0,
                        };
                    }
                    ConstraintKind::Vertical { line: constrained } if constrained == line => {
                        let ((x0, _y0), (x1, _y1)) = line_uv_endpoints(doc, line)?;
                        projected_u = match end {
                            LineEnd::Start => x1,
                            LineEnd::End => x0,
                        };
                    }
                    _ => {}
                }
            }
            if let Some((pu, pv)) =
                project_point_point_distance_drag(doc, dragged, projected_u, projected_v, None, None)?
            {
                projected_u = pu;
                projected_v = pv;
            }
            if let Some((pu, pv)) =
                project_endpoint_onto_direction(doc, line_index, end, projected_u, projected_v)?
            {
                projected_u = pu;
                projected_v = pv;
            }
            Ok((projected_u, projected_v))
        }
        _ => {
            if let Some((pu, pv)) =
                project_point_point_distance_drag(doc, dragged, u, v, None, None)?
            {
                Ok((pu, pv))
            } else {
                Ok((u, v))
            }
        }
    }
}

/// The unit direction a line's `start -> end` must have because of a direction constraint
/// (parallel/perpendicular/angle), oriented to match the line's current direction.
fn constrained_line_direction(doc: &Document, line_index: usize) -> Option<(f32, f32)> {
    let this = ConstraintLine::Line(line_index);
    let (cur_du, cur_dv) = line_direction_uv(doc, this)?;
    for (index, constraint) in doc.constraints.iter().enumerate() {
        if constraint.deleted {
            continue;
        }
        let candidates: Vec<(f32, f32)> = match constraint.kind {
            ConstraintKind::Parallel { line_a, line_b } => {
                let Some(reference) = direction_reference(this, line_a, line_b) else {
                    continue;
                };
                let (rdu, rdv) = line_direction_uv(doc, reference)?;
                vec![(rdu, rdv), (-rdu, -rdv)]
            }
            ConstraintKind::Perpendicular { line_a, line_b } => {
                let Some(reference) = direction_reference(this, line_a, line_b) else {
                    continue;
                };
                let (rdu, rdv) = line_direction_uv(doc, reference)?;
                vec![(-rdv, rdu), (rdv, -rdu)]
            }
            ConstraintKind::Angle { line_a, line_b, .. } => {
                let Some(reference) = direction_reference(this, line_a, line_b) else {
                    continue;
                };
                let (rdu, rdv) = line_direction_uv(doc, reference)?;
                let angle = constraint_evaluated_angle(doc, index)?;
                let (cos, sin) = (angle.cos(), angle.sin());
                let rot_pos = (rdu * cos - rdv * sin, rdu * sin + rdv * cos);
                let rot_neg = (rdu * cos + rdv * sin, -rdu * sin + rdv * cos);
                vec![
                    rot_pos,
                    rot_neg,
                    (-rot_pos.0, -rot_pos.1),
                    (-rot_neg.0, -rot_neg.1),
                ]
            }
            _ => continue,
        };
        // The line currently satisfies the constraint, so pick the candidate direction that
        // best matches the current orientation (resolving the parallel/perp/angle sign).
        return candidates.into_iter().max_by(|a, b| {
            let da = a.0 * cur_du + a.1 * cur_dv;
            let db = b.0 * cur_du + b.1 * cur_dv;
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    None
}

fn direction_reference(
    this: ConstraintLine,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
) -> Option<ConstraintLine> {
    if line_a == this {
        Some(line_b)
    } else if line_b == this {
        Some(line_a)
    } else {
        None
    }
}

/// If the dragged endpoint's line has a direction constraint and the other endpoint is fixed,
/// slide the target along the constrained ray (so the drag can't override the direction).
fn project_endpoint_onto_direction(
    doc: &Document,
    line_index: usize,
    end: LineEnd,
    u: f32,
    v: f32,
) -> Result<Option<(f32, f32)>, String> {
    let Some((dir_u, dir_v)) = constrained_line_direction(doc, line_index) else {
        return Ok(None);
    };
    let Some(sketch) = doc.lines.get(line_index).map(|line| line.sketch) else {
        return Ok(None);
    };
    let other = ConstraintPoint::LineEndpoint {
        line: line_index,
        end: match end {
            LineEnd::Start => LineEnd::End,
            LineEnd::End => LineEnd::Start,
        },
    };
    // Only constrain the drag when the other endpoint cannot move; otherwise the solver keeps
    // the direction by moving that endpoint and the dragged end should follow the cursor.
    if crate::sketch_solver::sketch_point_movable(doc, sketch, other).unwrap_or(true) {
        return Ok(None);
    }
    let (ox, ov) = point_uv(doc, other)?;
    // The ray runs from the fixed endpoint toward the dragged one along the line direction.
    let (ray_u, ray_v) = match end {
        LineEnd::End => (dir_u, dir_v),
        LineEnd::Start => (-dir_u, -dir_v),
    };
    let t = ((u - ox) * ray_u + (v - ov) * ray_v).max(1e-3);
    Ok(Some((ox + ray_u * t, ov + ray_v * t)))
}

fn coincident_group(doc: &Document, sketch: SketchId, seed: ConstraintPoint) -> Vec<ConstraintPoint> {
    let mut group = vec![seed];
    let mut changed = true;
    while changed {
        changed = false;
        for constraint in &doc.constraints {
            if constraint.deleted || constraint.sketch != sketch {
                continue;
            }
            let ConstraintKind::Coincident { a, b } = constraint.kind else {
                continue;
            };
            let Some(pa) = entity_point(a) else {
                continue;
            };
            let Some(pb) = entity_point(b) else {
                continue;
            };
            if group.contains(&pa) && !group.contains(&pb) {
                group.push(pb);
                changed = true;
            } else if group.contains(&pb) && !group.contains(&pa) {
                group.push(pa);
                changed = true;
            }
        }
    }
    group
}

fn entity_point(entity: ConstraintEntity) -> Option<ConstraintPoint> {
    match entity {
        ConstraintEntity::Point(point) => Some(point),
        ConstraintEntity::Line(_) => None,
    }
}

fn apply_length_constraints_for_drag(
    doc: &mut Document,
    dragged: ConstraintPoint,
    u: f32,
    v: f32,
    group: &[ConstraintPoint],
) -> Result<(), String> {
    let mut lines = std::collections::HashSet::new();
    for point in group {
        if let ConstraintPoint::LineEndpoint { line, .. } = *point {
            lines.insert(line);
        }
    }
    if let ConstraintPoint::LineEndpoint { line, .. } = dragged {
        lines.insert(line);
    }

    for line_index in lines {
        let Some(constraint_index) =
            find_distance_constraint(doc, DistanceTarget::LineLength(line_index))
        else {
            continue;
        };
        let Some(length) = constraint_evaluated_length(doc, constraint_index) else {
            continue;
        };
        if length <= 0.0 {
            continue;
        };

        let line = doc
            .lines
            .get(line_index)
            .ok_or_else(|| format!("Line {line_index} not found"))?;
        let start_in_group = group.contains(&ConstraintPoint::LineEndpoint {
            line: line_index,
            end: LineEnd::Start,
        });
        let end_in_group = group.contains(&ConstraintPoint::LineEndpoint {
            line: line_index,
            end: LineEnd::End,
        });

        let (fixed_u, fixed_v, move_start) = if start_in_group && !end_in_group {
            (line.x1, line.y1, true)
        } else if end_in_group && !start_in_group {
            (line.x0, line.y0, false)
        } else {
            continue;
        };

        let (nu, nv) = project_endpoint_with_length((fixed_u, fixed_v), (u, v), length);
        let entity = doc
            .lines
            .get_mut(line_index)
            .ok_or_else(|| format!("Line {line_index} not found"))?;
        if move_start {
            entity.x0 = nu;
            entity.y0 = nv;
        } else {
            entity.x1 = nu;
            entity.y1 = nv;
        }
    }
    Ok(())
}

fn project_endpoint_with_length(
    fixed: (f32, f32),
    target: (f32, f32),
    length: f32,
) -> (f32, f32) {
    let du = target.0 - fixed.0;
    let dv = target.1 - fixed.1;
    let dist = (du * du + dv * dv).sqrt();
    if dist < 1e-6 {
        return (fixed.0 + length, fixed.1);
    }
    let scale = length / dist;
    (fixed.0 + du * scale, fixed.1 + dv * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::{add_angle_constraint, add_distance_constraint, solve_document_constraints};
    use crate::model::RectEdge;
    use crate::geometric_constraints::{
        add_geometric_constraint_from_selection, line_direction_uv, GeometricConstraintType,
    };
    use crate::hierarchy::SceneElement;
    use crate::model::{ConstraintLine, Document, FaceId, Line, Rect};
    use crate::selection::{click_scene_selection, SceneSelection};

    const EPS: f32 = 1e-2;

    fn sketch_doc() -> (Document, SketchId) {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        (doc, sketch)
    }

    #[test]
    fn drag_line_endpoint_moves_to_target() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::End,
            },
            20.0,
            5.0,
        )
        .unwrap();
        let line = &doc.lines[0];
        assert!((line.x1 - 20.0).abs() < 1e-3);
        assert!((line.y1 - 5.0).abs() < 1e-3);
    }

    #[test]
    fn drag_line_translates_both_endpoints() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        let session = begin_line_drag_session(
            &doc,
            sketch,
            ConstraintLine::Line(0),
            (0.0, 0.0),
        )
        .unwrap();
        drag_line(&mut doc, sketch, &session, (5.0, 3.0)).unwrap();
        let line = &doc.lines[0];
        assert!((line.x0 - 5.0).abs() < EPS);
        assert!((line.y0 - 3.0).abs() < EPS);
        assert!((line.x1 - 15.0).abs() < EPS);
        assert!((line.y1 - 3.0).abs() < EPS);
    }

    /// Regression: dragging a bottom-left corner must not change a locked width.
    #[test]
    fn drag_rect_bottom_left_corner_preserves_locked_width() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "80mm".to_string(),
        )
        .unwrap();

        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::RectCorner {
                rect: 0,
                corner: 0,
            },
            -30.0,
            0.0,
        )
        .unwrap();

        assert!(
            (doc.rects[0].w - 80.0).abs() < EPS,
            "locked width must stay 80mm when dragging bottom-left corner, got w={}",
            doc.rects[0].w
        );
    }

    /// Regression: dragging a bottom-right corner on the constrained width side must not lengthen it.
    #[test]
    fn drag_rect_bottom_right_corner_preserves_locked_width() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "80mm".to_string(),
        )
        .unwrap();

        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::RectCorner {
                rect: 0,
                corner: 1,
            },
            150.0,
            0.0,
        )
        .unwrap();

        assert!(
            (doc.rects[0].w - 80.0).abs() < EPS,
            "locked width must stay 80mm when dragging bottom-right corner, got w={}",
            doc.rects[0].w
        );
    }

    /// Regression: dragging a bottom-left corner must not change a locked height.
    #[test]
    fn drag_rect_bottom_left_corner_preserves_locked_height() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectHeight(0),
            "40mm".to_string(),
        )
        .unwrap();

        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::RectCorner {
                rect: 0,
                corner: 0,
            },
            0.0,
            60.0,
        )
        .unwrap();

        assert!(
            (doc.rects[0].h - 40.0).abs() < EPS,
            "locked height must stay 40mm when dragging bottom-left corner, got h={}",
            doc.rects[0].h
        );
    }

    /// Regression: dragging top-left corner horizontally must not change locked width.
    #[test]
    fn drag_rect_top_left_corner_preserves_locked_width() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "80mm".to_string(),
        )
        .unwrap();

        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::RectCorner {
                rect: 0,
                corner: 3,
            },
            -25.0,
            40.0,
        )
        .unwrap();

        assert!(
            (doc.rects[0].w - 80.0).abs() < EPS,
            "locked width must stay 80mm when dragging top-left corner, got w={}",
            doc.rects[0].w
        );
    }

    /// Regression: dragging the constrained bottom edge must not change locked width.
    #[test]
    fn drag_rect_bottom_edge_preserves_locked_width() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "80mm".to_string(),
        )
        .unwrap();

        let session = begin_line_drag_session(
            &doc,
            sketch,
            ConstraintLine::RectEdge {
                rect: 0,
                edge: RectEdge::Bottom,
            },
            (0.0, 0.0),
        )
        .unwrap();
        drag_line(&mut doc, sketch, &session, (50.0, 0.0)).unwrap();

        assert!(
            (doc.rects[0].w - 80.0).abs() < EPS,
            "locked width must stay 80mm when dragging bottom edge, got w={}",
            doc.rects[0].w
        );
    }

    #[test]
    fn drag_rect_corner_preserves_locked_width() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "80mm".to_string(),
        )
        .unwrap();

        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::RectCorner {
                rect: 0,
                corner: 2,
            },
            200.0,
            60.0,
        )
        .unwrap();

        assert!((doc.rects[0].w - 80.0).abs() < EPS, "w={}", doc.rects[0].w);
        assert!((doc.rects[0].h - 60.0).abs() < EPS, "h={}", doc.rects[0].h);
    }

    #[test]
    fn drag_rect_corner_preserves_locked_height() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectHeight(0),
            "40mm".to_string(),
        )
        .unwrap();

        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::RectCorner {
                rect: 0,
                corner: 2,
            },
            100.0,
            90.0,
        )
        .unwrap();

        assert!((doc.rects[0].w - 100.0).abs() < EPS, "w={}", doc.rects[0].w);
        assert!((doc.rects[0].h - 40.0).abs() < EPS, "h={}", doc.rects[0].h);
    }

    #[test]
    fn drag_rect_corner_blocked_when_width_and_height_locked() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "80mm".to_string(),
        )
        .unwrap();
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectHeight(0),
            "40mm".to_string(),
        )
        .unwrap();

        let corner = ConstraintPoint::RectCorner {
            rect: 0,
            corner: 2,
        };
        assert!(!can_drag_point(&doc, sketch, corner));
    }

    #[test]
    fn drag_rect_right_edge_blocked_when_width_locked() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "80mm".to_string(),
        )
        .unwrap();

        assert!(!can_drag_line(
            &doc,
            sketch,
            ConstraintLine::RectEdge {
                rect: 0,
                edge: RectEdge::Right,
            },
        ));
    }

    #[test]
    fn drag_rect_edge_translates_edge_corners() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        let session = begin_line_drag_session(
            &doc,
            sketch,
            ConstraintLine::RectEdge {
                rect: 0,
                edge: RectEdge::Bottom,
            },
            (0.0, 0.0),
        )
        .unwrap();
        drag_line(&mut doc, sketch, &session, (0.0, 2.0)).unwrap();
        let bottom_left = point_uv(
            &doc,
            ConstraintPoint::RectCorner {
                rect: 0,
                corner: 0,
            },
        )
        .unwrap();
        let bottom_right = point_uv(
            &doc,
            ConstraintPoint::RectCorner {
                rect: 0,
                corner: 1,
            },
        )
        .unwrap();
        let top_left = point_uv(
            &doc,
            ConstraintPoint::RectCorner {
                rect: 0,
                corner: 3,
            },
        )
        .unwrap();
        assert!((bottom_left.0).abs() < EPS && (bottom_left.1 - 2.0).abs() < EPS);
        assert!((bottom_right.0 - 10.0).abs() < EPS && (bottom_right.1 - 2.0).abs() < EPS);
        assert!((top_left.1 - 5.0).abs() < EPS);
    }

    #[test]
    fn drag_line_with_length_constraint_keeps_length() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::LineLength(0),
            "10mm".to_string(),
        )
        .unwrap();
        let session = begin_line_drag_session(
            &doc,
            sketch,
            ConstraintLine::Line(0),
            (0.0, 0.0),
        )
        .unwrap();
        drag_line(&mut doc, sketch, &session, (4.0, 0.0)).unwrap();
        assert!((doc.lines[0].length() - 10.0).abs() < EPS);
        assert!((doc.lines[0].x0 - 4.0).abs() < EPS);
    }

    #[test]
    fn drag_line_with_horizontal_constraint_stays_horizontal() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Horizontal,
            &sel,
        )
        .unwrap();
        let session = begin_line_drag_session(
            &doc,
            sketch,
            ConstraintLine::Line(0),
            (0.0, 0.0),
        )
        .unwrap();
        drag_line(&mut doc, sketch, &session, (0.0, 7.0)).unwrap();
        let line = &doc.lines[0];
        assert!((line.y0 - line.y1).abs() < EPS);
        assert!((line.y0 - 7.0).abs() < EPS);
    }

    #[test]
    fn drag_with_length_constraint_maintains_length() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::LineLength(0),
            "10mm".to_string(),
        )
        .unwrap();
        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::End,
            },
            0.0,
            10.0,
        )
        .unwrap();
        let line = &doc.lines[0];
        assert!((line.length() - 10.0).abs() < 1e-2, "length was {}", line.length());
        assert!((line.x0).abs() < 1e-3 && (line.y0).abs() < 1e-3);
    }

    #[test]
    fn drag_coincident_point_moves_partner() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 10.0, 0.0, 10.0, 8.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(
            &mut sel,
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::End,
            }),
            false,
        );
        click_scene_selection(
            &mut sel,
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::Start,
            }),
            true,
        );
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();

        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::End,
            },
            5.0,
            5.0,
        )
        .unwrap();

        let line0 = &doc.lines[0];
        let line1 = &doc.lines[1];
        assert!((line0.x1 - 5.0).abs() < 1e-2);
        assert!((line0.y1 - 5.0).abs() < 1e-2);
        assert!((line1.x0 - 5.0).abs() < 1e-2);
        assert!((line1.y0 - 5.0).abs() < 1e-2);
    }

    #[test]
    fn drag_line_moves_coincident_endpoint_partner() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 10.0, 0.0, 10.0, 8.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(
            &mut sel,
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::End,
            }),
            false,
        );
        click_scene_selection(
            &mut sel,
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::Start,
            }),
            true,
        );
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();

        let session = begin_line_drag_session(
            &doc,
            sketch,
            ConstraintLine::Line(0),
            (0.0, 0.0),
        )
        .unwrap();
        drag_line(&mut doc, sketch, &session, (3.0, 4.0)).unwrap();

        assert!((doc.lines[0].x1 - 13.0).abs() < EPS);
        assert!((doc.lines[0].y1 - 4.0).abs() < EPS);
        assert!((doc.lines[1].x0 - 13.0).abs() < EPS);
        assert!((doc.lines[1].y0 - 4.0).abs() < EPS);
    }

    fn measured_angle_between_lines(
        doc: &Document,
        line_a: ConstraintLine,
        line_b: ConstraintLine,
    ) -> Option<f32> {
        let (adu, adv) = line_direction_uv(doc, line_a)?;
        let (bdu, bdv) = line_direction_uv(doc, line_b)?;
        let dot = (adu * bdu + adv * bdv).clamp(-1.0, 1.0);
        Some(dot.acos())
    }

    fn measured_line_line_distance(
        doc: &Document,
        line_a: ConstraintLine,
        line_b: ConstraintLine,
    ) -> Option<f32> {
        use crate::geometric_constraints::line_uv_endpoints;
        let ((ax0, ay0), (ax1, ay1)) = line_uv_endpoints(doc, line_a).ok()?;
        let ((bx0, by0), (bx1, by1)) = line_uv_endpoints(doc, line_b).ok()?;
        let du = ax1 - ax0;
        let dv = ay1 - ay0;
        let len = (du * du + dv * dv).sqrt();
        if len < 1e-6 {
            return None;
        }
        let perp_u = -dv / len;
        let perp_v = du / len;
        let amu = (ax0 + ax1) * 0.5;
        let amv = (ay0 + ay1) * 0.5;
        let bmu = (bx0 + bx1) * 0.5;
        let bmv = (by0 + by1) * 0.5;
        Some(((bmu - amu) * perp_u + (bmv - amv) * perp_v).abs())
    }

    fn setup_angle_parallel_spacing_lines(
        doc: &mut Document,
        sketch: SketchId,
    ) -> Result<(), String> {
        use crate::constraints::{add_angle_constraint, add_distance_constraint};

        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 100.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 10.0, 5.0, 50.0, 18.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 10.0, 30.0, 50.0, 43.0));
        doc.shape_order.push(crate::model::ShapeKind::Line);
        doc.shape_order.push(crate::model::ShapeKind::Line);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        add_angle_constraint(
            doc,
            sketch,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
            "16.7".to_string(),
        )?;
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(1), false);
        click_scene_selection(&mut sel, SceneElement::Line(2), true);
        add_geometric_constraint_from_selection(
            doc,
            sketch,
            GeometricConstraintType::Parallel,
            &sel,
        )?;
        add_distance_constraint(
            doc,
            sketch,
            DistanceTarget::LineLineDistance {
                line_a: ConstraintLine::Line(1),
                line_b: ConstraintLine::Line(2),
                side: 1,
            },
            "15mm".to_string(),
        )?;
        Ok(())
    }

    #[test]
    fn drag_vertex_preserves_angle_and_line_spacing() {
        let (mut doc, sketch) = sketch_doc();
        setup_angle_parallel_spacing_lines(&mut doc, sketch).unwrap();

        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::Start,
            },
            25.0,
            12.0,
        )
        .unwrap();

        let angle = measured_angle_between_lines(
            &doc,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
        )
        .unwrap();
        assert!(
            (angle.to_degrees() - 16.7).abs() < 1.0,
            "angle={}",
            angle.to_degrees()
        );

        let spacing = measured_line_line_distance(
            &doc,
            ConstraintLine::Line(1),
            ConstraintLine::Line(2),
        )
        .unwrap();
        assert!((spacing - 15.0).abs() < 0.5, "spacing={spacing}");

        let (bdu, bdv) = line_direction_uv(&doc, ConstraintLine::Line(1)).unwrap();
        let (cdu, cdv) = line_direction_uv(&doc, ConstraintLine::Line(2)).unwrap();
        let parallel_dot = (bdu * cdu + bdv * cdv).clamp(-1.0, 1.0);
        assert!((parallel_dot - 1.0).abs() < 0.01, "parallel_dot={parallel_dot}");
    }

    #[test]
    fn fully_constrained_line_vertex_drag_is_blocked() {
        use crate::constraints::add_distance_constraint;

        let (mut doc, sketch) = sketch_doc();
        setup_angle_parallel_spacing_lines(&mut doc, sketch).unwrap();
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::LineLength(1),
            "40mm".to_string(),
        )
        .unwrap();

        let point = ConstraintPoint::LineEndpoint {
            line: 1,
            end: LineEnd::Start,
        };
        assert!(!can_drag_point(&doc, sketch, point));
        assert!(!can_drag_line(&doc, sketch, ConstraintLine::Line(1)));
    }

    #[test]
    fn partially_constrained_line_vertex_drag_is_allowed() {
        let (mut doc, sketch) = sketch_doc();
        setup_angle_parallel_spacing_lines(&mut doc, sketch).unwrap();

        let point = ConstraintPoint::LineEndpoint {
            line: 1,
            end: LineEnd::Start,
        };
        assert!(can_drag_point(&doc, sketch, point));
        assert!(can_drag_line(&doc, sketch, ConstraintLine::Line(1)));
    }

    fn setup_rect_parallel_perpendicular_point_line_distance(
        doc: &mut Document,
        sketch: SketchId,
    ) -> Result<(ConstraintPoint, ConstraintLine, ConstraintLine), String> {
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 100.0, 0.0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 20.0, 10.0, 70.0, 40.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 30.0, 55.0, 30.0, 85.0));
        doc.shape_order.push(crate::model::ShapeKind::Line);
        doc.shape_order.push(crate::model::ShapeKind::Rect);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        let line_a = ConstraintLine::Line(0);
        let line_b = ConstraintLine::Line(1);
        let rect_top = ConstraintLine::RectEdge {
            rect: 0,
            edge: RectEdge::Top,
        };

        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Top), false);
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        add_geometric_constraint_from_selection(
            doc,
            sketch,
            GeometricConstraintType::Parallel,
            &sel,
        )?;

        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        add_geometric_constraint_from_selection(
            doc,
            sketch,
            GeometricConstraintType::Perpendicular,
            &sel,
        )?;

        let distance_point = ConstraintPoint::LineEndpoint {
            line: 1,
            end: LineEnd::Start,
        };
        add_distance_constraint(
            doc,
            sketch,
            DistanceTarget::PointLineDistance {
                point: distance_point,
                line: rect_top,
                side: 1,
            },
            "50mm".to_string(),
        )?;
        solve_document_constraints(doc)?;

        Ok((distance_point, line_a, line_b))
    }

    fn assert_lines_perpendicular(doc: &Document, line_a: ConstraintLine, line_b: ConstraintLine) {
        let (adu, adv) = line_direction_uv(doc, line_a).unwrap();
        let (bdu, bdv) = line_direction_uv(doc, line_b).unwrap();
        let dot = (adu * bdu + adv * bdv).clamp(-1.0, 1.0);
        assert!(dot.abs() < 0.01, "lines should stay perpendicular, dot={dot}");
    }

    #[test]
    fn drag_vertex_preserves_perpendicular_with_rect_point_line_distance() {
        let (mut doc, sketch) = sketch_doc();
        let (_distance_point, line_a, line_b) =
            setup_rect_parallel_perpendicular_point_line_distance(&mut doc, sketch).unwrap();

        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::End,
            },
            45.0,
            100.0,
        )
        .unwrap();

        assert_lines_perpendicular(&doc, line_a, line_b);
    }

    #[test]
    fn drag_distance_vertex_preserves_perpendicular_with_rect_point_line_distance() {
        let (mut doc, sketch) = sketch_doc();
        let (distance_point, line_a, line_b) =
            setup_rect_parallel_perpendicular_point_line_distance(&mut doc, sketch).unwrap();

        drag_point(&mut doc, sketch, distance_point, 55.0, 95.0).unwrap();

        assert_lines_perpendicular(&doc, line_a, line_b);
    }

    fn point_point_distance_mm(
        doc: &Document,
        anchor: ConstraintPoint,
        mover: ConstraintPoint,
    ) -> f32 {
        let (au, av) = point_uv(doc, anchor).unwrap();
        let (mu, mv) = point_uv(doc, mover).unwrap();
        (mu - au).hypot(mv - av)
    }

    /// Regression: dragging the line vertex must not change a locked point-point distance.
    #[test]
    fn drag_line_vertex_preserves_point_point_distance_from_rect_corner() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 130.0, 40.0, 160.0, 40.0));
        doc.shape_order.push(crate::model::ShapeKind::Rect);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        // Lock the rectangle so the anchor corner cannot slide to absorb drag error.
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "80mm".to_string(),
        )
        .unwrap();
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectHeight(0),
            "40mm".to_string(),
        )
        .unwrap();

        let anchor = ConstraintPoint::RectCorner {
            rect: 0,
            corner: 2,
        };
        let mover = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };

        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::PointPointDistance {
                anchor,
                mover,
                dir_u: 1.0,
                dir_v: 0.0,
            },
            "50mm".to_string(),
        )
        .unwrap();

        assert!(
            (point_point_distance_mm(&doc, anchor, mover) - 50.0).abs() < EPS,
            "initial distance={}",
            point_point_distance_mm(&doc, anchor, mover)
        );

        drag_point(&mut doc, sketch, mover, 200.0, 40.0).unwrap();

        assert!(
            (point_point_distance_mm(&doc, anchor, mover) - 50.0).abs() < EPS,
            "locked 50mm point-point distance must be preserved after drag, got {}",
            point_point_distance_mm(&doc, anchor, mover)
        );
    }

    /// Regression: locked rect width must win over point-point drag projection.
    #[test]
    fn drag_rect_corner_on_locked_side_preserves_width_with_point_point_distance() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 130.0, 40.0, 160.0, 40.0));
        doc.shape_order.push(crate::model::ShapeKind::Rect);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "80mm".to_string(),
        )
        .unwrap();

        let anchor = ConstraintPoint::RectCorner {
            rect: 0,
            corner: 2,
        };
        let mover = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };

        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::PointPointDistance {
                anchor,
                mover,
                dir_u: 1.0,
                dir_v: 0.0,
            },
            "50mm".to_string(),
        )
        .unwrap();

        drag_point(&mut doc, sketch, anchor, 150.0, 40.0).unwrap();

        assert!(
            (doc.rects[0].w - 80.0).abs() < EPS,
            "locked width must stay 80mm when dragging constrained corner, got w={}",
            doc.rects[0].w
        );
        assert!(
            (point_point_distance_mm(&doc, anchor, mover) - 50.0).abs() < EPS,
            "point-point distance after drag={}",
            point_point_distance_mm(&doc, anchor, mover)
        );
    }

    /// Locked width leaves one axis free; corner should still slide along the distance circle.
    #[test]
    fn drag_rect_corner_slides_on_circle_when_width_locked_with_point_point_distance() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 110.0, 75.0, 140.0, 75.0));
        doc.shape_order.push(crate::model::ShapeKind::Rect);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "80mm".to_string(),
        )
        .unwrap();

        let anchor = ConstraintPoint::RectCorner {
            rect: 0,
            corner: 2,
        };
        let mover = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };

        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::PointPointDistance {
                anchor,
                mover,
                dir_u: 1.0,
                dir_v: 0.0,
            },
            "50mm".to_string(),
        )
        .unwrap();

        let iv = point_uv(&doc, anchor).unwrap().1;
        drag_point(&mut doc, sketch, anchor, 120.0, 95.0).unwrap();
        let (fu, fv) = point_uv(&doc, anchor).unwrap();

        assert!((doc.rects[0].w - 80.0).abs() < EPS, "w={}", doc.rects[0].w);
        assert!(
            (point_point_distance_mm(&doc, anchor, mover) - 50.0).abs() < EPS,
            "distance={}",
            point_point_distance_mm(&doc, anchor, mover)
        );
        assert!((fu - 80.0).abs() < EPS, "locked side should stay at u=80, got {fu}");
        assert!(
            (fv - iv).abs() > 1.0,
            "corner should move along the free axis, iv={iv} got ({fu}, {fv})"
        );
    }

    /// Regression: point-point distance should allow dragging around a circle, not lock bearing.
    #[test]
    fn drag_line_vertex_around_point_point_distance_circle() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 80.0, 40.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 130.0, 40.0, 160.0, 40.0));
        doc.shape_order.push(crate::model::ShapeKind::Rect);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "80mm".to_string(),
        )
        .unwrap();
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectHeight(0),
            "40mm".to_string(),
        )
        .unwrap();

        let anchor = ConstraintPoint::RectCorner {
            rect: 0,
            corner: 2,
        };
        let mover = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };

        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::PointPointDistance {
                anchor,
                mover,
                dir_u: 1.0,
                dir_v: 0.0,
            },
            "50mm".to_string(),
        )
        .unwrap();

        let (iu, iv) = point_uv(&doc, mover).unwrap();
        assert!((iu - 130.0).abs() < EPS && (iv - 40.0).abs() < EPS, "iu={iu} iv={iv}");

        drag_point(&mut doc, sketch, mover, 80.0, 90.0).unwrap();

        let (fu, fv) = point_uv(&doc, mover).unwrap();
        assert!(
            (point_point_distance_mm(&doc, anchor, mover) - 50.0).abs() < EPS,
            "distance after drag={}",
            point_point_distance_mm(&doc, anchor, mover)
        );
        assert!(
            (fu - 80.0).abs() < EPS && (fv - 90.0).abs() < EPS,
            "mover should swing to (80, 90), got ({fu}, {fv})"
        );
        assert!(
            (fu - iu).abs() > 1.0 || (fv - iv).abs() > 1.0,
            "drag should move the vertex around the anchor"
        );
    }

    #[test]
    fn drag_horizontal_line_endpoint_stays_horizontal() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Horizontal,
            &sel,
        )
        .unwrap();

        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::End,
            },
            15.0,
            8.0,
        )
        .unwrap();

        let line = &doc.lines[0];
        assert!((line.y0 - line.y1).abs() < 1e-3, "line should stay horizontal");
        assert!(line.x1 > 10.0);
    }

    /// Regression: with the left side (height) locked, dragging the top-right corner inward
    /// must shorten the top edge. Previously the stale bottom-right corner pinned max-u so the
    /// width could only grow, never shrink.
    #[test]
    fn drag_rect_top_right_corner_can_shorten_top_with_locked_height() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 60.0, 80.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectHeight(0),
            "80mm".to_string(),
        )
        .unwrap();

        let w_before = doc.rects[0].w;
        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::RectCorner { rect: 0, corner: 2 },
            30.0,
            80.0,
        )
        .unwrap();

        assert!(
            doc.rects[0].w < w_before - 10.0,
            "top edge should shorten when dragging top-right corner inward, w={} (was {w_before})",
            doc.rects[0].w
        );
        assert!((doc.rects[0].h - 80.0).abs() < EPS, "height stays locked, h={}", doc.rects[0].h);
    }

    /// Regression: the bottom-right corner behaves the same as the top-right corner.
    #[test]
    fn drag_rect_bottom_right_corner_can_shorten_bottom_with_locked_height() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 60.0, 80.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectHeight(0),
            "80mm".to_string(),
        )
        .unwrap();

        let w_before = doc.rects[0].w;
        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::RectCorner { rect: 0, corner: 1 },
            30.0,
            0.0,
        )
        .unwrap();

        assert!(
            doc.rects[0].w < w_before - 10.0,
            "bottom edge should shorten when dragging bottom-right corner inward, w={} (was {w_before})",
            doc.rects[0].w
        );
    }

    /// Regression: the full reported scenario — left side locked to 80mm, and a line vertex
    /// held 45mm from the rect's top-left corner. Dragging the top-right corner left must
    /// shorten the top edge AND must never flip the top-left corner past the line vertex.
    #[test]
    fn drag_rect_top_right_corner_keeps_top_left_anchor_with_distanced_line() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 60.0, 80.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, -45.0, 80.0, -45.0, 100.0));
        doc.shape_order.push(crate::model::ShapeKind::Rect);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectHeight(0),
            "80mm".to_string(),
        )
        .unwrap();

        let rect_top_left = ConstraintPoint::RectCorner { rect: 0, corner: 3 };
        let line_vertex = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::PointPointDistance {
                anchor: rect_top_left,
                mover: line_vertex,
                dir_u: -1.0,
                dir_v: 0.0,
            },
            "45mm".to_string(),
        )
        .unwrap();

        let (tlu_before, tlv_before) = point_uv(&doc, rect_top_left).unwrap();

        // Push the top-right corner well to the left of the top-left corner.
        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::RectCorner { rect: 0, corner: 2 },
            -40.0,
            80.0,
        )
        .unwrap();

        // The top edge shrank (rather than the rect growing / flipping).
        assert!(doc.rects[0].w < 60.0, "width should not grow, w={}", doc.rects[0].w);
        // The top-left corner stayed put: it never flipped past the line vertex.
        let (tlu_after, tlv_after) = point_uv(&doc, rect_top_left).unwrap();
        assert!(
            (tlu_after - tlu_before).abs() < EPS && (tlv_after - tlv_before).abs() < EPS,
            "top-left corner must stay at ({tlu_before},{tlv_before}), got ({tlu_after},{tlv_after})"
        );
        // The 45mm distance to the line vertex is preserved.
        assert!(
            (point_point_distance_mm(&doc, rect_top_left, line_vertex) - 45.0).abs() < EPS,
            "distance to line vertex must stay 45mm, got {}",
            point_point_distance_mm(&doc, rect_top_left, line_vertex)
        );
    }

    fn line_top_edge_angle(doc: &Document) -> f32 {
        let (ldu, ldv) = line_direction_uv(doc, ConstraintLine::Line(0)).unwrap();
        let (tdu, tdv) = line_direction_uv(
            doc,
            ConstraintLine::RectEdge {
                rect: 0,
                edge: RectEdge::Top,
            },
        )
        .unwrap();
        (ldu * tdu + ldv * tdv).clamp(-1.0, 1.0).acos()
    }

    /// Regression: with one end of the line coincident to a rect corner and a 45° angle to the
    /// rect edge, dragging the free end must keep the angle — it must not slide off freely.
    #[test]
    fn drag_angle_constrained_line_end_preserves_angle() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 40.0, 40.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 40.0, 40.0, 80.0, 80.0));
        doc.shape_order.push(crate::model::ShapeKind::Rect);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        let corner2 = ConstraintPoint::RectCorner { rect: 0, corner: 2 };
        let line_start = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Point(corner2), false);
        click_scene_selection(&mut sel, SceneElement::Point(line_start), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();
        add_angle_constraint(
            &mut doc,
            sketch,
            ConstraintLine::RectEdge {
                rect: 0,
                edge: RectEdge::Top,
            },
            ConstraintLine::Line(0),
            "45 deg".to_string(),
        )
        .unwrap();

        let angle_before = line_top_edge_angle(&doc);
        // Drag the free end far off the 45° ray.
        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::End,
            },
            80.0,
            45.0,
        )
        .unwrap();
        let angle_after = line_top_edge_angle(&doc);
        assert!(
            (angle_after - angle_before).abs() < 2.0_f32.to_radians(),
            "angle drifted from {} to {} deg",
            angle_before.to_degrees(),
            angle_after.to_degrees()
        );
    }

    /// Regression: after deleting a coincident constraint, the two vertices must no longer
    /// move together.
    #[test]
    fn deleted_coincident_constraint_no_longer_couples_vertices() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 10.0, 0.0, 10.0, 8.0));
        doc.shape_order.push(crate::model::ShapeKind::Line);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        let p0 = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::End,
        };
        let p1 = ConstraintPoint::LineEndpoint {
            line: 1,
            end: LineEnd::Start,
        };
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Point(p0), false);
        click_scene_selection(&mut sel, SceneElement::Point(p1), true);
        let id = add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();

        // Delete the coincident constraint.
        crate::document_lifecycle::tombstone_element(&mut doc, SceneElement::Constraint(id));

        let partner_before = point_uv(&doc, p1).unwrap();
        drag_point(&mut doc, sketch, p0, 5.0, 5.0).unwrap();
        let partner_after = point_uv(&doc, p1).unwrap();

        assert!(
            (partner_after.0 - partner_before.0).abs() < EPS
                && (partner_after.1 - partner_before.1).abs() < EPS,
            "partner vertex moved with a deleted coincident constraint: {partner_before:?} -> {partner_after:?}"
        );
    }

    /// Dragging a corner past its diagonal anchor must not invert the rectangle (which would
    /// relabel the corners and jump constrained geometry).
    #[test]
    fn drag_rect_corner_past_anchor_does_not_invert() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 60.0, 80.0));

        drag_point(
            &mut doc,
            sketch,
            ConstraintPoint::RectCorner { rect: 0, corner: 2 },
            -40.0,
            -40.0,
        )
        .unwrap();

        // Bottom-left (the diagonal anchor) stays at the origin; rect collapses but never flips.
        let (blu, blv) = point_uv(&doc, ConstraintPoint::RectCorner { rect: 0, corner: 0 }).unwrap();
        assert!((blu).abs() < EPS && (blv).abs() < EPS, "anchor moved to ({blu},{blv})");
        assert!(doc.rects[0].w > 0.0 && doc.rects[0].h > 0.0, "extents must stay positive");
    }
}