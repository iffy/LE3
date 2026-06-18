//! View-cube HUD (top-right): bear model inside an oriented bounding box,
//! drag-to-orbit, click faces/edges/corners to animate standard views.

use crate::camera::{Camera, ProjectionMode, StandardView, VIEW_TRANSITION_DURATION};
use crate::stl::{fit_mesh_to_unit_cube, parse_ascii_stl, scale_mesh, MeshTriangle};
use eframe::egui::epaint::TextShape;
use eframe::egui::{self, Color32, FontId, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
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
/// Hide bear facets that are too edge-on; path feathering on thin fills causes streaks.
const BEAR_TRIANGLE_CULL_DOT: f32 = 0.12;
const BEAR_MAX_SCREEN_EDGE: f32 = CUBE_SIZE * 0.55;
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
    center: Pos2,
    /// Average corner depth along the camera forward axis (for painter order).
    depth: f32,
}

struct ProjectedBearTriangle {
    points: [Pos2; 3],
    depth: f32,
    color: Color32,
}

fn bear_world_normal(tri: &MeshTriangle, right: Vec3, up: Vec3, forward: Vec3) -> Vec3 {
    let e0 = tri.vertices[1] - tri.vertices[0];
    let e1 = tri.vertices[2] - tri.vertices[0];
    let mut normal = e0.cross(e1);
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

fn max_triangle_edge_length(points: [Pos2; 3]) -> f32 {
    [
        (points[0] - points[1]).length(),
        (points[1] - points[2]).length(),
        (points[2] - points[0]).length(),
    ]
    .into_iter()
    .fold(0.0f32, f32::max)
}

fn wind_triangle_clockwise(points: [Pos2; 3]) -> [Pos2; 3] {
    let a = points[1] - points[0];
    let b = points[2] - points[0];
    if a.x * b.y - a.y * b.x < 0.0 {
        [points[0], points[2], points[1]]
    } else {
        points
    }
}

fn bear_triangle_drawable(view_pts: [Vec3; 3]) -> Option<f32> {
    let e0 = view_pts[1] - view_pts[0];
    let e1 = view_pts[2] - view_pts[0];
    let normal = e0.cross(e1);
    let normal_len_sq = normal.length_squared();
    if normal.z >= 0.0 || normal_len_sq < 1e-10 {
        return None;
    }

    let head_on = (-normal.z) / normal_len_sq.sqrt();
    if head_on < BEAR_TRIANGLE_CULL_DOT {
        return None;
    }

    let min_z = view_pts[0].z.min(view_pts[1].z).min(view_pts[2].z);
    let max_z = view_pts[0].z.max(view_pts[1].z).max(view_pts[2].z);
    if min_z < 0.0 && max_z > 0.0 && head_on < 0.45 {
        return None;
    }

    Some((view_pts[0].z + view_pts[1].z + view_pts[2].z) / 3.0)
}

fn project_bear(cam: &Camera, center: Pos2, scale: f32) -> Vec<ProjectedBearTriangle> {
    let (right, up, forward) = view_cube_basis(cam);

    let mut triangles = Vec::new();
    for tri in bear_mesh() {
        let view_pts = tri.vertices.map(|v| transform_vertex(v, right, up, forward));
        let Some(depth) = bear_triangle_drawable(view_pts) else {
            continue;
        };

        let points = wind_triangle_clockwise(view_pts.map(|v| project_to_hud(v, center, scale)));
        if tri_screen_area(points) < 0.5 {
            continue;
        }
        if max_triangle_edge_length(points) > BEAR_MAX_SCREEN_EDGE {
            continue;
        }

        let color = shade_bear_color(bear_world_normal(tri, right, up, forward));
        triangles.push(ProjectedBearTriangle { points, depth, color });
    }
    triangles.sort_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal));
    triangles
}

fn draw_bear(painter: &Painter, clip: Rect, triangles: &[ProjectedBearTriangle]) {
    if triangles.is_empty() {
        return;
    }
    let mut mesh = Mesh::default();
    mesh.reserve_triangles(triangles.len());
    for tri in triangles {
        let base = mesh.vertices.len() as u32;
        for p in tri.points {
            mesh.colored_vertex(p, tri.color);
        }
        mesh.add_triangle(base, base + 1, base + 2);
    }
    painter.with_clip_rect(clip).add(Shape::mesh(mesh));
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
        let center_pt = Pos2::new(
            (points[0].x + points[1].x + points[2].x + points[3].x) * 0.25,
            (points[0].y + points[1].y + points[2].y + points[3].y) * 0.25,
        );
        faces.push(ProjectedFace {
            view: face.view,
            points,
            center: center_pt,
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
    let response = ui.allocate_rect(rect, Sense::click());
    let hovered = response.hovered();
    let clicked = response.clicked();
    let pressed = response.is_pointer_button_down_on();

    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let hint = match target {
            ProjectionMode::Orthographic => "Orthographic projection",
            ProjectionMode::Natural => "Natural (perspective) projection",
        };
        response.on_hover_text(hint);
    }

    let fill = if pressed {
        Color32::from_gray(42)
    } else if hovered {
        Color32::from_gray(34)
    } else {
        Color32::from_rgba_unmultiplied(26, 28, 34, 220)
    };
    ui.painter()
        .rect_filled(rect, 4.0, fill);
    ui.painter().rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, Color32::from_gray(if hovered { 110 } else { 72 })),
    );
    paint_projection_mode_icon(ui.painter(), rect, target);

    if clicked {
        cam.set_projection_mode(target);
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
pub fn show_hud(ctx: &egui::Context, cam: &mut Camera, viewport: Rect) {
    let screen_rect = cube_rect_in_viewport(viewport);
    egui::Area::new(egui::Id::new("view_cube_hud"))
        .fixed_pos(screen_rect.min)
        .order(egui::Order::Foreground)
        .interactable(true)
        .constrain(false)
        .show(ctx, |ui| {
            show(ui, cam, screen_rect);
        });
}

/// Draw and handle input for the view-cube HUD. All geometry uses screen coordinates.
fn show(ui: &mut Ui, cam: &mut Camera, screen_rect: Rect) {
    let center = screen_rect.center();
    let scale = CUBE_SIZE * 0.42;

    let faces = project_faces(cam, center, scale);
    let edges = project_edges(cam, center, scale);
    let corners = project_corners(cam, center, scale);
    let bear_triangles = project_bear(cam, center, scale);

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
    draw_bear(painter, pad_rect, &bear_triangles);
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
        assert!(point_in_quad(front.center, front.points));
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
        assert_eq!(pick_face(&faces, front.center), Some(StandardView::Front));
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
        let max_edge = BEAR_MAX_SCREEN_EDGE;
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
