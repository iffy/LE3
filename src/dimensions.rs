//! Architectural-style linear dimension graphics for sketch edit mode.

use eframe::egui::epaint::{Mesh, Shape, TextShape, Vertex};
use eframe::egui::{Color32, FontId, Painter, Pos2, Rect, Stroke, Vec2};
use glam::Vec3;

pub const OFFSET: f32 = 20.0;
pub const MIN_DIM_OFFSET: f32 = 8.0;
pub const MAX_DIM_OFFSET: f32 = 200.0;
pub const EXTENSION_OVERSHOOT: f32 = 4.0;
pub const ARROW_LENGTH: f32 = 6.0;
pub const ARROW_WING: f32 = 3.5;
pub const LINE_WIDTH: f32 = 1.0;
pub const LABEL_OUTSET: f32 = 6.0;
pub const LABEL_HIT_PAD: f32 = 4.0;
pub const LABEL_FONT_SIZE: f32 = 12.0;

/// Camera and sketch-plane context for orienting labels on the visible face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanarLabelView {
    pub plane_normal: Vec3,
    pub eye: Vec3,
    pub screen_right: Vec3,
    pub screen_up: Vec3,
}

impl PlanarLabelView {
    pub fn from_camera_and_plane(cam: &crate::camera::Camera, plane_normal: Vec3) -> Self {
        let eye = cam.eye();
        let forward = (cam.target - eye).normalize_or_zero();
        let (screen_right, screen_up) = cam.screen_axes(forward);
        Self {
            plane_normal,
            eye,
            screen_right,
            screen_up,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearDimensionWorldGeom {
    pub ext_a_near: Vec3,
    pub ext_a_far: Vec3,
    pub ext_b_near: Vec3,
    pub ext_b_far: Vec3,
    pub dim_a: Vec3,
    pub dim_b: Vec3,
    pub label_center: Vec3,
    pub along_world: Vec3,
    pub outward_world: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearDimensionGeom {
    pub ext_a_near: Pos2,
    pub ext_a_far: Pos2,
    pub ext_b_near: Pos2,
    pub ext_b_far: Pos2,
    pub dim_a: Pos2,
    pub dim_b: Pos2,
    pub label_center: Pos2,
    pub along: Vec2,
    pub outward: Vec2,
}

pub fn effective_dim_offset(stored: Option<f32>) -> f32 {
    stored
        .unwrap_or(OFFSET)
        .clamp(MIN_DIM_OFFSET, MAX_DIM_OFFSET)
}

/// Perpendicular on the side opposite `interior` (extension lines point away from the shape).
#[cfg(test)]
pub fn outward_perpendicular(pa: Pos2, pb: Pos2, interior: Pos2) -> Vec2 {
    let delta = pb - pa;
    if delta.length_sq() < 1e-4 {
        return Vec2::new(0.0, 1.0);
    }
    let dir = delta.normalized();
    let perp_a = Vec2::new(-dir.y, dir.x);
    let perp_b = Vec2::new(dir.y, -dir.x);
    let mid = pa.lerp(pb, 0.5);
    let away = mid - interior;
    if perp_a.dot(away) >= perp_b.dot(away) {
        perp_a
    } else {
        perp_b
    }
}

#[cfg(test)]
pub fn linear_dimension_geom(
    pa: Pos2,
    pb: Pos2,
    interior: Pos2,
    offset: f32,
) -> LinearDimensionGeom {
    let outward = outward_perpendicular(pa, pb, interior);
    let along = (pb - pa).normalized();
    let dim_a = pa + outward * offset;
    let dim_b = pb + outward * offset;
    let overshoot = outward * EXTENSION_OVERSHOOT;
    LinearDimensionGeom {
        ext_a_near: pa,
        ext_a_far: dim_a + overshoot,
        ext_b_near: pb,
        ext_b_far: dim_b + overshoot,
        dim_a,
        dim_b,
        label_center: dim_a.lerp(dim_b, 0.5) + outward * LABEL_OUTSET,
        along,
        outward,
    }
}

/// Outward perpendicular in sketch (u, v) coordinates.
pub fn outward_perpendicular_uv(
    ua: f32,
    va: f32,
    ub: f32,
    vb: f32,
    ui: f32,
    vi: f32,
) -> (f32, f32) {
    let du = ub - ua;
    let dv = vb - va;
    if du * du + dv * dv < 1e-8 {
        return (0.0, 1.0);
    }
    let inv_len = (du * du + dv * dv).sqrt().recip();
    let dir_u = du * inv_len;
    let dir_v = dv * inv_len;
    let perp_a_u = -dir_v;
    let perp_a_v = dir_u;
    let perp_b_u = dir_v;
    let perp_b_v = -dir_u;
    let mid_u = (ua + ub) * 0.5;
    let mid_v = (va + vb) * 0.5;
    let away_u = mid_u - ui;
    let away_v = mid_v - vi;
    if perp_a_u * away_u + perp_a_v * away_v >= perp_b_u * away_u + perp_b_v * away_v {
        (perp_a_u, perp_a_v)
    } else {
        (perp_b_u, perp_b_v)
    }
}

pub fn preferred_outward_uv(ua: f32, va: f32, ub: f32, vb: f32) -> (f32, f32) {
    let mid_u = (ua + ub) * 0.5;
    let mid_v = (va + vb) * 0.5;
    outward_perpendicular_uv(ua, va, ub, vb, mid_u - 1.0, mid_v - 1.0)
}

pub fn uv_dir_to_world(u_axis: Vec3, v_axis: Vec3, du: f32, dv: f32) -> Vec3 {
    (u_axis * du + v_axis * dv).normalize_or_zero()
}

pub fn pixels_to_world_distance(
    project: &impl Fn(Vec3) -> Option<Pos2>,
    anchor: Vec3,
    direction_world: Vec3,
    pixels: f32,
) -> f32 {
    let dir = direction_world.normalize_or_zero();
    if dir.length_squared() < 1e-8 {
        return pixels;
    }
    let Some(p0) = project(anchor) else {
        return pixels;
    };
    let Some(p1) = project(anchor + dir) else {
        return pixels;
    };
    let px_per_unit = (p1 - p0).length();
    if px_per_unit < 1e-4 {
        return pixels;
    }
    pixels / px_per_unit
}

pub fn linear_dimension_world_geom(
    pa: Vec3,
    pb: Vec3,
    outward_world: Vec3,
    offset_world: f32,
    overshoot_world: f32,
    label_outset_world: f32,
) -> LinearDimensionWorldGeom {
    let outward = outward_world.normalize_or_zero();
    let along = (pb - pa).normalize_or_zero();
    let dim_a = pa + outward * offset_world;
    let dim_b = pb + outward * offset_world;
    LinearDimensionWorldGeom {
        ext_a_near: pa,
        ext_a_far: dim_a + outward * overshoot_world,
        ext_b_near: pb,
        ext_b_far: dim_b + outward * overshoot_world,
        dim_a,
        dim_b,
        label_center: dim_a.lerp(dim_b, 0.5) + outward * label_outset_world,
        along_world: along,
        outward_world: outward,
    }
}

pub fn project_linear_dimension_geom(
    world: &LinearDimensionWorldGeom,
    project: &impl Fn(Vec3) -> Option<Pos2>,
) -> Option<LinearDimensionGeom> {
    let ext_a_near = project(world.ext_a_near)?;
    let ext_a_far = project(world.ext_a_far)?;
    let ext_b_near = project(world.ext_b_near)?;
    let ext_b_far = project(world.ext_b_far)?;
    let dim_a = project(world.dim_a)?;
    let dim_b = project(world.dim_b)?;
    let label_center = project(world.label_center)?;
    let along = (dim_b - dim_a).normalized();
    let outward = {
        let p0 = project(world.ext_a_near)?;
        let p1 = project(world.ext_a_near + world.outward_world)?;
        (p1 - p0).normalized()
    };
    Some(LinearDimensionGeom {
        ext_a_near,
        ext_a_far,
        ext_b_near,
        ext_b_far,
        dim_a,
        dim_b,
        label_center,
        along,
        outward,
    })
}

#[cfg(test)]
pub fn world_points_on_plane(points: &[Vec3], origin: Vec3, normal: Vec3) -> bool {
    points
        .iter()
        .all(|p| (p - origin).dot(normal).abs() < 1e-3)
}

/// Rotation for dimension text so it stays parallel to the dimension line and upright.
pub fn label_rotation_radians(along: Vec2) -> f32 {
    if along.length_sq() < 1e-4 {
        return 0.0;
    }
    let mut angle = along.y.atan2(along.x);
    if angle > std::f32::consts::FRAC_PI_2 {
        angle -= std::f32::consts::PI;
    } else if angle < -std::f32::consts::FRAC_PI_2 {
        angle += std::f32::consts::PI;
    }
    angle
}

fn rotate_vec(v: Vec2, angle: f32) -> Vec2 {
    let (s, c) = angle.sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// Shift the label center outward so the nearest edge sits on `label_center`.
fn screen_label_anchor(center: Pos2, outward: Vec2, half_y: f32) -> Pos2 {
    if outward.length_sq() < 1e-8 {
        return center;
    }
    center + outward.normalized() * half_y
}

fn world_label_anchor(label_center: Vec3, outward: Vec3, half_y: f32, outward_per_px: f32) -> Vec3 {
    label_center + outward * (half_y * outward_per_px)
}

pub fn linear_dimension_label_rect(center: Pos2, galley_size: Vec2, angle: f32) -> Rect {
    let half = galley_size * 0.5;
    let mut min = Pos2::new(f32::MAX, f32::MAX);
    let mut max = Pos2::new(f32::MIN, f32::MIN);
    for dx in [-1.0f32, 1.0] {
        for dy in [-1.0f32, 1.0] {
            let corner = center + rotate_vec(Vec2::new(dx * half.x, dy * half.y), angle);
            min.x = min.x.min(corner.x);
            min.y = min.y.min(corner.y);
            max.x = max.x.max(corner.x);
            max.y = max.y.max(corner.y);
        }
    }
    Rect::from_min_max(min, max).expand(LABEL_HIT_PAD)
}

fn draw_arrowhead(painter: &Painter, tip: Pos2, dir: Vec2, color: Color32) {
    if dir.length_sq() < 1e-4 {
        return;
    }
    let d = dir.normalized();
    let side = Vec2::new(-d.y, d.x);
    let base = tip - d * ARROW_LENGTH;
    let stroke = Stroke::new(LINE_WIDTH, color);
    painter.line_segment([tip, base + side * ARROW_WING], stroke);
    painter.line_segment([tip, base - side * ARROW_WING], stroke);
}

fn projected_axis_dir<Project>(
    project: &Project,
    anchor: Vec3,
    axis: Vec3,
) -> Option<Vec2>
where
    Project: Fn(Vec3) -> Option<Pos2>,
{
    let center = project(anchor)?;
    let step = pixels_to_world_distance(project, anchor, axis, 1.0);
    let tip = project(anchor + axis * step)?;
    let dir = tip - center;
    if dir.length_sq() < 1e-8 {
        None
    } else {
        Some(dir.normalized())
    }
}

/// World-space text axes parallel to the dimension line, on the camera-facing plane
/// side, and upright in screen space.
pub fn planar_label_axes_world<Project>(
    world: &LinearDimensionWorldGeom,
    view: &PlanarLabelView,
    project: &Project,
) -> (Vec3, Vec3)
where
    Project: Fn(Vec3) -> Option<Pos2>,
{
    let base_along = world.along_world.normalize_or_zero();
    if base_along.length_squared() < 1e-8 {
        return (base_along, world.outward_world);
    }

    let to_eye = (view.eye - world.label_center).normalize_or_zero();
    let mut plane_n = view.plane_normal.normalize_or_zero();
    if plane_n.length_squared() < 1e-8 {
        plane_n = base_along.cross(world.outward_world).normalize_or_zero();
    }
    if plane_n.dot(to_eye) < 0.0 {
        plane_n = -plane_n;
    }

    let mut along = base_along;
    if let Some(along_screen) = projected_axis_dir(project, world.label_center, along) {
        if along_screen.dot(Vec2::X) < 0.0 {
            along = -along;
        }
    }

    let mut text_up = plane_n.cross(along).normalize_or_zero();
    if text_up.length_squared() < 1e-8 {
        text_up = world.outward_world.normalize_or_zero();
    }
    if along.cross(text_up).dot(to_eye) < 0.0 {
        along = -along;
        text_up = plane_n.cross(along).normalize_or_zero();
    }
    if let Some(up_screen) = projected_axis_dir(project, world.label_center, text_up) {
        if up_screen.dot(Vec2::new(0.0, -1.0)) < 0.0 {
            along = -along;
            text_up = plane_n.cross(along).normalize_or_zero();
        }
    }

    (along, text_up)
}

fn bilinear_quad(tl: Pos2, tr: Pos2, br: Pos2, bl: Pos2, u: f32, v: f32) -> Pos2 {
    tl.lerp(tr, u).lerp(bl.lerp(br, u), v)
}

fn planar_label_corners_world<Project>(
    world: &LinearDimensionWorldGeom,
    view: &PlanarLabelView,
    galley_size: Vec2,
    project: &Project,
) -> Option<[Vec3; 4]>
where
    Project: Fn(Vec3) -> Option<Pos2>,
{
    let (along_w, text_up_w) = planar_label_axes_world(world, view, project);
    let offset_outward = world.outward_world.normalize_or_zero();
    let half = galley_size * 0.5;
    let along_per_px =
        pixels_to_world_distance(project, world.label_center, along_w, 1.0);
    let outward_per_px =
        pixels_to_world_distance(project, world.label_center, offset_outward, 1.0);
    let anchor = world_label_anchor(world.label_center, offset_outward, half.y, outward_per_px);
    let top_left =
        anchor - along_w * (half.x * along_per_px) + text_up_w * (half.y * outward_per_px);
    let size = galley_size;
    Some([
        top_left,
        top_left + along_w * (size.x * along_per_px),
        top_left + along_w * (size.x * along_per_px) - text_up_w * (size.y * outward_per_px),
        top_left - text_up_w * (size.y * outward_per_px),
    ])
}

fn planar_label_corners_screen<Project>(
    corners_world: &[Vec3; 4],
    project: &Project,
) -> Option<[Pos2; 4]>
where
    Project: Fn(Vec3) -> Option<Pos2>,
{
    let mut corners = [Pos2::ZERO; 4];
    for (dst, world) in corners.iter_mut().zip(corners_world) {
        *dst = project(*world)?;
    }
    Some(corners)
}

fn rect_from_screen_corners(corners: &[Pos2; 4]) -> Rect {
    let mut min = Pos2::new(f32::MAX, f32::MAX);
    let mut max = Pos2::new(f32::MIN, f32::MIN);
    for corner in corners {
        min.x = min.x.min(corner.x);
        min.y = min.y.min(corner.y);
        max.x = max.x.max(corner.x);
        max.y = max.y.max(corner.y);
    }
    Rect::from_min_max(min, max).expand(LABEL_HIT_PAD)
}

pub fn planar_dimension_label_layout<Project>(
    painter: &Painter,
    world: &LinearDimensionWorldGeom,
    view: &PlanarLabelView,
    label: &str,
    color: Color32,
    project: &Project,
) -> Rect
where
    Project: Fn(Vec3) -> Option<Pos2>,
{
    let galley = painter.layout_no_wrap(
        label.to_string(),
        FontId::proportional(LABEL_FONT_SIZE),
        color,
    );
    let Some(corners_world) = planar_label_corners_world(world, view, galley.size(), project) else {
        return Rect::NOTHING;
    };
    let Some(corners_screen) = planar_label_corners_screen(&corners_world, project) else {
        return Rect::NOTHING;
    };
    rect_from_screen_corners(&corners_screen)
}

fn draw_planar_dimension_label<Project>(
    painter: &Painter,
    world: &LinearDimensionWorldGeom,
    view: &PlanarLabelView,
    label: &str,
    color: Color32,
    project: &Project,
) -> Rect
where
    Project: Fn(Vec3) -> Option<Pos2>,
{
    let galley = painter.layout_no_wrap(
        label.to_string(),
        FontId::proportional(LABEL_FONT_SIZE),
        color,
    );
    let size = galley.size();
    if size.x < 1e-4 || size.y < 1e-4 {
        return Rect::NOTHING;
    }
    let Some(corners_world) = planar_label_corners_world(world, view, size, project) else {
        return Rect::NOTHING;
    };
    let Some(corners_screen) = planar_label_corners_screen(&corners_world, project) else {
        return Rect::NOTHING;
    };

    let font_tex_size = painter.ctx().fonts(|f| f.font_image_size());
    let uv_norm = Vec2::new(
        1.0 / font_tex_size[0] as f32,
        1.0 / font_tex_size[1] as f32,
    );
    let [tl, tr, br, bl] = corners_screen;

    let mut mesh = Mesh::default();
    for row in &galley.rows {
        if row.visuals.mesh.is_empty() {
            continue;
        }
        let index_base = mesh.vertices.len() as u32;
        mesh.texture_id = row.visuals.mesh.texture_id;
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
            mesh.vertices.push(Vertex {
                pos: bilinear_quad(tl, tr, br, bl, u, v),
                uv: (vertex.uv.to_vec2() * uv_norm).to_pos2(),
                color: glyph_color,
            });
        }
        mesh.indices.extend(
            row.visuals
                .mesh
                .indices
                .iter()
                .map(|index| index + index_base),
        );
    }
    if !mesh.vertices.is_empty() {
        painter.add(Shape::mesh(mesh));
    }
    rect_from_screen_corners(&corners_screen)
}

pub fn linear_dimension_label_layout(
    painter: &Painter,
    geom: &LinearDimensionGeom,
    label: &str,
    color: Color32,
) -> (f32, Rect) {
    let angle = label_rotation_radians(geom.along);
    let galley = painter.layout_no_wrap(
        label.to_string(),
        FontId::proportional(LABEL_FONT_SIZE),
        color,
    );
    let half = galley.size() * 0.5;
    let anchor = screen_label_anchor(geom.label_center, geom.outward, half.y);
    let rect = linear_dimension_label_rect(anchor, galley.size(), angle);
    (angle, rect)
}

pub fn draw_linear_dimension<Project>(
    painter: &Painter,
    geom: &LinearDimensionGeom,
    label: &str,
    color: Color32,
    planar: Option<(&LinearDimensionWorldGeom, &PlanarLabelView, &Project)>,
) -> Rect
where
    Project: Fn(Vec3) -> Option<Pos2>,
{
    let stroke = Stroke::new(LINE_WIDTH, color);
    painter.line_segment([geom.ext_a_near, geom.ext_a_far], stroke);
    painter.line_segment([geom.ext_b_near, geom.ext_b_far], stroke);
    painter.line_segment([geom.dim_a, geom.dim_b], stroke);
    draw_arrowhead(painter, geom.dim_a, -geom.along, color);
    draw_arrowhead(painter, geom.dim_b, geom.along, color);

    if let Some((world_geom, view, project)) = planar {
        return draw_planar_dimension_label(painter, world_geom, view, label, color, project);
    }

    let (angle, rect) = linear_dimension_label_layout(painter, geom, label, color);
    let galley = painter.layout_no_wrap(
        label.to_string(),
        FontId::proportional(LABEL_FONT_SIZE),
        color,
    );
    let half = galley.size() * 0.5;
    let anchor = screen_label_anchor(geom.label_center, geom.outward, half.y);
    let pos = anchor - rotate_vec(half, angle);
    painter.add(
        TextShape::new(pos, galley, color)
            .with_override_text_color(color)
            .with_angle(angle),
    );
    rect
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outward_perpendicular_points_away_from_interior() {
        let pa = Pos2::new(100.0, 100.0);
        let pb = Pos2::new(200.0, 100.0);
        let interior = Pos2::new(150.0, 130.0);
        let outward = outward_perpendicular(pa, pb, interior);
        let mid = pa.lerp(pb, 0.5);
        assert!(
            outward.dot(mid - interior) > 0.0,
            "extension lines should point away from the shape interior"
        );
    }

    #[test]
    fn dimension_line_is_parallel_offset_from_measured_segment() {
        let pa = Pos2::new(100.0, 200.0);
        let pb = Pos2::new(300.0, 200.0);
        let interior = Pos2::new(200.0, 250.0);
        let geom = linear_dimension_geom(pa, pb, interior, OFFSET);
        let measured = (pb - pa).normalized();
        let dim_line = (geom.dim_b - geom.dim_a).normalized();
        assert!(measured.dot(dim_line).abs() > 0.99);
        assert!((geom.dim_a - pa).length() > OFFSET * 0.9);
        assert!((geom.dim_b - pb).length() > OFFSET * 0.9);
    }

    #[test]
    fn extension_lines_run_perpendicular_to_measured_segment() {
        let pa = Pos2::new(50.0, 80.0);
        let pb = Pos2::new(150.0, 80.0);
        let interior = Pos2::new(100.0, 120.0);
        let geom = linear_dimension_geom(pa, pb, interior, OFFSET);
        let ext_dir = (geom.ext_a_far - geom.ext_a_near).normalized();
        let along = (pb - pa).normalized();
        assert!(
            ext_dir.dot(along).abs() < 0.05,
            "extension lines should be perpendicular to the measured edge"
        );
    }

    #[test]
    fn label_rotation_is_parallel_and_upright_for_horizontal_dim() {
        let along = Vec2::new(1.0, 0.0);
        let angle = label_rotation_radians(along);
        assert!(angle.abs() < 0.01);
    }

    #[test]
    fn label_rotation_flips_for_left_pointing_horizontal_dim() {
        let along = Vec2::new(-1.0, 0.0);
        let angle = label_rotation_radians(along);
        assert!(angle.abs() < 0.01, "text should stay upright, got {angle}");
    }

    #[test]
    fn label_rotation_is_parallel_for_vertical_dim() {
        let along = Vec2::new(0.0, 1.0);
        let angle = label_rotation_radians(along);
        assert!((angle - std::f32::consts::FRAC_PI_2).abs() < 0.01);
    }

    #[test]
    fn effective_dim_offset_defaults_and_clamps() {
        assert_eq!(effective_dim_offset(None), OFFSET);
        assert_eq!(effective_dim_offset(Some(2.0)), MIN_DIM_OFFSET);
        assert_eq!(effective_dim_offset(Some(500.0)), MAX_DIM_OFFSET);
    }

    #[test]
    fn outward_perpendicular_uv_points_away_from_interior() {
        let (ou, ov) = outward_perpendicular_uv(0.0, 0.0, 10.0, 0.0, 5.0, 5.0);
        assert!(ov < 0.0, "bottom edge should offset away from interior above");
        assert!(ou.abs() < 0.01);
    }

    #[test]
    fn world_dimension_geometry_stays_in_sketch_plane() {
        let u = Vec3::X;
        let v = Vec3::Y;
        let normal = Vec3::Z;
        let pa = Vec3::new(0.0, 0.0, 0.0);
        let pb = Vec3::new(100.0, 0.0, 0.0);
        let (ou, ov) = outward_perpendicular_uv(0.0, 0.0, 100.0, 0.0, 50.0, 40.0);
        let outward = uv_dir_to_world(u, v, ou, ov);
        let geom = linear_dimension_world_geom(pa, pb, outward, 8.0, 1.0, 2.0);
        let points = [
            geom.ext_a_near,
            geom.ext_a_far,
            geom.ext_b_near,
            geom.ext_b_far,
            geom.dim_a,
            geom.dim_b,
            geom.label_center,
        ];
        assert!(world_points_on_plane(&points, pa, normal));
        assert!(outward.dot(normal).abs() < 1e-4);
        assert!(geom.along_world.dot(outward).abs() < 1e-4);
    }

    fn segment_intersects_rect(pa: Pos2, pb: Pos2, rect: Rect) -> bool {
        if rect.contains(pa) || rect.contains(pb) {
            return true;
        }
        let edges = [
            (rect.left_top(), rect.right_top()),
            (rect.right_top(), rect.right_bottom()),
            (rect.right_bottom(), rect.left_bottom()),
            (rect.left_bottom(), rect.left_top()),
        ];
        for (c, d) in edges {
            if segments_intersect(pa, pb, c, d) {
                return true;
            }
        }
        false
    }

    fn segments_intersect(a: Pos2, b: Pos2, c: Pos2, d: Pos2) -> bool {
        fn cross(a: Pos2, b: Pos2, c: Pos2) -> f32 {
            (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
        }
        let ab = cross(a, b, c);
        let ab_d = cross(a, b, d);
        let cd = cross(c, d, a);
        let cd_b = cross(c, d, b);
        ab * ab_d <= 0.0 && cd * cd_b <= 0.0
    }

    fn label_rect_for_geom(geom: &LinearDimensionGeom, galley_size: Vec2) -> Rect {
        let angle = label_rotation_radians(geom.along);
        let anchor = screen_label_anchor(geom.label_center, geom.outward, galley_size.y * 0.5);
        linear_dimension_label_rect(anchor, galley_size, angle)
    }

    #[test]
    fn label_rect_avoids_horizontal_dimension_line() {
        let pa = Pos2::new(100.0, 200.0);
        let pb = Pos2::new(300.0, 200.0);
        let interior = Pos2::new(200.0, 250.0);
        let geom = linear_dimension_geom(pa, pb, interior, OFFSET);
        let rect = label_rect_for_geom(&geom, Vec2::new(56.0, 14.0));
        assert!(
            !segment_intersects_rect(geom.dim_a, geom.dim_b, rect),
            "horizontal label should sit clear of the dimension line"
        );
    }

    #[test]
    fn label_rect_avoids_vertical_dimension_line() {
        let pa = Pos2::new(100.0, 100.0);
        let pb = Pos2::new(100.0, 300.0);
        let interior = Pos2::new(150.0, 200.0);
        let geom = linear_dimension_geom(pa, pb, interior, OFFSET);
        let rect = label_rect_for_geom(&geom, Vec2::new(56.0, 14.0));
        assert!(
            !segment_intersects_rect(geom.dim_a, geom.dim_b, rect),
            "vertical label should sit clear of the dimension line"
        );
    }

    fn test_label_view_from_camera(cam: &crate::camera::Camera, plane_normal: Vec3) -> PlanarLabelView {
        PlanarLabelView::from_camera_and_plane(cam, plane_normal)
    }

    fn label_text_up_points_screen_up<Project>(
        world: &LinearDimensionWorldGeom,
        view: &PlanarLabelView,
        project: &Project,
    ) -> bool
    where
        Project: Fn(Vec3) -> Option<Pos2>,
    {
        let (_, text_up) = planar_label_axes_world(world, view, project);
        let Some(center) = project(world.label_center) else {
            return false;
        };
        let step = pixels_to_world_distance(project, world.label_center, text_up, 1.0);
        let Some(tip) = project(world.label_center + text_up * step) else {
            return false;
        };
        (tip - center).normalized().dot(Vec2::new(0.0, -1.0)) > 0.0
    }

    fn label_faces_camera<Project>(
        world: &LinearDimensionWorldGeom,
        view: &PlanarLabelView,
        project: &Project,
    ) -> bool
    where
        Project: Fn(Vec3) -> Option<Pos2>,
    {
        let (along, outward) = planar_label_axes_world(world, view, project);
        let to_eye = (view.eye - world.label_center).normalize_or_zero();
        along.cross(outward).dot(to_eye) > 0.0
    }

    #[test]
    fn planar_label_faces_camera_when_viewed_from_below_plane() {
        use crate::camera::Camera;
        use eframe::egui::Rect;

        let u = Vec3::X;
        let v = Vec3::Y;
        let origin = Vec3::ZERO;
        let world = linear_dimension_world_geom(
            origin,
            origin + u * 80.0,
            v,
            20.0,
            EXTENSION_OVERSHOOT,
            LABEL_OUTSET,
        );
        let mut cam = Camera::default();
        cam.pitch = -1.2;
        cam.distance = 200.0;
        let view = test_label_view_from_camera(&cam, Vec3::Z);
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let vp = cam.view_proj(viewport);
        let project = |p: Vec3| cam.project(p, viewport, &vp);
        assert!(
            label_faces_camera(&world, &view, &project),
            "label should face the camera below the XY plane"
        );
        assert!(
            label_text_up_points_screen_up(&world, &view, &project),
            "label should read upright when viewed from below"
        );
    }

    #[test]
    fn planar_label_faces_camera_when_viewed_from_above_plane() {
        use crate::camera::Camera;
        use eframe::egui::Rect;

        let u = Vec3::X;
        let v = Vec3::Y;
        let origin = Vec3::ZERO;
        let world = linear_dimension_world_geom(
            origin,
            origin + u * 80.0,
            v,
            20.0,
            EXTENSION_OVERSHOOT,
            LABEL_OUTSET,
        );
        let mut cam = Camera::default();
        cam.pitch = 1.2;
        cam.distance = 200.0;
        let view = test_label_view_from_camera(&cam, Vec3::Z);
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let vp = cam.view_proj(viewport);
        let project = |p: Vec3| cam.project(p, viewport, &vp);
        assert!(label_faces_camera(&world, &view, &project));
        assert!(
            label_text_up_points_screen_up(&world, &view, &project),
            "label should read upright when viewed from above"
        );
    }

    #[test]
    fn planar_label_rect_avoids_projected_dimension_line() {
        let u = Vec3::X;
        let v = Vec3::Y;
        let origin = Vec3::ZERO;
        let outward = v;
        let world = linear_dimension_world_geom(
            origin,
            origin + u * 80.0,
            outward,
            20.0,
            EXTENSION_OVERSHOOT,
            LABEL_OUTSET,
        );
        let project = |p: Vec3| Some(Pos2::new(p.x, -p.y));
        let geom = project_linear_dimension_geom(&world, &project).expect("geom");
        let mut cam = crate::camera::Camera::default();
        cam.pitch = 1.2;
        cam.distance = 200.0;
        let view = test_label_view_from_camera(&cam, Vec3::Z);
        let galley_size = Vec2::new(56.0, 14.0);
        let corners =
            planar_label_corners_world(&world, &view, galley_size, &project).expect("corners");
        let screen = planar_label_corners_screen(&corners, &project).expect("screen");
        let rect = rect_from_screen_corners(&screen);
        assert!(
            !segment_intersects_rect(geom.dim_a, geom.dim_b, rect),
            "planar label should not overlap the dimension line"
        );
    }

    #[test]
    fn planar_label_corners_stay_on_tilted_sketch_plane() {
        let u = Vec3::new(1.0, 0.0, 1.0).normalize();
        let v = Vec3::Y;
        let normal = u.cross(v).normalize();
        let origin = Vec3::new(10.0, 5.0, 20.0);
        let world = LinearDimensionWorldGeom {
            ext_a_near: origin,
            ext_a_far: origin + v,
            ext_b_near: origin + u * 80.0,
            ext_b_far: origin + u * 80.0 + v,
            dim_a: origin + v * 2.0,
            dim_b: origin + u * 80.0 + v * 2.0,
            label_center: origin + u * 40.0 + v * 3.0,
            along_world: u,
            outward_world: v,
        };
        let project = |p: Vec3| Some(Pos2::new(p.x, p.z));
        let mut cam = crate::camera::Camera::default();
        cam.pitch = 0.6;
        cam.yaw = 0.8;
        cam.distance = 200.0;
        let view = test_label_view_from_camera(&cam, normal);
        let corners = planar_label_corners_world(&world, &view, Vec2::new(60.0, 14.0), &project)
            .expect("corners");
        assert!(world_points_on_plane(&corners, origin, normal));
    }

    #[test]
    fn planar_label_baseline_follows_sketch_plane_projection() {
        use eframe::egui::Rect;

        let u = Vec3::new(1.0, 0.0, 1.0).normalize();
        let v = Vec3::Y;
        let origin = Vec3::ZERO;
        let world = LinearDimensionWorldGeom {
            ext_a_near: origin,
            ext_a_far: origin + v,
            ext_b_near: origin + u * 80.0,
            ext_b_far: origin + u * 80.0 + v,
            dim_a: origin + v * 2.0,
            dim_b: origin + u * 80.0 + v * 2.0,
            label_center: origin + u * 40.0 + v * 3.0,
            along_world: u,
            outward_world: v,
        };
        let mut cam = crate::camera::Camera::default();
        cam.pitch = 0.6;
        cam.yaw = 0.8;
        cam.distance = 200.0;
        let view = test_label_view_from_camera(&cam, u.cross(v));
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let vp = cam.view_proj(viewport);
        let project = |p: Vec3| cam.project(p, viewport, &vp);
        let corners =
            planar_label_corners_world(&world, &view, Vec2::new(60.0, 14.0), &project)
                .expect("corners");
        let screen = planar_label_corners_screen(&corners, &project).expect("screen corners");
        assert!(label_text_up_points_screen_up(&world, &view, &project));
        let baseline = screen[1] - screen[0];
        let u_span = project(origin + u * 20.0).unwrap() - project(origin).unwrap();
        assert!(
            baseline.normalized().dot(u_span.normalized()).abs() > 0.99,
            "label baseline should follow the sketch-plane axis on screen"
        );
        assert!(
            baseline.y.abs() > 0.5,
            "tilted plane should tilt the label on screen instead of keeping it axis-aligned"
        );
    }

    #[test]
    fn tilted_sketch_plane_keeps_dimension_geometry_coplanar() {
        let u = Vec3::new(1.0, 0.0, 1.0).normalize();
        let v = Vec3::Y;
        let normal = u.cross(v).normalize();
        let origin = Vec3::new(10.0, 5.0, 20.0);
        let pa = origin;
        let pb = origin + u * 80.0;
        let interior = origin + u * 40.0 + v * 30.0;
        let (iu, iv) = {
            let rel = interior - origin;
            (rel.dot(u), rel.dot(v))
        };
        let (au, av) = (0.0, 0.0);
        let (bu, bv) = (80.0, 0.0);
        let (ou, ov) = outward_perpendicular_uv(au, av, bu, bv, iu, iv);
        let outward = uv_dir_to_world(u, v, ou, ov);
        let geom = linear_dimension_world_geom(pa, pb, outward, 6.0, 1.0, 2.0);
        let points = [
            geom.ext_a_near,
            geom.ext_a_far,
            geom.ext_b_near,
            geom.ext_b_far,
            geom.dim_a,
            geom.dim_b,
            geom.label_center,
        ];
        assert!(world_points_on_plane(&points, origin, normal));
    }
}