//! In-memory document model.
//!
//! This is the very first slice of BearCAD (see SPEC.md): a document is a flat list
//! of rectangles and lines on a single 2D sketch. As the action-DAG, components,
//! and the OCCT kernel come online this will grow, but the persistence boundary
//! (`storage.rs`) is kept narrow so the file format can evolve underneath it.

use crate::face::default_xy_plane;
use crate::value::{AngleUnit, LengthUnit};
use serde::{Deserialize, Serialize};

/// A sketchable face that lines and rectangles can be drawn on.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaceId {
    Rect(usize),
    Circle(usize),
    /// A closed loop of plain `Line`s, identified by its ordered line indices (#66).
    Polygon(Vec<usize>),
    ConstructionPlane(usize),
    /// A planar cap face of an extruded body: one profile face of an extrusion,
    /// at either the base (`top = false`) or offset (`top = true`) end.
    ExtrudeCap {
        extrusion: usize,
        profile: ExtrudeFace,
        top: bool,
    },
    /// A planar side wall of an extruded body: the quad swept by one `edge` of a
    /// polygonal profile (rectangles only; circular profiles have no flat sides).
    ExtrudeSide {
        extrusion: usize,
        profile: ExtrudeFace,
        edge: u8,
    },
}

impl Default for FaceId {
    fn default() -> Self {
        FaceId::ConstructionPlane(0)
    }
}

impl FaceId {
    pub fn from_script(kind: &str, index: usize) -> Option<Self> {
        match kind.to_ascii_lowercase().as_str() {
            "rect" | "rectangle" => Some(FaceId::Rect(index)),
            "circle" => Some(FaceId::Circle(index)),
            "plane" | "construction_plane" | "constructionplane" => {
                Some(FaceId::ConstructionPlane(index))
            }
            _ => None,
        }
    }

    /// The extrusion index that owns this face, for the two body-face variants (#26/#27's
    /// `FaceVertex`/`FaceEdge` dependency tracking piggybacks on this: a sketch on a body face,
    /// or a constraint referencing that face's own boundary, both depend on the extrusion that
    /// produced it — same relationship `hierarchy::face_element` already tracks for sketches).
    pub fn extrusion_index(&self) -> Option<usize> {
        match self {
            FaceId::ExtrudeCap { extrusion, .. } | FaceId::ExtrudeSide { extrusion, .. } => {
                Some(*extrusion)
            }
            FaceId::Rect(_) | FaceId::Circle(_) | FaceId::Polygon(_) | FaceId::ConstructionPlane(_) => {
                None
            }
        }
    }
}

/// Index into [`Document::sketches`].
pub type SketchId = usize;

/// Geometry that drives a read-only parameter value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterSource {
    LineLength(usize),
}

/// A named length or angle parameter (expression stored verbatim, evaluated on demand).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub expression: String,
    #[serde(default)]
    pub deleted: bool,
    /// When set, [`expression`] is synced from geometry and the value is read-only.
    #[serde(default)]
    pub source: Option<ParameterSource>,
}

/// A 2D sketch hosted on a face. A single face may host multiple independent sketches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sketch {
    pub face: FaceId,
    /// User-visible label in the Elements pane; empty uses the default.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub deleted: bool,
    /// Default length unit override for this sketch; `None` inherits [`Document::default_length_unit`] (#52).
    #[serde(default)]
    pub length_unit: Option<LengthUnit>,
    /// Default angle unit override for this sketch; `None` inherits [`Document::default_angle_unit`] (#52).
    #[serde(default)]
    pub angle_unit: Option<AngleUnit>,
}

/// One edge of a rectangle (bottom → right → top → left, matching [`rect_edge_segments`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RectEdge {
    Bottom,
    Right,
    Top,
    Left,
}

impl RectEdge {
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => RectEdge::Bottom,
            1 => RectEdge::Right,
            2 => RectEdge::Top,
            _ => RectEdge::Left,
        }
    }

    pub fn index(self) -> usize {
        match self {
            RectEdge::Bottom => 0,
            RectEdge::Right => 1,
            RectEdge::Top => 2,
            RectEdge::Left => 3,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "bottom" | "b" | "0" => Some(RectEdge::Bottom),
            "right" | "r" | "1" => Some(RectEdge::Right),
            "top" | "t" | "2" => Some(RectEdge::Top),
            "left" | "l" | "3" => Some(RectEdge::Left),
            _ => None,
        }
    }

    pub fn script_name(self) -> &'static str {
        match self {
            RectEdge::Bottom => "bottom",
            RectEdge::Right => "right",
            RectEdge::Top => "top",
            RectEdge::Left => "left",
        }
    }

    /// Corner indices (0–3) at the endpoints of this edge.
    pub fn corner_indices(self) -> (u8, u8) {
        match self {
            RectEdge::Bottom => (0, 1),
            RectEdge::Right => (1, 2),
            RectEdge::Top => (2, 3),
            RectEdge::Left => (3, 0),
        }
    }
}

/// An axis-aligned rectangle in face-local coordinates (millimetres, per SPEC §5.3).
///
/// Stored by its origin (`x`, `y`) and signed `w`/`h` extents in the local (u, v)
/// frame of the sketch's host face. We normalise on creation so width/height are
/// always positive, which keeps hit-testing simple.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Rect {
    pub sketch: SketchId,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Width was explicitly typed by the user (show dimension in sketch edit mode).
    #[serde(default)]
    pub width_locked: bool,
    /// Height was explicitly typed by the user (show dimension in sketch edit mode).
    #[serde(default)]
    pub height_locked: bool,
    /// User-placed offset from the measured edge to the width dimension line (px).
    #[serde(default)]
    pub width_dim_offset: Option<f32>,
    /// User-placed offset from the measured edge to the height dimension line (px).
    #[serde(default)]
    pub height_dim_offset: Option<f32>,
    /// Expression text when [`width_locked`] is set.
    #[serde(default)]
    pub width_expr: Option<String>,
    /// Expression text when [`height_locked`] is set.
    #[serde(default)]
    pub height_expr: Option<String>,
    /// Per-edge construction flags (bottom, right, top, left).
    #[serde(default)]
    pub construction_edges: [bool; 4],
    /// User-visible label in the Elements pane; empty uses the default.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub deleted: bool,
}

impl Rect {
    /// Build a normalised rectangle from two opposite corners in face-local coords.
    pub fn from_local_corners(sketch: SketchId, u0: f32, v0: f32, u1: f32, v1: f32) -> Self {
        Rect {
            sketch,
            x: u0.min(u1),
            y: v0.min(v1),
            w: (u1 - u0).abs(),
            h: (v1 - v0).abs(),
            width_locked: false,
            height_locked: false,
            width_dim_offset: None,
            height_dim_offset: None,
            width_expr: None,
            height_expr: None,
            construction_edges: [false; 4],
            name: None,
            deleted: false,
        }
    }

    pub fn edge_construction(&self, edge: RectEdge) -> bool {
        self.construction_edges[edge.index()]
    }

    pub fn set_edge_construction(&mut self, edge: RectEdge, construction: bool) {
        self.construction_edges[edge.index()] = construction;
    }

    pub fn all_edges_construction(&self) -> bool {
        self.construction_edges.iter().all(|&c| c)
    }

    /// True when some edges are construction and some are substantial.
    pub fn has_mixed_edge_construction(&self) -> bool {
        self.construction_edges.iter().any(|&c| c) && !self.all_edges_construction()
    }
}

impl<'de> Deserialize<'de> for Rect {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawRect {
            sketch: SketchId,
            x: f32,
            y: f32,
            w: f32,
            h: f32,
            #[serde(default)]
            width_locked: bool,
            #[serde(default)]
            height_locked: bool,
            #[serde(default)]
            width_dim_offset: Option<f32>,
            #[serde(default)]
            height_dim_offset: Option<f32>,
            #[serde(default)]
            width_expr: Option<String>,
            #[serde(default)]
            height_expr: Option<String>,
            /// Legacy whole-shape flag; migrated to all edges when edges are unset.
            #[serde(default)]
            construction: bool,
            #[serde(default)]
            construction_edges: [bool; 4],
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            deleted: bool,
        }

        let raw = RawRect::deserialize(deserializer)?;
        let mut construction_edges = raw.construction_edges;
        if raw.construction && !construction_edges.iter().any(|&e| e) {
            construction_edges = [true; 4];
        }
        Ok(Rect {
            sketch: raw.sketch,
            x: raw.x,
            y: raw.y,
            w: raw.w,
            h: raw.h,
            width_locked: raw.width_locked,
            height_locked: raw.height_locked,
            width_dim_offset: raw.width_dim_offset,
            height_dim_offset: raw.height_dim_offset,
            width_expr: raw.width_expr,
            height_expr: raw.height_expr,
            construction_edges,
            name: raw.name,
            deleted: raw.deleted,
        })
    }
}

/// A line segment in face-local coordinates (millimetres, per SPEC §5.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub sketch: SketchId,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    /// Length was explicitly typed by the user (show dimension in sketch edit mode).
    #[serde(default)]
    pub length_locked: bool,
    /// User-placed offset from the measured segment to the length dimension line (px).
    #[serde(default)]
    pub length_dim_offset: Option<f32>,
    /// Expression text when [`length_locked`] is set.
    #[serde(default)]
    pub length_expr: Option<String>,
    /// Reference geometry (dashed, construction color); not solid model geometry.
    #[serde(default)]
    pub construction: bool,
    /// User-visible label in the Elements pane; empty uses the default.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub deleted: bool,
    /// Cubic-bezier tangent handles in face-local coords: `[near (x0,y0), near (x1,y1)]`.
    /// `None` means a straight segment (the common case).
    #[serde(default)]
    pub bezier: Option<[(f32, f32); 2]>,
    /// Set when this line is the bridging line created by a chamfer/fillet vertex treatment
    /// (#37/#38): the index of the (lower-index) trimmed line it nests under in the Elements
    /// pane (see [`crate::hierarchy`], #76). `None` for an ordinary line.
    #[serde(default)]
    pub chamfer_fillet_parent: Option<usize>,
}

/// Number of straight sub-segments used to approximate a curved [`Line`] for rendering,
/// hit-testing, and extrusion tessellation (mirrors [`CIRCLE_SEGMENTS`]-style faceting).
pub const BEZIER_SEGMENTS: usize = 24;

impl Line {
    pub fn from_local_endpoints(
        sketch: SketchId,
        u0: f32,
        v0: f32,
        u1: f32,
        v1: f32,
    ) -> Self {
        Self {
            sketch,
            x0: u0,
            y0: v0,
            x1: u1,
            y1: v1,
            length_locked: false,
            length_dim_offset: None,
            length_expr: None,
            construction: false,
            name: None,
            deleted: false,
            bezier: None,
            chamfer_fillet_parent: None,
        }
    }

    pub fn length(&self) -> f32 {
        let du = self.x1 - self.x0;
        let dv = self.y1 - self.y0;
        (du * du + dv * dv).sqrt()
    }

    pub fn is_curved(&self) -> bool {
        self.bezier.is_some()
    }

    /// Sample this segment as a polyline in local coords (`segments + 1` points).
    /// Straight lines just return the two endpoints regardless of `segments`.
    pub fn sample_local(&self, segments: usize) -> Vec<(f32, f32)> {
        let p0 = (self.x0, self.y0);
        let p1 = (self.x1, self.y1);
        match self.bezier {
            None => vec![p0, p1],
            Some([c0, c1]) => (0..=segments)
                .map(|i| cubic_bezier_point(p0, c0, c1, p1, i as f32 / segments as f32))
                .collect(),
        }
    }
}

fn cubic_bezier_point(p0: (f32, f32), c0: (f32, f32), c1: (f32, f32), p1: (f32, f32), t: f32) -> (f32, f32) {
    let mt = 1.0 - t;
    let a = mt * mt * mt;
    let b = 3.0 * mt * mt * t;
    let c = 3.0 * mt * t * t;
    let d = t * t * t;
    (
        a * p0.0 + b * c0.0 + c * c1.0 + d * p1.0,
        a * p0.1 + b * c0.1 + c * c1.1 + d * p1.1,
    )
}

/// Smooths the joint at a shared vertex `v` between two lines (right-click "convert to bezier
/// curve"), given each line's other endpoint `a`/`b`. The tangent through `v` runs along the
/// `a`→`b` chord (Catmull-Rom style), so the curve stays visually smooth across the joint; each
/// line's far handle (away from `v`) sits a third of the way toward `v`, keeping that end
/// nearly straight since only the joint itself is being rounded.
///
/// Returns `([handle_near_a, handle_near_v], [handle_near_v, handle_near_b])` for the first and
/// second line respectively.
pub fn smooth_joint_bezier(
    a: (f32, f32),
    v: (f32, f32),
    b: (f32, f32),
) -> ([(f32, f32); 2], [(f32, f32); 2]) {
    let tx = b.0 - a.0;
    let ty = b.1 - a.1;
    let tlen = (tx * tx + ty * ty).sqrt();
    let unit = if tlen > 1e-6 { (tx / tlen, ty / tlen) } else { (0.0, 0.0) };

    let dist_av = ((v.0 - a.0).powi(2) + (v.1 - a.1).powi(2)).sqrt();
    let dist_vb = ((b.0 - v.0).powi(2) + (b.1 - v.1).powi(2)).sqrt();

    let h1_far = (a.0 + (v.0 - a.0) / 3.0, a.1 + (v.1 - a.1) / 3.0);
    let h1_near = (v.0 - unit.0 * dist_av / 3.0, v.1 - unit.1 * dist_av / 3.0);
    let h2_near = (v.0 + unit.0 * dist_vb / 3.0, v.1 + unit.1 * dist_vb / 3.0);
    let h2_far = (b.0 + (v.0 - b.0) / 3.0, b.1 + (v.1 - b.1) / 3.0);

    ([h1_far, h1_near], [h2_near, h2_far])
}

/// Default "corner point" tangent handle a third of the way from `from` toward `to`. Used
/// for a curve-mode segment's own handle when the tangent-constraint toggle is off: each
/// side of a vertex gets this independent, un-mirrored handle instead of one derived from
/// [`smooth_joint_bezier`] (#73).
pub fn independent_corner_handle(from: (f32, f32), to: (f32, f32)) -> (f32, f32) {
    (from.0 + (to.0 - from.0) / 3.0, from.1 + (to.1 - from.1) / 3.0)
}

/// Whether a sketch-vertex treatment truncates the two adjoining lines and bridges them with a
/// straight cut (chamfer) or a rounded single-cubic-bezier arc (fillet). See SPEC §3.1, #37/#38.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VertexTreatmentKind {
    Chamfer,
    Fillet,
}

/// Truncated endpoints (and, for a fillet, bridging-line tangent-handle bezier control points)
/// produced by [`vertex_treatment_geometry`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VertexTreatmentGeometry {
    /// New endpoint for the line whose far point was `a` (truncated back from the vertex).
    pub p1: (f32, f32),
    /// New endpoint for the line whose far point was `b` (truncated back from the vertex).
    pub p2: (f32, f32),
    /// `Some` for a fillet (bridging line curves); `None` for a chamfer (bridging line is
    /// straight).
    pub bezier: Option<[(f32, f32); 2]>,
}

/// Interior angle (radians, within ~1° of 0° or 180°) treated as a degenerate corner: the two
/// edges are (nearly) parallel or anti-parallel, so there's no real corner to chamfer/fillet.
const VERTEX_TREATMENT_DEGENERATE_EPS: f32 = 0.0175; // ~1 degree

/// Computes the truncated endpoints (and bridging-line geometry) for a chamfer or fillet applied
/// at a sketch vertex `v` shared by two lines whose other ("far") endpoints are `a` and `b`, in
/// face-local/sketch-local UV coordinates (same convention as [`smooth_joint_bezier`]).
///
/// `amount` is the chamfer distance (straight tangent length back from `v`) or the fillet radius,
/// depending on `kind`. Returns `None` when `amount` isn't positive, either adjacent edge is
/// degenerate (zero length), or the corner itself is degenerate (interior angle within ~1° of 0°
/// or 180° — the edges are parallel/anti-parallel, so there's no real corner to round or cut).
///
/// The tangent length back from `v` is clamped so it never cuts back past either adjacent edge's
/// own far endpoint; for a fillet, the effective radius (and its arc) are recomputed from the
/// clamped tangent length so the arc stays geometrically consistent with where the truncated
/// endpoints actually land, rather than the originally requested radius.
pub fn vertex_treatment_geometry(
    v: (f32, f32),
    a: (f32, f32),
    b: (f32, f32),
    kind: VertexTreatmentKind,
    amount: f32,
) -> Option<VertexTreatmentGeometry> {
    if !(amount > 0.0) {
        return None;
    }
    let dist_va = ((a.0 - v.0).powi(2) + (a.1 - v.1).powi(2)).sqrt();
    let dist_vb = ((b.0 - v.0).powi(2) + (b.1 - v.1).powi(2)).sqrt();
    if dist_va < 1e-6 || dist_vb < 1e-6 {
        return None;
    }
    let dir_a = ((a.0 - v.0) / dist_va, (a.1 - v.1) / dist_va);
    let dir_b = ((b.0 - v.0) / dist_vb, (b.1 - v.1) / dist_vb);
    let cos_alpha = (dir_a.0 * dir_b.0 + dir_a.1 * dir_b.1).clamp(-1.0, 1.0);
    let alpha = cos_alpha.acos();
    if alpha < VERTEX_TREATMENT_DEGENERATE_EPS
        || alpha > std::f32::consts::PI - VERTEX_TREATMENT_DEGENERATE_EPS
    {
        return None;
    }

    let raw_t = match kind {
        VertexTreatmentKind::Chamfer => amount,
        VertexTreatmentKind::Fillet => amount / (alpha / 2.0).tan(),
    };
    let max_t = (dist_va * 0.95).min(dist_vb * 0.95);
    let t = raw_t.min(max_t);

    let p1 = (v.0 + dir_a.0 * t, v.1 + dir_a.1 * t);
    let p2 = (v.0 + dir_b.0 * t, v.1 + dir_b.1 * t);

    let bezier = match kind {
        VertexTreatmentKind::Chamfer => None,
        VertexTreatmentKind::Fillet => {
            // Recompute the effective radius from the (possibly clamped) tangent length so the
            // arc stays consistent with where p1/p2 actually landed.
            let radius = t * (alpha / 2.0).tan();
            let theta = std::f32::consts::PI - alpha;
            let k = radius * (4.0 / 3.0) * (theta / 4.0).tan();
            let h0 = (p1.0 - dir_a.0 * k, p1.1 - dir_a.1 * k);
            let h1 = (p2.0 - dir_b.0 * k, p2.1 - dir_b.1 * k);
            Some([h0, h1])
        }
    };

    Some(VertexTreatmentGeometry { p1, p2, bezier })
}

/// Which analytic edge family of an extrusion-sourced solid an [`EdgeTreatment`] targets
/// (#77): a 3D edge chamfer/fillet is a mesh-bevel approximation limited to the two edge
/// kinds that have a clean analytic definition for a `Rect`/`Polygon` profile — see
/// `crate::extrude::side_quad_world`/`cap_polygon_world`. A `Circle` profile has neither (its
/// side is curved, with no discrete side walls — `side_face_count` is 0), so it's out of
/// scope; so are STL/STEP-imported bodies (no analytic profile at all). See SPEC §3.4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtrusionEdgeRef {
    /// The vertical edge shared by side walls `edge` and `edge + 1` (mod the profile's vertex
    /// count) of `face` (an index into [`Extrusion::faces`]) — i.e. the edge at profile vertex
    /// `(edge + 1) % n`, running the full height from base to top cap.
    Vertical { face: usize, edge: usize },
    /// The edge where side wall `edge` of `face` meets a cap: the base cap when `top` is
    /// `false`, the top cap when `true` (also a `cap_polygon_world` boundary edge).
    Cap { face: usize, edge: usize, top: bool },
}

impl ExtrusionEdgeRef {
    /// The face index this edge belongs to (an index into [`Extrusion::faces`]).
    pub fn face(self) -> usize {
        match self {
            ExtrusionEdgeRef::Vertical { face, .. } => face,
            ExtrusionEdgeRef::Cap { face, .. } => face,
        }
    }
}

/// A parametric chamfer/fillet bevel applied to one analytic edge of an [`Extrusion`]'s solid
/// (#77): a mesh-bevel approximation, not a true BREP fillet (no tangent-continuous curved
/// surface, no vertex-miter blending) — see SPEC §3.4. Re-evaluated from the document every
/// frame by `crate::extrude::extrusion_mesh`, like everything else in this app; nothing here
/// is a baked/one-time mesh edit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeTreatment {
    pub edge: ExtrusionEdgeRef,
    pub kind: VertexTreatmentKind,
    /// Chamfer distance or fillet radius (mm); must be positive to have any effect.
    pub amount: f32,
}

/// A circle in face-local coordinates (millimetres, per SPEC §5.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Circle {
    pub sketch: SketchId,
    pub cx: f32,
    pub cy: f32,
    pub r: f32,
    /// Diameter was explicitly typed by the user (show dimension in sketch edit mode).
    #[serde(default)]
    pub diameter_locked: bool,
    /// User-placed outward offset of the diameter label from the dimension line (px).
    #[serde(default)]
    pub diameter_dim_offset: Option<f32>,
    /// Expression text when [`diameter_locked`] is set.
    #[serde(default)]
    pub diameter_expr: Option<String>,
    /// Angle (radians) of the diameter dimension line in local (u, v) coords.
    #[serde(default)]
    pub diameter_dim_angle: f32,
    /// Reference geometry (dashed, construction color); not solid model geometry.
    #[serde(default)]
    pub construction: bool,
    /// User-visible label in the Elements pane; empty uses the default.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub deleted: bool,
}

impl Circle {
    pub fn from_local_center_radius(
        sketch: SketchId,
        cx: f32,
        cy: f32,
        r: f32,
        diameter_dim_angle: f32,
    ) -> Self {
        Self {
            sketch,
            cx,
            cy,
            r,
            diameter_locked: false,
            diameter_dim_offset: None,
            diameter_expr: None,
            diameter_dim_angle,
            construction: false,
            name: None,
            deleted: false,
        }
    }

    pub fn diameter(&self) -> f32 {
        self.r * 2.0
    }
}

/// Reference geometry a construction plane was built from (for later editing).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlaneAnchor {
    Face {
        origin: glam::Vec3,
        normal: glam::Vec3,
        label: String,
    },
    Axis {
        origin: glam::Vec3,
        direction: glam::Vec3,
        label: String,
    },
}

/// Editable offset/angle parameters that define a construction plane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaneDefinition {
    pub anchor: PlaneAnchor,
    pub offset_mm: f32,
    pub angle_deg: f32,
}

impl PlaneDefinition {
    pub fn is_axis(&self) -> bool {
        matches!(self.anchor, PlaneAnchor::Axis { .. })
    }
}

/// Where a construction plane sits in the scene hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ConstructionPlaneParent {
    /// Datum plane (default XY, ground, global axes, etc.).
    #[default]
    Root,
    /// Derived from geometry in a sketch.
    Sketch(SketchId),
}

/// A construction plane in world space (millimetres).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConstructionPlane {
    pub origin: glam::Vec3,
    pub normal: glam::Vec3,
    pub u_axis: glam::Vec3,
    pub v_axis: glam::Vec3,
    pub parent: ConstructionPlaneParent,
    pub definition: PlaneDefinition,
    /// User-visible label in the Elements pane; empty uses the default.
    pub name: Option<String>,
    #[serde(default)]
    pub deleted: bool,
}

/// Which end of a line segment a constraint point refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEnd {
    Start,
    End,
}

/// A point-like sketch entity for coincident and other constraints.
///
/// Not `Copy`: [`FaceVertex`](Self::FaceVertex) embeds a [`FaceId`], which is not `Copy`
/// (its `Polygon`/extrusion-profile variants own a `Vec<usize>`). Callers that used to rely on
/// implicit copies now need an explicit `.clone()`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintPoint {
    LineEndpoint { line: usize, end: LineEnd },
    /// Corner index 0–3 matches [`crate::face::rect_world_corners_in_frame`] order.
    RectCorner { rect: usize, corner: u8 },
    CircleCenter(usize),
    /// A corner of an extrusion-backed face's own boundary loop (#26/#27): index into
    /// [`crate::extrude::face_boundary_loop_world`]'s ordered vertex list. Scoped to
    /// `FaceId::ExtrudeCap`/`FaceId::ExtrudeSide`; other face kinds never resolve. Fixed by
    /// the body's geometry, not draggable — mirrors [`ConstraintEntity::Origin`].
    FaceVertex { face: FaceId, index: usize },
}

/// A line-like sketch entity for parallel, perpendicular, and orientation constraints.
///
/// Not `Copy` — see [`ConstraintPoint`]'s doc comment.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintLine {
    Line(usize),
    RectEdge { rect: usize, edge: RectEdge },
    /// An edge of an extrusion-backed face's own boundary loop (#26/#27): runs from
    /// `boundary_loop[index]` to `boundary_loop[(index + 1) % boundary_loop.len()]`. Same
    /// scope and fixed-geometry treatment as [`ConstraintPoint::FaceVertex`].
    FaceEdge { face: FaceId, index: usize },
}

/// +1 or -1 disambiguation for constraints with two valid solutions.
pub type ConstraintSign = i8;

pub fn default_constraint_sign() -> ConstraintSign {
    1
}

/// Geometry a distance constraint applies to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistanceTarget {
    LineLength(usize),
    RectWidth(usize),
    RectHeight(usize),
    CircleDiameter(usize),
    /// Spacing between parallel lines. `side` is the sign of the movable line's
    /// perpendicular offset from the reference line (+1 = positive perpendicular side).
    LineLineDistance {
        line_a: ConstraintLine,
        line_b: ConstraintLine,
        #[serde(default = "default_constraint_sign")]
        side: ConstraintSign,
    },
    /// Distance between two points. `anchor` stays fixed; `mover` is placed
    /// `dir_u`/`dir_v` away from the anchor.
    PointPointDistance {
        anchor: ConstraintPoint,
        mover: ConstraintPoint,
        dir_u: f32,
        dir_v: f32,
    },
    /// Perpendicular distance from a point to a line. `side` is the sign of the
    /// point's offset from the line (+1 = positive perpendicular side).
    PointLineDistance {
        point: ConstraintPoint,
        line: ConstraintLine,
        #[serde(default = "default_constraint_sign")]
        side: ConstraintSign,
    },
}

/// Target for the dimension tool (distance or angle).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimensionTarget {
    Distance(DistanceTarget),
    Angle {
        line_a: ConstraintLine,
        line_b: ConstraintLine,
        #[serde(default = "default_constraint_sign")]
        rotation_sign: ConstraintSign,
    },
}

/// Kind of sketch constraint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    Distance { target: DistanceTarget },
    Parallel {
        line_a: ConstraintLine,
        line_b: ConstraintLine,
    },
    Perpendicular {
        line_a: ConstraintLine,
        line_b: ConstraintLine,
    },
    /// Two edges constrained to have equal length. See #47.
    Equal {
        line_a: ConstraintLine,
        line_b: ConstraintLine,
    },
    Coincident {
        a: ConstraintEntity,
        b: ConstraintEntity,
    },
    Midpoint {
        point: ConstraintPoint,
        line: ConstraintLine,
    },
    Horizontal { line: ConstraintLine },
    Vertical { line: ConstraintLine },
    Angle {
        line_a: ConstraintLine,
        line_b: ConstraintLine,
        /// +1: movable line rotates counterclockwise from reference; -1: clockwise.
        #[serde(default = "default_constraint_sign")]
        rotation_sign: ConstraintSign,
    },
}

/// Point or line reference for coincident constraints.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintEntity {
    Point(ConstraintPoint),
    Line(ConstraintLine),
    /// A circle's perimeter (point-on-circle when paired with a point).
    Circle(usize),
    /// The sketch origin (local UV `(0, 0)`); a fixed point for snapping.
    Origin,
}

/// A sketch constraint (distance is the first supported kind).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    pub sketch: SketchId,
    pub kind: ConstraintKind,
    pub expression: String,
    /// User-placed offset from the measured segment to the dimension line (px).
    #[serde(default)]
    pub dim_offset: Option<f32>,
    /// User-visible label in the Elements pane; empty uses the default.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub deleted: bool,
}

/// A boolean combination of two coplanar sketch faces (#16/#62): the atomic regions a user
/// can toggle when two shapes overlap (their shared intersection, or one minus the other).
/// No `Union` variant is needed — unioning two shapes is already achievable by toggling both
/// of their whole-shape `ExtrudeFace`s into the same extrusion (pre-existing multi-face
/// selection), see SPEC.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOp {
    Intersection,
    /// `a` minus `b`.
    Difference,
}

/// A closed sketch profile (face) included in an extrusion.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtrudeFace {
    Rect(usize),
    Circle(usize),
    /// A closed loop of plain `Line`s, identified by its ordered line indices (#66).
    Polygon(Vec<usize>),
    /// A boolean-combined region of two other faces (#16/#62), computed on demand via
    /// [`crate::polygon_boolean::polygon_boolean`] rather than stored as its own geometry.
    /// Recursive (`a`/`b` can themselves be `Boolean`) so the data model stays general, even
    /// though the interactive picker (see `src/face.rs`/`src/main.rs`) only ever constructs
    /// depth-1 combinations of two raw (`Rect`/`Circle`/`Polygon`) shapes.
    Boolean {
        op: BooleanOp,
        a: Box<ExtrudeFace>,
        b: Box<ExtrudeFace>,
    },
}

impl ExtrudeFace {
    /// The sketchable face this profile corresponds to. For `Boolean`, there's no `FaceId` of
    /// its own (it's not a stored shape) — this recurses into `a` since `a` and `b` always
    /// share the same underlying sketch plane, so `a`'s frame (axes/normal) is equally valid;
    /// only its in-plane origin differs, which callers of `face_id()` don't rely on.
    pub fn face_id(&self) -> FaceId {
        match self {
            ExtrudeFace::Rect(i) => FaceId::Rect(*i),
            ExtrudeFace::Circle(i) => FaceId::Circle(*i),
            ExtrudeFace::Polygon(lines) => FaceId::Polygon(lines.clone()),
            ExtrudeFace::Boolean { a, .. } => a.face_id(),
        }
    }
}

/// An object an extrusion is constrained to reach (its extended plane), instead of a fixed
/// distance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtrudeTarget {
    /// Up to the plane through a vertex (perpendicular to the extrusion normal).
    Vertex(ConstraintPoint),
    /// Up to the extended plane of a face.
    Face(ExtrudeFace),
    /// Up to a construction plane.
    Plane(usize),
}

/// An extrusion of one or more coplanar sketch faces into a 3D solid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Extrusion {
    /// The sketch whose plane the faces lie on (gives the extrusion normal).
    pub sketch: SketchId,
    /// Faces included in this extrusion (toggled on/off while editing).
    pub faces: Vec<ExtrudeFace>,
    /// Signed extrusion distance along the plane normal (mm); negative goes the other way.
    /// When `target` is set this is the cached/last value; the effective distance is derived.
    pub distance: f32,
    /// When set, the depth is constrained to reach this object's extended plane.
    #[serde(default)]
    pub target: Option<ExtrudeTarget>,
    /// Optional expression driving `distance` (empty = free/gizmo-driven, no constraint).
    #[serde(default)]
    pub expression: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub deleted: bool,
    /// Parametric 3D edge chamfer/fillet bevels applied to this extrusion's own analytic
    /// side/cap edges (#77) — see [`EdgeTreatment`].
    #[serde(default)]
    pub edge_treatments: Vec<EdgeTreatment>,
}

/// The feature(s) that produced a solid body.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodySource {
    Extrusion(usize),
    Extrusions(Vec<usize>),
    /// A mesh body brought in via STL import (#70); indexes `Document::imported_meshes`
    /// rather than depending on a sketch-based feature.
    Imported(usize),
}

impl BodySource {
    pub fn single(extrusion: usize) -> Self {
        Self::Extrusion(extrusion)
    }

    pub fn extrusion_indices(&self) -> &[usize] {
        match self {
            Self::Extrusion(index) => std::slice::from_ref(index),
            Self::Extrusions(indices) => indices.as_slice(),
            Self::Imported(_) => &[],
        }
    }

    pub fn imported_mesh_index(&self) -> Option<usize> {
        match self {
            Self::Imported(index) => Some(*index),
            Self::Extrusion(_) | Self::Extrusions(_) => None,
        }
    }

    pub fn owns_extrusion(&self, extrusion: usize) -> bool {
        self.extrusion_indices().contains(&extrusion)
    }

    pub fn append_extrusion(&mut self, extrusion: usize) {
        match self {
            Self::Extrusion(existing) => {
                *self = Self::Extrusions(vec![*existing, extrusion]);
            }
            Self::Extrusions(indices) => indices.push(extrusion),
            // An imported mesh body has no extrusion to merge into; unreachable in practice
            // since merge candidates only ever come from extrusion-backed bodies.
            Self::Imported(_) => {}
        }
    }

    /// Remove `extrusion` from this source (e.g. undoing a merge). Collapses back to the
    /// single-extrusion form when only one index remains. No-op if `extrusion` isn't owned
    /// or this is already a single-extrusion source (undo never removes a body's last/only
    /// extrusion this way — that path tombstones the whole body instead).
    pub fn remove_extrusion(&mut self, extrusion: usize) {
        if let Self::Extrusions(indices) = self {
            indices.retain(|&ei| ei != extrusion);
            if let [only] = indices.as_slice() {
                *self = Self::Extrusion(*only);
            }
        }
    }
}

/// Body index whose source includes `extrusion`, if any.
pub fn body_index_for_extrusion(doc: &Document, extrusion: usize) -> Option<usize> {
    doc.bodies.iter().position(|body| {
        !body.deleted && body.source.owns_extrusion(extrusion)
    })
}

/// A solid body produced by a feature; it depends on its source feature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub source: BodySource,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub deleted: bool,
}

/// A solid mesh brought in via file import (STL, #70), stored as-is (no scaling/centering)
/// in the document's coordinate space. Backs a `Body` via `BodySource::Imported`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportedMesh {
    pub triangles: Vec<[glam::Vec3; 3]>,
    /// Source file name (without extension), used as the default body name.
    pub source_name: String,
}

/// Which sketch primitive was created, in chronological order (for undo).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Sketch,
    Rect,
    Line,
    Circle,
    Parameter,
    Constraint,
    ConstructionPlane,
    Extrusion,
    Body,
    /// An in-place edit of an existing construction plane (undo restores the prior planes).
    /// Transient: never persisted (storage rebuilds `shape_order` from created shapes only).
    ConstructionPlaneEdit,
}

/// The whole document: sketches, sketch primitives, constraints, and construction planes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub parameters: Vec<Parameter>,
    pub sketches: Vec<Sketch>,
    pub rects: Vec<Rect>,
    pub lines: Vec<Line>,
    pub circles: Vec<Circle>,
    pub constraints: Vec<Constraint>,
    pub construction_planes: Vec<ConstructionPlane>,
    #[serde(default)]
    pub extrusions: Vec<Extrusion>,
    #[serde(default)]
    pub bodies: Vec<Body>,
    #[serde(default)]
    pub imported_meshes: Vec<ImportedMesh>,
    pub shape_order: Vec<ShapeKind>,
    /// Document-wide default length unit (context pane, nothing selected; #52).
    ///
    /// Drives dimension-label and Elements-pane display formatting via
    /// [`effective_length_unit`] (#85); bare-number expression parsing is unaffected and
    /// still defaults to mm.
    #[serde(default)]
    pub default_length_unit: LengthUnit,
    /// Document-wide default angle unit (context pane, nothing selected; #52). Same scope
    /// caveat as [`default_length_unit`](Document::default_length_unit).
    #[serde(default)]
    pub default_angle_unit: AngleUnit,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            parameters: Vec::new(),
            sketches: Vec::new(),
            rects: Vec::new(),
            lines: Vec::new(),
            circles: Vec::new(),
            constraints: Vec::new(),
            construction_planes: vec![default_xy_plane()],
            extrusions: Vec::new(),
            bodies: Vec::new(),
            imported_meshes: Vec::new(),
            shape_order: Vec::new(),
            default_length_unit: LengthUnit::default(),
            default_angle_unit: AngleUnit::default(),
        }
    }
}

impl Document {
    pub fn sketch_face(&self, sketch: SketchId) -> Option<FaceId> {
        self.sketches.get(sketch).map(|s| s.face.clone())
    }

    pub fn sketches_on_face(&self, face: FaceId) -> impl Iterator<Item = SketchId> + '_ {
        self.sketches
            .iter()
            .enumerate()
            .filter_map(move |(i, s)| (s.face == face).then_some(i))
    }

    pub fn sketch_has_geometry(&self, sketch: SketchId) -> bool {
        self.rects.iter().any(|r| r.sketch == sketch)
            || self.lines.iter().any(|l| l.sketch == sketch)
            || self.circles.iter().any(|c| c.sketch == sketch)
    }

    pub fn has_children(&self, face: &FaceId) -> bool {
        self.sketches.iter().any(|s| &s.face == face)
    }

    pub fn add_sketch(&mut self, face: FaceId) -> SketchId {
        let id = self.sketches.len();
        self.sketches.push(Sketch {
            face,
            name: None,
            deleted: false,
            length_unit: None,
            angle_unit: None,
        });
        self.shape_order.push(ShapeKind::Sketch);
        id
    }
}

/// Effective default length unit for `sketch`: its own override, or the document default if
/// unset or the sketch doesn't exist (#52).
pub fn effective_length_unit(doc: &Document, sketch: SketchId) -> LengthUnit {
    doc.sketches
        .get(sketch)
        .and_then(|s| s.length_unit)
        .unwrap_or(doc.default_length_unit)
}

/// Effective default angle unit for `sketch`: its own override, or the document default if
/// unset or the sketch doesn't exist (#52).
pub fn effective_angle_unit(doc: &Document, sketch: SketchId) -> AngleUnit {
    doc.sketches
        .get(sketch)
        .and_then(|s| s.angle_unit)
        .unwrap_or(doc.default_angle_unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_length_from_endpoints() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        let line = Line::from_local_endpoints(sketch, 0.0, 0.0, 3.0, 4.0);
        assert!((line.length() - 5.0).abs() < 1e-4);
    }

    #[test]
    fn straight_line_samples_to_just_its_two_endpoints() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        let line = Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0);
        assert_eq!(line.sample_local(BEZIER_SEGMENTS), vec![(0.0, 0.0), (10.0, 0.0)]);
        assert!(!line.is_curved());
    }

    #[test]
    fn curved_line_samples_pass_through_both_endpoints() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        let mut line = Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0);
        line.bezier = Some([(3.0, 4.0), (7.0, 4.0)]);
        let pts = line.sample_local(BEZIER_SEGMENTS);
        assert_eq!(pts.len(), BEZIER_SEGMENTS + 1);
        assert_eq!(pts[0], (0.0, 0.0));
        assert_eq!(*pts.last().unwrap(), (10.0, 0.0));
        // Bulges away from the straight chord partway through.
        assert!(pts[BEZIER_SEGMENTS / 2].1 > 1.0);
        assert!(line.is_curved());
    }

    #[test]
    fn smooth_joint_bezier_keeps_both_handles_on_the_a_to_b_tangent() {
        let a = (0.0, 0.0);
        let v = (10.0, 0.0);
        let b = (20.0, 0.0);
        let ([h1_far, h1_near], [h2_near, h2_far]) = smooth_joint_bezier(a, v, b);
        // Collinear a-v-b: every handle should stay on the same horizontal line.
        for (_, y) in [h1_far, h1_near, h2_near, h2_far] {
            assert!(y.abs() < 1e-4);
        }
        // Handles near the joint sit strictly between the far endpoints and v.
        assert!(h1_near.0 > a.0 && h1_near.0 < v.0);
        assert!(h2_near.0 > v.0 && h2_near.0 < b.0);
    }

    #[test]
    fn independent_corner_handle_sits_a_third_of_the_way_toward_the_target() {
        let h = independent_corner_handle((0.0, 0.0), (9.0, 6.0));
        assert!((h.0 - 3.0).abs() < 1e-4);
        assert!((h.1 - 2.0).abs() < 1e-4);
    }

    #[test]
    fn vertex_treatment_chamfer_on_a_right_angle_corner_is_symmetric() {
        let v = (0.0, 0.0);
        let a = (10.0, 0.0);
        let b = (0.0, 10.0);
        let geom =
            vertex_treatment_geometry(v, a, b, VertexTreatmentKind::Chamfer, 3.0).unwrap();
        assert!((geom.p1.0 - 3.0).abs() < 1e-4 && geom.p1.1.abs() < 1e-4);
        assert!((geom.p2.1 - 3.0).abs() < 1e-4 && geom.p2.0.abs() < 1e-4);
        assert_eq!(geom.bezier, None);
    }

    #[test]
    fn vertex_treatment_fillet_on_a_right_angle_corner_stays_radius_from_center() {
        let v = (0.0, 0.0);
        let a = (10.0, 0.0);
        let b = (0.0, 10.0);
        let radius = 3.0;
        let geom =
            vertex_treatment_geometry(v, a, b, VertexTreatmentKind::Fillet, radius).unwrap();
        // Tangent length for a 90 degree corner equals the radius (tan(45deg) == 1).
        assert!((geom.p1.0 - radius).abs() < 1e-4 && geom.p1.1.abs() < 1e-4);
        assert!((geom.p2.1 - radius).abs() < 1e-4 && geom.p2.0.abs() < 1e-4);
        let bezier = geom.bezier.expect("fillet should curve the bridging line");

        // The arc center sits on the inward bisector, equidistant (by `radius`) from both p1/p2.
        let center = (3.0, 3.0);
        let mut line =
            Line::from_local_endpoints(0, geom.p1.0, geom.p1.1, geom.p2.0, geom.p2.1);
        line.bezier = Some(bezier);
        for (x, y) in line.sample_local(BEZIER_SEGMENTS) {
            let dist = ((x - center.0).powi(2) + (y - center.1).powi(2)).sqrt();
            assert!(
                (dist - radius).abs() < radius * 0.02,
                "sampled point ({x}, {y}) at distance {dist} from center, expected ~{radius}"
            );
        }
    }

    #[test]
    fn vertex_treatment_fillet_on_a_45_degree_corner_stays_radius_from_center() {
        // A shallower corner: far points at 90 degrees apart around a 45 degree wedge.
        let v = (0.0, 0.0);
        let a = (10.0, 0.0);
        let b = (10.0 * (std::f32::consts::FRAC_PI_4).cos(), 10.0 * (std::f32::consts::FRAC_PI_4).sin());
        let radius = 2.0;
        let geom =
            vertex_treatment_geometry(v, a, b, VertexTreatmentKind::Fillet, radius).unwrap();
        let bezier = geom.bezier.unwrap();
        let alpha = std::f32::consts::FRAC_PI_4;
        let bisector_len = radius / (alpha / 2.0).sin();
        let bisector_angle = alpha / 2.0;
        let center = (
            bisector_len * bisector_angle.cos(),
            bisector_len * bisector_angle.sin(),
        );
        let mut line =
            Line::from_local_endpoints(0, geom.p1.0, geom.p1.1, geom.p2.0, geom.p2.1);
        line.bezier = Some(bezier);
        for (x, y) in line.sample_local(BEZIER_SEGMENTS) {
            let dist = ((x - center.0).powi(2) + (y - center.1).powi(2)).sqrt();
            assert!(
                (dist - radius).abs() < radius * 0.05,
                "sampled point ({x}, {y}) at distance {dist} from center, expected ~{radius}"
            );
        }
    }

    #[test]
    fn vertex_treatment_clamps_tangent_length_to_the_shorter_edge() {
        // Both edges only 2mm long; a 10mm chamfer distance must clamp back to ~1.9mm (0.95x).
        let v = (0.0, 0.0);
        let a = (2.0, 0.0);
        let b = (0.0, 2.0);
        let geom =
            vertex_treatment_geometry(v, a, b, VertexTreatmentKind::Chamfer, 10.0).unwrap();
        assert!((geom.p1.0 - 1.9).abs() < 1e-4);
        assert!((geom.p2.1 - 1.9).abs() < 1e-4);
    }

    #[test]
    fn vertex_treatment_rejects_a_degenerate_straight_corner() {
        let v = (0.0, 0.0);
        // a and b both lie along +X from v: the "corner" is actually a straight continuation.
        let a = (10.0, 0.0);
        let b = (20.0, 0.0);
        assert_eq!(
            vertex_treatment_geometry(v, a, b, VertexTreatmentKind::Chamfer, 3.0),
            None
        );
        assert_eq!(
            vertex_treatment_geometry(v, a, b, VertexTreatmentKind::Fillet, 3.0),
            None
        );
    }

    #[test]
    fn vertex_treatment_rejects_a_degenerate_folded_back_corner() {
        let v = (0.0, 0.0);
        // a and b point in opposite directions from v: a 180 degree fold, not a real corner.
        let a = (10.0, 0.0);
        let b = (-10.0, 0.0);
        assert_eq!(
            vertex_treatment_geometry(v, a, b, VertexTreatmentKind::Chamfer, 3.0),
            None
        );
    }

    #[test]
    fn vertex_treatment_rejects_non_positive_amount() {
        let v = (0.0, 0.0);
        let a = (10.0, 0.0);
        let b = (0.0, 10.0);
        assert_eq!(
            vertex_treatment_geometry(v, a, b, VertexTreatmentKind::Chamfer, 0.0),
            None
        );
        assert_eq!(
            vertex_treatment_geometry(v, a, b, VertexTreatmentKind::Fillet, -1.0),
            None
        );
    }

    #[test]
    fn face_id_from_script_parses_circle() {
        assert_eq!(FaceId::from_script("circle", 2), Some(FaceId::Circle(2)));
    }

    #[test]
    fn multiple_sketches_on_one_face() {
        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        let s1 = doc.add_sketch(FaceId::ConstructionPlane(0));
        assert_ne!(s0, s1);
        let on_plane: Vec<_> = doc.sketches_on_face(FaceId::ConstructionPlane(0)).collect();
        assert_eq!(on_plane, vec![0, 1]);
    }

    #[test]
    fn rect_deserializes_legacy_whole_shape_construction_flag() {
        let json = r#"{
            "sketch": 0,
            "x": 0.0,
            "y": 0.0,
            "w": 10.0,
            "h": 5.0,
            "construction": true
        }"#;
        let rect: Rect = serde_json::from_str(json).unwrap();
        assert!(rect.all_edges_construction());
    }

    #[test]
    fn rect_edge_construction_is_independent_per_edge() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        let mut rect = Rect::from_local_corners(sketch, 0.0, 0.0, 1.0, 1.0);
        rect.set_edge_construction(RectEdge::Left, true);
        assert!(rect.edge_construction(RectEdge::Left));
        assert!(!rect.edge_construction(RectEdge::Right));
    }

    #[test]
    fn rect_mixed_edge_construction_detected() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        let mut rect = Rect::from_local_corners(sketch, 0.0, 0.0, 1.0, 1.0);
        assert!(!rect.has_mixed_edge_construction());
        rect.set_edge_construction(RectEdge::Bottom, true);
        assert!(rect.has_mixed_edge_construction());
        for edge_index in 0..4 {
            rect.set_edge_construction(RectEdge::from_index(edge_index), true);
        }
        assert!(!rect.has_mixed_edge_construction());
        assert!(rect.all_edges_construction());
    }

    #[test]
    fn sketch_has_geometry_detects_primitives() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        assert!(!doc.sketch_has_geometry(sketch));
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 1.0, 1.0));
        assert!(doc.sketch_has_geometry(sketch));
    }

    #[test]
    fn circle_diameter_is_twice_radius() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        let circle = Circle::from_local_center_radius(sketch, 0.0, 0.0, 5.0, 0.0);
        assert!((circle.diameter() - 10.0).abs() < 1e-4);
    }

    #[test]
    fn default_document_units_are_mm_and_deg() {
        let doc = Document::default();
        assert_eq!(doc.default_length_unit, LengthUnit::Mm);
        assert_eq!(doc.default_angle_unit, AngleUnit::Deg);
    }

    #[test]
    fn new_sketch_inherits_document_units_by_default() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        assert_eq!(doc.sketches[sketch].length_unit, None);
        assert_eq!(doc.sketches[sketch].angle_unit, None);
        assert_eq!(effective_length_unit(&doc, sketch), LengthUnit::Mm);
        assert_eq!(effective_angle_unit(&doc, sketch), AngleUnit::Deg);
    }

    #[test]
    fn effective_units_follow_document_default_change() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.default_length_unit = LengthUnit::In;
        doc.default_angle_unit = AngleUnit::Rad;
        assert_eq!(effective_length_unit(&doc, sketch), LengthUnit::In);
        assert_eq!(effective_angle_unit(&doc, sketch), AngleUnit::Rad);
    }

    #[test]
    fn sketch_override_takes_precedence_over_document_default() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.sketches[sketch].length_unit = Some(LengthUnit::Cm);
        doc.sketches[sketch].angle_unit = Some(AngleUnit::Rad);
        assert_eq!(effective_length_unit(&doc, sketch), LengthUnit::Cm);
        assert_eq!(effective_angle_unit(&doc, sketch), AngleUnit::Rad);
        // Document default is unaffected by the sketch's override.
        assert_eq!(doc.default_length_unit, LengthUnit::Mm);
    }

    #[test]
    fn effective_units_for_missing_sketch_fall_back_to_document_default() {
        let doc = Document::default();
        assert_eq!(effective_length_unit(&doc, 99), LengthUnit::Mm);
        assert_eq!(effective_angle_unit(&doc, 99), AngleUnit::Deg);
    }
}