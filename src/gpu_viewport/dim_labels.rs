//! GPU mesh builders for committed sketch dimension labels.

use crate::dimensions::{
    planar_label_corners_world, LinearDimensionWorldGeom, PlanarLabelView, LABEL_FONT_SIZE,
};
use eframe::egui::{Color32, FontId};
use egui::Context;
use glam::Vec3;

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
    pub text_vertices: Vec<GpuTextVertex>,
    pub text_indices: Vec<u32>,
}

fn bilinear_quad_world(tl: Vec3, tr: Vec3, br: Vec3, bl: Vec3, u: f32, v: f32) -> Vec3 {
    tl.lerp(tr, u).lerp(bl.lerp(br, u), v)
}

/// Tessellate a planar dimension label into world-space textured vertices.
pub fn build_planar_label_mesh<Project>(
    ctx: &Context,
    world: &LinearDimensionWorldGeom,
    view: &PlanarLabelView,
    label: &str,
    color: Color32,
    project: &Project,
) -> (Vec<GpuTextVertex>, Vec<u32>)
where
    Project: Fn(Vec3) -> Option<egui::Pos2>,
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
    let [tl, tr, br, bl] = corners_world;
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
            let mut glyph_color = vertex.color;
            if glyph_color == Color32::PLACEHOLDER {
                glyph_color = color;
            } else if row.visuals.glyph_vertex_range.contains(&i) {
                glyph_color = color;
            }
            let world_pos = bilinear_quad_world(tl, tr, br, bl, u, v) + depth_bias;
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
    use crate::camera::Camera;
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
        let (vertices, indices) =
            build_planar_label_mesh(&ctx, &world, &view, "42.0 mm", Color32::WHITE, &project);
        assert!(!vertices.is_empty());
        assert!(!indices.is_empty());
        assert_eq!(indices.len() % 3, 0);
    }
}