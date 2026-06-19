//! Echo user actions as script instructions when `--show-commands` is enabled.

use crate::actions::Action;
use crate::model::Document;
use crate::script::{instruction_from_action, Instruction};
use crate::camera::Camera;
use egui::Vec2;

const EPS: f32 = 1e-4;

/// Records interactive user actions and prints script lines to stdout.
#[derive(Clone, Debug, Default)]
pub struct CommandLog {
    pending_orbit: Vec2,
    pending_pan: Vec2,
    pending_zoom: f32,
    pending_discrete: Option<Instruction>,
    defer_baseline: bool,
}

impl CommandLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_camera_action(action: &Action) -> bool {
        matches!(
            action,
            Action::OrbitCamera { .. }
                | Action::PanCamera { .. }
                | Action::ZoomCamera { .. }
                | Action::SetStandardView(_)
                | Action::SetViewEdge(_)
                | Action::SetViewCorner(_)
                | Action::ViewHome
                | Action::SetProjectionMode(_)
                | Action::ToggleProjectionMode
        )
    }

    fn is_automatic_camera_side_effect(action: &Action) -> bool {
        matches!(
            action,
            Action::BeginSketch { .. } | Action::OpenSketch { .. } | Action::ExitSketch
        )
    }

    fn should_log(action: &Action) -> bool {
        !matches!(
            action,
            Action::CancelOperation
                | Action::BeginConstructionPlane { .. }
                | Action::BeginDimensionEdit { .. }
                | Action::CommitRectangle
                | Action::CommitLine
                | Action::CommitCircle
        )
    }

    pub fn before_apply(&mut self, action: &Action, cam: &Camera) {
        if Self::should_log(action) && !Self::is_camera_action(action) {
            self.flush_camera(cam);
        }
    }

    pub fn after_apply(&mut self, action: Action, doc: &Document) {
        if Self::is_camera_action(&action) {
            self.note_camera_action(action);
            return;
        }
        if !Self::should_log(&action) {
            if Self::is_automatic_camera_side_effect(&action) {
                self.defer_baseline = true;
            }
            return;
        }
        if let Some(instruction) = instruction_from_action(&action, doc) {
            self.emit(instruction);
        }
        if Self::is_automatic_camera_side_effect(&action) {
            self.defer_baseline = true;
        }
    }

    pub fn on_transition_complete(&mut self, cam: &Camera) {
        if self.defer_baseline {
            self.clear_pending();
            self.defer_baseline = false;
            let _ = cam;
        }
    }

    pub fn note_orbit(&mut self, delta: Vec2) {
        if delta.length_sq() < EPS {
            return;
        }
        self.pending_orbit += delta;
        self.pending_discrete = None;
    }

    pub fn note_pan(&mut self, delta: Vec2) {
        if delta.length_sq() < EPS {
            return;
        }
        self.pending_pan += delta;
        self.pending_discrete = None;
    }

    pub fn note_zoom(&mut self, scroll: f32) {
        if scroll.abs() < EPS {
            return;
        }
        self.pending_zoom += scroll;
        self.pending_discrete = None;
    }

    pub fn note_view_instruction(&mut self, instruction: Instruction) {
        self.clear_pending();
        self.pending_discrete = Some(instruction);
    }

    fn note_camera_action(&mut self, action: Action) {
        match action {
            Action::OrbitCamera { delta } => self.note_orbit(Vec2::new(delta.0, delta.1)),
            Action::PanCamera { delta, .. } => self.note_pan(Vec2::new(delta.0, delta.1)),
            Action::ZoomCamera { scroll, .. } => self.note_zoom(scroll),
            Action::SetStandardView(view) => {
                self.note_view_instruction(Instruction::View(view));
            }
            Action::SetViewEdge(edge) => {
                self.note_view_instruction(Instruction::ViewEdge(edge));
            }
            Action::SetViewCorner(corner) => {
                self.note_view_instruction(Instruction::ViewCorner(corner));
            }
            Action::ViewHome => self.note_view_instruction(Instruction::ViewHome),
            Action::SetProjectionMode(mode) => {
                self.note_view_instruction(Instruction::ProjectionMode(mode));
            }
            Action::ToggleProjectionMode => {
                self.note_view_instruction(Instruction::ToggleProjectionMode);
            }
            _ => {}
        }
    }

    fn flush_camera(&mut self, cam: &Camera) {
        let has_delta = self.pending_orbit.length_sq() > EPS
            || self.pending_pan.length_sq() > EPS
            || self.pending_zoom.abs() > EPS;

        if !has_delta {
            if let Some(instruction) = self.pending_discrete.take() {
                self.emit(instruction);
            }
        } else {
            self.pending_discrete = None;
            if self.pending_orbit.length_sq() > EPS {
                self.emit(Instruction::Orbit {
                    dx: self.pending_orbit.x,
                    dy: self.pending_orbit.y,
                });
            }
            if self.pending_pan.length_sq() > EPS {
                self.emit(Instruction::Pan {
                    dx: self.pending_pan.x,
                    dy: self.pending_pan.y,
                });
            }
            if self.pending_zoom.abs() > EPS {
                self.emit(Instruction::Zoom {
                    scroll: self.pending_zoom,
                });
            }
        }

        self.clear_pending();
        let _ = cam;
    }

    fn clear_pending(&mut self) {
        self.pending_orbit = Vec2::ZERO;
        self.pending_pan = Vec2::ZERO;
        self.pending_zoom = 0.0;
        self.pending_discrete = None;
    }

    fn emit(&self, instruction: Instruction) {
        println!("{}", instruction.as_lua());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;

    #[test]
    fn accumulates_orbit_until_non_camera_action() {
        let mut log = CommandLog::new();
        let cam = Camera::default();
        log.note_orbit(Vec2::new(10.0, 0.0));
        log.note_orbit(Vec2::new(-4.0, 5.0));
        log.before_apply(&Action::SetTool(crate::actions::Tool::Select), &cam);
        assert_eq!(log.pending_orbit, Vec2::ZERO);
    }

    #[test]
    fn discrete_view_survives_until_flush_without_drag() {
        let mut log = CommandLog::new();
        let cam = Camera::default();
        log.note_view_instruction(Instruction::View(crate::camera::StandardView::Front));
        log.before_apply(&Action::SetTool(crate::actions::Tool::Rectangle), &cam);
        assert!(log.pending_discrete.is_none());
    }

    #[test]
    fn drag_after_view_clears_discrete_view() {
        let mut log = CommandLog::new();
        log.note_view_instruction(Instruction::View(crate::camera::StandardView::Front));
        log.note_orbit(Vec2::new(1.0, 2.0));
        assert!(log.pending_discrete.is_none());
        assert!(log.pending_orbit.length_sq() > 0.0);
    }
}