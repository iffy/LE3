//! Shared action layer (SPEC §8, §9, §11.2).
//!
//! GUI buttons, keyboard shortcuts, and instruction scripts all dispatch the
//! same [`Action`] values so behaviour stays in sync.

use crate::camera::{
    Camera, ProjectionMode, StandardView, SKETCH_FRAME_PADDING_PX, VIEW_TRANSITION_DURATION,
};
use crate::construction::{
    resolve_plane, AxisGizmoDrag, PlaneDim, PlaneReference,
};
use crate::face::{face_label, sketch_camera_target, sketch_frame, world_to_local};
use crate::view_cube::{self, CubeCornerId, CubeEdgeId};
use crate::model::{ConstructionPlane, Document, FaceId, Line, Rect, ShapeKind};
use crate::face::SketchFrame;
use crate::value::parse_positive_length_or;
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
    pub fn end_point(&self, frame: &SketchFrame) -> Vec3 {
        let (ou, ov) = world_to_local(frame, self.origin);
        let (mu, mv) = world_to_local(frame, self.last_mouse);
        let du = mu - ou;
        let dv = mv - ov;
        let w = parse_positive_length_or(&self.texts[0], du.abs());
        let h = parse_positive_length_or(&self.texts[1], dv.abs());
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
    pub fn end_point(&self, frame: &SketchFrame) -> Vec3 {
        let (ou, ov) = world_to_local(frame, self.origin);
        let (mu, mv) = world_to_local(frame, self.last_mouse);
        let du = mu - ou;
        let dv = mv - ov;
        let dist = (du * du + dv * dv).sqrt();
        let len = parse_positive_length_or(&self.text, dist);
        if dist < 1e-6 {
            return crate::face::local_to_world(frame, ou + len, ov);
        }
        let scale = len / dist;
        crate::face::local_to_world(frame, ou + du * scale, ov + dv * scale)
    }
}

/// State for the in-progress construction-plane creation.
#[derive(Clone, Debug)]
pub struct CreatingConstructionPlane {
    pub reference: PlaneReference,
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
    BeginConstructionPlane { reference: PlaneReference },
    CommitConstructionPlane,
    SetPlaneOffset { value: String },
    SetPlaneAngle { value: String },
    FocusPlaneDim { dim: PlaneDim },
    BeginSketch {
        face: FaceId,
        viewport: Option<egui::Rect>,
    },
    ExitSketch,
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
    SetProjectionMode(ProjectionMode),
    ToggleProjectionMode,
    SetPaneVisible { pane: Pane, visible: bool },
    TogglePane(Pane),
}

/// A toggleable UI pane (SPEC §11.1). For now only the orientation HUD cube
/// exists; this grows as the viewport, parameters, history, etc. panes land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// The rotating orientation cube in the viewport corner ([`view_cube`]).
    ViewCube,
}

impl Pane {
    /// All panes, in menu order.
    pub const ALL: &'static [Pane] = &[Pane::ViewCube];

    /// Human-readable label for menus.
    pub fn label(self) -> &'static str {
        match self {
            Pane::ViewCube => "Orientation Cube",
        }
    }

    /// Stable name used in instruction scripts.
    pub fn script_name(self) -> &'static str {
        match self {
            Pane::ViewCube => "view_cube",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "view_cube" | "viewcube" | "cube" | "hud" => Some(Pane::ViewCube),
            _ => None,
        }
    }
}

/// Which panes are currently shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneVisibility {
    pub view_cube: bool,
}

impl Default for PaneVisibility {
    fn default() -> Self {
        Self { view_cube: true }
    }
}

impl PaneVisibility {
    pub fn is_visible(&self, pane: Pane) -> bool {
        match pane {
            Pane::ViewCube => self.view_cube,
        }
    }

    pub fn set(&mut self, pane: Pane, visible: bool) {
        match pane {
            Pane::ViewCube => self.view_cube = visible,
        }
    }

    pub fn toggle(&mut self, pane: Pane) {
        let next = !self.is_visible(pane);
        self.set(pane, next);
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

/// Active sketch session: drawings are parented to this face until exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SketchSession {
    pub face: FaceId,
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
            status: String::new(),
        }
    }
}

fn pane_status(pane: Pane, visible: bool) -> String {
    format!("{} {}", pane.label(), if visible { "shown" } else { "hidden" })
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
                self.creating_rect = None;
                self.creating_line = None;
                self.creating_plane = None;
                self.tool = Tool::Select;
                self.status = "New document".to_string();
                ActionResult::Ok
            }
            Action::Open { path } => match crate::storage::open(&path) {
                Ok(doc) => {
                    let n_rects = doc.rects.len();
                    let n_lines = doc.lines.len();
                    self.doc = doc;
                    self.sketch_session = None;
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
                self.creating_rect = None;
                self.creating_line = None;
                self.status = "Cleared".to_string();
                ActionResult::Ok
            }
            Action::UndoLast => {
                match self.doc.shape_order.pop() {
                    Some(ShapeKind::Rect) => {
                        self.doc.rects.pop();
                        self.status = "Undid last rectangle".to_string();
                    }
                    Some(ShapeKind::Line) => {
                        self.doc.lines.pop();
                        self.status = "Undid last line".to_string();
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
                                if self.sketch_session == Some(SketchSession { face }) {
                                    self.sketch_session = None;
                                    self.tool = Tool::Select;
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
                if self.creating_rect.take().is_some()
                    || self.creating_line.take().is_some()
                    || self.creating_plane.take().is_some()
                {
                    self.status = "Cancelled".to_string();
                } else if self.sketch_session.take().is_some() {
                    self.creating_rect = None;
                    self.creating_line = None;
                    self.tool = Tool::Select;
                    self.status = "Exited sketch".to_string();
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
                if let Some(frame_target) = sketch_camera_target(&self.doc, face) {
                    let view_direction = self.cam.visible_face_view_direction(
                        frame_target.target,
                        frame_target.face_normal,
                    );
                    let zoom_distance = frame_target.zoom.and_then(|bounds| {
                        let frame = sketch_frame(&self.doc, face)?;
                        let vp = viewport?;
                        let corners = bounds.world_corners(&frame);
                        Some(self.cam.distance_to_fit_corners(
                            frame_target.target,
                            view_direction,
                            &corners,
                            SKETCH_FRAME_PADDING_PX,
                            vp,
                        ))
                    });
                    self.cam.start_sketch_view_transition(
                        frame_target.target,
                        frame_target.face_normal,
                        zoom_distance,
                        VIEW_TRANSITION_DURATION,
                    );
                }
                self.sketch_session = Some(SketchSession { face });
                self.creating_rect = None;
                self.creating_line = None;
                if !self.tool.is_sketch_draw_tool() {
                    self.tool = Tool::Select;
                }
                let face_name = face_label(&self.doc, face);
                self.status = match self.tool {
                    Tool::Rectangle => format!(
                        "Sketching on {face_name} — click to set corner"
                    ),
                    Tool::Line => format!("Sketching on {face_name} — click to set start"),
                    _ => format!("Sketching on {face_name} — pick line or rectangle"),
                };
                ActionResult::Ok
            }
            Action::ExitSketch => {
                if self.sketch_session.take().is_none() {
                    return ActionResult::Err("Not in sketch mode".to_string());
                }
                self.creating_rect = None;
                self.creating_line = None;
                self.tool = Tool::Select;
                self.status = "Sketch saved".to_string();
                ActionResult::Ok
            }
            Action::CommitRectangle => {
                let Some(session) = self.sketch_session else {
                    return ActionResult::Err("Not in sketch mode".to_string());
                };
                let Some(frame) = sketch_frame(&self.doc, session.face) else {
                    return ActionResult::Err("Sketch face no longer exists".to_string());
                };
                let Some(cr) = self.creating_rect.take() else {
                    return ActionResult::Err("No rectangle in progress".to_string());
                };
                let (ou, ov) = world_to_local(&frame, cr.origin);
                let end = cr.end_point(&frame);
                let (eu, ev) = world_to_local(&frame, end);
                let rect = Rect::from_local_corners(session.face, ou, ov, eu, ev);
                if rect.w > 0.5 && rect.h > 0.5 {
                    self.doc.rects.push(rect);
                    self.doc.shape_order.push(ShapeKind::Rect);
                    self.status = format!("Added rectangle ({:.1} × {:.1} mm)", rect.w, rect.h);
                    ActionResult::Ok
                } else {
                    self.creating_rect = Some(cr);
                    self.status = "Rectangle too small".to_string();
                    ActionResult::Err("Rectangle too small".to_string())
                }
            }
            Action::SetRectDimension { axis, value } => {
                let Some(cr) = &mut self.creating_rect else {
                    return ActionResult::Err("No rectangle in progress".to_string());
                };
                let idx = axis.index();
                cr.texts[idx] = value;
                cr.user_edited[idx] = true;
                ActionResult::Ok
            }
            Action::FocusRectDimension { axis } => {
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
                let Some(frame) = sketch_frame(&self.doc, session.face) else {
                    return ActionResult::Err("Sketch face no longer exists".to_string());
                };
                let Some(cl) = self.creating_line.take() else {
                    return ActionResult::Err("No line in progress".to_string());
                };
                let (u0, v0) = world_to_local(&frame, cl.origin);
                let end = cl.end_point(&frame);
                let (u1, v1) = world_to_local(&frame, end);
                let line = Line::from_local_endpoints(session.face, u0, v0, u1, v1);
                if line.length() > 0.5 {
                    let len = line.length();
                    self.doc.lines.push(line);
                    self.doc.shape_order.push(ShapeKind::Line);
                    self.status = format!("Added line ({:.1} mm)", len);
                    ActionResult::Ok
                } else {
                    self.creating_line = Some(cl);
                    self.status = "Line too short".to_string();
                    ActionResult::Err("Line too short".to_string())
                }
            }
            Action::SetLineLength { value } => {
                let Some(cl) = &mut self.creating_line else {
                    return ActionResult::Err("No line in progress".to_string());
                };
                cl.text = value;
                cl.user_edited = true;
                ActionResult::Ok
            }
            Action::FocusLineLength => {
                let Some(cl) = &mut self.creating_line else {
                    return ActionResult::Err("No line in progress".to_string());
                };
                cl.pending_focus = true;
                ActionResult::Ok
            }
            Action::BeginConstructionPlane { reference } => {
                self.creating_plane = Some(CreatingConstructionPlane {
                    reference,
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
                self.status = "Set offset • type to lock • Tab cycle dims • click/Enter commit • Esc cancel"
                    .to_string();
                ActionResult::Ok
            }
            Action::CommitConstructionPlane => {
                let Some(cp) = self.creating_plane.take() else {
                    return ActionResult::Err("No construction plane in progress".to_string());
                };
                let plane = cp.preview_plane();
                let (live_offset, _) = cp.live_dims();
                self.doc.construction_planes.push(plane);
                self.doc.shape_order.push(ShapeKind::ConstructionPlane);
                self.status = format!(
                    "Added construction plane ({:.1} mm from {})",
                    live_offset,
                    cp.reference.label()
                );
                ActionResult::Ok
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
        }
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

    fn begin_default_sketch(state: &mut AppState) {
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
    }

    #[test]
    fn new_document_clears_state() {
        let mut state = AppState::default();
        state.doc.rects.push(Rect::from_local_corners(
            FaceId::ConstructionPlane(0),
            0.,
            0.,
            10.,
            10.,
        ));
        state.doc.lines.push(Line::from_local_endpoints(
            FaceId::ConstructionPlane(0),
            0.,
            0.,
            1.,
            0.,
        ));
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
    fn commit_construction_plane_adds_to_document_not_export_list() {
        let mut state = AppState::default();
        state.apply(Action::BeginConstructionPlane {
            reference: PlaneReference::Face {
                origin: Vec3::ZERO,
                normal: Vec3::Z,
                label: "Ground".to_string(),
            },
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
        state.doc.rects.push(Rect::from_local_corners(
            FaceId::ConstructionPlane(0),
            0.,
            0.,
            1.,
            1.,
        ));
        state.doc.shape_order.push(ShapeKind::Rect);
        state.doc.lines.push(Line::from_local_endpoints(
            FaceId::ConstructionPlane(0),
            0.,
            0.,
            5.,
            0.,
        ));
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
        begin_default_sketch(&mut state);
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
        assert_eq!(state.doc.rects[0].parent, FaceId::ConstructionPlane(0));
        assert!(state.creating_rect.is_none());
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
        let end = cr.end_point(&frame);
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
        let end = cl.end_point(&frame);
        let (u0, v0) = world_to_local(&frame, cl.origin);
        let (u1, v1) = world_to_local(&frame, end);
        let line = Line::from_local_endpoints(FaceId::default(), u0, v0, u1, v1);
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
        let end = cl.end_point(&frame);
        let (u0, v0) = world_to_local(&frame, cl.origin);
        let (u1, v1) = world_to_local(&frame, end);
        let line = Line::from_local_endpoints(FaceId::default(), u0, v0, u1, v1);
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
        let end = cl.end_point(&frame);
        assert!((end.x - 7.0).abs() < 1e-4);
        assert!(end.y.abs() < 1e-4);
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
    fn begin_sketch_zooms_plane_when_it_has_children() {
        let mut state = AppState::default();
        state.doc.rects.push(Rect::from_local_corners(
            FaceId::ConstructionPlane(0),
            0.0,
            0.0,
            100.0,
            100.0,
        ));
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(800.0, 600.0));
        let before = state.cam.distance;
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: Some(viewport),
        });
        assert!(state.cam.is_transitioning());
        while state.cam.tick_transition(0.05) {}
        assert!(state.cam.distance < before);
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
    fn orbit_changes_camera() {
        let mut state = AppState::default();
        let yaw = state.cam.yaw;
        state.apply(Action::OrbitCamera { delta: (10.0, 5.0) });
        assert_ne!(state.cam.yaw, yaw);
    }
}