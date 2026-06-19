//! Lua scripting API (`paramcad` global) for driving the live application.

use crate::actions::{DimLabelAxis, Pane, RectAxis, Tool};
use crate::camera::{ProjectionMode, StandardView};
use crate::construction::PlaneDim;
use crate::geometric_constraints::GeometricConstraintType;
use crate::hierarchy::SceneElement;
use crate::model::{
    ConstraintLine, ConstraintPoint, DistanceTarget, FaceId, LineEnd, RectEdge, SketchId,
};
use crate::names::find_element_by_name;
use crate::script::{parse_key, Instruction, ScriptRunner, SyntheticInput};
use crate::view_cube::{CubeCornerId, CubeEdgeId};

use crate::actions::AppState;
use eframe::egui::{self, Key};
use mlua::{Lua, MultiValue, Table, UserData, UserDataMethods, Value};
use std::path::Path;

/// Per-tick context passed to Lua callbacks via `Lua::set_app_data`.
pub struct ScriptTickData {
    pub runner: *mut ScriptRunner,
    pub state: *mut AppState,
    pub synthetic: *mut SyntheticInput,
    pub viewport: Option<egui::Rect>,
    pub ctx: *mut egui::Context,
}

unsafe impl Send for ScriptTickData {}
unsafe impl Sync for ScriptTickData {}

impl ScriptTickData {
    pub(crate) unsafe fn runner(&self) -> &mut ScriptRunner {
        &mut *self.runner
    }

    pub(crate) unsafe fn state(&self) -> &mut AppState {
        &mut *self.state
    }

    pub(crate) unsafe fn synthetic(&self) -> &mut SyntheticInput {
        &mut *self.synthetic
    }

    pub(crate) unsafe fn egui_ctx(&self) -> &egui::Context {
        &*self.ctx
    }

    pub(crate) unsafe fn exec(&self, instr: Instruction) -> mlua::Result<()> {
        let runner = self.runner();
        let _ = runner.execute_instruction(
            instr,
            self.state(),
            self.synthetic(),
            self.viewport,
            self.egui_ctx(),
        );
        Ok(())
    }
}

/// A reference to a scene element used by Lua scripts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuaElement {
    pub element: SceneElement,
}

impl UserData for LuaElement {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("kind", |_, this, ()| Ok(element_kind_name(this.element)));
        methods.add_method("index", |_, this, ()| Ok(element_index(this.element)));
    }
}

fn element_kind_name(element: SceneElement) -> &'static str {
    match element {
        SceneElement::ConstructionPlane(_) => "construction_plane",
        SceneElement::Sketch(_) => "sketch",
        SceneElement::Rect(_) | SceneElement::RectEdge(_, _) => "rect",
        SceneElement::Line(_) => "line",
        SceneElement::Circle(_) => "circle",
        SceneElement::Constraint(_) => "constraint",
        SceneElement::Point(_) => "point",
    }
}

fn element_index(element: SceneElement) -> usize {
    match element {
        SceneElement::ConstructionPlane(i)
        | SceneElement::Sketch(i)
        | SceneElement::Rect(i)
        | SceneElement::Line(i)
        | SceneElement::Circle(i)
        | SceneElement::Constraint(i) => i,
        SceneElement::RectEdge(i, _) => i,
        SceneElement::Point(_) => 0,
    }
}

pub fn scene_element_from_kind(kind: &str, index: usize) -> Option<SceneElement> {
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

fn parse_tool(name: &str) -> Option<Tool> {
    match name.to_ascii_lowercase().as_str() {
        "select" => Some(Tool::Select),
        "rectangle" | "rect" => Some(Tool::Rectangle),
        "line" => Some(Tool::Line),
        "circle" => Some(Tool::Circle),
        "plane" | "construction_plane" => Some(Tool::ConstructionPlane),
        "sketch" => Some(Tool::Sketch),
        "dimension" | "dim" => Some(Tool::Dimension),
        "constraint" => Some(Tool::Constraint),
        _ => None,
    }
}

fn parse_rect_axis(name: &str) -> Option<RectAxis> {
    match name.to_ascii_lowercase().as_str() {
        "width" | "w" => Some(RectAxis::Width),
        "height" | "h" => Some(RectAxis::Height),
        _ => None,
    }
}

fn parse_dim_label_axis(name: &str) -> Option<DimLabelAxis> {
    match name.to_ascii_lowercase().as_str() {
        "width" | "w" => Some(DimLabelAxis::Width),
        "height" | "h" => Some(DimLabelAxis::Height),
        "length" | "len" | "l" => Some(DimLabelAxis::Length),

        _ => None,
    }
}

fn parse_plane_dim(name: &str) -> Option<PlaneDim> {
    match name.to_ascii_lowercase().as_str() {
        "offset" => Some(PlaneDim::Offset),
        "angle" => Some(PlaneDim::Angle),
        _ => None,
    }
}

fn parse_standard_view(name: &str) -> Option<StandardView> {
    match name.to_ascii_lowercase().as_str() {
        "front" | "f" => Some(StandardView::Front),
        "back" | "b" => Some(StandardView::Back),
        "left" | "l" => Some(StandardView::Left),
        "right" | "r" => Some(StandardView::Right),
        "top" | "t" => Some(StandardView::Top),
        "bottom" | "bo" => Some(StandardView::Bottom),
        _ => None,
    }
}

fn parse_pane(name: &str) -> Option<Pane> {
    match name.to_ascii_lowercase().as_str() {
        "view_cube" | "viewcube" | "cube" => Some(Pane::ViewCube),
        "hierarchy" | "tree" | "dag" | "elements" => Some(Pane::Hierarchy),
        "parameters" | "params" | "param" => Some(Pane::Parameters),
        "context" | "properties" | "props" => Some(Pane::Context),
        "hud" => Some(Pane::ViewCube),
        _ => None,
    }
}

fn parse_visibility(value: Value) -> mlua::Result<Option<bool>> {
    match value {
        Value::Nil => Ok(None),
        Value::Boolean(b) => Ok(Some(b)),
        Value::String(s) => match s.to_str()?.to_ascii_lowercase().as_str() {
            "show" | "on" | "true" | "yes" | "1" => Ok(Some(true)),
            "hide" | "off" | "false" | "no" | "0" => Ok(Some(false)),
            "toggle" => Ok(None),
            other => Err(mlua::Error::external(format!(
                "unknown visibility value '{other}'"
            ))),
        },
        other => Err(mlua::Error::external(format!(
            "expected boolean or string for visibility, got {other:?}"
        ))),
    }
}

fn parse_bool(value: Value, label: &str) -> mlua::Result<bool> {
    match value {
        Value::Boolean(b) => Ok(b),
        Value::String(s) => match s.to_str()?.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(true),
            "false" | "off" | "no" | "0" => Ok(false),
            other => Err(mlua::Error::external(format!(
                "unknown {label} value '{other}'"
            ))),
        },
        other => Err(mlua::Error::external(format!(
            "expected boolean for {label}, got {other:?}"
        ))),
    }
}

fn make_element(lua: &Lua, element: SceneElement) -> mlua::Result<Value> {
    Ok(Value::UserData(lua.create_userdata(LuaElement { element })?))
}

fn resolve_element(lua: &Lua, value: Value) -> mlua::Result<SceneElement> {
    match value {
        Value::UserData(ud) => {
            if let Ok(el) = ud.borrow::<LuaElement>() {
                return Ok(el.element);
            }
            Err(mlua::Error::external("expected paramcad element"))
        }
        Value::Table(table) => parse_element_table(lua, table),
        Value::String(s) => {
            let tick = lua
                .app_data_ref::<ScriptTickData>()
                .ok_or_else(|| mlua::Error::external("script tick context missing"))?;
            let name = s.to_str()?.to_string();
            unsafe {
                find_element_by_name(&tick.state().doc, &name)
                    .ok_or_else(|| mlua::Error::external(format!("no element named '{name}'")))
            }
        }
        other => Err(mlua::Error::external(format!(
            "expected element, name string, or table, got {other:?}"
        ))),
    }
}

fn parse_element_table(lua: &Lua, table: Table) -> mlua::Result<SceneElement> {
    if let Ok(name) = table.get::<String>("name") {
        let tick = lua
            .app_data_ref::<ScriptTickData>()
            .ok_or_else(|| mlua::Error::external("script tick context missing"))?;
        return unsafe {
            find_element_by_name(&tick.state().doc, &name).ok_or_else(|| {
                mlua::Error::external(format!("no element named '{name}'"))
            })
        };
    }
    let kind: String = table.get("kind").or_else(|_| table.get("type"))?;
    let index: usize = table.get("index")?;
    if let Ok(edge_name) = table.get::<String>("edge") {
        let edge = RectEdge::from_name(&edge_name).ok_or_else(|| {
            mlua::Error::external(format!("unknown rect edge '{edge_name}'"))
        })?;
        return Ok(SceneElement::RectEdge(index, edge));
    }
    scene_element_from_kind(&kind, index)
        .ok_or_else(|| mlua::Error::external(format!("unknown element kind '{kind}'")))
}

fn parse_constraint_line_table(table: Table) -> mlua::Result<ConstraintLine> {
    let kind: String = table.get("kind").or_else(|_| table.get("type"))?;
    let index: usize = table.get("index")?;
    match kind.to_ascii_lowercase().as_str() {
        "line" => Ok(ConstraintLine::Line(index)),
        "rect" | "rectangle" => {
            let edge_name: String = table.get("edge")?;
            let edge = RectEdge::from_name(&edge_name).ok_or_else(|| {
                mlua::Error::external(format!("unknown rect edge '{edge_name}'"))
            })?;
            Ok(ConstraintLine::RectEdge { rect: index, edge })
        }
        other => Err(mlua::Error::external(format!(
            "drag_line target must be line or rect, not '{other}'"
        ))),
    }
}

fn parse_constraint_point_table(table: Table) -> mlua::Result<ConstraintPoint> {
    let kind: String = table.get("kind").or_else(|_| table.get("type"))?;
    let index: usize = table.get("index")?;
    match kind.to_ascii_lowercase().as_str() {
        "line" => {
            let end_name: String = table.get("end")?;
            let end = match end_name.to_ascii_lowercase().as_str() {
                "start" | "0" => LineEnd::Start,
                "end" | "1" => LineEnd::End,
                other => {
                    return Err(mlua::Error::external(format!(
                        "unknown line endpoint '{other}'"
                    )));
                }
            };
            Ok(ConstraintPoint::LineEndpoint { line: index, end })
        }
        "rect" | "rectangle" => {
            let corner: u8 = table.get("corner")?;
            Ok(ConstraintPoint::RectCorner { rect: index, corner })
        }
        "circle" => Ok(ConstraintPoint::CircleCenter(index)),
        other => Err(mlua::Error::external(format!(
            "unknown point parent '{other}'"
        ))),
    }
}

fn parse_geometric_constraint(name: &str) -> Option<GeometricConstraintType> {
    match name.to_ascii_lowercase().as_str() {
        "parallel" => Some(GeometricConstraintType::Parallel),
        "perpendicular" => Some(GeometricConstraintType::Perpendicular),
        "coincident" => Some(GeometricConstraintType::Coincident),
        "midpoint" => Some(GeometricConstraintType::Midpoint),
        "horizontal" => Some(GeometricConstraintType::Horizontal),
        "vertical" => Some(GeometricConstraintType::Vertical),
        _ => None,
    }
}

fn parse_distance_target(table: Table) -> mlua::Result<DistanceTarget> {
    let kind: String = table.get("kind").or_else(|_| table.get("type"))?;
    let index: usize = table.get("index")?;
    match kind.to_ascii_lowercase().as_str() {
        "line" => Ok(DistanceTarget::LineLength(index)),
        "circle" => Ok(DistanceTarget::CircleDiameter(index)),
        "rect" | "rectangle" => {
            let axis_name: String = table.get("axis")?;
            let axis = parse_rect_axis(&axis_name).ok_or_else(|| {
                mlua::Error::external(format!("unknown rectangle axis '{axis_name}'"))
            })?;
            Ok(match axis {
                RectAxis::Width => DistanceTarget::RectWidth(index),
                RectAxis::Height => DistanceTarget::RectHeight(index),
            })
        }
        other => Err(mlua::Error::external(format!(
            "unknown constraint target '{other}'"
        ))),
    }
}

fn apply_optional_name(
    lua: &Lua,
    element: SceneElement,
    opts: Option<Table>,
) -> mlua::Result<()> {
    let Some(opts) = opts else { return Ok(()) };
    let Ok(name) = opts.get::<String>("name") else {
        return Ok(());
    };
    let tick = lua
        .app_data_ref::<ScriptTickData>()
        .ok_or_else(|| mlua::Error::external("script tick context missing"))?;
    unsafe { tick.exec(Instruction::SetElementName { element, name }) }
}

/// Register the global `paramcad` API table on a Lua state.
pub fn register_api(lua: &Lua) -> mlua::Result<()> {
    let api = lua.create_table()?;

    api.set(
        "new",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::New) }
        })?,
    )?;

    api.set(
        "open",
        lua.create_function(|lua, path: String| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Open(path)) }
        })?,
    )?;

    api.set(
        "save",
        lua.create_function(|lua, path: Option<String>| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Save(path)) }
        })?,
    )?;

    api.set(
        "clear",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Clear) }
        })?,
    )?;

    api.set(
        "undo",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Undo) }
        })?,
    )?;

    api.set(
        "quit",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Quit) }
        })?,
    )?;

    api.set(
        "tool",
        lua.create_function(|lua, name: String| {
            let tool = parse_tool(&name)
                .ok_or_else(|| mlua::Error::external(format!("unknown tool '{name}'")))?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Tool(tool)) }
        })?,
    )?;

    api.set(
        "begin_sketch",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_vec();
            let face = if let Some(Value::Table(table)) = args.first() {
                let kind: String = table.get("kind").or_else(|_| table.get("type"))?;
                let index: usize = table.get("index")?;
                FaceId::from_script(&kind, index).ok_or_else(|| {
                    mlua::Error::external(format!("unknown sketch face kind '{kind}'"))
                })?
            } else {
                let kind = match args.first() {
                    Some(Value::String(s)) => s.to_str()?.to_string(),
                    _ => return Err(mlua::Error::external("begin_sketch requires face kind")),
                };
                let index = match args.get(1) {
                    Some(Value::Integer(i)) => *i as usize,
                    Some(Value::Number(n)) => n.round() as usize,
                    _ => return Err(mlua::Error::external("begin_sketch requires face index")),
                };
                FaceId::from_script(&kind, index).ok_or_else(|| {
                    mlua::Error::external(format!("unknown sketch face kind '{kind}'"))
                })?
            };
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::BeginSketch { face }) }
        })?,
    )?;

    api.set(
        "open_sketch",
        lua.create_function(|lua, sketch: SketchId| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::OpenSketch { sketch }) }
        })?,
    )?;

    api.set(
        "exit_sketch",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::ExitSketch) }
        })?,
    )?;

    api.set(
        "element",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let element = scene_element_from_kind(&kind, index).ok_or_else(|| {
                mlua::Error::external(format!("unknown element kind '{kind}'"))
            })?;
            make_element(lua, element)
        })?,
    )?;

    api.set(
        "find",
        lua.create_function(|lua, name: String| {
            let tick = lua
                .app_data_ref::<ScriptTickData>()
                .ok_or_else(|| mlua::Error::external("script tick context missing"))?;
            let element = unsafe { find_element_by_name(&tick.state().doc, &name) };
            match element {
                Some(element) => Ok(Some(make_element(lua, element)?)),
                None => Ok(None),
            }
        })?,
    )?;

    api.set(
        "set_name",
        lua.create_function(|lua, (element, name): (Value, String)| {
            let element = resolve_element(lua, element)?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::SetElementName { element, name }) }
        })?,
    )?;

    api.set(
        "focus_name",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::FocusElementName) }
        })?,
    )?;

    api.set(
        "select",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_vec();
            let additive = matches!(args.last(), Some(Value::Boolean(true)))
                || matches!(
                    args.last(),
                    Some(Value::Table(t)) if t.get::<bool>("additive").unwrap_or(false)
                );
            let element_value = args.remove(0);
            let element = resolve_element(lua, element_value)?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe {
                tick.exec(Instruction::SelectSceneElement { element, additive },
                )
            }
        })?,
    )?;

    api.set(
        "clear_selection",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::ClearSceneSelection) }
        })?,
    )?;

    api.set(
        "set_visible",
        lua.create_function(|lua, (element, visible): (Value, Value)| {
            let element = resolve_element(lua, element)?;
            let visible = parse_visibility(visible)?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe {
                tick.exec(Instruction::SetElementVisible { element, visible },
                )
            }
        })?,
    )?;

    api.set(
        "set_construction",
        lua.create_function(|lua, (element, construction): (Value, Value)| {
            let element = resolve_element(lua, element)?;
            let construction = parse_bool(construction, "construction")?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe {
                tick.exec(Instruction::SetShapeConstruction {
                        element,
                        construction,
                    },
                )
            }
        })?,
    )?;

    api.set(
        "apply_construction",
        lua.create_function(|lua, construction: Value| {
            let construction = parse_bool(construction, "construction")?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::ApplyConstruction { construction }) }
        })?,
    )?;

    api.set(
        "toggle_construction",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::ToggleConstruction) }
        })?,
    )?;

    api.set(
        "set_dim",
        lua.create_function(|lua, (axis, value): (String, String)| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            if let Some(axis) = parse_rect_axis(&axis) {
                return unsafe { tick.exec(Instruction::SetDim { axis, value }) };
            }
            if axis.eq_ignore_ascii_case("length") || axis.eq_ignore_ascii_case("len") {
                return unsafe { tick.exec(Instruction::SetLineLength { value }) };
            }
            if axis.eq_ignore_ascii_case("diameter") || axis.eq_ignore_ascii_case("diam") {
                return unsafe { tick.exec(Instruction::SetCircleDiameter { value }) };
            }
            if axis.eq_ignore_ascii_case("offset") {
                return unsafe { tick.exec(Instruction::SetPlaneOffset { value }) };
            }
            if axis.eq_ignore_ascii_case("angle") {
                return unsafe { tick.exec(Instruction::SetPlaneAngle { value }) };
            }
            Err(mlua::Error::external(format!("unknown dimension '{axis}'")))
        })?,
    )?;

    api.set(
        "focus_dim",
        lua.create_function(|lua, axis: String| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            if let Some(axis) = parse_rect_axis(&axis) {
                return unsafe { tick.exec(Instruction::FocusDim(axis)) };
            }
            if axis.eq_ignore_ascii_case("length") {
                return unsafe { tick.exec(Instruction::FocusLineLength) };
            }
            if axis.eq_ignore_ascii_case("diameter") {
                return unsafe { tick.exec(Instruction::FocusCircleDiameter) };
            }
            if let Some(dim) = parse_plane_dim(&axis) {
                return unsafe { tick.exec(Instruction::FocusPlaneDim(dim)) };
            }
            Err(mlua::Error::external(format!("unknown dimension '{axis}'")))
        })?,
    )?;

    api.set(
        "edit_dim",
        lua.create_function(|lua, axis: String| {
            let axis = parse_dim_label_axis(&axis)
                .ok_or_else(|| mlua::Error::external(format!("unknown dimension '{axis}'")))?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::BeginEditCommittedDim { axis }) }
        })?,
    )?;

    api.set(
        "commit_dim",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::CommitCommittedDim) }
        })?,
    )?;

    api.set(
        "set_dim_label_offset",
        lua.create_function(|lua, (axis, offset): (String, f32)| {
            let axis = parse_dim_label_axis(&axis)
                .ok_or_else(|| mlua::Error::external(format!("unknown dimension '{axis}'")))?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::SetDimLabelOffset { axis, offset }) }
        })?,
    )?;

    api.set(
        "add_constraint",
        lua.create_function(|lua, (target, expression): (Table, String)| {
            let target = parse_distance_target(target)?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe {
                tick.exec(Instruction::AddDistanceConstraint { target, expression },
                )
            }
        })?,
    )?;

    api.set(
        "add_geometric_constraint",
        lua.create_function(|lua, name: String| {
            let kind = parse_geometric_constraint(&name).ok_or_else(|| {
                mlua::Error::external(format!("unknown geometric constraint '{name}'"))
            })?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::AddGeometricConstraint(kind)) }
        })?,
    )?;

    api.set(
        "constraint_shortcut",
        lua.create_function(|lua, index: u8| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::ApplyConstraintShortcut(index)) }
        })?,
    )?;

    api.set(
        "drag_vertex",
        lua.create_function(|lua, (point, u, v): (Table, f32, f32)| {
            let point = parse_constraint_point_table(point)?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::DragVertex { point, u, v }) }
        })?,
    )?;

    api.set(
        "drag_line",
        lua.create_function(
            |lua, (target, anchor_u, anchor_v, u, v): (Table, f32, f32, f32, f32)| {
                let target = parse_constraint_line_table(target)?;
                let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
                unsafe {
                    tick.exec(Instruction::DragLineSegment {
                            target,
                            anchor_u,
                            anchor_v,
                            u,
                            v,
                        },
                    )
                }
            },
        )?,
    )?;

    api.set(
        "edit_plane",
        lua.create_function(|lua, index: usize| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::BeginEditConstructionPlane { index }) }
        })?,
    )?;

    api.set(
        "commit_plane",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::CommitConstructionPlane) }
        })?,
    )?;

    api.set(
        "orbit",
        lua.create_function(|lua, (dx, dy): (f32, f32)| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Orbit { dx, dy }) }
        })?,
    )?;

    api.set(
        "pan",
        lua.create_function(|lua, (dx, dy): (f32, f32)| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Pan { dx, dy }) }
        })?,
    )?;

    api.set(
        "wheel",
        lua.create_function(|lua, scroll: f32| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Zoom { scroll }) }
        })?,
    )?;

    api.set(
        "_view",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_vec();
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            let first = args
                .first()
                .ok_or_else(|| mlua::Error::external("view requires an argument"))?;
            match first {
                Value::String(s) => {
                    let name = s.to_str()?.to_string();
                    if name.eq_ignore_ascii_case("orthographic") {
                        return unsafe {
                            tick.exec(Instruction::ProjectionMode(ProjectionMode::Orthographic))
                        };
                    }
                    if name.eq_ignore_ascii_case("natural") {
                        return unsafe {
                            tick.exec(Instruction::ProjectionMode(ProjectionMode::Natural))
                        };
                    }
                    if name.eq_ignore_ascii_case("edge") {
                        let edge_name = match args.get(1) {
                            Some(Value::String(s)) => s.to_str()?.as_ref().to_string(),
                            _ => return Err(mlua::Error::external("view edge requires edge id")),
                        };
                        let edge = CubeEdgeId::from_name(&edge_name).ok_or_else(|| {
                            mlua::Error::external(format!("unknown view edge '{edge_name}'"))
                        })?;
                        return unsafe { tick.exec(Instruction::ViewEdge(edge)) };
                    }
                    if name.eq_ignore_ascii_case("corner") {
                        let corner_name = match args.get(1) {
                            Some(Value::String(s)) => s.to_str()?.as_ref().to_string(),
                            _ => {
                                return Err(mlua::Error::external("view corner requires corner id"))
                            }
                        };
                        let corner = CubeCornerId::from_name(&corner_name).ok_or_else(|| {
                            mlua::Error::external(format!("unknown view corner '{corner_name}'"))
                        })?;
                        return unsafe { tick.exec(Instruction::ViewCorner(corner)) };
                    }
                    let view = parse_standard_view(&name).ok_or_else(|| {
                        mlua::Error::external(format!("unknown standard view '{name}'"))
                    })?;
                    unsafe { tick.exec(Instruction::View(view)) }
                }
                other => Err(mlua::Error::external(format!(
                    "view expects a string, got {other:?}"
                ))),
            }
        })?,
    )?;

    api.set(
        "_view_home",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::ViewHome) }
        })?,
    )?;

    api.set(
        "set_home_view",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::SetHomeView) }
        })?,
    )?;

    api.set(
        "toggle_projection",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::ToggleProjectionMode) }
        })?,
    )?;

    api.set(
        "pane",
        lua.create_function(|lua, (pane, visible): (String, Value)| {
            let pane = parse_pane(&pane)
                .ok_or_else(|| mlua::Error::external(format!("unknown pane '{pane}'")))?;
            let visible = parse_visibility(visible)?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::SetPane { pane, visible }) }
        })?,
    )?;

    api.set(
        "parameter",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_vec();
            let action = match args.first() {
                Some(Value::String(s)) => s.to_str()?.to_ascii_lowercase(),
                _ => return Err(mlua::Error::external("parameter requires action")),
            };
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            match action.as_str() {
                "add" => {
                    let name = match args.get(1) {
                        Some(Value::String(s)) => s.to_str()?.to_string(),
                        _ => return Err(mlua::Error::external("parameter add requires name")),
                    };
                    let expression = match args.get(2) {
                        Some(Value::String(s)) => s.to_str()?.to_string(),
                        _ => {
                            return Err(mlua::Error::external(
                                "parameter add requires expression",
                            ))
                        }
                    };
                    unsafe {
                        tick.exec(Instruction::AddParameter { name, expression },
                        )
                    }
                }
                "value" | "expression" => {
                    let index = match args.get(1) {
                        Some(Value::Integer(i)) => *i as usize,
                        Some(Value::Number(n)) => n.round() as usize,
                        _ => return Err(mlua::Error::external("parameter value requires index")),
                    };
                    let expression = match args.get(2) {
                        Some(Value::String(s)) => s.to_str()?.to_string(),
                        _ => {
                            return Err(mlua::Error::external(
                                "parameter value requires expression",
                            ))
                        }
                    };
                    unsafe {
                        tick.exec(Instruction::SetParameterExpression { index, expression },
                        )
                    }
                }
                "name" => {
                    let index = match args.get(1) {
                        Some(Value::Integer(i)) => *i as usize,
                        Some(Value::Number(n)) => n.round() as usize,
                        _ => return Err(mlua::Error::external("parameter name requires index")),
                    };
                    let name = match args.get(2) {
                        Some(Value::String(s)) => s.to_str()?.to_string(),
                        _ => return Err(mlua::Error::external("parameter name requires name")),
                    };
                    unsafe {
                        tick.exec(Instruction::SetParameterName { index, name })
                    }
                }
                "delete" => {
                    let index = match args.get(1) {
                        Some(Value::Integer(i)) => *i as usize,
                        Some(Value::Number(n)) => n.round() as usize,
                        _ => return Err(mlua::Error::external("parameter delete requires index")),
                    };
                    unsafe { tick.exec(Instruction::DeleteParameter { index }) }
                }
                other => Err(mlua::Error::external(format!(
                    "unknown parameter action '{other}'"
                ))),
            }
        })?,
    )?;

    api.set(
        "delete_selection",
        lua.create_function(|lua, ()| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::DeleteSelection) }
        })?,
    )?;

    api.set(
        "palette",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_vec();
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            if args.is_empty() {
                return unsafe { tick.exec(Instruction::SetCommandPalette { open: None }) };
            }
            match args.first() {
                Some(Value::String(s)) if s.to_str()? == "run" => {
                    let query = match args.get(1) {
                        Some(Value::String(s)) => s.to_str()?.to_string(),
                        _ => return Err(mlua::Error::external("palette run requires query")),
                    };
                    unsafe { tick.exec(Instruction::RunPaletteCommand { query }) }
                }
                Some(Value::String(s)) => {
                    let verb = s.to_str()?.to_ascii_lowercase();
                    let open = match verb.as_str() {
                        "show" | "open" => Some(true),
                        "hide" | "close" => Some(false),
                        "toggle" => None,
                        other => {
                            return Err(mlua::Error::external(format!(
                                "unknown palette action '{other}'"
                            )))
                        }
                    };
                    unsafe { tick.exec(Instruction::SetCommandPalette { open }) }
                }
                _ => Err(mlua::Error::external("palette expects a string action")),
            }
        })?,
    )?;

    api.set(
        "move",
        lua.create_function(|lua, (x, y): (f32, f32)| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Move { x, y }) }
        })?,
    )?;

    api.set(
        "click",
        lua.create_function(|lua, (x, y): (f32, f32)| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Click { x, y }) }
        })?,
    )?;

    api.set(
        "move_ground",
        lua.create_function(|lua, (x, y): (f32, f32)| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::MoveGround { x, y }) }
        })?,
    )?;

    api.set(
        "click_ground",
        lua.create_function(|lua, (x, y): (f32, f32)| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::ClickGround { x, y }) }
        })?,
    )?;

    api.set(
        "drag",
        lua.create_function(|lua, (x0, y0, x1, y1): (f32, f32, f32, f32)| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Drag { x0, y0, x1, y1 }) }
        })?,
    )?;

    api.set(
        "right_drag",
        lua.create_function(|lua, (dx, dy): (f32, f32)| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::RightDrag { dx, dy }) }
        })?,
    )?;

    api.set(
        "right_drag_pan",
        lua.create_function(|lua, (dx, dy): (f32, f32)| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::RightDragShift { dx, dy }) }
        })?,
    )?;

    api.set(
        "key",
        lua.create_function(|lua, name: String| {
            let key = parse_key(&name)
                .map_err(|e| mlua::Error::external(e))?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Key(key)) }
        })?,
    )?;

    api.set(
        "keydown",
        lua.create_function(|lua, name: String| {
            let key = parse_key(&name)
                .map_err(|e| mlua::Error::external(e))?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::KeyDown(key)) }
        })?,
    )?;

    api.set(
        "keyup",
        lua.create_function(|lua, name: String| {
            let key = parse_key(&name)
                .map_err(|e| mlua::Error::external(e))?;
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::KeyUp(key)) }
        })?,
    )?;

    api.set(
        "type",
        lua.create_function(|lua, text: String| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Type(text)) }
        })?,
    )?;

    api.set(
        "_wait",
        lua.create_function(|lua, frames: u32| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::WaitFrames(frames)) }
        })?,
    )?;

    api.set(
        "_wait_ms",
        lua.create_function(|lua, ms: u64| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::WaitMs(ms)) }
        })?,
    )?;

    api.set(
        "_screenshot",
        lua.create_function(|lua, path: String| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            unsafe { tick.exec(Instruction::Screenshot(path)) }
        })?,
    )?;

    api.set(
        "rect",
        lua.create_function(|lua, opts: Table| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            let width: f32 = opts.get("width")?;
            let height: f32 = opts.get("height")?;
            let x: f32 = opts.get("x").unwrap_or(0.0);
            let y: f32 = opts.get("y").unwrap_or(0.0);
            unsafe {
                tick.exec(Instruction::Tool(Tool::Rectangle))?;
                tick.exec(Instruction::SetDim {
                    axis: RectAxis::Width,
                    value: width.to_string(),
                })?;
                tick.exec(Instruction::SetDim {
                    axis: RectAxis::Height,
                    value: height.to_string(),
                })?;
                tick.exec(Instruction::ClickGround {
                    x,
                    y: y + height,
                })?;
                tick.exec(Instruction::Key(Key::Enter))?;
            }
            let element = SceneElement::Rect(unsafe { tick.state().doc.rects.len().saturating_sub(1) });
            apply_optional_name(lua, element, Some(opts))
        })?,
    )?;

    api.set(
        "line",
        lua.create_function(|lua, opts: Table| {
            let tick = lua.app_data_ref::<ScriptTickData>().unwrap();
            let length: f32 = opts.get("length")?;
            let x: f32 = opts.get("x").unwrap_or(0.0);
            let y: f32 = opts.get("y").unwrap_or(0.0);
            unsafe {
                tick.exec(Instruction::Tool(Tool::Line))?;
                tick.exec(Instruction::SetLineLength {
                    value: length.to_string(),
                })?;
                tick.exec(Instruction::ClickGround { x, y })?;
                tick.exec(Instruction::Key(Key::Enter))?;
            }
            let element = SceneElement::Line(unsafe { tick.state().doc.lines.len().saturating_sub(1) });
            apply_optional_name(lua, element, Some(opts))
        })?,
    )?;

    lua.globals().set("paramcad", api)?;
    lua.load(
        r#"
        local function yielding(name, native_name)
            local native = paramcad[native_name or name]
            paramcad[name] = function(...)
                native(...)
                coroutine.yield()
            end
        end
        yielding("wait", "_wait")
        yielding("wait_ms", "_wait_ms")
        yielding("screenshot", "_screenshot")
        yielding("view", "_view")
        yielding("view_home", "_view_home")
    "#,
    )
    .exec()?;
    Ok(())
}

/// Load a `.lua` script file into a coroutine thread.
pub fn load_script(lua: &Lua, path: &Path) -> mlua::Result<mlua::Thread> {
    let source = std::fs::read_to_string(path).map_err(|e| {
        mlua::Error::external(format!("failed to read {}: {e}", path.display()))
    })?;
    register_api(lua)?;
    let func = lua.load(&source).set_name(path.to_string_lossy()).into_function()?;
    lua.create_thread(func)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::AppState;
    use crate::model::FaceId;

    fn run_lua(source: &str) -> AppState {
        let mut runner = ScriptRunner::from_lua_source(source).unwrap();
        runner.verbose = false;
        let mut state = AppState::default();
        let mut synthetic = SyntheticInput::default();
        let ctx = egui::Context::default();
        let vp = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(960.0, 560.0));
        while !runner.done {
            runner.tick(&mut state, &mut synthetic, Some(vp), &ctx);
        }
        state
    }

    #[test]
    fn lua_new_and_tool() {
        let state = run_lua(
            r#"
            paramcad.new()
            paramcad.begin_sketch("construction_plane", 0)
            paramcad.tool("rectangle")
        "#,
        );
        assert_eq!(state.tool, Tool::Rectangle);
        assert!(state.sketch_session.is_some());
    }

    #[test]
    fn lua_find_and_set_name() {
        let mut runner = ScriptRunner::from_lua_source(
            r#"
            paramcad.set_name({ kind = "line", index = 0 }, "Main box")
            local found = paramcad.find("Main box")
            assert(found ~= nil)
        "#,
        )
        .unwrap();
        runner.verbose = false;
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.lines.push(crate::model::Line::from_local_endpoints(
            sketch, 0.0, 0.0, 10.0, 0.0,
        ));
        let mut synthetic = SyntheticInput::default();
        let ctx = egui::Context::default();
        while !runner.done {
            runner.tick(&mut state, &mut synthetic, None, &ctx);
        }
        assert_eq!(
            find_element_by_name(&state.doc, "Main box"),
            Some(SceneElement::Line(0))
        );
    }

    #[test]
    fn lua_wait_frames_advances() {
        let mut runner = ScriptRunner::from_lua_source(
            r#"
            paramcad.wait(2)
            paramcad.clear()
        "#,
        )
        .unwrap();
        runner.verbose = false;
        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.rects.push(crate::model::Rect::from_local_corners(
            sketch, 0., 0., 1., 1.,
        ));
        let mut synthetic = SyntheticInput::default();
        let ctx = egui::Context::default();
        let vp = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(960.0, 560.0));
        while !runner.done {
            runner.tick(&mut state, &mut synthetic, Some(vp), &ctx);
        }
        assert!(state.doc.rects.is_empty());
    }
}