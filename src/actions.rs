//! Shared action layer (SPEC §8, §9, §11.2).
//!
//! GUI buttons, keyboard shortcuts, and instruction scripts all dispatch the
//! same [`Action`] values so behaviour stays in sync.

use crate::camera::{Camera, ProjectionMode, StandardView, VIEW_TRANSITION_DURATION};
use crate::view_cube::{self, CubeCornerId, CubeEdgeId};
use crate::model::{Document, Line, Rect, ShapeKind};
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
}

impl Tool {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "select" => Some(Tool::Select),
            "rectangle" | "rect" => Some(Tool::Rectangle),
            "line" => Some(Tool::Line),
            _ => None,
        }
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
    /// Current opposite corner, respecting any locked dimensions from texts.
    pub fn end_point(&self) -> Vec3 {
        let dx = self.last_mouse.x - self.origin.x;
        let dy = self.last_mouse.y - self.origin.y;
        let w = if let Ok(v) = self.texts[0].trim().parse::<f32>() {
            if v > 0.0 { v } else { dx.abs() }
        } else {
            dx.abs()
        };
        let h = if let Ok(v) = self.texts[1].trim().parse::<f32>() {
            if v > 0.0 { v } else { dy.abs() }
        } else {
            dy.abs()
        };
        let sx = if dx < 0.0 { -1.0 } else { 1.0 };
        let sy = if dy < 0.0 { -1.0 } else { 1.0 };
        Vec3::new(self.origin.x + sx * w, self.origin.y + sy * h, 0.0)
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
    /// Current second endpoint, respecting any locked length from `text`.
    pub fn end_point(&self) -> Vec3 {
        let dx = self.last_mouse.x - self.origin.x;
        let dy = self.last_mouse.y - self.origin.y;
        let dist = (dx * dx + dy * dy).sqrt();
        let len = if let Ok(v) = self.text.trim().parse::<f32>() {
            if v > 0.0 { v } else { dist }
        } else {
            dist
        };
        if dist < 1e-6 {
            return Vec3::new(self.origin.x + len, self.origin.y, 0.0);
        }
        let scale = len / dist;
        Vec3::new(
            self.origin.x + dx * scale,
            self.origin.y + dy * scale,
            0.0,
        )
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

/// Application state that actions mutate.
pub struct AppState {
    pub doc: Document,
    pub path: Option<String>,
    pub tool: Tool,
    pub cam: Camera,
    pub creating_rect: Option<CreatingRect>,
    pub creating_line: Option<CreatingLine>,
    pub panes: PaneVisibility,
    pub status: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            doc: Document::default(),
            path: None,
            tool: Tool::default(),
            cam: Camera::default(),
            creating_rect: None,
            creating_line: None,
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
                self.creating_rect = None;
                self.creating_line = None;
                self.status = "New document".to_string();
                ActionResult::Ok
            }
            Action::Open { path } => match crate::storage::open(&path) {
                Ok(doc) => {
                    let n_rects = doc.rects.len();
                    let n_lines = doc.lines.len();
                    self.doc = doc;
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
                self.doc.rects.clear();
                self.doc.lines.clear();
                self.doc.shape_order.clear();
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
                self.tool = tool;
                self.status = match tool {
                    Tool::Select => "Select tool".to_string(),
                    Tool::Rectangle => "Rectangle tool".to_string(),
                    Tool::Line => "Line tool".to_string(),
                };
                ActionResult::Ok
            }
            Action::CancelOperation => {
                if self.creating_rect.take().is_some() || self.creating_line.take().is_some() {
                    self.status = "Cancelled".to_string();
                } else if self.tool != Tool::Select {
                    self.tool = Tool::Select;
                    self.status = "Select tool".to_string();
                }
                ActionResult::Ok
            }
            Action::CommitRectangle => {
                let Some(cr) = self.creating_rect.take() else {
                    return ActionResult::Err("No rectangle in progress".to_string());
                };
                let end = cr.end_point();
                let rect = Rect::from_corners(cr.origin.x, cr.origin.y, end.x, end.y);
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
                let Some(cl) = self.creating_line.take() else {
                    return ActionResult::Err("No line in progress".to_string());
                };
                let end = cl.end_point();
                let line = Line::from_endpoints(cl.origin.x, cl.origin.y, end.x, end.y);
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

    #[test]
    fn new_document_clears_state() {
        let mut state = AppState::default();
        state.doc.rects.push(Rect {
            x: 0.,
            y: 0.,
            w: 10.,
            h: 10.,
        });
        state.doc.lines.push(Line::from_endpoints(0., 0., 1., 0.));
        state.doc.shape_order.push(ShapeKind::Line);
        state.path = Some("/tmp/test.le3".to_string());
        state.apply(Action::NewDocument);
        assert!(state.doc.rects.is_empty());
        assert!(state.doc.lines.is_empty());
        assert!(state.path.is_none());
    }

    #[test]
    fn set_tool_line() {
        let mut state = AppState::default();
        state.apply(Action::SetTool(Tool::Line));
        assert_eq!(state.tool, Tool::Line);
    }

    #[test]
    fn undo_last_removes_most_recent_shape() {
        let mut state = AppState::default();
        state.doc.rects.push(Rect {
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
        });
        state.doc.shape_order.push(ShapeKind::Rect);
        state.doc.lines.push(Line::from_endpoints(0., 0., 5., 0.));
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
        assert!(state.creating_rect.is_none());
    }

    #[test]
    fn commit_line_adds_to_document() {
        let mut state = AppState::default();
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
    fn line_end_point_uses_locked_length() {
        let cl = CreatingLine {
            origin: Vec3::new(1.0, 2.0, 0.0),
            text: "5".to_string(),
            last_mouse: Vec3::new(4.0, 6.0, 0.0),
            user_edited: true,
            pending_focus: false,
        };
        let end = cl.end_point();
        let line = Line::from_endpoints(cl.origin.x, cl.origin.y, end.x, end.y);
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
        let end = cl.end_point();
        assert!((end.x - 7.0).abs() < 1e-4);
        assert!(end.y.abs() < 1e-4);
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