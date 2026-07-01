//! Scene element selection from the elements pane and viewport.

use crate::hierarchy::SceneElement;
use eframe::egui;
use std::collections::HashSet;

/// Shift+click or ⌘/Ctrl+click adds to the current selection instead of replacing it.
pub fn additive_click_modifiers(modifiers: &egui::Modifiers) -> bool {
    modifiers.command || modifiers.shift
}

/// Objects selected in the elements pane.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneSelection {
    elements: HashSet<SceneElement>,
}

impl SceneSelection {
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn is_selected(&self, element: SceneElement) -> bool {
        self.elements.contains(&element)
    }

    pub fn iter(&self) -> impl Iterator<Item = SceneElement> + '_ {
        self.elements.iter().cloned()
    }

    pub fn clear(&mut self) {
        self.elements.clear();
    }

    pub fn has_rect_edge_selected(&self, rect_index: usize) -> bool {
        self.elements.iter().any(|element| {
            matches!(element, SceneElement::RectEdge(index, _) if *index == rect_index)
        })
    }

    /// The sole selected element, if exactly one is selected.
    pub fn single(&self) -> Option<SceneElement> {
        let mut iter = self.iter();
        let first = iter.next()?;
        if iter.next().is_some() {
            None
        } else {
            Some(first)
        }
    }
}

/// Click an elements row: deselect when already selected; replace selection unless additive.
pub fn click_scene_selection(
    selection: &mut SceneSelection,
    element: SceneElement,
    additive: bool,
) {
    if selection.is_selected(element.clone()) {
        selection.elements.remove(&element);
        return;
    }
    if !additive {
        selection.clear();
    }
    selection.elements.insert(element);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection_count(selection: &SceneSelection) -> usize {
        selection.iter().count()
    }

    fn selection_single(selection: &SceneSelection) -> Option<SceneElement> {
        let mut iter = selection.iter();
        let first = iter.next()?;
        if iter.next().is_none() {
            Some(first)
        } else {
            None
        }
    }

    #[test]
    fn single_returns_one_selected_element() {
        let mut sel = SceneSelection::default();
        assert_eq!(sel.single(), None);
        click_scene_selection(&mut sel, SceneElement::Rect(0), false);
        assert_eq!(sel.single(), Some(SceneElement::Rect(0)));
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        assert_eq!(sel.single(), None);
    }

    #[test]
    fn click_replaces_selection_without_modifier() {
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Rect(0), false);
        assert_eq!(selection_single(&sel), Some(SceneElement::Rect(0)));
        click_scene_selection(&mut sel, SceneElement::Line(1), false);
        assert_eq!(selection_single(&sel), Some(SceneElement::Line(1)));
    }

    #[test]
    fn click_selected_deselects() {
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Rect(0), false);
        click_scene_selection(&mut sel, SceneElement::Rect(0), false);
        assert!(sel.is_empty());
    }

    #[test]
    fn additive_click_builds_multi_selection() {
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Rect(0), false);
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        assert_eq!(selection_count(&sel), 2);
        assert!(sel.is_selected(SceneElement::Rect(0)));
        assert!(sel.is_selected(SceneElement::Line(1)));
    }

    #[test]
    fn additive_click_rect_edges() {
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, crate::model::RectEdge::Bottom), false);
        click_scene_selection(
            &mut sel,
            SceneElement::RectEdge(0, crate::model::RectEdge::Top),
            true,
        );
        assert_eq!(selection_count(&sel), 2);
        assert!(sel.has_rect_edge_selected(0));
    }

    #[test]
    fn additive_click_selected_deselects_one() {
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Rect(0), false);
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        click_scene_selection(&mut sel, SceneElement::Rect(0), true);
        assert_eq!(selection_single(&sel), Some(SceneElement::Line(1)));
    }

    #[test]
    fn additive_click_modifiers_command() {
        let modifiers = egui::Modifiers {
            command: true,
            ..Default::default()
        };
        assert!(additive_click_modifiers(&modifiers));
    }

    #[test]
    fn additive_click_modifiers_shift() {
        let modifiers = egui::Modifiers {
            shift: true,
            ..Default::default()
        };
        assert!(additive_click_modifiers(&modifiers));
    }

    #[test]
    fn additive_click_modifiers_plain_click() {
        assert!(!additive_click_modifiers(&egui::Modifiers::default()));
    }
}