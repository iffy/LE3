//! Shared action layer (SPEC §8, §9, §11.2).
//!
//! GUI buttons, keyboard shortcuts, and instruction scripts all dispatch the
//! same [`Action`] values so behaviour stays in sync.

use crate::camera::{
    Camera, ProjectionMode, StandardView, SKETCH_EDIT_FRAME_PADDING_PX, VIEW_TRANSITION_DURATION,
};
use crate::construction::{
    apply_construction_plane_edit, definition_from_reference, plane_from_definition,
    reference_from_definition, resolve_plane, AxisGizmoDrag, PlaneDim, PlaneReference,
};
use crate::model::ConstructionPlaneParent;
use crate::face::{
    sketch_camera_target, sketch_frame, sketch_geometry_frame, sketch_label, sketch_view_up,
    world_to_local,
};
use crate::context::{
    construction_targets_from_selection, set_construction_for_targets, set_edge_construction,
    toggle_construction_for_targets,
};
use crate::hierarchy::SceneElement;
use crate::hierarchy::ElementVisibility;
use crate::names::{element_name, set_element_name, single_nameable_from_selection};
use crate::document_health::{
    health_status_label, recompute_document_health, require_constraint_editable,
    require_dimension_target_editable, require_element_editable,
    require_parameter_editable,
    selection_frozen_summary, DocumentHealth,
};
use crate::document_lifecycle::{delete_targets_from_selection, tombstone_elements};
use crate::selection::{click_scene_selection, SceneSelection};
use crate::model::SketchId;
use crate::view_cube::{self, CubeCornerId, CubeEdgeId};
use crate::constraints::{
    add_distance_constraint, apply_dimension_expression, constraint_expression,
    default_dimension_expression, dimension_edit_from_selection, find_dimension_constraint,
    find_distance_constraint, set_constraint_dim_offset, set_constraint_expression, ConstraintId,
};
use crate::model::{
    Circle, ConstructionPlane, ConstraintPoint, DimensionTarget, DistanceTarget, Document,
    ExtrudeFace, Extrusion, FaceId, Line, LineEnd, Rect, RectEdge, ShapeKind,
};
use crate::vertex_drag;
use crate::face::SketchFrame;
use crate::parameters::{
    add_computed_parameter_from_line_length, add_parameter, delete_parameter,
    recompute_document_geometry, require_parameter_value_editable, set_parameter_expression,
    try_commit_inline_parameter_definition,
    set_parameter_name, ParametersPaneState,
};
use crate::value::parse_positive_length_or_in_doc;
use eframe::egui;
use glam::Vec3;

/// The active viewport tool.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tool {
    /// Orbit/zoom only; no drawing.
    #[default]
    Select,
    /// Click to fix first corner of rectangle; move to position opposite corner;
    /// on-screen number inputs allow typing constraints; Enter commits.
    Rectangle,
    /// Click to fix first endpoint; move mouse for direction and length;
    /// on-screen length input allows typing a constraint; Enter commits.
    Line,
    /// Click to fix center; move mouse for radius; on-screen diameter input allows
    /// typing a constraint; Enter commits.
    Circle,
    /// Click a face or axis/line, then set offset (and angle for axes); Enter commits.
    ConstructionPlane,
    /// Pick a face to enter sketch mode; line/rectangle tools draw on that face.
    Sketch,
    /// Click a line segment to add or edit a distance constraint.
    Dimension,
    /// Select sketch entities and apply geometric constraints from the context pane.
    Constraint,
    /// Click coplanar faces to include them, then set a distance to extrude a solid.
    Extrude,
}

impl Tool {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "select" => Some(Tool::Select),
            "rectangle" | "rect" => Some(Tool::Rectangle),
            "line" => Some(Tool::Line),
            "circle" => Some(Tool::Circle),
            "plane" | "construction_plane" | "constructionplane" | "construction plane" => {
                Some(Tool::ConstructionPlane)
            }
            "sketch" => Some(Tool::Sketch),
            "dimension" | "dim" => Some(Tool::Dimension),
            "constraint" | "constraints" => Some(Tool::Constraint),
            "extrude" => Some(Tool::Extrude),
            _ => None,
        }
    }

    pub fn is_sketch_edit_tool(self) -> bool {
        matches!(
            self,
            Tool::Rectangle | Tool::Line | Tool::Circle | Tool::Dimension | Tool::Constraint
        )
    }
}

/// State for the in-progress (pre-Enter) rectangle creation.
#[derive(Clone, Debug)]
pub struct CreatingRect {
    /// Fixed first corner in ground coords.
    pub origin: Vec3,
    /// Text content of the two dimension inputs (width, height).
    pub texts: [String; 2],
    /// 0 = width (horiz side), 1 = height (vert side)
    pub focused: usize,
    /// Current mouse projected ground point (drives free dimension + signs).
    pub last_mouse: Vec3,
    /// Tracks whether user has typed into each field.
    pub user_edited: [bool; 2],
    /// When true, the focused dimension input should claim keyboard focus.
    pub pending_focus: bool,
    /// New rectangle edges are committed as construction geometry when true.
    pub construction: bool,
}

impl CreatingRect {
    /// Current opposite corner in world space, respecting locked dimensions.
    pub fn end_point(&self, frame: &SketchFrame, doc: &Document) -> Vec3 {
        let (ou, ov) = world_to_local(frame, self.origin);
        let (mu, mv) = world_to_local(frame, self.last_mouse);
        let du = mu - ou;
        let dv = mv - ov;
        let w = if self.user_edited[0] {
            parse_positive_length_or_in_doc(&self.texts[0], doc, du.abs())
        } else {
            du.abs()
        };
        let h = if self.user_edited[1] {
            parse_positive_length_or_in_doc(&self.texts[1], doc, dv.abs())
        } else {
            dv.abs()
        };
        let su = if du < 0.0 { -1.0 } else { 1.0 };
        let sv = if dv < 0.0 { -1.0 } else { 1.0 };
        crate::face::local_to_world(frame, ou + su * w, ov + sv * h)
    }
}

/// State for the in-progress (pre-Enter) line creation.
#[derive(Clone, Debug)]
pub struct CreatingLine {
    /// Fixed first endpoint in ground coords.
    pub origin: Vec3,
    /// Text content of the length input.
    pub text: String,
    /// Current mouse projected ground point (drives free length + direction).
    pub last_mouse: Vec3,
    /// Tracks whether user has typed into the length field.
    pub user_edited: bool,
    /// When true, the length input should claim keyboard focus.
    pub pending_focus: bool,
    /// Committed line is construction geometry when true.
    pub construction: bool,
}

/// State for the in-progress (pre-Enter) circle creation.
#[derive(Clone, Debug)]
pub struct CreatingCircle {
    /// Fixed center in ground coords.
    pub origin: Vec3,
    /// Text content of the diameter input.
    pub text: String,
    /// Current mouse projected ground point (drives free radius + direction).
    pub last_mouse: Vec3,
    /// Tracks whether user has typed into the diameter field.
    pub user_edited: bool,
    /// When true, the diameter input should claim keyboard focus.
    pub pending_focus: bool,
    /// Committed circle is construction geometry when true.
    pub construction: bool,
}

impl CreatingCircle {
    pub fn radius(&self, frame: &SketchFrame, doc: &Document) -> f32 {
        let (cu, cv) = world_to_local(frame, self.origin);
        let (mu, mv) = world_to_local(frame, self.last_mouse);
        let du = mu - cu;
        let dv = mv - cv;
        let dist = (du * du + dv * dv).sqrt();
        if self.user_edited {
            parse_positive_length_or_in_doc(&self.text, doc, dist * 2.0) / 2.0
        } else {
            dist
        }
    }

    pub fn diameter_dim_angle(&self, frame: &SketchFrame) -> f32 {
        let (cu, cv) = world_to_local(frame, self.origin);
        let (mu, mv) = world_to_local(frame, self.last_mouse);
        let du = mu - cu;
        let dv = mv - cv;
        if du * du + dv * dv < 1e-12 {
            0.0
        } else {
            dv.atan2(du)
        }
    }

    /// Point on the circle rim in world space, respecting any locked diameter.
    pub fn rim_point(&self, frame: &SketchFrame, doc: &Document) -> Vec3 {
        let r = self.radius(frame, doc);
        let angle = self.diameter_dim_angle(frame);
        let (cu, cv) = world_to_local(frame, self.origin);
        crate::face::local_to_world(
            frame,
            cu + angle.cos() * r,
            cv + angle.sin() * r,
        )
    }
}

/// In-progress (or being-edited) extrusion: selected faces + live signed distance.
#[derive(Clone, Debug)]
pub struct CreatingExtrusion {
    /// Sketch plane the faces lie on (all faces are coplanar).
    pub sketch: SketchId,
    pub faces: Vec<ExtrudeFace>,
    /// Live signed distance along the plane normal (gizmo-driven).
    pub distance: f32,
    /// Distance input text (magnitude); the sign follows `distance`.
    pub text: String,
    pub user_edited: bool,
    pub pending_focus: bool,
    /// When set, the depth is constrained to this object's extended plane.
    pub target: Option<crate::model::ExtrudeTarget>,
    /// `Some` when editing an existing extrusion rather than creating one.
    pub edit_index: Option<usize>,
}

impl CreatingExtrusion {
    /// Evaluated signed distance: typed magnitude (if edited) keeps the live sign.
    pub fn evaluated_distance(&self, doc: &Document) -> f32 {
        if self.user_edited {
            let magnitude = parse_positive_length_or_in_doc(&self.text, doc, self.distance.abs());
            let sign = if self.distance < 0.0 { -1.0 } else { 1.0 };
            magnitude * sign
        } else {
            self.distance
        }
    }
}

impl CreatingLine {
    /// Current second endpoint in world space, respecting any locked length.
    pub fn end_point(&self, frame: &SketchFrame, doc: &Document) -> Vec3 {
        let (ou, ov) = world_to_local(frame, self.origin);
        let (mu, mv) = world_to_local(frame, self.last_mouse);
        let du = mu - ou;
        let dv = mv - ov;
        let dist = (du * du + dv * dv).sqrt();
        let len = if self.user_edited {
            parse_positive_length_or_in_doc(&self.text, doc, dist)
        } else {
            dist
        };
        if dist < 1e-6 {
            return crate::face::local_to_world(frame, ou + len, ov);
        }
        let scale = len / dist;
        crate::face::local_to_world(frame, ou + du * scale, ov + dv * scale)
    }
}

/// State for creating or editing a construction plane.
#[derive(Clone, Debug)]
pub struct CreatingConstructionPlane {
    /// When set, commit updates this plane instead of adding a new one.
    pub edit_index: Option<usize>,
    pub reference: PlaneReference,
    pub parent: ConstructionPlaneParent,
    pub offset_text: String,
    pub angle_text: String,
    pub focused: PlaneDim,
    /// Live offset (mm); updated by gizmo drag or wheel.
    pub offset_live: f32,
    /// Live angle for axis references (degrees); updated by gizmo drag.
    pub axis_angle_deg: f32,
    pub user_edited_offset: bool,
    pub user_edited_angle: bool,
    pub pending_focus: bool,
    pub axis_gizmo_drag: Option<AxisGizmoDrag>,
}

impl CreatingConstructionPlane {
    pub fn preview_plane(&self) -> ConstructionPlane {
        let (live_offset, live_angle) = self.live_dims();
        resolve_plane(
            &self.reference,
            &self.offset_text,
            &self.angle_text,
            live_offset,
            live_angle,
            self.user_edited_offset,
            self.user_edited_angle,
        )
    }

    pub fn resolved_definition(&self) -> crate::model::PlaneDefinition {
        let (live_offset, live_angle) = self.live_dims();
        let offset = if self.user_edited_offset {
            crate::value::parse_length_or(&self.offset_text, live_offset)
        } else {
            live_offset
        };
        let angle = if self.user_edited_angle {
            self.angle_text
                .trim()
                .parse::<f32>()
                .unwrap_or(live_angle)
                .rem_euclid(360.0)
        } else {
            live_angle
        };
        definition_from_reference(&self.reference, offset, angle)
    }

    pub fn live_dims(&self) -> (f32, f32) {
        match &self.reference {
            PlaneReference::Face { .. } => (self.offset_live, 0.0),
            PlaneReference::Axis { .. } => (self.offset_live, self.axis_angle_deg),
        }
    }
}

/// Every user-visible operation the app supports.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    NewDocument,
    Open { path: String },
    Save { path: Option<String> },
    /// Export bodies to an STL file. `body` names a single body; `None` exports all bodies.
    ExportStl { path: String, body: Option<String> },
    Clear,
    UndoLast,
    SetTool(Tool),
    CancelOperation,
    CommitRectangle,
    SetRectDimension { axis: RectAxis, value: String },
    FocusRectDimension { axis: RectAxis },
    CommitLine,
    SetLineLength { value: String },
    FocusLineLength,
    CommitCircle,
    SetCircleDiameter { value: String },
    FocusCircleDiameter,
    SetDimLabelOffset {
        target: DimLabelTarget,
        offset: f32,
    },
    SetConstraintAngleValue {
        constraint_id: ConstraintId,
        angle_rad: f32,
    },
    BeginEditCommittedDim { target: DimLabelTarget },
    BeginDimensionEdit { target: DimensionTarget },
    CommitCommittedDim,
    BeginConstructionPlane {
        reference: PlaneReference,
        parent: ConstructionPlaneParent,
    },
    BeginEditConstructionPlane {
        index: usize,
    },
    CommitConstructionPlane,
    SetPlaneOffset { value: String },
    SetPlaneAngle { value: String },
    FocusPlaneDim { dim: PlaneDim },
    BeginSketch {
        face: FaceId,
        viewport: Option<egui::Rect>,
    },
    OpenSketch {
        sketch: SketchId,
        viewport: Option<egui::Rect>,
    },
    ExitSketch,
    SetElementVisible {
        element: SceneElement,
        visible: bool,
    },
    ToggleElementVisibility(SceneElement),
    OrbitCamera { delta: (f32, f32) },
    PanCamera { delta: (f32, f32), viewport_height: f32 },
    ZoomCamera {
        scroll: f32,
        focal: egui::Pos2,
        viewport: egui::Rect,
    },
    SetStandardView(StandardView),
    SetViewEdge(CubeEdgeId),
    SetViewCorner(CubeCornerId),
    ViewHome,
    SetHomeView,
    SetProjectionMode(ProjectionMode),
    ToggleProjectionMode,
    SetPaneVisible { pane: Pane, visible: bool },
    TogglePane(Pane),
    AddParameter { name: String, expression: String },
    /// Create a read-only parameter synced to an unconstrained line's length.
    CreateParameterFromLineLength { line_index: usize, name: Option<String> },
    CommitParameterName { index: usize, name: String },
    CommitParameterExpression { index: usize, expression: String },
    DeleteParameter { index: usize },
    /// Tombstone every element in the current scene selection.
    DeleteSelection,
    SetCommandPaletteOpen { open: bool },
    ToggleCommandPalette,
    ClickSceneElement {
        element: SceneElement,
        additive: bool,
    },
    ClearSceneSelection,
    SetShapeConstruction {
        element: SceneElement,
        construction: bool,
    },
    /// Set construction/substantial on the active draw op or all constructable selected targets.
    ApplyConstruction {
        construction: bool,
    },
    /// Toggle construction/substantial on the active draw op or each constructable selected target.
    ToggleConstruction,
    CommitElementName {
        element: SceneElement,
        name: String,
    },
    FocusElementName,
    /// Apply a geometric constraint type to the current selection (constraint tool).
    AddGeometricConstraint(crate::geometric_constraints::GeometricConstraintType),
    /// Apply the enabled constraint matching its mnemonic shortcut key (A/T/I/M/V/H).
    ApplyConstraintShortcut(char),
    /// Move a sketch vertex to local `(u, v)` while satisfying constraints.
    DragVertex {
        point: ConstraintPoint,
        u: f32,
        v: f32,
    },
    /// Start dragging a line or rectangle edge from an anchor in sketch-local coords.
    BeginLineDrag {
        target: crate::model::ConstraintLine,
        anchor_u: f32,
        anchor_v: f32,
    },
    /// Continue dragging the active line segment to sketch-local `(u, v)`.
    DragLine { u: f32, v: f32 },
    /// Finish an interactive line drag.
    EndLineDrag,
    /// Create a rectangle directly in the active sketch (face-local mm) with locked dimensions.
    CreateRectangle {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    /// Create a line directly in the active sketch (face-local mm) with a locked length.
    CreateLineSegment {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    },
    /// Create a circle directly in the active sketch (face-local mm) with a locked diameter.
    CreateCircle {
        cx: f32,
        cy: f32,
        r: f32,
    },
    /// Create an extrusion solid from coplanar sketch faces.
    CreateExtrusion {
        sketch: SketchId,
        faces: Vec<ExtrudeFace>,
        distance: f32,
    },
    /// Add/remove a face from the in-progress extrusion (starts one if needed).
    ToggleExtrudeFace { face: ExtrudeFace },
    /// Set the live (gizmo-driven) extrusion distance.
    SetExtrudeDistance { distance: f32 },
    /// Constrain (or unconstrain) the in-progress extrusion to an object's extended plane.
    SetExtrudeTarget {
        target: Option<crate::model::ExtrudeTarget>,
    },
    /// Begin editing an existing extrusion.
    EditExtrusion { index: usize },
    /// Finalize the in-progress extrusion (create or update).
    CommitExtrusion,
    /// Enable or disable snapping while drawing/dragging.
    SetSnapping(bool),
    /// Add the constraint implied by leaving `point` on a snap target.
    ApplySnapConstraint {
        point: ConstraintPoint,
        target: crate::snapping::SnapTarget,
    },
}

/// A toggleable UI pane (SPEC §11.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// The rotating orientation cube in the viewport corner ([`view_cube`]).
    ViewCube,
    /// Scene tree with visibility toggles and sketch editing.
    Hierarchy,
    /// Named parameters and expressions.
    Parameters,
    /// Properties for the current tree selection.
    Context,
}

impl Pane {
    /// All panes, in menu order.
    pub const ALL: &'static [Pane] = &[Pane::Hierarchy, Pane::Context, Pane::Parameters, Pane::ViewCube];

    /// Human-readable label for menus.
    pub fn label(self) -> &'static str {
        match self {
            Pane::ViewCube => "Orientation Cube",
            Pane::Hierarchy => "Elements",
            Pane::Parameters => "Parameters",
            Pane::Context => "Context",
        }
    }

    /// Stable name used in instruction scripts.
    pub fn script_name(self) -> &'static str {
        match self {
            Pane::ViewCube => "view_cube",
            Pane::Hierarchy => "hierarchy",
            Pane::Parameters => "parameters",
            Pane::Context => "context",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "view_cube" | "viewcube" | "cube" | "hud" => Some(Pane::ViewCube),
            "hierarchy" | "tree" | "dag" | "elements" => Some(Pane::Hierarchy),
            "parameters" | "params" | "param" => Some(Pane::Parameters),
            "context" | "properties" | "props" => Some(Pane::Context),
            _ => None,
        }
    }
}

/// Which panes are currently shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneVisibility {
    pub view_cube: bool,
    pub hierarchy: bool,
    pub parameters: bool,
    pub context: bool,
}

impl Default for PaneVisibility {
    fn default() -> Self {
        Self {
            view_cube: true,
            hierarchy: true,
            parameters: true,
            context: true,
        }
    }
}

impl PaneVisibility {
    pub fn is_visible(&self, pane: Pane) -> bool {
        match pane {
            Pane::ViewCube => self.view_cube,
            Pane::Hierarchy => self.hierarchy,
            Pane::Parameters => self.parameters,
            Pane::Context => self.context,
        }
    }

    pub fn set(&mut self, pane: Pane, visible: bool) {
        match pane {
            Pane::ViewCube => self.view_cube = visible,
            Pane::Hierarchy => self.hierarchy = visible,
            Pane::Parameters => self.parameters = visible,
            Pane::Context => self.context = visible,
        }
    }

    pub fn toggle(&mut self, pane: Pane) {
        let next = !self.is_visible(pane);
        self.set(pane, next);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DimLabelAxis {
    Width,
    Height,
    Length,
}

impl DimLabelAxis {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "width" | "w" => Some(Self::Width),
            "height" | "h" => Some(Self::Height),
            "length" | "len" | "l" => Some(Self::Length),
            _ => None,
        }
    }
}

pub fn dim_label_target_in_sketch(
    doc: &Document,
    sketch: SketchId,
    axis: DimLabelAxis,
) -> Option<DimLabelTarget> {
    let target = match axis {
        DimLabelAxis::Width => doc
            .rects
            .iter()
            .enumerate()
            .rev()
            .find(|(_, r)| r.sketch == sketch)
            .map(|(index, _)| DistanceTarget::RectWidth(index)),
        DimLabelAxis::Height => doc
            .rects
            .iter()
            .enumerate()
            .rev()
            .find(|(_, r)| r.sketch == sketch)
            .map(|(index, _)| DistanceTarget::RectHeight(index)),
        DimLabelAxis::Length => doc
            .lines
            .iter()
            .enumerate()
            .rev()
            .find(|(_, l)| l.sketch == sketch)
            .map(|(index, _)| DistanceTarget::LineLength(index)),
    }?;
    find_distance_constraint(doc, target)
}

/// A committed sketch dimension label the user can reposition.
pub type DimLabelTarget = ConstraintId;

/// Whether a constraint edit applies to a rectangle axis.
pub fn constraint_matches_rect_axis(doc: &Document, target: DimLabelTarget, axis: RectAxis) -> bool {
    let Some(constraint) = doc.constraints.get(target) else {
        return false;
    };
    matches!(
        (&constraint.kind, axis),
        (
            crate::model::ConstraintKind::Distance {
                target: DistanceTarget::RectWidth(_)
            },
            RectAxis::Width
        ) | (
            crate::model::ConstraintKind::Distance {
                target: DistanceTarget::RectHeight(_)
            },
            RectAxis::Height
        )
    )
}

pub fn constraint_is_line_length(doc: &Document, target: DimLabelTarget) -> bool {
    doc.constraints.get(target).is_some_and(|c| {
        matches!(
            c.kind,
            crate::model::ConstraintKind::Distance {
                target: DistanceTarget::LineLength(_)
            }
        )
    })
}

pub fn constraint_is_circle_diameter(doc: &Document, target: DimLabelTarget) -> bool {
    doc.constraints.get(target).is_some_and(|c| {
        matches!(
            c.kind,
            crate::model::ConstraintKind::Distance {
                target: DistanceTarget::CircleDiameter(_)
            }
        )
    })
}

pub fn constraint_is_angle(doc: &Document, target: DimLabelTarget) -> bool {
    doc.constraints.get(target).is_some_and(|c| {
        matches!(c.kind, crate::model::ConstraintKind::Angle { .. })
    })
}

pub fn dim_label_axis_for_target(doc: &Document, target: DimLabelTarget) -> Option<DimLabelAxis> {
    if constraint_matches_rect_axis(doc, target, RectAxis::Width) {
        Some(DimLabelAxis::Width)
    } else if constraint_matches_rect_axis(doc, target, RectAxis::Height) {
        Some(DimLabelAxis::Height)
    } else if constraint_is_line_length(doc, target) {
        Some(DimLabelAxis::Length)
    } else {
        None
    }
}

/// In-progress edit of a sketch dimension (Select or Dimension tool).
#[derive(Clone, Debug, PartialEq)]
pub enum DimEditTarget {
    Constraint(ConstraintId),
    New(DimensionTarget),
}

impl DimEditTarget {
    pub fn dimension_target(&self, doc: &Document) -> Option<DimensionTarget> {
        match self {
            DimEditTarget::New(target) => Some(*target),
            DimEditTarget::Constraint(id) => doc.constraints.get(*id).and_then(|c| match c.kind {
                crate::model::ConstraintKind::Distance { target } => {
                    Some(DimensionTarget::Distance(target))
                }
                crate::model::ConstraintKind::Angle {
                    line_a,
                    line_b,
                    rotation_sign,
                } => Some(DimensionTarget::Angle {
                    line_a,
                    line_b,
                    rotation_sign,
                }),
                _ => None,
            }),
        }
    }

    pub fn distance_target(&self, doc: &Document) -> Option<DistanceTarget> {
        match self.dimension_target(doc)? {
            DimensionTarget::Distance(target) => Some(target),
            DimensionTarget::Angle { .. } => None,
        }
    }

    pub fn is_angle(&self, doc: &Document) -> bool {
        matches!(
            self.dimension_target(doc),
            Some(DimensionTarget::Angle { .. })
        )
    }
}

/// Committed angle constraint whose gizmo should be visible while its text field is open.
pub fn angle_gizmo_constraint_for_edit(
    edit: Option<&EditingCommittedDim>,
    doc: &Document,
) -> Option<ConstraintId> {
    let edit = edit?;
    if !edit.target.is_angle(doc) {
        return None;
    }
    match edit.target {
        DimEditTarget::Constraint(id) => Some(id),
        DimEditTarget::New(_) => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditingCommittedDim {
    pub target: DimEditTarget,
    pub text: String,
    pub pending_focus: bool,
}

/// Expression text shown when editing a committed dimension.
pub fn committed_dim_expression(doc: &Document, target: DimLabelTarget) -> Option<String> {
    constraint_expression(doc, target)
}

fn apply_committed_dim_expression(
    doc: &mut Document,
    sketch: SketchId,
    target: DimEditTarget,
    expression: &str,
) -> Result<(), String> {
    match target {
        DimEditTarget::Constraint(id) => set_constraint_expression(doc, id, expression.to_string()),
        DimEditTarget::New(dimension_target) => {
            apply_dimension_expression(doc, sketch, dimension_target, expression)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RectAxis {
    Width,
    Height,
}

impl RectAxis {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "width" | "w" => Some(RectAxis::Width),
            "height" | "h" => Some(RectAxis::Height),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        match self {
            RectAxis::Width => 0,
            RectAxis::Height => 1,
        }
    }
}

/// Active sketch session: new geometry is parented to this sketch until exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SketchSession {
    pub sketch: SketchId,
}

/// Transient UI state for the command palette (SPEC §11.2).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandPaletteState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    pub request_focus: bool,
    /// Previous query text; used to reset selection when the filter changes.
    pub prior_query: String,
}

impl CommandPaletteState {
    pub fn open_palette(&mut self) {
        self.open = true;
        self.query.clear();
        self.prior_query.clear();
        self.selected = 0;
        self.request_focus = true;
    }

    pub fn close_palette(&mut self) {
        self.open = false;
        self.query.clear();
        self.prior_query.clear();
        self.selected = 0;
        self.request_focus = false;
    }
}

/// Application state that actions mutate.
pub struct AppState {
    pub doc: Document,
    pub path: Option<String>,
    pub tool: Tool,
    pub sketch_session: Option<SketchSession>,
    pub cam: Camera,
    pub creating_rect: Option<CreatingRect>,
    pub creating_line: Option<CreatingLine>,
    pub creating_circle: Option<CreatingCircle>,
    /// In-progress (or being-edited) extrusion: selected faces + live distance.
    pub creating_extrusion: Option<CreatingExtrusion>,
    /// Shared construction draw mode for rectangle, line, and circle tools.
    pub draw_construction: bool,
    pub creating_plane: Option<CreatingConstructionPlane>,
    pub panes: PaneVisibility,
    pub parameters_pane: ParametersPaneState,
    pub command_palette: CommandPaletteState,
    pub element_visibility: ElementVisibility,
    pub scene_selection: SceneSelection,
    pub context_pane: crate::context::ContextPaneState,
    pub editing_committed_dim: Option<EditingCommittedDim>,
    pub status: String,
    pub command_log: Option<std::cell::RefCell<crate::command_log::CommandLog>>,
    /// Reframe sketch geometry once the viewport rect is known (e.g. hierarchy open before first paint).
    pub sketch_reframe_pending: bool,
    /// Camera pose captured when entering a sketch, restored when exiting it.
    pub pre_sketch_pose: Option<crate::camera::HomeView>,
    pub document_health: DocumentHealth,
    pub line_drag_session: Option<crate::vertex_drag::LineDragSession>,
    /// Snap a moved/drawn point to nearby geometry (and add a constraint when left there).
    pub snapping_enabled: bool,
    /// The point being dragged and what it is currently snapped to (committed on release).
    pub active_snap: Option<(ConstraintPoint, crate::snapping::SnapTarget)>,
    /// Snap targets for the start/end of a line being drawn (applied on commit).
    pub line_start_snap: Option<crate::snapping::SnapTarget>,
    pub line_end_snap: Option<crate::snapping::SnapTarget>,
    /// Snap targets for the origin/opposite corners of a rectangle being drawn.
    pub rect_origin_snap: Option<crate::snapping::SnapTarget>,
    pub rect_opposite_snap: Option<crate::snapping::SnapTarget>,
    /// Snap target for the center of a circle being drawn.
    pub circle_center_snap: Option<crate::snapping::SnapTarget>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            doc: Document::default(),
            path: None,
            tool: Tool::default(),
            sketch_session: None,
            cam: Camera::default(),
            creating_rect: None,
            creating_line: None,
            creating_circle: None,
            creating_extrusion: None,
            draw_construction: false,
            creating_plane: None,
            panes: PaneVisibility::default(),
            parameters_pane: ParametersPaneState::default(),
            command_palette: CommandPaletteState::default(),
            element_visibility: ElementVisibility::default(),
            scene_selection: SceneSelection::default(),
            context_pane: crate::context::ContextPaneState::default(),
            editing_committed_dim: None,
            status: String::new(),
            command_log: None,
            sketch_reframe_pending: false,
            pre_sketch_pose: None,
            document_health: DocumentHealth::default(),
            line_drag_session: None,
            snapping_enabled: true,
            active_snap: None,
            line_start_snap: None,
            line_end_snap: None,
            rect_origin_snap: None,
            rect_opposite_snap: None,
            circle_center_snap: None,
        }
    }
}

impl AppState {
    pub fn refresh_document_health(&mut self) {
        self.document_health = recompute_document_health(&self.doc);
    }
}

/// Default starting extrusion distance (mm).
pub const DEFAULT_EXTRUDE_DISTANCE: f32 = 10.0;

/// The sketch a face (rect/circle profile) belongs to.
fn extrude_face_sketch(doc: &Document, face: ExtrudeFace) -> Option<SketchId> {
    match face {
        ExtrudeFace::Rect(i) => doc.rects.get(i).map(|r| r.sketch),
        ExtrudeFace::Circle(i) => doc.circles.get(i).map(|c| c.sketch),
    }
}

/// Corner index (0–3) of `rect` nearest to local point `(u, v)`.
fn rect_corner_index_at(rect: &Rect, u: f32, v: f32) -> u8 {
    let corners = [
        (rect.x, rect.y),
        (rect.x + rect.w, rect.y),
        (rect.x + rect.w, rect.y + rect.h),
        (rect.x, rect.y + rect.h),
    ];
    let mut best = 0u8;
    let mut best_d = f32::INFINITY;
    for (i, (cu, cv)) in corners.iter().enumerate() {
        let d = (cu - u).powi(2) + (cv - v).powi(2);
        if d < best_d {
            best_d = d;
            best = i as u8;
        }
    }
    best
}

fn pane_status(pane: Pane, visible: bool) -> String {
    format!("{} {}", pane.label(), if visible { "shown" } else { "hidden" })
}

fn draw_mode_status(tool: &str, construction: bool) -> String {
    format!(
        "{tool} draw mode: {}",
        if construction {
            "construction"
        } else {
            "substantial"
        }
    )
}

fn distance_target_status_label(target: DistanceTarget) -> String {
    match target {
        DistanceTarget::LineLength(i) => format!("line {i}"),
        DistanceTarget::RectWidth(i) => format!("rectangle {i} width"),
        DistanceTarget::RectHeight(i) => format!("rectangle {i} height"),
        DistanceTarget::CircleDiameter(i) => format!("circle {i} diameter"),
        DistanceTarget::LineLineDistance { .. } => "parallel line spacing".to_string(),
        DistanceTarget::PointPointDistance { .. } => "point distance".to_string(),
        DistanceTarget::PointLineDistance { .. } => "point-line distance".to_string(),
    }
}

fn dimension_target_status_label(target: DimensionTarget) -> String {
    match target {
        DimensionTarget::Distance(distance) => distance_target_status_label(distance),
        DimensionTarget::Angle { .. } => "angle".to_string(),
    }
}

fn element_label(element: SceneElement) -> String {
    match element {
        SceneElement::ConstructionPlane(i) => format!("Construction plane {i}"),
        SceneElement::Sketch(i) => format!("Sketch {i}"),
        SceneElement::Rect(i) => format!("Rectangle {i}"),
        SceneElement::Line(i) => format!("Line {i}"),
        SceneElement::Circle(i) => format!("Circle {i}"),
        SceneElement::RectEdge(i, edge) => {
            format!("Rectangle {i} {} edge", edge.script_name())
        }
        SceneElement::Constraint(i) => format!("Constraint {i}"),
        SceneElement::Point(_) => "Point".to_string(),
        SceneElement::Extrusion(i) => format!("Extrusion {i}"),
        SceneElement::Body(i) => format!("Body {i}"),
    }
}

fn require_construction_targets_editable(
    health: &DocumentHealth,
    selection: &SceneSelection,
) -> Result<(), String> {
    for element in construction_targets_from_selection(selection) {
        require_element_editable(health, element)?;
    }
    Ok(())
}

/// Result of dispatching an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionResult {
    Ok,
    /// Action needs a file path from a dialog (GUI-only).
    NeedsDialog,
    Err(String),
}

impl AppState {
    /// Whether committed sketch dimensions can be edited or repositioned.
    pub fn can_edit_sketch_dimensions(&self) -> bool {
        self.sketch_session.is_some()
            && self.creating_rect.is_none()
            && self.creating_line.is_none()
            && self.creating_circle.is_none()
    }

    /// Start editing a dimension on the current selection, if applicable.
    pub fn try_begin_dimension_from_selection(&mut self) -> bool {
        let Some(session) = self.sketch_session else {
            return false;
        };
        let Some(target) =
            dimension_edit_from_selection(&self.doc, session.sketch, &self.scene_selection)
        else {
            return false;
        };
        self.start_committed_dimension_edit(target);
        true
    }

    fn start_committed_dimension_edit(&mut self, target: DimensionTarget) {
        if self.sketch_session.is_none()
            || require_dimension_target_editable(&self.document_health, &self.doc, target).is_err()
        {
            return;
        }
        let edit_target = if let Some(id) = find_dimension_constraint(&self.doc, target) {
            DimEditTarget::Constraint(id)
        } else {
            DimEditTarget::New(target)
        };
        let text = match edit_target {
            DimEditTarget::Constraint(id) => committed_dim_expression(&self.doc, id)
                .unwrap_or_else(|| default_dimension_expression(&self.doc, target)),
            DimEditTarget::New(_) => default_dimension_expression(&self.doc, target),
        };
        let kind_label = match target {
            DimensionTarget::Distance(_) => "length",
            DimensionTarget::Angle { .. } => "angle",
        };
        self.editing_committed_dim = Some(EditingCommittedDim {
            target: edit_target,
            text,
            pending_focus: true,
        });
        self.status = format!(
            "Dimension {} • type {kind_label} • Enter commit • Esc cancel",
            dimension_target_status_label(target)
        );
    }

    /// Active or pending construction draw mode while the rectangle tool is selected.
    pub fn rect_draw_construction_mode(&self) -> Option<bool> {
        if self.tool != Tool::Rectangle {
            return None;
        }
        Some(
            self.creating_rect
                .as_ref()
                .map(|cr| cr.construction)
                .unwrap_or(self.draw_construction),
        )
    }

    /// Active or pending construction draw mode while the line tool is selected.
    pub fn line_draw_construction_mode(&self) -> Option<bool> {
        if self.tool != Tool::Line {
            return None;
        }
        Some(
            self.creating_line
                .as_ref()
                .map(|cl| cl.construction)
                .unwrap_or(self.draw_construction),
        )
    }

    /// Active or pending construction draw mode while the circle tool is selected.
    pub fn circle_draw_construction_mode(&self) -> Option<bool> {
        if self.tool != Tool::Circle {
            return None;
        }
        Some(
            self.creating_circle
                .as_ref()
                .map(|cc| cc.construction)
                .unwrap_or(self.draw_construction),
        )
    }

    pub fn apply(&mut self, action: Action) -> ActionResult {
        let logged_action = self.command_log.is_some().then(|| action.clone());
        if let Some(log) = &self.command_log {
            log.borrow_mut().before_apply(&action, &self.cam);
        }
        let result = match action {
            Action::NewDocument => {
                self.doc = Document::default();
                self.path = None;
                self.sketch_session = None;
                self.cam.set_view_up(None);
                self.creating_rect = None;
                self.creating_line = None;
                self.creating_circle = None;
                self.creating_plane = None;
                self.element_visibility = ElementVisibility::default();
                self.scene_selection.clear();
                self.tool = Tool::Select;
                self.document_health = DocumentHealth::default();
                self.status = "New document".to_string();
                ActionResult::Ok
            }
            Action::Open { path } => match crate::storage::open(&path) {
                Ok(mut doc) => {
                    if let Err(e) = recompute_document_geometry(&mut doc) {
                        self.status = format!("Open failed: {e}");
                        return ActionResult::Err(e);
                    }
                    let n_rects = doc.rects.len();
                    let n_lines = doc.lines.len();
                    self.doc = doc;
                    self.sketch_session = None;
                    self.cam.set_view_up(None);
                    self.refresh_document_health();
                    self.path = Some(path.clone());
                    self.status = format!(
                        "Opened {} ({} rectangle(s), {} line(s))",
                        path, n_rects, n_lines
                    );
                    ActionResult::Ok
                }
                Err(e) => {
                    self.status = format!("Open failed: {e}");
                    ActionResult::Err(e)
                }
            },
            Action::Save { path } => {
                let target = path.or_else(|| self.path.clone());
                match target {
                    Some(p) => self.write_to(&p),
                    None => ActionResult::NeedsDialog,
                }
            }
            Action::ExportStl { path, body } => {
                let (name, mesh) = match &body {
                    Some(name) => {
                        match self.doc.bodies.iter().position(|b| {
                            !b.deleted && b.name.as_deref() == Some(name.as_str())
                        }) {
                            Some(bi) => {
                                (name.clone(), crate::extrude::body_solid_mesh(&self.doc, bi))
                            }
                            None => {
                                self.status = format!("Export failed: no body named '{name}'");
                                return ActionResult::Err(self.status.clone());
                            }
                        }
                    }
                    None => (
                        "le3".to_string(),
                        Some(crate::extrude::document_solid_mesh(&self.doc)),
                    ),
                };
                match mesh {
                    Some(m) if !m.is_empty() => {
                        let stl = crate::stl::write_ascii_stl(&name, &m);
                        match std::fs::write(&path, stl) {
                            Ok(()) => {
                                self.status = format!(
                                    "Exported {} triangle(s) to {}",
                                    m.triangles.len(),
                                    path
                                );
                                ActionResult::Ok
                            }
                            Err(e) => {
                                self.status = format!("Export failed: {e}");
                                ActionResult::Err(self.status.clone())
                            }
                        }
                    }
                    _ => {
                        self.status = "Export failed: no solid geometry to export".to_string();
                        ActionResult::Err(self.status.clone())
                    }
                }
            }
            Action::Clear => {
                self.doc = Document::default();
                self.sketch_session = None;
                self.cam.set_view_up(None);
                self.creating_rect = None;
                self.creating_line = None;
                self.creating_circle = None;
                self.element_visibility = ElementVisibility::default();
                self.document_health = DocumentHealth::default();
                self.status = "Cleared".to_string();
                ActionResult::Ok
            }
            Action::UndoLast => {
                let mut undone = false;
                match self.doc.shape_order.pop() {
                    Some(ShapeKind::Sketch) => {
                        let idx = self.doc.sketches.len().saturating_sub(1);
                        if self.doc.sketch_has_geometry(idx) {
                            self.doc.shape_order.push(ShapeKind::Sketch);
                            self.status = "Cannot undo: sketch has geometry".to_string();
                        } else if self.doc.sketches.is_empty() {
                            self.status = "Nothing to undo".to_string();
                        } else {
                            self.doc.sketches.pop();
                            if self.sketch_session == Some(SketchSession { sketch: idx }) {
                                self.exit_sketch_session();
                            }
                            self.status = "Undid last sketch".to_string();
                            undone = true;
                        }
                    }
                    Some(ShapeKind::Rect) => {
                        self.doc.rects.pop();
                        self.status = "Undid last rectangle".to_string();
                        undone = true;
                    }
                    Some(ShapeKind::Line) => {
                        self.doc.lines.pop();
                        self.status = "Undid last line".to_string();
                        undone = true;
                    }
                    Some(ShapeKind::Circle) => {
                        self.doc.circles.pop();
                        self.status = "Undid last circle".to_string();
                        undone = true;
                    }
                    Some(ShapeKind::Constraint) => {
                        self.doc.constraints.pop();
                        let _ = recompute_document_geometry(&mut self.doc);
                        self.status = "Undid last constraint".to_string();
                        undone = true;
                    }
                    Some(ShapeKind::Parameter) => {
                        self.doc.parameters.pop();
                        self.status = "Undid last parameter".to_string();
                        undone = true;
                    }
                    Some(ShapeKind::Body) => {
                        self.doc.bodies.pop();
                        self.status = "Undid last body".to_string();
                        undone = true;
                    }
                    Some(ShapeKind::Extrusion) => {
                        self.doc.extrusions.pop();
                        self.status = "Undid last extrusion".to_string();
                        undone = true;
                    }
                    Some(ShapeKind::ConstructionPlane) => {
                        if self.doc.construction_planes.len() <= 1 {
                            self.doc.shape_order.push(ShapeKind::ConstructionPlane);
                            self.status = "Cannot undo default datum plane".to_string();
                        } else {
                            let idx = self.doc.construction_planes.len() - 1;
                            let face = FaceId::ConstructionPlane(idx);
                            if self.doc.has_children(face) {
                                self.doc.shape_order.push(ShapeKind::ConstructionPlane);
                                self.status =
                                    "Cannot undo: construction plane has child sketches"
                                        .to_string();
                            } else {
                                self.doc.construction_planes.pop();
                                if self.sketch_session.is_some_and(|s| {
                                    self.doc.sketch_face(s.sketch) == Some(face)
                                }) {
                                    self.exit_sketch_session();
                                }
                                self.status = "Undid last construction plane".to_string();
                                undone = true;
                            }
                        }
                    }
                    None => self.status = "Nothing to undo".to_string(),
                }
                if undone {
                    self.refresh_document_health();
                }
                ActionResult::Ok
            }
            Action::SetTool(tool) => {
                if self.creating_rect.is_some() && tool != Tool::Rectangle {
                    self.creating_rect = None;
                }
                if self.creating_line.is_some() && tool != Tool::Line {
                    self.creating_line = None;
                }
                if self.creating_circle.is_some() && tool != Tool::Circle {
                    self.creating_circle = None;
                }
                if self.creating_plane.is_some() && tool != Tool::ConstructionPlane {
                    self.creating_plane = None;
                }
                if self.creating_extrusion.is_some() && tool != Tool::Extrude {
                    self.creating_extrusion = None;
                }
                // Extruding acts on the 3D model, not sketch geometry: leave sketch
                // editing when the extrude tool is picked from inside a sketch.
                if tool == Tool::Extrude && self.sketch_session.is_some() {
                    self.exit_sketch_session();
                }
                if !matches!(tool, Tool::Select | Tool::Dimension | Tool::Constraint) {
                    self.editing_committed_dim = None;
                }
                self.tool = tool;
                self.status = match tool {
                    Tool::Select => {
                        "Select tool — Delete/Backspace removes selection".to_string()
                    }
                    Tool::Sketch => "Sketch tool — click a face".to_string(),
                    Tool::Rectangle if self.sketch_session.is_some() => {
                        "Rectangle tool".to_string()
                    }
                    Tool::Rectangle => "Rectangle tool — click a face".to_string(),
                    Tool::Line if self.sketch_session.is_some() => "Line tool".to_string(),
                    Tool::Line => "Line tool — click a face".to_string(),
                    Tool::Circle if self.sketch_session.is_some() => "Circle tool".to_string(),
                    Tool::Circle => "Circle tool — click a face".to_string(),
                    Tool::Dimension if self.sketch_session.is_some() => {
                        "Dimension tool — select geometry, then D, or click a segment".to_string()
                    }
                    Tool::Dimension => "Dimension tool — open a sketch first".to_string(),
                    Tool::Constraint if self.sketch_session.is_some() => {
                        "Constraint tool — select geometry, then pick a constraint".to_string()
                    }
                    Tool::Constraint => "Constraint tool — open a sketch first".to_string(),
                    Tool::ConstructionPlane => "Construction plane tool".to_string(),
                    Tool::Extrude => {
                        "Extrude tool — click coplanar faces, then set a distance".to_string()
                    }
                };
                if tool == Tool::Dimension {
                    self.try_begin_dimension_from_selection();
                }
                ActionResult::Ok
            }
            Action::CancelOperation => {
                self.line_start_snap = None;
                self.line_end_snap = None;
                self.rect_origin_snap = None;
                self.rect_opposite_snap = None;
                self.circle_center_snap = None;
                if self.editing_committed_dim.take().is_some() {
                    self.status = "Cancelled".to_string();
                } else if self.creating_extrusion.take().is_some() {
                    self.status = "Cancelled extrusion".to_string();
                } else if self.creating_rect.take().is_some()
                    || self.creating_line.take().is_some()
                    || self.creating_circle.take().is_some()
                    || self.creating_plane.take().is_some()
                {
                    self.status = "Cancelled".to_string();
                } else if self.sketch_session.is_some() {
                    if self.tool == Tool::Select {
                        self.exit_sketch_session();
                        self.status = "Exited sketch".to_string();
                    } else {
                        self.creating_rect = None;
                        self.creating_line = None;
                        self.creating_circle = None;
                        self.tool = Tool::Select;
                        self.status =
                            "Select tool — Delete/Backspace removes selection".to_string();
                    }
                } else if self.tool != Tool::Select {
                    self.tool = Tool::Select;
                    self.status =
                        "Select tool — Delete/Backspace removes selection".to_string();
                }
                ActionResult::Ok
            }
            Action::BeginSketch { face, viewport } => {
                if sketch_frame(&self.doc, face).is_none() {
                    return ActionResult::Err(format!("Unknown face {:?}", face));
                }
                let sketch = self.doc.add_sketch(face);
                self.enter_sketch(sketch, viewport, None)
            }
            Action::OpenSketch { sketch, viewport } => {
                if self.doc.sketches.get(sketch).is_none() {
                    return ActionResult::Err(format!("Unknown sketch {sketch}"));
                }
                self.enter_sketch(sketch, viewport, Some(SKETCH_EDIT_FRAME_PADDING_PX))
            }
            Action::ExitSketch => {
                if self.sketch_session.is_none() {
                    return ActionResult::Err("Not in sketch mode".to_string());
                }
                self.exit_sketch_session();
                self.status = "Sketch saved".to_string();
                ActionResult::Ok
            }
            Action::CommitRectangle => {
                let Some(session) = self.sketch_session else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                let Some(frame) = sketch_geometry_frame(&self.doc, session.sketch) else {
                    return ActionResult::Err("Sketch no longer exists".to_string());
                };
                let Some(mut cr) = self.creating_rect.take() else {
                    return ActionResult::Err("No rectangle in progress".to_string());
                };
                for i in 0..2 {
                    if cr.user_edited[i] {
                        if let Err(e) =
                            try_commit_inline_parameter_definition(&mut self.doc, &mut cr.texts[i])
                        {
                            self.creating_rect = Some(cr);
                            self.status = e.clone();
                            return ActionResult::Err(e);
                        }
                    }
                }
                let (ou, ov) = world_to_local(&frame, cr.origin);
                let end = cr.end_point(&frame, &self.doc);
                let (eu, ev) = world_to_local(&frame, end);
                let mut rect = Rect::from_local_corners(session.sketch, ou, ov, eu, ev);
                if cr.construction {
                    for edge_index in 0..4 {
                        rect.set_edge_construction(RectEdge::from_index(edge_index), true);
                    }
                }
                if rect.w > 0.5 && rect.h > 0.5 {
                    self.doc.rects.push(rect);
                    self.doc.shape_order.push(ShapeKind::Rect);
                    let rect_index = self.doc.rects.len() - 1;
                    // Map each snapped placement to its corner before width/height solving shifts
                    // positions (corner indices are stable labels regardless).
                    let origin_corner =
                        rect_corner_index_at(&self.doc.rects[rect_index], ou, ov);
                    let opposite_corner =
                        rect_corner_index_at(&self.doc.rects[rect_index], eu, ev);
                    let mut constraint_err = None;
                    if cr.user_edited[0] {
                        if let Err(e) = add_distance_constraint(
                            &mut self.doc,
                            session.sketch,
                            DistanceTarget::RectWidth(rect_index),
                            cr.texts[0].clone(),
                        ) {
                            constraint_err = Some(e);
                        }
                    }
                    if constraint_err.is_none() && cr.user_edited[1] {
                        if let Err(e) = add_distance_constraint(
                            &mut self.doc,
                            session.sketch,
                            DistanceTarget::RectHeight(rect_index),
                            cr.texts[1].clone(),
                        ) {
                            constraint_err = Some(e);
                        }
                    }
                    if let Some(e) = constraint_err {
                        while self.doc.shape_order.last() == Some(&ShapeKind::Constraint) {
                            self.doc.shape_order.pop();
                            self.doc.constraints.pop();
                        }
                        self.doc.rects.pop();
                        self.doc.shape_order.pop();
                        self.rect_origin_snap = None;
                        self.rect_opposite_snap = None;
                        self.creating_rect = Some(cr);
                        self.status = e.clone();
                        return ActionResult::Err(e);
                    }
                    // Pin corners that were left on a snap target.
                    if let Some(target) = self.rect_origin_snap.take() {
                        let _ = self.add_snap_constraint(
                            session.sketch,
                            ConstraintPoint::RectCorner {
                                rect: rect_index,
                                corner: origin_corner,
                            },
                            target,
                        );
                    }
                    if let Some(target) = self.rect_opposite_snap.take() {
                        let _ = self.add_snap_constraint(
                            session.sketch,
                            ConstraintPoint::RectCorner {
                                rect: rect_index,
                                corner: opposite_corner,
                            },
                            target,
                        );
                    }
                    let rect = self.doc.rects.last().unwrap();
                    let (w, h) = (rect.w, rect.h);
                    self.status = format!("Added rectangle ({w:.1} × {h:.1} mm)");
                    ActionResult::Ok
                } else {
                    self.rect_origin_snap = None;
                    self.rect_opposite_snap = None;
                    self.creating_rect = Some(cr);
                    self.status = "Rectangle too small".to_string();
                    ActionResult::Err("Rectangle too small".to_string())
                }
            }
            Action::SetRectDimension { axis, value } => {
                if let Some(edit) = &mut self.editing_committed_dim {
                    let matches = match &edit.target {
                        DimEditTarget::Constraint(id) => {
                            constraint_matches_rect_axis(&self.doc, *id, axis)
                        }
                        DimEditTarget::New(DimensionTarget::Distance(target)) => matches!(
                            (target, axis),
                            (DistanceTarget::RectWidth(_), RectAxis::Width)
                                | (DistanceTarget::RectHeight(_), RectAxis::Height)
                        ),
                        DimEditTarget::New(_) => false,
                    };
                    if matches {
                        edit.text = value;
                        return ActionResult::Ok;
                    }
                }
                let Some(cr) = &mut self.creating_rect else {
                    return ActionResult::Err("No rectangle in progress".to_string());
                };
                let idx = axis.index();
                cr.texts[idx] = value;
                cr.user_edited[idx] = true;
                ActionResult::Ok
            }
            Action::FocusRectDimension { axis } => {
                if let Some(edit) = &mut self.editing_committed_dim {
                    let matches = match &edit.target {
                        DimEditTarget::Constraint(id) => {
                            constraint_matches_rect_axis(&self.doc, *id, axis)
                        }
                        DimEditTarget::New(DimensionTarget::Distance(target)) => matches!(
                            (target, axis),
                            (DistanceTarget::RectWidth(_), RectAxis::Width)
                                | (DistanceTarget::RectHeight(_), RectAxis::Height)
                        ),
                        DimEditTarget::New(_) => false,
                    };
                    if matches {
                        edit.pending_focus = true;
                        return ActionResult::Ok;
                    }
                }
                let Some(cr) = &mut self.creating_rect else {
                    return ActionResult::Err("No rectangle in progress".to_string());
                };
                cr.focused = axis.index();
                cr.pending_focus = true;
                ActionResult::Ok
            }
            Action::CommitLine => {
                let Some(session) = self.sketch_session else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                let Some(frame) = sketch_geometry_frame(&self.doc, session.sketch) else {
                    return ActionResult::Err("Sketch no longer exists".to_string());
                };
                let Some(mut cl) = self.creating_line.take() else {
                    return ActionResult::Err("No line in progress".to_string());
                };
                if cl.user_edited {
                    if let Err(e) =
                        try_commit_inline_parameter_definition(&mut self.doc, &mut cl.text)
                    {
                        self.creating_line = Some(cl);
                        self.status = e.clone();
                        return ActionResult::Err(e);
                    }
                }
                let (u0, v0) = world_to_local(&frame, cl.origin);
                let end = cl.end_point(&frame, &self.doc);
                let (u1, v1) = world_to_local(&frame, end);
                let mut line = Line::from_local_endpoints(session.sketch, u0, v0, u1, v1);
                line.construction = cl.construction;
                if line.length() > 0.5 {
                    self.doc.lines.push(line);
                    self.doc.shape_order.push(ShapeKind::Line);
                    let line_index = self.doc.lines.len() - 1;
                    if cl.user_edited {
                        if let Err(e) = add_distance_constraint(
                            &mut self.doc,
                            session.sketch,
                            DistanceTarget::LineLength(line_index),
                            cl.text.clone(),
                        ) {
                            self.doc.lines.pop();
                            self.doc.shape_order.pop();
                            self.creating_line = Some(cl);
                            self.status = e.clone();
                            return ActionResult::Err(e);
                        }
                    }
                    // If the segment's end latched onto an existing vertex (or the origin),
                    // the polyline is closing/joining, so we stop chaining (#20).
                    let end_on_vertex = matches!(
                        self.line_end_snap,
                        Some(crate::snapping::SnapTarget::Vertex(_))
                            | Some(crate::snapping::SnapTarget::Origin)
                    );
                    // Pin endpoints that were left on a snap target.
                    if let Some(target) = self.line_start_snap.take() {
                        let _ = self.add_snap_constraint(
                            session.sketch,
                            ConstraintPoint::LineEndpoint {
                                line: line_index,
                                end: LineEnd::Start,
                            },
                            target,
                        );
                    }
                    if let Some(target) = self.line_end_snap.take() {
                        let _ = self.add_snap_constraint(
                            session.sketch,
                            ConstraintPoint::LineEndpoint {
                                line: line_index,
                                end: LineEnd::End,
                            },
                            target,
                        );
                    }
                    let len = self.doc.lines.last().unwrap().length();
                    // Chain into the next segment: start a new line at this endpoint so polygons
                    // can be drawn with successive clicks. The new start snaps to the just-placed
                    // endpoint (coincident on commit), keeping the polyline connected. Skip this
                    // when we closed onto an existing vertex (#20).
                    if self.tool == Tool::Line && !end_on_vertex {
                        self.line_start_snap = Some(crate::snapping::SnapTarget::Vertex(
                            ConstraintPoint::LineEndpoint {
                                line: line_index,
                                end: LineEnd::End,
                            },
                        ));
                        self.line_end_snap = None;
                        self.creating_line = Some(CreatingLine {
                            origin: end,
                            text: String::new(),
                            last_mouse: end,
                            user_edited: false,
                            pending_focus: true,
                            construction: cl.construction,
                        });
                        self.status =
                            format!("Added line ({:.1} mm) • click for next point • Esc to finish", len);
                    } else {
                        self.status = format!("Added line ({:.1} mm)", len);
                    }
                    ActionResult::Ok
                } else {
                    self.creating_line = Some(cl);
                    self.line_start_snap = None;
                    self.line_end_snap = None;
                    self.status = "Line too short".to_string();
                    ActionResult::Err("Line too short".to_string())
                }
            }
            Action::SetLineLength { value } => {
                if let Some(edit) = &mut self.editing_committed_dim {
                    let matches = match &edit.target {
                        DimEditTarget::Constraint(id) => constraint_is_line_length(&self.doc, *id),
                        DimEditTarget::New(DimensionTarget::Distance(DistanceTarget::LineLength(_))) => {
                            true
                        }
                        DimEditTarget::New(_) => false,
                    };
                    if matches {
                        edit.text = value;
                        return ActionResult::Ok;
                    }
                }
                let Some(cl) = &mut self.creating_line else {
                    return ActionResult::Err("No line in progress".to_string());
                };
                cl.text = value;
                cl.user_edited = true;
                ActionResult::Ok
            }
            Action::SetDimLabelOffset { target, offset } => {
                if let Err(e) = require_constraint_editable(&self.document_health, &self.doc, target)
                {
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                let offset = if constraint_is_circle_diameter(&self.doc, target) {
                    crate::dimensions::effective_circle_diameter_label_offset(Some(offset))
                } else if constraint_is_angle(&self.doc, target) {
                    crate::dimensions::effective_arc_dim_offset(Some(offset))
                } else {
                    crate::dimensions::effective_dim_offset(Some(offset))
                };
                match set_constraint_dim_offset(&mut self.doc, target, offset) {
                    Ok(()) => ActionResult::Ok,
                    Err(e) => {
                        self.status = e.clone();
                        ActionResult::Err(e)
                    }
                }
            }
            Action::SetConstraintAngleValue {
                constraint_id,
                angle_rad,
            } => {
                if let Err(e) =
                    require_constraint_editable(&self.document_health, &self.doc, constraint_id)
                {
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                match crate::constraints::set_constraint_angle_value(
                    &mut self.doc,
                    constraint_id,
                    angle_rad,
                ) {
                    Ok(()) => ActionResult::Ok,
                    Err(e) => {
                        self.status = e.clone();
                        ActionResult::Err(e)
                    }
                }
            }
            Action::FocusLineLength => {
                if let Some(edit) = &mut self.editing_committed_dim {
                    let matches = match &edit.target {
                        DimEditTarget::Constraint(id) => constraint_is_line_length(&self.doc, *id),
                        DimEditTarget::New(DimensionTarget::Distance(DistanceTarget::LineLength(_))) => {
                            true
                        }
                        DimEditTarget::New(_) => false,
                    };
                    if matches {
                        edit.pending_focus = true;
                        return ActionResult::Ok;
                    }
                }
                let Some(cl) = &mut self.creating_line else {
                    return ActionResult::Err("No line in progress".to_string());
                };
                cl.pending_focus = true;
                ActionResult::Ok
            }
            Action::CommitCircle => {
                let Some(session) = self.sketch_session else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                let Some(frame) = sketch_geometry_frame(&self.doc, session.sketch) else {
                    return ActionResult::Err("Sketch no longer exists".to_string());
                };
                let Some(mut cc) = self.creating_circle.take() else {
                    return ActionResult::Err("No circle in progress".to_string());
                };
                if cc.user_edited {
                    if let Err(e) =
                        try_commit_inline_parameter_definition(&mut self.doc, &mut cc.text)
                    {
                        self.creating_circle = Some(cc);
                        self.status = e.clone();
                        return ActionResult::Err(e);
                    }
                }
                let (cu, cv) = world_to_local(&frame, cc.origin);
                let r = cc.radius(&frame, &self.doc);
                let angle = cc.diameter_dim_angle(&frame);
                let mut circle =
                    Circle::from_local_center_radius(session.sketch, cu, cv, r, angle);
                circle.construction = cc.construction;
                if circle.r > 0.25 {
                    self.doc.circles.push(circle);
                    self.doc.shape_order.push(ShapeKind::Circle);
                    let circle_index = self.doc.circles.len() - 1;
                    if cc.user_edited {
                        if let Err(e) = add_distance_constraint(
                            &mut self.doc,
                            session.sketch,
                            DistanceTarget::CircleDiameter(circle_index),
                            cc.text.clone(),
                        ) {
                            self.doc.circles.pop();
                            self.doc.shape_order.pop();
                            self.circle_center_snap = None;
                            self.creating_circle = Some(cc);
                            self.status = e.clone();
                            return ActionResult::Err(e);
                        }
                    }
                    // Pin the center if it was left on a snap target.
                    if let Some(target) = self.circle_center_snap.take() {
                        let _ = self.add_snap_constraint(
                            session.sketch,
                            ConstraintPoint::CircleCenter(circle_index),
                            target,
                        );
                    }
                    let diameter = self.doc.circles.last().unwrap().diameter();
                    self.status = format!("Added circle (Ø{diameter:.1} mm)");
                    ActionResult::Ok
                } else {
                    self.circle_center_snap = None;
                    self.creating_circle = Some(cc);
                    self.status = "Circle too small".to_string();
                    ActionResult::Err("Circle too small".to_string())
                }
            }
            Action::SetCircleDiameter { value } => {
                if let Some(edit) = &mut self.editing_committed_dim {
                    let matches = match &edit.target {
                        DimEditTarget::Constraint(id) => {
                            constraint_is_circle_diameter(&self.doc, *id)
                        }
                        DimEditTarget::New(DimensionTarget::Distance(
                            DistanceTarget::CircleDiameter(_),
                        )) => true,
                        DimEditTarget::New(_) => false,
                    };
                    if matches {
                        edit.text = value;
                        return ActionResult::Ok;
                    }
                }
                let Some(cc) = &mut self.creating_circle else {
                    return ActionResult::Err("No circle in progress".to_string());
                };
                cc.text = value;
                cc.user_edited = true;
                ActionResult::Ok
            }
            Action::FocusCircleDiameter => {
                if let Some(edit) = &mut self.editing_committed_dim {
                    let matches = match &edit.target {
                        DimEditTarget::Constraint(id) => {
                            constraint_is_circle_diameter(&self.doc, *id)
                        }
                        DimEditTarget::New(DimensionTarget::Distance(
                            DistanceTarget::CircleDiameter(_),
                        )) => true,
                        DimEditTarget::New(_) => false,
                    };
                    if matches {
                        edit.pending_focus = true;
                        return ActionResult::Ok;
                    }
                }
                let Some(cc) = &mut self.creating_circle else {
                    return ActionResult::Err("No circle in progress".to_string());
                };
                cc.pending_focus = true;
                ActionResult::Ok
            }
            Action::BeginEditCommittedDim { target } => {
                if !self.can_edit_sketch_dimensions() {
                    return ActionResult::Err(
                        "Open a sketch and finish the current draw operation to edit dimensions"
                            .to_string(),
                    );
                }
                if let Err(e) = require_constraint_editable(&self.document_health, &self.doc, target)
                {
                    return ActionResult::Err(e);
                }
                let Some(text) = committed_dim_expression(&self.doc, target) else {
                    return ActionResult::Err("Dimension is not editable".to_string());
                };
                self.editing_committed_dim = Some(EditingCommittedDim {
                    target: DimEditTarget::Constraint(target),
                    text,
                    pending_focus: true,
                });
                self.status = "Edit dimension • Enter to commit • Esc to cancel".to_string();
                ActionResult::Ok
            }
            Action::BeginDimensionEdit { target } => {
                let Some(_session) = self.sketch_session else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                if self.tool != Tool::Dimension {
                    return ActionResult::Err("Dimension tool required".to_string());
                }
                self.start_committed_dimension_edit(target);
                ActionResult::Ok
            }
            Action::CommitCommittedDim => {
                let Some(session) = self.sketch_session else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                let Some(edit) = self.editing_committed_dim.take() else {
                    return ActionResult::Err("No committed dimension being edited".to_string());
                };
                let frozen = match &edit.target {
                    DimEditTarget::Constraint(id) => {
                        require_constraint_editable(&self.document_health, &self.doc, *id)
                    }
                    DimEditTarget::New(target) => {
                        require_dimension_target_editable(&self.document_health, &self.doc, *target)
                    }
                };
                if let Err(e) = frozen {
                    self.editing_committed_dim = Some(edit);
                    return ActionResult::Err(e);
                }
                let target = edit.target.clone();
                let mut text = edit.text.clone();
                if let Err(e) = try_commit_inline_parameter_definition(&mut self.doc, &mut text) {
                    self.editing_committed_dim = Some(edit);
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                match apply_committed_dim_expression(
                    &mut self.doc,
                    session.sketch,
                    target,
                    &text,
                ) {
                    Ok(()) => {
                        self.refresh_document_health();
                        self.status = "Updated dimension".to_string();
                        ActionResult::Ok
                    }
                    Err(e) => {
                        self.editing_committed_dim = Some(edit);
                        self.status = e.clone();
                        ActionResult::Err(e)
                    }
                }
            }
            Action::BeginConstructionPlane { reference, parent } => {
                self.creating_plane = Some(CreatingConstructionPlane {
                    edit_index: None,
                    reference,
                    parent,
                    offset_text: String::new(),
                    angle_text: String::new(),
                    focused: PlaneDim::Offset,
                    offset_live: 0.0,
                    axis_angle_deg: 0.0,
                    user_edited_offset: false,
                    user_edited_angle: false,
                    pending_focus: true,
                    axis_gizmo_drag: None,
                });
                self.tool = Tool::ConstructionPlane;
                self.status = "Set offset • type to lock • Tab cycle dims • click/Enter commit • Esc cancel"
                    .to_string();
                ActionResult::Ok
            }
            Action::BeginEditConstructionPlane { index } => {
                let Some(plane) = self.doc.construction_planes.get(index) else {
                    return ActionResult::Err(format!("Unknown construction plane {index}"));
                };
                let reference = reference_from_definition(&plane.definition);
                let (offset_live, axis_angle_deg) = match &reference {
                    PlaneReference::Face { .. } => (plane.definition.offset_mm, 0.0),
                    PlaneReference::Axis { .. } => {
                        (plane.definition.offset_mm, plane.definition.angle_deg)
                    }
                };
                self.creating_plane = Some(CreatingConstructionPlane {
                    edit_index: Some(index),
                    reference,
                    parent: plane.parent,
                    offset_text: format!("{offset_live:.1}"),
                    angle_text: format!("{axis_angle_deg:.0}"),
                    focused: PlaneDim::Offset,
                    offset_live,
                    axis_angle_deg,
                    user_edited_offset: false,
                    user_edited_angle: false,
                    pending_focus: true,
                    axis_gizmo_drag: None,
                });
                self.tool = Tool::ConstructionPlane;
                self.status = format!(
                    "Edit construction plane {index} • type to lock offset{} • Tab cycle dims • click/Enter commit • Esc cancel",
                    if plane.definition.is_axis() { "/angle" } else { "" }
                );
                ActionResult::Ok
            }
            Action::CommitConstructionPlane => {
                let Some(mut cp) = self.creating_plane.take() else {
                    return ActionResult::Err("No construction plane in progress".to_string());
                };
                if cp.user_edited_offset {
                    if let Err(e) =
                        try_commit_inline_parameter_definition(&mut self.doc, &mut cp.offset_text)
                    {
                        self.creating_plane = Some(cp);
                        self.status = e.clone();
                        return ActionResult::Err(e);
                    }
                }
                if cp.user_edited_angle {
                    if let Err(e) =
                        try_commit_inline_parameter_definition(&mut self.doc, &mut cp.angle_text)
                    {
                        self.creating_plane = Some(cp);
                        self.status = e.clone();
                        return ActionResult::Err(e);
                    }
                }
                let definition = cp.resolved_definition();
                let live_offset = definition.offset_mm;
                if let Some(index) = cp.edit_index {
                    match apply_construction_plane_edit(
                        &mut self.doc,
                        index,
                        &definition,
                        cp.parent,
                    ) {
                        Ok(()) => {
                            self.status = format!(
                                "Updated construction plane {index} ({live_offset:.1} mm from {})",
                                cp.reference.label()
                            );
                            ActionResult::Ok
                        }
                        Err(message) => {
                            self.creating_plane = Some(cp);
                            self.status = message.clone();
                            ActionResult::Err(message)
                        }
                    }
                } else {
                    let plane = plane_from_definition(&definition, cp.parent);
                    self.doc.construction_planes.push(plane);
                    self.doc.shape_order.push(ShapeKind::ConstructionPlane);
                    let index = self.doc.construction_planes.len() - 1;
                    self.scene_selection.clear();
                    click_scene_selection(
                        &mut self.scene_selection,
                        SceneElement::ConstructionPlane(index),
                        false,
                    );
                    self.status = format!(
                        "Added construction plane ({live_offset:.1} mm from {})",
                        cp.reference.label()
                    );
                    ActionResult::Ok
                }
            }
            Action::SetPlaneOffset { value } => {
                let Some(cp) = &mut self.creating_plane else {
                    return ActionResult::Err("No construction plane in progress".to_string());
                };
                cp.offset_text = value.clone();
                cp.user_edited_offset = true;
                if let Some(v) = crate::value::eval_length_mm(&value) {
                    cp.offset_live = v;
                }
                ActionResult::Ok
            }
            Action::SetPlaneAngle { value } => {
                let Some(cp) = &mut self.creating_plane else {
                    return ActionResult::Err("No construction plane in progress".to_string());
                };
                cp.angle_text = value.clone();
                cp.user_edited_angle = true;
                if let Ok(v) = value.trim().parse::<f32>() {
                    cp.axis_angle_deg = v.rem_euclid(360.0);
                }
                ActionResult::Ok
            }
            Action::FocusPlaneDim { dim } => {
                let Some(cp) = &mut self.creating_plane else {
                    return ActionResult::Err("No construction plane in progress".to_string());
                };
                cp.focused = dim;
                cp.pending_focus = true;
                ActionResult::Ok
            }
            Action::OrbitCamera { delta } => {
                self.cam.orbit(egui::vec2(delta.0, delta.1));
                ActionResult::Ok
            }
            Action::PanCamera {
                delta,
                viewport_height,
            } => {
                self.cam.pan(egui::vec2(delta.0, delta.1), viewport_height);
                ActionResult::Ok
            }
            Action::ZoomCamera {
                scroll,
                focal,
                viewport,
            } => {
                self.cam.zoom(scroll, focal, viewport);
                ActionResult::Ok
            }
            Action::SetStandardView(view) => {
                self.cam.start_view_transition(view, VIEW_TRANSITION_DURATION);
                self.status = format!("View: {:?}", view);
                ActionResult::Ok
            }
            Action::SetViewEdge(edge) => {
                self.cam.start_view_transition_to_direction(
                    view_cube::edge_view_direction(edge),
                    VIEW_TRANSITION_DURATION,
                );
                self.status = format!("View edge: {:?}", edge);
                ActionResult::Ok
            }
            Action::SetViewCorner(corner) => {
                self.cam.start_view_transition_to_direction(
                    view_cube::corner_view_direction(corner),
                    VIEW_TRANSITION_DURATION,
                );
                self.status = format!("View corner: {:?}", corner);
                ActionResult::Ok
            }
            Action::ViewHome => {
                self.cam.start_home_transition(VIEW_TRANSITION_DURATION);
                self.status = "View: home".to_string();
                ActionResult::Ok
            }
            Action::SetHomeView => {
                self.cam.set_home_from_current();
                self.status = "Home view set".to_string();
                ActionResult::Ok
            }
            Action::SetProjectionMode(mode) => {
                self.cam.set_projection_mode(mode);
                self.status = format!("Projection: {:?}", mode);
                ActionResult::Ok
            }
            Action::ToggleProjectionMode => {
                self.cam.toggle_projection_mode();
                self.status = format!("Projection: {:?}", self.cam.projection_mode());
                ActionResult::Ok
            }
            Action::AddParameter { name, expression } => {
                match add_parameter(&mut self.doc, name.clone(), expression.clone()) {
                    Ok(_) => {
                        self.status = format!("Added parameter {name}");
                        ActionResult::Ok
                    }
                    Err(e) => {
                        self.status = e.clone();
                        ActionResult::Err(e)
                    }
                }
            }
            Action::CreateParameterFromLineLength { line_index, name } => {
                match add_computed_parameter_from_line_length(&mut self.doc, line_index, name.clone())
                {
                    Ok(index) => {
                        let param_name = self.doc.parameters[index].name.clone();
                        self.refresh_document_health();
                        self.status = format!("Added parameter {param_name} from line length");
                        ActionResult::Ok
                    }
                    Err(e) => {
                        self.status = e.clone();
                        ActionResult::Err(e)
                    }
                }
            }
            Action::CommitParameterName { index, name } => {
                if let Err(e) = require_parameter_editable(&self.document_health, index) {
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                match set_parameter_name(&mut self.doc, index, name.clone()) {
                    Ok(()) => {
                        self.refresh_document_health();
                        self.status = format!("Renamed parameter to {name}");
                        ActionResult::Ok
                    }
                    Err(e) => {
                        self.status = e.clone();
                        ActionResult::Err(e)
                    }
                }
            }
            Action::CommitParameterExpression { index, expression } => {
                if let Err(e) = require_parameter_editable(&self.document_health, index) {
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                if let Some(param) = self.doc.parameters.get(index) {
                    if let Err(e) = require_parameter_value_editable(param) {
                        self.status = e.clone();
                        return ActionResult::Err(e);
                    }
                }
                match set_parameter_expression(&mut self.doc, index, expression.clone()) {
                    Ok(()) => {
                        let _ = recompute_document_geometry(&mut self.doc);
                        self.refresh_document_health();
                        self.status = "Updated parameter".to_string();
                        ActionResult::Ok
                    }
                    Err(e) => {
                        self.status = e.clone();
                        ActionResult::Err(e)
                    }
                }
            }
            Action::DeleteParameter { index } => {
                match delete_parameter(&mut self.doc, index) {
                    Ok(()) => {
                        let _ = recompute_document_geometry(&mut self.doc);
                        self.refresh_document_health();
                        self.status = "Deleted parameter".to_string();
                        ActionResult::Ok
                    }
                    Err(e) => {
                        self.status = e.clone();
                        ActionResult::Err(e)
                    }
                }
            }
            Action::DeleteSelection => {
                if self.scene_selection.is_empty() {
                    self.status = "Nothing selected".to_string();
                    return ActionResult::Ok;
                }
                let targets = delete_targets_from_selection(&self.scene_selection);
                let count = tombstone_elements(&mut self.doc, &targets);
                if let Some(session) = self.sketch_session {
                    if !crate::document_lifecycle::sketch_alive(&self.doc, session.sketch) {
                        self.exit_sketch_session();
                    }
                }
                self.scene_selection.clear();
                let _ = recompute_document_geometry(&mut self.doc);
                self.refresh_document_health();
                let mut status = format!("Deleted {count} element(s)");
                let invalid = self
                    .document_health
                    .elements
                    .values()
                    .filter(|s| **s == crate::document_health::HealthStatus::Invalid)
                    .count()
                    + self
                        .document_health
                        .parameters
                        .values()
                        .filter(|s| **s == crate::document_health::HealthStatus::Invalid)
                        .count();
                let unstable = self
                    .document_health
                    .elements
                    .values()
                    .filter(|s| **s == crate::document_health::HealthStatus::Unstable)
                    .count();
                if invalid > 0 || unstable > 0 {
                    status.push_str(&format!(" — {invalid} invalid, {unstable} unstable"));
                }
                self.status = status;
                ActionResult::Ok
            }
            Action::SetCommandPaletteOpen { open } => {
                if open {
                    self.command_palette.open_palette();
                    self.status = "Command palette".to_string();
                } else {
                    self.command_palette.close_palette();
                }
                ActionResult::Ok
            }
            Action::ToggleCommandPalette => {
                if self.command_palette.open {
                    self.command_palette.close_palette();
                } else {
                    self.command_palette.open_palette();
                    self.status = "Command palette".to_string();
                }
                ActionResult::Ok
            }
            Action::SetPaneVisible { pane, visible } => {
                self.panes.set(pane, visible);
                self.status = pane_status(pane, visible);
                ActionResult::Ok
            }
            Action::TogglePane(pane) => {
                self.panes.toggle(pane);
                self.status = pane_status(pane, self.panes.is_visible(pane));
                ActionResult::Ok
            }
            Action::DragVertex { point, u, v } => {
                let Some(sketch) = self.sketch_session.map(|s| s.sketch) else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                let element = vertex_drag::scene_element_for_point(point);
                if let Err(e) = require_element_editable(&self.document_health, element) {
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                if !vertex_drag::can_drag_point(&self.doc, sketch, point) {
                    return ActionResult::Err("Vertex is fully constrained".to_string());
                }
                match vertex_drag::drag_point(&mut self.doc, sketch, point, u, v) {
                    Ok(()) => ActionResult::Ok,
                    Err(e) => ActionResult::Err(e),
                }
            }
            Action::BeginLineDrag {
                target,
                anchor_u,
                anchor_v,
            } => {
                let Some(sketch) = self.sketch_session.map(|s| s.sketch) else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                let element = vertex_drag::scene_element_for_line(target);
                if let Err(e) = require_element_editable(&self.document_health, element) {
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                if !vertex_drag::can_drag_line(&self.doc, sketch, target) {
                    return ActionResult::Err("Line is fully constrained".to_string());
                }
                match vertex_drag::begin_line_drag_session(
                    &self.doc,
                    sketch,
                    target,
                    (anchor_u, anchor_v),
                ) {
                    Ok(session) => {
                        self.line_drag_session = Some(session);
                        ActionResult::Ok
                    }
                    Err(e) => ActionResult::Err(e),
                }
            }
            Action::DragLine { u, v } => {
                let Some(sketch) = self.sketch_session.map(|s| s.sketch) else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                let Some(session) = self.line_drag_session.clone() else {
                    return ActionResult::Err("No line drag in progress".to_string());
                };
                let element = vertex_drag::scene_element_for_line(session.target);
                if let Err(e) = require_element_editable(&self.document_health, element) {
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                match vertex_drag::drag_line(&mut self.doc, sketch, &session, (u, v)) {
                    Ok(()) => ActionResult::Ok,
                    Err(e) => ActionResult::Err(e),
                }
            }
            Action::EndLineDrag => {
                self.line_drag_session = None;
                ActionResult::Ok
            }
            Action::CreateRectangle {
                x,
                y,
                width,
                height,
            } => {
                let Some(session) = self.sketch_session else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                if width <= 0.0 || height <= 0.0 {
                    return ActionResult::Err(
                        "Rectangle needs positive width and height".to_string(),
                    );
                }
                let rect =
                    Rect::from_local_corners(session.sketch, x, y, x + width, y + height);
                self.doc.rects.push(rect);
                self.doc.shape_order.push(ShapeKind::Rect);
                let rect_index = self.doc.rects.len() - 1;
                let mut add_dim = |target, value: f32| {
                    add_distance_constraint(&mut self.doc, session.sketch, target, value.to_string())
                };
                if let Err(e) = add_dim(DistanceTarget::RectWidth(rect_index), width)
                    .and_then(|_| add_dim(DistanceTarget::RectHeight(rect_index), height))
                {
                    while self.doc.shape_order.last() == Some(&ShapeKind::Constraint) {
                        self.doc.shape_order.pop();
                        self.doc.constraints.pop();
                    }
                    self.doc.rects.pop();
                    self.doc.shape_order.pop();
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                self.refresh_document_health();
                self.status = format!("Added rectangle ({width:.1} × {height:.1} mm)");
                ActionResult::Ok
            }
            Action::CreateCircle { cx, cy, r } => {
                let Some(session) = self.sketch_session else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                if r <= 0.0 {
                    return ActionResult::Err("Circle needs a positive radius".to_string());
                }
                let circle = Circle::from_local_center_radius(session.sketch, cx, cy, r, 0.0);
                self.doc.circles.push(circle);
                self.doc.shape_order.push(ShapeKind::Circle);
                let circle_index = self.doc.circles.len() - 1;
                if let Err(e) = add_distance_constraint(
                    &mut self.doc,
                    session.sketch,
                    DistanceTarget::CircleDiameter(circle_index),
                    (r * 2.0).to_string(),
                ) {
                    while self.doc.shape_order.last() == Some(&ShapeKind::Constraint) {
                        self.doc.shape_order.pop();
                        self.doc.constraints.pop();
                    }
                    self.doc.circles.pop();
                    self.doc.shape_order.pop();
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                self.refresh_document_health();
                self.status = format!("Added circle (⌀{:.1} mm)", r * 2.0);
                ActionResult::Ok
            }
            Action::CreateLineSegment { x0, y0, x1, y1 } => {
                let Some(session) = self.sketch_session else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                let mut line = Line::from_local_endpoints(session.sketch, x0, y0, x1, y1);
                let length = line.length();
                if length <= 0.5 {
                    return ActionResult::Err("Line is too short".to_string());
                }
                line.construction = self.draw_construction;
                self.doc.lines.push(line);
                self.doc.shape_order.push(ShapeKind::Line);
                let line_index = self.doc.lines.len() - 1;
                if let Err(e) = add_distance_constraint(
                    &mut self.doc,
                    session.sketch,
                    DistanceTarget::LineLength(line_index),
                    length.to_string(),
                ) {
                    self.doc.lines.pop();
                    self.doc.shape_order.pop();
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                self.refresh_document_health();
                self.status = format!("Added line ({length:.1} mm)");
                ActionResult::Ok
            }
            Action::CreateExtrusion {
                sketch,
                faces,
                distance,
            } => {
                if faces.is_empty() {
                    return ActionResult::Err("Extrusion needs at least one face".to_string());
                }
                self.doc.extrusions.push(Extrusion {
                    sketch,
                    faces,
                    distance,
                    target: None,
                    expression: String::new(),
                    name: None,
                    deleted: false,
                });
                self.doc.shape_order.push(ShapeKind::Extrusion);
                let extrusion_index = self.doc.extrusions.len() - 1;
                // The extrusion produces a solid body that depends on it.
                self.doc.bodies.push(crate::model::Body {
                    source: crate::model::BodySource::Extrusion(extrusion_index),
                    name: None,
                    deleted: false,
                });
                self.doc.shape_order.push(ShapeKind::Body);
                self.refresh_document_health();
                self.status = format!("Added extrusion ({distance:.1} mm)");
                ActionResult::Ok
            }
            Action::ToggleExtrudeFace { face } => {
                let Some(sketch) = extrude_face_sketch(&self.doc, face) else {
                    return ActionResult::Err("Face not found".to_string());
                };
                match &mut self.creating_extrusion {
                    Some(ce) if ce.sketch == sketch => {
                        if let Some(pos) = ce.faces.iter().position(|f| *f == face) {
                            ce.faces.remove(pos);
                        } else {
                            ce.faces.push(face);
                        }
                    }
                    // A face on a different plane starts a fresh extrusion.
                    _ => {
                        self.creating_extrusion = Some(CreatingExtrusion {
                            sketch,
                            faces: vec![face],
                            distance: DEFAULT_EXTRUDE_DISTANCE,
                            text: crate::value::format_length_display(DEFAULT_EXTRUDE_DISTANCE),
                            user_edited: false,
                            pending_focus: true,
                            target: None,
                            edit_index: None,
                        });
                    }
                }
                ActionResult::Ok
            }
            Action::SetExtrudeDistance { distance } => {
                if let Some(ce) = &mut self.creating_extrusion {
                    ce.distance = distance;
                    if !ce.user_edited {
                        ce.text = crate::value::format_length_display(distance.abs());
                    }
                }
                ActionResult::Ok
            }
            Action::SetExtrudeTarget { target } => {
                if let Some(ce) = &mut self.creating_extrusion {
                    ce.target = target;
                    // Typing a distance again clears the object constraint.
                    if target.is_some() {
                        ce.user_edited = false;
                    }
                }
                ActionResult::Ok
            }
            Action::EditExtrusion { index } => {
                let Some(extrusion) = self.doc.extrusions.get(index) else {
                    return ActionResult::Err("Extrusion not found".to_string());
                };
                if extrusion.deleted {
                    return ActionResult::Err("Extrusion was deleted".to_string());
                }
                self.creating_extrusion = Some(CreatingExtrusion {
                    sketch: extrusion.sketch,
                    faces: extrusion.faces.clone(),
                    distance: extrusion.distance,
                    text: crate::value::format_length_display(extrusion.distance.abs()),
                    user_edited: false,
                    pending_focus: true,
                    target: extrusion.target,
                    edit_index: Some(index),
                });
                self.tool = Tool::Extrude;
                self.status = format!("Editing extrusion {index}");
                ActionResult::Ok
            }
            Action::CommitExtrusion => {
                let Some(ce) = self.creating_extrusion.take() else {
                    return ActionResult::Err("No extrusion in progress".to_string());
                };
                if ce.faces.is_empty() {
                    self.creating_extrusion = Some(ce);
                    return ActionResult::Err("Select at least one face".to_string());
                }
                let distance = ce.evaluated_distance(&self.doc);
                if distance.abs() < 1e-3 {
                    self.creating_extrusion = Some(ce);
                    return ActionResult::Err("Extrusion distance must be non-zero".to_string());
                }
                if let Some(idx) = ce.edit_index {
                    if let Some(extrusion) = self.doc.extrusions.get_mut(idx) {
                        extrusion.faces = ce.faces.clone();
                        extrusion.distance = distance;
                        extrusion.target = ce.target;
                    }
                    self.status = format!("Updated extrusion ({distance:.1} mm)");
                } else {
                    self.doc.extrusions.push(Extrusion {
                        sketch: ce.sketch,
                        faces: ce.faces.clone(),
                        distance,
                        target: ce.target,
                        expression: String::new(),
                        name: None,
                        deleted: false,
                    });
                    self.doc.shape_order.push(ShapeKind::Extrusion);
                    let ei = self.doc.extrusions.len() - 1;
                    self.doc.bodies.push(crate::model::Body {
                        source: crate::model::BodySource::Extrusion(ei),
                        name: None,
                        deleted: false,
                    });
                    self.doc.shape_order.push(ShapeKind::Body);
                    self.status = format!("Added extrusion ({distance:.1} mm)");
                }
                self.refresh_document_health();
                ActionResult::Ok
            }
            Action::SetSnapping(enabled) => {
                self.snapping_enabled = enabled;
                self.active_snap = None;
                self.status = if enabled {
                    "Snapping on".to_string()
                } else {
                    "Snapping off".to_string()
                };
                ActionResult::Ok
            }
            Action::ApplySnapConstraint { point, target } => {
                let Some(sketch) = self.sketch_session.map(|s| s.sketch) else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                match self.add_snap_constraint(sketch, point, target) {
                    Ok(()) => ActionResult::Ok,
                    Err(e) => ActionResult::Err(e),
                }
            }
            Action::ClickSceneElement { element, additive } => {
                click_scene_selection(&mut self.scene_selection, element, additive);
                if let Some((health_status, reason)) =
                    selection_frozen_summary(&self.document_health, &self.scene_selection)
                {
                    self.status = format!(
                        "{} — {}",
                        health_status_label(health_status),
                        reason
                    );
                }
                ActionResult::Ok
            }
            Action::ClearSceneSelection => {
                self.scene_selection.clear();
                ActionResult::Ok
            }
            Action::SetShapeConstruction {
                element,
                construction,
            } => {
                if let Err(e) = require_element_editable(&self.document_health, element) {
                    return ActionResult::Err(e);
                }
                match set_edge_construction(&mut self.doc, element, construction) {
                Ok(()) => {
                    self.status = format!(
                        "{} {}",
                        element_label(element),
                        if construction {
                            "marked construction"
                        } else {
                            "marked solid"
                        }
                    );
                    ActionResult::Ok
                }
                Err(e) => ActionResult::Err(e),
                }
            }
            Action::ApplyConstruction { construction } => {
                if let Some(cr) = &mut self.creating_rect {
                    cr.construction = construction;
                    self.draw_construction = construction;
                    self.status = draw_mode_status("Rectangle", construction);
                    return ActionResult::Ok;
                }
                if let Some(cl) = &mut self.creating_line {
                    cl.construction = construction;
                    self.draw_construction = construction;
                    self.status = draw_mode_status("Line", construction);
                    return ActionResult::Ok;
                }
                if let Some(cc) = &mut self.creating_circle {
                    cc.construction = construction;
                    self.draw_construction = construction;
                    self.status = draw_mode_status("Circle", construction);
                    return ActionResult::Ok;
                }
                if self.tool == Tool::Rectangle {
                    self.draw_construction = construction;
                    self.status = draw_mode_status("Rectangle", construction);
                    return ActionResult::Ok;
                }
                if self.tool == Tool::Line {
                    self.draw_construction = construction;
                    self.status = draw_mode_status("Line", construction);
                    return ActionResult::Ok;
                }
                if self.tool == Tool::Circle {
                    self.draw_construction = construction;
                    self.status = draw_mode_status("Circle", construction);
                    return ActionResult::Ok;
                }
                if let Err(e) =
                    require_construction_targets_editable(&self.document_health, &self.scene_selection)
                {
                    return ActionResult::Err(e);
                }
                let targets = construction_targets_from_selection(&self.scene_selection);
                match set_construction_for_targets(&mut self.doc, &targets, construction) {
                    Ok(count) if count > 0 => {
                        self.status = format!(
                            "{count} item(s) marked {}",
                            if construction {
                                "construction"
                            } else {
                                "substantial"
                            }
                        );
                        ActionResult::Ok
                    }
                    Ok(_) => ActionResult::Err("No constructable geometry selected".to_string()),
                    Err(e) => ActionResult::Err(e),
                }
            }
            Action::ToggleConstruction => {
                if let Some(cr) = &mut self.creating_rect {
                    cr.construction = !cr.construction;
                    self.draw_construction = cr.construction;
                    self.status = draw_mode_status("Rectangle", cr.construction);
                    return ActionResult::Ok;
                }
                if let Some(cl) = &mut self.creating_line {
                    cl.construction = !cl.construction;
                    self.draw_construction = cl.construction;
                    self.status = draw_mode_status("Line", cl.construction);
                    return ActionResult::Ok;
                }
                if let Some(cc) = &mut self.creating_circle {
                    cc.construction = !cc.construction;
                    self.draw_construction = cc.construction;
                    self.status = draw_mode_status("Circle", cc.construction);
                    return ActionResult::Ok;
                }
                if self.tool == Tool::Rectangle {
                    self.draw_construction = !self.draw_construction;
                    self.status = draw_mode_status("Rectangle", self.draw_construction);
                    return ActionResult::Ok;
                }
                if self.tool == Tool::Line {
                    self.draw_construction = !self.draw_construction;
                    self.status = draw_mode_status("Line", self.draw_construction);
                    return ActionResult::Ok;
                }
                if self.tool == Tool::Circle {
                    self.draw_construction = !self.draw_construction;
                    self.status = draw_mode_status("Circle", self.draw_construction);
                    return ActionResult::Ok;
                }
                if let Err(e) =
                    require_construction_targets_editable(&self.document_health, &self.scene_selection)
                {
                    return ActionResult::Err(e);
                }
                let targets = construction_targets_from_selection(&self.scene_selection);
                match toggle_construction_for_targets(&mut self.doc, &targets) {
                    Ok(count) if count > 0 => {
                        self.status = format!("Toggled construction on {count} item(s)");
                        ActionResult::Ok
                    }
                    Ok(_) => ActionResult::Err("No constructable geometry selected".to_string()),
                    Err(e) => ActionResult::Err(e),
                }
            }
            Action::SetElementVisible { element, visible } => {
                self.element_visibility.set_visible(element, visible);
                self.status = format!(
                    "{} {}",
                    element_label(element),
                    if visible { "shown" } else { "hidden" }
                );
                ActionResult::Ok
            }
            Action::ToggleElementVisibility(element) => {
                let visible = self.element_visibility.toggle(element);
                self.status = format!(
                    "{} {}",
                    element_label(element),
                    if visible { "shown" } else { "hidden" }
                );
                ActionResult::Ok
            }
            Action::CommitElementName { element, name } => {
                if let Err(e) = require_element_editable(&self.document_health, element) {
                    self.status = e.clone();
                    return ActionResult::Err(e);
                }
                match set_element_name(&mut self.doc, element, name) {
                    Ok(()) => {
                        let label = element_name(&self.doc, element)
                            .map(str::to_string)
                            .unwrap_or_else(|| element_label(element));
                        self.status = format!("Renamed to {label}");
                        ActionResult::Ok
                    }
                    Err(e) => {
                        self.status = e.clone();
                        ActionResult::Err(e)
                    }
                }
            }
            Action::AddGeometricConstraint(kind) => {
                let Some(session) = self.sketch_session else {
                    return ActionResult::Err("Open a sketch to add constraints".to_string());
                };
                for element in self.scene_selection.iter() {
                    if let Err(e) = require_element_editable(&self.document_health, element) {
                        return ActionResult::Err(e);
                    }
                }
                match crate::geometric_constraints::add_geometric_constraint_from_selection(
                    &mut self.doc,
                    session.sketch,
                    kind,
                    &self.scene_selection,
                ) {
                    Ok(index) => {
                        self.refresh_document_health();
                        self.status =
                            format!("Added {} constraint {index}", kind.label());
                        ActionResult::Ok
                    }
                    Err(e) => ActionResult::Err(e),
                }
            }
            Action::ApplyConstraintShortcut(key) => {
                let rows = crate::geometric_constraints::constraint_pane_rows(&self.scene_selection);
                let Some(kind) =
                    crate::geometric_constraints::enabled_constraint_for_key(&rows, key)
                else {
                    return ActionResult::Err(format!(
                        "Constraint shortcut '{}' is not active",
                        key.to_ascii_uppercase()
                    ));
                };
                self.apply(Action::AddGeometricConstraint(kind))
            }
            Action::FocusElementName => {
                let Some(element) = single_nameable_from_selection(&self.scene_selection) else {
                    return ActionResult::Err("Select a single element to rename".to_string());
                };
                self.panes.set(Pane::Context, true);
                self.context_pane.focus_name_field = true;
                self.context_pane.synced_element = Some(element);
                self.context_pane.name_draft =
                    element_name(&self.doc, element).unwrap_or_default().to_string();
                self.status = "Rename element".to_string();
                ActionResult::Ok
            }
        };
        if matches!(result, ActionResult::Ok) {
            if let (Some(log), Some(action)) = (&self.command_log, logged_action) {
                log.borrow_mut().after_apply(action, &self.doc);
            }
        }
        result
    }

    /// Add the constraint implied by leaving a snapped point on its target (deduped).
    fn add_snap_constraint(
        &mut self,
        sketch: SketchId,
        point: ConstraintPoint,
        target: crate::snapping::SnapTarget,
    ) -> Result<(), String> {
        if crate::snapping::snap_constraint_already_present(&self.doc, point, target) {
            return Ok(());
        }
        let kind = crate::snapping::snap_constraint_kind(point, target);
        self.doc.constraints.push(crate::model::Constraint {
            sketch,
            kind,
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        self.doc
            .shape_order
            .push(crate::model::ShapeKind::Constraint);
        let new_index = self.doc.constraints.len() - 1;
        crate::constraints::remove_subsumed_point_on_line(&mut self.doc, sketch, new_index);
        crate::constraints::solve_document_constraints(&mut self.doc)?;
        self.refresh_document_health();
        Ok(())
    }

    fn exit_sketch_session(&mut self) {
        self.active_snap = None;
        self.sketch_session = None;
        self.sketch_reframe_pending = false;
        self.creating_rect = None;
        self.creating_line = None;
        self.editing_committed_dim = None;
        // Return to the pre-sketch camera pose; the transition restores world-orbit mode on
        // completion (its `view_up` is `None`). Fall back to a plain mode-leave if unknown.
        if let Some(pose) = self.pre_sketch_pose.take() {
            self.cam.start_transition_to_view(pose, VIEW_TRANSITION_DURATION);
        } else {
            self.cam.leave_sketch_mode();
        }
        self.tool = Tool::Select;
    }

    fn sketch_zoom_distance(
        &self,
        sketch: SketchId,
        viewport: egui::Rect,
        frame_padding_px: f32,
    ) -> Option<f32> {
        let frame_target = sketch_camera_target(&self.doc, sketch)?;
        let bounds = frame_target.zoom?;
        let face = self.doc.sketch_face(sketch)?;
        let frame = sketch_frame(&self.doc, face)?;
        let view_direction = self.cam.visible_face_view_direction(
            frame_target.target,
            frame_target.face_normal,
        );
        let current_look = (frame_target.target - self.cam.eye()).normalize_or_zero();
        let sketch_up = sketch_view_up(
            view_direction,
            &frame,
            current_look,
            self.cam.view_up_hint(),
        );
        let corners = bounds.world_corners(&frame);
        Some(self.cam.distance_to_fit_corners_with_up(
            frame_target.target,
            view_direction,
            sketch_up,
            &corners,
            frame_padding_px,
            viewport,
        ))
    }

    /// Apply deferred sketch framing once the viewport rect is available.
    pub fn apply_pending_sketch_reframe(&mut self, viewport: egui::Rect) {
        if !self.sketch_reframe_pending {
            return;
        }
        let Some(sketch) = self.sketch_session.map(|session| session.sketch) else {
            self.sketch_reframe_pending = false;
            return;
        };
        if let Some(zoom_distance) =
            self.sketch_zoom_distance(sketch, viewport, SKETCH_EDIT_FRAME_PADDING_PX)
        {
            self.cam.set_transition_zoom(zoom_distance);
        }
        self.sketch_reframe_pending = false;
    }

    fn enter_sketch(
        &mut self,
        sketch: SketchId,
        viewport: Option<egui::Rect>,
        frame_padding_px: Option<f32>,
    ) -> ActionResult {
        self.sketch_reframe_pending = false;
        // Remember where the camera was so exiting can return to it. Only capture on the
        // first entry, so switching between sketches still returns to the pre-sketch pose.
        if self.sketch_session.is_none() {
            self.pre_sketch_pose = Some(self.cam.capture_view());
        }
        if let Some(frame_target) = sketch_camera_target(&self.doc, sketch) {
            let face = self.doc.sketch_face(sketch).unwrap();
            let frame = sketch_frame(&self.doc, face).unwrap();
            let view_direction = self.cam.visible_face_view_direction(
                frame_target.target,
                frame_target.face_normal,
            );
            let current_look = (frame_target.target - self.cam.eye()).normalize_or_zero();
            let sketch_up = sketch_view_up(
                view_direction,
                &frame,
                current_look,
                self.cam.view_up_hint(),
            );
            let zoom_distance = frame_target.zoom.and_then(|_| {
                let vp = viewport?;
                let padding = frame_padding_px?;
                self.sketch_zoom_distance(sketch, vp, padding)
            });
            if frame_target.zoom.is_some() && viewport.is_none() {
                self.sketch_reframe_pending = true;
            }
            self.cam.start_sketch_view_transition(
                frame_target.target,
                frame_target.face_normal,
                zoom_distance,
                VIEW_TRANSITION_DURATION,
                sketch_up,
            );
        }
        self.sketch_session = Some(SketchSession { sketch });
        self.creating_rect = None;
        self.creating_line = None;
        if !self.tool.is_sketch_edit_tool() {
            self.tool = Tool::Select;
        }
        let name = sketch_label(&self.doc, sketch);
        self.status = match self.tool {
            Tool::Rectangle => format!("{name} — click to set corner"),
            Tool::Line => format!("{name} — click to set start"),
            _ => format!("{name} — pick line or rectangle"),
        };
        ActionResult::Ok
    }

    fn write_to(&mut self, path: &str) -> ActionResult {
        match crate::storage::save(path, &self.doc) {
            Ok(()) => {
                self.path = Some(path.to_string());
                self.status = format!(
                    "Saved {} rectangle(s), {} line(s) to {}",
                    self.doc.rects.len(),
                    self.doc.lines.len(),
                    path
                );
                ActionResult::Ok
            }
            Err(e) => {
                self.status = format!("Save failed: {e}");
                ActionResult::Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::SketchFrame;

    fn xy_frame() -> SketchFrame {
        SketchFrame {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Y,
            normal: Vec3::Z,
        }
    }

    /// Dominant screen direction of a world axis from the origin (egui y-down).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ScreenAxisDir {
        Left,
        Right,
        Up,
        Down,
    }

    fn axis_screen_dir(
        cam: &crate::camera::Camera,
        viewport: egui::Rect,
        world_axis: Vec3,
    ) -> Option<ScreenAxisDir> {
        let vp = cam.view_proj(viewport);
        let o = cam.project(Vec3::ZERO, viewport, &vp)?;
        let p = cam.project(world_axis * 100.0, viewport, &vp)?;
        let d = p - o;
        if d.length() < 1.0 {
            return None;
        }
        if d.x.abs() >= d.y.abs() {
            Some(if d.x > 0.0 {
                ScreenAxisDir::Right
            } else {
                ScreenAxisDir::Left
            })
        } else if d.y > 0.0 {
            Some(ScreenAxisDir::Down)
        } else {
            Some(ScreenAxisDir::Up)
        }
    }

    fn axis_layout(
        cam: &crate::camera::Camera,
        viewport: egui::Rect,
    ) -> Option<(ScreenAxisDir, ScreenAxisDir)> {
        Some((
            axis_screen_dir(cam, viewport, Vec3::X)?,
            axis_screen_dir(cam, viewport, Vec3::Y)?,
        ))
    }

    fn begin_default_sketch(state: &mut AppState) -> SketchId {
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        state.sketch_session.unwrap().sketch
    }

    #[test]
    fn extrude_tool_toggle_distance_and_commit() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state
            .doc
            .rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        state.doc.shape_order.push(ShapeKind::Rect);
        state.refresh_document_health();

        state.apply(Action::SetTool(Tool::Extrude));
        state.apply(Action::ToggleExtrudeFace {
            face: ExtrudeFace::Rect(0),
        });
        assert_eq!(
            state.creating_extrusion.as_ref().unwrap().faces,
            vec![ExtrudeFace::Rect(0)]
        );
        // Toggling again removes it.
        state.apply(Action::ToggleExtrudeFace {
            face: ExtrudeFace::Rect(0),
        });
        assert!(state.creating_extrusion.as_ref().unwrap().faces.is_empty());

        state.apply(Action::ToggleExtrudeFace {
            face: ExtrudeFace::Rect(0),
        });
        state.apply(Action::SetExtrudeDistance { distance: 7.0 });
        state.apply(Action::CommitExtrusion);

        assert_eq!(state.doc.extrusions.len(), 1);
        assert_eq!(state.doc.extrusions[0].distance, 7.0);
        assert_eq!(state.doc.bodies.len(), 1);
        assert!(state.creating_extrusion.is_none());
    }

    #[test]
    fn export_stl_writes_a_parseable_file() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state
            .doc
            .rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        state.doc.shape_order.push(ShapeKind::Rect);
        state.apply(Action::SetTool(Tool::Extrude));
        state.apply(Action::ToggleExtrudeFace {
            face: ExtrudeFace::Rect(0),
        });
        state.apply(Action::SetExtrudeDistance { distance: 7.0 });
        state.apply(Action::CommitExtrusion);
        assert_eq!(state.doc.bodies.len(), 1);

        let path = std::env::temp_dir().join(format!("le3_export_{}.stl", std::process::id()));
        let path_str = path.to_string_lossy().to_string();
        let result = state.apply(Action::ExportStl {
            path: path_str.clone(),
            body: None,
        });
        assert_eq!(result, ActionResult::Ok, "status: {}", state.status);
        let text = std::fs::read_to_string(&path).expect("read exported stl");
        let tris = crate::stl::parse_ascii_stl(&text).expect("parse exported stl");
        assert_eq!(tris.len(), 12, "a box has 12 triangles");

        // Exporting a non-existent named body fails cleanly.
        let missing = state.apply(Action::ExportStl {
            path: path_str.clone(),
            body: Some("Nope".into()),
        });
        assert!(matches!(missing, ActionResult::Err(_)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn picking_extrude_tool_from_within_a_sketch_exits_sketch_editing() {
        let mut state = AppState::default();
        let _sketch = begin_default_sketch(&mut state);
        assert!(state.sketch_session.is_some());
        // Extruding acts on the 3D model, so entering the tool leaves sketch editing.
        state.apply(Action::SetTool(Tool::Extrude));
        assert_eq!(state.tool, Tool::Extrude);
        assert!(state.sketch_session.is_none());
    }

    #[test]
    fn edit_extrusion_loads_into_creating_state() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state
            .doc
            .rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        state.doc.shape_order.push(ShapeKind::Rect);
        state.apply(Action::CreateExtrusion {
            sketch,
            faces: vec![ExtrudeFace::Rect(0)],
            distance: 6.0,
        });

        state.apply(Action::EditExtrusion { index: 0 });
        let ce = state.creating_extrusion.as_ref().unwrap();
        assert_eq!(ce.edit_index, Some(0));
        assert_eq!(ce.distance, 6.0);
        assert_eq!(state.tool, Tool::Extrude);

        // Editing then committing updates in place (no new extrusion/body).
        state.apply(Action::SetExtrudeDistance { distance: 15.0 });
        state.apply(Action::CommitExtrusion);
        assert_eq!(state.doc.extrusions.len(), 1);
        assert_eq!(state.doc.extrusions[0].distance, 15.0);
        assert_eq!(state.doc.bodies.len(), 1);
    }

    #[test]
    fn apply_snap_constraint_adds_coincident_dedups_and_solves() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state
            .doc
            .lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        state
            .doc
            .lines
            .push(Line::from_local_endpoints(sketch, 10.3, 0.2, 20.0, 0.0));
        state.doc.shape_order.push(ShapeKind::Line);
        state.doc.shape_order.push(ShapeKind::Line);
        state.refresh_document_health();

        let moved = ConstraintPoint::LineEndpoint {
            line: 1,
            end: LineEnd::Start,
        };
        let anchor = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::End,
        };
        let target = crate::snapping::SnapTarget::Vertex(anchor);

        let before = state.doc.constraints.len();
        state.apply(Action::ApplySnapConstraint {
            point: moved,
            target,
        });
        assert_eq!(state.doc.constraints.len(), before + 1);

        let a = crate::geometric_constraints::point_uv(&state.doc, anchor).unwrap();
        let m = crate::geometric_constraints::point_uv(&state.doc, moved).unwrap();
        assert!(
            (a.0 - m.0).abs() < 1e-2 && (a.1 - m.1).abs() < 1e-2,
            "snapped endpoints should coincide: {a:?} vs {m:?}"
        );

        // Applying the same snap again must not add a duplicate constraint.
        state.apply(Action::ApplySnapConstraint {
            point: moved,
            target,
        });
        assert_eq!(state.doc.constraints.len(), before + 1);
    }

    #[test]
    fn set_snapping_toggles_flag() {
        let mut state = AppState::default();
        assert!(state.snapping_enabled);
        state.apply(Action::SetSnapping(false));
        assert!(!state.snapping_enabled);
        state.apply(Action::SetSnapping(true));
        assert!(state.snapping_enabled);
    }

    #[test]
    fn new_document_clears_state() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.rects.push(Rect::from_local_corners(sketch, 0., 0., 10., 10.));
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0., 0., 1., 0.));
        state.doc.shape_order.push(ShapeKind::Line);
        state.path = Some("/tmp/test.le3".to_string());
        state.apply(Action::NewDocument);
        assert!(state.doc.rects.is_empty());
        assert!(state.doc.lines.is_empty());
        assert_eq!(state.doc.construction_planes.len(), 1);
        assert!(state.path.is_none());
    }

    #[test]
    fn set_tool_line_without_sketch_session() {
        let mut state = AppState::default();
        let result = state.apply(Action::SetTool(Tool::Line));
        assert_eq!(result, ActionResult::Ok);
        assert_eq!(state.tool, Tool::Line);
        assert!(state.sketch_session.is_none());
    }

    #[test]
    fn begin_sketch_preserves_rectangle_tool() {
        let mut state = AppState::default();
        state.apply(Action::SetTool(Tool::Rectangle));
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        assert_eq!(state.tool, Tool::Rectangle);
        assert!(state.sketch_session.is_some());
    }

    #[test]
    fn begin_sketch_from_sketch_tool_resets_to_select() {
        let mut state = AppState::default();
        state.apply(Action::SetTool(Tool::Sketch));
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        assert_eq!(state.tool, Tool::Select);
    }

    #[test]
    fn set_tool_construction_plane() {
        let mut state = AppState::default();
        state.apply(Action::SetTool(Tool::ConstructionPlane));
        assert_eq!(state.tool, Tool::ConstructionPlane);
    }

    #[test]
    fn edit_construction_plane_updates_offset_and_descendants() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        state.doc.construction_planes.push(plane_from_definition(
            &definition_from_reference(
                &PlaneReference::Face {
                    origin: Vec3::ZERO,
                    normal: Vec3::Z,
                    label: "Ground".to_string(),
                },
                5.0,
                0.0,
            ),
            ConstructionPlaneParent::Sketch(sketch),
        ));
        let child_before = state.doc.construction_planes[1].origin.z;

        state.apply(Action::BeginEditConstructionPlane { index: 0 });
        state.apply(Action::SetPlaneOffset {
            value: "30".to_string(),
        });
        state.apply(Action::CommitConstructionPlane);

        assert!((state.doc.construction_planes[0].origin.z - 30.0).abs() < 1e-3);
        assert!((state.doc.construction_planes[1].origin.z - child_before - 30.0).abs() < 1e-3);
        assert!(state.creating_plane.is_none());
    }

    #[test]
    fn commit_construction_plane_adds_to_document_not_export_list() {
        let mut state = AppState::default();
        state.apply(Action::BeginConstructionPlane {
            reference: PlaneReference::Face {
                origin: Vec3::ZERO,
                normal: Vec3::Z,
                label: "Ground".to_string(),
            },
            parent: ConstructionPlaneParent::Root,
        });
        let mut cp = state.creating_plane.take().unwrap();
        cp.offset_text = "20".to_string();
        cp.user_edited_offset = true;
        state.creating_plane = Some(cp);
        state.apply(Action::CommitConstructionPlane);
        assert_eq!(state.doc.construction_planes.len(), 2);
        assert!((state.doc.construction_planes[1].origin.z - 20.0).abs() < 1e-3);
        assert!(state
            .scene_selection
            .is_selected(SceneElement::ConstructionPlane(1)));
    }

    #[test]
    fn commit_construction_plane_replaces_stale_selection() {
        let mut state = AppState::default();
        state.apply(Action::BeginConstructionPlane {
            reference: PlaneReference::Face {
                origin: Vec3::ZERO,
                normal: glam::Vec3::Z,
                label: "Ground".to_string(),
            },
            parent: ConstructionPlaneParent::Root,
        });
        state.scene_selection.clear();
        click_scene_selection(
            &mut state.scene_selection,
            SceneElement::ConstructionPlane(0),
            false,
        );
        let mut cp = state.creating_plane.take().unwrap();
        cp.offset_live = 12.0;
        state.creating_plane = Some(cp);
        state.apply(Action::CommitConstructionPlane);
        assert!(!state
            .scene_selection
            .is_selected(SceneElement::ConstructionPlane(0)));
        assert!(state
            .scene_selection
            .is_selected(SceneElement::ConstructionPlane(1)));
    }

    #[test]
    fn live_dims_use_offset_live_not_mouse() {
        let mut state = AppState::default();
        state.apply(Action::BeginConstructionPlane {
            reference: PlaneReference::Axis {
                origin: Vec3::ZERO,
                direction: Vec3::X,
                label: "Line".to_string(),
            },
            parent: ConstructionPlaneParent::Root,
        });
        let cp = state.creating_plane.as_mut().unwrap();
        cp.offset_live = 12.0;
        cp.axis_angle_deg = 45.0;
        assert_eq!(cp.live_dims(), (12.0, 45.0));
    }

    #[test]
    fn undo_construction_plane() {
        let mut state = AppState::default();
        state.apply(Action::BeginConstructionPlane {
            reference: PlaneReference::Face {
                origin: Vec3::ZERO,
                normal: Vec3::Z,
                label: "Ground".to_string(),
            },
            parent: ConstructionPlaneParent::Root,
        });
        let mut cp = state.creating_plane.take().unwrap();
        cp.offset_text = "5".to_string();
        cp.user_edited_offset = true;
        state.creating_plane = Some(cp);
        state.apply(Action::CommitConstructionPlane);
        state.apply(Action::UndoLast);
        assert_eq!(state.doc.construction_planes.len(), 1);
    }

    #[test]
    fn undo_last_removes_most_recent_shape() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.rects.push(Rect::from_local_corners(sketch, 0., 0., 1., 1.));
        state.doc.shape_order.push(ShapeKind::Rect);
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0., 0., 5., 0.));
        state.doc.shape_order.push(ShapeKind::Line);
        state.apply(Action::UndoLast);
        assert_eq!(state.doc.lines.len(), 0);
        assert_eq!(state.doc.rects.len(), 1);
        state.apply(Action::UndoLast);
        assert!(state.doc.rects.is_empty());
    }

    #[test]
    fn commit_rectangle_adds_to_document() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::new(0.0, 0.0, 0.0),
            texts: ["10".to_string(), "5".to_string()],
            focused: 0,
            last_mouse: Vec3::new(10.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitRectangle);
        assert_eq!(state.doc.rects.len(), 1);
        assert!((state.doc.rects[0].w - 10.0).abs() < 1e-4);
        assert!((state.doc.rects[0].h - 5.0).abs() < 1e-4);
        assert_eq!(state.doc.constraints.len(), 2);
        assert!(state.doc.rects[0].width_locked);
        assert!(state.doc.rects[0].height_locked);
        assert_eq!(state.doc.rects[0].sketch, sketch);
        assert!(state.creating_rect.is_none());
    }

    #[test]
    fn commit_rectangle_without_typed_dims_leaves_locks_clear() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::new(0.0, 0.0, 0.0),
            texts: ["".to_string(), "".to_string()],
            focused: 0,
            last_mouse: Vec3::new(10.0, 5.0, 0.0),
            user_edited: [false, false],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitRectangle);
        let rect = &state.doc.rects[0];
        assert!(!rect.width_locked);
        assert!(!rect.height_locked);
    }

    #[test]
    fn commit_rectangle_with_inline_parameter_definition() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["width=10mm".to_string(), "5".to_string()],
            focused: 0,
            last_mouse: Vec3::new(100.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitRectangle);
        assert_eq!(state.doc.parameters.len(), 1);
        assert_eq!(state.doc.parameters[0].name, "width");
        assert_eq!(state.doc.rects[0].width_expr.as_deref(), Some("width"));
        assert!((state.doc.rects[0].w - 10.0).abs() < 1e-3);
    }

    #[test]
    fn create_parameter_from_line_length_action() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.doc.lines.push(crate::model::Line::from_local_endpoints(
            sketch, 0.0, 0.0, 15.0, 0.0,
        ));
        state.doc.shape_order.push(crate::model::ShapeKind::Line);
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Line(0),
            additive: false,
        });
        state.apply(Action::CreateParameterFromLineLength {
            line_index: 0,
            name: None,
        });
        assert_eq!(state.doc.parameters.len(), 1);
        assert_eq!(state.doc.parameters[0].name, "line0_length");
        assert_eq!(state.doc.parameters[0].expression, "15.0 mm");
        assert!(crate::parameters::parameter_value_is_readonly(
            &state.doc.parameters[0]
        ));
    }

    #[test]
    fn apply_constraint_shortcut_a_adds_parallel() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.tool = Tool::Constraint;
        state.doc.lines.push(crate::model::Line::from_local_endpoints(
            sketch, 0.0, 0.0, 10.0, 0.0,
        ));
        state.doc.lines.push(crate::model::Line::from_local_endpoints(
            sketch, 0.0, 5.0, 2.0, 8.0,
        ));
        state.doc.shape_order.push(crate::model::ShapeKind::Line);
        state.doc.shape_order.push(crate::model::ShapeKind::Line);
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Line(0),
            additive: false,
        });
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Line(1),
            additive: true,
        });
        state.apply(Action::ApplyConstraintShortcut('A'));
        assert_eq!(state.doc.constraints.len(), 1);
        assert!(matches!(
            state.doc.constraints[0].kind,
            crate::model::ConstraintKind::Parallel { .. }
        ));
    }

    #[test]
    fn inline_parameter_survives_rectangle_deletion() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["width=10mm".to_string(), "5".to_string()],
            focused: 0,
            last_mouse: Vec3::new(100.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitRectangle);
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Rect(0),
            additive: false,
        });
        state.apply(Action::DeleteSelection);
        assert_eq!(state.doc.parameters.len(), 1);
        assert_eq!(state.doc.parameters[0].name, "width");
        assert!(state.doc.rects[0].deleted);
    }

    #[test]
    fn commit_rectangle_with_parameter_reference() {
        let mut state = AppState::default();
        add_parameter(&mut state.doc, "A".to_string(), "10mm".to_string()).unwrap();
        begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["A".to_string(), "5".to_string()],
            focused: 0,
            last_mouse: Vec3::new(100.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitRectangle);
        let rect = &state.doc.rects[0];
        assert!((rect.w - 10.0).abs() < 1e-3);
        assert_eq!(rect.width_expr.as_deref(), Some("A"));

        set_parameter_expression(&mut state.doc, 0, "20mm".to_string()).unwrap();
        assert!((state.doc.rects[0].w - 20.0).abs() < 1e-3);
    }

    #[test]
    fn rect_end_point_uses_parameter_reference() {
        let mut doc = Document::default();
        add_parameter(&mut doc, "A".to_string(), "10mm".to_string()).unwrap();
        let cr = CreatingRect {
            origin: Vec3::ZERO,
            texts: ["A".to_string(), "".to_string()],
            focused: 0,
            last_mouse: Vec3::new(100.0, 4.0, 0.0),
            user_edited: [true, false],
            pending_focus: false,
            construction: false,
        };
        let frame = xy_frame();
        let end = cr.end_point(&frame, &doc);
        assert!((end.x - 10.0).abs() < 1e-3);
        // Height is unconstrained, so it follows the mouse.
        assert!((end.y - 4.0).abs() < 1e-3);
    }

    #[test]
    fn commit_rectangle_expression_stores_geometry_not_expression_text() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["2in".to_string(), "5mm".to_string()],
            focused: 0,
            last_mouse: Vec3::new(100.0, 100.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitRectangle);
        let rect = &state.doc.rects[0];
        assert!((rect.w - 50.8).abs() < 1e-2);
        assert!((rect.h - 5.0).abs() < 1e-4);
        assert!(rect.width_locked);
    }

    #[test]
    fn set_dim_label_offset_persists_on_rectangle() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["10".to_string(), "5".to_string()],
            focused: 0,
            last_mouse: Vec3::new(10.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitRectangle);
        let width_constraint = find_distance_constraint(
            &state.doc,
            DistanceTarget::RectWidth(0),
        )
        .unwrap();
        state.apply(Action::SetDimLabelOffset {
            target: width_constraint,
            offset: 55.0,
        });
        assert_eq!(state.doc.constraints[0].dim_offset, Some(55.0));
        assert_eq!(state.doc.rects[0].width_dim_offset, Some(55.0));
    }

    #[test]
    fn dim_label_target_in_sketch_finds_width_constraint() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        state.doc.shape_order.push(ShapeKind::Rect);
        add_distance_constraint(
            &mut state.doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "10mm".to_string(),
        )
        .unwrap();
        let target = dim_label_target_in_sketch(&state.doc, sketch, DimLabelAxis::Width);
        assert_eq!(target, Some(0));
    }

    #[test]
    fn begin_edit_committed_dim_works_while_rectangle_tool_in_sketch() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        state.doc.shape_order.push(ShapeKind::Rect);
        add_distance_constraint(
            &mut state.doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "10mm".to_string(),
        )
        .unwrap();
        state.apply(Action::SetTool(Tool::Rectangle));
        let width_constraint = find_distance_constraint(
            &state.doc,
            DistanceTarget::RectWidth(0),
        )
        .unwrap();
        let result = state.apply(Action::BeginEditCommittedDim {
            target: width_constraint,
        });
        assert_eq!(result, ActionResult::Ok);
        assert!(state.editing_committed_dim.is_some());
    }

    #[test]
    fn begin_edit_committed_dim_blocked_while_drawing_rectangle() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["".to_string(), "".to_string()],
            focused: 0,
            last_mouse: Vec3::new(10.0, 5.0, 0.0),
            user_edited: [false, false],
            pending_focus: false,
            construction: false,
        });
        let result = state.apply(Action::BeginEditCommittedDim { target: 0 });
        assert!(matches!(result, ActionResult::Err(_)));
        assert!(state.editing_committed_dim.is_none());
    }

    #[test]
    fn commit_committed_dim_updates_rectangle_width() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["10".to_string(), "5".to_string()],
            focused: 0,
            last_mouse: Vec3::new(10.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitRectangle);
        let width_constraint = find_distance_constraint(
            &state.doc,
            DistanceTarget::RectWidth(0),
        )
        .unwrap();
        state.apply(Action::BeginEditCommittedDim {
            target: width_constraint,
        });
        state.apply(Action::SetRectDimension {
            axis: RectAxis::Width,
            value: "20mm".to_string(),
        });
        state.apply(Action::CommitCommittedDim);
        assert!((state.doc.rects[0].w - 20.0).abs() < 1e-3);
        assert_eq!(state.doc.constraints[0].expression, "20mm");
        assert!(state.editing_committed_dim.is_none());
    }

    #[test]
    fn cancel_operation_clears_committed_dim_edit_before_exiting_sketch() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["10".to_string(), "5".to_string()],
            focused: 0,
            last_mouse: Vec3::new(10.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitRectangle);
        let width_constraint = find_distance_constraint(
            &state.doc,
            DistanceTarget::RectWidth(0),
        )
        .unwrap();
        state.apply(Action::BeginEditCommittedDim {
            target: width_constraint,
        });
        state.apply(Action::CancelOperation);
        assert!(state.editing_committed_dim.is_none());
        assert!(state.sketch_session.is_some());
    }

    #[test]
    fn commit_line_adds_to_document() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_line = Some(CreatingLine {
            origin: Vec3::ZERO,
            text: "10".to_string(),
            last_mouse: Vec3::new(10.0, 0.0, 0.0),
            user_edited: true,
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitLine);
        assert_eq!(state.doc.lines.len(), 1);
        assert!((state.doc.lines[0].length() - 10.0).abs() < 1e-4);
        assert_eq!(state.doc.constraints.len(), 1);
        assert!(state.doc.lines[0].length_locked);
        assert!(state.creating_line.is_none());
    }

    #[test]
    fn line_tool_chains_into_next_segment() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.tool = Tool::Line;
        state.creating_line = Some(CreatingLine {
            origin: Vec3::ZERO,
            text: "10".to_string(),
            last_mouse: Vec3::new(10.0, 0.0, 0.0),
            user_edited: true,
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitLine);

        // The segment was committed, and a fresh segment is already started at its endpoint.
        assert_eq!(state.doc.lines.len(), 1);
        let cl = state
            .creating_line
            .as_ref()
            .expect("a new segment should be chained from the endpoint");
        let frame = sketch_geometry_frame(&state.doc, sketch).unwrap();
        let (ou, ov) = world_to_local(&frame, cl.origin);
        assert!((ou - 10.0).abs() < 1e-3 && ov.abs() < 1e-3, "new origin at endpoint");
        // The new start snaps to the previous endpoint so the polyline stays connected.
        assert!(matches!(
            state.line_start_snap,
            Some(crate::snapping::SnapTarget::Vertex(ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::End
            }))
        ));

        // Committing the chained segment connects the two lines (coincident constraint).
        state.creating_line.as_mut().unwrap().last_mouse = Vec3::new(10.0, 10.0, 0.0);
        state.creating_line.as_mut().unwrap().text.clear();
        state.creating_line.as_mut().unwrap().user_edited = false;
        state.apply(Action::CommitLine);
        assert_eq!(state.doc.lines.len(), 2);
        assert!(state
            .doc
            .constraints
            .iter()
            .any(|c| !c.deleted && matches!(c.kind, crate::model::ConstraintKind::Coincident { .. })));
    }

    #[test]
    fn line_tool_stops_chaining_when_closing_on_a_vertex() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.tool = Tool::Line;
        // An existing line whose start vertex sits at (10, 0).
        state
            .doc
            .lines
            .push(Line::from_local_endpoints(sketch, 10.0, 0.0, 20.0, 0.0));
        state.doc.shape_order.push(ShapeKind::Line);

        state.creating_line = Some(CreatingLine {
            origin: Vec3::ZERO,
            text: "10".to_string(),
            last_mouse: Vec3::new(10.0, 0.0, 0.0),
            user_edited: true,
            pending_focus: false,
            construction: false,
        });
        // The end latched onto the existing vertex at (10, 0).
        state.line_end_snap = Some(crate::snapping::SnapTarget::Vertex(
            ConstraintPoint::LineEndpoint {
                line: 0,
                end: LineEnd::Start,
            },
        ));
        state.apply(Action::CommitLine);

        assert_eq!(state.doc.lines.len(), 2);
        assert!(
            state.creating_line.is_none(),
            "closing onto a vertex finishes the polyline"
        );
    }

    #[test]
    fn commit_circle_adds_to_document_with_diameter_constraint() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_circle = Some(CreatingCircle {
            origin: Vec3::ZERO,
            text: "20".to_string(),
            last_mouse: Vec3::new(10.0, 0.0, 0.0),
            user_edited: true,
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitCircle);
        assert_eq!(state.doc.circles.len(), 1);
        assert!((state.doc.circles[0].diameter() - 20.0).abs() < 1e-4);
        assert_eq!(state.doc.constraints.len(), 1);
        assert!(state.doc.circles[0].diameter_locked);
        assert!(state.creating_circle.is_none());
    }

    #[test]
    fn dimension_tool_begins_edit_when_line_selected() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 8.0, 0.0));
        state.doc.shape_order.push(ShapeKind::Line);
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Line(0),
            additive: false,
        });
        state.apply(Action::SetTool(Tool::Dimension));
        assert!(state.editing_committed_dim.is_some());
        assert_eq!(
            state.editing_committed_dim.as_ref().unwrap().target,
            DimEditTarget::New(DimensionTarget::Distance(DistanceTarget::LineLength(0)))
        );
    }

    #[test]
    fn dimension_tool_begins_edit_when_rect_edge_selected() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        state.doc.shape_order.push(ShapeKind::Rect);
        state.apply(Action::ClickSceneElement {
            element: SceneElement::RectEdge(0, RectEdge::Left),
            additive: false,
        });
        state.apply(Action::SetTool(Tool::Dimension));
        assert_eq!(
            state.editing_committed_dim.as_ref().unwrap().target,
            DimEditTarget::New(DimensionTarget::Distance(DistanceTarget::RectHeight(0)))
        );
    }

    #[test]
    fn angle_gizmo_constraint_only_while_editing_committed_angle() {
        use crate::model::{ConstraintLine, DimensionTarget};

        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        state.doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 0.0, 10.0));
        state.doc.shape_order.push(ShapeKind::Line);
        state.doc.shape_order.push(ShapeKind::Line);
        crate::constraints::add_angle_constraint(
            &mut state.doc,
            sketch,
            ConstraintLine::Line(0),
            ConstraintLine::Line(1),
            "90deg".to_string(),
        )
        .unwrap();
        assert_eq!(
            angle_gizmo_constraint_for_edit(state.editing_committed_dim.as_ref(), &state.doc),
            None
        );
        state.editing_committed_dim = Some(EditingCommittedDim {
            target: DimEditTarget::Constraint(0),
            text: "90deg".to_string(),
            pending_focus: true,
        });
        assert_eq!(
            angle_gizmo_constraint_for_edit(state.editing_committed_dim.as_ref(), &state.doc),
            Some(0)
        );
        state.editing_committed_dim = Some(EditingCommittedDim {
            target: DimEditTarget::New(DimensionTarget::Angle {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
                rotation_sign: 1,
            }),
            text: "45deg".to_string(),
            pending_focus: true,
        });
        assert_eq!(
            angle_gizmo_constraint_for_edit(state.editing_committed_dim.as_ref(), &state.doc),
            None
        );
    }

    #[test]
    fn dimension_tool_begins_angle_edit_for_two_non_parallel_lines() {
        use crate::model::{ConstraintLine, DimensionTarget};

        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        state.doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 0.0, 10.0));
        state.doc.shape_order.push(ShapeKind::Line);
        state.doc.shape_order.push(ShapeKind::Line);
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Line(0),
            additive: false,
        });
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Line(1),
            additive: true,
        });
        state.apply(Action::SetTool(Tool::Dimension));
        assert_eq!(
            state.editing_committed_dim.as_ref().unwrap().target,
            DimEditTarget::New(DimensionTarget::Angle {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
                rotation_sign: 1,
            })
        );
    }

    #[test]
    fn dimension_tool_adds_distance_constraint_to_line() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 8.0, 0.0));
        state.doc.shape_order.push(ShapeKind::Line);
        state.apply(Action::SetTool(Tool::Dimension));
        state.apply(Action::BeginDimensionEdit {
            target: DimensionTarget::Distance(DistanceTarget::LineLength(0)),
        });
        state.apply(Action::SetLineLength {
            value: "12mm".to_string(),
        });
        state.apply(Action::CommitCommittedDim);
        assert_eq!(state.doc.constraints.len(), 1);
        assert!((state.doc.lines[0].length() - 12.0).abs() < 1e-3);
    }

    #[test]
    fn rect_end_point_evaluates_unit_expression() {
        let cr = CreatingRect {
            origin: Vec3::ZERO,
            texts: ["2in".to_string(), "5mm / 2".to_string()],
            focused: 0,
            last_mouse: Vec3::new(100.0, 100.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        };
        let frame = xy_frame();
        let doc = Document::default();
        let end = cr.end_point(&frame, &doc);
        assert!((end.x - 50.8).abs() < 1e-3);
        assert!((end.y - 2.5).abs() < 1e-3);
    }

    #[test]
    fn line_end_point_evaluates_mixed_expression() {
        let cl = CreatingLine {
            origin: Vec3::ZERO,
            text: "2in + 5mm / 2".to_string(),
            last_mouse: Vec3::new(10.0, 10.0, 0.0),
            user_edited: true,
            pending_focus: false,
            construction: false,
        };
        let frame = xy_frame();
        let doc = Document::default();
        let end = cl.end_point(&frame, &doc);
        let (u0, v0) = world_to_local(&frame, cl.origin);
        let (u1, v1) = world_to_local(&frame, end);
        let line = Line::from_local_endpoints(0, u0, v0, u1, v1);
        assert!((line.length() - 53.3).abs() < 1e-2);
    }

    #[test]
    fn set_plane_offset_evaluates_expression() {
        let mut state = AppState::default();
        state.apply(Action::BeginConstructionPlane {
            reference: PlaneReference::Face {
                origin: Vec3::ZERO,
                normal: Vec3::Z,
                label: "Ground".to_string(),
            },
            parent: ConstructionPlaneParent::Root,
        });
        state.apply(Action::SetPlaneOffset {
            value: "1in + 2mm".to_string(),
        });
        let cp = state.creating_plane.as_ref().unwrap();
        assert!((cp.offset_live - 27.4).abs() < 1e-3);
        assert_eq!(cp.offset_text, "1in + 2mm");
    }

    #[test]
    fn line_end_point_uses_locked_length() {
        let cl = CreatingLine {
            origin: Vec3::new(1.0, 2.0, 0.0),
            text: "5".to_string(),
            last_mouse: Vec3::new(4.0, 6.0, 0.0),
            user_edited: true,
            pending_focus: false,
            construction: false,
        };
        let frame = xy_frame();
        let doc = Document::default();
        let end = cl.end_point(&frame, &doc);
        let (u0, v0) = world_to_local(&frame, cl.origin);
        let (u1, v1) = world_to_local(&frame, end);
        let line = Line::from_local_endpoints(0, u0, v0, u1, v1);
        assert!((line.length() - 5.0).abs() < 1e-4);
    }

    #[test]
    fn line_end_point_defaults_along_x_when_no_direction() {
        let cl = CreatingLine {
            origin: Vec3::ZERO,
            text: "7".to_string(),
            last_mouse: Vec3::ZERO,
            user_edited: true,
            pending_focus: false,
            construction: false,
        };
        let frame = xy_frame();
        let doc = Document::default();
        let end = cl.end_point(&frame, &doc);
        assert!((end.x - 7.0).abs() < 1e-4);
        assert!(end.y.abs() < 1e-4);
    }

    #[test]
    fn escape_after_commit_rectangle_switches_to_select_not_exit_sketch() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.apply(Action::SetTool(Tool::Rectangle));
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["10".to_string(), "5".to_string()],
            focused: 0,
            last_mouse: Vec3::new(10.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CommitRectangle);
        assert_eq!(state.tool, Tool::Rectangle);
        assert!(state.sketch_session.is_some());

        state.apply(Action::CancelOperation);

        assert!(state.sketch_session.is_some());
        assert_eq!(state.tool, Tool::Select);
        assert_eq!(state.doc.rects.len(), 1);
    }

    #[test]
    fn escape_on_line_tool_in_sketch_switches_to_select() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.apply(Action::SetTool(Tool::Line));
        state.apply(Action::CancelOperation);
        assert!(state.sketch_session.is_some());
        assert_eq!(state.tool, Tool::Select);
    }

    #[test]
    fn escape_on_select_tool_in_sketch_exits_sketch() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        assert_eq!(state.tool, Tool::Select);
        state.apply(Action::CancelOperation);
        assert!(state.sketch_session.is_none());
        assert_eq!(state.tool, Tool::Select);
    }

    #[test]
    fn escape_while_drawing_rectangle_cancels_without_exiting_sketch() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.apply(Action::SetTool(Tool::Rectangle));
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["".to_string(), "".to_string()],
            focused: 0,
            last_mouse: Vec3::new(10.0, 5.0, 0.0),
            user_edited: [false, false],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::CancelOperation);
        assert!(state.sketch_session.is_some());
        assert_eq!(state.tool, Tool::Rectangle);
        assert!(state.creating_rect.is_none());
    }

    #[test]
    fn exit_sketch_restores_world_orbit_mode() {
        let mut state = AppState::default();
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        while state.cam.tick_transition(0.05) {}
        state.cam.orbit_trackball(egui::vec2(10.0, 6.0));
        state.apply(Action::ExitSketch);
        assert!(state.sketch_session.is_none());
        // Exit animates back to the pre-sketch pose; world-orbit mode is restored once the
        // return transition completes.
        while state.cam.tick_transition(0.05) {}
        assert!(!state.cam.has_custom_view_up());
        assert!(!state.cam.has_orbit_trackball_state());
    }

    #[test]
    fn exit_sketch_clears_session() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.apply(Action::ExitSketch);
        assert!(state.sketch_session.is_none());
        assert_eq!(state.tool, Tool::Select);
    }

    #[test]
    fn exit_sketch_returns_to_pre_sketch_view() {
        let mut state = AppState::default();
        let viewport =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let samples = [
            Vec3::ZERO,
            Vec3::new(40.0, 20.0, 0.0),
            Vec3::new(-30.0, 50.0, 10.0),
        ];

        // Capture the camera before entering the sketch.
        let vp_before = state.cam.view_proj(viewport);
        let screens_before: Vec<_> = samples
            .iter()
            .map(|p| state.cam.project(*p, viewport, &vp_before).unwrap())
            .collect();

        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        while state.cam.tick_transition(0.05) {}
        // Entering must actually have moved the camera (otherwise this proves nothing).
        let vp_sketch = state.cam.view_proj(viewport);
        let moved = samples.iter().zip(&screens_before).any(|(p, before)| {
            (state.cam.project(*p, viewport, &vp_sketch).unwrap() - *before).length() > 1.0
        });
        assert!(moved, "entering the sketch should reframe the camera");

        state.apply(Action::ExitSketch);
        while state.cam.tick_transition(0.05) {}

        let vp_after = state.cam.view_proj(viewport);
        for (p, before) in samples.iter().zip(screens_before) {
            let after = state.cam.project(*p, viewport, &vp_after).unwrap();
            assert!(
                (before - after).length() < 0.5,
                "exiting sketch should return to the pre-sketch view: {before:?} -> {after:?} for {p:?}"
            );
        }
    }

    #[test]
    fn begin_ground_plane_sketch_does_not_spin_yaw() {
        // The ground plane is a near-vertical (top-down) view, where yaw is just roll. Entry
        // must keep the current yaw rather than swinging it to zero (which looks like a spin).
        let mut state = AppState::default();
        let yaw_before = state.cam.yaw;
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        while state.cam.tick_transition(0.05) {}
        assert!(
            (state.cam.yaw - yaw_before).abs() < 0.02,
            "ground-plane sketch entry should not change yaw: {yaw_before} -> {}",
            state.cam.yaw
        );
    }

    #[test]
    fn begin_sketch_keeps_yaw_pitch_when_already_face_on() {
        use crate::camera::StandardView;

        let mut state = AppState::default();
        let (yaw, pitch) = StandardView::Top.yaw_pitch();
        state.cam.yaw = yaw;
        state.cam.pitch = pitch;
        state.cam.set_view_up(Some(Vec3::Y));
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        while state.cam.tick_transition(0.05) {}
        assert!((state.cam.yaw - yaw).abs() < 0.02);
        assert!((state.cam.pitch - pitch).abs() < 0.02);
    }

    #[test]
    fn begin_sketch_from_isometric_uses_minimal_axis_rotation() {
        let viewport =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let mut state = AppState::default();
        let start = axis_layout(&state.cam, viewport).expect("startup axes visible");
        assert_eq!(
            start,
            (ScreenAxisDir::Left, ScreenAxisDir::Right),
            "isometric startup should show red left and green right"
        );

        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        while state.cam.tick_transition(0.05) {}

        let end = axis_layout(&state.cam, viewport).expect("sketch axes visible");
        let minimal = [
            (ScreenAxisDir::Down, ScreenAxisDir::Right),
            (ScreenAxisDir::Right, ScreenAxisDir::Down),
        ];
        assert!(
            minimal.contains(&end),
            "sketch entry should use minimal roll: start={start:?} end={end:?}, expected one of {minimal:?}"
        );
        assert_ne!(
            end,
            (ScreenAxisDir::Right, ScreenAxisDir::Up),
            "should not over-rotate to red right + green up"
        );

        let frame = sketch_frame(&state.doc, FaceId::ConstructionPlane(0)).unwrap();
        let vp = state.cam.view_proj(viewport);
        let base = state.cam.project(frame.origin, viewport, &vp).unwrap();
        let u = state
            .cam
            .project(frame.origin + frame.u_axis * 10.0, viewport, &vp)
            .unwrap();
        let v = state
            .cam
            .project(frame.origin + frame.v_axis * 10.0, viewport, &vp)
            .unwrap();
        match end {
            (ScreenAxisDir::Down, ScreenAxisDir::Right) => {
                assert!(u.y > base.y + 5.0, "u should point down on screen");
                assert!(v.x > base.x + 5.0, "v should point right on screen");
            }
            (ScreenAxisDir::Right, ScreenAxisDir::Down) => {
                assert!(u.x > base.x + 5.0, "u should point right on screen");
                assert!(v.y > base.y + 5.0, "v should point down on screen");
            }
            other => panic!("unexpected end layout {other:?}"),
        }
    }

    #[test]
    fn begin_sketch_from_top_view_aligns_v_axis_up() {
        use crate::camera::StandardView;

        let mut state = AppState::default();
        let (yaw, pitch) = StandardView::Top.yaw_pitch();
        state.cam.yaw = yaw;
        state.cam.pitch = pitch;
        state.cam.set_view_up(Some(Vec3::Y));
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        let frame = sketch_frame(&state.doc, FaceId::ConstructionPlane(0)).unwrap();
        while state.cam.tick_transition(0.05) {}
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let vp = state.cam.view_proj(viewport);
        let base = state
            .cam
            .project(frame.origin, viewport, &vp)
            .expect("origin visible");
        let above = state
            .cam
            .project(frame.origin + frame.v_axis * 10.0, viewport, &vp)
            .expect("v offset visible");
        assert!(above.y < base.y, "plane v-axis should point up on screen");
    }

    #[test]
    fn begin_sketch_frames_camera_to_face_normal() {
        let mut state = AppState::default();
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(800.0, 600.0));
        let distance_before = state.cam.distance;
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: Some(viewport),
        });
        assert!(state.cam.is_transitioning());
        assert!(state.sketch_session.is_some());
        while state.cam.tick_transition(0.05) {}
        assert!((state.cam.distance - distance_before).abs() < 0.5);
        let view = (state.cam.eye() - state.cam.target).normalize();
        assert!(view.z > 0.95, "empty plane should look along face normal");
    }

    #[test]
    fn open_sketch_zooms_with_edit_padding() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 100.0, 100.0));
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(800.0, 600.0));
        let before = state.cam.distance;
        state.apply(Action::OpenSketch {
            sketch,
            viewport: Some(viewport),
        });
        assert!(state.cam.is_transitioning());
        while state.cam.tick_transition(0.05) {}
        assert!(state.cam.distance < before);

        let frame = sketch_frame(&state.doc, FaceId::ConstructionPlane(0)).unwrap();
        let bounds = sketch_camera_target(&state.doc, sketch)
            .unwrap()
            .zoom
            .unwrap();
        let corners = bounds.world_corners(&frame);
        let view = (state.cam.eye() - state.cam.target).normalize();
        let fitted = state.cam.distance_to_fit_corners_with_up(
            state.cam.target,
            view,
            state.cam.view_up_hint(),
            &corners,
            SKETCH_EDIT_FRAME_PADDING_PX,
            viewport,
        );
        assert!((state.cam.distance - fitted).abs() < 1.0);
    }

    #[test]
    fn open_sketch_zooms_to_include_circles_beyond_rectangles() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state
            .doc
            .rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 20.0, 20.0));
        state.doc.circles.push(Circle::from_local_center_radius(
            sketch, 80.0, 0.0, 20.0, 0.0,
        ));
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(800.0, 600.0));

        state.apply(Action::OpenSketch {
            sketch,
            viewport: Some(viewport),
        });
        while state.cam.tick_transition(0.05) {}

        let frame = sketch_frame(&state.doc, FaceId::ConstructionPlane(0)).unwrap();
        let bounds = sketch_camera_target(&state.doc, sketch)
            .unwrap()
            .zoom
            .expect("rectangles and circles should produce zoom bounds");
        assert!(
            bounds.half_u >= 50.0,
            "camera bounds should include distant circles, got half_u={}",
            bounds.half_u
        );
        let corners = bounds.world_corners(&frame);
        let view = (state.cam.eye() - state.cam.target).normalize();
        let fitted = state.cam.distance_to_fit_corners_with_up(
            state.cam.target,
            view,
            state.cam.view_up_hint(),
            &corners,
            SKETCH_EDIT_FRAME_PADDING_PX,
            viewport,
        );
        assert!(
            (state.cam.distance - fitted).abs() < 1.0,
            "open sketch should frame all sketch geometry"
        );
    }

    #[test]
    fn open_sketch_deferred_reframe_when_viewport_missing() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 100.0, 100.0));
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(800.0, 600.0));
        let before = state.cam.distance;

        state.apply(Action::OpenSketch {
            sketch,
            viewport: None,
        });
        assert!(state.sketch_reframe_pending);
        while state.cam.tick_transition(0.05) {}
        assert!(
            (state.cam.distance - before).abs() < 0.5,
            "zoom should wait for viewport"
        );

        state.apply_pending_sketch_reframe(viewport);
        while state.cam.tick_transition(0.05) {}
        assert!(state.cam.distance < before);
        assert!(!state.sketch_reframe_pending);

        let frame = sketch_frame(&state.doc, FaceId::ConstructionPlane(0)).unwrap();
        let bounds = sketch_camera_target(&state.doc, sketch)
            .unwrap()
            .zoom
            .unwrap();
        let corners = bounds.world_corners(&frame);
        let view = (state.cam.eye() - state.cam.target).normalize();
        let fitted = state.cam.distance_to_fit_corners_with_up(
            state.cam.target,
            view,
            state.cam.view_up_hint(),
            &corners,
            SKETCH_EDIT_FRAME_PADDING_PX,
            viewport,
        );
        assert!((state.cam.distance - fitted).abs() < 1.0);
    }

    #[test]
    fn open_sketch_frames_wide_geometry_from_isometric() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state
            .doc
            .rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 300.0, 80.0));
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(800.0, 600.0));
        let before = state.cam.distance;

        state.apply(Action::OpenSketch {
            sketch,
            viewport: Some(viewport),
        });
        while state.cam.tick_transition(0.05) {}
        assert!(state.cam.distance < before);

        let frame = sketch_frame(&state.doc, FaceId::ConstructionPlane(0)).unwrap();
        let bounds = sketch_camera_target(&state.doc, sketch)
            .unwrap()
            .zoom
            .unwrap();
        let corners = bounds.world_corners(&frame);
        let view = (state.cam.eye() - state.cam.target).normalize();
        let fitted = state.cam.distance_to_fit_corners_with_up(
            state.cam.target,
            view,
            state.cam.view_up_hint(),
            &corners,
            SKETCH_EDIT_FRAME_PADDING_PX,
            viewport,
        );
        assert!(
            (state.cam.distance - fitted).abs() < 1.0,
            "wide sketch from isometric should frame with sketch view up"
        );
    }

    #[test]
    fn begin_sketch_creates_new_sketch_each_time() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        let second = begin_default_sketch(&mut state);
        assert_eq!(second, 1);
        assert_eq!(state.doc.sketches.len(), 2);
        assert_eq!(
            state.doc.sketches[0].face,
            FaceId::ConstructionPlane(0)
        );
        assert_eq!(
            state.doc.sketches[1].face,
            FaceId::ConstructionPlane(0)
        );
    }

    #[test]
    fn begin_sketch_on_circle_face_hosts_child_sketch() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.circles.push(Circle::from_local_center_radius(
            sketch, 0.0, 0.0, 20.0, 0.0,
        ));
        state.doc.shape_order.push(ShapeKind::Circle);
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        assert!(matches!(
            state.apply(Action::BeginSketch {
                face: FaceId::Circle(0),
                viewport: Some(viewport),
            }),
            ActionResult::Ok
        ));
        assert_eq!(state.doc.sketches.len(), 2);
        assert_eq!(state.doc.sketches[1].face, FaceId::Circle(0));
        assert!(state.sketch_session.is_some());
    }

    #[test]
    fn tree_pane_visible_by_default() {
        let state = AppState::default();
        assert!(state.panes.is_visible(Pane::Hierarchy));
        assert_eq!(Pane::Hierarchy.label(), "Elements");
    }

    #[test]
    fn context_pane_visible_by_default() {
        let state = AppState::default();
        assert!(state.panes.is_visible(Pane::Context));
        assert_eq!(Pane::Context.label(), "Context");
    }

    #[test]
    fn click_scene_element_selects_and_deselects() {
        let mut state = AppState::default();
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Rect(0),
            additive: false,
        });
        assert!(state.scene_selection.is_selected(SceneElement::Rect(0)));
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Rect(0),
            additive: false,
        });
        assert!(state.scene_selection.is_empty());
    }

    #[test]
    fn delete_selection_tombstones_selected_geometry() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::default());
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        state.doc.shape_order.push(ShapeKind::Line);
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Line(0),
            additive: false,
        });
        state.apply(Action::DeleteSelection);
        assert!(state.doc.lines[0].deleted);
        assert!(state.scene_selection.is_empty());
        assert_eq!(
            state.document_health.element_status(SceneElement::Line(0)),
            crate::document_health::HealthStatus::Healthy
        );
    }

    #[test]
    fn delete_selection_status_reports_invalid_and_unstable_counts() {
        use crate::model::{Constraint, ConstraintKind, ConstraintLine};

        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        state.doc.shape_order.push(ShapeKind::Line);
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 5.0, 10.0, 5.0));
        state.doc.shape_order.push(ShapeKind::Line);
        state.doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Parallel {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Line(0),
            additive: false,
        });
        state.apply(Action::DeleteSelection);
        assert!(state.status.contains("1 invalid"));
        assert!(state.status.contains("1 unstable"));
    }

    #[test]
    fn invalid_constraint_blocks_dim_label_offset() {
        use crate::document_lifecycle::tombstone_element;

        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        state.doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        state.doc.shape_order.push(ShapeKind::Rect);
        add_distance_constraint(
            &mut state.doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "10mm".to_string(),
        )
        .unwrap();
        tombstone_element(&mut state.doc, SceneElement::Rect(0));
        state.refresh_document_health();
        let width_constraint =
            find_distance_constraint(&state.doc, DistanceTarget::RectWidth(0)).unwrap();
        assert_eq!(
            state.apply(Action::SetDimLabelOffset {
                target: width_constraint,
                offset: 55.0,
            }),
            ActionResult::Err("invalid: Dimension target was deleted".to_string())
        );
    }

    #[test]
    fn frozen_unstable_line_blocks_rename_and_vertex_drag() {
        use crate::document_lifecycle::tombstone_element;
        use crate::model::{Constraint, ConstraintKind, ConstraintLine, LineEnd};

        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        state.doc.shape_order.push(ShapeKind::Line);
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 5.0, 10.0, 5.0));
        state.doc.shape_order.push(ShapeKind::Line);
        state.doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Parallel {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        tombstone_element(&mut state.doc, SceneElement::Line(0));
        state.refresh_document_health();
        state.apply(Action::OpenSketch {
            sketch,
            viewport: None,
        });

        assert_eq!(
            state.apply(Action::CommitElementName {
                element: SceneElement::Line(1),
                name: "Partner".to_string(),
            }),
            ActionResult::Err("unstable: Lost parallel/perpendicular partner".to_string())
        );
        assert_eq!(
            state.apply(Action::DragVertex {
                point: ConstraintPoint::LineEndpoint {
                    line: 1,
                    end: LineEnd::Start,
                },
                u: 1.0,
                v: 5.0,
            }),
            ActionResult::Err("unstable: Lost parallel/perpendicular partner".to_string())
        );
    }

    #[test]
    fn undo_last_refreshes_document_health() {
        let mut state = AppState::default();
        state.doc.parameters.push(crate::model::Parameter {
            name: "bad".to_string(),
            expression: "1mm / 0".to_string(),
            deleted: false,
            source: None,
        });
        state.doc.shape_order.push(ShapeKind::Parameter);
        state.refresh_document_health();
        assert_eq!(
            state.document_health.parameter_status(0),
            crate::document_health::HealthStatus::Invalid
        );

        state.apply(Action::UndoLast);
        assert!(state.doc.parameters.is_empty());
        assert!(state.document_health.parameters.is_empty());
    }

    #[test]
    fn open_tombstoned_document_recomputes_health() {
        use crate::document_lifecycle::tombstone_element;
        use crate::model::{Constraint, ConstraintKind, ConstraintLine};

        let dir = std::env::temp_dir();
        let path = dir.join("le3_open_health.le3");
        let path = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);

        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 5.0, 10.0, 5.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Parallel {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        doc.shape_order.push(ShapeKind::Constraint);
        tombstone_element(&mut doc, SceneElement::Line(0));
        crate::storage::save(&path, &doc).unwrap();

        let loaded = crate::storage::open(&path).unwrap();
        assert!(loaded.lines[0].deleted);
        assert!(!loaded.lines[1].deleted);
        let health_after_load = crate::document_health::recompute_document_health(&loaded);
        assert_eq!(
            health_after_load.element_status(SceneElement::Constraint(0)),
            crate::document_health::HealthStatus::Invalid
        );
        assert_eq!(
            health_after_load.element_status(SceneElement::Line(1)),
            crate::document_health::HealthStatus::Unstable
        );

        let mut state = AppState::default();
        state.apply(Action::Open { path: path.clone() });
        assert_eq!(
            state.document_health.element_status(SceneElement::Constraint(0)),
            crate::document_health::HealthStatus::Invalid
        );
        assert_eq!(
            state.document_health.element_status(SceneElement::Line(1)),
            crate::document_health::HealthStatus::Unstable
        );

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn set_shape_construction_toggles_rectangle_edge() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 1.0, 1.0));
        assert_eq!(
            state.apply(Action::SetShapeConstruction {
                element: SceneElement::RectEdge(0, RectEdge::Bottom),
                construction: true,
            }),
            ActionResult::Ok
        );
        assert!(state.doc.rects[0].edge_construction(RectEdge::Bottom));
        assert!(!state.doc.rects[0].edge_construction(RectEdge::Top));
    }

    #[test]
    fn apply_construction_sets_all_selected_targets() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 1.0, 1.0));
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 2.0, 0.0));
        state.apply(Action::ClickSceneElement {
            element: SceneElement::RectEdge(0, RectEdge::Bottom),
            additive: false,
        });
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Line(0),
            additive: true,
        });
        assert_eq!(
            state.apply(Action::ApplyConstruction { construction: true }),
            ActionResult::Ok
        );
        assert!(state.doc.rects[0].edge_construction(RectEdge::Bottom));
        assert!(state.doc.lines[0].construction);
    }

    #[test]
    fn toggle_construction_flips_each_selected_target() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 1.0, 1.0));
        state.doc.rects[0].set_edge_construction(RectEdge::Bottom, true);
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 2.0, 0.0));
        state.apply(Action::ClickSceneElement {
            element: SceneElement::RectEdge(0, RectEdge::Bottom),
            additive: false,
        });
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Line(0),
            additive: true,
        });
        assert_eq!(state.apply(Action::ToggleConstruction), ActionResult::Ok);
        assert!(!state.doc.rects[0].edge_construction(RectEdge::Bottom));
        assert!(state.doc.lines[0].construction);
    }

    #[test]
    fn toggle_construction_rectangle_tool_before_drawing() {
        let mut state = AppState::default();
        state.apply(Action::SetTool(Tool::Rectangle));
        assert_eq!(state.rect_draw_construction_mode(), Some(false));
        assert_eq!(state.apply(Action::ToggleConstruction), ActionResult::Ok);
        assert_eq!(state.rect_draw_construction_mode(), Some(true));
        assert!(state.creating_rect.is_none());
    }

    #[test]
    fn apply_construction_line_tool_before_drawing() {
        let mut state = AppState::default();
        state.apply(Action::SetTool(Tool::Line));
        assert_eq!(
            state.apply(Action::ApplyConstruction { construction: true }),
            ActionResult::Ok
        );
        assert_eq!(state.line_draw_construction_mode(), Some(true));
        assert!(state.creating_line.is_none());
    }

    #[test]
    fn draw_construction_mode_persists_across_rectangle_and_line_tools() {
        let mut state = AppState::default();
        state.apply(Action::SetTool(Tool::Rectangle));
        state.apply(Action::ToggleConstruction);
        assert!(state.draw_construction);
        state.apply(Action::SetTool(Tool::Line));
        assert_eq!(state.line_draw_construction_mode(), Some(true));
        state.apply(Action::SetTool(Tool::Rectangle));
        assert_eq!(state.rect_draw_construction_mode(), Some(true));
    }

    #[test]
    fn toggle_construction_while_drawing_rectangle() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["".to_string(), "".to_string()],
            focused: 0,
            last_mouse: Vec3::new(10.0, 5.0, 0.0),
            user_edited: [false, false],
            pending_focus: false,
            construction: false,
        });
        assert_eq!(state.apply(Action::ToggleConstruction), ActionResult::Ok);
        assert!(state.creating_rect.as_ref().unwrap().construction);
    }

    #[test]
    fn pending_rectangle_draw_mode_applies_on_commit() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.apply(Action::SetTool(Tool::Rectangle));
        state.draw_construction = true;
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["10".to_string(), "5".to_string()],
            focused: 0,
            last_mouse: Vec3::new(10.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: state.draw_construction,
        });
        state.apply(Action::CommitRectangle);
        assert!(state.doc.rects[0].all_edges_construction());
    }

    #[test]
    fn commit_rectangle_with_construction_draw_mode() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["10".to_string(), "5".to_string()],
            focused: 0,
            last_mouse: Vec3::new(10.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: true,
        });
        state.apply(Action::CommitRectangle);
        let rect = &state.doc.rects[0];
        assert!(rect.all_edges_construction());
    }

    #[test]
    fn commit_line_with_construction_draw_mode() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.creating_line = Some(CreatingLine {
            origin: Vec3::ZERO,
            text: "10".to_string(),
            last_mouse: Vec3::new(10.0, 0.0, 0.0),
            user_edited: true,
            pending_focus: false,
            construction: true,
        });
        state.apply(Action::CommitLine);
        assert!(state.doc.lines[0].construction);
    }

    #[test]
    fn toggle_element_visibility() {
        let mut state = AppState::default();
        state.apply(Action::ToggleElementVisibility(SceneElement::Sketch(0)));
        assert!(!state.element_visibility.is_visible(SceneElement::Sketch(0)));
    }

    #[test]
    fn focus_rect_dimension_sets_pending_focus() {
        let mut state = AppState::default();
        state.creating_rect = Some(CreatingRect {
            origin: Vec3::ZERO,
            texts: ["".to_string(), "".to_string()],
            focused: 0,
            last_mouse: Vec3::ZERO,
            user_edited: [false, false],
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::FocusRectDimension {
            axis: RectAxis::Height,
        });
        let cr = state.creating_rect.as_ref().unwrap();
        assert_eq!(cr.focused, 1);
        assert!(cr.pending_focus);
    }

    #[test]
    fn focus_line_length_sets_pending_focus() {
        let mut state = AppState::default();
        state.creating_line = Some(CreatingLine {
            origin: Vec3::ZERO,
            text: String::new(),
            last_mouse: Vec3::ZERO,
            user_edited: false,
            pending_focus: false,
            construction: false,
        });
        state.apply(Action::FocusLineLength);
        assert!(state.creating_line.as_ref().unwrap().pending_focus);
    }

    #[test]
    fn view_cube_pane_visible_by_default() {
        let state = AppState::default();
        assert!(state.panes.is_visible(Pane::ViewCube));
    }

    #[test]
    fn toggle_pane_hides_then_shows() {
        let mut state = AppState::default();
        state.apply(Action::TogglePane(Pane::ViewCube));
        assert!(!state.panes.is_visible(Pane::ViewCube));
        state.apply(Action::TogglePane(Pane::ViewCube));
        assert!(state.panes.is_visible(Pane::ViewCube));
    }

    #[test]
    fn toggle_command_palette_opens_and_closes() {
        let mut state = AppState::default();
        assert!(!state.command_palette.open);
        state.apply(Action::ToggleCommandPalette);
        assert!(state.command_palette.open);
        state.apply(Action::SetCommandPaletteOpen { open: false });
        assert!(!state.command_palette.open);
    }

    #[test]
    fn set_pane_visible_is_explicit() {
        let mut state = AppState::default();
        state.apply(Action::SetPaneVisible {
            pane: Pane::ViewCube,
            visible: false,
        });
        assert!(!state.panes.is_visible(Pane::ViewCube));
        // Setting the same value again is idempotent.
        state.apply(Action::SetPaneVisible {
            pane: Pane::ViewCube,
            visible: false,
        });
        assert!(!state.panes.is_visible(Pane::ViewCube));
    }

    #[test]
    fn set_home_view_action_stores_current_camera_pose() {
        let mut state = AppState::default();
        state.cam.target = Vec3::new(5.0, -3.0, 2.0);
        state.cam.yaw = 0.9;
        state.cam.pitch = 0.4;
        state.cam.distance = 180.0;
        state.apply(Action::SetHomeView);
        let home = state.cam.home_view();
        assert!((home.target.x - 5.0).abs() < 1e-4);
        assert!((home.yaw - 0.9).abs() < 1e-4);
        assert_eq!(state.status, "Home view set");
    }

    #[test]
    fn orbit_changes_camera() {
        let mut state = AppState::default();
        let yaw = state.cam.yaw;
        state.apply(Action::OrbitCamera { delta: (10.0, 5.0) });
        assert_ne!(state.cam.yaw, yaw);
    }

    #[test]
    fn commit_element_name_updates_document() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 1.0, 0.0));
        assert_eq!(
            state.apply(Action::CommitElementName {
                element: SceneElement::Line(0),
                name: "Guide".to_string(),
            }),
            ActionResult::Ok
        );
        assert_eq!(state.doc.lines[0].name.as_deref(), Some("Guide"));
    }

    #[test]
    fn focus_element_name_requires_single_selection() {
        let mut state = AppState::default();
        assert_eq!(
            state.apply(Action::FocusElementName),
            ActionResult::Err("Select a single element to rename".to_string())
        );
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 1.0, 0.0));
        state.apply(Action::ClickSceneElement {
            element: SceneElement::Line(0),
            additive: false,
        });
        assert_eq!(state.apply(Action::FocusElementName), ActionResult::Ok);
        assert!(state.context_pane.focus_name_field);
    }
}