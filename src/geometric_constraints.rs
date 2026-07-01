//! Geometric sketch constraints (parallel, perpendicular, coincident, midpoint, horizontal, vertical).
//!
//! The constraint tool exposes these types in the context pane; eligibility depends on the
//! current selection. Distance/dimensional constraints remain on the dimension tool.

use crate::hierarchy::SceneElement;
use crate::model::{
    Constraint, ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, Document,
    LineEnd, SketchId,
};
use crate::selection::SceneSelection;
use glam::Vec3;

/// Constraint types shown in the constraint-tool context pane (fixed display order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeometricConstraintType {
    Parallel,
    Perpendicular,
    Equal,
    Coincident,
    Midpoint,
    Vertical,
    Horizontal,
}

impl GeometricConstraintType {
    pub const ALL: [Self; 7] = [
        Self::Parallel,
        Self::Perpendicular,
        Self::Equal,
        Self::Coincident,
        Self::Midpoint,
        Self::Vertical,
        Self::Horizontal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Parallel => "Parallel",
            Self::Perpendicular => "Perpendicular",
            Self::Equal => "Equal",
            Self::Coincident => "Coincident",
            Self::Midpoint => "Midpoint",
            Self::Vertical => "Vertical",
            Self::Horizontal => "Horizontal",
        }
    }

    /// Fixed context-pane shortcut (shown left of the constraint button). Mnemonic letters
    /// rather than numbers; chosen to avoid the global tool keys (S/R/L/C/O/P/D/E/X/N).
    pub fn shortcut_label(self) -> &'static str {
        match self {
            Self::Parallel => "A",      // p-A-rallel
            Self::Perpendicular => "T", // a "T" is a right angle
            Self::Equal => "Q",         // e-Q-ual
            Self::Coincident => "I",    // co-I-ncident
            Self::Midpoint => "M",      // Midpoint
            Self::Vertical => "V",      // Vertical
            Self::Horizontal => "H",    // Horizontal
        }
    }

    pub fn from_shortcut_key(key: char) -> Option<Self> {
        match key.to_ascii_uppercase() {
            'A' => Some(Self::Parallel),
            'T' => Some(Self::Perpendicular),
            'Q' => Some(Self::Equal),
            'I' => Some(Self::Coincident),
            'M' => Some(Self::Midpoint),
            'V' => Some(Self::Vertical),
            'H' => Some(Self::Horizontal),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConstraintRole {
    Line,
    Point,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConstraintRef {
    Line(ConstraintLine),
    Point(ConstraintPoint),
    /// A whole circle (its perimeter), for point-on-circle coincidence.
    Circle(usize),
}

/// One row in the constraint-tool context pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintPaneRow {
    pub kind: GeometricConstraintType,
    pub enabled: bool,
    /// Role names still needed when disabled (e.g. `"line"`).
    pub missing: Vec<&'static str>,
}

/// Build context-pane rows for the current selection.
pub fn constraint_pane_rows(selection: &SceneSelection) -> Vec<ConstraintPaneRow> {
    let refs = selected_constraint_refs(selection);
    let mut rows = Vec::new();

    // Every constraint type is always shown; ones the current selection can't satisfy appear
    // disabled (faded) with their required roles, so the full set stays discoverable (#22).
    for kind in GeometricConstraintType::ALL {
        let (enabled, missing) = if refs.is_empty() {
            (false, missing_for_kind(kind))
        } else {
            match match_kind(kind, &refs) {
                Some(state) => state,
                None => (false, missing_for_kind(kind)),
            }
        };
        rows.push(ConstraintPaneRow {
            kind,
            enabled,
            missing,
        });
    }

    rows
}

fn missing_for_kind(kind: GeometricConstraintType) -> Vec<&'static str> {
    match kind {
        GeometricConstraintType::Parallel
        | GeometricConstraintType::Perpendicular
        | GeometricConstraintType::Equal => {
            vec!["line", "line"]
        }
        GeometricConstraintType::Coincident => vec!["point", "point, line, or circle"],
        GeometricConstraintType::Midpoint => vec!["point", "line"],
        GeometricConstraintType::Vertical | GeometricConstraintType::Horizontal => vec!["line"],
    }
}

fn match_kind(
    kind: GeometricConstraintType,
    refs: &[ConstraintRef],
) -> Option<(bool, Vec<&'static str>)> {
    let patterns: &[&[ConstraintRole]] = match kind {
        GeometricConstraintType::Parallel
        | GeometricConstraintType::Perpendicular
        | GeometricConstraintType::Equal => {
            &[&[ConstraintRole::Line, ConstraintRole::Line][..]]
        }
        GeometricConstraintType::Coincident => {
            let points = refs
                .iter()
                .filter(|r| matches!(r, ConstraintRef::Point(_)))
                .count();
            let circles = refs
                .iter()
                .filter(|r| matches!(r, ConstraintRef::Circle(_)))
                .count();
            let lines = refs
                .iter()
                .filter(|r| matches!(r, ConstraintRef::Line(_)))
                .count();
            if points >= 2 {
                return Some((true, Vec::new()));
            }
            // A point on a circle's perimeter (point-on-circle coincidence).
            if circles > 0 {
                return if points == 1 && circles == 1 && lines == 0 {
                    Some((true, Vec::new()))
                } else if points == 0 && circles == 1 && lines == 0 {
                    Some((false, vec!["point"]))
                } else {
                    None
                };
            }
            &[
                &[ConstraintRole::Point, ConstraintRole::Point][..],
                &[ConstraintRole::Point, ConstraintRole::Line][..],
            ]
        }
        GeometricConstraintType::Midpoint => &[&[ConstraintRole::Point, ConstraintRole::Line][..]],
        GeometricConstraintType::Vertical | GeometricConstraintType::Horizontal => {
            &[&[ConstraintRole::Line][..]]
        }
    };

    let mut best: Option<(bool, Vec<&'static str>)> = None;
    for pattern in patterns {
        if let Some(state) = match_pattern(pattern, refs) {
            let replace = best.as_ref().is_none_or(|(enabled, _)| {
                if state.0 && !*enabled {
                    true
                } else if state.0 == *enabled {
                    state.1.len() < best.as_ref().unwrap().1.len()
                } else {
                    false
                }
            });
            if replace {
                best = Some(state);
            }
        }
    }
    best
}

fn match_pattern(pattern: &[ConstraintRole], refs: &[ConstraintRef]) -> Option<(bool, Vec<&'static str>)> {
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

    let mut line_i = 0usize;
    let mut point_i = 0usize;
    for (role_i, role) in pattern.iter().enumerate() {
        match role {
            ConstraintRole::Line => {
                if line_i < lines.len() {
                    line_i += 1;
                } else {
                    let lines_still_needed = pattern[role_i..]
                        .iter()
                        .filter(|r| **r == ConstraintRole::Line)
                        .count();
                    if lines.len().saturating_sub(line_i) > lines_still_needed {
                        return None;
                    }
                    let points_still_needed = pattern[role_i..]
                        .iter()
                        .filter(|r| **r == ConstraintRole::Point)
                        .count();
                    if points.len().saturating_sub(point_i) > points_still_needed {
                        return None;
                    }
                    return Some((false, vec![role_name(*role)]));
                }
            }
            ConstraintRole::Point => {
                if point_i < points.len() {
                    point_i += 1;
                } else {
                    let points_still_needed = pattern[role_i..]
                        .iter()
                        .filter(|r| **r == ConstraintRole::Point)
                        .count();
                    if points.len().saturating_sub(point_i) > points_still_needed {
                        return None;
                    }
                    let lines_still_needed = pattern[role_i..]
                        .iter()
                        .filter(|r| **r == ConstraintRole::Line)
                        .count();
                    if lines.len().saturating_sub(line_i) > lines_still_needed {
                        return None;
                    }
                    return Some((false, vec![role_name(*role)]));
                }
            }
        }
    }

    if line_i < lines.len() || point_i < points.len() {
        return None;
    }
    Some((true, Vec::new()))
}

fn role_name(role: ConstraintRole) -> &'static str {
    match role {
        ConstraintRole::Line => "line",
        ConstraintRole::Point => "point",
    }
}

pub fn selected_constraint_refs(selection: &SceneSelection) -> Vec<ConstraintRef> {
    let mut refs: Vec<ConstraintRef> = selection
        .iter()
        .filter_map(scene_element_to_constraint_ref)
        .collect();
    refs.sort_by_key(|reference| constraint_ref_sort_key(reference.clone()));
    refs
}

pub fn scene_element_to_constraint_ref(element: SceneElement) -> Option<ConstraintRef> {
    match element {
        SceneElement::Line(index) => Some(ConstraintRef::Line(ConstraintLine::Line(index))),
        SceneElement::Point(point) => Some(ConstraintRef::Point(point)),
        SceneElement::Circle(index) => Some(ConstraintRef::Circle(index)),
        // A face's own edge (#26/#27) — picked in the viewport via `SceneElement::FaceEdge`,
        // flows into the same Coincident/Midpoint/PointLineDistance pane as any other line.
        SceneElement::FaceEdge(line) => Some(ConstraintRef::Line(line)),
        _ => None,
    }
}

fn constraint_ref_sort_key(reference: ConstraintRef) -> (u8, usize, u8, u8) {
    match reference {
        ConstraintRef::Line(ConstraintLine::Line(i)) => (0, i, 0, 0),
        ConstraintRef::Point(ConstraintPoint::LineEndpoint { line, end }) => {
            (2, line, end as u8, 0)
        }
        ConstraintRef::Point(ConstraintPoint::CircleCenter(i)) => (4, i, 0, 0),
        ConstraintRef::Circle(i) => (5, i, 0, 0),
        ConstraintRef::Point(ConstraintPoint::FaceVertex { index, .. }) => (6, index, 0, 0),
        ConstraintRef::Line(ConstraintLine::FaceEdge { index, .. }) => (7, index, 0, 0),
    }
}

/// Enabled constraint type for a fixed shortcut key, if the row is active.
pub fn enabled_constraint_for_key(
    rows: &[ConstraintPaneRow],
    key: char,
) -> Option<GeometricConstraintType> {
    let kind = GeometricConstraintType::from_shortcut_key(key)?;
    rows.iter()
        .find(|row| row.kind == kind && row.enabled)
        .map(|row| row.kind)
}

/// When exactly one constraint row is enabled, return its type (for direct `C` apply).
pub fn sole_enabled_constraint_type(rows: &[ConstraintPaneRow]) -> Option<GeometricConstraintType> {
    let mut enabled = rows.iter().filter(|row| row.enabled);
    let first = enabled.next()?;
    if enabled.next().is_some() {
        return None;
    }
    Some(first.kind)
}

/// Add a geometric constraint from the current selection; returns the new constraint index.
pub fn add_geometric_constraint_from_selection(
    doc: &mut Document,
    sketch: SketchId,
    kind: GeometricConstraintType,
    selection: &SceneSelection,
) -> Result<usize, String> {
    let refs = selected_constraint_refs(selection);
    let rows = constraint_pane_rows(selection);
    let enabled = rows
        .iter()
        .find(|row| row.kind == kind && row.enabled)
        .ok_or_else(|| format!("{kind:?} constraint is not enabled for the current selection"))?;

    if !enabled.enabled {
        return Err(format!("{kind:?} constraint is not enabled for the current selection"));
    }

    let constraint_kind = build_constraint_kind(kind, &refs)?;
    validate_constraint_kind(doc, sketch, constraint_kind.clone())?;
    let id = doc.constraints.len();
    doc.constraints.push(Constraint {
        sketch,
        kind: constraint_kind,
        expression: String::new(),
        dim_offset: None,
        name: None,
        deleted: false,
    });
    doc.shape_order.push(crate::model::ShapeKind::Constraint);
    crate::constraints::remove_subsumed_point_on_line(doc, sketch, id);
    crate::constraints::solve_document_constraints(doc)?;
    Ok(id)
}

fn build_constraint_kind(
    kind: GeometricConstraintType,
    refs: &[ConstraintRef],
) -> Result<ConstraintKind, String> {
    let lines: Vec<ConstraintLine> = refs
        .iter()
        .filter_map(|r| match r {
            ConstraintRef::Line(line) => Some(line.clone()),
            _ => None,
        })
        .collect();
    let points: Vec<ConstraintPoint> = refs
        .iter()
        .filter_map(|r| match r {
            ConstraintRef::Point(point) => Some(point.clone()),
            _ => None,
        })
        .collect();
    let circles: Vec<usize> = refs
        .iter()
        .filter_map(|r| match r {
            ConstraintRef::Circle(index) => Some(*index),
            _ => None,
        })
        .collect();

    match kind {
        GeometricConstraintType::Parallel => Ok(ConstraintKind::Parallel {
            line_a: lines[0].clone(),
            line_b: lines[1].clone(),
        }),
        GeometricConstraintType::Perpendicular => Ok(ConstraintKind::Perpendicular {
            line_a: lines[0].clone(),
            line_b: lines[1].clone(),
        }),
        GeometricConstraintType::Equal => Ok(ConstraintKind::Equal {
            line_a: lines[0].clone(),
            line_b: lines[1].clone(),
        }),
        GeometricConstraintType::Horizontal => Ok(ConstraintKind::Horizontal { line: lines[0].clone() }),
        GeometricConstraintType::Vertical => Ok(ConstraintKind::Vertical { line: lines[0].clone() }),
        GeometricConstraintType::Coincident => {
            if points.len() >= 2 {
                Ok(ConstraintKind::Coincident {
                    a: ConstraintEntity::Point(points[0].clone()),
                    b: ConstraintEntity::Point(points[1].clone()),
                })
            } else if points.len() == 1 && !circles.is_empty() {
                Ok(ConstraintKind::Coincident {
                    a: ConstraintEntity::Point(points[0].clone()),
                    b: ConstraintEntity::Circle(circles[0]),
                })
            } else if points.len() == 1 && !lines.is_empty() {
                Ok(ConstraintKind::Coincident {
                    a: ConstraintEntity::Point(points[0].clone()),
                    b: ConstraintEntity::Line(lines[0].clone()),
                })
            } else {
                Err("Coincident requires two points, a point and a line, or a point and a circle"
                    .to_string())
            }
        }
        GeometricConstraintType::Midpoint => Ok(ConstraintKind::Midpoint {
            point: points[0].clone(),
            line: lines[0].clone(),
        }),
    }
}

fn validate_constraint_kind(
    doc: &Document,
    sketch: SketchId,
    kind: ConstraintKind,
) -> Result<(), String> {
    match kind {
        ConstraintKind::Distance { target } => {
            crate::constraints::validate_distance_target(doc, sketch, target)
        }
        ConstraintKind::Parallel { line_a, line_b }
        | ConstraintKind::Perpendicular { line_a, line_b }
        | ConstraintKind::Equal { line_a, line_b } => {
            validate_line_ref(doc, sketch, &line_a)?;
            validate_line_ref(doc, sketch, &line_b)?;
            if line_a == line_b {
                return Err("Constraint requires two different lines".to_string());
            }
            Ok(())
        }
        ConstraintKind::Horizontal { line } | ConstraintKind::Vertical { line } => {
            validate_line_ref(doc, sketch, &line)
        }
        ConstraintKind::Coincident { a, b } => {
            validate_entity_ref(doc, sketch, &a)?;
            validate_entity_ref(doc, sketch, &b)?;
            if a == b {
                return Err("Constraint requires two different entities".to_string());
            }
            Ok(())
        }
        ConstraintKind::Midpoint { point, line } => {
            validate_point_ref(doc, sketch, &point)?;
            validate_line_ref(doc, sketch, &line)?;
            Ok(())
        }
        ConstraintKind::Angle {
            line_a,
            line_b,
            rotation_sign: _,
        } => {
            validate_line_ref(doc, sketch, &line_a)?;
            validate_line_ref(doc, sketch, &line_b)?;
            if line_a == line_b {
                return Err("Constraint requires two different lines".to_string());
            }
            if lines_are_parallel(doc, sketch, line_a, line_b) {
                return Err("Angle constraint requires non-parallel lines".to_string());
            }
            Ok(())
        }
    }
}

/// Whether a `ConstraintPoint`/`ConstraintLine`'s face reference still resolves: the extrusion
/// exists and hasn't been deleted, and `index` is still within its current boundary loop. This
/// is the same "does the reference still resolve" check `document_health.rs` uses for dangling
/// sketch-native references, just applied to a face's own (extrusion-derived) geometry instead.
pub(crate) fn face_vertex_valid(doc: &Document, face: &crate::model::FaceId, index: usize) -> bool {
    crate::extrude::face_boundary_loop_world(doc, face).is_some_and(|loop_| index < loop_.len())
}

pub(crate) fn face_edge_valid(doc: &Document, face: &crate::model::FaceId, index: usize) -> bool {
    crate::extrude::face_boundary_loop_world(doc, face)
        .is_some_and(|loop_| !loop_.is_empty() && index < loop_.len())
}

fn validate_line_ref(doc: &Document, sketch: SketchId, line: &ConstraintLine) -> Result<(), String> {
    match line {
        ConstraintLine::Line(index) => {
            let entity = doc
                .lines
                .get(*index)
                .ok_or_else(|| format!("Line {index} not found"))?;
            if entity.sketch != sketch {
                return Err(format!("Line {index} is not in sketch {sketch}"));
            }
        }
        // A face's own edge has no owning sketch — it's valid for any sketch, as long as the
        // underlying extrusion/face still resolves (mirrors `ConstraintEntity::Origin`, which
        // is likewise valid regardless of `sketch`).
        ConstraintLine::FaceEdge { face, index } => {
            if !face_edge_valid(doc, face, *index) {
                return Err(format!("Face edge {index} no longer resolves"));
            }
        }
    }
    Ok(())
}

fn validate_entity_ref(
    doc: &Document,
    sketch: SketchId,
    entity: &ConstraintEntity,
) -> Result<(), String> {
    match entity {
        ConstraintEntity::Line(line) => validate_line_ref(doc, sketch, line),
        ConstraintEntity::Point(point) => validate_point_ref(doc, sketch, point),
        ConstraintEntity::Circle(circle) => {
            let entity = doc
                .circles
                .get(*circle)
                .ok_or_else(|| format!("Circle {circle} not found"))?;
            if entity.sketch != sketch {
                return Err(format!("Circle {circle} is not in sketch {sketch}"));
            }
            Ok(())
        }
        // The origin is a fixed point in every sketch; always valid.
        ConstraintEntity::Origin => Ok(()),
    }
}

fn validate_point_ref(doc: &Document, sketch: SketchId, point: &ConstraintPoint) -> Result<(), String> {
    match point {
        ConstraintPoint::LineEndpoint { line, end: _ } => {
            let entity = doc
                .lines
                .get(*line)
                .ok_or_else(|| format!("Line {line} not found"))?;
            if entity.sketch != sketch {
                return Err(format!("Line {line} is not in sketch {sketch}"));
            }
        }
        ConstraintPoint::CircleCenter(circle) => {
            let entity = doc
                .circles
                .get(*circle)
                .ok_or_else(|| format!("Circle {circle} not found"))?;
            if entity.sketch != sketch {
                return Err(format!("Circle {circle} is not in sketch {sketch}"));
            }
        }
        // A face's own vertex has no owning sketch — valid for any sketch as long as the
        // underlying extrusion/face still resolves. See `validate_line_ref`'s `FaceEdge` arm.
        ConstraintPoint::FaceVertex { face, index } => {
            if !face_vertex_valid(doc, face, *index) {
                return Err(format!("Face vertex {index} no longer resolves"));
            }
        }
    }
    Ok(())
}

/// Prefer moving sketch lines over rectangle edges when one side of the pair is fixed.
pub fn parallel_reference_and_movable(
    line_a: ConstraintLine,
    line_b: ConstraintLine,
) -> (ConstraintLine, ConstraintLine) {
    (line_a, line_b)
}

/// Prefer moving free line/circle points over rectangle corners, which reshape the rect.
pub fn coincident_mover_and_anchor(
    a: ConstraintPoint,
    b: ConstraintPoint,
) -> (ConstraintPoint, ConstraintPoint) {
    let a_mobility = coincident_point_mobility(&a);
    let b_mobility = coincident_point_mobility(&b);
    if a_mobility > b_mobility {
        (a, b)
    } else if b_mobility > a_mobility {
        (b, a)
    } else {
        (b, a)
    }
}

fn coincident_point_mobility(point: &ConstraintPoint) -> u8 {
    match point {
        ConstraintPoint::LineEndpoint { .. } | ConstraintPoint::CircleCenter(_) => 2,
        // Fixed by the body's own geometry: never the mover, so it always ranks below every
        // draggable sketch-native point (mirrors `ConstraintEntity::Origin`'s fixed treatment).
        ConstraintPoint::FaceVertex { .. } => 0,
    }
}

/// Whether two sketch lines are parallel (within tolerance).
pub fn lines_are_parallel(
    doc: &Document,
    sketch: SketchId,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
) -> bool {
    let Ok(((ax0, ay0), (ax1, ay1))) = line_uv_endpoints(doc, sketch, line_a) else {
        return false;
    };
    let Ok(((bx0, by0), (bx1, by1))) = line_uv_endpoints(doc, sketch, line_b) else {
        return false;
    };
    let adu = ax1 - ax0;
    let adv = ay1 - ay0;
    let bdu = bx1 - bx0;
    let bdv = by1 - by0;
    let alen = (adu * adu + adv * adv).sqrt();
    let blen = (bdu * bdu + bdv * bdv).sqrt();
    if alen < 1e-6 || blen < 1e-6 {
        return false;
    }
    let cross = adu * bdv - adv * bdu;
    (cross / (alen * blen)).abs() < 1e-3
}

/// Unit direction `(du, dv)` of a constraint line in sketch UV coordinates.
pub fn line_direction_uv(doc: &Document, sketch: SketchId, line: ConstraintLine) -> Option<(f32, f32)> {
    let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, sketch, line).ok()?;
    let du = x1 - x0;
    let dv = y1 - y0;
    let len = (du * du + dv * dv).sqrt();
    if len < 1e-6 {
        return None;
    }
    Some((du / len, dv / len))
}

pub fn line_uv_endpoints(
    doc: &Document,
    sketch: SketchId,
    line: ConstraintLine,
) -> Result<((f32, f32), (f32, f32)), String> {
    match line {
        ConstraintLine::Line(index) => {
            let entity = doc
                .lines
                .get(index)
                .ok_or_else(|| format!("Line {index} not found"))?;
            Ok(((entity.x0, entity.y0), (entity.x1, entity.y1)))
        }
        // A face's own edge has no stored local coordinate: it's a body-space 3D segment
        // resolved from the extrusion's analytic boundary and projected into `sketch`'s frame.
        ConstraintLine::FaceEdge { face, index } => {
            let (a, b) = face_edge_world(doc, &face, index)?;
            let frame = crate::face::sketch_geometry_frame(doc, sketch)
                .ok_or_else(|| "Sketch frame not available".to_string())?;
            Ok((
                crate::face::world_to_local(&frame, a),
                crate::face::world_to_local(&frame, b),
            ))
        }
    }
}

/// The world-space endpoints of a `FaceEdge`: `boundary_loop[index]` to
/// `boundary_loop[(index + 1) % boundary_loop.len()]`. `Err` if the extrusion/face can no
/// longer be resolved (e.g. the extrusion was deleted) or `index` is out of range.
fn face_edge_world(doc: &Document, face: &crate::model::FaceId, index: usize) -> Result<(Vec3, Vec3), String> {
    let boundary = crate::extrude::face_boundary_loop_world(doc, face)
        .ok_or_else(|| "Face boundary not available".to_string())?;
    let n = boundary.len();
    if n == 0 || index >= n {
        return Err(format!("Face edge {index} out of range"));
    }
    Ok((boundary[index], boundary[(index + 1) % n]))
}

/// Translate a sketch line or rectangle edge by `(du, dv)` in face-local coordinates.
pub fn translate_line(
    doc: &mut Document,
    sketch: SketchId,
    line: ConstraintLine,
    du: f32,
    dv: f32,
) -> Result<(), String> {
    let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, sketch, line.clone())?;
    set_line_uv_endpoints(doc, sketch, line, (x0 + du, y0 + dv), (x1 + du, y1 + dv))
}

pub fn set_line_uv_endpoints(
    doc: &mut Document,
    sketch: SketchId,
    line: ConstraintLine,
    start: (f32, f32),
    end: (f32, f32),
) -> Result<(), String> {
    let _ = sketch;
    match line {
        ConstraintLine::Line(index) => {
            let entity = doc
                .lines
                .get_mut(index)
                .ok_or_else(|| format!("Line {index} not found"))?;
            entity.x0 = start.0;
            entity.y0 = start.1;
            entity.x1 = end.0;
            entity.y1 = end.1;
            Ok(())
        }
        // Fixed by the body's own geometry, not by the sketch — mirrors how
        // `ConstraintEntity::Origin` is treated as a fixed, undraggable reference.
        ConstraintLine::FaceEdge { .. } => Err("Face edges are fixed and cannot be moved".to_string()),
    }
}

pub fn point_uv(doc: &Document, sketch: SketchId, point: ConstraintPoint) -> Result<(f32, f32), String> {
    match point {
        ConstraintPoint::LineEndpoint { line, end } => {
            let entity = doc
                .lines
                .get(line)
                .ok_or_else(|| format!("Line {line} not found"))?;
            Ok(match end {
                LineEnd::Start => (entity.x0, entity.y0),
                LineEnd::End => (entity.x1, entity.y1),
            })
        }
        ConstraintPoint::CircleCenter(circle) => {
            let entity = doc
                .circles
                .get(circle)
                .ok_or_else(|| format!("Circle {circle} not found"))?;
            Ok((entity.cx, entity.cy))
        }
        // A face's own vertex has no stored local coordinate: it's a body-space 3D point
        // resolved from the extrusion's analytic boundary and projected into `sketch`'s frame.
        ConstraintPoint::FaceVertex { face, index } => {
            let boundary = crate::extrude::face_boundary_loop_world(doc, &face)
                .ok_or_else(|| "Face boundary not available".to_string())?;
            let world = *boundary
                .get(index)
                .ok_or_else(|| format!("Face vertex {index} out of range"))?;
            let frame = crate::face::sketch_geometry_frame(doc, sketch)
                .ok_or_else(|| "Sketch frame not available".to_string())?;
            Ok(crate::face::world_to_local(&frame, world))
        }
    }
}

pub fn set_point_uv(
    doc: &mut Document,
    sketch: SketchId,
    point: ConstraintPoint,
    u: f32,
    v: f32,
) -> Result<(), String> {
    let _ = sketch;
    match point {
        ConstraintPoint::LineEndpoint { line, end } => {
            let entity = doc
                .lines
                .get_mut(line)
                .ok_or_else(|| format!("Line {line} not found"))?;
            match end {
                LineEnd::Start => {
                    entity.x0 = u;
                    entity.y0 = v;
                }
                LineEnd::End => {
                    entity.x1 = u;
                    entity.y1 = v;
                }
            }
            Ok(())
        }
        ConstraintPoint::CircleCenter(circle) => {
            let entity = doc
                .circles
                .get_mut(circle)
                .ok_or_else(|| format!("Circle {circle} not found"))?;
            entity.cx = u;
            entity.cy = v;
            Ok(())
        }
        // Fixed by the body's own geometry, not by the sketch — mirrors how
        // `ConstraintEntity::Origin` is treated as a fixed, undraggable reference.
        ConstraintPoint::FaceVertex { .. } => Err("Face vertices are fixed and cannot be moved".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Circle, Document, FaceId, Line, Rect, ShapeKind};
    use crate::selection::{click_scene_selection, SceneSelection};

    const EPS: f32 = 1e-2;

    fn sketch_doc() -> (Document, SketchId) {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        (doc, sketch)
    }

    fn line_dir(line: &Line) -> (f32, f32) {
        (line.x1 - line.x0, line.y1 - line.y0)
    }

    fn assert_dirs_parallel(du0: f32, dv0: f32, du1: f32, dv1: f32) {
        let cross = du0 * dv1 - dv0 * du1;
        assert!(
            cross.abs() < EPS,
            "directions not parallel: ({du0},{dv0}) vs ({du1},{dv1}), cross={cross}"
        );
    }

    fn assert_points_equal(doc: &Document, sketch: SketchId, a: ConstraintPoint, b: ConstraintPoint) {
        let (au, av) = point_uv(doc, sketch, a).unwrap();
        let (bu, bv) = point_uv(doc, sketch, b).unwrap();
        assert!(
            (au - bu).abs() < EPS && (av - bv).abs() < EPS,
            "points not equal: ({au},{av}) vs ({bu},{bv})"
        );
    }

    fn push_constraint(doc: &mut Document, sketch: SketchId, kind: ConstraintKind) {
        doc.constraints.push(Constraint {
            sketch,
            kind,
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        doc.shape_order.push(ShapeKind::Constraint);
        crate::constraints::solve_document_constraints(doc).unwrap();
    }

    fn select_two_points(
        sel: &mut SceneSelection,
        first: SceneElement,
        second: SceneElement,
    ) {
        click_scene_selection(sel, first, false);
        click_scene_selection(sel, second, true);
    }

    #[test]
    fn empty_selection_shows_all_constraints_disabled() {
        let rows = constraint_pane_rows(&SceneSelection::default());
        assert_eq!(rows.len(), GeometricConstraintType::ALL.len());
        assert!(rows.iter().all(|row| !row.enabled));
        assert_eq!(rows[0].kind, GeometricConstraintType::Parallel);
        assert_eq!(rows[0].missing, vec!["line", "line"]);
    }

    #[test]
    fn constraint_shortcut_keys_are_fixed_per_type() {
        assert_eq!(GeometricConstraintType::Parallel.shortcut_label(), "A");
        assert_eq!(
            GeometricConstraintType::from_shortcut_key('A'),
            Some(GeometricConstraintType::Parallel)
        );
        // Case-insensitive.
        assert_eq!(
            GeometricConstraintType::from_shortcut_key('h'),
            Some(GeometricConstraintType::Horizontal)
        );
        // No constraint uses a global tool key, so they never collide.
        for key in ['S', 'R', 'L', 'C', 'O', 'P', 'D', 'E', 'X', 'N'] {
            assert!(
                GeometricConstraintType::from_shortcut_key(key).is_none(),
                "{key} collides with a tool shortcut"
            );
        }
    }

    #[test]
    fn single_line_enables_vertical_and_horizontal_only() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 5.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        let rows = constraint_pane_rows(&sel);
        let by_kind: std::collections::HashMap<_, _> =
            rows.iter().map(|row| (row.kind, row)).collect();
        assert_eq!(by_kind.len(), GeometricConstraintType::ALL.len());
        assert!(!by_kind[&GeometricConstraintType::Parallel].enabled);
        assert!(!by_kind[&GeometricConstraintType::Perpendicular].enabled);
        assert!(!by_kind[&GeometricConstraintType::Coincident].enabled);
        assert!(!by_kind[&GeometricConstraintType::Midpoint].enabled);
        assert!(by_kind[&GeometricConstraintType::Vertical].enabled);
        assert!(by_kind[&GeometricConstraintType::Horizontal].enabled);
        assert_eq!(
            enabled_constraint_for_key(&rows, 'V'),
            Some(GeometricConstraintType::Vertical)
        );
        assert_eq!(
            enabled_constraint_for_key(&rows, 'H'),
            Some(GeometricConstraintType::Horizontal)
        );
        // A non-applicable type (Parallel) is present but disabled, not hidden.
        assert_eq!(enabled_constraint_for_key(&rows, 'A'), None);
        assert_eq!(sole_enabled_constraint_type(&rows), None);
        let _ = doc;
    }

    #[test]
    fn line_and_point_enable_only_coincident_and_midpoint() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(
            &mut sel,
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::Start,
            }),
            false,
        );
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        let rows = constraint_pane_rows(&sel);
        // All six types are present; only Coincident and Midpoint are enabled.
        assert_eq!(rows.len(), GeometricConstraintType::ALL.len());
        let by_kind: std::collections::HashMap<_, _> =
            rows.iter().map(|row| (row.kind, row)).collect();
        assert!(by_kind[&GeometricConstraintType::Coincident].enabled);
        assert!(by_kind[&GeometricConstraintType::Midpoint].enabled);
        assert!(!by_kind[&GeometricConstraintType::Parallel].enabled);
        assert!(!by_kind[&GeometricConstraintType::Vertical].enabled);
        assert_eq!(
            enabled_constraint_for_key(&rows, 'I'),
            Some(GeometricConstraintType::Coincident)
        );
        assert_eq!(
            enabled_constraint_for_key(&rows, 'M'),
            Some(GeometricConstraintType::Midpoint)
        );
        assert_eq!(sole_enabled_constraint_type(&rows), None);
        let _ = doc;
    }

    #[test]
    fn sole_enabled_constraint_type_returns_single_row() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 5.0, 5.0, 5.0));
        let mut sel = SceneSelection::default();
        select_two_points(
            &mut sel,
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::End,
            }),
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::Start,
            }),
        );
        let rows = constraint_pane_rows(&sel);
        // All types listed; exactly one (Coincident) is enabled.
        assert_eq!(rows.len(), GeometricConstraintType::ALL.len());
        assert_eq!(rows.iter().filter(|r| r.enabled).count(), 1);
        assert_eq!(sole_enabled_constraint_type(&rows), Some(GeometricConstraintType::Coincident));
        assert_eq!(
            enabled_constraint_for_key(&rows, 'I'),
            Some(GeometricConstraintType::Coincident)
        );
        let _ = doc;
    }

    #[test]
    fn horizontal_constraint_flattens_line() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 5.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Horizontal,
            &sel,
        )
        .unwrap();
        assert!((doc.lines[0].y0 - doc.lines[0].y1).abs() < EPS);
    }

    #[test]
    fn parallel_constraint_aligns_direction() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 5.0, 2.0, 8.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Parallel,
            &sel,
        )
        .unwrap();
        let (du0, dv0) = line_dir(&doc.lines[0]);
        let (du1, dv1) = line_dir(&doc.lines[1]);
        assert_dirs_parallel(du0, dv0, du1, dv1);
    }

    #[test]
    fn equal_constraint_enabled_for_two_lines_and_equalizes_length() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 5.0, 3.0, 5.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        // Pane shows Equal as enabled for two selected lines.
        let rows = constraint_pane_rows(&sel);
        assert!(
            rows.iter()
                .any(|row| row.kind == GeometricConstraintType::Equal && row.enabled),
            "Equal should be enabled for two lines"
        );
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Equal,
            &sel,
        )
        .unwrap();
        assert!(
            (doc.lines[0].length() - doc.lines[1].length()).abs() < EPS,
            "lengths: {} vs {}",
            doc.lines[0].length(),
            doc.lines[1].length()
        );
    }

    #[test]
    fn parallel_line_to_rect_edge_moves_line_not_rectangle() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 10.0, 3.0, 13.0));
        let rect_before = doc.rects[0].clone();

        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Bottom), false);
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Parallel,
            &sel,
        )
        .unwrap();

        let rect = &doc.rects[0];
        assert!((rect.x - rect_before.x).abs() < EPS);
        assert!((rect.y - rect_before.y).abs() < EPS);
        assert!((rect.w - rect_before.w).abs() < EPS);
        assert!((rect.h - rect_before.h).abs() < EPS);

        let edge_dir = (10.0_f32, 0.0);
        let (du, dv) = line_dir(&doc.lines[0]);
        assert_dirs_parallel(edge_dir.0, edge_dir.1, du, dv);
    }

    #[test]
    fn rect_edge_counts_as_line_for_parallel() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 10.0, 5.0, 15.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Bottom), false);
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        let rows = constraint_pane_rows(&sel);
        assert!(rows.iter().any(|row| row.kind == GeometricConstraintType::Parallel && row.enabled));
        let _ = doc;
    }

    #[test]
    fn coincident_point_on_line_projects() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 5.0, 8.0, 6.0, 9.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(
            &mut sel,
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::Start,
            }),
            false,
        );
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();
        assert!((doc.lines[1].x0 - 5.0).abs() < EPS);
        assert!(doc.lines[1].y0.abs() < EPS);
    }

    #[test]
    fn coincident_point_on_circle_perimeter() {
        let (mut doc, sketch) = sketch_doc();
        // Circle radius 10 centered at origin.
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 0.0, 0.0, 10.0, 0.0));
        // A line whose start sits inside the circle.
        doc.lines
            .push(Line::from_local_endpoints(sketch, 3.0, 1.0, 20.0, 20.0));
        doc.shape_order.push(ShapeKind::Circle);
        doc.shape_order.push(ShapeKind::Line);

        let point = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Point(point.clone()), false);
        click_scene_selection(&mut sel, SceneElement::Circle(0), true);

        // Selecting a point and a circle enables Coincident.
        let rows = constraint_pane_rows(&sel);
        assert!(rows
            .iter()
            .any(|row| row.kind == GeometricConstraintType::Coincident && row.enabled));

        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();

        let (pu, pv) = point_uv(&doc, sketch, point).unwrap();
        assert!(
            (pu.hypot(pv) - 10.0).abs() < EPS,
            "point should land on the perimeter (r=10), distance={}",
            pu.hypot(pv)
        );
        assert!(matches!(
            doc.constraints[0].kind,
            ConstraintKind::Coincident {
                a: ConstraintEntity::Point(_),
                b: ConstraintEntity::Circle(0),
            }
        ));
    }

    #[test]
    fn coincident_rect_corner_and_line_vertex_meet_at_corner() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 3.0, 7.0, 8.0, 7.0));
        let rect_corner = ConstraintPoint::RectCorner { rect: 0, corner: 0 };
        let line_start = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };

        let mut sel = SceneSelection::default();
        select_two_points(
            &mut sel,
            SceneElement::Point(rect_corner.clone()),
            SceneElement::Point(line_start.clone()),
        );
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();

        let (corner_u, corner_v) = point_uv(&doc, sketch, rect_corner).unwrap();
        let (line_u, line_v) = point_uv(&doc, sketch, line_start).unwrap();
        assert!((corner_u - 0.0).abs() < EPS && (corner_v - 0.0).abs() < EPS);
        assert!((line_u - corner_u).abs() < EPS && (line_v - corner_v).abs() < EPS);

        let kind = doc.constraints[0].kind.clone();
        assert!(matches!(
            kind,
            ConstraintKind::Coincident {
                a: ConstraintEntity::Point(_),
                b: ConstraintEntity::Point(_),
            }
        ));
    }

    #[test]
    fn coincident_two_points_wins_over_rect_edge_in_selection() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 4.0, 9.0, 8.0, 9.0));
        let rect_corner = ConstraintPoint::RectCorner { rect: 0, corner: 1 };
        let line_start = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };

        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Point(rect_corner.clone()), false);
        click_scene_selection(&mut sel, SceneElement::Point(line_start.clone()), true);
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Bottom), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();

        let (corner_u, corner_v) = point_uv(&doc, sketch, rect_corner).unwrap();
        let (line_u, line_v) = point_uv(&doc, sketch, line_start).unwrap();
        assert!((corner_u - 10.0).abs() < EPS && (corner_v - 0.0).abs() < EPS);
        assert!((line_u - corner_u).abs() < EPS && (line_v - corner_v).abs() < EPS);
        assert!(matches!(
            doc.constraints[0].kind,
            ConstraintKind::Coincident {
                a: ConstraintEntity::Point(_),
                b: ConstraintEntity::Point(_),
            }
        ));
    }

    #[test]
    fn pinning_to_endpoint_removes_earlier_point_on_line() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 3.0, 4.0, 7.0, 9.0));
        let free = ConstraintPoint::LineEndpoint {
            line: 1,
            end: LineEnd::Start,
        };

        // First: generic point-on-line (point + line selected).
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Point(free.clone()), false);
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        let on_line = add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();

        // Later: pin to a specific endpoint of that same line (two points selected).
        let endpoint = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::End,
        };
        let mut sel2 = SceneSelection::default();
        select_two_points(
            &mut sel2,
            SceneElement::Point(free),
            SceneElement::Point(endpoint),
        );
        let specific = add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel2,
        )
        .unwrap();

        assert!(doc.constraints[on_line].deleted, "point-on-line should be superseded");
        assert!(!doc.constraints[specific].deleted);
    }

    #[test]
    fn perpendicular_preserves_coincident_points() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 5.0, 5.0, 15.0, 5.0));

        let shared_on_line0 = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::End,
        };
        let shared_on_line1 = ConstraintPoint::LineEndpoint {
            line: 1,
            end: LineEnd::Start,
        };

        push_constraint(
            &mut doc,
            sketch,
            ConstraintKind::Coincident {
                a: ConstraintEntity::Point(shared_on_line0.clone()),
                b: ConstraintEntity::Point(shared_on_line1.clone()),
            },
        );
        assert_points_equal(&doc, sketch, shared_on_line0.clone(), shared_on_line1.clone());

        push_constraint(
            &mut doc,
            sketch,
            ConstraintKind::Perpendicular {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
            },
        );

        assert_points_equal(&doc, sketch, shared_on_line0, shared_on_line1);
        let (du0, dv0) = line_dir(&doc.lines[0]);
        let (du1, dv1) = line_dir(&doc.lines[1]);
        let dot = du0 * du1 + dv0 * dv1;
        assert!(dot.abs() < EPS, "lines should be perpendicular, dot={dot}");
    }

    #[test]
    fn midpoint_places_point_on_line_center() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 5.0, 8.0, 6.0, 9.0));

        let mut sel = SceneSelection::default();
        click_scene_selection(
            &mut sel,
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::Start,
            }),
            false,
        );
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Midpoint,
            &sel,
        )
        .unwrap();

        assert!((doc.lines[1].x0 - 5.0).abs() < EPS);
        assert!(doc.lines[1].y0.abs() < EPS);
        assert!(matches!(
            doc.constraints[0].kind,
            ConstraintKind::Midpoint {
                point: ConstraintPoint::LineEndpoint {
                    line: 1,
                    end: LineEnd::Start,
                },
                line: ConstraintLine::Line(0),
            }
        ));
    }

    #[test]
    fn coincident_mover_prefers_line_endpoint_over_rect_corner() {
        let line = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };
        let corner = ConstraintPoint::RectCorner { rect: 0, corner: 2 };
        let (mover, anchor) = coincident_mover_and_anchor(line.clone(), corner.clone());
        assert_eq!(mover, line);
        assert_eq!(anchor, corner);
    }

    #[test]
    fn parallel_reference_prefers_rect_edge_as_fixed() {
        let rect_edge = ConstraintLine::RectEdge {
            rect: 0,
            edge: RectEdge::Left,
        };
        let line = ConstraintLine::Line(1);
        let (reference, movable) = parallel_reference_and_movable(line.clone(), rect_edge.clone());
        assert_eq!(reference, rect_edge);
        assert_eq!(movable, line);
    }

    #[test]
    fn midpoint_with_rect_edge_uses_edge_midpoint() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 4.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 1.0, 9.0, 2.0, 10.0));

        let mut sel = SceneSelection::default();
        click_scene_selection(
            &mut sel,
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::Start,
            }),
            false,
        );
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Bottom), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Midpoint,
            &sel,
        )
        .unwrap();

        assert!((doc.lines[0].x0 - 5.0).abs() < EPS);
        assert!(doc.lines[0].y0.abs() < EPS);
    }

    // --- #26/#27: FaceVertex/FaceEdge projection into a sketch open on a body's own face. ---

    /// A 20x20 box extruded 10mm up from the ground plane (base rect at z=0, top cap at z=10).
    fn doc_with_extruded_box() -> Document {
        let mut doc = Document::default();
        let base_sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(base_sketch, 0.0, 0.0, 20.0, 20.0));
        doc.shape_order.push(ShapeKind::Rect);
        doc.extrusions.push(crate::model::Extrusion {
            sketch: base_sketch,
            faces: vec![crate::model::ExtrudeFace::Rect(0)],
            distance: 10.0,
            target: None,
            expression: String::new(),
            name: None,
            deleted: false,
            edge_treatments: Vec::new(),
        });
        doc.shape_order.push(ShapeKind::Extrusion);
        doc
    }

    #[test]
    fn point_uv_projects_face_vertex_on_a_flat_perpendicular_cap_sketch() {
        // A sketch opened directly on the extrusion's top cap: the cap's own frame origin
        // sits at the profile rect's first corner, so its own boundary loop's local
        // coordinates come out as the plain rectangle corners (0,0)-(20,20).
        let mut doc = doc_with_extruded_box();
        let cap = FaceId::ExtrudeCap {
            extrusion: 0,
            profile: crate::model::ExtrudeFace::Rect(0),
            top: true,
        };
        let sketch = doc.add_sketch(cap.clone());

        let expected = [(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)];
        for (index, (eu, ev)) in expected.into_iter().enumerate() {
            let (u, v) = point_uv(
                &doc,
                sketch,
                ConstraintPoint::FaceVertex {
                    face: cap.clone(),
                    index,
                },
            )
            .unwrap();
            assert!(
                (u - eu).abs() < EPS && (v - ev).abs() < EPS,
                "cap vertex {index}: expected ({eu},{ev}), got ({u},{v})"
            );
        }

        // FaceEdge 0 runs corner 0 -> corner 1.
        let ((u0, v0), (u1, v1)) = line_uv_endpoints(
            &doc,
            sketch,
            ConstraintLine::FaceEdge { face: cap, index: 0 },
        )
        .unwrap();
        assert!((u0 - 0.0).abs() < EPS && (v0 - 0.0).abs() < EPS);
        assert!((u1 - 20.0).abs() < EPS && (v1 - 0.0).abs() < EPS);
    }

    #[test]
    fn point_uv_projects_face_vertex_on_a_side_wall_sketch() {
        // Side face 0 (from the box's bottom edge y=0, rising +Z): its own frame has
        // u along the profile edge and v along the extrusion axis, so the boundary loop
        // projects to a flat (20 x 10) rectangle in the wall's own local coordinates.
        let mut doc = doc_with_extruded_box();
        let side = FaceId::ExtrudeSide {
            extrusion: 0,
            profile: crate::model::ExtrudeFace::Rect(0),
            edge: 0,
        };
        let sketch = doc.add_sketch(side.clone());

        let expected = [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)];
        for (index, (eu, ev)) in expected.into_iter().enumerate() {
            let (u, v) = point_uv(
                &doc,
                sketch,
                ConstraintPoint::FaceVertex {
                    face: side.clone(),
                    index,
                },
            )
            .unwrap();
            assert!(
                (u - eu).abs() < EPS && (v - ev).abs() < EPS,
                "side vertex {index}: expected ({eu},{ev}), got ({u},{v})"
            );
        }

        let ((u0, v0), (u1, v1)) = line_uv_endpoints(
            &doc,
            sketch,
            ConstraintLine::FaceEdge { face: side, index: 1 },
        )
        .unwrap();
        // Edge 1 runs corner 1 -> corner 2, i.e. up the wall at u=20.
        assert!((u0 - 20.0).abs() < EPS && (v0 - 0.0).abs() < EPS);
        assert!((u1 - 20.0).abs() < EPS && (v1 - 10.0).abs() < EPS);
    }

    #[test]
    fn coincident_pins_sketch_point_to_face_vertex() {
        // #26: a sketch point can be constrained coincident to a corner of the body face
        // it's drawn on.
        let mut doc = doc_with_extruded_box();
        let cap = FaceId::ExtrudeCap {
            extrusion: 0,
            profile: crate::model::ExtrudeFace::Rect(0),
            top: true,
        };
        let sketch = doc.add_sketch(cap.clone());
        // A free-floating line whose start point will be pinned to the cap's corner 2
        // (world (20,20,10), local (20,20) in the cap's own frame).
        doc.lines
            .push(Line::from_local_endpoints(sketch, 3.0, 4.0, 8.0, 1.0));
        doc.shape_order.push(ShapeKind::Line);
        let point = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };
        push_constraint(
            &mut doc,
            sketch,
            ConstraintKind::Coincident {
                a: ConstraintEntity::Point(point.clone()),
                b: ConstraintEntity::Point(ConstraintPoint::FaceVertex { face: cap, index: 2 }),
            },
        );
        let (u, v) = point_uv(&doc, sketch, point).unwrap();
        assert!(
            (u - 20.0).abs() < EPS && (v - 20.0).abs() < EPS,
            "expected the point to land on the cap's corner 2 at (20,20), got ({u},{v})"
        );
    }

    #[test]
    fn coincident_from_viewport_selection_pins_to_face_vertex() {
        // Same as `coincident_pins_sketch_point_to_face_vertex`, but through the interactive
        // selection -> constraint-pane path (#26's picking flow) rather than pushing the
        // constraint directly, confirming `add_geometric_constraint_from_selection` also
        // resolves a `FaceVertex` selection end to end.
        let mut doc = doc_with_extruded_box();
        let cap = FaceId::ExtrudeCap {
            extrusion: 0,
            profile: crate::model::ExtrudeFace::Rect(0),
            top: true,
        };
        let sketch = doc.add_sketch(cap.clone());
        doc.lines
            .push(Line::from_local_endpoints(sketch, 3.0, 4.0, 8.0, 1.0));
        doc.shape_order.push(ShapeKind::Line);
        let point = ConstraintPoint::LineEndpoint { line: 0, end: LineEnd::Start };
        let fv = ConstraintPoint::FaceVertex { face: cap, index: 2 };
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Point(point.clone()), false);
        click_scene_selection(&mut sel, SceneElement::Point(fv), true);
        assert!(constraint_pane_rows(&sel)
            .iter()
            .any(|row| row.kind == GeometricConstraintType::Coincident && row.enabled));
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();
        let (u, v) = point_uv(&doc, sketch, point).unwrap();
        assert!((u - 20.0).abs() < EPS && (v - 20.0).abs() < EPS, "got ({u},{v})");
    }

    #[test]
    fn point_line_distance_from_face_edge_places_circle_30mm_off_the_top_edge() {
        // #27's literal example: "draw a circle 30mm away from the top edge of the object."
        let mut doc = doc_with_extruded_box();
        let cap = FaceId::ExtrudeCap {
            extrusion: 0,
            profile: crate::model::ExtrudeFace::Rect(0),
            top: true,
        };
        let sketch = doc.add_sketch(cap.clone());
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 10.0, 5.0, 2.0, 0.0));
        doc.shape_order.push(ShapeKind::Circle);
        let point = ConstraintPoint::CircleCenter(0);
        // Face edge 0 runs local (0,0) -> (20,0): the cap's own "bottom" edge in its frame.
        let line = ConstraintLine::FaceEdge { face: cap, index: 0 };
        crate::constraints::add_distance_constraint(
            &mut doc,
            sketch,
            crate::model::DistanceTarget::PointLineDistance { point: point.clone(), line, side: 1 },
            "30mm".to_string(),
        )
        .unwrap();
        let (_u, v) = point_uv(&doc, sketch, point).unwrap();
        assert!((v - 30.0).abs() < EPS, "expected the circle center 30mm off the edge (v=30), got v={v}");
    }

    #[test]
    fn face_vertex_out_of_range_index_errors() {
        let mut doc = doc_with_extruded_box();
        let cap = FaceId::ExtrudeCap {
            extrusion: 0,
            profile: crate::model::ExtrudeFace::Rect(0),
            top: true,
        };
        let sketch = doc.add_sketch(cap.clone());
        assert!(point_uv(&doc, sketch, ConstraintPoint::FaceVertex { face: cap, index: 99 }).is_err());
    }

    #[test]
    fn face_vertex_construction_plane_never_resolves() {
        // `FaceVertex`/`FaceEdge` are scoped to extrusion-backed faces (#26/#27) — a
        // construction-plane `FaceId` never has a boundary loop to draw from.
        let (doc, sketch) = sketch_doc();
        let plane = FaceId::ConstructionPlane(0);
        assert!(point_uv(
            &doc,
            sketch,
            ConstraintPoint::FaceVertex { face: plane.clone(), index: 0 }
        )
        .is_err());
        assert!(line_uv_endpoints(
            &doc,
            sketch,
            ConstraintLine::FaceEdge { face: plane, index: 0 }
        )
        .is_err());
    }
}