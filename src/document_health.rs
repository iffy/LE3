//! Invalid/unstable health propagation and geometry snapshots after deletions or broken expressions.
//!
//! Health is assigned only for **direct** broken dependencies:
//! - constraints that reference deleted geometry or unevaluable expressions
//! - geometry that is still alive but was an endpoint of such a constraint (unstable)
//! - parameters with bad expressions or direct references to invalid parameters
//!
//! Siblings, child sketches, and other hierarchy descendants stay healthy unless they
//! meet one of the rules above. Tombstoned (`deleted`) entities are hidden separately.

use crate::constraints::find_dimension_constraint;
use crate::document_lifecycle::{
    constraint_entity_alive, constraint_line_alive, constraint_point_alive, distance_target_alive,
    element_alive,
};
use crate::hierarchy::SceneElement;
use crate::selection::SceneSelection;
use crate::model::{
    ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, DimensionTarget,
    DistanceTarget, Document, ParameterSource,
};
use crate::value::{
    eval_angle_rad_in_doc, eval_length_mm_in_doc, eval_parameter_in_doc,
    parameter_names_referenced_in_expression, EvaluatedParameter,
};
use eframe::egui::Color32;
use std::collections::HashMap;

pub const INVALID_DISPLAY: Color32 = Color32::from_rgb(220, 80, 80);
pub const UNSTABLE_DISPLAY: Color32 = Color32::from_rgb(255, 180, 60);

/// Blend a base stroke/fill color toward invalid/unstable display colors.
pub fn health_tint_color(base: Color32, status: HealthStatus) -> Color32 {
    match status {
        HealthStatus::Healthy => base,
        HealthStatus::Invalid => blend_color32(base, INVALID_DISPLAY, 0.55),
        HealthStatus::Unstable => blend_color32(base, UNSTABLE_DISPLAY, 0.45),
    }
}

fn blend_color32(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    Color32::from_rgba_unmultiplied(
        (f32::from(a.r()) * inv + f32::from(b.r()) * t) as u8,
        (f32::from(a.g()) * inv + f32::from(b.g()) * t) as u8,
        (f32::from(a.b()) * inv + f32::from(b.b()) * t) as u8,
        a.a(),
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HealthStatus {
    #[default]
    Healthy,
    Unstable,
    Invalid,
}

impl HealthStatus {
    pub fn is_frozen(self) -> bool {
        matches!(self, Self::Unstable | Self::Invalid)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ElementSnapshot {
    Line {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
    },
    Parameter {
        expression: String,
        evaluated_mm: Option<f32>,
        evaluated_angle_rad: Option<f32>,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocumentHealth {
    pub elements: HashMap<SceneElement, HealthStatus>,
    pub parameters: HashMap<usize, HealthStatus>,
    pub element_snapshots: HashMap<SceneElement, ElementSnapshot>,
    pub parameter_snapshots: HashMap<usize, ElementSnapshot>,
    pub element_reasons: HashMap<SceneElement, String>,
    pub parameter_reasons: HashMap<usize, String>,
}

impl DocumentHealth {
    pub fn element_status(&self, element: SceneElement) -> HealthStatus {
        self.elements.get(&element).copied().unwrap_or(HealthStatus::Healthy)
    }

    pub fn parameter_status(&self, index: usize) -> HealthStatus {
        self.parameters.get(&index).copied().unwrap_or(HealthStatus::Healthy)
    }

    pub fn element_reason(&self, element: SceneElement) -> Option<&str> {
        self.element_reasons.get(&element).map(String::as_str)
    }

    pub fn parameter_reason(&self, index: usize) -> Option<&str> {
        self.parameter_reasons.get(&index).map(String::as_str)
    }

    pub fn selection_frozen(&self, element: SceneElement) -> Option<HealthStatus> {
        let status = self.element_status(element);
        status.is_frozen().then_some(status)
    }
}

pub fn health_status_label(status: HealthStatus) -> &'static str {
    match status {
        HealthStatus::Healthy => "healthy",
        HealthStatus::Unstable => "unstable",
        HealthStatus::Invalid => "invalid",
    }
}

/// Worst frozen status among the current selection, if any.
pub fn selection_frozen_summary(
    health: &DocumentHealth,
    selection: &SceneSelection,
) -> Option<(HealthStatus, String)> {
    let mut worst: Option<(HealthStatus, String)> = None;
    for element in selection.iter() {
        let status = health.element_status(element.clone());
        if !status.is_frozen() {
            continue;
        }
        let reason = health
            .element_reason(element)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Element is {}", health_status_label(status)));
        match worst {
            None => worst = Some((status, reason)),
            Some((HealthStatus::Unstable, _)) if status == HealthStatus::Invalid => {
                worst = Some((status, reason));
            }
            _ => {}
        }
    }
    worst
}

fn scene_element_for_distance_target(target: &DistanceTarget) -> SceneElement {
    match target {
        DistanceTarget::LineLength(index) => SceneElement::Line(*index),
        DistanceTarget::RectWidth(index) | DistanceTarget::RectHeight(index) => {
            SceneElement::Rect(*index)
        }
        DistanceTarget::CircleDiameter(index) => SceneElement::Circle(*index),
        DistanceTarget::LineLineDistance { line_a, .. } => scene_element_for_line(line_a),
        DistanceTarget::PointPointDistance { anchor, .. } => scene_element_for_point(anchor),
        DistanceTarget::PointLineDistance { point, .. } => scene_element_for_point(point),
    }
}

fn scene_element_for_line(line: &ConstraintLine) -> SceneElement {
    match line {
        ConstraintLine::Line(index) => SceneElement::Line(*index),
        ConstraintLine::RectEdge { rect, .. } => SceneElement::Rect(*rect),
        // A face's own edge depends on the extrusion that produced the face — same
        // relationship `hierarchy::face_element` tracks for sketches placed on a body face.
        ConstraintLine::FaceEdge { face, .. } => scene_element_for_face(face),
    }
}

fn scene_element_for_point(point: &ConstraintPoint) -> SceneElement {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => SceneElement::Line(*line),
        ConstraintPoint::RectCorner { rect, .. } => SceneElement::Rect(*rect),
        ConstraintPoint::CircleCenter(circle) => SceneElement::Circle(*circle),
        ConstraintPoint::FaceVertex { face, .. } => scene_element_for_face(face),
    }
}

/// Best-effort owner element for a `FaceVertex`/`FaceEdge`'s face: the extrusion that produced
/// it (`SceneElement::Extrusion`) for the extrusion-backed faces these ever resolve to; falls
/// back to the constraint itself (a no-op "owner") for any other `FaceId`, which should not
/// occur in practice since `face_boundary_loop_world` never resolves for those.
fn scene_element_for_face(face: &crate::model::FaceId) -> SceneElement {
    // `usize::MAX` never indexes a real extrusion, so this resolves as a dead/unhealthy
    // reference rather than a real element — `face_boundary_loop_world` never resolves for
    // non-extrusion `FaceId`s in the first place, so this arm should be unreachable.
    SceneElement::Extrusion(face.extrusion_index().unwrap_or(usize::MAX))
}

fn scene_element_for_dimension_target(target: &DimensionTarget) -> SceneElement {
    match target {
        DimensionTarget::Distance(distance) => scene_element_for_distance_target(distance),
        DimensionTarget::Angle { line_a, .. } => scene_element_for_line(line_a),
    }
}

pub fn require_dimension_target_editable(
    health: &DocumentHealth,
    doc: &Document,
    target: DimensionTarget,
) -> Result<(), String> {
    require_element_editable(health, scene_element_for_dimension_target(&target))?;
    if let Some(index) = find_dimension_constraint(doc, target) {
        require_element_editable(health, SceneElement::Constraint(index))?;
    }
    Ok(())
}

pub fn require_constraint_editable(
    health: &DocumentHealth,
    doc: &Document,
    constraint: usize,
) -> Result<(), String> {
    require_element_editable(health, SceneElement::Constraint(constraint))?;
    let Some(kind) = doc.constraints.get(constraint).map(|c| c.kind.clone()) else {
        return Ok(());
    };
    match &kind {
        ConstraintKind::Distance { target } => {
            require_element_editable(health, scene_element_for_distance_target(target))?;
        }
        ConstraintKind::Angle {
            line_a,
            line_b,
            rotation_sign: _,
        } => {
            require_element_editable(health, scene_element_for_line(line_a))?;
            require_element_editable(health, scene_element_for_line(line_b))?;
        }
        _ => {}
    }
    Ok(())
}

pub fn require_element_editable(
    health: &DocumentHealth,
    element: SceneElement,
) -> Result<(), String> {
    match health.selection_frozen(element.clone()) {
        Some(status) => Err(
            health
                .element_reason(element)
                .map(|r| format!("{}: {r}", health_status_label(status)))
                .unwrap_or_else(|| format!("Element is {}", health_status_label(status))),
        ),
        None => Ok(()),
    }
}

/// Viewport/UI color for a constraint dimension from its health status.
pub fn constraint_annotation_color(
    health: &DocumentHealth,
    constraint: usize,
    base: Color32,
) -> Color32 {
    match health.element_status(SceneElement::Constraint(constraint)) {
        HealthStatus::Healthy => base,
        HealthStatus::Invalid => INVALID_DISPLAY,
        HealthStatus::Unstable => UNSTABLE_DISPLAY,
    }
}

pub fn require_parameter_editable(health: &DocumentHealth, index: usize) -> Result<(), String> {
    match health.parameter_status(index) {
        HealthStatus::Healthy => Ok(()),
        status => Err(
            health
                .parameter_reason(index)
                .map(|r| format!("{}: {r}", health_status_label(status)))
                .unwrap_or_else(|| format!("Parameter is {}", health_status_label(status))),
        ),
    }
}

/// Recompute health for the whole document.
pub fn recompute_document_health(doc: &Document) -> DocumentHealth {
    let mut health = DocumentHealth::default();
    mark_invalid_constraints_and_unstable_geometry(doc, &mut health);
    mark_invalid_parameters(doc, &mut health);
    health
}

fn geometry_elements_for_line(line: &ConstraintLine) -> Vec<SceneElement> {
    match line {
        ConstraintLine::Line(index) => vec![SceneElement::Line(*index)],
        ConstraintLine::RectEdge { rect, edge } => vec![SceneElement::RectEdge(*rect, *edge)],
        // A face's own edge isn't owned by anything markable-unstable in the usual sense (it
        // can't move); surface it via the extrusion instead, same as `scene_element_for_line`.
        ConstraintLine::FaceEdge { face, .. } => vec![scene_element_for_face(face)],
    }
}

fn mark_invalid_constraints_and_unstable_geometry(doc: &Document, health: &mut DocumentHealth) {
    for (index, constraint) in doc.constraints.iter().enumerate() {
        if constraint.deleted {
            continue;
        }
        let element = SceneElement::Constraint(index);
        match &constraint.kind {
            ConstraintKind::Distance { target } => {
                if !distance_target_alive(doc, target) {
                    set_element_invalid(
                        health,
                        doc,
                        element,
                        "Dimension target was deleted".to_string(),
                        None,
                    );
                } else if eval_length_mm_in_doc(&constraint.expression, doc).is_none() {
                    set_element_invalid(
                        health,
                        doc,
                        element,
                        "Constraint expression cannot be evaluated".to_string(),
                        None,
                    );
                }
            }
            ConstraintKind::Parallel { line_a, line_b }
            | ConstraintKind::Perpendicular { line_a, line_b }
            | ConstraintKind::Equal { line_a, line_b } => {
                let a_alive = constraint_line_alive(doc, line_a);
                let b_alive = constraint_line_alive(doc, line_b);
                if !a_alive || !b_alive {
                    set_element_invalid(
                        health,
                        doc,
                        element,
                        "Referenced geometry was deleted".to_string(),
                        None,
                    );
                    if a_alive {
                        for target in geometry_elements_for_line(line_a) {
                            mark_unstable_geometry(
                                health,
                                doc,
                                target,
                                "Lost parallel/perpendicular partner".to_string(),
                            );
                        }
                    }
                    if b_alive {
                        for target in geometry_elements_for_line(line_b) {
                            mark_unstable_geometry(
                                health,
                                doc,
                                target,
                                "Lost parallel/perpendicular partner".to_string(),
                            );
                        }
                    }
                }
            }
            ConstraintKind::Coincident { a, b } => {
                let a_alive = constraint_entity_alive(doc, a);
                let b_alive = constraint_entity_alive(doc, b);
                if !a_alive || !b_alive {
                    set_element_invalid(
                        health,
                        doc,
                        element,
                        "Referenced geometry was deleted".to_string(),
                        None,
                    );
                    if a_alive {
                        for target in geometry_elements_for_entity(a) {
                            mark_unstable_geometry(
                                health,
                                doc,
                                target,
                                "Lost coincident partner".to_string(),
                            );
                        }
                    }
                    if b_alive {
                        for target in geometry_elements_for_entity(b) {
                            mark_unstable_geometry(
                                health,
                                doc,
                                target,
                                "Lost coincident partner".to_string(),
                            );
                        }
                    }
                }
            }
            ConstraintKind::Midpoint { point, line } => {
                let point_alive = constraint_point_alive(doc, point);
                let line_alive = constraint_line_alive(doc, line);
                if !point_alive || !line_alive {
                    set_element_invalid(
                        health,
                        doc,
                        element,
                        "Referenced geometry was deleted".to_string(),
                        None,
                    );
                    if point_alive {
                        mark_unstable_geometry(
                            health,
                            doc,
                            point_owner_element(point),
                            "Lost midpoint line partner".to_string(),
                        );
                    }
                    if line_alive {
                        for target in geometry_elements_for_line(line) {
                            mark_unstable_geometry(
                                health,
                                doc,
                                target,
                                "Lost midpoint point partner".to_string(),
                            );
                        }
                    }
                }
            }
            ConstraintKind::Horizontal { line } | ConstraintKind::Vertical { line } => {
                if !constraint_line_alive(doc, line) {
                    set_element_invalid(
                        health,
                        doc,
                        element,
                        "Referenced geometry was deleted".to_string(),
                        None,
                    );
                }
            }
            ConstraintKind::Angle {
            line_a,
            line_b,
            rotation_sign: _,
        } => {
                let a_alive = constraint_line_alive(doc, line_a);
                let b_alive = constraint_line_alive(doc, line_b);
                if !a_alive || !b_alive {
                    set_element_invalid(
                        health,
                        doc,
                        element,
                        "Referenced geometry was deleted".to_string(),
                        None,
                    );
                } else if eval_angle_rad_in_doc(&constraint.expression, doc).is_none() {
                    set_element_invalid(
                        health,
                        doc,
                        element,
                        "Constraint expression cannot be evaluated".to_string(),
                        None,
                    );
                }
            }
        }
    }
}

fn geometry_elements_for_entity(entity: &ConstraintEntity) -> Vec<SceneElement> {
    match entity {
        ConstraintEntity::Point(point) => vec![point_owner_element(point)],
        ConstraintEntity::Line(line) => geometry_elements_for_line(line),
        ConstraintEntity::Circle(circle) => vec![SceneElement::Circle(*circle)],
        ConstraintEntity::Origin => Vec::new(),
    }
}

fn point_owner_element(point: &ConstraintPoint) -> SceneElement {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => SceneElement::Line(*line),
        ConstraintPoint::RectCorner { rect, .. } => SceneElement::Rect(*rect),
        ConstraintPoint::CircleCenter(circle) => SceneElement::Circle(*circle),
        ConstraintPoint::FaceVertex { face, .. } => scene_element_for_face(face),
    }
}

fn mark_invalid_parameters(doc: &Document, health: &mut DocumentHealth) {
    let known: Vec<&str> = doc
        .parameters
        .iter()
        .filter(|p| !p.deleted)
        .map(|p| p.name.as_str())
        .collect();
    for (index, param) in doc.parameters.iter().enumerate() {
        if param.deleted {
            continue;
        }
        if let Some(ParameterSource::LineLength(line_index)) = param.source {
            if !crate::document_lifecycle::line_alive(doc, line_index) {
                set_parameter_invalid(
                    health,
                    doc,
                    index,
                    "Source line was deleted".to_string(),
                );
                continue;
            }
        }
        if eval_parameter_in_doc(&param.expression, doc).is_none() {
            set_parameter_invalid(
                health,
                doc,
                index,
                "Parameter expression cannot be evaluated".to_string(),
            );
            continue;
        }
        for dep in parameter_names_referenced_in_expression(&param.expression, &known) {
            if health.parameter_status(index_of_parameter_name(doc, &dep)) == HealthStatus::Invalid {
                set_parameter_invalid(
                    health,
                    doc,
                    index,
                    format!("Depends on invalid parameter '{dep}'"),
                );
                break;
            }
        }
    }
}

fn index_of_parameter_name(doc: &Document, name: &str) -> usize {
    doc.parameters
        .iter()
        .position(|p| p.name == name)
        .unwrap_or(usize::MAX)
}

fn set_element_invalid(
    health: &mut DocumentHealth,
    doc: &Document,
    element: SceneElement,
    reason: String,
    snapshot: Option<ElementSnapshot>,
) {
    if health.element_status(element.clone()) == HealthStatus::Invalid {
        return;
    }
    capture_element_snapshot(health, doc, element.clone(), snapshot);
    health.elements.insert(element.clone(), HealthStatus::Invalid);
    health.element_reasons.insert(element, reason);
}

fn mark_unstable_geometry(
    health: &mut DocumentHealth,
    doc: &Document,
    element: SceneElement,
    reason: String,
) {
    if !element_alive(doc, element.clone()) {
        return;
    }
    if health.element_status(element.clone()) == HealthStatus::Invalid {
        return;
    }
    if health.element_status(element.clone()) == HealthStatus::Unstable {
        return;
    }
    capture_element_snapshot(health, doc, element.clone(), None);
    health.elements.insert(element.clone(), HealthStatus::Unstable);
    health.element_reasons.insert(element, reason);
}

fn set_parameter_invalid(
    health: &mut DocumentHealth,
    doc: &Document,
    index: usize,
    reason: String,
) {
    if health.parameter_status(index) == HealthStatus::Invalid {
        return;
    }
    if !health.parameter_snapshots.contains_key(&index) {
        if let Some(param) = doc.parameters.get(index) {
            health.parameter_snapshots.insert(
                index,
                ElementSnapshot::Parameter {
                    expression: param.expression.clone(),
                    evaluated_mm: match eval_parameter_in_doc(&param.expression, doc) {
                        Some(EvaluatedParameter::LengthMm(v)) => Some(v),
                        _ => None,
                    },
                    evaluated_angle_rad: match eval_parameter_in_doc(&param.expression, doc) {
                        Some(EvaluatedParameter::AngleRad(v)) => Some(v),
                        _ => None,
                    },
                },
            );
        }
    }
    health.parameters.insert(index, HealthStatus::Invalid);
    health.parameter_reasons.insert(index, reason);
}

fn capture_element_snapshot(
    health: &mut DocumentHealth,
    doc: &Document,
    element: SceneElement,
    snapshot: Option<ElementSnapshot>,
) {
    if health.element_snapshots.contains_key(&element) {
        return;
    }
    if let Some(snapshot) = snapshot {
        health.element_snapshots.insert(element, snapshot);
    } else if let Some(snapshot) = capture_geometry_snapshot(doc, element.clone()) {
        health.element_snapshots.insert(element, snapshot);
    }
}

fn capture_geometry_snapshot(doc: &Document, element: SceneElement) -> Option<ElementSnapshot> {
    match element {
        SceneElement::Line(index) => doc.lines.get(index).map(|line| ElementSnapshot::Line {
            x0: line.x0,
            y0: line.y0,
            x1: line.x1,
            y1: line.y1,
        }),
        SceneElement::Rect(index) => doc.rects.get(index).map(|rect| ElementSnapshot::Rect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }),
        SceneElement::Circle(index) => doc.circles.get(index).map(|circle| ElementSnapshot::Circle {
            cx: circle.cx,
            cy: circle.cy,
            r: circle.r,
        }),
        SceneElement::RectEdge(index, _) => doc.rects.get(index).map(|rect| ElementSnapshot::Rect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_lifecycle::tombstone_element;
    use crate::model::{Constraint, ConstraintKind, ConstraintLine, Document, Line, ShapeKind};
    use crate::model::FaceId;

    fn parallel_lines_doc() -> (Document, usize, usize) {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        let line_a = 0;
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 5.0, 10.0, 5.0));
        doc.shape_order.push(ShapeKind::Line);
        let line_b = 1;
        doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Parallel {
                line_a: ConstraintLine::Line(line_a),
                line_b: ConstraintLine::Line(line_b),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        doc.shape_order.push(ShapeKind::Constraint);
        (doc, line_a, line_b)
    }

    #[test]
    fn delete_line_marks_constraint_invalid_and_partner_unstable() {
        let (mut doc, line_a, line_b) = parallel_lines_doc();
        let y_before = doc.lines[line_b].y0;
        tombstone_element(&mut doc, SceneElement::Line(line_a));
        let health = recompute_document_health(&doc);
        assert_eq!(
            health.element_status(SceneElement::Constraint(0)),
            HealthStatus::Invalid
        );
        assert_eq!(
            health.element_status(SceneElement::Line(line_b)),
            HealthStatus::Unstable
        );
        assert!((doc.lines[line_b].y0 - y_before).abs() < 1e-4);
        assert!(health.element_snapshots.contains_key(&SceneElement::Line(line_b)));
    }

    #[test]
    fn invalid_parameter_expression_marks_parameter_invalid() {
        let mut doc = Document::default();
        doc.parameters.push(crate::model::Parameter {
            name: "width".to_string(),
            expression: "1mm / 0".to_string(),
            deleted: false,
            source: None,
        });
        let health = recompute_document_health(&doc);
        assert_eq!(health.parameter_status(0), HealthStatus::Invalid);
    }

    #[test]
    fn healthy_document_has_no_frozen_elements() {
        let (doc, _, _) = parallel_lines_doc();
        let health = recompute_document_health(&doc);
        assert_eq!(health.element_status(SceneElement::Line(0)), HealthStatus::Healthy);
        assert!(!health.element_status(SceneElement::Line(0)).is_frozen());
    }

    #[test]
    fn health_tint_color_blends_toward_invalid() {
        let base = Color32::from_rgb(120, 170, 240);
        let tinted = health_tint_color(base, HealthStatus::Invalid);
        assert_ne!(tinted, base);
        assert!(tinted.r() > base.r() || tinted.g() < base.g());
    }

    #[test]
    fn constraint_annotation_color_reflects_health() {
        let (mut doc, line_a, _) = parallel_lines_doc();
        tombstone_element(&mut doc, SceneElement::Line(line_a));
        let health = recompute_document_health(&doc);
        let base = Color32::from_rgb(180, 188, 204);
        assert_eq!(
            constraint_annotation_color(&health, 0, base),
            INVALID_DISPLAY
        );
    }

    #[test]
    fn unrelated_geometry_in_sketch_stays_healthy_when_partner_unstable() {
        let (mut doc, line_a, line_b) = parallel_lines_doc();
        let sketch = 0;
        doc.lines
            .push(Line::from_local_endpoints(sketch, 20.0, 0.0, 30.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        tombstone_element(&mut doc, SceneElement::Line(line_a));
        let health = recompute_document_health(&doc);
        assert_eq!(
            health.element_status(SceneElement::Line(line_b)),
            HealthStatus::Unstable
        );
        assert_eq!(
            health.element_status(SceneElement::Line(2)),
            HealthStatus::Healthy
        );
        assert_eq!(
            health.element_status(SceneElement::Sketch(sketch)),
            HealthStatus::Healthy
        );
    }

    /// #26/#27: a constraint referencing a `FaceVertex`/`FaceEdge` must be flagged unhealthy
    /// (not panic) once the extrusion that produced the face is deleted.
    #[test]
    fn constraint_on_deleted_extrusion_face_vertex_is_invalid_not_a_panic() {
        use crate::model::{ConstraintEntity, ConstraintPoint, ExtrudeFace, Extrusion, LineEnd};

        let mut doc = Document::default();
        let base_sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(crate::model::Rect::from_local_corners(base_sketch, 0.0, 0.0, 20.0, 20.0));
        doc.shape_order.push(ShapeKind::Rect);
        doc.extrusions.push(Extrusion {
            sketch: base_sketch,
            faces: vec![ExtrudeFace::Rect(0)],
            distance: 10.0,
            target: None,
            expression: String::new(),
            name: None,
            deleted: false,
            edge_treatments: Vec::new(),
        });
        doc.shape_order.push(ShapeKind::Extrusion);

        let cap = FaceId::ExtrudeCap {
            extrusion: 0,
            profile: ExtrudeFace::Rect(0),
            top: true,
        };
        let sketch = doc.add_sketch(cap.clone());
        doc.lines
            .push(Line::from_local_endpoints(sketch, 3.0, 4.0, 8.0, 1.0));
        doc.shape_order.push(ShapeKind::Line);
        let point = ConstraintPoint::LineEndpoint { line: 0, end: LineEnd::Start };
        doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Coincident {
                a: ConstraintEntity::Point(point),
                b: ConstraintEntity::Point(ConstraintPoint::FaceVertex { face: cap, index: 2 }),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        doc.shape_order.push(ShapeKind::Constraint);

        // Healthy while the extrusion (and its face) is still alive.
        let health = recompute_document_health(&doc);
        assert_eq!(
            health.element_status(SceneElement::Constraint(0)),
            HealthStatus::Healthy
        );

        // Deleting the extrusion should never panic (the reference must degrade gracefully),
        // and the constraint must now be flagged invalid.
        tombstone_element(&mut doc, SceneElement::Extrusion(0));
        let health = recompute_document_health(&doc);
        assert_eq!(
            health.element_status(SceneElement::Constraint(0)),
            HealthStatus::Invalid
        );
    }
}