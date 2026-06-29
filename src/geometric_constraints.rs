//! Geometric sketch constraints (parallel, perpendicular, coincident, midpoint, horizontal, vertical).
//!
//! The constraint tool exposes these types in the context pane; eligibility depends on the
//! current selection. Distance/dimensional constraints remain on the dimension tool.

use crate::hierarchy::SceneElement;
use crate::model::{
    Constraint, ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, Document,
    LineEnd, RectEdge, SketchId,
};
use crate::selection::SceneSelection;

/// Constraint types shown in the constraint-tool context pane (fixed display order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeometricConstraintType {
    Parallel,
    Perpendicular,
    Coincident,
    Midpoint,
    Vertical,
    Horizontal,
}

impl GeometricConstraintType {
    pub const ALL: [Self; 6] = [
        Self::Parallel,
        Self::Perpendicular,
        Self::Coincident,
        Self::Midpoint,
        Self::Vertical,
        Self::Horizontal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Parallel => "Parallel",
            Self::Perpendicular => "Perpendicular",
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        GeometricConstraintType::Parallel | GeometricConstraintType::Perpendicular => {
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
        GeometricConstraintType::Parallel | GeometricConstraintType::Perpendicular => {
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
    refs.sort_by_key(|reference| constraint_ref_sort_key(*reference));
    refs
}

pub fn scene_element_to_constraint_ref(element: SceneElement) -> Option<ConstraintRef> {
    match element {
        SceneElement::Line(index) => Some(ConstraintRef::Line(ConstraintLine::Line(index))),
        SceneElement::RectEdge(rect, edge) => Some(ConstraintRef::Line(ConstraintLine::RectEdge {
            rect,
            edge,
        })),
        SceneElement::Point(point) => Some(ConstraintRef::Point(point)),
        SceneElement::Circle(index) => Some(ConstraintRef::Circle(index)),
        _ => None,
    }
}

fn constraint_ref_sort_key(reference: ConstraintRef) -> (u8, usize, u8, u8) {
    match reference {
        ConstraintRef::Line(ConstraintLine::Line(i)) => (0, i, 0, 0),
        ConstraintRef::Line(ConstraintLine::RectEdge { rect, edge }) => {
            (1, rect, edge.index() as u8, 0)
        }
        ConstraintRef::Point(ConstraintPoint::LineEndpoint { line, end }) => {
            (2, line, end as u8, 0)
        }
        ConstraintRef::Point(ConstraintPoint::RectCorner { rect, corner }) => (3, rect, corner, 0),
        ConstraintRef::Point(ConstraintPoint::CircleCenter(i)) => (4, i, 0, 0),
        ConstraintRef::Circle(i) => (5, i, 0, 0),
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
    validate_constraint_kind(doc, sketch, constraint_kind)?;
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
            ConstraintRef::Line(line) => Some(*line),
            _ => None,
        })
        .collect();
    let points: Vec<ConstraintPoint> = refs
        .iter()
        .filter_map(|r| match r {
            ConstraintRef::Point(point) => Some(*point),
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
            line_a: lines[0],
            line_b: lines[1],
        }),
        GeometricConstraintType::Perpendicular => Ok(ConstraintKind::Perpendicular {
            line_a: lines[0],
            line_b: lines[1],
        }),
        GeometricConstraintType::Horizontal => Ok(ConstraintKind::Horizontal { line: lines[0] }),
        GeometricConstraintType::Vertical => Ok(ConstraintKind::Vertical { line: lines[0] }),
        GeometricConstraintType::Coincident => {
            if points.len() >= 2 {
                Ok(ConstraintKind::Coincident {
                    a: ConstraintEntity::Point(points[0]),
                    b: ConstraintEntity::Point(points[1]),
                })
            } else if points.len() == 1 && !circles.is_empty() {
                Ok(ConstraintKind::Coincident {
                    a: ConstraintEntity::Point(points[0]),
                    b: ConstraintEntity::Circle(circles[0]),
                })
            } else if points.len() == 1 && !lines.is_empty() {
                Ok(ConstraintKind::Coincident {
                    a: ConstraintEntity::Point(points[0]),
                    b: ConstraintEntity::Line(lines[0]),
                })
            } else {
                Err("Coincident requires two points, a point and a line, or a point and a circle"
                    .to_string())
            }
        }
        GeometricConstraintType::Midpoint => Ok(ConstraintKind::Midpoint {
            point: points[0],
            line: lines[0],
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
        | ConstraintKind::Perpendicular { line_a, line_b } => {
            validate_line_ref(doc, sketch, line_a)?;
            validate_line_ref(doc, sketch, line_b)?;
            if line_a == line_b {
                return Err("Constraint requires two different lines".to_string());
            }
            Ok(())
        }
        ConstraintKind::Horizontal { line } | ConstraintKind::Vertical { line } => {
            validate_line_ref(doc, sketch, line)
        }
        ConstraintKind::Coincident { a, b } => {
            validate_entity_ref(doc, sketch, a)?;
            validate_entity_ref(doc, sketch, b)?;
            if a == b {
                return Err("Constraint requires two different entities".to_string());
            }
            Ok(())
        }
        ConstraintKind::Midpoint { point, line } => {
            validate_point_ref(doc, sketch, point)?;
            validate_line_ref(doc, sketch, line)?;
            Ok(())
        }
        ConstraintKind::Angle {
            line_a,
            line_b,
            rotation_sign: _,
        } => {
            validate_line_ref(doc, sketch, line_a)?;
            validate_line_ref(doc, sketch, line_b)?;
            if line_a == line_b {
                return Err("Constraint requires two different lines".to_string());
            }
            if lines_are_parallel(doc, line_a, line_b) {
                return Err("Angle constraint requires non-parallel lines".to_string());
            }
            Ok(())
        }
    }
}

fn validate_line_ref(doc: &Document, sketch: SketchId, line: ConstraintLine) -> Result<(), String> {
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
        ConstraintLine::RectEdge { rect, edge: _ } => {
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

fn validate_entity_ref(
    doc: &Document,
    sketch: SketchId,
    entity: ConstraintEntity,
) -> Result<(), String> {
    match entity {
        ConstraintEntity::Line(line) => validate_line_ref(doc, sketch, line),
        ConstraintEntity::Point(point) => validate_point_ref(doc, sketch, point),
        ConstraintEntity::Circle(circle) => {
            let entity = doc
                .circles
                .get(circle)
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

fn validate_point_ref(doc: &Document, sketch: SketchId, point: ConstraintPoint) -> Result<(), String> {
    match point {
        ConstraintPoint::LineEndpoint { line, end: _ } => {
            let entity = doc
                .lines
                .get(line)
                .ok_or_else(|| format!("Line {line} not found"))?;
            if entity.sketch != sketch {
                return Err(format!("Line {line} is not in sketch {sketch}"));
            }
        }
        ConstraintPoint::RectCorner { rect, corner } => {
            if corner > 3 {
                return Err(format!("Invalid rect corner {corner}"));
            }
            let entity = doc
                .rects
                .get(rect)
                .ok_or_else(|| format!("Rectangle {rect} not found"))?;
            if entity.sketch != sketch {
                return Err(format!("Rectangle {rect} is not in sketch {sketch}"));
            }
        }
        ConstraintPoint::CircleCenter(circle) => {
            let entity = doc
                .circles
                .get(circle)
                .ok_or_else(|| format!("Circle {circle} not found"))?;
            if entity.sketch != sketch {
                return Err(format!("Circle {circle} is not in sketch {sketch}"));
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
    match (line_a, line_b) {
        (reference @ ConstraintLine::RectEdge { .. }, movable @ ConstraintLine::Line(_)) => {
            (reference, movable)
        }
        (movable @ ConstraintLine::Line(_), reference @ ConstraintLine::RectEdge { .. }) => {
            (reference, movable)
        }
        (line_a, line_b) => (line_a, line_b),
    }
}

/// Prefer moving free line/circle points over rectangle corners, which reshape the rect.
pub fn coincident_mover_and_anchor(
    a: ConstraintPoint,
    b: ConstraintPoint,
) -> (ConstraintPoint, ConstraintPoint) {
    let a_mobility = coincident_point_mobility(a);
    let b_mobility = coincident_point_mobility(b);
    if a_mobility > b_mobility {
        (a, b)
    } else if b_mobility > a_mobility {
        (b, a)
    } else {
        (b, a)
    }
}

fn coincident_point_mobility(point: ConstraintPoint) -> u8 {
    match point {
        ConstraintPoint::LineEndpoint { .. } | ConstraintPoint::CircleCenter(_) => 2,
        ConstraintPoint::RectCorner { .. } => 0,
    }
}

/// Whether two sketch lines are parallel (within tolerance).
pub fn lines_are_parallel(doc: &Document, line_a: ConstraintLine, line_b: ConstraintLine) -> bool {
    let Ok(((ax0, ay0), (ax1, ay1))) = line_uv_endpoints(doc, line_a) else {
        return false;
    };
    let Ok(((bx0, by0), (bx1, by1))) = line_uv_endpoints(doc, line_b) else {
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
pub fn line_direction_uv(doc: &Document, line: ConstraintLine) -> Option<(f32, f32)> {
    let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, line).ok()?;
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
        ConstraintLine::RectEdge { rect, edge } => {
            let entity = doc
                .rects
                .get(rect)
                .ok_or_else(|| format!("Rectangle {rect} not found"))?;
            let (u0, v0, u1, v1) = match edge {
                RectEdge::Bottom => (entity.x, entity.y, entity.x + entity.w, entity.y),
                RectEdge::Right => (
                    entity.x + entity.w,
                    entity.y,
                    entity.x + entity.w,
                    entity.y + entity.h,
                ),
                RectEdge::Top => (
                    entity.x + entity.w,
                    entity.y + entity.h,
                    entity.x,
                    entity.y + entity.h,
                ),
                RectEdge::Left => (entity.x, entity.y + entity.h, entity.x, entity.y),
            };
            Ok(((u0, v0), (u1, v1)))
        }
    }
}

/// Translate a sketch line or rectangle edge by `(du, dv)` in face-local coordinates.
pub fn translate_line(
    doc: &mut Document,
    line: ConstraintLine,
    du: f32,
    dv: f32,
) -> Result<(), String> {
    let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, line)?;
    set_line_uv_endpoints(doc, line, (x0 + du, y0 + dv), (x1 + du, y1 + dv))
}

pub fn set_line_uv_endpoints(
    doc: &mut Document,
    line: ConstraintLine,
    start: (f32, f32),
    end: (f32, f32),
) -> Result<(), String> {
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
        ConstraintLine::RectEdge { rect, edge } => {
            let entity = doc
                .rects
                .get_mut(rect)
                .ok_or_else(|| format!("Rectangle {rect} not found"))?;
            let corners = [
                (entity.x, entity.y),
                (entity.x + entity.w, entity.y),
                (entity.x + entity.w, entity.y + entity.h),
                (entity.x, entity.y + entity.h),
            ];
            let (c0, c1) = match edge {
                RectEdge::Bottom => (0, 1),
                RectEdge::Right => (1, 2),
                RectEdge::Top => (2, 3),
                RectEdge::Left => (3, 0),
            };
            let mut next = corners;
            next[c0] = start;
            next[c1] = end;
            let min_u = next.iter().map(|(x, _)| *x).fold(f32::INFINITY, f32::min);
            let max_u = next.iter().map(|(x, _)| *x).fold(f32::NEG_INFINITY, f32::max);
            let min_v = next.iter().map(|(_, y)| *y).fold(f32::INFINITY, f32::min);
            let max_v = next.iter().map(|(_, y)| *y).fold(f32::NEG_INFINITY, f32::max);
            entity.x = min_u;
            entity.y = min_v;
            entity.w = (max_u - min_u).max(1e-3);
            entity.h = (max_v - min_v).max(1e-3);
            Ok(())
        }
    }
}

pub fn point_uv(doc: &Document, point: ConstraintPoint) -> Result<(f32, f32), String> {
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
        ConstraintPoint::RectCorner { rect, corner } => {
            let entity = doc
                .rects
                .get(rect)
                .ok_or_else(|| format!("Rectangle {rect} not found"))?;
            Ok(match corner {
                0 => (entity.x, entity.y),
                1 => (entity.x + entity.w, entity.y),
                2 => (entity.x + entity.w, entity.y + entity.h),
                3 => (entity.x, entity.y + entity.h),
                _ => return Err(format!("Invalid rect corner {corner}")),
            })
        }
        ConstraintPoint::CircleCenter(circle) => {
            let entity = doc
                .circles
                .get(circle)
                .ok_or_else(|| format!("Circle {circle} not found"))?;
            Ok((entity.cx, entity.cy))
        }
    }
}

pub fn set_point_uv(doc: &mut Document, point: ConstraintPoint, u: f32, v: f32) -> Result<(), String> {
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
        ConstraintPoint::RectCorner { rect, corner } => {
            let entity = doc
                .rects
                .get_mut(rect)
                .ok_or_else(|| format!("Rectangle {rect} not found"))?;
            // A rect corner drag pivots about the diagonally opposite corner, which stays
            // fixed. The two adjacent corners are derived from the edges this corner moves,
            // so they must not anchor the new extents (doing so pins the rect and lets it
            // only grow, never shrink). Corners 0/3 are on the min-u (left) side and 1/2 on
            // the max-u (right) side; 0/1 are min-v (bottom) and 2/3 max-v (top). Clamp so
            // the dragged corner cannot cross the anchor and invert the rectangle.
            const MIN_EXTENT: f32 = 1e-3;
            let (anchor_u, anchor_v) = match corner {
                0 => (entity.x + entity.w, entity.y + entity.h),
                1 => (entity.x, entity.y + entity.h),
                2 => (entity.x, entity.y),
                3 => (entity.x + entity.w, entity.y),
                _ => return Err(format!("Invalid rect corner {corner}")),
            };
            let (min_u, max_u) = match corner {
                0 | 3 => (u.min(anchor_u - MIN_EXTENT), anchor_u),
                _ => (anchor_u, u.max(anchor_u + MIN_EXTENT)),
            };
            let (min_v, max_v) = match corner {
                0 | 1 => (v.min(anchor_v - MIN_EXTENT), anchor_v),
                _ => (anchor_v, v.max(anchor_v + MIN_EXTENT)),
            };
            entity.x = min_u;
            entity.y = min_v;
            entity.w = max_u - min_u;
            entity.h = max_v - min_v;
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

    fn assert_points_equal(doc: &Document, a: ConstraintPoint, b: ConstraintPoint) {
        let (au, av) = point_uv(doc, a).unwrap();
        let (bu, bv) = point_uv(doc, b).unwrap();
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
        click_scene_selection(&mut sel, SceneElement::Point(point), false);
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

        let (pu, pv) = point_uv(&doc, point).unwrap();
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
            SceneElement::Point(rect_corner),
            SceneElement::Point(line_start),
        );
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();

        let (corner_u, corner_v) = point_uv(&doc, rect_corner).unwrap();
        let (line_u, line_v) = point_uv(&doc, line_start).unwrap();
        assert!((corner_u - 0.0).abs() < EPS && (corner_v - 0.0).abs() < EPS);
        assert!((line_u - corner_u).abs() < EPS && (line_v - corner_v).abs() < EPS);

        let kind = doc.constraints[0].kind;
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
        click_scene_selection(&mut sel, SceneElement::Point(rect_corner), false);
        click_scene_selection(&mut sel, SceneElement::Point(line_start), true);
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Bottom), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();

        let (corner_u, corner_v) = point_uv(&doc, rect_corner).unwrap();
        let (line_u, line_v) = point_uv(&doc, line_start).unwrap();
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
        click_scene_selection(&mut sel, SceneElement::Point(free), false);
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
                a: ConstraintEntity::Point(shared_on_line0),
                b: ConstraintEntity::Point(shared_on_line1),
            },
        );
        assert_points_equal(&doc, shared_on_line0, shared_on_line1);

        push_constraint(
            &mut doc,
            sketch,
            ConstraintKind::Perpendicular {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
            },
        );

        assert_points_equal(&doc, shared_on_line0, shared_on_line1);
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
        let (mover, anchor) = coincident_mover_and_anchor(line, corner);
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
        let (reference, movable) = parallel_reference_and_movable(line, rect_edge);
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
}