//! In-memory document model.
//!
//! This is the very first slice of LE3 (see SPEC.md): a document is a flat list
//! of rectangles and lines on a single 2D sketch. As the action-DAG, components,
//! and the OCCT kernel come online this will grow, but the persistence boundary
//! (`storage.rs`) is kept narrow so the file format can evolve underneath it.

use crate::face::default_xy_plane;
use serde::{Deserialize, Serialize};

/// A sketchable face that lines and rectangles can be drawn on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaceId {
    Rect(usize),
    Circle(usize),
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
}

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
        }
    }

    pub fn length(&self) -> f32 {
        let du = self.x1 - self.x0;
        let dv = self.y1 - self.y0;
        (du * du + dv * dv).sqrt()
    }
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintPoint {
    LineEndpoint { line: usize, end: LineEnd },
    /// Corner index 0–3 matches [`crate::face::rect_world_corners_in_frame`] order.
    RectCorner { rect: usize, corner: u8 },
    CircleCenter(usize),
}

/// A line-like sketch entity for parallel, perpendicular, and orientation constraints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintLine {
    Line(usize),
    RectEdge { rect: usize, edge: RectEdge },
}

/// +1 or -1 disambiguation for constraints with two valid solutions.
pub type ConstraintSign = i8;

pub fn default_constraint_sign() -> ConstraintSign {
    1
}

/// Geometry a distance constraint applies to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// A closed sketch profile (face) included in an extrusion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtrudeFace {
    Rect(usize),
    Circle(usize),
}

impl ExtrudeFace {
    /// The sketchable face this profile corresponds to.
    pub fn face_id(self) -> FaceId {
        match self {
            ExtrudeFace::Rect(i) => FaceId::Rect(i),
            ExtrudeFace::Circle(i) => FaceId::Circle(i),
        }
    }
}

/// An object an extrusion is constrained to reach (its extended plane), instead of a fixed
/// distance.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
}

/// The feature that produced a solid body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodySource {
    Extrusion(usize),
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
    pub shape_order: Vec<ShapeKind>,
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
            shape_order: Vec::new(),
        }
    }
}

impl Document {
    pub fn sketch_face(&self, sketch: SketchId) -> Option<FaceId> {
        self.sketches.get(sketch).map(|s| s.face)
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

    pub fn has_children(&self, face: FaceId) -> bool {
        self.sketches.iter().any(|s| s.face == face)
    }

    pub fn add_sketch(&mut self, face: FaceId) -> SketchId {
        let id = self.sketches.len();
        self.sketches.push(Sketch {
            face,
            name: None,
            deleted: false,
        });
        self.shape_order.push(ShapeKind::Sketch);
        id
    }
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
}