//! Sketch constraints and a lightweight distance constraint solver.
//!
//! Distance constraints are the first constraint kind: they fix the length of a
//! line segment or a rectangle width/height. Each constraint is stored as a
//! first-class document element and evaluated when parameters or expressions change.

use crate::model::{Constraint, ConstraintKind, DistanceTarget, Document, RectEdge, SketchId};
use crate::value::{eval_length_mm_in_doc, format_length_display};

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
    });
    doc.shape_order.push(crate::model::ShapeKind::Constraint);
    solve_document_constraints(doc)?;
    Ok(id)
}

/// Update an existing distance constraint expression.
pub fn set_constraint_expression(
    doc: &mut Document,
    index: ConstraintId,
    expression: String,
) -> Result<(), String> {
    let expression = expression.trim().to_string();
    if expression.is_empty() {
        return Err("Constraint expression cannot be empty".to_string());
    }
    if doc.constraints.get(index).is_none() {
        return Err(format!("Constraint {index} not found"));
    }
    eval_length_mm_in_doc(&expression, doc)
        .filter(|v| *v > 0.0)
        .ok_or_else(|| format!("Invalid constraint expression '{expression}'"))?;
    doc.constraints[index].expression = expression;
    solve_document_constraints(doc)
}

pub fn set_constraint_dim_offset(doc: &mut Document, index: ConstraintId, offset: f32) -> Result<(), String> {
    if doc.constraints.get(index).is_none() {
        return Err(format!("Constraint {index} not found"));
    }
    doc.constraints[index].dim_offset = Some(offset);
    solve_document_constraints(doc)
}

pub fn find_distance_constraint(doc: &Document, target: DistanceTarget) -> Option<ConstraintId> {
    doc.constraints.iter().position(|c| {
        matches!(&c.kind, ConstraintKind::Distance { target: t } if *t == target)
    })
}

pub fn constraint_expression(doc: &Document, index: ConstraintId) -> Option<String> {
    doc.constraints.get(index).map(|c| c.expression.clone())
}

pub fn constraint_evaluated_length(doc: &Document, index: ConstraintId) -> Option<f32> {
    let constraint = doc.constraints.get(index)?;
    let ConstraintKind::Distance { target } = constraint.kind;
    eval_length_mm_in_doc(&constraint.expression, doc).or_else(|| match target {
        DistanceTarget::LineLength(i) => doc.lines.get(i).map(|l| l.length()),
        DistanceTarget::RectWidth(i) => doc.rects.get(i).map(|r| r.w),
        DistanceTarget::RectHeight(i) => doc.rects.get(i).map(|r| r.h),
    })
}

pub fn constraint_label(doc: &Document, index: ConstraintId) -> String {
    let Some(constraint) = doc.constraints.get(index) else {
        return format!("Constraint {index}");
    };
    let value = constraint_evaluated_length(doc, index)
        .map(format_length_display)
        .unwrap_or_else(|| "?".to_string());
    let target_label = match constraint.kind {
        ConstraintKind::Distance { target } => distance_target_label(target),
    };
    format!("Constraint {index} ({target_label}, {value})")
}

fn distance_target_label(target: DistanceTarget) -> String {
    match target {
        DistanceTarget::LineLength(i) => format!("Line {i} length"),
        DistanceTarget::RectWidth(i) => format!("Rectangle {i} width"),
        DistanceTarget::RectHeight(i) => format!("Rectangle {i} height"),
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
        _ => None,
    }
}

/// Default expression text when starting a new dimension on a segment.
pub fn default_distance_expression(doc: &Document, target: DistanceTarget) -> String {
    let length = match target {
        DistanceTarget::LineLength(i) => doc.lines.get(i).map(|l| l.length()),
        DistanceTarget::RectWidth(i) => doc.rects.get(i).map(|r| r.w),
        DistanceTarget::RectHeight(i) => doc.rects.get(i).map(|r| r.h),
    };
    length
        .map(format_length_display)
        .unwrap_or_else(|| "10mm".to_string())
}

fn validate_distance_target(doc: &Document, sketch: SketchId, target: DistanceTarget) -> Result<(), String> {
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
    }
    Ok(())
}

/// Apply all distance constraints to sketch geometry.
pub fn solve_document_constraints(doc: &mut Document) -> Result<(), String> {
    clear_legacy_dimension_locks(doc);
    for i in 0..doc.constraints.len() {
        let constraint = doc.constraints[i].clone();
        let ConstraintKind::Distance { target } = constraint.kind;
        let value = eval_length_mm_in_doc(&constraint.expression, doc)
            .ok_or_else(|| format!("Invalid constraint expression '{}'", constraint.expression))?;
        if value <= 0.0 {
            return Err(format!(
                "Constraint expression '{}' must be positive",
                constraint.expression
            ));
        }
        apply_distance_constraint(doc, target, value)?;
        sync_legacy_dimension_flags(doc, target, &constraint.expression, constraint.dim_offset);
    }
    Ok(())
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
    }
    Ok(())
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
    });
    doc.shape_order.push(crate::model::ShapeKind::Constraint);
    Ok(id)
}

/// World-space segment endpoints for a distance target.
pub fn distance_target_segment_endpoints(
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
                DistanceTarget::LineLength(_) => unreachable!(),
            };
            let segments = crate::construction::rect_edge_segments(doc, rect);
            let (a, b) = segments[edge.index()];
            Some((a, b))
        }
    }
}

/// World-space segment endpoints for a distance constraint, if geometry exists.
pub fn constraint_segment_endpoints(
    doc: &Document,
    index: ConstraintId,
) -> Option<(glam::Vec3, glam::Vec3)> {
    let constraint = doc.constraints.get(index)?;
    let ConstraintKind::Distance { target } = constraint.kind;
    distance_target_segment_endpoints(doc, target)
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
    use crate::model::{Document, FaceId, Line, Rect, ShapeKind};

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