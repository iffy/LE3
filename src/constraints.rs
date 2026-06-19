//! Sketch constraints and a lightweight distance constraint solver.
//!
//! Distance constraints are the first constraint kind: they fix the length of a
//! line segment or a rectangle width/height. Each constraint is stored as a
//! first-class document element and evaluated when parameters or expressions change.

use crate::geometric_constraints::{
    line_direction_uv, line_uv_endpoints, lines_are_parallel, parallel_reference_and_movable,
    point_uv, selected_constraint_refs, set_line_uv_endpoints, set_point_uv, translate_line,
    ConstraintRef,
};
use crate::model::{
    Constraint, ConstraintKind, ConstraintLine, ConstraintPoint, DimensionTarget, DistanceTarget,
    Document, RectEdge, SketchId,
};
use crate::value::{
    eval_angle_rad_in_doc, eval_length_mm_in_doc, format_angle_display, format_diameter_display,
    format_length_display,
};

/// Index into [`Document::constraints`].
pub type ConstraintId = usize;

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
    validate_distance_target(doc, sketch, target)?;
    if let Some(index) = find_distance_constraint(doc, target) {
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
        .map(|c| c.kind)
        .ok_or_else(|| format!("Constraint {index} not found"))?;
    validate_constraint_expression(doc, kind, &expression)?;
    doc.constraints[index].expression = expression;
    solve_document_constraints(doc)
}

fn validate_constraint_expression(
    doc: &Document,
    kind: ConstraintKind,
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
        matches!(&c.kind, ConstraintKind::Distance { target: t } if normalize_distance_target(*t) == target)
    })
}

pub fn find_angle_constraint(
    doc: &Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
) -> Option<ConstraintId> {
    let (line_a, line_b) = normalize_line_pair(line_a, line_b);
    doc.constraints.iter().position(|c| {
        matches!(
            c.kind,
            ConstraintKind::Angle {
                line_a: a,
                line_b: b,
            } if a == line_a && b == line_b
        )
    })
}

pub fn find_dimension_constraint(doc: &Document, target: DimensionTarget) -> Option<ConstraintId> {
    match target {
        DimensionTarget::Distance(distance) => find_distance_constraint(doc, distance),
        DimensionTarget::Angle { line_a, line_b } => find_angle_constraint(doc, line_a, line_b),
    }
}

pub fn constraint_expression(doc: &Document, index: ConstraintId) -> Option<String> {
    doc.constraints.get(index).map(|c| c.expression.clone())
}

pub fn constraint_evaluated_length(doc: &Document, index: ConstraintId) -> Option<f32> {
    let constraint = doc.constraints.get(index)?;
    let ConstraintKind::Distance { target } = constraint.kind else {
        return None;
    };
    eval_length_mm_in_doc(&constraint.expression, doc)
        .or_else(|| measured_distance(doc, target))
}

pub fn constraint_evaluated_angle(doc: &Document, index: ConstraintId) -> Option<f32> {
    let constraint = doc.constraints.get(index)?;
    let ConstraintKind::Angle { line_a, line_b } = constraint.kind else {
        return None;
    };
    eval_angle_rad_in_doc(&constraint.expression, doc)
        .or_else(|| measured_angle_between_lines(doc, line_a, line_b))
}

fn measured_distance(doc: &Document, target: DistanceTarget) -> Option<f32> {
    match target {
        DistanceTarget::LineLength(i) => doc.lines.get(i).map(|l| l.length()),
        DistanceTarget::RectWidth(i) => doc.rects.get(i).map(|r| r.w),
        DistanceTarget::RectHeight(i) => doc.rects.get(i).map(|r| r.h),
        DistanceTarget::CircleDiameter(i) => doc.circles.get(i).map(|c| c.diameter()),
        DistanceTarget::LineLineDistance { line_a, line_b } => {
            measured_line_line_distance(doc, line_a, line_b)
        }
        DistanceTarget::PointPointDistance { a, b } => {
            let (au, av) = point_uv(doc, a).ok()?;
            let (bu, bv) = point_uv(doc, b).ok()?;
            Some(((bu - au).hypot(bv - av)).abs())
        }
        DistanceTarget::PointLineDistance { point, line } => {
            measured_point_line_distance(doc, point, line)
        }
    }
}

fn measured_line_line_distance(
    doc: &Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
) -> Option<f32> {
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

fn measured_point_line_distance(
    doc: &Document,
    point: ConstraintPoint,
    line: ConstraintLine,
) -> Option<f32> {
    let (pu, pv) = point_uv(doc, point).ok()?;
    let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, line).ok()?;
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

pub fn constraint_label(doc: &Document, index: ConstraintId) -> String {
    let Some(constraint) = doc.constraints.get(index) else {
        return format!("Constraint {index}");
    };
    let value = match constraint.kind {
        ConstraintKind::Distance {
            target: DistanceTarget::CircleDiameter(_),
        } => constraint_evaluated_length(doc, index)
            .map(format_diameter_display)
            .unwrap_or_else(|| "?".to_string()),
        ConstraintKind::Distance { .. } => constraint_evaluated_length(doc, index)
            .map(format_length_display)
            .unwrap_or_else(|| "?".to_string()),
        ConstraintKind::Parallel { .. }
        | ConstraintKind::Perpendicular { .. }
        | ConstraintKind::Coincident { .. }
        | ConstraintKind::Midpoint { .. }
        | ConstraintKind::Horizontal { .. }
        | ConstraintKind::Vertical { .. } => String::new(),
        ConstraintKind::Angle { .. } => constraint_evaluated_angle(doc, index)
            .map(format_angle_display)
            .unwrap_or_else(|| "?".to_string()),
    };
    let target_label = match constraint.kind {
        ConstraintKind::Distance { target } => distance_target_label(target),
        ConstraintKind::Parallel { .. } => "Parallel".to_string(),
        ConstraintKind::Perpendicular { .. } => "Perpendicular".to_string(),
        ConstraintKind::Coincident { .. } => "Coincident".to_string(),
        ConstraintKind::Midpoint { .. } => "Midpoint".to_string(),
        ConstraintKind::Horizontal { .. } => "Horizontal".to_string(),
        ConstraintKind::Vertical { .. } => "Vertical".to_string(),
        ConstraintKind::Angle { .. } => "Angle".to_string(),
    };
    match constraint.kind {
        ConstraintKind::Distance { .. } | ConstraintKind::Angle { .. } => {
            format!("Constraint {index} ({target_label}, {value})")
        }
        _ => format!("Constraint {index} ({target_label})"),
    }
}

fn distance_target_label(target: DistanceTarget) -> String {
    match target {
        DistanceTarget::LineLength(i) => format!("Line {i} length"),
        DistanceTarget::RectWidth(i) => format!("Rectangle {i} width"),
        DistanceTarget::RectHeight(i) => format!("Rectangle {i} height"),
        DistanceTarget::CircleDiameter(i) => format!("Circle {i} diameter"),
        DistanceTarget::LineLineDistance { .. } => "Line spacing".to_string(),
        DistanceTarget::PointPointDistance { .. } => "Point distance".to_string(),
        DistanceTarget::PointLineDistance { .. } => "Point-line distance".to_string(),
    }
}

fn line_sort_key(line: ConstraintLine) -> (u8, usize, u8) {
    match line {
        ConstraintLine::Line(i) => (0, i, 0),
        ConstraintLine::RectEdge { rect, edge } => (1, rect, edge.index() as u8),
    }
}

pub fn normalize_line_pair(a: ConstraintLine, b: ConstraintLine) -> (ConstraintLine, ConstraintLine) {
    if line_sort_key(a) <= line_sort_key(b) {
        (a, b)
    } else {
        (b, a)
    }
}

fn point_sort_key(point: ConstraintPoint) -> (u8, usize, u8, u8) {
    match point {
        ConstraintPoint::LineEndpoint { line, end } => (0, line, end as u8, 0),
        ConstraintPoint::RectCorner { rect, corner } => (1, rect, corner, 0),
        ConstraintPoint::CircleCenter(circle) => (2, circle, 0, 0),
    }
}

fn normalize_point_pair(a: ConstraintPoint, b: ConstraintPoint) -> (ConstraintPoint, ConstraintPoint) {
    if point_sort_key(a) <= point_sort_key(b) {
        (a, b)
    } else {
        (b, a)
    }
}

pub fn normalize_distance_target(target: DistanceTarget) -> DistanceTarget {
    match target {
        DistanceTarget::LineLineDistance { line_a, line_b } => {
            let (line_a, line_b) = normalize_line_pair(line_a, line_b);
            DistanceTarget::LineLineDistance { line_a, line_b }
        }
        DistanceTarget::PointPointDistance { a, b } => {
            let (a, b) = normalize_point_pair(a, b);
            DistanceTarget::PointPointDistance { a, b }
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
            ConstraintRef::Line(line) => Some(*line),
            _ => None,
        })
        .collect();
    let points: Vec<ConstraintPoint> = refs
        .iter()
        .filter_map(|reference| match reference {
            ConstraintRef::Point(point) => Some(*point),
            _ => None,
        })
        .collect();

    if lines.len() == 2 {
        let line_a = lines[0];
        let line_b = lines[1];
        validate_line_in_sketch(doc, sketch, line_a).ok()?;
        validate_line_in_sketch(doc, sketch, line_b).ok()?;
        let (line_a, line_b) = normalize_line_pair(line_a, line_b);
        if lines_are_parallel(doc, line_a, line_b) {
            Some(DimensionTarget::Distance(DistanceTarget::LineLineDistance {
                line_a,
                line_b,
            }))
        } else {
            Some(DimensionTarget::Angle { line_a, line_b })
        }
    } else if points.len() == 2 {
        validate_point_in_sketch(doc, sketch, points[0]).ok()?;
        validate_point_in_sketch(doc, sketch, points[1]).ok()?;
        let (a, b) = normalize_point_pair(points[0], points[1]);
        Some(DimensionTarget::Distance(DistanceTarget::PointPointDistance {
            a,
            b,
        }))
    } else if points.len() == 1 && lines.len() == 1 {
        validate_point_in_sketch(doc, sketch, points[0]).ok()?;
        validate_line_in_sketch(doc, sketch, lines[0]).ok()?;
        Some(DimensionTarget::Distance(DistanceTarget::PointLineDistance {
            point: points[0],
            line: lines[0],
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
        ConstraintLine::RectEdge { rect, .. } => {
            let entity = doc
                .rects
                .get(rect)
                .ok_or_else(|| format!("Rectangle {rect} not found"))?;
            if entity.sketch != sketch {
                return Err(format!("Rectangle {rect} is not in sketch {sketch}"));
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
        ConstraintPoint::RectCorner { rect, .. } => validate_line_in_sketch(
            doc,
            sketch,
            ConstraintLine::RectEdge {
                rect,
                edge: RectEdge::Bottom,
            },
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
    use crate::model::RectEdge;
    match element {
        SceneElement::Line(index) => {
            let line = doc.lines.get(index)?;
            (line.sketch == sketch).then_some(DistanceTarget::LineLength(index))
        }
        SceneElement::RectEdge(rect_index, edge) => {
            let rect = doc.rects.get(rect_index)?;
            if rect.sketch != sketch {
                return None;
            }
            match edge {
                RectEdge::Bottom | RectEdge::Top => Some(DistanceTarget::RectWidth(rect_index)),
                RectEdge::Left | RectEdge::Right => Some(DistanceTarget::RectHeight(rect_index)),
            }
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
        crate::construction::PickTargetKind::ShapeEdge { rect_index, edge, .. } => {
            let rect = doc.rects.get(*rect_index)?;
            if rect.sketch != sketch {
                return None;
            }
            match edge {
                RectEdge::Bottom | RectEdge::Top => Some(DistanceTarget::RectWidth(*rect_index)),
                RectEdge::Left | RectEdge::Right => Some(DistanceTarget::RectHeight(*rect_index)),
            }
        }
        crate::construction::PickTargetKind::Circle(index) => {
            let circle = doc.circles.get(*index)?;
            (circle.sketch == sketch).then_some(DistanceTarget::CircleDiameter(*index))
        }
        _ => None,
    }
}

/// Default expression text when starting a new dimension on a segment.
pub fn default_distance_expression(doc: &Document, target: DistanceTarget) -> String {
    measured_distance(doc, target)
        .map(format_length_display)
        .unwrap_or_else(|| "10mm".to_string())
}

pub fn default_angle_expression(doc: &Document, line_a: ConstraintLine, line_b: ConstraintLine) -> String {
    measured_angle_between_lines(doc, line_a, line_b)
        .map(format_angle_display)
        .unwrap_or_else(|| "45 deg".to_string())
}

pub fn default_dimension_expression(doc: &Document, target: DimensionTarget) -> String {
    match target {
        DimensionTarget::Distance(distance) => default_distance_expression(doc, distance),
        DimensionTarget::Angle { line_a, line_b } => {
            default_angle_expression(doc, line_a, line_b)
        }
    }
}

/// Add an angle constraint; returns the new constraint index.
pub fn add_angle_constraint(
    doc: &mut Document,
    sketch: SketchId,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
    expression: String,
) -> Result<ConstraintId, String> {
    let expression = expression.trim().to_string();
    if expression.is_empty() {
        return Err("Constraint expression cannot be empty".to_string());
    }
    let (line_a, line_b) = normalize_line_pair(line_a, line_b);
    validate_line_in_sketch(doc, sketch, line_a)?;
    validate_line_in_sketch(doc, sketch, line_b)?;
    if line_a == line_b {
        return Err("Angle constraint requires two different lines".to_string());
    }
    if lines_are_parallel(doc, line_a, line_b) {
        return Err("Angle constraint requires non-parallel lines".to_string());
    }
    if let Some(index) = find_angle_constraint(doc, line_a, line_b) {
        return Err(format!("Angle constraint already exists (index {index})"));
    }
    let kind = ConstraintKind::Angle { line_a, line_b };
    validate_constraint_expression(doc, kind, &expression)?;
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
            if let Some(id) = find_distance_constraint(doc, distance) {
                set_constraint_expression(doc, id, expression.to_string())
            } else {
                add_distance_constraint(doc, sketch, distance, expression.to_string())?;
                Ok(())
            }
        }
        DimensionTarget::Angle { line_a, line_b } => {
            if let Some(id) = find_angle_constraint(doc, line_a, line_b) {
                set_constraint_expression(doc, id, expression.to_string())
            } else {
                add_angle_constraint(doc, sketch, line_a, line_b, expression.to_string())?;
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
        DistanceTarget::RectWidth(i) | DistanceTarget::RectHeight(i) => {
            let rect = doc
                .rects
                .get(i)
                .ok_or_else(|| format!("Rectangle {i} not found"))?;
            if rect.sketch != sketch {
                return Err(format!("Rectangle {i} is not in sketch {sketch}"));
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
        DistanceTarget::LineLineDistance { line_a, line_b } => {
            validate_line_in_sketch(doc, sketch, line_a)?;
            validate_line_in_sketch(doc, sketch, line_b)?;
            if line_a == line_b {
                return Err("Line spacing requires two different lines".to_string());
            }
            if !lines_are_parallel(doc, line_a, line_b) {
                return Err("Line spacing requires parallel lines".to_string());
            }
        }
        DistanceTarget::PointPointDistance { a, b } => {
            validate_point_in_sketch(doc, sketch, a)?;
            validate_point_in_sketch(doc, sketch, b)?;
            if a == b {
                return Err("Point distance requires two different points".to_string());
            }
        }
        DistanceTarget::PointLineDistance { point, line } => {
            validate_point_in_sketch(doc, sketch, point)?;
            validate_line_in_sketch(doc, sketch, line)?;
        }
    }
    Ok(())
}

/// Apply all distance constraints to sketch geometry.
pub fn solve_document_constraints(doc: &mut Document) -> Result<(), String> {
    clear_legacy_dimension_locks(doc);
    for i in 0..doc.constraints.len() {
        let constraint = doc.constraints[i].clone();
        if constraint.deleted {
            continue;
        }
        if let ConstraintKind::Distance { target } = constraint.kind {
            if !crate::document_lifecycle::distance_target_alive(doc, target) {
                continue;
            }
            let Some(value) = eval_length_mm_in_doc(&constraint.expression, doc) else {
                continue;
            };
            if value <= 0.0 {
                continue;
            }
            if apply_distance_constraint(doc, target, value).is_err() {
                continue;
            }
            sync_legacy_dimension_flags(doc, target, &constraint.expression, constraint.dim_offset);
        } else if let ConstraintKind::Angle {
            line_a,
            line_b,
        } = constraint.kind
        {
            if !crate::document_lifecycle::constraint_kind_applicable(doc, constraint.kind) {
                continue;
            }
            let Some(value) = eval_angle_rad_in_doc(&constraint.expression, doc) else {
                continue;
            };
            if value <= 0.0 || value >= std::f32::consts::PI {
                continue;
            }
            let _ = apply_angle_constraint(doc, line_a, line_b, value);
        }
    }
    crate::geometric_constraints::apply_geometric_constraints(doc)
}

fn clear_legacy_dimension_locks(doc: &mut Document) {
    for rect in &mut doc.rects {
        rect.width_locked = false;
        rect.height_locked = false;
        rect.width_expr = None;
        rect.height_expr = None;
    }
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
        DistanceTarget::RectWidth(i) => {
            if let Some(rect) = doc.rects.get_mut(i) {
                rect.width_locked = true;
                rect.width_expr = Some(expression.to_string());
                if dim_offset.is_some() {
                    rect.width_dim_offset = dim_offset;
                }
            }
        }
        DistanceTarget::RectHeight(i) => {
            if let Some(rect) = doc.rects.get_mut(i) {
                rect.height_locked = true;
                rect.height_expr = Some(expression.to_string());
                if dim_offset.is_some() {
                    rect.height_dim_offset = dim_offset;
                }
            }
        }
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

fn apply_distance_constraint(doc: &mut Document, target: DistanceTarget, value: f32) -> Result<(), String> {
    match target {
        DistanceTarget::RectWidth(i) => {
            let rect = doc
                .rects
                .get_mut(i)
                .ok_or_else(|| format!("Rectangle {i} not found"))?;
            rect.w = value;
        }
        DistanceTarget::RectHeight(i) => {
            let rect = doc
                .rects
                .get_mut(i)
                .ok_or_else(|| format!("Rectangle {i} not found"))?;
            rect.h = value;
        }
        DistanceTarget::LineLength(i) => {
            apply_line_length(doc, i, value)?;
        }
        DistanceTarget::CircleDiameter(i) => {
            let circle = doc
                .circles
                .get_mut(i)
                .ok_or_else(|| format!("Circle {i} not found"))?;
            circle.r = value / 2.0;
        }
        DistanceTarget::LineLineDistance { line_a, line_b } => {
            apply_line_line_distance(doc, line_a, line_b, value)?;
        }
        DistanceTarget::PointPointDistance { a, b } => {
            apply_point_point_distance(doc, a, b, value)?;
        }
        DistanceTarget::PointLineDistance { point, line } => {
            apply_point_line_distance(doc, point, line, value)?;
        }
    }
    Ok(())
}

fn apply_line_line_distance(
    doc: &mut Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
    value: f32,
) -> Result<(), String> {
    let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
    let ((ax0, ay0), (ax1, ay1)) = line_uv_endpoints(doc, reference)?;
    let ((bx0, by0), (bx1, by1)) = line_uv_endpoints(doc, movable)?;
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
    let sign = if signed >= 0.0 { 1.0 } else { -1.0 };
    let delta = value - signed.abs();
    translate_line(doc, movable, perp_u * delta * sign, perp_v * delta * sign)
}

fn apply_point_point_distance(
    doc: &mut Document,
    a: ConstraintPoint,
    b: ConstraintPoint,
    value: f32,
) -> Result<(), String> {
    use crate::geometric_constraints::coincident_mover_and_anchor;
    let (mover, anchor) = coincident_mover_and_anchor(a, b);
    let (au, av) = point_uv(doc, anchor)?;
    let (mu, mv) = point_uv(doc, mover)?;
    let du = mu - au;
    let dv = mv - av;
    let dist = (du * du + dv * dv).sqrt();
    let (nu, nv) = if dist < 1e-6 {
        (au + value, av)
    } else {
        let scale = value / dist;
        (au + du * scale, av + dv * scale)
    };
    set_point_uv(doc, mover, nu, nv)
}

fn apply_point_line_distance(
    doc: &mut Document,
    point: ConstraintPoint,
    line: ConstraintLine,
    value: f32,
) -> Result<(), String> {
    let (pu, pv) = point_uv(doc, point)?;
    let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, line)?;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return Err("Line has zero length".to_string());
    }
    let perp_u = -dy / len;
    let perp_v = dx / len;
    let signed = (pu - x0) * perp_u + (pv - y0) * perp_v;
    let sign = if signed >= 0.0 { 1.0 } else { -1.0 };
    let delta = value - signed.abs();
    set_point_uv(doc, point, pu + perp_u * delta * sign, pv + perp_v * delta * sign)
}

fn apply_angle_constraint(
    doc: &mut Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
    value: f32,
) -> Result<(), String> {
    let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
    let (rdu, rdv) = line_direction_uv(doc, reference).ok_or_else(|| {
        "Reference line has zero length".to_string()
    })?;
    let ((mx0, my0), (mx1, my1)) = line_uv_endpoints(doc, movable)?;
    let mdu = mx1 - mx0;
    let mdv = my1 - my0;
    let mlen = (mdu * mdu + mdv * mdv).sqrt();
    if mlen < 1e-6 {
        return Err("Constrained line has zero length".to_string());
    }
    let r_angle = rdv.atan2(rdu);
    let cross = rdu * mdv - rdv * mdu;
    let sign = if cross >= 0.0 { 1.0 } else { -1.0 };
    let new_angle = r_angle + sign * value;
    set_line_uv_endpoints(
        doc,
        movable,
        (mx0, my0),
        (mx0 + new_angle.cos() * mlen, my0 + new_angle.sin() * mlen),
    )
}

fn apply_line_length(doc: &mut Document, index: usize, len: f32) -> Result<(), String> {
    let line = doc
        .lines
        .get(index)
        .ok_or_else(|| format!("Line {index} not found"))?;
    let du = line.x1 - line.x0;
    let dv = line.y1 - line.y0;
    let dist = (du * du + dv * dv).sqrt();
    let (x1, y1) = if dist < 1e-6 {
        (line.x0 + len, line.y0)
    } else {
        let scale = len / dist;
        (line.x0 + du * scale, line.y0 + dv * scale)
    };
    doc.lines[index].x1 = x1;
    doc.lines[index].y1 = y1;
    Ok(())
}

/// Create constraints from legacy `*_locked` fields (pre-constraint documents).
pub fn migrate_legacy_dimensions(doc: &mut Document) {
    let mut pending = Vec::new();
    for (i, rect) in doc.rects.iter().enumerate() {
        if rect.width_locked {
            let expr = rect
                .width_expr
                .clone()
                .unwrap_or_else(|| format_length_display(rect.w));
            if find_distance_constraint(doc, DistanceTarget::RectWidth(i)).is_none() {
                pending.push((
                    rect.sketch,
                    DistanceTarget::RectWidth(i),
                    expr,
                    rect.width_dim_offset,
                ));
            }
        }
        if rect.height_locked {
            let expr = rect
                .height_expr
                .clone()
                .unwrap_or_else(|| format_length_display(rect.h));
            if find_distance_constraint(doc, DistanceTarget::RectHeight(i)).is_none() {
                pending.push((
                    rect.sketch,
                    DistanceTarget::RectHeight(i),
                    expr,
                    rect.height_dim_offset,
                ));
            }
        }
    }
    for (i, line) in doc.lines.iter().enumerate() {
        if line.length_locked {
            let expr = line
                .length_expr
                .clone()
                .unwrap_or_else(|| format_length_display(line.length()));
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
            let expr = circle
                .diameter_expr
                .clone()
                .unwrap_or_else(|| format_length_display(circle.diameter()));
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
    match constraint.kind {
        ConstraintKind::Distance { target } => distance_target_segment_endpoints(doc, target),
        ConstraintKind::Angle { .. } => None,
        _ => None,
    }
}

/// World-space endpoints for displaying a distance dimension.
pub fn distance_target_segment_endpoints(
    doc: &Document,
    target: DistanceTarget,
) -> Option<(glam::Vec3, glam::Vec3)> {
    distance_target_segment_endpoints_inner(doc, target)
}

fn local_to_world_for_target(doc: &Document, u: f32, v: f32, sketch: SketchId) -> Option<glam::Vec3> {
    let frame = crate::face::sketch_geometry_frame(doc, sketch)?;
    Some(crate::face::local_to_world(&frame, u, v))
}

fn distance_target_segment_endpoints_inner(
    doc: &Document,
    target: DistanceTarget,
) -> Option<(glam::Vec3, glam::Vec3)> {
    match target {
        DistanceTarget::LineLength(i) => {
            let line = doc.lines.get(i)?;
            crate::face::line_world_endpoints(doc, line)
        }
        DistanceTarget::RectWidth(i) | DistanceTarget::RectHeight(i) => {
            let rect = doc.rects.get(i)?;
            let edge = match target {
                DistanceTarget::RectWidth(_) => RectEdge::Bottom,
                DistanceTarget::RectHeight(_) => RectEdge::Left,
                _ => unreachable!(),
            };
            let segments = crate::construction::rect_edge_segments(doc, rect);
            let (a, b) = segments[edge.index()];
            Some((a, b))
        }
        DistanceTarget::CircleDiameter(i) => {
            let circle = doc.circles.get(i)?;
            crate::face::circle_world_diameter_endpoints(doc, circle)
        }
        DistanceTarget::LineLineDistance { line_a, line_b } => {
            let sketch = line_sketch(doc, line_a)?;
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
            let signed = (bmu - amu) * perp_u + (bmv - amv) * perp_v;
            let sign = if signed >= 0.0 { 1.0 } else { -1.0 };
            let a = local_to_world_for_target(doc, amu, amv, sketch)?;
            let b = local_to_world_for_target(
                doc,
                amu + perp_u * signed.abs() * sign,
                amv + perp_v * signed.abs() * sign,
                sketch,
            )?;
            Some((a, b))
        }
        DistanceTarget::PointPointDistance { a, b } => {
            let sketch = point_sketch(doc, a)?;
            let (au, av) = point_uv(doc, a).ok()?;
            let (bu, bv) = point_uv(doc, b).ok()?;
            Some((
                local_to_world_for_target(doc, au, av, sketch)?,
                local_to_world_for_target(doc, bu, bv, sketch)?,
            ))
        }
        DistanceTarget::PointLineDistance { point, line } => {
            let sketch = point_sketch(doc, point)?;
            let (pu, pv) = point_uv(doc, point).ok()?;
            let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, line).ok()?;
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
        ConstraintLine::RectEdge { rect, .. } => doc.rects.get(rect).map(|r| r.sketch),
    }
}

/// World-space center and unit directions for an angle dimension arc.
pub fn angle_constraint_display_dirs(
    doc: &Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
) -> Option<(glam::Vec3, glam::Vec3, glam::Vec3)> {
    let sketch = line_sketch(doc, line_a)?;
    let ((ax0, ay0), (ax1, ay1)) = line_uv_endpoints(doc, line_a).ok()?;
    let ((bx0, by0), (bx1, by1)) = line_uv_endpoints(doc, line_b).ok()?;
    let (cu, cv) = line_intersection_uv((ax0, ay0), (ax1, ay1), (bx0, by0), (bx1, by1))?;
    let frame = crate::face::sketch_geometry_frame(doc, sketch)?;
    let center = crate::face::local_to_world(&frame, cu, cv);
    let dir_a = crate::face::local_to_world(&frame, ax1, ay1) - center;
    let dir_b = crate::face::local_to_world(&frame, bx1, by1) - center;
    if dir_a.length_squared() < 1e-8 || dir_b.length_squared() < 1e-8 {
        return None;
    }
    Some((center, dir_a.normalize(), dir_b.normalize()))
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

fn point_sketch(doc: &Document, point: ConstraintPoint) -> Option<SketchId> {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => doc.lines.get(line).map(|l| l.sketch),
        ConstraintPoint::RectCorner { rect, .. } => doc.rects.get(rect).map(|r| r.sketch),
        ConstraintPoint::CircleCenter(circle) => doc.circles.get(circle).map(|c| c.sketch),
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
    use crate::model::{Circle, Document, FaceId, Line, Rect, ShapeKind};

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

    #[test]
    fn add_distance_constraint_for_rectangle_width() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        doc.shape_order.push(ShapeKind::Rect);
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "20mm".to_string(),
        )
        .unwrap();
        assert!((doc.rects[0].w - 20.0).abs() < 1e-3);
        assert!(doc.rects[0].width_locked);
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
    fn migrate_legacy_dimensions_creates_constraints() {
        let (mut doc, sketch) = sketch_doc();
        let mut rect = Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0);
        rect.width_locked = true;
        rect.width_expr = Some("10mm".to_string());
        doc.rects.push(rect);
        migrate_legacy_dimensions(&mut doc);
        assert_eq!(doc.constraints.len(), 1);
        assert_eq!(
            find_distance_constraint(&doc, DistanceTarget::RectWidth(0)),
            Some(0)
        );
    }

    #[test]
    fn distance_target_from_selection_maps_line_and_rect_edge() {
        use crate::hierarchy::SceneElement;
        use crate::model::RectEdge;
        use crate::selection::{click_scene_selection, SceneSelection};

        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 5.0, 0.0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));

        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        assert_eq!(
            distance_target_from_selection(&doc, sketch, &sel),
            Some(DistanceTarget::LineLength(0))
        );

        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Bottom), false);
        assert_eq!(
            distance_target_from_selection(&doc, sketch, &sel),
            Some(DistanceTarget::RectWidth(0))
        );

        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Left), false);
        assert_eq!(
            distance_target_from_selection(&doc, sketch, &sel),
            Some(DistanceTarget::RectHeight(0))
        );

        click_scene_selection(&mut sel, SceneElement::Rect(0), true);
        assert_eq!(distance_target_from_selection(&doc, sketch, &sel), None);
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
        add_angle_constraint(
            &mut doc,
            sketch,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
            "45".to_string(),
        )
        .unwrap();
        let angle = measured_angle_between_lines(
            &doc,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
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
            },
            "5mm".to_string(),
        )
        .unwrap();
        let dist = measured_line_line_distance(
            &doc,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
        )
        .unwrap();
        assert!((dist - 5.0).abs() < 0.2, "dist={dist}");
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