//! GPU mesh builders for committed sketch dimension labels.

use crate::camera::Camera;
use crate::dimensions::{
    bilinear_quad_screen, planar_label_corners_screen, planar_label_corners_world,
    LinearDimensionWorldGeom, PlanarLabelView, LABEL_FONT_SIZE,
};
use eframe::egui::{Color32, FontId, Pos2, Rect};
use egui::Context;
use glam::{Mat4, Vec3};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuTextVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

fn color32_to_gpu(color: Color32) -> [f32; 4] {
    let [r, g, b, a] = color.to_array();
    [
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ]
}

#[derive(Clone, Debug)]
pub struct ViewportDimLabel {
    pub world_geom: LinearDimensionWorldGeom,
    pub color: Color32,
    pub text_vertices: Vec<GpuTextVertex>,
    pub text_indices: Vec<u32>,
}

fn label_plane_normal(view: &PlanarLabelView, world: &LinearDimensionWorldGeom) -> Vec3 {
    let mut plane_n = view.plane_normal.normalize_or_zero();
    if plane_n.length_squared() < 1e-8 {
        plane_n = world
            .along_world
            .cross(world.outward_world)
            .normalize_or_zero();
    }
    plane_n
}

/// Tessellate a planar dimension label into world-space textured vertices.
///
/// Glyph positions are laid out in screen space (matching the CPU painter) and
/// then unprojected onto the sketch plane so labels stay upright without skew
/// as the camera rotates.
pub fn build_planar_label_mesh<Project>(
    ctx: &Context,
    world: &LinearDimensionWorldGeom,
    view: &PlanarLabelView,
    label: &str,
    color: Color32,
    cam: &Camera,
    viewport: Rect,
    view_proj: &Mat4,
    project: &Project,
) -> (Vec<GpuTextVertex>, Vec<u32>)
where
    Project: Fn(Vec3) -> Option<Pos2>,
{
    let galley = ctx.fonts(|fonts| {
        fonts.layout_no_wrap(
            label.to_owned(),
            FontId::proportional(LABEL_FONT_SIZE),
            color,
        )
    });
    let size = galley.size();
    if size.x < 1e-4 || size.y < 1e-4 {
        return (Vec::new(), Vec::new());
    }
    let Some(corners_world) = planar_label_corners_world(world, view, size, project) else {
        return (Vec::new(), Vec::new());
    };
    let Some(corners_screen) = planar_label_corners_screen(&corners_world, project) else {
        return (Vec::new(), Vec::new());
    };
    let [tl, tr, br, bl] = corners_screen;
    let plane_n = label_plane_normal(view, world);
    if plane_n.length_squared() < 1e-8 {
        return (Vec::new(), Vec::new());
    }
    let to_eye = (view.eye - world.label_center).normalize_or_zero();
    let depth_bias = to_eye * 0.25;

    let font_tex_size = ctx.fonts(|fonts| fonts.font_image_size());
    let uv_norm = egui::Vec2::new(
        1.0 / font_tex_size[0] as f32,
        1.0 / font_tex_size[1] as f32,
    );

    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for row in &galley.rows {
        if row.visuals.mesh.is_empty() {
            continue;
        }
        let index_base = vertices.len() as u32;
        for (i, vertex) in row.visuals.mesh.vertices.iter().enumerate() {
            let local = vertex.pos.to_vec2();
            let u = local.x / size.x;
            let v = local.y / size.y;
            let screen_pos = bilinear_quad_screen(tl, tr, br, bl, u, v);
            let Some(mut world_pos) = cam.ray_plane_hit(
                screen_pos,
                viewport,
                view_proj,
                world.label_center,
                plane_n,
            ) else {
                continue;
            };
            world_pos += depth_bias;
            let mut glyph_color = vertex.color;
            if glyph_color == Color32::PLACEHOLDER {
                glyph_color = color;
            } else if row.visuals.glyph_vertex_range.contains(&i) {
                glyph_color = color;
            }
            let uv = vertex.uv.to_vec2() * uv_norm;
            vertices.push(GpuTextVertex {
                position: world_pos.to_array(),
                uv: [uv.x, uv.y],
                color: color32_to_gpu(glyph_color),
            });
        }
        indices.extend(
            row.visuals
                .mesh
                .indices
                .iter()
                .map(|index| index + index_base),
        );
    }
    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimensions::linear_dimension_world_geom;
    use egui::Pos2;

    fn test_project(cam: &Camera, viewport: egui::Rect) -> impl Fn(Vec3) -> Option<Pos2> + '_ {
        let vp = cam.view_proj(viewport);
        move |w: Vec3| cam.project(w, viewport, &vp)
    }

    #[test]
    fn build_planar_label_mesh_emits_textured_vertices() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let cam = Camera::default();
        let viewport = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let vp = cam.view_proj(viewport);
        let project = test_project(&cam, viewport);
        let view = PlanarLabelView::from_camera_and_plane(&cam, Vec3::Z);
        let world = linear_dimension_world_geom(
            Vec3::new(-50.0, 10.0, 0.0),
            Vec3::new(50.0, 10.0, 0.0),
            Vec3::Y,
            5.0,
            1.0,
            2.0,
        );
        let (vertices, indices) = build_planar_label_mesh(
            &ctx,
            &world,
            &view,
            "42.0 mm",
            Color32::WHITE,
            &cam,
            viewport,
            &vp,
            &project,
        );
        assert!(!vertices.is_empty());
        assert!(!indices.is_empty());
        assert_eq!(indices.len() % 3, 0);
    }

    #[test]
    fn screen_label_layout_round_trips_through_sketch_plane_under_tilted_camera() {
        let mut cam = Camera::default();
        cam.orbit(egui::vec2(120.0, 45.0));
        let viewport = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let vp = cam.view_proj(viewport);
        let project = test_project(&cam, viewport);
        let view = PlanarLabelView::from_camera_and_plane(&cam, Vec3::Z);
        let world = linear_dimension_world_geom(
            Vec3::new(-50.0, 10.0, 0.0),
            Vec3::new(50.0, 10.0, 0.0),
            Vec3::Y,
            5.0,
            1.0,
            2.0,
        );
        let corners_world = planar_label_corners_world(
            &world,
            &view,
            egui::vec2(48.0, 14.0),
            &project,
        )
        .unwrap();
        let corners_screen = planar_label_corners_screen(&corners_world, &project).unwrap();
        let plane_n = label_plane_normal(&view, &world);
        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.5, 0.5)] {
            let screen = bilinear_quad_screen(
                corners_screen[0],
                corners_screen[1],
                corners_screen[2],
                corners_screen[3],
                u,
                v,
            );
            let world_pos = cam
                .ray_plane_hit(screen, viewport, &vp, world.label_center, plane_n)
                .expect("screen label point should hit sketch plane");
            let back = cam
                .project(world_pos, viewport, &vp)
                .expect("plane point should project");
            let err = (back - screen).length();
            assert!(
                err < 0.5,
                "screen layout ({u}, {v}) should round-trip through the plane, error {err}px"
            );
        }
    }
}