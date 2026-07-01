//! Construction geometry — helper objects that stay in-session but are not exported.
//!
//! Construction planes are defined by a reference face or axis/line, then an offset
//! (and optionally an angle around an axis).

use crate::face::{
    line_world_endpoints, line_world_polyline, sketch_frame,
    SketchFrame,
};
use crate::hierarchy::SceneElement;
use crate::model::{
    ConstructionPlane, ConstructionPlaneParent, ConstraintPoint, Document, FaceId, Line, LineEnd,
    PlaneAnchor, PlaneDefinition, SketchId,
};
use crate::value::{eval_length_mm, parse_length_or};
use eframe::egui;
use glam::{Quat, Vec3};
/// Shared stroke/fill colour for all construction geometry.
pub const CONSTRUCTION_RGBA: egui::Color32 = egui::Color32::from_rgb(230, 120, 40);
/// Brighter yellow fill for construction planes (semi-transparent in the viewport).
pub const PLANE_FILL_RGBA: egui::Color32 = egui::Color32::from_rgb(241, 196, 15);

/// Screen-space dash and gap lengths for construction line strokes (pixels).
pub const CONSTRUCTION_DASH_LENGTH_PX: f32 = 6.0;
pub const CONSTRUCTION_DASH_GAP_PX: f32 = 4.0;

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

pub fn reference_from_definition(def: &PlaneDefinition) -> PlaneReference {
    match &def.anchor {
        PlaneAnchor::Face {
            origin,
            normal,
            label,
        } => PlaneReference::Face {
            origin: *origin,
            normal: *normal,
            label: label.clone(),
        },
        PlaneAnchor::Axis {
            origin,
            direction,
            label,
        } => PlaneReference::Axis {
            origin: *origin,
            direction: *direction,
            label: label.clone(),
        },
    }
}

pub fn definition_from_reference(
    reference: &PlaneReference,
    offset_mm: f32,
    angle_deg: f32,
) -> PlaneDefinition {
    let anchor = match reference {
        PlaneReference::Face {
            origin,
            normal,
            label,
        } => PlaneAnchor::Face {
            origin: *origin,
            normal: *normal,
            label: label.clone(),
        },
        PlaneReference::Axis {
            origin,
            direction,
            label,
        } => PlaneAnchor::Axis {
            origin: *origin,
            direction: *direction,
            label: label.clone(),
        },
    };
    PlaneDefinition {
        anchor,
        offset_mm,
        angle_deg,
    }
}

pub fn plane_from_definition(def: &PlaneDefinition, parent: ConstructionPlaneParent) -> ConstructionPlane {
    let reference = reference_from_definition(def);
    let mut plane = resolve_plane(
        &reference,
        &def.offset_mm.to_string(),
        &def.angle_deg.to_string(),
        def.offset_mm,
        def.angle_deg,
        true,
        true,
    );
    plane.parent = parent;
    plane.definition = def.clone();
    plane
}

/// Construction-plane indices nested under sketches hosted on `root_plane`.
pub fn descendant_plane_indices(doc: &Document, root_plane: usize) -> Vec<usize> {
    let mut descendants = Vec::new();
    let mut faces = vec![FaceId::ConstructionPlane(root_plane)];
    let mut seen_faces = std::collections::HashSet::new();

    while let Some(face) = faces.pop() {
        if !seen_faces.insert(face.clone()) {
            continue;
        }
        for sketch in doc.sketches_on_face(face) {
            for (pi, plane) in doc.construction_planes.iter().enumerate() {
                if matches!(plane.parent, ConstructionPlaneParent::Sketch(s) if s == sketch) {
                    descendants.push(pi);
                    faces.push(FaceId::ConstructionPlane(pi));
                }
            }
            for (ci, circle) in doc.circles.iter().enumerate() {
                if circle.sketch == sketch {
                    faces.push(FaceId::Circle(ci));
                }
            }
        }
    }

    descendants
}

/// Faces hosted on or nested under sketches on `root_plane` (including the root plane).
pub fn descendant_faces(doc: &Document, root_plane: usize) -> Vec<FaceId> {
    let mut faces = vec![FaceId::ConstructionPlane(root_plane)];
    let mut seen_faces = std::collections::HashSet::new();
    let mut collected = Vec::new();

    while let Some(face) = faces.pop() {
        if !seen_faces.insert(face.clone()) {
            continue;
        }
        collected.push(face.clone());
        for sketch in doc.sketches_on_face(face) {
            for (pi, plane) in doc.construction_planes.iter().enumerate() {
                if matches!(plane.parent, ConstructionPlaneParent::Sketch(s) if s == sketch) {
                    faces.push(FaceId::ConstructionPlane(pi));
                }
            }
            for (ci, circle) in doc.circles.iter().enumerate() {
                if circle.sketch == sketch {
                    faces.push(FaceId::Circle(ci));
                }
            }
        }
    }

    collected
}

/// World-space preview of geometry that moves when a construction plane is edited.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaneEditDependentPreview {
    pub planes: Vec<(usize, ConstructionPlane)>,
    pub lines: Vec<(Vec3, Vec3)>,
}

/// Where dependent planes and hosted sketch geometry will land after `preview_plane` is committed.
pub fn preview_plane_edit_dependents(
    doc: &Document,
    plane_index: usize,
    preview_plane: &ConstructionPlane,
) -> Option<PlaneEditDependentPreview> {
    let old_frame = sketch_frame(doc, FaceId::ConstructionPlane(plane_index))?;
    let new_frame = SketchFrame {
        origin: preview_plane.origin,
        u_axis: preview_plane.u_axis,
        v_axis: preview_plane.v_axis,
        normal: preview_plane.normal,
    };

    let mut planes = Vec::new();
    for index in descendant_plane_indices(doc, plane_index) {
        let mut plane = doc.construction_planes[index].clone();
        transform_plane_between_frames(&old_frame, &new_frame, &mut plane);
        planes.push((index, plane));
    }

    let mut sketches = std::collections::HashSet::new();
    for face in descendant_faces(doc, plane_index) {
        for sketch in doc.sketches_on_face(face) {
            sketches.insert(sketch);
        }
    }

    let mut lines = Vec::new();
    for sketch in sketches {
        for line in &doc.lines {
            if line.sketch != sketch {
                continue;
            }
            let Some((a, b)) = line_world_endpoints(doc, line) else {
                continue;
            };
            lines.push((
                transform_point_between_frames(&old_frame, &new_frame, a),
                transform_point_between_frames(&old_frame, &new_frame, b),
            ));
        }
    }

    Some(PlaneEditDependentPreview {
        planes,
        lines,
    })
}

pub fn transform_point_between_frames(old: &SketchFrame, new: &SketchFrame, point: Vec3) -> Vec3 {
    let relative = point - old.origin;
    let along_u = relative.dot(old.u_axis);
    let along_v = relative.dot(old.v_axis);
    let along_n = relative.dot(old.normal);
    new.origin + new.u_axis * along_u + new.v_axis * along_v + new.normal * along_n
}

pub fn transform_vector_between_frames(old: &SketchFrame, new: &SketchFrame, vector: Vec3) -> Vec3 {
    let along_u = vector.dot(old.u_axis);
    let along_v = vector.dot(old.v_axis);
    let along_n = vector.dot(old.normal);
    new.u_axis * along_u + new.v_axis * along_v + new.normal * along_n
}

pub fn transform_plane_between_frames(
    old: &SketchFrame,
    new: &SketchFrame,
    plane: &mut ConstructionPlane,
) {
    plane.origin = transform_point_between_frames(old, new, plane.origin);
    plane.normal = transform_vector_between_frames(old, new, plane.normal).normalize_or_zero();
    plane.u_axis = transform_vector_between_frames(old, new, plane.u_axis).normalize_or_zero();
    plane.v_axis = transform_vector_between_frames(old, new, plane.v_axis).normalize_or_zero();
}

pub fn transform_definition_between_frames(
    old: &SketchFrame,
    new: &SketchFrame,
    definition: &mut PlaneDefinition,
) {
    match &mut definition.anchor {
        PlaneAnchor::Face { origin, normal, .. } => {
            *origin = transform_point_between_frames(old, new, *origin);
            *normal = transform_vector_between_frames(old, new, *normal).normalize_or_zero();
        }
        PlaneAnchor::Axis {
            origin,
            direction,
            ..
        } => {
            *origin = transform_point_between_frames(old, new, *origin);
            *direction = transform_vector_between_frames(old, new, *direction).normalize_or_zero();
        }
    }
}

/// Rebuild a construction plane from its definition and move descendants with it.
pub fn apply_construction_plane_edit(
    doc: &mut Document,
    plane_index: usize,
    definition: &PlaneDefinition,
    parent: ConstructionPlaneParent,
) -> Result<(), String> {
    if doc.construction_planes.get(plane_index).is_none() {
        return Err(format!("Unknown construction plane {plane_index}"));
    }

    let old_frame = sketch_frame(doc, FaceId::ConstructionPlane(plane_index))
        .ok_or_else(|| format!("Construction plane {plane_index} has no sketch frame"))?;
    let descendants = descendant_plane_indices(doc, plane_index);

    let plane = plane_from_definition(definition, parent);
    doc.construction_planes[plane_index] = plane;

    let new_frame = sketch_frame(doc, FaceId::ConstructionPlane(plane_index))
        .ok_or_else(|| format!("Construction plane {plane_index} has no sketch frame"))?;

    for index in descendants {
        let Some(child) = doc.construction_planes.get_mut(index) else {
            continue;
        };
        transform_plane_between_frames(&old_frame, &new_frame, child);
        transform_definition_between_frames(&old_frame, &new_frame, &mut child.definition);
    }

    Ok(())
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
        parent: ConstructionPlaneParent::Root,
        definition: definition_from_reference(
            &PlaneReference::Face {
                origin,
                normal: n,
                label: String::new(),
            },
            offset,
            0.0,
        ),
        name: None,
        deleted: false,
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
        parent: ConstructionPlaneParent::Root,
        definition: definition_from_reference(
            &PlaneReference::Axis {
                origin,
                direction: axis,
                label: String::new(),
            },
            offset,
            angle_deg,
        ),
        name: None,
        deleted: false,
    }
}

/// Sketch that owns geometry used as a construction-plane reference, if any.
pub fn sketch_from_pick_target(doc: &Document, kind: PickTargetKind) -> Option<SketchId> {
    match kind {
        PickTargetKind::Line(index) => doc.lines.get(index).map(|line| line.sketch),
        PickTargetKind::Circle(index) => doc.circles.get(index).map(|circle| circle.sketch),
        PickTargetKind::ConstructionPlane(index) => doc.construction_planes.get(index).and_then(|plane| {
            match plane.parent {
                ConstructionPlaneParent::Sketch(sketch) => Some(sketch),
                ConstructionPlaneParent::Root => None,
            }
        }),
        PickTargetKind::Point(point) => point_sketch(doc, point),
        PickTargetKind::PlaneEdge { .. }
        | PickTargetKind::BodyEdge { .. }
        | PickTargetKind::GlobalAxis(_)
        | PickTargetKind::Ground(_) => None,
    }
}

pub fn point_sketch(doc: &Document, point: ConstraintPoint) -> Option<SketchId> {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => doc.lines.get(line).map(|l| l.sketch),
        ConstraintPoint::CircleCenter(circle) => doc.circles.get(circle).map(|c| c.sketch),
        // A face's own vertex has no owning sketch of its own — it's referenced *from*
        // whichever sketch a constraint projects it into, not owned by one.
        ConstraintPoint::FaceVertex { .. } => None,
    }
}

/// Hierarchy parent for a new construction plane from a pick target.
pub fn parent_from_pick_target(doc: &Document, kind: PickTargetKind) -> ConstructionPlaneParent {
    sketch_from_pick_target(doc, kind)
        .map(ConstructionPlaneParent::Sketch)
        .unwrap_or(ConstructionPlaneParent::Root)
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

/// Minimum visual offset for the gizmo arrow when the live offset is near zero.
pub fn gizmo_display_offset(offset: f32) -> f32 {
    if offset.abs() < 2.0 {
        if offset == 0.0 {
            2.0
        } else {
            offset.signum() * 2.0
        }
    } else {
        offset
    }
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
    let tip = origin + n * gizmo_display_offset(offset);

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
#[derive(Clone, Debug, PartialEq)]
pub enum PickTargetKind {
    /// A sketch point (line endpoint, rect corner, or circle center).
    Point(ConstraintPoint),
    /// A standalone sketch line segment.
    Line(usize),
    /// A sketch circle (picked on its perimeter).
    Circle(usize),
    /// One edge of a construction-plane quad.
    PlaneEdge {
        a: Vec3,
        b: Vec3,
    },
    /// One feature edge of a 3D body's solid mesh (#31) — a mesh boundary or crease between
    /// two non-coplanar triangles, the same edges `ShadingMode::Wireframe` draws, extracted via
    /// `solid_mesh_unique_edges`. Works for any body (extrusion-sourced or STL/STEP-imported),
    /// since it's derived from the triangle mesh rather than an analytic profile.
    BodyEdge {
        body: usize,
        a: Vec3,
        b: Vec3,
    },
    GlobalAxis(GlobalAxis),
    ConstructionPlane(usize),
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
        draw_pick_highlight(painter, project, doc, self.kind.clone(), PICK_HOVER_RGBA);
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

    if let Some((kind, dist)) = nearest_sketch_point(screen, project, doc) {
        let origin = match &kind {
            PickTargetKind::Point(point) => {
                point_world_position(doc, point.clone()).unwrap_or(Vec3::ZERO)
            }
            _ => Vec3::ZERO,
        };
        consider(PickTarget {
            kind,
            reference: PlaneReference::Face {
                origin,
                normal: Vec3::Z,
                label: "Point".to_string(),
            },
            distance_px: dist,
            priority: 0,
        });
    }

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

    if let Some((kind, a, b, label, dist)) = nearest_body_edge(screen, project, doc) {
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

    if let Some((index, dist)) = nearest_construction_plane(screen, project, &doc.construction_planes)
    {
        let plane = &doc.construction_planes[index];
        let origin = ground_point.unwrap_or(plane.origin);
        let projected = project_point_on_plane(origin, plane);
        consider(PickTarget {
            kind: PickTargetKind::ConstructionPlane(index),
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

/// Map a viewport pick to a scene-tree selection target, when selectable.
pub fn scene_element_from_pick(kind: &PickTargetKind) -> Option<SceneElement> {
    match kind {
        PickTargetKind::Point(point) => Some(SceneElement::Point(point.clone())),
        PickTargetKind::Line(index) => Some(SceneElement::Line(*index)),
        PickTargetKind::Circle(index) => Some(SceneElement::Circle(*index)),
        _ => None,
    }
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
        PickTargetKind::Point(point) => {
            if let Some(world) = point_world_position(doc, point) {
                if let Some(sp) = project(world) {
                    painter.circle_filled(sp, 6.0, color);
                    painter.circle_stroke(sp, 6.0, egui::Stroke::new(2.0, color));
                }
            }
        }
        PickTargetKind::Line(index) => {
            if let Some(line) = doc.lines.get(index) {
                draw_line_highlight(painter, project, doc, line, color);
            }
        }
        PickTargetKind::Circle(index) => {
            if let Some(circle) = doc.circles.get(index) {
                draw_circle_highlight(painter, project, doc, circle, color);
            }
        }
        PickTargetKind::PlaneEdge { a, b } => {
            draw_segment_highlight(painter, project, a, b, color);
        }
        PickTargetKind::BodyEdge { a, b, .. } => {
            draw_segment_highlight(painter, project, a, b, color);
        }
        PickTargetKind::GlobalAxis(axis) => {
            let (a, b) = global_axis_segment(axis);
            let axis_color = axis.color().gamma_multiply(1.25);
            draw_segment_highlight(painter, project, a, b, axis_color);
        }
        PickTargetKind::ConstructionPlane(index) => {
            if let Some(plane) = doc.construction_planes.get(index) {
                draw_plane_face_highlight(painter, project, plane, color);
            }
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
    line: &Line,
    color: egui::Color32,
) {
    let Some(points) = line_world_polyline(doc, line) else {
        return;
    };
    for pair in points.windows(2) {
        if let (Some(pa), Some(pb)) = (project(pair[0]), project(pair[1])) {
            painter.line_segment([pa, pb], egui::Stroke::new(4.0, color));
        }
    }
    if let (Some(&a), Some(&b)) = (points.first(), points.last()) {
        for p in [a, b] {
            if let Some(sp) = project(p) {
                painter.circle_filled(sp, 5.0, color);
            }
        }
    }
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

/// Highlight a sketchable circle face with a filled overlay and border.
pub fn draw_circle_face_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &Document,
    circle: &crate::model::Circle,
    color: egui::Color32,
) {
    let Some(pts_world) = crate::face::circle_world_perimeter(doc, circle, 48) else {
        return;
    };
    let pts: Option<Vec<egui::Pos2>> = pts_world.iter().map(|p| project(*p)).collect();
    let Some(pts) = pts else { return };
    painter.add(egui::Shape::convex_polygon(
        pts,
        color.gamma_multiply(FACE_HOVER_FILL_MULTIPLIER),
        egui::Stroke::new(2.0, color),
    ));
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

/// Highlight an arbitrary planar face given by its world-space boundary loop.
pub fn draw_polygon_face_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    poly: &[Vec3],
    color: egui::Color32,
) {
    let pts: Option<Vec<egui::Pos2>> = poly.iter().map(|&p| project(p)).collect();
    let Some(pts) = pts else { return };
    if pts.len() < 3 {
        return;
    }
    let normal = (poly[1] - poly[0]).cross(poly[2] - poly[0]).normalize_or_zero();
    for [a, b, c] in crate::polygon::triangulate_planar(poly, normal) {
        painter.add(egui::Shape::convex_polygon(
            vec![pts[a], pts[b], pts[c]],
            color.gamma_multiply(FACE_HOVER_FILL_MULTIPLIER),
            egui::Stroke::new(2.0, color),
        ));
    }
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

pub fn point_world_position(doc: &Document, point: ConstraintPoint) -> Option<Vec3> {
    use crate::face::{circle_world_center, local_to_world, sketch_geometry_frame};
    match point {
        ConstraintPoint::LineEndpoint { line, end } => {
            let entity = doc.lines.get(line)?;
            let frame = sketch_geometry_frame(doc, entity.sketch)?;
            let (u, v) = match end {
                LineEnd::Start => (entity.x0, entity.y0),
                LineEnd::End => (entity.x1, entity.y1),
            };
            Some(local_to_world(&frame, u, v))
        }
        ConstraintPoint::CircleCenter(circle) => {
            let entity = doc.circles.get(circle)?;
            circle_world_center(doc, entity)
        }
        // Already a world-space point (#26/#27) — no sketch frame to project through.
        ConstraintPoint::FaceVertex { face, index } => {
            crate::extrude::face_boundary_loop_world(doc, &face)?.get(index).copied()
        }
    }
}

/// Nearest sketch vertex in `sketch` under the cursor, if any.
pub fn nearest_sketch_point_in_sketch(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &Document,
    sketch: SketchId,
) -> Option<(ConstraintPoint, f32)> {
    let mut best: Option<(ConstraintPoint, f32)> = None;

    let mut consider = |point: ConstraintPoint, world: Vec3| {
        if point_sketch(doc, point.clone()) != Some(sketch) {
            return;
        }
        let Some(sp) = project(world) else {
            return;
        };
        let dist = (screen - sp).length();
        if dist <= POINT_PICK_RADIUS_PX && best.as_ref().is_none_or(|(_, d)| dist < *d) {
            best = Some((point, dist));
        }
    };

    for (li, line) in doc.lines.iter().enumerate() {
        if line.deleted || line.sketch != sketch {
            continue;
        }
        let Some((a, b)) = line_world_endpoints(doc, line) else {
            continue;
        };
        consider(
            ConstraintPoint::LineEndpoint {
                line: li,
                end: LineEnd::Start,
            },
            a,
        );
        consider(
            ConstraintPoint::LineEndpoint {
                line: li,
                end: LineEnd::End,
            },
            b,
        );
    }

    for (ci, circle) in doc.circles.iter().enumerate() {
        if circle.deleted || circle.sketch != sketch {
            continue;
        }
        if let Some(center) = crate::face::circle_world_center(doc, circle) {
            consider(ConstraintPoint::CircleCenter(ci), center);
        }
    }

    // A sketch open directly on a body's own extrusion cap/side face (#26/#27) can also
    // constrain to that face's own boundary vertices. `point_sketch` can't recognize these
    // (a `FaceVertex` has no owning sketch, unlike sketch-native entities above), so they're
    // considered directly rather than through the shared `consider` closure's sketch filter.
    // Scoped to the *active sketch's own face* only, per the issue — not arbitrary other faces.
    if let Some(face) = doc.sketch_face(sketch) {
        if matches!(face, FaceId::ExtrudeCap { .. } | FaceId::ExtrudeSide { .. }) {
            if let Some(loop_) = crate::extrude::face_boundary_loop_world(doc, &face) {
                for (index, world) in loop_.into_iter().enumerate() {
                    let Some(sp) = project(world) else {
                        continue;
                    };
                    let dist = (screen - sp).length();
                    if dist <= POINT_PICK_RADIUS_PX && best.as_ref().is_none_or(|(_, d)| dist < *d) {
                        best = Some((
                            ConstraintPoint::FaceVertex {
                                face: face.clone(),
                                index,
                            },
                            dist,
                        ));
                    }
                }
            }
        }
    }

    best
}

/// Nearest line or rectangle edge in `sketch` under the cursor (not vertices).
pub fn nearest_sketch_line_in_sketch(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &Document,
    sketch: SketchId,
) -> Option<(crate::model::ConstraintLine, f32)> {
    use crate::model::ConstraintLine;
    let mut best: Option<(ConstraintLine, f32)> = None;

    let mut consider = |line: ConstraintLine, a: Vec3, b: Vec3| {
        let Some(dist) = segment_pick_distance(screen, project, a, b) else {
            return;
        };
        if best.as_ref().is_none_or(|(_, d)| dist < *d) {
            best = Some((line, dist));
        }
    };

    for (li, line) in doc.lines.iter().enumerate() {
        if line.deleted || line.sketch != sketch {
            continue;
        }
        let Some(points) = line_world_polyline(doc, line) else {
            continue;
        };
        for pair in points.windows(2) {
            consider(ConstraintLine::Line(li), pair[0], pair[1]);
        }
    }

    // Edges of the sketch's own body face (#26/#27), scoped exactly like the vertex loop in
    // `nearest_sketch_point_in_sketch` above. Vertices win over edges via the existing caller
    // precedence: callers already check `nearest_sketch_point_in_sketch` first and skip this
    // function on a hit (see e.g. `handle_vertex_drag`/`handle_line_drag` in main.rs).
    if let Some(face) = doc.sketch_face(sketch) {
        if matches!(face, FaceId::ExtrudeCap { .. } | FaceId::ExtrudeSide { .. }) {
            if let Some(loop_) = crate::extrude::face_boundary_loop_world(doc, &face) {
                let n = loop_.len();
                for index in 0..n {
                    consider(
                        ConstraintLine::FaceEdge {
                            face: face.clone(),
                            index,
                        },
                        loop_[index],
                        loop_[(index + 1) % n],
                    );
                }
            }
        }
    }

    best
}

fn nearest_sketch_point(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &Document,
) -> Option<(PickTargetKind, f32)> {
    let mut best: Option<(PickTargetKind, f32)> = None;

    let mut consider = |point: ConstraintPoint, world: Vec3| {
        let Some(sp) = project(world) else {
            return;
        };
        let dist = (screen - sp).length();
        if dist <= POINT_PICK_RADIUS_PX
            && best.as_ref().is_none_or(|(_, d)| dist < *d)
        {
            best = Some((PickTargetKind::Point(point), dist));
        }
    };

    for (li, line) in doc.lines.iter().enumerate() {
        if line.deleted {
            continue;
        }
        let Some((a, b)) = line_world_endpoints(doc, line) else {
            continue;
        };
        consider(
            ConstraintPoint::LineEndpoint {
                line: li,
                end: LineEnd::Start,
            },
            a,
        );
        consider(
            ConstraintPoint::LineEndpoint {
                line: li,
                end: LineEnd::End,
            },
            b,
        );
    }

    for (ci, circle) in doc.circles.iter().enumerate() {
        if circle.deleted {
            continue;
        }
        if let Some(center) = crate::face::circle_world_center(doc, circle) {
            consider(ConstraintPoint::CircleCenter(ci), center);
        }
    }

    best
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

    for (li, line) in doc.lines.iter().enumerate() {
        if line.deleted {
            continue;
        }
        let Some(points) = line_world_polyline(doc, line) else {
            continue;
        };
        for pair in points.windows(2) {
            consider(PickTargetKind::Line(li), pair[0], pair[1], "Line");
        }
    }

    for (ci, circle) in doc.circles.iter().enumerate() {
        if circle.deleted {
            continue;
        }
        let Some(pts) = crate::face::circle_world_perimeter(doc, circle, 32) else {
            continue;
        };
        for window in pts.windows(2) {
            consider(
                PickTargetKind::Circle(ci),
                window[0],
                window[1],
                "Circle",
            );
        }
    }

    best
}

/// Nearest feature edge of any 3D body's solid mesh (#31) — lets a construction plane be
/// referenced from any edge on any shape, not just 2D sketch geometry.
fn nearest_body_edge(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &Document,
) -> Option<(PickTargetKind, Vec3, Vec3, String, f32)> {
    let mut best: Option<(PickTargetKind, Vec3, Vec3, String, f32)> = None;

    let mut consider = |kind: PickTargetKind, a: Vec3, b: Vec3| {
        let Some(dist) = segment_pick_distance(screen, project, a, b) else {
            return;
        };
        if best.as_ref().is_none_or(|(_, _, _, _, d)| dist < *d) {
            best = Some((kind, a, b, "Body edge".to_string(), dist));
        }
    };

    for (bi, body) in doc.bodies.iter().enumerate() {
        if body.deleted {
            continue;
        }
        let Some(solid) = crate::extrude::body_solid_mesh(doc, bi) else {
            continue;
        };
        for (a, b) in crate::gpu_viewport::solid_mesh_unique_edges(&solid) {
            consider(PickTargetKind::BodyEdge { body: bi, a, b }, a, b);
        }
    }

    best
}

/// Nearest currently-treatable analytic extrusion edge (#77): the chamfer/fillet tool's own
/// picking path when no sketch is open, used instead of the generic [`nearest_body_edge`]
/// (mesh-feature-edge) picking above since it needs the structured `ExtrusionEdgeRef`, not just
/// two raw points — see `crate::extrude::treatable_edges`.
pub fn nearest_treatable_edge(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &Document,
) -> Option<(usize, crate::model::ExtrusionEdgeRef, Vec3, Vec3, f32)> {
    let mut best: Option<(usize, crate::model::ExtrusionEdgeRef, Vec3, Vec3, f32)> = None;
    for (extrusion, edge, a, b) in crate::extrude::treatable_edges(doc) {
        let Some(dist) = segment_pick_distance(screen, project, a, b) else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, _, _, _, d)| dist < *d) {
            best = Some((extrusion, edge, a, b, dist));
        }
    }
    best
}

fn draw_circle_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &Document,
    circle: &crate::model::Circle,
    color: egui::Color32,
) {
    let Some(pts) = crate::face::circle_world_perimeter(doc, circle, 48) else {
        return;
    };
    let screen_pts: Option<Vec<egui::Pos2>> = pts.iter().map(|p| project(*p)).collect();
    if let Some(screen_pts) = screen_pts {
        if screen_pts.len() >= 2 {
            painter.add(egui::Shape::closed_line(
                screen_pts,
                egui::Stroke::new(3.0, color),
            ));
        }
    }
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

fn nearest_construction_plane(
    screen: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    planes: &[ConstructionPlane],
) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (index, plane) in planes.iter().enumerate().rev() {
        let corners = plane_corners(plane, PLANE_DISPLAY_HALF);
        let pts: Option<Vec<egui::Pos2>> = corners.iter().map(|&c| project(c)).collect();
        let Some(pts) = pts else { continue };
        let quad = [pts[0], pts[1], pts[2], pts[3]];
        let dist = if point_in_screen_quad(screen, quad) {
            0.0
        } else {
            dist_point_to_quad_edges(screen, quad)
        };
        if dist <= FACE_PICK_MARGIN_PX {
            if best.as_ref().is_none_or(|(_, d)| dist < *d) {
                best = Some((index, dist));
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

/// Drop a rectangle as four plain `Line`s forming a closed loop (bottom → right → top →
/// left), joined at their shared corners by `Coincident` constraints, with `Horizontal`
/// constraints on the two horizontal edges and `Vertical` on the two vertical edges — so
/// the loop stays a rectangle under solving. This is the geometry a rectangle *is* now
/// (SPEC §5.3): the four lines are auto-recognised as a `Polygon` face (#66). Corner `i`
/// is the shared endpoint of `lines[i-1].End`/`lines[i].Start` (wrapping): corners
/// 0=BL, 1=BR, 2=TR, 3=TL; edges bottom, right, top, left.
///
/// Returns the four line indices in edge order. Does **not** add width/height dimensions or
/// solve — callers add `DistanceTarget::LineLength` dims and solve as needed.
pub fn add_line_rectangle(
    doc: &mut Document,
    sketch: SketchId,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    construction_edges: [bool; 4],
) -> [usize; 4] {
    use crate::model::{
        Constraint, ConstraintEntity, ConstraintKind, ConstraintLine, ShapeKind,
    };
    let corners = [
        (x, y),
        (x + w, y),
        (x + w, y + h),
        (x, y + h),
    ];
    let base = doc.lines.len();
    for i in 0..4 {
        let (u0, v0) = corners[i];
        let (u1, v1) = corners[(i + 1) % 4];
        let mut line = Line::from_local_endpoints(sketch, u0, v0, u1, v1);
        line.construction = construction_edges[i];
        doc.lines.push(line);
        doc.shape_order.push(ShapeKind::Line);
    }
    let idx = [base, base + 1, base + 2, base + 3];
    let mut push = |kind: ConstraintKind| {
        doc.constraints.push(Constraint {
            sketch,
            kind,
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        doc.shape_order.push(ShapeKind::Constraint);
    };
    // Coincident: each line's End meets the next line's Start, closing the loop.
    for i in 0..4 {
        push(ConstraintKind::Coincident {
            a: ConstraintEntity::Point(ConstraintPoint::LineEndpoint {
                line: idx[i],
                end: LineEnd::End,
            }),
            b: ConstraintEntity::Point(ConstraintPoint::LineEndpoint {
                line: idx[(i + 1) % 4],
                end: LineEnd::Start,
            }),
        });
    }
    // Horizontal on bottom (0) & top (2); Vertical on right (1) & left (3).
    push(ConstraintKind::Horizontal { line: ConstraintLine::Line(idx[0]) });
    push(ConstraintKind::Horizontal { line: ConstraintLine::Line(idx[2]) });
    push(ConstraintKind::Vertical { line: ConstraintLine::Line(idx[1]) });
    push(ConstraintKind::Vertical { line: ConstraintLine::Line(idx[3]) });
    idx
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

    fn doc_with_plane_sketch() -> (Document, usize) {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        (doc, sketch)
    }

    #[test]
    fn parent_from_line_pick_is_owning_sketch() {
        let (mut doc, sketch) = doc_with_plane_sketch();
        doc.lines = vec![Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0)];
        assert_eq!(
            parent_from_pick_target(&doc, PickTargetKind::Line(0)),
            ConstructionPlaneParent::Sketch(sketch)
        );
    }

    #[test]
    fn parent_from_ground_pick_is_root() {
        let doc = Document::default();
        assert_eq!(
            parent_from_pick_target(&doc, PickTargetKind::Ground(Vec3::ZERO)),
            ConstructionPlaneParent::Root
        );
    }

    #[test]
    fn pick_reference_prefers_line_over_ground() {
        let (mut doc, sketch) = doc_with_plane_sketch();
        doc.lines = vec![Line::from_local_endpoints(sketch, 0.0, 0.0, 100.0, 0.0)];
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let reference = resolve_pick_target(Pos2::new(50.0, 2.0), &project, Some(Vec3::ZERO), &doc)
            .map(|t| t.reference);
        assert!(matches!(reference, Some(PlaneReference::Axis { .. })));
    }

    #[test]
    fn nearest_treatable_edge_ignores_circle_profiles() {
        use crate::actions::{Action, AppState, Tool};
        use crate::model::{Circle, ExtrudeFace, FaceId};

        let mut state = AppState::default();
        state.apply(Action::BeginSketch { face: FaceId::ConstructionPlane(0), viewport: None });
        let sketch = state.sketch_session.unwrap().sketch;
        state.doc.circles.push(Circle::from_local_center_radius(sketch, 0.0, 0.0, 5.0, 0.0));
        state.doc.shape_order.push(crate::model::ShapeKind::Circle);
        state.apply(Action::SetTool(Tool::Extrude));
        state.apply(Action::ToggleExtrudeFace { face: ExtrudeFace::Circle(0) });
        state.apply(Action::SetExtrudeDistance { distance: 6.0 });
        state.apply(Action::CommitExtrusion);

        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        assert!(nearest_treatable_edge(Pos2::new(5.0, 0.0), &project, &state.doc).is_none());
    }

    #[test]
    fn line_picked_within_proximity_threshold() {
        let (mut doc, sketch) = doc_with_plane_sketch();
        doc.lines = vec![Line::from_local_endpoints(sketch, 0.0, 0.0, 100.0, 0.0)];
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(Pos2::new(50.0, 8.0), &project, None, &doc);
        assert!(matches!(
            target.map(|t| t.kind),
            Some(PickTargetKind::Line(_))
        ));
    }

    #[test]
    fn line_endpoint_picked_within_point_threshold() {
        let (mut doc, sketch) = doc_with_plane_sketch();
        doc.lines = vec![Line::from_local_endpoints(sketch, 100.0, 50.0, 200.0, 50.0)];
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let target = resolve_pick_target(Pos2::new(100.0, 59.0), &project, None, &doc);
        assert!(matches!(
            target.map(|t| t.kind),
            Some(PickTargetKind::Point(ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::Start,
            }))
        ));
    }

    #[test]
    fn axis_normal_at_zero_angle_is_perpendicular_to_axis() {
        let normal = axis_normal(Vec3::X, 0.0);
        assert!(normal.dot(Vec3::X).abs() < 1e-4);
        assert!(normal.length() > 0.9);
    }

    #[test]
    fn gizmo_display_offset_never_collapses_to_zero() {
        assert!((gizmo_display_offset(0.0) - 2.0).abs() < 1e-4);
        assert!((gizmo_display_offset(0.5) - 2.0).abs() < 1e-4);
        assert!((gizmo_display_offset(-0.5) + 2.0).abs() < 1e-4);
        assert!((gizmo_display_offset(12.0) - 12.0).abs() < 1e-4);
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
        let reference = resolve_pick_target(
            Pos2::new(80.0, 80.0),
            &project,
            Some(Vec3::new(80.0, 80.0, 0.0)),
            &doc,
        )
        .map(|t| t.reference);
        assert!(matches!(
            reference,
            Some(PlaneReference::Face { label, .. }) if label == "Ground"
        ));
    }

    #[test]
    fn edit_plane_offset_moves_descendant_planes() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        let child = plane_from_definition(
            &definition_from_reference(
                &PlaneReference::Face {
                    origin: Vec3::ZERO,
                    normal: Vec3::Z,
                    label: "Ground".to_string(),
                },
                5.0,
                0.0,
            ),
            ConstructionPlaneParent::Sketch(sketch),
        );
        doc.construction_planes.push(child);
        let child_origin_before = doc.construction_planes[1].origin.z;

        let definition = definition_from_reference(
            &PlaneReference::Face {
                origin: Vec3::ZERO,
                normal: Vec3::Z,
                label: "Ground".to_string(),
            },
            15.0,
            0.0,
        );
        apply_construction_plane_edit(
            &mut doc,
            0,
            &definition,
            ConstructionPlaneParent::Root,
        )
        .unwrap();

        let child_origin_after = doc.construction_planes[1].origin.z;
        assert!((child_origin_after - child_origin_before - 15.0).abs() < 1e-3);
    }

    // ---- Rectangle-as-four-lines (#66) ----

    #[test]
    fn add_line_rectangle_drops_four_lines_and_hv_coincident_constraints() {
        use crate::model::{ConstraintKind, ConstraintLine, Document, FaceId};
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        let lines = add_line_rectangle(&mut doc, sketch, 0.0, 0.0, 10.0, 5.0, [false; 4]);
        // Four plain lines forming a closed loop (bottom, right, top, left).
        assert_eq!(doc.lines.len(), 4);
        assert_eq!(lines, [0, 1, 2, 3]);
        let horizontal = doc
            .constraints
            .iter()
            .filter(|c| matches!(c.kind, ConstraintKind::Horizontal { .. }))
            .count();
        let vertical = doc
            .constraints
            .iter()
            .filter(|c| matches!(c.kind, ConstraintKind::Vertical { .. }))
            .count();
        let coincident = doc
            .constraints
            .iter()
            .filter(|c| matches!(c.kind, ConstraintKind::Coincident { .. }))
            .count();
        assert_eq!(horizontal, 2, "bottom + top are horizontal");
        assert_eq!(vertical, 2, "left + right are vertical");
        assert_eq!(coincident, 4, "four shared corners join the loop");
        // Horizontal is on the bottom (0) and top (2) edges.
        assert!(doc.constraints.iter().any(|c| matches!(
            &c.kind,
            ConstraintKind::Horizontal { line: ConstraintLine::Line(0) }
        )));
        assert!(doc.constraints.iter().any(|c| matches!(
            &c.kind,
            ConstraintKind::Vertical { line: ConstraintLine::Line(1) }
        )));
    }

    #[test]
    fn add_line_rectangle_forms_a_recognized_polygon_face() {
        use crate::model::{Document, FaceId};
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        add_line_rectangle(&mut doc, sketch, 0.0, 0.0, 10.0, 5.0, [false; 4]);
        let loops = crate::polygon::closed_line_loops(&doc, sketch);
        assert_eq!(loops.len(), 1, "the four lines are one closed loop");
        let mut sorted = loops[0].clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2, 3]);
    }

    #[test]
    fn typed_width_height_drive_the_rectangle_under_solving() {
        use crate::constraints::{add_distance_constraint, solve_document_constraints};
        use crate::model::{DistanceTarget, Document, FaceId};
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        // Start off-size, then lock width (bottom edge) and height (right edge).
        let lines = add_line_rectangle(&mut doc, sketch, 0.0, 0.0, 3.0, 3.0, [false; 4]);
        add_distance_constraint(&mut doc, sketch, DistanceTarget::LineLength(lines[0]), "20mm".into())
            .unwrap();
        add_distance_constraint(&mut doc, sketch, DistanceTarget::LineLength(lines[1]), "8mm".into())
            .unwrap();
        solve_document_constraints(&mut doc).unwrap();
        let loop_lines = crate::polygon::closed_line_loops(&doc, sketch);
        let verts = crate::polygon::loop_vertices_uv(&doc, sketch, &loop_lines[0]).unwrap();
        let min_u = verts.iter().map(|v| v.0).fold(f32::INFINITY, f32::min);
        let max_u = verts.iter().map(|v| v.0).fold(f32::NEG_INFINITY, f32::max);
        let min_v = verts.iter().map(|v| v.1).fold(f32::INFINITY, f32::min);
        let max_v = verts.iter().map(|v| v.1).fold(f32::NEG_INFINITY, f32::max);
        assert!((max_u - min_u - 20.0).abs() < 1e-2, "width solved to 20mm");
        assert!((max_v - min_v - 8.0).abs() < 1e-2, "height solved to 8mm");
    }
}