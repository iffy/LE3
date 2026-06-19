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
}

/// One row in the constraint-tool context pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstraintPaneRow {
    pub kind: GeometricConstraintType,
    pub enabled: bool,
    /// Role names still needed when disabled (e.g. `"line"`).
    pub missing: Vec<&'static str>,
    /// 1–9 shortcut when enabled; assigned top-to-bottom among visible rows.
    pub shortcut: Option<u8>,
}

/// Build context-pane rows for the current selection.
pub fn constraint_pane_rows(selection: &SceneSelection) -> Vec<ConstraintPaneRow> {
    let refs = selected_constraint_refs(selection);
    let mut rows = Vec::new();

    if refs.is_empty() {
        for kind in GeometricConstraintType::ALL {
            rows.push(ConstraintPaneRow {
                kind,
                enabled: false,
                missing: missing_for_kind(kind),
                shortcut: None,
            });
        }
    } else {
        for kind in GeometricConstraintType::ALL {
            if let Some((enabled, missing)) = match_kind(kind, &refs) {
                rows.push(ConstraintPaneRow {
                    kind,
                    enabled,
                    missing,
                    shortcut: None,
                });
            }
        }
    }

    let mut shortcut = 1u8;
    for row in &mut rows {
        if row.enabled {
            row.shortcut = Some(shortcut);
            shortcut = shortcut.saturating_add(1);
        }
    }
    rows
}

fn missing_for_kind(kind: GeometricConstraintType) -> Vec<&'static str> {
    match kind {
        GeometricConstraintType::Parallel | GeometricConstraintType::Perpendicular => {
            vec!["line", "line"]
        }
        GeometricConstraintType::Coincident => vec!["point", "point or line"],
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
            if refs.iter().filter(|r| matches!(r, ConstraintRef::Point(_))).count() >= 2 {
                return Some((true, Vec::new()));
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
    }
}

/// Nth enabled constraint type (1-based shortcut index).
pub fn enabled_constraint_type(rows: &[ConstraintPaneRow], shortcut: u8) -> Option<GeometricConstraintType> {
    rows.iter()
        .find(|row| row.shortcut == Some(shortcut))
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
    apply_geometric_constraints(doc)?;
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
            } else if points.len() == 1 && !lines.is_empty() {
                Ok(ConstraintKind::Coincident {
                    a: ConstraintEntity::Point(points[0]),
                    b: ConstraintEntity::Line(lines[0]),
                })
            } else {
                Err("Coincident constraint requires two points or a point and a line".to_string())
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

/// Apply all geometric constraints after distance constraints have been solved.
pub fn apply_geometric_constraints(doc: &mut Document) -> Result<(), String> {
    let constraints: Vec<ConstraintKind> = doc
        .constraints
        .iter()
        .filter(|c| !c.deleted)
        .filter_map(|c| match c.kind {
            ConstraintKind::Distance { .. } => None,
            other => Some(other),
        })
        .filter(|kind| crate::document_lifecycle::constraint_kind_applicable(doc, *kind))
        .collect();

    let mut orientation = Vec::new();
    let mut coincident = Vec::new();
    let mut midpoint = Vec::new();
    for kind in constraints {
        match kind {
            ConstraintKind::Coincident { .. } => coincident.push(kind),
            ConstraintKind::Midpoint { .. } => midpoint.push(kind),
            _ => orientation.push(kind),
        }
    }

    const MAX_PASSES: usize = 8;
    for _ in 0..MAX_PASSES {
        for kind in &orientation {
            let _ = apply_constraint_kind(doc, *kind);
        }
        for kind in &midpoint {
            let _ = apply_constraint_kind(doc, *kind);
        }
        for kind in &coincident {
            let _ = apply_constraint_kind(doc, *kind);
        }
    }
    Ok(())
}

fn apply_constraint_kind(doc: &mut Document, kind: ConstraintKind) -> Result<(), String> {
    match kind {
        ConstraintKind::Distance { .. } => Ok(()),
        ConstraintKind::Horizontal { line } => apply_horizontal(doc, line),
        ConstraintKind::Vertical { line } => apply_vertical(doc, line),
        ConstraintKind::Parallel { line_a, line_b } => apply_parallel(doc, line_a, line_b),
        ConstraintKind::Perpendicular { line_a, line_b } => {
            apply_perpendicular(doc, line_a, line_b)
        }
        ConstraintKind::Coincident { a, b } => apply_coincident(doc, a, b),
        ConstraintKind::Midpoint { point, line } => apply_midpoint(doc, point, line),
    }
}

fn apply_horizontal(doc: &mut Document, line: ConstraintLine) -> Result<(), String> {
    let ((x0, y0), (x1, _y1)) = line_uv_endpoints(doc, line)?;
    set_line_uv_endpoints(doc, line, (x0, y0), (x1, y0))
}

fn apply_vertical(doc: &mut Document, line: ConstraintLine) -> Result<(), String> {
    let ((x0, y0), (_x1, y1)) = line_uv_endpoints(doc, line)?;
    set_line_uv_endpoints(doc, line, (x0, y0), (x0, y1))
}

fn apply_parallel(doc: &mut Document, line_a: ConstraintLine, line_b: ConstraintLine) -> Result<(), String> {
    let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
    let ((ax0, ay0), (ax1, ay1)) = line_uv_endpoints(doc, reference)?;
    let ((bx0, by0), (bx1, by1)) = line_uv_endpoints(doc, movable)?;
    let du = ax1 - ax0;
    let dv = ay1 - ay0;
    let len = (du * du + dv * dv).sqrt();
    if len < 1e-6 {
        return Err("Reference line has zero length".to_string());
    }
    let bdu = bx1 - bx0;
    let bdv = by1 - by0;
    let blen = (bdu * bdu + bdv * bdv).sqrt();
    if blen < 1e-6 {
        return Err("Constrained line has zero length".to_string());
    }
    let scale = blen / len;
    set_line_uv_endpoints(
        doc,
        movable,
        (bx0, by0),
        (bx0 + du * scale, by0 + dv * scale),
    )
}

fn apply_perpendicular(
    doc: &mut Document,
    line_a: ConstraintLine,
    line_b: ConstraintLine,
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
    let bdu = bx1 - bx0;
    let bdv = by1 - by0;
    let blen = (bdu * bdu + bdv * bdv).sqrt();
    if blen < 1e-6 {
        return Err("Constrained line has zero length".to_string());
    }
    set_line_uv_endpoints(
        doc,
        movable,
        (bx0, by0),
        (bx0 + perp_u * blen, by0 + perp_v * blen),
    )
}

/// Prefer moving sketch lines over rectangle edges when one side of the pair is fixed.
fn parallel_reference_and_movable(
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

fn apply_midpoint(
    doc: &mut Document,
    point: ConstraintPoint,
    line: ConstraintLine,
) -> Result<(), String> {
    let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, line)?;
    set_point_uv(doc, point, (x0 + x1) * 0.5, (y0 + y1) * 0.5)
}

fn apply_coincident(doc: &mut Document, a: ConstraintEntity, b: ConstraintEntity) -> Result<(), String> {
    match (a, b) {
        (ConstraintEntity::Point(pa), ConstraintEntity::Point(pb)) => {
            let (mover, anchor) = coincident_mover_and_anchor(pa, pb);
            let (u, v) = point_uv(doc, anchor)?;
            set_point_uv(doc, mover, u, v)
        }
        (ConstraintEntity::Point(point), ConstraintEntity::Line(line))
        | (ConstraintEntity::Line(line), ConstraintEntity::Point(point)) => {
            let (pu, pv) = point_uv(doc, point)?;
            let ((x0, y0), (x1, y1)) = line_uv_endpoints(doc, line)?;
            let (proj_u, proj_v) = project_point_on_segment(pu, pv, x0, y0, x1, y1);
            set_point_uv(doc, point, proj_u, proj_v)
        }
        (ConstraintEntity::Line(_), ConstraintEntity::Line(_)) => {
            Err("Coincident line-line is not supported".to_string())
        }
    }
}

/// Prefer moving free line/circle points over rectangle corners, which reshape the rect.
fn coincident_mover_and_anchor(
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

fn project_point_on_segment(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> (f32, f32) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        return (x0, y0);
    }
    let t = ((px - x0) * dx + (py - y0) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    (x0 + dx * t, y0 + dy * t)
}

fn line_uv_endpoints(
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

fn set_line_uv_endpoints(
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
            let corners = [
                (entity.x, entity.y),
                (entity.x + entity.w, entity.y),
                (entity.x + entity.w, entity.y + entity.h),
                (entity.x, entity.y + entity.h),
            ];
            let mut next = corners;
            next[corner as usize] = (u, v);
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
    use crate::model::{Document, FaceId, Line, Rect, ShapeKind};
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
        apply_geometric_constraints(doc).unwrap();
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
        assert_eq!(by_kind[&GeometricConstraintType::Vertical].shortcut, Some(1));
        assert_eq!(by_kind[&GeometricConstraintType::Horizontal].shortcut, Some(2));
        assert_eq!(sole_enabled_constraint_type(&rows), None);
        let _ = doc;
    }

    #[test]
    fn line_and_point_show_only_coincident_and_midpoint() {
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
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, GeometricConstraintType::Coincident);
        assert!(rows[0].enabled);
        assert_eq!(rows[0].shortcut, Some(1));
        assert_eq!(rows[1].kind, GeometricConstraintType::Midpoint);
        assert!(rows[1].enabled);
        assert_eq!(rows[1].shortcut, Some(2));
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
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, GeometricConstraintType::Coincident);
        assert_eq!(sole_enabled_constraint_type(&rows), Some(GeometricConstraintType::Coincident));
        assert_eq!(
            enabled_constraint_type(&rows, 1),
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