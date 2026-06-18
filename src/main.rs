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

use actions::{Action, AppState, CreatingRect, RectAxis, Tool};
use eframe::egui;
use glam::Vec3;
use model::Rect;
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

        if self.state.creating_rect.is_none() && ctx.input(|i| i.key_pressed(egui::Key::R)) {
            if self.state.tool != Tool::Rectangle {
                self.state.apply(Action::SetTool(Tool::Rectangle));
            }
        }

        if self.state.tool != Tool::Rectangle {
            self.state.creating_rect = None;
        }

        let (enter_pressed, tab_pressed) = if self.state.creating_rect.is_some() {
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
                let target_id = if new_focused == 0 {
                    egui::Id::new("cr_width")
                } else {
                    egui::Id::new("cr_height")
                };
                ctx.memory_mut(|m| m.request_focus(target_id));
            }
            if enter_pressed {
                self.state.apply(Action::CommitRectangle);
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
    pub const RECT_LINE: Color32 = Color32::from_rgb(120, 170, 240);
    pub const PREVIEW: Color32 = Color32::from_rgb(240, 200, 120);
}

const GRID_EXTENT: f32 = 200.0;
const GRID_STEP: f32 = 20.0;

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
                self.state.cam.zoom(scroll);
            }
        }

        let vp = self.state.cam.view_proj(viewport);
        let project = |w: Vec3| self.state.cam.project(w, viewport, &vp);

        if self.state.tool == Tool::Rectangle {
            let ground = |p: egui::Pos2| self.state.cam.ground_point(p, viewport, &vp);
            let pointer_screen = response.hover_pos().or(response.interact_pointer_pos());

            if let Some(pp) = pointer_screen {
                if let Some(gp) = ground(pp) {
                    if self.state.creating_rect.is_none()
                        && ui.input(|i| i.pointer.primary_pressed())
                    {
                        self.state.creating_rect = Some(CreatingRect {
                            origin: gp,
                            texts: ["".to_string(), "".to_string()],
                            focused: 0,
                            last_mouse: gp,
                            user_edited: [false, false],
                        });
                        self.state.status = "Move mouse • type to lock dim • Tab cycle • Enter commit • Esc cancel".to_string();
                        ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("cr_width")));
                    }

                    if let Some(cr) = &mut self.state.creating_rect {
                        let cur_end = cr.end_point();
                        let x0 = cr.origin.x.min(cur_end.x);
                        let y0 = cr.origin.y.min(cur_end.y);
                        let x1 = cr.origin.x.max(cur_end.x);
                        let y1 = cr.origin.y.max(cur_end.y);
                        let mid_w = Vec3::new((x0 + x1) * 0.5, y0, 0.0);
                        let mid_h = Vec3::new(x0, (y0 + y1) * 0.5, 0.0);
                        let pw = project(mid_w);
                        let ph = project(mid_h);

                        let mut over_input = false;
                        if let (Some(pw), Some(ph)) = (pw, ph) {
                            let r_w = egui::Rect::from_min_size(
                                pw + egui::vec2(-20.0, 14.0),
                                egui::vec2(55.0, 20.0),
                            );
                            let r_h = egui::Rect::from_min_size(
                                ph + egui::vec2(-48.0, -4.0),
                                egui::vec2(55.0, 20.0),
                            );
                            if r_w.contains(pp) || r_h.contains(pp) {
                                over_input = true;
                            }
                        }

                        if !over_input {
                            cr.last_mouse = gp;
                            let rw = (gp.x - cr.origin.x).abs();
                            let rh = (gp.y - cr.origin.y).abs();
                            let fm = |v: f32| -> String {
                                if v < 0.1 {
                                    "0".to_string()
                                } else {
                                    format!("{:.1}", v)
                                }
                            };
                            if !cr.user_edited[0] {
                                cr.texts[0] = fm(rw);
                            }
                            if !cr.user_edited[1] {
                                cr.texts[1] = fm(rh);
                            }
                        }
                    }
                }
            }
        }

        draw_ground(&painter, &project);

        for r in &self.state.doc.rects {
            draw_rect(&painter, &project, *r, col::RECT_LINE, true);
        }
        if let Some(cr) = &self.state.creating_rect {
            let end = cr.end_point();
            let preview = Rect::from_corners(cr.origin.x, cr.origin.y, end.x, end.y);
            draw_rect(&painter, &project, preview, col::PREVIEW, false);
            if let Some(sp) = project(cr.origin) {
                painter.circle_filled(sp, 3.5, col::PREVIEW);
            }
        }

        if let Some(cr) = &mut self.state.creating_rect {
            let end = cr.end_point();
            let x0 = cr.origin.x.min(end.x);
            let y0 = cr.origin.y.min(end.y);
            let x1 = cr.origin.x.max(end.x);
            let y1 = cr.origin.y.max(end.y);
            let mid_w = Vec3::new((x0 + x1) * 0.5, y0, 0.0);
            let mid_h = Vec3::new(x0, (y0 + y1) * 0.5, 0.0);
            let pw = project(mid_w);
            let ph = project(mid_h);
            if let (Some(pw), Some(ph)) = (pw, ph) {
                let ctx = ui.ctx();
                let id_w = egui::Id::new("cr_width");
                let id_h = egui::Id::new("cr_height");

                egui::Area::new(egui::Id::new("cr_width_area"))
                    .fixed_pos(pw + egui::vec2(-20.0, 14.0))
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        ui.style_mut().spacing.text_edit_width = 48.0;
                        ui.visuals_mut().widgets.inactive.bg_fill = egui::Color32::from_gray(32);
                        ui.visuals_mut().widgets.inactive.fg_stroke =
                            egui::Stroke::new(1.0, egui::Color32::from_gray(230));
                        ui.visuals_mut().widgets.active.bg_fill = egui::Color32::from_gray(50);
                        ui.visuals_mut().widgets.active.fg_stroke =
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 220, 150));
                        let te = egui::TextEdit::singleline(&mut cr.texts[0])
                            .id_source(id_w)
                            .desired_width(48.0)
                            .font(egui::FontId::proportional(11.0))
                            .margin(egui::vec2(2.0, 1.0));
                        let resp = ui.add(te);
                        if resp.changed() {
                            cr.user_edited[0] = true;
                        }
                    });

                egui::Area::new(egui::Id::new("cr_height_area"))
                    .fixed_pos(ph + egui::vec2(-48.0, -4.0))
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        ui.style_mut().spacing.text_edit_width = 48.0;
                        ui.visuals_mut().widgets.inactive.bg_fill = egui::Color32::from_gray(32);
                        ui.visuals_mut().widgets.inactive.fg_stroke =
                            egui::Stroke::new(1.0, egui::Color32::from_gray(230));
                        ui.visuals_mut().widgets.active.bg_fill = egui::Color32::from_gray(50);
                        ui.visuals_mut().widgets.active.fg_stroke =
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 220, 150));
                        let te = egui::TextEdit::singleline(&mut cr.texts[1])
                            .id_source(id_h)
                            .desired_width(48.0)
                            .font(egui::FontId::proportional(11.0))
                            .margin(egui::vec2(2.0, 1.0));
                        let resp = ui.add(te);
                        if resp.changed() {
                            cr.user_edited[1] = true;
                        }
                    });

                let current = ctx.memory(|m| m.focused());
                if current == Some(id_w) {
                    cr.focused = 0;
                } else if current == Some(id_h) {
                    cr.focused = 1;
                }

                if current != Some(id_w) && current != Some(id_h) {
                    let target_id = if cr.focused == 0 { id_w } else { id_h };
                    ctx.memory_mut(|m| m.request_focus(target_id));
                }
            }
        }

        let hint = match self.state.tool {
            Tool::Select => {
                "Right-drag: orbit  •  Shift+right-drag: pan  •  Wheel: zoom  •  r: rectangle"
            }
            Tool::Rectangle => {
                if self.state.creating_rect.is_some() {
                    "Move mouse (free dim) • Type in focused input to constrain • Tab: switch dims • Enter: create rect • Esc: cancel"
                } else {
                    "r: rectangle  •  Left-click to set corner • move to size • Right-drag: orbit  • Shift+right-drag: pan  •  Wheel: zoom"
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

fn rect_corners(r: Rect) -> [Vec3; 4] {
    [
        Vec3::new(r.x, r.y, 0.0),
        Vec3::new(r.x + r.w, r.y, 0.0),
        Vec3::new(r.x + r.w, r.y + r.h, 0.0),
        Vec3::new(r.x, r.y + r.h, 0.0),
    ]
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

fn draw_ground(painter: &egui::Painter, project: &impl Fn(Vec3) -> Option<egui::Pos2>) {
    let e = GRID_EXTENT;
    let line = |a: Vec3, b: Vec3, color: egui::Color32, w: f32| {
        if let (Some(pa), Some(pb)) = (project(a), project(b)) {
            painter.line_segment([pa, pb], egui::Stroke::new(w, color));
        }
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
}

#[cfg(test)]
mod tests {
    use super::actions::CreatingRect;
    use glam::Vec3;

    fn make_cr(origin: (f32, f32), texts: [&str; 2], mouse: (f32, f32)) -> CreatingRect {
        CreatingRect {
            origin: Vec3::new(origin.0, origin.1, 0.0),
            texts: [texts[0].to_string(), texts[1].to_string()],
            focused: 0,
            last_mouse: Vec3::new(mouse.0, mouse.1, 0.0),
            user_edited: [true, true],
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