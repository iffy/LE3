//! Context pane: union of editable properties for the current selection or draw op.

use crate::hierarchy::SceneElement;
use crate::model::{Document, RectEdge};
use crate::selection::SceneSelection;
use crate::shortcuts;

pub const PANE_TITLE: &str = "Context";

/// Inputs needed to build context pane content (kept separate from [`AppState`] to avoid cycles).
pub struct ContextInput<'a> {
    pub doc: &'a Document,
    pub selection: &'a SceneSelection,
    pub draw_rect_construction: Option<bool>,
    pub draw_line_construction: Option<bool>,
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
    pub construction: Option<ConstructionControl>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstructionControl {
    pub value: TriState,
    pub target_count: usize,
}

pub fn context_pane_content(input: &ContextInput<'_>) -> ContextPaneContent {
    if let Some(construction) = input.draw_rect_construction {
        return ContextPaneContent {
            construction: Some(ConstructionControl {
                value: tri_state_from_bool(construction),
                target_count: 1,
            }),
        };
    }
    if let Some(construction) = input.draw_line_construction {
        return ContextPaneContent {
            construction: Some(ConstructionControl {
                value: tri_state_from_bool(construction),
                target_count: 1,
            }),
        };
    }

    let targets = construction_targets_from_selection(input.selection);
    ContextPaneContent {
        construction: (!targets.is_empty()).then(|| ConstructionControl {
            value: construction_tri_state(input.doc, &targets),
            target_count: targets.len(),
        }),
    }
}

pub fn construction_targets_from_selection(selection: &SceneSelection) -> Vec<SceneElement> {
    let mut targets = Vec::new();
    for element in selection.iter() {
        match element {
            SceneElement::Line(_) | SceneElement::RectEdge(_, _) => targets.push(element),
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
        SceneElement::RectEdge(i, edge) => (1, i, edge.index() as u8),
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
        _ => None,
    }
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
        _ => Err("Only lines and rectangle edges support construction mode".to_string()),
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
    ui: &mut eframe::egui::Ui,
    content: &ContextPaneContent,
    on_construction_changed: &mut impl FnMut(bool),
) {
    ui.heading(PANE_TITLE);
    ui.separator();

    let Some(control) = &content.construction else {
        ui.label(
            eframe::egui::RichText::new("Select geometry or draw to edit properties")
                .color(eframe::egui::Color32::from_gray(140))
                .size(12.0),
        );
        return;
    };

    let label = match control.value {
        TriState::Mixed => "Construction (mixed)",
        _ => "Construction",
    };
    let mut checked = control.value == TriState::On;
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
    if control.target_count > 1 {
        ui.label(
            eframe::egui::RichText::new(format!("{} items", control.target_count))
                .color(eframe::egui::Color32::from_gray(140))
                .size(11.0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Document, FaceId, Line, Rect};
    use crate::selection::click_scene_selection;

    fn input<'a>(doc: &'a Document, selection: &'a SceneSelection) -> ContextInput<'a> {
        ContextInput {
            doc,
            selection,
            draw_rect_construction: None,
            draw_line_construction: None,
        }
    }

    #[test]
    fn empty_when_nothing_selected() {
        let doc = Document::default();
        assert_eq!(
            context_pane_content(&input(&doc, &SceneSelection::default())),
            ContextPaneContent { construction: None }
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
                construction: Some(ConstructionControl {
                    value: TriState::Off,
                    target_count: 2,
                }),
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
            draw_rect_construction: Some(true),
            draw_line_construction: None,
        });
        assert_eq!(
            content,
            ContextPaneContent {
                construction: Some(ConstructionControl {
                    value: TriState::On,
                    target_count: 1,
                }),
            }
        );
    }

    #[test]
    fn shows_construction_before_drawing_when_rectangle_tool_active() {
        let doc = Document::default();
        let content = context_pane_content(&ContextInput {
            doc: &doc,
            selection: &SceneSelection::default(),
            draw_rect_construction: Some(false),
            draw_line_construction: None,
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
            draw_rect_construction: Some(true),
            draw_line_construction: None,
        });
        assert_eq!(
            content,
            ContextPaneContent {
                construction: Some(ConstructionControl {
                    value: TriState::On,
                    target_count: 1,
                }),
            }
        );
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