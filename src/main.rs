//! LE3 — early prototype GUI.
//!
//! Rectangle tool: click to fix first corner, move mouse for second, with live
//! dimension inputs on the sides. Type to constrain a side, Tab to cycle,
//! Enter to commit. Right-drag orbit, wheel zoom. Save/Open .le3. (prototype)
//!
//! Fully scriptable via instruction files (SPEC §9.3):
//!   le3 --script demo.le3script
//!   le3 demo.le3script --exit

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod camera;
mod construction;
mod face;
mod model;
mod native_menu;
mod script;
mod stl;
mod storage;
mod value;
mod view_cube;

use actions::{Action, AppState, CreatingLine, CreatingRect, Pane, RectAxis, SketchSession, Tool};
use construction::{
    angle_from_axis_plane_hit, axis_angle_handle, axis_gizmo_hit, axis_normal,
    axis_offset_handle, draw_axis_plane_gizmo, draw_offset_gizmo, draw_quad_face_highlight,
    offset_from_normal_drag, offset_gizmo_hit, offset_handle, pick_reference, plane_corners,
    resolve_pick_target, AxisGizmoDrag, AxisGizmoHit, PlaneDim, PlaneReference,
    AXIS_GIZMO_HANDLE_HIT_RADIUS_PX, PLANE_DISPLAY_HALF,
};
use face::{
    face_label, line_world_endpoints, pick_sketch_face, rect_world_corners, sketch_frame,
    world_to_local,
};
use model::{FaceId, Line, Rect};
use eframe::egui;
use native_menu::{MenuCommand, NativeMenu};
use glam::Vec3;
use model::ConstructionPlane;
use script::{ScriptRunner, SyntheticInput};
use std::path::Path;
use value::{eval_length_mm, format_length_display, shows_computed_length};

fn main() -> eframe::Result<()> {
    let script_opts = script::parse_args(std::env::args());

    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 640.0])
            .with_title("LE3")
            .with_icon(std::sync::Arc::new(egui::IconData::default())),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        options.event_loop_builder = Some(Box::new(|builder| {
            builder.with_default_menu(false);
        }));
    }

    let script = script_opts
        .script_path
        .as_ref()
        .map(|p| ScriptRunner::from_file(Path::new(p)))
        .transpose()
        .map_err(|e| eframe::Error::AppCreation(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        ))))?;

    eframe::run_native(
        "LE3",
        options,
        Box::new(move |cc| {
            let native_menu = NativeMenu::install(cc).map_err(|e| {
                eframe::Error::AppCreation(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                )))
            })?;
            Ok(Box::new(App::new(
                script,
                script_opts.exit_on_complete,
                native_menu,
            )) as Box<dyn eframe::App>)
        }),
    )
}

struct App {
    state: AppState,
    synthetic: SyntheticInput,
    script: Option<ScriptRunner>,
    exit_on_script_complete: bool,
    last_viewport: Option<egui::Rect>,
    native_menu: NativeMenu,
}

impl App {
    fn new(
        script: Option<ScriptRunner>,
        exit_on_script_complete: bool,
        native_menu: NativeMenu,
    ) -> Self {
        let status = if script.is_some() {
            "Running script…".to_string()
        } else {
            String::new()
        };
        Self {
            state: AppState {
                status,
                ..AppState::default()
            },
            synthetic: SyntheticInput::default(),
            script,
            exit_on_script_complete,
            last_viewport: None,
            native_menu,
        }
    }

    fn save_as(&mut self) {
        let start = rfd::FileDialog::new()
            .add_filter("LE3 document", &["le3"])
            .set_file_name("untitled.le3");
        if let Some(path) = start.save_file() {
            let path = path.to_string_lossy().to_string();
            self.state.apply(Action::Save {
                path: Some(path),
            });
        }
    }

    fn save(&mut self) {
        match self.state.apply(Action::Save { path: None }) {
            actions::ActionResult::NeedsDialog => self.save_as(),
            _ => {}
        }
    }

    fn open(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("LE3 document", &["le3"])
            .pick_file();
        if let Some(path) = picked {
            let path = path.to_string_lossy().to_string();
            self.state.apply(Action::Open { path });
        }
    }

    /// Handle selections from the native OS menu bar.
    fn handle_native_menu(&mut self, ctx: &egui::Context) {
        let events = self.native_menu.drain_events();
        for event in events {
            let Some(command) = native_menu::command_for_event(&event, &self.native_menu) else {
                continue;
            };
            match command {
                MenuCommand::Open => self.open(),
                MenuCommand::Save => self.save(),
                MenuCommand::SaveAs => self.save_as(),
                MenuCommand::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                MenuCommand::About => {
                    self.state.status =
                        "LE3 — on-device parametric CAD (prototype)".to_string();
                }
                _ => {
                    if let Some(action) = command.to_action() {
                        self.state.apply(action);
                    }
                }
            }
        }

        self.native_menu
            .sync_pane_checks(|pane| self.state.panes.is_visible(pane));
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.state.apply(Action::CancelOperation);
        }

        if self.state.creating_rect.is_none()
            && self.state.creating_line.is_none()
            && self.state.creating_plane.is_none()
            && ctx.input(|i| i.key_pressed(egui::Key::S))
        {
            if self.state.tool != Tool::Sketch {
                self.state.apply(Action::SetTool(Tool::Sketch));
            }
        }

        if self.state.creating_rect.is_none()
            && self.state.creating_line.is_none()
            && ctx.input(|i| i.key_pressed(egui::Key::R))
        {
            if self.state.tool != Tool::Rectangle {
                self.state.apply(Action::SetTool(Tool::Rectangle));
            }
        }

        if self.state.creating_rect.is_none()
            && self.state.creating_line.is_none()
            && self.state.creating_plane.is_none()
            && ctx.input(|i| i.key_pressed(egui::Key::L))
        {
            if self.state.tool != Tool::Line {
                self.state.apply(Action::SetTool(Tool::Line));
            }
        }

        if self.state.creating_rect.is_none()
            && self.state.creating_line.is_none()
            && self.state.creating_plane.is_none()
            && ctx.input(|i| i.key_pressed(egui::Key::P))
        {
            if self.state.tool != Tool::ConstructionPlane {
                self.state.apply(Action::SetTool(Tool::ConstructionPlane));
            }
        }

        if self.state.tool != Tool::Rectangle || self.state.sketch_session.is_none() {
            self.state.creating_rect = None;
        }
        if self.state.tool != Tool::Line || self.state.sketch_session.is_none() {
            self.state.creating_line = None;
        }
        if self.state.tool != Tool::ConstructionPlane {
            self.state.creating_plane = None;
        }

        let creating = self.state.creating_rect.is_some()
            || self.state.creating_line.is_some()
            || self.state.creating_plane.is_some();
        let (enter_pressed, tab_pressed) = if creating {
            (
                ctx.input(|i| i.key_pressed(egui::Key::Enter)),
                ctx.input(|i| i.key_pressed(egui::Key::Tab)),
            )
        } else {
            (false, false)
        };

        if enter_pressed {
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        }
        if tab_pressed {
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
        }

        if let Some(cr) = &mut self.state.creating_rect {
            if tab_pressed {
                let new_focused = 1 - cr.focused;
                let axis = if new_focused == 0 {
                    RectAxis::Width
                } else {
                    RectAxis::Height
                };
                self.state.apply(Action::FocusRectDimension { axis });
            }
            if enter_pressed {
                self.state.apply(Action::CommitRectangle);
            }
        }
        if self.state.creating_line.is_some() && enter_pressed {
            self.state.apply(Action::CommitLine);
        }

        if let Some(cp) = &mut self.state.creating_plane {
            if tab_pressed && cp.reference.is_axis() {
                let next = if cp.focused == PlaneDim::Offset {
                    PlaneDim::Angle
                } else {
                    PlaneDim::Offset
                };
                self.state.apply(Action::FocusPlaneDim { dim: next });
            }
            if enter_pressed {
                self.state.apply(Action::CommitConstructionPlane);
            }
        }
    }

    fn process_screenshots(&mut self, ctx: &egui::Context) {
        let screenshots: Vec<_> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
                .collect()
        });

        if let Some(runner) = &mut self.script {
            for image in screenshots {
                if let Err(e) = runner.on_screenshot(&image) {
                    runner.error = Some(e);
                    runner.done = true;
                    self.state.status = format!("Script error: {}", runner.error.as_deref().unwrap_or(""));
                }
            }
        }
    }

    fn tick_script(&mut self, ctx: &egui::Context) {
        let needs_repaint = if let Some(runner) = &mut self.script {
            if runner.done {
                if let Some(err) = &runner.error {
                    self.state.status = format!("Script error: {err}");
                } else if runner.should_quit {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else if self.exit_on_script_complete {
                    self.state.status = "Script complete".to_string();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.state.status = "Script complete".to_string();
                }
                false
            } else {
                let repaint = runner.tick(
                    &mut self.state,
                    &mut self.synthetic,
                    self.last_viewport,
                    ctx,
                );
                if let Some(err) = &runner.error {
                    self.state.status = format!("Script error: {err}");
                }
                repaint
            }
        } else {
            false
        };

        if needs_repaint || self.script.as_ref().is_some_and(|r| r.is_waiting()) {
            ctx.request_repaint();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let dt = ctx.input(|i| i.stable_dt);
        if self.state.cam.tick_transition(dt) {
            ctx.request_repaint();
        }

        self.process_screenshots(ctx);
        self.tick_script(ctx);
        self.synthetic.inject(ctx);

        self.handle_keyboard(ctx);

        self.handle_native_menu(ctx);

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.state.tool, Tool::Select, "Select");
                if ui
                    .selectable_label(self.state.tool == Tool::Sketch, "Sketch")
                    .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::Sketch));
                }
                if ui
                    .selectable_label(self.state.tool == Tool::Rectangle, "Rectangle")
                    .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::Rectangle));
                }
                if ui
                    .selectable_label(self.state.tool == Tool::Line, "Line")
                    .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::Line));
                }
                ui.selectable_value(
                    &mut self.state.tool,
                    Tool::ConstructionPlane,
                    "Plane",
                );
                if let Some(session) = self.state.sketch_session {
                    ui.separator();
                    ui.label(format!(
                        "Sketch: {}",
                        face_label(&self.state.doc, session.face)
                    ));
                }
                ui.separator();
                if ui.button("Clear").clicked() {
                    self.state.apply(Action::Clear);
                }
                if ui.button("Undo last").clicked() {
                    self.state.apply(Action::UndoLast);
                }
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            let name = self.state.path.as_deref().unwrap_or("(unsaved)");
            ui.horizontal(|ui| {
                ui.label(name);
                ui.separator();
                ui.label(&self.state.status);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.draw_viewport(ui);
        });
    }
}

/// Colours used in the viewport.
mod col {
    use egui::Color32;
    pub const BG: Color32 = Color32::from_gray(28);
    pub const GRID: Color32 = Color32::from_gray(55);
    pub const GRID_AXIS: Color32 = Color32::from_gray(90);
    pub const X_AXIS: Color32 = Color32::from_rgb(200, 70, 70);
    pub const Y_AXIS: Color32 = Color32::from_rgb(70, 190, 90);
    /// Matches the view-cube Z triad (`view_cube::AXES`).
    pub const Z_AXIS: Color32 = Color32::from_rgb(80, 140, 230);
    pub const RECT_LINE: Color32 = Color32::from_rgb(120, 170, 240);
    pub const LINE_STROKE: Color32 = Color32::from_rgb(180, 140, 240);
    pub const PREVIEW: Color32 = Color32::from_rgb(240, 200, 120);
    pub const DIM_INPUT_BG: Color32 = Color32::from_rgb(22, 24, 30);
    pub const DIM_INPUT_BG_FOCUS: Color32 = Color32::from_rgb(34, 36, 44);
    pub const DIM_INPUT_BORDER: Color32 = Color32::from_rgb(110, 118, 136);
    pub const DIM_INPUT_BORDER_FOCUS: Color32 = Color32::from_rgb(255, 186, 84);
    pub const DIM_INPUT_TEXT: Color32 = Color32::from_rgb(232, 235, 242);
    pub const DIM_INPUT_TEXT_FOCUS: Color32 = Color32::from_rgb(255, 255, 255);
    /// Faint highlight so selected digits stay readable on the dark input background.
    pub const DIM_INPUT_SELECTION: Color32 = Color32::from_rgba_premultiplied(36, 26, 12, 36);
    /// Highlight for the dimension edge/segment tied to the focused input.
    pub const DIM_EDGE_HIGHLIGHT: Color32 = DIM_INPUT_BORDER_FOCUS;
    /// All construction geometry (planes, etc.) shares this colour.
    pub const CONSTRUCTION: Color32 = crate::construction::CONSTRUCTION_RGBA;
    /// Faded appearance for geometry outside the active sketch face.
    pub const SKETCH_DIMMED: f32 = 0.28;
}

const GRID_EXTENT: f32 = 200.0;
const GRID_STEP: f32 = 20.0;

/// Screen-space height of a floating dimension input (frame + text field).
const DIM_INPUT_HEIGHT: f32 = 26.0;
/// Horizontal padding inside the dimension input frame (inner margin × 2).
const DIM_INPUT_FRAME_H_PAD: f32 = 10.0;
/// Minimum text-edit width (fits short live values like `80.0`).
const DIM_INPUT_MIN_TEXT_WIDTH: f32 = 48.0;
/// Approximate monospace glyph width at 13pt (used for layout sizing).
const DIM_INPUT_CHAR_WIDTH: f32 = 7.8;
/// Expression fields grow with content up to this many characters.
const DIM_INPUT_MAX_CHARS: usize = 20;

fn dim_input_text_width(text: &str) -> f32 {
    let chars = text.chars().count().clamp(1, DIM_INPUT_MAX_CHARS);
    (chars as f32 * DIM_INPUT_CHAR_WIDTH).max(DIM_INPUT_MIN_TEXT_WIDTH)
}

fn dim_input_total_width(text: &str) -> f32 {
    dim_input_text_width(text) + DIM_INPUT_FRAME_H_PAD
}

fn dim_input_size_for_text(text: &str) -> egui::Vec2 {
    egui::vec2(dim_input_total_width(text), DIM_INPUT_HEIGHT)
}

fn dim_input_max_size() -> egui::Vec2 {
    dim_input_size_for_text(&"m".repeat(DIM_INPUT_MAX_CHARS))
}
const DIM_LABEL_GAP: f32 = 8.0;
const DIM_LABEL_PAD: f32 = 2.0;
const DIM_REPULSION_ITERS: usize = 16;

/// Preferred offsets from edge anchors (width: bottom mid, height: left mid, line: segment mid).
const WIDTH_LABEL_OFFSET: egui::Vec2 = egui::Vec2::new(-20.0, 14.0);
const HEIGHT_LABEL_OFFSET: egui::Vec2 = egui::Vec2::new(-48.0, -4.0);
/// Perpendicular gap from the line to the nearest edge of the dimension input.
const LINE_LABEL_DISTANCE: f32 = 18.0;

/// Screen-space layout for a floating dimension input.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DimInputLayout {
    pos: egui::Pos2,
    rect: egui::Rect,
}

fn dim_input_rect_at(top_left: egui::Pos2, size: egui::Vec2) -> egui::Rect {
    egui::Rect::from_min_size(top_left, size)
}

fn layout_at(pos: egui::Pos2, size: egui::Vec2) -> DimInputLayout {
    DimInputLayout {
        pos,
        rect: dim_input_rect_at(pos, size),
    }
}

/// Smallest axis-aligned push to separate `moving` from `obstacle` (with padding).
fn separation_vector(moving: egui::Rect, obstacle: egui::Rect, padding: f32) -> egui::Vec2 {
    let obs = obstacle.expand(padding);
    if !moving.intersects(obs) {
        return egui::Vec2::ZERO;
    }
    let pen_left = moving.max.x - obs.min.x;
    let pen_right = obs.max.x - moving.min.x;
    let pen_top = moving.max.y - obs.min.y;
    let pen_bottom = obs.max.y - moving.min.y;
    // When boxes only touch (penetration 0), still nudge apart so we don't stall.
    const MIN_PUSH: f32 = 1.0;
    if pen_left.min(pen_right) < pen_top.min(pen_bottom) {
        if pen_left <= pen_right {
            egui::vec2(-pen_left.max(MIN_PUSH), 0.0)
        } else {
            egui::vec2(pen_right.max(MIN_PUSH), 0.0)
        }
    } else if pen_top <= pen_bottom {
        egui::vec2(0.0, -pen_top.max(MIN_PUSH))
    } else {
        egui::vec2(0.0, pen_bottom.max(MIN_PUSH))
    }
}

fn resolve_rectangle_dim_positions(
    bottom_mid: egui::Pos2,
    left_mid: egui::Pos2,
) -> (egui::Pos2, egui::Pos2) {
    let mut width_pos = bottom_mid + WIDTH_LABEL_OFFSET;
    let mut height_pos = left_mid + HEIGHT_LABEL_OFFSET;
    for _ in 0..DIM_REPULSION_ITERS {
        let w_rect = dim_input_rect_at(width_pos, dim_input_max_size());
        let h_rect = dim_input_rect_at(height_pos, dim_input_max_size());
        let w_push = separation_vector(w_rect, h_rect, DIM_LABEL_PAD);
        let h_push = separation_vector(h_rect, w_rect, DIM_LABEL_PAD);
        if w_push.length_sq() + h_push.length_sq() < 0.25 {
            break;
        }
        width_pos += w_push;
        height_pos += h_push;
    }
    (width_pos, height_pos)
}

fn rectangle_labels_clear(width: egui::Rect, height: egui::Rect) -> bool {
    !width.intersects(height.expand(DIM_LABEL_PAD))
}

fn rectangle_dim_layouts(
    bottom_mid: egui::Pos2,
    left_mid: egui::Pos2,
    width_text: &str,
    height_text: &str,
) -> (DimInputLayout, DimInputLayout) {
    let (width_pos, height_pos) = resolve_rectangle_dim_positions(bottom_mid, left_mid);
    let width = layout_at(width_pos, dim_input_size_for_text(width_text));
    let height = layout_at(height_pos, dim_input_size_for_text(height_text));
    debug_assert!(rectangle_labels_clear(width.rect, height.rect));
    (width, height)
}

fn segment_intersects_rect(pa: egui::Pos2, pb: egui::Pos2, rect: egui::Rect) -> bool {
    if rect.contains(pa) || rect.contains(pb) {
        return true;
    }
    let edges = [
        (rect.left_top(), rect.right_top()),
        (rect.right_top(), rect.right_bottom()),
        (rect.right_bottom(), rect.left_bottom()),
        (rect.left_bottom(), rect.left_top()),
    ];
    for (c, d) in edges {
        if segments_intersect(pa, pb, c, d) {
            return true;
        }
    }
    false
}

fn segments_intersect(a: egui::Pos2, b: egui::Pos2, c: egui::Pos2, d: egui::Pos2) -> bool {
    fn cross(a: egui::Pos2, b: egui::Pos2, c: egui::Pos2) -> f32 {
        (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
    }
    let ab = cross(a, b, c);
    let ab_d = cross(a, b, d);
    let cd = cross(c, d, a);
    let cd_b = cross(c, d, b);
    if ab == 0.0 && ab_d == 0.0 {
        return false;
    }
    ab * ab_d <= 0.0 && cd * cd_b <= 0.0
}

/// Unit vector perpendicular to the line, on the preferred label side (upper-left in screen space).
fn line_perpendicular_unit(pa: egui::Pos2, pb: egui::Pos2) -> egui::Vec2 {
    let delta = pb - pa;
    if delta.length_sq() < 1e-4 {
        return egui::vec2(-1.0, -1.0).normalized();
    }
    let dir = delta.normalized();
    let perp_a = egui::vec2(-dir.y, dir.x);
    let perp_b = egui::vec2(dir.y, -dir.x);
    let prefer = egui::vec2(-1.0, -1.0).normalized();
    if perp_a.dot(prefer) >= perp_b.dot(prefer) {
        perp_a
    } else {
        perp_b
    }
}

fn aabb_half_extent_along(dir: egui::Vec2, size: egui::Vec2) -> f32 {
    if dir.length_sq() < 1e-8 {
        return 0.0;
    }
    let n = dir.normalized();
    size.x * 0.5 * n.x.abs() + size.y * 0.5 * n.y.abs()
}

fn line_dim_top_left(
    pa: egui::Pos2,
    pb: egui::Pos2,
    gap_from_line: f32,
    size: egui::Vec2,
) -> egui::Pos2 {
    let mid = pa.lerp(pb, 0.5);
    let perp = line_perpendicular_unit(pa, pb);
    let center_dist = gap_from_line + aabb_half_extent_along(-perp, size);
    let center = mid + perp * center_dist;
    center - size * 0.5
}

#[cfg(test)]
fn dist_point_to_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    if ab.length_sq() < 1e-8 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

#[cfg(test)]
fn dist_rect_to_segment(rect: egui::Rect, pa: egui::Pos2, pb: egui::Pos2) -> f32 {
    if segment_intersects_rect(pa, pb, rect) {
        return 0.0;
    }
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    corners
        .into_iter()
        .map(|c| dist_point_to_segment(c, pa, pb))
        .fold(f32::MAX, f32::min)
}

fn line_dim_layout(pa: egui::Pos2, pb: egui::Pos2, text: &str) -> DimInputLayout {
    let size = dim_input_size_for_text(text);
    let mut gap = LINE_LABEL_DISTANCE;
    for _ in 0..DIM_REPULSION_ITERS {
        let pos = line_dim_top_left(pa, pb, gap, size);
        let rect = dim_input_rect_at(pos, size).expand(DIM_LABEL_GAP);
        if !segment_intersects_rect(pa, pb, rect) {
            return layout_at(pos, size);
        }
        gap += 2.0;
    }
    layout_at(line_dim_top_left(pa, pb, gap, size), size)
}

fn pointer_over_dim_inputs(pointer: egui::Pos2, layouts: &[DimInputLayout]) -> bool {
    layouts.iter().any(|layout| layout.rect.contains(pointer))
}

fn format_live_dimension(v: f32) -> String {
    if v.abs() < 0.1 {
        "0".to_string()
    } else {
        format!("{:.1}", v)
    }
}

/// Second click on the viewport (not a dimension input) commits the in-progress sketch.
fn should_commit_sketch_on_click(
    was_creating: bool,
    primary_pressed: bool,
    over_input: bool,
) -> bool {
    was_creating && primary_pressed && !over_input
}

/// Whether the dimension field should keep its entire value selected for overwrite typing.
fn should_select_all_rect_value(
    gained_focus: bool,
    has_focus: bool,
    is_focus_target: bool,
    pending_focus: bool,
    user_edited: bool,
    changed_this_frame: bool,
) -> bool {
    if changed_this_frame {
        return false;
    }
    gained_focus
        || (is_focus_target && pending_focus && has_focus)
        || (is_focus_target && has_focus && !user_edited)
}

/// Show a sketch dimension field; selects all text when it gains focus so typing replaces the value.
fn show_sketch_dimension_field(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    id: egui::Id,
    text: &mut String,
    is_focus_target: bool,
    pending_focus: &mut bool,
    user_edited: bool,
) -> bool {
    let has_focus = ctx.memory(|m| m.focused()) == Some(id);
    let frame = egui::Frame::default()
        .fill(if has_focus {
            col::DIM_INPUT_BG_FOCUS
        } else {
            col::DIM_INPUT_BG
        })
        .stroke(egui::Stroke::new(
            1.5,
            if has_focus {
                col::DIM_INPUT_BORDER_FOCUS
            } else {
                col::DIM_INPUT_BORDER
            },
        ))
        .inner_margin(egui::Margin::symmetric(5.0, 3.0))
        .rounding(3.0);

    let computed = eval_length_mm(text).filter(|_| shows_computed_length(text));
    let text_width = dim_input_text_width(text);

    let output = frame
        .show(ui, |ui| {
            ui.set_width(text_width);
            ui.vertical_centered(|ui| {
                if let Some(v) = computed {
                    ui.label(
                        egui::RichText::new(format_length_display(v))
                            .font(egui::FontId::monospace(11.0))
                            .color(col::DIM_INPUT_TEXT.gamma_multiply(0.65)),
                    );
                }
                ui.style_mut().spacing.text_edit_width = text_width;
                ui.visuals_mut().selection.bg_fill = col::DIM_INPUT_SELECTION;
                egui::TextEdit::singleline(text)
                    .id(id)
                    .frame(false)
                    .desired_width(text_width)
                    .font(egui::FontId::monospace(13.0))
                    .text_color(if has_focus {
                        col::DIM_INPUT_TEXT_FOCUS
                    } else {
                        col::DIM_INPUT_TEXT
                    })
                    .margin(egui::vec2(0.0, 0.0))
                    .show(ui)
            })
            .inner
        })
        .inner;
    let resp = &output.response;
    if is_focus_target && *pending_focus {
        resp.request_focus();
    }
    if should_select_all_rect_value(
        resp.gained_focus(),
        resp.has_focus(),
        is_focus_target,
        *pending_focus,
        user_edited,
        resp.changed(),
    ) {
        let len = text.chars().count();
        let mut state = output.state;
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::default(),
            egui::text::CCursor::new(len),
        )));
        state.store(ctx, id);
    }
    if is_focus_target && resp.has_focus() {
        *pending_focus = false;
    }
    resp.changed()
}

fn sketch_plane_point(
    cam: &camera::Camera,
    viewport: egui::Rect,
    vp: &glam::Mat4,
    doc: &model::Document,
    session: SketchSession,
    screen: egui::Pos2,
) -> Option<Vec3> {
    let frame = sketch_frame(doc, session.face)?;
    cam.ray_plane_hit(screen, viewport, vp, frame.origin, frame.normal)
}

fn rectangle_dim_layout_from_corners(
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    corners: [Vec3; 4],
    width_text: &str,
    height_text: &str,
) -> Option<(DimInputLayout, DimInputLayout)> {
    let bottom_mid = project(corners[0].lerp(corners[1], 0.5))?;
    let left_mid = project(corners[0].lerp(corners[3], 0.5))?;
    Some(rectangle_dim_layouts(
        bottom_mid,
        left_mid,
        width_text,
        height_text,
    ))
}

fn rect_highlight_edge(corners: [Vec3; 4], edge: RectDimEdge) -> (Vec3, Vec3) {
    match edge {
        RectDimEdge::Width => (corners[0], corners[1]),
        RectDimEdge::Height => (corners[0], corners[3]),
    }
}

fn draw_face_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    face: FaceId,
    color: egui::Color32,
) {
    match face {
        FaceId::ConstructionPlane(i) => {
            if let Some(plane) = doc.construction_planes.get(i) {
                let corners = plane_corners(plane, PLANE_DISPLAY_HALF);
                draw_quad_face_highlight(painter, project, corners, color);
            }
        }
        FaceId::Rect(i) => {
            if let Some(rect) = doc.rects.get(i) {
                if let Some(corners) = rect_world_corners(doc, rect) {
                    draw_quad_face_highlight(painter, project, corners, color);
                }
            }
        }
    }
}

impl App {
    fn draw_viewport(&mut self, ui: &mut egui::Ui) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let viewport = response.rect;
        self.last_viewport = Some(viewport);
        painter.rect_filled(viewport, 0.0, col::BG);

        // Apply scripted right-drag as direct camera motion.
        self.synthetic.apply_pending_drag(viewport, |delta, modifiers, h| {
            if modifiers.shift {
                self.state.cam.pan(delta, h);
            } else {
                self.state.cam.orbit(delta);
            }
        });

        if response.dragged_by(egui::PointerButton::Secondary) {
            if ui.input(|i| i.modifiers.shift) {
                self.state.cam.pan(response.drag_delta(), viewport.height());
            } else {
                self.state.cam.orbit(response.drag_delta());
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                let focal = response.hover_pos().unwrap_or(viewport.center());
                self.state.cam.zoom(scroll, focal, viewport);
            }
        }

        let cam = self.state.cam.clone();
        let vp = cam.view_proj(viewport);
        let cam_project = cam.clone();
        let project = move |w: Vec3| cam_project.project(w, viewport, &vp);

        if self.state.tool == Tool::Sketch {
            let pointer_screen = response.hover_pos().or(response.interact_pointer_pos());
            if let Some(pp) = pointer_screen {
                if ui.input(|i| i.pointer.primary_pressed()) {
                    if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc) {
                        self.state.apply(Action::BeginSketch {
                            face,
                            viewport: Some(viewport),
                        });
                    }
                } else if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc) {
                    draw_face_highlight(
                        &painter,
                        &project,
                        &self.state.doc,
                        face,
                        construction::PICK_HOVER_RGBA,
                    );
                }
            }
        }

        if self.state.tool == Tool::Rectangle {
            let pointer_screen = response.hover_pos().or(response.interact_pointer_pos());
            if self.state.sketch_session.is_none() {
                if let Some(pp) = pointer_screen {
                    if ui.input(|i| i.pointer.primary_pressed()) {
                        if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc) {
                            self.state.apply(Action::BeginSketch {
                                face,
                                viewport: Some(viewport),
                            });
                        }
                    } else if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc) {
                        draw_face_highlight(
                            &painter,
                            &project,
                            &self.state.doc,
                            face,
                            construction::PICK_HOVER_RGBA,
                        );
                    }
                }
            } else if let (Some(session), Some(pp)) =
                (self.state.sketch_session, pointer_screen)
            {
                if let Some(gp) =
                    sketch_plane_point(&cam, viewport, &vp, &self.state.doc, session, pp)
                {
                    let frame = sketch_frame(&self.state.doc, session.face).unwrap();
                    let was_creating = self.state.creating_rect.is_some();
                    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());

                    if !was_creating && primary_pressed {
                        self.state.creating_rect = Some(CreatingRect {
                            origin: gp,
                            texts: ["".to_string(), "".to_string()],
                            focused: 0,
                            last_mouse: gp,
                            user_edited: [false, false],
                            pending_focus: true,
                        });
                        self.state.status = "Move mouse • type to lock dim • Tab cycle • click/Enter commit • Esc cancel"
                            .to_string();
                    }

                    let mut commit_click = false;
                    if let Some(cr) = &mut self.state.creating_rect {
                        let end = cr.end_point(&frame);
                        let (ou, ov) = world_to_local(&frame, cr.origin);
                        let (eu, ev) = world_to_local(&frame, end);
                        let preview = Rect::from_local_corners(session.face, ou, ov, eu, ev);
                        let corners = rect_world_corners(&self.state.doc, &preview).unwrap();
                        let dim_layouts = rectangle_dim_layout_from_corners(
                            &project,
                            corners,
                            &cr.texts[0],
                            &cr.texts[1],
                        );
                        let over_input = dim_layouts
                            .as_ref()
                            .is_some_and(|(w, h)| w.rect.contains(pp) || h.rect.contains(pp));

                        if should_commit_sketch_on_click(was_creating, primary_pressed, over_input) {
                            commit_click = true;
                        } else if !over_input {
                            cr.last_mouse = gp;
                            let (au, av) = world_to_local(&frame, cr.origin);
                            let (bu, bv) = world_to_local(&frame, gp);
                            if !cr.user_edited[0] {
                                cr.texts[0] = format_live_dimension((bu - au).abs());
                            }
                            if !cr.user_edited[1] {
                                cr.texts[1] = format_live_dimension((bv - av).abs());
                            }
                        }
                    }
                    if commit_click {
                        self.state.apply(Action::CommitRectangle);
                    }
                }
            }
        }

        if self.state.tool == Tool::Line {
            let pointer_screen = response.hover_pos().or(response.interact_pointer_pos());
            if self.state.sketch_session.is_none() {
                if let Some(pp) = pointer_screen {
                    if ui.input(|i| i.pointer.primary_pressed()) {
                        if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc) {
                            self.state.apply(Action::BeginSketch {
                                face,
                                viewport: Some(viewport),
                            });
                        }
                    } else if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc) {
                        draw_face_highlight(
                            &painter,
                            &project,
                            &self.state.doc,
                            face,
                            construction::PICK_HOVER_RGBA,
                        );
                    }
                }
            } else if let (Some(session), Some(pp)) =
                (self.state.sketch_session, pointer_screen)
            {
                if let Some(gp) =
                    sketch_plane_point(&cam, viewport, &vp, &self.state.doc, session, pp)
                {
                    let frame = sketch_frame(&self.state.doc, session.face).unwrap();
                    let was_creating = self.state.creating_line.is_some();
                    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());

                    if !was_creating && primary_pressed {
                        self.state.creating_line = Some(CreatingLine {
                            origin: gp,
                            text: String::new(),
                            last_mouse: gp,
                            user_edited: false,
                            pending_focus: true,
                        });
                        self.state.status = "Move mouse • type to lock length • click/Enter commit • Esc cancel"
                            .to_string();
                    }

                    let mut commit_click = false;
                    if let Some(cl) = &mut self.state.creating_line {
                        let end = cl.end_point(&frame);
                        let over_input = project(cl.origin).zip(project(end)).is_some_and(
                            |(pa, pb)| {
                                pointer_over_dim_inputs(pp, &[line_dim_layout(pa, pb, &cl.text)])
                            },
                        );

                        if should_commit_sketch_on_click(was_creating, primary_pressed, over_input)
                        {
                            commit_click = true;
                        } else if !over_input {
                            cl.last_mouse = gp;
                            if !cl.user_edited {
                                let (au, av) = world_to_local(&frame, cl.origin);
                                let (bu, bv) = world_to_local(&frame, gp);
                                let du = bu - au;
                                let dv = bv - av;
                                cl.text = format_live_dimension((du * du + dv * dv).sqrt());
                            }
                        }
                    }
                    if commit_click {
                        self.state.apply(Action::CommitLine);
                    }
                }
            }
        }

        if self.state.tool == Tool::ConstructionPlane {
            let ground = |p: egui::Pos2| cam.ground_point(p, viewport, &vp);
            let pointer_screen = response.hover_pos().or(response.interact_pointer_pos());

            if let Some(pp) = pointer_screen {
                let gp = ground(pp);
                let was_creating = self.state.creating_plane.is_some();
                let primary_pressed = ui.input(|i| i.pointer.primary_pressed());

                if !was_creating && primary_pressed {
                    if let Some(reference) =
                        pick_reference(pp, &project, gp, &self.state.doc)
                    {
                        self.state.apply(Action::BeginConstructionPlane { reference });
                    }
                }

                let mut commit_click = false;
                if let Some(cp) = &mut self.state.creating_plane {
                    let scroll = ui.input(|i| i.raw_scroll_delta.y);
                    let primary_down = ui.input(|i| i.pointer.primary_down());
                    let primary_released = ui.input(|i| i.pointer.primary_released());

                    if primary_pressed {
                        match &cp.reference {
                            PlaneReference::Axis {
                                origin,
                                direction,
                                ..
                            } => {
                                if let Some(hit) = axis_gizmo_hit(
                                    pp,
                                    &project,
                                    *origin,
                                    *direction,
                                    cp.offset_live,
                                    cp.axis_angle_deg,
                                ) {
                                    cp.axis_gizmo_drag = Some(AxisGizmoDrag {
                                        hit,
                                        start_offset: cp.offset_live,
                                        start_angle_deg: cp.axis_angle_deg,
                                        start_screen: pp,
                                    });
                                    cp.user_edited_offset = false;
                                    cp.user_edited_angle = false;
                                }
                            }
                            PlaneReference::Face { origin, normal, .. } => {
                                if offset_gizmo_hit(
                                    pp,
                                    &project,
                                    *origin,
                                    *normal,
                                    cp.offset_live,
                                ) {
                                    cp.axis_gizmo_drag = Some(AxisGizmoDrag {
                                        hit: AxisGizmoHit::Offset,
                                        start_offset: cp.offset_live,
                                        start_angle_deg: 0.0,
                                        start_screen: pp,
                                    });
                                    cp.user_edited_offset = false;
                                }
                            }
                        }
                    }

                    let gizmo_drag = cp.axis_gizmo_drag;
                    if let Some(drag) = gizmo_drag {
                        if primary_down {
                            match drag.hit {
                                AxisGizmoHit::Offset => {
                                    let (origin, normal) = match &cp.reference {
                                        PlaneReference::Face { origin, normal, .. } => {
                                            (*origin, normal.normalize_or_zero())
                                        }
                                        PlaneReference::Axis {
                                            origin,
                                            direction,
                                            ..
                                        } => (
                                            *origin,
                                            axis_normal(*direction, drag.start_angle_deg),
                                        ),
                                    };
                                    cp.offset_live = offset_from_normal_drag(
                                        origin,
                                        normal,
                                        &project,
                                        drag.start_offset,
                                        drag.start_screen,
                                        pp,
                                    );
                                }
                                AxisGizmoHit::Angle => {
                                    if let PlaneReference::Axis {
                                        origin,
                                        direction,
                                        ..
                                    } = &cp.reference
                                    {
                                        if let Some(hit) = cam.ray_plane_hit(
                                            pp, viewport, &vp, *origin, *direction,
                                        ) {
                                            cp.axis_angle_deg = angle_from_axis_plane_hit(
                                                *origin, *direction, hit,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if primary_released {
                        cp.axis_gizmo_drag = None;
                    }

                    if scroll != 0.0
                        && !cp.user_edited_offset
                        && cp.axis_gizmo_drag.is_none()
                    {
                        cp.offset_live += scroll * 0.05;
                    }

                    if !cp.user_edited_offset {
                        let (off, ang) = cp.live_dims();
                        cp.offset_text = format_live_dimension(off);
                        if cp.reference.is_axis() && !cp.user_edited_angle {
                            cp.angle_text = format!("{:.0}", ang);
                        }
                    }

                    let preview = cp.preview_plane();
                    let dim_layouts = plane_dim_layouts(
                        &project,
                        &preview,
                        &cp.reference,
                        cp.offset_live,
                        cp.axis_angle_deg,
                    );
                    let over_input = dim_layouts.as_ref().is_some_and(|(offset, angle)| {
                        let mut layouts = vec![*offset];
                        if let Some(angle) = angle {
                            layouts.push(*angle);
                        }
                        pointer_over_dim_inputs(pp, &layouts)
                    });
                    let over_gizmo = match &cp.reference {
                        PlaneReference::Face { origin, normal, .. } => offset_gizmo_hit(
                            pp,
                            &project,
                            *origin,
                            *normal,
                            cp.offset_live,
                        ),
                        PlaneReference::Axis {
                            origin,
                            direction,
                            ..
                        } => axis_gizmo_hit(
                            pp,
                            &project,
                            *origin,
                            *direction,
                            cp.offset_live,
                            cp.axis_angle_deg,
                        )
                        .is_some(),
                    };

                    if should_commit_sketch_on_click(
                        was_creating,
                        primary_pressed,
                        over_input || over_gizmo || cp.axis_gizmo_drag.is_some(),
                    ) {
                        commit_click = true;
                    }
                }
                if commit_click {
                    self.state.apply(Action::CommitConstructionPlane);
                }
            }
        }

        let sketch_session = self.state.sketch_session;
        draw_ground(
            &painter,
            &project,
            viewport,
            sketch_session.is_some(),
        );

        for (ri, r) in self.state.doc.rects.iter().enumerate() {
            let dim = sketch_session.is_some_and(|s| !sketch_rect_is_active(s, ri, r.parent));
            let color = sketch_color(col::RECT_LINE, dim);
            draw_rect(&painter, &project, &self.state.doc, *r, color, true);
        }
        for line in &self.state.doc.lines {
            let dim = sketch_session.is_some_and(|s| line.parent != s.face);
            let color = sketch_color(col::LINE_STROKE, dim);
            draw_line_segment(&painter, &project, &self.state.doc, *line, color, 2.0);
        }
        for (i, plane) in self.state.doc.construction_planes.iter().enumerate() {
            let active = sketch_session.is_some_and(|s| s.face == FaceId::ConstructionPlane(i));
            let color = if active {
                col::DIM_EDGE_HIGHLIGHT
            } else {
                sketch_color(col::CONSTRUCTION, sketch_session.is_some())
            };
            draw_construction_plane(&painter, &project, plane, color, true);
        }
        if let Some(session) = sketch_session {
            if !matches!(session.face, FaceId::ConstructionPlane(_)) {
                draw_face_highlight(
                    &painter,
                    &project,
                    &self.state.doc,
                    session.face,
                    col::DIM_EDGE_HIGHLIGHT,
                );
            }
        }
        if let (Some(cr), Some(session)) =
            (&self.state.creating_rect, self.state.sketch_session)
        {
            if let Some(frame) = sketch_frame(&self.state.doc, session.face) {
                let end = cr.end_point(&frame);
                let (ou, ov) = world_to_local(&frame, cr.origin);
                let (eu, ev) = world_to_local(&frame, end);
                let preview = Rect::from_local_corners(session.face, ou, ov, eu, ev);
                draw_rect(&painter, &project, &self.state.doc, preview, col::PREVIEW, false);
                if let Some(sp) = project(cr.origin) {
                    painter.circle_filled(sp, 3.5, col::PREVIEW);
                }
            }
        }
        if let (Some(cl), Some(session)) =
            (&self.state.creating_line, self.state.sketch_session)
        {
            if let Some(frame) = sketch_frame(&self.state.doc, session.face) {
                let end = cl.end_point(&frame);
                if let (Some(pa), Some(pb)) = (project(cl.origin), project(end)) {
                    painter.line_segment([pa, pb], egui::Stroke::new(2.0, col::PREVIEW));
                }
                if let Some(sp) = project(cl.origin) {
                    painter.circle_filled(sp, 3.5, col::PREVIEW);
                }
            }
        }
        if let Some(cp) = &self.state.creating_plane {
            let preview = cp.preview_plane();
            draw_construction_plane(&painter, &project, &preview, col::PREVIEW, false);
            let gizmo_hover = response
                .hover_pos()
                .or(response.interact_pointer_pos())
                .and_then(|pp| match &cp.reference {
                    PlaneReference::Face { origin, normal, .. } => {
                        if offset_gizmo_hit(pp, &project, *origin, *normal, cp.offset_live) {
                            Some(AxisGizmoHit::Offset)
                        } else {
                            None
                        }
                    }
                    PlaneReference::Axis {
                        origin,
                        direction,
                        ..
                    } => axis_gizmo_hit(
                        pp,
                        &project,
                        *origin,
                        *direction,
                        cp.offset_live,
                        cp.axis_angle_deg,
                    ),
                });
            match &cp.reference {
                PlaneReference::Face { origin, normal, .. } => {
                    draw_offset_gizmo(
                        &painter,
                        &project,
                        *origin,
                        *normal,
                        cp.offset_live,
                        col::PREVIEW,
                        gizmo_hover == Some(AxisGizmoHit::Offset),
                    );
                }
                PlaneReference::Axis {
                    origin,
                    direction,
                    ..
                } => {
                    draw_axis_plane_gizmo(
                        &painter,
                        &project,
                        *origin,
                        *direction,
                        cp.offset_live,
                        cp.axis_angle_deg,
                        col::PREVIEW,
                        gizmo_hover,
                    );
                }
            }
        }

        if self.state.tool == Tool::ConstructionPlane && self.state.creating_plane.is_none() {
            if let Some(pp) = response.hover_pos().or(response.interact_pointer_pos()) {
                let gp = cam.ground_point(pp, viewport, &vp);
                if let Some(target) = resolve_pick_target(pp, &project, gp, &self.state.doc) {
                    target.draw_highlight(&painter, &project, &self.state.doc);
                }
            }
        }

        if let (Some(cr), Some(session)) =
            (&mut self.state.creating_rect, self.state.sketch_session)
        {
            let frame = sketch_frame(&self.state.doc, session.face).unwrap();
            let end = cr.end_point(&frame);
            let (ou, ov) = world_to_local(&frame, cr.origin);
            let (eu, ev) = world_to_local(&frame, end);
            let preview = Rect::from_local_corners(session.face, ou, ov, eu, ev);
            let corners = rect_world_corners(&self.state.doc, &preview).unwrap();
            if let Some((width_layout, height_layout)) = rectangle_dim_layout_from_corners(
                &project,
                corners,
                &cr.texts[0],
                &cr.texts[1],
            ) {
                let ctx = ui.ctx();
                let id_w = egui::Id::new("cr_width");
                let id_h = egui::Id::new("cr_height");

                egui::Area::new(egui::Id::new("cr_width_area"))
                    .fixed_pos(width_layout.pos)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        if show_sketch_dimension_field(
                            ui,
                            ctx,
                            id_w,
                            &mut cr.texts[0],
                            cr.focused == 0,
                            &mut cr.pending_focus,
                            cr.user_edited[0],
                        ) {
                            cr.user_edited[0] = true;
                        }
                    });

                egui::Area::new(egui::Id::new("cr_height_area"))
                    .fixed_pos(height_layout.pos)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        if show_sketch_dimension_field(
                            ui,
                            ctx,
                            id_h,
                            &mut cr.texts[1],
                            cr.focused == 1,
                            &mut cr.pending_focus,
                            cr.user_edited[1],
                        ) {
                            cr.user_edited[1] = true;
                        }
                    });

                let current = ctx.memory(|m| m.focused());
                if current == Some(id_w) {
                    cr.focused = 0;
                } else if current == Some(id_h) {
                    cr.focused = 1;
                } else if cr.pending_focus {
                    let target_id = if cr.focused == 0 { id_w } else { id_h };
                    ctx.memory_mut(|m| m.request_focus(target_id));
                }

                if let Some(edge) = current
                    .and_then(|id| {
                        if id == id_w {
                            rect_dim_edge_for_focus(0)
                        } else if id == id_h {
                            rect_dim_edge_for_focus(1)
                        } else {
                            None
                        }
                    })
                {
                    let (a, b) = rect_highlight_edge(corners, edge);
                    draw_world_segment(
                        &painter,
                        &project,
                        a,
                        b,
                        col::DIM_EDGE_HIGHLIGHT,
                        3.5,
                    );
                }
            }
        }

        if let (Some(cl), Some(session)) =
            (&mut self.state.creating_line, self.state.sketch_session)
        {
            let frame = sketch_frame(&self.state.doc, session.face).unwrap();
            let end = cl.end_point(&frame);
            if let (Some(pa), Some(pb)) = (project(cl.origin), project(end)) {
                let layout = line_dim_layout(pa, pb, &cl.text);
                let ctx = ui.ctx();
                let id_len = egui::Id::new("cl_length");

                egui::Area::new(egui::Id::new("cl_length_area"))
                    .fixed_pos(layout.pos)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        if show_sketch_dimension_field(
                            ui,
                            ctx,
                            id_len,
                            &mut cl.text,
                            true,
                            &mut cl.pending_focus,
                            cl.user_edited,
                        ) {
                            cl.user_edited = true;
                        }
                    });

                let length_focused = ctx.memory(|m| m.focused()) == Some(id_len);
                if !length_focused && cl.pending_focus {
                    ctx.memory_mut(|m| m.request_focus(id_len));
                } else if length_focused {
                    draw_world_segment(
                        &painter,
                        &project,
                        cl.origin,
                        end,
                        col::DIM_EDGE_HIGHLIGHT,
                        3.5,
                    );
                }
            }
        }

        if let Some(cp) = &mut self.state.creating_plane {
            let preview = cp.preview_plane();
            if let Some((offset_layout, angle_layout)) = plane_dim_layouts(
                &project,
                &preview,
                &cp.reference,
                cp.offset_live,
                cp.axis_angle_deg,
            )
            {
                let ctx = ui.ctx();
                let id_offset = egui::Id::new("cp_offset");
                let id_angle = egui::Id::new("cp_angle");

                egui::Area::new(egui::Id::new("cp_offset_area"))
                    .fixed_pos(offset_layout.pos)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        if show_sketch_dimension_field(
                            ui,
                            ctx,
                            id_offset,
                            &mut cp.offset_text,
                            cp.focused == PlaneDim::Offset,
                            &mut cp.pending_focus,
                            cp.user_edited_offset,
                        ) {
                            cp.user_edited_offset = true;
                        }
                    });

                if let Some(angle_layout) = angle_layout {
                    egui::Area::new(egui::Id::new("cp_angle_area"))
                        .fixed_pos(angle_layout.pos)
                        .order(egui::Order::Foreground)
                        .show(ctx, |ui| {
                            if show_sketch_dimension_field(
                                ui,
                                ctx,
                                id_angle,
                                &mut cp.angle_text,
                                cp.focused == PlaneDim::Angle,
                                &mut cp.pending_focus,
                                cp.user_edited_angle,
                            ) {
                                cp.user_edited_angle = true;
                            }
                        });
                }

                let current = ctx.memory(|m| m.focused());
                if current == Some(id_offset) {
                    cp.focused = PlaneDim::Offset;
                } else if current == Some(id_angle) {
                    cp.focused = PlaneDim::Angle;
                } else if cp.pending_focus {
                    let target_id = if cp.focused == PlaneDim::Offset {
                        id_offset
                    } else {
                        id_angle
                    };
                    ctx.memory_mut(|m| m.request_focus(target_id));
                }

                draw_construction_plane(
                    &painter,
                    &project,
                    &preview,
                    col::DIM_EDGE_HIGHLIGHT,
                    false,
                );
            }
        }

        if self.state.panes.is_visible(Pane::ViewCube) {
            view_cube::show_hud(ui.ctx(), &mut self.state.cam, viewport);
        }

        let hint = match self.state.tool {
            Tool::Select => {
                if self.state.sketch_session.is_some() {
                    "Sketch mode — pick rectangle or line tool  •  Esc: exit sketch (cancels in-progress draw first)"
                } else {
                    "Right-drag: orbit  •  Shift+right-drag: pan  •  Wheel: zoom  •  s: sketch  •  p: plane"
                }
            }
            Tool::Sketch => {
                "s: sketch  •  Click a rectangle or construction plane face  •  Esc: cancel"
            }
            Tool::Rectangle => {
                if self.state.creating_rect.is_some() {
                    "Move mouse (free dim) • Type in focused input to constrain • Tab: switch dims • Click/Enter: create rect • Esc: cancel"
                } else if self.state.sketch_session.is_none() {
                    "r: rectangle  •  Click a face to sketch on  •  Right-drag: orbit  •  Shift+right-drag: pan  •  Wheel: zoom"
                } else {
                    "r: rectangle  •  Left-click to set corner • move to size • Right-drag: orbit  • Shift+right-drag: pan  •  Wheel: zoom"
                }
            }
            Tool::Line => {
                if self.state.creating_line.is_some() {
                    "Move mouse (free length) • Type in length input to constrain • Click/Enter: create line • Esc: cancel"
                } else if self.state.sketch_session.is_none() {
                    "l: line  •  Click a face to sketch on  •  Right-drag: orbit  • Shift+right-drag: pan  •  Wheel: zoom"
                } else {
                    "l: line  •  Left-click to set start • move to aim • Right-drag: orbit  • Shift+right-drag: pan  •  Wheel: zoom"
                }
            }
            Tool::ConstructionPlane => {
                if self.state.creating_plane.is_some() {
                    if self
                        .state
                        .creating_plane
                        .as_ref()
                        .is_some_and(|cp| cp.reference.is_axis())
                    {
                        "Drag arrow for offset • drag circle handle for angle • type to lock • Tab: switch dims • Click/Enter: commit • Esc: cancel"
                    } else {
                        "Drag arrow for offset • wheel or type to lock • Click/Enter: create plane • Esc: cancel"
                    }
                } else {
                    "p: plane  •  Click a face, line, shape edge, global axis, or ground • then set offset (and angle for lines)"
                }
            }
        };
        painter.text(
            viewport.left_bottom() + egui::vec2(8.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            hint,
            egui::FontId::proportional(13.0),
            egui::Color32::from_gray(150),
        );
    }
}

/// Which normalized rectangle edge corresponds to a dimension input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RectDimEdge {
    /// Horizontal edge at min Y (width).
    Width,
    /// Vertical edge at min X (height).
    Height,
}

fn rect_dim_edge_for_focus(focused: usize) -> Option<RectDimEdge> {
    match focused {
        0 => Some(RectDimEdge::Width),
        1 => Some(RectDimEdge::Height),
        _ => None,
    }
}

fn draw_world_segment(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    a: Vec3,
    b: Vec3,
    color: egui::Color32,
    width: f32,
) {
    if let (Some(pa), Some(pb)) = (project(a), project(b)) {
        painter.line_segment([pa, pb], egui::Stroke::new(width, color));
    }
}

fn draw_world_quad(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    corners: [Vec3; 4],
    color: egui::Color32,
    fill: bool,
) {
    let pts: Option<Vec<egui::Pos2>> = corners.iter().map(|&c| project(c)).collect();
    let Some(pts) = pts else { return };
    if fill {
        painter.add(egui::Shape::convex_polygon(
            pts.clone(),
            color.gamma_multiply(0.25),
            egui::Stroke::new(1.5, color),
        ));
    } else {
        painter.add(egui::Shape::closed_line(pts, egui::Stroke::new(1.5, color)));
    }
}

fn draw_line_segment(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    line: Line,
    color: egui::Color32,
    width: f32,
) {
    let Some((a, b)) = line_world_endpoints(doc, &line) else {
        return;
    };
    draw_world_segment(painter, project, a, b, color, width);
}

fn dim_layout_near_screen_point(
    anchor: egui::Pos2,
    outward: egui::Vec2,
    gap_from_anchor: f32,
) -> DimInputLayout {
    let dir = if outward.length_sq() > 1e-4 {
        outward.normalized()
    } else {
        egui::vec2(-1.0, -1.0).normalized()
    };
    let size = dim_input_max_size();
    let center_dist = gap_from_anchor + aabb_half_extent_along(dir, size);
    let center = anchor + dir * center_dist;
    layout_at(center - size * 0.5, size)
}

fn dim_layout_avoiding_handle(
    anchor: egui::Pos2,
    outward: egui::Vec2,
    handle_size: f32,
) -> DimInputLayout {
    let mut gap = AXIS_GIZMO_HANDLE_HIT_RADIUS_PX + 6.0;
    let obstacle =
        egui::Rect::from_center_size(anchor, egui::vec2(handle_size, handle_size));
    for _ in 0..DIM_REPULSION_ITERS {
        let layout = dim_layout_near_screen_point(anchor, outward, gap);
        if !layout.rect.intersects(obstacle) {
            return layout;
        }
        gap += 2.0;
    }
    dim_layout_near_screen_point(anchor, outward, gap)
}

fn plane_dim_layouts(
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    _plane: &ConstructionPlane,
    reference: &PlaneReference,
    offset_live: f32,
    axis_angle_deg: f32,
) -> Option<(DimInputLayout, Option<DimInputLayout>)> {
    match reference {
        PlaneReference::Face { origin, normal, .. } => {
            let face_screen = project(*origin)?;
            let offset_screen = project(offset_handle(*origin, *normal, offset_live))?;
            let arrow = offset_screen - face_screen;
            let beside_arrow = if arrow.length_sq() > 1.0 {
                egui::vec2(-arrow.y, arrow.x).normalized()
            } else {
                egui::vec2(-1.0, 0.0)
            };
            let offset_layout =
                dim_layout_avoiding_handle(offset_screen, beside_arrow, 20.0);
            Some((offset_layout, None))
        }
        PlaneReference::Axis {
            origin,
            direction,
            ..
        } => {
            let axis_screen = project(*origin)?;
            let offset_screen = project(axis_offset_handle(
                *origin,
                *direction,
                offset_live,
                axis_angle_deg,
            ))?;
            let arrow = offset_screen - axis_screen;
            let beside_arrow = if arrow.length_sq() > 1.0 {
                egui::vec2(-arrow.y, arrow.x).normalized()
            } else {
                egui::vec2(-1.0, 0.0)
            };
            let offset_layout =
                dim_layout_avoiding_handle(offset_screen, beside_arrow, 20.0);

            let angle_screen = project(axis_angle_handle(
                *origin,
                *direction,
                axis_angle_deg,
            ))?;
            let radial = angle_screen - axis_screen;
            let angle_layout = dim_layout_avoiding_handle(angle_screen, radial, 24.0);

            Some((offset_layout, Some(angle_layout)))
        }
    }
}

fn draw_construction_plane(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    plane: &ConstructionPlane,
    color: egui::Color32,
    fill: bool,
) {
    let corners = plane_corners(plane, PLANE_DISPLAY_HALF);
    let pts: Option<Vec<egui::Pos2>> = corners.iter().map(|&c| project(c)).collect();
    let Some(pts) = pts else { return };
    if fill {
        painter.add(egui::Shape::convex_polygon(
            pts.clone(),
            color.gamma_multiply(0.18),
            egui::Stroke::new(1.5, color),
        ));
    } else {
        painter.add(egui::Shape::closed_line(
            pts,
            egui::Stroke::new(2.0, color),
        ));
    }
}

fn draw_rect(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    r: Rect,
    color: egui::Color32,
    fill: bool,
) {
    let Some(corners) = rect_world_corners(doc, &r) else {
        return;
    };
    draw_world_quad(painter, project, corners, color, fill);
}

/// Liang–Barsky clip of a screen-space segment to an axis-aligned rectangle.
fn clip_segment_to_rect(a: egui::Pos2, b: egui::Pos2, rect: egui::Rect) -> Option<(egui::Pos2, egui::Pos2)> {
    let mut t0 = 0.0f32;
    let mut t1 = 1.0f32;
    let d = b - a;
    let edges = [
        (-d.x, a.x - rect.min.x),
        (d.x, rect.max.x - a.x),
        (-d.y, a.y - rect.min.y),
        (d.y, rect.max.y - a.y),
    ];
    for (p, q) in edges {
        if p.abs() < 1e-8 {
            if q < 0.0 {
                return None;
            }
        } else if p < 0.0 {
            let r = q / p;
            if r > t1 {
                return None;
            }
            t0 = t0.max(r);
        } else {
            let r = q / p;
            if r < t0 {
                return None;
            }
            t1 = t1.min(r);
        }
    }
    Some((a + d * t0, a + d * t1))
}

fn draw_clipped_world_segment(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    viewport: egui::Rect,
    a: Vec3,
    b: Vec3,
    color: egui::Color32,
    width: f32,
) {
    let (Some(pa), Some(pb)) = (project(a), project(b)) else {
        return;
    };
    let Some((ca, cb)) = clip_segment_to_rect(pa, pb, viewport) else {
        return;
    };
    painter.line_segment([ca, cb], egui::Stroke::new(width, color));
}

fn sketch_color(color: egui::Color32, dim: bool) -> egui::Color32 {
    if dim {
        color.gamma_multiply(col::SKETCH_DIMMED)
    } else {
        color
    }
}

fn sketch_rect_is_active(session: SketchSession, rect_index: usize, parent: FaceId) -> bool {
    match session.face {
        FaceId::Rect(face_index) => rect_index == face_index || parent == session.face,
        FaceId::ConstructionPlane(_) => parent == session.face,
    }
}

fn draw_ground(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    viewport: egui::Rect,
    dim: bool,
) {
    let e = GRID_EXTENT;
    let line = |a: Vec3, b: Vec3, color: egui::Color32, w: f32| {
        draw_clipped_world_segment(painter, project, viewport, a, b, color, w);
    };

    let mut t = -e;
    while t <= e + 0.001 {
        let base = if t.abs() < 0.001 {
            col::GRID_AXIS
        } else {
            col::GRID
        };
        let color = sketch_color(base, dim);
        line(Vec3::new(-e, t, 0.0), Vec3::new(e, t, 0.0), color, 1.0);
        line(Vec3::new(t, -e, 0.0), Vec3::new(t, e, 0.0), color, 1.0);
        t += GRID_STEP;
    }

    line(
        Vec3::ZERO,
        Vec3::new(e, 0.0, 0.0),
        sketch_color(col::X_AXIS, dim),
        2.0,
    );
    line(
        Vec3::ZERO,
        Vec3::new(0.0, e, 0.0),
        sketch_color(col::Y_AXIS, dim),
        2.0,
    );
    line(
        Vec3::ZERO,
        Vec3::new(0.0, 0.0, e),
        sketch_color(col::Z_AXIS, dim),
        2.0,
    );
}

#[cfg(test)]
mod tests {
    use super::actions::CreatingRect;
    use super::{
        clip_segment_to_rect, col, should_commit_sketch_on_click, should_select_all_rect_value,
        GRID_EXTENT,
    };
    use crate::face::SketchFrame;
    use eframe::egui::{self, Pos2, Rect, Vec2};
    use egui::Color32;
    use glam::Vec3;

    #[test]
    fn clip_segment_clamps_infinite_spike_to_viewport() {
        let vp = Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(800.0, 600.0));
        let (a, b) = clip_segment_to_rect(
            Pos2::new(-12_000.0, 300.0),
            Pos2::new(12_000.0, 300.0),
            vp,
        )
        .expect("horizon spike should clip");
        assert!((a.x - vp.min.x).abs() < 0.01);
        assert!((b.x - vp.max.x).abs() < 0.01);
        assert!((a.y - 300.0).abs() < 0.01);
        assert!((b.y - 300.0).abs() < 0.01);
    }

    #[test]
    fn clip_segment_returns_none_when_fully_outside() {
        let vp = Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 100.0));
        assert!(clip_segment_to_rect(Pos2::new(-50.0, -20.0), Pos2::new(50.0, -10.0), vp).is_none());
    }

    #[test]
    fn clip_segment_preserves_interior_segment() {
        let vp = Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 100.0));
        let (a, b) = clip_segment_to_rect(Pos2::new(10.0, 20.0), Pos2::new(80.0, 70.0), vp).unwrap();
        assert_eq!(a, Pos2::new(10.0, 20.0));
        assert_eq!(b, Pos2::new(80.0, 70.0));
    }

    #[test]
    fn z_axis_color_matches_view_cube_blue() {
        assert_eq!(col::Z_AXIS, Color32::from_rgb(80, 140, 230));
    }

    #[test]
    fn z_axis_extends_along_positive_z_from_origin() {
        let end = Vec3::new(0.0, 0.0, GRID_EXTENT);
        assert!(end.z > 0.0);
        assert_eq!(end.x, 0.0);
        assert_eq!(end.y, 0.0);
    }

    #[test]
    fn second_viewport_click_commits_sketch() {
        assert!(should_commit_sketch_on_click(true, true, false));
        assert!(!should_commit_sketch_on_click(false, true, false));
        assert!(!should_commit_sketch_on_click(true, true, true));
        assert!(!should_commit_sketch_on_click(true, false, false));
    }

    #[test]
    fn select_all_while_focused_and_not_user_edited() {
        assert!(should_select_all_rect_value(false, true, true, false, false, false));
    }

    #[test]
    fn select_all_on_focus_gain_or_pending_focus() {
        assert!(should_select_all_rect_value(true, true, true, false, true, false));
        assert!(should_select_all_rect_value(false, true, true, true, true, false));
    }

    #[test]
    fn no_select_all_after_user_edited_without_focus_change() {
        assert!(!should_select_all_rect_value(false, true, true, false, true, false));
    }

    #[test]
    fn typing_multi_digit_value_does_not_reselect_after_each_digit() {
        // First keystroke on a live-tracked value: don't re-select after the digit lands.
        assert!(!should_select_all_rect_value(false, true, true, false, false, true));
        // Later frames while the user continues typing.
        assert!(!should_select_all_rect_value(false, true, true, false, true, false));
        assert!(!should_select_all_rect_value(false, true, true, false, true, true));
    }

    #[test]
    fn live_mouse_tracking_still_selects_before_user_types() {
        assert!(should_select_all_rect_value(false, true, true, false, false, false));
    }

    fn rectangle_anchors(shape: egui::Rect) -> (egui::Pos2, egui::Pos2) {
        (
            egui::pos2(shape.center().x, shape.max.y),
            egui::pos2(shape.min.x, shape.center().y),
        )
    }

    #[test]
    fn rectangle_dim_labels_use_preferred_offsets_when_clear() {
        use super::{
            rectangle_dim_layouts, HEIGHT_LABEL_OFFSET, WIDTH_LABEL_OFFSET,
        };
        let shape = egui::Rect::from_min_max(egui::pos2(50.0, 50.0), egui::pos2(400.0, 400.0));
        let (bottom_mid, left_mid) = rectangle_anchors(shape);
        let (width, height) = rectangle_dim_layouts(bottom_mid, left_mid, "10", "10");
        assert_eq!(width.pos, bottom_mid + WIDTH_LABEL_OFFSET);
        assert_eq!(height.pos, left_mid + HEIGHT_LABEL_OFFSET);
    }

    #[test]
    fn rectangle_dim_labels_avoid_each_other() {
        use super::{rectangle_dim_layouts, rectangle_labels_clear};
        let shape = egui::Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(200.0, 160.0));
        let (bottom_mid, left_mid) = rectangle_anchors(shape);
        let (width, height) = rectangle_dim_layouts(bottom_mid, left_mid, "10", "10");
        assert!(rectangle_labels_clear(width.rect, height.rect));
    }

    #[test]
    fn plane_angle_dim_layout_is_near_angle_gizmo_not_offset_tip() {
        use super::{
            axis_angle_handle, axis_offset_handle, plane_dim_layouts, PlaneReference,
        };
        use crate::construction::plane_from_axis;
        let reference = PlaneReference::Axis {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            label: "Line".to_string(),
        };
        let plane = plane_from_axis(20.0, 45.0, Vec3::ZERO, Vec3::X);
        let project = |w: Vec3| Some(Pos2::new(w.x, w.y));
        let layouts = plane_dim_layouts(&project, &plane, &reference, 20.0, 45.0).unwrap();
        let angle_layout = layouts.1.expect("axis mode should have angle layout");
        let angle_center = angle_layout.pos + super::dim_input_max_size() * 0.5;
        let handle_screen = project(axis_angle_handle(Vec3::ZERO, Vec3::X, 45.0)).unwrap();
        let offset_screen =
            project(axis_offset_handle(Vec3::ZERO, Vec3::X, 20.0, 45.0)).unwrap();
        assert!(
            (angle_center - handle_screen).length()
                < (angle_center - offset_screen).length()
        );
        let handle_rect =
            egui::Rect::from_center_size(handle_screen, egui::vec2(24.0, 24.0));
        assert!(!angle_layout.rect.intersects(handle_rect));
    }

    #[test]
    fn rectangle_dim_labels_push_apart_when_overlapping() {
        use super::{
            rectangle_dim_layouts, rectangle_labels_clear, HEIGHT_LABEL_OFFSET,
            WIDTH_LABEL_OFFSET,
        };
        // Very short preview: preferred width/height labels overlap near the bottom-left corner.
        let shape = egui::Rect::from_min_max(egui::pos2(300.0, 300.0), egui::pos2(340.0, 308.0));
        let (bottom_mid, left_mid) = rectangle_anchors(shape);
        let (width, height) = rectangle_dim_layouts(bottom_mid, left_mid, "10", "10");
        assert!(
            width.pos != bottom_mid + WIDTH_LABEL_OFFSET
                || height.pos != left_mid + HEIGHT_LABEL_OFFSET,
            "at least one label should move when they overlap"
        );
        assert!(rectangle_labels_clear(width.rect, height.rect));
    }

    fn line_dim_center(layout: super::DimInputLayout) -> egui::Pos2 {
        layout.pos + layout.rect.size() * 0.5
    }

    #[test]
    fn line_dim_label_stays_on_line_midpoint() {
        use super::{line_dim_layout, line_perpendicular_unit};
        let pa = egui::pos2(40.0, 180.0);
        let pb = egui::pos2(360.0, 220.0);
        let mid = pa.lerp(pb, 0.5);
        let dir = (pb - pa).normalized();
        let center = line_dim_center(line_dim_layout(pa, pb, "10"));
        let rel = center - mid;
        let along = rel.dot(dir);
        assert!(
            along.abs() < 1.0,
            "label center should sit on the line midpoint, along={along}"
        );
        let perp = line_perpendicular_unit(pa, pb);
        assert!(rel.dot(perp).abs() > 0.0);
    }

    #[test]
    fn line_dim_label_keeps_perpendicular_distance_when_line_tilts() {
        use super::{dist_rect_to_segment, line_dim_layout, LINE_LABEL_DISTANCE};
        let pa = egui::pos2(100.0, 200.0);
        for dy in [0.0, 40.0, 80.0, 120.0, -60.0] {
            let pb = egui::pos2(300.0, 200.0 + dy);
            let mid = pa.lerp(pb, 0.5);
            let dir = (pb - pa).normalized();
            let layout = line_dim_layout(pa, pb, "10");
            let center = line_dim_center(layout);
            let along = (center - mid).dot(dir);
            assert!(along.abs() < 1.0, "dy={dy}: along={along}");
            let gap = dist_rect_to_segment(layout.rect, pa, pb);
            assert!(
                (gap - LINE_LABEL_DISTANCE).abs() < 1.0,
                "dy={dy}: expected gap {LINE_LABEL_DISTANCE}, got {gap}"
            );
        }
    }

    #[test]
    fn line_dim_label_avoids_segment() {
        use super::{line_dim_layout, segment_intersects_rect, DIM_LABEL_GAP};
        let pa = egui::pos2(200.0, 200.0);
        let pb = egui::pos2(320.0, 260.0);
        let layout = line_dim_layout(pa, pb, "10");
        assert!(!segment_intersects_rect(
            pa,
            pb,
            layout.rect.expand(DIM_LABEL_GAP)
        ));
    }

    #[test]
    fn width_focus_maps_to_bottom_edge() {
        use super::{rect_dim_edge_for_focus, rect_highlight_edge, RectDimEdge};
        assert_eq!(rect_dim_edge_for_focus(0), Some(RectDimEdge::Width));
        let corners = [
            Vec3::new(1.0, 2.0, 0.0),
            Vec3::new(5.0, 2.0, 0.0),
            Vec3::new(5.0, 8.0, 0.0),
            Vec3::new(1.0, 8.0, 0.0),
        ];
        let (a, b) = rect_highlight_edge(corners, RectDimEdge::Width);
        assert_eq!(a, Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(b, Vec3::new(5.0, 2.0, 0.0));
    }

    #[test]
    fn height_focus_maps_to_left_edge() {
        use super::{rect_dim_edge_for_focus, rect_highlight_edge, RectDimEdge};
        assert_eq!(rect_dim_edge_for_focus(1), Some(RectDimEdge::Height));
        let corners = [
            Vec3::new(1.0, 2.0, 0.0),
            Vec3::new(5.0, 2.0, 0.0),
            Vec3::new(5.0, 8.0, 0.0),
            Vec3::new(1.0, 8.0, 0.0),
        ];
        let (a, b) = rect_highlight_edge(corners, RectDimEdge::Height);
        assert_eq!(a, Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(b, Vec3::new(1.0, 8.0, 0.0));
    }

    #[test]
    fn dim_input_text_width_grows_with_expression_up_to_max_chars() {
        assert!((super::dim_input_text_width("10") - 48.0).abs() < 1e-4);
        let expr = "2mm + 1ft";
        assert!(super::dim_input_text_width(expr) > 48.0);
        assert!(super::dim_input_text_width(expr) < super::dim_input_max_size().x);
        let capped = super::dim_input_text_width(&"x".repeat(30));
        let maxed = super::dim_input_text_width(&"x".repeat(20));
        assert!((capped - maxed).abs() < 1e-4);
    }

    #[test]
    fn dim_input_selection_highlight_is_faint() {
        use super::col::DIM_INPUT_SELECTION;
        assert!(
            DIM_INPUT_SELECTION.a() <= 48,
            "selection fill should be faint (alpha <= 48), got {}",
            DIM_INPUT_SELECTION.a()
        );
    }

    fn xy_frame() -> SketchFrame {
        SketchFrame {
            origin: Vec3::ZERO,
            u_axis: Vec3::X,
            v_axis: Vec3::Y,
            normal: Vec3::Z,
        }
    }

    fn make_cr(origin: (f32, f32), texts: [&str; 2], mouse: (f32, f32)) -> CreatingRect {
        CreatingRect {
            origin: Vec3::new(origin.0, origin.1, 0.0),
            texts: [texts[0].to_string(), texts[1].to_string()],
            focused: 0,
            last_mouse: Vec3::new(mouse.0, mouse.1, 0.0),
            user_edited: [true, true],
            pending_focus: false,
        }
    }

    #[test]
    fn end_point_free_follows_mouse() {
        let cr = make_cr((0., 0.), ["", ""], (10., 4.));
        let frame = xy_frame();
        let e = cr.end_point(&frame);
        assert!((e.x - 10.0).abs() < 1e-4);
        assert!((e.y - 4.0).abs() < 1e-4);
    }

    #[test]
    fn end_point_one_constrained() {
        let frame = xy_frame();
        let cr = make_cr((0., 0.), ["5", ""], (12., 3.));
        let e = cr.end_point(&frame);
        assert!((e.x - 5.0).abs() < 1e-4 && (e.y - 3.0).abs() < 1e-4);

        let cr2 = make_cr((10., 20.), ["5", ""], (3., 15.));
        let e2 = cr2.end_point(&frame);
        assert!((e2.x - 5.0).abs() < 1e-4);
        assert!((e2.y - 15.0).abs() < 1e-4);
    }

    #[test]
    fn end_point_both_constrained() {
        let frame = xy_frame();
        let cr = make_cr((0., 0.), ["3", "7"], (99., -4.));
        let e = cr.end_point(&frame);
        assert!((e.x - 3.0).abs() < 1e-4);
        assert!((e.y + 7.0).abs() < 1e-4);
    }

    #[test]
    fn end_point_invalid_text_falls_back_to_mouse() {
        let frame = xy_frame();
        let cr = make_cr((0., 0.), ["abc", "12x"], (8., 9.));
        let e = cr.end_point(&frame);
        assert!((e.x - 8.0).abs() < 1e-4);
        assert!((e.y - 9.0).abs() < 1e-4);
    }
}