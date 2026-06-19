//! Document parameters: named length expressions that drive sketch dimensions.

use crate::actions::{Action, ActionResult, AppState};
use crate::constraints::{propagate_parameter_rename_to_constraints, solve_document_constraints};
use crate::icons::{icon_button, IconId};
use crate::document_health::HealthStatus;
use crate::model::{Document, Parameter};
use crate::value::{
    eval_length_mm_in_doc, eval_length_mm_with_params, expression_references_document_parameter,
    format_length_display, format_unknown_variable_error, is_valid_parameter_name,
    parameter_names_referenced_in_expression, substitute_parameter_name,
    unknown_variables_in_parameter_expression,
};
use eframe::egui::{self, Color32, Id, Key, RichText};

pub const PANE_TITLE: &str = "Parameters";

const NEW_NAME_ID: &str = "le3_parameters_new_name";
const NEW_VALUE_ID: &str = "le3_parameters_new_value";
const INVALID_TEXT: Color32 = Color32::from_rgb(220, 80, 80);
const UNSTABLE_TEXT: Color32 = Color32::from_rgb(255, 180, 60);

fn styled_parameter_label(label: &str, status: HealthStatus) -> RichText {
    let text = RichText::new(label);
    match status {
        HealthStatus::Healthy => text,
        HealthStatus::Invalid => text.color(INVALID_TEXT),
        HealthStatus::Unstable => text.color(UNSTABLE_TEXT),
    }
}

fn param_name_id(index: usize) -> Id {
    Id::new(("le3_parameters_name", index))
}

fn param_value_id(index: usize) -> Id {
    Id::new(("le3_parameters_value", index))
}

/// Whether a stored parameter value should show computed + expression text.
pub fn parameter_value_is_expression(doc: &Document, expression: &str) -> bool {
    let expr = expression.trim();
    if expr.is_empty() {
        return false;
    }
    if expr.contains(['+', '*', '/', '(', ')']) {
        return true;
    }
    if expr.chars().skip(1).any(|c| c == '-') {
        return true;
    }
    expression_references_document_parameter(doc, expr)
}

/// Value-column label for a stored parameter expression.
pub fn format_parameter_value_display(doc: &Document, expression: &str) -> String {
    let expr = expression.trim();
    if !parameter_value_is_expression(doc, expr) {
        return expr.to_string();
    }
    match eval_length_mm_in_doc(expr, doc) {
        Some(v) => format!("{} ({expr})", format_length_display(v)),
        None => expr.to_string(),
    }
}

pub fn parameter_field_focused(ctx: &egui::Context, doc: &Document) -> bool {
    ctx.memory(|m| {
        m.focused().is_some_and(|id| {
            if id == Id::new(NEW_NAME_ID) || id == Id::new(NEW_VALUE_ID) {
                return true;
            }
            doc.parameters.iter().enumerate().any(|(index, _)| {
                id == param_name_id(index) || id == param_value_id(index)
            })
        })
    })
}

/// Which cell is being edited in the parameters table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterEditCell {
    Name(usize),
    Value(usize),
}

/// Transient UI state for the parameters pane.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParametersPaneState {
    pub editing: Option<ParameterEditCell>,
    pub draft: String,
    pub new_name: String,
    pub new_value: String,
    /// Focus the new-parameter name field on the next frame.
    pub focus_new_name: bool,
    /// Focus the new-parameter value field on the next frame.
    pub focus_new_value: bool,
    /// Focus the active edit cell once after [`begin_edit`].
    pub editing_focus: bool,
    /// Inline validation or action feedback shown under the table.
    pub message: Option<String>,
}

/// Whether the new-parameter row has enough input to attempt a commit.
pub fn new_parameter_row_ready(pane: &ParametersPaneState) -> bool {
    !pane.new_name.trim().is_empty() && !pane.new_value.trim().is_empty()
}

/// Commit the new-parameter row; clears inputs only on success.
pub fn commit_new_parameter(state: &mut AppState) -> Result<(), String> {
    if !new_parameter_row_ready(&state.parameters_pane) {
        return Err("Enter a name and value".to_string());
    }
    let name = state.parameters_pane.new_name.trim().to_string();
    let expression = state.parameters_pane.new_value.trim().to_string();
    match state.apply(Action::AddParameter { name, expression }) {
        ActionResult::Ok => {
            state.parameters_pane.new_name.clear();
            state.parameters_pane.new_value.clear();
            state.parameters_pane.focus_new_name = true;
            state.parameters_pane.message = None;
            Ok(())
        }
        ActionResult::Err(e) => {
            state.parameters_pane.message = Some(e.clone());
            Err(e)
        }
        ActionResult::NeedsDialog => Err("Unexpected dialog request".to_string()),
    }
}

impl ParametersPaneState {
    pub fn begin_edit(&mut self, cell: ParameterEditCell, current: &str) {
        self.editing = Some(cell);
        self.draft = current.to_string();
        self.editing_focus = true;
    }

    pub fn cancel_edit(&mut self) {
        self.editing = None;
        self.draft.clear();
        self.editing_focus = false;
    }
}

pub fn parameter_index_by_name(doc: &Document, name: &str) -> Option<usize> {
    doc.parameters
        .iter()
        .position(|p| p.name == name)
}

pub fn duplicate_parameter_name(doc: &Document, name: &str, except: Option<usize>) -> bool {
    parameter_index_by_name(doc, name).is_some_and(|i| except != Some(i))
}

/// Rename `old` to `new` in every expression that references it.
pub fn propagate_parameter_rename(doc: &mut Document, old: &str, new: &str) {
    if old == new {
        return;
    }
    for param in &mut doc.parameters {
        param.expression = substitute_parameter_name(&param.expression, old, new);
    }
    for rect in &mut doc.rects {
        if let Some(expr) = &mut rect.width_expr {
            *expr = substitute_parameter_name(expr, old, new);
        }
        if let Some(expr) = &mut rect.height_expr {
            *expr = substitute_parameter_name(expr, old, new);
        }
    }
    for line in &mut doc.lines {
        if let Some(expr) = &mut line.length_expr {
            *expr = substitute_parameter_name(expr, old, new);
        }
    }
    for circle in &mut doc.circles {
        if let Some(expr) = &mut circle.diameter_expr {
            *expr = substitute_parameter_name(expr, old, new);
        }
    }
    propagate_parameter_rename_to_constraints(doc, old, new);
}

/// Re-evaluate sketch constraints and apply solved geometry.
pub fn recompute_document_geometry(doc: &mut Document) -> Result<(), String> {
    solve_document_constraints(doc)
}

pub fn validate_new_parameter_name(doc: &Document, name: &str, except: Option<usize>) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Parameter name is required".to_string());
    }
    if !is_valid_parameter_name(name) {
        return Err(format!(
            "Invalid parameter name '{name}' (use letters, digits, underscore; start with a letter)"
        ));
    }
    if duplicate_parameter_name(doc, name, except) {
        return Err(format!("Parameter '{name}' already exists"));
    }
    Ok(())
}

/// Parameter name/expression pairs for validation, optionally overriding one row or appending a new one.
fn parameter_bindings_for_check(
    doc: &Document,
    param_name: &str,
    expression: &str,
    existing_index: Option<usize>,
) -> Vec<(String, String)> {
    let mut bindings: Vec<(String, String)> = doc
        .parameters
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let expr = if existing_index == Some(index) {
                expression.to_string()
            } else {
                param.expression.clone()
            };
            (param.name.clone(), expr)
        })
        .collect();
    if existing_index.is_none() && !bindings.iter().any(|(name, _)| name == param_name) {
        bindings.push((param_name.to_string(), expression.to_string()));
    }
    bindings
}

/// Cycle path starting and ending at the same parameter (e.g. `["A", "B", "C", "A"]`).
pub fn find_parameter_dependency_cycle(
    doc: &Document,
    param_name: &str,
    expression: &str,
    existing_index: Option<usize>,
) -> Option<Vec<String>> {
    let param_name = param_name.trim();
    if param_name.is_empty() {
        return None;
    }
    let bindings = parameter_bindings_for_check(doc, param_name, expression.trim(), existing_index);
    let known_names: Vec<&str> = bindings.iter().map(|(name, _)| name.as_str()).collect();
    let mut path = Vec::new();
    find_parameter_dependency_cycle_from(param_name, &bindings, &known_names, &mut path)
}

fn find_parameter_dependency_cycle_from(
    name: &str,
    bindings: &[(String, String)],
    known_names: &[&str],
    path: &mut Vec<String>,
) -> Option<Vec<String>> {
    if let Some(start) = path.iter().position(|n| n == name) {
        let mut cycle = path[start..].to_vec();
        cycle.push(name.to_string());
        return Some(cycle);
    }
    let expression = bindings
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, expr)| expr.as_str())?;
    path.push(name.to_string());
    for dep in parameter_names_referenced_in_expression(expression, known_names) {
        if let Some(cycle) =
            find_parameter_dependency_cycle_from(&dep, bindings, known_names, path)
        {
            return Some(cycle);
        }
    }
    path.pop();
    None
}

pub fn format_circular_dependency_error(cycle: &[String]) -> String {
    if cycle.is_empty() {
        return "Circular parameter dependency".to_string();
    }
    format!("Circular dependency: {}", cycle.join(" → "))
}

/// Live warning text for a draft expression, or `None` when no cycle is detected.
pub fn parameter_expression_cycle_warning(
    doc: &Document,
    param_name: &str,
    expression: &str,
    existing_index: Option<usize>,
) -> Option<String> {
    let expression = expression.trim();
    if expression.is_empty() || param_name.trim().is_empty() {
        return None;
    }
    find_parameter_dependency_cycle(doc, param_name, expression, existing_index)
        .map(|cycle| format_circular_dependency_error(&cycle))
}

pub fn validate_document_parameters_no_cycles(doc: &Document) -> Result<(), String> {
    for (index, param) in doc.parameters.iter().enumerate() {
        if let Some(cycle) = find_parameter_dependency_cycle(
            doc,
            &param.name,
            &param.expression,
            Some(index),
        ) {
            return Err(format_circular_dependency_error(&cycle));
        }
    }
    Ok(())
}

pub fn validate_parameter_expression_for(
    doc: &Document,
    param_name: &str,
    expression: &str,
    existing_index: Option<usize>,
) -> Result<(), String> {
    let expression = expression.trim();
    if expression.is_empty() {
        return Err("Parameter value is required".to_string());
    }
    if let Some(name) =
        unknown_variables_in_parameter_expression(expression, doc, param_name, existing_index).first()
    {
        return Err(format_unknown_variable_error(name));
    }
    if let Some(cycle) =
        find_parameter_dependency_cycle(doc, param_name, expression, existing_index)
    {
        return Err(format_circular_dependency_error(&cycle));
    }
    let bindings = parameter_bindings_for_check(doc, param_name, expression, existing_index);
    let params: Vec<(&str, &str)> = bindings
        .iter()
        .map(|(name, expr)| (name.as_str(), expr.as_str()))
        .collect();
    eval_length_mm_with_params(expression, &params)
        .ok_or_else(|| format!("Invalid expression '{expression}'"))?;
    Ok(())
}

pub fn add_parameter(doc: &mut Document, name: String, expression: String) -> Result<usize, String> {
    let name = name.trim().to_string();
    let expression = expression.trim().to_string();
    validate_new_parameter_name(doc, &name, None)?;
    validate_parameter_expression_for(doc, &name, &expression, None)?;
    let index = doc.parameters.len();
    doc.parameters.push(Parameter {
        name,
        expression,
        deleted: false,
    });
    doc.shape_order.push(crate::model::ShapeKind::Parameter);
    recompute_document_geometry(doc)?;
    Ok(index)
}

pub fn set_parameter_name(doc: &mut Document, index: usize, name: String) -> Result<(), String> {
    let name = name.trim().to_string();
    let old = doc
        .parameters
        .get(index)
        .ok_or_else(|| format!("Parameter {index} not found"))?
        .name
        .clone();
    if name == old {
        return Ok(());
    }
    validate_new_parameter_name(doc, &name, Some(index))?;
    propagate_parameter_rename(doc, &old, &name);
    doc.parameters[index].name = name;
    recompute_document_geometry(doc)
}

pub fn set_parameter_expression(
    doc: &mut Document,
    index: usize,
    expression: String,
) -> Result<(), String> {
    let expression = expression.trim().to_string();
    if doc.parameters.get(index).is_none() {
        return Err(format!("Parameter {index} not found"));
    }
    let param_name = doc.parameters[index].name.clone();
    validate_parameter_expression_for(doc, &param_name, &expression, Some(index))?;
    doc.parameters[index].expression = expression;
    recompute_document_geometry(doc)
}

pub fn delete_parameter(doc: &mut Document, index: usize) -> Result<(), String> {
    if index >= doc.parameters.len() {
        return Err(format!("Parameter {index} not found"));
    }
    if !crate::document_lifecycle::tombstone_parameter(doc, index) {
        return Err(format!("Parameter {index} already deleted"));
    }
    Ok(())
}

fn apply_parameter_action(state: &mut AppState, action: Action) -> ActionResult {
    let result = state.apply(action);
    match &result {
        ActionResult::Ok => state.parameters_pane.message = None,
        ActionResult::Err(e) => state.parameters_pane.message = Some(e.clone()),
        ActionResult::NeedsDialog => {
            state.parameters_pane.message = Some("Unexpected dialog request".to_string());
        }
    }
    result
}

/// Singleline [`TextEdit`] surrenders focus on Enter, so commit must treat `lost_focus` as active.
pub fn parameter_edit_enter_pressed(
    enter_pressed: bool,
    has_focus: bool,
    lost_focus: bool,
) -> bool {
    enter_pressed && (has_focus || lost_focus)
}

pub fn show_pane(ui: &mut egui::Ui, app: &mut AppState) {
    use crate::expression_input::{
        length_expression_field_errors, show_length_expression_text_edit, ParameterExpressionContext,
    };
    use egui::{Grid, ScrollArea, TextEdit};

    ui.heading(PANE_TITLE);
    ui.add_space(4.0);

    ScrollArea::vertical().show(ui, |ui| {
        Grid::new("parameters_table")
            .num_columns(3)
            .spacing([8.0, 4.0])
            .min_col_width(72.0)
            .show(ui, |ui| {
                ui.label("Name");
                ui.label("Value");
                ui.label("");
                ui.end_row();

                let count = app.doc.parameters.len();
                let enter = ui.input(|i| i.key_pressed(Key::Enter));

                for index in 0..count {
                    if !crate::document_lifecycle::parameter_alive(&app.doc, index) {
                        continue;
                    }
                    let (param_name, param_expression, param_display, param_status) = {
                        let param = &app.doc.parameters[index];
                        (
                            param.name.clone(),
                            param.expression.clone(),
                            format_parameter_value_display(&app.doc, &param.expression),
                            app.document_health.parameter_status(index),
                        )
                    };
                    let param_frozen = param_status.is_frozen();
                    if param_frozen {
                        match app.parameters_pane.editing {
                            Some(ParameterEditCell::Name(i) | ParameterEditCell::Value(i))
                                if i == index =>
                            {
                                app.parameters_pane.cancel_edit();
                            }
                            _ => {}
                        }
                    }
                    let editing_name = matches!(
                        app.parameters_pane.editing,
                        Some(ParameterEditCell::Name(i)) if i == index
                    );
                    let editing_value = matches!(
                        app.parameters_pane.editing,
                        Some(ParameterEditCell::Value(i)) if i == index
                    );

                    ui.horizontal(|ui| {
                        if editing_name {
                            let response = ui.add(
                                TextEdit::singleline(&mut app.parameters_pane.draft)
                                    .id(param_name_id(index))
                                    .desired_width(f32::INFINITY),
                            );
                            if app.parameters_pane.editing_focus {
                                response.request_focus();
                                app.parameters_pane.editing_focus = false;
                            }
                            if parameter_edit_enter_pressed(
                                enter,
                                response.has_focus(),
                                response.lost_focus(),
                            ) {
                                let draft = app.parameters_pane.draft.clone();
                                if apply_parameter_action(
                                    app,
                                    Action::CommitParameterName {
                                        index,
                                        name: draft,
                                    },
                                ) == ActionResult::Ok
                                {
                                    app.parameters_pane.cancel_edit();
                                }
                                ui.input_mut(|i| {
                                    i.consume_key(egui::Modifiers::NONE, Key::Enter);
                                });
                            }
                        } else if ui
                            .selectable_label(
                                false,
                                styled_parameter_label(&param_name, param_status),
                            )
                            .clicked()
                            && !param_frozen
                        {
                            app.parameters_pane
                                .begin_edit(ParameterEditCell::Name(index), &param_name);
                        }
                    });

                    ui.horizontal(|ui| {
                        if editing_value {
                            let value_errors = length_expression_field_errors(
                                &app.parameters_pane.draft,
                                &app.doc,
                                Some(&ParameterExpressionContext {
                                    param_name: param_name.clone(),
                                    existing_index: Some(index),
                                }),
                            );
                            let response = show_length_expression_text_edit(
                                ui,
                                &mut app.parameters_pane.draft,
                                param_value_id(index),
                                "",
                                &value_errors,
                            );
                            if app.parameters_pane.editing_focus {
                                response.request_focus();
                                app.parameters_pane.editing_focus = false;
                            }
                            if parameter_edit_enter_pressed(
                                enter,
                                response.has_focus(),
                                response.lost_focus(),
                            ) {
                                let draft = app.parameters_pane.draft.clone();
                                if apply_parameter_action(
                                    app,
                                    Action::CommitParameterExpression {
                                        index,
                                        expression: draft,
                                    },
                                ) == ActionResult::Ok
                                {
                                    app.parameters_pane.cancel_edit();
                                }
                                ui.input_mut(|i| {
                                    i.consume_key(egui::Modifiers::NONE, Key::Enter);
                                });
                            }
                        } else if ui
                            .selectable_label(
                                false,
                                styled_parameter_label(&param_display, param_status),
                            )
                            .clicked()
                            && !param_frozen
                        {
                            app.parameters_pane.begin_edit(
                                ParameterEditCell::Value(index),
                                &param_expression,
                            );
                        }
                    });
                    if param_frozen {
                        let reason = app
                            .document_health
                            .parameter_reason(index)
                            .unwrap_or("");
                        ui.label(
                            RichText::new(reason)
                                .color(if param_status == HealthStatus::Invalid {
                                    INVALID_TEXT
                                } else {
                                    UNSTABLE_TEXT
                                })
                                .size(11.0),
                        );
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }

                let name_response = ui.add(
                    TextEdit::singleline(&mut app.parameters_pane.new_name)
                        .id(Id::new(NEW_NAME_ID))
                        .hint_text("name")
                        .desired_width(f32::INFINITY),
                );
                if app.parameters_pane.focus_new_name {
                    name_response.request_focus();
                    app.parameters_pane.focus_new_name = false;
                }
                let new_param_context = (!app.parameters_pane.new_name.trim().is_empty()).then(|| {
                    ParameterExpressionContext {
                        param_name: app.parameters_pane.new_name.trim().to_string(),
                        existing_index: None,
                    }
                });
                let new_value_errors = length_expression_field_errors(
                    &app.parameters_pane.new_value,
                    &app.doc,
                    new_param_context.as_ref(),
                );
                let value_response = show_length_expression_text_edit(
                    ui,
                    &mut app.parameters_pane.new_value,
                    Id::new(NEW_VALUE_ID),
                    "value",
                    &new_value_errors,
                );
                if app.parameters_pane.focus_new_value {
                    value_response.request_focus();
                    app.parameters_pane.focus_new_value = false;
                }

                let add_clicked =
                    icon_button(ui, IconId::Plus, "Add parameter").clicked();

                if name_response.gained_focus() || value_response.gained_focus() {
                    app.parameters_pane.cancel_edit();
                }

                let mut commit_new = add_clicked;
                if parameter_edit_enter_pressed(
                    enter,
                    name_response.has_focus(),
                    name_response.lost_focus(),
                ) {
                    if !app.parameters_pane.new_name.trim().is_empty()
                        && app.parameters_pane.new_value.trim().is_empty()
                    {
                        app.parameters_pane.focus_new_value = true;
                        ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
                    } else if new_parameter_row_ready(&app.parameters_pane) {
                        commit_new = true;
                    }
                } else if parameter_edit_enter_pressed(
                    enter,
                    value_response.has_focus(),
                    value_response.lost_focus(),
                ) && new_parameter_row_ready(&app.parameters_pane)
                {
                    commit_new = true;
                }

                let lost_focus_commit = (name_response.lost_focus() || value_response.lost_focus())
                    && new_parameter_row_ready(&app.parameters_pane)
                    && !name_response.has_focus()
                    && !value_response.has_focus();

                if commit_new || lost_focus_commit {
                    let _ = commit_new_parameter(app);
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
                }

                ui.end_row();
            });
    });

    if let Some(message) = &app.parameters_pane.message {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(message)
                .color(egui::Color32::from_rgb(255, 140, 100))
                .size(12.0),
        );
    } else if app.doc.parameters.is_empty() {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Type name and value (e.g. A and 10mm), then press Enter or +")
                .color(egui::Color32::from_gray(140))
                .size(12.0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{Action, ActionResult, AppState};
    use crate::constraints::add_distance_constraint;
    use crate::model::{DistanceTarget, Document, FaceId, ShapeKind};
    use crate::Rect;

    fn doc_with_param_a() -> Document {
        let mut doc = Document::default();
        add_parameter(&mut doc, "A".to_string(), "5mm".to_string()).unwrap();
        doc
    }

    #[test]
    fn add_multiple_parameters_in_sequence() {
        let mut doc = Document::default();
        add_parameter(&mut doc, "A".to_string(), "5mm".to_string()).unwrap();
        add_parameter(&mut doc, "B".to_string(), "A + 5in".to_string()).unwrap();
        add_parameter(&mut doc, "width".to_string(), "2 * B".to_string()).unwrap();
        assert_eq!(doc.parameters.len(), 3);
        assert_eq!(doc.parameters[2].expression, "2 * B");
    }

    #[test]
    fn add_parameter_stores_name_and_expression() {
        let mut doc = Document::default();
        add_parameter(&mut doc, "width".to_string(), "2in".to_string()).unwrap();
        assert_eq!(doc.parameters.len(), 1);
        assert_eq!(doc.parameters[0].name, "width");
        assert_eq!(doc.parameters[0].expression, "2in");
        assert!(doc.shape_order.contains(&ShapeKind::Parameter));
    }

    #[test]
    fn parameter_rename_updates_dependent_expressions() {
        let mut doc = doc_with_param_a();
        add_parameter(&mut doc, "B".to_string(), "A + 5in".to_string()).unwrap();
        set_parameter_name(&mut doc, 0, "Len".to_string()).unwrap();
        assert_eq!(doc.parameters[1].expression, "Len + 5in");
    }

    #[test]
    fn parameter_value_change_recomputes_rectangle_width() {
        let mut doc = doc_with_param_a();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 5.0, 10.0));
        doc.shape_order.push(ShapeKind::Rect);
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "A".to_string(),
        )
        .unwrap();

        set_parameter_expression(&mut doc, 0, "10mm".to_string()).unwrap();
        assert!((doc.rects[0].w - 10.0).abs() < 1e-3);
    }

    #[test]
    fn rectangle_with_parameter_expression_evaluates_on_recompute() {
        let mut doc = doc_with_param_a();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 1.0, 10.0));
        doc.shape_order.push(ShapeKind::Rect);
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "A + 5in".to_string(),
        )
        .unwrap();
        recompute_document_geometry(&mut doc).unwrap();
        assert!((doc.rects[0].w - (5.0 + 5.0 * 25.4)).abs() < 1e-2);
    }

    #[test]
    fn rejects_duplicate_parameter_names() {
        let mut doc = doc_with_param_a();
        assert!(add_parameter(&mut doc, "A".to_string(), "1mm".to_string()).is_err());
    }

    #[test]
    fn rejects_invalid_parameter_name() {
        let mut doc = Document::default();
        assert!(add_parameter(&mut doc, "1bad".to_string(), "5mm".to_string()).is_err());
    }

    #[test]
    fn format_parameter_value_display_shows_literal_unchanged() {
        let doc = Document::default();
        assert_eq!(format_parameter_value_display(&doc, "10mm"), "10mm");
        assert_eq!(format_parameter_value_display(&doc, "50"), "50");
    }

    #[test]
    fn format_parameter_value_display_shows_computed_for_expressions() {
        let mut doc = doc_with_param_a();
        add_parameter(&mut doc, "B".to_string(), "A + 5mm".to_string()).unwrap();
        add_parameter(&mut doc, "C".to_string(), "2 * B".to_string()).unwrap();
        assert_eq!(
            format_parameter_value_display(&doc, "A + 5mm"),
            "10.0 mm (A + 5mm)"
        );
        assert_eq!(format_parameter_value_display(&doc, "A"), "5.0 mm (A)");
        assert_eq!(
            format_parameter_value_display(&doc, "2 * B"),
            "20.0 mm (2 * B)"
        );
    }

    #[test]
    fn parameter_edit_enter_pressed_accepts_lost_focus_from_singleline_textedit() {
        assert!(parameter_edit_enter_pressed(true, false, true));
        assert!(parameter_edit_enter_pressed(true, true, false));
        assert!(!parameter_edit_enter_pressed(true, false, false));
        assert!(!parameter_edit_enter_pressed(false, false, true));
    }

    #[test]
    fn commit_parameter_expression_via_action_recomputes_dependent_rectangle() {
        let mut state = AppState::default();
        add_parameter(&mut state.doc, "A".to_string(), "5mm".to_string()).unwrap();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        state.doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 5.0, 10.0));
        state.doc.shape_order.push(ShapeKind::Rect);
        add_distance_constraint(
            &mut state.doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "A".to_string(),
        )
        .unwrap();

        assert_eq!(
            state.apply(Action::CommitParameterExpression {
                index: 0,
                expression: "12mm".to_string(),
            }),
            ActionResult::Ok
        );
        assert_eq!(state.doc.parameters[0].expression, "12mm");
        assert!((state.doc.rects[0].w - 12.0).abs() < 1e-3);
    }

    #[test]
    fn commit_new_parameter_clears_fields_only_on_success() {
        let mut state = AppState::default();
        state.parameters_pane.new_name = "A".to_string();
        state.parameters_pane.new_value = "10mm".to_string();
        commit_new_parameter(&mut state).unwrap();
        assert_eq!(state.doc.parameters.len(), 1);
        assert!(state.parameters_pane.new_name.is_empty());
        assert!(state.parameters_pane.new_value.is_empty());
        assert!(state.parameters_pane.message.is_none());
    }

    #[test]
    fn commit_new_parameter_keeps_fields_on_validation_error() {
        let mut state = AppState::default();
        state.parameters_pane.new_name = "1bad".to_string();
        state.parameters_pane.new_value = "10mm".to_string();
        assert!(commit_new_parameter(&mut state).is_err());
        assert_eq!(state.doc.parameters.len(), 0);
        assert_eq!(state.parameters_pane.new_name, "1bad");
        assert_eq!(state.parameters_pane.new_value, "10mm");
        assert!(state.parameters_pane.message.is_some());
    }

    #[test]
    fn rejects_unknown_variable_in_parameter_expression() {
        let mut doc = doc_with_param_a();
        let err = set_parameter_expression(&mut doc, 0, "Missing".to_string()).unwrap_err();
        assert_eq!(err, "Unknown variable: Missing");
    }

    #[test]
    fn rejects_direct_self_referencing_parameter() {
        let mut doc = Document::default();
        assert!(add_parameter(&mut doc, "A".to_string(), "A".to_string()).is_err());
    }

    #[test]
    fn rejects_two_parameter_cycle() {
        let mut doc = doc_with_param_a();
        add_parameter(&mut doc, "B".to_string(), "A".to_string()).unwrap();
        let err = set_parameter_expression(&mut doc, 0, "B".to_string()).unwrap_err();
        assert!(err.contains("Circular dependency"));
        assert!(err.contains("A"));
        assert!(err.contains("B"));
    }

    #[test]
    fn rejects_three_parameter_cycle() {
        let mut doc = doc_with_param_a();
        add_parameter(&mut doc, "C".to_string(), "A".to_string()).unwrap();
        add_parameter(&mut doc, "B".to_string(), "C".to_string()).unwrap();
        let err = set_parameter_expression(&mut doc, 0, "B".to_string()).unwrap_err();
        assert_eq!(err, "Circular dependency: A → B → C → A");
    }

    #[test]
    fn rejects_add_parameter_that_references_itself() {
        let mut doc = Document::default();
        let err = add_parameter(&mut doc, "A".to_string(), "A".to_string()).unwrap_err();
        assert!(err.contains("Circular dependency"));
    }

    #[test]
    fn allows_non_circular_parameter_chain() {
        let mut doc = doc_with_param_a();
        add_parameter(&mut doc, "B".to_string(), "A + 5mm".to_string()).unwrap();
        add_parameter(&mut doc, "C".to_string(), "2 * B".to_string()).unwrap();
        assert_eq!(doc.parameters.len(), 3);
    }

    #[test]
    fn parameter_expression_cycle_warning_for_draft_expression() {
        let mut doc = doc_with_param_a();
        add_parameter(&mut doc, "B".to_string(), "A".to_string()).unwrap();
        let warning = parameter_expression_cycle_warning(&doc, "A", "B", Some(0)).unwrap();
        assert_eq!(warning, "Circular dependency: A → B → A");
    }

    #[test]
    fn validate_document_parameters_no_cycles_accepts_healthy_document() {
        let mut doc = doc_with_param_a();
        add_parameter(&mut doc, "B".to_string(), "A + 5mm".to_string()).unwrap();
        validate_document_parameters_no_cycles(&doc).unwrap();
    }

    #[test]
    fn commit_new_parameter_supports_multiple_adds_in_sequence() {
        let mut state = AppState::default();
        state.parameters_pane.new_name = "A".to_string();
        state.parameters_pane.new_value = "10mm".to_string();
        commit_new_parameter(&mut state).unwrap();
        state.parameters_pane.new_name = "B".to_string();
        state.parameters_pane.new_value = "A + 5mm".to_string();
        commit_new_parameter(&mut state).unwrap();
        assert_eq!(state.doc.parameters.len(), 2);
        assert_eq!(state.doc.parameters[1].expression, "A + 5mm");
    }
}