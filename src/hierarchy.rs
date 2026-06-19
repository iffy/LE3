//! Scene tree pane: construction planes, sketches, and sketch geometry.

/// Side-panel title shown in the UI.
pub const PANE_TITLE: &str = "Tree";

use crate::actions::SketchSession;
use crate::model::{ConstructionPlaneParent, Document, FaceId, RectEdge, SketchId};
use crate::selection::SceneSelection;
use eframe::egui;
use std::collections::HashSet;

/// A node in the scene hierarchy tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HierarchyNode {
    ConstructionPlane(usize),
    Sketch(SketchId),
    Rect(usize),
    Line(usize),
}

/// Identifies an element whose visibility can be toggled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SceneElement {
    ConstructionPlane(usize),
    Sketch(SketchId),
    Rect(usize),
    Line(usize),
    RectEdge(usize, RectEdge),
}

impl From<HierarchyNode> for SceneElement {
    fn from(node: HierarchyNode) -> Self {
        match node {
            HierarchyNode::ConstructionPlane(i) => SceneElement::ConstructionPlane(i),
            HierarchyNode::Sketch(i) => SceneElement::Sketch(i),
            HierarchyNode::Rect(i) => SceneElement::Rect(i),
            HierarchyNode::Line(i) => SceneElement::Line(i),
        }
    }
}

/// User-toggled visibility for scene elements. Absent entries are visible.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElementVisibility {
    hidden: HashSet<SceneElement>,
}

impl ElementVisibility {
    pub fn is_visible(&self, element: SceneElement) -> bool {
        !self.hidden.contains(&element)
    }

    pub fn set_visible(&mut self, element: SceneElement, visible: bool) {
        if visible {
            self.hidden.remove(&element);
        } else {
            self.hidden.insert(element);
        }
    }

    pub fn toggle(&mut self, element: SceneElement) -> bool {
        let next = !self.is_visible(element);
        self.set_visible(element, next);
        next
    }

    pub fn effective_visible(&self, doc: &Document, element: SceneElement) -> bool {
        if !self.is_visible(element) {
            return false;
        }
        match element {
            SceneElement::ConstructionPlane(index) => doc
                .construction_planes
                .get(index)
                .map(|plane| match plane.parent {
                    ConstructionPlaneParent::Root => true,
                    ConstructionPlaneParent::Sketch(sketch) => {
                        self.effective_visible(doc, SceneElement::Sketch(sketch))
                    }
                })
                .unwrap_or(true),
            SceneElement::Sketch(sketch) => doc
                .sketch_face(sketch)
                .is_some_and(|face| self.effective_visible(doc, face_element(face))),
            SceneElement::Rect(index) => doc.rects.get(index).is_some_and(|rect| {
                self.effective_visible(doc, SceneElement::Sketch(rect.sketch))
            }),
            SceneElement::Line(index) => doc.lines.get(index).is_some_and(|line| {
                self.effective_visible(doc, SceneElement::Sketch(line.sketch))
            }),
            SceneElement::RectEdge(index, _) => doc.rects.get(index).is_some_and(|rect| {
                self.effective_visible(doc, SceneElement::Sketch(rect.sketch))
            }),
        }
    }
}

fn face_element(face: FaceId) -> SceneElement {
    match face {
        FaceId::ConstructionPlane(i) => SceneElement::ConstructionPlane(i),
        FaceId::Rect(i) => SceneElement::Rect(i),
    }
}

/// A hierarchy entry with optional children for tree display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HierarchyEntry {
    pub node: HierarchyNode,
    pub children: Vec<HierarchyEntry>,
}

/// Build the hierarchy tree for the current view context.
pub fn build_hierarchy(
    doc: &Document,
    sketch_session: Option<SketchSession>,
) -> Vec<HierarchyEntry> {
    let mut roots = Vec::new();
    for (i, plane) in doc.construction_planes.iter().enumerate() {
        if !matches!(plane.parent, ConstructionPlaneParent::Root) {
            continue;
        }
        let face = FaceId::ConstructionPlane(i);
        let children = build_face_sketches(doc, face, sketch_session);
        roots.push(HierarchyEntry {
            node: HierarchyNode::ConstructionPlane(i),
            children,
        });
    }
    roots
}

fn build_face_sketches(
    doc: &Document,
    face: FaceId,
    sketch_session: Option<SketchSession>,
) -> Vec<HierarchyEntry> {
    doc.sketches_on_face(face)
        .map(|sketch| build_sketch_entry(doc, sketch, sketch_session))
        .collect()
}

fn build_sketch_child_planes(
    doc: &Document,
    sketch: SketchId,
    sketch_session: Option<SketchSession>,
) -> Vec<HierarchyEntry> {
    let mut children = Vec::new();
    for (pi, plane) in doc.construction_planes.iter().enumerate() {
        if !matches!(plane.parent, ConstructionPlaneParent::Sketch(s) if s == sketch) {
            continue;
        }
        let face = FaceId::ConstructionPlane(pi);
        children.push(HierarchyEntry {
            node: HierarchyNode::ConstructionPlane(pi),
            children: build_face_sketches(doc, face, sketch_session),
        });
    }
    children
}

fn build_sketch_entry(
    doc: &Document,
    sketch: SketchId,
    sketch_session: Option<SketchSession>,
) -> HierarchyEntry {
    let mut children = build_sketch_child_planes(doc, sketch, sketch_session);

    if sketch_session.is_some_and(|s| s.sketch == sketch) {
        for (ri, rect) in doc.rects.iter().enumerate() {
            if rect.sketch == sketch {
                let nested = build_face_sketches(doc, FaceId::Rect(ri), sketch_session);
                children.push(HierarchyEntry {
                    node: HierarchyNode::Rect(ri),
                    children: nested,
                });
            }
        }
        for (li, line) in doc.lines.iter().enumerate() {
            if line.sketch == sketch {
                children.push(HierarchyEntry {
                    node: HierarchyNode::Line(li),
                    children: vec![],
                });
            }
        }
    } else {
        for (ri, rect) in doc.rects.iter().enumerate() {
            if rect.sketch == sketch {
                let nested = build_face_sketches(doc, FaceId::Rect(ri), sketch_session);
                if !nested.is_empty() {
                    children.push(HierarchyEntry {
                        node: HierarchyNode::Rect(ri),
                        children: nested,
                    });
                }
            }
        }
    }

    HierarchyEntry {
        node: HierarchyNode::Sketch(sketch),
        children,
    }
}

pub fn node_label(doc: &Document, node: HierarchyNode) -> String {
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
    }
}

pub fn scene_element_for_node(node: HierarchyNode) -> SceneElement {
    SceneElement::from(node)
}

/// Draw the scene tree in a side panel.
pub fn show_pane(
    ui: &mut egui::Ui,
    doc: &Document,
    sketch_session: Option<SketchSession>,
    visibility: &mut ElementVisibility,
    selection: &SceneSelection,
    on_edit_sketch: &mut impl FnMut(SketchId),
    on_edit_plane: &mut impl FnMut(usize),
    on_toggle_visibility: &mut impl FnMut(SceneElement, bool),
    on_click_element: &mut impl FnMut(SceneElement, bool),
) {
    ui.heading(PANE_TITLE);
    ui.separator();

    egui::ScrollArea::vertical().show(ui, |ui| {
        let tree = build_hierarchy(doc, sketch_session);
        for entry in tree {
            show_entry(
                ui,
                doc,
                &entry,
                visibility,
                selection,
                on_edit_sketch,
                on_edit_plane,
                on_toggle_visibility,
                on_click_element,
                0,
            );
        }
    });
}

fn show_entry(
    ui: &mut egui::Ui,
    doc: &Document,
    entry: &HierarchyEntry,
    visibility: &mut ElementVisibility,
    selection: &SceneSelection,
    on_edit_sketch: &mut impl FnMut(SketchId),
    on_edit_plane: &mut impl FnMut(usize),
    on_toggle_visibility: &mut impl FnMut(SceneElement, bool),
    on_click_element: &mut impl FnMut(SceneElement, bool),
    depth: usize,
) {
    let element = scene_element_for_node(entry.node);
    let visible = visibility.effective_visible(doc, element);
    let indent = depth as f32 * 14.0;

    ui.horizontal(|ui| {
        ui.add_space(indent);
        let eye = if visible { "👁" } else { "◌" };
        if ui
            .button(eye)
            .on_hover_text(if visible { "Hide" } else { "Show" })
            .clicked()
        {
            let next = visibility.toggle(element);
            on_toggle_visibility(element, next);
        }

        let label = node_label(doc, entry.node);
        let selected = selection.is_selected(element)
            || matches!(element, SceneElement::Rect(index) if selection.has_rect_edge_selected(index));
        let response = ui.selectable_label(selected, label);
        match entry.node {
            HierarchyNode::Sketch(sketch) => {
                if response.double_clicked() {
                    on_edit_sketch(sketch);
                } else if response.clicked() {
                    let additive = ui.input(|i| i.modifiers.command);
                    on_click_element(element, additive);
                }
                response.context_menu(|ui| {
                    if ui.button("Edit sketch").clicked() {
                        on_edit_sketch(sketch);
                        ui.close_menu();
                    }
                });
            }
            HierarchyNode::ConstructionPlane(index) => {
                if response.clicked() {
                    let additive = ui.input(|i| i.modifiers.command);
                    on_click_element(element, additive);
                }
                response.context_menu(|ui| {
                    if ui.button("Edit plane").clicked() {
                        on_edit_plane(index);
                        ui.close_menu();
                    }
                });
            }
            HierarchyNode::Rect(_) | HierarchyNode::Line(_) => {
                if response.clicked() {
                    let additive = ui.input(|i| i.modifiers.command);
                    on_click_element(element, additive);
                }
            }
        }
    });

    for child in &entry.children {
        show_entry(
            ui,
            doc,
            child,
            visibility,
            selection,
            on_edit_sketch,
            on_edit_plane,
            on_toggle_visibility,
            on_click_element,
            depth + 1,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::construction::{definition_from_reference, plane_from_definition};
    use crate::face::default_xy_plane;
    use crate::construction::PlaneReference;
    use crate::model::{ConstructionPlaneParent, Line, Rect};

    fn doc_with_plane_sketches() -> Document {
        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        let s1 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects.push(Rect::from_local_corners(s0, 0.0, 0.0, 10.0, 10.0));
        doc.lines
            .push(Line::from_local_endpoints(s1, 0.0, 0.0, 5.0, 0.0));
        doc
    }

    #[test]
    fn main_view_shows_planes_and_sketches_only() {
        let doc = doc_with_plane_sketches();
        let tree = build_hierarchy(&doc, None);
        assert_eq!(tree.len(), 1);
        let plane = &tree[0];
        assert_eq!(plane.node, HierarchyNode::ConstructionPlane(0));
        assert_eq!(plane.children.len(), 2);
        assert!(plane
            .children
            .iter()
            .all(|c| matches!(c.node, HierarchyNode::Sketch(_))));
        assert!(plane.children[0].children.is_empty());
        assert!(plane.children[1].children.is_empty());
    }

    #[test]
    fn sketch_view_shows_geometry_of_active_sketch() {
        let doc = doc_with_plane_sketches();
        let tree = build_hierarchy(&doc, Some(SketchSession { sketch: 0 }));
        let sketch0 = &tree[0].children[0];
        assert_eq!(sketch0.node, HierarchyNode::Sketch(0));
        assert_eq!(sketch0.children.len(), 1);
        assert_eq!(sketch0.children[0].node, HierarchyNode::Rect(0));

        let sketch1 = &tree[0].children[1];
        assert!(sketch1.children.is_empty());

        let tree = build_hierarchy(&doc, Some(SketchSession { sketch: 1 }));
        let sketch1 = &tree[0].children[1];
        assert_eq!(sketch1.children.len(), 1);
        assert_eq!(sketch1.children[0].node, HierarchyNode::Line(0));
    }

    #[test]
    fn nested_sketches_on_rect_face_appear_under_parent_sketch() {
        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(s0, 0.0, 0.0, 20.0, 20.0));
        let s1 = doc.add_sketch(FaceId::Rect(0));
        let _ = s1;

        let tree = build_hierarchy(&doc, None);
        let sketch0 = &tree[0].children[0];
        assert_eq!(sketch0.children.len(), 1);
        assert_eq!(sketch0.children[0].node, HierarchyNode::Rect(0));
        assert_eq!(sketch0.children[0].children.len(), 1);
        assert_eq!(
            sketch0.children[0].children[0].node,
            HierarchyNode::Sketch(1)
        );
    }

    #[test]
    fn plane_from_sketch_geometry_nests_under_sketch() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        let derived = plane_from_definition(
            &definition_from_reference(
                &PlaneReference::Face {
                    origin: glam::Vec3::ZERO,
                    normal: glam::Vec3::Z,
                    label: "Ground".to_string(),
                },
                5.0,
                0.0,
            ),
            ConstructionPlaneParent::Sketch(sketch),
        );
        doc.construction_planes.push(derived);

        let tree = build_hierarchy(&doc, None);
        assert_eq!(tree.len(), 1);
        let sketch_entry = &tree[0].children[0];
        assert_eq!(sketch_entry.node, HierarchyNode::Sketch(0));
        assert_eq!(sketch_entry.children.len(), 1);
        assert_eq!(
            sketch_entry.children[0].node,
            HierarchyNode::ConstructionPlane(1)
        );
    }

    #[test]
    fn hiding_sketch_hides_derived_construction_plane() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.construction_planes.push(plane_from_definition(
            &default_xy_plane().definition,
            ConstructionPlaneParent::Sketch(sketch),
        ));

        let mut vis = ElementVisibility::default();
        vis.set_visible(SceneElement::Sketch(sketch), false);
        assert!(!vis.effective_visible(&doc, SceneElement::ConstructionPlane(1)));
    }

    #[test]
    fn hiding_sketch_hides_child_geometry() {
        let doc = doc_with_plane_sketches();
        let mut vis = ElementVisibility::default();
        vis.set_visible(SceneElement::Sketch(0), false);
        assert!(!vis.effective_visible(&doc, SceneElement::Rect(0)));
        assert!(vis.effective_visible(&doc, SceneElement::Line(0)));
    }

    #[test]
    fn toggle_visibility_flips_state() {
        let mut vis = ElementVisibility::default();
        assert!(vis.is_visible(SceneElement::Sketch(0)));
        assert!(!vis.toggle(SceneElement::Sketch(0)));
        assert!(!vis.is_visible(SceneElement::Sketch(0)));
    }

    #[test]
    fn pane_title_is_tree() {
        assert_eq!(PANE_TITLE, "Tree");
    }
}