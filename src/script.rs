//! Lua script runner and internal instruction dispatch (SPEC §8).
//!
//! Scripts are `.lua` files that call the global `bearcad` API. They drive the
//! live UI via synthetic pointer/keyboard events and headless actions.

use crate::actions::{
    dim_label_target_in_sketch, Action, AppState, DimLabelAxis, Pane, RectAxis, Tool,
};
use crate::command_palette::{best_match, commands_for_state, PaletteOutcome};
use crate::constraints::add_distance_constraint;
use crate::hierarchy::SceneElement;
use crate::model::{
    ConstraintLine, ConstraintPoint, DistanceTarget, ExtrudeFace, FaceId, SketchId,
    VertexTreatmentKind,
};
use crate::value::{AngleUnit, LengthUnit};

use crate::construction::PlaneDim;
use crate::camera::{ProjectionMode, ShadingMode, StandardView};
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
    /// Export bodies to an STL file at `path`; `body` names a single body (`None` = all).
    ExportStl { path: String, body: Option<String> },
    /// Export bodies to a STEP file at `path`; `body` names a single body (`None` = all).
    ExportStep { path: String, body: Option<String> },
    /// Import an STL file at `path` as a new body (#70).
    ImportStl { path: String },
    /// Import a STEP file at `path` as a new body (#71).
    ImportStep { path: String },
    Clear,
    Undo,
    Tool(Tool),
    BeginSketch { face: FaceId },
    OpenSketch { sketch: SketchId },
    ExitSketch,
    /// Create a rectangle directly in the active sketch (face-local mm) with locked dimensions.
    CreateRect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    /// Create a line directly in the active sketch (face-local mm endpoints) with a locked length.
    /// `bezier` (#54) makes it a curve: `[handle near (x0,y0), handle near (x1,y1)]`.
    CreateLine {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        bezier: Option<[(f32, f32); 2]>,
    },
    /// Create a circle directly in the active sketch (face-local mm) with a locked diameter.
    CreateCircle {
        cx: f32,
        cy: f32,
        r: f32,
    },
    /// Extrude coplanar sketch faces into a solid.
    Extrude {
        sketch: SketchId,
        faces: Vec<crate::model::ExtrudeFace>,
        distance: f32,
        /// How the extrusion attaches to bodies (#32/#35): new body, add to the extruded
        /// face's body, or cut it from that body.
        body: crate::actions::ExtrudeBodyChoice,
    },
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
    /// Set the document-wide default length/angle units (#52).
    SetDocumentUnits { length: LengthUnit, angle: AngleUnit },
    /// Set (or clear, via `None`) a per-sketch length/angle unit override (#52).
    SetSketchUnits {
        sketch: SketchId,
        length: Option<LengthUnit>,
        angle: Option<AngleUnit>,
    },
    SetDim { axis: RectAxis, value: String },
    SetDimLabelOffset { axis: DimLabelAxis, offset: f32 },
    BeginEditCommittedDim { axis: DimLabelAxis },
    CommitCommittedDim,
    AddDistanceConstraint {
        target: DistanceTarget,
        expression: String,
    },
    AddGeometricConstraint(crate::geometric_constraints::GeometricConstraintType),
    ApplyConstraintShortcut(char),
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
    /// Chamfer or fillet a sketch vertex where exactly two plain lines meet (#37/#38):
    /// truncates both lines back from the vertex and bridges them with a new line (straight
    /// for a chamfer, single-cubic-bezier arc for a fillet). `amount` is the chamfer distance
    /// or fillet radius depending on `kind`.
    VertexTreatment {
        point: ConstraintPoint,
        kind: VertexTreatmentKind,
        amount: f32,
    },
    /// Chamfer or fillet an analytic edge of an extrusion's 3D solid (#77) — a mesh-bevel
    /// approximation scoped to the vertical and side/cap edges of a `Rect`/`Polygon`-profiled
    /// extrusion (see `crate::model::ExtrusionEdgeRef`, SPEC §3.4). `amount` is the chamfer
    /// distance or fillet radius depending on `kind`.
    EdgeTreatment {
        extrusion: usize,
        edge: crate::model::ExtrusionEdgeRef,
        kind: VertexTreatmentKind,
        amount: f32,
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
    ShadingMode(ShadingMode),
    /// Show/hide a UI pane. `None` toggles.
    SetPane { pane: Pane, visible: Option<bool> },
    AddParameter { name: String, expression: String },
    CreateParameterFromLineLength { line_index: usize, name: Option<String> },
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
    Key(Key),
    KeyDown(Key),
    KeyUp(Key),
    Type(String),

    // Sequencing
    WaitMs(u64),
    WaitFrames(u32),
    /// Save a screenshot. `whole_window` captures the full window; otherwise just the 3D
    /// viewport (with the view-cube HUD suppressed).
    Screenshot {
        path: String,
        whole_window: bool,
    },
    Quit,
}

impl Instruction {
    /// Format this instruction as a Lua API call (for `--show-commands` logging).
    pub fn as_lua(&self) -> String {
        match self {
            Instruction::New => "bearcad.new()".to_string(),
            Instruction::Open(path) => format!("bearcad.open({path:?})"),
            Instruction::Save(None) => "bearcad.save()".to_string(),
            Instruction::Save(Some(path)) => format!("bearcad.save({path:?})"),
            Instruction::ExportStl { path, body: None } => format!("bearcad.export_stl({path:?})"),
            Instruction::ExportStl {
                path,
                body: Some(body),
            } => format!("bearcad.export_stl({path:?}, {body:?})"),
            Instruction::ExportStep { path, body: None } => format!("bearcad.export_step({path:?})"),
            Instruction::ExportStep {
                path,
                body: Some(body),
            } => format!("bearcad.export_step({path:?}, {body:?})"),
            Instruction::ImportStl { path } => format!("bearcad.import_stl({path:?})"),
            Instruction::ImportStep { path } => format!("bearcad.import_step({path:?})"),
            Instruction::Clear => "bearcad.clear()".to_string(),
            Instruction::Undo => "bearcad.undo()".to_string(),
            Instruction::Tool(tool) => format!("bearcad.ui.tool({:?})", tool_lua_name(*tool)),
            Instruction::BeginSketch { face } => {
                let (kind, index) = face_lua_parts(face);
                format!("bearcad.begin_sketch({kind:?}, {index})")
            }
            Instruction::OpenSketch { sketch } => format!("bearcad.open_sketch({sketch})"),
            Instruction::ExitSketch => "bearcad.exit_sketch()".to_string(),
            Instruction::CreateRect {
                x,
                y,
                width,
                height,
            } => format!("bearcad.rect{{ x = {x}, y = {y}, width = {width}, height = {height} }}"),
            Instruction::CreateLine { x0, y0, x1, y1, bezier } => {
                let bezier_arg = match bezier {
                    Some([(c0x, c0y), (c1x, c1y)]) => format!(
                        ", bezier = {{ {{ {c0x}, {c0y} }}, {{ {c1x}, {c1y} }} }}"
                    ),
                    None => String::new(),
                };
                format!("bearcad.line{{ x = {x0}, y = {y0}, x1 = {x1}, y1 = {y1}{bezier_arg} }}")
            }
            Instruction::CreateCircle { cx, cy, r } => {
                format!("bearcad.circle{{ x = {cx}, y = {cy}, r = {r} }}")
            }
            Instruction::Extrude {
                faces,
                distance,
                body,
                ..
            } => {
                let body = match body {
                    crate::actions::ExtrudeBodyChoice::New => "",
                    crate::actions::ExtrudeBodyChoice::Merge => ", body = \"merge\"",
                    crate::actions::ExtrudeBodyChoice::Cut => ", body = \"cut\"",
                };
                format!(
                    "bearcad.extrude{{ {}, distance = {distance}{body} }}",
                    extrude_face_args(faces)
                )
            }
            Instruction::SetElementVisible { element, visible } => {
                let target = element_lua_ref(element);
                let verb = match visible {
                    Some(true) => "show",
                    Some(false) => "hide",
                    None => "toggle",
                };
                format!("bearcad.set_visible({target}, {verb:?})")
            }
            Instruction::SelectSceneElement { element, additive } => {
                let target = element_lua_ref(element);
                if *additive {
                    format!("bearcad.select({target}, {{ additive = true }})")
                } else {
                    format!("bearcad.select({target})")
                }
            }
            Instruction::ClearSceneSelection => "bearcad.clear_selection()".to_string(),
            Instruction::SetShapeConstruction { element, construction } => {
                format!(
                    "bearcad.set_construction({}, {})",
                    element_lua_ref(element),
                    construction
                )
            }
            Instruction::ApplyConstruction { construction } => {
                format!("bearcad.apply_construction({construction})")
            }
            Instruction::ToggleConstruction => "bearcad.toggle_construction()".to_string(),
            Instruction::SetElementName { element, name } => {
                format!(
                    "bearcad.set_name({}, {name:?})",
                    element_lua_ref(element)
                )
            }
            Instruction::FocusElementName => "bearcad.ui.focus_name()".to_string(),
            Instruction::SetDocumentUnits { length, angle } => {
                format!(
                    "bearcad.set_units{{ length = {:?}, angle = {:?} }}",
                    length.script_name(),
                    angle.script_name()
                )
            }
            Instruction::SetSketchUnits { sketch, length, angle } => {
                let length_arg = match length {
                    Some(length) => format!(", length = {:?}", length.script_name()),
                    None => String::new(),
                };
                let angle_arg = match angle {
                    Some(angle) => format!(", angle = {:?}", angle.script_name()),
                    None => String::new(),
                };
                format!("bearcad.set_units{{ sketch = {sketch}{length_arg}{angle_arg} }}")
            }
            Instruction::SetDim { axis, value } => {
                format!(
                    "bearcad.set_dim({:?}, {value:?})",
                    rect_axis_lua_name(*axis)
                )
            }
            Instruction::SetDimLabelOffset { axis, offset } => {
                format!(
                    "bearcad.set_dim_label_offset({:?}, {offset})",
                    dim_label_axis_lua_name(*axis)
                )
            }
            Instruction::BeginEditCommittedDim { axis } => {
                format!(
                    "bearcad.edit_dim({:?})",
                    dim_label_axis_lua_name(*axis)
                )
            }
            Instruction::CommitCommittedDim => "bearcad.commit_dim()".to_string(),
            Instruction::AddDistanceConstraint { target, expression } => {
                format!(
                    "bearcad.add_constraint({}, {expression:?})",
                    distance_target_lua_ref(target)
                )
            }
            Instruction::AddGeometricConstraint(kind) => {
                format!(
                    "bearcad.add_geometric_constraint({:?})",
                    geometric_constraint_lua_name(*kind)
                )
            }
            Instruction::ApplyConstraintShortcut(key) => {
                format!("bearcad.constraint_shortcut({key:?})")
            }
            Instruction::DragVertex { point, u, v } => {
                format!(
                    "bearcad.ui.drag_vertex({}, {u}, {v})",
                    constraint_point_lua_ref(point)
                )
            }
            Instruction::DragLineSegment {
                target,
                anchor_u,
                anchor_v,
                u,
                v,
            } => format!(
                "bearcad.ui.drag_line({}, {anchor_u}, {anchor_v}, {u}, {v})",
                constraint_line_lua_ref(target)
            ),
            Instruction::VertexTreatment { point, kind, amount } => {
                let (fname, amount_key) = match kind {
                    VertexTreatmentKind::Chamfer => ("chamfer_vertex", "distance"),
                    VertexTreatmentKind::Fillet => ("fillet_vertex", "radius"),
                };
                format!(
                    "bearcad.{fname}{{ point = {}, {amount_key} = {amount} }}",
                    constraint_point_lua_ref(point)
                )
            }
            Instruction::EdgeTreatment { extrusion, edge, kind, amount } => {
                let (fname, amount_key) = match kind {
                    VertexTreatmentKind::Chamfer => ("chamfer_edge", "distance"),
                    VertexTreatmentKind::Fillet => ("fillet_edge", "radius"),
                };
                format!(
                    "bearcad.{fname}{{ extrusion = {extrusion}, edge = {}, {amount_key} = {amount} }}",
                    extrusion_edge_lua_ref(*edge)
                )
            }
            Instruction::SetLineLength { value } => {
                format!("bearcad.set_dim(\"length\", {value:?})")
            }
            Instruction::SetCircleDiameter { value } => {
                format!("bearcad.set_dim(\"diameter\", {value:?})")
            }
            Instruction::BeginEditConstructionPlane { index } => {
                format!("bearcad.edit_plane({index})")
            }
            Instruction::CommitConstructionPlane => "bearcad.commit_plane()".to_string(),
            Instruction::SetPlaneOffset { value } => {
                format!("bearcad.set_dim(\"offset\", {value:?})")
            }
            Instruction::SetPlaneAngle { value } => {
                format!("bearcad.set_dim(\"angle\", {value:?})")
            }
            Instruction::FocusDim(axis) => {
                format!("bearcad.ui.focus_dim({:?})", rect_axis_lua_name(*axis))
            }
            Instruction::FocusLineLength => "bearcad.ui.focus_dim(\"length\")".to_string(),
            Instruction::FocusCircleDiameter => "bearcad.ui.focus_dim(\"diameter\")".to_string(),
            Instruction::FocusPlaneDim(dim) => {
                format!("bearcad.ui.focus_dim({:?})", plane_dim_lua_name(*dim))
            }
            Instruction::Orbit { dx, dy } => format!("bearcad.ui.orbit({dx}, {dy})"),
            Instruction::Pan { dx, dy } => format!("bearcad.ui.pan({dx}, {dy})"),
            Instruction::Zoom { scroll } => format!("bearcad.ui.wheel({scroll})"),
            Instruction::View(view) => format!("bearcad.ui.view({:?})", view_script_name(*view)),
            Instruction::ViewEdge(edge) => {
                format!("bearcad.ui.view(\"edge\", {:?})", edge_script_name(*edge))
            }
            Instruction::ViewCorner(corner) => format!(
                "bearcad.ui.view(\"corner\", {:?})",
                corner_script_name(*corner)
            ),
            Instruction::ViewHome => "bearcad.ui.view_home()".to_string(),
            Instruction::SetHomeView => "bearcad.ui.set_home_view()".to_string(),
            Instruction::ProjectionMode(mode) => {
                format!("bearcad.ui.view({:?})", projection_mode_script_name(*mode))
            }
            Instruction::ToggleProjectionMode => "bearcad.ui.toggle_projection()".to_string(),
            Instruction::ShadingMode(mode) => {
                format!("bearcad.ui.shading({:?})", mode.script_name())
            }
            Instruction::SetPane { pane, visible } => {
                let verb = match visible {
                    Some(true) => "show",
                    Some(false) => "hide",
                    None => "toggle",
                };
                format!("bearcad.ui.pane({:?}, {verb:?})", pane.script_name())
            }
            Instruction::AddParameter { name, expression } => {
                format!("bearcad.parameter(\"add\", {name:?}, {expression:?})")
            }
            Instruction::CreateParameterFromLineLength { line_index, name } => match name {
                Some(name) => format!(
                    "bearcad.parameter(\"from_line_length\", {line_index}, {name:?})"
                ),
                None => format!("bearcad.parameter(\"from_line_length\", {line_index})"),
            },
            Instruction::SetParameterName { index, name } => {
                format!("bearcad.parameter(\"name\", {index}, {name:?})")
            }
            Instruction::SetParameterExpression { index, expression } => {
                format!("bearcad.parameter(\"value\", {index}, {expression:?})")
            }
            Instruction::DeleteParameter { index } => {
                format!("bearcad.parameter(\"delete\", {index})")
            }
            Instruction::DeleteSelection => "bearcad.delete_selection()".to_string(),
            Instruction::SetCommandPalette { open } => {
                let verb = match open {
                    Some(true) => "show",
                    Some(false) => "hide",
                    None => "toggle",
                };
                format!("bearcad.ui.palette({verb:?})")
            }
            Instruction::RunPaletteCommand { query } => {
                format!("bearcad.ui.palette(\"run\", {query:?})")
            }
            Instruction::Move { x, y } => format!("bearcad.ui.move({x}, {y})"),
            Instruction::Click { x, y } => format!("bearcad.ui.click({x}, {y})"),
            Instruction::MoveGround { x, y } => format!("bearcad.ui.move_ground({x}, {y})"),
            Instruction::ClickGround { x, y } => format!("bearcad.ui.click_ground({x}, {y})"),
            Instruction::Drag { x0, y0, x1, y1 } => {
                format!("bearcad.ui.drag({x0}, {y0}, {x1}, {y1})")
            }
            Instruction::RightDrag { dx, dy } => format!("bearcad.ui.right_drag({dx}, {dy})"),
            Instruction::RightDragShift { dx, dy } => {
                format!("bearcad.ui.right_drag_pan({dx}, {dy})")
            }
            Instruction::Key(key) => format!("bearcad.ui.key({:?})", key_name(*key)),
            Instruction::KeyDown(key) => format!("bearcad.ui.keydown({:?})", key_name(*key)),
            Instruction::KeyUp(key) => format!("bearcad.ui.keyup({:?})", key_name(*key)),
            Instruction::Type(text) => format!("bearcad.ui.type({text:?})"),
            Instruction::WaitMs(ms) => format!("bearcad.ui.wait_ms({ms})"),
            Instruction::WaitFrames(n) => format!("bearcad.ui.wait({n})"),
            Instruction::Screenshot { path, whole_window } => {
                if *whole_window {
                    format!("bearcad.ui.screenshot({path:?}, true)")
                } else {
                    format!("bearcad.ui.screenshot({path:?})")
                }
            }
            Instruction::Quit => "bearcad.quit()".to_string(),
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

struct ElementScriptTokens {
    kind: &'static str,
    index: usize,
    point: Option<crate::model::ConstraintPoint>,
}

fn element_script_tokens(element: SceneElement) -> ElementScriptTokens {
    match element {
        SceneElement::ConstructionPlane(i) => ElementScriptTokens {
            kind: "construction_plane",
            index: i,
            point: None,
        },
        SceneElement::Sketch(i) => ElementScriptTokens {
            kind: "sketch",
            index: i,
            point: None,
        },
        SceneElement::Line(i) => ElementScriptTokens {
            kind: "line",
            index: i,
            point: None,
        },
        SceneElement::Circle(i) => ElementScriptTokens {
            kind: "circle",
            index: i,
            point: None,
        },
        SceneElement::Constraint(i) => ElementScriptTokens {
            kind: "constraint",
            index: i,
            point: None,
        },
        SceneElement::Point(point) => ElementScriptTokens {
            kind: "point",
            index: 0,
            point: Some(point),
        },
        SceneElement::Extrusion(i) => ElementScriptTokens {
            kind: "extrusion",
            index: i,
            point: None,
        },
        SceneElement::Body(i) => ElementScriptTokens {
            kind: "body",
            index: i,
            point: None,
        },
        // Handled directly in `element_lua_ref` before this is reached (a `FaceEdge` doesn't
        // fit the `kind`/`index`/`edge`/`point` shape the other variants share).
        SceneElement::FaceEdge(_) => ElementScriptTokens {
            kind: "face_edge",
            index: 0,
            point: None,
        },
    }
}

fn geometric_constraint_script_name(
    kind: crate::geometric_constraints::GeometricConstraintType,
) -> &'static str {
    use crate::geometric_constraints::GeometricConstraintType;
    match kind {
        GeometricConstraintType::Parallel => "parallel",
        GeometricConstraintType::Perpendicular => "perpendicular",
        GeometricConstraintType::Equal => "equal",
        GeometricConstraintType::Coincident => "coincident",
        GeometricConstraintType::Midpoint => "midpoint",
        GeometricConstraintType::Horizontal => "horizontal",
        GeometricConstraintType::Vertical => "vertical",
    }
}

/// Map an applied [`Action`] to a script [`Instruction`] when one exists.
pub fn instruction_from_action(action: &Action, doc: &crate::model::Document) -> Option<Instruction> {
    use crate::actions::dim_label_axis_for_target;
    match action {
        Action::NewDocument => Some(Instruction::New),
        Action::Open { path } => Some(Instruction::Open(path.clone())),
        Action::Save { path } => Some(Instruction::Save(path.clone())),
        Action::ExportStl { path, body } => Some(Instruction::ExportStl {
            path: path.clone(),
            body: body.clone(),
        }),
        Action::ExportStep { path, body } => Some(Instruction::ExportStep {
            path: path.clone(),
            body: body.clone(),
        }),
        Action::ImportStl { path } => Some(Instruction::ImportStl { path: path.clone() }),
        Action::ImportStep { path } => Some(Instruction::ImportStep { path: path.clone() }),
        Action::Clear => Some(Instruction::Clear),
        Action::UndoLast => Some(Instruction::Undo),
        Action::SetTool(tool) => Some(Instruction::Tool(*tool)),
        // The interactive draw tools commit straight to `doc` without going through the
        // declarative Create*/Extrude actions (#59); replay them as the equivalent call
        // using the as-committed geometry. A failed commit (e.g. "too small") returns
        // `ActionResult::Err`, so `after_apply` never reaches here for those.
        // A rectangle is now four plain lines (#66 polygon); reconstruct its origin/extent
        // from the bounding box of the four lines just appended by the commit.
        Action::CommitRectangle => {
            let n = doc.lines.len();
            (n >= 4).then(|| {
                let rect_lines = &doc.lines[n - 4..];
                let mut min_x = f32::INFINITY;
                let mut min_y = f32::INFINITY;
                let mut max_x = f32::NEG_INFINITY;
                let mut max_y = f32::NEG_INFINITY;
                for l in rect_lines {
                    for (x, y) in [(l.x0, l.y0), (l.x1, l.y1)] {
                        min_x = min_x.min(x);
                        min_y = min_y.min(y);
                        max_x = max_x.max(x);
                        max_y = max_y.max(y);
                    }
                }
                Instruction::CreateRect {
                    x: min_x,
                    y: min_y,
                    width: max_x - min_x,
                    height: max_y - min_y,
                }
            })
        }
        Action::CommitLine => doc.lines.last().map(|l| Instruction::CreateLine {
            x0: l.x0,
            y0: l.y0,
            x1: l.x1,
            y1: l.y1,
            bezier: l.bezier,
        }),
        Action::CommitCircle => doc.circles.last().map(|c| Instruction::CreateCircle {
            cx: c.cx,
            cy: c.cy,
            r: c.r,
        }),
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
        Action::BeginSketch { face, .. } => Some(Instruction::BeginSketch { face: face.clone() }),
        Action::OpenSketch { sketch, .. } => Some(Instruction::OpenSketch { sketch: *sketch }),
        Action::ExitSketch => Some(Instruction::ExitSketch),
        Action::SetElementVisible { element, visible } => Some(Instruction::SetElementVisible {
            element: element.clone(),
            visible: Some(*visible),
        }),
        Action::ToggleElementVisibility(element) => Some(Instruction::SetElementVisible {
            element: element.clone(),
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
        Action::CreateParameterFromLineLength { line_index, name } => {
            Some(Instruction::CreateParameterFromLineLength {
                line_index: *line_index,
                name: name.clone(),
            })
        }
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
            element: element.clone(),
            additive: *additive,
        }),
        Action::ClearSceneSelection => Some(Instruction::ClearSceneSelection),
        Action::SetShapeConstruction {
            element,
            construction,
        } => Some(Instruction::SetShapeConstruction {
            element: element.clone(),
            construction: *construction,
        }),
        Action::ApplyConstruction { construction } => Some(Instruction::ApplyConstruction {
            construction: *construction,
        }),
        Action::ToggleConstruction => Some(Instruction::ToggleConstruction),
        Action::AddGeometricConstraint(kind) => Some(Instruction::AddGeometricConstraint(*kind)),
        Action::ApplyConstraintShortcut(key) => Some(Instruction::ApplyConstraintShortcut(*key)),
        Action::DragVertex { point, u, v } => Some(Instruction::DragVertex {
            point: point.clone(),
            u: *u,
            v: *v,
        }),
        Action::CommitElementName { element, name } => Some(Instruction::SetElementName {
            element: element.clone(),
            name: name.clone(),
        }),
        Action::FocusElementName => Some(Instruction::FocusElementName),
        Action::SetDocumentUnits { length, angle } => {
            Some(Instruction::SetDocumentUnits { length: *length, angle: *angle })
        }
        Action::SetSketchUnits { sketch, length, angle } => Some(Instruction::SetSketchUnits {
            sketch: *sketch,
            length: *length,
            angle: *angle,
        }),
        Action::CommitVertexTreatment { point, kind, amount } => {
            Some(Instruction::VertexTreatment {
                point: point.clone(),
                kind: *kind,
                amount: *amount,
            })
        }
        Action::CommitEdgeTreatment { extrusion, edge, kind, amount } => {
            Some(Instruction::EdgeTreatment {
                extrusion: *extrusion,
                edge: *edge,
                kind: *kind,
                amount: *amount,
            })
        }
        _ => None,
    }
}

/// Build a replayable `Instruction::Extrude` for the extrusion the interactive Extrude tool
/// just created (the last entry in `doc.extrusions`). Used by the command log instead of
/// `instruction_from_action`, since `Action::CommitExtrusion` carries no fields to read the
/// committed faces/distance/body choice from — only `doc`'s post-commit state has them (#59).
pub fn instruction_for_new_extrusion(doc: &crate::model::Document) -> Option<Instruction> {
    let ei = doc.extrusions.len().checked_sub(1)?;
    let extrusion = doc.extrusions.get(ei)?;
    let body = match crate::model::body_index_for_extrusion(doc, ei).and_then(|bi| doc.bodies.get(bi))
    {
        // Subtracted from its body → a cut (#35).
        Some(body) if body.source.cut_extrusion_indices().contains(&ei) => {
            crate::actions::ExtrudeBodyChoice::Cut
        }
        // Added alongside other extrusions → merged into an existing body (#32).
        Some(body) if body.source.extrusion_indices().len() > 1 => {
            crate::actions::ExtrudeBodyChoice::Merge
        }
        _ => crate::actions::ExtrudeBodyChoice::New,
    };
    Some(Instruction::Extrude {
        sketch: extrusion.sketch,
        faces: extrusion.faces.clone(),
        distance: extrusion.distance,
        body,
    })
}

/// Render an extrusion's faces as `bearcad.extrude{}` keyword arguments
/// (`rect=`/`rects=`, `circle=`/`circles=`, `polygon=`). A single rect or circle uses the
/// singular field to match how `bearcad.extrude` is normally called by hand; multiple of a
/// kind use the plural array form. Only the first polygon face is kept — the Lua API has no
/// way to extrude more than one closed-loop face alongside the others in one call.
fn extrude_face_args(faces: &[crate::model::ExtrudeFace]) -> String {
    use crate::model::ExtrudeFace;
    let mut circles = Vec::new();
    let mut polygon = None;
    let mut boolean = None;
    for face in faces {
        match face {
            ExtrudeFace::Circle(i) => circles.push(*i),
            ExtrudeFace::Polygon(lines) => {
                polygon.get_or_insert(lines);
            }
            // Only the first is kept, same "one non-rect/circle profile per call" limitation
            // as `polygon` above — the Lua API has no way to extrude more than one alongside
            // the others in a single call.
            ExtrudeFace::Boolean { op, a, b } => {
                boolean.get_or_insert((*op, a.as_ref(), b.as_ref()));
            }
        };
    }
    let index_list = |indices: &[usize]| -> String {
        indices.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
    };
    let mut parts = Vec::new();
    match circles.as_slice() {
        [] => {}
        [single] => parts.push(format!("circle = {single}")),
        many => parts.push(format!("circles = {{{}}}", index_list(many))),
    }
    if let Some(lines) = polygon {
        parts.push(format!("polygon = {{{}}}", index_list(lines)));
    }
    if let Some((op, a, b)) = boolean {
        parts.push(format!("boolean = {}", boolean_face_lua_table(op, a, b)));
    }
    parts.join(", ")
}

/// Lua table literal for a boolean-combined face's inner fields (#16/#62): `{op = "...",
/// a = <face spec>, b = <face spec>}`, matching the shape `lua_boolean_face_from_table`
/// (src/lua_script.rs) parses back.
fn boolean_face_lua_table(
    op: crate::model::BooleanOp,
    a: &crate::model::ExtrudeFace,
    b: &crate::model::ExtrudeFace,
) -> String {
    let op_str = match op {
        crate::model::BooleanOp::Intersection => "intersection",
        crate::model::BooleanOp::Difference => "difference",
    };
    format!(
        "{{op = \"{op_str}\", a = {}, b = {}}}",
        extrude_face_spec_table(a),
        extrude_face_spec_table(b)
    )
}

/// Lua face-spec table for any `ExtrudeFace` (`{rect = i}`, `{circle = i}`,
/// `{polygon = {..}}`, or a nested `{boolean = {...}}`) — the shape
/// `lua_extrude_face_from_table` (src/lua_script.rs) parses back into an `ExtrudeFace`.
fn extrude_face_spec_table(face: &crate::model::ExtrudeFace) -> String {
    use crate::model::ExtrudeFace;
    match face {
        ExtrudeFace::Circle(i) => format!("{{circle = {i}}}"),
        ExtrudeFace::Polygon(lines) => {
            let idx = lines.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
            format!("{{polygon = {{{idx}}}}}")
        }
        ExtrudeFace::Boolean { op, a, b } => {
            format!("{{boolean = {}}}", boolean_face_lua_table(*op, a, b))
        }
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
        Tool::Extrude => "extrude",
        Tool::Chamfer => "chamfer",
        Tool::Fillet => "fillet",
    }
}

fn face_lua_parts(face: &FaceId) -> (&'static str, usize) {
    match face {
        FaceId::Circle(i) => ("circle", *i),
        FaceId::ConstructionPlane(i) => ("construction_plane", *i),
        // Cap/side faces aren't yet addressable from the two-argument script form.
        FaceId::ExtrudeCap { extrusion, .. } => ("extrude_cap", *extrusion),
        FaceId::ExtrudeSide { extrusion, .. } => ("extrude_side", *extrusion),
        // A polygon's full line list isn't expressible as a single index; same limitation
        // as cap/side faces above (#66).
        FaceId::Polygon(lines) => ("polygon", *lines.first().unwrap_or(&0)),
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

fn element_lua_ref(element: &SceneElement) -> String {
    // #26/#27: a face's own edge, matching `lua_script::parse_element_table`'s
    // `{ kind = "face", face = {...}, index = N, edge = true }` shape.
    if let SceneElement::FaceEdge(line) = element {
        let ConstraintLine::FaceEdge { face, index } = line else {
            unreachable!("SceneElement::FaceEdge always wraps ConstraintLine::FaceEdge")
        };
        return format!(
            "{{ kind = \"face\", face = {}, index = {index}, edge = true }}",
            face_id_lua_ref(face)
        );
    }
    let tokens = element_script_tokens(element.clone());
    if let Some(point) = tokens.point {
        return format!("{{ kind = \"point\", {} }}", point_lua_fields(&point));
    }
    format!("{{ kind = \"{}\", index = {} }}", tokens.kind, tokens.index)
}

fn point_lua_fields(point: &ConstraintPoint) -> String {
    use crate::model::{ConstraintPoint, LineEnd};
    match point {
        ConstraintPoint::LineEndpoint { line, end } => {
            let end_name = match end {
                LineEnd::Start => "start",
                LineEnd::End => "end",
            };
            // `end` is a Lua reserved word, so it can't be a bareword table key; bracket it.
            format!("kind = \"line\", index = {line}, [\"end\"] = \"{end_name}\"")
        }
        ConstraintPoint::CircleCenter(circle) => {
            format!("kind = \"circle\", index = {circle}")
        }
        // #26/#27: mirrors `lua_script::parse_constraint_point_table`'s `"face"` shape.
        ConstraintPoint::FaceVertex { face, index } => {
            format!("kind = \"face\", face = {}, index = {index}", face_id_lua_ref(face))
        }
    }
}

fn constraint_line_lua_ref(line: &ConstraintLine) -> String {
    match line {
        ConstraintLine::Line(index) => format!("{{ kind = \"line\", index = {index} }}"),
        // #26/#27: mirrors `lua_script::parse_constraint_line_table`'s `"face"` shape.
        ConstraintLine::FaceEdge { face, index } => format!(
            "{{ kind = \"face\", face = {}, index = {index} }}",
            face_id_lua_ref(face)
        ),
    }
}

fn constraint_point_lua_ref(point: &ConstraintPoint) -> String {
    format!("{{ {} }}", point_lua_fields(point))
}

/// Lua table literal for a `FaceId`, matching `lua_script::parse_face_id_table`'s shape.
/// Cap/side profiles are limited to `rect`/`circle` (same limitation as `face_lua_parts` and
/// `parse_face_id_table` — a polygon profile isn't a single index, #66).
fn face_id_lua_ref(face: &FaceId) -> String {
    match face {
        FaceId::Circle(i) => format!("{{ kind = \"circle\", index = {i} }}"),
        FaceId::ConstructionPlane(i) => format!("{{ kind = \"construction_plane\", index = {i} }}"),
        FaceId::Polygon(lines) => format!(
            "{{ kind = \"polygon\", index = {} }}",
            lines.first().copied().unwrap_or(0)
        ),
        FaceId::ExtrudeCap { extrusion, profile, top } => format!(
            "{{ kind = \"extrude_cap\", extrusion = {extrusion}, {}, top = {top} }}",
            extrude_face_profile_lua_fields(profile)
        ),
        FaceId::ExtrudeSide { extrusion, profile, edge } => format!(
            "{{ kind = \"extrude_side\", extrusion = {extrusion}, {}, edge = {edge} }}",
            extrude_face_profile_lua_fields(profile)
        ),
    }
}

fn extrude_face_profile_lua_fields(profile: &ExtrudeFace) -> String {
    match profile {
        ExtrudeFace::Circle(i) => format!("profile = \"circle\", profile_index = {i}"),
        // Not round-trippable: `parse_face_id_table` only accepts `rect`/`circle` profiles
        // (same limitation as `face_lua_parts`'s polygon case, #66).
        ExtrudeFace::Polygon(lines) => format!(
            "profile = \"polygon\", profile_index = {}",
            lines.first().copied().unwrap_or(0)
        ),
        // Not round-trippable at all (no `parse_face_id_table` support for boolean profiles)
        // — falls back to `a`'s fields as a best-effort reference, same tradeoff as
        // `ExtrudeFace::face_id()`'s recursion into `a`.
        ExtrudeFace::Boolean { a, .. } => extrude_face_profile_lua_fields(a),
    }
}

/// Lua table literal for an `ExtrusionEdgeRef`, matching `parse_extrusion_edge_table`'s shape
/// (#77): `{ kind = "vertical", face = N, edge = N }` or `{ kind = "cap", face = N, edge = N,
/// top = true/false }`.
fn extrusion_edge_lua_ref(edge: crate::model::ExtrusionEdgeRef) -> String {
    use crate::model::ExtrusionEdgeRef;
    match edge {
        ExtrusionEdgeRef::Vertical { face, edge } => {
            format!("{{ kind = \"vertical\", face = {face}, edge = {edge} }}")
        }
        ExtrusionEdgeRef::Cap { face, edge, top } => {
            format!("{{ kind = \"cap\", face = {face}, edge = {edge}, top = {top} }}")
        }
    }
}

fn distance_target_lua_ref(target: &DistanceTarget) -> String {
    match target {
        DistanceTarget::LineLength(index) => {
            format!("{{ kind = \"line\", index = {index} }}")
        }
        DistanceTarget::CircleDiameter(index) => {
            format!("{{ kind = \"circle\", index = {index} }}")
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

/// A pending screenshot request, resolved when egui delivers the captured frame.
struct ScreenshotRequest {
    path: String,
    /// `Some` crops the captured framebuffer to the 3D viewport; `None` keeps the whole window.
    crop: Option<ScreenshotCrop>,
}

struct ScreenshotCrop {
    /// 3D viewport rect in logical points.
    rect: egui::Rect,
    /// Logical-to-physical pixel ratio of the captured framebuffer.
    pixels_per_point: f32,
}

/// Drives a script through the live application, one step at a time.
pub struct ScriptRunner {
    instructions: Vec<Instruction>,
    lua: Option<LuaRunner>,
    pc: usize,
    wait_until: Option<Instant>,
    wait_frames_remaining: u32,
    screenshot_pending: Option<ScreenshotRequest>,
    waiting_view_transition: bool,
    /// Prevents re-printing an instruction while waiting (e.g. for viewport layout).
    logged_pc: Option<usize>,
    pub verbose: bool,
    pub done: bool,
    pub error: Option<String>,
    pub should_quit: bool,
}

impl ScriptRunner {
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
        let Some(request) = self.screenshot_pending.take() else {
            return Ok(());
        };
        match request.crop {
            Some(crop) => {
                save_screenshot_cropped(&request.path, image, crop.rect, crop.pixels_per_point)?
            }
            None => save_screenshot(&request.path, image)?,
        }
        if self.lua.is_none() {
            self.pc += 1;
        }
        Ok(())
    }

    /// Whether the view-cube HUD should be hidden this frame for a pending viewport screenshot.
    pub fn screenshot_suppresses_hud(&self) -> bool {
        self.screenshot_pending
            .as_ref()
            .is_some_and(|request| request.crop.is_some())
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
            Instruction::ExportStl { path, body } => {
                state.apply(Action::ExportStl { path, body });
                StepResult::Continue
            }
            Instruction::ExportStep { path, body } => {
                state.apply(Action::ExportStep { path, body });
                StepResult::Continue
            }
            Instruction::ImportStl { path } => {
                state.apply(Action::ImportStl { path });
                StepResult::Continue
            }
            Instruction::ImportStep { path } => {
                state.apply(Action::ImportStep { path });
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
            Instruction::CreateRect {
                x,
                y,
                width,
                height,
            } => {
                state.apply(Action::CreateRectangle {
                    x,
                    y,
                    width,
                    height,
                });
                StepResult::Continue
            }
            Instruction::CreateLine { x0, y0, x1, y1, bezier } => {
                state.apply(Action::CreateLineSegment { x0, y0, x1, y1, bezier });
                StepResult::Continue
            }
            Instruction::CreateCircle { cx, cy, r } => {
                state.apply(Action::CreateCircle { cx, cy, r });
                StepResult::Continue
            }
            Instruction::Extrude {
                sketch,
                faces,
                distance,
                body,
            } => {
                state.apply(Action::CreateExtrusion {
                    sketch,
                    faces,
                    distance,
                    body,
                });
                StepResult::Continue
            }
            Instruction::VertexTreatment { point, kind, amount } => {
                state.apply(Action::CommitVertexTreatment { point, kind, amount });
                StepResult::Continue
            }
            Instruction::EdgeTreatment { extrusion, edge, kind, amount } => {
                state.apply(Action::CommitEdgeTreatment { extrusion, edge, kind, amount });
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
            Instruction::SetDocumentUnits { length, angle } => {
                let _ = state.apply(Action::SetDocumentUnits { length, angle });
                StepResult::Continue
            }
            Instruction::SetSketchUnits { sketch, length, angle } => {
                let _ = state.apply(Action::SetSketchUnits { sketch, length, angle });
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
            Instruction::ApplyConstraintShortcut(key) => {
                let _ = state.apply(Action::ApplyConstraintShortcut(key));
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
            Instruction::ShadingMode(mode) => {
                state.apply(Action::SetShadingMode(mode));
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
            Instruction::CreateParameterFromLineLength { line_index, name } => {
                state.apply(Action::CreateParameterFromLineLength { line_index, name });
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
                        | PaletteOutcome::SaveFileAs
                        | PaletteOutcome::ExportSessionCommands => {
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
            Instruction::Screenshot { path, whole_window } => {
                let crop = if whole_window {
                    None
                } else {
                    viewport.map(|rect| ScreenshotCrop {
                        rect,
                        pixels_per_point: ctx.pixels_per_point(),
                    })
                };
                self.screenshot_pending = Some(ScreenshotRequest { path, crop });
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
    let rgba: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b(), c.a()])
        .collect();
    save_rgba(path, image.width() as u32, image.height() as u32, &rgba)
}

/// Save the portion of `image` covered by `rect` (logical points), scaled by `pixels_per_point`.
fn save_screenshot_cropped(
    path: &str,
    image: &egui::ColorImage,
    rect: egui::Rect,
    pixels_per_point: f32,
) -> Result<(), String> {
    let (x0, y0, x1, y1) = crop_bounds(image.width(), image.height(), rect, pixels_per_point);
    let (w, h) = (x1 - x0, y1 - y0);
    if w == 0 || h == 0 {
        // Degenerate crop (e.g. viewport rect unknown): fall back to the whole frame.
        return save_screenshot(path, image);
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in y0..y1 {
        let row = y * image.width();
        for x in x0..x1 {
            let c = image.pixels[row + x];
            rgba.extend_from_slice(&[c.r(), c.g(), c.b(), c.a()]);
        }
    }
    save_rgba(path, w as u32, h as u32, &rgba)
}

/// Physical-pixel `(x0, y0, x1, y1)` crop bounds, clamped to the image.
fn crop_bounds(
    img_w: usize,
    img_h: usize,
    rect: egui::Rect,
    pixels_per_point: f32,
) -> (usize, usize, usize, usize) {
    let to_px = |v: f32, max: usize| ((v * pixels_per_point).round() as i32).clamp(0, max as i32) as usize;
    let x0 = to_px(rect.min.x, img_w);
    let y0 = to_px(rect.min.y, img_h);
    let x1 = to_px(rect.max.x, img_w).max(x0);
    let y1 = to_px(rect.max.y, img_h).max(y0);
    (x0, y0, x1, y1)
}

fn save_rgba(path: &str, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    image::save_buffer(path, rgba, width, height, image::ColorType::Rgba8)
        .map_err(|e| format!("failed to save screenshot to {path}: {e}"))
}

/// CLI launch options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptOptions {
    pub script_path: Option<String>,
    pub document_path: Option<String>,
    pub exit_on_complete: bool,
    pub show_commands: bool,
    /// Force-exit (non-zero) if the app hasn't closed on its own within this many
    /// seconds — a watchdog for unattended/CI launches. See #61.
    pub timeout_secs: Option<u64>,
}

/// Parsed command-line outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliOutcome {
    Help,
    /// Install the `bearcad` CLI symlink onto PATH (`bearcad install-cli`). See #49.
    InstallCli,
    /// Remove the `bearcad` CLI symlink (`bearcad uninstall-cli`).
    UninstallCli,
    Run(ScriptOptions),
}

/// Print usage information to stdout.
pub fn print_usage() {
    println!(
        "\
BearCAD — parametric CAD prototype

Usage:
  bearcad [options] [script.lua]
  bearcad <command>

Commands:
  install-cli           Symlink this executable onto PATH as `bearcad`
                        (default /usr/local/bin; use sudo if it is not writable)
  uninstall-cli         Remove the `bearcad` PATH symlink

Options:
  --script <path>       Run a Lua script
  --exit, --exit-on-complete
                        Exit after startup, or after the script finishes
  --show-commands       Print each user action as a script line on stdout
  --timeout <seconds>   Force-exit with an error if the app hasn't closed on
                        its own within this many seconds
  -h, --help            Show this help and exit

Examples:
  bearcad
  bearcad --exit
  bearcad drawing.bearcad --exit
  bearcad --script demo.lua
  bearcad demo.lua --exit
  bearcad --exit --timeout 30
  bearcad install-cli
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
    // Subcommands (args[0] is the program name).
    match args.get(1).map(String::as_str) {
        Some("install-cli") => return CliOutcome::InstallCli,
        Some("uninstall-cli") => return CliOutcome::UninstallCli,
        _ => {}
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
            "--timeout" => {
                i += 1;
                if i < args.len() {
                    opts.timeout_secs = args[i].parse::<u64>().ok();
                }
            }
            arg if !arg.starts_with('-') => {
                if opts.script_path.is_none()
                    && (arg.ends_with(".lua")
                        || Path::new(arg).extension().is_some_and(|e| e == "lua"))
                {
                    opts.script_path = Some(arg.to_string());
                } else if opts.document_path.is_none()
                    && (arg.ends_with(".bearcad")
                        || Path::new(arg).extension().is_some_and(|e| e == "bearcad"))
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
    fn create_line_instruction_renders_bezier_when_present() {
        let straight = Instruction::CreateLine { x0: 0.0, y0: 0.0, x1: 10.0, y1: 0.0, bezier: None };
        assert_eq!(straight.as_lua(), "bearcad.line{ x = 0, y = 0, x1 = 10, y1 = 0 }");

        let curved = Instruction::CreateLine {
            x0: 0.0,
            y0: 0.0,
            x1: 10.0,
            y1: 0.0,
            bezier: Some([(3.0, 4.0), (7.0, 4.0)]),
        };
        assert_eq!(
            curved.as_lua(),
            "bearcad.line{ x = 0, y = 0, x1 = 10, y1 = 0, bezier = { { 3, 4 }, { 7, 4 } } }"
        );
    }

    #[test]
    fn set_units_instructions_render_replayable_lua() {
        let doc_units = Instruction::SetDocumentUnits { length: LengthUnit::In, angle: AngleUnit::Rad };
        assert_eq!(
            doc_units.as_lua(),
            "bearcad.set_units{ length = \"in\", angle = \"rad\" }"
        );

        let sketch_override = Instruction::SetSketchUnits {
            sketch: 2,
            length: Some(LengthUnit::Cm),
            angle: None,
        };
        assert_eq!(
            sketch_override.as_lua(),
            "bearcad.set_units{ sketch = 2, length = \"cm\" }"
        );

        let sketch_inherit = Instruction::SetSketchUnits { sketch: 0, length: None, angle: None };
        assert_eq!(sketch_inherit.as_lua(), "bearcad.set_units{ sketch = 0 }");
    }

    #[test]
    fn parse_key_names() {
        assert_eq!(parse_key("enter").unwrap(), Key::Enter);
        assert_eq!(parse_key("ESC").unwrap(), Key::Escape);
        assert!(parse_key("notakey").is_err());
    }

    #[test]
    fn screenshot_crop_bounds_scale_by_pixels_per_point() {
        // 800x600 logical window at 2x DPI -> 1600x1200 framebuffer.
        let rect = egui::Rect::from_min_max(egui::pos2(220.0, 40.0), egui::pos2(800.0, 600.0));
        let (x0, y0, x1, y1) = crop_bounds(1600, 1200, rect, 2.0);
        assert_eq!((x0, y0, x1, y1), (440, 80, 1600, 1200));
    }

    #[test]
    fn screenshot_crop_bounds_clamp_to_image() {
        // Viewport extends past the framebuffer; bounds clamp instead of overflowing.
        let rect = egui::Rect::from_min_max(egui::pos2(-10.0, -10.0), egui::pos2(2000.0, 2000.0));
        let (x0, y0, x1, y1) = crop_bounds(1600, 1200, rect, 1.0);
        assert_eq!((x0, y0, x1, y1), (0, 0, 1600, 1200));
    }

    #[test]
    fn screenshot_crop_produces_subimage_dimensions() {
        // 4x4 image, crop the bottom-right 2x2 (logical rect at 1x DPI).
        let pixels = vec![egui::Color32::WHITE; 16];
        let image = egui::ColorImage {
            size: [4, 4],
            pixels,
            ..Default::default()
        };
        let rect = egui::Rect::from_min_max(egui::pos2(2.0, 2.0), egui::pos2(4.0, 4.0));
        let (x0, y0, x1, y1) = crop_bounds(image.width(), image.height(), rect, 1.0);
        assert_eq!((x1 - x0, y1 - y0), (2, 2));
    }

    #[test]
    fn parse_cli_help_flags() {
        assert_eq!(parse_cli(["bearcad", "--help"]), CliOutcome::Help);
        assert_eq!(parse_cli(["bearcad", "-h"]), CliOutcome::Help);
    }

    #[test]
    fn parse_show_commands_flag() {
        let opts = parse_args(["bearcad", "--show-commands"]);
        assert!(opts.show_commands);
    }

    #[test]
    fn instruction_from_action_preserves_a_curved_committed_line() {
        let mut doc = crate::model::Document::default();
        let sketch = doc.add_sketch(crate::model::FaceId::ConstructionPlane(0));
        let mut line = crate::model::Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0);
        line.bezier = Some([(3.0, 4.0), (7.0, 4.0)]);
        doc.lines.push(line);
        let instruction = instruction_from_action(&Action::CommitLine, &doc).unwrap();
        assert_eq!(
            instruction,
            Instruction::CreateLine {
                x0: 0.0,
                y0: 0.0,
                x1: 10.0,
                y1: 0.0,
                bezier: Some([(3.0, 4.0), (7.0, 4.0)]),
            }
        );
    }

    #[test]
    fn vertex_treatment_instruction_renders_as_the_matching_lua_call() {
        let point = ConstraintPoint::LineEndpoint { line: 0, end: crate::model::LineEnd::End };
        let chamfer = Instruction::VertexTreatment {
            point: point.clone(),
            kind: VertexTreatmentKind::Chamfer,
            amount: 3.0,
        };
        assert_eq!(
            chamfer.as_lua(),
            "bearcad.chamfer_vertex{ point = { kind = \"line\", index = 0, [\"end\"] = \"end\" }, distance = 3 }"
        );
        let fillet = Instruction::VertexTreatment {
            point,
            kind: VertexTreatmentKind::Fillet,
            amount: 2.5,
        };
        assert_eq!(
            fillet.as_lua(),
            "bearcad.fillet_vertex{ point = { kind = \"line\", index = 0, [\"end\"] = \"end\" }, radius = 2.5 }"
        );
    }

    #[test]
    fn instruction_from_action_maps_commit_vertex_treatment() {
        let doc = crate::model::Document::default();
        let point = ConstraintPoint::LineEndpoint { line: 2, end: crate::model::LineEnd::Start };
        let action = Action::CommitVertexTreatment {
            point: point.clone(),
            kind: VertexTreatmentKind::Fillet,
            amount: 4.0,
        };
        assert_eq!(
            instruction_from_action(&action, &doc),
            Some(Instruction::VertexTreatment {
                point,
                kind: VertexTreatmentKind::Fillet,
                amount: 4.0,
            })
        );
    }

    #[test]
    fn edge_treatment_instruction_renders_as_the_matching_lua_call() {
        use crate::model::ExtrusionEdgeRef;
        let edge = ExtrusionEdgeRef::Vertical { face: 0, edge: 2 };
        let chamfer = Instruction::EdgeTreatment {
            extrusion: 1,
            edge,
            kind: VertexTreatmentKind::Chamfer,
            amount: 3.0,
        };
        assert_eq!(
            chamfer.as_lua(),
            "bearcad.chamfer_edge{ extrusion = 1, edge = { kind = \"vertical\", face = 0, edge = 2 }, distance = 3 }"
        );
        let cap_edge = ExtrusionEdgeRef::Cap { face: 1, edge: 3, top: true };
        let fillet = Instruction::EdgeTreatment {
            extrusion: 0,
            edge: cap_edge,
            kind: VertexTreatmentKind::Fillet,
            amount: 1.5,
        };
        assert_eq!(
            fillet.as_lua(),
            "bearcad.fillet_edge{ extrusion = 0, edge = { kind = \"cap\", face = 1, edge = 3, top = true }, radius = 1.5 }"
        );
    }

    #[test]
    fn instruction_from_action_maps_commit_edge_treatment() {
        use crate::model::ExtrusionEdgeRef;
        let doc = crate::model::Document::default();
        let edge = ExtrusionEdgeRef::Cap { face: 0, edge: 1, top: false };
        let action = Action::CommitEdgeTreatment {
            extrusion: 2,
            edge,
            kind: VertexTreatmentKind::Chamfer,
            amount: 2.5,
        };
        assert_eq!(
            instruction_from_action(&action, &doc),
            Some(Instruction::EdgeTreatment {
                extrusion: 2,
                edge,
                kind: VertexTreatmentKind::Chamfer,
                amount: 2.5,
            })
        );
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
            parse_cli(["bearcad", "--script", "test.lua", "--exit"]),
            CliOutcome::Run(ScriptOptions {
                script_path: Some("test.lua".to_string()),
                document_path: None,
                exit_on_complete: true,
                show_commands: false,
                timeout_secs: None,
            })
        );
    }

    #[test]
    fn parse_args_finds_timeout_flag() {
        let opts = parse_args(["bearcad", "--exit", "--timeout", "30"]);
        assert_eq!(opts.timeout_secs, Some(30));
    }

    #[test]
    fn parse_args_ignores_invalid_timeout_value() {
        let opts = parse_args(["bearcad", "--timeout", "soon"]);
        assert_eq!(opts.timeout_secs, None);
    }

    #[test]
    fn parse_args_finds_script_flag() {
        let opts = parse_args(["bearcad", "--script", "test.lua", "--exit"]);
        assert_eq!(opts.script_path.as_deref(), Some("test.lua"));
        assert!(opts.exit_on_complete);
    }

    #[test]
    fn parse_args_finds_positional_script() {
        let opts = parse_args(["bearcad", "demo.lua"]);
        assert_eq!(opts.script_path.as_deref(), Some("demo.lua"));
    }

    #[test]
    fn parse_args_finds_positional_document_and_exit() {
        let opts = parse_args(["bearcad", "/tmp/test.bearcad", "--exit"]);
        assert_eq!(opts.document_path.as_deref(), Some("/tmp/test.bearcad"));
        assert!(opts.exit_on_complete);
        assert!(opts.script_path.is_none());
    }

    #[test]
    fn parse_args_exit_without_paths_exits_after_startup() {
        let opts = parse_args(["bearcad", "--exit"]);
        assert!(opts.exit_on_complete);
        assert!(opts.script_path.is_none());
        assert!(opts.document_path.is_none());
    }

    #[test]
    fn instruction_as_lua_formats_click() {
        let ins = Instruction::Click { x: 100.0, y: 200.0 };
        assert_eq!(ins.as_lua(), "bearcad.ui.click(100, 200)");
    }

    #[test]
    fn script_drag_line_translates_segment() {
        let mut runner = ScriptRunner::from_instructions(vec![
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
            curve_mode: false,
            tangent_constraint: true,
            chained_from: None,
            chained_from_bezier: None,
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
    fn script_palette_run_sets_top_view() {
        let mut runner = ScriptRunner::from_instructions(vec![Instruction::RunPaletteCommand {
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
        let mut runner = ScriptRunner::from_instructions(vec![
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
        let mut runner = ScriptRunner::from_instructions(vec![
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
    fn script_adds_angle_parameter() {
        let mut runner = ScriptRunner::from_instructions(vec![Instruction::AddParameter {
            name: "corner".into(),
            expression: "16.7deg".into(),
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
        assert_eq!(state.doc.parameters[0].expression, "16.7deg");
        let angle = crate::value::eval_parameter_in_doc("corner", &state.doc).unwrap();
        match angle {
            crate::value::EvaluatedParameter::AngleRad(v) => {
                assert!((v.to_degrees() - 16.7).abs() < 1e-2);
            }
            _ => panic!("expected angle parameter"),
        }
    }

    #[test]
    fn runner_set_dim_expression_evaluates_length() {
        let mut runner = ScriptRunner::from_instructions(vec![
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
            curve_mode: false,
            tangent_constraint: true,
            chained_from: None,
            chained_from_bezier: None,
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
}