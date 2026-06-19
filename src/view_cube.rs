//! View-cube HUD (top-right): bear model inside an oriented bounding box,
//! drag-to-orbit, click faces/edges/corners to animate standard views.

use crate::camera::{Camera, ProjectionMode, StandardView, VIEW_TRANSITION_DURATION};
use crate::stl::{fit_mesh_to_unit_cube, parse_ascii_stl, scale_mesh, MeshTriangle};
use eframe::egui::epaint::TextShape;
use eframe::egui::{
    self, Color32, ColorImage, FontId, Id, Painter, Pos2, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, Ui, Vec2,
};
use glam::Vec3;

const CUBE_SIZE: f32 = 96.0;
const CUBE_MARGIN: f32 = 12.0;
const HALF: f32 = 0.5;
const DRAG_CLICK_THRESHOLD: f32 = 4.0;
const EDGE_HIT_RADIUS: f32 = 6.0;
const CORNER_HIT_RADIUS: f32 = 7.0;
const EDGE_STROKE_HOVER: f32 = 3.5;
const FACE_STROKE_HOVER: f32 = 2.0;
const CORNER_RADIUS_HOVER: f32 = 5.5;
const BBOX_HOVER_STROKE: Color32 = Color32::from_rgb(255, 220, 120);
const PRESET_TOGGLE_SIZE: f32 = 20.0;
const PRESET_TOGGLE_MARGIN: f32 = 3.0;
const PRESET_TOGGLE_ICON_PAD: f32 = 4.0;
const PRESET_TOGGLE_ICON_STROKE: f32 = 1.4;
/// Hide faces that are too edge-on to the camera (they flare when orthographically projected).
const FACE_CULL_DOT: f32 = 0.22;
/// Hide only nearly edge-on facets (grazing triangles are unstable to rasterize).
const BEAR_TRIANGLE_CULL_DOT: f32 = 0.02;
/// World-space origin for the X/Y/Z axis triad (front–left–bottom corner).
const AXIS_ORIGIN: Vec3 = Vec3::new(-HALF, -HALF, -HALF);
const AXIS_LENGTH: f32 = 1.0;
const AXIS_STROKE: f32 = 2.0;
const BEAR_STL: &str = include_str!("assets/bear.stl");
const BEAR_BASE_COLOR: [f32; 3] = [156.0, 118.0, 78.0];
const BEAR_AMBIENT: f32 = 0.34;
/// Fixed directional light in HUD world space (Z-up).
const BEAR_LIGHT_DIR: Vec3 = Vec3::new(0.42, -0.28, 0.86);
const BEAR_MESH_MARGIN: f32 = 0.0;
/// Extra scale so the bear fills the HUD bbox (clipped to the cube silhouette when drawn).
const BEAR_MESH_SCALE: f32 = 2.45;

#[derive(Clone, Copy)]
struct AxisDef {
    label: &'static str,
    direction: Vec3,
    color: Color32,
}

const AXES: [AxisDef; 3] = [
    AxisDef {
        label: "X",
        direction: Vec3::X,
        color: Color32::from_rgb(220, 80, 80),
    },
    AxisDef {
        label: "Y",
        direction: Vec3::Y,
        color: Color32::from_rgb(80, 200, 100),
    },
    AxisDef {
        label: "Z",
        direction: Vec3::Z,
        color: Color32::from_rgb(80, 140, 230),
    },
];

#[derive(Clone, Copy)]
struct CubeFace {
    view: StandardView,
    corners: [Vec3; 4],
}

const FACES: [CubeFace; 6] = [
    CubeFace {
        view: StandardView::Front,
        corners: [
            Vec3::new(-HALF, -HALF, -HALF),
            Vec3::new(HALF, -HALF, -HALF),
            Vec3::new(HALF, -HALF, HALF),
            Vec3::new(-HALF, -HALF, HALF),
        ],
    },
    CubeFace {
        view: StandardView::Back,
        corners: [
            Vec3::new(HALF, HALF, -HALF),
            Vec3::new(-HALF, HALF, -HALF),
            Vec3::new(-HALF, HALF, HALF),
            Vec3::new(HALF, HALF, HALF),
        ],
    },
    CubeFace {
        view: StandardView::Right,
        corners: [
            Vec3::new(HALF, -HALF, -HALF),
            Vec3::new(HALF, HALF, -HALF),
            Vec3::new(HALF, HALF, HALF),
            Vec3::new(HALF, -HALF, HALF),
        ],
    },
    CubeFace {
        view: StandardView::Left,
        corners: [
            Vec3::new(-HALF, HALF, -HALF),
            Vec3::new(-HALF, -HALF, -HALF),
            Vec3::new(-HALF, -HALF, HALF),
            Vec3::new(-HALF, HALF, HALF),
        ],
    },
    CubeFace {
        view: StandardView::Top,
        corners: [
            Vec3::new(-HALF, -HALF, HALF),
            Vec3::new(HALF, -HALF, HALF),
            Vec3::new(HALF, HALF, HALF),
            Vec3::new(-HALF, HALF, HALF),
        ],
    },
    CubeFace {
        view: StandardView::Bottom,
        corners: [
            Vec3::new(-HALF, HALF, -HALF),
            Vec3::new(HALF, HALF, -HALF),
            Vec3::new(HALF, -HALF, -HALF),
            Vec3::new(-HALF, -HALF, -HALF),
        ],
    },
];

struct ProjectedFace {
    view: StandardView,
    points: [Pos2; 4],
    /// Average corner depth along the camera forward axis (for painter order).
    depth: f32,
}

struct ProjectedBearTriangle {
    points: [Pos2; 3],
    zs: [f32; 3],
    color: Color32,
}

/// Vertex for GPU HUD bear rendering (view-cube basis, per-vertex color).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuBearVertex {
    pub view_position: [f32; 3],
    pub color: [f32; 4],
}

/// Indexed mesh for the HUD bear in view-cube space.
pub struct BearGpuMesh {
    pub vertices: Vec<GpuBearVertex>,
    pub indices: Vec<u32>,
    pub z_min: f32,
    pub z_max: f32,
}

fn bear_world_normal(tri: &MeshTriangle, right: Vec3, up: Vec3, forward: Vec3) -> Vec3 {
    let mut normal = tri.normal;
    if transform_vertex(normal, right, up, forward).z >= 0.0 {
        normal = -normal;
    }
    normal.normalize_or_zero()
}

fn shade_bear_color(normal: Vec3) -> Color32 {
    let n = normal.normalize_or_zero();
    let light = BEAR_LIGHT_DIR.normalize_or_zero();
    let diffuse = n.dot(light).max(0.0);
    let factor = BEAR_AMBIENT + (1.0 - BEAR_AMBIENT) * diffuse;
    Color32::from_rgb(
        (BEAR_BASE_COLOR[0] * factor).round() as u8,
        (BEAR_BASE_COLOR[1] * factor).round() as u8,
        (BEAR_BASE_COLOR[2] * factor).round() as u8,
    )
}

fn bear_mesh() -> &'static [MeshTriangle] {
    static MESH: std::sync::OnceLock<Vec<MeshTriangle>> = std::sync::OnceLock::new();
    MESH.get_or_init(|| {
        let raw = parse_ascii_stl(BEAR_STL).expect("bear.stl");
        // bear.stl forward is +X; skip the old −Y reorientation.
        scale_mesh(
            &fit_mesh_to_unit_cube(&raw, HALF, BEAR_MESH_MARGIN),
            BEAR_MESH_SCALE,
        )
    })
}

fn tri_screen_area(points: [Pos2; 3]) -> f32 {
    let a = points[1] - points[0];
    let b = points[2] - points[0];
    (a.x * b.y - a.y * b.x).abs() * 0.5
}

#[cfg(test)]
fn max_triangle_edge_length(points: [Pos2; 3]) -> f32 {
    [
        (points[0] - points[1]).length(),
        (points[1] - points[2]).length(),
        (points[2] - points[0]).length(),
    ]
    .into_iter()
    .fold(0.0f32, f32::max)
}

fn wind_triangle_clockwise_with_z(
    points: [Pos2; 3],
    zs: [f32; 3],
) -> ([Pos2; 3], [f32; 3]) {
    let a = points[1] - points[0];
    let b = points[2] - points[0];
    if a.x * b.y - a.y * b.x < 0.0 {
        ([points[0], points[2], points[1]], [zs[0], zs[2], zs[1]])
    } else {
        (points, zs)
    }
}

fn triangle_barycentric(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> Option<[f32; 3]> {
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
        return None;
    }
    let inv = 1.0 / denom;
    let u = (dot11 * dot02 - dot01 * dot12) * inv;
    let v = (dot00 * dot12 - dot01 * dot02) * inv;
    let w = 1.0 - u - v;
    Some([w, v, u])
}

fn rasterize_bear_triangle(
    points: [Pos2; 3],
    zs: [f32; 3],
    color: Color32,
    origin: Pos2,
    width: usize,
    height: usize,
    z_buf: &mut [f32],
    pixels: &mut [Color32],
) {
    let min_x = points[0].x.min(points[1].x).min(points[2].x).floor() as i32;
    let max_x = points[0].x.max(points[1].x).max(points[2].x).ceil() as i32;
    let min_y = points[0].y.min(points[1].y).min(points[2].y).floor() as i32;
    let max_y = points[0].y.max(points[1].y).max(points[2].y).ceil() as i32;

    let x0 = origin.x.floor() as i32;
    let y0 = origin.y.floor() as i32;
    let x1 = x0 + width as i32 - 1;
    let y1 = y0 + height as i32 - 1;

    for y in min_y.max(y0)..=max_y.min(y1) {
        for x in min_x.max(x0)..=max_x.min(x1) {
            let sample = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
            let Some(bc) = triangle_barycentric(sample, points[0], points[1], points[2]) else {
                continue;
            };
            if bc[0] < -1e-4 || bc[1] < -1e-4 || bc[2] < -1e-4 {
                continue;
            }
            let depth = bc[0] * zs[0] + bc[1] * zs[1] + bc[2] * zs[2];
            let lx = x - x0;
            let ly = y - y0;
            let idx = ly as usize * width + lx as usize;
            if depth < z_buf[idx] {
                z_buf[idx] = depth;
                pixels[idx] = color;
            }
        }
    }
}

fn rasterize_bear(triangles: &[ProjectedBearTriangle], rect: Rect) -> ColorImage {
    let width = rect.width().ceil().max(1.0) as usize;
    let height = rect.height().ceil().max(1.0) as usize;
    let mut z_buf = vec![f32::INFINITY; width * height];
    let mut pixels = vec![Color32::TRANSPARENT; width * height];
    let origin = rect.min;

    for tri in triangles {
        rasterize_bear_triangle(
            tri.points,
            tri.zs,
            tri.color,
            origin,
            width,
            height,
            &mut z_buf,
            &mut pixels,
        );
    }

    ColorImage {
        size: [width, height],
        pixels,
    }
}

fn bear_triangle_visible(
    tri: &MeshTriangle,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
) -> bool {
    let view_normal = transform_vertex(tri.normal.normalize_or_zero(), right, up, forward);
    let normal_len_sq = view_normal.length_squared();
    if view_normal.z >= 0.0 || normal_len_sq < 1e-10 {
        return false;
    }

    let head_on = (-view_normal.z) / normal_len_sq.sqrt();
    head_on >= BEAR_TRIANGLE_CULL_DOT
}

fn color32_to_rgba(c: Color32) -> [f32; 4] {
    [
        c.r() as f32 / 255.0,
        c.g() as f32 / 255.0,
        c.b() as f32 / 255.0,
        c.a() as f32 / 255.0,
    ]
}

fn wind_triangle_clockwise_view(
    view_pts: [Vec3; 3],
    screen_pts: [Pos2; 3],
    zs: [f32; 3],
) -> ([Vec3; 3], [f32; 3]) {
    let a = screen_pts[1] - screen_pts[0];
    let b = screen_pts[2] - screen_pts[0];
    if a.x * b.y - a.y * b.x < 0.0 {
        ([view_pts[0], view_pts[2], view_pts[1]], [zs[0], zs[2], zs[1]])
    } else {
        (view_pts, zs)
    }
}

/// Build an indexed mesh for GPU bear rendering (same culling/shading as [`project_bear`]).
pub fn build_bear_gpu_mesh(cam: &Camera, center: Pos2, scale: f32) -> BearGpuMesh {
    let (right, up, forward) = view_cube_basis(cam);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut z_min = f32::INFINITY;
    let mut z_max = f32::NEG_INFINITY;

    for tri in bear_mesh() {
        let view_pts = tri.vertices.map(|v| transform_vertex(v, right, up, forward));
        if !bear_triangle_visible(tri, right, up, forward) {
            continue;
        }

        let screen_pts = view_pts.map(|v| project_to_hud(v, center, scale));
        let zs = [view_pts[0].z, view_pts[1].z, view_pts[2].z];
        let (screen_pts, zs) = wind_triangle_clockwise_with_z(screen_pts, zs);
        if tri_screen_area(screen_pts) < 0.25 {
            continue;
        }

        let (view_pts, _zs) = wind_triangle_clockwise_view(view_pts, screen_pts, zs);
        let color = color32_to_rgba(shade_bear_color(bear_world_normal(tri, right, up, forward)));
        let base = vertices.len() as u32;
        for (vp, z) in view_pts.into_iter().zip(zs) {
            z_min = z_min.min(z);
            z_max = z_max.max(z);
            vertices.push(GpuBearVertex {
                view_position: vp.to_array(),
                color,
            });
        }
        indices.extend([base, base + 1, base + 2]);
    }

    if vertices.is_empty() {
        z_min = 0.0;
        z_max = 1.0;
    }

    BearGpuMesh {
        vertices,
        indices,
        z_min,
        z_max,
    }
}

fn project_bear(cam: &Camera, center: Pos2, scale: f32) -> Vec<ProjectedBearTriangle> {
    let (right, up, forward) = view_cube_basis(cam);

    let mut triangles = Vec::new();
    for tri in bear_mesh() {
        let view_pts = tri.vertices.map(|v| transform_vertex(v, right, up, forward));
        if !bear_triangle_visible(tri, right, up, forward) {
            continue;
        }

        let zs = [view_pts[0].z, view_pts[1].z, view_pts[2].z];
        let (points, zs) = wind_triangle_clockwise_with_z(
            view_pts.map(|v| project_to_hud(v, center, scale)),
            zs,
        );
        if tri_screen_area(points) < 0.25 {
            continue;
        }

        let color = shade_bear_color(bear_world_normal(tri, right, up, forward));
        triangles.push(ProjectedBearTriangle { points, zs, color });
    }
    triangles
}

fn draw_bear(
    ui: &Ui,
    painter: &Painter,
    raster_rect: Rect,
    cam: &Camera,
    center: Pos2,
    scale: f32,
    render_state: Option<&eframe::egui_wgpu::RenderState>,
    gpu_bear: bool,
) {
    if gpu_bear {
        if let Some(render_state) = render_state {
            let mesh = build_bear_gpu_mesh(cam, center, scale);
            if !mesh.indices.is_empty() {
                let scene = crate::gpu_view_cube::BearGpuScene {
                    mesh,
                    rect: raster_rect,
                    center,
                    scale,
                };
                if crate::gpu_view_cube::paint(Some(render_state), painter, raster_rect, scene) {
                    return;
                }
            }
        }
    }

    let triangles = project_bear(cam, center, scale);
    if triangles.is_empty() {
        return;
    }
    let image = rasterize_bear(&triangles, raster_rect);
    let ctx = ui.ctx();
    let bear_tex_storage = Id::new("view_cube_bear_raster");

    // `load_texture` allocates once; per-frame updates use `TextureHandle::set`.
    // Calling `load_texture` every frame destroys the GPU texture while wgpu may
    // still reference it from the previous frame.
    let texture_id = if let Some(mut handle) =
        ctx.data(|d| d.get_temp::<TextureHandle>(bear_tex_storage))
    {
        handle.set(image, TextureOptions::NEAREST);
        let id = handle.id();
        ctx.data_mut(|d| d.insert_temp(bear_tex_storage, handle));
        id
    } else {
        let handle = ctx.load_texture(
            "view_cube_bear_raster",
            image,
            TextureOptions::NEAREST,
        );
        let id = handle.id();
        ctx.data_mut(|d| d.insert_temp(bear_tex_storage, handle));
        id
    };

    painter.image(
        texture_id,
        raster_rect,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CubeEdgeId {
    FrontBottom,
    RightBottom,
    BackBottom,
    LeftBottom,
    FrontTop,
    RightTop,
    BackTop,
    LeftTop,
    FrontLeft,
    FrontRight,
    BackRight,
    BackLeft,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CubeCornerId {
    FrontLeftBottom,
    FrontRightBottom,
    BackRightBottom,
    BackLeftBottom,
    FrontLeftTop,
    FrontRightTop,
    BackRightTop,
    BackLeftTop,
}

#[derive(Clone, Copy)]
struct CubeEdge {
    id: CubeEdgeId,
    a: Vec3,
    b: Vec3,
    /// Outward normals of the two faces that meet on this edge.
    normals: [Vec3; 2],
}

#[derive(Clone, Copy)]
struct CubeCorner {
    id: CubeCornerId,
    pos: Vec3,
    /// Outward normals of the three faces that meet at this corner.
    normals: [Vec3; 3],
}

const EDGES: [CubeEdge; 12] = [
    CubeEdge {
        id: CubeEdgeId::FrontBottom,
        a: Vec3::new(-HALF, -HALF, -HALF),
        b: Vec3::new(HALF, -HALF, -HALF),
        normals: [Vec3::NEG_Y, Vec3::NEG_Z],
    },
    CubeEdge {
        id: CubeEdgeId::RightBottom,
        a: Vec3::new(HALF, -HALF, -HALF),
        b: Vec3::new(HALF, HALF, -HALF),
        normals: [Vec3::X, Vec3::NEG_Z],
    },
    CubeEdge {
        id: CubeEdgeId::BackBottom,
        a: Vec3::new(HALF, HALF, -HALF),
        b: Vec3::new(-HALF, HALF, -HALF),
        normals: [Vec3::Y, Vec3::NEG_Z],
    },
    CubeEdge {
        id: CubeEdgeId::LeftBottom,
        a: Vec3::new(-HALF, HALF, -HALF),
        b: Vec3::new(-HALF, -HALF, -HALF),
        normals: [Vec3::NEG_X, Vec3::NEG_Z],
    },
    CubeEdge {
        id: CubeEdgeId::FrontTop,
        a: Vec3::new(-HALF, -HALF, HALF),
        b: Vec3::new(HALF, -HALF, HALF),
        normals: [Vec3::NEG_Y, Vec3::Z],
    },
    CubeEdge {
        id: CubeEdgeId::RightTop,
        a: Vec3::new(HALF, -HALF, HALF),
        b: Vec3::new(HALF, HALF, HALF),
        normals: [Vec3::X, Vec3::Z],
    },
    CubeEdge {
        id: CubeEdgeId::BackTop,
        a: Vec3::new(HALF, HALF, HALF),
        b: Vec3::new(-HALF, HALF, HALF),
        normals: [Vec3::Y, Vec3::Z],
    },
    CubeEdge {
        id: CubeEdgeId::LeftTop,
        a: Vec3::new(-HALF, HALF, HALF),
        b: Vec3::new(-HALF, -HALF, HALF),
        normals: [Vec3::NEG_X, Vec3::Z],
    },
    CubeEdge {
        id: CubeEdgeId::FrontLeft,
        a: Vec3::new(-HALF, -HALF, -HALF),
        b: Vec3::new(-HALF, -HALF, HALF),
        normals: [Vec3::NEG_Y, Vec3::NEG_X],
    },
    CubeEdge {
        id: CubeEdgeId::FrontRight,
        a: Vec3::new(HALF, -HALF, -HALF),
        b: Vec3::new(HALF, -HALF, HALF),
        normals: [Vec3::NEG_Y, Vec3::X],
    },
    CubeEdge {
        id: CubeEdgeId::BackRight,
        a: Vec3::new(HALF, HALF, -HALF),
        b: Vec3::new(HALF, HALF, HALF),
        normals: [Vec3::Y, Vec3::X],
    },
    CubeEdge {
        id: CubeEdgeId::BackLeft,
        a: Vec3::new(-HALF, HALF, -HALF),
        b: Vec3::new(-HALF, HALF, HALF),
        normals: [Vec3::Y, Vec3::NEG_X],
    },
];

const CORNERS: [CubeCorner; 8] = [
    CubeCorner {
        id: CubeCornerId::FrontLeftBottom,
        pos: Vec3::new(-HALF, -HALF, -HALF),
        normals: [Vec3::NEG_Y, Vec3::NEG_X, Vec3::NEG_Z],
    },
    CubeCorner {
        id: CubeCornerId::FrontRightBottom,
        pos: Vec3::new(HALF, -HALF, -HALF),
        normals: [Vec3::NEG_Y, Vec3::X, Vec3::NEG_Z],
    },
    CubeCorner {
        id: CubeCornerId::BackRightBottom,
        pos: Vec3::new(HALF, HALF, -HALF),
        normals: [Vec3::Y, Vec3::X, Vec3::NEG_Z],
    },
    CubeCorner {
        id: CubeCornerId::BackLeftBottom,
        pos: Vec3::new(-HALF, HALF, -HALF),
        normals: [Vec3::Y, Vec3::NEG_X, Vec3::NEG_Z],
    },
    CubeCorner {
        id: CubeCornerId::FrontLeftTop,
        pos: Vec3::new(-HALF, -HALF, HALF),
        normals: [Vec3::NEG_Y, Vec3::NEG_X, Vec3::Z],
    },
    CubeCorner {
        id: CubeCornerId::FrontRightTop,
        pos: Vec3::new(HALF, -HALF, HALF),
        normals: [Vec3::NEG_Y, Vec3::X, Vec3::Z],
    },
    CubeCorner {
        id: CubeCornerId::BackRightTop,
        pos: Vec3::new(HALF, HALF, HALF),
        normals: [Vec3::Y, Vec3::X, Vec3::Z],
    },
    CubeCorner {
        id: CubeCornerId::BackLeftTop,
        pos: Vec3::new(-HALF, HALF, HALF),
        normals: [Vec3::Y, Vec3::NEG_X, Vec3::Z],
    },
];

fn combine_normals(parts: &[Vec3]) -> Vec3 {
    let mut sum = Vec3::ZERO;
    for n in parts {
        sum += *n;
    }
    sum.normalize_or_zero()
}

pub fn edge_view_direction(id: CubeEdgeId) -> Vec3 {
    let def = EDGES.iter().find(|e| e.id == id).expect("edge");
    combine_normals(&def.normals)
}

pub fn corner_view_direction(id: CubeCornerId) -> Vec3 {
    let def = CORNERS.iter().find(|c| c.id == id).expect("corner");
    combine_normals(&def.normals)
}

impl CubeEdgeId {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "front_bottom" | "frontbottom" | "fb" => Some(Self::FrontBottom),
            "right_bottom" | "rightbottom" | "rb" => Some(Self::RightBottom),
            "back_bottom" | "backbottom" | "bb" => Some(Self::BackBottom),
            "left_bottom" | "leftbottom" | "lb" => Some(Self::LeftBottom),
            "front_top" | "fronttop" | "ft" => Some(Self::FrontTop),
            "right_top" | "righttop" | "rt" => Some(Self::RightTop),
            "back_top" | "backtop" | "bt" => Some(Self::BackTop),
            "left_top" | "lefttop" | "lt" => Some(Self::LeftTop),
            "front_left" | "frontleft" | "fl" => Some(Self::FrontLeft),
            "front_right" | "frontright" | "fr" => Some(Self::FrontRight),
            "back_right" | "backright" | "br" => Some(Self::BackRight),
            "back_left" | "backleft" | "bl" => Some(Self::BackLeft),
            _ => None,
        }
    }
}

impl CubeCornerId {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "front_left_bottom" | "frontleftbottom" | "flb" => Some(Self::FrontLeftBottom),
            "front_right_bottom" | "frontrightbottom" | "frb" => Some(Self::FrontRightBottom),
            "back_right_bottom" | "backrightbottom" | "brb" => Some(Self::BackRightBottom),
            "back_left_bottom" | "backleftbottom" | "blb" => Some(Self::BackLeftBottom),
            "front_left_top" | "frontlefttop" | "flt" => Some(Self::FrontLeftTop),
            "front_right_top" | "frontrighttop" | "frt" => Some(Self::FrontRightTop),
            "back_right_top" | "backrighttop" | "brt" => Some(Self::BackRightTop),
            "back_left_top" | "backlefttop" | "blt" => Some(Self::BackLeftTop),
            _ => None,
        }
    }
}

struct ProjectedEdge {
    id: CubeEdgeId,
    a: Pos2,
    b: Pos2,
    depth: f32,
}

struct ProjectedCorner {
    id: CubeCornerId,
    pos: Pos2,
    depth: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CubePick {
    Corner(CubeCornerId),
    Edge(CubeEdgeId),
    Face(StandardView),
}

struct ProjectedAxis {
    label: &'static str,
    color: Color32,
    from: Pos2,
    to: Pos2,
    depth: f32,
}

fn view_cube_basis(cam: &Camera) -> (Vec3, Vec3, Vec3) {
    let forward = (cam.target - cam.eye()).normalize();
    let mut right = forward.cross(Vec3::Z);
    if right.length_squared() < 1e-8 {
        right = Vec3::new(cam.yaw.cos(), cam.yaw.sin(), 0.0);
    } else {
        right = right.normalize();
    }
    let up = right.cross(forward).normalize();
    (right, up, forward)
}

fn transform_vertex(v: Vec3, right: Vec3, up: Vec3, forward: Vec3) -> Vec3 {
    Vec3::new(v.dot(right), v.dot(up), v.dot(forward))
}

fn face_normal(corners: [Vec3; 4]) -> Vec3 {
    let e0 = corners[1] - corners[0];
    let e1 = corners[3] - corners[0];
    e0.cross(e1).normalize()
}

fn project_to_hud(v: Vec3, center: Pos2, scale: f32) -> Pos2 {
    Pos2::new(center.x + v.x * scale, center.y - v.y * scale)
}

/// Max screen-space radius of the cube silhouette for the current orientation.
fn cube_silhouette_radius(right: Vec3, up: Vec3, forward: Vec3, scale: f32) -> f32 {
    let mut max_r = 0.0f32;
    for face in &FACES {
        for corner in &face.corners {
            let t = transform_vertex(*corner, right, up, forward);
            let r = (t.x * t.x + t.y * t.y).sqrt() * scale;
            max_r = max_r.max(r);
        }
    }
    max_r
}

fn clamp_point_to_silhouette(p: Pos2, center: Pos2, max_r: f32) -> Pos2 {
    let d = p - center;
    let len = d.length();
    if len > max_r {
        center + d * (max_r / len)
    } else {
        p
    }
}

fn face_head_on(
    corners: [Vec3; 4],
    right: Vec3,
    up: Vec3,
    forward: Vec3,
) -> f32 {
    let n = face_normal(corners);
    let n_view = transform_vertex(n, right, up, forward);
    // 1.0 = face perpendicular to view, 0.0 = edge-on (the case that flares).
    n_view.z.abs()
}

fn face_visible_in_view(
    corners: [Vec3; 4],
    right: Vec3,
    up: Vec3,
    forward: Vec3,
) -> bool {
    let n = face_normal(corners);
    if !face_visible_for_normal(n, right, up, forward) {
        return false;
    }
    let head_on = face_head_on(corners, right, up, forward);
    if head_on < FACE_CULL_DOT {
        return false;
    }
    let mut min_z = f32::MAX;
    let mut max_z = f32::MIN;
    for corner in corners {
        let t = transform_vertex(corner, right, up, forward);
        min_z = min_z.min(t.z);
        max_z = max_z.max(t.z);
    }
    // Grazing faces that cross the view plane flare when orthographically projected.
    if min_z < 0.0 && max_z > 0.0 {
        return head_on > 0.5;
    }
    true
}

fn point_in_tri(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
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
    u >= 0.0 && v >= 0.0 && u + v <= 1.0
}

fn point_in_quad(p: Pos2, quad: [Pos2; 4]) -> bool {
    point_in_tri(p, quad[0], quad[1], quad[2]) || point_in_tri(p, quad[0], quad[2], quad[3])
}

fn project_point(
    world: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
    center: Pos2,
    scale: f32,
) -> (Pos2, f32) {
    let view = transform_vertex(world, right, up, forward);
    (project_to_hud(view, center, scale), view.z)
}

fn project_axes(cam: &Camera, center: Pos2, scale: f32) -> Vec<ProjectedAxis> {
    let (right, up, forward) = view_cube_basis(cam);
    let mut axes = Vec::with_capacity(AXES.len());
    let (origin, origin_depth) =
        project_point(AXIS_ORIGIN, right, up, forward, center, scale);
    for axis in &AXES {
        let end_world = AXIS_ORIGIN + axis.direction * AXIS_LENGTH;
        let (end, end_depth) = project_point(end_world, right, up, forward, center, scale);
        axes.push(ProjectedAxis {
            label: axis.label,
            color: axis.color,
            from: origin,
            to: end,
            depth: (origin_depth + end_depth) * 0.5,
        });
    }
    axes.sort_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal));
    axes
}

fn draw_axes(ui: &mut Ui, axes: &[ProjectedAxis]) {
    let painter = ui.painter();
    for axis in axes {
        painter.line_segment(
            [axis.from, axis.to],
            Stroke::new(AXIS_STROKE, axis.color),
        );
        let galley = ui.fonts(|fonts| {
            fonts.layout_no_wrap(
                axis.label.to_owned(),
                FontId::proportional(9.0),
                axis.color,
            )
        });
        let tip = axis.to;
        let dir = (axis.to - axis.from).normalized();
        let offset = if dir.length_sq() > 1e-6 {
            dir * 3.0
        } else {
            Vec2::ZERO
        };
        painter.add(
            TextShape::new(
                tip + offset - galley.size() * 0.5,
                galley,
                axis.color,
            )
            .with_override_text_color(axis.color),
        );
    }
}

fn project_faces(cam: &Camera, center: Pos2, scale: f32) -> Vec<ProjectedFace> {
    let (right, up, forward) = view_cube_basis(cam);
    let max_r = cube_silhouette_radius(right, up, forward, scale);
    let mut faces = Vec::with_capacity(FACES.len());
    for face in &FACES {
        if !face_visible_in_view(face.corners, right, up, forward) {
            continue;
        }
        let mut depth = 0.0;
        let mut points = [Pos2::ZERO; 4];
        for (i, corner) in face.corners.iter().enumerate() {
            let t = transform_vertex(*corner, right, up, forward);
            depth += t.z;
            points[i] = clamp_point_to_silhouette(
                project_to_hud(t, center, scale),
                center,
                max_r,
            );
        }
        depth /= 4.0;
        faces.push(ProjectedFace {
            view: face.view,
            points,
            depth,
        });
    }
    // Paint farther faces first; nearer faces (smaller depth) win hit tests.
    faces.sort_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal));
    faces
}

fn pick_face(faces: &[ProjectedFace], pos: Pos2) -> Option<StandardView> {
    let mut best: Option<&ProjectedFace> = None;
    for face in faces.iter().rev() {
        if point_in_quad(pos, face.points) {
            best = Some(face);
            break;
        }
    }
    best.map(|f| f.view)
}

fn edge_visible(edge: &CubeEdge, right: Vec3, up: Vec3, forward: Vec3) -> bool {
    face_visible_for_normal(edge.normals[0], right, up, forward)
        || face_visible_for_normal(edge.normals[1], right, up, forward)
}

fn face_visible_for_normal(normal: Vec3, right: Vec3, up: Vec3, forward: Vec3) -> bool {
    let n_view = transform_vertex(normal, right, up, forward);
    n_view.z < 0.0
}

fn project_edges(cam: &Camera, center: Pos2, scale: f32) -> Vec<ProjectedEdge> {
    let (right, up, forward) = view_cube_basis(cam);
    let max_r = cube_silhouette_radius(right, up, forward, scale);
    let mut edges = Vec::with_capacity(EDGES.len());
    for edge in &EDGES {
        if !edge_visible(edge, right, up, forward) {
            continue;
        }
        let (a, za) = project_point(edge.a, right, up, forward, center, scale);
        let (b, zb) = project_point(edge.b, right, up, forward, center, scale);
        let a = clamp_point_to_silhouette(a, center, max_r);
        let b = clamp_point_to_silhouette(b, center, max_r);
        edges.push(ProjectedEdge {
            id: edge.id,
            a,
            b,
            depth: (za + zb) * 0.5,
        });
    }
    edges.sort_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal));
    edges
}

fn project_corners(cam: &Camera, center: Pos2, scale: f32) -> Vec<ProjectedCorner> {
    let (right, up, forward) = view_cube_basis(cam);
    let max_r = cube_silhouette_radius(right, up, forward, scale);
    let mut corners = Vec::with_capacity(CORNERS.len());
    for corner in &CORNERS {
        let outward_visible = corner
            .normals
            .iter()
            .filter(|n| face_visible_for_normal(**n, right, up, forward))
            .count();
        if outward_visible < 2 {
            continue;
        }
        let (pos, depth) = project_point(corner.pos, right, up, forward, center, scale);
        corners.push(ProjectedCorner {
            id: corner.id,
            pos: clamp_point_to_silhouette(pos, center, max_r),
            depth,
        });
    }
    corners.sort_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal));
    corners
}

fn dist_point_to_segment(p: Pos2, a: Pos2, b: Pos2) -> (f32, f32) {
    let ab = b - a;
    let ap = p - a;
    let len_sq = ab.length_sq();
    if len_sq < 1e-8 {
        return (ap.length(), 0.0);
    }
    let t = (ap.dot(ab) / len_sq).clamp(0.0, 1.0);
    let closest = a + ab * t;
    ((p - closest).length(), t)
}

fn pick_priority(pick: CubePick) -> u8 {
    match pick {
        CubePick::Corner(_) => 0,
        CubePick::Edge(_) => 1,
        CubePick::Face(_) => 2,
    }
}

fn pick_cube(
    faces: &[ProjectedFace],
    edges: &[ProjectedEdge],
    corners: &[ProjectedCorner],
    pos: Pos2,
) -> Option<CubePick> {
    let mut best: Option<(CubePick, f32, f32)> = None;

    let mut consider = |pick: CubePick, depth: f32, dist: f32| {
        let better = match best {
            None => true,
            Some((bp, bd, bdist)) => {
                let pp = pick_priority(pick);
                let bpp = pick_priority(bp);
                pp < bpp
                    || (pp == bpp
                        && (depth < bd - 0.01
                            || ((depth - bd).abs() < 0.01 && dist < bdist)))
            }
        };
        if better {
            best = Some((pick, depth, dist));
        }
    };

    for corner in corners.iter().rev() {
        let dist = (pos - corner.pos).length();
        if dist <= CORNER_HIT_RADIUS {
            consider(CubePick::Corner(corner.id), corner.depth, dist);
        }
    }

    for edge in edges.iter().rev() {
        let (dist, t) = dist_point_to_segment(pos, edge.a, edge.b);
        if dist <= EDGE_HIT_RADIUS {
            consider(CubePick::Edge(edge.id), edge.depth, dist);
        }
        let _ = t;
    }

    if let Some(view) = pick_face(faces, pos) {
        if let Some(face) = faces.iter().find(|f| f.view == view) {
            consider(CubePick::Face(view), face.depth, 0.0);
        }
    }

    best.map(|(pick, _, _)| pick)
}

fn apply_cube_pick(cam: &mut Camera, pick: CubePick) {
    match pick {
        CubePick::Face(view) => cam.start_view_transition(view, VIEW_TRANSITION_DURATION),
        CubePick::Edge(id) => cam.start_view_transition_to_direction(
            edge_view_direction(id),
            VIEW_TRANSITION_DURATION,
        ),
        CubePick::Corner(id) => cam.start_view_transition_to_direction(
            corner_view_direction(id),
            VIEW_TRANSITION_DURATION,
        ),
    }
}

fn draw_hovered_edge(painter: &egui::Painter, edges: &[ProjectedEdge], id: CubeEdgeId) {
    let Some(edge) = edges.iter().find(|e| e.id == id) else {
        return;
    };
    painter.line_segment(
        [edge.a, edge.b],
        Stroke::new(EDGE_STROKE_HOVER, BBOX_HOVER_STROKE),
    );
}

fn draw_hovered_corner(painter: &egui::Painter, corners: &[ProjectedCorner], id: CubeCornerId) {
    let Some(corner) = corners.iter().find(|c| c.id == id) else {
        return;
    };
    painter.circle_filled(corner.pos, CORNER_RADIUS_HOVER, BBOX_HOVER_STROKE);
    painter.circle_stroke(
        corner.pos,
        CORNER_RADIUS_HOVER + 1.0,
        Stroke::new(1.0, Color32::from_gray(220)),
    );
}

fn draw_hovered_face(painter: &egui::Painter, face: &ProjectedFace) {
    let stroke = Stroke::new(FACE_STROKE_HOVER, BBOX_HOVER_STROKE);
    let points = face.points;
    for i in 0..4 {
        let j = (i + 1) % 4;
        painter.line_segment([points[i], points[j]], stroke);
    }
}

fn view_preset_toggle_rect(pad_rect: Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(
            pad_rect.min.x + PRESET_TOGGLE_MARGIN,
            pad_rect.max.y - PRESET_TOGGLE_SIZE - PRESET_TOGGLE_MARGIN,
        ),
        Vec2::splat(PRESET_TOGGLE_SIZE),
    )
}

fn view_home_toggle_rect(pad_rect: Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(
            pad_rect.max.x - PRESET_TOGGLE_SIZE - PRESET_TOGGLE_MARGIN,
            pad_rect.max.y - PRESET_TOGGLE_SIZE - PRESET_TOGGLE_MARGIN,
        ),
        Vec2::splat(PRESET_TOGGLE_SIZE),
    )
}

fn projection_toggle_icon_rect(button: Rect) -> Rect {
    button.shrink(PRESET_TOGGLE_ICON_PAD)
}

fn icon_point(rect: Rect, u: f32, v: f32) -> Pos2 {
    Pos2::new(
        rect.min.x + rect.width() * u,
        rect.min.y + rect.height() * v,
    )
}

fn orthographic_icon_rect(rect: Rect) -> Rect {
    rect.shrink(rect.width() * 0.22)
}

fn natural_icon_segments(rect: Rect) -> [(Pos2, Pos2); 4] {
    let p = |u: f32, v: f32| icon_point(rect, u, v);
    let bl = p(0.20, 0.78);
    let br = p(0.80, 0.78);
    let tl = p(0.30, 0.24);
    let tr = p(0.70, 0.24);
    [(bl, br), (br, tr), (tr, tl), (tl, bl)]
}

fn paint_icon_segments(painter: &Painter, segments: &[(Pos2, Pos2)], color: Color32) {
    let stroke = Stroke::new(PRESET_TOGGLE_ICON_STROKE, color);
    for &(a, b) in segments {
        painter.line_segment([a, b], stroke);
    }
}

/// Flat square — parallel projection has no vanishing point.
fn paint_orthographic_icon(painter: &Painter, rect: Rect, color: Color32) {
    painter.rect_stroke(
        orthographic_icon_rect(rect),
        1.0,
        Stroke::new(PRESET_TOGGLE_ICON_STROKE, color),
    );
}

/// Perspective trapezoid — converging edges.
fn paint_natural_icon(painter: &Painter, rect: Rect, color: Color32) {
    let segments = natural_icon_segments(rect);
    paint_icon_segments(painter, &segments, color);
}

fn paint_home_icon(painter: &Painter, rect: Rect, color: Color32) {
    let p = |u: f32, v: f32| icon_point(rect, u, v);
    let stroke = Stroke::new(PRESET_TOGGLE_ICON_STROKE, color);
    let peak = p(0.5, 0.18);
    let eave_l = p(0.22, 0.42);
    let eave_r = p(0.78, 0.42);
    let base_l = p(0.30, 0.82);
    let base_r = p(0.70, 0.82);
    let door_top = p(0.46, 0.58);
    let door_bl = p(0.46, 0.82);
    let door_br = p(0.54, 0.82);
    painter.line_segment([eave_l, peak], stroke);
    painter.line_segment([peak, eave_r], stroke);
    painter.line_segment([eave_l, base_l], stroke);
    painter.line_segment([eave_r, base_r], stroke);
    painter.line_segment([base_l, base_r], stroke);
    painter.line_segment([door_top, door_bl], stroke);
    painter.line_segment([door_bl, door_br], stroke);
    painter.line_segment([door_br, door_top], stroke);
}

fn paint_icon_toggle_button(ui: &Ui, rect: Rect, hovered: bool, pressed: bool) {
    let fill = if pressed {
        Color32::from_gray(42)
    } else if hovered {
        Color32::from_gray(34)
    } else {
        Color32::from_rgba_unmultiplied(26, 28, 34, 220)
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    ui.painter().rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, Color32::from_gray(if hovered { 110 } else { 72 })),
    );
}

fn show_icon_toggle_button(
    ui: &mut Ui,
    rect: Rect,
    hover_hint: &str,
    paint_icon: impl FnOnce(&Painter, Rect, Color32),
    on_click: impl FnOnce(),
) {
    let response = ui.allocate_rect(rect, Sense::click());
    let hovered = response.hovered();
    let clicked = response.clicked();
    let pressed = response.is_pointer_button_down_on();

    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        response.on_hover_text(hover_hint);
    }

    paint_icon_toggle_button(ui, rect, hovered, pressed);
    paint_icon(ui.painter(), rect, Color32::from_gray(210));

    if clicked {
        on_click();
    }
}

fn paint_projection_mode_icon(painter: &Painter, button: Rect, mode: ProjectionMode) {
    let color = Color32::from_gray(210);
    let icon_rect = projection_toggle_icon_rect(button);
    match mode {
        ProjectionMode::Orthographic => paint_orthographic_icon(painter, icon_rect, color),
        ProjectionMode::Natural => paint_natural_icon(painter, icon_rect, color),
    }
}

fn show_projection_mode_toggle(ui: &mut Ui, cam: &mut Camera, pad_rect: Rect) {
    let rect = view_preset_toggle_rect(pad_rect);
    let active = cam.projection_mode();
    let target = active.opposite();
    let hint = match target {
        ProjectionMode::Orthographic => "Orthographic projection",
        ProjectionMode::Natural => "Natural (perspective) projection",
    };
    show_icon_toggle_button(ui, rect, hint, |painter, button, color| {
        paint_projection_mode_icon(painter, button, target);
        let _ = color;
    }, || {
        cam.set_projection_mode(target);
    });
}

fn show_home_button(ui: &mut Ui, cam: &mut Camera, pad_rect: Rect) {
    let rect = view_home_toggle_rect(pad_rect);
    let response = ui.allocate_rect(rect, Sense::click());
    let hovered = response.hovered();
    let pressed = response.is_pointer_button_down_on();

    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        response.clone().on_hover_text("Home view");
    }

    paint_icon_toggle_button(ui, rect, hovered, pressed);
    paint_home_icon(ui.painter(), rect, Color32::from_gray(210));

    response.context_menu(|ui| {
        if ui.button("Set current view as home").clicked() {
            cam.set_home_from_current();
            ui.close_menu();
        }
    });

    if response.clicked() {
        cam.start_home_transition(VIEW_TRANSITION_DURATION);
    }
}

fn cube_rect_in_viewport(viewport: Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(
            viewport.max.x - CUBE_SIZE - CUBE_MARGIN,
            viewport.min.y + CUBE_MARGIN,
        ),
        Vec2::splat(CUBE_SIZE),
    )
}

/// Show the view-cube HUD overlay in the top-right of `viewport`.
pub fn show_hud(
    ctx: &egui::Context,
    cam: &mut Camera,
    viewport: Rect,
    render_state: Option<&eframe::egui_wgpu::RenderState>,
    gpu_bear: bool,
) {
    let screen_rect = cube_rect_in_viewport(viewport);
    egui::Area::new(egui::Id::new("view_cube_hud"))
        .fixed_pos(screen_rect.min)
        .order(egui::Order::Foreground)
        .interactable(true)
        .constrain(false)
        .show(ctx, |ui| {
            show(ui, cam, screen_rect, render_state, gpu_bear);
        });
}

/// Draw and handle input for the view-cube HUD. All geometry uses screen coordinates.
fn show(
    ui: &mut Ui,
    cam: &mut Camera,
    screen_rect: Rect,
    render_state: Option<&eframe::egui_wgpu::RenderState>,
    gpu_bear: bool,
) {
    let center = screen_rect.center();
    let scale = CUBE_SIZE * 0.42;

    let faces = project_faces(cam, center, scale);
    let edges = project_edges(cam, center, scale);
    let corners = project_corners(cam, center, scale);

    let response = ui.allocate_rect(screen_rect, Sense::click_and_drag());

    let hover_pick = response
        .hover_pos()
        .and_then(|p| pick_cube(&faces, &edges, &corners, p));
    let hover_edge = match hover_pick {
        Some(CubePick::Edge(id)) => Some(id),
        _ => None,
    };
    let hover_corner = match hover_pick {
        Some(CubePick::Corner(id)) => Some(id),
        _ => None,
    };
    let hover_face = match hover_pick {
        Some(CubePick::Face(view)) => Some(view),
        _ => None,
    };

    if response.hovered() {
        ui.ctx().set_cursor_icon(if hover_pick.is_some() {
            egui::CursorIcon::PointingHand
        } else {
            egui::CursorIcon::Grab
        });
    }

    if response.dragged() {
        cam.orbit_trackball(response.drag_delta());
    }

    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            if response.drag_delta().length() < DRAG_CLICK_THRESHOLD {
                if let Some(pick) = pick_cube(&faces, &edges, &corners, pos) {
                    apply_cube_pick(cam, pick);
                }
            }
        }
    }

    let pad_rect = screen_rect.expand(4.0);
    {
        let painter = ui.painter();
        painter.rect_filled(pad_rect, 6.0, Color32::from_rgba_unmultiplied(18, 20, 26, 200));
        painter.rect_stroke(pad_rect, 6.0, Stroke::new(1.0, Color32::from_gray(70)));
    }

    let axes = project_axes(cam, center, scale);
    draw_axes(ui, &axes);

    let painter = ui.painter();
    draw_bear(
        ui,
        painter,
        screen_rect,
        cam,
        center,
        scale,
        render_state,
        gpu_bear,
    );
    if let Some(view) = hover_face {
        if let Some(face) = faces.iter().find(|f| f.view == view) {
            draw_hovered_face(painter, face);
        }
    }
    if let Some(id) = hover_edge {
        draw_hovered_edge(painter, &edges, id);
    }
    if let Some(id) = hover_corner {
        draw_hovered_corner(painter, &corners, id);
    }
    show_projection_mode_toggle(ui, cam, pad_rect);
    show_home_button(ui, cam, pad_rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_rect_is_in_viewport_top_right() {
        let vp = Rect::from_min_size(Pos2::new(0.0, 40.0), Vec2::new(800.0, 600.0));
        let cube = cube_rect_in_viewport(vp);
        assert!(cube.max.x <= vp.max.x);
        assert!(cube.min.y >= vp.min.y);
        assert!((cube.width() - CUBE_SIZE).abs() < 0.01);
    }

    fn cam_at_view(view: StandardView) -> Camera {
        let (yaw, pitch) = view.yaw_pitch();
        let mut cam = Camera::default();
        cam.yaw = yaw;
        cam.pitch = pitch;
        cam
    }

    fn face_center(face: &ProjectedFace) -> Pos2 {
        Pos2::new(
            (face.points[0].x + face.points[1].x + face.points[2].x + face.points[3].x) * 0.25,
            (face.points[0].y + face.points[1].y + face.points[2].y + face.points[3].y) * 0.25,
        )
    }

    #[test]
    fn front_face_visible_from_front_view() {
        let cam = cam_at_view(StandardView::Front);
        let center = Pos2::new(100.0, 100.0);
        let faces = project_faces(&cam, center, 40.0);
        assert!(faces.iter().any(|f| f.view == StandardView::Front));
        let front = faces
            .iter()
            .find(|f| f.view == StandardView::Front)
            .expect("front face");
        assert!(point_in_quad(face_center(front), front.points));
    }

    #[test]
    fn edge_view_direction_averages_adjacent_face_normals() {
        let dir = edge_view_direction(CubeEdgeId::FrontTop);
        let expected = Vec3::new(0.0, -1.0, 1.0).normalize();
        assert!((dir - expected).length() < 0.01);
    }

    #[test]
    fn corner_view_direction_averages_three_face_normals() {
        let dir = corner_view_direction(CubeCornerId::FrontRightTop);
        let expected = Vec3::new(1.0, -1.0, 1.0).normalize();
        assert!((dir - expected).length() < 0.01);
    }

    #[test]
    fn dist_point_to_segment_finds_perpendicular_foot() {
        let (dist, t) = dist_point_to_segment(
            Pos2::new(10.0, 5.0),
            Pos2::new(0.0, 0.0),
            Pos2::new(20.0, 0.0),
        );
        assert!((dist - 5.0).abs() < 0.01);
        assert!((t - 0.5).abs() < 0.01);
    }

    #[test]
    fn pick_corner_beats_face_at_front_top_right() {
        // Isometric view exposes ≥2 outward faces at FRT; pure front view culls it.
        let cam = Camera::default();
        let center = Pos2::new(120.0, 120.0);
        let scale = 40.0;
        let faces = project_faces(&cam, center, scale);
        let edges = project_edges(&cam, center, scale);
        let corners = project_corners(&cam, center, scale);
        let frt = corners
            .iter()
            .find(|c| c.id == CubeCornerId::FrontRightTop)
            .expect("front-right-top corner");
        let pick = pick_cube(&faces, &edges, &corners, frt.pos).expect("pick");
        assert_eq!(pick, CubePick::Corner(CubeCornerId::FrontRightTop));
    }

    #[test]
    fn pick_face_finds_front_at_center() {
        let cam = cam_at_view(StandardView::Front);
        let center = Pos2::new(120.0, 120.0);
        let faces = project_faces(&cam, center, 40.0);
        let front = faces
            .iter()
            .find(|f| f.view == StandardView::Front)
            .expect("front");
        assert_eq!(pick_face(&faces, face_center(front)), Some(StandardView::Front));
    }

    #[test]
    fn top_view_cube_drag_down_reveals_back_face() {
        let mut cam = cam_at_view(StandardView::Top);
        cam.orbit_trackball(egui::vec2(0.0, 90.0));
        let center = Pos2::new(120.0, 120.0);
        let faces = project_faces(&cam, center, 40.0);
        assert!(
            faces.iter().any(|f| f.view == StandardView::Back),
            "pulling the top face down should bring the back face into view"
        );
    }

    #[test]
    fn bear_mesh_is_scaled_to_fill_hud() {
        let mut max_abs = 0.0f32;
        for tri in bear_mesh() {
            for vertex in tri.vertices {
                max_abs = max_abs
                    .max(vertex.x.abs())
                    .max(vertex.y.abs())
                    .max(vertex.z.abs());
            }
        }
        assert!(
            max_abs > HALF,
            "bear should extend past the unit cube to fill the HUD, got {max_abs}"
        );
    }

    #[test]
    fn shade_bear_color_is_brightest_facing_light() {
        let light = BEAR_LIGHT_DIR.normalize();
        let lit = shade_bear_color(light);
        let away = shade_bear_color(-light);
        let ambient = shade_bear_color(Vec3::new(0.0, 1.0, 0.0));
        assert!(lit.r() > away.r());
        assert!(lit.r() > ambient.r());
        assert!(away.r() >= ambient.r());
    }

    #[test]
    fn bear_projection_uses_per_triangle_shading() {
        let cam = Camera::default();
        let center = Pos2::new(120.0, 120.0);
        let triangles = project_bear(&cam, center, 40.0);
        let first = triangles.first().expect("triangles").color;
        assert!(
            triangles.iter().any(|t| t.color != first),
            "bear shading should vary across facets"
        );
    }

    #[test]
    fn bear_projects_visible_triangles_from_default_view() {
        let cam = Camera::default();
        let center = Pos2::new(120.0, 120.0);
        let triangles = project_bear(&cam, center, 40.0);
        assert!(!triangles.is_empty(), "bear should have visible triangles");
    }

    #[test]
    fn bear_gpu_mesh_matches_cpu_triangle_count() {
        let cam = Camera::default();
        let center = Pos2::new(120.0, 120.0);
        let scale = 40.0;
        let cpu = project_bear(&cam, center, scale);
        let gpu = build_bear_gpu_mesh(&cam, center, scale);
        assert_eq!(
            gpu.indices.len() / 3,
            cpu.len(),
            "GPU mesh should include the same visible triangles as CPU projection"
        );
        assert!(!gpu.vertices.is_empty());
        assert!(gpu.z_min <= gpu.z_max);
    }

    #[test]
    fn bear_gpu_mesh_builds_for_many_orientations() {
        let center = Pos2::new(60.0, 60.0);
        let scale = CUBE_SIZE * 0.42;
        for yaw in [0.0, 0.8, 2.2] {
            for pitch in [-0.5, 0.0, 0.6] {
                let mut cam = Camera::default();
                cam.yaw = yaw;
                cam.pitch = pitch;
                let mesh = build_bear_gpu_mesh(&cam, center, scale);
                assert!(
                    mesh.indices.len() >= 3 * 20,
                    "yaw={yaw} pitch={pitch}: expected a substantial bear mesh"
                );
            }
        }
    }

    #[test]
    fn bear_back_faces_are_never_drawn() {
        for yaw in [0.0, 0.35, 0.8, 1.4, 2.2, 3.5] {
            for pitch in [-1.2, -0.5, 0.0, 0.35, 0.6, 1.1] {
                let mut cam = Camera::default();
                cam.yaw = yaw;
                cam.pitch = pitch;
                let (right, up, forward) = view_cube_basis(&cam);
                for tri in bear_mesh() {
                    let view_normal =
                        transform_vertex(tri.normal.normalize_or_zero(), right, up, forward);
                    if view_normal.z >= 0.0 {
                        assert!(
                            !bear_triangle_visible(tri, right, up, forward),
                            "back-facing facet should be culled at yaw={yaw} pitch={pitch}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn bear_raster_has_opaque_coverage() {
        let cam = Camera::default();
        let rect = Rect::from_center_size(Pos2::new(60.0, 60.0), Vec2::splat(CUBE_SIZE));
        let tris = project_bear(&cam, rect.center(), CUBE_SIZE * 0.42);
        let image = rasterize_bear(&tris, rect);
        let opaque = image.pixels.iter().filter(|p| p.a() == 255).count();
        assert!(
            opaque > 400,
            "bear raster should be mostly solid, got {opaque} opaque pixels"
        );
    }

    #[test]
    fn bear_projection_keeps_facets_from_isometric_view() {
        let cam = Camera::default();
        let count = project_bear(&cam, Pos2::new(60.0, 60.0), CUBE_SIZE * 0.42).len();
        assert!(
            count >= 55,
            "isometric view should show most of the bear, got {count}"
        );
    }

    #[test]
    fn bear_projection_bounds_stay_near_hud_center() {
        let cam = Camera::default();
        let center = Pos2::new(60.0, 60.0);
        let scale = CUBE_SIZE * 0.42;
        let tris = project_bear(&cam, center, scale);
        let mut min = Pos2::new(f32::INFINITY, f32::INFINITY);
        let mut max = Pos2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
        for t in &tris {
            for p in t.points {
                min.x = min.x.min(p.x);
                min.y = min.y.min(p.y);
                max.x = max.x.max(p.x);
                max.y = max.y.max(p.y);
            }
        }
        let pad = 8.0;
        let hud = Rect::from_center_size(center, Vec2::splat(CUBE_SIZE));
        assert!(min.x >= hud.min.x - pad, "min.x={}", min.x);
        assert!(min.y >= hud.min.y - pad, "min.y={}", min.y);
        assert!(max.x <= hud.max.x + pad, "max.x={}", max.x);
        assert!(max.y <= hud.max.y + pad, "max.y={}", max.y);
    }

    #[test]
    fn bear_projection_has_no_silhouette_spikes() {
        let center = Pos2::new(120.0, 120.0);
        let scale = CUBE_SIZE * 0.42;
        let max_edge = CUBE_SIZE * 0.75;
        for yaw in [0.0, 0.35, 0.8, 1.4, 2.2, 3.5] {
            for pitch in [-1.2, -0.5, 0.0, 0.35, 0.6, 1.1] {
                let mut cam = Camera::default();
                cam.yaw = yaw;
                cam.pitch = pitch;
                for tri in project_bear(&cam, center, scale) {
                    let edge = max_triangle_edge_length(tri.points);
                    assert!(
                        edge < max_edge,
                        "yaw={yaw} pitch={pitch}: triangle edge {edge} looks like a silhouette spike"
                    );
                }
            }
        }
    }

    #[test]
    fn projected_faces_stay_inside_hud_rect() {
        let vp = Rect::from_min_size(Pos2::new(0.0, 40.0), Vec2::new(800.0, 600.0));
        let screen_rect = cube_rect_in_viewport(vp);
        let center = screen_rect.center();
        let faces = project_faces(&Camera::default(), center, CUBE_SIZE * 0.42);
        let pad = screen_rect.expand(4.0);
        for face in &faces {
            for p in face.points {
                assert!(
                    pad.contains(p),
                    "face vertex {p:?} should lie inside HUD pad {pad:?}"
                );
            }
        }
    }

    #[test]
    fn projected_vertices_stay_within_silhouette_at_many_angles() {
        let center = Pos2::new(200.0, 120.0);
        let scale = CUBE_SIZE * 0.42;
        for yaw in [0.0, 0.35, 0.8, 1.4, 2.2, 3.5] {
            for pitch in [-1.2, -0.5, 0.0, 0.35, 0.6, 1.1] {
                let mut cam = Camera::default();
                cam.yaw = yaw;
                cam.pitch = pitch;
                let (right, up, forward) = view_cube_basis(&cam);
                let max_r = cube_silhouette_radius(right, up, forward, scale);
                let faces = project_faces(&cam, center, scale);
                for face in &faces {
                    for p in face.points {
                        let r = (p - center).length();
                        assert!(
                            r <= max_r + 0.01,
                            "yaw={yaw} pitch={pitch}: vertex {p:?} outside silhouette radius {max_r}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn projection_toggle_icons_fit_inside_button() {
        let button = Rect::from_min_size(Pos2::ZERO, Vec2::splat(PRESET_TOGGLE_SIZE));
        let icon = projection_toggle_icon_rect(button);
        let stroke_pad = PRESET_TOGGLE_ICON_STROKE * 0.5 + 0.5;
        let bounds = button.shrink(stroke_pad);
        let square = orthographic_icon_rect(icon);
        for corner in [square.left_top(), square.right_top(), square.right_bottom(), square.left_bottom()] {
            assert!(bounds.contains(corner), "point {corner:?} outside {bounds:?}");
        }
        for &(a, b) in natural_icon_segments(icon).iter() {
            assert!(bounds.contains(a), "point {a:?} outside {bounds:?}");
            assert!(bounds.contains(b), "point {b:?} outside {bounds:?}");
        }
    }

    #[test]
    fn projection_mode_toggle_sits_in_hud_bottom_left() {
        let vp = Rect::from_min_size(Pos2::new(0.0, 40.0), Vec2::new(800.0, 600.0));
        let screen_rect = cube_rect_in_viewport(vp);
        let pad_rect = screen_rect.expand(4.0);
        let toggle = view_preset_toggle_rect(pad_rect);
        assert!(pad_rect.contains(toggle.center()));
        assert!(toggle.max.x < screen_rect.center().x);
        assert!(toggle.max.y > screen_rect.center().y);
    }

    #[test]
    fn home_button_sits_in_hud_bottom_right() {
        let vp = Rect::from_min_size(Pos2::new(0.0, 40.0), Vec2::new(800.0, 600.0));
        let screen_rect = cube_rect_in_viewport(vp);
        let pad_rect = screen_rect.expand(4.0);
        let home = view_home_toggle_rect(pad_rect);
        assert!(pad_rect.contains(home.center()));
        assert!(home.min.x > screen_rect.center().x);
        assert!(home.max.y > screen_rect.center().y);
    }

    #[test]
    fn home_icon_fits_inside_button() {
        let button = Rect::from_min_size(Pos2::ZERO, Vec2::splat(PRESET_TOGGLE_SIZE));
        let icon = projection_toggle_icon_rect(button);
        let stroke_pad = PRESET_TOGGLE_ICON_STROKE * 0.5 + 0.5;
        let bounds = button.shrink(stroke_pad);
        let p = |u: f32, v: f32| icon_point(icon, u, v);
        for corner in [
            p(0.22, 0.18),
            p(0.78, 0.18),
            p(0.30, 0.82),
            p(0.70, 0.82),
            p(0.54, 0.58),
        ] {
            assert!(bounds.contains(corner), "point {corner:?} outside {bounds:?}");
        }
    }

    #[test]
    fn default_startup_culls_back_faces() {
        let cam = Camera::default();
        let center = Pos2::new(120.0, 120.0);
        let faces = project_faces(&cam, center, 40.0);
        let views: Vec<_> = faces.iter().map(|f| f.view).collect();
        assert_eq!(
            faces.len(),
            3,
            "default camera should show exactly three faces, got {views:?}"
        );
        assert!(!views.contains(&StandardView::Front));
        assert!(!views.contains(&StandardView::Left));
        assert!(!views.contains(&StandardView::Bottom));
        let opposing = [
            (StandardView::Top, StandardView::Bottom),
            (StandardView::Left, StandardView::Right),
            (StandardView::Front, StandardView::Back),
        ];
        for (a, b) in opposing {
            assert!(
                !(views.contains(&a) && views.contains(&b)),
                "opposing faces {a:?} and {b:?} should not both be visible: {views:?}"
            );
        }
    }

    #[test]
    fn edge_on_faces_are_culled() {
        let cam = cam_at_view(StandardView::Top);
        let center = Pos2::new(120.0, 120.0);
        let scale = 40.0;
        let faces = project_faces(&cam, center, scale);
        let views: Vec<_> = faces.iter().map(|f| f.view).collect();
        assert!(views.contains(&StandardView::Top));
        assert!(!views.contains(&StandardView::Right));
        assert!(!views.contains(&StandardView::Left));
    }

    #[test]
    fn axis_origin_is_front_left_bottom_corner() {
        let corner = AXIS_ORIGIN;
        assert_eq!(corner.x, -HALF);
        assert_eq!(corner.y, -HALF);
        assert_eq!(corner.z, -HALF);
        assert!(FACES.iter().any(|f| {
            f.view == StandardView::Front && f.corners.iter().any(|c| (*c - corner).length() < 1e-6)
        }));
        assert!(FACES.iter().any(|f| {
            f.view == StandardView::Left && f.corners.iter().any(|c| (*c - corner).length() < 1e-6)
        }));
        assert!(FACES.iter().any(|f| {
            f.view == StandardView::Bottom
                && f.corners.iter().any(|c| (*c - corner).length() < 1e-6)
        }));
    }

    #[test]
    fn projected_axes_share_origin_from_front_view() {
        let cam = cam_at_view(StandardView::Front);
        let center = Pos2::new(120.0, 120.0);
        let axes = project_axes(&cam, center, 40.0);
        assert_eq!(axes.len(), 3);
        let origin = axes[0].from;
        for axis in &axes {
            assert!(
                (axis.from - origin).length() < 0.01,
                "{label} axis should start at the shared origin",
                label = axis.label
            );
        }
    }

    #[test]
    fn projected_axes_from_front_view_point_right_and_up() {
        let cam = cam_at_view(StandardView::Front);
        let center = Pos2::new(120.0, 120.0);
        let axes = project_axes(&cam, center, 40.0);
        let x = axes.iter().find(|a| a.label == "X").expect("x axis");
        let y = axes.iter().find(|a| a.label == "Y").expect("y axis");
        let z = axes.iter().find(|a| a.label == "Z").expect("z axis");
        assert!(
            x.to.x > x.from.x + 4.0,
            "X should run to the right on screen from the front view"
        );
        assert!(
            z.to.y < z.from.y - 4.0,
            "Z should run upward on screen from the front view"
        );
        assert!(
            (y.to - y.from).length() < 6.0,
            "Y should point into the screen and barely move in front view"
        );
    }

}
