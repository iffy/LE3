//! CPU-side scene mesh builder for the GPU viewport.

use crate::actions::SketchSession;
use crate::camera::Camera;
use crate::construction::{
    plane_corners, CONSTRUCTION_DASH_GAP_PX, CONSTRUCTION_DASH_LENGTH_PX, CONSTRUCTION_RGBA,
    PLANE_DISPLAY_HALF,
};
use crate::context::selection_highlight_dashed;
use crate::face::{circle_world_perimeter, rect_world_corners, sketch_geometry_frame};
use crate::hierarchy::SceneElement;
use crate::model::{
    Circle, ConstructionPlane, Document, FaceId, Line, Rect as ModelRect, RectEdge,
};
use crate::hierarchy::ElementVisibility;
use crate::dimensions::{
    pixels_to_world_distance, LinearDimensionWorldGeom, PlanarLabelView, ARROW_LENGTH, ARROW_WING,
    LINE_WIDTH,
};
use crate::gpu_viewport::dim_labels::ViewportDimLabel;
use crate::selection::SceneSelection;
use eframe::egui::Color32;
use egui::Rect as UiRect;
use glam::{Mat4, Vec3};

pub const GRID_EXTENT: f32 = 200.0;
pub const GRID_STEP: f32 = 20.0;
pub const SKETCH_DIMMED: f32 = 0.28;
pub const CIRCLE_SEGMENTS: usize = 96;

/// Fill opacity for substantial sketch faces (matches pre-GPU painter, slightly stronger).
pub const SOLID_FILL_OPACITY: f32 = 0.38;
/// Fill opacity for construction geometry.
pub const CONSTRUCTION_FILL_OPACITY: f32 = 0.28;
/// Construction planes sit behind sketch shapes along the face normal.
pub const PLANE_FILL_DEPTH_BIAS: f32 = 0.0;
/// Base depth lift for sketch shape fills toward the camera.
pub const SHAPE_FILL_DEPTH_BIAS_BASE: f32 = 0.04;
/// Per-shape increment so coplanar overlaps resolve stably (higher index wins).
pub const SHAPE_FILL_DEPTH_BIAS_STEP: f32 = 0.008;
/// In-progress previews render above committed geometry.
pub const PREVIEW_FILL_DEPTH_BIAS: f32 = 0.2;

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
    pub indices: Vec<u32>,
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
            dim_edge_highlight: Color32::from_rgb(255, 186, 84),
        }
    }
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
    pub preview_plane: Option<ConstructionPlane>,
    pub active_sketch_face: Option<FaceId>,
    pub dimension_labels: &'a [ViewportDimLabel],
    pub dim_label_view: Option<PlanarLabelView>,
    pub dim_annotation_color: Color32,
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
                sketch_color(input.palette.construction, sketch_dimmed)
            };
            mesh.push_plane(
                plane,
                i,
                color,
                input.cam,
                input.viewport,
                &vp,
            );
        }

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
            mesh.push_rect(
                input.doc,
                rect,
                ri,
                input.cam,
                input.viewport,
                &vp,
                sketch_color(input.palette.rect_line, dim),
                sketch_color(input.palette.construction, dim),
                shape_fill_depth_bias(ri),
            );
        }

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
            mesh.push_circle(
                input.doc,
                circle,
                ci,
                input.cam,
                input.viewport,
                &vp,
                sketch_color(input.palette.rect_line, dim),
                sketch_color(input.palette.construction, dim),
                shape_fill_depth_bias(ci),
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
            mesh.push_face_highlight(input.doc, face, input.palette.dim_edge_highlight);
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
        if let Some(plane) = input.preview_plane.as_ref() {
            mesh.push_plane(
                plane,
                0,
                input.palette.preview,
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

pub(crate) struct SceneMesh<'a> {
    scene: &'a mut ViewportScene,
}

impl<'a> SceneMesh<'a> {
    fn new(scene: &'a mut ViewportScene) -> Self {
        Self { scene }
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
        self.scene.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

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
        self.push_triangle(fill_corners[0], fill_corners[1], fill_corners[2], fill);
        self.push_triangle(fill_corners[0], fill_corners[2], fill_corners[3], fill);
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
        self.scene
            .indices
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
            let color = sketch_color(base, dim);
            self.push_line_segment(
                Vec3::new(-e, t, 0.0),
                Vec3::new(e, t, 0.0),
                color,
                1.0,
                cam,
                viewport,
                view_proj,
            );
            self.push_line_segment(
                Vec3::new(t, -e, 0.0),
                Vec3::new(t, e, 0.0),
                color,
                1.0,
                cam,
                viewport,
                view_proj,
            );
            t += GRID_STEP;
        }
        self.push_line_segment(
            Vec3::ZERO,
            Vec3::new(e, 0.0, 0.0),
            sketch_color(palette.x_axis, dim),
            2.0,
            cam,
            viewport,
            view_proj,
        );
        self.push_line_segment(
            Vec3::ZERO,
            Vec3::new(0.0, e, 0.0),
            sketch_color(palette.y_axis, dim),
            2.0,
            cam,
            viewport,
            view_proj,
        );
        self.push_line_segment(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, e),
            sketch_color(palette.z_axis, dim),
            2.0,
            cam,
            viewport,
            view_proj,
        );
    }

    fn push_rect(
        &mut self,
        doc: &Document,
        rect: &ModelRect,
        _index: usize,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
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
        let fill_corners = offset_corners_toward_camera(corners, frame.normal, cam.eye(), fill_depth_bias);
        let all_construction = rect.all_edges_construction();
        let has_solid_edge = rect.construction_edges.iter().any(|&c| !c);
        if all_construction {
            self.push_quad(
                corners,
                fill_corners,
                fill_color(construction, CONSTRUCTION_FILL_OPACITY),
                construction,
                1.5,
                true,
                cam,
                viewport,
                view_proj,
            );
        } else if has_solid_edge && rect.has_mixed_edge_construction() {
            self.push_quad(
                corners,
                fill_corners,
                fill_color(solid, SOLID_FILL_OPACITY),
                solid,
                1.5,
                false,
                cam,
                viewport,
                view_proj,
            );
            for (edge_index, (a, b)) in rect_edge_segments(doc, rect).into_iter().enumerate() {
                let edge = RectEdge::from_index(edge_index);
                if rect.edge_construction(edge) {
                    self.push_dashed_line_segment(
                        a,
                        b,
                        construction,
                        1.5,
                        cam,
                        viewport,
                        view_proj,
                    );
                } else {
                    self.push_line_segment(a, b, solid, 1.5, cam, viewport, view_proj);
                }
            }
        } else if has_solid_edge {
            self.push_quad(
                corners,
                fill_corners,
                fill_color(solid, SOLID_FILL_OPACITY),
                solid,
                1.5,
                false,
                cam,
                viewport,
                view_proj,
            );
        }
    }

    fn push_circle(
        &mut self,
        doc: &Document,
        circle: &Circle,
        _index: usize,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
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
        if circle.construction {
            let fill = fill_color(construction, CONSTRUCTION_FILL_OPACITY);
            for window in perimeter.windows(2) {
                let a = offset_toward_camera(window[0], frame.normal, eye, fill_depth_bias);
                let b = offset_toward_camera(window[1], frame.normal, eye, fill_depth_bias);
                self.push_triangle(center, a, b, fill);
            }
            for window in perimeter.windows(2) {
                self.push_dashed_line_segment(
                    window[0],
                    window[1],
                    construction,
                    1.5,
                    cam,
                    viewport,
                    view_proj,
                );
            }
        } else {
            let fill = fill_color(solid, SOLID_FILL_OPACITY);
            for window in perimeter.windows(2) {
                let a = offset_toward_camera(window[0], frame.normal, eye, fill_depth_bias);
                let b = offset_toward_camera(window[1], frame.normal, eye, fill_depth_bias);
                self.push_triangle(center, a, b, fill);
            }
            for window in perimeter.windows(2) {
                self.push_line_segment(window[0], window[1], solid, 1.5, cam, viewport, view_proj);
            }
        }
    }

    fn push_plane(
        &mut self,
        plane: &ConstructionPlane,
        index: usize,
        color: Color32,
        cam: &Camera,
        viewport: UiRect,
        view_proj: &Mat4,
    ) {
        let corners = plane_corners(plane, PLANE_DISPLAY_HALF);
        let fill_bias = plane_fill_depth_bias(index);
        let eye = cam.eye();
        let fill_corners = offset_corners_toward_camera(corners, plane.normal, eye, fill_bias);
        let fill = fill_color(color, CONSTRUCTION_FILL_OPACITY);
        self.push_triangle(fill_corners[0], fill_corners[1], fill_corners[2], fill);
        self.push_triangle(fill_corners[0], fill_corners[2], fill_corners[3], fill);
        for (a, b) in [
            (corners[0], corners[1]),
            (corners[1], corners[2]),
            (corners[2], corners[3]),
            (corners[3], corners[0]),
        ] {
            self.push_line_segment(a, b, color, 1.5, cam, viewport, view_proj);
        }
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

    fn push_face_highlight(&mut self, doc: &Document, face: FaceId, color: Color32) {
        match face {
            FaceId::Rect(index) => {
                if let Some(rect) = doc.rects.get(index) {
                    if let Some(corners) = rect_world_corners(doc, rect) {
                        let fill = color.gamma_multiply(0.12);
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
                        let fill = color.gamma_multiply(0.12);
                        for window in perimeter.windows(2) {
                            self.push_triangle(center, window[0], window[1], fill);
                        }
                    }
                }
            }
            FaceId::ConstructionPlane(index) => {
                if let Some(plane) = doc.construction_planes.get(index) {
                    let corners = plane_corners(plane, PLANE_DISPLAY_HALF);
                    let fill = color.gamma_multiply(0.12);
                    self.push_triangle(corners[0], corners[1], corners[2], fill);
                    self.push_triangle(corners[0], corners[2], corners[3], fill);
                }
            }
        }
    }
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
    let side = along
        .cross(world.outward_world.normalize_or_zero())
        .normalize_or_zero();
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

fn sketch_color(color: Color32, dim: bool) -> Color32 {
    if dim {
        color.gamma_multiply(SKETCH_DIMMED)
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
    let corners = rect_world_corners(doc, rect).expect("rect corners");
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
    use crate::model::FaceId;
    use egui::Rect as UiRect;

    fn test_viewport() -> UiRect {
        UiRect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(960.0, 560.0))
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
            preview_plane: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
        });
        assert!(!scene.vertices.is_empty());
        assert!(!scene.indices.is_empty());
        assert_eq!(scene.clear_color[0], color32_to_gpu(Color32::from_gray(28))[0]);
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
            preview_plane: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
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
            preview_plane: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
        });
        assert!(scene.indices.len() > CIRCLE_SEGMENTS);
    }

    #[test]
    fn solid_fill_is_more_opaque_than_old_double_dim() {
        let base = Color32::from_rgb(120, 170, 240);
        let old_gpu = base.gamma_multiply(0.25).gamma_multiply(0.22);
        let new_fill = fill_color(base, SOLID_FILL_OPACITY);
        assert!(new_fill.a() > old_gpu.a());
        assert!(new_fill.a() >= 90);
    }

    #[test]
    fn shape_fill_depth_bias_increases_with_index() {
        assert!(shape_fill_depth_bias(2) > shape_fill_depth_bias(1));
        assert!(shape_fill_depth_bias(1) > shape_fill_depth_bias(0));
        assert!(shape_fill_depth_bias(0) > plane_fill_depth_bias(0));
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
            preview_plane: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: Some(view),
            dim_annotation_color: Color32::WHITE,
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
            preview_plane: None,
            active_sketch_face: None,
            dimension_labels: std::slice::from_ref(&dim_label),
            dim_label_view: Some(view),
            dim_annotation_color: Color32::WHITE,
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
            preview_plane: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
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
            preview_plane: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
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
            preview_plane: None,
            active_sketch_face: None,
            dimension_labels: &[],
            dim_label_view: None,
            dim_annotation_color: Color32::WHITE,
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