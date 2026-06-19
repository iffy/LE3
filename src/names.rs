//! Custom names for scene elements shown in the Elements pane.

use crate::constraints::constraint_label;
use crate::hierarchy::{HierarchyNode, SceneElement};
use crate::model::Document;

/// Map a selected element to the object that owns a user-visible name.
pub fn nameable_element(element: SceneElement) -> Option<SceneElement> {
    match element {
        SceneElement::RectEdge(index, _) => Some(SceneElement::Rect(index)),
        SceneElement::ConstructionPlane(_)
        | SceneElement::Sketch(_)
        | SceneElement::Rect(_)
        | SceneElement::Line(_)
        | SceneElement::Circle(_)
        | SceneElement::Constraint(_) => Some(element),
        SceneElement::Point(_) => None,
    }
}

/// When exactly one nameable element is selected, return it.
pub fn single_nameable_from_selection(
    selection: &crate::selection::SceneSelection,
) -> Option<SceneElement> {
    selection.single().and_then(nameable_element)
}

fn name_matches(stored: Option<&str>, query: &str) -> bool {
    stored
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some_and(|s| s == query)
}

/// Find the first scene element with the given custom name (case-sensitive).
pub fn find_element_by_name(doc: &Document, name: &str) -> Option<SceneElement> {
    let query = name.trim();
    if query.is_empty() {
        return None;
    }
    for (index, plane) in doc.construction_planes.iter().enumerate() {
        if name_matches(plane.name.as_deref(), query) {
            return Some(SceneElement::ConstructionPlane(index));
        }
    }
    for (index, sketch) in doc.sketches.iter().enumerate() {
        if name_matches(sketch.name.as_deref(), query) {
            return Some(SceneElement::Sketch(index));
        }
    }
    for (index, line) in doc.lines.iter().enumerate() {
        if line.deleted {
            continue;
        }
        if name_matches(line.name.as_deref(), query) {
            return Some(SceneElement::Line(index));
        }
    }
    for (index, rect) in doc.rects.iter().enumerate() {
        if rect.deleted {
            continue;
        }
        if name_matches(rect.name.as_deref(), query) {
            return Some(SceneElement::Rect(index));
        }
    }
    for (index, circle) in doc.circles.iter().enumerate() {
        if circle.deleted {
            continue;
        }
        if name_matches(circle.name.as_deref(), query) {
            return Some(SceneElement::Circle(index));
        }
    }
    for (index, constraint) in doc.constraints.iter().enumerate() {
        if constraint.deleted {
            continue;
        }
        if name_matches(constraint.name.as_deref(), query) {
            return Some(SceneElement::Constraint(index));
        }
    }
    None
}

pub fn element_name(doc: &Document, element: SceneElement) -> Option<&str> {
    let name = match element {
        SceneElement::ConstructionPlane(index) => doc.construction_planes.get(index)?.name.as_deref(),
        SceneElement::Sketch(index) => doc.sketches.get(index)?.name.as_deref(),
        SceneElement::Rect(index) => doc.rects.get(index)?.name.as_deref(),
        SceneElement::Line(index) => doc.lines.get(index)?.name.as_deref(),
        SceneElement::Circle(index) => doc.circles.get(index)?.name.as_deref(),
        SceneElement::Constraint(index) => doc.constraints.get(index)?.name.as_deref(),
        SceneElement::RectEdge(_, _) | SceneElement::Point(_) => None,
    }?;
    let trimmed = name.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub fn set_element_name(doc: &mut Document, element: SceneElement, name: String) -> Result<(), String> {
    let stored = {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };
    match element {
        SceneElement::ConstructionPlane(index) => {
            let plane = doc
                .construction_planes
                .get_mut(index)
                .ok_or_else(|| format!("construction plane {index} not found"))?;
            plane.name = stored;
        }
        SceneElement::Sketch(index) => {
            let sketch = doc
                .sketches
                .get_mut(index)
                .ok_or_else(|| format!("sketch {index} not found"))?;
            sketch.name = stored;
        }
        SceneElement::Rect(index) => {
            let rect = doc
                .rects
                .get_mut(index)
                .ok_or_else(|| format!("rectangle {index} not found"))?;
            rect.name = stored;
        }
        SceneElement::Line(index) => {
            let line = doc
                .lines
                .get_mut(index)
                .ok_or_else(|| format!("line {index} not found"))?;
            line.name = stored;
        }
        SceneElement::Circle(index) => {
            let circle = doc
                .circles
                .get_mut(index)
                .ok_or_else(|| format!("circle {index} not found"))?;
            circle.name = stored;
        }
        SceneElement::Constraint(index) => {
            let constraint = doc
                .constraints
                .get_mut(index)
                .ok_or_else(|| format!("constraint {index} not found"))?;
            constraint.name = stored;
        }
        SceneElement::RectEdge(_, _) => {
            return Err("rectangle edges cannot be renamed".to_string());
        }
        SceneElement::Point(_) => {
            return Err("points cannot be renamed".to_string());
        }
    }
    Ok(())
}

pub fn default_node_label(doc: &Document, node: HierarchyNode) -> String {
    match node {
        HierarchyNode::ConstructionPlane(i) => {
            if i == 0 {
                "Construction plane (XY)".to_string()
            } else {
                format!("Construction plane {i}")
            }
        }
        HierarchyNode::Sketch(i) => format!("Sketch {i}"),
        HierarchyNode::Rect(i) => {
            let rect = &doc.rects[i];
            format!("Rectangle {i} ({:.1} × {:.1} mm)", rect.w, rect.h)
        }
        HierarchyNode::Line(i) => {
            let len = doc.lines[i].length();
            format!("Line {i} ({len:.1} mm)")
        }
        HierarchyNode::Circle(i) => {
            let diameter = doc.circles[i].diameter();
            format!("Circle {i} (Ø{diameter:.1} mm)")
        }
        HierarchyNode::Constraint(i) => constraint_label(doc, i),
    }
}

pub fn node_label(doc: &Document, node: HierarchyNode) -> String {
    let element = crate::hierarchy::scene_element_for_node(node);
    element_name(doc, element)
        .map(str::to_string)
        .unwrap_or_else(|| default_node_label(doc, node))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::add_distance_constraint;
    use crate::model::{Document, FaceId, Line, Rect};
    use crate::selection::{click_scene_selection, SceneSelection};

    #[test]
    fn rect_edge_maps_to_parent_rect_for_naming() {
        assert_eq!(
            nameable_element(SceneElement::RectEdge(2, crate::model::RectEdge::Top)),
            Some(SceneElement::Rect(2))
        );
        assert_eq!(
            nameable_element(SceneElement::Line(0)),
            Some(SceneElement::Line(0))
        );
    }

    #[test]
    fn single_nameable_requires_exactly_one_selected() {
        let mut sel = SceneSelection::default();
        assert_eq!(single_nameable_from_selection(&sel), None);
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        assert_eq!(
            single_nameable_from_selection(&sel),
            Some(SceneElement::Line(0))
        );
        click_scene_selection(&mut sel, SceneElement::Rect(1), true);
        assert_eq!(single_nameable_from_selection(&sel), None);
    }

    #[test]
    fn custom_name_replaces_default_label() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        set_element_name(&mut doc, SceneElement::Line(0), "Guide".to_string()).unwrap();
        assert_eq!(node_label(&doc, HierarchyNode::Line(0)), "Guide");
        assert_eq!(
            element_name(&doc, SceneElement::Line(0)),
            Some("Guide")
        );
    }

    #[test]
    fn empty_name_clears_custom_label() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        set_element_name(&mut doc, SceneElement::Rect(0), "Box".to_string()).unwrap();
        set_element_name(&mut doc, SceneElement::Rect(0), "   ".to_string()).unwrap();
        assert_eq!(element_name(&doc, SceneElement::Rect(0)), None);
        assert!(node_label(&doc, HierarchyNode::Rect(0)).starts_with("Rectangle 0"));
    }

    #[test]
    fn find_element_by_name_returns_first_match() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        set_element_name(&mut doc, SceneElement::Line(0), "Guide".to_string()).unwrap();
        set_element_name(&mut doc, SceneElement::Rect(0), "Guide".to_string()).unwrap();
        assert_eq!(
            find_element_by_name(&doc, "Guide"),
            Some(SceneElement::Line(0))
        );
        assert_eq!(find_element_by_name(&doc, "Missing"), None);
    }

    #[test]
    fn constraint_custom_name_shown_in_elements_pane() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            crate::model::DistanceTarget::LineLength(0),
            "10mm".to_string(),
        )
        .unwrap();
        set_element_name(&mut doc, SceneElement::Constraint(0), "Length lock".to_string())
            .unwrap();
        assert_eq!(
            node_label(&doc, HierarchyNode::Constraint(0)),
            "Length lock"
        );
    }
}