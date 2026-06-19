//! Elements pane: construction planes, sketches, and sketch geometry.

/// Side-panel title shown in the UI.
pub const PANE_TITLE: &str = "Elements";

use crate::actions::SketchSession;
use crate::constraints::constraint_label;
use crate::model::{ConstructionPlaneParent, Document, FaceId, RectEdge, ShapeKind, SketchId};
use crate::selection::SceneSelection;
use eframe::egui::{self, Color32, RichText};
use std::collections::{HashMap, HashSet};

/// A node in the scene hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HierarchyNode {
    ConstructionPlane(usize),
    Sketch(SketchId),
    Rect(usize),
    Line(usize),
    Constraint(usize),
}

/// Identifies an element whose visibility can be toggled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SceneElement {
    ConstructionPlane(usize),
    Sketch(SketchId),
    Rect(usize),
    Line(usize),
    RectEdge(usize, RectEdge),
    Constraint(usize),
}

impl From<HierarchyNode> for SceneElement {
    fn from(node: HierarchyNode) -> Self {
        match node {
            HierarchyNode::ConstructionPlane(i) => SceneElement::ConstructionPlane(i),
            HierarchyNode::Sketch(i) => SceneElement::Sketch(i),
            HierarchyNode::Rect(i) => SceneElement::Rect(i),
            HierarchyNode::Line(i) => SceneElement::Line(i),
            HierarchyNode::Constraint(i) => SceneElement::Constraint(i),
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
            SceneElement::Constraint(index) => doc.constraints.get(index).is_some_and(|c| {
                self.effective_visible(doc, SceneElement::Sketch(c.sketch))
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

/// A hierarchy entry with optional children (used to derive parent links).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HierarchyEntry {
    pub node: HierarchyNode,
    pub children: Vec<HierarchyEntry>,
}

#[derive(Clone, Debug, Default)]
struct CreationRanks {
    sketches: HashMap<SketchId, usize>,
    rects: HashMap<usize, usize>,
    lines: HashMap<usize, usize>,
    constraints: HashMap<usize, usize>,
    planes: HashMap<usize, usize>,
}

fn build_creation_ranks(doc: &Document) -> CreationRanks {
    let mut ranks = CreationRanks::default();
    ranks.planes.insert(0, 0);
    let mut sketch_n = 0usize;
    let mut rect_n = 0usize;
    let mut line_n = 0usize;
    let mut constraint_n = 0usize;
    let mut plane_n = 1usize;
    for (rank, kind) in doc.shape_order.iter().enumerate() {
        match kind {
            ShapeKind::Sketch => {
                ranks.sketches.insert(sketch_n, rank);
                sketch_n += 1;
            }
            ShapeKind::Rect => {
                ranks.rects.insert(rect_n, rank);
                rect_n += 1;
            }
            ShapeKind::Line => {
                ranks.lines.insert(line_n, rank);
                line_n += 1;
            }
            ShapeKind::Constraint => {
                ranks.constraints.insert(constraint_n, rank);
                constraint_n += 1;
            }
            ShapeKind::ConstructionPlane => {
                ranks.planes.insert(plane_n, rank);
                plane_n += 1;
            }
            ShapeKind::Parameter => {}
        }
    }
    ranks
}

fn creation_rank(ranks: &CreationRanks, node: HierarchyNode) -> usize {
    match node {
        HierarchyNode::ConstructionPlane(i) => *ranks.planes.get(&i).unwrap_or(&i),
        HierarchyNode::Sketch(i) => *ranks.sketches.get(&i).unwrap_or(&i),
        HierarchyNode::Rect(i) => *ranks.rects.get(&i).unwrap_or(&i),
        HierarchyNode::Line(i) => *ranks.lines.get(&i).unwrap_or(&i),
        HierarchyNode::Constraint(i) => *ranks.constraints.get(&i).unwrap_or(&i),
    }
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

/// Flat element list: parents always above descendants; newer elements after older ones when possible.
pub fn build_element_list(
    doc: &Document,
    sketch_session: Option<SketchSession>,
) -> Vec<HierarchyNode> {
    let tree = build_hierarchy(doc, sketch_session);
    let ranks = build_creation_ranks(doc);
    let mut nodes = Vec::new();
    let mut parent_of = HashMap::new();
    for entry in &tree {
        collect_with_parents(entry, None, &mut nodes, &mut parent_of);
    }
    topological_flat_sort(nodes, parent_of, |node| creation_rank(&ranks, node))
}

fn collect_with_parents(
    entry: &HierarchyEntry,
    parent: Option<HierarchyNode>,
    nodes: &mut Vec<HierarchyNode>,
    parent_of: &mut HashMap<HierarchyNode, HierarchyNode>,
) {
    if let Some(parent) = parent {
        parent_of.insert(entry.node, parent);
    }
    nodes.push(entry.node);
    for child in &entry.children {
        collect_with_parents(child, Some(entry.node), nodes, parent_of);
    }
}

fn topological_flat_sort(
    nodes: Vec<HierarchyNode>,
    parent_of: HashMap<HierarchyNode, HierarchyNode>,
    rank: impl Fn(HierarchyNode) -> usize,
) -> Vec<HierarchyNode> {
    let mut remaining: HashSet<HierarchyNode> = nodes.into_iter().collect();
    let mut result = Vec::new();
    while !remaining.is_empty() {
        let mut ready: Vec<HierarchyNode> = remaining
            .iter()
            .filter(|node| {
                parent_of
                    .get(node)
                    .map(|parent| !remaining.contains(parent))
                    .unwrap_or(true)
            })
            .copied()
            .collect();
        ready.sort_by_key(|node| rank(*node));
        for node in ready {
            remaining.remove(&node);
            result.push(node);
        }
    }
    result
}

fn parent_element(doc: &Document, element: SceneElement) -> Option<SceneElement> {
    match element {
        SceneElement::ConstructionPlane(index) => doc.construction_planes.get(index).and_then(
            |plane| match plane.parent {
                ConstructionPlaneParent::Root => None,
                ConstructionPlaneParent::Sketch(sketch) => Some(SceneElement::Sketch(sketch)),
            },
        ),
        SceneElement::Sketch(sketch) => doc
            .sketch_face(sketch)
            .map(face_element),
        SceneElement::Rect(index) => doc
            .rects
            .get(index)
            .map(|rect| SceneElement::Sketch(rect.sketch)),
        SceneElement::Line(index) => doc
            .lines
            .get(index)
            .map(|line| SceneElement::Sketch(line.sketch)),
        SceneElement::RectEdge(index, _) => Some(SceneElement::Rect(index)),
        SceneElement::Constraint(index) => doc
            .constraints
            .get(index)
            .map(|c| SceneElement::Sketch(c.sketch)),
    }
}

fn collect_ancestors(doc: &Document, element: SceneElement, out: &mut HashSet<SceneElement>) {
    let mut current = element;
    while let Some(parent) = parent_element(doc, current) {
        out.insert(parent);
        current = parent;
    }
}

fn collect_descendants(doc: &Document, element: SceneElement, out: &mut HashSet<SceneElement>) {
    match element {
        SceneElement::ConstructionPlane(index) => {
            let face = FaceId::ConstructionPlane(index);
            for sketch in doc.sketches_on_face(face) {
                out.insert(SceneElement::Sketch(sketch));
                collect_descendants(doc, SceneElement::Sketch(sketch), out);
            }
        }
        SceneElement::Sketch(sketch) => {
            for (ri, rect) in doc.rects.iter().enumerate() {
                if rect.sketch == sketch {
                    out.insert(SceneElement::Rect(ri));
                    collect_descendants(doc, SceneElement::Rect(ri), out);
                }
            }
            for (li, line) in doc.lines.iter().enumerate() {
                if line.sketch == sketch {
                    out.insert(SceneElement::Line(li));
                }
            }
            for (ci, constraint) in doc.constraints.iter().enumerate() {
                if constraint.sketch == sketch {
                    out.insert(SceneElement::Constraint(ci));
                }
            }
            for (pi, plane) in doc.construction_planes.iter().enumerate() {
                if matches!(plane.parent, ConstructionPlaneParent::Sketch(s) if s == sketch) {
                    out.insert(SceneElement::ConstructionPlane(pi));
                    collect_descendants(doc, SceneElement::ConstructionPlane(pi), out);
                }
            }
        }
        SceneElement::Rect(index) => {
            for sketch in doc.sketches_on_face(FaceId::Rect(index)) {
                out.insert(SceneElement::Sketch(sketch));
                collect_descendants(doc, SceneElement::Sketch(sketch), out);
            }
        }
        SceneElement::Line(_)
        | SceneElement::RectEdge(_, _)
        | SceneElement::Constraint(_) => {}
    }
}

fn selection_anchor(element: SceneElement) -> SceneElement {
    match element {
        SceneElement::RectEdge(index, _) => SceneElement::Rect(index),
        other => other,
    }
}

/// Selected elements plus their ancestors and descendants.
pub fn selection_context_elements(
    doc: &Document,
    selection: &SceneSelection,
) -> HashSet<SceneElement> {
    let mut context = HashSet::new();
    for element in selection.iter() {
        let anchor = selection_anchor(element);
        context.insert(anchor);
        collect_ancestors(doc, anchor, &mut context);
        collect_descendants(doc, anchor, &mut context);
    }
    context
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowStyle {
    Selected,
    InContext,
    Normal,
    Faint,
}

fn row_is_selected(element: SceneElement, selection: &SceneSelection) -> bool {
    selection.is_selected(element)
        || matches!(element, SceneElement::Rect(index) if selection.has_rect_edge_selected(index))
}

/// Only dim the list when a selected element is actually shown in it.
fn selection_styles_visible_list(elements: &[HierarchyNode], selection: &SceneSelection) -> bool {
    if selection.is_empty() {
        return false;
    }
    let list_elements: HashSet<SceneElement> = elements
        .iter()
        .map(|node| scene_element_for_node(*node))
        .collect();
    selection.iter().any(|element| {
        let anchor = selection_anchor(element);
        list_elements.contains(&anchor)
    })
}

fn row_style(
    element: SceneElement,
    selection: &SceneSelection,
    context: &HashSet<SceneElement>,
    style_selection: bool,
) -> RowStyle {
    if !style_selection {
        return RowStyle::Normal;
    }
    if row_is_selected(element, selection) {
        RowStyle::Selected
    } else if context.contains(&element) {
        RowStyle::InContext
    } else {
        RowStyle::Faint
    }
}

fn styled_label(label: &str, style: RowStyle) -> RichText {
    match style {
        RowStyle::Selected | RowStyle::InContext | RowStyle::Normal => RichText::new(label),
        RowStyle::Faint => RichText::new(label).color(Color32::from_gray(120)),
    }
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
        for (ci, constraint) in doc.constraints.iter().enumerate() {
            if constraint.sketch == sketch {
                children.push(HierarchyEntry {
                    node: HierarchyNode::Constraint(ci),
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
        HierarchyNode::Constraint(i) => constraint_label(doc, i),
    }
}

pub fn scene_element_for_node(node: HierarchyNode) -> SceneElement {
    SceneElement::from(node)
}

/// Draw the elements list in a side panel.
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

    let context = selection_context_elements(doc, selection);
    let elements = build_element_list(doc, sketch_session);
    let style_selection = selection_styles_visible_list(&elements, selection);

    egui::ScrollArea::vertical().show(ui, |ui| {
        for node in elements {
            show_row(
                ui,
                doc,
                node,
                visibility,
                selection,
                &context,
                style_selection,
                on_edit_sketch,
                on_edit_plane,
                on_toggle_visibility,
                on_click_element,
            );
        }
    });
}

fn show_row(
    ui: &mut egui::Ui,
    doc: &Document,
    node: HierarchyNode,
    visibility: &mut ElementVisibility,
    selection: &SceneSelection,
    context: &HashSet<SceneElement>,
    style_selection: bool,
    on_edit_sketch: &mut impl FnMut(SketchId),
    on_edit_plane: &mut impl FnMut(usize),
    on_toggle_visibility: &mut impl FnMut(SceneElement, bool),
    on_click_element: &mut impl FnMut(SceneElement, bool),
) {
    let element = scene_element_for_node(node);
    let visible = visibility.effective_visible(doc, element);
    let style = row_style(element, selection, context, style_selection);

    ui.horizontal(|ui| {
        let eye = if visible { "👁" } else { "◌" };
        if ui
            .button(eye)
            .on_hover_text(if visible { "Hide" } else { "Show" })
            .clicked()
        {
            let next = visibility.toggle(element);
            on_toggle_visibility(element, next);
        }

        let label = node_label(doc, node);
        let response = ui.selectable_label(
            style == RowStyle::Selected,
            styled_label(&label, style),
        );
        match node {
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
            HierarchyNode::Rect(_) | HierarchyNode::Line(_) | HierarchyNode::Constraint(_) => {
                if response.clicked() {
                    let additive = ui.input(|i| i.modifiers.command);
                    on_click_element(element, additive);
                }
            }
        }
    });
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
    fn main_view_lists_planes_and_sketches_only() {
        let doc = doc_with_plane_sketches();
        let list = build_element_list(&doc, None);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], HierarchyNode::ConstructionPlane(0));
        assert_eq!(list[1], HierarchyNode::Sketch(0));
        assert_eq!(list[2], HierarchyNode::Sketch(1));
    }

    #[test]
    fn sketch_view_lists_geometry_of_active_sketch() {
        let doc = doc_with_plane_sketches();
        let list = build_element_list(&doc, Some(SketchSession { sketch: 0 }));
        assert_eq!(
            list,
            vec![
                HierarchyNode::ConstructionPlane(0),
                HierarchyNode::Sketch(0),
                HierarchyNode::Sketch(1),
                HierarchyNode::Rect(0),
            ]
        );

        let list = build_element_list(&doc, Some(SketchSession { sketch: 1 }));
        assert_eq!(
            list,
            vec![
                HierarchyNode::ConstructionPlane(0),
                HierarchyNode::Sketch(0),
                HierarchyNode::Sketch(1),
                HierarchyNode::Line(0),
            ]
        );
    }

    #[test]
    fn sketch_view_lists_constraints_for_active_sketch() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 5.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        crate::constraints::add_distance_constraint(
            &mut doc,
            sketch,
            crate::model::DistanceTarget::LineLength(0),
            "5mm".to_string(),
        )
        .unwrap();
        let list = build_element_list(&doc, Some(SketchSession { sketch }));
        assert!(list.contains(&HierarchyNode::Constraint(0)));
        assert!(!build_element_list(&doc, None).contains(&HierarchyNode::Constraint(0)));
    }

    #[test]
    fn nested_sketches_on_rect_face_follow_parent_order() {
        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(s0, 0.0, 0.0, 20.0, 20.0));
        let s1 = doc.add_sketch(FaceId::Rect(0));

        let list = build_element_list(&doc, None);
        assert_eq!(
            list,
            vec![
                HierarchyNode::ConstructionPlane(0),
                HierarchyNode::Sketch(0),
                HierarchyNode::Rect(0),
                HierarchyNode::Sketch(1),
            ]
        );
        let _ = s1;
    }

    #[test]
    fn plane_from_sketch_geometry_lists_under_sketch() {
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
        doc.shape_order.push(ShapeKind::ConstructionPlane);

        let list = build_element_list(&doc, None);
        assert_eq!(
            list,
            vec![
                HierarchyNode::ConstructionPlane(0),
                HierarchyNode::Sketch(0),
                HierarchyNode::ConstructionPlane(1),
            ]
        );
    }

    #[test]
    fn creation_order_can_place_siblings_between_parent_and_child() {
        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(s0, 0.0, 0.0, 10.0, 10.0));
        doc.shape_order.push(ShapeKind::Rect);
        let s1 = doc.add_sketch(FaceId::ConstructionPlane(0));
        let _ = s1;

        let list = build_element_list(&doc, Some(SketchSession { sketch: 0 }));
        let plane = list.iter().position(|n| *n == HierarchyNode::ConstructionPlane(0)).unwrap();
        let sketch0 = list.iter().position(|n| *n == HierarchyNode::Sketch(0)).unwrap();
        let sketch1 = list.iter().position(|n| *n == HierarchyNode::Sketch(1)).unwrap();
        let rect0 = list.iter().position(|n| *n == HierarchyNode::Rect(0)).unwrap();
        assert!(plane < sketch0);
        assert!(sketch0 < rect0);
        assert!(sketch0 < sketch1);
        assert!(sketch1 < rect0);
    }

    #[test]
    fn selection_context_includes_selected_ancestors_and_descendants() {
        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(s0, 0.0, 0.0, 10.0, 10.0));
        let _s1 = doc.add_sketch(FaceId::Rect(0));
        doc.add_sketch(FaceId::ConstructionPlane(0));

        let mut selection = SceneSelection::default();
        crate::selection::click_scene_selection(
            &mut selection,
            SceneElement::Rect(0),
            false,
        );
        let context = selection_context_elements(&doc, &selection);
        assert!(context.contains(&SceneElement::Rect(0)));
        assert!(context.contains(&SceneElement::Sketch(0)));
        assert!(context.contains(&SceneElement::ConstructionPlane(0)));
        assert!(context.contains(&SceneElement::Sketch(1)));
        assert!(!context.contains(&SceneElement::Sketch(2)));
    }

    #[test]
    fn row_style_faints_unrelated_rows_when_selection_active() {
        let mut doc = Document::default();
        let _s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.add_sketch(FaceId::ConstructionPlane(0));
        let mut selection = SceneSelection::default();
        crate::selection::click_scene_selection(
            &mut selection,
            SceneElement::Sketch(0),
            false,
        );
        let context = selection_context_elements(&doc, &selection);
        let list = build_element_list(&doc, None);
        let style_selection = selection_styles_visible_list(&list, &selection);
        assert!(style_selection);
        assert_eq!(
            row_style(
                SceneElement::Sketch(0),
                &selection,
                &context,
                style_selection
            ),
            RowStyle::Selected
        );
        assert_eq!(
            row_style(
                SceneElement::ConstructionPlane(0),
                &selection,
                &context,
                style_selection
            ),
            RowStyle::InContext
        );
        assert_eq!(
            row_style(
                SceneElement::Sketch(1),
                &selection,
                &context,
                style_selection
            ),
            RowStyle::Faint
        );
    }

    #[test]
    fn hidden_selection_does_not_faint_visible_rows() {
        let doc = doc_with_plane_sketches();
        let mut selection = SceneSelection::default();
        crate::selection::click_scene_selection(
            &mut selection,
            SceneElement::Rect(0),
            false,
        );
        let list = build_element_list(&doc, None);
        let context = selection_context_elements(&doc, &selection);
        assert!(!selection_styles_visible_list(&list, &selection));
        assert_eq!(
            row_style(
                SceneElement::ConstructionPlane(0),
                &selection,
                &context,
                false
            ),
            RowStyle::Normal
        );
    }

    #[test]
    fn new_child_plane_is_normal_when_selection_is_off_list() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.construction_planes.push(plane_from_definition(
            &default_xy_plane().definition,
            ConstructionPlaneParent::Sketch(sketch),
        ));
        doc.shape_order.push(ShapeKind::ConstructionPlane);
        let mut selection = SceneSelection::default();
        crate::selection::click_scene_selection(
            &mut selection,
            SceneElement::Rect(99),
            false,
        );
        let list = build_element_list(&doc, None);
        let context = selection_context_elements(&doc, &selection);
        assert!(!selection_styles_visible_list(&list, &selection));
        assert_eq!(
            row_style(
                SceneElement::ConstructionPlane(1),
                &selection,
                &context,
                false
            ),
            RowStyle::Normal
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
    fn pane_title_is_elements() {
        assert_eq!(PANE_TITLE, "Elements");
    }
}