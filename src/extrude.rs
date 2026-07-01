//! Extrusions: turning coplanar sketch faces into 3D solid meshes.
//!
//! Stage 1 builds the data-driven solid geometry (a prism/cylinder per face) from an
//! [`Extrusion`]. Rendering and the interactive tool layer build on top of this.
// The mesh API is exercised by tests and consumed by the (next-stage) GPU renderer.
#![allow(dead_code)]

use crate::face::{local_to_world, sketch_geometry_frame, SketchFrame};
use crate::geometric_constraints::point_uv;
use crate::model::{
    vertex_treatment_geometry, Document, EdgeTreatment, ExtrudeFace, ExtrudeTarget, Extrusion,
    ExtrusionEdgeRef, FaceId, VertexTreatmentKind,
};
use glam::Vec3;
use std::collections::HashMap;

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
    // First real switch onto the OCCT kernel (#86): a plain single-profile
    // extrusion becomes a genuine BREP prism, tessellated by OCCT. Falls through
    // to the hand-rolled mesher for everything it doesn't yet cover (slanted
    // targets, edge chamfers/fillets, multi-face bodies) so behavior is preserved.
    #[cfg(feature = "occt")]
    if let Some(mesh) = occt_extrusion_mesh(doc, extrusion, distance) {
        return Some(mesh);
    }
    let mut mesh = SolidMesh::default();
    for (face_index, face) in extrusion.faces.iter().enumerate() {
        if let Some((profile, normal)) = face_profile_world(doc, face) {
            let top: Vec<Vec3> = profile
                .iter()
                .map(|p| extruded_top_point(doc, extrusion, normal, *p, distance))
                .collect();
            let treatments: Vec<&EdgeTreatment> = extrusion
                .edge_treatments
                .iter()
                .filter(|t| t.edge.face() == face_index && t.amount > 0.0)
                .collect();
            if treatments.is_empty() {
                extrude_profile(&profile, &top, &mut mesh.triangles);
            } else {
                extrude_profile_with_treatments(&profile, &top, &treatments, &mut mesh.triangles);
            }
        }
    }
    (!mesh.is_empty()).then_some(mesh)
}

/// OCCT BREP solid for the extrusions the kernel currently handles (#86/#77): a
/// single profile face extruded by a pure translation (prism) or to a slanted
/// target (ruled loft), with any 3D edge chamfer/fillet edge treatments applied as
/// *real* `BRepFilletAPI` fillets/chamfers on the built solid (#77). `None` for
/// anything else — a multi-face extrusion, a degenerate profile, or any edge
/// treatment the kernel can't place (see [`edge_ref_world_endpoints`]) — so callers
/// fall back to the hand-rolled mesher and we never ship broken geometry.
#[cfg(feature = "occt")]
fn occt_extrusion_shape(
    doc: &Document,
    extrusion: &Extrusion,
    distance: f32,
) -> Option<crate::kernel::Shape> {
    // Exactly one face.
    let [face] = extrusion.faces.as_slice() else {
        return None;
    };
    let (profile, normal) = face_profile_world(doc, face)?;
    if profile.len() < 3 {
        return None;
    }
    let top: Vec<Vec3> = profile
        .iter()
        .map(|p| extruded_top_point(doc, extrusion, normal, *p, distance))
        .collect();
    // A pure translation is a single prism (simplest/most robust); a slanted
    // target (per-vertex top offset, e.g. extrude-to-an-angled-face) is a ruled
    // loft between the bottom and top loops.
    let dir = top[0] - profile[0];
    let is_translation = profile
        .iter()
        .zip(&top)
        .all(|(p, t)| (*t - *p - dir).length() <= 1e-4);
    let base_shape = if is_translation {
        crate::kernel::Shape::prism(&profile, dir)
    } else {
        crate::kernel::Shape::loft(&profile, &top)
    }?;

    // Real BREP edge fillets/chamfers (#77). Split the active treatments into fillet
    // and chamfer groups (each applied in one batched kernel call), matching each
    // edge to the built solid by its analytic world-space endpoints. Any missing edge
    // or kernel error returns `None` -> the whole extrusion falls back to the mesher.
    let mut fillet_edges: Vec<(Vec3, Vec3)> = Vec::new();
    let mut fillet_radii: Vec<f32> = Vec::new();
    let mut chamfer_edges: Vec<(Vec3, Vec3)> = Vec::new();
    let mut chamfer_dists: Vec<f32> = Vec::new();
    for t in &extrusion.edge_treatments {
        if t.amount <= 0.0 {
            continue;
        }
        let endpoints = edge_ref_world_endpoints(doc, extrusion, &t.edge)?;
        match t.kind {
            VertexTreatmentKind::Fillet => {
                fillet_edges.push(endpoints);
                fillet_radii.push(t.amount);
            }
            VertexTreatmentKind::Chamfer => {
                chamfer_edges.push(endpoints);
                chamfer_dists.push(t.amount);
            }
        }
    }
    if fillet_edges.is_empty() && chamfer_edges.is_empty() {
        return Some(base_shape);
    }
    let mut shape = base_shape;
    if !fillet_edges.is_empty() {
        shape = shape.fillet(&fillet_edges, &fillet_radii)?;
    }
    if !chamfer_edges.is_empty() {
        shape = shape.chamfer(&chamfer_edges, &chamfer_dists)?;
    }
    Some(shape)
}

/// World-space endpoints of one analytic extrusion edge (#77), derived from the very
/// same analytic geometry [`treatable_edges`] and the hand-rolled mesh-bevel builder
/// use — so the OCCT edge-matching in [`occt_extrusion_shape`] keys off the identical
/// coordinates the picking/preview code does. A `Vertical` edge runs from a bottom
/// profile vertex to the corresponding top vertex; a `Cap` edge is the boundary
/// between consecutive vertices of the chosen (base/top) ring. `None` if the face is
/// missing/degenerate or the edge index is out of range for its profile loop.
#[cfg(feature = "occt")]
fn edge_ref_world_endpoints(
    doc: &Document,
    extrusion: &Extrusion,
    edge: &ExtrusionEdgeRef,
) -> Option<(Vec3, Vec3)> {
    let face = extrusion.faces.get(edge.face())?;
    let (base, normal) = face_profile_world(doc, face)?;
    let n = base.len();
    if n < 3 {
        return None;
    }
    let distance = effective_distance(doc, extrusion);
    let top = |i: usize| extruded_top_point(doc, extrusion, normal, base[i], distance);
    match *edge {
        ExtrusionEdgeRef::Vertical { edge, .. } => {
            if edge >= n {
                return None;
            }
            let v = (edge + 1) % n;
            Some((base[v], top(v)))
        }
        ExtrusionEdgeRef::Cap { edge, top: is_top, .. } => {
            if edge >= n {
                return None;
            }
            let e2 = (edge + 1) % n;
            if is_top {
                Some((top(edge), top(e2)))
            } else {
                Some((base[edge], base[e2]))
            }
        }
    }
}

/// OCCT-backed mesh for a single extrusion (see [`occt_extrusion_shape`]).
#[cfg(feature = "occt")]
fn occt_extrusion_mesh(doc: &Document, extrusion: &Extrusion, distance: f32) -> Option<SolidMesh> {
    let shape = occt_extrusion_shape(doc, extrusion, distance)?;
    let tris = shape.tessellate(OCCT_DEFLECTION as f64);
    (!tris.is_empty()).then_some(SolidMesh { triangles: tris })
}

/// OCCT solid fusing every kernel-representable extrusion in `indices` into one real unioned
/// shape. `None` if any listed extrusion isn't kernel-representable; the outer `Option`-of-
/// -`Option` collapses to `Some(None)` when the list contributes no geometry at all (all
/// deleted/degenerate).
#[cfg(feature = "occt")]
fn occt_fused_extrusions(
    doc: &Document,
    indices: &[usize],
) -> Option<Option<crate::kernel::Shape>> {
    use crate::kernel::BoolOp;
    let mut fused: Option<crate::kernel::Shape> = None;
    for &ei in indices {
        let extrusion = doc.extrusions.get(ei)?;
        if extrusion.deleted {
            continue;
        }
        let distance = effective_distance(doc, extrusion);
        if extrusion.faces.is_empty() || distance.abs() < 1e-4 {
            continue;
        }
        let shape = occt_extrusion_shape(doc, extrusion, distance)?;
        fused = Some(match fused {
            None => shape,
            Some(acc) => acc.boolean(&shape, BoolOp::Fuse)?,
        });
    }
    Some(fused)
}

/// OCCT-backed mesh for a whole body whose every extrusion the kernel can
/// represent: the per-extrusion prisms are **fused** into one real unioned solid
/// (#86), then any **cut** extrusions are subtracted from that solid (#35) — so
/// overlapping add-to-body extrusions merge into a single watertight shape and cuts
/// carve real holes, instead of concatenated triangle soup with internal walls.
/// `None` if any add/cut extrusion isn't kernel-representable, so [`body_solid_mesh`]
/// falls back to the hand-rolled per-extrusion concatenation.
#[cfg(feature = "occt")]
fn occt_body_mesh(
    doc: &Document,
    add_indices: &[usize],
    cut_indices: &[usize],
) -> Option<SolidMesh> {
    let solid = occt_body_shape_from_indices(doc, add_indices, cut_indices)?;
    let tris = solid.tessellate(OCCT_DEFLECTION as f64);
    (!tris.is_empty()).then_some(SolidMesh { triangles: tris })
}

/// Build the fused/cut OCCT solid for the extrusions in `add_indices`/`cut_indices` — the
/// real BREP shape *before* tessellation (see [`occt_body_mesh`]). `None` if any add/cut
/// extrusion isn't kernel-representable, or the adds contribute no geometry at all.
#[cfg(feature = "occt")]
fn occt_body_shape_from_indices(
    doc: &Document,
    add_indices: &[usize],
    cut_indices: &[usize],
) -> Option<crate::kernel::Shape> {
    use crate::kernel::BoolOp;
    let mut solid = occt_fused_extrusions(doc, add_indices)??;
    // Subtract each cut extrusion's solid. A cut that isn't kernel-representable aborts to the
    // fallback (returns None); a cut contributing no geometry is a no-op.
    for &ei in cut_indices {
        let extrusion = doc.extrusions.get(ei)?;
        if extrusion.deleted {
            continue;
        }
        let distance = effective_distance(doc, extrusion);
        if extrusion.faces.is_empty() || distance.abs() < 1e-4 {
            continue;
        }
        let cut = occt_extrusion_shape(doc, extrusion, distance)?;
        solid = solid.boolean(&cut, BoolOp::Cut)?;
    }
    Some(solid)
}

/// The body's real OCCT BREP solid (adds fused, cuts subtracted), *before* tessellation —
/// used by STEP export (#65) to write genuine BREP rather than tessellated triangles. `None`
/// for a deleted/missing body, an imported-mesh body (no kernel solid), or a body whose
/// geometry isn't fully kernel-representable (the caller then falls back to the mesh path).
#[cfg(feature = "occt")]
pub fn occt_body_shape(doc: &Document, body_index: usize) -> Option<crate::kernel::Shape> {
    let body = doc.bodies.get(body_index)?;
    if body.deleted || body.source.imported_mesh_index().is_some() {
        return None;
    }
    occt_body_shape_from_indices(
        doc,
        body.source.extrusion_indices(),
        body.source.cut_extrusion_indices(),
    )
}

/// Linear tessellation deflection (mm) for OCCT meshing (#86). Flat prism faces
/// triangulate exactly regardless; this only bounds the chord error on curved
/// faces once those go through the kernel.
#[cfg(feature = "occt")]
pub const OCCT_DEFLECTION: f32 = 0.05;

/// Build the solid mesh for a single body (by index), or `None` if the body is deleted,
/// missing, or its source feature produces no geometry.
pub fn body_solid_mesh(doc: &Document, body_index: usize) -> Option<SolidMesh> {
    let body = doc.bodies.get(body_index)?;
    if body.deleted {
        return None;
    }
    if let Some(idx) = body.source.imported_mesh_index() {
        let imported = doc.imported_meshes.get(idx)?;
        return (!imported.triangles.is_empty()).then(|| SolidMesh {
            triangles: imported.triangles.clone(),
        });
    }
    // Fuse the body's added extrusions into one real solid via OCCT and subtract its cut
    // extrusions (#86/#35) when they're all kernel-representable; otherwise fall back to
    // per-extrusion meshing below.
    //
    // KNOWN LIMITATION (non-`occt` build): the hand-rolled fallback below cannot perform a
    // solid subtraction, so a body with cut extrusions renders its additive geometry only —
    // the cut is silently ignored. This is resolved once the kernel is the default (#89);
    // until then, cut mode is only *offered* in the GUI when `occt` is compiled in.
    #[cfg(feature = "occt")]
    if let Some(mesh) = occt_body_mesh(
        doc,
        body.source.extrusion_indices(),
        body.source.cut_extrusion_indices(),
    ) {
        return Some(mesh);
    }
    let mut mesh = SolidMesh::default();
    for &ei in body.source.extrusion_indices() {
        let Some(extrusion) = doc.extrusions.get(ei) else {
            continue;
        };
        if extrusion.deleted {
            continue;
        }
        if let Some(solid) = extrusion_mesh(doc, extrusion) {
            mesh.triangles.extend(solid.triangles);
        }
    }
    (!mesh.is_empty()).then_some(mesh)
}

/// Combined solid mesh of every non-deleted body in the document (the geometry an STL/OBJ
/// export should contain). Bodies are concatenated into one triangle soup.
pub fn document_solid_mesh(doc: &Document) -> SolidMesh {
    let mut mesh = SolidMesh::default();
    for bi in 0..doc.bodies.len() {
        if let Some(solid) = body_solid_mesh(doc, bi) {
            mesh.triangles.extend(solid.triangles);
        }
    }
    mesh
}

/// The `(point, normal)` plane an extrusion's top cap should lie in, when its target defines
/// one. A vertex target or a plain typed distance has no such plane.
pub fn target_top_plane(doc: &Document, extrusion: &Extrusion) -> Option<(Vec3, Vec3)> {
    match extrusion.target.as_ref()? {
        ExtrudeTarget::Face(face) => face_plane(doc, face),
        ExtrudeTarget::Plane(index) => {
            let plane = doc.construction_planes.get(*index)?;
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
    if let Some(target) = &extrusion.target {
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
    target: &ExtrudeTarget,
) -> Option<f32> {
    match target {
        ExtrudeTarget::Vertex(point) => {
            let world = constraint_point_world(doc, point.clone())?;
            Some((world - base).dot(normal))
        }
        ExtrudeTarget::Face(face) => {
            let (p, n) = face_plane(doc, face)?;
            plane_axis_distance(base, normal, p, n)
        }
        ExtrudeTarget::Plane(index) => {
            let plane = doc.construction_planes.get(*index)?;
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

fn face_plane(doc: &Document, face: &ExtrudeFace) -> Option<(Vec3, Vec3)> {
    let (center, normal) = face_center_world(doc, face)?;
    Some((center, normal))
}

pub fn constraint_point_world(doc: &Document, point: crate::model::ConstraintPoint) -> Option<Vec3> {
    // A face's own vertex is already a world-space point (#26/#27) — no sketch frame to
    // project through, unlike the other variants below.
    if let crate::model::ConstraintPoint::FaceVertex { face, index } = &point {
        return face_boundary_loop_world(doc, face)?.get(*index).copied();
    }
    let sketch = match point {
        crate::model::ConstraintPoint::LineEndpoint { line, .. } => doc.lines.get(line)?.sketch,
        crate::model::ConstraintPoint::RectCorner { rect, .. } => doc.rects.get(rect)?.sketch,
        crate::model::ConstraintPoint::CircleCenter(circle) => doc.circles.get(circle)?.sketch,
        crate::model::ConstraintPoint::FaceVertex { .. } => unreachable!("handled above"),
    };
    let frame = sketch_geometry_frame(doc, sketch)?;
    let (u, v) = point_uv(doc, sketch, point).ok()?;
    Some(local_to_world(&frame, u, v))
}

/// Gizmo anchor for a set of coplanar faces: the centroid of their centers and the plane
/// normal (the extrusion direction).
pub fn faces_anchor(doc: &Document, faces: &[ExtrudeFace]) -> Option<(Vec3, Vec3)> {
    let mut sum = Vec3::ZERO;
    let mut count = 0u32;
    let mut normal = Vec3::ZERO;
    for face in faces {
        if let Some(center) = face_center_world(doc, face) {
            sum += center.0;
            normal = center.1;
            count += 1;
        }
    }
    (count > 0).then(|| (sum / count as f32, normal))
}

/// World center and normal of a face.
fn face_center_world(doc: &Document, face: &ExtrudeFace) -> Option<(Vec3, Vec3)> {
    match face {
        ExtrudeFace::Rect(i) => {
            let rect = doc.rects.get(*i)?;
            let frame = sketch_geometry_frame(doc, rect.sketch)?;
            Some((
                local_to_world(&frame, rect.x + rect.w * 0.5, rect.y + rect.h * 0.5),
                frame.normal,
            ))
        }
        ExtrudeFace::Circle(i) => {
            let circle = doc.circles.get(*i)?;
            let frame = sketch_geometry_frame(doc, circle.sketch)?;
            Some((local_to_world(&frame, circle.cx, circle.cy), frame.normal))
        }
        ExtrudeFace::Polygon(lines) => {
            let (profile, normal) = polygon_profile_world(doc, lines)?;
            let centroid = profile.iter().copied().sum::<Vec3>() / profile.len() as f32;
            Some((centroid, normal))
        }
        ExtrudeFace::Boolean { .. } => {
            let (profile, normal) = face_profile_world(doc, face)?;
            let centroid = profile.iter().copied().sum::<Vec3>() / profile.len() as f32;
            Some((centroid, normal))
        }
    }
}

/// World-space boundary loop (CCW in the face frame) and outward normal of a face.
pub fn face_profile_world(doc: &Document, face: &ExtrudeFace) -> Option<(Vec<Vec3>, Vec3)> {
    match face {
        ExtrudeFace::Rect(index) => {
            let rect = doc.rects.get(*index)?;
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
            let circle = doc.circles.get(*index)?;
            if circle.deleted {
                return None;
            }
            let frame = sketch_geometry_frame(doc, circle.sketch)?;
            let profile = circle_profile_world(&frame, circle.cx, circle.cy, circle.r);
            Some((profile, frame.normal))
        }
        ExtrudeFace::Polygon(lines) => polygon_profile_world(doc, lines),
        ExtrudeFace::Boolean { .. } => boolean_profile_world(doc, face),
    }
}

/// World-space boundary loop and outward normal of a `Boolean`-combined face (#16/#62):
/// resolves `a`/`b`'s loops in their shared sketch's UV frame (recursively, in case they're
/// themselves `Boolean`), runs [`crate::polygon_boolean::polygon_boolean`], and projects the
/// resulting loop back to world space through that same frame. `None` if the sketch/frame
/// can't be resolved, or the boolean result isn't a single simple polygon loop (see
/// `polygon_boolean`'s module docs for the deliberate scope limits).
fn boolean_profile_world(doc: &Document, face: &ExtrudeFace) -> Option<(Vec<Vec3>, Vec3)> {
    let sketch = crate::actions::extrude_face_sketch(doc, face)?;
    let frame = sketch_geometry_frame(doc, sketch)?;
    let region = extrude_face_uv_loop(doc, sketch, face)?;
    let profile = region.into_iter().map(|(u, v)| local_to_world(&frame, u, v)).collect();
    Some((profile, frame.normal))
}

/// The boundary loop of `face`, in `sketch`'s local UV frame (not world space) — used for the
/// 2D polygon-boolean overlap detection and click resolution in [`overlapping_partner`] and
/// [`resolve_boolean_click`] (#16/#62), and to build [`boolean_profile_world`]. `None` if
/// `face` doesn't belong to `sketch`, its underlying geometry is missing/deleted, or (for
/// `Boolean`) the combination doesn't reduce to a single simple loop.
pub fn extrude_face_uv_loop(
    doc: &Document,
    sketch: crate::model::SketchId,
    face: &ExtrudeFace,
) -> Option<Vec<(f32, f32)>> {
    match face {
        ExtrudeFace::Rect(i) => {
            let rect = doc.rects.get(*i)?;
            if rect.deleted || rect.sketch != sketch {
                return None;
            }
            let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
            Some(vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)])
        }
        ExtrudeFace::Circle(i) => {
            let circle = doc.circles.get(*i)?;
            if circle.deleted || circle.sketch != sketch {
                return None;
            }
            Some(
                (0..CIRCLE_SEGMENTS)
                    .map(|k| {
                        let a = k as f32 / CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
                        (circle.cx + circle.r * a.cos(), circle.cy + circle.r * a.sin())
                    })
                    .collect(),
            )
        }
        ExtrudeFace::Polygon(lines) => {
            let first = doc.lines.get(*lines.first()?)?;
            if first.deleted || first.sketch != sketch {
                return None;
            }
            crate::polygon::loop_vertices_uv(doc, sketch, lines)
        }
        ExtrudeFace::Boolean { op, a, b } => {
            let loop_a = extrude_face_uv_loop(doc, sketch, a)?;
            let loop_b = extrude_face_uv_loop(doc, sketch, b)?;
            crate::polygon_boolean::polygon_boolean(&loop_a, &loop_b, *op)
        }
    }
}

/// Every raw (non-`Boolean`) extrude face belonging to `sketch`: each rect, circle, and
/// closed line-loop polygon (#66) whose owning sketch is `sketch`.
fn raw_faces_in_sketch(doc: &Document, sketch: crate::model::SketchId) -> Vec<ExtrudeFace> {
    let mut out = Vec::new();
    for (i, r) in doc.rects.iter().enumerate() {
        if !r.deleted && r.sketch == sketch {
            out.push(ExtrudeFace::Rect(i));
        }
    }
    for (i, c) in doc.circles.iter().enumerate() {
        if !c.deleted && c.sketch == sketch {
            out.push(ExtrudeFace::Circle(i));
        }
    }
    for lines in crate::polygon::closed_line_loops(doc, sketch) {
        out.push(ExtrudeFace::Polygon(lines));
    }
    out
}

/// If exactly one other raw shape in `face`'s sketch has nonzero-area overlap with it — and no
/// third shape also overlaps that pair — that shape; else `None`. This is the "exactly two
/// overlapping shapes" gate for #16/#62's boolean-region click resolution (see scope note in
/// SPEC.md): a sketch with three or more mutually-overlapping shapes falls back to today's
/// whole-shape picking instead of attempting an N-way arrangement.
pub fn overlapping_partner(
    doc: &Document,
    sketch: crate::model::SketchId,
    face: &ExtrudeFace,
) -> Option<ExtrudeFace> {
    let loop_a = extrude_face_uv_loop(doc, sketch, face)?;
    let mut overlaps: Vec<ExtrudeFace> = Vec::new();
    for other in raw_faces_in_sketch(doc, sketch) {
        if &other == face {
            continue;
        }
        let Some(loop_b) = extrude_face_uv_loop(doc, sketch, &other) else {
            continue;
        };
        // `polygon_boolean`'s own near-zero-area rejection means `Some` here already implies
        // genuine, nonzero-area overlap — no separate area check needed.
        if crate::polygon_boolean::polygon_boolean(&loop_a, &loop_b, crate::model::BooleanOp::Intersection)
            .is_some()
        {
            overlaps.push(other);
            if overlaps.len() > 1 {
                return None;
            }
        }
    }
    (overlaps.len() == 1).then(|| overlaps.remove(0))
}

/// Resolve a click at local UV point `point` against `face` and its unique overlapping
/// `other` into the right atomic boolean region (#16/#62): inside both -> `Intersection`,
/// inside only one -> that one minus the other, inside neither -> `None` (falls back to
/// whole-shape picking of `face` itself).
pub fn resolve_boolean_click(
    doc: &Document,
    sketch: crate::model::SketchId,
    face: &ExtrudeFace,
    other: &ExtrudeFace,
    point: (f32, f32),
) -> Option<ExtrudeFace> {
    let loop_a = extrude_face_uv_loop(doc, sketch, face)?;
    let loop_b = extrude_face_uv_loop(doc, sketch, other)?;
    let in_a = crate::polygon::point_in_polygon_2d(point, &loop_a);
    let in_b = crate::polygon::point_in_polygon_2d(point, &loop_b);
    match (in_a, in_b) {
        (true, true) => Some(ExtrudeFace::Boolean {
            op: crate::model::BooleanOp::Intersection,
            a: Box::new(face.clone()),
            b: Box::new(other.clone()),
        }),
        (true, false) => Some(ExtrudeFace::Boolean {
            op: crate::model::BooleanOp::Difference,
            a: Box::new(face.clone()),
            b: Box::new(other.clone()),
        }),
        (false, true) => Some(ExtrudeFace::Boolean {
            op: crate::model::BooleanOp::Difference,
            a: Box::new(other.clone()),
            b: Box::new(face.clone()),
        }),
        (false, false) => None,
    }
}

/// World-space boundary loop and outward normal of a closed polygon, given its ordered
/// line indices (#66). `None` if any line is missing/deleted or the loop isn't closed.
fn polygon_profile_world(doc: &Document, lines: &[usize]) -> Option<(Vec<Vec3>, Vec3)> {
    let first = doc.lines.get(*lines.first()?)?;
    if first.deleted || lines.iter().any(|&li| doc.lines.get(li).is_none_or(|l| l.deleted)) {
        return None;
    }
    let frame = sketch_geometry_frame(doc, first.sketch)?;
    let vertices_uv = crate::polygon::loop_vertices_uv(doc, first.sketch, lines)?;
    let profile = vertices_uv
        .into_iter()
        .map(|(u, v)| local_to_world(&frame, u, v))
        .collect();
    Some((profile, frame.normal))
}

/// World-space boundary loop of an extrusion cap. `top` selects the offset end
/// (base + distance·normal); otherwise the base end at the sketch plane.
pub fn cap_polygon_world(
    doc: &Document,
    extrusion: usize,
    profile: &ExtrudeFace,
    top: bool,
) -> Option<Vec<Vec3>> {
    let ext = doc.extrusions.get(extrusion)?;
    if ext.deleted || !ext.faces.contains(profile) {
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

/// Number of flat, sketchable side walls of a profile (rectangles have 4, polygons have
/// one per edge; circular profiles are curved and have none).
pub fn side_face_count(profile: &ExtrudeFace) -> usize {
    match profile {
        ExtrudeFace::Rect(_) => 4,
        ExtrudeFace::Circle(_) => 0,
        ExtrudeFace::Polygon(lines) => lines.len(),
        // The resolved edge count depends on the boolean-clipped geometry (Document state),
        // which this function has no access to; sketching on a boolean-derived extrusion's
        // flat side walls isn't offered (documented limitation, mirrors `Circle`'s curved
        // walls above) — the extrusion mesh itself is unaffected (`extrusion_mesh` walks the
        // resolved profile loop directly, not through this count).
        ExtrudeFace::Boolean { .. } => 0,
    }
}

/// World-space quad of an extrusion side wall, swept by `edge` of a polygonal profile.
/// Ordered `[base_a, base_b, top_b, top_a]`. `None` for circular profiles, out-of-range
/// edges, or a deleted/foreign extrusion.
pub fn side_quad_world(
    doc: &Document,
    extrusion: usize,
    profile: &ExtrudeFace,
    edge: usize,
) -> Option<[Vec3; 4]> {
    let ext = doc.extrusions.get(extrusion)?;
    if ext.deleted || !ext.faces.contains(profile) || edge >= side_face_count(profile) {
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

/// Ordered world-space boundary loop of an extrusion-backed body face (#26/#27): dispatches to
/// [`cap_polygon_world`] for `FaceId::ExtrudeCap` and [`side_quad_world`] for
/// `FaceId::ExtrudeSide`, reusing the same analytic geometry sketch-on-face already relies on.
/// `None` for any other `FaceId` variant (construction planes, 2D shapes) — this only serves
/// extrusion body faces, and imported STL/STEP bodies have no `FaceId` of this shape at all.
pub fn face_boundary_loop_world(doc: &Document, face: &FaceId) -> Option<Vec<Vec3>> {
    match face {
        FaceId::ExtrudeCap {
            extrusion,
            profile,
            top,
        } => cap_polygon_world(doc, *extrusion, profile, *top),
        FaceId::ExtrudeSide {
            extrusion,
            profile,
            edge,
        } => side_quad_world(doc, *extrusion, profile, *edge as usize).map(|quad| quad.to_vec()),
        FaceId::Rect(_)
        | FaceId::Circle(_)
        | FaceId::Polygon(_)
        | FaceId::ConstructionPlane(_) => None,
    }
}

fn circle_profile_world(frame: &SketchFrame, cx: f32, cy: f32, r: f32) -> Vec<Vec3> {
    (0..CIRCLE_SEGMENTS)
        .map(|i| {
            let a = i as f32 / CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
            local_to_world(frame, cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// Emit caps + side walls for a simple (possibly concave) profile, given its base loop and
/// the matching `top` loop (one top vertex per base vertex, so the top cap may be slanted).
fn extrude_profile(profile: &[Vec3], top: &[Vec3], triangles: &mut Vec<[Vec3; 3]>) {
    let n = profile.len();
    if n < 3 || top.len() != n {
        return;
    }

    let normal = (profile[1] - profile[0])
        .cross(profile[2] - profile[0])
        .normalize_or_zero();
    let cap_tris = crate::polygon::triangulate_planar(profile, normal);
    for &[a, b, c] in &cap_tris {
        triangles.push([profile[a], profile[c], profile[b]]);
    }
    for &[a, b, c] in &cap_tris {
        triangles.push([top[a], top[b], top[c]]);
    }
    // Side walls (one quad per edge).
    for i in 0..n {
        let j = (i + 1) % n;
        triangles.push([profile[i], profile[j], top[j]]);
        triangles.push([profile[i], top[j], top[i]]);
    }
}

// --- 3D edge chamfer/fillet (#77) ---------------------------------------------------------
//
// A mesh-bevel approximation of a solid-edge chamfer/fillet, scoped to the two edge families
// with a clean analytic definition on a `Rect`/`Polygon` profile: a vertical side-wall-to-
// side-wall edge, and a side-wall-to-cap edge (see `ExtrusionEdgeRef`). There's no BREP kernel
// here (SPEC §3.4/§10), so this doesn't attempt a true tangent-continuous curved surface, and
// it doesn't attempt to blend a shared corner where 3+ treated edges would meet — see
// `edge_treatment_conflicts`.

/// Number of segments used to facet a fillet edge-treatment bevel. Reuses
/// [`crate::model::BEZIER_SEGMENTS`] directly: an edge-treatment fillet is the same
/// cubic-bezier-approximated arc a sketch-vertex fillet uses
/// ([`crate::model::vertex_treatment_geometry`]), just embedded in 3D via [`corner_bevel_3d`]
/// and swept along the edge, so the same faceting density is the natural, consistent choice
/// (mirrors how [`CIRCLE_SEGMENTS`] is this module's own precedent for curve faceting).
pub const EDGE_TREATMENT_FILLET_SEGMENTS: usize = crate::model::BEZIER_SEGMENTS;

/// Truncated points (and, for a fillet, bridging-arc tangent-handle control points) for a
/// chamfer/fillet corner cut at 3D vertex `v`, generalizing
/// [`crate::model::vertex_treatment_geometry`] to arbitrary (non-coplanar) 3D directions.
///
/// `a` and `b` are `v`'s two real neighboring points — the same corner triangle the 2D version
/// takes, just embedded in 3D. Any two rays from a shared point span a flat 2D subspace, so
/// this is an *exact* embedding (angles and distances are preserved, not approximated): `v`,
/// `a`, and `b` are mapped into an orthonormal 2D basis of that subspace, the existing 2D
/// vertex-treatment math runs unchanged, and the results are mapped back into 3D.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CornerBevel3d {
    /// Truncated point along `v` → `a`.
    pub p1: Vec3,
    /// Truncated point along `v` → `b`.
    pub p2: Vec3,
    /// `Some` for a fillet (bridging arc's tangent-handle control points); `None` for a
    /// chamfer (the bridge is the straight segment `p1`–`p2`).
    pub arc: Option<[Vec3; 2]>,
}

/// Computes a [`CornerBevel3d`] at 3D vertex `v`, given its two real neighboring points `a`/`b`.
/// `None` when `amount` isn't positive, either adjacent edge is degenerate, or `v`/`a`/`b` are
/// collinear (no real corner to bevel) — same failure cases as
/// [`crate::model::vertex_treatment_geometry`], which this delegates the actual math to.
pub fn corner_bevel_3d(v: Vec3, a: Vec3, b: Vec3, kind: VertexTreatmentKind, amount: f32) -> Option<CornerBevel3d> {
    let da = a - v;
    let dist_a = da.length();
    let db = b - v;
    let dist_b = db.length();
    if dist_a < 1e-6 || dist_b < 1e-6 {
        return None;
    }
    let e1 = da / dist_a;
    let e2 = (db - e1 * db.dot(e1)).normalize_or_zero();
    if e2.length_squared() < 1e-8 {
        return None; // v, a, b are collinear: no real corner.
    }
    let a_local = (dist_a, 0.0);
    let b_local = (db.dot(e1), db.dot(e2));
    let geom = vertex_treatment_geometry((0.0, 0.0), a_local, b_local, kind, amount)?;
    let to_world = |p: (f32, f32)| v + e1 * p.0 + e2 * p.1;
    Some(CornerBevel3d {
        p1: to_world(geom.p1),
        p2: to_world(geom.p2),
        arc: geom.bezier.map(|[h0, h1]| [to_world(h0), to_world(h1)]),
    })
}

fn cubic_bezier_point_3d(p0: Vec3, c0: Vec3, c1: Vec3, p1: Vec3, t: f32) -> Vec3 {
    let mt = 1.0 - t;
    p0 * (mt * mt * mt) + c0 * (3.0 * mt * mt * t) + c1 * (3.0 * mt * t * t) + p1 * (t * t * t)
}

/// Discretized points tracing a corner bevel from `p1` to `p2`: just the two endpoints for a
/// chamfer (a straight cut), or [`EDGE_TREATMENT_FILLET_SEGMENTS`]` + 1` points sampled from
/// the bridging arc for a fillet.
pub fn sample_corner_bevel(bevel: &CornerBevel3d, kind: VertexTreatmentKind) -> Vec<Vec3> {
    match (kind, bevel.arc) {
        (VertexTreatmentKind::Fillet, Some([h0, h1])) => (0..=EDGE_TREATMENT_FILLET_SEGMENTS)
            .map(|i| {
                cubic_bezier_point_3d(
                    bevel.p1,
                    h0,
                    h1,
                    bevel.p2,
                    i as f32 / EDGE_TREATMENT_FILLET_SEGMENTS as f32,
                )
            })
            .collect(),
        _ => vec![bevel.p1, bevel.p2],
    }
}

/// Which ring (base or top cap) an [`ExtrusionEdgeRef`] touches at a given profile vertex.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum EdgeRing {
    Base,
    Top,
}

/// The `(vertex, ring)` pairs an edge treatment claims on its face's `n`-vertex profile loop.
/// A `Vertical` edge claims its one vertex on both rings (it runs the full height, base to
/// top); a `Cap` edge claims both its endpoint vertices, but only on the ring it touches.
fn touched_vertex_rings(edge: ExtrusionEdgeRef, n: usize) -> [(usize, EdgeRing); 2] {
    match edge {
        ExtrusionEdgeRef::Vertical { edge, .. } => {
            let v = if n == 0 { 0 } else { (edge + 1) % n };
            [(v, EdgeRing::Base), (v, EdgeRing::Top)]
        }
        ExtrusionEdgeRef::Cap { edge, top, .. } => {
            let ring = if top { EdgeRing::Top } else { EdgeRing::Base };
            let e2 = if n == 0 { 0 } else { (edge + 1) % n };
            [(edge, ring), (e2, ring)]
        }
    }
}

/// Whether adding an edge treatment on `new` would make it share a `(vertex, ring)` with an
/// *different* edge already treated on the same face in `existing` — a vertex miter, which
/// this mesh-bevel approximation doesn't attempt to blend (SPEC §3.4: reject rather than try
/// to combine three-or-more bevels at a shared corner). Re-treating the exact same edge (e.g.
/// dragging its amount again) is not a conflict with itself.
pub fn edge_treatment_conflicts(existing: &[EdgeTreatment], new: ExtrusionEdgeRef, n: usize) -> bool {
    if n == 0 {
        return false;
    }
    let new_touch = touched_vertex_rings(new, n);
    existing.iter().any(|t| {
        t.edge.face() == new.face()
            && t.edge != new
            && touched_vertex_rings(t.edge, n)
                .iter()
                .any(|p| new_touch.contains(p))
    })
}

/// Whether `edge` names a currently-treatable analytic edge: `extrusion` exists and isn't
/// deleted, `edge.face()` indexes one of its faces, that face has an analytic (`Rect`/
/// `Polygon`, at least 3 sides) profile — a `Circle` profile has none, see
/// [`side_face_count`] — and `edge`'s own index is in range.
pub fn extrusion_edge_exists(doc: &Document, extrusion: usize, edge: ExtrusionEdgeRef) -> bool {
    let Some(ext) = doc.extrusions.get(extrusion) else {
        return false;
    };
    if ext.deleted {
        return false;
    }
    let Some(face) = ext.faces.get(edge.face()) else {
        return false;
    };
    let n = side_face_count(face);
    if n < 3 {
        return false;
    }
    match edge {
        ExtrusionEdgeRef::Vertical { edge, .. } | ExtrusionEdgeRef::Cap { edge, .. } => edge < n,
    }
}

/// World-space endpoints of every currently-treatable analytic edge in the document (#77): for
/// each non-deleted extrusion's `Rect`/`Polygon` faces, every vertical side edge and every
/// side/cap edge (see [`ExtrusionEdgeRef`]). The chamfer/fillet tool picks from this list
/// directly (rather than the generic mesh-feature-edge extraction used for construction-plane
/// referencing, #31) when no sketch is open, since it needs the structured edge reference, not
/// just two raw points.
pub fn treatable_edges(doc: &Document) -> Vec<(usize, ExtrusionEdgeRef, Vec3, Vec3)> {
    let mut out = Vec::new();
    for (ei, ext) in doc.extrusions.iter().enumerate() {
        if ext.deleted {
            continue;
        }
        for (fi, face) in ext.faces.iter().enumerate() {
            let n = side_face_count(face);
            if n < 3 {
                continue;
            }
            let Some((base, normal)) = face_profile_world(doc, face) else {
                continue;
            };
            let distance = effective_distance(doc, ext);
            let top: Vec<Vec3> = base
                .iter()
                .map(|p| extruded_top_point(doc, ext, normal, *p, distance))
                .collect();
            for edge in 0..n {
                let v = (edge + 1) % n;
                out.push((ei, ExtrusionEdgeRef::Vertical { face: fi, edge }, base[v], top[v]));
                let e2 = (edge + 1) % n;
                out.push((
                    ei,
                    ExtrusionEdgeRef::Cap { face: fi, edge, top: false },
                    base[edge],
                    base[e2],
                ));
                out.push((
                    ei,
                    ExtrusionEdgeRef::Cap { face: fi, edge, top: true },
                    top[edge],
                    top[e2],
                ));
            }
        }
    }
    out
}

/// World-space origin (edge midpoint) and normal (inward bisector of the edge's two adjacent
/// faces, pointing into the material so pulling the gizmo away from the edge increases the
/// amount) for the 3D edge chamfer/fillet gizmo — the 3D analogue of `vertex_treatment_anchor`
/// in `main.rs`. `None` if the edge no longer resolves (deleted extrusion, out-of-range index,
/// or degenerate geometry).
pub fn extrusion_edge_anchor(doc: &Document, extrusion: usize, edge: ExtrusionEdgeRef) -> Option<(Vec3, Vec3)> {
    let ext = doc.extrusions.get(extrusion)?;
    if ext.deleted {
        return None;
    }
    let face = ext.faces.get(edge.face())?;
    let n = side_face_count(face);
    if n < 3 {
        return None;
    }
    let (base, normal) = face_profile_world(doc, face)?;
    let distance = effective_distance(doc, ext);
    let top: Vec<Vec3> = base
        .iter()
        .map(|p| extruded_top_point(doc, ext, normal, *p, distance))
        .collect();
    match edge {
        ExtrusionEdgeRef::Vertical { edge, .. } => {
            if edge >= n {
                return None;
            }
            let v = (edge + 1) % n;
            let prev = (v + n - 1) % n;
            let next = (v + 1) % n;
            let dir_a = (base[prev] - base[v]).normalize_or_zero();
            let dir_b = (base[next] - base[v]).normalize_or_zero();
            let bisector = (dir_a + dir_b).normalize_or_zero();
            if bisector.length_squared() < 1e-8 {
                return None;
            }
            Some(((base[v] + top[v]) * 0.5, bisector))
        }
        ExtrusionEdgeRef::Cap { edge, top: is_top, .. } => {
            if edge >= n {
                return None;
            }
            let e2 = (edge + 1) % n;
            let (ring, other_ring) = if is_top { (&top, &base) } else { (&base, &top) };
            let edge_dir = (ring[e2] - ring[edge]).normalize_or_zero();
            if edge_dir.length_squared() < 1e-8 {
                return None;
            }
            let prev = (edge + n - 1) % n;
            let raw = ring[prev] - ring[edge];
            let inward = (raw - edge_dir * raw.dot(edge_dir)).normalize_or_zero();
            let wall_dir = (other_ring[edge] - ring[edge]).normalize_or_zero();
            let bisector = (inward + wall_dir).normalize_or_zero();
            if bisector.length_squared() < 1e-8 {
                return None;
            }
            Some(((ring[edge] + ring[e2]) * 0.5, bisector))
        }
    }
}

/// Whether `kind`/`amount` would actually produce a non-degenerate bevel at `edge` right now —
/// i.e. [`corner_bevel_3d`] succeeds at every vertex the edge touches. Used to give a precise
/// "corner is degenerate" rejection (mirroring [`crate::model::vertex_treatment_geometry`]'s
/// own failure mode for the 2D case) before [`crate::actions::Action::CommitEdgeTreatment`]
/// stores the treatment, rather than relying on the mesh builder's silent per-treatment
/// fallback (which never panics, but also never reports *why* an edge didn't visibly change).
pub fn edge_treatment_would_bevel(
    doc: &Document,
    extrusion: usize,
    edge: ExtrusionEdgeRef,
    kind: VertexTreatmentKind,
    amount: f32,
) -> bool {
    if !(amount > 0.0) {
        return false;
    }
    let Some(ext) = doc.extrusions.get(extrusion) else {
        return false;
    };
    if ext.deleted {
        return false;
    }
    let Some(face) = ext.faces.get(edge.face()) else {
        return false;
    };
    let n = side_face_count(face);
    if n < 3 {
        return false;
    }
    let Some((base, normal)) = face_profile_world(doc, face) else {
        return false;
    };
    let distance = effective_distance(doc, ext);
    let top: Vec<Vec3> = base
        .iter()
        .map(|p| extruded_top_point(doc, ext, normal, *p, distance))
        .collect();
    match edge {
        ExtrusionEdgeRef::Vertical { edge, .. } => {
            if edge >= n {
                return false;
            }
            let v = (edge + 1) % n;
            let prev = (v + n - 1) % n;
            let next = (v + 1) % n;
            corner_bevel_3d(base[v], base[prev], base[next], kind, amount).is_some()
                && corner_bevel_3d(top[v], top[prev], top[next], kind, amount).is_some()
        }
        ExtrusionEdgeRef::Cap { edge, top: is_top, .. } => {
            if edge >= n {
                return false;
            }
            let e2 = (edge + 1) % n;
            let (ring, other_ring) = if is_top { (&top, &base) } else { (&base, &top) };
            let edge_dir = (ring[e2] - ring[edge]).normalize_or_zero();
            if edge_dir.length_squared() < 1e-8 {
                return false;
            }
            let prev = (edge + n - 1) % n;
            let next = (e2 + 1) % n;
            let inward_at = |vertex: usize, neighbor: usize| -> Option<Vec3> {
                let raw = ring[neighbor] - ring[vertex];
                let rejected = raw - edge_dir * raw.dot(edge_dir);
                (rejected.length_squared() > 1e-8).then(|| rejected.normalize_or_zero())
            };
            let Some(inward1) = inward_at(edge, prev) else {
                return false;
            };
            let Some(inward2) = inward_at(e2, next) else {
                return false;
            };
            let reach1 = (ring[edge] - ring[prev]).length().max(amount * 4.0);
            let reach2 = (ring[e2] - ring[next]).length().max(amount * 4.0);
            let a1 = ring[edge] + inward1 * reach1;
            let a2 = ring[e2] + inward2 * reach2;
            corner_bevel_3d(ring[edge], a1, other_ring[edge], kind, amount).is_some()
                && corner_bevel_3d(ring[e2], a2, other_ring[e2], kind, amount).is_some()
        }
    }
}

/// Returns a clone of `extrusion`'s source extrusion with `treatment` applied (replacing any
/// existing treatment of the same edge, so re-dragging an already-treated edge updates it in
/// place rather than stacking a duplicate). Used both for the live interactive preview (a ghost
/// extrusion fed straight into `extrusion_mesh`, never touching `doc` until commit) and by
/// [`crate::actions::Action::CommitEdgeTreatment`] to build the value it stores.
pub fn extrusion_with_edge_treatment(
    doc: &Document,
    extrusion: usize,
    treatment: EdgeTreatment,
) -> Option<Extrusion> {
    let mut ext = doc.extrusions.get(extrusion)?.clone();
    ext.edge_treatments.retain(|t| t.edge != treatment.edge);
    ext.edge_treatments.push(treatment);
    Some(ext)
}

/// Pushes `tri` oriented so its normal points away from `interior` (a rough interior reference
/// point of the solid) — used throughout the edge-treatment mesh builder below so new geometry
/// doesn't need its winding hand-derived per call site; a triangle's *shape* still has to be
/// right, but which of its two windings gets emitted is corrected here uniformly.
fn push_oriented(triangles: &mut Vec<[Vec3; 3]>, tri: [Vec3; 3], interior: Vec3) {
    let normal = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
    let centroid = (tri[0] + tri[1] + tri[2]) / 3.0;
    if normal.dot(centroid - interior) < 0.0 {
        triangles.push([tri[0], tri[2], tri[1]]);
    } else {
        triangles.push(tri);
    }
}

/// Ear-clips a (possibly non-convex) boundary loop into cap triangles, oriented outward from
/// `interior`. Degenerate (near-zero-area / too-short) boundaries are silently skipped.
fn triangulate_cap(boundary: &[Vec3], interior: Vec3, triangles: &mut Vec<[Vec3; 3]>) {
    if boundary.len() < 3 {
        return;
    }
    let normal = (boundary[1] - boundary[0])
        .cross(boundary[2] - boundary[0])
        .normalize_or_zero();
    if normal.length_squared() < 1e-8 {
        return;
    }
    for &[a, b, c] in &crate::polygon::triangulate_planar(boundary, normal) {
        push_oriented(triangles, [boundary[a], boundary[b], boundary[c]], interior);
    }
}

/// Applies one cap-edge treatment (base or top ring, whichever `ring` is) at polygon edge
/// `edge` (between profile vertices `edge` and `edge + 1`).
///
/// Physically this is subtracting a uniform-cross-section prism (triangular for a chamfer, a
/// quarter-round for a fillet) that runs the *entire* length of the treated edge — so the two
/// endpoint vertices (`edge` and `edge + 1`), which are corners of the *original* box, are cut
/// away entirely: they don't appear anywhere in the treated mesh anymore. That has three
/// knock-on effects, each handled here:
/// 1. The cap ring's boundary loses that vertex, replaced by the single inset point `p1`
///    (spliced into `ring_corners`, consumed by [`triangulate_cap`]).
/// 2. The treated wall itself (`edge`) starts (or ends) at the single raised point `p2`
///    instead (recorded in `wall_own_start`/`wall_own_end`, keyed by the wall/edge index).
/// 3. Each *untreated* neighboring wall that used to share that corner vertex — wall
///    `edge - 1` at the `edge` end, wall `edge + 1` at the `edge + 1` end — loses its own
///    corner too: since the prism's cross-section is the *same* at every point along the
///    treated edge (including right at its ends), the neighboring wall's flat face is
///    "notched" by that same cross-section where the two meet, so the neighbor's corner must
///    be replaced by the *full* sampled bevel run (not just its two endpoints — for a fillet
///    the notch is genuinely curved, since the neighbor wall is flat and the removed material
///    follows the arc all the way to the very end of the treated edge). These are recorded in
///    `neighbor_notch_end`/`neighbor_notch_start`, consumed by the main wall loop in
///    [`extrude_profile_with_treatments`], which triangulates each wall's own (possibly
///    notched, `n`-gon) boundary via [`triangulate_cap`] rather than assuming a plain quad.
///
/// The samples for the neighbor's notch are exactly the bevel face's own end cross-section, so
/// the neighbor wall and the new bevel face share that boundary exactly — no T-junction, no
/// gap, and no extra "return" triangle is needed (the sharp corner point is simply gone).
#[allow(clippy::too_many_arguments)]
fn apply_cap_edge_treatment(
    ring: &[Vec3],
    other_ring: &[Vec3],
    edge: usize,
    kind: VertexTreatmentKind,
    amount: f32,
    n: usize,
    // Whether `ring` is the *top* cap: the wall loop in `extrude_profile_with_treatments`
    // visits the top ring in the opposite spatial sense to the base ring (base_start -> ... ->
    // top_end -> top_start -> close), so a top-ring notch's sample order needs to be the
    // mirror image of a base-ring notch's to still read "outward edge toward the wall level,
    // inward toward the cap level" consistently around that loop.
    ring_is_top: bool,
    ring_corners: &mut [Vec<Vec3>],
    wall_own_start: &mut HashMap<usize, Vec3>,
    wall_own_end: &mut HashMap<usize, Vec3>,
    neighbor_notch_end: &mut HashMap<usize, Vec<Vec3>>,
    neighbor_notch_start: &mut HashMap<usize, Vec<Vec3>>,
    interior: Vec3,
    triangles: &mut Vec<[Vec3; 3]>,
) {
    let e2 = (edge + 1) % n;
    let edge_dir = (ring[e2] - ring[edge]).normalize_or_zero();
    if edge_dir.length_squared() < 1e-8 {
        return;
    }
    // Inward direction within the ring's plane, perpendicular to the treated edge: the
    // direction toward each endpoint's *other* neighbor on the ring, with the component along
    // the treated edge itself removed. Exact for a rectangle; a reasonable approximation for a
    // general (possibly non-right-angle) polygon profile.
    let prev = (edge + n - 1) % n;
    let next = (e2 + 1) % n;
    let inward_at = |vertex: usize, neighbor: usize| -> Option<Vec3> {
        let raw = ring[neighbor] - ring[vertex];
        let rejected = raw - edge_dir * raw.dot(edge_dir);
        (rejected.length_squared() > 1e-8).then(|| rejected.normalize_or_zero())
    };
    let Some(inward1) = inward_at(edge, prev) else {
        return;
    };
    let Some(inward2) = inward_at(e2, next) else {
        return;
    };
    // A synthetic "far point" along the inward direction, just to give `corner_bevel_3d` a
    // sensible clamp bound (its own adjacent cap edge's length, or 4x the amount if that's
    // somehow shorter) — there's no *real* adjacent vertex in this direction to clamp against.
    let reach1 = (ring[edge] - ring[prev]).length().max(amount * 4.0);
    let reach2 = (ring[e2] - ring[next]).length().max(amount * 4.0);
    let a1 = ring[edge] + inward1 * reach1;
    let a2 = ring[e2] + inward2 * reach2;

    let Some(bevel1) = corner_bevel_3d(ring[edge], a1, other_ring[edge], kind, amount) else {
        return;
    };
    let Some(bevel2) = corner_bevel_3d(ring[e2], a2, other_ring[e2], kind, amount) else {
        return;
    };
    let samples1 = sample_corner_bevel(&bevel1, kind); // ordered cap-level (p1) -> wall-level (p2)
    let samples2 = sample_corner_bevel(&bevel2, kind);

    ring_corners[edge] = vec![bevel1.p1];
    ring_corners[e2] = vec![bevel2.p1];
    wall_own_start.insert(edge, bevel1.p2);
    wall_own_end.insert(edge, bevel2.p2);
    // Base-ring notches read forward at the wall's *end* slot and reversed at its *start*
    // slot (see the doc comment above); a top-ring notch is visited in the mirrored spatial
    // sense by the wall loop, so it needs the opposite of each.
    let (mut end_samples, mut start_samples) = (samples1.clone(), samples2.clone());
    if ring_is_top {
        end_samples.reverse();
    } else {
        start_samples.reverse();
    }
    neighbor_notch_end.insert(prev, end_samples);
    neighbor_notch_start.insert(e2, start_samples);

    // Bevel face: a quad strip (one quad for a chamfer) between the cap-level samples and the
    // wall-level samples — the corner geometry repeats uniformly along a straight prism edge,
    // so corresponding sample indices at the two endpoints line up into a valid, non-twisting
    // strip.
    let m = samples1.len().min(samples2.len());
    for k in 0..m.saturating_sub(1) {
        let (c1a, c1b) = (samples1[k], samples1[k + 1]);
        let (c2a, c2b) = (samples2[k], samples2[k + 1]);
        push_oriented(triangles, [c1a, c2a, c2b], interior);
        push_oriented(triangles, [c1a, c2b, c1b], interior);
    }
}

/// Emits caps + side walls for a profile with one or more [`EdgeTreatment`]s applied (#77),
/// generalizing [`extrude_profile`]. `treatments` must already be filtered to this face.
///
/// The core idea: represent each cap ring not as `n` points but as `n` *lists* of points (one
/// per profile vertex, normally a singleton), and each side wall not as a fixed quad but as a
/// general boundary loop triangulated via [`triangulate_cap`]. A vertical-edge treatment
/// replaces its one vertex's contribution with a short bevel run (`[p1, ...arc, p2]`) on *both*
/// rings — the ordinary per-edge wall loop picks that run's endpoints straight up, and a
/// separate pass stitches the small bevel walls between consecutive points of the run itself.
/// A cap-edge treatment instead cuts its two endpoint vertices away entirely — physically, it's
/// subtracting a uniform-cross-section prism that runs the whole length of the edge, so those
/// corner points genuinely don't exist in the result anymore — replacing each with the single
/// inset cap-ring point, the treated wall's own single raised point, and a *notch* (the bevel's
/// full sample run, not just its endpoints) spliced into each untreated neighboring wall that
/// used to share that corner; see [`apply_cap_edge_treatment`] for the full derivation. A given
/// analytic edge conflicting with another at a shared vertex (a vertex miter) is rejected
/// before it ever reaches here — see [`edge_treatment_conflicts`] — so this function doesn't
/// attempt to resolve that itself; if the document somehow holds conflicting treatments anyway
/// it applies them in order, later ones winning at a shared vertex, rather than panicking.
fn extrude_profile_with_treatments(
    base: &[Vec3],
    top: &[Vec3],
    treatments: &[&EdgeTreatment],
    triangles: &mut Vec<[Vec3; 3]>,
) {
    let n = base.len();
    if n < 3 || top.len() != n {
        return;
    }

    let mut vertical: HashMap<usize, (VertexTreatmentKind, f32)> = HashMap::new();
    let mut cap_bottom: HashMap<usize, (VertexTreatmentKind, f32)> = HashMap::new();
    let mut cap_top: HashMap<usize, (VertexTreatmentKind, f32)> = HashMap::new();
    for t in treatments {
        if t.amount <= 0.0 {
            continue;
        }
        match t.edge {
            ExtrusionEdgeRef::Vertical { edge, .. } if edge < n => {
                vertical.insert((edge + 1) % n, (t.kind, t.amount));
            }
            ExtrusionEdgeRef::Cap { edge, top: is_top, .. } if edge < n => {
                if is_top {
                    cap_top.insert(edge, (t.kind, t.amount));
                } else {
                    cap_bottom.insert(edge, (t.kind, t.amount));
                }
            }
            _ => {}
        }
    }
    if vertical.is_empty() && cap_bottom.is_empty() && cap_top.is_empty() {
        extrude_profile(base, top, triangles);
        return;
    }

    let interior = (base.iter().chain(top.iter()).copied().sum::<Vec3>()) / (2 * n) as f32;

    let mut base_corners: Vec<Vec<Vec3>> = Vec::with_capacity(n);
    let mut top_corners: Vec<Vec<Vec3>> = Vec::with_capacity(n);
    for v in 0..n {
        let expanded = vertical.get(&v).and_then(|&(kind, amount)| {
            let prev = (v + n - 1) % n;
            let next = (v + 1) % n;
            let bevel_b = corner_bevel_3d(base[v], base[prev], base[next], kind, amount)?;
            let bevel_t = corner_bevel_3d(top[v], top[prev], top[next], kind, amount)?;
            Some((sample_corner_bevel(&bevel_b, kind), sample_corner_bevel(&bevel_t, kind)))
        });
        match expanded {
            Some((sb, st)) => {
                base_corners.push(sb);
                top_corners.push(st);
            }
            None => {
                base_corners.push(vec![base[v]]);
                top_corners.push(vec![top[v]]);
            }
        }
    }

    // Wall-corner overrides are keyed by the *wall/edge* index, not by the shared vertex: a
    // vertex can be an endpoint of an untreated neighboring wall too, which needs a different
    // treatment (a full notch tracing the bevel, not just its raised corner point — see
    // `apply_cap_edge_treatment`'s doc comment) than the treated wall's own corner.
    let mut base_wall_own_start: HashMap<usize, Vec3> = HashMap::new();
    let mut base_wall_own_end: HashMap<usize, Vec3> = HashMap::new();
    let mut base_notch_end: HashMap<usize, Vec<Vec3>> = HashMap::new();
    let mut base_notch_start: HashMap<usize, Vec<Vec3>> = HashMap::new();
    let mut top_wall_own_start: HashMap<usize, Vec3> = HashMap::new();
    let mut top_wall_own_end: HashMap<usize, Vec3> = HashMap::new();
    let mut top_notch_end: HashMap<usize, Vec<Vec3>> = HashMap::new();
    let mut top_notch_start: HashMap<usize, Vec<Vec3>> = HashMap::new();
    for (&edge, &(kind, amount)) in &cap_bottom {
        apply_cap_edge_treatment(
            base,
            top,
            edge,
            kind,
            amount,
            n,
            false,
            &mut base_corners,
            &mut base_wall_own_start,
            &mut base_wall_own_end,
            &mut base_notch_end,
            &mut base_notch_start,
            interior,
            triangles,
        );
    }
    for (&edge, &(kind, amount)) in &cap_top {
        apply_cap_edge_treatment(
            top,
            base,
            edge,
            kind,
            amount,
            n,
            true,
            &mut top_corners,
            &mut top_wall_own_start,
            &mut top_wall_own_end,
            &mut top_notch_end,
            &mut top_notch_start,
            interior,
            triangles,
        );
    }

    let base_loop: Vec<Vec3> = base_corners.iter().flatten().copied().collect();
    let top_loop: Vec<Vec3> = top_corners.iter().flatten().copied().collect();
    triangulate_cap(&base_loop, interior, triangles);
    triangulate_cap(&top_loop, interior, triangles);

    // Main walls: one per original polygon edge. Ordinarily a plain quad, but a wall next to a
    // treated cap edge gets one (or both) of its corners replaced: a full point (raised/lowered)
    // if *this* wall is itself the treated one, or a full notch run (see doc comment on
    // `apply_cap_edge_treatment`) if it's the untreated neighbor of a treatment at that corner.
    // Triangulated as a general polygon (usually 4 points, more when notched) via
    // `triangulate_cap`, since a double-notched wall isn't a simple quad anymore.
    for e in 0..n {
        let e2 = (e + 1) % n;
        let mut wall_loop = Vec::with_capacity(4);
        match base_wall_own_start.get(&e) {
            Some(&p) => wall_loop.push(p),
            None => match base_notch_start.get(&e) {
                Some(samples) => wall_loop.extend(samples.iter().copied()),
                None => wall_loop.push(*base_corners[e].last().unwrap()),
            },
        }
        match base_wall_own_end.get(&e) {
            Some(&p) => wall_loop.push(p),
            None => match base_notch_end.get(&e) {
                Some(samples) => wall_loop.extend(samples.iter().copied()),
                None => wall_loop.push(*base_corners[e2].first().unwrap()),
            },
        }
        match top_wall_own_end.get(&e) {
            Some(&p) => wall_loop.push(p),
            None => match top_notch_end.get(&e) {
                Some(samples) => wall_loop.extend(samples.iter().copied()),
                None => wall_loop.push(*top_corners[e2].first().unwrap()),
            },
        }
        match top_wall_own_start.get(&e) {
            Some(&p) => wall_loop.push(p),
            None => match top_notch_start.get(&e) {
                Some(samples) => wall_loop.extend(samples.iter().copied()),
                None => wall_loop.push(*top_corners[e].last().unwrap()),
            },
        }
        triangulate_cap(&wall_loop, interior, triangles);
    }

    // Vertical-treatment mini-walls: consecutive pairs within one vertex's own expanded run
    // (its bevel face — a flat quad for a chamfer, a faceted strip for a fillet).
    for v in 0..n {
        let sb = &base_corners[v];
        let st = &top_corners[v];
        if sb.len() < 2 || st.len() != sb.len() {
            continue;
        }
        for k in 0..sb.len() - 1 {
            push_oriented(triangles, [sb[k], sb[k + 1], st[k + 1]], interior);
            push_oriented(triangles, [sb[k], st[k + 1], st[k]], interior);
        }
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
            edge_treatments: Vec::new(),
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
    fn face_boundary_loop_world_returns_cap_and_side_vertices() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 4.0));
        let ext = extrusion(sketch, vec![ExtrudeFace::Rect(0)], 6.0);
        doc.extrusions.push(ext);

        let base_cap = FaceId::ExtrudeCap {
            extrusion: 0,
            profile: ExtrudeFace::Rect(0),
            top: false,
        };
        let loop_vertices = face_boundary_loop_world(&doc, &base_cap).unwrap();
        assert_eq!(loop_vertices, cap_polygon_world(&doc, 0, &ExtrudeFace::Rect(0), false).unwrap());
        assert_eq!(loop_vertices.len(), 4);
        for v in &loop_vertices {
            assert!(v.z.abs() < 1e-4);
        }

        let top_cap = FaceId::ExtrudeCap {
            extrusion: 0,
            profile: ExtrudeFace::Rect(0),
            top: true,
        };
        let top_loop = face_boundary_loop_world(&doc, &top_cap).unwrap();
        for v in &top_loop {
            assert!((v.z - 6.0).abs() < 1e-4);
        }

        let side = FaceId::ExtrudeSide {
            extrusion: 0,
            profile: ExtrudeFace::Rect(0),
            edge: 0,
        };
        let side_loop = face_boundary_loop_world(&doc, &side).unwrap();
        assert_eq!(
            side_loop,
            side_quad_world(&doc, 0, &ExtrudeFace::Rect(0), 0).unwrap().to_vec()
        );
        assert_eq!(side_loop.len(), 4);
    }

    #[test]
    fn face_boundary_loop_world_none_for_construction_plane() {
        let doc = Document::default();
        assert!(face_boundary_loop_world(&doc, &FaceId::ConstructionPlane(0)).is_none());
    }

    #[test]
    fn closed_line_loop_extrudes_to_a_prism_mesh() {
        use crate::model::{Constraint, ConstraintEntity, ConstraintKind, ConstraintPoint, Line, LineEnd};

        let (mut doc, sketch) = sketch_doc();
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines.push(Line::from_local_endpoints(sketch, 10.0, 0.0, 5.0, 8.0));
        doc.lines.push(Line::from_local_endpoints(sketch, 5.0, 8.0, 0.0, 0.0));
        let coincident = |a, b| Constraint {
            sketch,
            kind: ConstraintKind::Coincident {
                a: ConstraintEntity::Point(a),
                b: ConstraintEntity::Point(b),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        };
        let point = |line, end| ConstraintPoint::LineEndpoint { line, end };
        doc.constraints.push(coincident(point(0, LineEnd::End), point(1, LineEnd::Start)));
        doc.constraints.push(coincident(point(1, LineEnd::End), point(2, LineEnd::Start)));
        doc.constraints.push(coincident(point(2, LineEnd::End), point(0, LineEnd::Start)));

        let loops = crate::polygon::closed_line_loops(&doc, sketch);
        assert_eq!(loops.len(), 1);
        let ext = extrusion(sketch, vec![ExtrudeFace::Polygon(loops[0].clone())], 6.0);
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        // A triangular prism: 1 (bottom fan) + 1 (top fan) + 3 sides * 2 = 8 triangles.
        assert_eq!(mesh.triangles.len(), 8);
        let (min, max) = mesh.bounds().unwrap();
        assert!((min.z).abs() < 1e-4 && (max.z - 6.0).abs() < 1e-4, "z [{},{}]", min.z, max.z);
    }

    #[test]
    fn document_solid_mesh_collects_bodies() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 4.0));
        doc.extrusions
            .push(extrusion(sketch, vec![ExtrudeFace::Rect(0)], 6.0));
        doc.bodies.push(crate::model::Body {
            source: crate::model::BodySource::Extrusion(0),
            name: Some("Box".into()),
            deleted: false,
        });
        let combined = document_solid_mesh(&doc);
        assert_eq!(combined.triangles.len(), 12);
        // A deleted body contributes nothing.
        doc.bodies[0].deleted = true;
        assert!(document_solid_mesh(&doc).is_empty());
        assert!(body_solid_mesh(&doc, 0).is_none());
    }

    /// A body built from a 10x10x5 box (extrusion 0) with a 4x4 column (extrusion 1, centered)
    /// cut through it (#35): source `Solid { add: [0], cut: [1] }`.
    fn cut_body_doc() -> Document {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 10.0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 3.0, 3.0, 7.0, 7.0));
        doc.extrusions
            .push(extrusion(sketch, vec![ExtrudeFace::Rect(0)], 5.0));
        doc.extrusions
            .push(extrusion(sketch, vec![ExtrudeFace::Rect(1)], 5.0));
        doc.bodies.push(crate::model::Body {
            source: crate::model::BodySource::Solid {
                add: vec![0],
                cut: vec![1],
            },
            name: None,
            deleted: false,
        });
        doc
    }

    #[cfg(not(feature = "occt"))]
    #[test]
    fn cut_body_renders_additive_geometry_only_without_kernel() {
        // KNOWN LIMITATION (#35/#89): a non-kernel build can't subtract solids, so a body with
        // a cut extrusion falls back to the additive geometry (the 10x10x5 box). It must still
        // produce a mesh (not panic / not return None).
        let doc = cut_body_doc();
        let mesh = body_solid_mesh(&doc, 0).expect("additive fallback mesh");
        assert!(!mesh.is_empty());
        // Just the box: 12 triangles, occupying the full 10x10x5 footprint (the cut is ignored).
        assert_eq!(mesh.triangles.len(), 12);
        let (min, max) = mesh.bounds().unwrap();
        assert!((max.x - min.x - 10.0).abs() < 1e-4 && (max.y - min.y - 10.0).abs() < 1e-4);
        assert!((max.z - min.z - 5.0).abs() < 1e-4);
    }

    #[cfg(feature = "occt")]
    #[test]
    fn occt_cut_body_subtracts_overlapping_extrusion_volume() {
        // A 10x10x5 box (500 mm^3) with a 4x4 column cut clean through it removes 4*4*5 = 80,
        // leaving ~420. The result is meshed via the kernel's Cut boolean (#35); its
        // divergence-theorem volume should match.
        let doc = cut_body_doc();
        let mesh = body_solid_mesh(&doc, 0).expect("occt cut-body mesh");
        let volume = mesh_signed_volume(&mesh).abs();
        assert!(
            (volume - 420.0).abs() < 5.0,
            "cut-body volume {volume}, expected ~420"
        );
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
        let cap = cap_polygon_world(&doc, 0, &ExtrudeFace::Rect(0), true).unwrap();
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
        let base = cap_polygon_world(&doc, 0, &ExtrudeFace::Rect(0), false).unwrap();
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

    // --- 3D edge chamfer/fillet (#77) ---------------------------------------------------

    /// Every edge of a closed mesh should be shared by exactly two triangles (a manifold,
    /// watertight solid) — the strongest generic check available for a hand-derived mesh-bevel
    /// algorithm without visualizing it. Coordinates are snapped to a millimetre/1000 grid so
    /// two triangles' shared edge compares equal despite unrelated floating-point paths.
    #[cfg(not(feature = "occt"))]
    fn assert_watertight(mesh: &SolidMesh) {
        use std::collections::HashMap;
        let key = |p: Vec3| {
            (
                (p.x * 1000.0).round() as i64,
                (p.y * 1000.0).round() as i64,
                (p.z * 1000.0).round() as i64,
            )
        };
        let mut edge_count: HashMap<((i64, i64, i64), (i64, i64, i64)), u32> = HashMap::new();
        for tri in &mesh.triangles {
            for i in 0..3 {
                let a = key(tri[i]);
                let b = key(tri[(i + 1) % 3]);
                assert_ne!(a, b, "degenerate zero-length edge in {tri:?}");
                let e = if a <= b { (a, b) } else { (b, a) };
                *edge_count.entry(e).or_insert(0) += 1;
            }
        }
        for (e, c) in &edge_count {
            assert_eq!(*c, 2, "edge {e:?} used by {c} triangle(s), expected exactly 2 (not watertight)");
        }
    }

    fn box_doc() -> (Document, crate::model::SketchId, Extrusion) {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 10.0));
        let ext = extrusion(sketch, vec![ExtrudeFace::Rect(0)], 5.0);
        (doc, sketch, ext)
    }

    #[test]
    fn corner_bevel_3d_matches_2d_math_when_embedded_flat() {
        // v=(0,0,0), a=(10,0,0), b=(0,10,0): a right-angle corner in the XY plane, chamfer 3 —
        // should match `vertex_treatment_geometry`'s (v=(0,0), a=(10,0), b=(0,10)) exactly.
        let bevel = corner_bevel_3d(
            Vec3::ZERO,
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
            VertexTreatmentKind::Chamfer,
            3.0,
        )
        .unwrap();
        assert!((bevel.p1 - Vec3::new(3.0, 0.0, 0.0)).length() < 1e-4, "{:?}", bevel.p1);
        assert!((bevel.p2 - Vec3::new(0.0, 3.0, 0.0)).length() < 1e-4, "{:?}", bevel.p2);
        assert!(bevel.arc.is_none());
    }

    #[test]
    fn corner_bevel_3d_fillet_has_arc_and_is_none_when_degenerate() {
        let bevel = corner_bevel_3d(
            Vec3::ZERO,
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
            VertexTreatmentKind::Fillet,
            2.0,
        )
        .unwrap();
        assert!(bevel.arc.is_some());
        let samples = sample_corner_bevel(&bevel, VertexTreatmentKind::Fillet);
        assert_eq!(samples.len(), EDGE_TREATMENT_FILLET_SEGMENTS + 1);
        assert!((samples[0] - bevel.p1).length() < 1e-4);
        assert!((*samples.last().unwrap() - bevel.p2).length() < 1e-4);

        // Collinear v/a/b: no real corner.
        assert!(corner_bevel_3d(
            Vec3::ZERO,
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(-5.0, 0.0, 0.0),
            VertexTreatmentKind::Chamfer,
            1.0,
        )
        .is_none());
    }

    // The next several tests assert mesh-bevel-specific triangle counts and removed
    // volumes — only valid for the hand-rolled mesher. Under `--features occt` these
    // extrusions build true BREP fillets/chamfers (#77), a different tessellation and
    // a different (true-arc vs faceted-bezier) removed volume, so they're scoped to
    // the default build; the OCCT path has its own watertightness tests below.
    #[cfg(not(feature = "occt"))]
    #[test]
    fn vertical_edge_chamfer_is_watertight_and_adds_expected_triangles() {
        let (doc, _sketch, mut ext) = box_doc();
        // Vertical edge index 0 sits at profile vertex 1 (see `ExtrusionEdgeRef::Vertical`).
        ext.edge_treatments.push(EdgeTreatment {
            edge: ExtrusionEdgeRef::Vertical { face: 0, edge: 0 },
            kind: VertexTreatmentKind::Chamfer,
            amount: 2.0,
        });
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        assert_watertight(&mesh);
        // Untreated box: 12 triangles. One chamfered vertical corner: caps grow from a
        // quadrilateral (2 tri) to a pentagon (3 tri) each = +2, plus a 2-triangle bevel wall.
        assert_eq!(mesh.triangles.len(), 12 + 2 + 2);
        // The treated corner is cut back, so nothing should reach the original sharp corner
        // at local (10, 0) (profile vertex 1) anymore.
        let cut_corner = Vec3::new(10.0, 0.0, 0.0);
        assert!(mesh.triangles.iter().flatten().all(|p| (*p - cut_corner).length() > 1e-3));
    }

    #[cfg(not(feature = "occt"))]
    #[test]
    fn vertical_edge_fillet_is_watertight_and_adds_expected_triangles() {
        let (doc, _sketch, mut ext) = box_doc();
        ext.edge_treatments.push(EdgeTreatment {
            edge: ExtrusionEdgeRef::Vertical { face: 0, edge: 0 },
            kind: VertexTreatmentKind::Fillet,
            amount: 2.0,
        });
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        assert_watertight(&mesh);
        let m = EDGE_TREATMENT_FILLET_SEGMENTS; // arc has m+1 points, m segments
        let cap_points = 3 + (m + 1); // 3 untouched corners + the filleted corner's run
        let cap_tris_each = cap_points - 2;
        let expected = cap_tris_each * 2 // bottom + top caps
            + 4 * 2 // the 4 original-edge main walls (unchanged count)
            + m * 2; // the fillet's own faceted bevel wall
        assert_eq!(mesh.triangles.len(), expected);
    }

    /// Signed volume of a closed mesh via the divergence theorem
    /// (`sum(dot(a, cross(b, c))) / 6`) — an independent, end-to-end sanity check that a
    /// treated mesh removes (or adds, for a hypothetical future outward bevel) roughly the
    /// expected amount of material, complementing `assert_watertight`'s purely topological check.
    fn mesh_signed_volume(mesh: &SolidMesh) -> f32 {
        mesh.triangles
            .iter()
            .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
            .sum()
    }

    #[cfg(not(feature = "occt"))]
    #[test]
    fn cap_edge_chamfer_is_watertight_and_removes_expected_volume() {
        let (doc, _sketch, mut ext) = box_doc();
        ext.edge_treatments.push(EdgeTreatment {
            edge: ExtrusionEdgeRef::Cap { face: 0, edge: 0, top: false },
            kind: VertexTreatmentKind::Chamfer,
            amount: 2.0,
        });
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        assert_watertight(&mesh);
        // Cap stays a quad (just repositioned, +0); the two neighboring walls each gain one
        // extra triangle from their notch (4 points -> 3 triangles instead of 2, +1 each);
        // plus the bevel's own quad (2 tri). The two corner points cut away entirely (see
        // `apply_cap_edge_treatment`'s doc comment) don't add cap points back.
        assert_eq!(mesh.triangles.len(), 12 + 1 + 1 + 2);
        // Nothing should touch the original sharp bottom-front edge (z = 0, y = 0) anymore.
        assert!(mesh
            .triangles
            .iter()
            .flatten()
            .all(|p| !(p.y.abs() < 1e-3 && p.z.abs() < 1e-3)));
        // A 10x10x5 box (volume 500) with a 2mm chamfer shaved off one 10mm-long bottom edge
        // removes a triangular-prism sliver of volume 0.5 * 2 * 2 * 10 = 20.
        let volume = mesh_signed_volume(&mesh);
        assert!((volume - 480.0).abs() < 1.0, "volume {volume}");
    }

    #[cfg(not(feature = "occt"))]
    #[test]
    fn cap_edge_fillet_on_top_is_watertight_and_removes_expected_volume() {
        let (doc, _sketch, mut ext) = box_doc();
        ext.edge_treatments.push(EdgeTreatment {
            edge: ExtrusionEdgeRef::Cap { face: 0, edge: 2, top: true },
            kind: VertexTreatmentKind::Fillet,
            amount: 1.5,
        });
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        assert_watertight(&mesh);
        // A quarter-circle-ish fillet of radius 1.5 shaves roughly (1 - pi/4) * r^2 * length
        // off the box (500) along the 10mm top edge.
        let removed = (1.0 - std::f32::consts::FRAC_PI_4) * 1.5 * 1.5 * 10.0;
        let volume = mesh_signed_volume(&mesh);
        assert!((volume - (500.0 - removed)).abs() < 0.5, "volume {volume}, removed ~{removed}");
    }

    #[cfg(not(feature = "occt"))]
    #[test]
    fn multiple_non_conflicting_treatments_combine_and_stay_watertight() {
        let (doc, _sketch, mut ext) = box_doc();
        ext.edge_treatments.push(EdgeTreatment {
            edge: ExtrusionEdgeRef::Vertical { face: 0, edge: 0 },
            kind: VertexTreatmentKind::Chamfer,
            amount: 2.0,
        });
        // Edge 2 (opposite side) doesn't touch vertex 1, so it's independent.
        ext.edge_treatments.push(EdgeTreatment {
            edge: ExtrusionEdgeRef::Cap { face: 0, edge: 2, top: false },
            kind: VertexTreatmentKind::Fillet,
            amount: 1.0,
        });
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        assert_watertight(&mesh);
        let volume = mesh_signed_volume(&mesh);
        assert!(volume > 400.0 && volume < 500.0, "volume {volume}");
    }

    // --- OCCT path (#77): true BREP fillets/chamfers replace the mesh-bevel above. ---
    // These don't hard-code triangle counts (OCCT tessellation differs); instead they
    // check the treated solid is watertight (its mesh's divergence-theorem volume
    // matches OCCT's own exact solid volume) and that a treatment removed a sane, small
    // amount of material. Roundness of a fillet can't be verified in a headless env.

    #[cfg(feature = "occt")]
    #[test]
    fn occt_vertical_edge_fillet_is_watertight_and_removes_material() {
        let (doc, _sketch, base) = box_doc();
        let dist = effective_distance(&doc, &base);
        let untreated = occt_extrusion_shape(&doc, &base, dist).unwrap().volume().unwrap();

        let mut ext = base;
        ext.edge_treatments.push(EdgeTreatment {
            edge: ExtrusionEdgeRef::Vertical { face: 0, edge: 0 },
            kind: VertexTreatmentKind::Fillet,
            amount: 2.0,
        });
        let solid_vol = occt_extrusion_shape(&doc, &ext, dist).unwrap().volume().unwrap();
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        let mesh_vol = mesh_signed_volume(&mesh).abs() as f64;
        assert!(mesh_vol.is_finite() && mesh_vol > 0.0, "mesh vol {mesh_vol}");
        // Watertight: the closed mesh's divergence-theorem volume matches the exact solid.
        assert!(
            (mesh_vol - solid_vol).abs() < solid_vol * 2e-2,
            "mesh vol {mesh_vol} vs solid vol {solid_vol}"
        );
        // A fillet removes only a small sliver of the 10x10x5 box.
        assert!(
            solid_vol < untreated && solid_vol > untreated * 0.9,
            "solid {solid_vol}, untreated {untreated}"
        );
    }

    #[cfg(feature = "occt")]
    #[test]
    fn occt_cap_edge_chamfer_is_watertight_and_removes_material() {
        let (doc, _sketch, base) = box_doc();
        let dist = effective_distance(&doc, &base);
        let untreated = occt_extrusion_shape(&doc, &base, dist).unwrap().volume().unwrap();

        let mut ext = base;
        ext.edge_treatments.push(EdgeTreatment {
            edge: ExtrusionEdgeRef::Cap { face: 0, edge: 0, top: false },
            kind: VertexTreatmentKind::Chamfer,
            amount: 2.0,
        });
        let solid_vol = occt_extrusion_shape(&doc, &ext, dist).unwrap().volume().unwrap();
        let mesh = extrusion_mesh(&doc, &ext).unwrap();
        let mesh_vol = mesh_signed_volume(&mesh).abs() as f64;
        assert!(mesh_vol.is_finite() && mesh_vol > 0.0, "mesh vol {mesh_vol}");
        assert!(
            (mesh_vol - solid_vol).abs() < solid_vol * 2e-2,
            "mesh vol {mesh_vol} vs solid vol {solid_vol}"
        );
        // A 2mm chamfer off one 10mm bottom edge removes a ~20mm^3 triangular prism.
        assert!(
            solid_vol < untreated && solid_vol > untreated * 0.9,
            "solid {solid_vol}, untreated {untreated}"
        );
    }

    #[test]
    fn nonpositive_amount_treatment_is_ignored() {
        let (doc, _sketch, mut ext) = box_doc();
        let untreated = extrusion_mesh(&doc, &ext).unwrap().triangles.len();
        ext.edge_treatments.push(EdgeTreatment {
            edge: ExtrusionEdgeRef::Vertical { face: 0, edge: 0 },
            kind: VertexTreatmentKind::Chamfer,
            amount: 0.0,
        });
        assert_eq!(extrusion_mesh(&doc, &ext).unwrap().triangles.len(), untreated);
    }

    #[test]
    fn treatable_edges_enumerates_verticals_and_caps_for_rect_none_for_circle() {
        let (doc, _sketch, ext) = box_doc();
        let mut doc = doc;
        doc.extrusions.push(ext);
        let edges = treatable_edges(&doc);
        // 4 vertical + 4 bottom cap + 4 top cap = 12 for a rectangular profile.
        assert_eq!(edges.len(), 12);
        assert!(edges.iter().all(|(ei, _, _, _)| *ei == 0));

        let (mut cdoc, csketch) = sketch_doc();
        cdoc.circles
            .push(Circle::from_local_center_radius(csketch, 0.0, 0.0, 5.0, 0.0));
        cdoc.extrusions
            .push(extrusion(csketch, vec![ExtrudeFace::Circle(0)], 6.0));
        assert!(treatable_edges(&cdoc).is_empty());
    }

    #[test]
    fn extrusion_edge_anchor_points_at_edge_midpoint() {
        let (mut doc, _sketch, ext) = box_doc();
        doc.extrusions.push(ext);
        // Vertical edge 0 -> profile vertex 1 = local (10, 0); base z=0, top z=5.
        let (origin, normal) =
            extrusion_edge_anchor(&doc, 0, ExtrusionEdgeRef::Vertical { face: 0, edge: 0 })
                .unwrap();
        assert!((origin - Vec3::new(10.0, 0.0, 2.5)).length() < 1e-3, "{origin:?}");
        assert!(normal.length() > 0.9 && normal.length() < 1.1);

        // A deleted extrusion, an out-of-range extrusion index, and an out-of-range edge index
        // all resolve to `None`.
        doc.extrusions[0].deleted = true;
        assert!(
            extrusion_edge_anchor(&doc, 0, ExtrusionEdgeRef::Vertical { face: 0, edge: 0 })
                .is_none()
        );
        doc.extrusions[0].deleted = false;
        assert!(extrusion_edge_anchor(&doc, 7, ExtrusionEdgeRef::Vertical { face: 0, edge: 0 })
            .is_none());
        assert!(
            extrusion_edge_anchor(&doc, 0, ExtrusionEdgeRef::Vertical { face: 0, edge: 9 })
                .is_none()
        );
    }

    #[test]
    fn edge_treatment_conflicts_detects_shared_vertex_not_the_same_edge() {
        let n = 4;
        let existing = vec![EdgeTreatment {
            edge: ExtrusionEdgeRef::Vertical { face: 0, edge: 0 }, // touches vertex 1
            kind: VertexTreatmentKind::Chamfer,
            amount: 2.0,
        }];
        // Cap edge 0 touches vertices 0 and 1 (base ring) -> shares vertex 1 with the vertical.
        assert!(edge_treatment_conflicts(
            &existing,
            ExtrusionEdgeRef::Cap { face: 0, edge: 0, top: false },
            n
        ));
        // Cap edge 1 touches vertices 1 and 2 -> also shares vertex 1.
        assert!(edge_treatment_conflicts(
            &existing,
            ExtrusionEdgeRef::Cap { face: 0, edge: 1, top: false },
            n
        ));
        // Vertical edge 1 touches vertex 2 only -> no conflict.
        assert!(!edge_treatment_conflicts(
            &existing,
            ExtrusionEdgeRef::Vertical { face: 0, edge: 1 },
            n
        ));
        // A top-cap edge sharing the same vertex on a *different* ring doesn't conflict, since
        // the existing vertical treatment already reserves both rings at vertex 1 — wait, it
        // does conflict (vertical reserves top too): edge 0's top-cap also touches vertex 1.
        assert!(edge_treatment_conflicts(
            &existing,
            ExtrusionEdgeRef::Cap { face: 0, edge: 0, top: true },
            n
        ));
        // Re-treating the exact same edge is not a conflict with itself.
        assert!(!edge_treatment_conflicts(
            &existing,
            ExtrusionEdgeRef::Vertical { face: 0, edge: 0 },
            n
        ));
        // A different face entirely never conflicts.
        assert!(!edge_treatment_conflicts(
            &existing,
            ExtrusionEdgeRef::Cap { face: 1, edge: 0, top: false },
            n
        ));
    }

    #[test]
    fn extrusion_edge_exists_checks_range_and_profile_kind() {
        let (doc, _sketch, mut ext) = box_doc();
        let mut doc = doc;
        doc.extrusions.push(ext.clone());
        assert!(extrusion_edge_exists(&doc, 0, ExtrusionEdgeRef::Vertical { face: 0, edge: 3 }));
        assert!(!extrusion_edge_exists(&doc, 0, ExtrusionEdgeRef::Vertical { face: 0, edge: 4 }));
        assert!(!extrusion_edge_exists(&doc, 5, ExtrusionEdgeRef::Vertical { face: 0, edge: 0 }));
        assert!(!extrusion_edge_exists(&doc, 0, ExtrusionEdgeRef::Vertical { face: 1, edge: 0 }));
        ext.deleted = true;
        doc.extrusions[0] = ext;
        assert!(!extrusion_edge_exists(&doc, 0, ExtrusionEdgeRef::Vertical { face: 0, edge: 0 }));
    }

    #[test]
    fn extrusion_with_edge_treatment_replaces_same_edge_rather_than_stacking() {
        let (doc, _sketch, ext) = box_doc();
        let mut doc = doc;
        doc.extrusions.push(ext);
        let edge = ExtrusionEdgeRef::Vertical { face: 0, edge: 0 };
        let once = extrusion_with_edge_treatment(
            &doc,
            0,
            EdgeTreatment { edge, kind: VertexTreatmentKind::Chamfer, amount: 1.0 },
        )
        .unwrap();
        doc.extrusions[0] = once;
        let twice = extrusion_with_edge_treatment(
            &doc,
            0,
            EdgeTreatment { edge, kind: VertexTreatmentKind::Fillet, amount: 3.0 },
        )
        .unwrap();
        assert_eq!(twice.edge_treatments.len(), 1);
        assert_eq!(twice.edge_treatments[0].kind, VertexTreatmentKind::Fillet);
        assert_eq!(twice.edge_treatments[0].amount, 3.0);
    }

    // ---- #16/#62: overlap detection + click resolution scope gate ----

    #[test]
    fn overlapping_partner_finds_the_unique_overlapping_shape() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 10.0));
        doc.circles.push(Circle::from_local_center_radius(sketch, 5.0, 5.0, 3.0, 0.0));
        let partner = overlapping_partner(&doc, sketch, &ExtrudeFace::Rect(0));
        assert_eq!(partner, Some(ExtrudeFace::Circle(0)));
        // Symmetric the other way too.
        let partner_rev = overlapping_partner(&doc, sketch, &ExtrudeFace::Circle(0));
        assert_eq!(partner_rev, Some(ExtrudeFace::Rect(0)));
    }

    #[test]
    fn overlapping_partner_is_none_when_shapes_dont_overlap() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 10.0));
        doc.circles.push(Circle::from_local_center_radius(sketch, 100.0, 100.0, 3.0, 0.0));
        assert_eq!(overlapping_partner(&doc, sketch, &ExtrudeFace::Rect(0)), None);
    }

    #[test]
    fn overlapping_partner_is_none_when_a_third_shape_also_overlaps() {
        // Scope note (#16/#62): if a third shape also overlaps the pair, this feature doesn't
        // apply — fall back to today's whole-shape picking.
        let (mut doc, sketch) = sketch_doc();
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 10.0));
        doc.circles.push(Circle::from_local_center_radius(sketch, 5.0, 5.0, 3.0, 0.0));
        // A second rect also overlapping both the first rect and the circle (a genuine
        // partial overlap via two transversal crossings, not just a shared-corner touch).
        doc.rects.push(Rect::from_local_corners(sketch, 3.0, 3.0, 13.0, 13.0));
        assert_eq!(overlapping_partner(&doc, sketch, &ExtrudeFace::Rect(0)), None);
        assert_eq!(overlapping_partner(&doc, sketch, &ExtrudeFace::Circle(0)), None);
    }

    #[test]
    fn resolve_boolean_click_picks_the_right_atomic_region() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, -20.0, 20.0, 20.0));
        doc.circles.push(Circle::from_local_center_radius(sketch, 0.0, 0.0, 5.0, 0.0));
        let rect = ExtrudeFace::Rect(0);
        let circle = ExtrudeFace::Circle(0);

        // Inside both (right half of the circle, which lies within the rect's x >= 0 span).
        let both = resolve_boolean_click(&doc, sketch, &rect, &circle, (2.0, 0.0));
        assert_eq!(
            both,
            Some(ExtrudeFace::Boolean {
                op: crate::model::BooleanOp::Intersection,
                a: Box::new(rect.clone()),
                b: Box::new(circle.clone()),
            })
        );

        // Inside the rect only (circle doesn't reach x=15).
        let rect_only = resolve_boolean_click(&doc, sketch, &rect, &circle, (15.0, 0.0));
        assert_eq!(
            rect_only,
            Some(ExtrudeFace::Boolean {
                op: crate::model::BooleanOp::Difference,
                a: Box::new(rect.clone()),
                b: Box::new(circle.clone()),
            })
        );

        // Inside the circle only (left half, x < 0, outside the rect).
        let circle_only = resolve_boolean_click(&doc, sketch, &rect, &circle, (-2.0, 0.0));
        assert_eq!(
            circle_only,
            Some(ExtrudeFace::Boolean {
                op: crate::model::BooleanOp::Difference,
                a: Box::new(circle.clone()),
                b: Box::new(rect.clone()),
            })
        );

        // Outside both.
        assert_eq!(resolve_boolean_click(&doc, sketch, &rect, &circle, (-100.0, -100.0)), None);
    }

    #[test]
    fn boolean_face_profile_world_and_side_face_count() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, -20.0, 20.0, 20.0));
        doc.circles.push(Circle::from_local_center_radius(sketch, 0.0, 0.0, 5.0, 0.0));
        let face = ExtrudeFace::Boolean {
            op: crate::model::BooleanOp::Intersection,
            a: Box::new(ExtrudeFace::Rect(0)),
            b: Box::new(ExtrudeFace::Circle(0)),
        };
        let (profile, _normal) = face_profile_world(&doc, &face).expect("resolved loop");
        assert!(profile.len() >= 3);
        // No flat-side-wall sketching offered on boolean-derived profiles (documented
        // limitation) — but this doesn't affect `extrusion_mesh`, which walks the resolved
        // profile loop directly rather than through `side_face_count`.
        assert_eq!(side_face_count(&face), 0);
    }
}
