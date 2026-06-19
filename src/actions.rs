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
use crate::hierarchy::SceneElement;
use crate::hierarchy::ElementVisibility;
use crate::model::SketchId;
use crate::view_cube::{self, CubeCornerId, CubeEdgeId};
use crate::model::{ConstructionPlane, Document, FaceId, Line, Rect, ShapeKind};
use crate::face::SketchFrame;
use crate::parameters::{
    add_parameter, delete_parameter, recompute_document_geometry, set_parameter_expression,
    set_parameter_name, ParametersPaneState,
};
use crate::value::{eval_length_mm_in_doc, format_length_display, parse_positive_length_or_in_doc};
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
    /// Click a face or axis/line, then set offset (and angle for axes); Enter commits.
    ConstructionPlane,
    /// Pick a face to enter sketch mode; line/rectangle tools draw on that face.
    Sketch,
}

impl Tool {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "select" => Some(Tool::Select),
            "rectangle" | "rect" => Some(Tool::Rectangle),
            "line" => Some(Tool::Line),
            "plane" | "construction_plane" | "constructionplane" | "construction plane" => {
                Some(Tool::ConstructionPlane)
            }
            "sketch" => Some(Tool::Sketch),
            _ => None,
        }
    }

    pub fn is_sketch_draw_tool(self) -> bool {
        matches!(self, Tool::Rectangle | Tool::Line)
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
    SetDimLabelOffset {
        target: DimLabelTarget,
        offset: f32,
    },
    BeginEditCommittedDim { target: DimLabelTarget },
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
    CommitParameterName { index: usize, name: String },
    CommitParameterExpression { index: usize, expression: String },
    DeleteParameter { index: usize },
    SetCommandPaletteOpen { open: bool },
    ToggleCommandPalette,
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
}

impl Pane {
    /// All panes, in menu order.
    pub const ALL: &'static [Pane] = &[Pane::Hierarchy, Pane::Parameters, Pane::ViewCube];

    /// Human-readable label for menus.
    pub fn label(self) -> &'static str {
        match self {
            Pane::ViewCube => "Orientation Cube",
            Pane::Hierarchy => "Tree",
            Pane::Parameters => "Parameters",
        }
    }

    /// Stable name used in instruction scripts.
    pub fn script_name(self) -> &'static str {
        match self {
            Pane::ViewCube => "view_cube",
            Pane::Hierarchy => "hierarchy",
            Pane::Parameters => "parameters",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "view_cube" | "viewcube" | "cube" | "hud" => Some(Pane::ViewCube),
            "hierarchy" | "tree" | "dag" => Some(Pane::Hierarchy),
            "parameters" | "params" | "param" => Some(Pane::Parameters),
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
}

impl Default for PaneVisibility {
    fn default() -> Self {
        Self {
            view_cube: true,
            hierarchy: true,
            parameters: true,
        }
    }
}

impl PaneVisibility {
    pub fn is_visible(&self, pane: Pane) -> bool {
        match pane {
            Pane::ViewCube => self.view_cube,
            Pane::Hierarchy => self.hierarchy,
            Pane::Parameters => self.parameters,
        }
    }

    pub fn set(&mut self, pane: Pane, visible: bool) {
        match pane {
            Pane::ViewCube => self.view_cube = visible,
            Pane::Hierarchy => self.hierarchy = visible,
            Pane::Parameters => self.parameters = visible,
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
    match axis {
        DimLabelAxis::Width => doc
            .rects
            .iter()
            .enumerate()
            .rev()
            .find(|(_, r)| r.sketch == sketch && r.width_locked)
            .map(|(index, _)| DimLabelTarget::RectWidth { index }),
        DimLabelAxis::Height => doc
            .rects
            .iter()
            .enumerate()
            .rev()
            .find(|(_, r)| r.sketch == sketch && r.height_locked)
            .map(|(index, _)| DimLabelTarget::RectHeight { index }),
        DimLabelAxis::Length => doc
            .lines
            .iter()
            .enumerate()
            .rev()
            .find(|(_, l)| l.sketch == sketch && l.length_locked)
            .map(|(index, _)| DimLabelTarget::LineLength { index }),
    }
}

/// A committed sketch dimension label the user can reposition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DimLabelTarget {
    RectWidth { index: usize },
    RectHeight { index: usize },
    LineLength { index: usize },
}

impl DimLabelTarget {
    pub fn matches_rect_axis(self, axis: RectAxis) -> bool {
        matches!(
            (self, axis),
            (DimLabelTarget::RectWidth { .. }, RectAxis::Width)
                | (DimLabelTarget::RectHeight { .. }, RectAxis::Height)
        )
    }

    pub fn is_line_length(self) -> bool {
        matches!(self, DimLabelTarget::LineLength { .. })
    }
}

/// In-progress edit of a committed sketch dimension (Select tool).
#[derive(Clone, Debug, PartialEq)]
pub struct EditingCommittedDim {
    pub target: DimLabelTarget,
    pub text: String,
    pub pending_focus: bool,
}

/// Expression text shown when editing a committed dimension.
pub fn committed_dim_expression(doc: &Document, target: DimLabelTarget) -> Option<String> {
    match target {
        DimLabelTarget::RectWidth { index } => {
            let rect = doc.rects.get(index)?;
            if !rect.width_locked {
                return None;
            }
            Some(
                rect.width_expr
                    .clone()
                    .unwrap_or_else(|| format_length_display(rect.w)),
            )
        }
        DimLabelTarget::RectHeight { index } => {
            let rect = doc.rects.get(index)?;
            if !rect.height_locked {
                return None;
            }
            Some(
                rect.height_expr
                    .clone()
                    .unwrap_or_else(|| format_length_display(rect.h)),
            )
        }
        DimLabelTarget::LineLength { index } => {
            let line = doc.lines.get(index)?;
            if !line.length_locked {
                return None;
            }
            Some(
                line.length_expr
                    .clone()
                    .unwrap_or_else(|| format_length_display(line.length())),
            )
        }
    }
}

fn apply_committed_dim_expression(
    doc: &mut Document,
    target: DimLabelTarget,
    expression: &str,
) -> Result<(), String> {
    let trimmed = expression.trim();
    if trimmed.is_empty() {
        return Err("Dimension value cannot be empty".to_string());
    }
    let value = eval_length_mm_in_doc(trimmed, doc)
        .ok_or_else(|| format!("Invalid dimension expression '{trimmed}'"))?;
    if value <= 0.0 {
        return Err(format!("Dimension expression '{trimmed}' must be positive"));
    }
    match target {
        DimLabelTarget::RectWidth { index } => {
            let rect = doc
                .rects
                .get_mut(index)
                .ok_or_else(|| format!("Rectangle {index} not found"))?;
            if !rect.width_locked {
                return Err("Rectangle width is not dimensioned".to_string());
            }
            rect.width_expr = Some(trimmed.to_string());
        }
        DimLabelTarget::RectHeight { index } => {
            let rect = doc
                .rects
                .get_mut(index)
                .ok_or_else(|| format!("Rectangle {index} not found"))?;
            if !rect.height_locked {
                return Err("Rectangle height is not dimensioned".to_string());
            }
            rect.height_expr = Some(trimmed.to_string());
        }
        DimLabelTarget::LineLength { index } => {
            let line = doc
                .lines
                .get_mut(index)
                .ok_or_else(|| format!("Line {index} not found"))?;
            if !line.length_locked {
                return Err("Line length is not dimensioned".to_string());
            }
            line.length_expr = Some(trimmed.to_string());
        }
    }
    recompute_document_geometry(doc)
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
    pub creating_plane: Option<CreatingConstructionPlane>,
    pub panes: PaneVisibility,
    pub parameters_pane: ParametersPaneState,
    pub command_palette: CommandPaletteState,
    pub element_visibility: ElementVisibility,
    pub editing_committed_dim: Option<EditingCommittedDim>,
    pub status: String,
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
            creating_plane: None,
            panes: PaneVisibility::default(),
            parameters_pane: ParametersPaneState::default(),
            command_palette: CommandPaletteState::default(),
            element_visibility: ElementVisibility::default(),
            editing_committed_dim: None,
            status: String::new(),
        }
    }
}

fn pane_status(pane: Pane, visible: bool) -> String {
    format!("{} {}", pane.label(), if visible { "shown" } else { "hidden" })
}

fn element_label(element: SceneElement) -> String {
    match element {
        SceneElement::ConstructionPlane(i) => format!("Construction plane {i}"),
        SceneElement::Sketch(i) => format!("Sketch {i}"),
        SceneElement::Rect(i) => format!("Rectangle {i}"),
        SceneElement::Line(i) => format!("Line {i}"),
    }
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
    pub fn apply(&mut self, action: Action) -> ActionResult {
        match action {
            Action::NewDocument => {
                self.doc = Document::default();
                self.path = None;
                self.sketch_session = None;
                self.cam.set_view_up(None);
                self.creating_rect = None;
                self.creating_line = None;
                self.creating_plane = None;
                self.element_visibility = ElementVisibility::default();
                self.tool = Tool::Select;
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
            Action::Clear => {
                self.doc = Document::default();
                self.sketch_session = None;
                self.cam.set_view_up(None);
                self.creating_rect = None;
                self.creating_line = None;
                self.element_visibility = ElementVisibility::default();
                self.status = "Cleared".to_string();
                ActionResult::Ok
            }
            Action::UndoLast => {
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
                        }
                    }
                    Some(ShapeKind::Rect) => {
                        self.doc.rects.pop();
                        self.status = "Undid last rectangle".to_string();
                    }
                    Some(ShapeKind::Line) => {
                        self.doc.lines.pop();
                        self.status = "Undid last line".to_string();
                    }
                    Some(ShapeKind::Parameter) => {
                        self.doc.parameters.pop();
                        self.status = "Undid last parameter".to_string();
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
                            }
                        }
                    }
                    None => self.status = "Nothing to undo".to_string(),
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
                if self.creating_plane.is_some() && tool != Tool::ConstructionPlane {
                    self.creating_plane = None;
                }
                if tool != Tool::Select {
                    self.editing_committed_dim = None;
                }
                self.tool = tool;
                self.status = match tool {
                    Tool::Select => "Select tool".to_string(),
                    Tool::Sketch => "Sketch tool — click a face".to_string(),
                    Tool::Rectangle if self.sketch_session.is_some() => {
                        "Rectangle tool".to_string()
                    }
                    Tool::Rectangle => "Rectangle tool — click a face".to_string(),
                    Tool::Line if self.sketch_session.is_some() => "Line tool".to_string(),
                    Tool::Line => "Line tool — click a face".to_string(),
                    Tool::ConstructionPlane => "Construction plane tool".to_string(),
                };
                ActionResult::Ok
            }
            Action::CancelOperation => {
                if self.editing_committed_dim.take().is_some() {
                    self.status = "Cancelled".to_string();
                } else if self.creating_rect.take().is_some()
                    || self.creating_line.take().is_some()
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
                        self.tool = Tool::Select;
                        self.status = "Select tool".to_string();
                    }
                } else if self.tool != Tool::Select {
                    self.tool = Tool::Select;
                    self.status = "Select tool".to_string();
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
                let Some(cr) = self.creating_rect.take() else {
                    return ActionResult::Err("No rectangle in progress".to_string());
                };
                let (ou, ov) = world_to_local(&frame, cr.origin);
                let end = cr.end_point(&frame, &self.doc);
                let (eu, ev) = world_to_local(&frame, end);
                let mut rect = Rect::from_local_corners(session.sketch, ou, ov, eu, ev);
                rect.width_locked = cr.user_edited[0];
                rect.height_locked = cr.user_edited[1];
                rect.width_expr = cr.user_edited[0].then(|| cr.texts[0].clone());
                rect.height_expr = cr.user_edited[1].then(|| cr.texts[1].clone());
                if rect.w > 0.5 && rect.h > 0.5 {
                    self.doc.rects.push(rect);
                    self.doc.shape_order.push(ShapeKind::Rect);
                    if let Err(e) = recompute_document_geometry(&mut self.doc) {
                        self.doc.rects.pop();
                        self.doc.shape_order.pop();
                        self.creating_rect = Some(cr);
                        self.status = e.clone();
                        return ActionResult::Err(e);
                    }
                    let rect = self.doc.rects.last().unwrap();
                    let (w, h) = (rect.w, rect.h);
                    self.status = format!("Added rectangle ({w:.1} × {h:.1} mm)");
                    ActionResult::Ok
                } else {
                    self.creating_rect = Some(cr);
                    self.status = "Rectangle too small".to_string();
                    ActionResult::Err("Rectangle too small".to_string())
                }
            }
            Action::SetRectDimension { axis, value } => {
                if let Some(edit) = &mut self.editing_committed_dim {
                    if edit.target.matches_rect_axis(axis) {
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
                    if edit.target.matches_rect_axis(axis) {
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
                let Some(cl) = self.creating_line.take() else {
                    return ActionResult::Err("No line in progress".to_string());
                };
                let (u0, v0) = world_to_local(&frame, cl.origin);
                let end = cl.end_point(&frame, &self.doc);
                let (u1, v1) = world_to_local(&frame, end);
                let mut line = Line::from_local_endpoints(session.sketch, u0, v0, u1, v1);
                line.length_locked = cl.user_edited;
                line.length_expr = cl.user_edited.then(|| cl.text.clone());
                if line.length() > 0.5 {
                    self.doc.lines.push(line);
                    self.doc.shape_order.push(ShapeKind::Line);
                    if let Err(e) = recompute_document_geometry(&mut self.doc) {
                        self.doc.lines.pop();
                        self.doc.shape_order.pop();
                        self.creating_line = Some(cl);
                        self.status = e.clone();
                        return ActionResult::Err(e);
                    }
                    let len = self.doc.lines.last().unwrap().length();
                    self.status = format!("Added line ({:.1} mm)", len);
                    ActionResult::Ok
                } else {
                    self.creating_line = Some(cl);
                    self.status = "Line too short".to_string();
                    ActionResult::Err("Line too short".to_string())
                }
            }
            Action::SetLineLength { value } => {
                if let Some(edit) = &mut self.editing_committed_dim {
                    if edit.target.is_line_length() {
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
                let offset = crate::dimensions::effective_dim_offset(Some(offset));
                match target {
                    DimLabelTarget::RectWidth { index } => {
                        let Some(rect) = self.doc.rects.get_mut(index) else {
                            return ActionResult::Err(format!("Rectangle {index} not found"));
                        };
                        if !rect.width_locked {
                            return ActionResult::Err("Rectangle width is not dimensioned".to_string());
                        }
                        rect.width_dim_offset = Some(offset);
                    }
                    DimLabelTarget::RectHeight { index } => {
                        let Some(rect) = self.doc.rects.get_mut(index) else {
                            return ActionResult::Err(format!("Rectangle {index} not found"));
                        };
                        if !rect.height_locked {
                            return ActionResult::Err("Rectangle height is not dimensioned".to_string());
                        }
                        rect.height_dim_offset = Some(offset);
                    }
                    DimLabelTarget::LineLength { index } => {
                        let Some(line) = self.doc.lines.get_mut(index) else {
                            return ActionResult::Err(format!("Line {index} not found"));
                        };
                        if !line.length_locked {
                            return ActionResult::Err("Line length is not dimensioned".to_string());
                        }
                        line.length_dim_offset = Some(offset);
                    }
                }
                ActionResult::Ok
            }
            Action::FocusLineLength => {
                if let Some(edit) = &mut self.editing_committed_dim {
                    if edit.target.is_line_length() {
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
            Action::BeginEditCommittedDim { target } => {
                if self.sketch_session.is_none() {
                    return ActionResult::Err("Not in sketch mode".to_string());
                }
                if self.tool != Tool::Select {
                    return ActionResult::Err("Select tool required to edit dimensions".to_string());
                }
                let Some(text) = committed_dim_expression(&self.doc, target) else {
                    return ActionResult::Err("Dimension is not editable".to_string());
                };
                self.editing_committed_dim = Some(EditingCommittedDim {
                    target,
                    text,
                    pending_focus: true,
                });
                self.status = "Edit dimension • Enter to commit • Esc to cancel".to_string();
                ActionResult::Ok
            }
            Action::CommitCommittedDim => {
                let Some(edit) = self.editing_committed_dim.take() else {
                    return ActionResult::Err("No committed dimension being edited".to_string());
                };
                match apply_committed_dim_expression(&mut self.doc, edit.target, &edit.text) {
                    Ok(()) => {
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
                let Some(cp) = self.creating_plane.take() else {
                    return ActionResult::Err("No construction plane in progress".to_string());
                };
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
            Action::CommitParameterName { index, name } => {
                match set_parameter_name(&mut self.doc, index, name.clone()) {
                    Ok(()) => {
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
                match set_parameter_expression(&mut self.doc, index, expression.clone()) {
                    Ok(()) => {
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
                        self.status = "Deleted parameter".to_string();
                        ActionResult::Ok
                    }
                    Err(e) => {
                        self.status = e.clone();
                        ActionResult::Err(e)
                    }
                }
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
        }
    }

    fn exit_sketch_session(&mut self) {
        self.sketch_session = None;
        self.creating_rect = None;
        self.creating_line = None;
        self.editing_committed_dim = None;
        self.cam.leave_sketch_mode();
        self.tool = Tool::Select;
    }

    fn enter_sketch(
        &mut self,
        sketch: SketchId,
        viewport: Option<egui::Rect>,
        frame_padding_px: Option<f32>,
    ) -> ActionResult {
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
            let zoom_distance = frame_target.zoom.and_then(|bounds| {
                let frame = sketch_frame(&self.doc, face)?;
                let vp = viewport?;
                let padding = frame_padding_px?;
                let corners = bounds.world_corners(&frame);
                Some(self.cam.distance_to_fit_corners(
                    frame_target.target,
                    view_direction,
                    &corners,
                    padding,
                    vp,
                ))
            });
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
        if !self.tool.is_sketch_draw_tool() {
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
        });
        state.apply(Action::CommitRectangle);
        assert_eq!(state.doc.rects.len(), 1);
        assert!((state.doc.rects[0].w - 10.0).abs() < 1e-4);
        assert!((state.doc.rects[0].h - 5.0).abs() < 1e-4);
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
        });
        state.apply(Action::CommitRectangle);
        let rect = &state.doc.rects[0];
        assert!(!rect.width_locked);
        assert!(!rect.height_locked);
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
        });
        state.apply(Action::CommitRectangle);
        state.apply(Action::SetDimLabelOffset {
            target: DimLabelTarget::RectWidth { index: 0 },
            offset: 55.0,
        });
        assert_eq!(state.doc.rects[0].width_dim_offset, Some(55.0));
    }

    #[test]
    fn dim_label_target_in_sketch_finds_locked_width() {
        let mut state = AppState::default();
        let sketch = begin_default_sketch(&mut state);
        let mut rect = Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0);
        rect.width_locked = true;
        state.doc.rects.push(rect);
        state.doc.shape_order.push(ShapeKind::Rect);
        let target = dim_label_target_in_sketch(&state.doc, sketch, DimLabelAxis::Width);
        assert_eq!(target, Some(DimLabelTarget::RectWidth { index: 0 }));
    }

    #[test]
    fn begin_edit_committed_dim_requires_select_tool() {
        let mut state = AppState::default();
        begin_default_sketch(&mut state);
        state.apply(Action::SetTool(Tool::Rectangle));
        let result = state.apply(Action::BeginEditCommittedDim {
            target: DimLabelTarget::RectWidth { index: 0 },
        });
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
        });
        state.apply(Action::CommitRectangle);
        state.apply(Action::BeginEditCommittedDim {
            target: DimLabelTarget::RectWidth { index: 0 },
        });
        state.apply(Action::SetRectDimension {
            axis: RectAxis::Width,
            value: "20mm".to_string(),
        });
        state.apply(Action::CommitCommittedDim);
        assert!((state.doc.rects[0].w - 20.0).abs() < 1e-3);
        assert_eq!(state.doc.rects[0].width_expr.as_deref(), Some("20mm"));
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
        });
        state.apply(Action::CommitRectangle);
        state.apply(Action::BeginEditCommittedDim {
            target: DimLabelTarget::RectWidth { index: 0 },
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
        });
        state.apply(Action::CommitLine);
        assert_eq!(state.doc.lines.len(), 1);
        assert!((state.doc.lines[0].length() - 10.0).abs() < 1e-4);
        assert!(state.doc.lines[0].length_locked);
        assert!(state.creating_line.is_none());
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
    fn exit_sketch_preserves_camera_view() {
        let mut state = AppState::default();
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        while state.cam.tick_transition(0.05) {}
        let viewport =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let samples = [
            Vec3::ZERO,
            Vec3::new(40.0, 20.0, 0.0),
            Vec3::new(-30.0, 50.0, 10.0),
        ];
        let vp_before = state.cam.view_proj(viewport);
        let screens_before: Vec<_> = samples
            .iter()
            .map(|p| state.cam.project(*p, viewport, &vp_before).unwrap())
            .collect();

        state.apply(Action::ExitSketch);

        let vp_after = state.cam.view_proj(viewport);
        for (p, before) in samples.iter().zip(screens_before) {
            let after = state.cam.project(*p, viewport, &vp_after).unwrap();
            assert!(
                (before - after).length() < 0.5,
                "exiting sketch should not move the camera: {before:?} -> {after:?} for {p:?}"
            );
        }
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
        let fitted = state.cam.distance_to_fit_corners(
            state.cam.target,
            view,
            &corners,
            SKETCH_EDIT_FRAME_PADDING_PX,
            viewport,
        );
        assert!((state.cam.distance - fitted).abs() < 1.0);
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
    fn tree_pane_visible_by_default() {
        let state = AppState::default();
        assert!(state.panes.is_visible(Pane::Hierarchy));
        assert_eq!(Pane::Hierarchy.label(), "Tree");
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
}