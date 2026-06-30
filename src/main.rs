//! LE3 — early prototype GUI.
//!
//! Rectangle tool: click to fix first corner, move mouse for second, with live
//! dimension inputs on the sides. Type to constrain a side, Tab to cycle,
//! Enter to commit. Right-drag orbit, wheel zoom. Save/Open .le3. (prototype)
//!
//! Fully scriptable via Lua files (SPEC §8):
//!   le3 --script demo.lua
//!   le3 --exit
//!   le3 drawing.le3 --exit
//!   le3 demo.lua --exit

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod camera;
mod command_log;
mod command_palette;
mod constraints;
mod constraint_viewport;
mod geometric_constraints;
mod context;
mod construction;
mod dimensions;
mod document_health;
mod document_lifecycle;
mod expression_input;
mod extrude;
mod face;
mod gpu_view_cube;
mod gpu_viewport;
mod hierarchy;
mod icons;
mod names;
mod parameters;

mod model;
mod native_menu;
mod lua_script;
mod release_artifacts;
mod script;
mod selection;
mod shortcuts;
mod sketch_solver;
mod snapping;
mod stl;
mod storage;
mod theme;
mod value;
mod vertex_drag;
mod view_cube;

use actions::{
    angle_gizmo_constraint_for_edit, constraint_is_angle, constraint_is_circle_diameter, Action,
    AppState, CreatingCircle, CreatingConstructionPlane, CreatingExtrusion, CreatingLine,
    CreatingRect,
    DimEditTarget, DimLabelTarget, Pane, RectAxis, SketchSession, Tool,
};
use constraint_viewport::{
    build_constraint_icon_hits, draw_constraint_connectors, draw_constraint_icons,
    pointer_over_constraint_icon, viewport_constraints_for_selection,
};
use constraints::{
    constraint_evaluated_length, constraint_segment_endpoints, distance_target_from_pick,
    distance_target_segment_endpoints,
};
use std::collections::HashSet;
use command_palette::{commands_for_state, filter_commands, show_palette, PaletteOutcome};
use hierarchy::SceneElement;
use selection::additive_click_modifiers;
use construction::{
    angle_from_axis_plane_hit, axis_angle_handle, axis_gizmo_hit, axis_normal,
    axis_offset_handle, draw_axis_plane_gizmo, draw_circle_face_highlight, draw_offset_gizmo,
    draw_polygon_face_highlight, draw_quad_face_highlight,
    nearest_sketch_line_in_sketch, nearest_sketch_point_in_sketch, offset_from_normal_drag,
    offset_gizmo_hit, offset_handle,
    parent_from_pick_target, plane_corners, point_world_position, preview_plane_edit_dependents,
    rect_edge_segments, resolve_pick_target, scene_element_from_pick, AxisGizmoDrag,
    AxisGizmoHit, PlaneDim, PlaneReference, AXIS_GIZMO_HANDLE_HIT_RADIUS_PX, PLANE_DISPLAY_HALF,
};
use document_health::{health_tint_color, DocumentHealth, HealthStatus};
use document_lifecycle::{circle_alive, constraint_alive, line_alive, rect_alive};
use constraints::{
    angle_constraint_display, angle_rad_from_sketch_hit, constraint_evaluated_angle,
    AngleConstraintDisplay,
};
use dimensions::{
    angle_gizmo_handle_hit, angle_gizmo_handle_world, arc_dimension_world_geom,
    circle_diameter_dimension_world_geom, circle_diameter_label_outward_px,
    draw_angle_constraint_annotation, draw_linear_dimension, effective_circle_diameter_label_offset,
    effective_arc_dim_offset, effective_dim_offset, planar_dimension_label_layout, PlanarLabelView,
    linear_dimension_world_geom,
    outward_perpendicular_uv, pixels_to_world_distance, preferred_outward_uv,
    project_arc_dimension_geom, project_linear_dimension_geom, uv_dir_to_world,
    EXTENSION_OVERSHOOT, LABEL_FONT_SIZE, LABEL_OUTSET,
};
use face::{
    circle_world_diameter_endpoints, circle_world_perimeter,
    line_world_endpoints, pick_sketch_face, rect_world_corners, sketch_frame,
    sketch_geometry_frame, sketch_label, world_to_local,
};
use model::SketchId;
use model::{
    Circle, ConstraintKind, ConstraintPoint, DistanceTarget, FaceId, Line, Rect, RectEdge,
};
use eframe::egui;
use native_menu::{MenuCommand, NativeMenu};
use glam::Vec3;
use model::ConstructionPlane;
use script::{ScriptRunner, SyntheticInput};
use std::path::Path;
use expression_input::{
    expression_autocomplete_handle_keys, expression_autocomplete_show_dropdown,
    length_expression_field_errors, show_expression_error_tooltips_above, INVALID_BG,
    INVALID_BORDER, INVALID_TEXT,
};
use value::{
    computed_length_in_doc, format_diameter_display, format_length_display,
    shows_computed_length_in_doc,
};

/// macOS maximize must run after eframe shows the window (post-first-paint).
fn uses_deferred_launch_maximize() -> bool {
    cfg!(target_os = "macos")
}

/// Frames to wait after startup before sending maximize on macOS.
const MACOS_LAUNCH_MAXIMIZE_DELAY_FRAMES: u8 = 2;

fn initial_launch_maximize_frames() -> u8 {
    if uses_deferred_launch_maximize() {
        MACOS_LAUNCH_MAXIMIZE_DELAY_FRAMES
    } else {
        0
    }
}

fn tick_launch_maximize(frames_remaining: &mut u8, ctx: &egui::Context) {
    if *frames_remaining == 0 {
        return;
    }
    *frames_remaining -= 1;
    if *frames_remaining == 0 {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }
}

fn native_options() -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([960.0, 640.0])
        .with_title("LE3")
        .with_icon(std::sync::Arc::new(egui::IconData::default()));
    if !uses_deferred_launch_maximize() {
        viewport = viewport.with_maximized(true);
    }

    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        let mut options = eframe::NativeOptions {
            viewport,
            renderer: eframe::Renderer::Wgpu,
            ..Default::default()
        };
        options.event_loop_builder = Some(Box::new(|builder| {
            builder.with_default_menu(false);
        }));
        options
    }
    #[cfg(not(target_os = "macos"))]
    {
        eframe::NativeOptions {
            viewport,
            renderer: eframe::Renderer::Wgpu,
            ..Default::default()
        }
    }
}

fn main() -> eframe::Result<()> {
    match script::parse_cli(std::env::args()) {
        script::CliOutcome::Help => {
            script::print_usage();
            return Ok(());
        }
        script::CliOutcome::Run(script_opts) => run_app(script_opts),
    }
}

fn run_app(script_opts: script::ScriptOptions) -> eframe::Result<()> {
    let options = native_options();

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
            theme::apply(&cc.egui_ctx);
            let native_menu = NativeMenu::install(cc).map_err(|e| {
                eframe::Error::AppCreation(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                )))
            })?;
            Ok(Box::new(App::new(
                cc,
                script,
                script_opts.document_path,
                script_opts.exit_on_complete,
                script_opts.show_commands,
                native_menu,
            )) as Box<dyn eframe::App>)
        }),
    )
}

#[cfg(test)]
mod cli_tests {
    use super::script;

    #[test]
    fn help_outcome_is_distinct_from_default_run() {
        assert_ne!(
            script::parse_cli(["le3", "--help"]),
            script::CliOutcome::Run(script::ScriptOptions::default())
        );
    }
}

const DIM_LABEL_DRAG_THRESHOLD_PX: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct DimLabelDrag {
    target: DimLabelTarget,
    outward: egui::Vec2,
    start_offset: f32,
    anchor_screen: egui::Pos2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AngleGizmoDrag {
    constraint_id: DimLabelTarget,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ExtrudeGizmoDrag {
    start_screen: egui::Pos2,
    start_distance: f32,
}

struct VertexDrag {
    point: ConstraintPoint,
}

#[derive(Clone, Debug, PartialEq)]
struct CommittedDimLayout {
    target: DimLabelTarget,
    geom: dimensions::LinearDimensionGeom,
    world_geom: dimensions::LinearDimensionWorldGeom,
    arc_geom: Option<dimensions::ArcDimensionGeom>,
    angle_display: Option<AngleConstraintDisplay>,
    angle_radius_world: f32,
    label: String,
    label_rect: egui::Rect,
    outward: egui::Vec2,
    offset: f32,
}

struct App {
    state: AppState,
    synthetic: SyntheticInput,
    script: Option<ScriptRunner>,
    exit_on_script_complete: bool,
    exit_after_startup: bool,
    exit_after_startup_sent: bool,
    show_commands: bool,
    last_viewport: Option<egui::Rect>,
    native_menu: NativeMenu,
    dim_label_drag: Option<DimLabelDrag>,
    angle_gizmo_drag: Option<AngleGizmoDrag>,
    vertex_drag: Option<VertexDrag>,
    extrude_gizmo_drag: Option<ExtrudeGizmoDrag>,
    /// Object the extrude gizmo is currently snapped to (applied on release).
    pending_extrude_target: Option<model::ExtrudeTarget>,
    launch_maximize_frames_remaining: u8,
    gpu_viewport: bool,
    gpu_view_cube: bool,
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        script: Option<ScriptRunner>,
        document_path: Option<String>,
        exit_on_script_complete: bool,
        show_commands: bool,
        native_menu: NativeMenu,
    ) -> Self {
        let status = if script.is_some() {
            "Running script…".to_string()
        } else {
            String::new()
        };
        let mut state = AppState {
            status,
            ..AppState::default()
        };
        if let Some(path) = document_path {
            match state.apply(Action::Open { path }) {
                actions::ActionResult::Err(message) => state.status = message,
                _ => {}
            }
        }
        if show_commands && script.is_none() {
            state.command_log = Some(std::cell::RefCell::new(command_log::CommandLog::new()));
        }
        let exit_after_startup = exit_on_script_complete && script.is_none();
        Self {
            state,
            synthetic: SyntheticInput::default(),
            script,
            exit_on_script_complete,
            exit_after_startup,
            exit_after_startup_sent: false,
            show_commands,
            last_viewport: None,
            native_menu,
            dim_label_drag: None,
            angle_gizmo_drag: None,
            extrude_gizmo_drag: None,
            pending_extrude_target: None,
            vertex_drag: None,
            launch_maximize_frames_remaining: initial_launch_maximize_frames(),
            gpu_viewport: gpu_viewport::install(cc),
            gpu_view_cube: gpu_view_cube::install(cc),
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

    /// Export all bodies to an STL file chosen via a save dialog (File → Export STL…).
    fn export_stl_all(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("STL mesh", &["stl"])
            .set_file_name("model.stl")
            .save_file();
        if let Some(path) = picked {
            self.state.apply(Action::ExportStl {
                path: path.to_string_lossy().to_string(),
                body: None,
            });
        }
    }

    /// Export a single body (by index) to an STL file chosen via a save dialog.
    fn export_stl_body(&mut self, body: usize) {
        let default_name = self
            .state
            .doc
            .bodies
            .get(body)
            .and_then(|b| b.name.clone())
            .unwrap_or_else(|| format!("body-{body}"));
        let picked = rfd::FileDialog::new()
            .add_filter("STL mesh", &["stl"])
            .set_file_name(format!("{default_name}.stl"))
            .save_file();
        if let Some(path) = picked {
            self.state.apply(Action::ExportStlBody {
                path: path.to_string_lossy().to_string(),
                body,
            });
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
                MenuCommand::ExportStl => self.export_stl_all(),
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

    fn dispatch_palette_outcome(&mut self, outcome: PaletteOutcome) {
        match outcome {
            PaletteOutcome::Action(action) => {
                self.state.apply(action);
            }
            PaletteOutcome::OpenFile => self.open(),
            PaletteOutcome::SaveFile => self.save(),
            PaletteOutcome::SaveFileAs => self.save_as(),
        }
        self.state.command_palette.close_palette();
    }

    fn handle_keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        if self.state.command_palette.open {
            return;
        }

        // While any text field has focus, leave unmodified keys to the input (e.g. "bar" must not
        // switch tools on "r"). Modifier shortcuts (Cmd/Ctrl+P, etc.) use the OS menu layer.
        if !keyboard_shortcuts_suppressed(ctx) {
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
                && ctx.input(|i| i.key_pressed(egui::Key::L))
            {
                if self.state.tool != Tool::Line {
                    self.state.apply(Action::SetTool(Tool::Line));
                }
            }

            if self.state.creating_rect.is_none()
                && self.state.creating_line.is_none()
                && self.state.creating_circle.is_none()
                && self.state.creating_plane.is_none()
                && ctx.input(|i| i.key_pressed(egui::Key::C))
            {
                if self.state.tool == Tool::Constraint && !self.state.scene_selection.is_empty() {
                    let rows = crate::geometric_constraints::constraint_pane_rows(
                        &self.state.scene_selection,
                    );
                    if let Some(kind) =
                        crate::geometric_constraints::sole_enabled_constraint_type(&rows)
                    {
                        self.state.apply(Action::AddGeometricConstraint(kind));
                    }
                } else if self.state.tool != Tool::Constraint {
                    self.state.apply(Action::SetTool(Tool::Constraint));
                }
            }

            if self.state.creating_rect.is_none()
                && self.state.creating_line.is_none()
                && self.state.creating_circle.is_none()
                && ctx.input(|i| i.key_pressed(egui::Key::O))
            {
                if self.state.tool != Tool::Circle {
                    self.state.apply(Action::SetTool(Tool::Circle));
                }
            }

            if self.state.creating_rect.is_none()
                && self.state.creating_line.is_none()
                && self.state.creating_circle.is_none()
                && self.state.creating_plane.is_none()
                && ctx.input(|i| i.key_pressed(egui::Key::P))
            {
                if self.state.tool != Tool::ConstructionPlane {
                    self.state.apply(Action::SetTool(Tool::ConstructionPlane));
                }
            }

            if self.state.creating_rect.is_none()
                && self.state.creating_line.is_none()
                && self.state.creating_circle.is_none()
                && self.state.creating_plane.is_none()
                && ctx.input(|i| i.key_pressed(egui::Key::D))
            {
                self.state.apply(Action::SetTool(Tool::Dimension));
            }

            if self.state.creating_rect.is_none()
                && self.state.creating_line.is_none()
                && self.state.creating_circle.is_none()
                && self.state.creating_plane.is_none()
                && ctx.input(|i| i.key_pressed(egui::Key::E))
            {
                if self.state.tool != Tool::Extrude {
                    self.state.apply(Action::SetTool(Tool::Extrude));
                }
            }

            if ctx.input(|i| i.key_pressed(egui::Key::X)) {
                self.state.apply(Action::ToggleConstruction);
            }

            if ctx.input(|i| i.key_pressed(egui::Key::N)) {
                self.state.apply(Action::FocusElementName);
            }

            if self.state.creating_rect.is_none()
                && self.state.creating_line.is_none()
                && self.state.creating_circle.is_none()
                && self.state.creating_plane.is_none()
                && !self.state.scene_selection.is_empty()
                && (ctx.input(|i| i.key_pressed(egui::Key::Delete))
                    || ctx.input(|i| i.key_pressed(egui::Key::Backspace)))
            {
                self.state.apply(Action::DeleteSelection);
            }

            if self.state.tool == Tool::Constraint {
                // Mnemonic letter shortcuts for the constraint pane (see
                // GeometricConstraintType::shortcut_label). `C` is reserved for the tool itself.
                for (key, egui_key) in [
                    ('A', egui::Key::A),
                    ('T', egui::Key::T),
                    ('I', egui::Key::I),
                    ('M', egui::Key::M),
                    ('V', egui::Key::V),
                    ('H', egui::Key::H),
                ] {
                    if ctx.input(|i| i.key_pressed(egui_key)) {
                        self.state.apply(Action::ApplyConstraintShortcut(key));
                    }
                }
            }
        }

        if self.state.tool != Tool::Rectangle || self.state.sketch_session.is_none() {
            self.state.creating_rect = None;
        }
        if self.state.tool != Tool::Line || self.state.sketch_session.is_none() {
            self.state.creating_line = None;
        }
        if self.state.tool != Tool::Circle || self.state.sketch_session.is_none() {
            self.state.creating_circle = None;
        }
        if self.state.tool != Tool::ConstructionPlane {
            self.state.creating_plane = None;
        }
        if !matches!(
            self.state.tool,
            Tool::Select | Tool::Dimension | Tool::Constraint
        ) {
            self.state.editing_committed_dim = None;
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
        if screenshots.is_empty() {
            return;
        }

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

    /// Extrude tool interaction: click faces to toggle inclusion, and drag the normal gizmo
    /// (rendered in the GPU scene) to set the distance, snapping to objects under the cursor.
    fn handle_extrude_tool(
        &mut self,
        ui: &egui::Ui,
        project: &impl Fn(Vec3) -> Option<egui::Pos2>,
        pointer_screen: Option<egui::Pos2>,
    ) {
        let primary_pressed = ui.input(|i| i.pointer.primary_pressed());

        // If the in-progress extrusion went away (committed or cancelled), stop following.
        if self.state.creating_extrusion.is_none() {
            self.extrude_gizmo_drag = None;
            self.pending_extrude_target = None;
        }

        // Snapshot the pending extrusion so we can mutate state without holding a borrow.
        let pending = self
            .state
            .creating_extrusion
            .as_ref()
            .filter(|ce| !ce.faces.is_empty())
            .map(|ce| (ce.faces.clone(), ce.evaluated_distance(&self.state.doc)));

        // The handle is a click-to-grab control: one click grabs it, then it follows
        // the cursor (no held button) until the next click, which finishes the extrude.
        let following = self.extrude_gizmo_drag.is_some();
        let mut gizmo_active = false;
        if let Some((faces, distance)) = &pending {
            if let Some((origin, normal)) = extrude::faces_anchor(&self.state.doc, faces) {
                let handle_offset = extrude_gizmo_display_offset(*distance);
                let hovered = pointer_screen.is_some_and(|pp| {
                    construction::offset_gizmo_hit(pp, project, origin, normal, handle_offset)
                });
                if !following && primary_pressed && hovered {
                    if let Some(pp) = pointer_screen {
                        self.extrude_gizmo_drag = Some(ExtrudeGizmoDrag {
                            start_screen: pp,
                            start_distance: *distance,
                        });
                        // Grabbing the gizmo hands distance control back to it,
                        // so the typed text resyncs to the dragged value.
                        if let Some(ce) = self.state.creating_extrusion.as_mut() {
                            ce.user_edited = false;
                        }
                        // Release the distance field's keyboard focus so a subsequent
                        // keystroke overwrites the dragged value rather than appending to it.
                        ui.ctx().memory_mut(|m| {
                            m.surrender_focus(egui::Id::new(EXTRUDE_DISTANCE_FIELD_ID))
                        });
                    }
                }
                // While following, track the cursor every frame (no button required).
                if let Some(drag) = self.extrude_gizmo_drag {
                    gizmo_active = true;
                    if let Some(pp) = pointer_screen {
                        if let Some((target, dist)) = pick_extrude_target(
                            pp,
                            project,
                            &self.state.doc,
                            origin,
                            normal,
                            faces,
                            self.state.cam.eye(),
                        ) {
                            self.pending_extrude_target = Some(target);
                            self.state.apply(Action::SetExtrudeDistance { distance: dist });
                        } else {
                            self.pending_extrude_target = None;
                            let new_distance = construction::offset_from_normal_drag(
                                origin,
                                normal,
                                project,
                                drag.start_distance,
                                drag.start_screen,
                                pp,
                            );
                            self.state
                                .apply(Action::SetExtrudeDistance { distance: new_distance });
                        }
                    }
                }
            }
        }

        // A click while following commits the extrusion, snapping to any pending target.
        if following && primary_pressed {
            let target = self.pending_extrude_target.take();
            self.state.apply(Action::SetExtrudeTarget { target });
            self.state.apply(Action::CommitExtrusion);
            self.extrude_gizmo_drag = None;
            return;
        }
        if gizmo_active {
            return;
        }

        // Click toggles the face under the cursor (highlighted via the GPU hover).
        if primary_pressed {
            if let Some(pp) = pointer_screen {
                if let Some(face) = pick_extrude_face(pp, project, &self.state.doc, self.state.cam.eye()) {
                    self.state.apply(Action::ToggleExtrudeFace { face });
                }
            }
        }
    }

    /// Floating distance field for the in-progress extrusion (Enter commits).
    fn show_extrude_distance_input(
        &mut self,
        ui: &egui::Ui,
        project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    ) {
        let pos = {
            let Some(ce) = self.state.creating_extrusion.as_ref() else {
                return;
            };
            if ce.faces.is_empty() {
                return;
            }
            let handle_offset = extrude_gizmo_display_offset(ce.evaluated_distance(&self.state.doc));
            extrude::faces_anchor(&self.state.doc, &ce.faces)
                .map(|(o, n)| construction::offset_handle(o, n, handle_offset))
                .and_then(project)
                .map(|p| p + egui::vec2(14.0, -12.0))
        };
        let Some(pos) = pos else {
            return;
        };
        let ctx = ui.ctx();
        let id = egui::Id::new(EXTRUDE_DISTANCE_FIELD_ID);
        let mut commit = false;

        // Enter commits the extrusion even when the distance field is unfocused (e.g.
        // while driving depth with the pull handle), matching the other sketch tools.
        if !ctx.memory(|m| m.has_focus(id)) && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.state.apply(Action::CommitExtrusion);
            return;
        }

        // Typing a number while the field is unfocused grabs focus and overwrites
        // the current value, so the user can just start typing a depth.
        if !ctx.memory(|m| m.has_focus(id)) {
            let typed: String = ctx.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect()
            });
            let typed: String = typed
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                .collect();
            if !typed.is_empty() {
                if let Some(ce) = self.state.creating_extrusion.as_mut() {
                    ce.text = typed;
                    ce.user_edited = true;
                    ce.pending_focus = true;
                }
            }
        }
        if let Some(ce) = self.state.creating_extrusion.as_mut() {
            let want_focus = ce.pending_focus;
            egui::Area::new(egui::Id::new("extrude_distance_area"))
                .fixed_pos(pos)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut ce.text)
                                .id(id)
                                .desired_width(64.0),
                        );
                        if resp.changed() {
                            ce.user_edited = true;
                        }
                        if want_focus {
                            resp.request_focus();
                            ce.pending_focus = false;
                        }
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            commit = true;
                        }
                        ui.label("mm");
                    });
                });
        }
        if commit {
            self.state.apply(Action::CommitExtrusion);
        }
    }

    fn tick_exit_after_startup(&mut self, ctx: &egui::Context) {
        if !self.exit_after_startup || self.exit_after_startup_sent {
            return;
        }
        if self.launch_maximize_frames_remaining > 0 {
            return;
        }
        self.exit_after_startup_sent = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn tick_script(&mut self, ctx: &egui::Context) {
        if self.script.as_ref().is_some_and(|r| !r.done) {
            self.state.command_log = None;
        } else if self.show_commands && self.state.command_log.is_none() {
            self.state.command_log =
                Some(std::cell::RefCell::new(command_log::CommandLog::new()));
        }
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
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        tick_launch_maximize(&mut self.launch_maximize_frames_remaining, ctx);
        theme::apply(ctx);

        let dt = ctx.input(|i| i.stable_dt);
        let transition_active = self.state.cam.tick_transition(dt);
        if transition_active {
            ctx.request_repaint();
        } else if let Some(log) = &self.state.command_log {
            log.borrow_mut()
                .on_transition_complete(&self.state.cam);
        }

        self.process_screenshots(ctx);
        self.tick_script(ctx);
        self.tick_exit_after_startup(ctx);
        self.synthetic.inject(ctx);

        self.handle_keyboard_shortcuts(ctx);

        self.handle_native_menu(ctx);

        egui::TopBottomPanel::top("toolbar")
            .frame(theme::panel_frame())
            .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if icons::selectable_icon_button(
                    ui,
                    icons::IconId::Select,
                    self.state.tool == Tool::Select,
                    shortcuts::compact_label("Select", shortcuts::tool_shortcut(Tool::Select)),
                )
                .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::Select));
                }
                if icons::selectable_icon_button(
                    ui,
                    icons::IconId::Sketch,
                    self.state.tool == Tool::Sketch,
                    shortcuts::compact_label("Sketch", shortcuts::tool_shortcut(Tool::Sketch)),
                )
                .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::Sketch));
                }
                if icons::selectable_icon_button(
                    ui,
                    icons::IconId::Rectangle,
                    self.state.tool == Tool::Rectangle,
                    shortcuts::compact_label(
                        "Rectangle",
                        shortcuts::tool_shortcut(Tool::Rectangle),
                    ),
                )
                .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::Rectangle));
                }
                if icons::selectable_icon_button(
                    ui,
                    icons::IconId::Line,
                    self.state.tool == Tool::Line,
                    shortcuts::compact_label("Line", shortcuts::tool_shortcut(Tool::Line)),
                )
                .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::Line));
                }
                if icons::selectable_icon_button(
                    ui,
                    icons::IconId::Circle,
                    self.state.tool == Tool::Circle,
                    shortcuts::compact_label("Circle", shortcuts::tool_shortcut(Tool::Circle)),
                )
                .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::Circle));
                }
                if icons::selectable_icon_button(
                    ui,
                    icons::IconId::Dimension,
                    self.state.tool == Tool::Dimension,
                    shortcuts::compact_label(
                        "Dimension",
                        shortcuts::tool_shortcut(Tool::Dimension),
                    ),
                )
                .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::Dimension));
                }
                if icons::selectable_icon_button(
                    ui,
                    icons::IconId::Constraint,
                    self.state.tool == Tool::Constraint,
                    shortcuts::compact_label(
                        "Constraint",
                        shortcuts::tool_shortcut(Tool::Constraint),
                    ),
                )
                .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::Constraint));
                }
                if icons::selectable_icon_button(
                    ui,
                    icons::IconId::Plane,
                    self.state.tool == Tool::ConstructionPlane,
                    shortcuts::compact_label(
                        "Plane",
                        shortcuts::tool_shortcut(Tool::ConstructionPlane),
                    ),
                )
                .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::ConstructionPlane));
                }
                if icons::selectable_icon_button(
                    ui,
                    icons::IconId::Extrude,
                    self.state.tool == Tool::Extrude,
                    shortcuts::compact_label("Extrude", shortcuts::tool_shortcut(Tool::Extrude)),
                )
                .clicked()
                {
                    self.state.apply(Action::SetTool(Tool::Extrude));
                }
                if let Some(session) = self.state.sketch_session {
                    ui.separator();
                    ui.label(sketch_label(&self.state.doc, session.sketch));
                }
                ui.separator();
                if ui.button("Clear").clicked() {
                    self.state.apply(Action::Clear);
                }
            });
        });

        if self.state.command_palette.open {
            let commands = commands_for_state(&self.state);
            let matches = filter_commands(&self.state.command_palette.query, &commands);
            let mut outcome = None;
            egui::TopBottomPanel::bottom("command_palette")
                .resizable(false)
                .exact_height(280.0)
                .frame(
                    egui::Frame::default()
                        .fill(theme::palette_console_fill())
                        .inner_margin(egui::Margin::symmetric(12.0, 8.0)),
                )
                .show(ctx, |ui| {
                    outcome = show_palette(ui, &mut self.state.command_palette, &matches);
                });
            if let Some(chosen) = outcome {
                self.dispatch_palette_outcome(chosen);
            }
        }

        egui::TopBottomPanel::bottom("status")
            .frame(theme::panel_frame())
            .show(ctx, |ui| {
            let name = self.state.path.as_deref().unwrap_or("(unsaved)");
            ui.horizontal(|ui| {
                ui.label(name);
                ui.separator();
                ui.label(&self.state.status);
            });
        });

        if self.state.panes.is_visible(Pane::Hierarchy) {
            let mut edit_sketch: Option<SketchId> = None;
            let mut edit_plane: Option<usize> = None;
            let mut edit_extrusion: Option<usize> = None;
            let mut export_body: Option<usize> = None;
            let mut click_element: Option<(SceneElement, bool)> = None;
            egui::SidePanel::left("tree")
                .resizable(true)
                .default_width(220.0)
                .frame(theme::panel_frame())
                .show(ctx, |ui| {
                    let mut queue_edit_sketch = |sketch: SketchId| {
                        edit_sketch = Some(sketch);
                    };
                    let mut queue_edit_plane = |index: usize| {
                        edit_plane = Some(index);
                    };
                    let mut queue_edit_extrusion = |index: usize| {
                        edit_extrusion = Some(index);
                    };
                    let mut queue_export_body = |index: usize| {
                        export_body = Some(index);
                    };
                    let mut noop_visibility = |_: SceneElement, _: bool| {};
                    let mut queue_click = |element: SceneElement, additive: bool| {
                        click_element = Some((element, additive));
                    };
                    // Highlight the elements that use the variable focused in the Parameters pane.
                    let highlight_elements = parameters::focused_parameter_name(ctx, &self.state.doc)
                        .map(|name| parameters::elements_using_parameter(&self.state.doc, &name))
                        .unwrap_or_default();
                    hierarchy::show_pane(
                        ui,
                        &self.state.doc,
                        self.state.sketch_session,
                        &mut self.state.element_visibility,
                        &self.state.scene_selection,
                        &self.state.document_health,
                        &mut queue_edit_sketch,
                        &mut queue_edit_plane,
                        &mut queue_edit_extrusion,
                        &mut queue_export_body,
                        &mut noop_visibility,
                        &mut queue_click,
                        &highlight_elements,
                    );
                });
            if let Some((element, additive)) = click_element {
                self.state.apply(Action::ClickSceneElement { element, additive });
            }
            if let Some(sketch) = edit_sketch {
                self.state.apply(Action::OpenSketch {
                    sketch,
                    viewport: self.last_viewport,
                });
            }
            if let Some(index) = edit_plane {
                self.state.apply(Action::BeginEditConstructionPlane { index });
            }
            if let Some(index) = edit_extrusion {
                self.state.apply(Action::EditExtrusion { index });
            }
            if let Some(index) = export_body {
                self.export_stl_body(index);
            }
        }

        if self.state.panes.is_visible(Pane::Parameters) {
            egui::SidePanel::right("parameters")
                .resizable(true)
                .default_width(240.0)
                .frame(theme::panel_frame())
                .show(ctx, |ui| {
                    parameters::show_pane(ui, &mut self.state);
                });
        }

        if self.state.panes.is_visible(Pane::Context) {
            let context_input = context::ContextInput {
                doc: &self.state.doc,
                selection: &self.state.scene_selection,
                tool: self.state.tool,
                draw_rect_construction: self.state.rect_draw_construction_mode(),
                draw_line_construction: self.state.line_draw_construction_mode(),
                draw_circle_construction: self.state.circle_draw_construction_mode(),
                in_sketch: self.state.sketch_session.is_some(),
                snapping_enabled: self.state.snapping_enabled,
            };
            let content = context::context_pane_content(&context_input);
            context::sync_name_draft(&mut self.state.context_pane, &self.state.doc, &content);
            let mut construction_change: Option<bool> = None;
            let mut name_commit: Option<(SceneElement, String)> = None;
            let mut constraint_apply: Option<crate::geometric_constraints::GeometricConstraintType> =
                None;
            let mut snapping_change: Option<bool> = None;
            egui::SidePanel::right("context")
                .resizable(true)
                .default_width(200.0)
                .max_width(280.0)
                .frame(theme::panel_frame())
                .show(ctx, |ui| {
                    context::show_pane(
                        ui,
                        ctx,
                        &content,
                        &mut self.state.context_pane,
                        &self.state.document_health,
                        &self.state.scene_selection,
                        &mut |element, name| name_commit = Some((element, name)),
                        &mut |construction| {
                            construction_change = Some(construction);
                        },
                        &mut |kind| constraint_apply = Some(kind),
                        &mut |enabled| snapping_change = Some(enabled),
                    );
                });
            if let Some(enabled) = snapping_change {
                self.state.apply(Action::SetSnapping(enabled));
            }
            if let Some(kind) = constraint_apply {
                self.state.apply(Action::AddGeometricConstraint(kind));
            }
            if let Some((element, name)) = name_commit {
                self.state
                    .apply(Action::CommitElementName { element, name });
            }
            if let Some(construction) = construction_change {
                self.state
                    .apply(Action::ApplyConstruction { construction });
            }
        }

        let render_state = frame.wgpu_render_state();
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                self.draw_viewport(ui, render_state);
            });
    }
}

/// Suppress unmodified keyboard shortcuts while a [`egui::TextEdit`] (or other focused text input)
/// is active.
fn keyboard_shortcuts_suppressed(ctx: &egui::Context) -> bool {
    ctx.wants_keyboard_input()
}

fn next_rect_focus_axis(focused: usize) -> RectAxis {
    if focused == 0 {
        RectAxis::Height
    } else {
        RectAxis::Width
    }
}

fn next_plane_focus_dim(focused: PlaneDim) -> PlaneDim {
    if focused == PlaneDim::Offset {
        PlaneDim::Angle
    } else {
        PlaneDim::Offset
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
    /// Pivot shown while right-dragging to orbit the camera.
    pub const ORBIT_PIVOT: Color32 = Color32::from_rgb(255, 105, 180);
    /// Drop line from the orbit pivot to the ground plane.
    pub const ORBIT_PIVOT_DROP: Color32 = Color32::from_rgba_premultiplied(255, 105, 180, 70);
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
    /// Committed sketch dimension lines and labels in edit mode.
    pub const DIM_ANNOTATION: Color32 = Color32::from_rgb(180, 188, 204);
    /// All construction geometry (planes, etc.) shares this colour.
    pub const CONSTRUCTION: Color32 = crate::construction::CONSTRUCTION_RGBA;
    /// Faded appearance for geometry outside the active sketch face.
    pub const SKETCH_DIMMED: f32 = crate::gpu_viewport::SKETCH_DIMMED;
}

const GRID_EXTENT: f32 = gpu_viewport::GRID_EXTENT;
const GRID_STEP: f32 = gpu_viewport::GRID_STEP;

/// Screen-space height of a floating dimension input (frame + text field).
const DIM_INPUT_HEIGHT: f32 = 26.0;
/// Horizontal padding inside the dimension input frame (inner margin × 2).
const DIM_INPUT_FRAME_H_PAD: f32 = 10.0;
/// Minimum text-edit width (fits short live values like `80.0`).
const DIM_INPUT_MIN_TEXT_WIDTH: f32 = 48.0;
/// Approximate monospace glyph width at 13pt (used for layout sizing).
const DIM_INPUT_CHAR_WIDTH: f32 = 7.8;

fn build_gpu_dimension_labels(
    ctx: &egui::Context,
    layouts: &[CommittedDimLayout],
    view: &PlanarLabelView,
    cam: &camera::Camera,
    viewport: egui::Rect,
    view_proj: &glam::Mat4,
    project: &impl Fn(glam::Vec3) -> Option<egui::Pos2>,
    skip_constraint: Option<DimLabelTarget>,
    health: &document_health::DocumentHealth,
) -> Vec<gpu_viewport::ViewportDimLabel> {
    layouts
        .iter()
        .filter(|layout| layout.arc_geom.is_none())
        .map(|layout| {
            let color = document_health::constraint_annotation_color(
                health,
                layout.target,
                col::DIM_ANNOTATION,
            );
            let (text_vertices, text_indices) = if skip_constraint == Some(layout.target) {
                (Vec::new(), Vec::new())
            } else {
                gpu_viewport::build_planar_label_mesh(
                    ctx,
                    &layout.world_geom,
                    view,
                    &layout.label,
                    color,
                    cam,
                    viewport,
                    view_proj,
                    project,
                )
            };
            gpu_viewport::ViewportDimLabel {
                world_geom: layout.world_geom,
                color,
                text_vertices,
                text_indices,
                draw_dimension_lines: layout.arc_geom.is_none(),
            }
        })
        .collect()
}

const SIDE_PANEL_IDS: &[&str] = &["tree", "parameters", "context"];

/// True while the pointer is on a side-panel resize grip (don't override its cursor).
fn side_panel_resize_active(ctx: &egui::Context) -> bool {
    SIDE_PANEL_IDS.iter().any(|id| {
        ctx.read_response(egui::Id::new(*id).with("__resize"))
            .is_some_and(|r| r.dragged() || r.hovered())
    })
}

/// Set a viewport cursor only when the viewport owns the pointer this frame.
fn set_viewport_cursor(
    ctx: &egui::Context,
    response: &egui::Response,
    viewport_owns_pointer: bool,
    icon: egui::CursorIcon,
) {
    if side_panel_resize_active(ctx) {
        return;
    }
    if viewport_owns_pointer || response.hovered() {
        ctx.set_cursor_icon(icon);
    }
}

/// Pointer in viewport coordinates for hit-testing and drags.
fn viewport_pointer_pos(
    response: &egui::Response,
    viewport_owns_pointer: bool,
) -> Option<egui::Pos2> {
    response
        .hover_pos()
        .or(viewport_owns_pointer.then_some(response.interact_pointer_pos()).flatten())
}

/// True while orbiting/panning or dragging sketch geometry — pick hover is distracting then.
fn suppress_viewport_pick_hover(
    ui: &egui::Ui,
    response: &egui::Response,
    vertex_drag_active: bool,
    line_drag_active: bool,
    dim_label_drag_active: bool,
    angle_gizmo_drag_active: bool,
    plane_gizmo_drag_active: bool,
) -> bool {
    ui.input(|i| i.pointer.secondary_down())
        || response.dragged_by(egui::PointerButton::Secondary)
        || vertex_drag_active
        || line_drag_active
        || dim_label_drag_active
        || angle_gizmo_drag_active
        || plane_gizmo_drag_active
}

fn resolve_viewport_hover_highlight(
    suppress_hover: bool,
    tool: Tool,
    sketch_session: Option<SketchSession>,
    creating_plane: bool,
    editing_committed_dim: bool,
    over_committed_dim_label: bool,
    dim_label_drag: bool,
    pointer_screen: Option<egui::Pos2>,
    cam: &camera::Camera,
    viewport: egui::Rect,
    vp: &glam::Mat4,
    doc: &model::Document,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
) -> Option<gpu_viewport::ViewportHoverHighlight> {
    if suppress_hover {
        return None;
    }
    let pp = pointer_screen?;
    match tool {
        Tool::Sketch => pick_sketch_face(pp, project, doc, cam.eye())
            .map(gpu_viewport::ViewportHoverHighlight::SketchFace),
        Tool::Rectangle | Tool::Line | Tool::Circle if sketch_session.is_none() => {
            pick_sketch_face(pp, project, doc, cam.eye())
                .map(gpu_viewport::ViewportHoverHighlight::SketchFace)
        }
        Tool::ConstructionPlane if !creating_plane => {
            let gp = cam.ground_point(pp, viewport, vp);
            resolve_pick_target(pp, project, gp, doc)
                .map(|t| gpu_viewport::ViewportHoverHighlight::PickTarget(t.kind))
        }
        Tool::Select | Tool::Constraint
            if !editing_committed_dim && !over_committed_dim_label && !dim_label_drag =>
        {
            let gp = cam.ground_point(pp, viewport, vp);
            resolve_pick_target(pp, project, gp, doc).and_then(|t| {
                scene_element_from_pick(&t.kind)
                    .map(|_| gpu_viewport::ViewportHoverHighlight::PickTarget(t.kind))
            })
        }
        _ => None,
    }
}

fn plane_gizmo_hover(
    cp: &CreatingConstructionPlane,
    pointer_screen: Option<egui::Pos2>,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
) -> Option<AxisGizmoHit> {
    let pp = pointer_screen?;
    match &cp.reference {
        PlaneReference::Face { origin, normal, .. } => {
            if offset_gizmo_hit(pp, project, *origin, *normal, cp.offset_live) {
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
            project,
            *origin,
            *direction,
            cp.offset_live,
            cp.axis_angle_deg,
        ),
    }
}

fn build_viewport_scene_input<'a>(
    doc: &'a model::Document,
    cam: &'a camera::Camera,
    viewport: egui::Rect,
    sketch_session: Option<SketchSession>,
    element_visibility: &'a hierarchy::ElementVisibility,
    selection: &'a crate::selection::SceneSelection,
    document_health: &'a document_health::DocumentHealth,
    creating_rect: Option<&CreatingRect>,
    creating_line: Option<&CreatingLine>,
    creating_circle: Option<&CreatingCircle>,
    creating_plane: Option<&CreatingConstructionPlane>,
    creating_extrusion: Option<&CreatingExtrusion>,
    plane_gizmo: Option<gpu_viewport::ViewportPlaneGizmo>,
    extrude_gizmo: Option<gpu_viewport::ViewportExtrudeGizmo>,
    hover_highlight: Option<gpu_viewport::ViewportHoverHighlight>,
    dimension_labels: &'a [gpu_viewport::ViewportDimLabel],
    dim_label_view: Option<PlanarLabelView>,
    constraint_graphics: Option<&'a [constraint_viewport::ConstraintViewportGraphic]>,
) -> gpu_viewport::ViewportSceneInput<'a> {
    let preview_rect = creating_rect.and_then(|cr| {
        let session = sketch_session?;
        let frame = sketch_geometry_frame(doc, session.sketch)?;
        let end = cr.end_point(&frame, doc);
        let (ou, ov) = world_to_local(&frame, cr.origin);
        let (eu, ev) = world_to_local(&frame, end);
        let mut preview = Rect::from_local_corners(session.sketch, ou, ov, eu, ev);
        if cr.construction {
            for edge_index in 0..4 {
                preview.set_edge_construction(RectEdge::from_index(edge_index), true);
            }
        }
        Some(preview)
    });
    let preview_line = creating_line.and_then(|cl| {
        let session = sketch_session?;
        let frame = sketch_geometry_frame(doc, session.sketch)?;
        let end = cl.end_point(&frame, doc);
        let (u0, v0) = world_to_local(&frame, cl.origin);
        let (u1, v1) = world_to_local(&frame, end);
        let mut preview = Line::from_local_endpoints(session.sketch, u0, v0, u1, v1);
        preview.construction = cl.construction;
        Some(preview)
    });
    let preview_circle = creating_circle.and_then(|cc| {
        let session = sketch_session?;
        let frame = sketch_geometry_frame(doc, session.sketch)?;
        let (cu, cv) = world_to_local(&frame, cc.origin);
        let r = cc.radius(&frame, doc);
        let angle = cc.diameter_dim_angle(&frame);
        let mut preview = Circle::from_local_center_radius(
            session.sketch, cu, cv, r, angle,
        );
        preview.construction = cc.construction;
        Some(preview)
    });
    let vp = cam.view_proj(viewport);
    let plane_preview = creating_plane.map(|cp| {
        let plane = cp.preview_plane();
        let dependents = cp
            .edit_index
            .and_then(|index| preview_plane_edit_dependents(doc, index, &plane));
        let dim_outline = plane_dim_layouts(
            &|w: Vec3| cam.project(w, viewport, &vp),
            &plane,
            &cp.reference,
            cp.offset_live,
            cp.axis_angle_deg,
        )
        .is_some();
        gpu_viewport::ViewportPlanePreview {
            plane,
            dependents,
            dim_outline,
        }
    });
    let active_sketch_face = sketch_session.and_then(|session| doc.sketch_face(session.sketch));
    let active_sketch_face = active_sketch_face.filter(|face| !matches!(face, FaceId::ConstructionPlane(_)));

    let editing_extrusion = creating_extrusion.and_then(|ce| ce.edit_index);

    let preview_extrusion = creating_extrusion.and_then(|ce| {
        (!ce.faces.is_empty()).then(|| model::Extrusion {
            sketch: ce.sketch,
            faces: ce.faces.clone(),
            distance: ce.evaluated_distance(doc),
            target: ce.target,
            expression: String::new(),
            name: None,
            deleted: false,
        })
    });

    gpu_viewport::ViewportSceneInput {
        doc,
        cam,
        viewport,
        palette: gpu_viewport::ViewportPalette {
            background: col::BG,
            grid: col::GRID,
            grid_axis: col::GRID_AXIS,
            x_axis: col::X_AXIS,
            y_axis: col::Y_AXIS,
            z_axis: col::Z_AXIS,
            rect_line: col::RECT_LINE,
            line_stroke: col::LINE_STROKE,
            preview: col::PREVIEW,
            construction: col::CONSTRUCTION,
            dim_edge_highlight: col::DIM_EDGE_HIGHLIGHT,
            construction_plane_fill: construction::PLANE_FILL_RGBA,
            construction_plane_opacity: gpu_viewport::DEFAULT_CONSTRUCTION_PLANE_OPACITY,
        },
        sketch_session,
        selection,
        element_visibility,
        preview_rect,
        preview_line,
        preview_circle,
        preview_extrusion,
        editing_extrusion,
        plane_preview,
        active_sketch_face,
        dimension_labels,
        dim_label_view,
        plane_gizmo,
        extrude_gizmo,
        hover_highlight,
        hover_color: construction::PICK_HOVER_RGBA,
        document_health,
        constraint_graphics,
        constraint_connector_color: Some(col::DIM_EDGE_HIGHLIGHT),
    }
}
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SketchDimFieldResult {
    changed: bool,
    enter_commit: bool,
    lost_focus: bool,
    inline_parameter_added: Option<String>,
    inline_parameter_error: Option<String>,
}

fn sketch_dimension_enter_pressed(ui: &egui::Ui) -> bool {
    ui.input(|i| i.key_pressed(egui::Key::Enter))
}

fn consume_sketch_dimension_enter(ui: &mut egui::Ui) {
    ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
}

/// Commit when Enter was pressed on a focused dim field, or when Enter is pressed with no dim focused.
fn should_commit_sketch_on_enter(
    field_enter_commit: bool,
    dim_field_focused: bool,
    enter_pressed: bool,
) -> bool {
    field_enter_commit || (enter_pressed && !dim_field_focused)
}

fn angle_expression_field_errors(text: &str, doc: &model::Document) -> Vec<String> {
    let t = text.trim();
    if t.is_empty() {
        return vec!["Expression cannot be empty".to_string()];
    }
    if crate::value::eval_angle_rad_in_doc(t, doc).is_none() {
        return vec![format!("Invalid angle expression '{t}'")];
    }
    Vec::new()
}

/// Show a sketch dimension field; selects all text when it gains focus so typing replaces the value.
fn show_sketch_dimension_field(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    id: egui::Id,
    text: &mut String,
    doc: &mut model::Document,
    is_focus_target: bool,
    pending_focus: &mut bool,
    user_edited: bool,
    angle: bool,
) -> SketchDimFieldResult {
    let has_focus = ctx.memory(|m| m.focused()) == Some(id);
    if has_focus {
        expression_autocomplete_handle_keys(ui, ctx, id, text, doc, &[]);
    }
    let field_errors = if angle {
        angle_expression_field_errors(text, doc)
    } else {
        length_expression_field_errors(text, doc, None)
    };
    let has_errors = !field_errors.is_empty();
    let show_computed_row = if angle {
        crate::value::shows_computed_angle_in_doc(text, doc)
    } else {
        shows_computed_length_in_doc(text, doc)
    };
    let widget = if has_focus {
        &ui.style().visuals.widgets.active
    } else {
        &ui.style().visuals.widgets.inactive
    };
    let frame = egui::Frame::default()
        .fill(if has_errors {
            INVALID_BG
        } else if has_focus {
            col::DIM_INPUT_BG_FOCUS
        } else {
            col::DIM_INPUT_BG
        })
        .stroke(egui::Stroke::new(
            widget.bg_stroke.width,
            if has_errors {
                INVALID_BORDER
            } else if has_focus {
                col::DIM_INPUT_BORDER_FOCUS
            } else {
                col::DIM_INPUT_BORDER
            },
        ))
        .inner_margin(egui::Margin::symmetric(5.0, 3.0))
        .rounding(3.0);

    let computed = if has_errors {
        None
    } else if angle {
        crate::value::computed_angle_in_doc(text, doc)
            .filter(|_| show_computed_row)
            .map(crate::value::format_angle_display)
    } else {
        computed_length_in_doc(text, doc)
            .filter(|_| show_computed_row)
            .map(format_length_display)
    };
    let text_width = dim_input_text_width(text);

    let frame_output = frame.show(ui, |ui| {
        ui.set_width(text_width);
        ui.vertical_centered(|ui| {
            if let Some(v) = computed {
                ui.label(
                    egui::RichText::new(v)
                        .font(egui::FontId::monospace(11.0))
                        .color(col::DIM_INPUT_TEXT.gamma_multiply(0.65)),
                );
            } else if show_computed_row {
                ui.add_space(14.0);
            }
            ui.style_mut().spacing.text_edit_width = text_width;
            ui.visuals_mut().selection.bg_fill = col::DIM_INPUT_SELECTION;
            egui::TextEdit::singleline(text)
                .id(id)
                .frame(false)
                .desired_width(text_width)
                .font(egui::FontId::monospace(13.0))
                .text_color(if has_errors {
                    INVALID_TEXT
                } else if has_focus {
                    col::DIM_INPUT_TEXT_FOCUS
                } else {
                    col::DIM_INPUT_TEXT
                })
                .margin(egui::vec2(0.0, 0.0))
                .show(ui)
        })
        .inner
    });
    let output = frame_output.inner;
    if output.response.has_focus() {
        let cursor = output
            .state
            .cursor
            .char_range()
            .map(|range| range.primary.index)
            .unwrap_or_else(|| text.chars().count());
        if expression_autocomplete_show_dropdown(
            ui,
            ctx,
            &output.response,
            id,
            text,
            doc,
            &[],
            cursor,
        ) {
            output.state.clone().store(ctx, id);
        }
    }
    show_expression_error_tooltips_above(ui, &frame_output.response, &field_errors);
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
    let enter_commit = sketch_dimension_enter_pressed(ui) && resp.has_focus();
    if enter_commit {
        consume_sketch_dimension_enter(ui);
    }
    let lost_focus = resp.lost_focus();
    let mut inline_parameter_added = None;
    let mut inline_parameter_error = None;
    if enter_commit || lost_focus {
        match crate::parameters::try_commit_inline_parameter_definition(doc, text) {
            Ok(Some(name)) => inline_parameter_added = Some(name),
            Ok(None) => {}
            Err(error) => inline_parameter_error = Some(error),
        }
    }
    SketchDimFieldResult {
        changed: resp.changed(),
        enter_commit,
        lost_focus,
        inline_parameter_added,
        inline_parameter_error,
    }
}

fn apply_dimension_field_feedback(state: &mut AppState, result: &SketchDimFieldResult) {
    if let Some(name) = &result.inline_parameter_added {
        state.refresh_document_health();
        state.status = format!("Added parameter {name}");
    } else if let Some(error) = &result.inline_parameter_error {
        state.status = error.clone();
    }
}

fn sketch_plane_point(
    cam: &camera::Camera,
    viewport: egui::Rect,
    vp: &glam::Mat4,
    doc: &model::Document,
    session: SketchSession,
    screen: egui::Pos2,
) -> Option<Vec3> {
    let face = doc.sketch_face(session.sketch)?;
    let frame = sketch_frame(doc, face)?;
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

fn push_circle_diameter_dim_layout(
    layouts: &mut Vec<CommittedDimLayout>,
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    label_view: &PlanarLabelView,
    frame: &face::SketchFrame,
    circle: &Circle,
    target: DimLabelTarget,
    a: Vec3,
    b: Vec3,
    stored_label_offset: Option<f32>,
    label: String,
) {
    let color = col::DIM_ANNOTATION;
    let (ua, va) = world_to_local(frame, a);
    let (ub, vb) = world_to_local(frame, b);
    let outward_uv = outward_perpendicular_uv(ua, va, ub, vb, circle.cx, circle.cy);
    let outward_world = uv_dir_to_world(frame.u_axis, frame.v_axis, outward_uv.0, outward_uv.1);
    if outward_world.length_squared() < 1e-8 {
        return;
    }
    let galley = painter.layout_no_wrap(
        label.clone(),
        egui::FontId::proportional(LABEL_FONT_SIZE),
        color,
    );
    let galley_size = galley.size();
    let diameter_px = project(a)
        .zip(project(b))
        .map(|(pa, pb)| (pb - pa).length())
        .unwrap_or(0.0);
    let label_outward_px = circle_diameter_label_outward_px(
        diameter_px,
        galley_size.x,
        galley_size.y,
        stored_label_offset,
    );
    let world_geom = circle_diameter_dimension_world_geom(
        a,
        b,
        outward_world,
        label_outward_px,
        galley_size.y,
        &project,
    );
    let Some(geom) = project_linear_dimension_geom(&world_geom, &project) else {
        return;
    };
    let label_rect = planar_dimension_label_layout(
        painter,
        &world_geom,
        label_view,
        &label,
        color,
        &project,
    );
    layouts.push(CommittedDimLayout {
        target,
        geom,
        world_geom,
        arc_geom: None,
        angle_display: None,
        angle_radius_world: 0.0,
        label,
        label_rect,
        outward: geom.outward,
        offset: label_outward_px,
    });
}

fn push_arc_dim_layout(
    layouts: &mut Vec<CommittedDimLayout>,
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    frame: &face::SketchFrame,
    doc: &model::Document,
    target: DimLabelTarget,
    line_a: model::ConstraintLine,
    line_b: model::ConstraintLine,
    dim_offset: Option<f32>,
    label: String,
) {
    let Some(display) = angle_constraint_display(doc, line_a, line_b) else {
        return;
    };
    let center = display.center;
    let dir_a = display.dir_a;
    let dir_b = display.dir_b;
    let plane_normal = frame.normal;
    let pixel_offset = effective_arc_dim_offset(dim_offset);
    let radius_world = pixels_to_world_distance(&project, center, dir_a, pixel_offset);
    let label_outset_world = pixels_to_world_distance(&project, center, dir_a, LABEL_OUTSET);
    let Some(world_geom) = arc_dimension_world_geom(
        center,
        dir_a,
        dir_b,
        plane_normal,
        radius_world,
        label_outset_world,
    ) else {
        return;
    };
    let Some(arc_geom) = project_arc_dimension_geom(&world_geom, &project) else {
        return;
    };
    let color = col::DIM_ANNOTATION;
    let label_rect = {
        let galley = painter.layout_no_wrap(
            label.clone(),
            egui::FontId::proportional(LABEL_FONT_SIZE),
            color,
        );
        egui::Rect::from_center_size(arc_geom.label_center, galley.size())
            .expand(dimensions::LABEL_HIT_PAD)
    };
    let outward = dimensions::arc_label_outward_screen(&arc_geom);
    layouts.push(CommittedDimLayout {
        target,
        geom: dimensions::LinearDimensionGeom {
            ext_a_near: arc_geom.start,
            ext_a_far: arc_geom.start,
            ext_b_near: arc_geom.end,
            ext_b_far: arc_geom.end,
            dim_a: arc_geom.start,
            dim_b: arc_geom.end,
            label_center: arc_geom.label_center,
            along: (arc_geom.end - arc_geom.start).normalized(),
            outward,
        },
        world_geom: dimensions::LinearDimensionWorldGeom {
            ext_a_near: center,
            ext_a_far: center,
            ext_b_near: center,
            ext_b_far: center,
            dim_a: center,
            dim_b: center,
            label_center: world_geom.label_center,
            along_world: dir_a,
            outward_world: plane_normal,
        },
        arc_geom: Some(arc_geom),
        angle_display: Some(display),
        angle_radius_world: radius_world,
        label,
        label_rect,
        outward,
        offset: pixel_offset,
    });
}

fn push_committed_dim_layout(
    layouts: &mut Vec<CommittedDimLayout>,
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    label_view: &PlanarLabelView,
    frame: &face::SketchFrame,
    target: DimLabelTarget,
    a: Vec3,
    b: Vec3,
    outward_uv: (f32, f32),
    pixel_offset: f32,
    label: String,
) {
    let color = col::DIM_ANNOTATION;
    let outward_world = uv_dir_to_world(frame.u_axis, frame.v_axis, outward_uv.0, outward_uv.1);
    if outward_world.length_squared() < 1e-8 {
        return;
    }
    let anchor = a.lerp(b, 0.5);
    let offset_world = pixels_to_world_distance(&project, anchor, outward_world, pixel_offset);
    let overshoot_world =
        pixels_to_world_distance(&project, anchor, outward_world, EXTENSION_OVERSHOOT);
    let label_outset_world =
        pixels_to_world_distance(&project, anchor, outward_world, LABEL_OUTSET);
    let world_geom = linear_dimension_world_geom(
        a,
        b,
        outward_world,
        offset_world,
        overshoot_world,
        label_outset_world,
    );
    let Some(geom) = project_linear_dimension_geom(&world_geom, &project) else {
        return;
    };
    let label_rect = planar_dimension_label_layout(
        painter,
        &world_geom,
        label_view,
        &label,
        color,
        &project,
    );
    layouts.push(CommittedDimLayout {
        target,
        geom,
        world_geom,
        arc_geom: None,
        angle_display: None,
        angle_radius_world: 0.0,
        label,
        label_rect,
        outward: geom.outward,
        offset: pixel_offset,
    });
}

fn build_committed_dim_layouts(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    label_view: &PlanarLabelView,
    doc: &model::Document,
    session: SketchSession,
) -> Vec<CommittedDimLayout> {
    let Some(frame) = sketch_geometry_frame(doc, session.sketch) else {
        return Vec::new();
    };
    let mut layouts = Vec::new();
    for (index, constraint) in doc
        .constraints
        .iter()
        .enumerate()
        .filter(|(_, c)| c.sketch == session.sketch)
    {
        let ConstraintKind::Distance { target } = constraint.kind else {
            continue;
        };
        if matches!(target, DistanceTarget::CircleDiameter(_)) {
            continue;
        }
        let Some((a, b)) = constraint_segment_endpoints(doc, index) else {
            continue;
        };
        let outward_uv = match target {
            DistanceTarget::LineLength(_) => {
                let (ua, va) = world_to_local(&frame, a);
                let (ub, vb) = world_to_local(&frame, b);
                preferred_outward_uv(ua, va, ub, vb)
            }
            DistanceTarget::RectWidth(i) | DistanceTarget::RectHeight(i) => {
                let Some(rect) = doc.rects.get(i) else {
                    continue;
                };
                let Some(corners) = rect_world_corners(doc, rect) else {
                    continue;
                };
                let interior = corners.iter().fold(Vec3::ZERO, |acc, c| acc + *c) / 4.0;
                let (iu, iv) = world_to_local(&frame, interior);
                let (ua, va) = world_to_local(&frame, a);
                let (ub, vb) = world_to_local(&frame, b);
                outward_perpendicular_uv(ua, va, ub, vb, iu, iv)
            }
            DistanceTarget::CircleDiameter(_) => unreachable!("handled above"),
            DistanceTarget::LineLineDistance { .. }
            | DistanceTarget::PointPointDistance { .. }
            | DistanceTarget::PointLineDistance { .. } => {
                let (ua, va) = world_to_local(&frame, a);
                let (ub, vb) = world_to_local(&frame, b);
                preferred_outward_uv(ua, va, ub, vb)
            }
        };
        let label = constraint_evaluated_length(doc, index)
            .map(format_length_display)
            .unwrap_or_else(|| "?".to_string());
        push_committed_dim_layout(
            &mut layouts,
            painter,
            &project,
            label_view,
            &frame,
            index,
            a,
            b,
            outward_uv,
            effective_dim_offset(constraint.dim_offset),
            label,
        );
    }
    for (index, constraint) in doc
        .constraints
        .iter()
        .enumerate()
        .filter(|(_, c)| c.sketch == session.sketch)
    {
        let ConstraintKind::Distance {
            target: DistanceTarget::CircleDiameter(i),
        } = constraint.kind
        else {
            continue;
        };
        let Some(circle) = doc.circles.get(i) else {
            continue;
        };
        let Some((a, b)) = constraint_segment_endpoints(doc, index) else {
            continue;
        };
        let label = constraint_evaluated_length(doc, index)
            .map(format_diameter_display)
            .unwrap_or_else(|| "?".to_string());
        push_circle_diameter_dim_layout(
            &mut layouts,
            painter,
            &project,
            label_view,
            &frame,
            circle,
            index,
            a,
            b,
            constraint.dim_offset,
            label,
        );
    }
    for (index, constraint) in doc
        .constraints
        .iter()
        .enumerate()
        .filter(|(_, c)| c.sketch == session.sketch)
    {
        let ConstraintKind::Angle {
            line_a,
            line_b,
            rotation_sign: _,
        } = constraint.kind
        else {
            continue;
        };
        let label = constraint_evaluated_angle(doc, index)
            .map(crate::value::format_angle_display)
            .unwrap_or_else(|| "?".to_string());
        push_arc_dim_layout(
            &mut layouts,
            painter,
            &project,
            &frame,
            doc,
            index,
            line_a,
            line_b,
            constraint.dim_offset,
            label,
        );
    }
    layouts
}

fn draw_committed_dim_layouts<Project>(
    painter: &egui::Painter,
    layouts: &[CommittedDimLayout],
    label_view: &PlanarLabelView,
    project: &Project,
    health: &document_health::DocumentHealth,
    angle_gizmo_constraint: Option<DimLabelTarget>,
    hovered_angle_gizmo: Option<DimLabelTarget>,
    viewport: egui::Rect,
) where
    Project: Fn(Vec3) -> Option<egui::Pos2>,
{
    for layout in layouts {
        let color = document_health::constraint_annotation_color(
            health,
            layout.target,
            col::DIM_ANNOTATION,
        );
        if let (Some(arc_geom), Some(display)) =
            (&layout.arc_geom, layout.angle_display.as_ref())
        {
            let show_gizmo = angle_gizmo_constraint == Some(layout.target);
            let gizmo_hovered = show_gizmo && hovered_angle_gizmo == Some(layout.target);
            // Keep the angle annotation/gizmo on screen: if the lines' meeting point projects
            // outside the viewport, slide the whole annotation to the padded edge.
            let offset = project(display.center)
                .map(|c| {
                    dimensions::angle_gizmo_viewport_offset(c, viewport, ANGLE_GIZMO_VIEWPORT_PAD)
                })
                .unwrap_or(egui::Vec2::ZERO);
            let shifted_arc;
            let arc_ref = if offset == egui::Vec2::ZERO {
                arc_geom
            } else {
                shifted_arc = arc_geom.translated(offset);
                &shifted_arc
            };
            let project_shifted = |w: Vec3| project(w).map(|p| p + offset);
            draw_angle_constraint_annotation(
                painter,
                &project_shifted,
                display,
                layout.world_geom.outward_world,
                arc_ref,
                &layout.label,
                color,
                layout.angle_radius_world,
                show_gizmo,
                gizmo_hovered,
            );
        } else {
            draw_linear_dimension(
                painter,
                &layout.geom,
                &layout.label,
                color,
                Some((&layout.world_geom, label_view, project)),
            );
        }
    }
}

/// Padding (px) keeping the clamped angle gizmo clear of the viewport edge.
const ANGLE_GIZMO_VIEWPORT_PAD: f32 = 48.0;

/// Pixel offset of the extrude-height dimension line from the measured edge.
const EXTRUDE_DIM_OFFSET: f32 = 24.0;

/// Draw a dimension line along one vertical edge of an in-progress extrusion when its
/// height is a constrained (typed) value, so the constraint reads like a sketch dimension.
fn draw_extrude_height_dimension<Project>(
    painter: &egui::Painter,
    project: &Project,
    doc: &model::Document,
    ce: &actions::CreatingExtrusion,
) where
    Project: Fn(Vec3) -> Option<egui::Pos2>,
{
    if !ce.user_edited || ce.faces.is_empty() {
        return;
    }
    let distance = ce.evaluated_distance(doc);
    if distance.abs() < 1e-4 {
        return;
    }
    let Some((corners, normal)) = extrude::face_profile_world(doc, ce.faces[0]) else {
        return;
    };
    if corners.len() < 3 {
        return;
    }
    // One vertical edge of the prism: a base corner up to its extruded top.
    let pa = corners[0];
    let pb = pa + normal * distance;
    // Offset the dimension line away from the solid, within the sketch plane.
    let center = corners
        .iter()
        .fold(Vec3::ZERO, |acc, c| acc + *c)
        / corners.len() as f32;
    let outward_world = (pa - center).normalize_or_zero();
    if outward_world.length_squared() < 1e-8 {
        return;
    }
    let anchor = pa.lerp(pb, 0.5);
    let offset_world = pixels_to_world_distance(project, anchor, outward_world, EXTRUDE_DIM_OFFSET);
    let overshoot_world =
        pixels_to_world_distance(project, anchor, outward_world, EXTENSION_OVERSHOOT);
    let label_outset_world =
        pixels_to_world_distance(project, anchor, outward_world, LABEL_OUTSET);
    let world_geom = linear_dimension_world_geom(
        pa,
        pb,
        outward_world,
        offset_world,
        overshoot_world,
        label_outset_world,
    );
    let Some(geom) = project_linear_dimension_geom(&world_geom, project) else {
        return;
    };
    let label = crate::value::format_length_display(distance.abs());
    draw_linear_dimension::<fn(Vec3) -> Option<egui::Pos2>>(
        painter,
        &geom,
        &label,
        col::DIM_ANNOTATION,
        None,
    );
}

fn angle_gizmo_hit_target(
    layouts: &[CommittedDimLayout],
    pointer: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    angle_gizmo_constraint: Option<DimLabelTarget>,
    viewport: egui::Rect,
) -> Option<DimLabelTarget> {
    let active = angle_gizmo_constraint?;
    layouts.iter().rev().find_map(|layout| {
        if layout.target != active {
            return None;
        }
        let display = layout.angle_display.as_ref()?;
        // Match the on-screen clamping used when drawing, so the handle stays grabbable.
        let offset = project(display.center)
            .map(|c| dimensions::angle_gizmo_viewport_offset(c, viewport, ANGLE_GIZMO_VIEWPORT_PAD))
            .unwrap_or(egui::Vec2::ZERO);
        let project_shifted = |w: Vec3| project(w).map(|p| p + offset);
        let handle = angle_gizmo_handle_world(display, layout.angle_radius_world);
        angle_gizmo_handle_hit(pointer, &project_shifted, handle).then_some(layout.target)
    })
}

fn draw_angle_dim_for_lines<Project>(
    painter: &egui::Painter,
    project: &Project,
    frame: &face::SketchFrame,
    doc: &model::Document,
    line_a: model::ConstraintLine,
    line_b: model::ConstraintLine,
    dim_offset: Option<f32>,
    label: &str,
    show_gizmo: bool,
    gizmo_hovered: bool,
) where
    Project: Fn(Vec3) -> Option<egui::Pos2>,
{
    let Some(display) = angle_constraint_display(doc, line_a, line_b) else {
        return;
    };
    let pixel_offset = effective_arc_dim_offset(dim_offset);
    let radius_world =
        pixels_to_world_distance(&project, display.center, display.dir_a, pixel_offset);
    let label_outset_world =
        pixels_to_world_distance(&project, display.center, display.dir_a, LABEL_OUTSET);
    let Some(world_geom) = arc_dimension_world_geom(
        display.center,
        display.dir_a,
        display.dir_b,
        frame.normal,
        radius_world,
        label_outset_world,
    ) else {
        return;
    };
    let Some(arc_geom) = project_arc_dimension_geom(&world_geom, &project) else {
        return;
    };
    draw_angle_constraint_annotation(
        painter,
        project,
        &display,
        frame.normal,
        &arc_geom,
        label,
        col::DIM_ANNOTATION,
        radius_world,
        show_gizmo,
        gizmo_hovered,
    );
}

fn pointer_over_committed_dim_label(
    layouts: &[CommittedDimLayout],
    pointer: egui::Pos2,
) -> bool {
    layouts.iter().any(|l| l.label_rect.contains(pointer))
}

fn dim_input_layout_centered_on(label_rect: egui::Rect, text: &str) -> DimInputLayout {
    let size = dim_input_size_for_text(text);
    let pos = label_rect.center() - size * 0.5;
    layout_at(pos, size)
}

fn handle_committed_dim_label_double_click(
    ui: &egui::Ui,
    layouts: &[CommittedDimLayout],
    state: &mut AppState,
) -> bool {
    if !state.can_edit_sketch_dimensions() || state.editing_committed_dim.is_some() {
        return false;
    }
    if !ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary)) {
        return false;
    }
    let Some(pos) = ui.input(|i| i.pointer.hover_pos()) else {
        return false;
    };
    let Some(hit) = layouts.iter().rev().find(|h| h.label_rect.contains(pos)) else {
        return false;
    };
    state.apply(Action::BeginEditCommittedDim { target: hit.target });
    true
}

/// The extrude-able face (rectangle/circle) under the cursor, if any.
fn pick_extrude_face(
    pp: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    eye: Vec3,
) -> Option<model::ExtrudeFace> {
    match pick_sketch_face(pp, project, doc, eye)? {
        FaceId::Rect(i) => Some(model::ExtrudeFace::Rect(i)),
        FaceId::Circle(i) => Some(model::ExtrudeFace::Circle(i)),
        FaceId::ConstructionPlane(_) | FaceId::ExtrudeCap { .. } | FaceId::ExtrudeSide { .. } => {
            None
        }
    }
}

fn extrude_face_id(face: model::ExtrudeFace) -> FaceId {
    match face {
        model::ExtrudeFace::Rect(i) => FaceId::Rect(i),
        model::ExtrudeFace::Circle(i) => FaceId::Circle(i),
    }
}

/// Object under the cursor to extrude up to (vertex preferred, then face/plane), with the
/// signed distance from the extrusion base to its extended plane. Excludes the faces being
/// extruded.
/// Distance, in sketch units, that the extrude gizmo handle floats above the
/// solid's top face so it sits a little above the surface rather than on it.
const EXTRUDE_GIZMO_LIFT: f32 = 4.0;

/// egui id of the floating extrude-distance text field.
const EXTRUDE_DISTANCE_FIELD_ID: &str = "extrude_distance_input";

/// Where the extrude gizmo handle is drawn along the normal: the actual extrude
/// distance plus a small lift in the extrusion direction.
fn extrude_gizmo_display_offset(distance: f32) -> f32 {
    let dir = if distance < 0.0 { -1.0 } else { 1.0 };
    distance + dir * EXTRUDE_GIZMO_LIFT
}

fn pick_extrude_target(
    pp: egui::Pos2,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    base: Vec3,
    normal: Vec3,
    exclude: &[model::ExtrudeFace],
    eye: Vec3,
) -> Option<(model::ExtrudeTarget, f32)> {
    use model::ExtrudeTarget;
    const VERTEX_RADIUS_PX: f32 = 12.0;

    // Nearest vertex.
    let mut best: Option<(f32, ExtrudeTarget)> = None;
    for vertex in snapping::all_sketch_vertices(doc) {
        if let Some(world) = extrude::constraint_point_world(doc, vertex) {
            if let Some(sp) = project(world) {
                let d = (sp - pp).length();
                if d <= VERTEX_RADIUS_PX && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                    best = Some((d, ExtrudeTarget::Vertex(vertex)));
                }
            }
        }
    }

    let target = if let Some((_, t)) = best {
        t
    } else {
        match pick_sketch_face(pp, project, doc, eye)? {
            FaceId::Rect(i) if !exclude.contains(&model::ExtrudeFace::Rect(i)) => {
                ExtrudeTarget::Face(model::ExtrudeFace::Rect(i))
            }
            FaceId::Circle(i) if !exclude.contains(&model::ExtrudeFace::Circle(i)) => {
                ExtrudeTarget::Face(model::ExtrudeFace::Circle(i))
            }
            FaceId::ConstructionPlane(i) => ExtrudeTarget::Plane(i),
            _ => return None,
        }
    };
    let dist = extrude::target_distance(doc, base, normal, target)?;
    Some((target, dist))
}

/// Snap radius in screen pixels, converted to sketch units per the current view.
const SNAP_RADIUS_PX: f32 = 12.0;

/// The snap radius in sketch-local units near `world` on the sketch plane.
fn snap_radius_uv(
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    frame: &face::SketchFrame,
    world: Vec3,
) -> f32 {
    pixels_to_world_distance(project, world, frame.u_axis, SNAP_RADIUS_PX)
}

/// World position and target of the active snap (dragged vertex or line end), for the marker.
fn active_snap(state: &AppState, frame: &face::SketchFrame) -> Option<(Vec3, snapping::SnapTarget)> {
    if let Some((point, target)) = state.active_snap {
        let (u, v) = crate::geometric_constraints::point_uv(&state.doc, point).ok()?;
        return Some((face::local_to_world(frame, u, v), target));
    }
    if let Some(target) = state.line_end_snap {
        if let Some(cl) = &state.creating_line {
            return Some((cl.end_point(frame, &state.doc), target));
        }
    }
    if let Some(target) = state.rect_opposite_snap {
        if let Some(cr) = &state.creating_rect {
            return Some((cr.end_point(frame, &state.doc), target));
        }
    }
    None
}

/// The constraint icon representing a snap target.
fn snap_icon(target: snapping::SnapTarget) -> icons::IconId {
    match target {
        snapping::SnapTarget::Midpoint(_) => icons::IconId::Midpoint,
        snapping::SnapTarget::Vertex(_)
        | snapping::SnapTarget::Origin
        | snapping::SnapTarget::OnLine(_)
        | snapping::SnapTarget::OnLineExtension(_) => icons::IconId::Coincident,
    }
}

/// Snap a world-space sketch-plane point to nearby geometry, returning the (possibly snapped)
/// world point and the snap target it latched onto.
fn snap_ground_point(
    state: &AppState,
    session: SketchSession,
    frame: &face::SketchFrame,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    world: Vec3,
    exclude: &[ConstraintPoint],
) -> (Vec3, Option<snapping::SnapTarget>) {
    if !state.snapping_enabled {
        return (world, None);
    }
    let (u, v) = world_to_local(frame, world);
    let radius = snap_radius_uv(project, frame, world);
    if let Some(snap) = snapping::find_snap(&state.doc, session.sketch, (u, v), radius, exclude) {
        return (
            face::local_to_world(frame, snap.uv.0, snap.uv.1),
            Some(snap.target),
        );
    }
    // No direct snap: fall back to the extension guides of the last-hovered vertex (#21),
    // letting the point latch onto the infinite extension of those edges.
    if !state.extension_anchors.is_empty() {
        if let Some(snap) =
            snapping::find_extension_snap(&state.doc, &state.extension_anchors, (u, v), radius, exclude)
        {
            return (
                face::local_to_world(frame, snap.uv.0, snap.uv.1),
                Some(snap.target),
            );
        }
    }
    (world, None)
}

/// Update the active extension-snap guides (#21) from the latest snap result: hovering a real
/// vertex makes its incident edges the extension anchors; other snaps leave the guides in place
/// so the user can pull away from the vertex and still snap to its edges' extensions.
fn update_extension_anchors(state: &mut AppState, snap_target: Option<snapping::SnapTarget>) {
    if let Some(snapping::SnapTarget::Vertex(point)) = snap_target {
        state.extension_anchors = snapping::vertex_extension_anchors(point);
    }
}

fn handle_vertex_drag(
    ui: &egui::Ui,
    drag: &mut Option<VertexDrag>,
    state: &mut AppState,
    session: SketchSession,
    viewport: egui::Rect,
    vp: &glam::Mat4,
    cam: &camera::Camera,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    pointer_screen: Option<egui::Pos2>,
) -> bool {
    if state.creating_rect.is_some()
        || state.creating_line.is_some()
        || state.creating_circle.is_some()
        || state.editing_committed_dim.is_some()
    {
        *drag = None;
        return false;
    }

    let primary_down = ui.input(|i| i.pointer.primary_down());
    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
    let primary_released = ui.input(|i| i.pointer.primary_released());

    if let Some(active) = drag.as_ref() {
        if primary_released {
            // Leaving a snapped vertex in place pins it with the implied constraint.
            if let Some((point, target)) = state.active_snap.take() {
                let _ = state.apply(Action::ApplySnapConstraint { point, target });
            }
            *drag = None;
            return false;
        }
        if primary_down {
            if let Some(pp) = pointer_screen {
                if let Some(world) =
                    sketch_plane_point(cam, viewport, vp, &state.doc, session, pp)
                {
                    let frame = sketch_geometry_frame(&state.doc, session.sketch).unwrap();
                    let (mut u, mut v) = world_to_local(&frame, world);
                    state.active_snap = None;
                    if state.snapping_enabled {
                        let radius = snap_radius_uv(project, &frame, world);
                        let exclude = vertex_drag::coincident_group(
                            &state.doc,
                            session.sketch,
                            active.point,
                        );
                        if let Some(snap) = snapping::find_snap(
                            &state.doc,
                            session.sketch,
                            (u, v),
                            radius,
                            &exclude,
                        ) {
                            u = snap.uv.0;
                            v = snap.uv.1;
                            state.active_snap = Some((active.point, snap.target));
                        }
                    }
                    let _ = state.apply(Action::DragVertex {
                        point: active.point,
                        u,
                        v,
                    });
                }
            }
            return true;
        }
        *drag = None;
    }

    if primary_pressed {
        if let Some(pp) = pointer_screen {
            if let Some((point, _)) =
                nearest_sketch_point_in_sketch(pp, project, &state.doc, session.sketch)
            {
                let element = vertex_drag::scene_element_for_point(point);
                if document_health::require_element_editable(&state.document_health, element)
                    .is_err()
                {
                    return false;
                }
                *drag = Some(VertexDrag { point });
                state.apply(Action::ClickSceneElement {
                    element: SceneElement::Point(point),
                    additive: ui.input(|i| additive_click_modifiers(&i.modifiers)),
                });
                return true;
            }
        }
    }

    false
}

fn handle_line_drag(
    ui: &egui::Ui,
    state: &mut AppState,
    session: SketchSession,
    viewport: egui::Rect,
    vp: &glam::Mat4,
    cam: &camera::Camera,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    pointer_screen: Option<egui::Pos2>,
) -> bool {
    if state.creating_rect.is_some()
        || state.creating_line.is_some()
        || state.creating_circle.is_some()
        || state.editing_committed_dim.is_some()
    {
        if state.line_drag_session.is_some() {
            let _ = state.apply(Action::EndLineDrag);
        }
        return false;
    }

    let primary_down = ui.input(|i| i.pointer.primary_down());
    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
    let primary_released = ui.input(|i| i.pointer.primary_released());

    if state.line_drag_session.is_some() {
        if primary_released {
            let _ = state.apply(Action::EndLineDrag);
            return false;
        }
        if primary_down {
            if let Some(pp) = pointer_screen {
                if let Some(world) =
                    sketch_plane_point(cam, viewport, vp, &state.doc, session, pp)
                {
                    let frame = sketch_geometry_frame(&state.doc, session.sketch).unwrap();
                    let (u, v) = world_to_local(&frame, world);
                    let _ = state.apply(Action::DragLine { u, v });
                }
            }
            return true;
        }
        let _ = state.apply(Action::EndLineDrag);
        return false;
    }

    if primary_pressed {
        if let Some(pp) = pointer_screen {
            if nearest_sketch_point_in_sketch(pp, project, &state.doc, session.sketch).is_some() {
                return false;
            }
            if let Some((target, _)) =
                nearest_sketch_line_in_sketch(pp, project, &state.doc, session.sketch)
            {
                let element = vertex_drag::scene_element_for_line(target);
                if document_health::require_element_editable(&state.document_health, element)
                    .is_err()
                {
                    return false;
                }
                if let Some(world) =
                    sketch_plane_point(cam, viewport, vp, &state.doc, session, pp)
                {
                    let frame = sketch_geometry_frame(&state.doc, session.sketch).unwrap();
                    let (u, v) = world_to_local(&frame, world);
                    let _ = state.apply(Action::BeginLineDrag {
                        target,
                        anchor_u: u,
                        anchor_v: v,
                    });
                    let _ = state.apply(Action::DragLine { u, v });
                    state.apply(Action::ClickSceneElement {
                        element,
                        additive: ui.input(|i| additive_click_modifiers(&i.modifiers)),
                    });
                    return true;
                }
            }
        }
    }

    false
}

fn handle_angle_gizmo_drag(
    ui: &egui::Ui,
    layouts: &[CommittedDimLayout],
    drag: &mut Option<AngleGizmoDrag>,
    state: &mut AppState,
    session: SketchSession,
    viewport: egui::Rect,
    vp: &glam::Mat4,
    cam: &camera::Camera,
    angle_gizmo_constraint: DimLabelTarget,
) -> bool {
    if !state.can_edit_sketch_dimensions() || state.editing_committed_dim.is_none() {
        return false;
    }
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let primary_down = ui.input(|i| i.pointer.primary_down());
    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
    let primary_released = ui.input(|i| i.pointer.primary_released());
    let Some(frame) = sketch_geometry_frame(&state.doc, session.sketch) else {
        return false;
    };

    if let Some(active) = drag.as_ref() {
        if primary_released {
            *drag = None;
            return false;
        }
        if primary_down {
            if let Some(pp) = pointer {
                if let Some(layout) =
                    layouts.iter().find(|l| l.target == active.constraint_id)
                {
                    if let Some(display) = layout.angle_display {
                        if let Some(hit) = cam.ray_plane_hit(
                            pp, viewport, vp, display.center, frame.normal,
                        ) {
                            if let Some(angle_rad) =
                                angle_rad_from_sketch_hit(&display, frame.normal, hit)
                            {
                                let _ = state.apply(Action::SetConstraintAngleValue {
                                    constraint_id: active.constraint_id,
                                    angle_rad,
                                });
                            }
                        }
                    }
                }
            }
            return true;
        }
        *drag = None;
    }

    if primary_pressed {
        if let Some(pos) = pointer {
            let project = |w: glam::Vec3| cam.project(w, viewport, vp);
            if let Some(target) =
                angle_gizmo_hit_target(layouts, pos, &project, Some(angle_gizmo_constraint), viewport)
            {
                if document_health::require_constraint_editable(
                    &state.document_health,
                    &state.doc,
                    target,
                )
                .is_err()
                {
                    return false;
                }
                *drag = Some(AngleGizmoDrag {
                    constraint_id: target,
                });
                return true;
            }
        }
    }

    false
}

fn handle_committed_dim_label_drag(
    ui: &egui::Ui,
    layouts: &[CommittedDimLayout],
    drag: &mut Option<DimLabelDrag>,
    state: &mut AppState,
) -> bool {
    if !state.can_edit_sketch_dimensions() || state.editing_committed_dim.is_some() {
        return false;
    }

    let pointer = ui.input(|i| i.pointer.hover_pos());
    let primary_down = ui.input(|i| i.pointer.primary_down());
    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
    let primary_released = ui.input(|i| i.pointer.primary_released());
    let double_clicked =
        ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));

    if let Some(active) = drag.as_ref() {
        if primary_released || double_clicked {
            *drag = None;
            return !double_clicked;
        }
        if primary_down {
            if let Some(pos) = pointer {
                let moved = (pos - active.anchor_screen).length();
                if moved >= DIM_LABEL_DRAG_THRESHOLD_PX {
                    let delta = (pos - active.anchor_screen).dot(active.outward);
                    let offset = if constraint_is_circle_diameter(&state.doc, active.target) {
                        effective_circle_diameter_label_offset(Some(active.start_offset + delta))
                    } else if constraint_is_angle(&state.doc, active.target) {
                        effective_arc_dim_offset(Some(active.start_offset + delta))
                    } else {
                        effective_dim_offset(Some(active.start_offset + delta))
                    };
                    state.apply(Action::SetDimLabelOffset {
                        target: active.target,
                        offset,
                    });
                    return true;
                }
            }
            return layouts.iter().any(|layout| {
                pointer.is_some_and(|pos| layout.label_rect.contains(pos))
            });
        }
        *drag = None;
    }

    if primary_pressed && !double_clicked {
        if let Some(pos) = pointer {
            if let Some(hit) = layouts.iter().rev().find(|h| h.label_rect.contains(pos)) {
                if document_health::require_constraint_editable(
                    &state.document_health,
                    &state.doc,
                    hit.target,
                )
                .is_err()
                {
                    return false;
                }
                *drag = Some(DimLabelDrag {
                    target: hit.target,
                    outward: hit.outward,
                    start_offset: hit.offset,
                    anchor_screen: pos,
                });
                return true;
            }
        }
    }

    false
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
        FaceId::Circle(i) => {
            if let Some(circle) = doc.circles.get(i) {
                draw_circle_face_highlight(painter, project, doc, circle, color);
            }
        }
        FaceId::ExtrudeCap {
            extrusion,
            profile,
            top,
        } => {
            if let Some(poly) = extrude::cap_polygon_world(doc, extrusion, profile, top) {
                draw_polygon_face_highlight(painter, project, &poly, color);
            }
        }
        FaceId::ExtrudeSide {
            extrusion,
            profile,
            edge,
        } => {
            if let Some(quad) = extrude::side_quad_world(doc, extrusion, profile, edge as usize) {
                draw_polygon_face_highlight(painter, project, &quad, color);
            }
        }
    }
}

/// Highlight the object an in-progress extrusion is currently snapping to (a vertex,
/// face, or plane), so the extrude-to-object target is visible while dragging the gizmo.
fn draw_extrude_target_highlight(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    target: model::ExtrudeTarget,
    color: egui::Color32,
) {
    match target {
        model::ExtrudeTarget::Vertex(point) => {
            if let Some(sp) = extrude::constraint_point_world(doc, point).and_then(project) {
                painter.circle_filled(sp, 5.0, color);
                painter.circle_stroke(sp, 8.0, egui::Stroke::new(2.0, color));
            }
        }
        model::ExtrudeTarget::Face(face) => {
            draw_face_highlight(painter, project, doc, extrude_face_id(face), color);
        }
        model::ExtrudeTarget::Plane(index) => {
            draw_face_highlight(painter, project, doc, FaceId::ConstructionPlane(index), color);
        }
    }
}

impl App {
    /// Tab for in-progress sketch dimensions. Consumes Tab so focus cannot escape to the toolbar
    /// while creating geometry. Enter is handled after dim TextEdits render (see draw_viewport).
    fn handle_in_progress_object_keyboard(&mut self, ui: &mut egui::Ui) {
        if self.state.command_palette.open {
            return;
        }
        if parameters::parameter_field_focused(ui.ctx(), &self.state.doc) {
            return;
        }

        let tab_pressed = ui.input(|i| i.key_pressed(egui::Key::Tab));

        if self.state.creating_rect.is_some() {
            if tab_pressed {
                ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
                let focused = self
                    .state
                    .creating_rect
                    .as_ref()
                    .map(|cr| cr.focused)
                    .unwrap_or(0);
                self.state
                    .apply(Action::FocusRectDimension {
                        axis: next_rect_focus_axis(focused),
                    });
            }
            return;
        }

        if self.state.creating_line.is_some() {
            if tab_pressed {
                ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
                if let Some(cl) = &mut self.state.creating_line {
                    cl.pending_focus = true;
                }
            }
            return;
        }

        if self.state.creating_plane.is_some() {
            if tab_pressed {
                ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
                if self
                    .state
                    .creating_plane
                    .as_ref()
                    .is_some_and(|cp| cp.reference.is_axis())
                {
                    let focused = self
                        .state
                        .creating_plane
                        .as_ref()
                        .map(|cp| cp.focused)
                        .unwrap_or(PlaneDim::Offset);
                    self.state.apply(Action::FocusPlaneDim {
                        dim: next_plane_focus_dim(focused),
                    });
                } else if let Some(cp) = &mut self.state.creating_plane {
                    cp.pending_focus = true;
                }
            }
            return;
        }

        if self.state.editing_committed_dim.is_some() && tab_pressed {
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
        }
    }

    fn draw_viewport(
        &mut self,
        ui: &mut egui::Ui,
        render_state: Option<&eframe::egui_wgpu::RenderState>,
    ) {
        self.handle_in_progress_object_keyboard(ui);

        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let viewport = response.rect;
        self.last_viewport = Some(viewport);
        self.state.apply_pending_sketch_reframe(viewport);
        let mut inline_parameter_field_results = Vec::<SketchDimFieldResult>::new();

        // Apply scripted right-drag as direct camera motion.
        self.synthetic.apply_pending_drag(viewport, |delta, modifiers, h| {
            if modifiers.shift {
                self.state.cam.pan(delta, h);
                if let Some(log) = &self.state.command_log {
                    log.borrow_mut().note_pan(delta);
                }
            } else {
                self.state.cam.orbit(delta);
                if let Some(log) = &self.state.command_log {
                    log.borrow_mut().note_orbit(delta);
                }
            }
        });

        if response.dragged_by(egui::PointerButton::Secondary) {
            if ui.input(|i| i.modifiers.shift) {
                let delta = response.drag_delta();
                self.state.cam.pan(delta, viewport.height());
                if let Some(log) = &self.state.command_log {
                    log.borrow_mut().note_pan(delta);
                }
            } else {
                let delta = response.drag_delta();
                self.state.cam.orbit(delta);
                if let Some(log) = &self.state.command_log {
                    log.borrow_mut().note_orbit(delta);
                }
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                let focal = response.hover_pos().unwrap_or(viewport.center());
                self.state.cam.zoom(scroll, focal, viewport);
                if let Some(log) = &self.state.command_log {
                    log.borrow_mut().note_zoom(scroll);
                }
            }
        }

        let cam = self.state.cam.clone();
        let vp = cam.view_proj(viewport);
        let cam_project = cam.clone();
        let project = move |w: Vec3| cam_project.project(w, viewport, &vp);

        let sketch_session = self.state.sketch_session;
        let planar_label_view = sketch_session.and_then(|session| {
            sketch_geometry_frame(&self.state.doc, session.sketch)
                .map(|frame| PlanarLabelView::from_camera_and_plane(&cam, frame.normal))
        });
        let committed_dim_layouts = sketch_session.zip(planar_label_view).map(|(session, view)| {
            build_committed_dim_layouts(&painter, &project, &view, &self.state.doc, session)
        });
        let viewport_owns_pointer = self.vertex_drag.is_some()
            || self.state.line_drag_session.is_some()
            || self.dim_label_drag.is_some()
            || self.angle_gizmo_drag.is_some()
            || response.dragged_by(egui::PointerButton::Secondary);
        let pointer_screen = viewport_pointer_pos(&response, viewport_owns_pointer);
        let layouts_slice = committed_dim_layouts.as_deref().unwrap_or(&[]);
        let angle_gizmo_constraint = angle_gizmo_constraint_for_edit(
            self.state.editing_committed_dim.as_ref(),
            &self.state.doc,
        );
        if angle_gizmo_constraint.is_none() {
            self.angle_gizmo_drag = None;
        }
        let angle_dim_constraints: HashSet<usize> = layouts_slice
            .iter()
            .filter(|layout| layout.arc_geom.is_some())
            .map(|layout| layout.target)
            .collect();
        let constraint_graphics = viewport_constraints_for_selection(
            &self.state.doc,
            &self.state.element_visibility,
            &self.state.scene_selection,
            &angle_dim_constraints,
        );
        let constraint_icon_hits =
            build_constraint_icon_hits(&project, &constraint_graphics);
        let over_constraint_icon = pointer_screen.is_some_and(|pp| {
            pointer_over_constraint_icon(&constraint_icon_hits, pp).is_some()
        });
        let over_committed_dim_label = self.state.can_edit_sketch_dimensions()
            && (pointer_screen.is_some_and(|pp| {
                pointer_over_committed_dim_label(layouts_slice, pp)
            }) || self.dim_label_drag.is_some());
        if handle_committed_dim_label_double_click(ui, layouts_slice, &mut self.state) {
            self.dim_label_drag = None;
            self.angle_gizmo_drag = None;
        }
        let mut angle_gizmo_dragging = false;
        if let (Some(session), Some(active_gizmo)) =
            (sketch_session, angle_gizmo_constraint)
        {
            angle_gizmo_dragging = handle_angle_gizmo_drag(
                ui,
                layouts_slice,
                &mut self.angle_gizmo_drag,
                &mut self.state,
                session,
                viewport,
                &vp,
                &cam,
                active_gizmo,
            );
        }
        if angle_gizmo_dragging {
            self.dim_label_drag = None;
            set_viewport_cursor(
                ui.ctx(),
                &response,
                true,
                egui::CursorIcon::Grabbing,
            );
        } else if handle_committed_dim_label_drag(
            ui,
            layouts_slice,
            &mut self.dim_label_drag,
            &mut self.state,
        ) {
            self.angle_gizmo_drag = None;
            set_viewport_cursor(
                ui.ctx(),
                &response,
                true,
                egui::CursorIcon::Grabbing,
            );
        } else if over_committed_dim_label {
            set_viewport_cursor(ui.ctx(), &response, false, egui::CursorIcon::Grab);
        } else if over_constraint_icon {
            set_viewport_cursor(ui.ctx(), &response, false, egui::CursorIcon::PointingHand);
        } else if let Some(pp) = pointer_screen {
            let project = |w: glam::Vec3| cam.project(w, viewport, &vp);
            if angle_gizmo_hit_target(
                layouts_slice,
                pp,
                &project,
                angle_gizmo_constraint,
                viewport,
            )
            .is_some()
            {
                set_viewport_cursor(ui.ctx(), &response, false, egui::CursorIcon::Grab);
            }
        }

        let mut vertex_dragging = false;
        let mut line_dragging = false;
        if matches!(self.state.tool, Tool::Select | Tool::Constraint)
            && self.state.editing_committed_dim.is_none()
            && !over_committed_dim_label
            && self.dim_label_drag.is_none()
            && !angle_gizmo_dragging
            && self.angle_gizmo_drag.is_none()
        {
            if let Some(session) = sketch_session {
                line_dragging = handle_line_drag(
                    ui,
                    &mut self.state,
                    session,
                    viewport,
                    &vp,
                    &cam,
                    &project,
                    pointer_screen,
                );
                if !line_dragging && self.state.line_drag_session.is_none() {
                    vertex_dragging = handle_vertex_drag(
                        ui,
                        &mut self.vertex_drag,
                        &mut self.state,
                        session,
                        viewport,
                        &vp,
                        &cam,
                        &project,
                        pointer_screen,
                    );
                }
                if vertex_dragging || line_dragging || self.state.line_drag_session.is_some() {
                    set_viewport_cursor(
                        ui.ctx(),
                        &response,
                        true,
                        egui::CursorIcon::Grabbing,
                    );
                } else if let Some(pp) = pointer_screen {
                    if nearest_sketch_line_in_sketch(
                        pp,
                        &project,
                        &self.state.doc,
                        session.sketch,
                    )
                    .is_some()
                    {
                        set_viewport_cursor(ui.ctx(), &response, false, egui::CursorIcon::Grab);
                    }
                }
            }
        }

        let suppress_hover_highlight = suppress_viewport_pick_hover(
            ui,
            &response,
            self.vertex_drag.is_some(),
            self.state.line_drag_session.is_some(),
            self.dim_label_drag.is_some(),
            self.angle_gizmo_drag.is_some(),
            self.state
                .creating_plane
                .as_ref()
                .is_some_and(|cp| cp.axis_gizmo_drag.is_some()),
        );

        if matches!(self.state.tool, Tool::Select | Tool::Constraint)
            && self.state.editing_committed_dim.is_none()
            && !over_committed_dim_label
            && self.dim_label_drag.is_none()
            && self.angle_gizmo_drag.is_none()
            && !vertex_dragging
            && !line_dragging
            && self.vertex_drag.is_none()
            && self.state.line_drag_session.is_none()
        {
            if let Some(pp) = pointer_screen {
                let gp = cam.ground_point(pp, viewport, &vp);
                if ui.input(|i| i.pointer.primary_pressed()) {
                    let additive = ui.input(|i| additive_click_modifiers(&i.modifiers));
                    if let Some(index) =
                        pointer_over_constraint_icon(&constraint_icon_hits, pp)
                    {
                        self.state.apply(Action::ClickSceneElement {
                            element: SceneElement::Constraint(index),
                            additive,
                        });
                    } else if let Some(target) =
                        resolve_pick_target(pp, &project, gp, &self.state.doc)
                    {
                        if let Some(element) = scene_element_from_pick(&target.kind) {
                            self.state
                                .apply(Action::ClickSceneElement { element, additive });
                        } else if !additive {
                            self.state.apply(Action::ClearSceneSelection);
                        }
                    } else if !additive {
                        self.state.apply(Action::ClearSceneSelection);
                    }
                } else if !self.gpu_viewport && !suppress_hover_highlight {
                    if let Some(target) = resolve_pick_target(pp, &project, gp, &self.state.doc) {
                        if scene_element_from_pick(&target.kind).is_some() {
                            target.draw_highlight(&painter, &project, &self.state.doc);
                        }
                    }
                }
            }
        }

        if self.state.tool == Tool::Sketch {
            if let Some(pp) = pointer_screen {
                if ui.input(|i| i.pointer.primary_pressed()) {
                    if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc, self.state.cam.eye()) {
                        self.state.apply(Action::BeginSketch {
                            face,
                            viewport: Some(viewport),
                        });
                    }
                } else if !self.gpu_viewport && !suppress_hover_highlight {
                    if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc, self.state.cam.eye()) {
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
        }

        if self.state.tool == Tool::Rectangle {
            if self.state.sketch_session.is_none() {
                if let Some(pp) = pointer_screen {
                    if ui.input(|i| i.pointer.primary_pressed()) {
                        if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc, self.state.cam.eye()) {
                            self.state.apply(Action::BeginSketch {
                                face,
                                viewport: Some(viewport),
                            });
                        }
                    } else if !self.gpu_viewport && !suppress_hover_highlight {
                        if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc, self.state.cam.eye()) {
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
            } else if let (Some(session), Some(pp)) =
                (self.state.sketch_session, pointer_screen)
            {
                if let Some(gp) =
                    sketch_plane_point(&cam, viewport, &vp, &self.state.doc, session, pp)
                {
                    let frame = sketch_geometry_frame(&self.state.doc, session.sketch).unwrap();
                    let was_creating = self.state.creating_rect.is_some();
                    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
                    let (sgp, snap_target) =
                        snap_ground_point(&self.state, session, &frame, &project, gp, &[]);
                    update_extension_anchors(&mut self.state, snap_target);

                    if !was_creating && primary_pressed && !over_committed_dim_label {
                        self.state.rect_origin_snap = snap_target;
                        self.state.rect_opposite_snap = None;
                        self.state.creating_rect = Some(CreatingRect {
                            origin: sgp,
                            texts: ["".to_string(), "".to_string()],
                            focused: 0,
                            last_mouse: sgp,
                            user_edited: [false, false],
                            pending_focus: true,
                            construction: self.state.draw_construction,
                        });
                        self.state.status = "Move mouse • type to lock dim • Tab cycle • click/Enter commit • Esc cancel"
                            .to_string();
                    }

                    let mut commit_click = false;
                    if let Some(cr) = &mut self.state.creating_rect {
                        let end = cr.end_point(&frame, &self.state.doc);
                        let (ou, ov) = world_to_local(&frame, cr.origin);
                        let (eu, ev) = world_to_local(&frame, end);
                        let preview = Rect::from_local_corners(session.sketch, ou, ov, eu, ev);
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

                        if should_commit_sketch_on_click(
                            was_creating,
                            primary_pressed,
                            over_input || over_committed_dim_label,
                        ) {
                            commit_click = true;
                        } else if !over_input && !over_committed_dim_label {
                            cr.last_mouse = sgp;
                            let (au, av) = world_to_local(&frame, cr.origin);
                            let (bu, bv) = world_to_local(&frame, sgp);
                            if !cr.user_edited[0] {
                                cr.texts[0] = format_live_dimension((bu - au).abs());
                            }
                            if !cr.user_edited[1] {
                                cr.texts[1] = format_live_dimension((bv - av).abs());
                            }
                            // The opposite corner only tracks the cursor when both dims are free.
                            self.state.rect_opposite_snap =
                                if cr.user_edited[0] || cr.user_edited[1] {
                                    None
                                } else {
                                    snap_target
                                };
                        }
                    }
                    if commit_click {
                        self.state.apply(Action::CommitRectangle);
                    }
                }
            }
        }

        if self.state.tool == Tool::Circle {
            if self.state.sketch_session.is_none() {
                if let Some(pp) = pointer_screen {
                    if ui.input(|i| i.pointer.primary_pressed()) {
                        if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc, self.state.cam.eye()) {
                            self.state.apply(Action::BeginSketch {
                                face,
                                viewport: Some(viewport),
                            });
                        }
                    } else if !self.gpu_viewport && !suppress_hover_highlight {
                        if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc, self.state.cam.eye()) {
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
            } else if let (Some(session), Some(pp)) =
                (self.state.sketch_session, pointer_screen)
            {
                if let Some(gp) =
                    sketch_plane_point(&cam, viewport, &vp, &self.state.doc, session, pp)
                {
                    let frame = sketch_geometry_frame(&self.state.doc, session.sketch).unwrap();
                    let was_creating = self.state.creating_circle.is_some();
                    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());

                    if !was_creating && primary_pressed && !over_committed_dim_label {
                        // Snap the center; the rim follows the cursor freely.
                        let (center, center_snap) =
                            snap_ground_point(&self.state, session, &frame, &project, gp, &[]);
                        update_extension_anchors(&mut self.state, center_snap);
                        self.state.circle_center_snap = center_snap;
                        self.state.creating_circle = Some(CreatingCircle {
                            origin: center,
                            text: String::new(),
                            last_mouse: gp,
                            user_edited: false,
                            pending_focus: true,
                            construction: self.state.draw_construction,
                        });
                        self.state.status = "Move mouse • type to lock diameter • click/Enter commit • Esc cancel"
                            .to_string();
                    }

                    let mut commit_click = false;
                    if let Some(cc) = &mut self.state.creating_circle {
                        let rim = cc.rim_point(&frame, &self.state.doc);
                        let over_input = project(cc.origin).zip(project(rim)).is_some_and(
                            |(pa, pb)| {
                                pointer_over_dim_inputs(pp, &[line_dim_layout(pa, pb, &cc.text)])
                            },
                        );

                        if should_commit_sketch_on_click(
                            was_creating,
                            primary_pressed,
                            over_input || over_committed_dim_label,
                        ) {
                            commit_click = true;
                        } else if !over_input && !over_committed_dim_label {
                            cc.last_mouse = gp;
                            if !cc.user_edited {
                                let radius = cc.radius(&frame, &self.state.doc);
                                cc.text = format_live_dimension(radius * 2.0);
                            }
                        }
                    }
                    if commit_click {
                        self.state.apply(Action::CommitCircle);
                    }
                }
            }
        }

        if self.state.tool == Tool::Line {
            if self.state.sketch_session.is_none() {
                if let Some(pp) = pointer_screen {
                    if ui.input(|i| i.pointer.primary_pressed()) {
                        if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc, self.state.cam.eye()) {
                            self.state.apply(Action::BeginSketch {
                                face,
                                viewport: Some(viewport),
                            });
                        }
                    } else if !self.gpu_viewport && !suppress_hover_highlight {
                        if let Some(face) = pick_sketch_face(pp, &project, &self.state.doc, self.state.cam.eye()) {
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
            } else if let (Some(session), Some(pp)) =
                (self.state.sketch_session, pointer_screen)
            {
                if let Some(gp) =
                    sketch_plane_point(&cam, viewport, &vp, &self.state.doc, session, pp)
                {
                    let frame = sketch_geometry_frame(&self.state.doc, session.sketch).unwrap();
                    let was_creating = self.state.creating_line.is_some();
                    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());

                    // Snap the cursor to nearby geometry (vertices, midpoints, lines).
                    let (sgp, snap_target) =
                        snap_ground_point(&self.state, session, &frame, &project, gp, &[]);
                    update_extension_anchors(&mut self.state, snap_target);

                    if !was_creating && primary_pressed && !over_committed_dim_label {
                        self.state.line_start_snap = snap_target;
                        self.state.line_end_snap = None;
                        self.state.creating_line = Some(CreatingLine {
                            origin: sgp,
                            text: String::new(),
                            last_mouse: sgp,
                            user_edited: false,
                            pending_focus: true,
                            construction: self.state.draw_construction,
                        });
                        self.state.status = "Move mouse • type to lock length • click/Enter commit • Esc cancel"
                            .to_string();
                    }

                    let mut commit_click = false;
                    if let Some(cl) = &mut self.state.creating_line {
                        let end = cl.end_point(&frame, &self.state.doc);
                        let over_input = project(cl.origin).zip(project(end)).is_some_and(
                            |(pa, pb)| {
                                pointer_over_dim_inputs(pp, &[line_dim_layout(pa, pb, &cl.text)])
                            },
                        );

                        if should_commit_sketch_on_click(
                            was_creating,
                            primary_pressed,
                            over_input || over_committed_dim_label,
                        ) {
                            commit_click = true;
                        } else if !over_input && !over_committed_dim_label {
                            cl.last_mouse = sgp;
                            // A typed length overrides the free end, so the snap no longer applies.
                            self.state.line_end_snap = if cl.user_edited {
                                None
                            } else {
                                let (au, av) = world_to_local(&frame, cl.origin);
                                let (bu, bv) = world_to_local(&frame, sgp);
                                let du = bu - au;
                                let dv = bv - av;
                                cl.text = format_live_dimension((du * du + dv * dv).sqrt());
                                snap_target
                            };
                        }
                    }
                    if commit_click {
                        self.state.apply(Action::CommitLine);
                    }
                }
            }
        }

        if self.state.tool == Tool::Extrude {
            self.handle_extrude_tool(ui, &project, pointer_screen);
            self.show_extrude_distance_input(ui, &project);
        }

        if self.state.tool == Tool::Dimension {
            if let (Some(session), Some(pp)) =
                (self.state.sketch_session, pointer_screen)
            {
                if let Some(gp) =
                    sketch_plane_point(&cam, viewport, &vp, &self.state.doc, session, pp)
                {
                    let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
                    if self.state.editing_committed_dim.is_none()
                        && primary_pressed
                        && !over_committed_dim_label
                    {
                        if let Some(target) =
                            resolve_pick_target(pp, &project, Some(gp), &self.state.doc)
                        {
                            if let Some(distance_target) = distance_target_from_pick(
                                &self.state.doc,
                                session.sketch,
                                &target.kind,
                            ) {
                                self.state.apply(Action::BeginDimensionEdit {
                                    target: model::DimensionTarget::Distance(distance_target),
                                });
                            }
                        }
                    } else if self.state.editing_committed_dim.is_none() && !suppress_hover_highlight {
                        if let Some(target) =
                            resolve_pick_target(pp, &project, Some(gp), &self.state.doc)
                        {
                            if distance_target_from_pick(
                                &self.state.doc,
                                session.sketch,
                                &target.kind,
                            )
                            .is_some()
                            {
                                target.draw_highlight(&painter, &project, &self.state.doc);
                            }
                        }
                    }
                }
            }
        }

        if self.state.tool == Tool::ConstructionPlane {
            let ground = |p: egui::Pos2| cam.ground_point(p, viewport, &vp);

            if let Some(pp) = pointer_screen {
                let gp = ground(pp);
                let was_creating = self.state.creating_plane.is_some();
                let primary_pressed = ui.input(|i| i.pointer.primary_pressed());

                if !was_creating && primary_pressed {
                    if let Some(target) =
                        resolve_pick_target(pp, &project, gp, &self.state.doc)
                    {
                        let parent = parent_from_pick_target(&self.state.doc, target.kind);
                        self.state.apply(Action::BeginConstructionPlane {
                            reference: target.reference,
                            parent,
                        });
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

        let doc = &self.state.doc;
        let editing_constraint = self.state.editing_committed_dim.as_ref().and_then(|edit| {
            match &edit.target {
                DimEditTarget::Constraint(id) => Some(*id),
                DimEditTarget::New(_) => None,
            }
        });
        let gpu_dim_labels = if self.gpu_viewport {
            committed_dim_layouts
                .as_ref()
                .zip(planar_label_view)
                .map(|(layouts, view)| {
                    build_gpu_dimension_labels(
                        ui.ctx(),
                        layouts,
                        &view,
                        &cam,
                        viewport,
                        &vp,
                        &project,
                        editing_constraint,
                        &self.state.document_health,
                    )
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let plane_gizmo = self.state.creating_plane.as_ref().map(|cp| {
            gpu_viewport::ViewportPlaneGizmo {
                reference: cp.reference.clone(),
                offset: cp.offset_live,
                angle_deg: cp.axis_angle_deg,
                color: col::PREVIEW,
                hover: plane_gizmo_hover(cp, pointer_screen, &project),
            }
        });
        let mut hover_highlight = resolve_viewport_hover_highlight(
            suppress_hover_highlight,
            self.state.tool,
            sketch_session,
            self.state.creating_plane.is_some(),
            self.state.editing_committed_dim.is_some(),
            over_committed_dim_label,
            self.dim_label_drag.is_some(),
            pointer_screen,
            &cam,
            viewport,
            &vp,
            doc,
            &project,
        );
        // Extrude tool: highlight the face under the cursor and render the normal gizmo (same
        // arrow as the construction-plane offset gizmo) through the GPU scene.
        let mut extrude_gizmo = None;
        if self.state.tool == Tool::Extrude {
            if self.extrude_gizmo_drag.is_none() {
                hover_highlight = pointer_screen
                    .and_then(|pp| pick_extrude_face(pp, &project, doc, cam.eye()))
                    .map(|f| gpu_viewport::ViewportHoverHighlight::SketchFace(extrude_face_id(f)));
            }
            if let Some(ce) = self.state.creating_extrusion.as_ref() {
                if let Some((origin, normal)) = extrude::faces_anchor(doc, &ce.faces) {
                    let handle_offset =
                        extrude_gizmo_display_offset(ce.evaluated_distance(doc));
                    let hovered = self.extrude_gizmo_drag.is_some()
                        || pointer_screen.is_some_and(|pp| {
                            construction::offset_gizmo_hit(pp, &project, origin, normal, handle_offset)
                        });
                    extrude_gizmo = Some(gpu_viewport::ViewportExtrudeGizmo {
                        origin,
                        normal,
                        offset: handle_offset,
                        color: col::PREVIEW,
                        hovered,
                    });
                }
            }
        }
        let scene_input = build_viewport_scene_input(
            doc,
            &cam,
            viewport,
            sketch_session,
            &self.state.element_visibility,
            &self.state.scene_selection,
            &self.state.document_health,
            self.state.creating_rect.as_ref(),
            self.state.creating_line.as_ref(),
            self.state.creating_circle.as_ref(),
            self.state.creating_plane.as_ref(),
            self.state.creating_extrusion.as_ref(),
            plane_gizmo,
            extrude_gizmo,
            hover_highlight,
            &gpu_dim_labels,
            planar_label_view,
            Some(&constraint_graphics),
        );
        let scene = gpu_viewport::ViewportScene::build(&scene_input);
        let gpu_drawn =
            self.gpu_viewport && gpu_viewport::paint(render_state, &painter, viewport, scene);

        if !gpu_drawn {
            painter.rect_filled(viewport, 0.0, col::BG);
            draw_ground(
                &painter,
                &project,
                viewport,
                sketch_session.is_some(),
            );

            let visibility = &self.state.element_visibility;
            let health = &self.state.document_health;
            for (ri, r) in doc.rects.iter().enumerate() {
                if !rect_alive(doc, ri)
                    || !visibility.effective_visible(doc, SceneElement::Rect(ri))
                {
                    continue;
                }
                let dim = sketch_session
                    .is_some_and(|s| !sketch_rect_is_active(doc, s, ri, r.sketch));
                let element_health = health.element_status(SceneElement::Rect(ri));
                draw_rect_edges(&painter, &project, doc, r, dim, element_health);
            }
            for (li, line) in doc.lines.iter().enumerate() {
                if !line_alive(doc, li)
                    || !visibility.effective_visible(doc, SceneElement::Line(li))
                    || self.state.scene_selection.is_selected(SceneElement::Line(li))
                {
                    continue;
                }
                let dim = sketch_session.is_some_and(|s| line.sketch != s.sketch);
                let base = if line.construction {
                    sketch_color(col::CONSTRUCTION, dim)
                } else {
                    sketch_color(col::LINE_STROKE, dim)
                };
                let color = health_tint_color(base, health.element_status(SceneElement::Line(li)));
                if line.construction {
                    draw_construction_line_segment(&painter, &project, doc, line, color, 2.0);
                } else {
                    draw_line_segment(&painter, &project, doc, line, color, 2.0);
                }
            }
            for (ci, circle) in doc.circles.iter().enumerate() {
                if !circle_alive(doc, ci)
                    || !visibility.effective_visible(doc, SceneElement::Circle(ci))
                {
                    continue;
                }
                let dim = sketch_session
                    .is_some_and(|s| !sketch_circle_is_active(doc, s, ci, circle.sketch));
                let element_health = health.element_status(SceneElement::Circle(ci));
                draw_circle_edges(&painter, &project, doc, circle, dim, element_health);
            }
            for (i, plane) in doc.construction_planes.iter().enumerate() {
                if plane.deleted
                    || !visibility.effective_visible(doc, SceneElement::ConstructionPlane(i))
                {
                    continue;
                }
                let session_face =
                    sketch_session.and_then(|s| doc.sketch_face(s.sketch));
                let active = session_face == Some(FaceId::ConstructionPlane(i));
                let color = if active {
                    col::DIM_EDGE_HIGHLIGHT
                } else {
                    sketch_color(col::CONSTRUCTION, sketch_session.is_some())
                };
                draw_construction_plane(&painter, &project, plane, color, true);
            }
            draw_scene_selection_highlights(
                &painter,
                &project,
                doc,
                health,
                &self.state.scene_selection,
            );
            if let Some(session) = sketch_session {
                if let Some(face) = doc.sketch_face(session.sketch) {
                    if !matches!(face, FaceId::ConstructionPlane(_)) {
                        draw_face_highlight(
                            &painter,
                            &project,
                            doc,
                            face,
                            col::DIM_EDGE_HIGHLIGHT,
                        );
                    }
                }
            }
        }

        if !constraint_graphics.is_empty() {
            if !gpu_drawn {
                draw_constraint_connectors(
                    &painter,
                    &project,
                    &self.state.document_health,
                    &self.state.scene_selection,
                    &constraint_graphics,
                    col::DIM_EDGE_HIGHLIGHT,
                );
            }
            draw_constraint_icons(
                &painter,
                ui.ctx(),
                &project,
                &self.state.document_health,
                &self.state.scene_selection,
                &constraint_graphics,
                pointer_screen.and_then(|pp| {
                    pointer_over_constraint_icon(&constraint_icon_hits, pp)
                }),
                col::DIM_ANNOTATION,
                col::DIM_EDGE_HIGHLIGHT,
            );
        }

        if self.state.tool == Tool::Extrude {
            if let Some(ce) = self.state.creating_extrusion.as_ref() {
                draw_extrude_height_dimension(&painter, &project, doc, ce);
            }
            // Highlight the object the extrusion is currently snapping to.
            if let Some(target) = self.pending_extrude_target {
                draw_extrude_target_highlight(
                    &painter,
                    &project,
                    doc,
                    target,
                    col::DIM_EDGE_HIGHLIGHT,
                );
            }
        }

        if sketch_session.is_some() {
            let mut commit_committed_dim = false;
            if let (Some(layouts), Some(view)) = (&committed_dim_layouts, planar_label_view) {
                let hovered_angle_gizmo = pointer_screen
                    .and_then(|pp| {
                        angle_gizmo_hit_target(
                            layouts,
                            pp,
                            &project,
                            angle_gizmo_constraint,
                            viewport,
                        )
                    })
                    .or(self.angle_gizmo_drag.map(|d| d.constraint_id));
                if !gpu_drawn {
                    draw_committed_dim_layouts(
                        &painter,
                        layouts,
                        &view,
                        &project,
                        &self.state.document_health,
                        angle_gizmo_constraint,
                        hovered_angle_gizmo,
                        viewport,
                    );
                } else {
                    let arc_layouts: Vec<_> = layouts
                        .iter()
                        .filter(|layout| layout.arc_geom.is_some())
                        .cloned()
                        .collect();
                    if !arc_layouts.is_empty() {
                        draw_committed_dim_layouts(
                            &painter,
                            &arc_layouts,
                            &view,
                            &project,
                            &self.state.document_health,
                            angle_gizmo_constraint,
                            hovered_angle_gizmo,
                            viewport,
                        );
                    }
                }
                if let Some(edit) = &mut self.state.editing_committed_dim {
                    let is_angle = edit.target.is_angle(&self.state.doc);
                    let constraint_id = match &edit.target {
                        DimEditTarget::Constraint(id) => Some(*id),
                        DimEditTarget::New(_) => None,
                    };
                    let input_layout = if let Some(id) = constraint_id {
                        layouts
                            .iter()
                            .find(|l| l.target == id)
                            .map(|layout| {
                                dim_input_layout_centered_on(layout.label_rect, &edit.text)
                            })
                    } else if let Some(target) = edit.target.distance_target(&self.state.doc) {
                        distance_target_segment_endpoints(&self.state.doc, target).and_then(
                            |(a, b)| {
                                project(a).zip(project(b)).map(|(pa, pb)| {
                                    line_dim_layout(pa, pb, &edit.text)
                                })
                            },
                        )
                    } else if let Some(model::DimensionTarget::Angle {
                        line_a,
                        line_b,
                        rotation_sign: _,
                    }) = edit.target.dimension_target(&self.state.doc)
                    {
                        // Place the input inside the angle (on the bisector), not on the vertex
                        // where it would overlap both lines.
                        sketch_session
                            .and_then(|s| sketch_geometry_frame(&self.state.doc, s.sketch))
                            .zip(angle_constraint_display(&self.state.doc, line_a, line_b))
                            .and_then(|(frame, display)| {
                                let radius_world = pixels_to_world_distance(
                                    &project,
                                    display.center,
                                    display.dir_a,
                                    effective_arc_dim_offset(None),
                                );
                                let label_outset_world = pixels_to_world_distance(
                                    &project,
                                    display.center,
                                    display.dir_a,
                                    LABEL_OUTSET,
                                );
                                arc_dimension_world_geom(
                                    display.center,
                                    display.dir_a,
                                    display.dir_b,
                                    frame.normal,
                                    radius_world,
                                    label_outset_world,
                                )
                                .and_then(|wg| project(wg.label_center))
                                .map(|pc| {
                                    dim_input_layout_centered_on(
                                        egui::Rect::from_center_size(
                                            pc,
                                            dim_input_size_for_text(&edit.text),
                                        ),
                                        &edit.text,
                                    )
                                })
                            })
                    } else {
                        None
                    };
                    if let Some(input_layout) = input_layout {
                        let ctx = ui.ctx();
                        let id = egui::Id::new(("committed_dim", format!("{:?}", edit.target)));
                        let mut commit_dim = false;
                        let mut dim_field_result = SketchDimFieldResult::default();
                        let doc = &mut self.state.doc;
                        egui::Area::new(egui::Id::new((
                            "committed_dim_area",
                            format!("{:?}", edit.target),
                        )))
                        .fixed_pos(input_layout.pos)
                        .order(egui::Order::Foreground)
                        .show(ctx, |ui| {
                            dim_field_result = show_sketch_dimension_field(
                                ui,
                                ctx,
                                id,
                                &mut edit.text,
                                doc,
                                true,
                                &mut edit.pending_focus,
                                true,
                                is_angle,
                            );
                            commit_dim = dim_field_result.enter_commit;
                        });
                        inline_parameter_field_results.push(dim_field_result);
                        let dim_focused = ctx.memory(|m| m.focused()) == Some(id);
                        if edit.pending_focus {
                            ctx.memory_mut(|m| m.request_focus(id));
                        }
                        commit_committed_dim = should_commit_sketch_on_enter(
                            commit_dim,
                            dim_focused,
                            sketch_dimension_enter_pressed(ui),
                        );
                        if commit_committed_dim && !commit_dim {
                            consume_sketch_dimension_enter(ui);
                        }
                    }
                    if let Some(target) = edit.target.distance_target(&self.state.doc) {
                        if let Some((a, b)) =
                            distance_target_segment_endpoints(&self.state.doc, target)
                        {
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
                    if is_angle && matches!(&edit.target, DimEditTarget::New(_)) {
                        if let Some(frame) = sketch_session
                            .and_then(|s| sketch_geometry_frame(&self.state.doc, s.sketch))
                        {
                            if let Some(model::DimensionTarget::Angle {
                                line_a,
                                line_b,
                                rotation_sign: _,
                            }) = edit.target.dimension_target(&self.state.doc)
                            {
                                draw_angle_dim_for_lines(
                                    &painter,
                                    &project,
                                    &frame,
                                    &self.state.doc,
                                    line_a,
                                    line_b,
                                    None,
                                    &edit.text,
                                    true,
                                    false,
                                );
                            }
                        }
                    }
                }
            }
            if commit_committed_dim {
                self.state.apply(Action::CommitCommittedDim);
            }
        } else {
            self.dim_label_drag = None;
            self.state.editing_committed_dim = None;
        }
        if let (Some(cr), Some(session)) =
            (&self.state.creating_rect, self.state.sketch_session)
        {
            if let Some(frame) = sketch_geometry_frame(&self.state.doc, session.sketch) {
                if !gpu_drawn {
                    let end = cr.end_point(&frame, &self.state.doc);
                    let (ou, ov) = world_to_local(&frame, cr.origin);
                    let (eu, ev) = world_to_local(&frame, end);
                    let mut preview = Rect::from_local_corners(session.sketch, ou, ov, eu, ev);
                    if cr.construction {
                        for edge_index in 0..4 {
                            preview.set_edge_construction(RectEdge::from_index(edge_index), true);
                        }
                        draw_rect_edges(
                            &painter,
                            &project,
                            &self.state.doc,
                            &preview,
                            false,
                            HealthStatus::Healthy,
                        );
                    } else {
                        draw_rect(&painter, &project, &self.state.doc, &preview, col::PREVIEW, false);
                    }
                }
                let anchor_color = if cr.construction {
                    col::CONSTRUCTION
                } else {
                    col::PREVIEW
                };
                if let Some(sp) = project(cr.origin) {
                    painter.circle_filled(sp, 3.5, anchor_color);
                }
            }
        }
        if let (Some(cl), Some(session)) =
            (&self.state.creating_line, self.state.sketch_session)
        {
            if let Some(frame) = sketch_geometry_frame(&self.state.doc, session.sketch) {
                if !gpu_drawn {
                    let end = cl.end_point(&frame, &self.state.doc);
                    let (u0, v0) = world_to_local(&frame, cl.origin);
                    let (u1, v1) = world_to_local(&frame, end);
                    let preview =
                        Line::from_local_endpoints(session.sketch, u0, v0, u1, v1);
                    if cl.construction {
                        draw_construction_line_segment(
                            &painter,
                            &project,
                            &self.state.doc,
                            &preview,
                            col::CONSTRUCTION,
                            2.0,
                        );
                    } else if let (Some(pa), Some(pb)) = (project(cl.origin), project(end)) {
                        painter.line_segment([pa, pb], egui::Stroke::new(2.0, col::PREVIEW));
                    }
                }
                let anchor_color = if cl.construction {
                    col::CONSTRUCTION
                } else {
                    col::PREVIEW
                };
                if let Some(sp) = project(cl.origin) {
                    painter.circle_filled(sp, 3.5, anchor_color);
                }
            }
        }
        if let (Some(cc), Some(session)) =
            (&self.state.creating_circle, self.state.sketch_session)
        {
            if let Some(frame) = sketch_geometry_frame(&self.state.doc, session.sketch) {
                if !gpu_drawn {
                    let (cu, cv) = world_to_local(&frame, cc.origin);
                    let r = cc.radius(&frame, &self.state.doc);
                    let angle = cc.diameter_dim_angle(&frame);
                    let preview = Circle::from_local_center_radius(
                        session.sketch,
                        cu,
                        cv,
                        r,
                        angle,
                    );
                    if cc.construction {
                        draw_circle_edges(
                            &painter,
                            &project,
                            &self.state.doc,
                            &preview,
                            false,
                            HealthStatus::Healthy,
                        );
                    } else {
                        draw_circle(
                            &painter,
                            &project,
                            &self.state.doc,
                            &preview,
                            col::PREVIEW,
                            false,
                            1.5,
                        );
                    }
                }
                let anchor_color = if cc.construction {
                    col::CONSTRUCTION
                } else {
                    col::PREVIEW
                };
                if let Some(sp) = project(cc.origin) {
                    painter.circle_filled(sp, 3.5, anchor_color);
                }
            }
        }
        if let Some(cp) = &self.state.creating_plane {
            if !gpu_drawn {
                let preview = cp.preview_plane();
                draw_construction_plane(&painter, &project, &preview, col::PREVIEW, false);
                if let Some(edit_index) = cp.edit_index {
                    if let Some(dependent) =
                        preview_plane_edit_dependents(&self.state.doc, edit_index, &preview)
                    {
                        for (_, plane) in &dependent.planes {
                            draw_construction_plane(
                                &painter,
                                &project,
                                plane,
                                col::PREVIEW,
                                false,
                            );
                        }
                        for corners in &dependent.rects {
                            draw_world_quad(&painter, &project, *corners, col::PREVIEW, false);
                        }
                        for &(a, b) in &dependent.lines {
                            draw_world_segment(&painter, &project, a, b, col::PREVIEW, 2.0);
                        }
                    }
                }
            }
            if !gpu_drawn {
                let gizmo_hover = plane_gizmo_hover(cp, pointer_screen, &project);
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
        }

        if !gpu_drawn
            && self.state.tool == Tool::ConstructionPlane
            && self.state.creating_plane.is_none()
            && !suppress_hover_highlight
        {
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
            let frame = sketch_geometry_frame(&self.state.doc, session.sketch).unwrap();
            let end = cr.end_point(&frame, &self.state.doc);
            let (ou, ov) = world_to_local(&frame, cr.origin);
            let (eu, ev) = world_to_local(&frame, end);
            let preview = Rect::from_local_corners(session.sketch, ou, ov, eu, ev);
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

                let mut commit_rect = false;
                let mut width_field_result = SketchDimFieldResult::default();
                let mut height_field_result = SketchDimFieldResult::default();
                let doc = &mut self.state.doc;
                egui::Area::new(egui::Id::new("cr_width_area"))
                    .fixed_pos(width_layout.pos)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        width_field_result = show_sketch_dimension_field(
                            ui,
                            ctx,
                            id_w,
                            &mut cr.texts[0],
                            doc,
                            cr.focused == 0,
                            &mut cr.pending_focus,
                            cr.user_edited[0],
                            false,
                        );
                        if width_field_result.changed {
                            cr.user_edited[0] = true;
                        }
                        if width_field_result.enter_commit {
                            commit_rect = true;
                        }
                    });
                inline_parameter_field_results.push(width_field_result);

                let doc = &mut self.state.doc;
                egui::Area::new(egui::Id::new("cr_height_area"))
                    .fixed_pos(height_layout.pos)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        height_field_result = show_sketch_dimension_field(
                            ui,
                            ctx,
                            id_h,
                            &mut cr.texts[1],
                            doc,
                            cr.focused == 1,
                            &mut cr.pending_focus,
                            cr.user_edited[1],
                            false,
                        );
                        if height_field_result.changed {
                            cr.user_edited[1] = true;
                        }
                        if height_field_result.enter_commit {
                            commit_rect = true;
                        }
                    });
                inline_parameter_field_results.push(height_field_result);

                let current = ctx.memory(|m| m.focused());
                if current == Some(id_w) {
                    cr.focused = 0;
                } else if current == Some(id_h) {
                    cr.focused = 1;
                } else if cr.pending_focus {
                    let target_id = if cr.focused == 0 { id_w } else { id_h };
                    ctx.memory_mut(|m| m.request_focus(target_id));
                }

                let dim_field_focused =
                    current == Some(id_w) || current == Some(id_h);
                if should_commit_sketch_on_enter(
                    commit_rect,
                    dim_field_focused,
                    sketch_dimension_enter_pressed(ui),
                ) {
                    if !commit_rect {
                        consume_sketch_dimension_enter(ui);
                    }
                    self.state.apply(Action::CommitRectangle);
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
            let frame = sketch_geometry_frame(&self.state.doc, session.sketch).unwrap();
            let end = cl.end_point(&frame, &self.state.doc);
            if let (Some(pa), Some(pb)) = (project(cl.origin), project(end)) {
                let layout = line_dim_layout(pa, pb, &cl.text);
                let id_len = egui::Id::new("cl_length");

                let mut commit_line = false;
                let mut line_field_result = SketchDimFieldResult::default();
                {
                    let ctx = ui.ctx();
                    let doc = &mut self.state.doc;
                    egui::Area::new(egui::Id::new("cl_length_area"))
                        .fixed_pos(layout.pos)
                        .order(egui::Order::Foreground)
                        .show(ctx, |ui| {
                            line_field_result = show_sketch_dimension_field(
                                ui,
                                ctx,
                                id_len,
                                &mut cl.text,
                                doc,
                                true,
                                &mut cl.pending_focus,
                                cl.user_edited,
                                false,
                            );
                            if line_field_result.changed {
                                cl.user_edited = true;
                            }
                            commit_line = line_field_result.enter_commit;
                        });
                }
                inline_parameter_field_results.push(line_field_result);

                let length_focused = {
                    let ctx = ui.ctx();
                    let focused = ctx.memory(|m| m.focused()) == Some(id_len);
                    if !focused && cl.pending_focus {
                        ctx.memory_mut(|m| m.request_focus(id_len));
                    }
                    focused
                };
                let commit_line_now = should_commit_sketch_on_enter(
                    commit_line,
                    length_focused,
                    sketch_dimension_enter_pressed(ui),
                );
                if commit_line_now {
                    if !commit_line {
                        consume_sketch_dimension_enter(ui);
                    }
                    self.state.apply(Action::CommitLine);
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

        if let (Some(cc), Some(session)) =
            (&mut self.state.creating_circle, self.state.sketch_session)
        {
            let frame = sketch_geometry_frame(&self.state.doc, session.sketch).unwrap();
            let (cu, cv) = world_to_local(&frame, cc.origin);
            let preview = Circle::from_local_center_radius(
                session.sketch,
                cu,
                cv,
                cc.radius(&frame, &self.state.doc),
                cc.diameter_dim_angle(&frame),
            );
            if let Some((a, b)) = circle_world_diameter_endpoints(&self.state.doc, &preview) {
                if let (Some(pa), Some(pb)) = (project(a), project(b)) {
                    let layout = line_dim_layout(pa, pb, &cc.text);
                    let id_diam = egui::Id::new("cc_diameter");

                    let mut commit_circle = false;
                    let mut circle_field_result = SketchDimFieldResult::default();
                    {
                        let ctx = ui.ctx();
                        let doc = &mut self.state.doc;
                        egui::Area::new(egui::Id::new("cc_diameter_area"))
                            .fixed_pos(layout.pos)
                            .order(egui::Order::Foreground)
                            .show(ctx, |ui| {
                                circle_field_result = show_sketch_dimension_field(
                                    ui,
                                    ctx,
                                    id_diam,
                                    &mut cc.text,
                                    doc,
                                    true,
                                    &mut cc.pending_focus,
                                    cc.user_edited,
                                    false,
                                );
                                if circle_field_result.changed {
                                    cc.user_edited = true;
                                }
                                commit_circle = circle_field_result.enter_commit;
                            });
                    }
                    inline_parameter_field_results.push(circle_field_result);

                    let diameter_focused = {
                        let ctx = ui.ctx();
                        let focused = ctx.memory(|m| m.focused()) == Some(id_diam);
                        if !focused && cc.pending_focus {
                            ctx.memory_mut(|m| m.request_focus(id_diam));
                        }
                        focused
                    };
                    let commit_circle_now = should_commit_sketch_on_enter(
                        commit_circle,
                        diameter_focused,
                        sketch_dimension_enter_pressed(ui),
                    );
                    if commit_circle_now {
                        if !commit_circle {
                            consume_sketch_dimension_enter(ui);
                        }
                        self.state.apply(Action::CommitCircle);
                    } else if diameter_focused {
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

                let mut commit_plane = false;
                let mut offset_field_result = SketchDimFieldResult::default();
                let doc = &mut self.state.doc;
                egui::Area::new(egui::Id::new("cp_offset_area"))
                    .fixed_pos(offset_layout.pos)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        offset_field_result = show_sketch_dimension_field(
                            ui,
                            ctx,
                            id_offset,
                            &mut cp.offset_text,
                            doc,
                            cp.focused == PlaneDim::Offset,
                            &mut cp.pending_focus,
                            cp.user_edited_offset,
                            false,
                        );
                        if offset_field_result.changed {
                            cp.user_edited_offset = true;
                        }
                        if offset_field_result.enter_commit {
                            commit_plane = true;
                        }
                    });
                inline_parameter_field_results.push(offset_field_result);

                if let Some(angle_layout) = angle_layout {
                    let doc = &mut self.state.doc;
                    let mut angle_field_result = SketchDimFieldResult::default();
                    egui::Area::new(egui::Id::new("cp_angle_area"))
                        .fixed_pos(angle_layout.pos)
                        .order(egui::Order::Foreground)
                        .show(ctx, |ui| {
                            angle_field_result = show_sketch_dimension_field(
                                ui,
                                ctx,
                                id_angle,
                                &mut cp.angle_text,
                                doc,
                                cp.focused == PlaneDim::Angle,
                                &mut cp.pending_focus,
                                cp.user_edited_angle,
                                true,
                            );
                            if angle_field_result.changed {
                                cp.user_edited_angle = true;
                            }
                            if angle_field_result.enter_commit {
                                commit_plane = true;
                            }
                        });
                    inline_parameter_field_results.push(angle_field_result);
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

                let dim_field_focused =
                    current == Some(id_offset) || current == Some(id_angle);
                if should_commit_sketch_on_enter(
                    commit_plane,
                    dim_field_focused,
                    sketch_dimension_enter_pressed(ui),
                ) {
                    if !commit_plane {
                        consume_sketch_dimension_enter(ui);
                    }
                    self.state.apply(Action::CommitConstructionPlane);
                }

                if !gpu_drawn {
                    draw_construction_plane(
                        &painter,
                        &project,
                        &preview,
                        col::DIM_EDGE_HIGHLIGHT,
                        false,
                    );
                }
            }
        }

        let shift_held = ui.input(|i| i.modifiers.shift);
        if camera::Camera::shows_camera_pivot(
            response.dragged_by(egui::PointerButton::Secondary),
            shift_held,
        ) {
            draw_orbit_pivot_indicator(&painter, &project, cam.target);
        }

        if matches!(self.state.tool, Tool::Select | Tool::Constraint) {
            let mut create_parameter_from_line = None;
            crate::parameters::show_computed_line_length_context_menu(
                &response,
                &self.state.doc,
                &self.state.scene_selection,
                &mut |line_index| create_parameter_from_line = Some(line_index),
            );
            if let Some(line_index) = create_parameter_from_line {
                self.state.apply(Action::CreateParameterFromLineLength {
                    line_index,
                    name: None,
                });
            }
        }

        // Snap indicator: a ring where a dragged/drawn point has latched onto geometry, or
        // where the first point of a line would land if clicked now.
        if let Some(session) = self.state.sketch_session {
            if let Some(frame) = sketch_geometry_frame(&self.state.doc, session.sketch) {
                let snap = active_snap(&self.state, &frame).or_else(|| {
                    // Preview where the next click would place a point (the first point of a
                    // line/rectangle, or a circle center), before any geometry exists.
                    let drawing = matches!(
                        self.state.tool,
                        Tool::Line | Tool::Rectangle | Tool::Circle
                    );
                    let mid_op = self.state.creating_line.is_some()
                        || self.state.creating_rect.is_some()
                        || self.state.creating_circle.is_some();
                    if !drawing || mid_op || self.vertex_drag.is_some() || !self.state.snapping_enabled
                    {
                        return None;
                    }
                    let pp = pointer_screen?;
                    let gp =
                        sketch_plane_point(&cam, viewport, &vp, &self.state.doc, session, pp)?;
                    let (sgp, target) =
                        snap_ground_point(&self.state, session, &frame, &project, gp, &[]);
                    target.map(|t| (sgp, t))
                });
                if let Some((world, target)) = snap {
                    if let Some(sp) = project(world) {
                        let color = egui::Color32::from_rgb(120, 215, 230);
                        // Inference guide (#21): a dashed line from the anchor edge through the
                        // snapped point, showing the extension the point is aligned with.
                        if let snapping::SnapTarget::OnLineExtension(line) = target {
                            if let Ok(((x0, y0), (x1, y1))) =
                                geometric_constraints::line_uv_endpoints(&self.state.doc, line)
                            {
                                let (su, sv) = world_to_local(&frame, world);
                                let d0 = (x0 - su).hypot(y0 - sv);
                                let d1 = (x1 - su).hypot(y1 - sv);
                                let (au, av) = if d0 <= d1 { (x0, y0) } else { (x1, y1) };
                                let anchor_world = face::local_to_world(&frame, au, av);
                                if let Some(ap) = project(anchor_world) {
                                    painter.extend(egui::Shape::dashed_line(
                                        &[ap, sp],
                                        egui::Stroke::new(1.5, color),
                                        6.0,
                                        4.0,
                                    ));
                                }
                            }
                        }
                        painter.circle_stroke(sp, 7.0, egui::Stroke::new(2.0, color));
                        // Emphasize the actual vertex being snapped to.
                        if matches!(target, snapping::SnapTarget::Vertex(_)) {
                            painter.circle_filled(sp, 3.5, color);
                        }
                        // Show the constraint a click would add (coincident, midpoint, …).
                        let icon_rect = egui::Rect::from_min_size(
                            sp + egui::vec2(9.0, -19.0),
                            egui::vec2(16.0, 16.0),
                        );
                        icons::paint_icon(&painter, ui.ctx(), snap_icon(target), icon_rect, color);
                    }
                }
            }
        }

        // Hide the view-cube HUD while a viewport screenshot is being captured this frame.
        let suppress_hud_for_screenshot = self
            .script
            .as_ref()
            .is_some_and(|runner| runner.screenshot_suppresses_hud());
        if self.state.panes.is_visible(Pane::ViewCube) && !suppress_hud_for_screenshot {
            let command_log = self
                .state
                .command_log
                .as_ref()
                .map(|log| log.borrow_mut());
            view_cube::show_hud(
                ui.ctx(),
                &mut self.state.cam,
                viewport,
                render_state,
                self.gpu_view_cube,
                command_log,
            );
        }

        let hint = match self.state.tool {
            Tool::Select => {
                if self.state.editing_committed_dim.is_some() {
                    "Edit dimension • Enter to commit • Esc to cancel"
                } else if self.state.sketch_session.is_some() {
                    "Sketch mode — drag vertices • Shift+click or ⌘/Ctrl+click multi-select • double-click a dimension to edit • Esc: exit sketch"
                } else {
                    "Click to select • Shift+click or ⌘/Ctrl+click multi-select • Right-drag: orbit  •  Wheel: zoom  •  s: sketch  •  p: plane"
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
            Tool::Circle => {
                if self.state.creating_circle.is_some() {
                    "Move mouse (free diameter) • Type in diameter input to constrain • Click/Enter: create circle • Esc: cancel"
                } else if self.state.sketch_session.is_none() {
                    "o: circle  •  Click a face to sketch on  •  Right-drag: orbit  • Shift+right-drag: pan  •  Wheel: zoom"
                } else {
                    "o: circle  •  Left-click to set center • move to size • Right-drag: orbit  • Shift+right-drag: pan  •  Wheel: zoom"
                }
            }
            Tool::Constraint => {
                if self.state.sketch_session.is_none() {
                    "c: constraint  •  Open a sketch to add geometric constraints"
                } else {
                    "c: constraint  •  Shift+click or ⌘/Ctrl+click multi-select • A/E/I/M/V/H apply constraint • context pane shows options"
                }
            }
            Tool::Dimension => {
                if self.state.editing_committed_dim.is_some() {
                    "Edit dimension • Enter to commit • Esc to cancel"
                } else if self.state.sketch_session.is_none() {
                    "d: dimension  •  Open a sketch to add distance constraints"
                } else {
                    "d: dimension  •  Select geometry, press D, or click a segment • Enter commit"
                }
            }
            Tool::ConstructionPlane => {
                if self.state.creating_plane.is_some() {
                    let editing = self
                        .state
                        .creating_plane
                        .as_ref()
                        .and_then(|cp| cp.edit_index)
                        .is_some();
                    if self
                        .state
                        .creating_plane
                        .as_ref()
                        .is_some_and(|cp| cp.reference.is_axis())
                    {
                        if editing {
                            "Edit plane • drag arrow/circle or type to lock • Tab: switch dims • Click/Enter: commit • Esc: cancel"
                        } else {
                            "Drag arrow for offset • drag circle handle for angle • type to lock • Tab: switch dims • Click/Enter: commit • Esc: cancel"
                        }
                    } else if editing {
                        "Edit plane • drag arrow or type to lock offset • Click/Enter: commit • Esc: cancel"
                    } else {
                        "Drag arrow for offset • wheel or type to lock • Click/Enter: create plane • Esc: cancel"
                    }
                } else {
                    "p: plane  •  Click a face, line, shape edge, global axis, or ground • then set offset (and angle for lines)"
                }
            }
            Tool::Extrude => {
                if self.state.creating_extrusion.is_some() {
                    "e: extrude  •  Click faces to toggle • drag the arrow or type a distance • Enter: commit • Esc: cancel"
                } else {
                    "e: extrude  •  Click a coplanar face (rectangle/circle) to start an extrusion"
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

        for result in inline_parameter_field_results {
            apply_dimension_field_feedback(&mut self.state, &result);
        }
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

fn draw_world_segment_dashed(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    a: Vec3,
    b: Vec3,
    color: egui::Color32,
    width: f32,
) {
    if let (Some(pa), Some(pb)) = (project(a), project(b)) {
        painter.add(egui::Shape::dashed_line(
            &[pa, pb],
            egui::Stroke::new(width, color),
            construction::CONSTRUCTION_DASH_LENGTH_PX,
            construction::CONSTRUCTION_DASH_GAP_PX,
        ));
    }
}

const ORBIT_PIVOT_RADIUS: f32 = 4.0;
const ORBIT_PIVOT_GROUND_RADIUS: f32 = 2.0;

fn draw_orbit_pivot_indicator(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    target: Vec3,
) {
    if camera::orbit_pivot_has_ground_drop(target) {
        let foot = camera::orbit_pivot_ground_foot(target);
        draw_world_segment_dashed(
            painter,
            project,
            target,
            foot,
            col::ORBIT_PIVOT_DROP,
            1.0,
        );
        if let Some(foot_sp) = project(foot) {
            painter.circle_filled(foot_sp, ORBIT_PIVOT_GROUND_RADIUS, col::ORBIT_PIVOT);
        }
    }
    if let Some(sp) = project(target) {
        painter.circle_filled(sp, ORBIT_PIVOT_RADIUS, col::ORBIT_PIVOT);
    }
}

fn draw_construction_line_segment(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    line: &Line,
    color: egui::Color32,
    width: f32,
) {
    let Some((a, b)) = line_world_endpoints(doc, line) else {
        return;
    };
    draw_world_segment_dashed(painter, project, a, b, color, width);
}

fn circle_screen_perimeter(
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    circle: &Circle,
) -> Option<Vec<egui::Pos2>> {
    let pts = circle_world_perimeter(doc, circle, 64)?;
    pts.iter().map(|p| project(*p)).collect()
}

fn draw_circle(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    circle: &Circle,
    color: egui::Color32,
    fill: bool,
    width: f32,
) {
    let Some(screen_pts) = circle_screen_perimeter(project, doc, circle) else {
        return;
    };
    if screen_pts.len() < 2 {
        return;
    }
    if fill {
        painter.add(egui::Shape::convex_polygon(
            screen_pts.clone(),
            color.gamma_multiply(0.25),
            egui::Stroke::new(width, color),
        ));
    } else {
        painter.add(egui::Shape::closed_line(
            screen_pts,
            egui::Stroke::new(width, color),
        ));
    }
}

fn draw_construction_circle(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    circle: &Circle,
    color: egui::Color32,
    width: f32,
) {
    let Some(pts) = circle_world_perimeter(doc, circle, 64) else {
        return;
    };
    for window in pts.windows(2) {
        draw_world_segment_dashed(painter, project, window[0], window[1], color, width);
    }
}

fn draw_circle_edges(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    circle: &Circle,
    dim: bool,
    health: HealthStatus,
) {
    let solid_color = health_tint_color(sketch_color(col::RECT_LINE, dim), health);
    let construction_color = health_tint_color(sketch_color(col::CONSTRUCTION, dim), health);
    if circle.construction {
        if let Some(screen_pts) = circle_screen_perimeter(project, doc, circle) {
            painter.add(egui::Shape::convex_polygon(
                screen_pts,
                construction_color.gamma_multiply(0.18),
                egui::Stroke::NONE,
            ));
        }
        draw_construction_circle(painter, project, doc, circle, construction_color, 1.5);
    } else {
        draw_circle(painter, project, doc, circle, solid_color, true, 1.5);
    }
}

fn draw_rect_edges(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    r: &Rect,
    dim: bool,
    health: HealthStatus,
) {
    let Some(corners) = rect_world_corners(doc, r) else {
        return;
    };
    let solid_color = health_tint_color(sketch_color(col::RECT_LINE, dim), health);
    let construction_color = health_tint_color(sketch_color(col::CONSTRUCTION, dim), health);
    let all_construction = r.all_edges_construction();
    let has_solid_edge = r.construction_edges.iter().any(|&c| !c);

    if all_construction {
        let pts: Option<Vec<egui::Pos2>> = corners.iter().map(|&c| project(c)).collect();
        if let Some(pts) = pts {
            painter.add(egui::Shape::convex_polygon(
                pts.clone(),
                construction_color.gamma_multiply(0.18),
                egui::Stroke::NONE,
            ));
        }
    } else if has_solid_edge && r.has_mixed_edge_construction() {
        let pts: Option<Vec<egui::Pos2>> = corners.iter().map(|&c| project(c)).collect();
        if let Some(pts) = pts {
            painter.add(egui::Shape::convex_polygon(
                pts,
                solid_color.gamma_multiply(0.25),
                egui::Stroke::NONE,
            ));
        }
    } else if has_solid_edge {
        draw_world_quad(painter, project, corners, solid_color, true);
    }

    for (edge_index, (a, b)) in rect_edge_segments(doc, r).into_iter().enumerate() {
        let edge = RectEdge::from_index(edge_index);
        if r.edge_construction(edge) {
            draw_world_segment_dashed(painter, project, a, b, construction_color, 1.5);
        } else {
            draw_world_segment(painter, project, a, b, solid_color, 1.5);
        }
    }
}

fn draw_scene_selection_highlights(
    painter: &egui::Painter,
    project: &impl Fn(Vec3) -> Option<egui::Pos2>,
    doc: &model::Document,
    health: &DocumentHealth,
    selection: &crate::selection::SceneSelection,
) {
    if selection.is_empty() {
        return;
    }
    let base_color = col::DIM_EDGE_HIGHLIGHT;
    let width = 3.0;
    for element in selection.iter() {
        let color = health_tint_color(base_color, health.element_status(element));
        let dashed = context::selection_highlight_dashed(doc, element) == Some(true);
        match element {
            SceneElement::Line(index) => {
                if !line_alive(doc, index) {
                    continue;
                }
                if let Some(line) = doc.lines.get(index) {
                    if dashed {
                        draw_construction_line_segment(painter, project, doc, line, color, width);
                    } else {
                        draw_line_segment(painter, project, doc, line, color, width);
                    }
                }
            }
            SceneElement::RectEdge(index, edge) => {
                if !rect_alive(doc, index) {
                    continue;
                }
                if let Some(rect) = doc.rects.get(index) {
                    let segments = rect_edge_segments(doc, rect);
                    let (a, b) = segments[edge.index()];
                    if dashed {
                        draw_world_segment_dashed(painter, project, a, b, color, width);
                    } else {
                        draw_world_segment(painter, project, a, b, color, width);
                    }
                }
            }
            SceneElement::Rect(index) => {
                if !rect_alive(doc, index) {
                    continue;
                }
                if let Some(rect) = doc.rects.get(index) {
                    for (edge_index, (a, b)) in
                        rect_edge_segments(doc, rect).into_iter().enumerate()
                    {
                        let edge = RectEdge::from_index(edge_index);
                        let stroke = if rect.edge_construction(edge) {
                            color.gamma_multiply(0.85)
                        } else {
                            color
                        };
                        if dashed && rect.edge_construction(edge) {
                            draw_world_segment_dashed(painter, project, a, b, stroke, width);
                        } else {
                            draw_world_segment(painter, project, a, b, stroke, width);
                        }
                    }
                }
            }
            SceneElement::Circle(index) => {
                if !circle_alive(doc, index) {
                    continue;
                }
                if let Some(circle) = doc.circles.get(index) {
                    if dashed {
                        draw_construction_circle(painter, project, doc, circle, color, width);
                    } else {
                        draw_circle(painter, project, doc, circle, color, false, width);
                    }
                }
            }
            SceneElement::Constraint(index) => {
                if !constraint_alive(doc, index) {
                    continue;
                }
                if let Some((a, b)) = constraint_segment_endpoints(doc, index) {
                    draw_world_segment(painter, project, a, b, color, width);
                }
            }
            SceneElement::Point(point) => {
                if let Some(world) = point_world_position(doc, point) {
                    if let Some(screen) = project(world) {
                        painter.circle_filled(screen, 6.0, color);
                    }
                }
            }
            _ => {}
        }
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
    line: &Line,
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
        let plane_color = if color == col::CONSTRUCTION {
            construction::PLANE_FILL_RGBA
        } else {
            color
        };
        painter.add(egui::Shape::convex_polygon(
            pts,
            gpu_viewport::fill_color(
                plane_color,
                gpu_viewport::DEFAULT_CONSTRUCTION_PLANE_OPACITY,
            ),
            egui::Stroke::NONE,
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
    r: &Rect,
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

fn sketch_rect_is_active(
    doc: &model::Document,
    session: SketchSession,
    rect_index: usize,
    rect_sketch: SketchId,
) -> bool {
    if rect_sketch == session.sketch {
        return true;
    }
    if let Some(FaceId::Rect(face_index)) = doc.sketch_face(session.sketch) {
        return rect_index == face_index;
    }
    false
}

fn sketch_circle_is_active(
    doc: &model::Document,
    session: SketchSession,
    circle_index: usize,
    circle_sketch: SketchId,
) -> bool {
    if circle_sketch == session.sketch {
        return true;
    }
    if let Some(FaceId::Circle(face_index)) = doc.sketch_face(session.sketch) {
        return circle_index == face_index;
    }
    false
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
        let color = gpu_viewport::sketch_ground_color(base, dim);
        line(Vec3::new(-e, t, 0.0), Vec3::new(e, t, 0.0), color, 1.0);
        line(Vec3::new(t, -e, 0.0), Vec3::new(t, e, 0.0), color, 1.0);
        t += GRID_STEP;
    }

    line(
        Vec3::ZERO,
        Vec3::new(e, 0.0, 0.0),
        gpu_viewport::sketch_ground_color(col::X_AXIS, dim),
        2.0,
    );
    line(
        Vec3::ZERO,
        Vec3::new(0.0, e, 0.0),
        gpu_viewport::sketch_ground_color(col::Y_AXIS, dim),
        2.0,
    );
    line(
        Vec3::ZERO,
        Vec3::new(0.0, 0.0, e),
        gpu_viewport::sketch_ground_color(col::Z_AXIS, dim),
        2.0,
    );
}

#[cfg(test)]
mod tests {
    use super::actions::CreatingRect;
    use super::{
        clip_segment_to_rect, col, initial_launch_maximize_frames, native_options,
        should_commit_sketch_on_click, should_select_all_rect_value, side_panel_resize_active,
        tick_launch_maximize, uses_deferred_launch_maximize, MACOS_LAUNCH_MAXIMIZE_DELAY_FRAMES,
        GRID_EXTENT, ORBIT_PIVOT_GROUND_RADIUS, ORBIT_PIVOT_RADIUS,
    };
    use crate::face::SketchFrame;
    use eframe::egui::{self, Pos2, Rect, Vec2};
    use egui::Color32;
    use glam::Vec3;

    #[test]
    fn circles_use_rectangle_stroke_color() {
        assert_ne!(col::RECT_LINE, col::LINE_STROKE);
        assert_eq!(col::RECT_LINE, Color32::from_rgb(120, 170, 240));
    }

    #[test]
    fn launch_maximize_strategy_matches_platform() {
        if uses_deferred_launch_maximize() {
            assert_eq!(native_options().viewport.maximized, None);
        } else {
            assert_eq!(native_options().viewport.maximized, Some(true));
        }
    }

    #[test]
    fn launch_maximize_waits_for_post_first_paint_on_macos() {
        if uses_deferred_launch_maximize() {
            assert_eq!(
                initial_launch_maximize_frames(),
                MACOS_LAUNCH_MAXIMIZE_DELAY_FRAMES
            );
        } else {
            assert_eq!(initial_launch_maximize_frames(), 0);
        }
    }

    #[test]
    fn tick_launch_maximize_counts_down_to_zero() {
        let ctx = egui::Context::default();
        let mut frames = 2;
        tick_launch_maximize(&mut frames, &ctx);
        assert_eq!(frames, 1);
        tick_launch_maximize(&mut frames, &ctx);
        assert_eq!(frames, 0);
        tick_launch_maximize(&mut frames, &ctx);
        assert_eq!(frames, 0);
    }

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
    fn orbit_pivot_ground_marker_is_smaller_than_pivot() {
        assert!(ORBIT_PIVOT_GROUND_RADIUS < ORBIT_PIVOT_RADIUS);
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
    fn keyboard_shortcuts_suppressed_while_text_input_focused() {
        use super::keyboard_shortcuts_suppressed;
        let ctx = egui::Context::default();
        assert!(!keyboard_shortcuts_suppressed(&ctx));
        ctx.memory_mut(|mem| mem.request_focus(egui::Id::new("test_text_input")));
        assert!(keyboard_shortcuts_suppressed(&ctx));
    }

    #[test]
    fn should_commit_sketch_on_enter_focused_field_or_unfocused_viewport() {
        use super::should_commit_sketch_on_enter;
        assert!(should_commit_sketch_on_enter(true, true, false));
        assert!(should_commit_sketch_on_enter(false, false, true));
        assert!(!should_commit_sketch_on_enter(false, true, true));
        assert!(!should_commit_sketch_on_enter(false, false, false));
    }

    #[test]
    fn next_rect_focus_axis_toggles_width_and_height() {
        use super::{next_rect_focus_axis, RectAxis};
        assert_eq!(next_rect_focus_axis(0), RectAxis::Height);
        assert_eq!(next_rect_focus_axis(1), RectAxis::Width);
    }

    #[test]
    fn next_plane_focus_dim_toggles_offset_and_angle() {
        use super::{next_plane_focus_dim, PlaneDim};
        assert_eq!(next_plane_focus_dim(PlaneDim::Offset), PlaneDim::Angle);
        assert_eq!(next_plane_focus_dim(PlaneDim::Angle), PlaneDim::Offset);
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
    fn resolve_viewport_hover_highlight_suppressed_returns_none() {
        use super::resolve_viewport_hover_highlight;
        let doc = crate::model::Document::default();
        let cam = crate::camera::Camera::default();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let vp = cam.view_proj(viewport);
        let project = |_: glam::Vec3| Some(egui::Pos2::ZERO);
        assert!(
            resolve_viewport_hover_highlight(
                true,
                crate::actions::Tool::Select,
                None,
                false,
                false,
                false,
                false,
                Some(egui::Pos2::ZERO),
                &cam,
                viewport,
                &vp,
                &doc,
                &project,
            )
            .is_none()
        );
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
            construction: false,
        }
    }

    #[test]
    fn end_point_free_follows_mouse() {
        let doc = crate::model::Document::default();
        let cr = make_cr((0., 0.), ["", ""], (10., 4.));
        let frame = xy_frame();
        let e = cr.end_point(&frame, &doc);
        assert!((e.x - 10.0).abs() < 1e-4);
        assert!((e.y - 4.0).abs() < 1e-4);
    }

    #[test]
    fn end_point_one_constrained() {
        let doc = crate::model::Document::default();
        let frame = xy_frame();
        let cr = make_cr((0., 0.), ["5", ""], (12., 3.));
        let e = cr.end_point(&frame, &doc);
        assert!((e.x - 5.0).abs() < 1e-4 && (e.y - 3.0).abs() < 1e-4);

        let cr2 = make_cr((10., 20.), ["5", ""], (3., 15.));
        let e2 = cr2.end_point(&frame, &doc);
        assert!((e2.x - 5.0).abs() < 1e-4);
        assert!((e2.y - 15.0).abs() < 1e-4);
    }

    #[test]
    fn end_point_both_constrained() {
        let doc = crate::model::Document::default();
        let frame = xy_frame();
        let cr = make_cr((0., 0.), ["3", "7"], (99., -4.));
        let e = cr.end_point(&frame, &doc);
        assert!((e.x - 3.0).abs() < 1e-4);
        assert!((e.y + 7.0).abs() < 1e-4);
    }

    #[test]
    fn end_point_invalid_text_falls_back_to_mouse() {
        let doc = crate::model::Document::default();
        let frame = xy_frame();
        let cr = make_cr((0., 0.), ["abc", "12x"], (8., 9.));
        let e = cr.end_point(&frame, &doc);
        assert!((e.x - 8.0).abs() < 1e-4);
        assert!((e.y - 9.0).abs() < 1e-4);
    }

    #[test]
    fn side_panel_resize_inactive_without_resize_drag() {
        egui::__run_test_ctx(|ctx| {
            assert!(!side_panel_resize_active(ctx));
        });
    }
}