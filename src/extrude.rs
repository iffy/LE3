//! Extrusions: turning coplanar sketch faces into 3D solid meshes.
//!
//! Stage 1 builds the data-driven solid geometry (a prism/cylinder per face) from an
//! [`Extrusion`]. Rendering and the interactive tool layer build on top of this.
// The mesh API is exercised by tests and consumed by the (next-stage) GPU renderer.
#![allow(dead_code)]

use crate::face::{local_to_world, sketch_geometry_frame, SketchFrame};
use crate::geometric_constraints::point_uv;
use crate::model::{Document, ExtrudeFace, ExtrudeTarget, Extrusion};
use glam::Vec3;

/// Number of segments used to facet a circular profile.
pub const CIRCLE_SEGMENTS: usize = 48;

/// A triangle solid mesh in world space (3 positions per triangle).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SolidMesh {
    pub triangles: Vec<[Vec3; 3]>,
}

impl SolidMesh {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// Axis-aligned bounds of all triangle vertices, if any.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut iter = self.triangles.iter().flat_map(|t| t.iter());
        let first = *iter.next()?;
        let mut min = first;
        let mut max = first;
        for p in iter {
            min = min.min(*p);
            max = max.max(*p);
        }
        Some((min, max))
    }
}

/// Build the solid mesh for an extrusion, or `None` if it has no faces or zero distance.
pub fn extrusion_mesh(doc: &Document, extrusion: &Extrusion) -> Option<SolidMesh> {
    let distance = effective_distance(doc, extrusion);
    if extrusion.faces.is_empty() || distance.abs() < 1e-4 {
        return None;
    }
    let mut mesh = SolidMesh::default();
    for face in &extrusion.faces {
        if let Some((profile, normal)) = face_profile_world(doc, *face) {
            let top: Vec<Vec3> = profile
                .iter()
                .map(|p| extruded_top_point(doc, extrusion, normal, *p, distance))
                .collect();
            extrude_profile(&profile, &top, &mut mesh.triangles);
        }
    }
    (!mesh.is_empty()).then_some(mesh)
}

/// The `(point, normal)` plane an extrusion's top cap should lie in, when its target defines
/// one. A vertex target or a plain typed distance has no such plane.
pub fn target_top_plane(doc: &Document, extrusion: &Extrusion) -> Option<(Vec3, Vec3)> {
    match extrusion.target? {
        ExtrudeTarget::Face(face) => face_plane(doc, face),
        ExtrudeTarget::Plane(index) => {
            let plane = doc.construction_planes.get(index)?;
            Some((plane.origin, plane.normal))
        }
        ExtrudeTarget::Vertex(_) => None,
    }
}

/// Where a base profile vertex `v` lands when extruded along `dir`. With a target plane each
/// vertex slides until it meets that plane, so the whole top cap lies in it (full contact even
/// when the plane is slanted); otherwise the vertex is offset uniformly by `uniform`.
pub fn extruded_top_point(
    doc: &Document,
    extrusion: &Extrusion,
    dir: Vec3,
    v: Vec3,
    uniform: f32,
) -> Vec3 {
    if let Some((p, n)) = target_top_plane(doc, extrusion) {
        if let Some(t) = plane_axis_distance(v, dir, p, n) {
            return v + dir * t;
        }
    }
    v + dir * uniform
}

/// The effective signed depth: derived from `target`'s extended plane when set, else `distance`.
pub fn effective_distance(doc: &Document, extrusion: &Extrusion) -> f32 {
    if let Some(target) = extrusion.target {
        if let Some((base, normal)) = faces_anchor(doc, &extrusion.faces) {
            if let Some(d) = target_distance(doc, base, normal, target) {
                return d;
            }
        }
    }
    extrusion.distance
}

/// Signed distance along `normal` from `base` to where the axis reaches `target`'s plane.
pub fn target_distance(
    doc: &Document,
    base: Vec3,
    normal: Vec3,
    target: ExtrudeTarget,
) -> Option<f32> {
    match target {
        ExtrudeTarget::Vertex(point) => {
            let world = constraint_point_world(doc, point)?;
            Some((world - base).dot(normal))
        }
        ExtrudeTarget::Face(face) => {
            let (p, n) = face_plane(doc, face)?;
            plane_axis_distance(base, normal, p, n)
        }
        ExtrudeTarget::Plane(index) => {
            let plane = doc.construction_planes.get(index)?;
            plane_axis_distance(base, normal, plane.origin, plane.normal)
        }
    }
}

/// Distance along `dir` from `base` to the plane (`point`, `plane_normal`).
fn plane_axis_distance(base: Vec3, dir: Vec3, point: Vec3, plane_normal: Vec3) -> Option<f32> {
    let denom = dir.dot(plane_normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    Some((point - base).dot(plane_normal) / denom)
}

fn face_plane(doc: &Document, face: ExtrudeFace) -> Option<(Vec3, Vec3)> {
    let (center, normal) = face_center_world(doc, face)?;
    Some((center, normal))
}

pub fn constraint_point_world(doc: &Document, point: crate::model::ConstraintPoint) -> Option<Vec3> {
    let sketch = match point {
        crate::model::ConstraintPoint::LineEndpoint { line, .. } => doc.lines.get(line)?.sketch,
        crate::model::ConstraintPoint::RectCorner { rect, .. } => doc.rects.get(rect)?.sketch,
        crate::model::ConstraintPoint::CircleCenter(circle) => doc.circles.get(circle)?.sketch,
    };
    let frame = sketch_geometry_frame(doc, sketch)?;
    let (u, v) = point_uv(doc, point).ok()?;
    Some(local_to_world(&frame, u, v))
}

/// Gizmo anchor for a set of coplanar faces: the centroid of their centers and the plane
/// normal (the extrusion direction).
pub fn faces_anchor(doc: &Document, faces: &[ExtrudeFace]) -> Option<(Vec3, Vec3)> {
    let mut sum = Vec3::ZERO;
    let mut count = 0u32;
    let mut normal = Vec3::ZERO;
    for face in faces {
        if let Some(center) = face_center_world(doc, *face) {
            sum += center.0;
            normal = center.1;
            count += 1;
        }
    }
    (count > 0).then(|| (sum / count as f32, normal))
}

/// World center and normal of a face.
fn face_center_world(doc: &Document, face: ExtrudeFace) -> Option<(Vec3, Vec3)> {
    match face {
        ExtrudeFace::Rect(i) => {
            let rect = doc.rects.get(i)?;
            let frame = sketch_geometry_frame(doc, rect.sketch)?;
            Some((
                local_to_world(&frame, rect.x + rect.w * 0.5, rect.y + rect.h * 0.5),
                frame.normal,
            ))
        }
        ExtrudeFace::Circle(i) => {
            let circle = doc.circles.get(i)?;
            let frame = sketch_geometry_frame(doc, circle.sketch)?;
            Some((local_to_world(&frame, circle.cx, circle.cy), frame.normal))
        }
    }
}

/// World-space boundary loop (CCW in the face frame) and outward normal of a face.
pub fn face_profile_world(doc: &Document, face: ExtrudeFace) -> Option<(Vec<Vec3>, Vec3)> {
    match face {
        ExtrudeFace::Rect(index) => {
            let rect = doc.rects.get(index)?;
            if rect.deleted {
                return None;
            }
            let frame = sketch_geometry_frame(doc, rect.sketch)?;
            let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
            let corners = [
                local_to_world(&frame, x, y),
                local_to_world(&frame, x + w, y),
                local_to_world(&frame, x + w, y + h),
                local_to_world(&frame, x, y + h),
            ];
            Some((corners.to_vec(), frame.normal))
        }
        ExtrudeFace::Circle(index) => {
            let circle = doc.circles.get(index)?;
            if circle.deleted {
                return None;
            }
            let frame = sketch_geometry_frame(doc, circle.sketch)?;
            let profile = circle_profile_world(&frame, circle.cx, circle.cy, circle.r);
            Some((profile, frame.normal))
        }
    }
}

/// World-space boundary loop of an extrusion cap. `top` selects the offset end
/// (base + distance·normal); otherwise the base end at the sketch plane.
pub fn cap_polygon_world(
    doc: &Document,
    extrusion: usize,
    profile: ExtrudeFace,
    top: bool,
) -> Option<Vec<Vec3>> {
    let ext = doc.extrusions.get(extrusion)?;
    if ext.deleted || !ext.faces.contains(&profile) {
        return None;
    }
    let (poly, normal) = face_profile_world(doc, profile)?;
    if !top {
        return Some(poly);
    }
    // The top cap follows the (possibly slanted) target plane, vertex by vertex.
    let distance = effective_distance(doc, ext);
    Some(
        poly.into_iter()
            .map(|p| extruded_top_point(doc, ext, normal, p, distance))
            .collect(),
    )
}

/// Number of flat, sketchable side walls of a profile (rectangles have 4; circular
/// profiles are curved and have none).
pub fn side_face_count(profile: ExtrudeFace) -> usize {
    match profile {
        ExtrudeFace::Rect(_) => 4,
        ExtrudeFace::Circle(_) => 0,
    }
}

/// World-space quad of an extrusion side wall, swept by `edge` of a polygonal profile.
/// Ordered `[base_a, base_b, top_b, top_a]`. `None` for circular profiles, out-of-range
/// edges, or a deleted/foreign extrusion.
pub fn side_quad_world(
    doc: &Document,
    extrusion: usize,
    profile: ExtrudeFace,
    edge: usize,
) -> Option<[Vec3; 4]> {
    let ext = doc.extrusions.get(extrusion)?;
    if ext.deleted || !ext.faces.contains(&profile) || edge >= side_face_count(profile) {
        return None;
    }
    let (poly, normal) = face_profile_world(doc, profile)?;
    let n = poly.len();
    if edge >= n {
        return None;
    }
    let a = poly[edge];
    let b = poly[(edge + 1) % n];
    // The top edge follows the (possibly slanted) target plane, so the wall stays planar.
    let distance = effective_distance(doc, ext);
    let top_a = extruded_top_point(doc, ext, normal, a, distance);
    let top_b = extruded_top_point(doc, ext, normal, b, distance);
    Some([a, b, top_b, top_a])
}

fn circle_profile_world(frame: &SketchFrame, cx: f32, cy: f32, r: f32) -> Vec<Vec3> {
    (0..CIRCLE_SEGMENTS)
        .map(|i| {
            let a = i as f32 / CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
            local_to_world(frame, cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// Emit caps + side walls for a convex profile, given its base loop and the matching
/// `top` loop (one top vertex per base vertex, so the top cap may be slanted).
fn extrude_profile(profile: &[Vec3], top: &[Vec3], triangles: &mut Vec<[Vec3; 3]>) {
    let n = profile.len();
    if n < 3 || top.len() != n {
        return;
    }

    // Bottom cap (fan).
    for i in 1..n - 1 {
        triangles.push([profile[0], profile[i + 1], profile[i]]);
    }
    // Top cap (fan, opposite winding).
    for i in 1..n - 1 {
        triangles.push([top[0], top[i], top[i + 1]]);
    }
    // Side walls (one quad per edge).
    for i in 0..n {
        let j = (i + 1) % n;
        triangles.push([profile[i], profile[j], top[j]]);
        triangles.push([profile[i], top[j], top[i]]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Circle, Document, FaceId, Rect};

    fn sketch_doc() -> (Document, crate::model::SketchId) {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        (doc, sketch)
    }

    fn extrusion(sketch: crate::model::SketchId, faces: Vec<ExtrudeFace>, distance: f32) -> Extrusion {
        Extrusion {
            sketch,
            faces,
            distance,
            target: None,
            expression: String::new(),
            name: None,
            deleted: false,
        }
    }

    #[test]
    fn rect_extrudes_to_a_box_mesh() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 4.0));
        let ext = extrusion(sketch, vec![ExtrudeFace::Rect(0)], 6.0);
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        // A box: 2 (bottom) + 2 (top) + 4 edges * 2 = 12 triangles.
        assert_eq!(mesh.triangles.len(), 12);
        // Ground plane normal is +Z, so the solid spans z in [0, 6].
        let (min, max) = mesh.bounds().unwrap();
        assert!((min.z).abs() < 1e-4 && (max.z - 6.0).abs() < 1e-4, "z [{},{}]", min.z, max.z);
        assert!((max.x - min.x - 10.0).abs() < 1e-4 && (max.y - min.y - 4.0).abs() < 1e-4);
    }

    #[test]
    fn negative_distance_extrudes_the_other_way() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 4.0));
        let ext = extrusion(sketch, vec![ExtrudeFace::Rect(0)], -5.0);
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        let (min, max) = mesh.bounds().unwrap();
        assert!((min.z + 5.0).abs() < 1e-4 && (max.z).abs() < 1e-4, "z [{},{}]", min.z, max.z);
    }

    #[test]
    fn circle_extrudes_to_a_cylinder_mesh() {
        let (mut doc, sketch) = sketch_doc();
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 0.0, 0.0, 5.0, 0.0));
        let ext = extrusion(sketch, vec![ExtrudeFace::Circle(0)], 8.0);
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        // Cylinder: 2 caps of (N-2) + 2N side triangles.
        let n = CIRCLE_SEGMENTS;
        assert_eq!(mesh.triangles.len(), 2 * (n - 2) + 2 * n);
        let (min, max) = mesh.bounds().unwrap();
        assert!((max.z - 8.0).abs() < 1e-4 && min.z.abs() < 1e-4);
        // Radius 5 → diameter 10 in x and y.
        assert!((max.x - min.x - 10.0).abs() < 0.1 && (max.y - min.y - 10.0).abs() < 0.1);
    }

    #[test]
    fn multiple_faces_combine() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 4.0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 20.0, 0.0, 30.0, 4.0));
        let ext = extrusion(
            sketch,
            vec![ExtrudeFace::Rect(0), ExtrudeFace::Rect(1)],
            6.0,
        );
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        assert_eq!(mesh.triangles.len(), 24);
    }

    #[test]
    fn extrude_up_to_plane_uses_target_plane() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        // A parallel construction plane 20mm above the ground.
        let mut above = crate::face::default_xy_plane();
        above.origin = Vec3::new(0.0, 0.0, 20.0);
        doc.construction_planes.push(above);

        let mut ext = extrusion(sketch, vec![ExtrudeFace::Rect(0)], 6.0);
        ext.target = Some(ExtrudeTarget::Plane(1));

        // Effective depth reaches the target plane regardless of the stored distance.
        assert!((effective_distance(&doc, &ext) - 20.0).abs() < 1e-3);
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        let (min, max) = mesh.bounds().unwrap();
        assert!((max.z - 20.0).abs() < 1e-3 && min.z.abs() < 1e-3, "z [{},{}]", min.z, max.z);
    }

    #[test]
    fn extrude_to_slanted_plane_lands_top_cap_in_the_plane() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 6.0));
        // A construction plane tilted about the X axis, ~21.8°, raised above the sketch.
        let plane_origin = Vec3::new(0.0, 0.0, 12.0);
        let plane_normal = Vec3::new(0.0, 0.4, 1.0).normalize();
        let mut slanted = crate::face::default_xy_plane();
        slanted.origin = plane_origin;
        slanted.normal = plane_normal;
        doc.construction_planes.push(slanted);

        let mut ext = extrusion(sketch, vec![ExtrudeFace::Rect(0)], 6.0);
        ext.target = Some(ExtrudeTarget::Plane(1));
        doc.extrusions.push(ext.clone());

        // Every top-cap corner lies exactly in the slanted plane (full contact).
        let cap = cap_polygon_world(&doc, 0, ExtrudeFace::Rect(0), true).unwrap();
        assert_eq!(cap.len(), 4);
        for corner in &cap {
            let signed = (*corner - plane_origin).dot(plane_normal);
            assert!(signed.abs() < 1e-3, "cap corner off the target plane: {signed}");
        }
        // The cap really is slanted: corners reach the plane at different heights.
        let heights: Vec<f32> = cap.iter().map(|c| c.z).collect();
        let zmin = heights.iter().cloned().fold(f32::MAX, f32::min);
        let zmax = heights.iter().cloned().fold(f32::MIN, f32::max);
        assert!(zmax - zmin > 1.0, "expected a slanted top, spread {}", zmax - zmin);

        // The base cap stays on the sketch plane (z = 0).
        let base = cap_polygon_world(&doc, 0, ExtrudeFace::Rect(0), false).unwrap();
        for corner in &base {
            assert!(corner.z.abs() < 1e-4);
        }
    }

    #[test]
    fn zero_distance_or_no_faces_yields_no_mesh() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 4.0));
        assert!(extrusion_mesh(&doc, &extrusion(sketch, vec![ExtrudeFace::Rect(0)], 0.0)).is_none());
        assert!(extrusion_mesh(&doc, &extrusion(sketch, vec![], 6.0)).is_none());
    }
}
