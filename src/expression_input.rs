//! Shared validation and UI for length expression text fields.

use crate::command_palette::fuzzy_score;
use crate::model::Document;
use crate::parameters::{format_parameter_autocomplete_value, parameter_expression_cycle_warning};
use crate::value::{
    document_parameter_names, format_unknown_variable_error,
    unknown_variables_in_expression, unknown_variables_in_parameter_expression,
};
use eframe::egui::{self, Frame, Id, Key, Margin, Order, Response, Stroke, TextEdit};
use egui::text::{CCursor, CCursorRange};
use egui::widgets::text_edit::TextEditState;

pub const ERROR_TOOLTIP_GAP: f32 = 4.0;
pub const INVALID_BORDER: egui::Color32 = egui::Color32::from_rgb(220, 100, 90);
pub const INVALID_BG: egui::Color32 = egui::Color32::from_rgb(52, 30, 30);
pub const INVALID_TEXT: egui::Color32 = egui::Color32::from_rgb(255, 190, 170);
pub const ERROR_TOOLTIP_TEXT: egui::Color32 = egui::Color32::from_rgb(255, 180, 120);
const AUTOCOMPLETE_MAX: usize = 8;

/// Context for validating a parameter definition expression (cycle detection).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterExpressionContext {
    pub param_name: String,
    pub existing_index: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutocompleteMatch {
    pub name: String,
    pub value: String,
    pub score: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct AutocompleteUiState {
    highlight: usize,
    last_query: String,
}

/// Live validation errors for a length expression field.
pub fn length_expression_field_errors(
    text: &str,
    doc: &Document,
    parameter_context: Option<&ParameterExpressionContext>,
) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    let mut errors = Vec::new();
    if let Some(ctx) = parameter_context {
        errors.extend(
            unknown_variables_in_parameter_expression(
                text,
                doc,
                &ctx.param_name,
                ctx.existing_index,
            )
            .into_iter()
            .map(|name| format_unknown_variable_error(&name)),
        );
        if let Some(warning) = parameter_expression_cycle_warning(
            doc,
            &ctx.param_name,
            text,
            ctx.existing_index,
        ) {
            errors.push(warning);
        }
    } else {
        let known_names = document_parameter_names(doc);
        errors.extend(
            unknown_variables_in_expression(text, &known_names)
                .into_iter()
                .map(|name| format_unknown_variable_error(&name)),
        );
    }

    errors
}

pub fn show_expression_error_tooltips_above(ui: &egui::Ui, anchor: &Response, errors: &[String]) {
    if errors.is_empty() {
        return;
    }

    use egui::{Align2, Area, Frame, Order};

    Area::new(anchor.id.with("expression_error_tooltip"))
        .order(Order::Tooltip)
        .pivot(Align2::LEFT_BOTTOM)
        .fixed_pos(anchor.rect.left_top() - egui::vec2(0.0, ERROR_TOOLTIP_GAP))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            Frame::popup(&ui.style()).show(ui, |ui| {
                for error in errors {
                    ui.label(egui::RichText::new(error).color(ERROR_TOOLTIP_TEXT));
                }
            });
        });
}

/// Frame matching default [`TextEdit`] metrics so error styling only changes colors.
fn length_expression_text_edit_frame(ui: &egui::Ui, id: Id, invalid: bool) -> Frame {
    let visuals = &ui.style().visuals;
    let focused = ui.ctx().memory(|m| m.focused()) == Some(id);
    let widget = if focused {
        &visuals.widgets.active
    } else {
        &visuals.widgets.inactive
    };
    let stroke = if invalid {
        Stroke::new(widget.bg_stroke.width, INVALID_BORDER)
    } else {
        widget.bg_stroke
    };

    Frame::default()
        .fill(if invalid {
            INVALID_BG
        } else {
            visuals.extreme_bg_color
        })
        .stroke(stroke)
        .inner_margin(Margin::symmetric(4.0, 2.0))
        .rounding(widget.rounding)
}

fn is_identifier_part(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Character range `[start, end)` of the identifier token touching `cursor_char_index`.
pub fn identifier_token_at_cursor(text: &str, cursor_char_index: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let cursor = cursor_char_index.min(chars.len());
    let mut start = cursor;
    while start > 0 && is_identifier_part(chars[start - 1]) {
        start -= 1;
    }
    let mut end = cursor;
    while end < chars.len() && is_identifier_part(chars[end]) {
        end += 1;
    }
    if start == end {
        return None;
    }
    let first = chars[start];
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if start > 0 {
        let before = chars[..start]
            .iter()
            .rposition(|c| !c.is_whitespace())
            .map(|idx| chars[idx]);
        if before.is_some_and(|c| c.is_ascii_digit() || c == '.') {
            return None;
        }
    }
    Some((start, end))
}

fn token_query(text: &str, token: (usize, usize)) -> String {
    text.chars().skip(token.0).take(token.1 - token.0).collect()
}

fn char_range_to_byte_range(text: &str, start: usize, end: usize) -> (usize, usize) {
    let byte_start = text
        .char_indices()
        .nth(start)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    let byte_end = text
        .char_indices()
        .nth(end)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    (byte_start, byte_end)
}

pub fn parameter_autocomplete_candidates(
    doc: &Document,
    query: &str,
    exclude_names: &[&str],
) -> Vec<AutocompleteMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    for (index, param) in doc.parameters.iter().enumerate() {
        if param.deleted || exclude_names.iter().any(|name| *name == param.name) {
            continue;
        }
        let Some(score) = fuzzy_score(query, &param.name) else {
            continue;
        };
        matches.push(AutocompleteMatch {
            name: param.name.clone(),
            value: format_parameter_autocomplete_value(doc, index),
            score,
        });
    }
    matches.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    matches.truncate(AUTOCOMPLETE_MAX);
    matches
}

fn autocomplete_state_id(id: Id) -> Id {
    id.with("expression_autocomplete")
}

fn load_autocomplete_state(ctx: &egui::Context, id: Id) -> AutocompleteUiState {
    ctx.data_mut(|d| {
        d.get_temp::<AutocompleteUiState>(autocomplete_state_id(id))
            .unwrap_or_default()
    })
}

fn store_autocomplete_state(ctx: &egui::Context, id: Id, state: AutocompleteUiState) {
    ctx.data_mut(|d| d.insert_temp(autocomplete_state_id(id), state));
}

fn apply_parameter_completion(
    text: &mut String,
    token: (usize, usize),
    name: &str,
    text_state: &mut TextEditState,
) {
    let (byte_start, byte_end) = char_range_to_byte_range(text, token.0, token.1);
    text.replace_range(byte_start..byte_end, name);
    let cursor = token.0 + name.chars().count();
    text_state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(cursor))));
}

fn cursor_char_index(state: Option<&TextEditState>, text: &str) -> usize {
    state
        .and_then(|state| state.cursor.char_range())
        .map(|range| range.primary.index)
        .unwrap_or_else(|| text.chars().count())
}

/// Handle autocomplete keyboard input before the text edit runs.
pub fn expression_autocomplete_handle_keys(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    id: Id,
    text: &mut String,
    doc: &Document,
    exclude_names: &[&str],
) -> bool {
    let Some(mut text_state) = TextEditState::load(ctx, id) else {
        return false;
    };
    let cursor = cursor_char_index(Some(&text_state), text);
    let Some(token) = identifier_token_at_cursor(text, cursor) else {
        return false;
    };
    let query = token_query(text, token);
    let candidates = parameter_autocomplete_candidates(doc, &query, exclude_names);
    if candidates.is_empty() {
        return false;
    }

    let mut ui_state = load_autocomplete_state(ctx, id);
    if ui_state.last_query != query {
        ui_state.highlight = 0;
        ui_state.last_query = query;
    }
    ui_state.highlight = ui_state.highlight.min(candidates.len().saturating_sub(1));

    let up = ui.input(|i| i.key_pressed(Key::ArrowUp));
    let down = ui.input(|i| i.key_pressed(Key::ArrowDown));
    let space = ui.input(|i| i.key_pressed(Key::Space));
    let tab = ui.input(|i| i.key_pressed(Key::Tab));
    let enter = ui.input(|i| i.key_pressed(Key::Enter));
    let mut changed = false;

    if up {
        ui_state.highlight = ui_state.highlight.saturating_sub(1);
        ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
    } else if down {
        ui_state.highlight = (ui_state.highlight + 1).min(candidates.len() - 1);
        ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
    } else if space || tab {
        // Space or Tab accepts the highlighted (top by default) candidate and keeps editing.
        let name = candidates[ui_state.highlight].name.clone();
        apply_parameter_completion(text, token, &name, &mut text_state);
        text_state.store(ctx, id);
        let key = if space { Key::Space } else { Key::Tab };
        ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
        changed = true;
    } else if enter {
        // Enter accepts the highlighted candidate too, but is left unconsumed so the field's
        // own Enter handling still commits the (now completed) expression in one keystroke (#50).
        let name = candidates[ui_state.highlight].name.clone();
        apply_parameter_completion(text, token, &name, &mut text_state);
        text_state.store(ctx, id);
        changed = true;
    }

    store_autocomplete_state(ctx, id, ui_state);
    changed
}

/// Show the autocomplete dropdown below a focused expression field.
pub fn expression_autocomplete_show_dropdown(
    _ui: &mut egui::Ui,
    ctx: &egui::Context,
    anchor: &Response,
    id: Id,
    text: &mut String,
    doc: &Document,
    exclude_names: &[&str],
    cursor_char_index: usize,
) -> bool {
    let Some(token) = identifier_token_at_cursor(text, cursor_char_index) else {
        return false;
    };
    let query = token_query(text, token);
    let candidates = parameter_autocomplete_candidates(doc, &query, exclude_names);
    if candidates.is_empty() {
        return false;
    }

    let mut ui_state = load_autocomplete_state(ctx, id);
    if ui_state.last_query != query {
        ui_state.highlight = 0;
        ui_state.last_query = query;
    }
    ui_state.highlight = ui_state.highlight.min(candidates.len().saturating_sub(1));
    store_autocomplete_state(ctx, id, ui_state.clone());

    let highlight = ui_state.highlight;
    let mut changed = false;
    let anchor_id = anchor.id;
    let token_for_click = token;

    egui::Area::new(anchor_id.with("expression_autocomplete"))
        .order(Order::Foreground)
        .fixed_pos(anchor.rect.left_bottom())
        .show(ctx, |ui| {
            Frame::popup(&ui.style()).show(ui, |ui| {
                ui.set_min_width(anchor.rect.width().max(160.0));
                for (index, candidate) in candidates.iter().enumerate() {
                    let selected = index == highlight;
                    let label = format!("{}   {}", candidate.name, candidate.value);
                    let response = ui.selectable_label(selected, label);
                    if response.clicked() {
                        if let Some(mut text_state) = TextEditState::load(ctx, id) {
                            apply_parameter_completion(
                                text,
                                token_for_click,
                                &candidate.name,
                                &mut text_state,
                            );
                            text_state.store(ctx, id);
                            changed = true;
                        }
                    }
                }
            });
        });

    changed
}

/// Parameters-pane style length expression input with shared validation UI.
pub fn show_length_expression_text_edit(
    ui: &mut egui::Ui,
    text: &mut String,
    id: Id,
    hint_text: &str,
    errors: &[String],
    doc: &Document,
    exclude_names: &[&str],
) -> Response {
    let ctx = ui.ctx().clone();
    let had_focus = ctx.memory(|m| m.focused()) == Some(id);
    if had_focus {
        expression_autocomplete_handle_keys(ui, &ctx, id, text, doc, exclude_names);
    }

    let invalid = !errors.is_empty();
    let output = length_expression_text_edit_frame(ui, id, invalid)
        .show(ui, |ui| {
            let mut edit = TextEdit::singleline(text)
                .id(id)
                .hint_text(hint_text)
                .desired_width(f32::INFINITY)
                .frame(false)
                .margin(Margin::ZERO);
            if invalid {
                edit = edit.text_color(INVALID_TEXT);
            }
            edit.show(ui)
        })
        .inner;

    if output.response.has_focus() {
        let cursor = cursor_char_index(Some(&output.state), text);
        if expression_autocomplete_show_dropdown(
            ui,
            &ctx,
            &output.response,
            id,
            text,
            doc,
            exclude_names,
            cursor,
        ) {
            output.state.clone().store(&ctx, id);
        }
    }

    show_expression_error_tooltips_above(ui, &output.response, errors);
    output.response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parameters::add_parameter;

    #[test]
    fn length_expression_field_errors_reports_unknown_variable() {
        let mut doc = Document::default();
        add_parameter(&mut doc, "A".to_string(), "10mm".to_string()).unwrap();
        let errors = length_expression_field_errors("A + B", &doc, None);
        assert_eq!(errors, vec!["Unknown variable: B".to_string()]);
    }

    #[test]
    fn length_expression_field_errors_reports_cycle_for_parameter_context() {
        let mut doc = Document::default();
        add_parameter(&mut doc, "A".to_string(), "5mm".to_string()).unwrap();
        add_parameter(&mut doc, "B".to_string(), "A".to_string()).unwrap();
        let errors = length_expression_field_errors(
            "B",
            &doc,
            Some(&ParameterExpressionContext {
                param_name: "A".to_string(),
                existing_index: Some(0),
            }),
        );
        assert_eq!(errors, vec!["Circular dependency: A → B → A".to_string()]);
    }

    #[test]
    fn length_expression_field_errors_reports_unknown_before_cycle() {
        let mut doc = Document::default();
        add_parameter(&mut doc, "A".to_string(), "5mm".to_string()).unwrap();
        let errors = length_expression_field_errors(
            "Missing + B",
            &doc,
            Some(&ParameterExpressionContext {
                param_name: "A".to_string(),
                existing_index: Some(0),
            }),
        );
        assert_eq!(
            errors,
            vec![
                "Unknown variable: Missing".to_string(),
                "Unknown variable: B".to_string(),
            ]
        );
    }

    #[test]
    fn identifier_token_at_cursor_finds_partial_name() {
        let text = "10mm + wid";
        let end = text.chars().count();
        assert_eq!(identifier_token_at_cursor(text, end), Some((7, 10)));
    }

    #[test]
    fn identifier_token_at_cursor_ignores_unit_suffix() {
        let text = "10mm";
        assert_eq!(identifier_token_at_cursor(text, 4), None);
    }

    #[test]
    fn parameter_autocomplete_candidates_fuzzy_matches() {
        let mut doc = Document::default();
        add_parameter(&mut doc, "width".to_string(), "10mm".to_string()).unwrap();
        add_parameter(&mut doc, "height".to_string(), "5mm".to_string()).unwrap();
        let matches = parameter_autocomplete_candidates(&doc, "wid", &[]);
        assert_eq!(matches.first().map(|m| m.name.as_str()), Some("width"));
    }

    #[test]
    fn apply_parameter_completion_replaces_partial_token() {
        // Backs the Tab/Enter completion in #50: "wid" -> "width".
        let mut text = "10mm + wid".to_string();
        let token = identifier_token_at_cursor(&text, text.chars().count()).unwrap();
        let mut state = TextEditState::default();
        apply_parameter_completion(&mut text, token, "width", &mut state);
        assert_eq!(text, "10mm + width");
    }

    #[test]
    fn parameter_autocomplete_candidates_exclude_names() {
        let mut doc = Document::default();
        add_parameter(&mut doc, "width".to_string(), "10mm".to_string()).unwrap();
        let matches = parameter_autocomplete_candidates(&doc, "wid", &["width"]);
        assert!(matches.is_empty());
    }
}