//! Scene element selection from the tree pane and viewport.

use crate::hierarchy::SceneElement;
use std::collections::HashSet;

/// Objects selected in the tree pane.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneSelection {
    elements: HashSet<SceneElement>,
}

impl SceneSelection {
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub fn len(&self) -> usize {
        self.elements.len()
    }

    pub fn is_selected(&self, element: SceneElement) -> bool {
        self.elements.contains(&element)
    }

    pub fn iter(&self) -> impl Iterator<Item = SceneElement> + '_ {
        self.elements.iter().copied()
    }

    pub fn single(&self) -> Option<SceneElement> {
        if self.elements.len() == 1 {
            self.elements.iter().copied().next()
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.elements.clear();
    }

    pub fn has_rect_edge_selected(&self, rect_index: usize) -> bool {
        self.elements.iter().any(|element| {
            matches!(element, SceneElement::RectEdge(index, _) if *index == rect_index)
        })
    }
}

/// Click a tree row: deselect when already selected; replace selection unless additive.
pub fn click_scene_selection(
    selection: &mut SceneSelection,
    element: SceneElement,
    additive: bool,
) {
    if selection.is_selected(element) {
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

    #[test]
    fn click_replaces_selection_without_modifier() {
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Rect(0), false);
        assert_eq!(sel.single(), Some(SceneElement::Rect(0)));
        click_scene_selection(&mut sel, SceneElement::Line(1), false);
        assert_eq!(sel.single(), Some(SceneElement::Line(1)));
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
        assert_eq!(sel.len(), 2);
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
        assert_eq!(sel.len(), 2);
        assert!(sel.has_rect_edge_selected(0));
    }

    #[test]
    fn additive_click_selected_deselects_one() {
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Rect(0), false);
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        click_scene_selection(&mut sel, SceneElement::Rect(0), true);
        assert_eq!(sel.single(), Some(SceneElement::Line(1)));
    }
}