//! Drag sketch vertices and line segments in the viewport while satisfying active constraints.

use crate::constraints::{
    constraint_evaluated_angle, constraint_evaluated_length, find_distance_constraint,
    solve_document_constraints_with_pins,
};
use crate::construction::point_sketch;
use crate::geometric_constraints::{
    line_direction_uv, line_uv_endpoints, point_uv, set_point_uv,
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
        ConstraintPoint::CircleCenter(circle) => SceneElement::Circle(circle),
        // A face's own vertex tracks the extrusion that produced its face, same convention
        // as `document_health`/`hierarchy`'s owner mappings for `FaceVertex`/`FaceEdge`.
        ConstraintPoint::FaceVertex { face, .. } => {
            SceneElement::Extrusion(face.extrusion_index().unwrap_or(usize::MAX))
        }
    }
}

pub fn scene_element_for_line(line: ConstraintLine) -> SceneElement {
    match line {
        ConstraintLine::Line(index) => SceneElement::Line(index),
        // A face's own edge (#26/#27) is itself a first-class selectable/constraint-authoring
        // target, so it wraps whole like `ConstraintPoint`/`SceneElement::Point` do — not the
        // extrusion-owner mapping other (dependency-tracking) call sites use.
        face_edge @ ConstraintLine::FaceEdge { .. } => SceneElement::FaceEdge(face_edge),
    }
}

pub fn line_drag_seed_points(line: ConstraintLine) -> Vec<ConstraintPoint> {
    match line {
        ConstraintLine::Line(index) => vec![
            ConstraintPoint::LineEndpoint {
                line: index,
                end: LineEnd::Start,
            },
            ConstraintPoint::LineEndpoint {
                line: index,
                end: LineEnd::End,
            },
        ],
        // A face's own edge is fixed (not draggable), so it has no seed points to drag.
        ConstraintLine::FaceEdge { .. } => Vec::new(),
    }
}

/// Whether a sketch vertex may be dragged (fully constrained vertices are blocked).
pub fn can_drag_point(doc: &Document, sketch: SketchId, point: ConstraintPoint) -> bool {
    if !point_in_sketch(doc, point.clone(), sketch) {
        return false;
    }
    if let ConstraintPoint::LineEndpoint { line, .. } = point {
        return !crate::sketch_solver::sketch_line_vertex_drag_blocked(doc, sketch, line)
            .unwrap_or(false);
    }
    // A face's own vertex is fixed by the body's geometry, never draggable.
    if let ConstraintPoint::FaceVertex { .. } = point {
        return false;
    }
    true
}

/// Whether a sketch line may be translated by dragging (fully constrained lines are blocked).
pub fn can_drag_line(doc: &Document, sketch: SketchId, target: ConstraintLine) -> bool {
    match target {
        ConstraintLine::Line(line) => {
            point_in_sketch(doc, line_drag_seed_points(target)[0].clone(), sketch)
                && !crate::sketch_solver::sketch_line_vertex_drag_blocked(doc, sketch, line)
                    .unwrap_or(false)
        }
        // Fixed by the body's own geometry, never draggable.
        ConstraintLine::FaceEdge { .. } => false,
    }
}

pub fn begin_line_drag_session(
    doc: &Document,
    sketch: SketchId,
    target: ConstraintLine,
    anchor_uv: (f32, f32),
) -> Result<LineDragSession, String> {
    validate_line_drag_target(doc, sketch, target.clone())?;
    let initial_positions = collect_line_drag_positions(doc, sketch, target.clone())?;
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
    validate_line_drag_target(doc, sketch, session.target.clone())?;
    let du = current_uv.0 - session.anchor_uv.0;
    let dv = current_uv.1 - session.anchor_uv.1;
    let seeds = line_drag_seed_points(session.target.clone());
    translate_line(doc, sketch, session.target.clone(), du, dv)?;
    for (point, (iu, iv)) in &session.initial_positions {
        if seeds.contains(point) || !point_in_sketch(doc, point.clone(), sketch) {
            continue;
        }
        set_point_uv(doc, sketch, point.clone(), iu + du, iv + dv)?;
    }
    let pins: Vec<(ConstraintPoint, (f32, f32))> = session
        .initial_positions
        .iter()
        .filter(|(point, _)| point_in_sketch(doc, (*point).clone(), sketch))
        .map(|(point, (iu, iv))| {
            let projected = project_drag_uv(doc, sketch, point.clone(), iu + du, iv + dv)
                .unwrap_or((iu + du, iv + dv));
            (point.clone(), projected)
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
        // Fixed by the body's own geometry — never a valid line-drag target.
        ConstraintLine::FaceEdge { index, .. } => {
            return Err(format!("Face edge {index} cannot be dragged"));
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
    points.sort_by_key(|point| constraint_point_sort_key(point.clone()));
    points.dedup();

    let mut initial_positions = HashMap::new();
    for point in points {
        let uv = point_uv(doc, sketch, point.clone())?;
        initial_positions.insert(point, uv);
    }
    Ok(initial_positions)
}

fn constraint_point_sort_key(point: ConstraintPoint) -> (u8, usize, u8, u8) {
    match point {
        ConstraintPoint::LineEndpoint { line, end } => (0, line, end as u8, 0),
        ConstraintPoint::CircleCenter(circle) => (2, circle, 0, 0),
        ConstraintPoint::FaceVertex { index, .. } => (3, index, 0, 0),
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
    if !point_in_sketch(doc, dragged.clone(), sketch) {
        return Err("Point is not in the active sketch".to_string());
    }

    let (u, v) = project_drag_uv(doc, sketch, dragged.clone(), u, v)?;
    let group = coincident_group(doc, sketch, dragged.clone());
    for point in &group {
        set_point_uv(doc, sketch, point.clone(), u, v)?;
    }

    apply_length_constraints_for_drag(doc, dragged, u, v, &group)?;
    let pins: Vec<(ConstraintPoint, (f32, f32))> =
        group.iter().map(|point| (point.clone(), (u, v))).collect();
    solve_document_constraints_with_pins(doc, &pins)
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
    sketch: SketchId,
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
        } = constraint.kind.clone()
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
            let (au, av) = point_uv(doc, sketch, anchor)?;
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
            let (mu, mv) = point_uv(doc, sketch, mover)?;
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
    sketch: SketchId,
    dragged: ConstraintPoint,
    u: f32,
    v: f32,
) -> Result<(f32, f32), String> {
    // A point pinned to a line's midpoint or onto a line must not be draggable off it; project
    // the cursor onto the constrained position so the drag pin can't break the constraint.
    if let Some((pu, pv)) = project_onto_anchoring_constraint(doc, sketch, dragged.clone(), u, v)? {
        return Ok((pu, pv));
    }
    match dragged.clone() {
        ConstraintPoint::LineEndpoint { line: line_index, end } => {
            let line = ConstraintLine::Line(line_index);
            let mut projected_u = u;
            let mut projected_v = v;
            for constraint in &doc.constraints {
                if constraint.deleted {
                    continue;
                }
                match &constraint.kind {
                    ConstraintKind::Horizontal { line: constrained } if *constrained == line => {
                        let ((_x0, y0), (_x1, y1)) = line_uv_endpoints(doc, sketch, line.clone())?;
                        projected_v = match end {
                            LineEnd::Start => y1,
                            LineEnd::End => y0,
                        };
                    }
                    ConstraintKind::Vertical { line: constrained } if *constrained == line => {
                        let ((x0, _y0), (x1, _y1)) = line_uv_endpoints(doc, sketch, line.clone())?;
                        projected_u = match end {
                            LineEnd::Start => x1,
                            LineEnd::End => x0,
                        };
                    }
                    _ => {}
                }
            }
            if let Some((pu, pv)) = project_point_point_distance_drag(
                doc,
                sketch,
                dragged.clone(),
                projected_u,
                projected_v,
                None,
                None,
            )? {
                projected_u = pu;
                projected_v = pv;
            }
            if let Some((pu, pv)) =
                project_endpoint_onto_direction(doc, sketch, line_index, end, projected_u, projected_v)?
            {
                projected_u = pu;
                projected_v = pv;
            }
            Ok((projected_u, projected_v))
        }
        _ => {
            if let Some((pu, pv)) =
                project_point_point_distance_drag(doc, sketch, dragged, u, v, None, None)?
            {
                Ok((pu, pv))
            } else {
                Ok((u, v))
            }
        }
    }
}

/// If `dragged` is pinned to a line's midpoint or onto a line, return the constrained position
/// (the midpoint, or the perpendicular foot on the line) so the drag can't pull it off.
fn project_onto_anchoring_constraint(
    doc: &Document,
    sketch: SketchId,
    dragged: ConstraintPoint,
    u: f32,
    v: f32,
) -> Result<Option<(f32, f32)>, String> {
    for constraint in &doc.constraints {
        if constraint.deleted {
            continue;
        }
        match &constraint.kind {
            ConstraintKind::Midpoint { point, line } if *point == dragged => {
                let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, sketch, line.clone())?;
                return Ok(Some(((x0 + x1) * 0.5, (y0 + y1) * 0.5)));
            }
            ConstraintKind::Coincident { a, b } => {
                let line = match (a, b) {
                    (ConstraintEntity::Point(p), ConstraintEntity::Line(l))
                    | (ConstraintEntity::Line(l), ConstraintEntity::Point(p))
                        if *p == dragged =>
                    {
                        Some(l.clone())
                    }
                    _ => None,
                };
                if let Some(line) = line {
                    return Ok(Some(project_point_onto_line_uv(doc, sketch, line, u, v)?));
                }
            }
            _ => {}
        }
    }
    Ok(None)
}


/// Perpendicular foot of `(u, v)` on the infinite line through `line` (sketch units).
fn project_point_onto_line_uv(
    doc: &Document,
    sketch: SketchId,
    line: ConstraintLine,
    u: f32,
    v: f32,
) -> Result<(f32, f32), String> {
    let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, sketch, line)?;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        return Ok((u, v));
    }
    let t = ((u - x0) * dx + (v - y0) * dy) / len_sq;
    Ok((x0 + dx * t, y0 + dy * t))
}

/// The unit direction a line's `start -> end` must have because of a direction constraint
/// (parallel/perpendicular/angle), oriented to match the line's current direction.
fn constrained_line_direction(doc: &Document, line_index: usize) -> Option<(f32, f32)> {
    let this = ConstraintLine::Line(line_index);
    let sketch = doc.lines.get(line_index)?.sketch;
    let (cur_du, cur_dv) = line_direction_uv(doc, sketch, this.clone())?;
    for (index, constraint) in doc.constraints.iter().enumerate() {
        if constraint.deleted {
            continue;
        }
        let candidates: Vec<(f32, f32)> = match &constraint.kind {
            ConstraintKind::Parallel { line_a, line_b } => {
                let Some(reference) = direction_reference(this.clone(), line_a.clone(), line_b.clone())
                else {
                    continue;
                };
                let (rdu, rdv) = line_direction_uv(doc, sketch, reference)?;
                vec![(rdu, rdv), (-rdu, -rdv)]
            }
            ConstraintKind::Perpendicular { line_a, line_b } => {
                let Some(reference) = direction_reference(this.clone(), line_a.clone(), line_b.clone())
                else {
                    continue;
                };
                let (rdu, rdv) = line_direction_uv(doc, sketch, reference)?;
                vec![(-rdv, rdu), (rdv, -rdu)]
            }
            ConstraintKind::Angle { line_a, line_b, .. } => {
                let Some(reference) = direction_reference(this.clone(), line_a.clone(), line_b.clone())
                else {
                    continue;
                };
                let (rdu, rdv) = line_direction_uv(doc, sketch, reference)?;
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
    sketch: SketchId,
    line_index: usize,
    end: LineEnd,
    u: f32,
    v: f32,
) -> Result<Option<(f32, f32)>, String> {
    let Some((dir_u, dir_v)) = constrained_line_direction(doc, line_index) else {
        return Ok(None);
    };
    if doc.lines.get(line_index).is_none() {
        return Ok(None);
    }
    let other = ConstraintPoint::LineEndpoint {
        line: line_index,
        end: match end {
            LineEnd::Start => LineEnd::End,
            LineEnd::End => LineEnd::Start,
        },
    };
    // Only constrain the drag when the other endpoint cannot move; otherwise the solver keeps
    // the direction by moving that endpoint and the dragged end should follow the cursor.
    if crate::sketch_solver::sketch_point_movable(doc, sketch, other.clone()).unwrap_or(true) {
        return Ok(None);
    }
    let (ox, ov) = point_uv(doc, sketch, other)?;
    // The ray runs from the fixed endpoint toward the dragged one along the line direction.
    let (ray_u, ray_v) = match end {
        LineEnd::End => (dir_u, dir_v),
        LineEnd::Start => (-dir_u, -dir_v),
    };
    let t = ((u - ox) * ray_u + (v - ov) * ray_v).max(1e-3);
    Ok(Some((ox + ray_u * t, ov + ray_v * t)))
}

pub fn coincident_group(doc: &Document, sketch: SketchId, seed: ConstraintPoint) -> Vec<ConstraintPoint> {
    let mut group = vec![seed];
    let mut changed = true;
    while changed {
        changed = false;
        for constraint in &doc.constraints {
            if constraint.deleted || constraint.sketch != sketch {
                continue;
            }
            let ConstraintKind::Coincident { a, b } = constraint.kind.clone() else {
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

/// The two `(line, end)` pairs meeting at `point` via `Coincident` constraints, if exactly two
/// distinct plain-line endpoints share that vertex. Used by both "convert to bezier" (#54) and
/// chamfer/fillet (#37/#38) to find the two edges that meet at a sketch vertex.
pub fn incident_two_lines(
    doc: &Document,
    sketch: SketchId,
    point: ConstraintPoint,
) -> Option<[(usize, LineEnd); 2]> {
    let mut endpoints: Vec<(usize, LineEnd)> = coincident_group(doc, sketch, point)
        .into_iter()
        .filter_map(|p| match p {
            ConstraintPoint::LineEndpoint { line, end } => Some((line, end)),
            _ => None,
        })
        .collect();
    endpoints.sort_by_key(|&(line, end)| (line, matches!(end, LineEnd::End)));
    endpoints.dedup();
    match endpoints.as_slice() {
        [a, b] => Some([*a, *b]),
        _ => None,
    }
}

/// The two lines, their vertex-side ends, and the resolved corner geometry (shared vertex `v`
/// plus each line's far endpoint `a`/`b`, in sketch-local UV coords) for a chamfer/fillet vertex
/// treatment at `point`. Feeds directly into [`crate::model::vertex_treatment_geometry`].
///
/// `line1 < line2`, mirroring [`incident_two_lines`]'s ordering, so callers that need a single
/// deterministic "primary" line for the pair (e.g. nesting a bridging line under the lower-index
/// trimmed line in the Elements pane, #76) get a consistent answer.
///
/// Factored out of what used to be near-identical resolution code in "convert to bezier" (#54),
/// commit-time chamfer/fillet (#37/#38), and the chamfer/fillet gizmo anchor/live-preview (#76).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VertexTreatmentCorner {
    pub line1: usize,
    pub end1: LineEnd,
    pub line2: usize,
    pub end2: LineEnd,
    /// Shared vertex, in sketch-local UV coords.
    pub v: (f32, f32),
    /// `line1`'s far (non-vertex) endpoint, in sketch-local UV coords.
    pub a: (f32, f32),
    /// `line2`'s far (non-vertex) endpoint, in sketch-local UV coords.
    pub b: (f32, f32),
}

pub fn treatment_corner(
    doc: &Document,
    sketch: SketchId,
    point: ConstraintPoint,
) -> Option<VertexTreatmentCorner> {
    let [(line1, end1), (line2, end2)] = incident_two_lines(doc, sketch, point)?;
    let l1 = doc.lines.get(line1)?;
    let (v, a) = match end1 {
        LineEnd::Start => ((l1.x0, l1.y0), (l1.x1, l1.y1)),
        LineEnd::End => ((l1.x1, l1.y1), (l1.x0, l1.y0)),
    };
    let l2 = doc.lines.get(line2)?;
    let b = match end2 {
        LineEnd::Start => (l2.x1, l2.y1),
        LineEnd::End => (l2.x0, l2.y0),
    };
    Some(VertexTreatmentCorner { line1, end1, line2, end2, v, a, b })
}

fn entity_point(entity: ConstraintEntity) -> Option<ConstraintPoint> {
    match entity {
        ConstraintEntity::Point(point) => Some(point),
        // The origin is a fixed reference, not a draggable vertex.
        ConstraintEntity::Line(_) | ConstraintEntity::Circle(_) | ConstraintEntity::Origin => None,
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
    use crate::constraints::{add_distance_constraint, solve_document_constraints};
    use crate::geometric_constraints::{
        add_geometric_constraint_from_selection, line_direction_uv, GeometricConstraintType,
    };
    use crate::hierarchy::SceneElement;
    use crate::model::{ConstraintLine, Document, FaceId, Line};
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
        sketch: SketchId,
        line_a: ConstraintLine,
        line_b: ConstraintLine,
    ) -> Option<f32> {
        let (adu, adv) = line_direction_uv(doc, sketch, line_a)?;
        let (bdu, bdv) = line_direction_uv(doc, sketch, line_b)?;
        let dot = (adu * bdu + adv * bdv).clamp(-1.0, 1.0);
        Some(dot.acos())
    }

    fn measured_line_line_distance(
        doc: &Document,
        sketch: SketchId,
        line_a: ConstraintLine,
        line_b: ConstraintLine,
    ) -> Option<f32> {
        use crate::geometric_constraints::line_uv_endpoints;
        let ((ax0, ay0), (ax1, ay1)) = line_uv_endpoints(doc, sketch, line_a).ok()?;
        let ((bx0, by0), (bx1, by1)) = line_uv_endpoints(doc, sketch, line_b).ok()?;
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
        use crate::constraints::{
            add_angle_constraint_with_sign, add_distance_constraint, angle_constraint_natural_sign,
        };

        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 100.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 10.0, 5.0, 50.0, 18.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 10.0, 30.0, 50.0, 43.0));
        doc.shape_order.push(crate::model::ShapeKind::Line);
        doc.shape_order.push(crate::model::ShapeKind::Line);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        let rotation_sign =
            angle_constraint_natural_sign(doc, ConstraintLine::Line(0), ConstraintLine::Line(1))
                .ok_or_else(|| "Lines do not intersect".to_string())?;
        add_angle_constraint_with_sign(
            doc,
            sketch,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
            rotation_sign,
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
            sketch,
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
            sketch,
            ConstraintLine::Line(1),
            ConstraintLine::Line(2),
        )
        .unwrap();
        assert!((spacing - 15.0).abs() < 0.5, "spacing={spacing}");

        let (bdu, bdv) = line_direction_uv(&doc, sketch, ConstraintLine::Line(1)).unwrap();
        let (cdu, cdv) = line_direction_uv(&doc, sketch, ConstraintLine::Line(2)).unwrap();
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
        // line 0 (line_a): horizontal reference line
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 100.0, 0.0));
        doc.shape_order.push(crate::model::ShapeKind::Line);
        // line 1 (line_b): vertical line whose Start is used for the point-line distance
        doc.lines
            .push(Line::from_local_endpoints(sketch, 30.0, 55.0, 30.0, 85.0));
        doc.shape_order.push(crate::model::ShapeKind::Line);
        // rect as 4 lines (corners (20,10)-(70,40) => x=20,y=10,w=50,h=30); top edge is rect_lines[2]
        let rect_lines =
            crate::construction::add_line_rectangle(doc, sketch, 20.0, 10.0, 50.0, 30.0, [false; 4]);

        let line_a = ConstraintLine::Line(0);
        let line_b = ConstraintLine::Line(1);
        let rect_top = ConstraintLine::Line(rect_lines[2]);

        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(rect_lines[2]), false);
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
                point: distance_point.clone(),
                line: rect_top,
                side: 1,
            },
            "50mm".to_string(),
        )?;
        solve_document_constraints(doc)?;

        Ok((distance_point, line_a, line_b))
    }

    fn assert_lines_perpendicular(
        doc: &Document,
        sketch: SketchId,
        line_a: ConstraintLine,
        line_b: ConstraintLine,
    ) {
        let (adu, adv) = line_direction_uv(doc, sketch, line_a).unwrap();
        let (bdu, bdv) = line_direction_uv(doc, sketch, line_b).unwrap();
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

        assert_lines_perpendicular(&doc, sketch, line_a, line_b);
    }

    #[test]
    fn drag_distance_vertex_preserves_perpendicular_with_rect_point_line_distance() {
        let (mut doc, sketch) = sketch_doc();
        let (distance_point, line_a, line_b) =
            setup_rect_parallel_perpendicular_point_line_distance(&mut doc, sketch).unwrap();

        drag_point(&mut doc, sketch, distance_point, 55.0, 95.0).unwrap();

        assert_lines_perpendicular(&doc, sketch, line_a, line_b);
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
        click_scene_selection(&mut sel, SceneElement::Point(p0.clone()), false);
        click_scene_selection(&mut sel, SceneElement::Point(p1.clone()), true);
        let id = add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();

        // Delete the coincident constraint.
        crate::document_lifecycle::tombstone_element(&mut doc, SceneElement::Constraint(id));

        let partner_before = point_uv(&doc, sketch, p1.clone()).unwrap();
        drag_point(&mut doc, sketch, p0, 5.0, 5.0).unwrap();
        let partner_after = point_uv(&doc, sketch, p1).unwrap();

        assert!(
            (partner_after.0 - partner_before.0).abs() < EPS
                && (partner_after.1 - partner_before.1).abs() < EPS,
            "partner vertex moved with a deleted coincident constraint: {partner_before:?} -> {partner_after:?}"
        );
    }

    fn push_coincident(doc: &mut Document, sketch: SketchId, a: ConstraintPoint, b: ConstraintPoint) {
        doc.constraints.push(crate::model::Constraint {
            sketch,
            kind: ConstraintKind::Coincident {
                a: ConstraintEntity::Point(a),
                b: ConstraintEntity::Point(b),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
    }

    #[test]
    fn treatment_corner_resolves_shared_vertex_and_far_endpoints() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines.push(Line::from_local_endpoints(sketch, 10.0, 0.0, 10.0, 10.0));
        push_coincident(
            &mut doc,
            sketch,
            ConstraintPoint::LineEndpoint { line: 0, end: LineEnd::End },
            ConstraintPoint::LineEndpoint { line: 1, end: LineEnd::Start },
        );
        let point = ConstraintPoint::LineEndpoint { line: 0, end: LineEnd::End };
        let corner = treatment_corner(&doc, sketch, point).unwrap();
        assert_eq!(corner.line1, 0);
        assert_eq!(corner.end1, LineEnd::End);
        assert_eq!(corner.line2, 1);
        assert_eq!(corner.end2, LineEnd::Start);
        assert_eq!(corner.v, (10.0, 0.0));
        assert_eq!(corner.a, (0.0, 0.0));
        assert_eq!(corner.b, (10.0, 10.0));
    }

    #[test]
    fn treatment_corner_none_when_vertex_has_only_one_line() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        let point = ConstraintPoint::LineEndpoint { line: 0, end: LineEnd::Start };
        assert!(treatment_corner(&doc, sketch, point).is_none());
    }
}