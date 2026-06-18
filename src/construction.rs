//! Construction geometry — helper objects that stay in-session but are not exported.
//!
//! Construction planes are defined by a reference face or axis/line, then an offset
//! (and optionally an angle around an axis).

use crate::face::{line_world_endpoints, rect_center_world, rect_world_corners};
use crate::model::{ConstructionPlane, Document, Line, Rect};
use crate::value::{eval_length_mm, parse_length_or};
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

/// Fill strength when highlighting a whole sketchable face on hover.
pub const FACE_HOVER_FILL_MULTIPLIER: f32 = 0.38;

/// Hover accent for axis gizmo drag handles.
pub const GIZMO_HANDLE_HOVER_RGBA: egui::Color32 = egui::Color32::from_rgb(255, 230, 120);

/// Visible length of the global X/Y/Z axes from the origin (millimetres).
pub const GLOBAL_AXIS_EXTENT_MM: f32 = 200.0;

/// Radius of the angle gizmo circle around an axis reference (millimetres).
pub const AXIS_ANGLE_GIZMO_RADIUS_MM: f32 = 25.0;

/// Screen-space hit radius for axis gizmo drag handles (pixels).
pub const AXIS_GIZMO_HANDLE_HIT_RADIUS_PX: f32 = 14.0;

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
    let n = axis_normal(direction, angle_deg);
    // Anchor the in-plane basis to the reference axis so the visible plane does not
    // flip when `plane_basis` switches its world-aligned hint (the Z/X threshold).
    let u = axis;
    let v = axis.cross(n).normalize_or_zero();
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
    match reference {
        PlaneReference::Face { origin, normal, .. } => {
            let offset = parse_or_live_signed(offset_text, live_offset, user_edited_offset);
            plane_from_face(offset, *origin, *normal)
        }
        PlaneReference::Axis {
            origin,
            direction,
            ..
        } => {
            let offset = parse_or_live_signed(offset_text, live_offset, user_edited_offset);
            let angle = parse_or_live(angle_text, live_angle_deg, user_edited_angle);
            plane_from_axis(offset, angle, *origin, *direction)
        }
    }
}

fn parse_or_live(text: &str, live: f32, user_edited: bool) -> f32 {
    if user_edited {
        eval_length_mm(text)
            .or_else(|| text.trim().parse::<f32>().ok())
            .unwrap_or(live)
            .max(0.0)
    } else {
        live.max(0.0)
    }
}

fn parse_or_live_signed(text: &str, live: f32, user_edited: bool) -> f32 {
    if user_edited {
        parse_length_or(text, live)
    } else {
        live
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
#[cfg(test)]
pub fn live_face_offset(origin: Vec3, normal: Vec3, hover: Vec3) -> f32 {
    let n = normal.normalize_or_zero();
    (hover - origin).dot(n).max(0.0)
}

/// Reference perpendicular to an axis (stable when axis is nearly vertical).
pub fn axis_reference_perp(direction: Vec3) -> Vec3 {
    let axis = direction.normalize_or_zero();
    let mut perp = axis.cross(Vec3::Z);
    if perp.length_squared() < 1e-6 {
        perp = axis.cross(Vec3::X);
    }
    perp.normalize_or_zero()
}

/// Plane normal for an axis reference at the given angle (degrees around the axis).
pub fn axis_normal(direction: Vec3, angle_deg: f32) -> Vec3 {
    let axis = direction.normalize_or_zero();
    let perp = axis_reference_perp(axis);
    (Quat::from_axis_angle(axis, angle_deg.to_radians()) * perp).normalize_or_zero()
}

/// World position of the offset drag handle along a plane normal.
pub fn offset_handle(origin: Vec3, normal: Vec3, offset: f32) -> Vec3 {
    origin + normal.normalize_or_zero() * offset
}

/// World position of the offset drag handle for an axis-referenced plane.
pub fn axis_offset_handle(origin: Vec3, direction: Vec3, offset: f32, angle_deg: f32) -> Vec3 {
    offset_handle(origin, axis_normal(direction, angle_deg), offset)
}

/// World position of the angle drag handle on the gizmo circle.
pub fn axis_angle_handle(origin: Vec3, direction: Vec3, angle_deg: f32) -> Vec3 {
    origin + axis_normal(direction, angle_deg) * AXIS_ANGLE_GIZMO_RADIUS_MM
}

/// Angle (degrees) from a ray hit on the plane perpendicular to the axis through `origin`.
pub fn angle_from_axis_plane_hit(origin: Vec3, direction: Vec3, hit: Vec3) -> f32 {
    let axis = direction.normalize_or_zero();
    let rel = hit - origin;
    let radial = rel - axis * rel.dot(axis);
    if radial.length_squared() < 1e-8 {
        return 0.0;
    }
    let dir = radial.normalize_or_zero();
    let perp = axis_reference_perp(axis);
    let tangent = axis.cross(perp).normalize_or_zero();
    let cos = dir.dot(perp);
    let sin = dir.dot(tangent);
    sin.atan2(cos).to_degrees().rem_euclid(360.0)
}

/// Offset (mm) after dragging the normal arrow along its screen projection.
pub fn offset_from_normal_drag(
    origin: Vec3,
    normal: Vec3,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    start_offset: f32,
    start_screen: egui::Pos2,
    current_screen: egui::Pos2,
) -> f32 {
    let Some(p0) = project(origin) else {
        return start_offset;
    };
    let Some(p1) = project(origin + normal) else {
        return start_offset;
    };
    let screen_axis = p1 - p0;
    let len = screen_axis.length();
    if len < 1e-3 {
        return start_offset;
    }
    let delta_px = (current_screen - start_screen).dot(screen_axis) / len;
    start_offset + delta_px / len
}

/// Which axis gizmo handle is under the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisGizmoHit {
    Offset,
    Angle,
}

/// Hit-test the offset arrow handle at a screen position.
pub fn offset_gizmo_hit(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    origin: Vec3,
    normal: Vec3,
    offset: f32,
) -> bool {
    let Some(sp) = project(offset_handle(origin, normal, offset)) else {
        return false;
    };
    (screen - sp).length() <= AXIS_GIZMO_HANDLE_HIT_RADIUS_PX
}

/// Hit-test axis gizmo handles at a screen position.
pub fn axis_gizmo_hit(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    origin: Vec3,
    direction: Vec3,
    offset: f32,
    angle_deg: f32,
) -> Option<AxisGizmoHit> {
    let normal = axis_normal(direction, angle_deg);
    if offset_gizmo_hit(screen, project, origin, normal, offset) {
        return Some(AxisGizmoHit::Offset);
    }
    let angle_pos = axis_angle_handle(origin, direction, angle_deg);
    if let Some(sp) = project(angle_pos) {
        if (screen - sp).length() <= AXIS_GIZMO_HANDLE_HIT_RADIUS_PX {
            return Some(AxisGizmoHit::Angle);
        }
    }
    None
}

/// Active drag on an axis gizmo handle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisGizmoDrag {
    pub hit: AxisGizmoHit,
    pub start_offset: f32,
    pub start_angle_deg: f32,
    pub start_screen: egui::Pos2,
}

/// World coordinate axis (origin triad).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalAxis {
    X,
    Y,
    Z,
}

impl GlobalAxis {
    pub fn direction(self) -> Vec3 {
        match self {
            GlobalAxis::X => Vec3::X,
            GlobalAxis::Y => Vec3::Y,
            GlobalAxis::Z => Vec3::Z,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GlobalAxis::X => "X axis",
            GlobalAxis::Y => "Y axis",
            GlobalAxis::Z => "Z axis",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            GlobalAxis::X => egui::Color32::from_rgb(200, 70, 70),
            GlobalAxis::Y => egui::Color32::from_rgb(70, 190, 90),
            GlobalAxis::Z => egui::Color32::from_rgb(80, 140, 230),
        }
    }
}

/// Segment from the origin along a global axis (for picking and highlight).
pub fn global_axis_segment(axis: GlobalAxis) -> (Vec3, Vec3) {
    let e = GLOBAL_AXIS_EXTENT_MM;
    (Vec3::ZERO, axis.direction() * e)
}

fn draw_gizmo_handle_hover(
    painter: &egui::Painter,
    screen: egui::Pos2,
    accent: egui::Color32,
) {
    painter.circle_filled(screen, 9.0, accent.gamma_multiply(0.35));
    painter.circle_stroke(screen, 9.0, egui::Stroke::new(2.5, accent));
    painter.circle_stroke(screen, 14.0, egui::Stroke::new(1.5, accent.gamma_multiply(0.75)));
}

/// Draw the offset arrow gizmo along a plane normal.
pub fn draw_offset_gizmo(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    origin: Vec3,
    normal: Vec3,
    offset: f32,
    color: egui::Color32,
    hovered: bool,
) {
    let n = normal.normalize_or_zero();
    let display_offset = if offset.abs() < 2.0 {
        if offset == 0.0 {
            2.0
        } else {
            offset.signum() * 2.0
        }
    } else {
        offset
    };
    let tip = origin + n * display_offset;

    let offset_stroke = if hovered { 4.0 } else { 2.5 };
    let offset_color = if hovered {
        GIZMO_HANDLE_HOVER_RGBA
    } else {
        color
    };

    if let (Some(base), Some(end)) = (project(origin), project(tip)) {
        painter.line_segment([base, end], egui::Stroke::new(offset_stroke, offset_color));
        let shaft = end - base;
        if shaft.length_sq() > 1.0 {
            let dir = shaft.normalized();
            let perp = egui::vec2(-dir.y, dir.x);
            let head = 8.0;
            let wing = 4.0;
            painter.line_segment(
                [end, end - dir * head + perp * wing],
                egui::Stroke::new(offset_stroke, offset_color),
            );
            painter.line_segment(
                [end, end - dir * head - perp * wing],
                egui::Stroke::new(offset_stroke, offset_color),
            );
        }
        if hovered {
            draw_gizmo_handle_hover(painter, end, GIZMO_HANDLE_HOVER_RGBA);
        } else {
            painter.circle_filled(end, 6.0, color);
            painter.circle_stroke(end, 6.0, egui::Stroke::new(1.5, color.gamma_multiply(0.5)));
        }
    }
}

/// Draw offset arrow and angle circle handles for an axis-referenced plane.
pub fn draw_axis_plane_gizmo(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    origin: Vec3,
    direction: Vec3,
    offset: f32,
    angle_deg: f32,
    color: egui::Color32,
    hover: Option<AxisGizmoHit>,
) {
    let normal = axis_normal(direction, angle_deg);
    draw_offset_gizmo(
        painter,
        project,
        origin,
        normal,
        offset,
        color,
        hover == Some(AxisGizmoHit::Offset),
    );

    let axis = direction.normalize_or_zero();
    let perp = axis_reference_perp(axis);
    let segments = 48;
    let mut circle_pts = Vec::with_capacity(segments + 1);
    for i in 0..=segments {
        let a = i as f32 / segments as f32 * std::f32::consts::TAU;
        let dir = Quat::from_axis_angle(axis, a) * perp;
        if let Some(sp) = project(origin + dir * AXIS_ANGLE_GIZMO_RADIUS_MM) {
            circle_pts.push(sp);
        }
    }
    let angle_hovered = hover == Some(AxisGizmoHit::Angle);
    let circle_color = if angle_hovered {
        GIZMO_HANDLE_HOVER_RGBA.gamma_multiply(0.9)
    } else {
        color.gamma_multiply(0.85)
    };
    if circle_pts.len() >= 2 {
        painter.add(egui::Shape::line(
            circle_pts,
            egui::Stroke::new(if angle_hovered { 2.5 } else { 1.5 }, circle_color),
        ));
    }

    let handle = axis_angle_handle(origin, direction, angle_deg);
    let handle_dir = (handle - origin).normalize_or_zero();
    let tangent = axis.cross(handle_dir).normalize_or_zero();
    let angle_color = if angle_hovered {
        GIZMO_HANDLE_HOVER_RGBA
    } else {
        color
    };
    if let Some(sp) = project(handle) {
        if angle_hovered {
            draw_gizmo_handle_hover(painter, sp, GIZMO_HANDLE_HOVER_RGBA);
        } else {
            painter.circle_filled(sp, 6.0, color);
        }
        if let (Some(ta), Some(tb)) = (
            project(handle + tangent * 6.0),
            project(handle - tangent * 6.0),
        ) {
            let t_screen = (ta - tb).normalized();
            if t_screen.length_sq() > 1e-4 {
                let arrow = 5.0;
                let wing = 3.0;
                for sign in [-1.0f32, 1.0] {
                    let tip = sp + t_screen * sign * arrow;
                    let side = egui::vec2(-t_screen.y, t_screen.x) * wing * sign;
                    painter.line_segment(
                        [tip, tip - t_screen * sign * arrow + side],
                        egui::Stroke::new(2.0, angle_color),
                    );
                    painter.line_segment(
                        [tip, tip - t_screen * sign * arrow - side],
                        egui::Stroke::new(2.0, angle_color),
                    );
                }
            }
        }
    }
}

/// Which geometry would be selected at a viewport position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickTargetKind {
    /// A standalone sketch line segment.
    Line(Line),
    /// One edge of a rectangle (or other 2D shape).
    ShapeEdge(Line),
    /// One edge of a construction-plane quad.
    PlaneEdge {
        a: Vec3,
        b: Vec3,
    },
    GlobalAxis(GlobalAxis),
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
        doc: &Document,
    ) {
        draw_pick_highlight(painter, project, doc, self.kind, PICK_HOVER_RGBA);
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

    if let Some((kind, a, b, label, dist)) = nearest_sketch_edge(screen, project, doc) {
        consider(PickTarget {
            kind,
            reference: PlaneReference::Axis {
                origin: segment_midpoint(a, b),
                direction: segment_direction(a, b),
                label,
            },
            distance_px: dist,
            priority: 0,
        });
    }

    if let Some((axis, dist)) = nearest_global_axis(screen, project) {
        consider(PickTarget {
            kind: PickTargetKind::GlobalAxis(axis),
            reference: PlaneReference::Axis {
                origin: Vec3::ZERO,
                direction: axis.direction(),
                label: axis.label().to_string(),
            },
            distance_px: dist,
            priority: 0,
        });
    }

    if let Some((rect, dist)) = nearest_rect(screen, project, doc) {
        let origin = rect_center_world(doc, &rect)
            .or_else(|| ground_point)
            .unwrap_or(rect_center_legacy(rect));
        let normal = crate::face::sketch_frame(doc, rect.parent)
            .map(|f| f.normal)
            .unwrap_or(Vec3::Z);
        consider(PickTarget {
            kind: PickTargetKind::Rect(rect),
            reference: PlaneReference::Face {
                origin,
                normal,
                label: "Rectangle face".to_string(),
            },
            distance_px: dist,
            priority: 1,
        });
    }

    if let Some((a, b, dist)) = nearest_construction_plane_edge(screen, project, doc) {
        consider(PickTarget {
            kind: PickTargetKind::PlaneEdge { a, b },
            reference: PlaneReference::Axis {
                origin: segment_midpoint(a, b),
                direction: segment_direction(a, b),
                label: "Construction plane edge".to_string(),
            },
            distance_px: dist,
            priority: 2,
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
    doc: &Document,
    kind: PickTargetKind,
    color: egui::Color32,
) {
    match kind {
        PickTargetKind::Line(line) | PickTargetKind::ShapeEdge(line) => {
            draw_line_highlight(painter, project, doc, line, color);
        }
        PickTargetKind::PlaneEdge { a, b } => {
            draw_segment_highlight(painter, project, a, b, color);
        }
        PickTargetKind::GlobalAxis(axis) => {
            let (a, b) = global_axis_segment(axis);
            let axis_color = axis.color().gamma_multiply(1.25);
            draw_segment_highlight(painter, project, a, b, axis_color);
        }
        PickTargetKind::Rect(rect) => {
            draw_rect_highlight(painter, project, doc, rect, color);
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
    doc: &Document,
    line: Line,
    color: egui::Color32,
) {
    let Some((a, b)) = line_world_endpoints(doc, &line) else {
        return;
    };
    draw_segment_highlight(painter, project, a, b, color);
}

fn draw_segment_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    a: Vec3,
    b: Vec3,
    color: egui::Color32,
) {
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
    doc: &Document,
    rect: Rect,
    color: egui::Color32,
) {
    let corners = rect_corners_world(doc, rect);
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

/// Highlight a sketchable face quad with a filled overlay and border.
pub fn draw_quad_face_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    corners: [Vec3; 4],
    color: egui::Color32,
) {
    let pts: Option<Vec<egui::Pos2>> = corners.iter().map(|&c| project(c)).collect();
    let Some(pts) = pts else { return };
    painter.add(egui::Shape::convex_polygon(
        pts,
        color.gamma_multiply(FACE_HOVER_FILL_MULTIPLIER),
        egui::Stroke::new(2.0, color),
    ));
}

fn draw_plane_face_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    plane: &ConstructionPlane,
    color: egui::Color32,
) {
    let corners = plane_corners(plane, PLANE_DISPLAY_HALF);
    draw_quad_face_highlight(painter, project, corners, color);
}

fn project_point_on_plane(point: Vec3, plane: &ConstructionPlane) -> Vec3 {
    let n = plane.normal;
    let dist = (point - plane.origin).dot(n);
    point - n * dist
}

fn segment_midpoint(a: Vec3, b: Vec3) -> Vec3 {
    (a + b) * 0.5
}

fn segment_direction(a: Vec3, b: Vec3) -> Vec3 {
    (b - a).normalize_or_zero()
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

/// Edges of a rectangle as world-space segment pairs (bottom, right, top, left).
pub fn rect_edge_segments(doc: &Document, rect: Rect) -> [(Vec3, Vec3); 4] {
    let c = rect_corners_world(doc, rect);
    [(c[0], c[1]), (c[1], c[2]), (c[2], c[3]), (c[3], c[0])]
}

/// Edges of a construction-plane quad as world-space segment pairs.
pub fn construction_plane_edge_segments(plane: &ConstructionPlane) -> [(Vec3, Vec3); 4] {
    let c = plane_corners(plane, PLANE_DISPLAY_HALF);
    [(c[0], c[1]), (c[1], c[2]), (c[2], c[3]), (c[3], c[0])]
}

fn segment_pick_distance(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    a: Vec3,
    b: Vec3,
) -> Option<f32> {
    let (Some(pa), Some(pb)) = (project(a), project(b)) else {
        return None;
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
        Some(dist)
    } else {
        None
    }
}

fn nearest_sketch_edge(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &Document,
) -> Option<(PickTargetKind, Vec3, Vec3, String, f32)> {
    let mut best: Option<(PickTargetKind, Vec3, Vec3, String, f32)> = None;

    let mut consider = |kind: PickTargetKind, a: Vec3, b: Vec3, label: &str| {
        let Some(dist) = segment_pick_distance(screen, project, a, b) else {
            return;
        };
        if best.as_ref().is_none_or(|(_, _, _, _, d)| dist < *d) {
            best = Some((kind, a, b, label.to_string(), dist));
        }
    };

    for &line in &doc.lines {
        let Some((a, b)) = line_world_endpoints(doc, &line) else {
            continue;
        };
        consider(PickTargetKind::Line(line), a, b, "Line");
    }

    for &rect in &doc.rects {
        for (a, b) in rect_edge_segments(doc, rect) {
            let edge_line = Line::from_local_endpoints(rect.parent, a.x, a.y, b.x, b.y);
            consider(PickTargetKind::ShapeEdge(edge_line), a, b, "Rectangle edge");
        }
    }

    best
}

fn nearest_construction_plane_edge(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &Document,
) -> Option<(Vec3, Vec3, f32)> {
    let mut best: Option<(Vec3, Vec3, f32)> = None;

    for plane in &doc.construction_planes {
        for (a, b) in construction_plane_edge_segments(plane) {
            let Some(dist) = segment_pick_distance(screen, project, a, b) else {
                continue;
            };
            if best.as_ref().is_none_or(|(_, _, d)| dist < *d) {
                best = Some((a, b, dist));
            }
        }
    }

    best
}

fn nearest_global_axis(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
) -> Option<(GlobalAxis, f32)> {
    let mut best: Option<(GlobalAxis, f32)> = None;
    for axis in [GlobalAxis::X, GlobalAxis::Y, GlobalAxis::Z] {
        let (a, b) = global_axis_segment(axis);
        let Some(dist) = segment_pick_distance(screen, project, a, b) else {
            continue;
        };
        if best.map(|(_, d)| dist < d).unwrap_or(true) {
            best = Some((axis, dist));
        }
    }
    best
}

fn nearest_rect(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &Document,
) -> Option<(Rect, f32)> {
    let mut best: Option<(Rect, f32)> = None;
    for &rect in &doc.rects {
        let corners = rect_corners_world(doc, rect);
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

fn rect_corners_world(doc: &Document, rect: Rect) -> [Vec3; 4] {
    rect_world_corners(doc, &rect).unwrap_or_else(|| rect_corners_world_legacy(rect))
}

fn rect_corners_world_legacy(rect: Rect) -> [Vec3; 4] {
    [
        Vec3::new(rect.x, rect.y, 0.0),
        Vec3::new(rect.x + rect.w, rect.y, 0.0),
        Vec3::new(rect.x + rect.w, rect.y + rect.h, 0.0),
        Vec3::new(rect.x, rect.y + rect.h, 0.0),
    ]
}

fn rect_center_legacy(rect: Rect) -> Vec3 {
    Vec3::new(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5, 0.0)
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
    fn axis_plane_basis_stays_continuous_through_full_rotation() {
        let direction = Vec3::new(1.0, 0.5, 0.2);
        let axis = direction.normalize();
        let mut prev_v: Option<Vec3> = None;
        for deg in (0..=360).step_by(3) {
            let plane = plane_from_axis(0.0, deg as f32, Vec3::ZERO, direction);
            assert!(
                plane.u_axis.dot(axis).abs() > 0.99,
                "u_axis should follow the reference line at {deg}°"
            );
            if let Some(pv) = prev_v {
                assert!(
                    pv.dot(plane.v_axis).abs() > 0.99,
                    "v_axis jumped at {deg}° (dot={})",
                    pv.dot(plane.v_axis)
                );
            }
            prev_v = Some(plane.v_axis);
        }
    }

    #[test]
    fn axis_plane_basis_avoids_hint_flip_near_z_threshold() {
        // For an X-axis line, |normal.z| crosses 0.9 near 64° — the old `plane_basis`
        // hint switch caused a visible discontinuity in this range.
        let mut prev_v: Option<Vec3> = None;
        for deg in 55..=75 {
            let plane = plane_from_axis(0.0, deg as f32, Vec3::ZERO, Vec3::X);
            if let Some(pv) = prev_v {
                assert!(
                    pv.dot(plane.v_axis).abs() > 0.99,
                    "v_axis flipped at {deg}°"
                );
            }
            prev_v = Some(plane.v_axis);
        }
    }

    #[test]
    fn typed_offset_evaluates_unit_expression() {
        let reference = PlaneReference::Face {
            origin: Vec3::ZERO,
            normal: Vec3::Z,
            label: "Ground".to_string(),
        };
        let plane = resolve_plane(&reference, "1in + 2mm", "", 3.0, 0.0, true, false);
        assert!((plane.origin.z - 27.4).abs() < 1e-3);
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
    fn face_hover_fill_is_visible_but_translucent() {
        assert!(
            FACE_HOVER_FILL_MULTIPLIER > 0.2 && FACE_HOVER_FILL_MULTIPLIER < 0.6,
            "hover fill should read as a tint, not opaque or invisible"
        );
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
    fn global_x_axis_picked_near_positive_x() {
        let doc = Document::default();
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(
            Pos2::new(50.0, 2.0),
            &project,
            Some(Vec3::new(50.0, 2.0, 0.0)),
            &doc,
        )
        .unwrap();
        assert!(matches!(target.kind, PickTargetKind::GlobalAxis(GlobalAxis::X)));
        assert!(matches!(
            target.reference,
            PlaneReference::Axis { label, .. } if label == "X axis"
        ));
    }

    #[test]
    fn global_axis_beats_ground_when_near_origin_triad() {
        let doc = Document::default();
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(
            Pos2::new(3.0, 2.0),
            &project,
            Some(Vec3::new(3.0, 2.0, 0.0)),
            &doc,
        )
        .unwrap();
        assert!(matches!(target.kind, PickTargetKind::GlobalAxis(_)));
    }

    #[test]
    fn pick_reference_prefers_line_over_ground() {
        let doc = Document {
            lines: vec![Line::from_local_endpoints(crate::model::FaceId::ConstructionPlane(0),0.0, 0.0, 100.0, 0.0)],
            ..Default::default()
        };
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let reference = pick_reference(Pos2::new(50.0, 2.0), &project, Some(Vec3::ZERO), &doc);
        assert!(matches!(reference, Some(PlaneReference::Axis { .. })));
    }

    #[test]
    fn line_picked_within_proximity_threshold() {
        let doc = Document {
            lines: vec![Line::from_local_endpoints(crate::model::FaceId::ConstructionPlane(0),0.0, 0.0, 100.0, 0.0)],
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
            lines: vec![Line::from_local_endpoints(crate::model::FaceId::ConstructionPlane(0),100.0, 50.0, 200.0, 50.0)],
            ..Default::default()
        };
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(Pos2::new(100.0, 59.0), &project, None, &doc);
        assert!(matches!(
            target.map(|t| t.kind),
            Some(PickTargetKind::Line(_))
        ));
    }

    #[test]
    fn rect_edge_picked_for_axis_reference() {
        let doc = Document {
            rects: vec![Rect::from_local_corners(
                crate::model::FaceId::ConstructionPlane(0),
                10.0,
                10.0,
                50.0,
                40.0,
            )],
            ..Default::default()
        };
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(Pos2::new(50.0, 8.0), &project, None, &doc).unwrap();
        assert!(matches!(target.kind, PickTargetKind::ShapeEdge(_)));
        assert!(matches!(
            target.reference,
            PlaneReference::Axis { label, .. } if label == "Rectangle edge"
        ));
    }

    #[test]
    fn rect_edge_beats_face_when_near_boundary() {
        let doc = Document {
            rects: vec![Rect::from_local_corners(
                crate::model::FaceId::ConstructionPlane(0),
                0.0,
                0.0,
                100.0,
                100.0,
            )],
            ..Default::default()
        };
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(Pos2::new(50.0, 2.0), &project, None, &doc);
        assert!(matches!(
            target.map(|t| t.kind),
            Some(PickTargetKind::ShapeEdge(_))
        ));
    }

    #[test]
    fn standalone_line_beats_rect_edge_when_closer() {
        let doc = Document {
            lines: vec![Line::from_local_endpoints(crate::model::FaceId::ConstructionPlane(0),48.0, 0.0, 52.0, 0.0)],
            rects: vec![Rect::from_local_corners(
                crate::model::FaceId::ConstructionPlane(0),
                0.0,
                10.0,
                100.0,
                40.0,
            )],
            ..Default::default()
        };
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(Pos2::new(50.0, 1.0), &project, None, &doc);
        assert!(matches!(
            target.map(|t| t.kind),
            Some(PickTargetKind::Line(_))
        ));
    }

    #[test]
    fn rect_face_picked_from_interior() {
        let doc = Document {
            rects: vec![Rect::from_local_corners(
                crate::model::FaceId::ConstructionPlane(0),
                10.0,
                10.0,
                50.0,
                40.0,
            )],
            ..Default::default()
        };
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(Pos2::new(30.0, 25.0), &project, None, &doc);
        assert!(matches!(
            target.map(|t| t.kind),
            Some(PickTargetKind::Rect(_))
        ));
    }

    #[test]
    fn axis_normal_at_zero_angle_is_perpendicular_to_axis() {
        let normal = axis_normal(Vec3::X, 0.0);
        assert!(normal.dot(Vec3::X).abs() < 1e-4);
        assert!(normal.length() > 0.9);
    }

    #[test]
    fn offset_gizmo_hit_finds_face_offset_handle() {
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        assert!(offset_gizmo_hit(
            Pos2::new(0.0, 12.0),
            &project,
            Vec3::ZERO,
            Vec3::Z,
            12.0,
        ));
    }

    #[test]
    fn offset_from_normal_drag_moves_with_screen_motion() {
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let offset = offset_from_normal_drag(
            Vec3::ZERO,
            Vec3::Y,
            &project,
            0.0,
            Pos2::new(0.0, 0.0),
            Pos2::new(0.0, 10.0),
        );
        assert!((offset - 10.0).abs() < 1e-3);
    }

    #[test]
    fn offset_from_normal_drag_allows_negative_values() {
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let offset = offset_from_normal_drag(
            Vec3::ZERO,
            Vec3::Y,
            &project,
            5.0,
            Pos2::new(0.0, 5.0),
            Pos2::new(0.0, -5.0),
        );
        assert!((offset + 5.0).abs() < 1e-3);
    }

    #[test]
    fn axis_offset_handle_supports_negative_offset() {
        let tip = axis_offset_handle(Vec3::ZERO, Vec3::Y, -10.0, 0.0);
        assert!(tip.x < -9.0);
    }

    #[test]
    fn signed_axis_offset_resolves_for_negative_text() {
        let reference = PlaneReference::Axis {
            origin: Vec3::ZERO,
            direction: Vec3::Y,
            label: "Line".to_string(),
        };
        let plane = resolve_plane(&reference, "-8", "", 0.0, 0.0, true, false);
        assert!(plane.origin.x < -7.0);
    }

    #[test]
    fn angle_from_axis_plane_hit_round_trips_gizmo_handle() {
        for deg in [0.0, 45.0, 90.0, 135.0, 180.0] {
            let hit = axis_angle_handle(Vec3::ZERO, Vec3::Y, deg);
            let angle = angle_from_axis_plane_hit(Vec3::ZERO, Vec3::Y, hit);
            let diff = (angle - deg).abs();
            assert!(
                diff < 1.0 || (diff - 360.0).abs() < 1.0,
                "deg={deg} got={angle}"
            );
        }
    }

    #[test]
    fn axis_gizmo_hit_finds_offset_handle_near_tip() {
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let tip = axis_offset_handle(Vec3::ZERO, Vec3::X, 15.0, 0.0);
        let screen = project(tip).unwrap();
        let hit = axis_gizmo_hit(
            screen,
            &project,
            Vec3::ZERO,
            Vec3::X,
            15.0,
            0.0,
        );
        assert_eq!(hit, Some(AxisGizmoHit::Offset));
    }

    #[test]
    fn pick_reference_uses_ground_when_empty() {
        let doc = Document::default();
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let reference = pick_reference(
            Pos2::new(80.0, 80.0),
            &project,
            Some(Vec3::new(80.0, 80.0, 0.0)),
            &doc,
        );
        assert!(matches!(
            reference,
            Some(PlaneReference::Face { label, .. }) if label == "Ground"
        ));
    }
}