//! Elements pane: construction planes, sketches, and sketch geometry.

/// Side-panel title shown in the UI.
pub const PANE_TITLE: &str = "Elements";

use crate::actions::SketchSession;
use crate::icons::{icon_button, icon_for_visibility};
use crate::document_health::{DocumentHealth, HealthStatus};
use crate::document_lifecycle::{element_alive, sketch_alive};
use crate::model::{
    ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, ConstructionPlaneParent,
    DistanceTarget, Document, FaceId, RectEdge, ShapeKind, SketchId,
};
use crate::names;
use crate::selection::{additive_click_modifiers, SceneSelection};
use eframe::egui::{self, Color32, RichText};
use std::collections::{HashMap, HashSet};

/// A node in the scene hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HierarchyNode {
    ConstructionPlane(usize),
    Sketch(SketchId),
    Rect(usize),
    Line(usize),
    Circle(usize),
    Constraint(usize),
}

/// Identifies an element whose visibility can be toggled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SceneElement {
    ConstructionPlane(usize),
    Sketch(SketchId),
    Rect(usize),
    Line(usize),
    Circle(usize),
    RectEdge(usize, RectEdge),
    Point(ConstraintPoint),
    Constraint(usize),
}

impl From<HierarchyNode> for SceneElement {
    fn from(node: HierarchyNode) -> Self {
        match node {
            HierarchyNode::ConstructionPlane(i) => SceneElement::ConstructionPlane(i),
            HierarchyNode::Sketch(i) => SceneElement::Sketch(i),
            HierarchyNode::Rect(i) => SceneElement::Rect(i),
            HierarchyNode::Line(i) => SceneElement::Line(i),
            HierarchyNode::Circle(i) => SceneElement::Circle(i),
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
            SceneElement::Circle(index) => doc.circles.get(index).is_some_and(|circle| {
                self.effective_visible(doc, SceneElement::Sketch(circle.sketch))
            }),
            SceneElement::RectEdge(index, _) => doc.rects.get(index).is_some_and(|rect| {
                self.effective_visible(doc, SceneElement::Sketch(rect.sketch))
            }),
            SceneElement::Point(point) => point_effective_visible(self, doc, point),
            SceneElement::Constraint(index) => doc.constraints.get(index).is_some_and(|c| {
                self.effective_visible(doc, SceneElement::Sketch(c.sketch))
            }),
        }
    }
}

fn point_effective_visible(
    visibility: &ElementVisibility,
    doc: &Document,
    point: ConstraintPoint,
) -> bool {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => doc.lines.get(line).is_some_and(|entity| {
            visibility.effective_visible(doc, SceneElement::Sketch(entity.sketch))
        }),
        ConstraintPoint::RectCorner { rect, .. } => doc.rects.get(rect).is_some_and(|entity| {
            visibility.effective_visible(doc, SceneElement::Sketch(entity.sketch))
        }),
        ConstraintPoint::CircleCenter(circle) => doc.circles.get(circle).is_some_and(|entity| {
            visibility.effective_visible(doc, SceneElement::Sketch(entity.sketch))
        }),
    }
}

fn face_element(face: FaceId) -> SceneElement {
    match face {
        FaceId::ConstructionPlane(i) => SceneElement::ConstructionPlane(i),
        FaceId::Rect(i) => SceneElement::Rect(i),
        FaceId::Circle(i) => SceneElement::Circle(i),
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
    circles: HashMap<usize, usize>,
    constraints: HashMap<usize, usize>,
    planes: HashMap<usize, usize>,
}

fn build_creation_ranks(doc: &Document) -> CreationRanks {
    let mut ranks = CreationRanks::default();
    ranks.planes.insert(0, 0);
    let mut sketch_n = 0usize;
    let mut rect_n = 0usize;
    let mut line_n = 0usize;
    let mut circle_n = 0usize;
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
            ShapeKind::Circle => {
                ranks.circles.insert(circle_n, rank);
                circle_n += 1;
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
        HierarchyNode::Circle(i) => *ranks.circles.get(&i).unwrap_or(&i),
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
        if plane.deleted || !matches!(plane.parent, ConstructionPlaneParent::Root) {
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
        SceneElement::Circle(index) => doc
            .circles
            .get(index)
            .map(|circle| SceneElement::Sketch(circle.sketch)),
        SceneElement::RectEdge(index, _) => Some(SceneElement::Rect(index)),
        SceneElement::Constraint(index) => doc
            .constraints
            .get(index)
            .map(|c| SceneElement::Sketch(c.sketch)),
        SceneElement::Point(point) => point_parent_element(doc, point),
    }
}

fn point_parent_element(doc: &Document, point: ConstraintPoint) -> Option<SceneElement> {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => doc
            .lines
            .get(line)
            .map(|_| SceneElement::Line(line)),
        ConstraintPoint::RectCorner { rect, .. } => Some(SceneElement::Rect(rect)),
        ConstraintPoint::CircleCenter(circle) => Some(SceneElement::Circle(circle)),
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
            for (ci, circle) in doc.circles.iter().enumerate() {
                if circle.sketch == sketch {
                    out.insert(SceneElement::Circle(ci));
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
        SceneElement::Circle(index) => {
            for sketch in doc.sketches_on_face(FaceId::Circle(index)) {
                out.insert(SceneElement::Sketch(sketch));
                collect_descendants(doc, SceneElement::Sketch(sketch), out);
            }
        }
        SceneElement::Line(_)
        | SceneElement::RectEdge(_, _)
        | SceneElement::Constraint(_)
        | SceneElement::Point(_) => {}
    }
}

fn selection_anchor(element: SceneElement) -> SceneElement {
    match element {
        SceneElement::RectEdge(index, _) => SceneElement::Rect(index),
        other => other,
    }
}

fn distance_target_touches_element(target: DistanceTarget, element: SceneElement) -> bool {
    match (target, element) {
        (DistanceTarget::LineLength(i), SceneElement::Line(j)) => i == j,
        (DistanceTarget::RectWidth(r) | DistanceTarget::RectHeight(r), SceneElement::Rect(i)) => {
            r == i
        }
        (DistanceTarget::RectWidth(r), SceneElement::RectEdge(i, RectEdge::Bottom | RectEdge::Top)) => {
            r == i
        }
        (
            DistanceTarget::RectHeight(r),
            SceneElement::RectEdge(i, RectEdge::Left | RectEdge::Right),
        ) => r == i,
        (DistanceTarget::CircleDiameter(c), SceneElement::Circle(i)) => c == i,
        (DistanceTarget::LineLineDistance { line_a, line_b }, element) => {
            constraint_line_touches_element(line_a, element)
                || constraint_line_touches_element(line_b, element)
        }
        (DistanceTarget::PointPointDistance { a, b }, element) => {
            constraint_point_touches_element(a, element)
                || constraint_point_touches_element(b, element)
        }
        (DistanceTarget::PointLineDistance { point, line }, element) => {
            constraint_point_touches_element(point, element)
                || constraint_line_touches_element(line, element)
        }
        _ => false,
    }
}

fn constraint_line_touches_element(line: ConstraintLine, element: SceneElement) -> bool {
    match (line, element) {
        (ConstraintLine::Line(i), SceneElement::Line(j)) => i == j,
        (
            ConstraintLine::Line(i),
            SceneElement::Point(ConstraintPoint::LineEndpoint { line, .. }),
        ) => i == line,
        (ConstraintLine::RectEdge { rect, edge }, SceneElement::RectEdge(r, e)) => {
            rect == r && edge == e
        }
        (ConstraintLine::RectEdge { rect, .. }, SceneElement::Rect(r)) => rect == r,
        (
            ConstraintLine::RectEdge { rect, .. },
            SceneElement::Point(ConstraintPoint::RectCorner { rect: r, .. }),
        ) => rect == r,
        _ => false,
    }
}

fn constraint_point_touches_element(point: ConstraintPoint, element: SceneElement) -> bool {
    match (point, element) {
        (p, SceneElement::Point(q)) => p == q,
        (ConstraintPoint::LineEndpoint { line, .. }, SceneElement::Line(i)) => line == i,
        (ConstraintPoint::RectCorner { rect, .. }, SceneElement::Rect(r)) => rect == r,
        (ConstraintPoint::RectCorner { rect, .. }, SceneElement::RectEdge(r, _)) => rect == r,
        (ConstraintPoint::CircleCenter(c), SceneElement::Circle(i)) => c == i,
        _ => false,
    }
}

fn constraint_entity_touches_element(entity: ConstraintEntity, element: SceneElement) -> bool {
    match entity {
        ConstraintEntity::Point(point) => constraint_point_touches_element(point, element),
        ConstraintEntity::Line(line) => constraint_line_touches_element(line, element),
    }
}

fn constraint_kind_touches_element(kind: ConstraintKind, element: SceneElement) -> bool {
    match kind {
        ConstraintKind::Distance { target } => distance_target_touches_element(target, element),
        ConstraintKind::Parallel { line_a, line_b }
        | ConstraintKind::Perpendicular { line_a, line_b } => {
            constraint_line_touches_element(line_a, element)
                || constraint_line_touches_element(line_b, element)
        }
        ConstraintKind::Coincident { a, b } => {
            constraint_entity_touches_element(a, element)
                || constraint_entity_touches_element(b, element)
        }
        ConstraintKind::Midpoint { point, line } => {
            constraint_point_touches_element(point, element)
                || constraint_line_touches_element(line, element)
        }
        ConstraintKind::Horizontal { line } | ConstraintKind::Vertical { line } => {
            constraint_line_touches_element(line, element)
        }
        ConstraintKind::Angle { line_a, line_b } => {
            constraint_line_touches_element(line_a, element)
                || constraint_line_touches_element(line_b, element)
        }
    }
}

fn constraints_for_element(doc: &Document, element: SceneElement) -> Vec<usize> {
    doc.constraints
        .iter()
        .enumerate()
        .filter_map(|(index, constraint)| {
            constraint_kind_touches_element(constraint.kind, element).then_some(index)
        })
        .collect()
}

/// Constraint indices that apply to the current selection (for Elements pane highlighting).
pub fn selection_related_constraints(
    doc: &Document,
    selection: &SceneSelection,
) -> HashSet<usize> {
    let mut related = HashSet::new();
    for element in selection.iter() {
        let anchor = selection_anchor(element);
        related.extend(constraints_for_element(doc, anchor));
        if anchor != element {
            related.extend(constraints_for_element(doc, element));
        }
    }
    related
}

/// Selected elements plus their ancestors, descendants, and related constraints.
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
    for index in selection_related_constraints(doc, selection) {
        context.insert(SceneElement::Constraint(index));
    }
    context
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowStyle {
    Selected,
    RelatedConstraint,
    Invalid,
    Unstable,
    InContext,
    Normal,
    Faint,
}

/// Accent for constraint rows tied to the current selection.
const RELATED_CONSTRAINT_TEXT: Color32 = Color32::from_rgb(255, 205, 88);
const INVALID_TEXT: Color32 = Color32::from_rgb(220, 80, 80);
const UNSTABLE_TEXT: Color32 = Color32::from_rgb(255, 180, 60);

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
    related_constraints: &HashSet<usize>,
    style_selection: bool,
    health: &DocumentHealth,
) -> RowStyle {
    match health.element_status(element) {
        HealthStatus::Invalid => return RowStyle::Invalid,
        HealthStatus::Unstable => return RowStyle::Unstable,
        HealthStatus::Healthy => {}
    }
    if !style_selection {
        return RowStyle::Normal;
    }
    if row_is_selected(element, selection) {
        RowStyle::Selected
    } else if matches!(element, SceneElement::Constraint(index) if related_constraints.contains(&index)) {
        RowStyle::RelatedConstraint
    } else if context.contains(&element) {
        RowStyle::InContext
    } else {
        RowStyle::Faint
    }
}

fn styled_label(label: &str, style: RowStyle) -> RichText {
    match style {
        RowStyle::Selected | RowStyle::InContext | RowStyle::Normal => RichText::new(label),
        RowStyle::RelatedConstraint => RichText::new(label).color(RELATED_CONSTRAINT_TEXT),
        RowStyle::Invalid => RichText::new(label).color(INVALID_TEXT),
        RowStyle::Unstable => RichText::new(label).color(UNSTABLE_TEXT),
        RowStyle::Faint => RichText::new(label).color(Color32::from_gray(120)),
    }
}

/// Primary double-click on a row label (fallback when [`egui::Response::double_clicked`] misses).
fn row_primary_double_clicked(response: &egui::Response, ui: &egui::Ui) -> bool {
    if response.double_clicked() {
        return true;
    }
    let pointer_double = ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary));
    if !pointer_double {
        return false;
    }
    let pos = response
        .interact_pointer_pos()
        .or_else(|| ui.input(|i| i.pointer.interact_pos()));
    pos.is_some_and(|pos| response.rect.contains(pos))
}

/// How a sketch row should react to pointer input this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SketchRowAction {
    None,
    Select { additive: bool },
    Edit,
}

pub fn sketch_row_action(double_clicked: bool, clicked: bool, additive: bool) -> SketchRowAction {
    if double_clicked {
        SketchRowAction::Edit
    } else if clicked {
        SketchRowAction::Select { additive }
    } else {
        SketchRowAction::None
    }
}

fn build_face_sketches(
    doc: &Document,
    face: FaceId,
    sketch_session: Option<SketchSession>,
) -> Vec<HierarchyEntry> {
    doc.sketches_on_face(face)
        .filter(|sketch| sketch_alive(doc, *sketch))
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
        if plane.deleted || !matches!(plane.parent, ConstructionPlaneParent::Sketch(s) if s == sketch) {
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
            if rect.deleted || rect.sketch != sketch {
                continue;
            }
            let nested = build_face_sketches(doc, FaceId::Rect(ri), sketch_session);
            children.push(HierarchyEntry {
                node: HierarchyNode::Rect(ri),
                children: nested,
            });
        }
        for (li, line) in doc.lines.iter().enumerate() {
            if line.deleted || line.sketch != sketch {
                continue;
            }
            children.push(HierarchyEntry {
                node: HierarchyNode::Line(li),
                children: vec![],
            });
        }
        for (ci, circle) in doc.circles.iter().enumerate() {
            if circle.deleted || circle.sketch != sketch {
                continue;
            }
            let nested = build_face_sketches(doc, FaceId::Circle(ci), sketch_session);
            children.push(HierarchyEntry {
                node: HierarchyNode::Circle(ci),
                children: nested,
            });
        }
        for (ci, constraint) in doc.constraints.iter().enumerate() {
            if constraint.deleted || constraint.sketch != sketch {
                continue;
            }
            children.push(HierarchyEntry {
                node: HierarchyNode::Constraint(ci),
                children: vec![],
            });
        }
    } else {
        for (ri, rect) in doc.rects.iter().enumerate() {
            if rect.deleted || rect.sketch != sketch {
                continue;
            }
            let nested = build_face_sketches(doc, FaceId::Rect(ri), sketch_session);
            if !nested.is_empty() {
                children.push(HierarchyEntry {
                    node: HierarchyNode::Rect(ri),
                    children: nested,
                });
            }
        }
        for (ci, circle) in doc.circles.iter().enumerate() {
            if circle.deleted || circle.sketch != sketch {
                continue;
            }
            let nested = build_face_sketches(doc, FaceId::Circle(ci), sketch_session);
            if !nested.is_empty() {
                children.push(HierarchyEntry {
                    node: HierarchyNode::Circle(ci),
                    children: nested,
                });
            }
        }
    }

    HierarchyEntry {
        node: HierarchyNode::Sketch(sketch),
        children,
    }
}

pub fn node_label(doc: &Document, node: HierarchyNode) -> String {
    names::node_label(doc, node)
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
    health: &DocumentHealth,
    on_edit_sketch: &mut impl FnMut(SketchId),
    on_edit_plane: &mut impl FnMut(usize),
    on_toggle_visibility: &mut impl FnMut(SceneElement, bool),
    on_click_element: &mut impl FnMut(SceneElement, bool),
) {
    ui.heading(PANE_TITLE);
    ui.separator();

    let context = selection_context_elements(doc, selection);
    let related_constraints = selection_related_constraints(doc, selection);
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
                health,
                &context,
                &related_constraints,
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
    health: &DocumentHealth,
    context: &HashSet<SceneElement>,
    related_constraints: &HashSet<usize>,
    style_selection: bool,
    on_edit_sketch: &mut impl FnMut(SketchId),
    on_edit_plane: &mut impl FnMut(usize),
    on_toggle_visibility: &mut impl FnMut(SceneElement, bool),
    on_click_element: &mut impl FnMut(SceneElement, bool),
) {
    let element = scene_element_for_node(node);
    if !element_alive(doc, element) {
        return;
    }
    let visible = visibility.effective_visible(doc, element);
    let style = row_style(
        element,
        selection,
        context,
        related_constraints,
        style_selection,
        health,
    );

    ui.horizontal(|ui| {
        if icon_button(
            ui,
            icon_for_visibility(visible),
            if visible { "Hide" } else { "Show" },
        )
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
                let additive = ui.input(|i| additive_click_modifiers(&i.modifiers));
                match sketch_row_action(
                    row_primary_double_clicked(&response, ui),
                    response.clicked(),
                    additive,
                ) {
                    SketchRowAction::Edit => on_edit_sketch(sketch),
                    SketchRowAction::Select { additive } => {
                        on_click_element(element, additive)
                    }
                    SketchRowAction::None => {}
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
                    let additive = ui.input(|i| additive_click_modifiers(&i.modifiers));
                    on_click_element(element, additive);
                }
                response.context_menu(|ui| {
                    if ui.button("Edit plane").clicked() {
                        on_edit_plane(index);
                        ui.close_menu();
                    }
                });
            }
            HierarchyNode::Rect(_)
            | HierarchyNode::Line(_)
            | HierarchyNode::Circle(_)
            | HierarchyNode::Constraint(_) => {
                if response.clicked() {
                    let additive = ui.input(|i| additive_click_modifiers(&i.modifiers));
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
    fn sketch_row_double_click_opens_for_edit_not_select() {
        assert_eq!(
            sketch_row_action(true, true, false),
            SketchRowAction::Edit
        );
        assert_eq!(
            sketch_row_action(false, true, false),
            SketchRowAction::Select { additive: false }
        );
        assert_eq!(sketch_row_action(false, false, false), SketchRowAction::None);
    }

    #[test]
    fn open_sketch_from_elements_pane_action() {
        use crate::actions::{Action, AppState, SketchSession};

        let mut state = AppState::default();
        let sketch = state.doc.add_sketch(FaceId::ConstructionPlane(0));
        assert!(state.sketch_session.is_none());
        assert_eq!(
            state.apply(Action::OpenSketch {
                sketch,
                viewport: None,
            }),
            crate::actions::ActionResult::Ok
        );
        assert_eq!(state.sketch_session, Some(SketchSession { sketch }));
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
    fn nested_sketches_on_circle_face_follow_parent_order() {
        let mut doc = Document::default();
        let s0 = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.circles
            .push(crate::model::Circle::from_local_center_radius(s0, 0.0, 0.0, 20.0, 0.0));
        let s1 = doc.add_sketch(FaceId::Circle(0));

        let list = build_element_list(&doc, None);
        assert_eq!(
            list,
            vec![
                HierarchyNode::ConstructionPlane(0),
                HierarchyNode::Sketch(0),
                HierarchyNode::Circle(0),
                HierarchyNode::Sketch(1),
            ]
        );
        let _ = s1;
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
        let related_constraints = selection_related_constraints(&doc, &selection);
        let list = build_element_list(&doc, None);
        let style_selection = selection_styles_visible_list(&list, &selection);
        assert!(style_selection);
        let health = DocumentHealth::default();
        assert_eq!(
            row_style(
                SceneElement::Sketch(0),
                &selection,
                &context,
                &related_constraints,
                style_selection,
                &health,
            ),
            RowStyle::Selected
        );
        assert_eq!(
            row_style(
                SceneElement::ConstructionPlane(0),
                &selection,
                &context,
                &related_constraints,
                style_selection,
                &health,
            ),
            RowStyle::InContext
        );
        assert_eq!(
            row_style(
                SceneElement::Sketch(1),
                &selection,
                &context,
                &related_constraints,
                style_selection,
                &health,
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
        let related_constraints = selection_related_constraints(&doc, &selection);
        assert!(!selection_styles_visible_list(&list, &selection));
        assert_eq!(
            row_style(
                SceneElement::ConstructionPlane(0),
                &selection,
                &context,
                &related_constraints,
                false,
                &DocumentHealth::default(),
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
        let related_constraints = selection_related_constraints(&doc, &selection);
        assert!(!selection_styles_visible_list(&list, &selection));
        assert_eq!(
            row_style(
                SceneElement::ConstructionPlane(1),
                &selection,
                &context,
                &related_constraints,
                false,
                &DocumentHealth::default(),
            ),
            RowStyle::Normal
        );
    }

    #[test]
    fn selection_context_includes_constraints_for_selected_line() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 5.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        crate::constraints::add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::LineLength(0),
            "5mm".to_string(),
        )
        .unwrap();

        let mut selection = SceneSelection::default();
        crate::selection::click_scene_selection(
            &mut selection,
            SceneElement::Line(0),
            false,
        );
        let context = selection_context_elements(&doc, &selection);
        let related = selection_related_constraints(&doc, &selection);
        assert!(context.contains(&SceneElement::Constraint(0)));
        assert!(related.contains(&0));
    }

    #[test]
    fn row_style_highlights_related_constraint_when_line_selected() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 5.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        crate::constraints::add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::LineLength(0),
            "5mm".to_string(),
        )
        .unwrap();

        let mut selection = SceneSelection::default();
        crate::selection::click_scene_selection(
            &mut selection,
            SceneElement::Line(0),
            false,
        );
        let context = selection_context_elements(&doc, &selection);
        let related = selection_related_constraints(&doc, &selection);
        let list = build_element_list(&doc, Some(SketchSession { sketch }));
        let style_selection = selection_styles_visible_list(&list, &selection);
        let health = DocumentHealth::default();
        assert_eq!(
            row_style(
                SceneElement::Constraint(0),
                &selection,
                &context,
                &related,
                style_selection,
                &health,
            ),
            RowStyle::RelatedConstraint
        );
        assert_eq!(
            row_style(
                SceneElement::Line(1),
                &selection,
                &context,
                &related,
                style_selection,
                &health,
            ),
            RowStyle::Faint
        );
    }

    #[test]
    fn row_style_prefers_invalid_and_unstable_over_selection() {
        use crate::document_lifecycle::tombstone_element;
        use crate::model::{Constraint, ConstraintKind, ConstraintLine, Line, ShapeKind};

        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        let line_a = 0;
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 5.0, 10.0, 5.0));
        doc.shape_order.push(ShapeKind::Line);
        let line_b = 1;
        doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Parallel {
                line_a: ConstraintLine::Line(line_a),
                line_b: ConstraintLine::Line(line_b),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        tombstone_element(&mut doc, SceneElement::Line(line_a));
        let health = crate::document_health::recompute_document_health(&doc);
        let mut selection = SceneSelection::default();
        crate::selection::click_scene_selection(
            &mut selection,
            SceneElement::Line(line_b),
            false,
        );
        let context = selection_context_elements(&doc, &selection);
        let related = selection_related_constraints(&doc, &selection);
        assert_eq!(
            row_style(
                SceneElement::Constraint(0),
                &selection,
                &context,
                &related,
                true,
                &health,
            ),
            RowStyle::Invalid
        );
        assert_eq!(
            row_style(
                SceneElement::Line(line_b),
                &selection,
                &context,
                &related,
                true,
                &health,
            ),
            RowStyle::Unstable
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