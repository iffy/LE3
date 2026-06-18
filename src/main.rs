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
mod model;
mod script;
mod storage;
mod view_cube;

use actions::{Action, AppState, CreatingLine, CreatingRect, RectAxis, Tool};
use eframe::egui;
use glam::Vec3;
use model::{Line, Rect};
use script::{ScriptRunner, SyntheticInput};
use std::path::Path;

fn main() -> eframe::Result<()> {
    let script_opts = script::parse_args(std::env::args());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 640.0])
            .with_title("LE3")
            .with_icon(std::sync::Arc::new(egui::IconData::default())),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

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
        Box::new(move |_cc| {
            Ok(Box::new(App::new(script, script_opts.exit_on_complete)) as Box<dyn eframe::App>)
        }),
    )
}

struct App {
    state: AppState,
    synthetic: SyntheticInput,
    script: Option<ScriptRunner>,
    exit_on_script_complete: bool,
    last_viewport: Option<egui::Rect>,
}

impl App {
    fn new(script: Option<ScriptRunner>, exit_on_script_complete: bool) -> Self {
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

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.state.apply(Action::CancelOperation);
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
            && ctx.input(|i| i.key_pressed(egui::Key::L))
        {
            if self.state.tool != Tool::Line {
                self.state.apply(Action::SetTool(Tool::Line));
            }
        }

        if self.state.tool != Tool::Rectangle {
            self.state.creating_rect = None;
        }
        if self.state.tool != Tool::Line {
            self.state.creating_line = None;
        }

        let creating = self.state.creating_rect.is_some() || self.state.creating_line.is_some();
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

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("New").clicked() {
                    self.state.apply(Action::NewDocument);
                }
                if ui.button("Open…").clicked() {
                    self.open();
                }
                if ui.button("Save").clicked() {
                    self.save();
                }
                if ui.button("Save As…").clicked() {
                    self.save_as();
                }
                ui.separator();
                ui.selectable_value(&mut self.state.tool, Tool::Select, "Select");
                ui.selectable_value(&mut self.state.tool, Tool::Rectangle, "Rectangle");
                ui.selectable_value(&mut self.state.tool, Tool::Line, "Line");
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
}

const GRID_EXTENT: f32 = 200.0;
const GRID_STEP: f32 = 20.0;

/// Screen-space size of a floating dimension input (frame + text field).
const DIM_INPUT_SIZE: egui::Vec2 = egui::Vec2::new(58.0, 26.0);
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

fn dim_input_rect_at(top_left: egui::Pos2) -> egui::Rect {
    egui::Rect::from_min_size(top_left, DIM_INPUT_SIZE)
}

fn layout_at(pos: egui::Pos2) -> DimInputLayout {
    DimInputLayout {
        pos,
        rect: dim_input_rect_at(pos),
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
        let w_rect = dim_input_rect_at(width_pos);
        let h_rect = dim_input_rect_at(height_pos);
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
) -> (DimInputLayout, DimInputLayout) {
    let (width_pos, height_pos) = resolve_rectangle_dim_positions(bottom_mid, left_mid);
    let width = layout_at(width_pos);
    let height = layout_at(height_pos);
    debug_assert!(rectangle_labels_clear(width.rect, height.rect));
    (width, height)
}

fn rectangle_dim_layout_from_world(
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
) -> Option<(DimInputLayout, DimInputLayout)> {
    let bottom_mid = project(Vec3::new((x0 + x1) * 0.5, y0, 0.0))?;
    let left_mid = project(Vec3::new(x0, (y0 + y1) * 0.5, 0.0))?;
    Some(rectangle_dim_layouts(bottom_mid, left_mid))
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

fn aabb_half_extent_along(dir: egui::Vec2) -> f32 {
    if dir.length_sq() < 1e-8 {
        return 0.0;
    }
    let n = dir.normalized();
    DIM_INPUT_SIZE.x * 0.5 * n.x.abs() + DIM_INPUT_SIZE.y * 0.5 * n.y.abs()
}

fn line_dim_top_left(pa: egui::Pos2, pb: egui::Pos2, gap_from_line: f32) -> egui::Pos2 {
    let mid = pa.lerp(pb, 0.5);
    let perp = line_perpendicular_unit(pa, pb);
    let center_dist = gap_from_line + aabb_half_extent_along(-perp);
    let center = mid + perp * center_dist;
    center - DIM_INPUT_SIZE * 0.5
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

fn line_dim_layout(pa: egui::Pos2, pb: egui::Pos2) -> DimInputLayout {
    let mut gap = LINE_LABEL_DISTANCE;
    for _ in 0..DIM_REPULSION_ITERS {
        let pos = line_dim_top_left(pa, pb, gap);
        let rect = dim_input_rect_at(pos).expand(DIM_LABEL_GAP);
        if !segment_intersects_rect(pa, pb, rect) {
            return layout_at(pos);
        }
        gap += 2.0;
    }
    layout_at(line_dim_top_left(pa, pb, gap))
}

fn pointer_over_dim_inputs(pointer: egui::Pos2, layouts: &[DimInputLayout]) -> bool {
    layouts.iter().any(|layout| layout.rect.contains(pointer))
}

fn format_live_dimension(v: f32) -> String {
    if v < 0.1 {
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

    let output = frame.show(ui, |ui| {
        ui.style_mut().spacing.text_edit_width = 48.0;
        ui.visuals_mut().selection.bg_fill = col::DIM_INPUT_SELECTION;
        egui::TextEdit::singleline(text)
            .id(id)
            .frame(false)
            .desired_width(48.0)
            .font(egui::FontId::monospace(13.0))
            .text_color(if has_focus {
                col::DIM_INPUT_TEXT_FOCUS
            } else {
                col::DIM_INPUT_TEXT
            })
            .margin(egui::vec2(0.0, 0.0))
            .show(ui)
    }).inner;
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

        if self.state.tool == Tool::Rectangle {
            let ground = |p: egui::Pos2| cam.ground_point(p, viewport, &vp);
            let pointer_screen = response.hover_pos().or(response.interact_pointer_pos());

            if let Some(pp) = pointer_screen {
                if let Some(gp) = ground(pp) {
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
                        let cur_end = cr.end_point();
                        let x0 = cr.origin.x.min(cur_end.x);
                        let y0 = cr.origin.y.min(cur_end.y);
                        let x1 = cr.origin.x.max(cur_end.x);
                        let y1 = cr.origin.y.max(cur_end.y);
                        let dim_layouts = rectangle_dim_layout_from_world(&project, x0, y0, x1, y1);
                        let over_input = dim_layouts
                            .as_ref()
                            .is_some_and(|(w, h)| w.rect.contains(pp) || h.rect.contains(pp));

                        if should_commit_sketch_on_click(was_creating, primary_pressed, over_input) {
                            commit_click = true;
                        } else if !over_input {
                            cr.last_mouse = gp;
                            let rw = (gp.x - cr.origin.x).abs();
                            let rh = (gp.y - cr.origin.y).abs();
                            if !cr.user_edited[0] {
                                cr.texts[0] = format_live_dimension(rw);
                            }
                            if !cr.user_edited[1] {
                                cr.texts[1] = format_live_dimension(rh);
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
            let ground = |p: egui::Pos2| cam.ground_point(p, viewport, &vp);
            let pointer_screen = response.hover_pos().or(response.interact_pointer_pos());

            if let Some(pp) = pointer_screen {
                if let Some(gp) = ground(pp) {
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
                        let end = cl.end_point();
                        let over_input = project(Vec3::new(cl.origin.x, cl.origin.y, 0.0))
                            .zip(project(Vec3::new(end.x, end.y, 0.0)))
                            .is_some_and(|(pa, pb)| {
                                pointer_over_dim_inputs(pp, &[line_dim_layout(pa, pb)])
                            });

                        if should_commit_sketch_on_click(was_creating, primary_pressed, over_input)
                        {
                            commit_click = true;
                        } else if !over_input {
                            cl.last_mouse = gp;
                            if !cl.user_edited {
                                let dx = gp.x - cl.origin.x;
                                let dy = gp.y - cl.origin.y;
                                cl.text = format_live_dimension((dx * dx + dy * dy).sqrt());
                            }
                        }
                    }
                    if commit_click {
                        self.state.apply(Action::CommitLine);
                    }
                }
            }
        }

        draw_ground(&painter, &project, viewport);

        for r in &self.state.doc.rects {
            draw_rect(&painter, &project, *r, col::RECT_LINE, true);
        }
        for line in &self.state.doc.lines {
            draw_line_segment(&painter, &project, *line, col::LINE_STROKE, 2.0);
        }
        if let Some(cr) = &self.state.creating_rect {
            let end = cr.end_point();
            let preview = Rect::from_corners(cr.origin.x, cr.origin.y, end.x, end.y);
            draw_rect(&painter, &project, preview, col::PREVIEW, false);
            if let Some(sp) = project(cr.origin) {
                painter.circle_filled(sp, 3.5, col::PREVIEW);
            }
        }
        if let Some(cl) = &self.state.creating_line {
            let end = cl.end_point();
            let preview = Line::from_endpoints(cl.origin.x, cl.origin.y, end.x, end.y);
            draw_line_segment(&painter, &project, preview, col::PREVIEW, 2.0);
            if let Some(sp) = project(cl.origin) {
                painter.circle_filled(sp, 3.5, col::PREVIEW);
            }
        }

        if let Some(cr) = &mut self.state.creating_rect {
            let end = cr.end_point();
            let x0 = cr.origin.x.min(end.x);
            let y0 = cr.origin.y.min(end.y);
            let x1 = cr.origin.x.max(end.x);
            let y1 = cr.origin.y.max(end.y);
            if let Some((width_layout, height_layout)) =
                rectangle_dim_layout_from_world(&project, x0, y0, x1, y1)
            {
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
                    let (a, b) = rect_edge_endpoints(x0, y0, x1, y1, edge);
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

        if let Some(cl) = &mut self.state.creating_line {
            let end = cl.end_point();
            if let (Some(pa), Some(pb)) = (
                project(Vec3::new(cl.origin.x, cl.origin.y, 0.0)),
                project(Vec3::new(end.x, end.y, 0.0)),
            ) {
                let layout = line_dim_layout(pa, pb);
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
                    let preview = Line::from_endpoints(cl.origin.x, cl.origin.y, end.x, end.y);
                    draw_line_segment(
                        &painter,
                        &project,
                        preview,
                        col::DIM_EDGE_HIGHLIGHT,
                        3.5,
                    );
                }
            }
        }

        view_cube::show_hud(ui.ctx(), &mut self.state.cam, viewport);

        let hint = match self.state.tool {
            Tool::Select => {
                "Right-drag: orbit  •  Shift+right-drag: pan  •  Wheel: zoom  •  r: rectangle  •  l: line"
            }
            Tool::Rectangle => {
                if self.state.creating_rect.is_some() {
                    "Move mouse (free dim) • Type in focused input to constrain • Tab: switch dims • Click/Enter: create rect • Esc: cancel"
                } else {
                    "r: rectangle  •  Left-click to set corner • move to size • Right-drag: orbit  • Shift+right-drag: pan  •  Wheel: zoom"
                }
            }
            Tool::Line => {
                if self.state.creating_line.is_some() {
                    "Move mouse (free length) • Type in length input to constrain • Click/Enter: create line • Esc: cancel"
                } else {
                    "l: line  •  Left-click to set start • move to aim • Right-drag: orbit  • Shift+right-drag: pan  •  Wheel: zoom"
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

fn rect_edge_endpoints(x0: f32, y0: f32, x1: f32, y1: f32, edge: RectDimEdge) -> (Vec3, Vec3) {
    match edge {
        RectDimEdge::Width => (Vec3::new(x0, y0, 0.0), Vec3::new(x1, y0, 0.0)),
        RectDimEdge::Height => (Vec3::new(x0, y0, 0.0), Vec3::new(x0, y1, 0.0)),
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

fn rect_corners(r: Rect) -> [Vec3; 4] {
    [
        Vec3::new(r.x, r.y, 0.0),
        Vec3::new(r.x + r.w, r.y, 0.0),
        Vec3::new(r.x + r.w, r.y + r.h, 0.0),
        Vec3::new(r.x, r.y + r.h, 0.0),
    ]
}

fn draw_line_segment(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    line: Line,
    color: egui::Color32,
    width: f32,
) {
    let a = Vec3::new(line.x0, line.y0, 0.0);
    let b = Vec3::new(line.x1, line.y1, 0.0);
    if let (Some(pa), Some(pb)) = (project(a), project(b)) {
        painter.line_segment([pa, pb], egui::Stroke::new(width, color));
    }
}

fn draw_rect(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    r: Rect,
    color: egui::Color32,
    fill: bool,
) {
    let pts: Option<Vec<egui::Pos2>> = rect_corners(r).iter().map(|&c| project(c)).collect();
    let Some(pts) = pts else { return };
    if fill {
        painter.add(egui::Shape::convex_polygon(
            pts.clone(),
            color.gamma_multiply(0.25),
            egui::Stroke::new(1.5, color),
        ));
    } else {
        painter.add(egui::Shape::closed_line(
            pts,
            egui::Stroke::new(1.5, color),
        ));
    }
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

fn draw_ground(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    viewport: egui::Rect,
) {
    let e = GRID_EXTENT;
    let line = |a: Vec3, b: Vec3, color: egui::Color32, w: f32| {
        draw_clipped_world_segment(painter, project, viewport, a, b, color, w);
    };

    let mut t = -e;
    while t <= e + 0.001 {
        let color = if t.abs() < 0.001 {
            col::GRID_AXIS
        } else {
            col::GRID
        };
        line(Vec3::new(-e, t, 0.0), Vec3::new(e, t, 0.0), color, 1.0);
        line(Vec3::new(t, -e, 0.0), Vec3::new(t, e, 0.0), color, 1.0);
        t += GRID_STEP;
    }

    line(Vec3::ZERO, Vec3::new(e, 0.0, 0.0), col::X_AXIS, 2.0);
    line(Vec3::ZERO, Vec3::new(0.0, e, 0.0), col::Y_AXIS, 2.0);
    line(Vec3::ZERO, Vec3::new(0.0, 0.0, e), col::Z_AXIS, 2.0);
}

#[cfg(test)]
mod tests {
    use super::actions::CreatingRect;
    use super::{
        clip_segment_to_rect, col, should_commit_sketch_on_click, should_select_all_rect_value,
        GRID_EXTENT,
    };
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
        let (width, height) = rectangle_dim_layouts(bottom_mid, left_mid);
        assert_eq!(width.pos, bottom_mid + WIDTH_LABEL_OFFSET);
        assert_eq!(height.pos, left_mid + HEIGHT_LABEL_OFFSET);
    }

    #[test]
    fn rectangle_dim_labels_avoid_each_other() {
        use super::{rectangle_dim_layouts, rectangle_labels_clear};
        let shape = egui::Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(200.0, 160.0));
        let (bottom_mid, left_mid) = rectangle_anchors(shape);
        let (width, height) = rectangle_dim_layouts(bottom_mid, left_mid);
        assert!(rectangle_labels_clear(width.rect, height.rect));
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
        let (width, height) = rectangle_dim_layouts(bottom_mid, left_mid);
        assert!(
            width.pos != bottom_mid + WIDTH_LABEL_OFFSET
                || height.pos != left_mid + HEIGHT_LABEL_OFFSET,
            "at least one label should move when they overlap"
        );
        assert!(rectangle_labels_clear(width.rect, height.rect));
    }

    fn line_dim_center(layout: super::DimInputLayout) -> egui::Pos2 {
        layout.pos + super::DIM_INPUT_SIZE * 0.5
    }

    #[test]
    fn line_dim_label_stays_on_line_midpoint() {
        use super::{line_dim_layout, line_perpendicular_unit};
        let pa = egui::pos2(40.0, 180.0);
        let pb = egui::pos2(360.0, 220.0);
        let mid = pa.lerp(pb, 0.5);
        let dir = (pb - pa).normalized();
        let center = line_dim_center(line_dim_layout(pa, pb));
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
            let layout = line_dim_layout(pa, pb);
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
        let layout = line_dim_layout(pa, pb);
        assert!(!segment_intersects_rect(
            pa,
            pb,
            layout.rect.expand(DIM_LABEL_GAP)
        ));
    }

    #[test]
    fn width_focus_maps_to_bottom_edge() {
        use super::{rect_dim_edge_for_focus, rect_edge_endpoints, RectDimEdge};
        assert_eq!(rect_dim_edge_for_focus(0), Some(RectDimEdge::Width));
        let (a, b) = rect_edge_endpoints(1.0, 2.0, 5.0, 8.0, RectDimEdge::Width);
        assert_eq!(a, Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(b, Vec3::new(5.0, 2.0, 0.0));
    }

    #[test]
    fn height_focus_maps_to_left_edge() {
        use super::{rect_dim_edge_for_focus, rect_edge_endpoints, RectDimEdge};
        assert_eq!(rect_dim_edge_for_focus(1), Some(RectDimEdge::Height));
        let (a, b) = rect_edge_endpoints(1.0, 2.0, 5.0, 8.0, RectDimEdge::Height);
        assert_eq!(a, Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(b, Vec3::new(1.0, 8.0, 0.0));
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
        let e = cr.end_point();
        assert!((e.x - 10.0).abs() < 1e-4);
        assert!((e.y - 4.0).abs() < 1e-4);
    }

    #[test]
    fn end_point_one_constrained() {
        let cr = make_cr((0., 0.), ["5", ""], (12., 3.));
        let e = cr.end_point();
        assert!((e.x - 5.0).abs() < 1e-4 && (e.y - 3.0).abs() < 1e-4);

        let cr2 = make_cr((10., 20.), ["5", ""], (3., 15.));
        let e2 = cr2.end_point();
        assert!((e2.x - 5.0).abs() < 1e-4);
        assert!((e2.y - 15.0).abs() < 1e-4);
    }

    #[test]
    fn end_point_both_constrained() {
        let cr = make_cr((0., 0.), ["3", "7"], (99., -4.));
        let e = cr.end_point();
        assert!((e.x - 3.0).abs() < 1e-4);
        assert!((e.y + 7.0).abs() < 1e-4);
    }

    #[test]
    fn end_point_invalid_text_falls_back_to_mouse() {
        let cr = make_cr((0., 0.), ["abc", "12x"], (8., 9.));
        let e = cr.end_point();
        assert!((e.x - 8.0).abs() < 1e-4);
        assert!((e.y - 9.0).abs() < 1e-4);
    }
}