//! Drag sketch vertices and line segments in the viewport while satisfying active constraints.

use crate::constraints::{constraint_evaluated_length, find_distance_constraint};
use crate::construction::point_sketch;
use crate::geometric_constraints::{
    apply_geometric_constraints, point_uv, set_point_uv, translate_line,
};
use crate::hierarchy::SceneElement;
use crate::model::{
    ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, DistanceTarget, Document,
    LineEnd, RectEdge, SketchId,
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
    let du = current_uv.0 - session.anchor_uv.0;
    let dv = current_uv.1 - session.anchor_uv.1;
    let seeds = line_drag_seed_points(session.target);
    if matches!(session.target, ConstraintLine::RectEdge { .. }) {
        translate_line(doc, session.target, du, dv)?;
        for (point, (iu, iv)) in &session.initial_positions {
            if seeds.iter().any(|seed| seed == point) || !point_in_sketch(doc, *point, sketch) {
                continue;
            }
            set_point_uv(doc, *point, iu + du, iv + dv)?;
        }
    } else {
        for (point, (iu, iv)) in &session.initial_positions {
            if !point_in_sketch(doc, *point, sketch) {
                continue;
            }
            set_point_uv(doc, *point, iu + du, iv + dv)?;
        }
    }
    apply_geometric_constraints(doc)
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

    let group = coincident_group(doc, sketch, dragged);
    for point in &group {
        set_point_uv(doc, *point, u, v)?;
    }

    apply_length_constraints_for_drag(doc, dragged, u, v, &group)?;
    apply_geometric_constraints(doc)
}

fn coincident_group(doc: &Document, sketch: SketchId, seed: ConstraintPoint) -> Vec<ConstraintPoint> {
    let mut group = vec![seed];
    let mut changed = true;
    while changed {
        changed = false;
        for constraint in &doc.constraints {
            if constraint.sketch != sketch {
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
    use crate::constraints::add_distance_constraint;
    use crate::geometric_constraints::{
        add_geometric_constraint_from_selection, GeometricConstraintType,
    };
    use crate::hierarchy::SceneElement;
    use crate::model::{Document, FaceId, Line, Rect};
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
}