//! CPU-side scene mesh builder for the GPU viewport.

use crate::actions::SketchSession;
use crate::camera::Camera;
use crate::construction::{
    axis_angle_handle, axis_normal, axis_reference_perp, gizmo_display_offset, global_axis_segment,
    plane_corners, AxisGizmoHit, AXIS_ANGLE_GIZMO_RADIUS_MM, CONSTRUCTION_DASH_GAP_PX,
    CONSTRUCTION_DASH_LENGTH_PX, CONSTRUCTION_RGBA, FACE_HOVER_FILL_MULTIPLIER, PLANE_FILL_RGBA,
    GIZMO_HANDLE_HOVER_RGBA, PLANE_DISPLAY_HALF, PickTargetKind, PlaneEditDependentPreview,
    PlaneReference,
};
use crate::context::selection_highlight_dashed;
use crate::face::{
    circle_world_perimeter, rect_world_corners, rect_world_corners_resolved, sketch_geometry_frame,
};
use crate::hierarchy::SceneElement;
use crate::model::{
    Circle, ConstructionPlane, Document, FaceId, Line, Rect as ModelRect, RectEdge,
};
use crate::hierarchy::ElementVisibility;
use crate::dimensions::{
    dimension_arrow_wing_world, pixels_to_world_distance, LinearDimensionWorldGeom,
    PlanarLabelView, ARROW_LENGTH, ARROW_WING, LINE_WIDTH,
};
use crate::gpu_viewport::dim_labels::ViewportDimLabel;
use crate::selection::SceneSelection;
use eframe::egui::Color32;
use egui::Rect as UiRect;
use glam::{Mat4, Quat, Vec3};

pub const GRID_EXTENT: f32 = 200.0;
pub const GRID_STEP: f32 = 20.0;
/// Brightness multiplier for geometry outside the active sketch (other sketches, planes).
pub const SKETCH_DIMMED: f32 = 0.50;
/// Ground grid and world axes stay readable while sketching.
pub const SKETCH_GROUND_DIMMED: f32 = 0.82;
pub const CIRCLE_SEGMENTS: usize = 96;

/// Fill opacity for substantial sketch faces (matches the CPU painter).
pub const SOLID_FILL_OPACITY: f32 = 0.25;
/// Fill opacity for all-construction sketch shapes (rectangles, circles).
pub const CONSTRUCTION_FILL_OPACITY: f32 = 0.18;
/// Default semi-transparent fill for construction planes.
pub const DEFAULT_CONSTRUCTION_PLANE_OPACITY: f32 = 0.30;
/// Lift plane fills slightly toward the camera so they win over the ground grid.
pub const PLANE_FILL_DEPTH_BIAS: f32 = 0.02;
/// Base depth lift for sketch shape fills toward the camera.
pub const SHAPE_FILL_DEPTH_BIAS_BASE: f32 = 0.04;
/// Per-shape increment so coplanar overlaps resolve stably (higher index wins).
pub const SHAPE_FILL_DEPTH_BIAS_STEP: f32 = 0.008;
/// In-progress previews render above committed geometry.
pub const PREVIEW_FILL_DEPTH_BIAS: f32 = 0.2;
/// Ground grid lines sit on the reference plane so element strokes win overlaps.
pub const GRID_DEPTH_BIAS: f32 = 0.0;
/// Lift strokes toward the camera so lines draw over coplanar face fills and grid.
pub const STROKE_DEPTH_BIAS: f32 = 0.10;
/// Lift construction-plane hover fills above the plane surface (avoids z-fighting).
const HOVER_PLANE_DEPTH_LIFT: f32 = 0.02;

const GIZMO_OFFSET_STROKE_PX: f32 = 2.5;
const GIZMO_OFFSET_STROKE_HOVER_PX: f32 = 4.0;
const GIZMO_ARROW_HEAD_PX: f32 = 8.0;
const GIZMO_ARROW_WING_PX: f32 = 4.0;
const GIZMO_HANDLE_RADIUS_PX: f32 = 6.0;
const GIZMO_HOVER_INNER_RADIUS_PX: f32 = 9.0;
const GIZMO_HOVER_OUTER_RADIUS_PX: f32 = 14.0;
const GIZMO_ANGLE_CIRCLE_SEGMENTS: usize = 48;
const GIZMO_ANGLE_STROKE_PX: f32 = 1.5;
const GIZMO_ANGLE_STROKE_HOVER_PX: f32 = 2.5;
const GIZMO_ANGLE_ARROW_PX: f32 = 5.0;
const GIZMO_ANGLE_WING_PX: f32 = 3.0;
const GIZMO_HANDLE_RING_STROKE_PX: f32 = 1.5;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
}

use crate::gpu_viewport::dim_labels::GpuTextVertex;

#[derive(Clone, Debug, Default)]
pub struct ViewportScene {
    pub vertices: Vec<GpuVertex>,
    /// Ground grid, sketch fills, and standalone lines (drawn first).
    pub indices: Vec<u32>,
    /// Construction-plane fills (drawn after sketch fills, without depth write).
    pub plane_fill_indices: Vec<u32>,
    /// Strokes, selection, hover, and previews (drawn on top of plane fills).
    pub overlay_indices: Vec<u32>,
    pub text_vertices: Vec<GpuTextVertex>,
    pub text_indices: Vec<u32>,
    pub view_proj: Mat4,
    pub clear_color: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct ViewportPalette {
    pub background: Color32,
    pub grid: Color32,
    pub grid_axis: Color32,
    pub x_axis: Color32,
    pub y_axis: Color32,
    pub z_axis: Color32,
    pub rect_line: Color32,
    pub line_stroke: Color32,
    pub preview: Color32,
    pub construction: Color32,
    pub dim_edge_highlight: Color32,
    pub construction_plane_fill: Color32,
    pub construction_plane_opacity: f32,
}

impl Default for ViewportPalette {
    fn default() -> Self {
        Self {
            background: Color32::from_gray(28),
            grid: Color32::from_gray(55),
            grid_axis: Color32::from_gray(90),
            x_axis: Color32::from_rgb(200, 70, 70),
            y_axis: Color32::from_rgb(70, 190, 90),
            z_axis: Color32::from_rgb(80, 140, 230),
            rect_line: Color32::from_rgb(120, 170, 240),
            line_stroke: Color32::from_rgb(180, 140, 240),
            preview: Color32::from_rgb(240, 200, 120),
            construction: CONSTRUCTION_RGBA,
            dim_edge_highlight: Color32::from_rgb(255, 205, 88),
            construction_plane_fill: PLANE_FILL_RGBA,
            construction_plane_opacity: DEFAULT_CONSTRUCTION_PLANE_OPACITY,
        }
    }
}

/// Hover highlight while picking a sketch face or construction-plane reference.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewportHoverHighlight {
    SketchFace(FaceId),
    PickTarget(PickTargetKind),
}

/// Prospective construction plane while creating or editing.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewportPlanePreview {
    pub plane: ConstructionPlane,
    pub dependents: Option<PlaneEditDependentPreview>,
    /// Extra outline while offset/angle dimension inputs are visible.
    pub dim_outline: bool,
}

/// Construction-plane offset/angle gizmo while creating or editing a plane.
#[derive(Clone, Debug)]
pub struct ViewportPlaneGizmo {
    pub reference: PlaneReference,
    pub offset: f32,
    pub angle_deg: f32,
    pub color: Color32,
    pub hover: Option<AxisGizmoHit>,
}

#[derive(Clone, Debug)]
pub struct ViewportSceneInput<'a> {
    pub doc: &'a Document,
    pub cam: &'a Camera,
    pub viewport: UiRect,
    pub palette: ViewportPalette,
    pub sketch_session: Option<SketchSession>,
    pub selection: &'a SceneSelection,
    pub element_visibility: &'a ElementVisibility,
    pub preview_rect: Option<ModelRect>,
    pub preview_line: Option<Line>,
    pub preview_circle: Option<Circle>,
    pub plane_preview: Option<ViewportPlanePreview>,
    pub active_sketch_face: Option<FaceId>,
    pub dimension_labels: &'a [ViewportDimLabel],
    pub dim_label_view: Option<PlanarLabelView>,
    pub dim_annotation_color: Color32,
    pub plane_gizmo: Option<ViewportPlaneGizmo>,
    pub hover_highlight: Option<ViewportHoverHighlight>,
    pub hover_color: Color32,
}

impl ViewportScene {
    pub fn build(input: &ViewportSceneInput<'_>) -> Self {
        let vp = input.cam.view_proj(input.viewport);
        let mut scene = Self {
            view_proj: vp,
            clear_color: color32_to_gpu(input.palette.background),
            ..Default::default()
        };
        let mut mesh = SceneMesh::new(&mut scene);
        let sketch_dimmed = input.sketch_session.is_some();
        mesh.push_ground(
            input.cam,
            input.viewport,
            &vp,
            sketch_dimmed,
            &input.palette,
        );

        for (ri, rect) in input.doc.rects.iter().enumerate() {
            if !input
                .element_visibility
                .effective_visible(input.doc, SceneElement::Rect(ri))
            {
                continue;
            }
            let dim = input.sketch_session.is_some_and(|s| {
                !sketch_rect_is_active(input.doc, s, ri, rect.sketch)
            });
            mesh.push_rect_fill(
                input.doc,
                rect,
                ri,
                input.cam,
                sketch_color(input.palette.rect_line, dim),
                sketch_color(input.palette.construction, dim),
                shape_fill_depth_bias(ri),
            );
        }

        for (ci, circle) in input.doc.circles.iter().enumerate() {
            if !input
                .element_visibility
                .effective_visible(input.doc, SceneElement::Circle(ci))
            {
                continue;
            }
            let dim = input.sketch_session.is_some_and(|s| {
                !sketch_circle_is_active(input.doc, s, ci, circle.sketch)
            });
            mesh.push_circle_fill(
                input.doc,
                circle,
                ci,
                input.cam,
                sketch_color(input.palette.rect_line, dim),
                sketch_color(input.palette.construction, dim),
                shape_fill_depth_bias(ci),
            );
        }

        let mut plane_draws: Vec<(usize, ConstructionPlane, Color32, f32)> = Vec::new();
        for (i, plane) in input.doc.construction_planes.iter().enumerate() {
            if !input
                .element_visibility
                .effective_visible(input.doc, SceneElement::ConstructionPlane(i))
            {
                continue;
            }
            let session_face = input
                .sketch_session
                .and_then(|s| input.doc.sketch_face(s.sketch));
            let active = session_face == Some(FaceId::ConstructionPlane(i));
            let color = if active {
                input.palette.dim_edge_highlight
            } else {
                input.palette.construction_plane_fill
            };
            plane_draws.push((i, plane.clone(), color, plane_camera_depth(plane, input.cam)));
        }
        plane_draws.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
        mesh.set_index_layer(MeshIndexLayer::PlaneFill);
        let plane_opacity = input.palette.construction_plane_opacity;
        for (i, plane, color, _) in plane_draws {
            mesh.push_plane(&plane, i, color, plane_opacity, input.cam);
        }
        mesh.set_index_layer(MeshIndexLayer::Base);

        for (li, line) in input.doc.lines.iter().enumerate() {
            if !input
                .element_visibility
                .effective_visible(input.doc, SceneElement::Line(li))
            {
                continue;
            }
            let dim = input.sketch_session.is_some_and(|s| line.sketch != s.sketch);
            let color = if line.construction {
                sketch_color(input.palette.construction, dim)
            } else {
                sketch_color(input.palette.line_stroke, dim)
            };
            if let Some((a, b)) = line_world_endpoints(input.doc, line) {
                if line.construction {
                    mesh.push_dashed_line_segment(
                        a,
                        b,
                        color,
                        2.0,
                        input.cam,
                        input.viewport,
                        &vp,
                    );
                } else {
                    mesh.push_line_segment(a, b, color, 2.0, input.cam, input.viewport, &vp);
                }
            }
        }

        mesh.set_index_layer(MeshIndexLayer::Overlay);
        for (ri, rect) in input.doc.rects.iter().enumerate() {
            if !input
                .element_visibility
                .effective_visible(input.doc, SceneElement::Rect(ri))
            {
                continue;
            }
            let dim = input.sketch_session.is_some_and(|s| {
                !sketch_rect_is_active(input.doc, s, ri, rect.sketch)
            });
            mesh.push_rect_strokes(
                input.doc,
                rect,
                input.cam,
                input.viewport,
                &vp,
                sketch_color(input.palette.rect_line, dim),
                sketch_color(input.palette.construction, dim),
            );
        }
        for (ci, circle) in input.doc.circles.iter().enumerate() {
            if !input
                .element_visibility
                .effective_visible(input.doc, SceneElement::Circle(ci))
            {
                continue;
            }
            let dim = input.sketch_session.is_some_and(|s| {
                !sketch_circle_is_active(input.doc, s, ci, circle.sketch)
            });
            mesh.push_circle_strokes(
                input.doc,
                circle,
                ci,
                input.cam,
                input.viewport,
                &vp,
                sketch_color(input.palette.rect_line, dim),
                sketch_color(input.palette.construction, dim),
            );
        }

        mesh.push_selection(
            input.doc,
            input.selection,
            input.cam,
            input.viewport,
            &vp,
            input.palette.dim_edge_highlight,
        );

        if let Some(face) = input.active_sketch_face {
            mesh.push_face_highlight(
                input.doc,
                face,
                input.palette.dim_edge_highlight,
                input.cam,
            );
        }

        if let Some(rect) = input.preview_rect.as_ref() {
            mesh.push_rect(
                input.doc,
                rect,
                usize::MAX,
                input.cam,
                input.viewport,
                &vp,
                input.palette.preview,
                input.palette.construction,
                PREVIEW_FILL_DEPTH_BIAS,
            );
        }
        if let Some(line) = input.preview_line.as_ref() {
            let color = if line.construction {
                input.palette.construction
            } else {
                input.palette.preview
            };
            if let Some((a, b)) = line_world_endpoints(input.doc, line) {
                if line.construction {
                    mesh.push_dashed_line_segment(
                        a,
                        b,
                        color,
                        2.0,
                        input.cam,
                        input.viewport,
                        &vp,
                    );
                } else {
                    mesh.push_line_segment(a, b, color, 2.0, input.cam, input.viewport, &vp);
                }
            }
        }
        if let Some(circle) = input.preview_circle.as_ref() {
            let solid = if circle.construction {
                input.palette.construction
            } else {
                input.palette.preview
            };
            mesh.push_circle(
                input.doc,
                circle,
                usize::MAX,
                input.cam,
                input.viewport,
                &vp,
                solid,
                input.palette.construction,
                PREVIEW_FILL_DEPTH_BIAS,
            );
        }
        if let Some(preview) = input.plane_preview.as_ref() {
            mesh.push_plane_creation_preview(
                preview,
                input.palette.preview,
                input.palette.dim_edge_highlight,
                input.cam,
                input.viewport,
                &vp,
            );
        }

        if let Some(gizmo) = input.plane_gizmo.as_ref() {
            let project = |w: Vec3| input.cam.project(w, input.viewport, &vp);
            mesh.push_plane_gizmo(gizmo, input.cam, input.viewport, &vp, &project);
        }

        if let Some(hover) = input.hover_highlight.as_ref() {
            mesh.push_hover_highlight(
                input.doc,
                hover,
                input.hover_color,
                input.cam,
                input.viewport,
                &vp,
            );
        }

        if input.dim_label_view.is_some() {
            let project = |w: Vec3| input.cam.project(w, input.viewport, &vp);
            for label in input.dimension_labels {
                push_linear_dimension_world(
                    &mut mesh,
                    &label.world_geom,
                    input.dim_annotation_color,
                    input.cam,
                    input.viewport,
                    &vp,
                    &project,
                );
            }
        }
        drop(mesh);

        for label in input.dimension_labels {
            if !label.text_vertices.is_empty() {
                let base = scene.text_vertices.len() as u32;
                scene.text_vertices.extend_from_slice(&label.text_vertices);
                scene
                    .text_indices
                    .extend(label.text_indices.iter().map(|i| i + base));
            }
        }

        scene
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MeshIndexLayer {
    #[default]
    Base,
    PlaneFill,
    Overlay,
}

pub(crate) struct SceneMesh<'a> {
    scene: &'a mut ViewportScene,
    index_layer: MeshIndexLayer,
}

impl<'a> SceneMesh<'a> {
    fn new(scene: &'a mut ViewportScene) -> Self {
        Self {
            scene,
            index_layer: MeshIndexLayer::Base,
        }
    }

    fn set_index_layer(&mut self, layer: MeshIndexLayer) {
        self.index_layer = layer;
    }

    fn indices_mut(&mut self) -> &mut Vec<u32> {
        match self.index_layer {
            MeshIndexLayer::Base => &mut self.scene.indices,
            MeshIndexLayer::PlaneFill => &mut self.scene.plane_fill_indices,
            MeshIndexLayer::Overlay => &mut self.scene.overlay_indices,
        }
    }

    fn push_vertex(&mut self, position: Vec3, color: Color32) {
        self.scene.vertices.push(GpuVertex {
            position: position.to_array(),
            color: color32_to_gpu(color),
        });
    }

    fn push_triangle(&mut self, a: Vec3, b: Vec3, c: Vec3, color: Color32) {
        let base = self.scene.vertices.len() as u32;
        self.push_vertex(a, color);
        self.push_vertex(b, color);
        self.push_vertex(c, color);
        self.indices_mut()
            .extend_from_slice(&[base, base + 1, base + 2]);
    }

    fn push_quad_fill(&mut self, fill_corners: [Vec3; 4], fill: Color32) {
        self.push_triangle(fill_corners[0], fill_corners[1], fill_corners[2], fill);
        self.push_triangle(fill_corners[0], fill_corners[2], fill_corners[3], fill);
    }

    #[allow(dead_code)]
    fn push_quad(
        &mut self,
        corners: [Vec3; 4],
        fill_corners: [Vec3; 4],
        fill: Color32,
        stroke: Color32,
        stroke_width: f32,
        stroke_dashed: bool,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
    ) {
        self.push_quad_fill(fill_corners, fill);
        for (a, b) in [
            (corners[0], corners[1]),
            (corners[1], corners[2]),
            (corners[2], corners[3]),
            (corners[3], corners[0]),
        ] {
            if stroke_dashed {
                self.push_dashed_line_segment(a, b, stroke, stroke_width, cam, viewport, view_proj);
            } else {
                self.push_line_segment(a, b, stroke, stroke_width, cam, viewport, view_proj);
            }
        }
    }

    pub(crate) fn push_dashed_line_segment(
        &mut self,
        a: Vec3,
        b: Vec3,
        color: Color32,
        width_px: f32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
    ) {
        for (wa, wb) in dashed_world_segments(
            a,
            b,
            CONSTRUCTION_DASH_LENGTH_PX,
            CONSTRUCTION_DASH_GAP_PX,
            cam,
            viewport,
            view_proj,
        ) {
            self.push_line_segment(wa, wb, color, width_px, cam, viewport, view_proj);
        }
    }

    pub(crate) fn push_line_segment(
        &mut self,
        a: Vec3,
        b: Vec3,
        color: Color32,
        width_px: f32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
    ) {
        self.push_line_segment_with_bias(
            a,
            b,
            color,
            width_px,
            cam,
            viewport,
            view_proj,
            STROKE_DEPTH_BIAS,
        );
    }

    pub(crate) fn push_line_segment_with_bias(
        &mut self,
        a: Vec3,
        b: Vec3,
        color: Color32,
        width_px: f32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        depth_bias: f32,
    ) {
        let (a, b) = offset_segment_toward_camera(a, b, cam.eye(), depth_bias);
        let Some(quad) = line_screen_quad(a, b, width_px, cam, viewport, view_proj) else {
            return;
        };
        let base = self.scene.vertices.len() as u32;
        let gpu = color32_to_gpu(color);
        for p in quad {
            self.scene.vertices.push(GpuVertex {
                position: p.to_array(),
                color: gpu,
            });
        }
        self.indices_mut()
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    fn push_ground(
        &mut self,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        dim: bool,
        palette: &ViewportPalette,
    ) {
        let e = GRID_EXTENT;
        let mut t = -e;
        while t <= e + 0.001 {
            let base = if t.abs() < 0.001 {
                palette.grid_axis
            } else {
                palette.grid
            };
            let color = sketch_ground_color(base, dim);
            self.push_line_segment_with_bias(
                Vec3::new(-e, t, 0.0),
                Vec3::new(e, t, 0.0),
                color,
                1.0,
                cam,
                viewport,
                view_proj,
                GRID_DEPTH_BIAS,
            );
            self.push_line_segment_with_bias(
                Vec3::new(t, -e, 0.0),
                Vec3::new(t, e, 0.0),
                color,
                1.0,
                cam,
                viewport,
                view_proj,
                GRID_DEPTH_BIAS,
            );
            t += GRID_STEP;
        }
        self.push_line_segment_with_bias(
            Vec3::ZERO,
            Vec3::new(e, 0.0, 0.0),
            sketch_ground_color(palette.x_axis, dim),
            2.0,
            cam,
            viewport,
            view_proj,
            GRID_DEPTH_BIAS,
        );
        self.push_line_segment_with_bias(
            Vec3::ZERO,
            Vec3::new(0.0, e, 0.0),
            sketch_ground_color(palette.y_axis, dim),
            2.0,
            cam,
            viewport,
            view_proj,
            GRID_DEPTH_BIAS,
        );
        self.push_line_segment_with_bias(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, e),
            sketch_ground_color(palette.z_axis, dim),
            2.0,
            cam,
            viewport,
            view_proj,
            GRID_DEPTH_BIAS,
        );
    }

    fn push_rect_fill(
        &mut self,
        doc: &Document,
        rect: &ModelRect,
        _index: usize,
        cam: &Camera,
        solid: Color32,
        construction: Color32,
        fill_depth_bias: f32,
    ) {
        let Some(corners) = rect_world_corners(doc, rect) else {
            return;
        };
        let Some(frame) = sketch_geometry_frame(doc, rect.sketch) else {
            return;
        };
        let fill_corners =
            offset_corners_toward_camera(corners, frame.normal, cam.eye(), fill_depth_bias);
        let all_construction = rect.all_edges_construction();
        let has_solid_edge = rect.construction_edges.iter().any(|&c| !c);
        if all_construction {
            self.push_quad_fill(
                fill_corners,
                fill_color(construction, CONSTRUCTION_FILL_OPACITY),
            );
        } else if has_solid_edge {
            self.push_quad_fill(fill_corners, fill_color(solid, SOLID_FILL_OPACITY));
        }
    }

    fn push_rect_strokes(
        &mut self,
        doc: &Document,
        rect: &ModelRect,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        solid: Color32,
        construction: Color32,
    ) {
        let all_construction = rect.all_edges_construction();
        let has_solid_edge = rect.construction_edges.iter().any(|&c| !c);
        if !all_construction && !has_solid_edge {
            return;
        }
        for (edge_index, (a, b)) in rect_edge_segments(doc, rect).into_iter().enumerate() {
            let edge = RectEdge::from_index(edge_index);
            if rect.edge_construction(edge) {
                self.push_dashed_line_segment(a, b, construction, 1.5, cam, viewport, view_proj);
            } else {
                self.push_line_segment(a, b, solid, 1.5, cam, viewport, view_proj);
            }
        }
    }

    fn push_rect(
        &mut self,
        doc: &Document,
        rect: &ModelRect,
        index: usize,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        solid: Color32,
        construction: Color32,
        fill_depth_bias: f32,
    ) {
        self.push_rect_fill(doc, rect, index, cam, solid, construction, fill_depth_bias);
        self.push_rect_strokes(doc, rect, cam, viewport, view_proj, solid, construction);
    }

    fn push_circle_fill(
        &mut self,
        doc: &Document,
        circle: &Circle,
        _index: usize,
        cam: &Camera,
        solid: Color32,
        construction: Color32,
        fill_depth_bias: f32,
    ) {
        let Some(perimeter) = circle_world_perimeter(doc, circle, CIRCLE_SEGMENTS) else {
            return;
        };
        let frame = sketch_geometry_frame(doc, circle.sketch).expect("circle sketch frame");
        let eye = cam.eye();
        let center = offset_toward_camera(
            crate::face::local_to_world(&frame, circle.cx, circle.cy),
            frame.normal,
            eye,
            fill_depth_bias,
        );
        let fill = if circle.construction {
            fill_color(construction, CONSTRUCTION_FILL_OPACITY)
        } else {
            fill_color(solid, SOLID_FILL_OPACITY)
        };
        for window in perimeter.windows(2) {
            let a = offset_toward_camera(window[0], frame.normal, eye, fill_depth_bias);
            let b = offset_toward_camera(window[1], frame.normal, eye, fill_depth_bias);
            self.push_triangle(center, a, b, fill);
        }
    }

    fn push_circle_strokes(
        &mut self,
        doc: &Document,
        circle: &Circle,
        _index: usize,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        solid: Color32,
        construction: Color32,
    ) {
        let Some(perimeter) = circle_world_perimeter(doc, circle, CIRCLE_SEGMENTS) else {
            return;
        };
        let stroke = if circle.construction {
            construction
        } else {
            solid
        };
        for window in perimeter.windows(2) {
            if circle.construction {
                self.push_dashed_line_segment(
                    window[0],
                    window[1],
                    stroke,
                    1.5,
                    cam,
                    viewport,
                    view_proj,
                );
            } else {
                self.push_line_segment(window[0], window[1], stroke, 1.5, cam, viewport, view_proj);
            }
        }
    }

    fn push_circle(
        &mut self,
        doc: &Document,
        circle: &Circle,
        index: usize,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        solid: Color32,
        construction: Color32,
        fill_depth_bias: f32,
    ) {
        self.push_circle_fill(doc, circle, index, cam, solid, construction, fill_depth_bias);
        self.push_circle_strokes(doc, circle, index, cam, viewport, view_proj, solid, construction);
    }

    fn push_plane_outline(
        &mut self,
        plane: &ConstructionPlane,
        color: Color32,
        stroke_width: f32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
    ) {
        let corners = plane_corners(plane, PLANE_DISPLAY_HALF);
        self.push_quad_outline(corners, color, stroke_width, cam, viewport, view_proj);
    }

    fn push_quad_outline(
        &mut self,
        corners: [Vec3; 4],
        color: Color32,
        stroke_width: f32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
    ) {
        for (a, b) in [
            (corners[0], corners[1]),
            (corners[1], corners[2]),
            (corners[2], corners[3]),
            (corners[3], corners[0]),
        ] {
            self.push_line_segment(a, b, color, stroke_width, cam, viewport, view_proj);
        }
    }

    fn push_plane_creation_preview(
        &mut self,
        preview: &ViewportPlanePreview,
        preview_color: Color32,
        dim_edge_color: Color32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
    ) {
        const PREVIEW_STROKE: f32 = 2.0;
        self.push_plane_outline(
            &preview.plane,
            preview_color,
            PREVIEW_STROKE,
            cam,
            viewport,
            view_proj,
        );
        if preview.dim_outline {
            self.push_plane_outline(
                &preview.plane,
                dim_edge_color,
                PREVIEW_STROKE,
                cam,
                viewport,
                view_proj,
            );
        }
        let Some(dependents) = preview.dependents.as_ref() else {
            return;
        };
        for (_, plane) in &dependents.planes {
            self.push_plane_outline(plane, preview_color, PREVIEW_STROKE, cam, viewport, view_proj);
        }
        for corners in &dependents.rects {
            self.push_quad_outline(*corners, preview_color, PREVIEW_STROKE, cam, viewport, view_proj);
        }
        for &(a, b) in &dependents.lines {
            self.push_line_segment(a, b, preview_color, PREVIEW_STROKE, cam, viewport, view_proj);
        }
    }

    fn push_plane(
        &mut self,
        plane: &ConstructionPlane,
        index: usize,
        color: Color32,
        opacity: f32,
        cam: &Camera,
    ) {
        let corners = plane_corners(plane, PLANE_DISPLAY_HALF);
        let fill_bias = plane_fill_depth_bias(index);
        let eye = cam.eye();
        let fill_corners = offset_corners_toward_camera(corners, plane.normal, eye, fill_bias);
        let fill = fill_color(color, opacity);
        self.push_quad_fill(fill_corners, fill);
    }

    fn push_selection(
        &mut self,
        doc: &Document,
        selection: &SceneSelection,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        color: Color32,
    ) {
        if selection.is_empty() {
            return;
        }
        for element in selection.iter() {
            let dashed = selection_highlight_dashed(doc, element) == Some(true);
            match element {
                SceneElement::Line(index) => {
                    if let Some(line) = doc.lines.get(index) {
                        if let Some((a, b)) = line_world_endpoints(doc, line) {
                            if dashed {
                                self.push_dashed_line_segment(
                                    a, b, color, 3.0, cam, viewport, view_proj,
                                );
                            } else {
                                self.push_line_segment(a, b, color, 3.0, cam, viewport, view_proj);
                            }
                        }
                    }
                }
                SceneElement::RectEdge(index, edge) => {
                    if let Some(rect) = doc.rects.get(index) {
                        let segments = rect_edge_segments(doc, rect);
                        let (a, b) = segments[edge.index()];
                        if dashed {
                            self.push_dashed_line_segment(
                                a, b, color, 3.0, cam, viewport, view_proj,
                            );
                        } else {
                            self.push_line_segment(a, b, color, 3.0, cam, viewport, view_proj);
                        }
                    }
                }
                SceneElement::Rect(index) => {
                    if let Some(rect) = doc.rects.get(index) {
                        for (edge_index, (a, b)) in
                            rect_edge_segments(doc, rect).into_iter().enumerate()
                        {
                            let edge = RectEdge::from_index(edge_index);
                            let stroke = if rect.edge_construction(edge) {
                                color.gamma_multiply(0.85)
                            } else {
                                color
                            };
                            if dashed && rect.edge_construction(edge) {
                                self.push_dashed_line_segment(
                                    a, b, stroke, 3.0, cam, viewport, view_proj,
                                );
                            } else {
                                self.push_line_segment(a, b, stroke, 3.0, cam, viewport, view_proj);
                            }
                        }
                    }
                }
                SceneElement::Circle(index) => {
                    if let Some(circle) = doc.circles.get(index) {
                        if let Some(perimeter) =
                            circle_world_perimeter(doc, circle, CIRCLE_SEGMENTS)
                        {
                            for window in perimeter.windows(2) {
                                if dashed {
                                    self.push_dashed_line_segment(
                                        window[0],
                                        window[1],
                                        color,
                                        3.0,
                                        cam,
                                        viewport,
                                        view_proj,
                                    );
                                } else {
                                    self.push_line_segment(
                                        window[0],
                                        window[1],
                                        color,
                                        3.0,
                                        cam,
                                        viewport,
                                        view_proj,
                                    );
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn push_face_highlight(
        &mut self,
        doc: &Document,
        face: FaceId,
        color: Color32,
        cam: &Camera,
    ) {
        match face {
            FaceId::ConstructionPlane(index) => {
                if let Some(plane) = doc.construction_planes.get(index) {
                    self.push_construction_plane_hover_fill(plane, index, color, 0.12, cam);
                }
            }
            _ => self.push_sketch_face_hover(doc, face, color, 0.12),
        }
    }

    fn push_construction_plane_hover_fill(
        &mut self,
        plane: &ConstructionPlane,
        index: usize,
        color: Color32,
        fill_multiplier: f32,
        cam: &Camera,
    ) {
        let corners = plane_corners(plane, PLANE_DISPLAY_HALF);
        let bias = plane_fill_depth_bias(index) + HOVER_PLANE_DEPTH_LIFT;
        let fill_corners =
            offset_corners_toward_camera(corners, plane.normal, cam.eye(), bias);
        self.push_quad_fill(fill_corners, color.gamma_multiply(fill_multiplier));
    }

    fn push_sketch_face_hover(
        &mut self,
        doc: &Document,
        face: FaceId,
        color: Color32,
        fill_multiplier: f32,
    ) {
        let fill = color.gamma_multiply(fill_multiplier);
        match face {
            FaceId::Rect(index) => {
                if let Some(rect) = doc.rects.get(index) {
                    if let Some(corners) = rect_world_corners(doc, rect) {
                        self.push_triangle(corners[0], corners[1], corners[2], fill);
                        self.push_triangle(corners[0], corners[2], corners[3], fill);
                    }
                }
            }
            FaceId::Circle(index) => {
                if let Some(circle) = doc.circles.get(index) {
                    if let Some(perimeter) =
                        circle_world_perimeter(doc, circle, CIRCLE_SEGMENTS)
                    {
                        let frame =
                            sketch_geometry_frame(doc, circle.sketch).expect("circle frame");
                        let center = crate::face::local_to_world(&frame, circle.cx, circle.cy);
                        for window in perimeter.windows(2) {
                            self.push_triangle(center, window[0], window[1], fill);
                        }
                    }
                }
            }
            FaceId::ConstructionPlane(_) => {}
        }
    }

    fn push_hover_highlight(
        &mut self,
        doc: &Document,
        hover: &ViewportHoverHighlight,
        color: Color32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
    ) {
        let project = |w: Vec3| cam.project(w, viewport, view_proj);
        match hover {
            ViewportHoverHighlight::SketchFace(face) => match *face {
                FaceId::ConstructionPlane(index) => {
                    if let Some(plane) = doc.construction_planes.get(index) {
                        self.push_construction_plane_hover_fill(
                            plane,
                            index,
                            color,
                            FACE_HOVER_FILL_MULTIPLIER,
                            cam,
                        );
                    }
                }
                _ => {
                    self.push_sketch_face_hover(doc, *face, color, FACE_HOVER_FILL_MULTIPLIER);
                    self.push_sketch_face_hover_border(
                        doc,
                        *face,
                        color,
                        2.0,
                        cam,
                        viewport,
                        view_proj,
                    );
                }
            },
            ViewportHoverHighlight::PickTarget(kind) => {
                self.push_pick_target_highlight(
                    doc,
                    kind,
                    color,
                    cam,
                    viewport,
                    view_proj,
                    &project,
                );
            }
        }
    }

    fn push_sketch_face_hover_border(
        &mut self,
        doc: &Document,
        face: FaceId,
        color: Color32,
        width: f32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
    ) {
        match face {
            FaceId::Rect(index) => {
                if let Some(rect) = doc.rects.get(index) {
                    if let Some(corners) = rect_world_corners(doc, rect) {
                        for (a, b) in [
                            (corners[0], corners[1]),
                            (corners[1], corners[2]),
                            (corners[2], corners[3]),
                            (corners[3], corners[0]),
                        ] {
                            self.push_line_segment(a, b, color, width, cam, viewport, view_proj);
                        }
                    }
                }
            }
            FaceId::Circle(index) => {
                if let Some(circle) = doc.circles.get(index) {
                    if let Some(perimeter) =
                        circle_world_perimeter(doc, circle, CIRCLE_SEGMENTS)
                    {
                        for window in perimeter.windows(2) {
                            self.push_line_segment(
                                window[0],
                                window[1],
                                color,
                                width,
                                cam,
                                viewport,
                                view_proj,
                            );
                        }
                    }
                }
            }
            FaceId::ConstructionPlane(_) => {}
        }
    }

    fn push_pick_target_highlight(
        &mut self,
        doc: &Document,
        kind: &PickTargetKind,
        color: Color32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    ) {
        match kind {
            PickTargetKind::Line(index) => {
                if let Some(line) = doc.lines.get(*index) {
                    if let Some((a, b)) = line_world_endpoints(doc, line) {
                        self.push_segment_hover(a, b, color, cam, viewport, view_proj, project);
                    }
                }
            }
            PickTargetKind::Circle(index) => {
                if let Some(circle) = doc.circles.get(*index) {
                    self.push_segment_hover_ring(doc, circle, color, cam, viewport, view_proj);
                }
            }
            PickTargetKind::ShapeEdge { a, b, .. } | PickTargetKind::PlaneEdge { a, b } => {
                self.push_segment_hover(*a, *b, color, cam, viewport, view_proj, project);
            }
            PickTargetKind::GlobalAxis(axis) => {
                let (a, b) = global_axis_segment(*axis);
                let axis_color = axis.color().gamma_multiply(1.25);
                self.push_segment_hover(a, b, axis_color, cam, viewport, view_proj, project);
            }
            PickTargetKind::Rect(rect) => {
                if let Some(corners) = rect_world_corners(doc, rect) {
                    for (a, b) in [
                        (corners[0], corners[1]),
                        (corners[1], corners[2]),
                        (corners[2], corners[3]),
                        (corners[3], corners[0]),
                    ] {
                        self.push_line_segment(a, b, color, 3.0, cam, viewport, view_proj);
                    }
                    for corner in corners {
                        push_screen_disc(
                            self,
                            corner,
                            4.0,
                            color,
                            cam,
                            viewport,
                            view_proj,
                            project,
                        );
                    }
                }
            }
            PickTargetKind::ConstructionPlane(index) => {
                if let Some(plane) = doc.construction_planes.get(*index) {
                    self.push_construction_plane_hover_fill(
                        plane,
                        *index,
                        color,
                        FACE_HOVER_FILL_MULTIPLIER,
                        cam,
                    );
                }
            }
            PickTargetKind::Ground(p) => {
                push_ground_hover_marker(self, *p, color, cam, viewport, view_proj, project);
            }
        }
    }

    fn push_segment_hover(
        &mut self,
        a: Vec3,
        b: Vec3,
        color: Color32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    ) {
        self.push_line_segment(a, b, color, 4.0, cam, viewport, view_proj);
        for p in [a, b] {
            push_screen_disc(self, p, 5.0, color, cam, viewport, view_proj, project);
        }
    }

    fn push_segment_hover_ring(
        &mut self,
        doc: &Document,
        circle: &Circle,
        color: Color32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
    ) {
        if let Some(perimeter) = circle_world_perimeter(doc, circle, CIRCLE_SEGMENTS) {
            for window in perimeter.windows(2) {
                self.push_line_segment(
                    window[0],
                    window[1],
                    color,
                    3.0,
                    cam,
                    viewport,
                    view_proj,
                );
            }
        }
    }
}

fn push_ground_hover_marker(
    mesh: &mut SceneMesh<'_>,
    point: Vec3,
    color: Color32,
    cam: &Camera,
    viewport: UiRect,
    view_proj: &Mat4,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
) {
    push_screen_ring(mesh, point, 8.0, color, 2.0, cam, viewport, view_proj, project);
    let (tangent, bitangent) = camera_disc_basis(point, cam);
    let arm = pixels_to_world_distance(project, point, tangent, 6.0);
    mesh.push_line_segment(
        point - tangent * arm,
        point + tangent * arm,
        color,
        2.0,
        cam,
        viewport,
        view_proj,
    );
    mesh.push_line_segment(
        point - bitangent * arm,
        point + bitangent * arm,
        color,
        2.0,
        cam,
        viewport,
        view_proj,
    );
}

pub fn fill_color(base: Color32, opacity: f32) -> Color32 {
    base.gamma_multiply(opacity)
}

pub fn shape_fill_depth_bias(index: usize) -> f32 {
    SHAPE_FILL_DEPTH_BIAS_BASE + index as f32 * SHAPE_FILL_DEPTH_BIAS_STEP
}

pub fn plane_fill_depth_bias(index: usize) -> f32 {
    PLANE_FILL_DEPTH_BIAS - index as f32 * SHAPE_FILL_DEPTH_BIAS_STEP * 0.25
}

fn plane_camera_depth(plane: &ConstructionPlane, cam: &Camera) -> f32 {
    let corners = plane_corners(plane, PLANE_DISPLAY_HALF);
    let center = (corners[0] + corners[1] + corners[2] + corners[3]) * 0.25;
    (cam.eye() - center).length()
}

fn offset_segment_toward_camera(a: Vec3, b: Vec3, eye: Vec3, bias: f32) -> (Vec3, Vec3) {
    if bias == 0.0 {
        return (a, b);
    }
    let mid = (a + b) * 0.5;
    let to_cam = (eye - mid).normalize_or_zero();
    (a + to_cam * bias, b + to_cam * bias)
}

pub fn offset_toward_camera(pos: Vec3, normal: Vec3, eye: Vec3, bias: f32) -> Vec3 {
    if bias == 0.0 {
        return pos;
    }
    let n = normal.normalize_or_zero();
    if n.length_squared() < 1e-8 {
        return pos;
    }
    let toward_camera = if n.dot(eye - pos) >= 0.0 { n } else { -n };
    pos + toward_camera * bias
}

fn offset_corners_toward_camera(
    corners: [Vec3; 4],
    normal: Vec3,
    eye: Vec3,
    bias: f32,
) -> [Vec3; 4] {
    [
        offset_toward_camera(corners[0], normal, eye, bias),
        offset_toward_camera(corners[1], normal, eye, bias),
        offset_toward_camera(corners[2], normal, eye, bias),
        offset_toward_camera(corners[3], normal, eye, bias),
    ]
}

pub(crate) fn color32_to_gpu(color: Color32) -> [f32; 4] {
    let [r, g, b, a] = color.to_array();
    [
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ]
}

fn push_linear_dimension_world(
    mesh: &mut SceneMesh<'_>,
    world: &LinearDimensionWorldGeom,
    color: Color32,
    cam: &Camera,
    viewport: UiRect,
    view_proj: &Mat4,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
) {
    mesh.push_line_segment(
        world.ext_a_near,
        world.ext_a_far,
        color,
        LINE_WIDTH,
        cam,
        viewport,
        view_proj,
    );
    mesh.push_line_segment(
        world.ext_b_near,
        world.ext_b_far,
        color,
        LINE_WIDTH,
        cam,
        viewport,
        view_proj,
    );
    mesh.push_line_segment(
        world.dim_a,
        world.dim_b,
        color,
        LINE_WIDTH,
        cam,
        viewport,
        view_proj,
    );
    push_arrowhead_world(
        mesh,
        world,
        world.dim_a,
        -world.along_world,
        color,
        cam,
        viewport,
        view_proj,
        project,
    );
    push_arrowhead_world(
        mesh,
        world,
        world.dim_b,
        world.along_world,
        color,
        cam,
        viewport,
        view_proj,
        project,
    );
}

fn push_arrowhead_world(
    mesh: &mut SceneMesh<'_>,
    world: &LinearDimensionWorldGeom,
    tip: Vec3,
    dir: Vec3,
    color: Color32,
    cam: &Camera,
    viewport: UiRect,
    view_proj: &Mat4,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
) {
    let along = dir.normalize_or_zero();
    if along.length_squared() < 1e-8 {
        return;
    }
    let arrow_len = pixels_to_world_distance(project, tip, along, ARROW_LENGTH);
    let arrow_wing = pixels_to_world_distance(project, tip, along, ARROW_WING);
    let mut side = dimension_arrow_wing_world(along, world.outward_world);
    if side.length_squared() < 1e-8 {
        let to_cam = (cam.eye() - tip).normalize_or_zero();
        side = along.cross(to_cam).normalize_or_zero();
    }
    if side.length_squared() < 1e-8 {
        return;
    }
    let base = tip - along * arrow_len;
    mesh.push_line_segment(
        tip,
        base + side * arrow_wing,
        color,
        LINE_WIDTH,
        cam,
        viewport,
        view_proj,
    );
    mesh.push_line_segment(
        tip,
        base - side * arrow_wing,
        color,
        LINE_WIDTH,
        cam,
        viewport,
        view_proj,
    );
}

impl<'a> SceneMesh<'a> {
    fn push_plane_gizmo(
        &mut self,
        gizmo: &ViewportPlaneGizmo,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    ) {
        match &gizmo.reference {
            PlaneReference::Face { origin, normal, .. } => self.push_offset_gizmo(
                *origin,
                *normal,
                gizmo.offset,
                gizmo.color,
                gizmo.hover == Some(AxisGizmoHit::Offset),
                cam,
                viewport,
                view_proj,
                project,
            ),
            PlaneReference::Axis {
                origin,
                direction,
                ..
            } => self.push_axis_plane_gizmo(
                *origin,
                *direction,
                gizmo.offset,
                gizmo.angle_deg,
                gizmo.color,
                gizmo.hover,
                cam,
                viewport,
                view_proj,
                project,
            ),
        }
    }

    fn push_offset_gizmo(
        &mut self,
        origin: Vec3,
        normal: Vec3,
        offset: f32,
        color: Color32,
        hovered: bool,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    ) {
        let n = normal.normalize_or_zero();
        let tip = origin + n * gizmo_display_offset(offset);
        let stroke = if hovered {
            GIZMO_OFFSET_STROKE_HOVER_PX
        } else {
            GIZMO_OFFSET_STROKE_PX
        };
        let stroke_color = if hovered {
            GIZMO_HANDLE_HOVER_RGBA
        } else {
            color
        };
        if project(origin).is_some() && project(tip).is_some() {
            self.push_line_segment(origin, tip, stroke_color, stroke, cam, viewport, view_proj);
            push_gizmo_arrowhead(
                self,
                tip,
                n,
                GIZMO_ARROW_HEAD_PX,
                GIZMO_ARROW_WING_PX,
                stroke,
                stroke_color,
                cam,
                viewport,
                view_proj,
                project,
            );
        }
        if hovered {
            push_gizmo_handle_hover(self, tip, GIZMO_HANDLE_HOVER_RGBA, cam, viewport, view_proj, project);
        } else {
            push_gizmo_handle(self, tip, color, cam, viewport, view_proj, project);
        }
    }

    fn push_axis_plane_gizmo(
        &mut self,
        origin: Vec3,
        direction: Vec3,
        offset: f32,
        angle_deg: f32,
        color: Color32,
        hover: Option<AxisGizmoHit>,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
        project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    ) {
        let normal = axis_normal(direction, angle_deg);
        self.push_offset_gizmo(
            origin,
            normal,
            offset,
            color,
            hover == Some(AxisGizmoHit::Offset),
            cam,
            viewport,
            view_proj,
            project,
        );

        let axis = direction.normalize_or_zero();
        let perp = axis_reference_perp(axis);
        let angle_hovered = hover == Some(AxisGizmoHit::Angle);
        let circle_color = if angle_hovered {
            GIZMO_HANDLE_HOVER_RGBA.gamma_multiply(0.9)
        } else {
            color.gamma_multiply(0.85)
        };
        let circle_stroke = if angle_hovered {
            GIZMO_ANGLE_STROKE_HOVER_PX
        } else {
            GIZMO_ANGLE_STROKE_PX
        };
        let mut prev: Option<Vec3> = None;
        for i in 0..=GIZMO_ANGLE_CIRCLE_SEGMENTS {
            let a = i as f32 / GIZMO_ANGLE_CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
            let dir = Quat::from_axis_angle(axis, a) * perp;
            let pt = origin + dir * AXIS_ANGLE_GIZMO_RADIUS_MM;
            if let Some(p0) = prev {
                self.push_line_segment(p0, pt, circle_color, circle_stroke, cam, viewport, view_proj);
            }
            prev = Some(pt);
        }

        let handle = axis_angle_handle(origin, direction, angle_deg);
        let handle_dir = (handle - origin).normalize_or_zero();
        let tangent = axis.cross(handle_dir).normalize_or_zero();
        let angle_color = if angle_hovered {
            GIZMO_HANDLE_HOVER_RGBA
        } else {
            color
        };
        if angle_hovered {
            push_gizmo_handle_hover(
                self,
                handle,
                GIZMO_HANDLE_HOVER_RGBA,
                cam,
                viewport,
                view_proj,
                project,
            );
        } else {
            push_gizmo_handle(self, handle, color, cam, viewport, view_proj, project);
        }
        let tangent_len = pixels_to_world_distance(project, handle, tangent, GIZMO_ANGLE_ARROW_PX);
        if tangent_len > 1e-6 {
            for sign in [-1.0f32, 1.0] {
                let along = tangent * sign;
                let tip = handle + along * tangent_len;
                push_gizmo_arrowhead(
                    self,
                    tip,
                    along,
                    GIZMO_ANGLE_ARROW_PX,
                    GIZMO_ANGLE_WING_PX,
                    2.0,
                    angle_color,
                    cam,
                    viewport,
                    view_proj,
                    project,
                );
            }
        }
    }
}

fn push_gizmo_arrowhead(
    mesh: &mut SceneMesh<'_>,
    tip: Vec3,
    along_world: Vec3,
    head_px: f32,
    wing_px: f32,
    stroke_px: f32,
    color: Color32,
    cam: &Camera,
    viewport: UiRect,
    view_proj: &Mat4,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
) {
    let along = along_world.normalize_or_zero();
    if along.length_squared() < 1e-8 {
        return;
    }
    let eye = cam.eye();
    let to_cam = (eye - tip).normalize_or_zero();
    let mut side = along.cross(to_cam);
    if side.length_squared() < 1e-8 {
        side = along.cross(cam.view_up_hint());
    }
    if side.length_squared() < 1e-8 {
        return;
    }
    side = side.normalize();
    let arrow_len = pixels_to_world_distance(project, tip, along, head_px);
    let arrow_wing = pixels_to_world_distance(project, tip, side, wing_px);
    let base = tip - along * arrow_len;
    mesh.push_line_segment(
        tip,
        base + side * arrow_wing,
        color,
        stroke_px,
        cam,
        viewport,
        view_proj,
    );
    mesh.push_line_segment(
        tip,
        base - side * arrow_wing,
        color,
        stroke_px,
        cam,
        viewport,
        view_proj,
    );
}

fn push_gizmo_handle(
    mesh: &mut SceneMesh<'_>,
    center: Vec3,
    color: Color32,
    cam: &Camera,
    viewport: UiRect,
    view_proj: &Mat4,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
) {
    push_screen_disc(
        mesh,
        center,
        GIZMO_HANDLE_RADIUS_PX,
        color,
        cam,
        viewport,
        view_proj,
        project,
    );
    push_screen_ring(
        mesh,
        center,
        GIZMO_HANDLE_RADIUS_PX,
        color.gamma_multiply(0.5),
        GIZMO_HANDLE_RING_STROKE_PX,
        cam,
        viewport,
        view_proj,
        project,
    );
}

fn push_gizmo_handle_hover(
    mesh: &mut SceneMesh<'_>,
    center: Vec3,
    accent: Color32,
    cam: &Camera,
    viewport: UiRect,
    view_proj: &Mat4,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
) {
    push_screen_disc(
        mesh,
        center,
        GIZMO_HOVER_INNER_RADIUS_PX,
        accent.gamma_multiply(0.35),
        cam,
        viewport,
        view_proj,
        project,
    );
    push_screen_ring(
        mesh,
        center,
        GIZMO_HOVER_INNER_RADIUS_PX,
        accent,
        2.5,
        cam,
        viewport,
        view_proj,
        project,
    );
    push_screen_ring(
        mesh,
        center,
        GIZMO_HOVER_OUTER_RADIUS_PX,
        accent.gamma_multiply(0.75),
        1.5,
        cam,
        viewport,
        view_proj,
        project,
    );
}

fn push_screen_disc(
    mesh: &mut SceneMesh<'_>,
    center: Vec3,
    radius_px: f32,
    color: Color32,
    cam: &Camera,
    _viewport: UiRect,
    _view_proj: &Mat4,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
) {
    let (tangent, bitangent) = camera_disc_basis(center, cam);
    let radius = pixels_to_world_distance(project, center, tangent, radius_px);
    if radius < 1e-6 {
        return;
    }
    const SEGMENTS: usize = 16;
    let base = mesh.scene.vertices.len() as u32;
    mesh.push_vertex(center, color);
    for i in 0..SEGMENTS {
        let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let p = center + tangent * a.cos() * radius + bitangent * a.sin() * radius;
        mesh.push_vertex(p, color);
    }
    for i in 0..SEGMENTS {
        let next = (i + 1) % SEGMENTS;
        mesh.scene
            .indices
            .extend_from_slice(&[base, base + 1 + i as u32, base + 1 + next as u32]);
    }
}

fn push_screen_ring(
    mesh: &mut SceneMesh<'_>,
    center: Vec3,
    radius_px: f32,
    color: Color32,
    stroke_px: f32,
    cam: &Camera,
    viewport: UiRect,
    view_proj: &Mat4,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
) {
    let (tangent, bitangent) = camera_disc_basis(center, cam);
    let radius = pixels_to_world_distance(project, center, tangent, radius_px);
    if radius < 1e-6 {
        return;
    }
    const SEGMENTS: usize = 24;
    let mut prev: Option<Vec3> = None;
    for i in 0..=SEGMENTS {
        let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let p = center + tangent * a.cos() * radius + bitangent * a.sin() * radius;
        if let Some(p0) = prev {
            mesh.push_line_segment(p0, p, color, stroke_px, cam, viewport, view_proj);
        }
        prev = Some(p);
    }
}

fn camera_disc_basis(center: Vec3, cam: &Camera) -> (Vec3, Vec3) {
    let eye = cam.eye();
    let to_cam = (eye - center).normalize_or_zero();
    let mut tangent = to_cam.cross(Vec3::Z);
    if tangent.length_squared() < 1e-8 {
        tangent = to_cam.cross(Vec3::X);
    }
    tangent = tangent.normalize_or_zero();
    let bitangent = to_cam.cross(tangent).normalize_or_zero();
    (tangent, bitangent)
}

fn sketch_color(color: Color32, dim: bool) -> Color32 {
    if dim {
        color.gamma_multiply(SKETCH_DIMMED)
    } else {
        color
    }
}

pub fn sketch_ground_color(color: Color32, in_sketch: bool) -> Color32 {
    if in_sketch {
        color.gamma_multiply(SKETCH_GROUND_DIMMED)
    } else {
        color
    }
}

fn sketch_rect_is_active(
    doc: &Document,
    session: SketchSession,
    rect_index: usize,
    rect_sketch: crate::model::SketchId,
) -> bool {
    if rect_sketch == session.sketch {
        return true;
    }
    if let Some(FaceId::Rect(face_index)) = doc.sketch_face(session.sketch) {
        return rect_index == face_index;
    }
    false
}

fn sketch_circle_is_active(
    doc: &Document,
    session: SketchSession,
    circle_index: usize,
    circle_sketch: crate::model::SketchId,
) -> bool {
    if circle_sketch == session.sketch {
        return true;
    }
    if let Some(FaceId::Circle(face_index)) = doc.sketch_face(session.sketch) {
        return circle_index == face_index;
    }
    false
}

fn line_world_endpoints(doc: &Document, line: &Line) -> Option<(Vec3, Vec3)> {
    let frame = sketch_geometry_frame(doc, line.sketch)?;
    let a = crate::face::local_to_world(&frame, line.x0, line.y0);
    let b = crate::face::local_to_world(&frame, line.x1, line.y1);
    Some((a, b))
}

fn rect_edge_segments(doc: &Document, rect: &ModelRect) -> [(Vec3, Vec3); 4] {
    let corners = rect_world_corners_resolved(doc, rect);
    [
        (corners[0], corners[1]),
        (corners[1], corners[2]),
        (corners[2], corners[3]),
        (corners[3], corners[0]),
    ]
}

fn world_t_at_screen_fraction(
    a: Vec3,
    b: Vec3,
    fraction: f32,
    cam: &Camera,
    viewport: UiRect,
    view_proj: &Mat4,
) -> Option<f32> {
    let sa = cam.project(a, viewport, view_proj)?;
    let sb = cam.project(b, viewport, view_proj)?;
    let axis = sb - sa;
    let len = axis.length();
    if len < 1e-3 {
        return None;
    }
    let dir = axis / len;
    let target_along = fraction.clamp(0.0, 1.0) * len;
    let mut lo = 0.0f32;
    let mut hi = 1.0f32;
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        let p = cam.project(a.lerp(b, mid), viewport, view_proj)?;
        let along = (p - sa).dot(dir);
        if along < target_along {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some((lo + hi) * 0.5)
}

/// Split a world segment into dash spans using screen-space lengths (matches egui dashed lines).
pub fn dashed_world_segments(
    a: Vec3,
    b: Vec3,
    dash_length_px: f32,
    gap_length_px: f32,
    cam: &Camera,
    viewport: UiRect,
    view_proj: &Mat4,
) -> Vec<(Vec3, Vec3)> {
    let Some(sa) = cam.project(a, viewport, view_proj) else {
        return Vec::new();
    };
    let Some(sb) = cam.project(b, viewport, view_proj) else {
        return Vec::new();
    };
    let len = (sb - sa).length();
    if len < 1e-3 || dash_length_px <= 0.0 {
        return Vec::new();
    }
    let period = (dash_length_px + gap_length_px).max(1e-3);
    let mut segments = Vec::new();
    let mut pos = 0.0f32;
    while pos < len {
        let dash_start = pos;
        let dash_end = (pos + dash_length_px).min(len);
        if dash_end > dash_start + 1e-3 {
            let f0 = dash_start / len;
            let f1 = dash_end / len;
            if let (Some(u0), Some(u1)) = (
                world_t_at_screen_fraction(a, b, f0, cam, viewport, view_proj),
                world_t_at_screen_fraction(a, b, f1, cam, viewport, view_proj),
            ) {
                segments.push((a.lerp(b, u0), a.lerp(b, u1)));
            }
        }
        pos += period;
    }
    segments
}

/// Build a camera-facing line ribbon in world space for a given screen width.
pub fn line_screen_quad(
    a: Vec3,
    b: Vec3,
    width_px: f32,
    cam: &Camera,
    viewport: UiRect,
    view_proj: &Mat4,
) -> Option<[Vec3; 4]> {
    let _ = view_proj;
    let Some(sa) = cam.project(a, viewport, view_proj) else {
        return None;
    };
    let Some(sb) = cam.project(b, viewport, view_proj) else {
        return None;
    };
    if (sa - sb).length() < 1e-3 {
        return None;
    }
    let dir = (b - a).normalize_or_zero();
    if dir.length_squared() < 1e-8 {
        return None;
    }
    let mid = (a + b) * 0.5;
    let eye = cam.eye();
    let to_cam = (eye - mid).normalize_or_zero();
    let mut perp = dir.cross(to_cam);
    if perp.length_squared() < 1e-8 {
        perp = dir.cross(cam.view_up_hint());
    }
    if perp.length_squared() < 1e-8 {
        return None;
    }
    perp = perp.normalize();
    let aspect = (viewport.width() / viewport.height().max(1.0)).max(0.01);
    let (_, half_h) = cam.viewport_half_extents(aspect);
    let world_per_px = 2.0 * half_h / viewport.height().max(1.0);
    let half_width = width_px * 0.5 * world_per_px;
    Some([
        a + perp * half_width,
        a - perp * half_width,
        b - perp * half_width,
        b + perp * half_width,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::AppState;
    use crate::model::{FaceId, RectEdge};
    use egui::Rect as UiRect;

    fn test_viewport() -> UiRect {
        UiRect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(960.0, 560.0))
    }

    #[test]
    fn plane_creation_preview_adds_outline_geometry() {
        let state = AppState::default();
        let cam = state.cam.clone();
        let viewport = test_viewport();
        let base = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport,
            palette: ViewportPalette::default(),
            sketch_session: None,
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        });
        let preview_plane = state.doc.construction_planes[0].clone();
        let with_preview = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport,
            palette: ViewportPalette::default(),
            sketch_session: None,
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: Some(ViewportPlanePreview {
                plane: preview_plane,
                dependents: None,
                dim_outline: false,
            }),
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        });
        assert!(
            with_preview.overlay_indices.len() > base.overlay_indices.len(),
            "plane creation preview should add outline triangles"
        );
    }

    #[test]
    fn hover_highlight_adds_mesh_geometry() {
        let state = AppState::default();
        let cam = state.cam.clone();
        let viewport = test_viewport();
        let base = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport,
            palette: ViewportPalette::default(),
            sketch_session: None,
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: crate::construction::PICK_HOVER_RGBA,
        });
        let with_hover = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport,
            palette: ViewportPalette::default(),
            sketch_session: None,
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: Some(ViewportHoverHighlight::SketchFace(
                FaceId::ConstructionPlane(0),
            )),
            hover_color: crate::construction::PICK_HOVER_RGBA,
        });
        let hover_indices =
            with_hover.overlay_indices.len() - base.overlay_indices.len();
        assert_eq!(
            hover_indices, 6,
            "construction-plane hover should add only a biased fill quad"
        );
    }

    #[test]
    fn plane_gizmo_adds_mesh_geometry() {
        use crate::construction::PlaneReference;

        let state = AppState::default();
        let cam = state.cam.clone();
        let viewport = test_viewport();
        let base = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport,
            palette: ViewportPalette::default(),
            sketch_session: None,
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        });
        let gizmo = ViewportPlaneGizmo {
            reference: PlaneReference::Face {
                origin: Vec3::ZERO,
                normal: Vec3::Z,
                label: "XY".into(),
            },
            offset: 12.0,
            angle_deg: 0.0,
            color: Color32::from_rgb(240, 200, 120),
            hover: None,
        };
        let with_gizmo = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport,
            palette: ViewportPalette::default(),
            sketch_session: None,
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: Some(gizmo),
            hover_highlight: None,
            hover_color: Color32::WHITE,
        });
        assert!(
            with_gizmo.indices.len() > base.indices.len(),
            "plane gizmo should add triangles to the viewport scene"
        );
    }

    #[test]
    fn scene_always_includes_ground_grid_and_clear_color() {
        let state = AppState::default();
        let cam = state.cam.clone();
        let scene = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport: test_viewport(),
            palette: ViewportPalette::default(),
            sketch_session: None,
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        });
        assert!(!scene.vertices.is_empty());
        assert!(!scene.indices.is_empty());
        assert_eq!(scene.clear_color[0], color32_to_gpu(Color32::from_gray(28))[0]);
    }

    fn count_opaque_stroke_vertices(scene: &ViewportScene, stroke: Color32) -> usize {
        let gpu = color32_to_gpu(stroke);
        scene
            .vertices
            .iter()
            .filter(|v| {
                v.color[3] > 0.99
                    && (v.color[0] - gpu[0]).abs() < 0.02
                    && (v.color[1] - gpu[1]).abs() < 0.02
                    && (v.color[2] - gpu[2]).abs() < 0.02
            })
            .count()
    }

    fn build_scene_for_doc(state: &AppState) -> ViewportScene {
        let cam = state.cam.clone();
        ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport: test_viewport(),
            palette: ViewportPalette::default(),
            sketch_session: None,
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        })
    }

    fn commit_test_rectangle(state: &mut AppState) {
        state.apply(crate::actions::Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        state.creating_rect = Some(crate::actions::CreatingRect {
            origin: glam::Vec3::ZERO,
            texts: ["10".into(), "5".into()],
            focused: 0,
            last_mouse: glam::Vec3::new(10.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(crate::actions::Action::CommitRectangle);
    }

    #[test]
    fn construction_planes_render_fill_without_edge_strokes() {
        use crate::hierarchy::SceneElement;

        let mut hidden = AppState::default();
        hidden
            .element_visibility
            .set_visible(SceneElement::ConstructionPlane(0), false);

        let with_plane = build_scene_for_doc(&AppState::default());
        let without_plane = build_scene_for_doc(&hidden);
        let plane_indices =
            with_plane.plane_fill_indices.len() - without_plane.plane_fill_indices.len();
        assert_eq!(
            plane_indices, 6,
            "each construction plane should add only two fill triangles"
        );
    }

    #[test]
    fn mixed_construction_rect_skips_solid_stroke_on_construction_edges() {
        let mut all_solid = AppState::default();
        commit_test_rectangle(&mut all_solid);
        let solid_scene = build_scene_for_doc(&all_solid);
        let solid_strokes =
            count_opaque_stroke_vertices(&solid_scene, ViewportPalette::default().rect_line);

        let mut mixed = AppState::default();
        commit_test_rectangle(&mut mixed);
        mixed.doc.rects[0].set_edge_construction(RectEdge::Bottom, true);
        assert!(mixed.doc.rects[0].has_mixed_edge_construction());
        let mixed_scene = build_scene_for_doc(&mixed);
        let mixed_strokes =
            count_opaque_stroke_vertices(&mixed_scene, ViewportPalette::default().rect_line);

        assert_eq!(solid_strokes, 16, "all-solid rect draws four 4-vertex line quads");
        assert_eq!(
            mixed_strokes, 12,
            "mixed rect should draw solid strokes only on non-construction edges"
        );
    }

    #[test]
    fn rectangle_adds_fill_and_edge_triangles() {
        let mut state = AppState::default();
        state.apply(crate::actions::Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        state.creating_rect = Some(crate::actions::CreatingRect {
            origin: glam::Vec3::ZERO,
            texts: ["10".into(), "5".into()],
            focused: 0,
            last_mouse: glam::Vec3::new(10.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(crate::actions::Action::CommitRectangle);
        let cam = state.cam.clone();
        let scene = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport: test_viewport(),
            palette: ViewportPalette::default(),
            sketch_session: None,
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        });
        assert!(scene.vertices.len() >= 8);
        assert!(scene.indices.len() >= 18);
    }

    #[test]
    fn circle_uses_more_segments_than_old_cpu_path() {
        let mut state = AppState::default();
        state.apply(crate::actions::Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        state.creating_circle = Some(crate::actions::CreatingCircle {
            origin: glam::Vec3::ZERO,
            text: "20".into(),
            last_mouse: glam::Vec3::new(10.0, 0.0, 0.0),
            user_edited: true,
            pending_focus: false,
            construction: false,
        });
        state.apply(crate::actions::Action::CommitCircle);
        let cam = state.cam.clone();
        let scene = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport: test_viewport(),
            palette: ViewportPalette::default(),
            sketch_session: None,
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        });
        assert!(scene.indices.len() > CIRCLE_SEGMENTS);
    }

    #[test]
    fn sketch_dimmed_geometry_stays_readable() {
        let base = Color32::from_rgb(120, 170, 240);
        let dimmed = sketch_color(base, true);
        let legacy = base.gamma_multiply(0.28);
        assert!(
            dimmed.r() > legacy.r(),
            "outside-sketch geometry should be brighter than the old 0.28 multiplier"
        );
        assert!(
            dimmed.r() < base.r(),
            "outside-sketch geometry should still be de-emphasized"
        );
    }

    #[test]
    fn sketch_ground_stays_brighter_than_other_dimmed_geometry() {
        let base = Color32::from_rgb(120, 170, 240);
        let ground = sketch_ground_color(base, true);
        let other = sketch_color(base, true);
        assert!(ground.r() > other.r());
        assert!(ground.r() < base.r());
    }

    #[test]
    fn shape_fill_depth_bias_increases_with_index() {
        assert!(shape_fill_depth_bias(2) > shape_fill_depth_bias(1));
        assert!(shape_fill_depth_bias(1) > shape_fill_depth_bias(0));
        assert!(shape_fill_depth_bias(0) > plane_fill_depth_bias(0));
    }

    #[test]
    fn stroke_depth_bias_beats_shape_fill_bias() {
        assert!(STROKE_DEPTH_BIAS > shape_fill_depth_bias(0));
        assert!(STROKE_DEPTH_BIAS > plane_fill_depth_bias(0));
    }

    #[test]
    fn stroke_depth_bias_beats_grid_depth_bias() {
        assert!(STROKE_DEPTH_BIAS > GRID_DEPTH_BIAS);
    }

    #[test]
    fn element_strokes_sit_closer_to_camera_than_coplanar_grid() {
        let cam = Camera::default();
        let eye = cam.eye();
        let on_plane = Vec3::new(10.0, 10.0, 0.0);
        let grid = offset_toward_camera(on_plane, Vec3::Z, eye, GRID_DEPTH_BIAS);
        let (stroke_a, _) =
            offset_segment_toward_camera(on_plane, on_plane + Vec3::X, eye, STROKE_DEPTH_BIAS);
        assert!(
            (eye - stroke_a).length() < (eye - grid).length(),
            "element strokes should render above coplanar grid lines"
        );
    }

    #[test]
    fn line_segments_are_biased_toward_camera_over_coplanar_fills() {
        let cam = Camera::default();
        let eye = cam.eye();
        let on_plane = Vec3::new(10.0, 0.0, 0.0);
        let fill = offset_toward_camera(on_plane, Vec3::Z, eye, shape_fill_depth_bias(0));
        let (stroke_a, stroke_b) =
            offset_segment_toward_camera(on_plane, on_plane + Vec3::X, eye, STROKE_DEPTH_BIAS);
        let fill_dist = (eye - fill).length();
        let stroke_dist = (eye - stroke_a).length();
        assert!(
            stroke_dist < fill_dist,
            "strokes should sit closer to the camera than coplanar face fills"
        );
        assert_eq!(stroke_a.z, stroke_b.z);
    }

    #[test]
    fn shape_fills_sit_above_coplanar_plane_toward_camera() {
        let cam = Camera::default();
        let eye = cam.eye();
        let on_plane = Vec3::new(10.0, 10.0, 0.0);
        let plane = offset_toward_camera(on_plane, Vec3::Z, eye, plane_fill_depth_bias(0));
        let shape = offset_toward_camera(on_plane, Vec3::Z, eye, shape_fill_depth_bias(0));
        assert!(shape.z > plane.z);
    }

    #[test]
    fn higher_shape_index_wins_coplanar_overlap() {
        let cam = Camera::default();
        let eye = cam.eye();
        let p = Vec3::new(0.0, 0.0, 0.0);
        let a = offset_toward_camera(p, Vec3::Z, eye, shape_fill_depth_bias(0));
        let b = offset_toward_camera(p, Vec3::Z, eye, shape_fill_depth_bias(3));
        assert!(b.z > a.z);
    }

    #[test]
    fn committed_dimension_labels_add_text_and_line_geometry() {
        let mut state = AppState::default();
        state.apply(crate::actions::Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        state.creating_rect = Some(crate::actions::CreatingRect {
            origin: glam::Vec3::ZERO,
            texts: ["40".into(), "20".into()],
            focused: 0,
            last_mouse: glam::Vec3::new(40.0, 20.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(crate::actions::Action::CommitRectangle);
        let session = state.sketch_session.unwrap();
        let cam = state.cam.clone();
        let viewport = test_viewport();
        let vp = cam.view_proj(viewport);
        let project = |w: glam::Vec3| cam.project(w, viewport, &vp);
        let view = crate::dimensions::PlanarLabelView::from_camera_and_plane(&cam, glam::Vec3::Z);
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let constraint = state.doc.constraints.iter().find(|c| c.sketch == session.sketch);
        let constraint = constraint.expect("rectangle should have a width constraint");
        let (a, b) = crate::constraints::constraint_segment_endpoints(&state.doc, 0).unwrap();
        let world = crate::dimensions::linear_dimension_world_geom(
            a,
            b,
            glam::Vec3::Y,
            5.0,
            1.0,
            2.0,
        );
        let label_text = crate::constraints::constraint_evaluated_length(&state.doc, 0)
            .map(crate::value::format_length_display)
            .unwrap();
        let (text_vertices, text_indices) = crate::gpu_viewport::build_planar_label_mesh(
            &ctx,
            &world,
            &view,
            &label_text,
            Color32::WHITE,
            &cam,
            viewport,
            &vp,
            &project,
        );
        let dim_label = crate::gpu_viewport::ViewportDimLabel {
            world_geom: world,
            text_vertices,
            text_indices,
        };
        let vertex_count_before = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport,
            palette: ViewportPalette::default(),
            sketch_session: Some(session),
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: Some(view),
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        })
        .vertices
        .len();
        let scene = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: &cam,
            viewport,
            palette: ViewportPalette::default(),
            sketch_session: Some(session),
            selection: &state.scene_selection,
            element_visibility: &state.element_visibility,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: std::slice::from_ref(&dim_label),
            dim_label_view: Some(view),
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        });
        assert!(!scene.text_vertices.is_empty());
        assert!(!scene.text_indices.is_empty());
        assert!(scene.vertices.len() > vertex_count_before);
        let _ = constraint;
    }

    #[test]
    fn dashed_world_segments_use_six_pixel_dashes_and_four_pixel_gaps() {
        let cam = Camera::default();
        let viewport = test_viewport();
        let vp = cam.view_proj(viewport);
        let a = Vec3::new(-80.0, 5.0, 0.0);
        let b = Vec3::new(80.0, 5.0, 0.0);
        let pa = cam.project(a, viewport, &vp).unwrap();
        let pb = cam.project(b, viewport, &vp).unwrap();
        let screen_len = (pb - pa).length();
        let segments = dashed_world_segments(
            a,
            b,
            CONSTRUCTION_DASH_LENGTH_PX,
            CONSTRUCTION_DASH_GAP_PX,
            &cam,
            viewport,
            &vp,
        );
        let expected = ((screen_len + CONSTRUCTION_DASH_GAP_PX) / 10.0).ceil() as usize;
        assert!(segments.len() >= expected.saturating_sub(1));
        assert!(segments.len() <= expected + 1);
        for (wa, wb) in &segments {
            let wa_s = cam.project(*wa, viewport, &vp).unwrap();
            let wb_s = cam.project(*wb, viewport, &vp).unwrap();
            let dash_px = (wb_s - wa_s).length();
            assert!(dash_px <= CONSTRUCTION_DASH_LENGTH_PX + 0.5);
            assert!(dash_px > 0.5);
        }
    }

    #[test]
    fn construction_line_produces_more_gpu_segments_than_solid_line() {
        let mut state = AppState::default();
        state.apply(crate::actions::Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        let session = state.sketch_session.unwrap();
        let line = crate::model::Line::from_local_endpoints(
            session.sketch,
            0.0,
            0.0,
            80.0,
            0.0,
        );
        let mut construction = line.clone();
        construction.construction = true;
        let mut solid = line;
        solid.construction = false;
        let cam = state.cam.clone();
        let viewport = test_viewport();
        let mut dashed_doc = state.doc.clone();
        dashed_doc.lines = vec![construction];
        let mut solid_doc = state.doc.clone();
        solid_doc.lines = vec![solid];
        let scene_fields = (
            &cam,
            viewport,
            ViewportPalette::default(),
            Some(session),
            &state.scene_selection,
            &state.element_visibility,
        );
        let grid_indices = ViewportScene::build(&ViewportSceneInput {
            doc: &state.doc,
            cam: scene_fields.0,
            viewport: scene_fields.1,
            palette: scene_fields.2,
            sketch_session: scene_fields.3,
            selection: scene_fields.4,
            element_visibility: scene_fields.5,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        })
        .indices
        .len();
        let dashed_scene = ViewportScene::build(&ViewportSceneInput {
            doc: &dashed_doc,
            cam: scene_fields.0,
            viewport: scene_fields.1,
            palette: scene_fields.2,
            sketch_session: scene_fields.3,
            selection: scene_fields.4,
            element_visibility: scene_fields.5,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        });
        let solid_scene = ViewportScene::build(&ViewportSceneInput {
            doc: &solid_doc,
            cam: scene_fields.0,
            viewport: scene_fields.1,
            palette: scene_fields.2,
            sketch_session: scene_fields.3,
            selection: scene_fields.4,
            element_visibility: scene_fields.5,
            preview_rect: None,
            preview_line: None,
            preview_circle: None,
            plane_preview: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
            plane_gizmo: None,
            hover_highlight: None,
            hover_color: Color32::WHITE,
        });
        let dashed_line_indices = dashed_scene.indices.len().saturating_sub(grid_indices);
        let solid_line_indices = solid_scene.indices.len().saturating_sub(grid_indices);
        assert!(dashed_line_indices > solid_line_indices);
    }

    #[test]
    fn line_screen_quad_has_four_corners() {
        let cam = Camera::default();
        let viewport = test_viewport();
        let vp = cam.view_proj(viewport);
        let quad = line_screen_quad(
            Vec3::ZERO,
            Vec3::new(100.0, 0.0, 0.0),
            2.0,
            &cam,
            viewport,
            &vp,
        )
        .expect("visible segment");
        assert_ne!(quad[0], quad[1]);
        assert_ne!(quad[2], quad[3]);
    }
}