//! Shared action layer (SPEC §8, §9, §11.2).
//!
//! GUI buttons, keyboard shortcuts, and instruction scripts all dispatch the
//! same [`Action`] values so behaviour stays in sync.

use crate::camera::Camera;
use crate::model::{Document, Rect};
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
}

impl Tool {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "select" => Some(Tool::Select),
            "rectangle" | "rect" => Some(Tool::Rectangle),
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
    OrbitCamera { delta: (f32, f32) },
    PanCamera { delta: (f32, f32), viewport_height: f32 },
    ZoomCamera { scroll: f32 },
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
            status: String::new(),
        }
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
                self.creating_rect = None;
                self.status = "New document".to_string();
                ActionResult::Ok
            }
            Action::Open { path } => match crate::storage::open(&path) {
                Ok(doc) => {
                    self.doc = doc;
                    self.path = Some(path.clone());
                    self.status = format!(
                        "Opened {} ({} rectangle(s))",
                        path,
                        self.doc.rects.len()
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
                self.status = "Cleared".to_string();
                ActionResult::Ok
            }
            Action::UndoLast => {
                if self.doc.rects.pop().is_some() {
                    self.status = "Undid last rectangle".to_string();
                } else {
                    self.status = "Nothing to undo".to_string();
                }
                ActionResult::Ok
            }
            Action::SetTool(tool) => {
                if self.creating_rect.is_some() && tool != Tool::Rectangle {
                    self.creating_rect = None;
                }
                self.tool = tool;
                self.status = match tool {
                    Tool::Select => "Select tool".to_string(),
                    Tool::Rectangle => "Rectangle tool".to_string(),
                };
                ActionResult::Ok
            }
            Action::CancelOperation => {
                if self.creating_rect.take().is_some() {
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
                    self.status = format!("Added rectangle ({:.1} × {:.1} mm)", rect.w, rect.h);
                    ActionResult::Ok
                } else {
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
            Action::ZoomCamera { scroll } => {
                self.cam.zoom(scroll);
                ActionResult::Ok
            }
        }
    }

    fn write_to(&mut self, path: &str) -> ActionResult {
        match crate::storage::save(path, &self.doc) {
            Ok(()) => {
                self.path = Some(path.to_string());
                self.status = format!("Saved {} rectangle(s) to {}", self.doc.rects.len(), path);
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
        state.path = Some("/tmp/test.le3".to_string());
        state.apply(Action::NewDocument);
        assert!(state.doc.rects.is_empty());
        assert!(state.path.is_none());
    }

    #[test]
    fn set_tool_rectangle() {
        let mut state = AppState::default();
        state.apply(Action::SetTool(Tool::Rectangle));
        assert_eq!(state.tool, Tool::Rectangle);
    }

    #[test]
    fn undo_last_removes_rectangle() {
        let mut state = AppState::default();
        state.doc.rects.push(Rect {
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
        });
        state.apply(Action::UndoLast);
        assert!(state.doc.rects.is_empty());
    }

    #[test]
    fn orbit_changes_camera() {
        let mut state = AppState::default();
        let yaw = state.cam.yaw;
        state.apply(Action::OrbitCamera { delta: (10.0, 5.0) });
        assert_ne!(state.cam.yaw, yaw);
    }
}