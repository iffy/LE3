//! Sketch faces and parent/child dependencies between faces and sketch entities.

use crate::model::{
    Circle, ConstructionPlane, ConstructionPlaneParent, Document, FaceId, Line, PlaneAnchor,
    PlaneDefinition, Rect, SketchId,
};
use glam::Vec3;

/// Local (u, v) coordinate frame of a sketchable face in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SketchFrame {
    pub origin: Vec3,
    pub u_axis: Vec3,
    pub v_axis: Vec3,
    pub normal: Vec3,
}

/// Default definition for the datum XY construction plane.
pub fn default_xy_plane_definition() -> PlaneDefinition {
    PlaneDefinition {
        anchor: PlaneAnchor::Face {
            origin: Vec3::ZERO,
            normal: Vec3::Z,
            label: "Ground".to_string(),
        },
        offset_mm: 0.0,
        angle_deg: 0.0,
    }
}

/// Default XY ground construction plane for new documents.
pub fn default_xy_plane() -> ConstructionPlane {
    ConstructionPlane {
        origin: Vec3::ZERO,
        normal: Vec3::Z,
        u_axis: Vec3::X,
        v_axis: Vec3::Y,
        parent: ConstructionPlaneParent::Root,
        definition: default_xy_plane_definition(),
        name: None,
        deleted: false,
    }
}

/// Resolve the world-space sketch frame for a face.
pub fn sketch_frame(doc: &Document, face: FaceId) -> Option<SketchFrame> {
    match face {
        FaceId::ConstructionPlane(i) => {
            let plane = doc.construction_planes.get(i)?;
            Some(SketchFrame {
                origin: plane.origin,
                u_axis: plane.u_axis,
                v_axis: plane.v_axis,
                normal: plane.normal,
            })
        }
        FaceId::Rect(i) => {
            let rect = doc.rects.get(i)?;
            let face = doc.sketch_face(rect.sketch)?;
            let parent = sketch_frame(doc, face)?;
            let origin = local_to_world(&parent, rect.x, rect.y);
            Some(SketchFrame {
                origin,
                u_axis: parent.u_axis,
                v_axis: parent.v_axis,
                normal: parent.normal,
            })
        }
        FaceId::Circle(i) => {
            let circle = doc.circles.get(i)?;
            let face = doc.sketch_face(circle.sketch)?;
            let parent = sketch_frame(doc, face)?;
            let origin = local_to_world(&parent, circle.cx, circle.cy);
            Some(SketchFrame {
                origin,
                u_axis: parent.u_axis,
                v_axis: parent.v_axis,
                normal: parent.normal,
            })
        }
        FaceId::ExtrudeCap {
            extrusion,
            profile,
            top,
        } => {
            let ext = doc.extrusions.get(extrusion)?;
            if ext.deleted || !ext.faces.contains(&profile) {
                return None;
            }
            let base = sketch_frame(doc, profile.face_id())?;
            // A top cap that meets a slanted target plane lies in that plane, so derive its
            // frame from the actual (slanted) cap polygon rather than a parallel offset.
            if top && crate::extrude::target_top_plane(doc, ext).is_some() {
                let poly = crate::extrude::cap_polygon_world(doc, extrusion, profile, true)?;
                return frame_from_polygon(&poly, base.normal);
            }
            // Otherwise the cap shares the profile's in-plane axes, shifted along the
            // extrusion normal to the base or offset end.
            let dist = if top {
                crate::extrude::effective_distance(doc, ext)
            } else {
                0.0
            };
            Some(SketchFrame {
                origin: base.origin + base.normal * dist,
                u_axis: base.u_axis,
                v_axis: base.v_axis,
                normal: base.normal,
            })
        }
        FaceId::ExtrudeSide {
            extrusion,
            profile,
            edge,
        } => {
            let quad = crate::extrude::side_quad_world(doc, extrusion, profile, edge as usize)?;
            let (poly, _) = crate::extrude::face_profile_world(doc, profile)?;
            let centroid = poly.iter().fold(Vec3::ZERO, |acc, p| acc + *p) / poly.len() as f32;
            let (a, b, a_top) = (quad[0], quad[1], quad[3]);
            let u_axis = (b - a).normalize_or_zero();
            let up = (a_top - a).normalize_or_zero();
            if u_axis.length_squared() < 1e-8 || up.length_squared() < 1e-8 {
                return None;
            }
            // Outward wall normal: perpendicular to the wall, pointing away from the solid.
            let mut normal = u_axis.cross(up).normalize_or_zero();
            if normal.length_squared() < 1e-8 {
                return None;
            }
            let edge_mid = (a + b) * 0.5;
            if normal.dot(edge_mid - centroid) < 0.0 {
                normal = -normal;
            }
            // (u, v, normal) right-handed: v = normal × u keeps u × v == normal.
            let v_axis = normal.cross(u_axis).normalize_or_zero();
            Some(SketchFrame {
                origin: a,
                u_axis,
                v_axis,
                normal,
            })
        }
    }
}

/// Build a sketch frame from a planar world-space polygon: origin at the first vertex, U along
/// the first edge, and a normal flipped to agree with `reference_normal` (so a slanted cap keeps
/// the same facing as its base). Returns `None` for degenerate polygons.
fn frame_from_polygon(poly: &[Vec3], reference_normal: Vec3) -> Option<SketchFrame> {
    if poly.len() < 3 {
        return None;
    }
    let origin = poly[0];
    let mut normal = (poly[1] - poly[0]).cross(poly[2] - poly[0]).normalize_or_zero();
    if normal.length_squared() < 1e-8 {
        return None;
    }
    if normal.dot(reference_normal) < 0.0 {
        normal = -normal;
    }
    // U along the first edge, made orthogonal to the (possibly flipped) normal.
    let mut u_axis = poly[1] - poly[0];
    u_axis = (u_axis - normal * u_axis.dot(normal)).normalize_or_zero();
    if u_axis.length_squared() < 1e-8 {
        return None;
    }
    // v = normal × u keeps (u, v, normal) right-handed with u × v == normal.
    let v_axis = normal.cross(u_axis).normalize_or_zero();
    Some(SketchFrame {
        origin,
        u_axis,
        v_axis,
        normal,
    })
}

/// Resolve the world-space frame for geometry in a sketch.
pub fn sketch_geometry_frame(doc: &Document, sketch: SketchId) -> Option<SketchFrame> {
    let face = doc.sketch_face(sketch)?;
    sketch_frame(doc, face)
}

pub fn world_to_local(frame: &SketchFrame, p: Vec3) -> (f32, f32) {
    let rel = p - frame.origin;
    (rel.dot(frame.u_axis), rel.dot(frame.v_axis))
}

pub fn local_to_world(frame: &SketchFrame, u: f32, v: f32) -> Vec3 {
    frame.origin + frame.u_axis * u + frame.v_axis * v
}

fn camera_up_from_look_at_hint(look_forward: Vec3, up_hint: Vec3) -> Vec3 {
    let mut right = look_forward.cross(up_hint);
    if right.length_squared() < 1e-8 {
        return up_hint.normalize_or_zero();
    }
    right = right.normalize();
    right.cross(look_forward).normalize_or_zero()
}

fn axis_screen_vec(axis: Vec3, look_forward: Vec3, up_hint: Vec3) -> glam::Vec2 {
    let right = look_forward.cross(up_hint).normalize_or_zero();
    if right.length_squared() < 1e-8 {
        return glam::Vec2::ZERO;
    }
    let up = right.cross(look_forward).normalize_or_zero();
    glam::Vec2::new(axis.dot(right), -axis.dot(up))
}

fn axis_screen_preserve_weight(screen: glam::Vec2) -> f32 {
    let len = screen.length();
    if len < 1e-6 {
        0.0
    } else if screen.x > 0.0 {
        // Already pointing right on screen — keep it there.
        screen.x / len
    } else if screen.y < 0.0 {
        // Already pointing up on screen (egui y-down).
        screen.y.abs() / len
    } else {
        0.0
    }
}

fn axes_match_sketch_convention(u_screen: glam::Vec2, v_screen: glam::Vec2) -> bool {
    let u_right = u_screen.x > 0.0 && u_screen.x.abs() >= u_screen.y.abs();
    let v_up = v_screen.y < 0.0 && v_screen.y.abs() >= v_screen.x.abs();
    u_right && v_up
}

fn axis_is_screen_horizontal(screen: glam::Vec2) -> bool {
    screen.x.abs() > screen.y.abs()
}

fn sketch_view_up_score(
    u_screen_before: glam::Vec2,
    v_screen_before: glam::Vec2,
    u_screen_after: glam::Vec2,
    v_screen_after: glam::Vec2,
) -> f32 {
    let use_minimal_roll =
        axis_is_screen_horizontal(u_screen_before) && axis_is_screen_horizontal(v_screen_before);
    if use_minimal_roll {
        let delta_u = u_screen_after - u_screen_before;
        let delta_v = v_screen_after - v_screen_before;
        let u_preserve = axis_screen_preserve_weight(u_screen_before);
        let v_preserve = axis_screen_preserve_weight(v_screen_before);
        let mut score = (1.0 + 3.0 * u_preserve) * delta_u.length_squared()
            + (1.0 + 3.0 * v_preserve) * delta_v.length_squared()
            - 2.0 * u_preserve * u_screen_after.dot(u_screen_before)
            - 2.0 * v_preserve * v_screen_after.dot(v_screen_before);
        if !axes_match_sketch_convention(u_screen_after, v_screen_after) {
            score += 0.2;
        }
        score
    } else if axes_match_sketch_convention(u_screen_after, v_screen_after) {
        0.0
    } else {
        1.0
    }
}

/// Camera up hint that places the sketch plane's u/v axes on the screen axes with the
/// smallest roll change from the current view.
pub fn sketch_view_up(
    view_direction: Vec3,
    frame: &SketchFrame,
    current_look_forward: Vec3,
    current_up_hint: Vec3,
) -> Vec3 {
    // `view_direction` points from the face toward the eye; `look_at_rh` uses the opposite.
    let target_look = (-view_direction).normalize_or_zero();
    let current_look = current_look_forward.normalize_or_zero();
    let current_up_hint = current_up_hint.normalize_or_zero();
    let u = frame.u_axis.normalize_or_zero();
    let v = frame.v_axis.normalize_or_zero();
    if u.length_squared() < 1e-8 || v.length_squared() < 1e-8 {
        return Vec3::Z;
    }

    let u_screen_before = axis_screen_vec(u, current_look, current_up_hint);
    let v_screen_before = axis_screen_vec(v, current_look, current_up_hint);
    let mut best_hint = v;
    let mut best_score = f32::MAX;

    // For a near-vertical face (e.g. the side wall of a solid) there is a natural
    // "up": world +Z. Orient the sketch so the ground falls to the bottom of the
    // screen rather than rolling sideways to preserve the previous view. Faces that
    // are horizontal or only mildly tilted have little in-plane vertical component,
    // so they keep the roll-preservation behavior. A vertical wall's in-plane
    // vertical component is ~1; the 0.9 cutoff admits faces within ~25° of vertical.
    let plane_normal = (-target_look).normalize_or_zero();
    let world_up_in_plane = Vec3::Z - plane_normal * Vec3::Z.dot(plane_normal);
    let prefer_world_up = world_up_in_plane.length() > 0.9;

    for hint in [u, -u, v, -v] {
        let right = target_look.cross(hint).normalize_or_zero();
        if right.length_squared() < 1e-8 {
            continue;
        }

        let cam_up = camera_up_from_look_at_hint(target_look, hint);
        let u_h = u.dot(right).abs();
        let u_v = u.dot(cam_up).abs();
        let v_h = v.dot(right).abs();
        let v_v = v.dot(cam_up).abs();
        const AXIS_EPS: f32 = 0.05;
        let u_axis_aligned = (u_h > AXIS_EPS) ^ (u_v > AXIS_EPS);
        let v_axis_aligned = (v_h > AXIS_EPS) ^ (v_v > AXIS_EPS);
        if !u_axis_aligned || !v_axis_aligned || u_h + u_v < 0.9 || v_h + v_v < 0.9 {
            continue;
        }
        if (u_h > AXIS_EPS) == (v_h > AXIS_EPS) {
            continue;
        }

        let score = if prefer_world_up {
            // Smaller is better: pick the orientation whose screen-up points most
            // toward world +Z, keeping the ground at the bottom of the view.
            -cam_up.dot(Vec3::Z)
        } else {
            let u_screen_after = axis_screen_vec(u, target_look, hint);
            let v_screen_after = axis_screen_vec(v, target_look, hint);
            sketch_view_up_score(
                u_screen_before,
                v_screen_before,
                u_screen_after,
                v_screen_after,
            )
        };
        if score < best_score {
            best_score = score;
            best_hint = hint;
        }
    }

    if best_score < f32::MAX {
        return best_hint;
    }

    let mut up = v;
    let right = target_look.cross(up).normalize_or_zero();
    if right.dot(u) < 0.0 {
        up = -up;
    }
    up
}

pub fn rect_world_corners(doc: &Document, rect: &Rect) -> Option<[Vec3; 4]> {
    let frame = sketch_geometry_frame(doc, rect.sketch)?;
    Some(rect_world_corners_in_frame(&frame, rect))
}

pub fn rect_world_corners_in_frame(frame: &SketchFrame, rect: &Rect) -> [Vec3; 4] {
    [
        local_to_world(frame, rect.x, rect.y),
        local_to_world(frame, rect.x + rect.w, rect.y),
        local_to_world(frame, rect.x + rect.w, rect.y + rect.h),
        local_to_world(frame, rect.x, rect.y + rect.h),
    ]
}

/// Rectangle corners when the sketch frame is missing (legacy XY fallback).
pub fn rect_world_corners_legacy(rect: &Rect) -> [Vec3; 4] {
    [
        Vec3::new(rect.x, rect.y, 0.0),
        Vec3::new(rect.x + rect.w, rect.y, 0.0),
        Vec3::new(rect.x + rect.w, rect.y + rect.h, 0.0),
        Vec3::new(rect.x, rect.y + rect.h, 0.0),
    ]
}

pub fn rect_world_corners_resolved(doc: &Document, rect: &Rect) -> [Vec3; 4] {
    rect_world_corners(doc, rect).unwrap_or_else(|| rect_world_corners_legacy(rect))
}

pub fn line_world_endpoints(doc: &Document, line: &Line) -> Option<(Vec3, Vec3)> {
    let frame = sketch_geometry_frame(doc, line.sketch)?;
    Some((
        local_to_world(&frame, line.x0, line.y0),
        local_to_world(&frame, line.x1, line.y1),
    ))
}

pub fn rect_center_world(doc: &Document, rect: &Rect) -> Option<Vec3> {
    let frame = sketch_geometry_frame(doc, rect.sketch)?;
    Some(local_to_world(
        &frame,
        rect.x + rect.w * 0.5,
        rect.y + rect.h * 0.5,
    ))
}

pub fn circle_world_center(doc: &Document, circle: &Circle) -> Option<Vec3> {
    let frame = sketch_geometry_frame(doc, circle.sketch)?;
    Some(local_to_world(&frame, circle.cx, circle.cy))
}

/// Rim-to-rim diameter segment through the circle center.
pub fn circle_world_diameter_endpoints(doc: &Document, circle: &Circle) -> Option<(Vec3, Vec3)> {
    let frame = sketch_geometry_frame(doc, circle.sketch)?;
    let du = circle.diameter_dim_angle.cos() * circle.r;
    let dv = circle.diameter_dim_angle.sin() * circle.r;
    Some((
        local_to_world(&frame, circle.cx - du, circle.cy - dv),
        local_to_world(&frame, circle.cx + du, circle.cy + dv),
    ))
}

/// Sampled world-space points around a circle perimeter (closed loop).
pub fn circle_world_perimeter(doc: &Document, circle: &Circle, segments: usize) -> Option<Vec<Vec3>> {
    let frame = sketch_geometry_frame(doc, circle.sketch)?;
    let segments = segments.max(8);
    let mut pts = Vec::with_capacity(segments + 1);
    for i in 0..=segments {
        let t = i as f32 / segments as f32 * std::f32::consts::TAU;
        let u = circle.cx + circle.r * t.cos();
        let v = circle.cy + circle.r * t.sin();
        pts.push(local_to_world(&frame, u, v));
    }
    Some(pts)
}

/// Axis-aligned bounds in a face's local (u, v) coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SketchZoomBounds {
    pub center_u: f32,
    pub center_v: f32,
    pub half_u: f32,
    pub half_v: f32,
}

/// Camera framing parameters when entering sketch mode on a sketch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SketchCameraTarget {
    pub target: glam::Vec3,
    /// Outward face normal; the camera picks ±this to stay on the visible side.
    pub face_normal: glam::Vec3,
    pub zoom: Option<SketchZoomBounds>,
}

impl SketchZoomBounds {
    fn from_uv_rect(u0: f32, v0: f32, u1: f32, v1: f32) -> Self {
        let u_min = u0.min(u1);
        let u_max = u0.max(u1);
        let v_min = v0.min(v1);
        let v_max = v0.max(v1);
        let half_u = ((u_max - u_min) * 0.5).max(1.0);
        let half_v = ((v_max - v_min) * 0.5).max(1.0);
        Self {
            center_u: (u_min + u_max) * 0.5,
            center_v: (v_min + v_max) * 0.5,
            half_u,
            half_v,
        }
    }

    fn union(a: Self, b: Self) -> Self {
        let u_min = (a.center_u - a.half_u).min(b.center_u - b.half_u);
        let u_max = (a.center_u + a.half_u).max(b.center_u + b.half_u);
        let v_min = (a.center_v - a.half_v).min(b.center_v - b.half_v);
        let v_max = (a.center_v + a.half_v).max(b.center_v + b.half_v);
        Self::from_uv_rect(u_min, v_min, u_max, v_max)
    }

    pub fn world_corners(&self, frame: &SketchFrame) -> [Vec3; 4] {
        [
            local_to_world(
                frame,
                self.center_u - self.half_u,
                self.center_v - self.half_v,
            ),
            local_to_world(
                frame,
                self.center_u + self.half_u,
                self.center_v - self.half_v,
            ),
            local_to_world(
                frame,
                self.center_u + self.half_u,
                self.center_v + self.half_v,
            ),
            local_to_world(
                frame,
                self.center_u - self.half_u,
                self.center_v + self.half_v,
            ),
        ]
    }
}

fn extend_sketch_bounds(bounds: &mut Option<SketchZoomBounds>, u0: f32, v0: f32, u1: f32, v1: f32) {
    let next = SketchZoomBounds::from_uv_rect(u0, v0, u1, v1);
    *bounds = Some(match bounds.take() {
        Some(existing) => SketchZoomBounds::union(existing, next),
        None => next,
    });
}

/// Axis-aligned zoom bounds for all geometry in a sketch (rects, lines, and circles).
fn sketch_local_bounds(doc: &Document, sketch: SketchId) -> Option<SketchZoomBounds> {
    let mut bounds = None;
    for rect in &doc.rects {
        if rect.sketch == sketch {
            extend_sketch_bounds(&mut bounds, rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
        }
    }
    for line in &doc.lines {
        if line.sketch == sketch {
            extend_sketch_bounds(&mut bounds, line.x0, line.y0, line.x1, line.y1);
        }
    }
    for circle in &doc.circles {
        if circle.sketch == sketch {
            extend_sketch_bounds(
                &mut bounds,
                circle.cx - circle.r,
                circle.cy - circle.r,
                circle.cx + circle.r,
                circle.cy + circle.r,
            );
        }
    }
    bounds
}

/// Resolve camera target, view direction, and optional zoom bounds for sketch mode.
pub fn sketch_camera_target(doc: &Document, sketch: SketchId) -> Option<SketchCameraTarget> {
    let face = doc.sketch_face(sketch)?;
    let frame = sketch_frame(doc, face)?;
    let face_normal = frame.normal;

    match face {
        FaceId::ConstructionPlane(_) => {
            if let Some(zoom) = sketch_local_bounds(doc, sketch) {
                let target = local_to_world(&frame, zoom.center_u, zoom.center_v);
                Some(SketchCameraTarget {
                    target,
                    face_normal,
                    zoom: Some(zoom),
                })
            } else {
                Some(SketchCameraTarget {
                    target: frame.origin,
                    face_normal,
                    zoom: None,
                })
            }
        }
        FaceId::Rect(i) => {
            let rect = doc.rects.get(i)?;
            let mut zoom = SketchZoomBounds::from_uv_rect(
                rect.x,
                rect.y,
                rect.x + rect.w,
                rect.y + rect.h,
            );
            if let Some(children) = sketch_local_bounds(doc, sketch) {
                zoom = SketchZoomBounds::union(zoom, children);
            }
            let target = local_to_world(&frame, zoom.center_u, zoom.center_v);
            Some(SketchCameraTarget {
                target,
                face_normal,
                zoom: Some(zoom),
            })
        }
        FaceId::Circle(i) => {
            let circle = doc.circles.get(i)?;
            let mut zoom = SketchZoomBounds::from_uv_rect(
                circle.cx - circle.r,
                circle.cy - circle.r,
                circle.cx + circle.r,
                circle.cy + circle.r,
            );
            if let Some(children) = sketch_local_bounds(doc, sketch) {
                zoom = SketchZoomBounds::union(zoom, children);
            }
            let target = local_to_world(&frame, zoom.center_u, zoom.center_v);
            Some(SketchCameraTarget {
                target,
                face_normal,
                zoom: Some(zoom),
            })
        }
        FaceId::ExtrudeCap {
            extrusion,
            profile,
            top,
        } => {
            let poly = crate::extrude::cap_polygon_world(doc, extrusion, profile, top)?;
            let mut zoom: Option<SketchZoomBounds> = None;
            for p in &poly {
                let (u, v) = world_to_local(&frame, *p);
                extend_sketch_bounds(&mut zoom, u, v, u, v);
            }
            if let Some(children) = sketch_local_bounds(doc, sketch) {
                zoom = Some(match zoom {
                    Some(z) => SketchZoomBounds::union(z, children),
                    None => children,
                });
            }
            let zoom = zoom?;
            let target = local_to_world(&frame, zoom.center_u, zoom.center_v);
            Some(SketchCameraTarget {
                target,
                face_normal,
                zoom: Some(zoom),
            })
        }
        FaceId::ExtrudeSide {
            extrusion,
            profile,
            edge,
        } => {
            let quad = crate::extrude::side_quad_world(doc, extrusion, profile, edge as usize)?;
            let mut zoom: Option<SketchZoomBounds> = None;
            for p in &quad {
                let (u, v) = world_to_local(&frame, *p);
                extend_sketch_bounds(&mut zoom, u, v, u, v);
            }
            if let Some(children) = sketch_local_bounds(doc, sketch) {
                zoom = Some(match zoom {
                    Some(z) => SketchZoomBounds::union(z, children),
                    None => children,
                });
            }
            let zoom = zoom?;
            let target = local_to_world(&frame, zoom.center_u, zoom.center_v);
            Some(SketchCameraTarget {
                target,
                face_normal,
                zoom: Some(zoom),
            })
        }
    }
}

pub fn sketch_label(doc: &Document, sketch: SketchId) -> String {
    let face = doc
        .sketch_face(sketch)
        .map(|face| face_label(doc, face))
        .unwrap_or_else(|| "unknown face".to_string());
    format!("Sketch {sketch} on {face}")
}

pub fn face_label(_doc: &Document, face: FaceId) -> String {
    match face {
        FaceId::ConstructionPlane(i) => format!("Construction plane {i}"),
        FaceId::Rect(i) => format!("Rectangle face {i}"),
        FaceId::Circle(i) => format!("Circle face {i}"),
        FaceId::ExtrudeCap {
            extrusion, top, ..
        } => {
            let end = if top { "top" } else { "bottom" };
            format!("Extrusion {extrusion} {end} face")
        }
        FaceId::ExtrudeSide {
            extrusion, edge, ..
        } => format!("Extrusion {extrusion} side face {edge}"),
    }
}

/// Screen-distance band within which two face picks count as "the same depth
/// under the cursor", so the nearer (camera-facing) one is preferred. This is
/// what keeps a hovered solid from selecting its hidden back face.
const FACE_PICK_DEPTH_TIE_PX: f32 = 0.5;

fn consider_face_pick(
    best: &mut Option<(FaceId, f32, f32)>,
    face: FaceId,
    dist: f32,
    depth: f32,
) {
    if dist > crate::construction::FACE_PICK_MARGIN_PX {
        return;
    }
    let better = match best.as_ref() {
        None => true,
        Some((_, best_dist, best_depth)) => {
            if dist < best_dist - FACE_PICK_DEPTH_TIE_PX {
                true
            } else if dist > best_dist + FACE_PICK_DEPTH_TIE_PX {
                false
            } else {
                // Essentially the same screen distance (e.g. cursor inside both the
                // front and back face of a solid): prefer the one nearer the camera.
                depth < *best_depth
            }
        }
    };
    if better {
        *best = Some((face, dist, depth));
    }
}

fn centroid(points: &[Vec3]) -> Vec3 {
    if points.is_empty() {
        return Vec3::ZERO;
    }
    points.iter().copied().sum::<Vec3>() / points.len() as f32
}

fn quad_face_pick_distance(
    screen: eframe::egui::Pos2,
    project: &impl Fn(Vec3) -> Option<eframe::egui::Pos2>,
    corners: [Vec3; 4],
) -> Option<(f32, Vec3)> {
    let pts: Option<Vec<eframe::egui::Pos2>> = corners.iter().map(|&c| project(c)).collect();
    let pts = pts?;
    let quad = [pts[0], pts[1], pts[2], pts[3]];
    let dist = if point_in_screen_quad(screen, quad) {
        0.0
    } else {
        dist_point_to_quad_edges(screen, quad)
    };
    Some((dist, centroid(&corners)))
}

/// Pick a sketchable face (rectangle, circle, or construction plane) under the cursor.
pub fn pick_sketch_face(
    screen: eframe::egui::Pos2,
    project: &impl Fn(Vec3) -> Option<eframe::egui::Pos2>,
    doc: &Document,
    eye: Vec3,
) -> Option<FaceId> {
    let mut best: Option<(FaceId, f32, f32)> = None;
    let depth = |p: Vec3| (p - eye).length();

    for (i, rect) in doc.rects.iter().enumerate().rev() {
        if let Some(corners) = rect_world_corners(doc, rect) {
            if let Some((dist, c)) = quad_face_pick_distance(screen, project, corners) {
                consider_face_pick(&mut best, FaceId::Rect(i), dist, depth(c));
            }
        }
    }

    for (i, circle) in doc.circles.iter().enumerate().rev() {
        if let Some((dist, c)) = circle_face_pick_distance(screen, doc, circle, project) {
            consider_face_pick(&mut best, FaceId::Circle(i), dist, depth(c));
        }
    }

    // Planar caps of extruded bodies (so sketches can be placed on them). Tested
    // before construction planes since a solid cap occludes the datum plane.
    for (ei, extrusion) in doc.extrusions.iter().enumerate().rev() {
        if extrusion.deleted {
            continue;
        }
        for &profile in &extrusion.faces {
            for top in [true, false] {
                if let Some((dist, c)) =
                    cap_face_pick_distance(screen, project, doc, ei, profile, top)
                {
                    consider_face_pick(
                        &mut best,
                        FaceId::ExtrudeCap {
                            extrusion: ei,
                            profile,
                            top,
                        },
                        dist,
                        depth(c),
                    );
                }
            }
            // Flat side walls (rectangular profiles) are sketchable too.
            for edge in 0..crate::extrude::side_face_count(profile) {
                if let Some((dist, c)) =
                    side_face_pick_distance(screen, project, doc, ei, profile, edge)
                {
                    consider_face_pick(
                        &mut best,
                        FaceId::ExtrudeSide {
                            extrusion: ei,
                            profile,
                            edge: edge as u8,
                        },
                        dist,
                        depth(c),
                    );
                }
            }
        }
    }

    for (i, plane) in doc.construction_planes.iter().enumerate().rev() {
        let corners = crate::construction::plane_corners(plane, crate::construction::PLANE_DISPLAY_HALF);
        if let Some((dist, c)) = quad_face_pick_distance(screen, project, corners) {
            consider_face_pick(&mut best, FaceId::ConstructionPlane(i), dist, depth(c));
        }
    }

    best.map(|(face, _, _)| face)
}

/// Screen-space pick distance to an extrusion cap polygon (0 inside).
fn cap_face_pick_distance(
    screen: eframe::egui::Pos2,
    project: &impl Fn(Vec3) -> Option<eframe::egui::Pos2>,
    doc: &Document,
    extrusion: usize,
    profile: crate::model::ExtrudeFace,
    top: bool,
) -> Option<(f32, Vec3)> {
    let poly = crate::extrude::cap_polygon_world(doc, extrusion, profile, top)?;
    polygon_face_pick_distance(screen, project, &poly)
}

/// Screen-space pick distance to an extrusion side wall (0 inside).
fn side_face_pick_distance(
    screen: eframe::egui::Pos2,
    project: &impl Fn(Vec3) -> Option<eframe::egui::Pos2>,
    doc: &Document,
    extrusion: usize,
    profile: crate::model::ExtrudeFace,
    edge: usize,
) -> Option<(f32, Vec3)> {
    let quad = crate::extrude::side_quad_world(doc, extrusion, profile, edge)?;
    polygon_face_pick_distance(screen, project, &quad)
}

/// Screen-space pick distance to a planar world-space polygon (0 inside, else
/// nearest edge), paired with the polygon's world centroid for depth ordering.
fn polygon_face_pick_distance(
    screen: eframe::egui::Pos2,
    project: &impl Fn(Vec3) -> Option<eframe::egui::Pos2>,
    poly: &[Vec3],
) -> Option<(f32, Vec3)> {
    let pts: Option<Vec<eframe::egui::Pos2>> = poly.iter().map(|&p| project(p)).collect();
    let pts = pts?;
    if pts.len() < 3 {
        return None;
    }
    let c = centroid(poly);
    // Inside test via triangle fan from the first vertex.
    let inside = (1..pts.len() - 1).any(|i| point_in_tri(screen, pts[0], pts[i], pts[i + 1]));
    if inside {
        return Some((0.0, c));
    }
    let mut edge = f32::MAX;
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        edge = edge.min(dist_point_to_segment_px(screen, pts[i], pts[j]));
    }
    Some((edge, c))
}

fn circle_face_pick_distance(
    screen: eframe::egui::Pos2,
    doc: &Document,
    circle: &Circle,
    project: &impl Fn(Vec3) -> Option<eframe::egui::Pos2>,
) -> Option<(f32, Vec3)> {
    let center = circle_world_center(doc, circle)?;
    let frame = sketch_geometry_frame(doc, circle.sketch)?;
    let rim = local_to_world(&frame, circle.cx + circle.r, circle.cy);
    let center_sp = project(center)?;
    let rim_sp = project(rim)?;
    let radius = (rim_sp - center_sp).length();
    if radius < 1e-3 {
        return None;
    }
    let d = (screen - center_sp).length();
    Some((if d <= radius { 0.0 } else { d - radius }, center))
}

fn point_in_screen_quad(p: eframe::egui::Pos2, quad: [eframe::egui::Pos2; 4]) -> bool {
    point_in_tri(p, quad[0], quad[1], quad[2]) || point_in_tri(p, quad[0], quad[2], quad[3])
}

fn point_in_tri(p: eframe::egui::Pos2, a: eframe::egui::Pos2, b: eframe::egui::Pos2, c: eframe::egui::Pos2) -> bool {
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

fn dist_point_to_quad_edges(p: eframe::egui::Pos2, quad: [eframe::egui::Pos2; 4]) -> f32 {
    let edges = [(0, 1), (1, 2), (2, 3), (3, 0)];
    edges
        .iter()
        .map(|&(i, j)| dist_point_to_segment_px(p, quad[i], quad[j]))
        .fold(f32::MAX, f32::min)
}

fn dist_point_to_segment_px(p: eframe::egui::Pos2, a: eframe::egui::Pos2, b: eframe::egui::Pos2) -> f32 {
    let ab = b - a;
    if ab.length_sq() < 1e-4 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Sketch;

    #[test]
    fn default_document_has_xy_construction_plane() {
        let doc = Document::default();
        assert_eq!(doc.construction_planes.len(), 1);
        assert!((doc.construction_planes[0].normal.z - 1.0).abs() < 1e-4);
        assert!(doc.shape_order.is_empty());
    }

    #[test]
    fn sketch_on_plane_stores_local_coordinates() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        let frame = sketch_geometry_frame(&doc, sketch).unwrap();
        let p = local_to_world(&frame, 10.0, 20.0);
        let (u, v) = world_to_local(&frame, p);
        assert!((u - 10.0).abs() < 1e-4);
        assert!((v - 20.0).abs() < 1e-4);
    }

    #[test]
    fn rect_face_frame_follows_parent_plane() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(
            sketch,
            5.0,
            5.0,
            15.0,
            15.0,
        ));
        let frame = sketch_frame(&doc, FaceId::Rect(0)).unwrap();
        assert!((frame.origin.x - 5.0).abs() < 1e-4);
        assert!((frame.origin.y - 5.0).abs() < 1e-4);
    }

    #[test]
    fn child_rect_is_offset_on_parent_face() {
        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(s0, 0.0, 0.0, 10.0, 10.0));
        let s1 = doc.add_sketch(FaceId::Rect(0));
        doc.rects.push(Rect::from_local_corners(s1, 2.0, 3.0, 5.0, 6.0));
        let corners = rect_world_corners(&doc, &doc.rects[1]).unwrap();
        assert!((corners[0].x - 2.0).abs() < 1e-4);
        assert!((corners[0].y - 3.0).abs() < 1e-4);
    }

    #[test]
    fn circle_face_frame_origin_is_center() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 5.0, 7.0, 10.0, 0.0));
        let frame = sketch_frame(&doc, FaceId::Circle(0)).unwrap();
        assert!((frame.origin.x - 5.0).abs() < 1e-4);
        assert!((frame.origin.y - 7.0).abs() < 1e-4);
    }

    #[test]
    fn child_sketch_on_circle_face_uses_center_origin() {
        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.circles
            .push(Circle::from_local_center_radius(s0, 10.0, 10.0, 5.0, 0.0));
        let s1 = doc.add_sketch(FaceId::Circle(0));
        let frame = sketch_geometry_frame(&doc, s1).unwrap();
        let p = local_to_world(&frame, 2.0, 3.0);
        assert!((p.x - 12.0).abs() < 1e-4);
        assert!((p.y - 13.0).abs() < 1e-4);
    }

    #[test]
    fn pick_sketch_face_finds_circle_interior() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 0.0, 0.0, 20.0, 0.0));
        let project = |p: Vec3| Some(eframe::egui::Pos2::new(p.x, p.y));
        let face = pick_sketch_face(eframe::egui::pos2(5.0, 0.0), &project, &doc, Vec3::new(0.0, 0.0, 100.0));
        assert_eq!(face, Some(FaceId::Circle(0)));
    }

    #[test]
    fn sketch_camera_circle_face_includes_face_and_children() {
        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.circles
            .push(Circle::from_local_center_radius(s0, 0.0, 0.0, 20.0, 0.0));
        let s1 = doc.add_sketch(FaceId::Circle(0));
        doc.lines
            .push(Line::from_local_endpoints(s1, -5.0, -5.0, 5.0, 5.0));
        let target = sketch_camera_target(&doc, s1).unwrap();
        let zoom = target.zoom.unwrap();
        assert!(zoom.half_u >= 5.0);
        assert!(zoom.half_v >= 5.0);
    }

    fn doc_with_extruded_box() -> Document {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 20.0, 20.0));
        doc.extrusions.push(crate::model::Extrusion {
            sketch,
            faces: vec![crate::model::ExtrudeFace::Rect(0)],
            distance: 10.0,
            target: None,
            expression: String::new(),
            name: None,
            deleted: false,
        });
        doc
    }

    #[test]
    fn sketch_on_extrusion_top_cap_is_offset_by_distance() {
        let doc = doc_with_extruded_box();
        let profile = crate::model::ExtrudeFace::Rect(0);
        let top = sketch_frame(
            &doc,
            FaceId::ExtrudeCap {
                extrusion: 0,
                profile,
                top: true,
            },
        )
        .unwrap();
        assert!((top.origin.z - 10.0).abs() < 1e-4, "top cap 10 above base");
        assert!((top.normal.z - 1.0).abs() < 1e-4);

        let bottom = sketch_frame(
            &doc,
            FaceId::ExtrudeCap {
                extrusion: 0,
                profile,
                top: false,
            },
        )
        .unwrap();
        assert!(bottom.origin.z.abs() < 1e-4, "bottom cap at base plane");

        // Geometry drawn on the top cap lands at the cap's height.
        let p = local_to_world(&top, 5.0, 5.0);
        assert!((p.z - 10.0).abs() < 1e-4);
    }

    #[test]
    fn sketch_on_slanted_top_cap_lies_in_the_target_plane() {
        let mut doc = doc_with_extruded_box();
        let plane_origin = Vec3::new(0.0, 0.0, 25.0);
        let plane_normal = Vec3::new(0.3, 0.0, 1.0).normalize();
        let mut slanted = default_xy_plane();
        slanted.origin = plane_origin;
        slanted.normal = plane_normal;
        doc.construction_planes.push(slanted);
        doc.extrusions[0].target = Some(crate::model::ExtrudeTarget::Plane(1));

        let frame = sketch_frame(
            &doc,
            FaceId::ExtrudeCap {
                extrusion: 0,
                profile: crate::model::ExtrudeFace::Rect(0),
                top: true,
            },
        )
        .unwrap();
        // The cap frame lies in the slanted plane, keeping the base's (+Z) facing.
        assert!(frame.normal.dot(plane_normal).abs() > 0.999);
        assert!(frame.normal.z > 0.0);
        for (u, v) in [(0.0, 0.0), (5.0, 3.0), (-2.0, 4.0)] {
            let p = local_to_world(&frame, u, v);
            let signed = (p - plane_origin).dot(plane_normal);
            assert!(signed.abs() < 1e-3, "sketched point off the cap plane: {signed}");
        }
    }

    #[test]
    fn pick_sketch_face_finds_extrusion_cap() {
        let doc = doc_with_extruded_box();
        // Offset screen x by height so the top cap (z=10) separates from the base
        // rect; click where only the lifted top cap projects.
        let project = |p: Vec3| Some(eframe::egui::Pos2::new(p.x + p.z, p.y));
        let face = pick_sketch_face(eframe::egui::pos2(25.0, 10.0), &project, &doc, Vec3::new(0.0, 0.0, 100.0));
        assert!(
            matches!(
                face,
                Some(FaceId::ExtrudeCap {
                    extrusion: 0,
                    top: true,
                    ..
                })
            ),
            "clicking the lifted top cap should pick it, got {face:?}"
        );
    }

    #[test]
    fn pick_prefers_the_camera_facing_cap_not_the_hidden_one() {
        // Top-down orthographic projection: both the top cap (z=10) and the bottom
        // cap (z=0) of the box project onto the same screen rectangle, so the cursor
        // at the center is inside both. The visible (camera-facing) cap must win.
        let doc = doc_with_extruded_box();
        let project = |p: Vec3| Some(eframe::egui::Pos2::new(p.x, p.y));
        let cursor = eframe::egui::pos2(10.0, 10.0);

        // Eye above the box: the near top cap must be picked, never the hidden
        // bottom cap (z=0) which faces away from the camera.
        let from_above = pick_sketch_face(cursor, &project, &doc, Vec3::new(10.0, 10.0, 100.0));
        assert!(
            matches!(from_above, Some(FaceId::ExtrudeCap { top: true, .. })),
            "looking down should pick the visible top cap, got {from_above:?}"
        );
    }

    #[test]
    fn sketch_on_extrusion_side_wall_lies_in_the_wall_plane() {
        let doc = doc_with_extruded_box();
        let profile = crate::model::ExtrudeFace::Rect(0);
        // Edge 0 runs along +X at y=0; the wall rises in +Z.
        let frame = sketch_frame(
            &doc,
            FaceId::ExtrudeSide {
                extrusion: 0,
                profile,
                edge: 0,
            },
        )
        .unwrap();
        // Origin at the base corner, in-plane axes along the edge (+X) and up the wall (+Z).
        assert!(frame.origin.abs_diff_eq(Vec3::ZERO, 1e-4));
        assert!((frame.u_axis.x - 1.0).abs() < 1e-4);
        assert!((frame.v_axis.z.abs() - 1.0).abs() < 1e-4);
        // Outward normal points away from the box centroid (-Y for this wall).
        assert!(frame.normal.y < -0.9, "outward normal {:?}", frame.normal);
        // Geometry drawn on the wall stays on the y=0 plane.
        let p = local_to_world(&frame, 5.0, 4.0);
        assert!(p.y.abs() < 1e-4, "point off the wall plane: {p:?}");
    }

    #[test]
    fn circular_profiles_have_no_flat_side_walls() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.extrusions.push(crate::model::Extrusion {
            sketch,
            faces: vec![crate::model::ExtrudeFace::Circle(0)],
            distance: 8.0,
            target: None,
            expression: String::new(),
            name: None,
            deleted: false,
        });
        let profile = crate::model::ExtrudeFace::Circle(0);
        assert_eq!(crate::extrude::side_face_count(profile), 0);
        assert!(crate::extrude::side_quad_world(&doc, 0, profile, 0).is_none());
    }

    #[test]
    fn pick_sketch_face_finds_extrusion_side_wall() {
        let doc = doc_with_extruded_box();
        // Project to the XZ plane so the y=0 side wall shows as a 20x10 rectangle.
        let project = |p: Vec3| Some(eframe::egui::Pos2::new(p.x, p.z));
        let face = pick_sketch_face(eframe::egui::pos2(10.0, 5.0), &project, &doc, Vec3::new(0.0, 0.0, 100.0));
        assert!(
            matches!(face, Some(FaceId::ExtrudeSide { extrusion: 0, .. })),
            "clicking a side wall should pick it, got {face:?}"
        );
    }

    #[test]
    fn has_children_detects_dependents() {
        let mut doc = Document::default();
        assert!(!doc.has_children(FaceId::ConstructionPlane(0)));
        doc.sketches.push(Sketch {
            face: FaceId::ConstructionPlane(0),
            name: None,
            deleted: false,
        });
        assert!(doc.has_children(FaceId::ConstructionPlane(0)));
    }

    #[test]
    fn sketch_camera_empty_plane_orients_without_zoom() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        let target = sketch_camera_target(&doc, sketch).unwrap();
        assert!(target.zoom.is_none());
        assert!(target.target.length_squared() < 1e-8);
        assert!((target.face_normal.z - 1.0).abs() < 1e-4);
    }

    #[test]
    fn sketch_camera_plane_includes_circles_lines_and_all_rects() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 20.0, 20.0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 70.0, 0.0, 90.0, 20.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 30.0, 5.0, 50.0, 15.0));
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 40.0, 50.0, 15.0, 0.0));
        let zoom = sketch_camera_target(&doc, sketch)
            .unwrap()
            .zoom
            .expect("mixed sketch should request zoom");
        assert!(
            zoom.half_u >= 44.0,
            "zoom should span both rectangles and the circle, got half_u={}",
            zoom.half_u
        );
        assert!(
            zoom.half_v >= 32.0,
            "zoom should include circle vertical extent, got half_v={}",
            zoom.half_v
        );
    }

    #[test]
    fn sketch_camera_plane_with_children_requests_zoom() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(
            sketch,
            10.0,
            20.0,
            90.0,
            60.0,
        ));
        let target = sketch_camera_target(&doc, sketch).unwrap();
        let zoom = target.zoom.expect("children should request zoom");
        assert!((zoom.center_u - 50.0).abs() < 1e-4);
        assert!((zoom.center_v - 40.0).abs() < 1e-4);
        assert!((zoom.half_u - 40.0).abs() < 1e-4);
        assert!((zoom.half_v - 20.0).abs() < 1e-4);
    }

    #[test]
    fn sketch_camera_rect_face_includes_face_and_children() {
        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(s0, 0.0, 0.0, 20.0, 20.0));
        let s1 = doc.add_sketch(FaceId::Rect(0));
        doc.rects.push(Rect::from_local_corners(
            s1,
            2.0,
            2.0,
            18.0,
            18.0,
        ));
        doc.lines.push(Line::from_local_endpoints(
            s1,
            5.0,
            5.0,
            15.0,
            10.0,
        ));
        let target = sketch_camera_target(&doc, s1).unwrap();
        let zoom = target.zoom.unwrap();
        assert!(zoom.half_u >= 8.0);
        assert!(zoom.half_v >= 8.0);
    }

    #[test]
    fn sketch_view_up_from_isometric_prefers_green_right_red_down() {
        use crate::camera::Camera;

        let cam = Camera::default();
        let frame = SketchFrame {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Y,
            normal: Vec3::Z,
        };
        let view_dir = cam.visible_face_view_direction(Vec3::ZERO, Vec3::Z);
        let current_look = (Vec3::ZERO - cam.eye()).normalize_or_zero();
        let current_up = cam.view_up_hint();
        let target_look = (-view_dir).normalize_or_zero();
        let u = frame.u_axis;
        let v = frame.v_axis;
        let u0 = axis_screen_vec(u, current_look, current_up);
        let v0 = axis_screen_vec(v, current_look, current_up);

        let score_neg_x = {
            let h = -Vec3::X;
            sketch_view_up_score(
                u0,
                v0,
                axis_screen_vec(u, target_look, h),
                axis_screen_vec(v, target_look, h),
            )
        };
        let score_neg_y = {
            let h = -Vec3::Y;
            sketch_view_up_score(
                u0,
                v0,
                axis_screen_vec(u, target_look, h),
                axis_screen_vec(v, target_look, h),
            )
        };
        assert!(
            score_neg_x <= score_neg_y + 1e-4,
            "±X hint should beat ±Y: score_neg_x={score_neg_x} score_neg_y={score_neg_y}"
        );

        let hint = sketch_view_up(view_dir, &frame, current_look, current_up);
        assert!(
            hint.dot(Vec3::X).abs() > 0.9,
            "isometric entry should pick ±X up hint to preserve green right, got {hint:?}"
        );
    }

    #[test]
    fn sketch_view_up_prefers_minimal_roll_flip() {
        let frame = SketchFrame {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Y,
            normal: Vec3::Z,
        };
        let hint = sketch_view_up(Vec3::Z, &frame, -Vec3::Z, Vec3::Y);
        assert!(
            hint.dot(Vec3::Y) > 0.0,
            "already aligned with +Y should keep +Y hint, got {hint:?}"
        );
    }

    #[test]
    fn sketch_view_up_on_vertical_wall_keeps_ground_at_the_bottom() {
        // A side wall whose in-plane axes are u along world +X and v along world
        // +Z (a vertical wall facing -Y). Regardless of how the camera was rolled
        // before, the sketch should orient so world up (+Z, our v axis) points up
        // on screen, putting the ground at the bottom.
        let frame = SketchFrame {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Z,
            normal: -Vec3::Y,
        };
        // view_direction points from the face toward the eye (outward normal, -Y).
        let view_direction = -Vec3::Y;
        // Start from a rolled-sideways view (current up pointing along +X).
        let hint = sketch_view_up(view_direction, &frame, Vec3::Y, Vec3::X);
        assert!(
            hint.dot(Vec3::Z) > 0.9,
            "vertical wall sketch should orient world +Z up, got {hint:?}"
        );
    }

    #[test]
    fn sketch_view_up_aligns_plane_axes_with_screen() {
        use crate::camera::Camera;
        use crate::construction::{
            definition_from_reference, plane_from_definition, PlaneReference,
        };
        use crate::model::ConstructionPlaneParent;
        use eframe::egui::{Pos2, Rect};

        let mut doc = Document::default();
        doc.construction_planes.push(plane_from_definition(
            &definition_from_reference(
                &PlaneReference::Axis {
                    origin: Vec3::ZERO,
                    direction: Vec3::X,
                    label: "X axis".to_string(),
                },
                0.0,
                45.0,
            ),
            ConstructionPlaneParent::Root,
        ));
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(1));
        let frame = sketch_frame(&doc, FaceId::ConstructionPlane(1)).unwrap();
        let mut cam = Camera::default();
        cam.target = frame.origin;
        cam.distance = 200.0;
        let view_direction =
            cam.visible_face_view_direction(frame.origin, frame.normal);
        let look_forward = (cam.target - cam.eye()).normalize_or_zero();
        let hint = sketch_view_up(
            view_direction,
            &frame,
            look_forward,
            cam.view_up_hint(),
        );
        cam.set_view_up(Some(hint));
        let (yaw, pitch) = Camera::view_direction_to_yaw_pitch(view_direction);
        cam.yaw = yaw;
        cam.pitch = pitch;

        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let vp = cam.view_proj(viewport);
        let base = cam.project(frame.origin, viewport, &vp).unwrap();
        let above = cam
            .project(frame.origin + frame.v_axis * 10.0, viewport, &vp)
            .unwrap();
        let right = cam
            .project(frame.origin + frame.u_axis * 10.0, viewport, &vp)
            .unwrap();

        assert!(
            above.y < base.y,
            "positive v should point up on screen (smaller egui y)"
        );
        assert!(
            right.x > base.x,
            "positive u should point right on screen"
        );
        let _ = sketch;
    }
}