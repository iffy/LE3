//! Context pane: union of editable properties for the current selection or draw op.

use crate::actions::Tool;
use crate::document_health::{health_status_label, selection_frozen_summary, DocumentHealth, HealthStatus};
use crate::geometric_constraints::{constraint_pane_rows, ConstraintPaneRow};
use crate::hierarchy::SceneElement;
use crate::model::{Document, RectEdge};
use crate::names::{element_name, single_nameable_from_selection};
use crate::selection::SceneSelection;
use crate::icons::icon_for_constraint;
use crate::shortcuts;
use eframe::egui::{self, Key, TextEdit};

pub const PANE_TITLE: &str = "Context";

/// Inputs needed to build context pane content (kept separate from [`AppState`] to avoid cycles).
pub struct ContextInput<'a> {
    pub doc: &'a Document,
    pub selection: &'a SceneSelection,
    pub tool: Tool,
    pub draw_rect_construction: Option<bool>,
    pub draw_line_construction: Option<bool>,
    pub draw_circle_construction: Option<bool>,
}

/// Tri-state value for a property shared by multiple targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriState {
    Off,
    On,
    Mixed,
}

/// What the context pane should display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextPaneContent {
    pub name: Option<NameControl>,
    pub construction: Option<ConstructionControl>,
    pub constraints: Option<Vec<ConstraintPaneRow>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameControl {
    pub element: SceneElement,
}

/// Draft text and focus state for the name field in the context pane.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextPaneState {
    pub name_draft: String,
    pub focus_name_field: bool,
    pub synced_element: Option<SceneElement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstructionControl {
    pub value: TriState,
    pub target_count: usize,
}

pub fn context_pane_content(input: &ContextInput<'_>) -> ContextPaneContent {
    let name = single_nameable_from_selection(input.selection).map(|element| NameControl { element });

    if let Some(construction) = input.draw_rect_construction {
        return ContextPaneContent {
            name,
            construction: Some(ConstructionControl {
                value: tri_state_from_bool(construction),
                target_count: 1,
            }),
            constraints: None,
        };
    }
    if let Some(construction) = input.draw_line_construction {
        return ContextPaneContent {
            name,
            construction: Some(ConstructionControl {
                value: tri_state_from_bool(construction),
                target_count: 1,
            }),
            constraints: None,
        };
    }
    if let Some(construction) = input.draw_circle_construction {
        return ContextPaneContent {
            name,
            construction: Some(ConstructionControl {
                value: tri_state_from_bool(construction),
                target_count: 1,
            }),
            constraints: None,
        };
    }

    let targets = construction_targets_from_selection(input.selection);
    let constraints = (input.tool == Tool::Constraint)
        .then(|| constraint_pane_rows(input.selection));
    ContextPaneContent {
        name,
        construction: (!targets.is_empty()).then(|| ConstructionControl {
            value: construction_tri_state(input.doc, &targets),
            target_count: targets.len(),
        }),
        constraints,
    }
}

pub fn sync_name_draft(
    state: &mut ContextPaneState,
    doc: &Document,
    content: &ContextPaneContent,
) {
    let Some(control) = &content.name else {
        state.synced_element = None;
        return;
    };
    if state.synced_element == Some(control.element) {
        return;
    }
    state.synced_element = Some(control.element);
    state.name_draft = element_name(doc, control.element)
        .unwrap_or_default()
        .to_string();
}

pub fn construction_targets_from_selection(selection: &SceneSelection) -> Vec<SceneElement> {
    let mut targets = Vec::new();
    for element in selection.iter() {
        match element {
            SceneElement::Line(_)
            | SceneElement::Circle(_)
            | SceneElement::RectEdge(_, _) => targets.push(element),
            SceneElement::Rect(index) => {
                for edge_index in 0..4 {
                    targets.push(SceneElement::RectEdge(
                        index,
                        RectEdge::from_index(edge_index),
                    ));
                }
            }
            _ => {}
        }
    }
    targets.sort_by_key(|element| scene_element_sort_key(*element));
    targets.dedup();
    targets
}

fn scene_element_sort_key(element: SceneElement) -> (u8, usize, u8) {
    match element {
        SceneElement::Line(i) => (0, i, 0),
        SceneElement::Circle(i) => (1, i, 0),
        SceneElement::RectEdge(i, edge) => (2, i, edge.index() as u8),
        _ => (2, 0, 0),
    }
}

pub fn edge_construction_for_element(doc: &Document, element: SceneElement) -> Option<bool> {
    match element {
        SceneElement::RectEdge(index, edge) => doc
            .rects
            .get(index)
            .map(|rect| rect.edge_construction(edge)),
        SceneElement::Line(index) => doc.lines.get(index).map(|line| line.construction),
        SceneElement::Circle(index) => doc.circles.get(index).map(|circle| circle.construction),
        _ => None,
    }
}

/// Whether a selected line, edge, or curve uses dashed (construction) highlighting.
pub fn selection_highlight_dashed(doc: &Document, element: SceneElement) -> Option<bool> {
    edge_construction_for_element(doc, element)
}

pub fn construction_tri_state(doc: &Document, targets: &[SceneElement]) -> TriState {
    let mut any_on = false;
    let mut any_off = false;
    for element in targets {
        let Some(value) = edge_construction_for_element(doc, *element) else {
            continue;
        };
        if value {
            any_on = true;
        } else {
            any_off = true;
        }
    }
    tri_state_from_flags(any_on, any_off)
}

fn tri_state_from_bool(value: bool) -> TriState {
    if value {
        TriState::On
    } else {
        TriState::Off
    }
}

fn tri_state_from_flags(any_on: bool, any_off: bool) -> TriState {
    match (any_on, any_off) {
        (true, false) => TriState::On,
        (false, true) => TriState::Off,
        (true, true) => TriState::Mixed,
        (false, false) => TriState::Off,
    }
}

pub fn set_edge_construction(
    doc: &mut Document,
    element: SceneElement,
    construction: bool,
) -> Result<(), String> {
    match element {
        SceneElement::RectEdge(index, edge) => {
            let rect = doc
                .rects
                .get_mut(index)
                .ok_or_else(|| format!("Rectangle {index} not found"))?;
            rect.set_edge_construction(edge, construction);
            Ok(())
        }
        SceneElement::Line(index) => {
            let line = doc
                .lines
                .get_mut(index)
                .ok_or_else(|| format!("Line {index} not found"))?;
            line.construction = construction;
            Ok(())
        }
        SceneElement::Circle(index) => {
            let circle = doc
                .circles
                .get_mut(index)
                .ok_or_else(|| format!("Circle {index} not found"))?;
            circle.construction = construction;
            Ok(())
        }
        _ => Err("Only lines, circles, and rectangle edges support construction mode".to_string()),
    }
}

pub fn set_construction_for_targets(
    doc: &mut Document,
    targets: &[SceneElement],
    construction: bool,
) -> Result<usize, String> {
    let mut updated = 0usize;
    for element in targets {
        set_edge_construction(doc, *element, construction)?;
        updated += 1;
    }
    Ok(updated)
}

pub fn toggle_construction_for_targets(
    doc: &mut Document,
    targets: &[SceneElement],
) -> Result<usize, String> {
    let mut updated = 0usize;
    for element in targets {
        let Some(current) = edge_construction_for_element(doc, *element) else {
            continue;
        };
        set_edge_construction(doc, *element, !current)?;
        updated += 1;
    }
    Ok(updated)
}

pub fn show_pane(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    content: &ContextPaneContent,
    pane_state: &mut ContextPaneState,
    health: &DocumentHealth,
    selection: &SceneSelection,
    on_name_committed: &mut impl FnMut(SceneElement, String),
    on_construction_changed: &mut impl FnMut(bool),
    on_constraint_clicked: &mut impl FnMut(crate::geometric_constraints::GeometricConstraintType),
) {
    ui.heading(PANE_TITLE);
    ui.separator();

    let frozen = selection_frozen_summary(health, selection);
    if let Some((status, reason)) = &frozen {
        let color = match status {
            HealthStatus::Invalid => egui::Color32::from_rgb(220, 80, 80),
            HealthStatus::Unstable => egui::Color32::from_rgb(255, 180, 60),
            HealthStatus::Healthy => egui::Color32::from_gray(140),
        };
        ui.label(
            egui::RichText::new(format!(
                "{} — editing frozen",
                health_status_label(*status).to_uppercase()
            ))
            .color(color)
            .strong(),
        );
        ui.label(
            egui::RichText::new(reason.as_str())
                .color(egui::Color32::from_gray(140))
                .size(11.0),
        );
        ui.add_space(4.0);
    }

    let controls_enabled = frozen.is_none();
    let mut any_control = false;

    if let Some(control) = &content.name {
        any_control = true;
        ui.label(shortcuts::compact_label("Name", Some(shortcuts::FOCUS_ELEMENT_NAME)));
        let id = egui::Id::new(("element_name", control.element));
        let mut committed = false;
        ui.add_enabled_ui(controls_enabled, |ui| {
            let output = TextEdit::singleline(&mut pane_state.name_draft)
                .id(id)
                .desired_width(f32::INFINITY)
                .show(ui);
            let response = &output.response;
            let should_select_all = pane_state.focus_name_field;
            if should_select_all {
                response.request_focus();
            }
            if (should_select_all && response.has_focus()) || response.gained_focus() {
                let len = pane_state.name_draft.chars().count();
                let mut state = output.state;
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::default(),
                    egui::text::CCursor::new(len),
                )));
                state.store(ctx, id);
                pane_state.focus_name_field = false;
            }
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            if (enter && response.has_focus()) || response.lost_focus() {
                committed = true;
                if enter && response.has_focus() {
                    ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
                }
            }
        });
        if committed {
            on_name_committed(control.element, pane_state.name_draft.clone());
        }
        ui.add_space(4.0);
    }

    if let Some(rows) = &content.constraints {
        any_control = true;
        ui.label("Constraints");
        for row in rows {
            ui.horizontal(|ui| {
                let row_w = ui.available_width();
                let enabled = controls_enabled && row.enabled;
                let response = ui
                    .add_enabled(
                        enabled,
                        egui::ImageButton::new(crate::icons::sized_texture(
                            ui.ctx(),
                            icon_for_constraint(row.kind),
                        ))
                        .frame(true),
                    )
                    .on_hover_text(row.kind.label());
                if let Some(shortcut) = row.shortcut {
                    shortcuts::show_right_aligned_shortcut(
                        ui,
                        row_w,
                        response.rect.width(),
                        response.rect.height(),
                        shortcuts::constraint_number_hint(shortcut),
                    );
                }
                if enabled && response.clicked() {
                    on_constraint_clicked(row.kind);
                }
                if !row.enabled && !row.missing.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("needs {}", row.missing.join(", ")))
                            .color(egui::Color32::from_gray(140))
                            .size(11.0),
                    );
                }
            });
        }
        ui.add_space(4.0);
    }

    if let Some(control) = &content.construction {
        any_control = true;
        let label = match control.value {
            TriState::Mixed => "Construction (mixed)",
            _ => "Construction",
        };
        let mut checked = control.value == TriState::On;
        ui.add_enabled_ui(controls_enabled, |ui| {
            if shortcuts::checkbox_with_shortcut(
                ui,
                &mut checked,
                label,
                Some(shortcuts::TOGGLE_CONSTRUCTION),
            )
            .changed()
            {
                on_construction_changed(checked);
            }
        });
        if control.target_count > 1 {
            ui.label(
                egui::RichText::new(format!("{} items", control.target_count))
                    .color(egui::Color32::from_gray(140))
                    .size(11.0),
            );
        }
    }

    if !any_control {
        ui.label(
            egui::RichText::new("Select geometry or draw to edit properties")
                .color(egui::Color32::from_gray(140))
                .size(12.0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Circle, Document, FaceId, Line, Rect};
    use crate::selection::click_scene_selection;

    fn input<'a>(doc: &'a Document, selection: &'a SceneSelection) -> ContextInput<'a> {
        ContextInput {
            doc,
            selection,
            tool: Tool::Select,
            draw_rect_construction: None,
            draw_line_construction: None,
            draw_circle_construction: None,
        }
    }

    #[test]
    fn empty_when_nothing_selected() {
        let doc = Document::default();
        assert_eq!(
            context_pane_content(&input(&doc, &SceneSelection::default())),
            ContextPaneContent {
                name: None,
                construction: None,
                constraints: None,
            }
        );
    }

    #[test]
    fn shows_construction_union_for_multiple_selected() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 5.0, 0.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Bottom), false);
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        assert_eq!(
            context_pane_content(&input(&doc, &sel)),
            ContextPaneContent {
                name: None,
                construction: Some(ConstructionControl {
                    value: TriState::Off,
                    target_count: 2,
                }),
                constraints: None,
            }
        );
    }

    #[test]
    fn mixed_when_selected_edges_disagree() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        doc.rects[0].set_edge_construction(RectEdge::Bottom, true);
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Bottom), false);
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Top), true);
        assert_eq!(
            construction_tri_state(&doc, &construction_targets_from_selection(&sel)),
            TriState::Mixed
        );
    }

    #[test]
    fn whole_rectangle_expands_to_all_edges() {
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Rect(0), false);
        assert_eq!(construction_targets_from_selection(&sel).len(), 4);
    }

    #[test]
    fn shows_construction_while_drawing_rectangle() {
        let doc = Document::default();
        let content = context_pane_content(&ContextInput {
            doc: &doc,
            selection: &SceneSelection::default(),
            tool: Tool::Select,
            draw_rect_construction: Some(true),
            draw_line_construction: None,
            draw_circle_construction: None,
        });
        assert_eq!(
            content,
            ContextPaneContent {
                name: None,
                construction: Some(ConstructionControl {
                    value: TriState::On,
                    target_count: 1,
                }),
                constraints: None,
            }
        );
    }

    #[test]
    fn shows_name_when_single_element_selected() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 1.0, 0.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        assert_eq!(
            context_pane_content(&input(&doc, &sel)),
            ContextPaneContent {
                name: Some(NameControl {
                    element: SceneElement::Line(0),
                }),
                construction: Some(ConstructionControl {
                    value: TriState::Off,
                    target_count: 1,
                }),
                constraints: None,
            }
        );
    }

    #[test]
    fn shows_construction_before_drawing_when_rectangle_tool_active() {
        let doc = Document::default();
        let content = context_pane_content(&ContextInput {
            doc: &doc,
            selection: &SceneSelection::default(),
            tool: Tool::Select,
            draw_rect_construction: Some(false),
            draw_line_construction: None,
            draw_circle_construction: None,
        });
        assert_eq!(
            content.construction.unwrap().value,
            TriState::Off
        );
    }

    #[test]
    fn draw_mode_takes_precedence_over_selection() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 1.0, 0.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        let content = context_pane_content(&ContextInput {
            doc: &doc,
            selection: &sel,
            tool: Tool::Select,
            draw_rect_construction: Some(true),
            draw_line_construction: None,
            draw_circle_construction: None,
        });
        assert_eq!(
            content,
            ContextPaneContent {
                name: Some(NameControl {
                    element: SceneElement::Line(0),
                }),
                construction: Some(ConstructionControl {
                    value: TriState::On,
                    target_count: 1,
                }),
                constraints: None,
            }
        );
    }

    #[test]
    fn constraint_tool_shows_constraint_rows() {
        let doc = Document::default();
        let content = context_pane_content(&ContextInput {
            doc: &doc,
            selection: &SceneSelection::default(),
            tool: Tool::Constraint,
            draw_rect_construction: None,
            draw_line_construction: None,
            draw_circle_construction: None,
        });
        assert_eq!(
            content.constraints.as_ref().map(|rows| rows.len()),
            Some(crate::geometric_constraints::GeometricConstraintType::ALL.len())
        );
    }

    #[test]
    fn selection_highlight_dashed_for_construction_primitives() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 5.0, 0.0));
        doc.lines[0].construction = true;
        doc.circles
            .push(Circle::from_local_center_radius(sketch, 0.0, 0.0, 5.0, 0.0));
        doc.circles[0].construction = true;
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        doc.rects[0].set_edge_construction(RectEdge::Bottom, true);

        assert_eq!(
            selection_highlight_dashed(&doc, SceneElement::Line(0)),
            Some(true)
        );
        assert_eq!(
            selection_highlight_dashed(&doc, SceneElement::Circle(0)),
            Some(true)
        );
        assert_eq!(
            selection_highlight_dashed(&doc, SceneElement::RectEdge(0, RectEdge::Bottom)),
            Some(true)
        );
        assert_eq!(
            selection_highlight_dashed(&doc, SceneElement::RectEdge(0, RectEdge::Top)),
            Some(false)
        );
        assert_eq!(selection_highlight_dashed(&doc, SceneElement::Rect(0)), None);
    }

    #[test]
    fn set_construction_for_targets_updates_all() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 1.0, 1.0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 2.0, 0.0));
        let targets = vec![
            SceneElement::RectEdge(0, RectEdge::Left),
            SceneElement::Line(0),
        ];
        set_construction_for_targets(&mut doc, &targets, true).unwrap();
        assert!(doc.rects[0].edge_construction(RectEdge::Left));
        assert!(doc.lines[0].construction);
    }

    #[test]
    fn toggle_construction_for_targets_flips_each() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(sketch, 0.0, 0.0, 1.0, 1.0));
        doc.rects[0].set_edge_construction(RectEdge::Bottom, true);
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 2.0, 0.0));
        let targets = vec![
            SceneElement::RectEdge(0, RectEdge::Bottom),
            SceneElement::Line(0),
        ];
        toggle_construction_for_targets(&mut doc, &targets).unwrap();
        assert!(!doc.rects[0].edge_construction(RectEdge::Bottom));
        assert!(doc.lines[0].construction);
    }
}