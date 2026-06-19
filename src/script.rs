//! Lua script runner and internal instruction dispatch (SPEC §8).
//!
//! Scripts are `.lua` files that call the global `paramcad` API. They drive the
//! live UI via synthetic pointer/keyboard events and headless actions.

use crate::actions::{
    dim_label_target_in_sketch, Action, AppState, DimLabelAxis, Pane, RectAxis, Tool,
};
use crate::command_palette::{best_match, commands_for_state, PaletteOutcome};
use crate::constraints::add_distance_constraint;
use crate::hierarchy::SceneElement;
use crate::model::{ConstraintLine, ConstraintPoint, DistanceTarget, FaceId, RectEdge, SketchId};

use crate::construction::PlaneDim;
use crate::camera::{ProjectionMode, StandardView};
use crate::view_cube::{CubeCornerId, CubeEdgeId};

use crate::lua_script::{load_script, ScriptTickData};
use eframe::egui::{self, Key, Modifiers, PointerButton};
use glam::Vec3;
use mlua::Lua;
use std::path::Path;
use std::time::{Duration, Instant};

/// A single script instruction.
#[derive(Clone, Debug, PartialEq)]
pub enum Instruction {
    // Document / tool actions
    New,
    Open(String),
    Save(Option<String>),
    Clear,
    Undo,
    Tool(Tool),
    BeginSketch { face: FaceId },
    OpenSketch { sketch: SketchId },
    ExitSketch,
    SetElementVisible {
        element: SceneElement,
        visible: Option<bool>,
    },
    /// Click a tree row: replaces selection unless `additive` is true.
    SelectSceneElement {
        element: SceneElement,
        additive: bool,
    },
    ClearSceneSelection,
    SetShapeConstruction {
        element: SceneElement,
        construction: bool,
    },
    /// Set construction/substantial on draw op or all constructable selected targets.
    ApplyConstruction {
        construction: bool,
    },
    /// Toggle construction/substantial on draw op or each constructable selected target.
    ToggleConstruction,
    SetElementName {
        element: SceneElement,
        name: String,
    },
    FocusElementName,
    SetDim { axis: RectAxis, value: String },
    SetDimLabelOffset { axis: DimLabelAxis, offset: f32 },
    BeginEditCommittedDim { axis: DimLabelAxis },
    CommitCommittedDim,
    AddDistanceConstraint {
        target: DistanceTarget,
        expression: String,
    },
    AddGeometricConstraint(crate::geometric_constraints::GeometricConstraintType),
    ApplyConstraintShortcut(u8),
    DragVertex {
        point: ConstraintPoint,
        u: f32,
        v: f32,
    },
    DragLineSegment {
        target: crate::model::ConstraintLine,
        anchor_u: f32,
        anchor_v: f32,
        u: f32,
        v: f32,
    },
    SetLineLength { value: String },
    SetCircleDiameter { value: String },
    BeginEditConstructionPlane { index: usize },
    CommitConstructionPlane,
    SetPlaneOffset { value: String },
    SetPlaneAngle { value: String },
    FocusDim(RectAxis),
    FocusLineLength,
    FocusCircleDiameter,
    FocusPlaneDim(PlaneDim),
    Orbit { dx: f32, dy: f32 },
    Pan { dx: f32, dy: f32 },
    Zoom { scroll: f32 },
    View(StandardView),
    ViewEdge(CubeEdgeId),
    ViewCorner(CubeCornerId),
    ViewHome,
    SetHomeView,
    ProjectionMode(ProjectionMode),
    ToggleProjectionMode,
    /// Show/hide a UI pane. `None` toggles.
    SetPane { pane: Pane, visible: Option<bool> },
    AddParameter { name: String, expression: String },
    SetParameterName { index: usize, name: String },
    SetParameterExpression { index: usize, expression: String },
    DeleteParameter { index: usize },
    DeleteSelection,
    /// Show/hide the command palette. `None` toggles.
    SetCommandPalette { open: Option<bool> },
    /// Run the best-matching palette command for a query.
    RunPaletteCommand { query: String },
    // Synthetic input (viewport-local pixel coordinates)
    Move { x: f32, y: f32 },
    Click { x: f32, y: f32 },
    /// Move/click at ground-plane world coordinates (millimetres, z = 0).
    MoveGround { x: f32, y: f32 },
    ClickGround { x: f32, y: f32 },
    Drag {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    },
    RightDrag { dx: f32, dy: f32 },
    RightDragShift { dx: f32, dy: f32 },
    Scroll { delta: f32 },
    Key(Key),
    KeyDown(Key),
    KeyUp(Key),
    Type(String),

    // Sequencing
    WaitMs(u64),
    WaitFrames(u32),
    Screenshot(String),
    Quit,
}

impl Instruction {
    /// Format this instruction as a script line (for logging).
    pub fn as_line(&self) -> String {
        match self {
            Instruction::New => "new".to_string(),
            Instruction::Open(path) => format!("open {path}"),
            Instruction::Save(None) => "save".to_string(),
            Instruction::Save(Some(path)) => format!("save {path}"),
            Instruction::Clear => "clear".to_string(),
            Instruction::Undo => "undo".to_string(),
            Instruction::Tool(Tool::Select) => "tool select".to_string(),
            Instruction::Tool(Tool::Rectangle) => "tool rectangle".to_string(),
            Instruction::Tool(Tool::Line) => "tool line".to_string(),
            Instruction::Tool(Tool::Circle) => "tool circle".to_string(),
            Instruction::Tool(Tool::ConstructionPlane) => "tool plane".to_string(),
            Instruction::Tool(Tool::Sketch) => "tool sketch".to_string(),
            Instruction::Tool(Tool::Dimension) => "tool dimension".to_string(),
            Instruction::Tool(Tool::Constraint) => "tool constraint".to_string(),
            Instruction::BeginSketch { face } => format!("begin_sketch {}", face_script_name(*face)),
            Instruction::OpenSketch { sketch } => format!("open_sketch {sketch}"),
            Instruction::ExitSketch => "exit_sketch".to_string(),
            Instruction::SetElementVisible { element, visible } => {
                let (kind, index) = element_script_parts(*element);
                let verb = match visible {
                    Some(true) => "show",
                    Some(false) => "hide",
                    None => "toggle",
                };
                format!("element {kind} {index} {verb}")
            }
            Instruction::SelectSceneElement { element, additive } => {
                let tokens = element_script_tokens(*element);
                let target = if let Some(point) = tokens.point {
                    point_script_tokens(point)
                } else {
                    let edge = tokens
                        .edge
                        .map(|edge| format!(" {}", edge.script_name()))
                        .unwrap_or_default();
                    format!("{} {}{}", tokens.kind, tokens.index, edge)
                };
                if *additive {
                    format!("select {target} add")
                } else {
                    format!("select {target}")
                }
            }
            Instruction::ClearSceneSelection => "clear_selection".to_string(),
            Instruction::SetShapeConstruction { element, construction } => {
                let tokens = element_script_tokens(*element);
                let edge = tokens
                    .edge
                    .map(|edge| format!(" {}", edge.script_name()))
                    .unwrap_or_default();
                format!(
                    "set_construction {} {}{} {}",
                    tokens.kind,
                    tokens.index,
                    edge,
                    if *construction { "true" } else { "false" }
                )
            }
            Instruction::ApplyConstruction { construction } => format!(
                "apply_construction {}",
                if *construction { "true" } else { "false" }
            ),
            Instruction::ToggleConstruction => "toggle_construction".to_string(),
            Instruction::SetElementName { element, name } => {
                let (kind, index) = element_script_parts(*element);
                format!("set_name {kind} {index} {name}")
            }
            Instruction::FocusElementName => "focus_name".to_string(),
            Instruction::SetDim { axis, value } => {
                let name = match axis {
                    RectAxis::Width => "width",
                    RectAxis::Height => "height",
                };
                format!("set_dim {name} {value}")
            }
            Instruction::SetLineLength { value } => format!("set_dim length {value}"),
            Instruction::SetCircleDiameter { value } => format!("set_dim diameter {value}"),
            Instruction::SetDimLabelOffset { axis, offset } => {
                let name = match axis {
                    DimLabelAxis::Width => "width",
                    DimLabelAxis::Height => "height",
                    DimLabelAxis::Length => "length",
                };
                format!("set_dim_label_offset {name} {offset}")
            }
            Instruction::BeginEditCommittedDim { axis } => {
                let name = match axis {
                    DimLabelAxis::Width => "width",
                    DimLabelAxis::Height => "height",
                    DimLabelAxis::Length => "length",
                };
                format!("edit_dim {name}")
            }
            Instruction::CommitCommittedDim => "commit_dim".to_string(),
            Instruction::AddDistanceConstraint { target, expression } => {
                let target_name = match target {
                    DistanceTarget::LineLength(i) => format!("line {i}"),
                    DistanceTarget::RectWidth(i) => format!("rect {i} width"),
                    DistanceTarget::RectHeight(i) => format!("rect {i} height"),
                    DistanceTarget::CircleDiameter(i) => format!("circle {i}"),
                    DistanceTarget::LineLineDistance { .. } => "line_line".to_string(),
                    DistanceTarget::PointPointDistance { .. } => "point_point".to_string(),
                    DistanceTarget::PointLineDistance { .. } => "point_line".to_string(),
                };
                format!("add_constraint {target_name} {expression}")
            }
            Instruction::AddGeometricConstraint(kind) => {
                format!("add_geometric_constraint {}", geometric_constraint_script_name(*kind))
            }
            Instruction::ApplyConstraintShortcut(index) => {
                format!("constraint_shortcut {index}")
            }
            Instruction::DragVertex { point, u, v } => {
                format!("drag_vertex {} {} {}", point_script_tokens(*point), u, v)
            }
            Instruction::DragLineSegment {
                target,
                anchor_u,
                anchor_v,
                u,
                v,
            } => format!(
                "drag_line {} {} {} {} {}",
                constraint_line_script_tokens(*target),
                anchor_u,
                anchor_v,
                u,
                v
            ),
            Instruction::BeginEditConstructionPlane { index } => format!("edit_plane {index}"),
            Instruction::CommitConstructionPlane => "commit_plane".to_string(),
            Instruction::SetPlaneOffset { value } => format!("set_dim offset {value}"),
            Instruction::SetPlaneAngle { value } => format!("set_dim angle {value}"),
            Instruction::FocusDim(axis) => {
                let name = match axis {
                    RectAxis::Width => "width",
                    RectAxis::Height => "height",
                };
                format!("focus_dim {name}")
            }
            Instruction::FocusLineLength => "focus_dim length".to_string(),
            Instruction::FocusCircleDiameter => "focus_dim diameter".to_string(),
            Instruction::FocusPlaneDim(PlaneDim::Offset) => "focus_dim offset".to_string(),
            Instruction::FocusPlaneDim(PlaneDim::Angle) => "focus_dim angle".to_string(),
            Instruction::Orbit { dx, dy } => format!("orbit {dx} {dy}"),
            Instruction::Pan { dx, dy } => format!("pan {dx} {dy}"),
            Instruction::Zoom { scroll } => format!("zoom {scroll}"),
            Instruction::View(view) => format!("view {}", view_script_name(*view)),
            Instruction::ViewEdge(edge) => format!("view edge {}", edge_script_name(*edge)),
            Instruction::ViewCorner(corner) => {
                format!("view corner {}", corner_script_name(*corner))
            }
            Instruction::ViewHome => "view_home".to_string(),
            Instruction::SetHomeView => "set_home_view".to_string(),
            Instruction::ProjectionMode(mode) => {
                format!("view {}", projection_mode_script_name(*mode))
            }
            Instruction::ToggleProjectionMode => "toggle_projection".to_string(),
            Instruction::SetPane { pane, visible } => {
                let verb = match visible {
                    Some(true) => "show",
                    Some(false) => "hide",
                    None => "toggle",
                };
                format!("pane {} {verb}", pane.script_name())
            }
            Instruction::AddParameter { name, expression } => {
                format!("parameter add {name} {expression}")
            }
            Instruction::SetParameterName { index, name } => {
                format!("parameter name {index} {name}")
            }
            Instruction::SetParameterExpression { index, expression } => {
                format!("parameter value {index} {expression}")
            }
            Instruction::DeleteParameter { index } => format!("parameter delete {index}"),
            Instruction::DeleteSelection => "delete_selection".to_string(),
            Instruction::SetCommandPalette { open } => {
                let verb = match open {
                    Some(true) => "show",
                    Some(false) => "hide",
                    None => "toggle",
                };
                format!("palette {verb}")
            }
            Instruction::RunPaletteCommand { query } => format!("palette run {query}"),

            Instruction::Move { x, y } => format!("move {x} {y}"),
            Instruction::Click { x, y } => format!("click {x} {y}"),
            Instruction::MoveGround { x, y } => format!("move_ground {x} {y}"),
            Instruction::ClickGround { x, y } => format!("click_ground {x} {y}"),
            Instruction::Drag { x0, y0, x1, y1 } => format!("drag {x0} {y0} {x1} {y1}"),
            Instruction::RightDrag { dx, dy } => format!("right_drag_rel {dx} {dy}"),
            Instruction::RightDragShift { dx, dy } => format!("right_drag_pan {dx} {dy}"),
            Instruction::Scroll { delta } => format!("wheel {delta}"),
            Instruction::Key(key) => format!("key {}", key_name(*key)),
            Instruction::KeyDown(key) => format!("keydown {}", key_name(*key)),
            Instruction::KeyUp(key) => format!("keyup {}", key_name(*key)),
            Instruction::Type(text) => {
                if text.contains(' ') {
                    format!("type \"{text}\"")
                } else {
                    format!("type {text}")
                }
            }
            Instruction::WaitMs(ms) => format!("wait {ms}ms"),
            Instruction::WaitFrames(n) => format!("wait {n}"),
            Instruction::Screenshot(path) => format!("screenshot {path}"),
            Instruction::Quit => "quit".to_string(),
        }
    }

    /// Format this instruction as a Lua API call (for `--show-commands` logging).
    pub fn as_lua(&self) -> String {
        match self {
            Instruction::New => "paramcad.new()".to_string(),
            Instruction::Open(path) => format!("paramcad.open({path:?})"),
            Instruction::Save(None) => "paramcad.save()".to_string(),
            Instruction::Save(Some(path)) => format!("paramcad.save({path:?})"),
            Instruction::Clear => "paramcad.clear()".to_string(),
            Instruction::Undo => "paramcad.undo()".to_string(),
            Instruction::Tool(tool) => format!("paramcad.tool({:?})", tool_lua_name(*tool)),
            Instruction::BeginSketch { face } => {
                let (kind, index) = face_lua_parts(*face);
                format!("paramcad.begin_sketch({kind:?}, {index})")
            }
            Instruction::OpenSketch { sketch } => format!("paramcad.open_sketch({sketch})"),
            Instruction::ExitSketch => "paramcad.exit_sketch()".to_string(),
            Instruction::SetElementVisible { element, visible } => {
                let target = element_lua_ref(*element);
                let verb = match visible {
                    Some(true) => "show",
                    Some(false) => "hide",
                    None => "toggle",
                };
                format!("paramcad.set_visible({target}, {verb:?})")
            }
            Instruction::SelectSceneElement { element, additive } => {
                let target = element_lua_ref(*element);
                if *additive {
                    format!("paramcad.select({target}, {{ additive = true }})")
                } else {
                    format!("paramcad.select({target})")
                }
            }
            Instruction::ClearSceneSelection => "paramcad.clear_selection()".to_string(),
            Instruction::SetShapeConstruction { element, construction } => {
                format!(
                    "paramcad.set_construction({}, {})",
                    element_lua_ref(*element),
                    construction
                )
            }
            Instruction::ApplyConstruction { construction } => {
                format!("paramcad.apply_construction({construction})")
            }
            Instruction::ToggleConstruction => "paramcad.toggle_construction()".to_string(),
            Instruction::SetElementName { element, name } => {
                format!(
                    "paramcad.set_name({}, {name:?})",
                    element_lua_ref(*element)
                )
            }
            Instruction::FocusElementName => "paramcad.focus_name()".to_string(),
            Instruction::SetDim { axis, value } => {
                format!(
                    "paramcad.set_dim({:?}, {value:?})",
                    rect_axis_lua_name(*axis)
                )
            }
            Instruction::SetDimLabelOffset { axis, offset } => {
                format!(
                    "paramcad.set_dim_label_offset({:?}, {offset})",
                    dim_label_axis_lua_name(*axis)
                )
            }
            Instruction::BeginEditCommittedDim { axis } => {
                format!(
                    "paramcad.edit_dim({:?})",
                    dim_label_axis_lua_name(*axis)
                )
            }
            Instruction::CommitCommittedDim => "paramcad.commit_dim()".to_string(),
            Instruction::AddDistanceConstraint { target, expression } => {
                format!(
                    "paramcad.add_constraint({}, {expression:?})",
                    distance_target_lua_ref(target)
                )
            }
            Instruction::AddGeometricConstraint(kind) => {
                format!(
                    "paramcad.add_geometric_constraint({:?})",
                    geometric_constraint_lua_name(*kind)
                )
            }
            Instruction::ApplyConstraintShortcut(index) => {
                format!("paramcad.constraint_shortcut({index})")
            }
            Instruction::DragVertex { point, u, v } => {
                format!(
                    "paramcad.drag_vertex({}, {u}, {v})",
                    constraint_point_lua_ref(*point)
                )
            }
            Instruction::DragLineSegment {
                target,
                anchor_u,
                anchor_v,
                u,
                v,
            } => format!(
                "paramcad.drag_line({}, {anchor_u}, {anchor_v}, {u}, {v})",
                constraint_line_lua_ref(*target)
            ),
            Instruction::SetLineLength { value } => {
                format!("paramcad.set_dim(\"length\", {value:?})")
            }
            Instruction::SetCircleDiameter { value } => {
                format!("paramcad.set_dim(\"diameter\", {value:?})")
            }
            Instruction::BeginEditConstructionPlane { index } => {
                format!("paramcad.edit_plane({index})")
            }
            Instruction::CommitConstructionPlane => "paramcad.commit_plane()".to_string(),
            Instruction::SetPlaneOffset { value } => {
                format!("paramcad.set_dim(\"offset\", {value:?})")
            }
            Instruction::SetPlaneAngle { value } => {
                format!("paramcad.set_dim(\"angle\", {value:?})")
            }
            Instruction::FocusDim(axis) => {
                format!("paramcad.focus_dim({:?})", rect_axis_lua_name(*axis))
            }
            Instruction::FocusLineLength => "paramcad.focus_dim(\"length\")".to_string(),
            Instruction::FocusCircleDiameter => "paramcad.focus_dim(\"diameter\")".to_string(),
            Instruction::FocusPlaneDim(dim) => {
                format!("paramcad.focus_dim({:?})", plane_dim_lua_name(*dim))
            }
            Instruction::Orbit { dx, dy } => format!("paramcad.orbit({dx}, {dy})"),
            Instruction::Pan { dx, dy } => format!("paramcad.pan({dx}, {dy})"),
            Instruction::Zoom { scroll } => format!("paramcad.wheel({scroll})"),
            Instruction::View(view) => format!("paramcad.view({:?})", view_script_name(*view)),
            Instruction::ViewEdge(edge) => {
                format!("paramcad.view(\"edge\", {:?})", edge_script_name(*edge))
            }
            Instruction::ViewCorner(corner) => format!(
                "paramcad.view(\"corner\", {:?})",
                corner_script_name(*corner)
            ),
            Instruction::ViewHome => "paramcad.view_home()".to_string(),
            Instruction::SetHomeView => "paramcad.set_home_view()".to_string(),
            Instruction::ProjectionMode(mode) => {
                format!("paramcad.view({:?})", projection_mode_script_name(*mode))
            }
            Instruction::ToggleProjectionMode => "paramcad.toggle_projection()".to_string(),
            Instruction::SetPane { pane, visible } => {
                let verb = match visible {
                    Some(true) => "show",
                    Some(false) => "hide",
                    None => "toggle",
                };
                format!("paramcad.pane({:?}, {verb:?})", pane.script_name())
            }
            Instruction::AddParameter { name, expression } => {
                format!("paramcad.parameter(\"add\", {name:?}, {expression:?})")
            }
            Instruction::SetParameterName { index, name } => {
                format!("paramcad.parameter(\"name\", {index}, {name:?})")
            }
            Instruction::SetParameterExpression { index, expression } => {
                format!("paramcad.parameter(\"value\", {index}, {expression:?})")
            }
            Instruction::DeleteParameter { index } => {
                format!("paramcad.parameter(\"delete\", {index})")
            }
            Instruction::DeleteSelection => "paramcad.delete_selection()".to_string(),
            Instruction::SetCommandPalette { open } => {
                let verb = match open {
                    Some(true) => "show",
                    Some(false) => "hide",
                    None => "toggle",
                };
                format!("paramcad.palette({verb:?})")
            }
            Instruction::RunPaletteCommand { query } => {
                format!("paramcad.palette(\"run\", {query:?})")
            }
            Instruction::Move { x, y } => format!("paramcad.move({x}, {y})"),
            Instruction::Click { x, y } => format!("paramcad.click({x}, {y})"),
            Instruction::MoveGround { x, y } => format!("paramcad.move_ground({x}, {y})"),
            Instruction::ClickGround { x, y } => format!("paramcad.click_ground({x}, {y})"),
            Instruction::Drag { x0, y0, x1, y1 } => {
                format!("paramcad.drag({x0}, {y0}, {x1}, {y1})")
            }
            Instruction::RightDrag { dx, dy } => format!("paramcad.right_drag({dx}, {dy})"),
            Instruction::RightDragShift { dx, dy } => {
                format!("paramcad.right_drag_pan({dx}, {dy})")
            }
            Instruction::Scroll { delta } => format!("paramcad.wheel({delta})"),
            Instruction::Key(key) => format!("paramcad.key({:?})", key_name(*key)),
            Instruction::KeyDown(key) => format!("paramcad.keydown({:?})", key_name(*key)),
            Instruction::KeyUp(key) => format!("paramcad.keyup({:?})", key_name(*key)),
            Instruction::Type(text) => format!("paramcad.type({text:?})"),
            Instruction::WaitMs(ms) => format!("paramcad.wait_ms({ms})"),
            Instruction::WaitFrames(n) => format!("paramcad.wait({n})"),
            Instruction::Screenshot(path) => format!("paramcad.screenshot({path:?})"),
            Instruction::Quit => "paramcad.quit()".to_string(),
        }
    }
}

/// Script load / execution errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError {
    pub message: String,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ScriptError {}

/// Map a human-readable key name to an egui [`Key`].
pub fn parse_key(name: &str) -> Result<Key, String> {
    match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Ok(Key::Enter),
        "tab" => Ok(Key::Tab),
        "escape" | "esc" => Ok(Key::Escape),
        "backspace" => Ok(Key::Backspace),
        "delete" | "del" => Ok(Key::Delete),
        "left" => Ok(Key::ArrowLeft),
        "right" => Ok(Key::ArrowRight),
        "up" => Ok(Key::ArrowUp),
        "down" => Ok(Key::ArrowDown),
        "space" => Ok(Key::Space),
        "r" => Ok(Key::R),
        "a" => Ok(Key::A),
        "b" => Ok(Key::B),
        "c" => Ok(Key::C),
        "d" => Ok(Key::D),
        "e" => Ok(Key::E),
        "f" => Ok(Key::F),
        "g" => Ok(Key::G),
        "h" => Ok(Key::H),
        "i" => Ok(Key::I),
        "j" => Ok(Key::J),
        "k" => Ok(Key::K),
        "l" => Ok(Key::L),
        "m" => Ok(Key::M),
        "n" => Ok(Key::N),
        "o" => Ok(Key::O),
        "p" => Ok(Key::P),
        "q" => Ok(Key::Q),
        "s" => Ok(Key::S),
        "t" => Ok(Key::T),
        "u" => Ok(Key::U),
        "v" => Ok(Key::V),
        "w" => Ok(Key::W),
        "x" => Ok(Key::X),
        "y" => Ok(Key::Y),
        "z" => Ok(Key::Z),
        "0" => Ok(Key::Num0),
        "1" => Ok(Key::Num1),
        "2" => Ok(Key::Num2),
        "3" => Ok(Key::Num3),
        "4" => Ok(Key::Num4),
        "5" => Ok(Key::Num5),
        "6" => Ok(Key::Num6),
        "7" => Ok(Key::Num7),
        "8" => Ok(Key::Num8),
        "9" => Ok(Key::Num9),
        _ => Err(format!("unknown key '{name}'")),
    }
}

fn face_script_name(face: FaceId) -> String {
    match face {
        FaceId::Rect(i) => format!("rect {i}"),
        FaceId::Circle(i) => format!("circle {i}"),
        FaceId::ConstructionPlane(i) => format!("construction_plane {i}"),
    }
}

struct ElementScriptTokens {
    kind: &'static str,
    index: usize,
    edge: Option<RectEdge>,
    point: Option<crate::model::ConstraintPoint>,
}

fn element_script_parts(element: SceneElement) -> (&'static str, usize) {
    let tokens = element_script_tokens(element);
    (tokens.kind, tokens.index)
}

fn element_script_tokens(element: SceneElement) -> ElementScriptTokens {
    match element {
        SceneElement::ConstructionPlane(i) => ElementScriptTokens {
            kind: "construction_plane",
            index: i,
            edge: None,
            point: None,
        },
        SceneElement::Sketch(i) => ElementScriptTokens {
            kind: "sketch",
            index: i,
            edge: None,
            point: None,
        },
        SceneElement::Rect(i) => ElementScriptTokens {
            kind: "rect",
            index: i,
            edge: None,
            point: None,
        },
        SceneElement::Line(i) => ElementScriptTokens {
            kind: "line",
            index: i,
            edge: None,
            point: None,
        },
        SceneElement::Circle(i) => ElementScriptTokens {
            kind: "circle",
            index: i,
            edge: None,
            point: None,
        },
        SceneElement::RectEdge(i, edge) => ElementScriptTokens {
            kind: "rect",
            index: i,
            edge: Some(edge),
            point: None,
        },
        SceneElement::Constraint(i) => ElementScriptTokens {
            kind: "constraint",
            index: i,
            edge: None,
            point: None,
        },
        SceneElement::Point(point) => ElementScriptTokens {
            kind: "point",
            index: 0,
            edge: None,
            point: Some(point),
        },
    }
}

fn point_script_tokens(point: crate::model::ConstraintPoint) -> String {
    use crate::model::{ConstraintPoint, LineEnd};
    match point {
        ConstraintPoint::LineEndpoint { line, end } => {
            let end_name = match end {
                LineEnd::Start => "start",
                LineEnd::End => "end",
            };
            format!("point line {line} {end_name}")
        }
        ConstraintPoint::RectCorner { rect, corner } => {
            format!("point rect {rect} corner {corner}")
        }
        ConstraintPoint::CircleCenter(circle) => format!("point circle {circle} center"),
    }
}

fn geometric_constraint_script_name(
    kind: crate::geometric_constraints::GeometricConstraintType,
) -> &'static str {
    use crate::geometric_constraints::GeometricConstraintType;
    match kind {
        GeometricConstraintType::Parallel => "parallel",
        GeometricConstraintType::Perpendicular => "perpendicular",
        GeometricConstraintType::Coincident => "coincident",
        GeometricConstraintType::Midpoint => "midpoint",
        GeometricConstraintType::Horizontal => "horizontal",
        GeometricConstraintType::Vertical => "vertical",
    }
}

fn constraint_line_script_tokens(line: ConstraintLine) -> String {
    match line {
        ConstraintLine::Line(index) => format!("line {index}"),
        ConstraintLine::RectEdge { rect, edge } => {
            format!("rect {rect} {}", edge.script_name())
        }
    }
}


/// Map an applied [`Action`] to a script [`Instruction`] when one exists.
pub fn instruction_from_action(action: &Action, doc: &crate::model::Document) -> Option<Instruction> {
    use crate::actions::dim_label_axis_for_target;
    match action {
        Action::NewDocument => Some(Instruction::New),
        Action::Open { path } => Some(Instruction::Open(path.clone())),
        Action::Save { path } => Some(Instruction::Save(path.clone())),
        Action::Clear => Some(Instruction::Clear),
        Action::UndoLast => Some(Instruction::Undo),
        Action::SetTool(tool) => Some(Instruction::Tool(*tool)),
        Action::SetRectDimension { axis, value } => Some(Instruction::SetDim {
            axis: *axis,
            value: value.clone(),
        }),
        Action::FocusRectDimension { axis } => Some(Instruction::FocusDim(*axis)),
        Action::SetLineLength { value } => Some(Instruction::SetLineLength {
            value: value.clone(),
        }),
        Action::FocusLineLength => Some(Instruction::FocusLineLength),
        Action::SetCircleDiameter { value } => Some(Instruction::SetCircleDiameter {
            value: value.clone(),
        }),
        Action::FocusCircleDiameter => Some(Instruction::FocusCircleDiameter),
        Action::SetDimLabelOffset { target, offset } => {
            dim_label_axis_for_target(doc, *target).map(|axis| {
                Instruction::SetDimLabelOffset {
                    axis,
                    offset: *offset,
                }
            })
        }
        Action::BeginEditCommittedDim { target } => {
            dim_label_axis_for_target(doc, *target).map(|axis| {
                Instruction::BeginEditCommittedDim { axis }
            })
        }
        Action::CommitCommittedDim => Some(Instruction::CommitCommittedDim),
        Action::BeginEditConstructionPlane { index } => {
            Some(Instruction::BeginEditConstructionPlane { index: *index })
        }
        Action::CommitConstructionPlane => Some(Instruction::CommitConstructionPlane),
        Action::SetPlaneOffset { value } => Some(Instruction::SetPlaneOffset {
            value: value.clone(),
        }),
        Action::SetPlaneAngle { value } => Some(Instruction::SetPlaneAngle {
            value: value.clone(),
        }),
        Action::FocusPlaneDim { dim } => Some(Instruction::FocusPlaneDim(*dim)),
        Action::BeginSketch { face, .. } => Some(Instruction::BeginSketch { face: *face }),
        Action::OpenSketch { sketch, .. } => Some(Instruction::OpenSketch { sketch: *sketch }),
        Action::ExitSketch => Some(Instruction::ExitSketch),
        Action::SetElementVisible { element, visible } => Some(Instruction::SetElementVisible {
            element: *element,
            visible: Some(*visible),
        }),
        Action::ToggleElementVisibility(element) => Some(Instruction::SetElementVisible {
            element: *element,
            visible: None,
        }),
        Action::SetHomeView => Some(Instruction::SetHomeView),
        Action::SetPaneVisible { pane, visible } => Some(Instruction::SetPane {
            pane: *pane,
            visible: Some(*visible),
        }),
        Action::TogglePane(pane) => Some(Instruction::SetPane {
            pane: *pane,
            visible: None,
        }),
        Action::AddParameter { name, expression } => Some(Instruction::AddParameter {
            name: name.clone(),
            expression: expression.clone(),
        }),
        Action::CommitParameterName { index, name } => Some(Instruction::SetParameterName {
            index: *index,
            name: name.clone(),
        }),
        Action::CommitParameterExpression { index, expression } => {
            Some(Instruction::SetParameterExpression {
                index: *index,
                expression: expression.clone(),
            })
        }
        Action::DeleteParameter { index } => Some(Instruction::DeleteParameter { index: *index }),
        Action::DeleteSelection => Some(Instruction::DeleteSelection),
        Action::SetCommandPaletteOpen { open } => Some(Instruction::SetCommandPalette {
            open: Some(*open),
        }),
        Action::ToggleCommandPalette => Some(Instruction::SetCommandPalette { open: None }),
        Action::ClickSceneElement { element, additive } => Some(Instruction::SelectSceneElement {
            element: *element,
            additive: *additive,
        }),
        Action::ClearSceneSelection => Some(Instruction::ClearSceneSelection),
        Action::SetShapeConstruction {
            element,
            construction,
        } => Some(Instruction::SetShapeConstruction {
            element: *element,
            construction: *construction,
        }),
        Action::ApplyConstruction { construction } => Some(Instruction::ApplyConstruction {
            construction: *construction,
        }),
        Action::ToggleConstruction => Some(Instruction::ToggleConstruction),
        Action::AddGeometricConstraint(kind) => Some(Instruction::AddGeometricConstraint(*kind)),
        Action::ApplyConstraintShortcut(index) => Some(Instruction::ApplyConstraintShortcut(*index)),
        Action::DragVertex { point, u, v } => Some(Instruction::DragVertex {
            point: *point,
            u: *u,
            v: *v,
        }),
        Action::CommitElementName { element, name } => Some(Instruction::SetElementName {
            element: *element,
            name: name.clone(),
        }),
        Action::FocusElementName => Some(Instruction::FocusElementName),
        _ => None,
    }
}

fn view_script_name(view: StandardView) -> &'static str {
    match view {
        StandardView::Front => "front",
        StandardView::Back => "back",
        StandardView::Left => "left",
        StandardView::Right => "right",
        StandardView::Top => "top",
        StandardView::Bottom => "bottom",
    }
}

fn projection_mode_script_name(mode: ProjectionMode) -> &'static str {
    match mode {
        ProjectionMode::Orthographic => "orthographic",
        ProjectionMode::Natural => "natural",
    }
}

fn edge_script_name(edge: CubeEdgeId) -> &'static str {
    match edge {
        CubeEdgeId::FrontBottom => "front_bottom",
        CubeEdgeId::RightBottom => "right_bottom",
        CubeEdgeId::BackBottom => "back_bottom",
        CubeEdgeId::LeftBottom => "left_bottom",
        CubeEdgeId::FrontTop => "front_top",
        CubeEdgeId::RightTop => "right_top",
        CubeEdgeId::BackTop => "back_top",
        CubeEdgeId::LeftTop => "left_top",
        CubeEdgeId::FrontLeft => "front_left",
        CubeEdgeId::FrontRight => "front_right",
        CubeEdgeId::BackRight => "back_right",
        CubeEdgeId::BackLeft => "back_left",
    }
}

fn corner_script_name(corner: CubeCornerId) -> &'static str {
    match corner {
        CubeCornerId::FrontLeftBottom => "front_left_bottom",
        CubeCornerId::FrontRightBottom => "front_right_bottom",
        CubeCornerId::BackRightBottom => "back_right_bottom",
        CubeCornerId::BackLeftBottom => "back_left_bottom",
        CubeCornerId::FrontLeftTop => "front_left_top",
        CubeCornerId::FrontRightTop => "front_right_top",
        CubeCornerId::BackRightTop => "back_right_top",
        CubeCornerId::BackLeftTop => "back_left_top",
    }
}

fn key_name(key: Key) -> &'static str {
    match key {
        Key::Enter => "enter",
        Key::Tab => "tab",
        Key::Escape => "escape",
        Key::Backspace => "backspace",
        Key::Delete => "delete",
        Key::ArrowLeft => "left",
        Key::ArrowRight => "right",
        Key::ArrowUp => "up",
        Key::ArrowDown => "down",
        Key::Space => "space",
        Key::R => "r",
        Key::A => "a",
        Key::B => "b",
        Key::C => "c",
        Key::D => "d",
        Key::E => "e",
        Key::F => "f",
        Key::G => "g",
        Key::H => "h",
        Key::I => "i",
        Key::J => "j",
        Key::K => "k",
        Key::L => "l",
        Key::M => "m",
        Key::N => "n",
        Key::O => "o",
        Key::P => "p",
        Key::Q => "q",
        Key::S => "s",
        Key::T => "t",
        Key::U => "u",
        Key::V => "v",
        Key::W => "w",
        Key::X => "x",
        Key::Y => "y",
        Key::Z => "z",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        _ => "?",
    }
}

fn tool_lua_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "select",
        Tool::Rectangle => "rectangle",
        Tool::Line => "line",
        Tool::Circle => "circle",
        Tool::ConstructionPlane => "construction_plane",
        Tool::Sketch => "sketch",
        Tool::Dimension => "dimension",
        Tool::Constraint => "constraint",
    }
}

fn face_lua_parts(face: FaceId) -> (&'static str, usize) {
    match face {
        FaceId::Rect(i) => ("rect", i),
        FaceId::Circle(i) => ("circle", i),
        FaceId::ConstructionPlane(i) => ("construction_plane", i),
    }
}

fn rect_axis_lua_name(axis: RectAxis) -> &'static str {
    match axis {
        RectAxis::Width => "width",
        RectAxis::Height => "height",
    }
}

fn dim_label_axis_lua_name(axis: DimLabelAxis) -> &'static str {
    match axis {
        DimLabelAxis::Width => "width",
        DimLabelAxis::Height => "height",
        DimLabelAxis::Length => "length",
    }
}

fn plane_dim_lua_name(dim: PlaneDim) -> &'static str {
    match dim {
        PlaneDim::Offset => "offset",
        PlaneDim::Angle => "angle",
    }
}

fn geometric_constraint_lua_name(
    kind: crate::geometric_constraints::GeometricConstraintType,
) -> &'static str {
    geometric_constraint_script_name(kind)
}

fn element_lua_ref(element: SceneElement) -> String {
    let tokens = element_script_tokens(element);
    if let Some(point) = tokens.point {
        return format!("{{ kind = \"point\", {} }}", point_lua_fields(point));
    }
    if let Some(edge) = tokens.edge {
        return format!(
            "{{ kind = \"{}\", index = {}, edge = {:?} }}",
            tokens.kind, tokens.index, edge.script_name()
        );
    }
    format!("{{ kind = \"{}\", index = {} }}", tokens.kind, tokens.index)
}

fn point_lua_fields(point: ConstraintPoint) -> String {
    use crate::model::{ConstraintPoint, LineEnd};
    match point {
        ConstraintPoint::LineEndpoint { line, end } => {
            let end_name = match end {
                LineEnd::Start => "start",
                LineEnd::End => "end",
            };
            format!("kind = \"line\", index = {line}, end = \"{end_name}\"")
        }
        ConstraintPoint::RectCorner { rect, corner } => {
            format!("kind = \"rect\", index = {rect}, corner = {corner}")
        }
        ConstraintPoint::CircleCenter(circle) => {
            format!("kind = \"circle\", index = {circle}")
        }
    }
}

fn constraint_line_lua_ref(line: ConstraintLine) -> String {
    match line {
        ConstraintLine::Line(index) => format!("{{ kind = \"line\", index = {index} }}"),
        ConstraintLine::RectEdge { rect, edge } => format!(
            "{{ kind = \"rect\", index = {rect}, edge = {:?} }}",
            edge.script_name()
        ),
    }
}

fn constraint_point_lua_ref(point: ConstraintPoint) -> String {
    format!("{{ {} }}", point_lua_fields(point))
}

fn distance_target_lua_ref(target: &DistanceTarget) -> String {
    match target {
        DistanceTarget::LineLength(index) => {
            format!("{{ kind = \"line\", index = {index} }}")
        }
        DistanceTarget::CircleDiameter(index) => {
            format!("{{ kind = \"circle\", index = {index} }}")
        }
        DistanceTarget::RectWidth(index) => {
            format!("{{ kind = \"rect\", index = {index}, axis = \"width\" }}")
        }
        DistanceTarget::RectHeight(index) => {
            format!("{{ kind = \"rect\", index = {index}, axis = \"height\" }}")
        }
        DistanceTarget::LineLineDistance { .. }
        | DistanceTarget::PointPointDistance { .. }
        | DistanceTarget::PointLineDistance { .. } => {
            "{ kind = \"selection\" }".to_string()
        }
    }
}

/// Queued synthetic pointer/keyboard events injected into egui each frame.
#[derive(Default)]
pub struct SyntheticInput {
    events: Vec<egui::Event>,
    pointer_pos: Option<egui::Pos2>,
    /// When set, secondary-button drag deltas are applied via events.
    pending_right_drag: Option<(egui::Vec2, Modifiers)>,
}

impl SyntheticInput {
    pub fn inject(&mut self, ctx: &egui::Context) {
        if self.events.is_empty() && self.pending_right_drag.is_none() {
            return;
        }
        ctx.input_mut(|input| {
            input.events.extend(self.events.drain(..));
        });
    }

    /// Apply secondary-button drag after egui has processed pointer state.
    pub fn apply_pending_drag(&mut self, viewport: egui::Rect, on_drag: impl FnMut(egui::Vec2, Modifiers, f32)) {
        if let Some((delta, modifiers)) = self.pending_right_drag.take() {
            let mut callback = on_drag;
            callback(delta, modifiers, viewport.height());
        }
    }

    fn viewport_pos(viewport: egui::Rect, x: f32, y: f32) -> egui::Pos2 {
        viewport.min + egui::vec2(x, y)
    }

    pub fn move_to(&mut self, viewport: egui::Rect, x: f32, y: f32) {
        let pos = Self::viewport_pos(viewport, x, y);
        self.pointer_pos = Some(pos);
        self.events.push(egui::Event::PointerMoved(pos));
    }

    pub fn click(&mut self, viewport: egui::Rect, x: f32, y: f32) {
        let pos = Self::viewport_pos(viewport, x, y);
        self.pointer_pos = Some(pos);
        self.events.push(egui::Event::PointerMoved(pos));
        self.events.push(egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        self.events.push(egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
    }

    pub fn drag(&mut self, viewport: egui::Rect, x0: f32, y0: f32, x1: f32, y1: f32) {
        let p0 = Self::viewport_pos(viewport, x0, y0);
        let p1 = Self::viewport_pos(viewport, x1, y1);
        self.pointer_pos = Some(p1);
        self.events.push(egui::Event::PointerMoved(p0));
        self.events.push(egui::Event::PointerButton {
            pos: p0,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        self.events.push(egui::Event::PointerMoved(p1));
        self.events.push(egui::Event::PointerButton {
            pos: p1,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
    }

    pub fn right_drag(&mut self, viewport: egui::Rect, dx: f32, dy: f32, shift: bool) {
        let pos = self
            .pointer_pos
            .unwrap_or_else(|| viewport.center());
        self.events.push(egui::Event::PointerMoved(pos));
        self.events.push(egui::Event::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: if shift { Modifiers::SHIFT } else { Modifiers::NONE },
        });
        self.pending_right_drag = Some((egui::vec2(dx, dy), if shift { Modifiers::SHIFT } else { Modifiers::NONE }));
        self.events.push(egui::Event::PointerButton {
            pos: pos + egui::vec2(dx, dy),
            button: PointerButton::Secondary,
            pressed: false,
            modifiers: if shift { Modifiers::SHIFT } else { Modifiers::NONE },
        });
    }

    pub fn scroll(&mut self, delta: f32) {
        self.events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, delta),
            modifiers: Modifiers::NONE,
        });
    }

    pub fn key(&mut self, key: Key) {
        self.push_key(key, true);
        self.push_key(key, false);
    }

    pub fn key_down(&mut self, key: Key) {
        self.push_key(key, true);
    }

    pub fn key_up(&mut self, key: Key) {
        self.push_key(key, false);
    }

    fn push_key(&mut self, key: Key, pressed: bool) {
        self.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    }

    pub fn type_text(&mut self, text: &str) {
        self.events.push(egui::Event::Text(text.to_string()));
    }
}

struct LuaRunner {
    lua: Lua,
    thread: mlua::Thread,
    finished: bool,
}

/// Drives a script through the live application, one step at a time.
pub struct ScriptRunner {
    instructions: Vec<Instruction>,
    lua: Option<LuaRunner>,
    pc: usize,
    wait_until: Option<Instant>,
    wait_frames_remaining: u32,
    screenshot_pending: Option<String>,
    waiting_view_transition: bool,
    /// Prevents re-printing an instruction while waiting (e.g. for viewport layout).
    logged_pc: Option<usize>,
    pub verbose: bool,
    pub done: bool,
    pub error: Option<String>,
    pub should_quit: bool,
}

impl ScriptRunner {
    pub fn new(instructions: Vec<Instruction>) -> Self {
        Self::from_instructions(instructions)
    }

    pub fn from_instructions(instructions: Vec<Instruction>) -> Self {
        Self {
            instructions,
            lua: None,
            pc: 0,
            wait_until: None,
            wait_frames_remaining: 0,
            screenshot_pending: None,
            waiting_view_transition: false,
            logged_pc: None,
            verbose: true,
            done: false,
            error: None,
            should_quit: false,
        }
    }

    #[cfg(test)]
    pub fn from_lua_source(source: &str) -> Result<Self, ScriptError> {
        let lua = Lua::new();
        crate::lua_script::register_api(&lua).map_err(|e| ScriptError {
            message: e.to_string(),
        })?;
        let func = lua.load(source).into_function().map_err(|e| ScriptError {
            message: e.to_string(),
        })?;
        let thread = lua.create_thread(func).map_err(|e| ScriptError {
            message: e.to_string(),
        })?;
        let mut runner = Self::from_instructions(vec![]);
        runner.lua = Some(LuaRunner {
            lua,
            thread,
            finished: false,
        });
        Ok(runner)
    }

    pub fn from_file(path: &Path) -> Result<Self, ScriptError> {
        if path.extension().and_then(|e| e.to_str()) != Some("lua") {
            return Err(ScriptError {
                message: format!(
                    "scripts must use the .lua extension: {}",
                    path.display()
                ),
            });
        }
        let lua = Lua::new();
        let thread = load_script(&lua, path).map_err(|e| ScriptError {
            message: e.to_string(),
        })?;
        let mut runner = Self::from_instructions(vec![]);
        runner.lua = Some(LuaRunner {
            lua,
            thread,
            finished: false,
        });
        if runner.verbose {
            println!("Running script: {}", path.display());
            println!("---");
        }
        Ok(runner)
    }

    pub fn lua_done(&self) -> bool {
        self.lua.as_ref().is_some_and(|lua| lua.finished) || self.done
    }

    fn log_instruction(&mut self, instr: &Instruction) {
        if self.verbose && self.logged_pc != Some(self.pc) {
            println!("{}", instr.as_lua());
            self.logged_pc = Some(self.pc);
        }
    }

    pub fn is_waiting(&self) -> bool {
        self.wait_until.is_some()
            || self.wait_frames_remaining > 0
            || self.screenshot_pending.is_some()
            || self.waiting_view_transition
    }

    fn clear_instruction_wait(&mut self) {
        self.wait_until = None;
        self.pc += 1;
        self.logged_pc = None;
    }

    fn advance_after_wait(&mut self) {
        if self.lua.is_some() {
            self.logged_pc = None;
        } else {
            self.clear_instruction_wait();
        }
    }

    /// Advance the script. Returns true if a repaint should be requested.
    pub fn tick(
        &mut self,
        state: &mut AppState,
        synthetic: &mut SyntheticInput,
        viewport: Option<egui::Rect>,
        ctx: &egui::Context,
    ) -> bool {
        if self.lua.is_some() {
            return self.tick_lua_mode(state, synthetic, viewport, ctx);
        }
        self.tick_instructions(state, synthetic, viewport, ctx)
    }

    fn tick_lua_mode(
        &mut self,
        state: &mut AppState,
        synthetic: &mut SyntheticInput,
        viewport: Option<egui::Rect>,
        ctx: &egui::Context,
    ) -> bool {
        if self.done {
            return false;
        }

        if let Some(until) = self.wait_until {
            if Instant::now() < until {
                return true;
            }
            self.wait_until = None;
            self.advance_after_wait();
        }

        if self.wait_frames_remaining > 0 {
            self.wait_frames_remaining -= 1;
            if self.wait_frames_remaining == 0 {
                self.advance_after_wait();
            }
            return true;
        }

        if self.waiting_view_transition {
            if state.cam.is_transitioning() {
                return true;
            }
            self.waiting_view_transition = false;
            self.advance_after_wait();
        }

        if self.screenshot_pending.is_some() {
            return true;
        }

        let runner_ptr = self as *mut ScriptRunner;
        let lua_runner = self.lua.as_mut().unwrap();
        if lua_runner.finished {
            self.done = true;
            return false;
        }

        lua_runner.lua.set_app_data(ScriptTickData {
            runner: runner_ptr,
            state: state as *mut AppState,
            synthetic: synthetic as *mut SyntheticInput,
            viewport,
            ctx: ctx as *const egui::Context as *mut egui::Context,
        });

        match lua_runner.thread.resume::<()>(()) {
            Ok(_) => match lua_runner.thread.status() {
                mlua::ThreadStatus::Finished => {
                    lua_runner.finished = true;
                    self.done = true;
                    if self.verbose {
                        println!("---");
                        println!("Script complete.");
                    }
                    false
                }
                mlua::ThreadStatus::Resumable => true,
                mlua::ThreadStatus::Running => true,
                mlua::ThreadStatus::Error => {
                    self.error = Some("Lua thread error".to_string());
                    lua_runner.finished = true;
                    self.done = true;
                    false
                }
            },
            Err(e) => {
                self.error = Some(e.to_string());
                lua_runner.finished = true;
                self.done = true;
                false
            }
        }
    }

    pub(crate) fn tick_lua(
        &mut self,
        lua: &Lua,
        thread: &mlua::Thread,
        state: &mut AppState,
        synthetic: &mut SyntheticInput,
        viewport: Option<egui::Rect>,
        ctx: &egui::Context,
    ) -> bool {
        lua.set_app_data(ScriptTickData {
            runner: self as *mut ScriptRunner,
            state: state as *mut AppState,
            synthetic: synthetic as *mut SyntheticInput,
            viewport,
            ctx: ctx as *const egui::Context as *mut egui::Context,
        });
        match thread.resume::<()>(()) {
            Ok(_) => match thread.status() {
                mlua::ThreadStatus::Finished => {
                    self.done = true;
                    false
                }
                mlua::ThreadStatus::Resumable | mlua::ThreadStatus::Running => true,
                mlua::ThreadStatus::Error => {
                    self.error = Some("Lua thread error".to_string());
                    self.done = true;
                    false
                }
            },
            Err(e) => {
                self.error = Some(e.to_string());
                self.done = true;
                false
            }
        }
    }

    fn tick_instructions(
        &mut self,
        state: &mut AppState,
        synthetic: &mut SyntheticInput,
        viewport: Option<egui::Rect>,
        ctx: &egui::Context,
    ) -> bool {
        if self.done {
            return false;
        }

        if let Some(until) = self.wait_until {
            if Instant::now() < until {
                return true;
            }
            self.clear_instruction_wait();
        }

        if self.wait_frames_remaining > 0 {
            self.wait_frames_remaining -= 1;
            if self.wait_frames_remaining == 0 {
                self.clear_instruction_wait();
            }
            return true;
        }

        if self.waiting_view_transition {
            if state.cam.is_transitioning() {
                return true;
            }
            self.waiting_view_transition = false;
            self.clear_instruction_wait();
        }

        if self.screenshot_pending.is_some() {
            return true;
        }

        while self.pc < self.instructions.len() {
            let instr = self.instructions[self.pc].clone();
            self.log_instruction(&instr);
            match self.execute_instruction(instr, state, synthetic, viewport, ctx) {
                StepResult::Continue => {
                    self.pc += 1;
                }
                StepResult::Wait => return true,
                StepResult::Done => {
                    self.done = true;
                    return false;
                }
            }
        }

        self.done = true;
        if self.verbose {
            println!("---");
            println!("Script complete.");
        }
        false
    }

    pub(crate) fn execute_instruction(
        &mut self,
        instr: Instruction,
        state: &mut AppState,
        synthetic: &mut SyntheticInput,
        viewport: Option<egui::Rect>,
        ctx: &egui::Context,
    ) -> StepResult {
        let result = self.execute_one(instr, state, synthetic, viewport, ctx);
        if self.should_quit {
            if let Some(lua_runner) = self.lua.as_mut() {
                lua_runner.finished = true;
            }
            self.done = true;
            return StepResult::Done;
        }
        result
    }

    /// Called when egui delivers a screenshot response for a pending request.
    pub fn on_screenshot(&mut self, image: &egui::ColorImage) -> Result<(), String> {
        let Some(path) = self.screenshot_pending.take() else {
            return Ok(());
        };
        save_screenshot(&path, image)?;
        if self.lua.is_none() {
            self.pc += 1;
        }
        Ok(())
    }
}

pub(crate) enum StepResult {
    Continue,
    Wait,
    Done,
}

impl ScriptRunner {
    fn ground_pointer(
        synthetic: &mut SyntheticInput,
        state: &AppState,
        viewport: Option<egui::Rect>,
        x: f32,
        y: f32,
        click: bool,
    ) {
        let Some(vp) = viewport else { return };
        let world = Vec3::new(x, y, 0.0);
        let mat = state.cam.view_proj(vp);
        let Some(screen) = state.cam.project(world, vp, &mat) else {
            return;
        };
        let local_x = screen.x - vp.min.x;
        let local_y = screen.y - vp.min.y;
        if click {
            synthetic.click(vp, local_x, local_y);
        } else {
            synthetic.move_to(vp, local_x, local_y);
        }
    }

    fn execute_one(
        &mut self,
        instr: Instruction,
        state: &mut AppState,
        synthetic: &mut SyntheticInput,
        viewport: Option<egui::Rect>,
        ctx: &egui::Context,
    ) -> StepResult {
        match instr {
            Instruction::New => {
                state.apply(Action::NewDocument);
                StepResult::Continue
            }
            Instruction::Open(path) => {
                state.apply(Action::Open { path });
                StepResult::Continue
            }
            Instruction::Save(path) => {
                state.apply(Action::Save { path });
                StepResult::Continue
            }
            Instruction::Clear => {
                state.apply(Action::Clear);
                StepResult::Continue
            }
            Instruction::Undo => {
                state.apply(Action::UndoLast);
                StepResult::Continue
            }
            Instruction::Tool(tool) => {
                state.apply(Action::SetTool(tool));
                StepResult::Continue
            }
            Instruction::BeginSketch { face } => {
                state.apply(Action::BeginSketch {
                    face,
                    viewport: viewport,
                });
                StepResult::Continue
            }
            Instruction::OpenSketch { sketch } => {
                state.apply(Action::OpenSketch {
                    sketch,
                    viewport: viewport,
                });
                StepResult::Continue
            }
            Instruction::ExitSketch => {
                state.apply(Action::ExitSketch);
                StepResult::Continue
            }
            Instruction::SetElementVisible { element, visible } => {
                match visible {
                    Some(v) => state.apply(Action::SetElementVisible { element, visible: v }),
                    None => state.apply(Action::ToggleElementVisibility(element)),
                };
                StepResult::Continue
            }
            Instruction::SelectSceneElement { element, additive } => {
                state.apply(Action::ClickSceneElement { element, additive });
                StepResult::Continue
            }
            Instruction::ClearSceneSelection => {
                state.apply(Action::ClearSceneSelection);
                StepResult::Continue
            }
            Instruction::SetShapeConstruction { element, construction } => {
                let _ = state.apply(Action::SetShapeConstruction {
                    element,
                    construction,
                });
                StepResult::Continue
            }
            Instruction::ApplyConstruction { construction } => {
                let _ = state.apply(Action::ApplyConstruction { construction });
                StepResult::Continue
            }
            Instruction::ToggleConstruction => {
                let _ = state.apply(Action::ToggleConstruction);
                StepResult::Continue
            }
            Instruction::SetElementName { element, name } => {
                state.apply(Action::CommitElementName { element, name });
                StepResult::Continue
            }
            Instruction::FocusElementName => {
                state.apply(Action::FocusElementName);
                StepResult::Continue
            }
            Instruction::SetDim { axis, value } => {
                let _ = state.apply(Action::SetRectDimension { axis, value });
                StepResult::Continue
            }
            Instruction::SetDimLabelOffset { axis, offset } => {
                if let Some(session) = state.sketch_session {
                    if let Some(target) =
                        dim_label_target_in_sketch(&state.doc, session.sketch, axis)
                    {
                        let _ = state.apply(Action::SetDimLabelOffset { target, offset });
                    }
                }
                StepResult::Continue
            }
            Instruction::BeginEditCommittedDim { axis } => {
                if let Some(session) = state.sketch_session {
                    if let Some(target) =
                        dim_label_target_in_sketch(&state.doc, session.sketch, axis)
                    {
                        let _ = state.apply(Action::BeginEditCommittedDim { target });
                    }
                }
                StepResult::Continue
            }
            Instruction::CommitCommittedDim => {
                let _ = state.apply(Action::CommitCommittedDim);
                StepResult::Continue
            }
            Instruction::AddDistanceConstraint { target, expression } => {
                if let Some(session) = state.sketch_session {
                    let _ = add_distance_constraint(
                        &mut state.doc,
                        session.sketch,
                        target,
                        expression,
                    );
                }
                StepResult::Continue
            }
            Instruction::AddGeometricConstraint(kind) => {
                let _ = state.apply(Action::AddGeometricConstraint(kind));
                StepResult::Continue
            }
            Instruction::ApplyConstraintShortcut(index) => {
                let _ = state.apply(Action::ApplyConstraintShortcut(index));
                StepResult::Continue
            }
            Instruction::DragVertex { point, u, v } => {
                let _ = state.apply(Action::DragVertex { point, u, v });
                StepResult::Continue
            }
            Instruction::DragLineSegment {
                target,
                anchor_u,
                anchor_v,
                u,
                v,
            } => {
                let _ = state.apply(Action::BeginLineDrag {
                    target,
                    anchor_u,
                    anchor_v,
                });
                let _ = state.apply(Action::DragLine { u, v });
                let _ = state.apply(Action::EndLineDrag);
                StepResult::Continue
            }
            Instruction::SetLineLength { value } => {
                let _ = state.apply(Action::SetLineLength { value });
                StepResult::Continue
            }
            Instruction::SetCircleDiameter { value } => {
                let _ = state.apply(Action::SetCircleDiameter { value });
                StepResult::Continue
            }
            Instruction::BeginEditConstructionPlane { index } => {
                state.apply(Action::BeginEditConstructionPlane { index });
                StepResult::Continue
            }
            Instruction::CommitConstructionPlane => {
                state.apply(Action::CommitConstructionPlane);
                StepResult::Continue
            }
            Instruction::SetPlaneOffset { value } => {
                let _ = state.apply(Action::SetPlaneOffset { value });
                StepResult::Continue
            }
            Instruction::SetPlaneAngle { value } => {
                let _ = state.apply(Action::SetPlaneAngle { value });
                StepResult::Continue
            }
            Instruction::FocusDim(axis) => {
                let _ = state.apply(Action::FocusRectDimension { axis });
                StepResult::Continue
            }
            Instruction::FocusLineLength => {
                let _ = state.apply(Action::FocusLineLength);
                StepResult::Continue
            }
            Instruction::FocusCircleDiameter => {
                let _ = state.apply(Action::FocusCircleDiameter);
                StepResult::Continue
            }
            Instruction::FocusPlaneDim(dim) => {
                let _ = state.apply(Action::FocusPlaneDim { dim });
                StepResult::Continue
            }
            Instruction::Orbit { dx, dy } => {
                state.apply(Action::OrbitCamera { delta: (dx, dy) });
                StepResult::Continue
            }
            Instruction::Pan { dx, dy } => {
                let h = viewport.map(|r| r.height()).unwrap_or(640.0);
                state.apply(Action::PanCamera {
                    delta: (dx, dy),
                    viewport_height: h,
                });
                StepResult::Continue
            }
            Instruction::Zoom { scroll } => {
                let Some(vp) = viewport else {
                    return StepResult::Wait;
                };
                state.apply(Action::ZoomCamera {
                    scroll,
                    focal: vp.center(),
                    viewport: vp,
                });
                StepResult::Continue
            }
            Instruction::View(view) => {
                state.apply(Action::SetStandardView(view));
                self.waiting_view_transition = true;
                StepResult::Wait
            }
            Instruction::ViewEdge(edge) => {
                state.apply(Action::SetViewEdge(edge));
                self.waiting_view_transition = true;
                StepResult::Wait
            }
            Instruction::ViewCorner(corner) => {
                state.apply(Action::SetViewCorner(corner));
                self.waiting_view_transition = true;
                StepResult::Wait
            }
            Instruction::ViewHome => {
                state.apply(Action::ViewHome);
                self.waiting_view_transition = true;
                StepResult::Wait
            }
            Instruction::SetHomeView => {
                state.apply(Action::SetHomeView);
                StepResult::Continue
            }
            Instruction::ProjectionMode(mode) => {
                state.apply(Action::SetProjectionMode(mode));
                StepResult::Continue
            }
            Instruction::ToggleProjectionMode => {
                state.apply(Action::ToggleProjectionMode);
                StepResult::Continue
            }
            Instruction::SetPane { pane, visible } => {
                match visible {
                    Some(v) => state.apply(Action::SetPaneVisible { pane, visible: v }),
                    None => state.apply(Action::TogglePane(pane)),
                };
                StepResult::Continue
            }
            Instruction::AddParameter { name, expression } => {
                state.apply(Action::AddParameter { name, expression });
                StepResult::Continue
            }
            Instruction::SetParameterName { index, name } => {
                state.apply(Action::CommitParameterName { index, name });
                StepResult::Continue
            }
            Instruction::SetParameterExpression { index, expression } => {
                state.apply(Action::CommitParameterExpression { index, expression });
                StepResult::Continue
            }
            Instruction::DeleteParameter { index } => {
                state.apply(Action::DeleteParameter { index });
                StepResult::Continue
            }
            Instruction::DeleteSelection => {
                state.apply(Action::DeleteSelection);
                StepResult::Continue
            }
            Instruction::SetCommandPalette { open } => {
                match open {
                    Some(true) => state.apply(Action::SetCommandPaletteOpen { open: true }),
                    Some(false) => state.apply(Action::SetCommandPaletteOpen { open: false }),
                    None => state.apply(Action::ToggleCommandPalette),
                };
                StepResult::Continue
            }
            Instruction::RunPaletteCommand { query } => {
                let commands = commands_for_state(state);
                if let Some(cmd) = best_match(&query, &commands) {
                    match cmd.outcome() {
                        PaletteOutcome::Action(action) => {
                            state.apply(action);
                        }
                        PaletteOutcome::OpenFile | PaletteOutcome::SaveFile
                        | PaletteOutcome::SaveFileAs => {
                            state.status =
                                "Palette file commands require the GUI".to_string();
                        }
                    }
                } else {
                    state.status = format!("No palette command matches '{query}'");
                }
                StepResult::Continue
            }

            Instruction::Move { x, y } => {
                let Some(vp) = viewport else {
                    return StepResult::Wait;
                };
                synthetic.move_to(vp, x, y);
                StepResult::Continue
            }
            Instruction::Click { x, y } => {
                let Some(vp) = viewport else {
                    return StepResult::Wait;
                };
                synthetic.click(vp, x, y);
                StepResult::Continue
            }
            Instruction::MoveGround { x, y } => {
                if viewport.is_none() {
                    return StepResult::Wait;
                }
                Self::ground_pointer(synthetic, state, viewport, x, y, false);
                StepResult::Continue
            }
            Instruction::ClickGround { x, y } => {
                if viewport.is_none() {
                    return StepResult::Wait;
                }
                Self::ground_pointer(synthetic, state, viewport, x, y, true);
                StepResult::Continue
            }
            Instruction::Drag { x0, y0, x1, y1 } => {
                let Some(vp) = viewport else {
                    return StepResult::Wait;
                };
                synthetic.drag(vp, x0, y0, x1, y1);
                StepResult::Continue
            }
            Instruction::RightDrag { dx, dy } => {
                let Some(vp) = viewport else {
                    return StepResult::Wait;
                };
                synthetic.right_drag(vp, dx, dy, false);
                StepResult::Continue
            }
            Instruction::RightDragShift { dx, dy } => {
                let Some(vp) = viewport else {
                    return StepResult::Wait;
                };
                synthetic.right_drag(vp, dx, dy, true);
                StepResult::Continue
            }
            Instruction::Scroll { delta } => {
                synthetic.scroll(delta);
                StepResult::Continue
            }
            Instruction::Key(key) => {
                synthetic.key(key);
                StepResult::Continue
            }
            Instruction::KeyDown(key) => {
                synthetic.key_down(key);
                StepResult::Continue
            }
            Instruction::KeyUp(key) => {
                synthetic.key_up(key);
                StepResult::Continue
            }
            Instruction::Type(text) => {
                synthetic.type_text(&text);
                StepResult::Continue
            }

            Instruction::WaitMs(ms) => {
                self.wait_until = Some(Instant::now() + Duration::from_millis(ms));
                StepResult::Wait
            }
            Instruction::WaitFrames(n) => {
                if n == 0 {
                    StepResult::Continue
                } else {
                    self.wait_frames_remaining = n;
                    StepResult::Wait
                }
            }
            Instruction::Screenshot(path) => {
                self.screenshot_pending = Some(path);
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
                StepResult::Wait
            }
            Instruction::Quit => {
                self.should_quit = true;
                StepResult::Done
            }
        }
    }
}

/// Save an egui [`egui::ColorImage`] to a PNG file.
pub fn save_screenshot(path: &str, image: &egui::ColorImage) -> Result<(), String> {
    let width = image.width() as u32;
    let height = image.height() as u32;
    let rgba: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
        .collect();
    image::save_buffer(path, &rgba, width, height, image::ColorType::Rgba8)
        .map_err(|e| format!("failed to save screenshot to {path}: {e}"))
}

/// CLI launch options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptOptions {
    pub script_path: Option<String>,
    pub document_path: Option<String>,
    pub exit_on_complete: bool,
    pub show_commands: bool,
}

/// Parsed command-line outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliOutcome {
    Help,
    Run(ScriptOptions),
}

/// Print usage information to stdout.
pub fn print_usage() {
    println!(
        "\
LE3 — parametric CAD prototype

Usage:
  le3 [options] [script.lua]

Options:
  --script <path>       Run a Lua script
  --exit, --exit-on-complete
                        Exit after startup, or after the script finishes
  --show-commands       Print each user action as a script line on stdout
  -h, --help            Show this help and exit

Examples:
  le3
  le3 --exit
  le3 drawing.le3 --exit
  le3 --script demo.lua
  le3 demo.lua --exit
"
    );
}

/// Parse command-line arguments.
pub fn parse_cli(args: impl IntoIterator<Item = impl AsRef<str>>) -> CliOutcome {
    let args: Vec<String> = args
        .into_iter()
        .map(|a| a.as_ref().to_string())
        .collect();
    if args
        .iter()
        .any(|arg| arg == "--help" || arg == "-h")
    {
        return CliOutcome::Help;
    }
    CliOutcome::Run(parse_args_from_vec(&args))
}

/// Parse command-line arguments for script mode (without handling `--help`).
#[allow(dead_code)] // public API; exercised by unit tests
pub fn parse_args(args: impl IntoIterator<Item = impl AsRef<str>>) -> ScriptOptions {
    let args: Vec<String> = args
        .into_iter()
        .map(|a| a.as_ref().to_string())
        .collect();
    parse_args_from_vec(&args)
}

fn parse_args_from_vec(args: &[String]) -> ScriptOptions {
    let mut opts = ScriptOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--script" => {
                i += 1;
                if i < args.len() {
                    opts.script_path = Some(args[i].clone());
                }
            }
            "--exit" | "--exit-on-complete" => {
                opts.exit_on_complete = true;
            }
            "--show-commands" => {
                opts.show_commands = true;
            }
            arg if !arg.starts_with('-') => {
                if opts.script_path.is_none()
                    && (arg.ends_with(".lua")
                        || Path::new(arg).extension().is_some_and(|e| e == "lua"))
                {
                    opts.script_path = Some(arg.to_string());
                } else if opts.document_path.is_none()
                    && (arg.ends_with(".le3")
                        || Path::new(arg).extension().is_some_and(|e| e == "le3"))
                {
                    opts.document_path = Some(arg.to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    opts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ConstraintLine;

    #[test]
    fn parse_key_names() {
        assert_eq!(parse_key("enter").unwrap(), Key::Enter);
        assert_eq!(parse_key("ESC").unwrap(), Key::Escape);
        assert!(parse_key("notakey").is_err());
    }

    #[test]
    fn parse_cli_help_flags() {
        assert_eq!(parse_cli(["le3", "--help"]), CliOutcome::Help);
        assert_eq!(parse_cli(["le3", "-h"]), CliOutcome::Help);
    }

    #[test]
    fn parse_show_commands_flag() {
        let opts = parse_args(["le3", "--show-commands"]);
        assert!(opts.show_commands);
    }

    #[test]
    fn instruction_from_action_maps_tool_changes() {
        let state = AppState::default();
        let instruction =
            instruction_from_action(&Action::SetTool(Tool::Rectangle), &state.doc).unwrap();
        assert_eq!(instruction, Instruction::Tool(Tool::Rectangle));
    }

    #[test]
    fn parse_cli_run_delegates_to_script_options() {
        assert_eq!(
            parse_cli(["le3", "--script", "test.lua", "--exit"]),
            CliOutcome::Run(ScriptOptions {
                script_path: Some("test.lua".to_string()),
                document_path: None,
                exit_on_complete: true,
                show_commands: false,
            })
        );
    }

    #[test]
    fn parse_args_finds_script_flag() {
        let opts = parse_args(["le3", "--script", "test.lua", "--exit"]);
        assert_eq!(opts.script_path.as_deref(), Some("test.lua"));
        assert!(opts.exit_on_complete);
    }

    #[test]
    fn parse_args_finds_positional_script() {
        let opts = parse_args(["le3", "demo.lua"]);
        assert_eq!(opts.script_path.as_deref(), Some("demo.lua"));
    }

    #[test]
    fn parse_args_finds_positional_document_and_exit() {
        let opts = parse_args(["le3", "/tmp/test.le3", "--exit"]);
        assert_eq!(opts.document_path.as_deref(), Some("/tmp/test.le3"));
        assert!(opts.exit_on_complete);
        assert!(opts.script_path.is_none());
    }

    #[test]
    fn parse_args_exit_without_paths_exits_after_startup() {
        let opts = parse_args(["le3", "--exit"]);
        assert!(opts.exit_on_complete);
        assert!(opts.script_path.is_none());
        assert!(opts.document_path.is_none());
    }

    #[test]
    fn instruction_as_lua_formats_click() {
        let ins = Instruction::Click { x: 100.0, y: 200.0 };
        assert_eq!(ins.as_lua(), "paramcad.click(100, 200)");
    }

    #[test]
    fn wait_frames_advances_to_next_instruction() {
        let mut runner =
            ScriptRunner::new(vec![Instruction::WaitFrames(2), Instruction::Clear]);
        runner.verbose = false;
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state
            .doc
            .rects
            .push(crate::model::Rect::from_local_corners(sketch, 0., 0., 1., 1.));
        let mut synthetic = SyntheticInput::default();
        let ctx = egui::Context::default();
        let vp = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(960.0, 560.0));

        assert!(runner.tick(&mut state, &mut synthetic, Some(vp), &ctx));
        assert_eq!(runner.pc, 0);
        assert_eq!(runner.wait_frames_remaining, 2);

        assert!(runner.tick(&mut state, &mut synthetic, Some(vp), &ctx));
        assert_eq!(runner.pc, 0);
        assert_eq!(runner.wait_frames_remaining, 1);

        assert!(runner.tick(&mut state, &mut synthetic, Some(vp), &ctx));
        assert_eq!(runner.pc, 1);
        assert_eq!(runner.wait_frames_remaining, 0);

        runner.tick(&mut state, &mut synthetic, Some(vp), &ctx);
        assert!(state.doc.rects.is_empty());
        assert!(runner.done);
    }

    #[test]
    fn script_drag_line_translates_segment() {
        let mut runner = ScriptRunner::new(vec![
            Instruction::Tool(Tool::Line),
            Instruction::Tool(Tool::Select),
            Instruction::DragLineSegment {
                target: ConstraintLine::Line(0),
                anchor_u: 0.0,
                anchor_v: 0.0,
                u: 4.0,
                v: 0.0,
            },
        ]);
        runner.verbose = false;
        let mut state = AppState::default();
        let mut synthetic = SyntheticInput::default();
        state.apply(crate::actions::Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        state.creating_line = Some(crate::actions::CreatingLine {
            origin: glam::Vec3::ZERO,
            text: String::new(),
            last_mouse: glam::Vec3::new(10.0, 0.0, 0.0),
            user_edited: false,
            pending_focus: false,
            construction: false,
        });
        state.apply(crate::actions::Action::CommitLine);
        while !runner.done {
            runner.tick(
                &mut state,
                &mut synthetic,
                None,
                &egui::Context::default(),
            );
        }
        let line = &state.doc.lines[0];
        assert!((line.x0 - 4.0).abs() < 1e-2);
        assert!((line.y0).abs() < 1e-2);
        assert!((line.x1 - 14.0).abs() < 1e-2);
    }

    #[test]
    fn script_select_and_set_construction() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.rects.push(crate::model::Rect::from_local_corners(
            sketch, 0.0, 0.0, 10.0, 5.0,
        ));
        let mut runner = ScriptRunner::new(vec![
            Instruction::SelectSceneElement {
                element: SceneElement::RectEdge(0, RectEdge::Bottom),
                additive: false,
            },
            Instruction::SetShapeConstruction {
                element: SceneElement::RectEdge(0, RectEdge::Bottom),
                construction: true,
            },
            Instruction::ClearSceneSelection,
        ]);
        runner.verbose = false;
        let mut synthetic = SyntheticInput::default();
        let ctx = egui::Context::default();
        while !runner.done {
            runner.tick(&mut state, &mut synthetic, None, &ctx);
        }
        assert!(!state.scene_selection.is_selected(SceneElement::RectEdge(
            0,
            RectEdge::Bottom
        )));
        assert!(state.doc.rects[0].edge_construction(RectEdge::Bottom));
        assert!(!state.doc.rects[0].edge_construction(RectEdge::Top));
    }

    #[test]
    fn script_rectangle_dimension_uses_parameter_and_updates() {
        let mut runner = ScriptRunner::new(vec![
            Instruction::AddParameter {
                name: "A".into(),
                expression: "10mm".into(),
            },
            Instruction::Tool(Tool::Rectangle),
            Instruction::SetDim {
                axis: RectAxis::Width,
                value: "A".into(),
            },
            Instruction::SetDim {
                axis: RectAxis::Height,
                value: "5".into(),
            },
        ]);
        runner.verbose = false;
        let mut state = AppState::default();
        let mut synthetic = SyntheticInput::default();
        state.apply(crate::actions::Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        state.creating_rect = Some(crate::actions::CreatingRect {
            origin: glam::Vec3::ZERO,
            texts: [String::new(), String::new()],
            last_mouse: glam::Vec3::new(100.0, 5.0, 0.0),
            focused: 0,
            user_edited: [false, false],
            pending_focus: false,
            construction: false,
        });

        while !runner.done {
            runner.tick(
                &mut state,
                &mut synthetic,
                None,
                &egui::Context::default(),
            );
        }

        state.apply(crate::actions::Action::CommitRectangle);
        assert_eq!(state.doc.rects.len(), 1);
        assert!((state.doc.rects[0].w - 10.0).abs() < 1e-3);
        assert_eq!(state.doc.rects[0].width_expr.as_deref(), Some("A"));

        state.apply(crate::actions::Action::CommitParameterExpression {
            index: 0,
            expression: "20mm".to_string(),
        });
        assert!((state.doc.rects[0].w - 20.0).abs() < 1e-3);
    }

    #[test]
    fn script_palette_run_sets_top_view() {
        let mut runner = ScriptRunner::new(vec![Instruction::RunPaletteCommand {
            query: "view top".into(),
        }]);
        runner.verbose = false;
        let mut state = AppState::default();
        let mut synthetic = SyntheticInput::default();
        while !runner.done {
            runner.tick(
                &mut state,
                &mut synthetic,
                None,
                &egui::Context::default(),
            );
        }
        assert!(state.cam.is_transitioning());
    }

    #[test]
    fn script_delete_selection_tombstones_line() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(crate::model::FaceId::default());
        state.doc.lines.push(crate::model::Line::from_local_endpoints(
            sketch, 0.0, 0.0, 5.0, 0.0,
        ));
        state.doc.shape_order.push(crate::model::ShapeKind::Line);
        let mut runner = ScriptRunner::new(vec![
            Instruction::SelectSceneElement {
                element: SceneElement::Line(0),
                additive: false,
            },
            Instruction::DeleteSelection,
        ]);
        runner.verbose = false;
        let mut synthetic = SyntheticInput::default();
        let ctx = egui::Context::default();
        while !runner.done {
            runner.tick(&mut state, &mut synthetic, None, &ctx);
        }
        assert!(state.doc.lines[0].deleted);
    }

    #[test]
    fn script_adds_and_renames_parameters() {
        let mut runner = ScriptRunner::new(vec![
            Instruction::AddParameter {
                name: "A".into(),
                expression: "5mm".into(),
            },
            Instruction::AddParameter {
                name: "B".into(),
                expression: "A+5in".into(),
            },
            Instruction::SetParameterName {
                index: 0,
                name: "Len".into(),
            },
        ]);
        runner.verbose = false;
        let mut state = AppState::default();
        let mut synthetic = SyntheticInput::default();
        while !runner.done {
            runner.tick(
                &mut state,
                &mut synthetic,
                None,
                &egui::Context::default(),
            );
        }
        assert_eq!(state.doc.parameters.len(), 2);
        assert_eq!(state.doc.parameters[0].name, "Len");
        assert_eq!(state.doc.parameters[1].expression, "Len+5in");
    }

    #[test]
    fn view_command_waits_until_transition_finishes() {
        let mut runner = ScriptRunner::new(vec![
            Instruction::View(StandardView::Front),
            Instruction::Clear,
        ]);
        runner.verbose = false;
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state
            .doc
            .rects
            .push(crate::model::Rect::from_local_corners(sketch, 0., 0., 1., 1.));
        let mut synthetic = SyntheticInput::default();
        let ctx = egui::Context::default();
        let vp = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(960.0, 560.0));

        assert!(runner.tick(&mut state, &mut synthetic, Some(vp), &ctx));
        assert_eq!(runner.pc, 0);
        assert!(runner.waiting_view_transition);
        assert!(state.cam.is_transitioning());

        let mut blocked_while_animating = false;
        for _ in 0..100 {
            if runner.pc == 0 && state.cam.is_transitioning() {
                blocked_while_animating = true;
            }
            if state.cam.is_transitioning() {
                state.cam.tick_transition(0.05);
            }
            runner.tick(&mut state, &mut synthetic, Some(vp), &ctx);
            if runner.done {
                break;
            }
        }

        assert!(
            blocked_while_animating,
            "script should block while the view animates"
        );
        assert!(state.doc.rects.is_empty());
        assert!(runner.done);
    }

    #[test]
    fn script_edit_committed_dim_updates_rectangle_width() {
        let mut state = AppState::default();
        state.apply(crate::actions::Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        state.creating_rect = Some(crate::actions::CreatingRect {
            origin: glam::Vec3::ZERO,
            texts: ["10".to_string(), "5".to_string()],
            focused: 0,
            last_mouse: glam::Vec3::new(10.0, 5.0, 0.0),
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(crate::actions::Action::CommitRectangle);
        let mut runner = ScriptRunner::new(vec![
            Instruction::BeginEditCommittedDim {
                axis: DimLabelAxis::Width,
            },
            Instruction::SetDim {
                axis: RectAxis::Width,
                value: "25mm".into(),
            },
            Instruction::CommitCommittedDim,
        ]);
        runner.verbose = false;
        let mut synthetic = SyntheticInput::default();
        while !runner.done {
            runner.tick(
                &mut state,
                &mut synthetic,
                None,
                &egui::Context::default(),
            );
        }
        assert!((state.doc.rects[0].w - 25.0).abs() < 1e-3);
        assert_eq!(state.doc.rects[0].width_expr.as_deref(), Some("25mm"));
    }

    #[test]
    fn script_set_dim_label_offset_updates_rectangle() {
        let mut state = AppState::default();
        let mut synthetic = SyntheticInput::default();
        state.apply(crate::actions::Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        state.creating_rect = Some(crate::actions::CreatingRect {
            origin: glam::Vec3::ZERO,
            texts: ["10".to_string(), "5".to_string()],
            last_mouse: glam::Vec3::new(10.0, 5.0, 0.0),
            focused: 0,
            user_edited: [true, true],
            pending_focus: false,
            construction: false,
        });
        state.apply(crate::actions::Action::CommitRectangle);
        let mut runner = ScriptRunner::new(vec![Instruction::SetDimLabelOffset {
            axis: DimLabelAxis::Width,
            offset: 60.0,
        }]);
        while !runner.done {
            runner.tick(
                &mut state,
                &mut synthetic,
                None,
                &egui::Context::default(),
            );
        }
        assert_eq!(state.doc.rects[0].width_dim_offset, Some(60.0));
    }

    #[test]
    fn script_set_dim_commit_displays_computed_mm_not_expression() {
        use crate::value::format_length_display;

        let mut runner = ScriptRunner::new(vec![
            Instruction::Tool(Tool::Rectangle),
            Instruction::SetDim {
                axis: RectAxis::Width,
                value: "2in".into(),
            },
            Instruction::SetDim {
                axis: RectAxis::Height,
                value: "5mm".into(),
            },
        ]);
        runner.verbose = false;
        let mut state = AppState::default();
        let mut synthetic = SyntheticInput::default();
        state.apply(crate::actions::Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        state.creating_rect = Some(crate::actions::CreatingRect {
            origin: glam::Vec3::ZERO,
            texts: [String::new(), String::new()],
            last_mouse: glam::Vec3::new(100.0, 100.0, 0.0),
            focused: 0,
            user_edited: [false, false],
            pending_focus: false,
            construction: false,
        });

        while !runner.done {
            runner.tick(
                &mut state,
                &mut synthetic,
                None,
                &egui::Context::default(),
            );
        }

        state.apply(crate::actions::Action::CommitRectangle);
        let rect = &state.doc.rects[0];
        assert!(rect.width_locked);
        assert!(rect.height_locked);
        assert!((rect.w - 50.8).abs() < 1e-2);
        assert_eq!(format_length_display(rect.w), "50.8 mm");
        assert_eq!(format_length_display(rect.h), "5.0 mm");
    }

    #[test]
    fn runner_set_dim_expression_evaluates_length() {
        let mut runner = ScriptRunner::new(vec![
            Instruction::Tool(Tool::Line),
            Instruction::SetLineLength {
                value: "2in + 5mm / 2".into(),
            },
        ]);
        runner.verbose = false;
        let mut state = AppState::default();
        let mut synthetic = SyntheticInput::default();
        state.apply(crate::actions::Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        state.creating_line = Some(crate::actions::CreatingLine {
            origin: glam::Vec3::ZERO,
            text: String::new(),
            last_mouse: glam::Vec3::new(10.0, 10.0, 0.0),
            user_edited: false,
            pending_focus: false,
            construction: false,
        });

        while !runner.done {
            runner.tick(
                &mut state,
                &mut synthetic,
                None,
                &egui::Context::default(),
            );
        }

        let cl = state.creating_line.as_ref().unwrap();
        assert_eq!(cl.text, "2in + 5mm / 2");
        let sketch = state.sketch_session.unwrap().sketch;
        let frame = crate::face::sketch_geometry_frame(&state.doc, sketch).unwrap();
        let end = cl.end_point(&frame, &state.doc);
        let (u0, v0) = crate::face::world_to_local(&frame, cl.origin);
        let (u1, v1) = crate::face::world_to_local(&frame, end);
        let len = crate::model::Line::from_local_endpoints(sketch, u0, v0, u1, v1).length();
        assert!((len - 53.3).abs() < 1e-2);
    }

    #[test]
    fn runner_executes_headless_actions() {
        let mut runner = ScriptRunner::new(vec![
            Instruction::New,
            Instruction::BeginSketch {
                face: FaceId::ConstructionPlane(0),
            },
            Instruction::Tool(Tool::Rectangle),
            Instruction::SetDim {
                axis: RectAxis::Width,
                value: "50".into(),
            },
            Instruction::Orbit { dx: 10.0, dy: 5.0 },
            Instruction::Clear,
        ]);
        runner.verbose = false;
        let mut state = AppState::default();
        let mut synthetic = SyntheticInput::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state
            .doc
            .rects
            .push(crate::model::Rect::from_local_corners(sketch, 0., 0., 1., 1.));

        while !runner.done {
            runner.tick(
                &mut state,
                &mut synthetic,
                Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 40.0),
                    egui::vec2(960.0, 560.0),
                )),
                &egui::Context::default(),
            );
        }

        assert!(state.doc.rects.is_empty());
        assert_eq!(state.tool, Tool::Rectangle);
        assert!(runner.error.is_none());
    }
}