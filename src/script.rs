//! Instruction script parser and runner (SPEC §9.3).
//!
//! Scripts are human-readable, one instruction per line. They drive the live UI
//! via synthetic pointer/keyboard events and headless actions.

use crate::actions::{
    dim_label_target_in_sketch, Action, AppState, DimLabelAxis, Pane, RectAxis, Tool,
};
use crate::command_palette::{best_match, commands_for_state, PaletteOutcome};
use crate::constraints::add_distance_constraint;
use crate::hierarchy::SceneElement;
use crate::model::{DistanceTarget, FaceId, RectEdge, SketchId};
use crate::construction::PlaneDim;
use crate::camera::{ProjectionMode, StandardView};
use crate::view_cube::{CubeCornerId, CubeEdgeId};

use eframe::egui::{self, Key, Modifiers, PointerButton};
use glam::Vec3;
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
                let edge = tokens
                    .edge
                    .map(|edge| format!(" {}", edge.script_name()))
                    .unwrap_or_default();
                if *additive {
                    format!("select {} {}{} add", tokens.kind, tokens.index, edge)
                } else {
                    format!("select {} {}{}", tokens.kind, tokens.index, edge)
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
                };
                format!("add_constraint {target_name} {expression}")
            }
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
}

/// Parse errors from script files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// Parse a script from its text content.
pub fn parse(source: &str) -> Result<Vec<Instruction>, ParseError> {
    let mut instructions = Vec::new();
    for (i, raw) in source.lines().enumerate() {
        let line = raw.trim();
        let line_no = i + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        instructions.push(parse_line(line, line_no)?);
    }
    Ok(instructions)
}

/// Parse a script file from disk.
pub fn parse_file(path: &Path) -> Result<Vec<Instruction>, ParseError> {
    let source = std::fs::read_to_string(path).map_err(|e| ParseError {
        line: 0,
        message: format!("failed to read {}: {e}", path.display()),
    })?;
    parse(&source)
}

fn parse_line(line: &str, line_no: usize) -> Result<Instruction, ParseError> {
    let err = |msg: &str| ParseError {
        line: line_no,
        message: msg.to_string(),
    };

    let (cmd, rest) = line
        .split_once(char::is_whitespace)
        .map(|(c, r)| (c, r.trim()))
        .unwrap_or((line, ""));

    match cmd.to_ascii_lowercase().as_str() {
        "new" => Ok(Instruction::New),
        "clear" => Ok(Instruction::Clear),
        "undo" => Ok(Instruction::Undo),
        "quit" | "exit" => Ok(Instruction::Quit),

        "open" => {
            let path = rest.trim_matches('"');
            if path.is_empty() {
                return Err(err("open requires a path"));
            }
            Ok(Instruction::Open(path.to_string()))
        }

        "save" => {
            if rest.is_empty() {
                Ok(Instruction::Save(None))
            } else {
                Ok(Instruction::Save(Some(rest.trim_matches('"').to_string())))
            }
        }

        "tool" => {
            let name = rest.split_whitespace().next().unwrap_or("");
            Tool::from_name(name).map(Instruction::Tool).ok_or_else(|| {
                err(&format!(
                    "unknown tool '{name}' (expected select, sketch, rectangle, line, circle, dimension, or plane)"
                ))
            })
        }

        "add_constraint" | "addconstraint" | "constraint" => {
            let mut parts = rest.split_whitespace();
            let kind = parts
                .next()
                .ok_or_else(|| err("add_constraint requires target kind and index"))?;
            let index = parts
                .next()
                .ok_or_else(|| err("add_constraint requires target index"))?
                .parse::<usize>()
                .map_err(|_| err("add_constraint index must be an integer"))?;
            let target = match kind.to_ascii_lowercase().as_str() {
                "line" | "segment" => DistanceTarget::LineLength(index),
                "circle" => DistanceTarget::CircleDiameter(index),
                "rect" | "rectangle" => {
                    let dim = parts
                        .next()
                        .ok_or_else(|| err("add_constraint rect requires width or height"))?;
                    let expression = parts.collect::<Vec<_>>().join(" ");
                    if expression.is_empty() {
                        return Err(err("add_constraint requires expression"));
                    }
                    let target = match dim.to_ascii_lowercase().as_str() {
                        "width" | "w" => DistanceTarget::RectWidth(index),
                        "height" | "h" => DistanceTarget::RectHeight(index),
                        other => {
                            return Err(err(&format!(
                                "unknown rectangle dimension '{other}' (expected width or height)"
                            )));
                        }
                    };
                    return Ok(Instruction::AddDistanceConstraint { target, expression });
                }
                other => {
                    return Err(err(&format!(
                        "unknown constraint target '{other}' (expected line, circle, or rect)"
                    )));
                }
            };
            let expression = parts.collect::<Vec<_>>().join(" ");
            if expression.is_empty() {
                return Err(err("add_constraint requires expression"));
            }
            Ok(Instruction::AddDistanceConstraint { target, expression })
        }

        "begin_sketch" | "beginsketch" => {
            let mut parts = rest.split_whitespace();
            let kind = parts
                .next()
                .ok_or_else(|| err("begin_sketch requires face kind and index"))?;
            let index = parts
                .next()
                .ok_or_else(|| err("begin_sketch requires face index"))?
                .parse::<usize>()
                .map_err(|_| err("begin_sketch index must be an integer"))?;
            let face = FaceId::from_script(kind, index)
                .ok_or_else(|| err(&format!("unknown face kind '{kind}'")))?;
            Ok(Instruction::BeginSketch { face })
        }

        "open_sketch" | "opensketch" | "edit_sketch" | "editsketch" => {
            let index = rest
                .split_whitespace()
                .next()
                .ok_or_else(|| err("open_sketch requires sketch index"))?
                .parse::<usize>()
                .map_err(|_| err("open_sketch index must be an integer"))?;
            Ok(Instruction::OpenSketch { sketch: index })
        }

        "exit_sketch" | "exitsketch" => Ok(Instruction::ExitSketch),

        "edit_plane" | "editplane" => {
            let index = rest
                .split_whitespace()
                .next()
                .ok_or_else(|| err("edit_plane requires plane index"))?
                .parse::<usize>()
                .map_err(|_| err("edit_plane index must be an integer"))?;
            Ok(Instruction::BeginEditConstructionPlane { index })
        }

        "commit_plane" | "commitplane" => Ok(Instruction::CommitConstructionPlane),

        "element" => {
            let mut parts = rest.split_whitespace();
            let kind = parts
                .next()
                .ok_or_else(|| err("element requires kind and index"))?;
            let index = parts
                .next()
                .ok_or_else(|| err("element requires index"))?
                .parse::<usize>()
                .map_err(|_| err("element index must be an integer"))?;
            let element = scene_element_from_script(kind, index)
                .ok_or_else(|| err(&format!("unknown element kind '{kind}'")))?;
            let visible = match parts.next().map(|s| s.to_ascii_lowercase()).as_deref() {
                None | Some("toggle") => None,
                Some("show") | Some("on") | Some("true") => Some(true),
                Some("hide") | Some("off") | Some("false") => Some(false),
                Some(other) => {
                    return Err(err(&format!(
                        "unknown element state '{other}' (expected show, hide, or toggle)"
                    )))
                }
            };
            Ok(Instruction::SetElementVisible { element, visible })
        }

        "select" | "select_scene" => {
            let mut parts = rest.split_whitespace();
            let kind = parts
                .next()
                .ok_or_else(|| err("select requires kind and index"))?;
            let index = parts
                .next()
                .ok_or_else(|| err("select requires index"))?
                .parse::<usize>()
                .map_err(|_| err("select index must be an integer"))?;
            let (element, additive) =
                parse_scene_element_with_tail(kind, index, &mut parts, line_no)?;
            Ok(Instruction::SelectSceneElement { element, additive })
        }

        "clear_selection" | "deselect" | "select_clear" => Ok(Instruction::ClearSceneSelection),

        "set_construction" | "construction" => {
            let mut parts = rest.split_whitespace();
            let kind = parts
                .next()
                .ok_or_else(|| err("set_construction requires kind and index"))?;
            let index = parts
                .next()
                .ok_or_else(|| err("set_construction requires index"))?
                .parse::<usize>()
                .map_err(|_| err("set_construction index must be an integer"))?;
            let (element, tail) =
                parse_scene_element_with_optional_edge(kind, index, &mut parts, line_no)?;
            let value = tail.ok_or_else(|| ParseError {
                line: line_no,
                message: "set_construction requires true or false".to_string(),
            })?;
            let construction = parse_construction_value(value, line_no)?;
            Ok(Instruction::SetShapeConstruction {
                element,
                construction,
            })
        }

        "apply_construction" => {
            let value = rest
                .split_whitespace()
                .next()
                .ok_or_else(|| err("apply_construction requires true or false"))?;
            let construction = parse_construction_value(value, line_no)?;
            Ok(Instruction::ApplyConstruction { construction })
        }

        "toggle_construction" => Ok(Instruction::ToggleConstruction),

        "set_name" | "rename" => {
            let mut parts = rest.split_whitespace();
            let kind = parts
                .next()
                .ok_or_else(|| err("set_name requires kind and index"))?;
            let index = parts
                .next()
                .ok_or_else(|| err("set_name requires index"))?
                .parse::<usize>()
                .map_err(|_| err("set_name index must be an integer"))?;
            let element = scene_element_from_script(kind, index)
                .ok_or_else(|| err(&format!("unknown element kind '{kind}'")))?;
            let name = parts.collect::<Vec<_>>().join(" ");
            if name.trim().is_empty() {
                return Err(err("set_name requires a name"));
            }
            Ok(Instruction::SetElementName { element, name })
        }

        "focus_name" | "focus_element_name" => Ok(Instruction::FocusElementName),

        "pane" => {
            let mut parts = rest.split_whitespace();
            let name = parts.next().ok_or_else(|| err("pane requires a name"))?;
            let pane = Pane::from_name(name).ok_or_else(|| {
                err(&format!(
                    "unknown pane '{name}' (expected tree, context, parameters, or view_cube)"
                ))
            })?;
            let visible = match parts.next().map(|s| s.to_ascii_lowercase()).as_deref() {
                None | Some("toggle") => None,
                Some("show") | Some("on") | Some("true") => Some(true),
                Some("hide") | Some("off") | Some("false") => Some(false),
                Some(other) => {
                    return Err(err(&format!(
                        "unknown pane state '{other}' (expected show, hide, or toggle)"
                    )))
                }
            };
            Ok(Instruction::SetPane { pane, visible })
        }

        "palette" | "command_palette" | "commandpalette" => {
            let mut parts = rest.split_whitespace();
            let sub = parts.next().unwrap_or("toggle");
            match sub.to_ascii_lowercase().as_str() {
                "show" | "on" | "open" => Ok(Instruction::SetCommandPalette {
                    open: Some(true),
                }),
                "hide" | "off" | "close" => Ok(Instruction::SetCommandPalette {
                    open: Some(false),
                }),
                "toggle" => Ok(Instruction::SetCommandPalette { open: None }),
                "run" | "exec" | "execute" => {
                    let query = parts.collect::<Vec<_>>().join(" ");
                    if query.trim().is_empty() {
                        return Err(err("palette run requires a query"));
                    }
                    Ok(Instruction::RunPaletteCommand { query })
                }
                other => Err(err(&format!(
                    "unknown palette command '{other}' (expected show, hide, toggle, or run)"
                ))),
            }
        }

        "parameter" | "param" => {
            let (sub, tail) = rest
                .split_once(char::is_whitespace)
                .map(|(s, t)| (s, t.trim()))
                .unwrap_or((rest, ""));
            match sub.to_ascii_lowercase().as_str() {
                "add" => {
                    let (name, expression) = tail
                        .split_once(char::is_whitespace)
                        .ok_or_else(|| err("parameter add requires name and expression"))?;
                    let expression = expression.trim();
                    if expression.is_empty() {
                        return Err(err("parameter add requires an expression"));
                    }
                    Ok(Instruction::AddParameter {
                        name: name.to_string(),
                        expression: expression.to_string(),
                    })
                }
                "name" | "rename" => {
                    let (index_str, name) = tail
                        .split_once(char::is_whitespace)
                        .ok_or_else(|| err("parameter name requires index and name"))?;
                    let index = index_str
                        .parse::<usize>()
                        .map_err(|_| err("parameter name index must be an integer"))?;
                    let name = name.trim();
                    if name.is_empty() {
                        return Err(err("parameter name requires a name"));
                    }
                    Ok(Instruction::SetParameterName {
                        index,
                        name: name.to_string(),
                    })
                }
                "value" | "expr" | "expression" => {
                    let (index_str, expression) = tail
                        .split_once(char::is_whitespace)
                        .ok_or_else(|| err("parameter value requires index and expression"))?;
                    let index = index_str
                        .parse::<usize>()
                        .map_err(|_| err("parameter value index must be an integer"))?;
                    let expression = expression.trim();
                    if expression.is_empty() {
                        return Err(err("parameter value requires an expression"));
                    }
                    Ok(Instruction::SetParameterExpression {
                        index,
                        expression: expression.to_string(),
                    })
                }
                "delete" | "del" | "remove" => {
                    let index = tail
                        .split_whitespace()
                        .next()
                        .ok_or_else(|| err("parameter delete requires index"))?
                        .parse::<usize>()
                        .map_err(|_| err("parameter delete index must be an integer"))?;
                    Ok(Instruction::DeleteParameter { index })
                }
                other => Err(err(&format!(
                    "unknown parameter command '{other}' (expected add, name, value, or delete)"
                ))),
            }
        }

        "edit_dim" | "editdim" => {
            let axis_name = rest.split_whitespace().next().unwrap_or("");
            let axis = DimLabelAxis::from_name(axis_name)
                .ok_or_else(|| err(&format!("unknown axis '{axis_name}'")))?;
            Ok(Instruction::BeginEditCommittedDim { axis })
        }

        "commit_dim" | "commitdim" => Ok(Instruction::CommitCommittedDim),

        "set_dim_label_offset" | "setdimlabeloffset" | "dim_label_offset" => {
            let (axis_name, value) = rest
                .split_once(|c: char| c.is_whitespace())
                .ok_or_else(|| err("set_dim_label_offset requires axis and offset"))?;
            let axis = DimLabelAxis::from_name(axis_name.trim())
                .ok_or_else(|| err(&format!("unknown axis '{}'", axis_name.trim())))?;
            let offset = value
                .trim()
                .parse::<f32>()
                .map_err(|_| err("set_dim_label_offset offset must be a number"))?;
            Ok(Instruction::SetDimLabelOffset { axis, offset })
        }

        "set_dim" | "setdim" => {
            let (axis_name, value) = rest
                .split_once(|c: char| c.is_whitespace())
                .ok_or_else(|| err("set_dim requires axis and value"))?;
            let axis_name = axis_name.trim();
            let value = value.trim();
            if value.is_empty() {
                return Err(err("set_dim requires a value"));
            }
            match axis_name.to_ascii_lowercase().as_str() {
                "length" | "len" | "l" => Ok(Instruction::SetLineLength {
                    value: value.to_string(),
                }),
                "diameter" | "diam" | "d" => Ok(Instruction::SetCircleDiameter {
                    value: value.to_string(),
                }),
                _ if PlaneDim::from_name(axis_name).is_some() => {
                    match PlaneDim::from_name(axis_name).unwrap() {
                        PlaneDim::Offset => Ok(Instruction::SetPlaneOffset {
                            value: value.to_string(),
                        }),
                        PlaneDim::Angle => Ok(Instruction::SetPlaneAngle {
                            value: value.to_string(),
                        }),
                    }
                }
                _ => {
                    let axis = RectAxis::from_name(axis_name)
                        .ok_or_else(|| err(&format!("unknown axis '{axis_name}'")))?;
                    Ok(Instruction::SetDim {
                        axis,
                        value: value.to_string(),
                    })
                }
            }
        }

        "focus_dim" | "focusdim" => {
            let axis_name = rest.split_whitespace().next().unwrap_or("");
            match axis_name.to_ascii_lowercase().as_str() {
                "length" | "len" | "l" => Ok(Instruction::FocusLineLength),
                "diameter" | "diam" | "d" => Ok(Instruction::FocusCircleDiameter),
                _ if PlaneDim::from_name(axis_name).is_some() => {
                    Ok(Instruction::FocusPlaneDim(PlaneDim::from_name(axis_name).unwrap()))
                }
                _ => {
                    let axis = RectAxis::from_name(axis_name)
                        .ok_or_else(|| err(&format!("unknown axis '{axis_name}'")))?;
                    Ok(Instruction::FocusDim(axis))
                }
            }
        }

        "orbit" | "right_drag" | "rightdrag" => {
            let (dx, dy) = parse_two_floats(rest, &err)?;
            Ok(Instruction::Orbit { dx, dy })
        }

        "pan" | "right_drag_shift" | "rightdragshift" => {
            let (dx, dy) = parse_two_floats(rest, &err)?;
            Ok(Instruction::Pan { dx, dy })
        }

        "zoom" | "scroll" => {
            let delta = parse_one_float(rest, &err)?;
            Ok(Instruction::Zoom { scroll: delta })
        }

        "view" => {
            let mut parts = rest.split_whitespace();
            let first = parts.next().unwrap_or("");
            match first {
                "edge" => {
                    let name = parts.next().ok_or_else(|| err("view edge requires a name"))?;
                    CubeEdgeId::from_name(name)
                        .map(Instruction::ViewEdge)
                        .ok_or_else(|| err(&format!("unknown cube edge '{name}'")))
                }
                "corner" => {
                    let name = parts.next().ok_or_else(|| err("view corner requires a name"))?;
                    CubeCornerId::from_name(name)
                        .map(Instruction::ViewCorner)
                        .ok_or_else(|| err(&format!("unknown cube corner '{name}'")))
                }
                _ => ProjectionMode::from_name(first)
                    .map(Instruction::ProjectionMode)
                    .or_else(|| {
                        StandardView::from_name(first).map(Instruction::View)
                    })
                    .ok_or_else(|| err(&format!("unknown view '{first}'"))),
            }
        }

        "toggle_projection" | "projection_toggle" | "toggle_view" | "view_toggle" => {
            Ok(Instruction::ToggleProjectionMode)
        }

        "view_home" | "home" | "camera_home" => Ok(Instruction::ViewHome),

        "set_home_view" | "sethomeview" | "set_home" => Ok(Instruction::SetHomeView),

        "move" | "mousemove" => {
            let (x, y) = parse_two_floats(rest, &err)?;
            Ok(Instruction::Move { x, y })
        }

        "click" => {
            let (x, y) = parse_two_floats(rest, &err)?;
            Ok(Instruction::Click { x, y })
        }

        "move_ground" | "moveground" => {
            let (x, y) = parse_two_floats(rest, &err)?;
            Ok(Instruction::MoveGround { x, y })
        }

        "click_ground" | "clickground" => {
            let (x, y) = parse_two_floats(rest, &err)?;
            Ok(Instruction::ClickGround { x, y })
        }

        "drag" => {
            let parts: Vec<f32> = rest
                .split_whitespace()
                .map(|s| s.parse::<f32>())
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| err("drag requires four numbers: x0 y0 x1 y1"))?;
            if parts.len() != 4 {
                return Err(err("drag requires four numbers: x0 y0 x1 y1"));
            }
            Ok(Instruction::Drag {
                x0: parts[0],
                y0: parts[1],
                x1: parts[2],
                y1: parts[3],
            })
        }

        "right_drag_rel" => {
            let (dx, dy) = parse_two_floats(rest, &err)?;
            Ok(Instruction::RightDrag { dx, dy })
        }

        "right_drag_pan" => {
            let (dx, dy) = parse_two_floats(rest, &err)?;
            Ok(Instruction::RightDragShift { dx, dy })
        }

        "wheel" => {
            let delta = parse_one_float(rest, &err)?;
            Ok(Instruction::Scroll { delta })
        }

        "key" => {
            let key_name = rest.split_whitespace().next().unwrap_or("");
            parse_key(key_name).map(Instruction::Key).map_err(|m| err(&m))
        }

        "keydown" => {
            let key_name = rest.split_whitespace().next().unwrap_or("");
            parse_key(key_name)
                .map(Instruction::KeyDown)
                .map_err(|m| err(&m))
        }

        "keyup" => {
            let key_name = rest.split_whitespace().next().unwrap_or("");
            parse_key(key_name)
                .map(Instruction::KeyUp)
                .map_err(|m| err(&m))
        }

        "type" => {
            let text = parse_type_text(rest);
            Ok(Instruction::Type(text))
        }

        "wait" => {
            if rest.ends_with("ms") {
                let ms: u64 = rest
                    .trim_end_matches("ms")
                    .trim()
                    .parse()
                    .map_err(|_| err("wait requires a duration like 100ms or 5"))?;
                Ok(Instruction::WaitMs(ms))
            } else {
                let frames: u32 = rest
                    .parse()
                    .map_err(|_| err("wait requires a frame count or duration like 100ms"))?;
                Ok(Instruction::WaitFrames(frames))
            }
        }

        "screenshot" => {
            let path = rest.trim_matches('"');
            if path.is_empty() {
                return Err(err("screenshot requires an output path"));
            }
            Ok(Instruction::Screenshot(path.to_string()))
        }

        _ => Err(err(&format!("unknown instruction '{cmd}'"))),
    }
}

fn parse_type_text(rest: &str) -> String {
    let rest = rest.trim();
    if (rest.starts_with('"') && rest.ends_with('"')) || (rest.starts_with('\'') && rest.ends_with('\'')) {
        rest[1..rest.len() - 1].to_string()
    } else {
        rest.to_string()
    }
}

fn parse_one_float(rest: &str, err: &impl Fn(&str) -> ParseError) -> Result<f32, ParseError> {
    rest.split_whitespace()
        .next()
        .ok_or_else(|| err("expected a number"))?
        .parse()
        .map_err(|_| err("expected a number"))
}

fn parse_two_floats(rest: &str, err: &impl Fn(&str) -> ParseError) -> Result<(f32, f32), ParseError> {
    let mut parts = rest.split_whitespace();
    let x: f32 = parts
        .next()
        .ok_or_else(|| err("expected two numbers"))?
        .parse()
        .map_err(|_| err("expected a number"))?;
    let y: f32 = parts
        .next()
        .ok_or_else(|| err("expected two numbers"))?
        .parse()
        .map_err(|_| err("expected a number"))?;
    Ok((x, y))
}

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
        },
        SceneElement::Sketch(i) => ElementScriptTokens {
            kind: "sketch",
            index: i,
            edge: None,
        },
        SceneElement::Rect(i) => ElementScriptTokens {
            kind: "rect",
            index: i,
            edge: None,
        },
        SceneElement::Line(i) => ElementScriptTokens {
            kind: "line",
            index: i,
            edge: None,
        },
        SceneElement::Circle(i) => ElementScriptTokens {
            kind: "circle",
            index: i,
            edge: None,
        },
        SceneElement::RectEdge(i, edge) => ElementScriptTokens {
            kind: "rect",
            index: i,
            edge: Some(edge),
        },
        SceneElement::Constraint(i) => ElementScriptTokens {
            kind: "constraint",
            index: i,
            edge: None,
        },
    }
}

fn parse_construction_value(value: &str, line_no: usize) -> Result<bool, ParseError> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        other => Err(ParseError {
            line: line_no,
            message: format!(
                "unknown construction value '{other}' (expected true or false)"
            ),
        }),
    }
}

fn is_additive_token(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "add" | "additive" | "+"
    )
}

fn parse_scene_element_with_optional_edge<'a, I>(
    kind: &str,
    index: usize,
    parts: &mut I,
    line_no: usize,
) -> Result<(SceneElement, Option<&'a str>), ParseError>
where
    I: Iterator<Item = &'a str>,
{
    let unknown_kind = || ParseError {
        line: line_no,
        message: format!("unknown element kind '{kind}'"),
    };
    let next = parts.next();
    if let Some(token) = next {
        if token.eq_ignore_ascii_case("edge") {
            let edge_name = parts.next().ok_or_else(|| ParseError {
                line: line_no,
                message: "rectangle edge name required after 'edge'".to_string(),
            })?;
            let edge = RectEdge::from_name(edge_name).ok_or_else(|| ParseError {
                line: line_no,
                message: format!("unknown rectangle edge '{edge_name}'"),
            })?;
            return Ok((SceneElement::RectEdge(index, edge), parts.next()));
        }
        if matches!(kind.to_ascii_lowercase().as_str(), "rect" | "rectangle") {
            if let Some(edge) = RectEdge::from_name(token) {
                return Ok((SceneElement::RectEdge(index, edge), parts.next()));
            }
        }
        if is_additive_token(token) {
            let element = scene_element_from_script(kind, index).ok_or_else(unknown_kind)?;
            return Ok((element, Some(token)));
        }
        let element = scene_element_from_script(kind, index).ok_or_else(unknown_kind)?;
        return Ok((element, Some(token)));
    }
    let element = scene_element_from_script(kind, index).ok_or_else(unknown_kind)?;
    Ok((element, None))
}

fn parse_scene_element_with_tail<'a, I>(
    kind: &str,
    index: usize,
    parts: &mut I,
    line_no: usize,
) -> Result<(SceneElement, bool), ParseError>
where
    I: Iterator<Item = &'a str>,
{
    let (element, tail) =
        parse_scene_element_with_optional_edge(kind, index, parts, line_no)?;
    let additive = tail.is_some_and(is_additive_token);
    Ok((element, additive))
}

fn scene_element_from_script(kind: &str, index: usize) -> Option<SceneElement> {
    match kind.to_ascii_lowercase().as_str() {
        "plane" | "construction_plane" | "constructionplane" => {
            Some(SceneElement::ConstructionPlane(index))
        }
        "sketch" => Some(SceneElement::Sketch(index)),
        "rect" | "rectangle" => Some(SceneElement::Rect(index)),
        "line" => Some(SceneElement::Line(index)),
        "circle" => Some(SceneElement::Circle(index)),
        "constraint" => Some(SceneElement::Constraint(index)),
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

/// Drives a script through the live application, one step at a time.
pub struct ScriptRunner {
    instructions: Vec<Instruction>,
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
        Self {
            instructions,
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

    pub fn from_file(path: &Path) -> Result<Self, ParseError> {
        let runner = Self::new(parse_file(path)?);
        if runner.verbose {
            println!("Running script: {}", path.display());
            println!("---");
        }
        Ok(runner)
    }

    fn log_instruction(&mut self, instr: &Instruction) {
        if self.verbose && self.logged_pc != Some(self.pc) {
            println!("{}", instr.as_line());
            self.logged_pc = Some(self.pc);
        }
    }

    pub fn is_waiting(&self) -> bool {
        self.wait_until.is_some()
            || self.wait_frames_remaining > 0
            || self.screenshot_pending.is_some()
            || self.waiting_view_transition
    }

    /// Advance the script. Returns true if a repaint should be requested.
    pub fn tick(
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
            self.pc += 1;
            self.logged_pc = None;
        }

        if self.wait_frames_remaining > 0 {
            self.wait_frames_remaining -= 1;
            if self.wait_frames_remaining == 0 {
                self.pc += 1;
                self.logged_pc = None;
            }
            return true;
        }

        if self.waiting_view_transition {
            if state.cam.is_transitioning() {
                return true;
            }
            self.waiting_view_transition = false;
            self.pc += 1;
            self.logged_pc = None;
        }

        if self.screenshot_pending.is_some() {
            // Wait for screenshot event to be processed elsewhere.
            return true;
        }

        while self.pc < self.instructions.len() {
            let instr = self.instructions[self.pc].clone();
            self.log_instruction(&instr);
            match self.execute_one(instr, state, synthetic, viewport, ctx) {
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

    /// Called when egui delivers a screenshot response for a pending request.
    pub fn on_screenshot(&mut self, image: &egui::ColorImage) -> Result<(), String> {
        let Some(path) = self.screenshot_pending.take() else {
            return Ok(());
        };
        save_screenshot(&path, image)?;
        self.pc += 1;
        Ok(())
    }
}

enum StepResult {
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

/// CLI options for script execution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptOptions {
    pub script_path: Option<String>,
    pub exit_on_complete: bool,
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
  le3 [options] [script.le3script]

Options:
  --script <path>       Run an instruction script
  --exit, --exit-on-complete
                        Exit after the script finishes
  -h, --help            Show this help and exit

Examples:
  le3
  le3 --script demo.le3script
  le3 demo.le3script --exit
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
            arg if !arg.starts_with('-') && opts.script_path.is_none() => {
                if arg.ends_with(".le3script")
                    || arg.ends_with(".script")
                    || Path::new(arg).extension().is_some_and(|e| e == "le3script")
                {
                    opts.script_path = Some(arg.to_string());
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

    #[test]
    fn parses_basic_instructions() {
        let script = r#"
            # setup
            new
            tool rectangle
            click 100 200
            key enter
            screenshot out.png
        "#;
        let ins = parse(script).unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::New,
                Instruction::Tool(Tool::Rectangle),
                Instruction::Click { x: 100.0, y: 200.0 },
                Instruction::Key(Key::Enter),
                Instruction::Screenshot("out.png".to_string()),
            ]
        );
    }

    #[test]
    fn parses_wait_variants() {
        let ins = parse("wait 100ms\nwait 3").unwrap();
        assert_eq!(
            ins,
            vec![Instruction::WaitMs(100), Instruction::WaitFrames(3)]
        );
    }

    #[test]
    fn parses_type_with_quotes() {
        let ins = parse(r#"type "12.5""#).unwrap();
        assert_eq!(ins, vec![Instruction::Type("12.5".to_string())]);
    }

    #[test]
    fn parses_open_save_paths() {
        let ins = parse("open /tmp/test.le3\nsave /tmp/out.le3").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::Open("/tmp/test.le3".to_string()),
                Instruction::Save(Some("/tmp/out.le3".to_string())),
            ]
        );
    }

    #[test]
    fn parse_error_on_unknown_instruction() {
        let err = parse("foobar").unwrap_err();
        assert_eq!(err.line, 1);
        assert!(err.message.contains("unknown"));
    }

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
    fn parse_cli_run_delegates_to_script_options() {
        assert_eq!(
            parse_cli(["le3", "--script", "test.le3script", "--exit"]),
            CliOutcome::Run(ScriptOptions {
                script_path: Some("test.le3script".to_string()),
                exit_on_complete: true,
            })
        );
    }

    #[test]
    fn parse_args_finds_script_flag() {
        let opts = parse_args(["le3", "--script", "test.le3script", "--exit"]);
        assert_eq!(opts.script_path.as_deref(), Some("test.le3script"));
        assert!(opts.exit_on_complete);
    }

    #[test]
    fn parse_args_finds_positional_script() {
        let opts = parse_args(["le3", "demo.le3script"]);
        assert_eq!(opts.script_path.as_deref(), Some("demo.le3script"));
    }

    #[test]
    fn instruction_as_line_round_trips() {
        let line = "click 100 200";
        let ins = parse(line).unwrap().into_iter().next().unwrap();
        assert_eq!(ins.as_line(), line);
    }

    #[test]
    fn wait_frames_advances_to_next_instruction() {
        let script = "wait 2\nclear";
        let mut runner = ScriptRunner::new(parse(script).unwrap());
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

        // Frame 1: start wait 2
        assert!(runner.tick(&mut state, &mut synthetic, Some(vp), &ctx));
        assert_eq!(runner.pc, 0);
        assert_eq!(runner.wait_frames_remaining, 2);

        // Frame 2: 2 -> 1
        assert!(runner.tick(&mut state, &mut synthetic, Some(vp), &ctx));
        assert_eq!(runner.pc, 0);
        assert_eq!(runner.wait_frames_remaining, 1);

        // Frame 3: 1 -> 0, advance past wait
        assert!(runner.tick(&mut state, &mut synthetic, Some(vp), &ctx));
        assert_eq!(runner.pc, 1);
        assert_eq!(runner.wait_frames_remaining, 0);

        // Frame 4: run clear
        runner.tick(&mut state, &mut synthetic, Some(vp), &ctx);
        assert!(state.doc.rects.is_empty());
        assert!(runner.done);
    }

    #[test]
    fn parses_view_commands() {
        let ins = parse("view front\nview top\nview right").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::View(StandardView::Front),
                Instruction::View(StandardView::Top),
                Instruction::View(StandardView::Right),
            ]
        );
    }

    #[test]
    fn parses_set_home_view_command() {
        let ins = parse("set_home_view\nset_home").unwrap();
        assert_eq!(
            ins,
            vec![Instruction::SetHomeView, Instruction::SetHomeView]
        );
    }

    #[test]
    fn parses_view_home_command() {
        let ins = parse("view_home\nhome\ncamera_home").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::ViewHome,
                Instruction::ViewHome,
                Instruction::ViewHome,
            ]
        );
    }

    #[test]
    fn parses_projection_mode_commands() {
        let ins = parse("view orthographic\nview natural\ntoggle_projection").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::ProjectionMode(ProjectionMode::Orthographic),
                Instruction::ProjectionMode(ProjectionMode::Natural),
                Instruction::ToggleProjectionMode,
            ]
        );
    }

    #[test]
    fn parses_edit_plane_commands() {
        let ins = parse("edit_plane 1\ncommit_plane").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::BeginEditConstructionPlane { index: 1 },
                Instruction::CommitConstructionPlane,
            ]
        );
    }

    #[test]
    fn parses_edit_sketch_alias() {
        let ins = parse("edit_sketch 2\nopen_sketch 2").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::OpenSketch { sketch: 2 },
                Instruction::OpenSketch { sketch: 2 },
            ]
        );
    }

    #[test]
    fn parses_tree_pane_commands() {
        let ins = parse("pane tree show\npane hierarchy hide\npane dag toggle").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::SetPane {
                    pane: Pane::Hierarchy,
                    visible: Some(true),
                },
                Instruction::SetPane {
                    pane: Pane::Hierarchy,
                    visible: Some(false),
                },
                Instruction::SetPane {
                    pane: Pane::Hierarchy,
                    visible: None,
                },
            ]
        );
    }

    #[test]
    fn parses_pane_commands() {
        let ins = parse("pane view_cube show\npane cube hide\npane hud toggle\npane viewcube")
            .unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::SetPane {
                    pane: Pane::ViewCube,
                    visible: Some(true),
                },
                Instruction::SetPane {
                    pane: Pane::ViewCube,
                    visible: Some(false),
                },
                Instruction::SetPane {
                    pane: Pane::ViewCube,
                    visible: None,
                },
                Instruction::SetPane {
                    pane: Pane::ViewCube,
                    visible: None,
                },
            ]
        );
    }

    #[test]
    fn pane_command_round_trips_through_as_line() {
        for line in ["pane view_cube show", "pane view_cube hide", "pane view_cube toggle"] {
            let ins = parse(line).unwrap();
            assert_eq!(ins[0].as_line(), line);
        }
    }

    #[test]
    fn parses_context_pane_commands() {
        let ins = parse("pane context show\npane context hide\npane context toggle").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::SetPane {
                    pane: Pane::Context,
                    visible: Some(true),
                },
                Instruction::SetPane {
                    pane: Pane::Context,
                    visible: Some(false),
                },
                Instruction::SetPane {
                    pane: Pane::Context,
                    visible: None,
                },
            ]
        );
    }

    #[test]
    fn parses_set_name_commands() {
        let ins = parse("set_name line 0 Guide\nfocus_name\nrename rect 1 My box").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::SetElementName {
                    element: SceneElement::Line(0),
                    name: "Guide".to_string(),
                },
                Instruction::FocusElementName,
                Instruction::SetElementName {
                    element: SceneElement::Rect(1),
                    name: "My box".to_string(),
                },
            ]
        );
    }

    #[test]
    fn parses_apply_and_toggle_construction_commands() {
        let ins = parse("apply_construction true\ntoggle_construction\napply_construction false")
            .unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::ApplyConstruction { construction: true },
                Instruction::ToggleConstruction,
                Instruction::ApplyConstruction { construction: false },
            ]
        );
    }

    #[test]
    fn parses_selection_and_construction_commands() {
        let ins = parse(
            "select rect 0 bottom\nselect line 1 add\nclear_selection\nset_construction rect 0 top true",
        )
        .unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::SelectSceneElement {
                    element: SceneElement::RectEdge(0, RectEdge::Bottom),
                    additive: false,
                },
                Instruction::SelectSceneElement {
                    element: SceneElement::Line(1),
                    additive: true,
                },
                Instruction::ClearSceneSelection,
                Instruction::SetShapeConstruction {
                    element: SceneElement::RectEdge(0, RectEdge::Top),
                    construction: true,
                },
            ]
        );
    }

    #[test]
    fn script_select_and_set_construction() {
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.rects.push(crate::model::Rect::from_local_corners(
            sketch, 0.0, 0.0, 10.0, 5.0,
        ));
        let script = "select rect 0 bottom\nset_construction rect 0 bottom true\nclear_selection";
        let mut runner = ScriptRunner::new(parse(script).unwrap());
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
    fn parses_parameters_pane_commands() {
        let ins = parse("pane parameters show\npane params hide\npane param toggle").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::SetPane {
                    pane: Pane::Parameters,
                    visible: Some(true),
                },
                Instruction::SetPane {
                    pane: Pane::Parameters,
                    visible: Some(false),
                },
                Instruction::SetPane {
                    pane: Pane::Parameters,
                    visible: None,
                },
            ]
        );
    }

    #[test]
    fn rejects_unknown_pane() {
        assert!(parse("pane bogus show").is_err());
        assert!(parse("pane view_cube sideways").is_err());
    }

    #[test]
    fn parses_palette_commands() {
        assert_eq!(
            parse("palette").unwrap(),
            vec![Instruction::SetCommandPalette { open: None }]
        );
        let ins = parse("palette show\npalette hide\npalette toggle\npalette run view top")
            .unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::SetCommandPalette {
                    open: Some(true),
                },
                Instruction::SetCommandPalette {
                    open: Some(false),
                },
                Instruction::SetCommandPalette { open: None },
                Instruction::RunPaletteCommand {
                    query: "view top".to_string(),
                },
            ]
        );
    }

    #[test]
    fn script_rectangle_dimension_uses_parameter_and_updates() {
        let script = "parameter add A 10mm\ntool rectangle\nset_dim width A\nset_dim height 5";
        let mut runner = ScriptRunner::new(parse(script).unwrap());
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
        let mut runner = ScriptRunner::new(parse("palette run view top").unwrap());
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
    fn parses_parameter_commands() {
        let ins = parse(
            "parameter add A 5mm\nparameter value 0 A + 5in\nparameter name 0 Len\nparameter delete 1",
        )
        .unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::AddParameter {
                    name: "A".to_string(),
                    expression: "5mm".to_string(),
                },
                Instruction::SetParameterExpression {
                    index: 0,
                    expression: "A + 5in".to_string(),
                },
                Instruction::SetParameterName {
                    index: 0,
                    name: "Len".to_string(),
                },
                Instruction::DeleteParameter { index: 1 },
            ]
        );
    }

    #[test]
    fn script_adds_and_renames_parameters() {
        let script = "parameter add A 5mm\nparameter add B A+5in\nparameter name 0 Len";
        let mut runner = ScriptRunner::new(parse(script).unwrap());
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
    fn parses_view_edge_and_corner_commands() {
        let ins = parse("view edge front_top\nview corner frt").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::ViewEdge(CubeEdgeId::FrontTop),
                Instruction::ViewCorner(CubeCornerId::FrontRightTop),
            ]
        );
    }

    #[test]
    fn view_command_waits_until_transition_finishes() {
        let script = "view front\nclear";
        let mut runner = ScriptRunner::new(parse(script).unwrap());
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

        assert!(blocked_while_animating, "script should block while the view animates");
        assert!(state.doc.rects.is_empty());
        assert!(runner.done);
    }

    #[test]
    fn parses_line_tool_and_length_dim() {
        let ins = parse("tool line\nset_dim length 25\nfocus_dim length").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::Tool(Tool::Line),
                Instruction::SetLineLength {
                    value: "25".to_string()
                },
                Instruction::FocusLineLength,
            ]
        );
    }

    #[test]
    fn parses_begin_sketch_on_circle_face() {
        let ins = parse("begin_sketch circle 0").unwrap();
        assert_eq!(
            ins,
            vec![Instruction::BeginSketch {
                face: FaceId::Circle(0),
            }]
        );
    }

    #[test]
    fn parses_circle_tool_and_diameter_dim() {
        let ins = parse("tool circle\nset_dim diameter 40\nfocus_dim diameter\nadd_constraint circle 0 40mm")
            .unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::Tool(Tool::Circle),
                Instruction::SetCircleDiameter {
                    value: "40".to_string()
                },
                Instruction::FocusCircleDiameter,
                Instruction::AddDistanceConstraint {
                    target: DistanceTarget::CircleDiameter(0),
                    expression: "40mm".to_string(),
                },
            ]
        );
    }

    #[test]
    fn parses_plane_tool_and_dims() {
        let ins = parse("tool plane\nset_dim offset 12\nset_dim angle 45\nfocus_dim angle")
            .unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::Tool(Tool::ConstructionPlane),
                Instruction::SetPlaneOffset {
                    value: "12".to_string()
                },
                Instruction::SetPlaneAngle {
                    value: "45".to_string()
                },
                Instruction::FocusPlaneDim(PlaneDim::Angle),
            ]
        );
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
        let script = "edit_dim width\nset_dim width 25mm\ncommit_dim";
        let mut runner = ScriptRunner::new(parse(script).unwrap());
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
    fn parses_edit_dim_and_commit_dim() {
        let ins = parse("edit_dim width\ncommit_dim").unwrap();
        assert_eq!(
            ins,
            vec![
                Instruction::BeginEditCommittedDim {
                    axis: DimLabelAxis::Width
                },
                Instruction::CommitCommittedDim,
            ]
        );
    }

    #[test]
    fn parses_set_dim_label_offset() {
        let ins = parse("set_dim_label_offset width 48").unwrap();
        assert_eq!(
            ins,
            vec![Instruction::SetDimLabelOffset {
                axis: DimLabelAxis::Width,
                offset: 48.0,
            }]
        );
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
        let mut runner = ScriptRunner::new(parse("set_dim_label_offset width 60").unwrap());
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
    fn parses_set_dim_expression_with_spaces() {
        let ins = parse("set_dim width 2in + 5mm / 2").unwrap();
        assert_eq!(
            ins,
            vec![Instruction::SetDim {
                axis: RectAxis::Width,
                value: "2in + 5mm / 2".to_string(),
            }]
        );
    }

    #[test]
    fn script_set_dim_commit_displays_computed_mm_not_expression() {
        use crate::value::format_length_display;

        let script = "tool rectangle\nset_dim width 2in\nset_dim height 5mm";
        let mut runner = ScriptRunner::new(parse(script).unwrap());
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
        let script = "tool line\nset_dim length 2in + 5mm / 2";
        let mut runner = ScriptRunner::new(parse(script).unwrap());
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
        let script = "new\nbegin_sketch construction_plane 0\ntool rectangle\nset_dim width 50\norbit 10 5\nclear";
        let mut runner = ScriptRunner::new(parse(script).unwrap());
        runner.verbose = false;
        let mut state = AppState::default();
        let mut synthetic = SyntheticInput::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state
            .doc
            .rects
            .push(crate::model::Rect::from_local_corners(sketch, 0., 0., 1., 1.));

        while !runner.done {
            runner.tick(&mut state, &mut synthetic, Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 40.0),
                egui::vec2(960.0, 560.0),
            )), &egui::Context::default());
        }

        assert!(state.doc.rects.is_empty());
        assert_eq!(state.tool, Tool::Rectangle);
        assert!(runner.error.is_none());
    }
}