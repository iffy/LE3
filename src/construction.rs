//! Construction geometry — helper objects that stay in-session but are not exported.
//!
//! Construction planes are defined by a reference face or axis/line, then an offset
//! (and optionally an angle around an axis).

use crate::model::{ConstructionPlane, Document, Line, Rect};
use eframe::egui;
use glam::{Quat, Vec3};
/// Shared stroke/fill colour for all construction geometry.
pub const CONSTRUCTION_RGBA: egui::Color32 = egui::Color32::from_rgb(230, 120, 40);

/// Half-edge length of the visible plane quad (millimetres).
pub const PLANE_DISPLAY_HALF: f32 = 50.0;

/// Screen-space pick tolerance for lines (pixels). The pointer need not land on the stroke.
pub const LINE_PICK_RADIUS_PX: f32 = 12.0;

/// Screen-space pick tolerance for points such as line endpoints (pixels).
pub const POINT_PICK_RADIUS_PX: f32 = 12.0;

/// Extra margin when picking faces by proximity to their projected edges (pixels).
pub const FACE_PICK_MARGIN_PX: f32 = 8.0;

/// Visual highlight for a pickable target under the cursor.
pub const PICK_HOVER_RGBA: egui::Color32 = egui::Color32::from_rgb(255, 210, 90);

/// What the user picked as the plane reference on the first click.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaneReference {
    /// A planar face: offset moves the plane along `normal`.
    Face {
        origin: Vec3,
        normal: Vec3,
        label: String,
    },
    /// A line or axis: offset is perpendicular distance; `angle_deg` spins the plane around the axis.
    Axis {
        origin: Vec3,
        direction: Vec3,
        label: String,
    },
}

impl PlaneReference {
    pub fn is_axis(&self) -> bool {
        matches!(self, PlaneReference::Axis { .. })
    }

    pub fn label(&self) -> &str {
        match self {
            PlaneReference::Face { label, .. } | PlaneReference::Axis { label, .. } => label,
        }
    }
}

/// Which dimension field is focused while creating a plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaneDim {
    Offset,
    Angle,
}

impl PlaneDim {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "offset" | "o" | "d" | "distance" => Some(PlaneDim::Offset),
            "angle" | "a" | "deg" | "degrees" => Some(PlaneDim::Angle),
            _ => None,
        }
    }

}

/// Build an orthonormal (u, v) basis on a plane from its unit normal.
pub fn plane_basis(normal: Vec3) -> (Vec3, Vec3) {
    let n = normal.normalize_or_zero();
    if n.length_squared() < 1e-8 {
        return (Vec3::X, Vec3::Y);
    }
    let hint = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let u = n.cross(hint).normalize_or_zero();
    let v = n.cross(u);
    (u, v)
}

/// Offset a face reference along its normal.
pub fn plane_from_face(offset: f32, origin: Vec3, normal: Vec3) -> ConstructionPlane {
    let n = normal.normalize_or_zero();
    let (u, v) = plane_basis(n);
    ConstructionPlane {
        origin: origin + n * offset,
        normal: n,
        u_axis: u,
        v_axis: v,
    }
}

/// Build a plane from an axis reference, perpendicular distance, and rotation (degrees).
pub fn plane_from_axis(
    offset: f32,
    angle_deg: f32,
    origin: Vec3,
    direction: Vec3,
) -> ConstructionPlane {
    let axis = direction.normalize_or_zero();
    let mut perp = axis.cross(Vec3::Z);
    if perp.length_squared() < 1e-6 {
        perp = axis.cross(Vec3::X);
    }
    perp = perp.normalize_or_zero();
    let normal = Quat::from_axis_angle(axis, angle_deg.to_radians()) * perp;
    let n = normal.normalize_or_zero();
    let (u, v) = plane_basis(n);
    ConstructionPlane {
        origin: origin + n * offset,
        normal: n,
        u_axis: u,
        v_axis: v,
    }
}

/// Resolve the final plane from a reference and dimension texts (typed or live).
pub fn resolve_plane(
    reference: &PlaneReference,
    offset_text: &str,
    angle_text: &str,
    live_offset: f32,
    live_angle_deg: f32,
    user_edited_offset: bool,
    user_edited_angle: bool,
) -> ConstructionPlane {
    let offset = parse_or_live(offset_text, live_offset, user_edited_offset);
    match reference {
        PlaneReference::Face { origin, normal, .. } => plane_from_face(offset, *origin, *normal),
        PlaneReference::Axis {
            origin,
            direction,
            ..
        } => {
            let angle = parse_or_live(angle_text, live_angle_deg, user_edited_angle);
            plane_from_axis(offset, angle, *origin, *direction)
        }
    }
}

fn parse_or_live(text: &str, live: f32, user_edited: bool) -> f32 {
    if user_edited {
        text.trim().parse::<f32>().unwrap_or(live).max(0.0)
    } else {
        live.max(0.0)
    }
}

/// Corners of the visible plane quad in world space.
pub fn plane_corners(plane: &ConstructionPlane, half: f32) -> [Vec3; 4] {
    let o = plane.origin;
    let u = plane.u_axis * half;
    let v = plane.v_axis * half;
    [
        o - u - v,
        o + u - v,
        o + u + v,
        o - u + v,
    ]
}

/// Live offset for a face reference from a world-space hover point.
pub fn live_face_offset(origin: Vec3, normal: Vec3, hover: Vec3) -> f32 {
    let n = normal.normalize_or_zero();
    (hover - origin).dot(n).max(0.0)
}

/// Live offset (perpendicular distance) and angle (degrees) for an axis reference.
pub fn live_axis_dims(origin: Vec3, direction: Vec3, hover: Vec3) -> (f32, f32) {
    let axis = direction.normalize_or_zero();
    let rel = hover - origin;
    let along = rel.dot(axis);
    let radial = rel - axis * along;
    let offset = radial.length().max(0.0);
    let mut perp = axis.cross(Vec3::Z);
    if perp.length_squared() < 1e-6 {
        perp = axis.cross(Vec3::X);
    }
    perp = perp.normalize_or_zero();
    let rotated = Quat::from_axis_angle(axis, 0.0) * perp;
    let angle_rad = if offset < 1e-4 {
        0.0
    } else {
        let dir = radial.normalize_or_zero();
        let sin = axis.cross(dir).length();
        let cos = dir.dot(rotated);
        sin.atan2(cos)
    };
    (offset, angle_rad.to_degrees().rem_euclid(360.0))
}

/// Which geometry would be selected at a viewport position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickTargetKind {
    Line(Line),
    Rect(Rect),
    ConstructionPlane(ConstructionPlane),
    Ground(Vec3),
}

/// A resolved pick target with its plane reference and screen-space distance.
#[derive(Clone, Debug, PartialEq)]
pub struct PickTarget {
    pub kind: PickTargetKind,
    pub reference: PlaneReference,
    distance_px: f32,
    priority: u8,
}

impl PickTarget {
    /// Draw a hover highlight for this target.
    pub fn draw_highlight(
        &self,
        painter: &egui::Painter,
        project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    ) {
        draw_pick_highlight(painter, project, self.kind, PICK_HOVER_RGBA);
    }
}

/// Resolve the best pick target under the cursor (shared by hover and click).
pub fn resolve_pick_target(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    ground_point: Option<Vec3>,
    doc: &Document,
) -> Option<PickTarget> {
    let mut best: Option<PickTarget> = None;

    let mut consider = |candidate: PickTarget| {
        if best.as_ref().is_none_or(|b| candidate.beats(b)) {
            best = Some(candidate);
        }
    };

    if let Some((line, dist)) = nearest_line(screen, project, &doc.lines) {
        consider(PickTarget {
            kind: PickTargetKind::Line(line),
            reference: PlaneReference::Axis {
                origin: line_midpoint(line),
                direction: line_direction(line),
                label: "Line".to_string(),
            },
            distance_px: dist,
            priority: 0,
        });
    }

    if let Some((rect, dist)) = nearest_rect(screen, project, &doc.rects) {
        let origin = ground_point.unwrap_or(rect_center(rect));
        consider(PickTarget {
            kind: PickTargetKind::Rect(rect),
            reference: PlaneReference::Face {
                origin: Vec3::new(origin.x, origin.y, 0.0),
                normal: Vec3::Z,
                label: "Rectangle face".to_string(),
            },
            distance_px: dist,
            priority: 1,
        });
    }

    if let Some((plane, dist)) = nearest_construction_plane(screen, project, &doc.construction_planes)
    {
        let origin = ground_point.unwrap_or(plane.origin);
        let projected = project_point_on_plane(origin, &plane);
        consider(PickTarget {
            kind: PickTargetKind::ConstructionPlane(plane),
            reference: PlaneReference::Face {
                origin: projected,
                normal: plane.normal,
                label: "Construction plane".to_string(),
            },
            distance_px: dist,
            priority: 2,
        });
    }

    if let Some(p) = ground_point {
        consider(PickTarget {
            kind: PickTargetKind::Ground(p),
            reference: PlaneReference::Face {
                origin: p,
                normal: Vec3::Z,
                label: "Ground".to_string(),
            },
            distance_px: f32::MAX,
            priority: 3,
        });
    }

    best
}

impl PickTarget {
    fn beats(&self, other: &PickTarget) -> bool {
        if self.priority != other.priority {
            return self.priority < other.priority;
        }
        self.distance_px < other.distance_px
    }
}

/// Pick a plane/axis reference from a viewport click.
pub fn pick_reference(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    ground_point: Option<Vec3>,
    doc: &Document,
) -> Option<PlaneReference> {
    resolve_pick_target(screen, project, ground_point, doc).map(|t| t.reference)
}

/// Draw a hover highlight for a pickable target.
pub fn draw_pick_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    kind: PickTargetKind,
    color: egui::Color32,
) {
    match kind {
        PickTargetKind::Line(line) => {
            draw_line_highlight(painter, project, line, color);
        }
        PickTargetKind::Rect(rect) => {
            draw_rect_highlight(painter, project, rect, color);
        }
        PickTargetKind::ConstructionPlane(plane) => {
            draw_plane_face_highlight(painter, project, &plane, color);
        }
        PickTargetKind::Ground(p) => {
            if let Some(sp) = project(p) {
                painter.circle_stroke(sp, 8.0, egui::Stroke::new(2.0, color));
                let r = 6.0;
                painter.line_segment(
                    [sp + egui::vec2(-r, 0.0), sp + egui::vec2(r, 0.0)],
                    egui::Stroke::new(2.0, color),
                );
                painter.line_segment(
                    [sp + egui::vec2(0.0, -r), sp + egui::vec2(0.0, r)],
                    egui::Stroke::new(2.0, color),
                );
            }
        }
    }
}

fn draw_line_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    line: Line,
    color: egui::Color32,
) {
    let a = Vec3::new(line.x0, line.y0, 0.0);
    let b = Vec3::new(line.x1, line.y1, 0.0);
    if let (Some(pa), Some(pb)) = (project(a), project(b)) {
        painter.line_segment([pa, pb], egui::Stroke::new(4.0, color));
        for p in [pa, pb] {
            painter.circle_filled(p, 5.0, color);
        }
    }
}

fn draw_rect_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    rect: Rect,
    color: egui::Color32,
) {
    let corners = rect_corners_world(rect);
    let pts: Option<Vec<egui::Pos2>> = corners.iter().map(|&c| project(c)).collect();
    let Some(pts) = pts else { return };
    painter.add(egui::Shape::closed_line(
        pts,
        egui::Stroke::new(3.0, color),
    ));
    for p in corners {
        if let Some(sp) = project(p) {
            painter.circle_filled(sp, 4.0, color);
        }
    }
}

fn draw_plane_face_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    plane: &ConstructionPlane,
    color: egui::Color32,
) {
    let corners = plane_corners(plane, PLANE_DISPLAY_HALF);
    let pts: Option<Vec<egui::Pos2>> = corners.iter().map(|&c| project(c)).collect();
    let Some(pts) = pts else { return };
    painter.add(egui::Shape::closed_line(
        pts,
        egui::Stroke::new(3.5, color),
    ));
}

fn project_point_on_plane(point: Vec3, plane: &ConstructionPlane) -> Vec3 {
    let n = plane.normal;
    let dist = (point - plane.origin).dot(n);
    point - n * dist
}

fn line_midpoint(line: Line) -> Vec3 {
    Vec3::new(
        (line.x0 + line.x1) * 0.5,
        (line.y0 + line.y1) * 0.5,
        0.0,
    )
}

fn line_direction(line: Line) -> Vec3 {
    Vec3::new(line.x1 - line.x0, line.y1 - line.y0, 0.0).normalize_or_zero()
}

fn rect_center(rect: Rect) -> Vec3 {
    Vec3::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5, 0.0)
}

fn point_in_screen_quad(p: egui::Pos2, quad: [egui::Pos2; 4]) -> bool {
    // Split quad into two triangles and test barycentric inclusion.
    point_in_tri(p, quad[0], quad[1], quad[2]) || point_in_tri(p, quad[0], quad[2], quad[3])
}

fn point_in_tri(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2, c: egui::Pos2) -> bool {
    let v0 = c - a;
    let v1 = b - a;
    let v2 = p - a;
    let dot00 = v0.dot(v0);
    let dot01 = v0.dot(v1);
    let dot02 = v0.dot(v2);
    let dot11 = v1.dot(v1);
    let dot12 = v1.dot(v2);
    let denom = dot00 * dot11 - dot01 * dot01;
    if denom.abs() < 1e-8 {
        return false;
    }
    let inv = 1.0 / denom;
    let u = (dot11 * dot02 - dot01 * dot12) * inv;
    let v = (dot00 * dot12 - dot01 * dot02) * inv;
    u >= 0.0 && v >= 0.0 && (u + v) <= 1.0
}

fn dist_point_to_segment_px(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    if ab.length_sq() < 1e-4 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

fn nearest_line(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    lines: &[Line],
) -> Option<(Line, f32)> {
    let mut best: Option<(Line, f32)> = None;
    for &line in lines {
        let a = Vec3::new(line.x0, line.y0, 0.0);
        let b = Vec3::new(line.x1, line.y1, 0.0);
        let (Some(pa), Some(pb)) = (project(a), project(b)) else {
            continue;
        };
        let seg_dist = dist_point_to_segment_px(screen, pa, pb);
        let end_a = (screen - pa).length();
        let end_b = (screen - pb).length();
        let dist = seg_dist.min(end_a).min(end_b);
        let threshold = if end_a <= POINT_PICK_RADIUS_PX || end_b <= POINT_PICK_RADIUS_PX {
            POINT_PICK_RADIUS_PX
        } else {
            LINE_PICK_RADIUS_PX
        };
        if dist <= threshold {
            if best.map(|(_, d)| dist < d).unwrap_or(true) {
                best = Some((line, dist));
            }
        }
    }
    best
}

fn nearest_rect(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    rects: &[Rect],
) -> Option<(Rect, f32)> {
    let mut best: Option<(Rect, f32)> = None;
    for &rect in rects {
        let corners = rect_corners_world(rect);
        let pts: Option<Vec<egui::Pos2>> = corners.iter().map(|&c| project(c)).collect();
        let Some(pts) = pts else { continue };
        let quad = [pts[0], pts[1], pts[2], pts[3]];
        let dist = if point_in_screen_quad(screen, quad) {
            0.0
        } else {
            dist_point_to_quad_edges(screen, quad)
        };
        if dist <= FACE_PICK_MARGIN_PX {
            if best.map(|(_, d)| dist < d).unwrap_or(true) {
                best = Some((rect, dist));
            }
        }
    }
    best
}

fn nearest_construction_plane(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    planes: &[ConstructionPlane],
) -> Option<(ConstructionPlane, f32)> {
    let mut best: Option<(ConstructionPlane, f32)> = None;
    for &plane in planes.iter().rev() {
        let corners = plane_corners(&plane, PLANE_DISPLAY_HALF);
        let pts: Option<Vec<egui::Pos2>> = corners.iter().map(|&c| project(c)).collect();
        let Some(pts) = pts else { continue };
        let quad = [pts[0], pts[1], pts[2], pts[3]];
        let dist = if point_in_screen_quad(screen, quad) {
            0.0
        } else {
            dist_point_to_quad_edges(screen, quad)
        };
        if dist <= FACE_PICK_MARGIN_PX {
            if best.map(|(_, d)| dist < d).unwrap_or(true) {
                best = Some((plane, dist));
            }
        }
    }
    best
}

fn dist_point_to_quad_edges(p: egui::Pos2, quad: [egui::Pos2; 4]) -> f32 {
    let edges = [(0, 1), (1, 2), (2, 3), (3, 0)];
    edges
        .iter()
        .map(|&(i, j)| dist_point_to_segment_px(p, quad[i], quad[j]))
        .fold(f32::MAX, f32::min)
}

fn rect_corners_world(rect: Rect) -> [Vec3; 4] {
    [
        Vec3::new(rect.x, rect.y, 0.0),
        Vec3::new(rect.x + rect.w, rect.y, 0.0),
        Vec3::new(rect.x + rect.w, rect.y + rect.h, 0.0),
        Vec3::new(rect.x, rect.y + rect.h, 0.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Pos2;

    #[test]
    fn face_offset_moves_along_normal() {
        let plane = plane_from_face(10.0, Vec3::ZERO, Vec3::Z);
        assert!((plane.origin.z - 10.0).abs() < 1e-4);
        assert!((plane.normal.z - 1.0).abs() < 1e-4);
    }

    #[test]
    fn axis_offset_and_angle_produce_tilted_plane() {
        let plane = plane_from_axis(5.0, 90.0, Vec3::ZERO, Vec3::X);
        assert!(plane.normal.z.abs() > 0.9);
        assert!((plane.origin.length() - 5.0).abs() < 1e-3);
    }

    #[test]
    fn typed_offset_overrides_live_value() {
        let reference = PlaneReference::Face {
            origin: Vec3::ZERO,
            normal: Vec3::Z,
            label: "Ground".to_string(),
        };
        let plane = resolve_plane(&reference, "12.5", "", 3.0, 0.0, true, false);
        assert!((plane.origin.z - 12.5).abs() < 1e-4);
    }

    #[test]
    fn live_offset_used_when_not_user_edited() {
        let reference = PlaneReference::Face {
            origin: Vec3::ZERO,
            normal: Vec3::Z,
            label: "Ground".to_string(),
        };
        let plane = resolve_plane(&reference, "", "", 7.0, 0.0, false, false);
        assert!((plane.origin.z - 7.0).abs() < 1e-4);
    }

    #[test]
    fn live_face_offset_is_signed_distance_along_normal() {
        let offset = live_face_offset(Vec3::ZERO, Vec3::Z, Vec3::new(1.0, 2.0, 15.0));
        assert!((offset - 15.0).abs() < 1e-4);
    }

    #[test]
    fn plane_corners_are_centered_on_origin() {
        let plane = plane_from_face(0.0, Vec3::new(10.0, 20.0, 0.0), Vec3::Z);
        let corners = plane_corners(&plane, 10.0);
        let center = corners.iter().fold(Vec3::ZERO, |acc, c| acc + *c) / 4.0;
        assert!((center.x - 10.0).abs() < 1e-3);
        assert!((center.y - 20.0).abs() < 1e-3);
    }

    #[test]
    fn pick_reference_prefers_line_over_ground() {
        let doc = Document {
            lines: vec![Line::from_endpoints(0.0, 0.0, 100.0, 0.0)],
            ..Default::default()
        };
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let reference = pick_reference(Pos2::new(50.0, 2.0), &project, Some(Vec3::ZERO), &doc);
        assert!(matches!(reference, Some(PlaneReference::Axis { .. })));
    }

    #[test]
    fn line_picked_within_proximity_threshold() {
        let doc = Document {
            lines: vec![Line::from_endpoints(0.0, 0.0, 100.0, 0.0)],
            ..Default::default()
        };
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(Pos2::new(50.0, 8.0), &project, None, &doc);
        assert!(matches!(
            target.map(|t| t.kind),
            Some(PickTargetKind::Line(_))
        ));
    }

    #[test]
    fn line_endpoint_picked_within_point_threshold() {
        let doc = Document {
            lines: vec![Line::from_endpoints(0.0, 0.0, 100.0, 0.0)],
            ..Default::default()
        };
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(Pos2::new(0.0, 9.0), &project, None, &doc);
        assert!(matches!(
            target.map(|t| t.kind),
            Some(PickTargetKind::Line(_))
        ));
    }

    #[test]
    fn rect_picked_near_edge_within_margin() {
        let doc = Document {
            rects: vec![Rect {
                x: 10.0,
                y: 10.0,
                w: 40.0,
                h: 30.0,
            }],
            ..Default::default()
        };
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(Pos2::new(8.0, 25.0), &project, None, &doc);
        assert!(matches!(
            target.map(|t| t.kind),
            Some(PickTargetKind::Rect(_))
        ));
    }

    #[test]
    fn pick_reference_uses_ground_when_empty() {
        let doc = Document::default();
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let reference = pick_reference(
            Pos2::new(5.0, 5.0),
            &project,
            Some(Vec3::new(5.0, 5.0, 0.0)),
            &doc,
        );
        assert!(matches!(
            reference,
            Some(PlaneReference::Face { label, .. }) if label == "Ground"
        ));
    }
}