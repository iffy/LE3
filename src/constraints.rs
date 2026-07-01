//! Sketch constraints backed by the numeric [`crate::sketch_solver`].

use crate::geometric_constraints::{
    line_uv_endpoints, lines_are_parallel, parallel_reference_and_movable, point_uv,
    selected_constraint_refs, ConstraintRef,
};
use crate::model::{
    default_constraint_sign, effective_angle_unit, effective_length_unit, Constraint,
    ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, DimensionTarget,
    DistanceTarget, Document, SketchId,
};
use crate::value::{
    eval_angle_rad_in_doc, eval_length_mm_in_doc, format_angle_display_in, format_diameter_display_in,
    format_length_display_in,
};

/// Index into [`Document::constraints`].
pub type ConstraintId = usize;

fn constraint_sign_from_scalar(sign: f32) -> crate::model::ConstraintSign {
    if sign >= 0.0 { 1 } else { -1 }
}

/// Fill in disambiguation fields from the current sketch geometry.
pub fn finalize_distance_target(
    doc: &Document,
    sketch: SketchId,
    target: DistanceTarget,
) -> Result<DistanceTarget, String> {
    match target {
        DistanceTarget::LineLength(_)
        | DistanceTarget::CircleDiameter(_) => Ok(target),
        DistanceTarget::LineLineDistance { line_a, line_b, .. } => {
            let (line_a, line_b) = normalize_line_pair(line_a, line_b);
            let side = capture_line_line_side(doc, sketch, line_a.clone(), line_b.clone())?;
            Ok(DistanceTarget::LineLineDistance {
                line_a,
                line_b,
                side,
            })
        }
        DistanceTarget::PointPointDistance { anchor, mover, .. } => {
            Ok(capture_point_point_distance(doc, sketch, anchor, mover)?)
        }
        DistanceTarget::PointLineDistance { point, line, .. } => {
            let side = capture_point_line_side(doc, sketch, point.clone(), line.clone())?;
            Ok(DistanceTarget::PointLineDistance { point, line, side })
        }
    }
}

fn capture_line_line_side(
    doc: &Document,
    sketch: SketchId,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
) -> Result<crate::model::ConstraintSign, String> {
    let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
    let ((ax0, ay0), (ax1, ay1)) = line_uv_endpoints(doc, sketch, reference)?;
    let ((bx0, by0), (bx1, by1)) = line_uv_endpoints(doc, sketch, movable)?;
    let du = ax1 - ax0;
    let dv = ay1 - ay0;
    let len = (du * du + dv * dv).sqrt();
    if len < 1e-6 {
        return Err("Reference line has zero length".to_string());
    }
    let perp_u = -dv / len;
    let perp_v = du / len;
    let amu = (ax0 + ax1) * 0.5;
    let amv = (ay0 + ay1) * 0.5;
    let bmu = (bx0 + bx1) * 0.5;
    let bmv = (by0 + by1) * 0.5;
    let signed = (bmu - amu) * perp_u + (bmv - amv) * perp_v;
    Ok(constraint_sign_from_scalar(if signed.abs() < 1e-6 {
        1.0
    } else {
        signed
    }))
}

fn capture_point_line_side(
    doc: &Document,
    sketch: SketchId,
    point: ConstraintPoint,
    line: ConstraintLine,
) -> Result<crate::model::ConstraintSign, String> {
    let (pu, pv) = point_uv(doc, sketch, point)?;
    let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, sketch, line)?;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return Err("Line has zero length".to_string());
    }
    let perp_u = -dy / len;
    let perp_v = dx / len;
    let signed = (pu - x0) * perp_u + (pv - y0) * perp_v;
    Ok(constraint_sign_from_scalar(if signed.abs() < 1e-6 {
        1.0
    } else {
        signed
    }))
}

fn capture_point_point_distance(
    doc: &Document,
    sketch: SketchId,
    anchor: ConstraintPoint,
    mover: ConstraintPoint,
) -> Result<DistanceTarget, String> {
    use crate::geometric_constraints::coincident_mover_and_anchor;
    let (resolved_mover, resolved_anchor) = coincident_mover_and_anchor(anchor, mover);
    let (au, av) = point_uv(doc, sketch, resolved_anchor.clone())?;
    let (mu, mv) = point_uv(doc, sketch, resolved_mover.clone())?;
    let du = mu - au;
    let dv = mv - av;
    let len = (du * du + dv * dv).sqrt();
    let (dir_u, dir_v) = if len < 1e-6 {
        (1.0, 0.0)
    } else {
        (du / len, dv / len)
    };
    Ok(DistanceTarget::PointPointDistance {
        anchor: resolved_anchor,
        mover: resolved_mover,
        dir_u,
        dir_v,
    })
}

/// The rotation sign that reproduces [`angle_natural_legs`]'s unflipped leg directions
/// (i.e. the sign `angle_constraint_display` would need to leave `dir_b` untouched).
pub fn angle_constraint_natural_sign(
    doc: &Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
) -> Option<crate::model::ConstraintSign> {
    let (frame, _center, leg_a, leg_b) = angle_natural_legs(doc, line_a, line_b)?;
    Some(leg_rotation_sign(leg_a.dir, leg_b.dir, frame.normal))
}

fn leg_rotation_sign(
    dir_a: glam::Vec3,
    dir_b: glam::Vec3,
    plane_normal: glam::Vec3,
) -> crate::model::ConstraintSign {
    let cross = dir_a.cross(dir_b).dot(plane_normal.normalize_or_zero());
    constraint_sign_from_scalar(if cross.abs() < 1e-6 { 1.0 } else { cross })
}

/// Which of the (up to 4) angular wedges around the lines' intersection contains
/// `hover_world`, expressed as the [`ConstraintSign`] that reproduces it: the two
/// wedges adjacent to the natural leg pair share `angle_constraint_natural_sign`'s
/// value, the other two (the supplementary angle) share its negation (#40).
pub fn angle_dimension_hover_sign(
    doc: &Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
    hover_world: glam::Vec3,
) -> Option<crate::model::ConstraintSign> {
    let (frame, center, leg_a, leg_b) = angle_natural_legs(doc, line_a, line_b)?;
    let natural_sign = leg_rotation_sign(leg_a.dir, leg_b.dir, frame.normal);
    let theta = leg_a.dir.dot(leg_b.dir).clamp(-1.0, 1.0).acos();
    let radial = hover_world - center;
    if radial.length_squared() < 1e-8 {
        return None;
    }
    let radial = radial.normalize();
    let n = frame.normal.normalize_or_zero();
    let cross = leg_a.dir.cross(radial).dot(n);
    let dot = leg_a.dir.dot(radial).clamp(-1.0, 1.0);
    let phi = cross.atan2(dot);
    // Rotate into the natural leg's own sense of "positive" before folding mod π, so the
    // wedge test below works the same whether the natural pair turns CW or CCW.
    let psi = phi * natural_sign as f32;
    let psi_reduced = psi.rem_euclid(std::f32::consts::PI);
    Some(if psi_reduced <= theta {
        natural_sign
    } else {
        -natural_sign
    })
}

/// Add a distance constraint; returns the new constraint index.
pub fn add_distance_constraint(
    doc: &mut Document,
    sketch: SketchId,
    target: DistanceTarget,
    expression: String,
) -> Result<ConstraintId, String> {
    let expression = expression.trim().to_string();
    if expression.is_empty() {
        return Err("Constraint expression cannot be empty".to_string());
    }
    let target = finalize_distance_target(doc, sketch, target)?;
    validate_distance_target(doc, sketch, target.clone())?;
    if let Some(index) = find_distance_constraint(doc, target.clone()) {
        return Err(format!("Constraint already exists for {target:?} (index {index})"));
    }
    eval_length_mm_in_doc(&expression, doc)
        .filter(|v| *v > 0.0)
        .ok_or_else(|| format!("Invalid constraint expression '{expression}'"))?;
    let id = doc.constraints.len();
    doc.constraints.push(Constraint {
        sketch,
        kind: ConstraintKind::Distance { target },
        expression,
        dim_offset: None,
        name: None,
        deleted: false,
    });
    doc.shape_order.push(crate::model::ShapeKind::Constraint);
    solve_document_constraints(doc)?;
    Ok(id)
}

/// Update an existing constraint expression (distance or angle).
pub fn set_constraint_expression(
    doc: &mut Document,
    index: ConstraintId,
    expression: String,
) -> Result<(), String> {
    let expression = expression.trim().to_string();
    if expression.is_empty() {
        return Err("Constraint expression cannot be empty".to_string());
    }
    let kind = doc
        .constraints
        .get(index)
        .map(|c| c.kind.clone())
        .ok_or_else(|| format!("Constraint {index} not found"))?;
    validate_constraint_expression(doc, &kind, &expression)?;
    doc.constraints[index].expression = expression;
    solve_document_constraints(doc)
}

fn validate_constraint_expression(
    doc: &Document,
    kind: &ConstraintKind,
    expression: &str,
) -> Result<(), String> {
    match kind {
        ConstraintKind::Distance { .. } => {
            eval_length_mm_in_doc(expression, doc)
                .filter(|v| *v > 0.0)
                .ok_or_else(|| format!("Invalid constraint expression '{expression}'"))?;
        }
        ConstraintKind::Angle { .. } => {
            eval_angle_rad_in_doc(expression, doc)
                .filter(|v| *v > 0.0 && *v < std::f32::consts::PI)
                .ok_or_else(|| format!("Invalid angle expression '{expression}'"))?;
        }
        _ => return Err("Constraint expression is not editable".to_string()),
    }
    Ok(())
}

pub fn set_constraint_dim_offset(doc: &mut Document, index: ConstraintId, offset: f32) -> Result<(), String> {
    if doc.constraints.get(index).is_none() {
        return Err(format!("Constraint {index} not found"));
    }
    doc.constraints[index].dim_offset = Some(offset);
    solve_document_constraints(doc)
}

pub fn find_distance_constraint(doc: &Document, target: DistanceTarget) -> Option<ConstraintId> {
    let target = normalize_distance_target(target);
    doc.constraints.iter().position(|c| {
        !c.deleted
            && matches!(&c.kind, ConstraintKind::Distance { target: t } if normalize_distance_target(t.clone()) == target)
    })
}

pub fn find_angle_constraint(
    doc: &Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
) -> Option<ConstraintId> {
    let (line_a, line_b) = normalize_line_pair(line_a, line_b);
    doc.constraints.iter().position(|c| {
        !c.deleted
            && matches!(
                &c.kind,
                ConstraintKind::Angle {
                    line_a: a,
                    line_b: b,
                    ..
                } if *a == line_a && *b == line_b
            )
    })
}

pub fn find_dimension_constraint(doc: &Document, target: DimensionTarget) -> Option<ConstraintId> {
    match target {
        DimensionTarget::Distance(distance) => find_distance_constraint(doc, distance),
        DimensionTarget::Angle {
            line_a,
            line_b,
            rotation_sign: _,
        } => find_angle_constraint(doc, line_a, line_b),
    }
}

pub fn constraint_expression(doc: &Document, index: ConstraintId) -> Option<String> {
    doc.constraints.get(index).map(|c| c.expression.clone())
}

pub fn constraint_evaluated_length(doc: &Document, index: ConstraintId) -> Option<f32> {
    let constraint = doc.constraints.get(index)?;
    let ConstraintKind::Distance { target } = constraint.kind.clone() else {
        return None;
    };
    eval_length_mm_in_doc(&constraint.expression, doc)
        .or_else(|| measured_distance(doc, constraint.sketch, target))
}

pub fn constraint_evaluated_angle(doc: &Document, index: ConstraintId) -> Option<f32> {
    let constraint = doc.constraints.get(index)?;
    let ConstraintKind::Angle {
        line_a,
        line_b,
        rotation_sign,
    } = constraint.kind.clone()
    else {
        return None;
    };
    eval_angle_rad_in_doc(&constraint.expression, doc)
        .or_else(|| measured_angle_between_lines(doc, line_a, line_b, rotation_sign))
}

fn measured_distance(doc: &Document, sketch: SketchId, target: DistanceTarget) -> Option<f32> {
    match target {
        DistanceTarget::LineLength(i) => doc.lines.get(i).map(|l| l.length()),
        DistanceTarget::CircleDiameter(i) => doc.circles.get(i).map(|c| c.diameter()),
        DistanceTarget::LineLineDistance { line_a, line_b, .. } => {
            measured_line_line_distance(doc, sketch, line_a, line_b)
        }
        DistanceTarget::PointPointDistance { anchor, mover, .. } => {
            let (au, av) = point_uv(doc, sketch, anchor).ok()?;
            let (mu, mv) = point_uv(doc, sketch, mover).ok()?;
            Some(((mu - au).hypot(mv - av)).abs())
        }
        DistanceTarget::PointLineDistance { point, line, .. } => {
            measured_point_line_distance(doc, sketch, point, line)
        }
    }
}

fn measured_line_line_distance(
    doc: &Document,
    sketch: SketchId,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
) -> Option<f32> {
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

fn measured_point_line_distance(
    doc: &Document,
    sketch: SketchId,
    point: ConstraintPoint,
    line: ConstraintLine,
) -> Option<f32> {
    let (pu, pv) = point_uv(doc, sketch, point).ok()?;
    let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, sketch, line).ok()?;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return None;
    }
    let perp_u = -dy / len;
    let perp_v = dx / len;
    Some(((pu - x0) * perp_u + (pv - y0) * perp_v).abs())
}

/// Angle between two lines as it will actually be drawn/edited, matching
/// [`angle_constraint_display`]'s leg directions rather than the lines' raw stored
/// endpoint order (which previously made the reported value depend on which way each
/// line happened to be drawn, see #40).
fn measured_angle_between_lines(
    doc: &Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
    rotation_sign: crate::model::ConstraintSign,
) -> Option<f32> {
    let (frame, _center, leg_a, leg_b) = angle_natural_legs(doc, line_a, line_b)?;
    let natural_sign = leg_rotation_sign(leg_a.dir, leg_b.dir, frame.normal);
    let theta = leg_a.dir.dot(leg_b.dir).clamp(-1.0, 1.0).acos();
    Some(if rotation_sign == natural_sign {
        theta
    } else {
        std::f32::consts::PI - theta
    })
}

pub fn constraint_label(doc: &Document, index: ConstraintId) -> String {
    let Some(constraint) = doc.constraints.get(index) else {
        return format!("Constraint {index}");
    };
    let value = match &constraint.kind {
        ConstraintKind::Distance {
            target: DistanceTarget::CircleDiameter(_),
        } => constraint_evaluated_length(doc, index)
            .map(|v| format_diameter_display_in(v, effective_length_unit(doc, constraint.sketch)))
            .unwrap_or_else(|| "?".to_string()),
        ConstraintKind::Distance { .. } => constraint_evaluated_length(doc, index)
            .map(|v| format_length_display_in(v, effective_length_unit(doc, constraint.sketch)))
            .unwrap_or_else(|| "?".to_string()),
        ConstraintKind::Parallel { .. }
        | ConstraintKind::Perpendicular { .. }
        | ConstraintKind::Equal { .. }
        | ConstraintKind::Coincident { .. }
        | ConstraintKind::Midpoint { .. }
        | ConstraintKind::Horizontal { .. }
        | ConstraintKind::Vertical { .. } => String::new(),
        ConstraintKind::Angle { .. } => constraint_evaluated_angle(doc, index)
            .map(|v| format_angle_display_in(v, effective_angle_unit(doc, constraint.sketch)))
            .unwrap_or_else(|| "?".to_string()),
    };
    let target_label = match &constraint.kind {
        ConstraintKind::Distance { target } => distance_target_label(target.clone()),
        ConstraintKind::Parallel { .. } => "Parallel".to_string(),
        ConstraintKind::Perpendicular { .. } => "Perpendicular".to_string(),
        ConstraintKind::Equal { .. } => "Equal".to_string(),
        ConstraintKind::Coincident { .. } => "Coincident".to_string(),
        ConstraintKind::Midpoint { .. } => "Midpoint".to_string(),
        ConstraintKind::Horizontal { .. } => "Horizontal".to_string(),
        ConstraintKind::Vertical { .. } => "Vertical".to_string(),
        ConstraintKind::Angle { .. } => "Angle".to_string(),
    };
    match &constraint.kind {
        ConstraintKind::Distance { .. } | ConstraintKind::Angle { .. } => {
            format!("Constraint {index} ({target_label}, {value})")
        }
        _ => format!("Constraint {index} ({target_label})"),
    }
}

fn distance_target_label(target: DistanceTarget) -> String {
    match target {
        DistanceTarget::LineLength(i) => format!("Line {i} length"),
        DistanceTarget::CircleDiameter(i) => format!("Circle {i} diameter"),
        DistanceTarget::LineLineDistance { .. } => "Line spacing".to_string(),
        DistanceTarget::PointPointDistance { .. } => "Point distance".to_string(),
        DistanceTarget::PointLineDistance { .. } => "Point-line distance".to_string(),
    }
}

fn line_sort_key(line: &ConstraintLine) -> (u8, usize, u8, usize) {
    match line {
        ConstraintLine::Line(i) => (0, *i, 0, 0),
        ConstraintLine::FaceEdge { index, .. } => (2, *index, 0, 0),
    }
}

pub fn normalize_line_pair(a: ConstraintLine, b: ConstraintLine) -> (ConstraintLine, ConstraintLine) {
    if line_sort_key(&a) <= line_sort_key(&b) {
        (a, b)
    } else {
        (b, a)
    }
}

pub fn normalize_distance_target(target: DistanceTarget) -> DistanceTarget {
    match target {
        DistanceTarget::LineLineDistance {
            line_a,
            line_b,
            side,
        } => {
            let (line_a, line_b) = normalize_line_pair(line_a, line_b);
            DistanceTarget::LineLineDistance {
                line_a,
                line_b,
                side,
            }
        }
        other => other,
    }
}

/// Map the current selection to a dimension target in the active sketch.
pub fn dimension_edit_from_selection(
    doc: &Document,
    sketch: SketchId,
    selection: &crate::selection::SceneSelection,
) -> Option<DimensionTarget> {
    let refs = selected_constraint_refs(selection);
    match refs.len() {
        0 => None,
        1 => distance_target_from_selection(doc, sketch, selection)
            .map(DimensionTarget::Distance),
        2 => resolve_two_selection_dimension(doc, sketch, &refs),
        _ => None,
    }
}

fn resolve_two_selection_dimension(
    doc: &Document,
    sketch: SketchId,
    refs: &[ConstraintRef],
) -> Option<DimensionTarget> {
    let lines: Vec<ConstraintLine> = refs
        .iter()
        .filter_map(|reference| match reference {
            ConstraintRef::Line(line) => Some(line.clone()),
            _ => None,
        })
        .collect();
    let points: Vec<ConstraintPoint> = refs
        .iter()
        .filter_map(|reference| match reference {
            ConstraintRef::Point(point) => Some(point.clone()),
            _ => None,
        })
        .collect();

    if lines.len() == 2 {
        let line_a = lines[0].clone();
        let line_b = lines[1].clone();
        validate_line_in_sketch(doc, sketch, line_a.clone()).ok()?;
        validate_line_in_sketch(doc, sketch, line_b.clone()).ok()?;
        let (line_a, line_b) = normalize_line_pair(line_a, line_b);
        if lines_are_parallel(doc, sketch, line_a.clone(), line_b.clone()) {
            Some(DimensionTarget::Distance(DistanceTarget::LineLineDistance {
                line_a,
                line_b,
                side: default_constraint_sign(),
            }))
        } else {
            let rotation_sign = angle_constraint_natural_sign(doc, line_a.clone(), line_b.clone())
                .unwrap_or_else(default_constraint_sign);
            Some(DimensionTarget::Angle {
                line_a,
                line_b,
                rotation_sign,
            })
        }
    } else if points.len() == 2 {
        validate_point_in_sketch(doc, sketch, points[0].clone()).ok()?;
        validate_point_in_sketch(doc, sketch, points[1].clone()).ok()?;
        finalize_distance_target(
            doc,
            sketch,
            DistanceTarget::PointPointDistance {
                anchor: points[0].clone(),
                mover: points[1].clone(),
                dir_u: 1.0,
                dir_v: 0.0,
            },
        )
        .ok()
        .map(DimensionTarget::Distance)
    } else if points.len() == 1 && lines.len() == 1 {
        validate_point_in_sketch(doc, sketch, points[0].clone()).ok()?;
        validate_line_in_sketch(doc, sketch, lines[0].clone()).ok()?;
        Some(DimensionTarget::Distance(DistanceTarget::PointLineDistance {
            point: points[0].clone(),
            line: lines[0].clone(),
            side: default_constraint_sign(),
        }))
    } else {
        None
    }
}

fn validate_line_in_sketch(
    doc: &Document,
    sketch: SketchId,
    line: ConstraintLine,
) -> Result<(), String> {
    match line {
        ConstraintLine::Line(index) => {
            let entity = doc
                .lines
                .get(index)
                .ok_or_else(|| format!("Line {index} not found"))?;
            if entity.sketch != sketch {
                return Err(format!("Line {index} is not in sketch {sketch}"));
            }
        }
        // A face's own edge has no owning sketch — valid for any sketch as long as the
        // underlying extrusion/face still resolves (mirrors `geometric_constraints`'
        // `validate_line_ref`).
        ConstraintLine::FaceEdge { face, index } => {
            if !crate::geometric_constraints::face_edge_valid(doc, &face, index) {
                return Err(format!("Face edge {index} no longer resolves"));
            }
        }
    }
    Ok(())
}

fn validate_point_in_sketch(
    doc: &Document,
    sketch: SketchId,
    point: ConstraintPoint,
) -> Result<(), String> {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => validate_line_in_sketch(
            doc,
            sketch,
            ConstraintLine::Line(line),
        ),
        ConstraintPoint::CircleCenter(circle) => {
            let entity = doc
                .circles
                .get(circle)
                .ok_or_else(|| format!("Circle {circle} not found"))?;
            if entity.sketch != sketch {
                return Err(format!("Circle {circle} is not in sketch {sketch}"));
            }
            Ok(())
        }
        ConstraintPoint::FaceVertex { face, index } => {
            if !crate::geometric_constraints::face_vertex_valid(doc, &face, index) {
                return Err(format!("Face vertex {index} no longer resolves"));
            }
            Ok(())
        }
    }
}

/// Map a single scene selection to a distance target in the active sketch.
pub fn distance_target_from_selection(
    doc: &Document,
    sketch: SketchId,
    selection: &crate::selection::SceneSelection,
) -> Option<DistanceTarget> {
    selection
        .single()
        .and_then(|element| distance_target_from_scene_element(doc, sketch, element))
}

/// Map a scene element to a distance target in the active sketch.
pub fn distance_target_from_scene_element(
    doc: &Document,
    sketch: SketchId,
    element: crate::hierarchy::SceneElement,
) -> Option<DistanceTarget> {
    use crate::hierarchy::SceneElement;
    match element {
        SceneElement::Line(index) => {
            let line = doc.lines.get(index)?;
            (line.sketch == sketch).then_some(DistanceTarget::LineLength(index))
        }
        SceneElement::Circle(index) => {
            let circle = doc.circles.get(index)?;
            (circle.sketch == sketch).then_some(DistanceTarget::CircleDiameter(index))
        }
        _ => None,
    }
}

/// Map a viewport pick to a distance target in the active sketch.
pub fn distance_target_from_pick(
    doc: &Document,
    sketch: SketchId,
    kind: &crate::construction::PickTargetKind,
) -> Option<DistanceTarget> {
    match kind {
        crate::construction::PickTargetKind::Line(index) => {
            let line = doc.lines.get(*index)?;
            (line.sketch == sketch).then_some(DistanceTarget::LineLength(*index))
        }
        crate::construction::PickTargetKind::Circle(index) => {
            let circle = doc.circles.get(*index)?;
            (circle.sketch == sketch).then_some(DistanceTarget::CircleDiameter(*index))
        }
        _ => None,
    }
}

/// Default expression text when starting a new dimension on a segment.
pub fn default_distance_expression(doc: &Document, sketch: SketchId, target: DistanceTarget) -> String {
    measured_distance(doc, sketch, target)
        .map(|v| format_length_display_in(v, effective_length_unit(doc, sketch)))
        .unwrap_or_else(|| "10mm".to_string())
}

pub fn default_angle_expression(
    doc: &Document,
    sketch: SketchId,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
    rotation_sign: crate::model::ConstraintSign,
) -> String {
    measured_angle_between_lines(doc, line_a, line_b, rotation_sign)
        .map(|v| format_angle_display_in(v, effective_angle_unit(doc, sketch)))
        .unwrap_or_else(|| "45 deg".to_string())
}

pub fn default_dimension_expression(doc: &Document, sketch: SketchId, target: DimensionTarget) -> String {
    match target {
        DimensionTarget::Distance(distance) => default_distance_expression(doc, sketch, distance),
        DimensionTarget::Angle {
            line_a,
            line_b,
            rotation_sign,
        } => default_angle_expression(doc, sketch, line_a, line_b, rotation_sign),
    }
}

/// Add an angle constraint with an explicit rotation sign (e.g. the quadrant the user
/// placed it in, see #40); returns the new constraint index.
pub fn add_angle_constraint_with_sign(
    doc: &mut Document,
    sketch: SketchId,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
    rotation_sign: crate::model::ConstraintSign,
    expression: String,
) -> Result<ConstraintId, String> {
    let expression = expression.trim().to_string();
    if expression.is_empty() {
        return Err("Constraint expression cannot be empty".to_string());
    }
    let (line_a, line_b) = normalize_line_pair(line_a, line_b);
    validate_line_in_sketch(doc, sketch, line_a.clone())?;
    validate_line_in_sketch(doc, sketch, line_b.clone())?;
    if line_a == line_b {
        return Err("Angle constraint requires two different lines".to_string());
    }
    if lines_are_parallel(doc, sketch, line_a.clone(), line_b.clone()) {
        return Err("Angle constraint requires non-parallel lines".to_string());
    }
    if let Some(index) = find_angle_constraint(doc, line_a.clone(), line_b.clone()) {
        return Err(format!("Angle constraint already exists (index {index})"));
    }
    let kind = ConstraintKind::Angle {
        line_a,
        line_b,
        rotation_sign,
    };
    validate_constraint_expression(doc, &kind, &expression)?;
    let id = doc.constraints.len();
    doc.constraints.push(Constraint {
        sketch,
        kind,
        expression,
        dim_offset: None,
        name: None,
        deleted: false,
    });
    doc.shape_order.push(crate::model::ShapeKind::Constraint);
    solve_document_constraints(doc)?;
    Ok(id)
}

/// Apply a dimension expression for a new or existing constraint target.
pub fn apply_dimension_expression(
    doc: &mut Document,
    sketch: SketchId,
    target: DimensionTarget,
    expression: &str,
) -> Result<(), String> {
    match target {
        DimensionTarget::Distance(distance) => {
            if let Some(id) = find_distance_constraint(doc, distance.clone()) {
                set_constraint_expression(doc, id, expression.to_string())
            } else {
                add_distance_constraint(doc, sketch, distance, expression.to_string())?;
                Ok(())
            }
        }
        DimensionTarget::Angle {
            line_a,
            line_b,
            rotation_sign,
        } => {
            if let Some(id) = find_angle_constraint(doc, line_a.clone(), line_b.clone()) {
                set_constraint_expression(doc, id, expression.to_string())
            } else {
                add_angle_constraint_with_sign(
                    doc,
                    sketch,
                    line_a,
                    line_b,
                    rotation_sign,
                    expression.to_string(),
                )?;
                Ok(())
            }
        }
    }
}

pub fn validate_distance_target(
    doc: &Document,
    sketch: SketchId,
    target: DistanceTarget,
) -> Result<(), String> {
    match target {
        DistanceTarget::LineLength(i) => {
            let line = doc
                .lines
                .get(i)
                .ok_or_else(|| format!("Line {i} not found"))?;
            if line.sketch != sketch {
                return Err(format!("Line {i} is not in sketch {sketch}"));
            }
        }
        DistanceTarget::CircleDiameter(i) => {
            let circle = doc
                .circles
                .get(i)
                .ok_or_else(|| format!("Circle {i} not found"))?;
            if circle.sketch != sketch {
                return Err(format!("Circle {i} is not in sketch {sketch}"));
            }
        }
        DistanceTarget::LineLineDistance {
            line_a,
            line_b,
            side: _,
        } => {
            validate_line_in_sketch(doc, sketch, line_a.clone())?;
            validate_line_in_sketch(doc, sketch, line_b.clone())?;
            if line_a == line_b {
                return Err("Line spacing requires two different lines".to_string());
            }
            if !lines_are_parallel(doc, sketch, line_a, line_b) {
                return Err("Line spacing requires parallel lines".to_string());
            }
        }
        DistanceTarget::PointPointDistance { anchor, mover, .. } => {
            validate_point_in_sketch(doc, sketch, anchor.clone())?;
            validate_point_in_sketch(doc, sketch, mover.clone())?;
            if anchor == mover {
                return Err("Point distance requires two different points".to_string());
            }
        }
        DistanceTarget::PointLineDistance { point, line, .. } => {
            validate_point_in_sketch(doc, sketch, point.clone())?;
            validate_line_in_sketch(doc, sketch, line.clone())?;
        }
    }
    Ok(())
}

/// Remaining degrees of freedom for a sketch's numeric constraint system.
pub fn sketch_degrees_of_freedom(doc: &Document, sketch: SketchId) -> Result<i32, String> {
    crate::sketch_solver::sketch_dof_remaining(doc, sketch)
}

/// Constraint indices contributing most to an unsatisfied sketch solve.
pub fn sketch_conflicting_constraints(
    doc: &Document,
    sketch: SketchId,
) -> Result<Vec<ConstraintId>, String> {
    crate::sketch_solver::sketch_conflicting_constraints(doc, sketch)
}

/// Apply all distance constraints to sketch geometry.
pub fn solve_document_constraints(doc: &mut Document) -> Result<(), String> {
    solve_document_constraints_with_pins(doc, &[])
}

/// Apply all constraints while keeping pinned sketch points fixed (used during vertex/line drag).
pub fn solve_document_constraints_with_pins(
    doc: &mut Document,
    pins: &[(ConstraintPoint, (f32, f32))],
) -> Result<(), String> {
    if pins.is_empty() {
        clear_legacy_dimension_locks(doc);
    }
    crate::sketch_solver::solve_document_sketches(doc, pins)?;
    if pins.is_empty() {
        let dimension_flags: Vec<_> = doc
            .constraints
            .iter()
            .filter(|constraint| !constraint.deleted)
            .filter_map(|constraint| match &constraint.kind {
                ConstraintKind::Distance { target } => Some((
                    target.clone(),
                    constraint.expression.clone(),
                    constraint.dim_offset,
                )),
                _ => None,
            })
            .collect();
        for (target, expression, dim_offset) in dimension_flags {
            if crate::document_lifecycle::distance_target_alive(doc, &target) {
                sync_legacy_dimension_flags(doc, target, &expression, dim_offset);
            }
        }
    }
    crate::parameters::sync_computed_parameters(doc);
    Ok(())
}

fn clear_legacy_dimension_locks(doc: &mut Document) {
    for line in &mut doc.lines {
        line.length_locked = false;
        line.length_expr = None;
    }
    for circle in &mut doc.circles {
        circle.diameter_locked = false;
        circle.diameter_expr = None;
    }
}

fn sync_legacy_dimension_flags(
    doc: &mut Document,
    target: DistanceTarget,
    expression: &str,
    dim_offset: Option<f32>,
) {
    match target {
        DistanceTarget::LineLength(i) => {
            if let Some(line) = doc.lines.get_mut(i) {
                line.length_locked = true;
                line.length_expr = Some(expression.to_string());
                if dim_offset.is_some() {
                    line.length_dim_offset = dim_offset;
                }
            }
        }
        DistanceTarget::CircleDiameter(i) => {
            if let Some(circle) = doc.circles.get_mut(i) {
                circle.diameter_locked = true;
                circle.diameter_expr = Some(expression.to_string());
                if dim_offset.is_some() {
                    circle.diameter_dim_offset = dim_offset;
                }
            }
        }
        DistanceTarget::LineLineDistance { .. }
        | DistanceTarget::PointPointDistance { .. }
        | DistanceTarget::PointLineDistance { .. } => {}
    }
}

/// Create constraints from legacy `*_locked` fields (pre-constraint documents).
pub fn migrate_legacy_dimensions(doc: &mut Document) {
    let mut pending = Vec::new();
    for (i, line) in doc.lines.iter().enumerate() {
        if line.length_locked {
            let expr = line.length_expr.clone().unwrap_or_else(|| {
                format_length_display_in(line.length(), effective_length_unit(doc, line.sketch))
            });
            if find_distance_constraint(doc, DistanceTarget::LineLength(i)).is_none() {
                pending.push((
                    line.sketch,
                    DistanceTarget::LineLength(i),
                    expr,
                    line.length_dim_offset,
                ));
            }
        }
    }
    for (i, circle) in doc.circles.iter().enumerate() {
        if circle.diameter_locked {
            let expr = circle.diameter_expr.clone().unwrap_or_else(|| {
                format_length_display_in(circle.diameter(), effective_length_unit(doc, circle.sketch))
            });
            if find_distance_constraint(doc, DistanceTarget::CircleDiameter(i)).is_none() {
                pending.push((
                    circle.sketch,
                    DistanceTarget::CircleDiameter(i),
                    expr,
                    circle.diameter_dim_offset,
                ));
            }
        }
    }
    for (sketch, target, expr, dim_offset) in pending {
        let _ = add_distance_constraint_internal(doc, sketch, target, expr, dim_offset);
    }
}

fn add_distance_constraint_internal(
    doc: &mut Document,
    sketch: SketchId,
    target: DistanceTarget,
    expression: String,
    dim_offset: Option<f32>,
) -> Result<ConstraintId, String> {
    let id = doc.constraints.len();
    doc.constraints.push(Constraint {
        sketch,
        kind: ConstraintKind::Distance { target },
        expression,
        dim_offset,
        name: None,
        deleted: false,
    });
    doc.shape_order.push(crate::model::ShapeKind::Constraint);
    Ok(id)
}

/// World-space segment endpoints for a distance constraint, if geometry exists.
pub fn constraint_segment_endpoints(
    doc: &Document,
    index: ConstraintId,
) -> Option<(glam::Vec3, glam::Vec3)> {
    let constraint = doc.constraints.get(index)?;
    match &constraint.kind {
        ConstraintKind::Distance { target } => {
            distance_target_segment_endpoints(doc, constraint.sketch, target.clone())
        }
        ConstraintKind::Angle { .. } => None,
        _ => None,
    }
}

/// World-space endpoints for displaying a distance dimension.
pub fn distance_target_segment_endpoints(
    doc: &Document,
    sketch: SketchId,
    target: DistanceTarget,
) -> Option<(glam::Vec3, glam::Vec3)> {
    distance_target_segment_endpoints_inner(doc, sketch, target)
}

fn local_to_world_for_target(doc: &Document, u: f32, v: f32, sketch: SketchId) -> Option<glam::Vec3> {
    let frame = crate::face::sketch_geometry_frame(doc, sketch)?;
    Some(crate::face::local_to_world(&frame, u, v))
}

fn distance_target_segment_endpoints_inner(
    doc: &Document,
    sketch: SketchId,
    target: DistanceTarget,
) -> Option<(glam::Vec3, glam::Vec3)> {
    match target.clone() {
        DistanceTarget::LineLength(i) => {
            let line = doc.lines.get(i)?;
            crate::face::line_world_endpoints(doc, line)
        }
        DistanceTarget::CircleDiameter(i) => {
            let circle = doc.circles.get(i)?;
            crate::face::circle_world_diameter_endpoints(doc, circle)
        }
        DistanceTarget::LineLineDistance {
            line_a,
            line_b,
            side,
        } => {
            let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
            let ((ax0, ay0), (ax1, ay1)) = line_uv_endpoints(doc, sketch, reference).ok()?;
            let ((bx0, by0), (bx1, by1)) = line_uv_endpoints(doc, sketch, movable).ok()?;
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
            let current_signed = (bmu - amu) * perp_u + (bmv - amv) * perp_v;
            let a = local_to_world_for_target(doc, amu, amv, sketch)?;
            let b = local_to_world_for_target(
                doc,
                amu + perp_u * current_signed,
                amv + perp_v * current_signed,
                sketch,
            )?;
            let _ = side;
            Some((a, b))
        }
        DistanceTarget::PointPointDistance { anchor, mover, .. } => {
            let (au, av) = point_uv(doc, sketch, anchor).ok()?;
            let (mu, mv) = point_uv(doc, sketch, mover).ok()?;
            Some((
                local_to_world_for_target(doc, au, av, sketch)?,
                local_to_world_for_target(doc, mu, mv, sketch)?,
            ))
        }
        DistanceTarget::PointLineDistance { point, line, .. } => {
            let (pu, pv) = point_uv(doc, sketch, point).ok()?;
            let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, sketch, line).ok()?;
            let dx = x1 - x0;
            let dy = y1 - y0;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-6 {
                return None;
            }
            let perp_u = -dy / len;
            let perp_v = dx / len;
            let signed = (pu - x0) * perp_u + (pv - y0) * perp_v;
            let foot_u = pu - perp_u * signed;
            let foot_v = pv - perp_v * signed;
            Some((
                local_to_world_for_target(doc, pu, pv, sketch)?,
                local_to_world_for_target(doc, foot_u, foot_v, sketch)?,
            ))
        }
    }
}

fn line_sketch(doc: &Document, line: ConstraintLine) -> Option<SketchId> {
    match line {
        ConstraintLine::Line(index) => doc.lines.get(index).map(|l| l.sketch),
        // A face's own edge has no owning sketch of its own (it's referenced *from* a sketch,
        // not owned by one) — angle constraints/display against a `FaceEdge` reference aren't
        // supported (out of scope for #26/#27, which only asks for coincident-to-vertex and
        // point-line-distance-to-edge), so this degrades to "no display" rather than a panic.
        ConstraintLine::FaceEdge { .. } => None,
    }
}

/// World-space geometry for rendering and interacting with an angle constraint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AngleConstraintDisplay {
    pub center: glam::Vec3,
    /// Unit direction along the reference line from the intersection.
    pub dir_a: glam::Vec3,
    /// Unit direction along the movable line from the intersection.
    pub dir_b: glam::Vec3,
    /// Root of the reference leg on (or nearest to) the reference segment.
    pub leg_a_root: glam::Vec3,
    /// Root of the movable leg on (or nearest to) the movable segment.
    pub leg_b_root: glam::Vec3,
    pub extend_a: bool,
    pub extend_b: bool,
}

#[derive(Clone, Copy)]
struct LineAngleLeg {
    dir: glam::Vec3,
    root: glam::Vec3,
    extend: bool,
}

fn line_angle_leg(
    frame: &crate::face::SketchFrame,
    a0: (f32, f32),
    a1: (f32, f32),
    center_uv: (f32, f32),
) -> Option<LineAngleLeg> {
    let du = a1.0 - a0.0;
    let dv = a1.1 - a0.1;
    let len = (du * du + dv * dv).sqrt();
    if len < 1e-6 {
        return None;
    }
    let dir_u = du / len;
    let dir_v = dv / len;
    let ca_u = center_uv.0 - a0.0;
    let ca_v = center_uv.1 - a0.1;
    let t = ca_u * dir_u + ca_v * dir_v;
    let da = (a0.0 - center_uv.0).hypot(a0.1 - center_uv.1);
    let db = (a1.0 - center_uv.0).hypot(a1.1 - center_uv.1);
    let sign = if da >= db { 1.0 } else { -1.0 };
    let ray_u = dir_u * sign;
    let ray_v = dir_v * sign;
    let dir = crate::dimensions::uv_dir_to_world(frame.u_axis, frame.v_axis, ray_u, ray_v);
    if dir.length_squared() < 1e-8 {
        return None;
    }
    let extend = t < -1e-3 || t > len + 1e-3;
    let root_uv = if t < 0.0 {
        a0
    } else if t > len {
        a1
    } else {
        center_uv
    };
    let root = crate::face::local_to_world(frame, root_uv.0, root_uv.1);
    Some(LineAngleLeg {
        dir: dir.normalize(),
        root,
        extend,
    })
}

/// Intersection point and unflipped leg directions for a line pair, shared by
/// [`angle_constraint_display`], [`angle_constraint_natural_sign`], and
/// [`angle_dimension_hover_sign`] so they all agree on what "natural" means (#40).
fn angle_natural_legs(
    doc: &Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
) -> Option<(crate::face::SketchFrame, glam::Vec3, LineAngleLeg, LineAngleLeg)> {
    let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
    let sketch = line_sketch(doc, reference.clone())?;
    let ((ax0, ay0), (ax1, ay1)) = line_uv_endpoints(doc, sketch, reference).ok()?;
    let ((bx0, by0), (bx1, by1)) = line_uv_endpoints(doc, sketch, movable).ok()?;
    let (cu, cv) = line_intersection_uv((ax0, ay0), (ax1, ay1), (bx0, by0), (bx1, by1))?;
    let frame = crate::face::sketch_geometry_frame(doc, sketch)?;
    let center = crate::face::local_to_world(&frame, cu, cv);
    let leg_a = line_angle_leg(&frame, (ax0, ay0), (ax1, ay1), (cu, cv))?;
    let leg_b = line_angle_leg(&frame, (bx0, by0), (bx1, by1), (cu, cv))?;
    Some((frame, center, leg_a, leg_b))
}

pub fn angle_constraint_display(
    doc: &Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
    rotation_sign: crate::model::ConstraintSign,
) -> Option<AngleConstraintDisplay> {
    let (frame, center, leg_a, mut leg_b) = angle_natural_legs(doc, line_a, line_b)?;
    let natural_sign = leg_rotation_sign(leg_a.dir, leg_b.dir, frame.normal);
    if rotation_sign != natural_sign {
        leg_b.dir = -leg_b.dir;
    }
    Some(AngleConstraintDisplay {
        center,
        dir_a: leg_a.dir,
        dir_b: leg_b.dir,
        leg_a_root: leg_a.root,
        leg_b_root: leg_b.root,
        extend_a: leg_a.extend,
        extend_b: leg_b.extend,
    })
}

/// Angle (radians, 0..π) from a sketch-plane hit relative to the reference direction.
pub fn angle_rad_from_sketch_hit(
    display: &AngleConstraintDisplay,
    plane_normal: glam::Vec3,
    hit: glam::Vec3,
) -> Option<f32> {
    let radial = hit - display.center;
    if radial.length_squared() < 1e-8 {
        return None;
    }
    let radial_n = radial.normalize();
    let cross = display.dir_a.cross(radial_n);
    let sin = cross.dot(plane_normal.normalize_or_zero());
    let cos = display.dir_a.dot(radial_n).clamp(-1.0, 1.0);
    let angle = sin.atan2(cos).abs();
    if angle <= 1e-6 || angle >= std::f32::consts::PI - 1e-6 {
        return None;
    }
    Some(angle)
}

/// Update an angle constraint value from a gizmo drag (expression + solve).
pub fn set_constraint_angle_value(
    doc: &mut Document,
    index: ConstraintId,
    angle_rad: f32,
) -> Result<(), String> {
    let angle_rad = angle_rad.clamp(1e-4, std::f32::consts::PI - 1e-4);
    let unit = doc
        .constraints
        .get(index)
        .map(|c| effective_angle_unit(doc, c.sketch))
        .unwrap_or(doc.default_angle_unit);
    set_constraint_expression(doc, index, format_angle_display_in(angle_rad, unit))
}

fn line_intersection_uv(
    a0: (f32, f32),
    a1: (f32, f32),
    b0: (f32, f32),
    b1: (f32, f32),
) -> Option<(f32, f32)> {
    let dax = a1.0 - a0.0;
    let day = a1.1 - a0.1;
    let dbx = b1.0 - b0.0;
    let dby = b1.1 - b0.1;
    let denom = dax * dby - day * dbx;
    if denom.abs() < 1e-8 {
        return None;
    }
    let t = ((b0.0 - a0.0) * dby - (b0.1 - a0.1) * dbx) / denom;
    Some((a0.0 + dax * t, a0.1 + day * t))
}

/// The line a point lies on by virtue of being one of its endpoints, if any.
fn endpoint_line(point: ConstraintPoint) -> Option<ConstraintLine> {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => Some(ConstraintLine::Line(line)),
        _ => None,
    }
}

/// `(point, line)` pairs that a constraint pins to a *specific* spot on a line: a midpoint
/// constraint, or a coincidence between a free point and a line's endpoint.
fn point_line_pins(kind: &ConstraintKind) -> Vec<(ConstraintPoint, ConstraintLine)> {
    match kind {
        ConstraintKind::Midpoint { point, line } => vec![(point.clone(), line.clone())],
        ConstraintKind::Coincident {
            a: ConstraintEntity::Point(pa),
            b: ConstraintEntity::Point(pb),
        } => {
            let mut pins = Vec::new();
            if let Some(line) = endpoint_line(pb.clone()) {
                pins.push((pa.clone(), line));
            }
            if let Some(line) = endpoint_line(pa.clone()) {
                pins.push((pb.clone(), line));
            }
            pins
        }
        _ => Vec::new(),
    }
}

/// Whether a coincident constraint's entities are exactly the generic point-on-line pair
/// `(point, line)` (in either order).
fn is_point_on_line(
    a: &ConstraintEntity,
    b: &ConstraintEntity,
    point: &ConstraintPoint,
    line: &ConstraintLine,
) -> bool {
    matches!(
        (a, b),
        (ConstraintEntity::Point(p), ConstraintEntity::Line(l))
            | (ConstraintEntity::Line(l), ConstraintEntity::Point(p))
        if p == point && l == line
    )
}

/// A point constrained coincident with a *specific* point on a line (its endpoint or
/// midpoint) makes an earlier generic point-on-line coincidence for that same point and line
/// redundant. Mark such constraints deleted so the more specific constraint wins (#23).
/// `new_index` is the just-added constraint that should be kept.
pub fn remove_subsumed_point_on_line(doc: &mut Document, sketch: SketchId, new_index: usize) {
    let Some(new) = doc.constraints.get(new_index) else {
        return;
    };
    if new.deleted || new.sketch != sketch {
        return;
    }
    let pins = point_line_pins(&new.kind);
    if pins.is_empty() {
        return;
    }
    for i in 0..doc.constraints.len() {
        if i == new_index {
            continue;
        }
        let c = &doc.constraints[i];
        if c.deleted || c.sketch != sketch {
            continue;
        }
        if let ConstraintKind::Coincident { a, b } = &c.kind {
            if pins
                .iter()
                .any(|(point, line)| is_point_on_line(a, b, point, line))
            {
                doc.constraints[i].deleted = true;
            }
        }
    }
}

pub fn propagate_parameter_rename_to_constraints(doc: &mut Document, old: &str, new: &str) {
    if old == new {
        return;
    }
    for constraint in &mut doc.constraints {
        constraint.expression =
            crate::value::substitute_parameter_name(&constraint.expression, old, new);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Circle, Document, FaceId, Line, ShapeKind};

    fn sketch_doc() -> (Document, SketchId) {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        (doc, sketch)
    }

    #[test]
    fn add_distance_constraint_for_line() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        let id = add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::LineLength(0),
            "5mm".to_string(),
        )
        .unwrap();
        assert_eq!(id, 0);
        assert!((doc.lines[0].length() - 5.0).abs() < 1e-3);
        assert!(doc.lines[0].length_locked);
    }

    fn push_coincident(doc: &mut Document, sketch: SketchId, kind: ConstraintKind) -> usize {
        let id = doc.constraints.len();
        doc.constraints.push(Constraint {
            sketch,
            kind,
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        doc.shape_order.push(ShapeKind::Constraint);
        id
    }

    #[test]
    fn endpoint_coincidence_subsumes_point_on_line() {
        use crate::model::LineEnd;
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 3.0, 4.0, 6.0, 8.0));
        let free = ConstraintPoint::LineEndpoint {
            line: 1,
            end: LineEnd::Start,
        };
        let on_line = push_coincident(
            &mut doc,
            sketch,
            ConstraintKind::Coincident {
                a: ConstraintEntity::Point(free.clone()),
                b: ConstraintEntity::Line(ConstraintLine::Line(0)),
            },
        );
        // Later: pin the same point to a specific endpoint of line 0.
        let specific = push_coincident(
            &mut doc,
            sketch,
            ConstraintKind::Coincident {
                a: ConstraintEntity::Point(free),
                b: ConstraintEntity::Point(ConstraintPoint::LineEndpoint {
                    line: 0,
                    end: LineEnd::End,
                }),
            },
        );
        remove_subsumed_point_on_line(&mut doc, sketch, specific);
        assert!(doc.constraints[on_line].deleted, "generic point-on-line should be removed");
        assert!(!doc.constraints[specific].deleted, "specific coincidence is kept");
    }

    #[test]
    fn midpoint_subsumes_point_on_line_but_not_other_lines() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 5.0, 10.0, 5.0));
        let pt = ConstraintPoint::CircleCenter(0);
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 5.0, 0.0, 1.0, 0.0));
        let on_line0 = push_coincident(
            &mut doc,
            sketch,
            ConstraintKind::Coincident {
                a: ConstraintEntity::Point(pt.clone()),
                b: ConstraintEntity::Line(ConstraintLine::Line(0)),
            },
        );
        let on_line1 = push_coincident(
            &mut doc,
            sketch,
            ConstraintKind::Coincident {
                a: ConstraintEntity::Point(pt.clone()),
                b: ConstraintEntity::Line(ConstraintLine::Line(1)),
            },
        );
        let mid = push_coincident(
            &mut doc,
            sketch,
            ConstraintKind::Midpoint {
                point: pt,
                line: ConstraintLine::Line(0),
            },
        );
        remove_subsumed_point_on_line(&mut doc, sketch, mid);
        assert!(doc.constraints[on_line0].deleted, "midpoint subsumes point-on-line-0");
        assert!(!doc.constraints[on_line1].deleted, "point-on-line-1 is unrelated");
    }

    #[test]
    fn sketch_degrees_of_freedom_reports_positive_for_open_line() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        assert!(sketch_degrees_of_freedom(&doc, sketch).unwrap() > 0);
    }

    #[test]
    fn set_constraint_expression_updates_geometry() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::LineLength(0),
            "10mm".to_string(),
        )
        .unwrap();
        set_constraint_expression(&mut doc, 0, "15mm".to_string()).unwrap();
        assert!((doc.lines[0].length() - 15.0).abs() < 1e-3);
    }

    #[test]
    fn constraint_label_starts_with_constraint() {
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
        let label = constraint_label(&doc, 0);
        assert!(label.starts_with("Constraint 0"));
        assert!(label.contains("Line 0 length"));
        assert!(label.contains("10.0 mm"));
    }

    #[test]
    fn distance_target_from_line_pick_requires_active_sketch() {
        let (doc, sketch) = sketch_doc();
        let mut doc = doc;
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 5.0, 0.0));
        let kind = crate::construction::PickTargetKind::Line(0);
        assert_eq!(
            distance_target_from_pick(&doc, sketch, &kind),
            Some(DistanceTarget::LineLength(0))
        );
        assert_eq!(distance_target_from_pick(&doc, sketch + 1, &kind), None);
    }

    #[test]
    fn add_distance_constraint_for_circle_diameter() {
        let (mut doc, sketch) = sketch_doc();
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Circle);
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::CircleDiameter(0),
            "30mm".to_string(),
        )
        .unwrap();
        assert!((doc.circles[0].r - 15.0).abs() < 1e-3);
        assert!((doc.circles[0].diameter() - 30.0).abs() < 1e-3);
        assert!(doc.circles[0].diameter_locked);
    }

    #[test]
    fn circle_constraint_label_uses_diameter_prefix() {
        let (mut doc, sketch) = sketch_doc();
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 0.0, 0.0, 5.0, 0.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::CircleDiameter(0),
            "10mm".to_string(),
        )
        .unwrap();
        let label = constraint_label(&doc, 0);
        assert!(label.contains("Ø10.0 mm"));
        assert!(label.contains("Circle 0 diameter"));
    }

    #[test]
    fn constraint_label_respects_document_default_length_unit() {
        // #85: a document-wide default unit of inches must show up in constraint/dimension
        // labels, not stay hardcoded to mm.
        let (mut doc, sketch) = sketch_doc();
        doc.default_length_unit = crate::value::LengthUnit::In;
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 25.4, 0.0));
        add_distance_constraint(&mut doc, sketch, DistanceTarget::LineLength(0), "25.4mm".to_string())
            .unwrap();
        let label = constraint_label(&doc, 0);
        assert!(label.contains("1.0 in"), "expected inches in {label:?}");
        assert!(!label.contains("mm"), "should not show mm: {label:?}");
    }

    #[test]
    fn constraint_label_respects_sketch_length_unit_override() {
        // #85: a per-sketch override takes priority over the document default.
        let (mut doc, sketch) = sketch_doc();
        doc.default_length_unit = crate::value::LengthUnit::Mm;
        doc.sketches[sketch].length_unit = Some(crate::value::LengthUnit::Ft);
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 0.0, 0.0, 152.4, 0.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::CircleDiameter(0),
            "304.8mm".to_string(),
        )
        .unwrap();
        let label = constraint_label(&doc, 0);
        assert!(label.contains("1.0 ft"), "expected feet in {label:?}");
    }

    #[test]
    fn dimension_edit_from_two_parallel_lines() {
        use crate::hierarchy::SceneElement;
        use crate::model::{ConstraintLine, DimensionTarget};
        use crate::selection::{click_scene_selection, SceneSelection};

        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 5.0, 10.0, 5.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.shape_order.push(ShapeKind::Line);

        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        assert_eq!(
            dimension_edit_from_selection(&doc, sketch, &sel),
            Some(DimensionTarget::Distance(DistanceTarget::LineLineDistance {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
                side: 1,
            }))
        );
    }

    #[test]
    fn dimension_edit_from_two_non_parallel_lines() {
        use crate::hierarchy::SceneElement;
        use crate::model::{ConstraintLine, DimensionTarget};
        use crate::selection::{click_scene_selection, SceneSelection};

        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 0.0, 10.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.shape_order.push(ShapeKind::Line);

        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        assert_eq!(
            dimension_edit_from_selection(&doc, sketch, &sel),
            Some(DimensionTarget::Angle {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
                rotation_sign: 1,
            })
        );
    }

    #[test]
    fn add_angle_constraint_rotates_line() {
        use crate::model::ConstraintLine;

        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 10.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.shape_order.push(ShapeKind::Line);
        let rotation_sign =
            angle_constraint_natural_sign(&doc, ConstraintLine::Line(0), ConstraintLine::Line(1))
                .unwrap();
        add_angle_constraint_with_sign(
            &mut doc,
            sketch,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
            rotation_sign,
            "45".to_string(),
        )
        .unwrap();
        let angle = measured_angle_between_lines(
            &doc,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
            rotation_sign,
        )
        .unwrap();
        assert!((angle.to_degrees() - 45.0).abs() < 1.0, "angle={}", angle.to_degrees());
    }

    #[test]
    fn add_line_line_distance_constraint() {
        use crate::model::ConstraintLine;

        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 8.0, 10.0, 8.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.shape_order.push(ShapeKind::Line);
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::LineLineDistance {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
                side: 1,
            },
            "5mm".to_string(),
        )
        .unwrap();
        let dist = measured_line_line_distance(
            &doc,
            sketch,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
        )
        .unwrap();
        assert!((dist - 5.0).abs() < 0.2, "dist={dist}");
        let ConstraintKind::Distance {
            target:
                DistanceTarget::LineLineDistance {
                    side, ..
                },
        } = doc.constraints[0].kind
        else {
            panic!("expected line spacing constraint");
        };
        assert_eq!(side, 1);
        assert!((doc.lines[1].y0 - 5.0).abs() < 0.2);
        set_constraint_expression(&mut doc, 0, "12mm".to_string()).unwrap();
        assert!((doc.lines[1].y0 - 12.0).abs() < 0.2, "y0={}", doc.lines[1].y0);
    }

    #[test]
    fn line_line_distance_keeps_negative_side() {
        use crate::model::ConstraintLine;

        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, -8.0, 10.0, -8.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.shape_order.push(ShapeKind::Line);
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::LineLineDistance {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
                side: 1,
            },
            "5mm".to_string(),
        )
        .unwrap();
        let ConstraintKind::Distance {
            target:
                DistanceTarget::LineLineDistance {
                    side, ..
                },
        } = doc.constraints[0].kind
        else {
            panic!("expected line spacing constraint");
        };
        assert_eq!(side, -1);
        assert!(doc.lines[1].y0 < -0.5, "y0={}", doc.lines[1].y0);
        set_constraint_expression(&mut doc, 0, "3mm".to_string()).unwrap();
        assert!(
            doc.lines[1].y0 < -0.5 && (doc.lines[1].y0 + 3.0).abs() < 0.2,
            "y0={}",
            doc.lines[1].y0
        );
    }

    #[test]
    fn point_line_distance_keeps_negative_side() {
        use crate::model::{ConstraintLine, ConstraintPoint, LineEnd};

        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 5.0, -4.0, 6.0, -4.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.shape_order.push(ShapeKind::Line);
        let point = ConstraintPoint::LineEndpoint {
            line: 1,
            end: LineEnd::Start,
        };
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::PointLineDistance {
                point: point.clone(),
                line: ConstraintLine::Line(0),
                side: 1,
            },
            "3mm".to_string(),
        )
        .unwrap();
        let ConstraintKind::Distance {
            target:
                DistanceTarget::PointLineDistance {
                    side, ..
                },
        } = doc.constraints[0].kind
        else {
            panic!("expected point-line distance constraint");
        };
        assert_eq!(side, -1);
        let (_pu, pv) = point_uv(&doc, sketch, point).unwrap();
        assert!(pv < -0.5, "pv={pv}");
        assert!((pv + 3.0).abs() < 0.2, "pv={pv}");
    }

    #[test]
    fn point_point_distance_preserves_direction() {
        use crate::model::{ConstraintPoint, LineEnd};

        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 1.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 3.0, 4.0, 4.0, 4.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.shape_order.push(ShapeKind::Line);
        let anchor = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };
        let mover = ConstraintPoint::LineEndpoint {
            line: 1,
            end: LineEnd::Start,
        };
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::PointPointDistance {
                anchor,
                mover: mover.clone(),
                dir_u: 1.0,
                dir_v: 0.0,
            },
            "10mm".to_string(),
        )
        .unwrap();
        let ConstraintKind::Distance {
            target:
                DistanceTarget::PointPointDistance {
                    dir_u,
                    dir_v,
                    ..
                },
        } = doc.constraints[0].kind
        else {
            panic!("expected point-point distance constraint");
        };
        assert!((dir_u - 0.6).abs() < 0.01, "dir_u={dir_u}");
        assert!((dir_v - 0.8).abs() < 0.01, "dir_v={dir_v}");
        let (mu, mv) = point_uv(&doc, sketch, mover).unwrap();
        assert!((mu - 6.0).abs() < 0.2, "mu={mu}");
        assert!((mv - 8.0).abs() < 0.2, "mv={mv}");
    }

    #[test]
    fn angle_display_extends_segment_when_intersection_is_off_line() {
        use crate::model::ConstraintLine;

        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 5.0, 10.0, 5.0, 20.0));
        let rotation_sign =
            angle_constraint_natural_sign(&doc, ConstraintLine::Line(0), ConstraintLine::Line(1))
                .unwrap();
        let display = angle_constraint_display(
            &doc,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
            rotation_sign,
        )
        .unwrap();
        assert!(!display.extend_a);
        assert!(display.extend_b);
    }

    #[test]
    fn angle_rad_from_sketch_hit_returns_acute_angle() {
        use crate::model::ConstraintLine;

        let (doc, sketch) = sketch_doc();
        let mut doc = doc;
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 10.0));
        let rotation_sign =
            angle_constraint_natural_sign(&doc, ConstraintLine::Line(0), ConstraintLine::Line(1))
                .unwrap();
        let display = angle_constraint_display(
            &doc,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
            rotation_sign,
        )
        .unwrap();
        let frame = crate::face::sketch_geometry_frame(&doc, sketch).unwrap();
        let hit = display.center + display.dir_b * 5.0;
        let angle = angle_rad_from_sketch_hit(&display, frame.normal, hit).unwrap();
        assert!((angle.to_degrees() - 45.0).abs() < 1.0, "angle={}", angle.to_degrees());
    }

    #[test]
    fn angle_constraint_keeps_clockwise_rotation_sign() {
        use crate::model::ConstraintLine;

        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, -10.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.shape_order.push(ShapeKind::Line);
        let rotation_sign =
            angle_constraint_natural_sign(&doc, ConstraintLine::Line(0), ConstraintLine::Line(1))
                .unwrap();
        assert_eq!(rotation_sign, -1);
        add_angle_constraint_with_sign(
            &mut doc,
            sketch,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
            rotation_sign,
            "45".to_string(),
        )
        .unwrap();
        let ConstraintKind::Angle {
            rotation_sign, ..
        } = doc.constraints[0].kind
        else {
            panic!("expected angle constraint");
        };
        assert_eq!(rotation_sign, -1);
        assert!(doc.lines[1].y1 < -5.0, "y1={}", doc.lines[1].y1);
    }

    #[test]
    fn angle_dimension_hover_sign_distinguishes_supplementary_quadrants() {
        use crate::model::ConstraintLine;

        // Baseline with a line leaving its midpoint at ~80 degrees, matching the #40 repro
        // (a line crossing near the middle of another at ~80 degrees was reported as 111.5).
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 100.0, 0.0));
        let angle_rad = 80f32.to_radians();
        doc.lines.push(Line::from_local_endpoints(
            sketch,
            50.0,
            0.0,
            50.0 + 50.0 * angle_rad.cos(),
            50.0 * angle_rad.sin(),
        ));
        let line_a = ConstraintLine::Line(0);
        let line_b = ConstraintLine::Line(1);

        let natural_sign = angle_constraint_natural_sign(&doc, line_a.clone(), line_b.clone()).unwrap();
        let display_natural =
            angle_constraint_display(&doc, line_a.clone(), line_b.clone(), natural_sign).unwrap();
        let display_flipped =
            angle_constraint_display(&doc, line_a.clone(), line_b.clone(), -natural_sign).unwrap();

        let hover_natural =
            display_natural.center + (display_natural.dir_a + display_natural.dir_b).normalize() * 10.0;
        let hover_flipped =
            display_flipped.center + (display_flipped.dir_a + display_flipped.dir_b).normalize() * 10.0;

        assert_eq!(
            angle_dimension_hover_sign(&doc, line_a.clone(), line_b.clone(), hover_natural),
            Some(natural_sign)
        );
        assert_eq!(
            angle_dimension_hover_sign(&doc, line_a.clone(), line_b.clone(), hover_flipped),
            Some(-natural_sign)
        );

        // The two interpretations are supplementary, and one of them must be the ~80 degree
        // angle that was actually drawn — not always 180-80=100ish (#40).
        let natural_angle =
            measured_angle_between_lines(&doc, line_a.clone(), line_b.clone(), natural_sign).unwrap();
        let flipped_angle =
            measured_angle_between_lines(&doc, line_a, line_b, -natural_sign).unwrap();
        assert!((natural_angle + flipped_angle - std::f32::consts::PI).abs() < 1e-3);
        assert!(
            (natural_angle.to_degrees() - 80.0).abs() < 1.0
                || (flipped_angle.to_degrees() - 80.0).abs() < 1.0,
            "natural={} flipped={}",
            natural_angle.to_degrees(),
            flipped_angle.to_degrees()
        );
    }

    #[test]
    fn rejects_duplicate_constraint_on_same_target() {
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
        assert!(add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::LineLength(0),
            "5mm".to_string(),
        )
        .is_err());
    }
}