//! Tombstone deletion: entities are marked deleted but keep their indices so references stay valid.

use crate::hierarchy::SceneElement;
use crate::model::{
    ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, DistanceTarget, Document,
    FaceId, ShapeKind, SketchId,
};
use crate::selection::SceneSelection;
use std::collections::HashSet;

/// Whether a stored entity at `index` is still active (not tombstoned).
pub fn parameter_alive(doc: &Document, index: usize) -> bool {
    doc.parameters.get(index).is_some_and(|p| !p.deleted)
}

pub fn sketch_alive(doc: &Document, sketch: SketchId) -> bool {
    doc.sketches.get(sketch).is_some_and(|s| !s.deleted)
}

pub fn rect_alive(doc: &Document, index: usize) -> bool {
    doc.rects.get(index).is_some_and(|r| !r.deleted)
}

pub fn line_alive(doc: &Document, index: usize) -> bool {
    doc.lines.get(index).is_some_and(|l| !l.deleted)
}

pub fn circle_alive(doc: &Document, index: usize) -> bool {
    doc.circles.get(index).is_some_and(|c| !c.deleted)
}

pub fn constraint_alive(doc: &Document, index: usize) -> bool {
    doc.constraints.get(index).is_some_and(|c| !c.deleted)
}

pub fn construction_plane_alive(doc: &Document, index: usize) -> bool {
    doc.construction_planes
        .get(index)
        .is_some_and(|p| !p.deleted)
}

/// Whether a scene element is present and not tombstoned.
pub fn element_alive(doc: &Document, element: SceneElement) -> bool {
    match element {
        SceneElement::ConstructionPlane(index) => construction_plane_alive(doc, index),
        SceneElement::Sketch(sketch) => sketch_alive(doc, sketch),
        SceneElement::Rect(index) => rect_alive(doc, index),
        SceneElement::Line(index) => line_alive(doc, index),
        SceneElement::Circle(index) => circle_alive(doc, index),
        SceneElement::RectEdge(index, _) => rect_alive(doc, index),
        SceneElement::Point(point) => point_owner_alive(doc, &point),
        SceneElement::Constraint(index) => constraint_alive(doc, index),
        SceneElement::Extrusion(index) => extrusion_alive(doc, index),
        SceneElement::Body(index) => body_alive(doc, index),
        SceneElement::FaceEdge(line) => constraint_line_alive(doc, &line),
    }
}

pub fn extrusion_alive(doc: &Document, index: usize) -> bool {
    doc.extrusions.get(index).is_some_and(|e| !e.deleted)
}

pub fn body_alive(doc: &Document, index: usize) -> bool {
    doc.bodies.get(index).is_some_and(|b| !b.deleted)
}

fn point_owner_alive(
    doc: &Document,
    point: &crate::model::ConstraintPoint,
) -> bool {
    use crate::model::ConstraintPoint;
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => line_alive(doc, *line),
        ConstraintPoint::RectCorner { rect, .. } => rect_alive(doc, *rect),
        ConstraintPoint::CircleCenter(circle) => circle_alive(doc, *circle),
        // A face's own vertex is "alive" exactly when it still resolves (extrusion present,
        // index still within its current boundary loop) — it has no owning scene entity.
        ConstraintPoint::FaceVertex { face, index } => {
            crate::extrude::face_boundary_loop_world(doc, face).is_some_and(|l| *index < l.len())
        }
    }
}

/// Normalize a selection entry to the entity that should be tombstoned.
pub fn delete_target_for_element(element: SceneElement) -> SceneElement {
    match element {
        SceneElement::RectEdge(index, _) => SceneElement::Rect(index),
        SceneElement::Point(point) => match point_owner_element(&point) {
            Some(owner) => owner,
            // A face's own vertex has no owning scene entity to delete instead — deleting it
            // is a no-op (it's fixed by the body, mirrors `ConstraintEntity::Origin`).
            None => SceneElement::Point(point),
        },
        other => other,
    }
}

fn point_owner_element(point: &crate::model::ConstraintPoint) -> Option<SceneElement> {
    use crate::model::ConstraintPoint;
    Some(match point {
        ConstraintPoint::LineEndpoint { line, .. } => SceneElement::Line(*line),
        ConstraintPoint::RectCorner { rect, .. } => SceneElement::Rect(*rect),
        ConstraintPoint::CircleCenter(circle) => SceneElement::Circle(*circle),
        ConstraintPoint::FaceVertex { .. } => return None,
    })
}

/// Unique tombstone targets from the current selection (deduped).
pub fn delete_targets_from_selection(selection: &SceneSelection) -> Vec<SceneElement> {
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for element in selection.iter() {
        let target = delete_target_for_element(element);
        if seen.insert(target.clone()) {
            targets.push(target);
        }
    }
    targets
}

/// Tombstone one element and any owned children. Returns true if anything changed.
pub fn tombstone_element(doc: &mut Document, element: SceneElement) -> bool {
    let mut changed = false;
    match element {
        SceneElement::ConstructionPlane(index) => {
            if tombstone_construction_plane(doc, index) {
                changed = true;
            }
        }
        SceneElement::Sketch(sketch) => {
            if tombstone_sketch(doc, sketch) {
                changed = true;
            }
        }
        SceneElement::Rect(index) => {
            if tombstone_rect(doc, index) {
                changed = true;
            }
        }
        SceneElement::Circle(index) => {
            if tombstone_circle(doc, index) {
                changed = true;
            }
        }
        SceneElement::Line(index) => {
            if tombstone_line(doc, index) {
                changed = true;
            }
        }
        SceneElement::Constraint(index) => {
            if tombstone_constraint(doc, index) {
                changed = true;
            }
        }
        SceneElement::RectEdge(index, _) => {
            if tombstone_rect(doc, index) {
                changed = true;
            }
        }
        SceneElement::Point(point) => {
            if let Some(owner) = point_owner_element(&point) {
                changed |= tombstone_element(doc, owner);
            }
        }
        SceneElement::Extrusion(index) => {
            if tombstone_extrusion(doc, index) {
                changed = true;
            }
        }
        SceneElement::Body(index) => {
            if tombstone_body(doc, index) {
                changed = true;
            }
        }
        // Fixed by the body's own geometry — deleting it is a no-op, same as `FaceVertex`.
        SceneElement::FaceEdge(_) => {}
    }
    changed
}

fn tombstone_extrusion(doc: &mut Document, index: usize) -> bool {
    let Some(extrusion) = doc.extrusions.get_mut(index) else {
        return false;
    };
    if extrusion.deleted {
        return false;
    }
    extrusion.deleted = true;
    remove_shape_order_entry(doc, ShapeKind::Extrusion, index);
    // A body that depends solely on this extrusion is removed with it; a body merging this
    // extrusion with others (#32) just drops this one and keeps the rest.
    let dependent: Vec<usize> = doc
        .bodies
        .iter()
        .enumerate()
        .filter(|(_, body)| !body.deleted && body.source.owns_extrusion(index))
        .map(|(i, _)| i)
        .collect();
    for bi in dependent {
        let solely_owned = doc.bodies[bi].source.extrusion_indices() == [index];
        if solely_owned {
            tombstone_body(doc, bi);
        } else {
            doc.bodies[bi].source.remove_extrusion(index);
        }
    }
    true
}

fn tombstone_body(doc: &mut Document, index: usize) -> bool {
    let Some(body) = doc.bodies.get_mut(index) else {
        return false;
    };
    if body.deleted {
        return false;
    }
    body.deleted = true;
    remove_shape_order_entry(doc, ShapeKind::Body, index);
    true
}

/// Tombstone every target in `elements`.
pub fn tombstone_elements(doc: &mut Document, elements: &[SceneElement]) -> usize {
    let mut count = 0usize;
    for element in elements {
        if tombstone_element(doc, element.clone()) {
            count += 1;
        }
    }
    count
}

fn tombstone_construction_plane(doc: &mut Document, index: usize) -> bool {
    let Some(plane) = doc.construction_planes.get_mut(index) else {
        return false;
    };
    if plane.deleted {
        return false;
    }
    plane.deleted = true;
    remove_shape_order_entry(doc, ShapeKind::ConstructionPlane, index);
    let face = FaceId::ConstructionPlane(index);
    for sketch in doc.sketches_on_face(face).collect::<Vec<_>>() {
        tombstone_sketch(doc, sketch);
    }
    true
}

fn tombstone_sketch(doc: &mut Document, sketch: SketchId) -> bool {
    let Some(entry) = doc.sketches.get_mut(sketch) else {
        return false;
    };
    if entry.deleted {
        return false;
    }
    entry.deleted = true;
    remove_shape_order_entry(doc, ShapeKind::Sketch, sketch);

    let rects: Vec<usize> = doc
        .rects
        .iter()
        .enumerate()
        .filter(|(_, rect)| rect.sketch == sketch && !rect.deleted)
        .map(|(i, _)| i)
        .collect();
    for ri in rects {
        tombstone_rect(doc, ri);
    }
    let lines: Vec<usize> = doc
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.sketch == sketch && !line.deleted)
        .map(|(i, _)| i)
        .collect();
    for li in lines {
        tombstone_line(doc, li);
    }
    let circles: Vec<usize> = doc
        .circles
        .iter()
        .enumerate()
        .filter(|(_, circle)| circle.sketch == sketch && !circle.deleted)
        .map(|(i, _)| i)
        .collect();
    for ci in circles {
        tombstone_circle(doc, ci);
    }
    let constraints: Vec<usize> = doc
        .constraints
        .iter()
        .enumerate()
        .filter(|(_, c)| c.sketch == sketch && !c.deleted)
        .map(|(i, _)| i)
        .collect();
    for ci in constraints {
        tombstone_constraint(doc, ci);
    }
    let planes: Vec<usize> = doc
        .construction_planes
        .iter()
        .enumerate()
        .filter(|(_, plane)| {
            matches!(plane.parent, crate::model::ConstructionPlaneParent::Sketch(s) if s == sketch)
                && !plane.deleted
        })
        .map(|(i, _)| i)
        .collect();
    for pi in planes {
        tombstone_construction_plane(doc, pi);
    }
    true
}

fn tombstone_rect(doc: &mut Document, index: usize) -> bool {
    let Some(rect) = doc.rects.get_mut(index) else {
        return false;
    };
    if rect.deleted {
        return false;
    }
    rect.deleted = true;
    remove_shape_order_entry(doc, ShapeKind::Rect, index);
    let face = FaceId::Rect(index);
    for sketch in doc.sketches_on_face(face).collect::<Vec<_>>() {
        tombstone_sketch(doc, sketch);
    }
    true
}

fn tombstone_circle(doc: &mut Document, index: usize) -> bool {
    let Some(circle) = doc.circles.get_mut(index) else {
        return false;
    };
    if circle.deleted {
        return false;
    }
    circle.deleted = true;
    remove_shape_order_entry(doc, ShapeKind::Circle, index);
    let face = FaceId::Circle(index);
    for sketch in doc.sketches_on_face(face).collect::<Vec<_>>() {
        tombstone_sketch(doc, sketch);
    }
    true
}

fn tombstone_line(doc: &mut Document, index: usize) -> bool {
    let Some(line) = doc.lines.get_mut(index) else {
        return false;
    };
    if line.deleted {
        return false;
    }
    line.deleted = true;
    remove_shape_order_entry(doc, ShapeKind::Line, index);
    true
}

fn tombstone_constraint(doc: &mut Document, index: usize) -> bool {
    let Some(constraint) = doc.constraints.get_mut(index) else {
        return false;
    };
    if constraint.deleted {
        return false;
    }
    constraint.deleted = true;
    remove_shape_order_entry(doc, ShapeKind::Constraint, index);
    true
}

/// Tombstone a parameter by index (used by `DeleteParameter` and selection delete).
pub fn tombstone_parameter(doc: &mut Document, index: usize) -> bool {
    let Some(param) = doc.parameters.get_mut(index) else {
        return false;
    };
    if param.deleted {
        return false;
    }
    param.deleted = true;
    remove_shape_order_entry(doc, ShapeKind::Parameter, index);
    true
}

pub fn distance_target_alive(doc: &Document, target: &DistanceTarget) -> bool {
    match target {
        DistanceTarget::LineLength(index) => line_alive(doc, *index),
        DistanceTarget::RectWidth(index) | DistanceTarget::RectHeight(index) => {
            rect_alive(doc, *index)
        }
        DistanceTarget::CircleDiameter(index) => circle_alive(doc, *index),
        DistanceTarget::LineLineDistance {
            line_a,
            line_b,
            side: _,
        } => constraint_line_alive(doc, line_a) && constraint_line_alive(doc, line_b),
        DistanceTarget::PointPointDistance { anchor, mover, .. } => {
            constraint_point_alive(doc, anchor) && constraint_point_alive(doc, mover)
        }
        DistanceTarget::PointLineDistance { point, line, .. } => {
            constraint_point_alive(doc, point) && constraint_line_alive(doc, line)
        }
    }
}

pub fn constraint_line_alive(doc: &Document, line: &ConstraintLine) -> bool {
    match line {
        ConstraintLine::Line(index) => line_alive(doc, *index),
        ConstraintLine::RectEdge { rect, .. } => rect_alive(doc, *rect),
        ConstraintLine::FaceEdge { face, index } => {
            crate::extrude::face_boundary_loop_world(doc, face).is_some_and(|l| *index < l.len())
        }
    }
}

pub fn constraint_entity_alive(doc: &Document, entity: &ConstraintEntity) -> bool {
    match entity {
        ConstraintEntity::Point(point) => constraint_point_alive(doc, point),
        ConstraintEntity::Line(line) => constraint_line_alive(doc, line),
        ConstraintEntity::Circle(circle) => circle_alive(doc, *circle),
        ConstraintEntity::Origin => true,
    }
}

pub fn constraint_point_alive(doc: &Document, point: &ConstraintPoint) -> bool {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => line_alive(doc, *line),
        ConstraintPoint::RectCorner { rect, .. } => rect_alive(doc, *rect),
        ConstraintPoint::CircleCenter(circle) => circle_alive(doc, *circle),
        ConstraintPoint::FaceVertex { face, index } => {
            crate::extrude::face_boundary_loop_world(doc, face).is_some_and(|l| *index < l.len())
        }
    }
}

/// Whether a constraint can still be applied (all referenced geometry is alive).
pub fn constraint_kind_applicable(doc: &Document, kind: &ConstraintKind) -> bool {
    match kind {
        ConstraintKind::Distance { target } => distance_target_alive(doc, target),
        ConstraintKind::Parallel { line_a, line_b }
        | ConstraintKind::Perpendicular { line_a, line_b }
        | ConstraintKind::Equal { line_a, line_b } => {
            constraint_line_alive(doc, line_a) && constraint_line_alive(doc, line_b)
        }
        ConstraintKind::Coincident { a, b } => {
            constraint_entity_alive(doc, a) && constraint_entity_alive(doc, b)
        }
        ConstraintKind::Midpoint { point, line } => {
            constraint_point_alive(doc, point) && constraint_line_alive(doc, line)
        }
        ConstraintKind::Horizontal { line } | ConstraintKind::Vertical { line } => {
            constraint_line_alive(doc, line)
        }
        ConstraintKind::Angle {
            line_a,
            line_b,
            rotation_sign: _,
        } => constraint_line_alive(doc, line_a) && constraint_line_alive(doc, line_b),
    }
}

fn remove_shape_order_entry(doc: &mut Document, kind: ShapeKind, ordinal: usize) {
    if let Some(pos) = doc
        .shape_order
        .iter()
        .enumerate()
        .filter(|(_, k)| **k == kind)
        .nth(ordinal)
        .map(|(i, _)| i)
    {
        doc.shape_order.remove(pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Constraint, ConstraintKind, ConstraintLine, Document, Line, Rect};
    use crate::selection::{click_scene_selection, SceneSelection};

    fn sketch_with_two_lines() -> (Document, SketchId, usize, usize) {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        let line_a = 0;
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 5.0, 10.0, 5.0));
        doc.shape_order.push(ShapeKind::Line);
        let line_b = 1;
        (doc, sketch, line_a, line_b)
    }

    #[test]
    fn tombstone_line_preserves_index_for_constraint_refs() {
        let (mut doc, sketch, line_a, line_b) = sketch_with_two_lines();
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
        assert!(tombstone_line(&mut doc, line_a));
        assert!(doc.lines[line_a].deleted);
        assert!(!line_alive(&doc, line_a));
        assert!(line_alive(&doc, line_b));
        assert_eq!(doc.lines.len(), 2);
        let constraint = &doc.constraints[0];
        assert!(matches!(
            constraint.kind,
            ConstraintKind::Parallel {
                line_a: ConstraintLine::Line(0),
                ..
            }
        ));
    }

    #[test]
    fn tombstone_sketch_cascades_geometry() {
        let (mut doc, sketch, line_a, _) = sketch_with_two_lines();
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 5.0, 5.0));
        doc.shape_order.push(ShapeKind::Rect);
        assert!(tombstone_sketch(&mut doc, sketch));
        assert!(doc.sketches[sketch].deleted);
        assert!(doc.lines[line_a].deleted);
        assert!(doc.rects[0].deleted);
        assert!(!element_alive(&doc, SceneElement::Line(line_a)));
    }

    #[test]
    fn delete_targets_from_selection_expands_rect_edge_and_point() {
        use crate::model::{ConstraintPoint, LineEnd, RectEdge};
        let mut sel = SceneSelection::default();
        click_scene_selection(
            &mut sel,
            SceneElement::RectEdge(0, RectEdge::Bottom),
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
        let targets = delete_targets_from_selection(&sel);
        assert_eq!(targets.len(), 2);
        assert!(targets.contains(&SceneElement::Rect(0)));
        assert!(targets.contains(&SceneElement::Line(1)));
    }

    #[test]
    fn tombstone_elements_counts_unique_targets() {
        let (mut doc, _, line_a, line_b) = sketch_with_two_lines();
        let count = tombstone_elements(
            &mut doc,
            &[
                SceneElement::Line(line_a),
                SceneElement::Line(line_b),
            ],
        );
        assert_eq!(count, 2);
        assert!(doc.lines[line_a].deleted);
        assert!(doc.lines[line_b].deleted);
    }

    #[test]
    fn element_alive_false_for_tombstoned_rect() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 1.0, 1.0));
        doc.shape_order.push(ShapeKind::Rect);
        tombstone_rect(&mut doc, 0);
        assert!(!element_alive(&doc, SceneElement::Rect(0)));
        assert!(!element_alive(&doc, SceneElement::RectEdge(0, crate::model::RectEdge::Left)));
    }
}