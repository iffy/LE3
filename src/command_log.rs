//! Echo user actions as script instructions when `--show-commands` is enabled.

use crate::actions::Action;
use crate::model::Document;
use crate::script::{instruction_from_action, Instruction};
use crate::camera::Camera;
use egui::Vec2;

const EPS: f32 = 1e-4;

/// Records interactive user actions as script instructions. Every emitted instruction is
/// kept in `history` so the whole session can be exported as a Lua script (#43); it is also
/// echoed to stdout when `print_stdout` is set (the `--show-commands` flag).
#[derive(Clone, Debug, Default)]
pub struct CommandLog {
    pending_orbit: Vec2,
    pending_pan: Vec2,
    pending_zoom: f32,
    pending_discrete: Option<Instruction>,
    defer_baseline: bool,
    print_stdout: bool,
    history: Vec<Instruction>,
}

impl CommandLog {
    /// A recording log; `print_stdout` echoes each instruction to stdout (`--show-commands`).
    pub fn new_recording(print_stdout: bool) -> Self {
        Self {
            print_stdout,
            ..Self::default()
        }
    }

    /// Whether any instruction has been recorded this session.
    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }

    /// The recorded session as a replayable, timestamped Lua script.
    pub fn session_lua_script(&self, timestamp: &str) -> String {
        let mut out = String::new();
        out.push_str("-- BearCAD session commands\n");
        out.push_str(&format!("-- Exported {timestamp} UTC\n"));
        out.push_str("-- Replay headless with: cargo run -- --script <file> --exit\n\n");
        for instruction in &self.history {
            out.push_str(&instruction.as_lua());
            out.push('\n');
        }
        out
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

    fn emit(&mut self, instruction: Instruction) {
        if self.print_stdout {
            println!("{}", instruction.as_lua());
        }
        self.history.push(instruction);
    }
}

/// Current UTC time as `YYYYMMDD-HHMMSS` (Howard Hinnant's civil-from-days algorithm), used
/// for session-export filenames and headers without pulling in a date/time dependency.
pub fn utc_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hour, min, sec) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    // Days since 1970-01-01 -> civil (year, month, day).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}{month:02}{day:02}-{hour:02}{min:02}{sec:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;

    #[test]
    fn accumulates_orbit_until_non_camera_action() {
        let mut log = CommandLog::new_recording(false);
        let cam = Camera::default();
        log.note_orbit(Vec2::new(10.0, 0.0));
        log.note_orbit(Vec2::new(-4.0, 5.0));
        log.before_apply(&Action::SetTool(crate::actions::Tool::Select), &cam);
        assert_eq!(log.pending_orbit, Vec2::ZERO);
    }

    #[test]
    fn discrete_view_survives_until_flush_without_drag() {
        let mut log = CommandLog::new_recording(false);
        let cam = Camera::default();
        log.note_view_instruction(Instruction::View(crate::camera::StandardView::Front));
        log.before_apply(&Action::SetTool(crate::actions::Tool::Rectangle), &cam);
        assert!(log.pending_discrete.is_none());
    }

    #[test]
    fn session_script_contains_recorded_instructions_with_header() {
        let mut log = CommandLog::new_recording(false);
        log.emit(Instruction::New);
        log.emit(Instruction::CreateRect {
            x: 0.0,
            y: 0.0,
            width: 80.0,
            height: 50.0,
        });
        let script = log.session_lua_script("20260630-000000");
        assert!(script.starts_with("-- BearCAD session commands"));
        assert!(script.contains("-- Exported 20260630-000000 UTC"));
        assert!(script.contains("bearcad.new()"));
        assert!(script.contains("bearcad.rect"));
        assert!(!log.is_empty());
    }

    #[test]
    fn utc_timestamp_has_expected_shape() {
        let ts = utc_timestamp();
        assert_eq!(ts.len(), 15, "timestamp = {ts}");
        assert_eq!(&ts[8..9], "-");
        assert!(ts[..8].chars().all(|c| c.is_ascii_digit()));
        assert!(ts[9..].chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn drag_after_view_clears_discrete_view() {
        let mut log = CommandLog::new_recording(false);
        log.note_view_instruction(Instruction::View(crate::camera::StandardView::Front));
        log.note_orbit(Vec2::new(1.0, 2.0));
        assert!(log.pending_discrete.is_none());
        assert!(log.pending_orbit.length_sq() > 0.0);
    }
}