//! Elements pane: construction planes, sketches, and sketch geometry.

/// Side-panel title shown in the UI.
pub const PANE_TITLE: &str = "Elements";

use crate::actions::SketchSession;
use crate::icons::{
    icon_button, icon_for_constraint_kind, icon_for_visibility, selectable_icon_button,
    sized_texture, IconId,
};
use crate::document_health::{DocumentHealth, HealthStatus};
use crate::document_lifecycle::{element_alive, sketch_alive};
use crate::model::{
    ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, ConstructionPlaneParent,
    DistanceTarget, Document, FaceId, ShapeKind, SketchId,
};
use crate::names;
use crate::selection::{additive_click_modifiers, SceneSelection};
use eframe::egui::{self, Color32, RichText};
use std::collections::{HashMap, HashSet};

/// A node in the scene hierarchy.
///
/// The derived `Ord` (variant order, then index) gives a stable, creation-ordered
/// tiebreak when two nodes share a creation rank, so sibling ordering is deterministic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HierarchyNode {
    /// Synthetic singleton root shown at the top of the Elements pane; every other
    /// top-level node (root construction planes, orphaned extrusions, orphaned bodies)
    /// nests under it. It carries no index — there is exactly one per document — and has
    /// no corresponding [`SceneElement`]: it isn't individually selectable, hideable, or
    /// otherwise dispatched through the scene graph (see [`scene_element_for_node`]).
    Document,
    ConstructionPlane(usize),
    Sketch(SketchId),
    Line(usize),
    Circle(usize),
    Constraint(usize),
    Extrusion(usize),
    Body(usize),
}

/// Identifies an element whose visibility can be toggled.
///
/// Not `Copy` — see [`crate::model::ConstraintPoint`]'s doc comment: `Point` embeds a
/// `ConstraintPoint`, which embeds a `FaceId` for `FaceVertex` (#26/#27), and `FaceId` isn't
/// `Copy`. Callers that used to rely on implicit copies now need an explicit `.clone()`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SceneElement {
    ConstructionPlane(usize),
    Sketch(SketchId),
    Line(usize),
    Circle(usize),
    Point(ConstraintPoint),
    Constraint(usize),
    Extrusion(usize),
    Body(usize),
    /// An edge of an extrusion-backed body face's own boundary loop (#26/#27), for
    /// constraint-authoring selection — mirrors `Point` wrapping the whole `ConstraintPoint`
    /// enum; only ever constructed with `ConstraintLine::FaceEdge` (the `Line`
    /// variant already has its own dedicated `SceneElement::Line`).
    FaceEdge(ConstraintLine),
}

/// The [`SceneElement`] a hierarchy node dispatches through for selection, visibility,
/// and health lookups — `None` for [`HierarchyNode::Document`], the synthetic root, which
/// has no independent selectable/hideable identity of its own.
pub fn scene_element_for_node(node: HierarchyNode) -> Option<SceneElement> {
    Some(match node {
        HierarchyNode::Document => return None,
        HierarchyNode::ConstructionPlane(i) => SceneElement::ConstructionPlane(i),
        HierarchyNode::Sketch(i) => SceneElement::Sketch(i),
        HierarchyNode::Line(i) => SceneElement::Line(i),
        HierarchyNode::Circle(i) => SceneElement::Circle(i),
        HierarchyNode::Constraint(i) => SceneElement::Constraint(i),
        HierarchyNode::Extrusion(i) => SceneElement::Extrusion(i),
        HierarchyNode::Body(i) => SceneElement::Body(i),
    })
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
        let next = !self.is_visible(element.clone());
        self.set_visible(element, next);
        next
    }

    pub fn effective_visible(&self, doc: &Document, element: SceneElement) -> bool {
        if !self.is_visible(element.clone()) {
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
            SceneElement::Line(index) => doc.lines.get(index).is_some_and(|line| {
                self.effective_visible(doc, SceneElement::Sketch(line.sketch))
            }),
            SceneElement::Circle(index) => doc.circles.get(index).is_some_and(|circle| {
                self.effective_visible(doc, SceneElement::Sketch(circle.sketch))
            }),
            SceneElement::Point(point) => point_effective_visible(self, doc, point),
            SceneElement::Constraint(index) => doc.constraints.get(index).is_some_and(|c| {
                self.effective_visible(doc, SceneElement::Sketch(c.sketch))
            }),
            SceneElement::Extrusion(index) => self.is_visible(SceneElement::Extrusion(index)),
            SceneElement::Body(index) => {
                self.is_visible(SceneElement::Body(index))
                    && doc.bodies.get(index).is_some_and(|body| {
                        body.source.extrusion_indices().iter().any(|&ei| {
                            self.effective_visible(doc, SceneElement::Extrusion(ei))
                        })
                    })
            }
            // A face's own edge tracks the extrusion that produced its face, same as
            // `FaceVertex` in `point_effective_visible` below.
            SceneElement::FaceEdge(line) => {
                let extrusion = match &line {
                    ConstraintLine::FaceEdge { face, .. } => face.extrusion_index(),
                    ConstraintLine::Line(_) => None,
                };
                self.effective_visible(
                    doc,
                    SceneElement::Extrusion(extrusion.unwrap_or(usize::MAX)),
                )
            }
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
        ConstraintPoint::CircleCenter(circle) => doc.circles.get(circle).is_some_and(|entity| {
            visibility.effective_visible(doc, SceneElement::Sketch(entity.sketch))
        }),
        // A face's own vertex tracks the extrusion that produced its face — same dependency
        // `face_element` gives a sketch placed on a body cap/side wall.
        ConstraintPoint::FaceVertex { face, .. } => visibility.effective_visible(
            doc,
            face.extrusion_index()
                .map(SceneElement::Extrusion)
                .unwrap_or(SceneElement::Extrusion(usize::MAX)),
        ),
    }
}

fn face_element(face: FaceId) -> SceneElement {
    match face {
        FaceId::ConstructionPlane(i) => SceneElement::ConstructionPlane(i),
        FaceId::Circle(i) => SceneElement::Circle(i),
        // A polygon face is just a closed loop of existing lines (#66); its visibility
        // tracks its first constituent line.
        FaceId::Polygon(lines) => SceneElement::Line(lines[0]),
        // A sketch on a body cap or side wall depends on the extrusion that produced it.
        FaceId::ExtrudeCap { extrusion, .. } | FaceId::ExtrudeSide { extrusion, .. } => {
            SceneElement::Extrusion(extrusion)
        }
    }
}

/// A hierarchy entry with optional children (used to derive parent links).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HierarchyEntry {
    pub node: HierarchyNode,
    pub children: Vec<HierarchyEntry>,
}

/// Which layout the Elements pane renders its nodes in (#issue 34). This is an ephemeral UI
/// preference, not document data — it's stored on `App` (see `selected_bezier_handle` in
/// main.rs for the same convention) and threaded into [`show_pane`], never persisted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HierarchyViewMode {
    /// Flat, topologically-sorted list (the pre-existing default view).
    #[default]
    List,
    /// The real nested tree, each level indented farther than its parent.
    Tree,
    /// A 2D node-link diagram: column = depth, row = position within that column.
    Graph,
}

/// One node's position in the graph-node view's deterministic column/row layout — pure data,
/// no `egui` types, so it's directly unit-testable. Column equals tree depth; row is the
/// node's sequential position within that column in tree-walk (pre-order, depth-first) order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphNodePosition {
    pub node: HierarchyNode,
    pub parent: Option<HierarchyNode>,
    pub depth: usize,
    pub column: usize,
    pub row: usize,
}

/// Compute the graph-node view's layout: depth-first walk of `tree`, assigning each node a
/// column (its depth) and a row (its sequential order within that column). Deterministic and
/// non-force-directed, per #34 — the whole graph is meant to fit horizontally by construction
/// (column count is bounded by tree depth), with height handled by vertical scrolling.
pub fn graph_node_positions(tree: &[HierarchyEntry]) -> Vec<GraphNodePosition> {
    fn walk(
        entry: &HierarchyEntry,
        depth: usize,
        parent: Option<HierarchyNode>,
        next_row_in_column: &mut HashMap<usize, usize>,
        out: &mut Vec<GraphNodePosition>,
    ) {
        let row = next_row_in_column.entry(depth).or_insert(0);
        let this_row = *row;
        *row += 1;
        out.push(GraphNodePosition {
            node: entry.node,
            parent,
            depth,
            column: depth,
            row: this_row,
        });
        for child in &entry.children {
            walk(child, depth + 1, Some(entry.node), next_row_in_column, out);
        }
    }

    let mut next_row_in_column = HashMap::new();
    let mut positions = Vec::new();
    for entry in tree {
        walk(entry, 0, None, &mut next_row_in_column, &mut positions);
    }
    positions
}

/// Find `node`'s entry anywhere in `tree` (not just at the root — e.g. a sketch nests under
/// its construction plane).
fn find_hierarchy_entry(tree: &[HierarchyEntry], node: HierarchyNode) -> Option<&HierarchyEntry> {
    for entry in tree {
        if entry.node == node {
            return Some(entry);
        }
        if let Some(found) = find_hierarchy_entry(&entry.children, node) {
            return Some(found);
        }
    }
    None
}

fn collect_entry_descendants(entry: &HierarchyEntry, out: &mut HashSet<HierarchyNode>) {
    for child in &entry.children {
        out.insert(child.node);
        collect_entry_descendants(child, out);
    }
}

/// The graph-node view's highlight set for a selected node: the node itself, all its
/// ancestors (walked via the parent links from [`graph_node_positions`]), and all its
/// descendants (walked via `tree`'s own nested `children`, no `SceneElement` lookups needed —
/// the tree structure already gives parent/child relationships directly).
pub fn graph_related_nodes(tree: &[HierarchyEntry], selected: HierarchyNode) -> HashSet<HierarchyNode> {
    let positions = graph_node_positions(tree);
    let parent_of: HashMap<HierarchyNode, HierarchyNode> = positions
        .iter()
        .filter_map(|p| p.parent.map(|parent| (p.node, parent)))
        .collect();

    let mut related = HashSet::new();
    related.insert(selected);

    let mut current = selected;
    while let Some(&parent) = parent_of.get(&current) {
        related.insert(parent);
        current = parent;
    }

    if let Some(entry) = find_hierarchy_entry(tree, selected) {
        collect_entry_descendants(entry, &mut related);
    }

    related
}

/// Persistent physics state for the Graph view's force-directed layout (#94). Held on `App`
/// (never persisted to disk — a purely ephemeral view state, like [`HierarchyViewMode`]) and
/// threaded into [`show_graph_view`], so node positions/velocities carry across frames and the
/// simulation can animate ("bounce around") until it settles. Coordinates are layout-local:
/// x is contained to the pane width, y flows top-to-bottom by tree depth.
#[derive(Default)]
pub struct GraphLayout {
    nodes: HashMap<HierarchyNode, GraphNodeState>,
}

/// One node's live physics state in [`GraphLayout`]: current position and velocity.
#[derive(Clone, Copy, Debug)]
struct GraphNodeState {
    pos: egui::Vec2,
    vel: egui::Vec2,
}

/// Vertical spacing between successive tree depths — the "somewhat vertical" target the
/// layering force pulls each node toward (parents above children, flow top-to-bottom, #94).
const LAYER_HEIGHT: f32 = 64.0;
/// Horizontal inset kept clear at each side of the pane; x is soft-restored and hard-clamped
/// into `[MARGIN, width - MARGIN]` so the graph never exceeds the pane width (#34).
const GRAPH_MARGIN: f32 = 18.0;

/// Deterministic horizontal seed for a freshly-inserted node, derived purely from the node's
/// identity (no `rand`, so layout is reproducible across runs and in tests). Spreads new nodes
/// across the pane width so the simulation starts un-coincident.
fn seed_x(node: HierarchyNode, width: f32) -> f32 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    node.hash(&mut hasher);
    let frac = (hasher.finish() % 10_000) as f32 / 10_000.0;
    let lo = GRAPH_MARGIN;
    let hi = (width - GRAPH_MARGIN).max(GRAPH_MARGIN + 1.0);
    lo + frac * (hi - lo)
}

/// Advance the force-directed layout one integration step (semi-implicit Euler) and return the
/// total kinetic energy (Σ‖vel‖²) — a pure, `egui`-painting-free function so the physics is
/// directly unit-testable (settle, containment, vertical ordering, determinism). `edges` are
/// `(child, parent)` pairs; `depth_of` gives each node's tree depth; `width` is the pane width
/// the x coordinate is contained to.
///
/// Forces: a vertical layering spring pulling `y` toward `depth * LAYER_HEIGHT`; pairwise
/// inverse-square repulsion (min-distance/max-force capped) spreading siblings sideways;
/// parent↔child edge springs toward a rest length; a soft horizontal-containment restoring
/// force; and per-step velocity damping so it settles rather than oscillating forever.
fn step_graph_layout(
    nodes: &mut HashMap<HierarchyNode, GraphNodeState>,
    edges: &[(HierarchyNode, HierarchyNode)],
    depth_of: &HashMap<HierarchyNode, usize>,
    width: f32,
    dt: f32,
) -> f32 {
    const LAYER_STIFFNESS: f32 = 10.0;
    const REPULSION: f32 = 5000.0;
    const MIN_DIST: f32 = 6.0;
    const MAX_REPULSION_FORCE: f32 = 2000.0;
    const EDGE_SPRING_K: f32 = 7.0;
    const EDGE_REST_LENGTH: f32 = 58.0;
    const CONTAIN_STIFFNESS: f32 = 14.0;
    const DAMPING: f32 = 0.86;

    // Iterate a sorted key list (not the HashMap's arbitrary order) so force accumulation is
    // order-independent and thus bit-for-bit deterministic across runs.
    let mut keys: Vec<HierarchyNode> = nodes.keys().copied().collect();
    keys.sort();
    let index_of: HashMap<HierarchyNode, usize> =
        keys.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let mut forces = vec![egui::Vec2::ZERO; keys.len()];

    // Vertical layering spring: pull y toward the node's depth band.
    for (i, node) in keys.iter().enumerate() {
        let target_y = *depth_of.get(node).unwrap_or(&0) as f32 * LAYER_HEIGHT;
        let y = nodes[node].pos.y;
        forces[i].y += LAYER_STIFFNESS * (target_y - y);
    }

    // Pairwise repulsion (inverse-square, min-distance and max-force capped).
    for a in 0..keys.len() {
        for b in (a + 1)..keys.len() {
            let pa = nodes[&keys[a]].pos;
            let pb = nodes[&keys[b]].pos;
            let mut delta = pa - pb;
            let mut dist = delta.length();
            if dist < MIN_DIST {
                // Coincident (or nearly): shove apart along a deterministic axis to avoid a
                // divide-by-zero / NaN blowup.
                if dist < 1e-4 {
                    delta = egui::vec2(1.0, 0.0);
                    dist = 1.0;
                }
                dist = dist.max(MIN_DIST);
            }
            let dir = delta / dist;
            let mag = (REPULSION / (dist * dist)).min(MAX_REPULSION_FORCE);
            let f = dir * mag;
            forces[a] += f;
            forces[b] -= f;
        }
    }

    // Edge springs (parent↔child attraction toward a rest length).
    for (child, parent) in edges {
        let (Some(&ci), Some(&pi)) = (index_of.get(child), index_of.get(parent)) else {
            continue;
        };
        let delta = nodes[child].pos - nodes[parent].pos;
        let dist = delta.length().max(MIN_DIST);
        let dir = delta / dist;
        let f = dir * (-EDGE_SPRING_K * (dist - EDGE_REST_LENGTH));
        forces[ci] += f;
        forces[pi] -= f;
    }

    // Horizontal soft-containment restoring force.
    let lo = GRAPH_MARGIN;
    let hi = (width - GRAPH_MARGIN).max(lo);
    for (i, node) in keys.iter().enumerate() {
        let x = nodes[node].pos.x;
        if x < lo {
            forces[i].x += CONTAIN_STIFFNESS * (lo - x);
        } else if x > hi {
            forces[i].x += CONTAIN_STIFFNESS * (hi - x);
        }
    }

    // Integrate: vel += force*dt; vel *= damping; pos += vel*dt; then hard-clamp x.
    let mut kinetic = 0.0;
    for (i, node) in keys.iter().enumerate() {
        let state = nodes.get_mut(node).expect("key came from this map");
        let mut force = forces[i];
        if !force.x.is_finite() {
            force.x = 0.0;
        }
        if !force.y.is_finite() {
            force.y = 0.0;
        }
        state.vel += force * dt;
        state.vel *= DAMPING;
        state.pos += state.vel * dt;
        state.pos.x = state.pos.x.clamp(lo, hi);
        if !state.pos.x.is_finite() {
            state.pos.x = lo;
        }
        if !state.pos.y.is_finite() {
            state.pos.y = 0.0;
        }
        kinetic += state.vel.length_sq();
    }
    kinetic
}

impl GraphLayout {
    /// Sync the live node set to `positions` (seed newly-appeared nodes deterministically,
    /// drop departed ones), then advance the simulation `substeps` times, returning the final
    /// kinetic energy for settle detection.
    fn sync_and_step(
        &mut self,
        positions: &[GraphNodePosition],
        width: f32,
        substeps: u32,
        dt: f32,
    ) -> f32 {
        let present: HashSet<HierarchyNode> = positions.iter().map(|p| p.node).collect();
        self.nodes.retain(|node, _| present.contains(node));
        let depth_of: HashMap<HierarchyNode, usize> =
            positions.iter().map(|p| (p.node, p.depth)).collect();
        for p in positions {
            self.nodes.entry(p.node).or_insert_with(|| GraphNodeState {
                pos: egui::vec2(seed_x(p.node, width), p.depth as f32 * LAYER_HEIGHT),
                vel: egui::Vec2::ZERO,
            });
        }
        let edges: Vec<(HierarchyNode, HierarchyNode)> = positions
            .iter()
            .filter_map(|p| p.parent.map(|parent| (p.node, parent)))
            .collect();
        let mut kinetic = 0.0;
        for _ in 0..substeps.max(1) {
            kinetic = step_graph_layout(&mut self.nodes, &edges, &depth_of, width, dt);
        }
        kinetic
    }

    fn pos_of(&self, node: HierarchyNode) -> Option<egui::Vec2> {
        self.nodes.get(&node).map(|s| s.pos)
    }
}

#[derive(Clone, Debug, Default)]
struct CreationRanks {
    sketches: HashMap<SketchId, usize>,
    lines: HashMap<usize, usize>,
    circles: HashMap<usize, usize>,
    constraints: HashMap<usize, usize>,
    planes: HashMap<usize, usize>,
    extrusions: HashMap<usize, usize>,
    bodies: HashMap<usize, usize>,
}

fn build_creation_ranks(doc: &Document) -> CreationRanks {
    let mut ranks = CreationRanks::default();
    ranks.planes.insert(0, 0);
    let mut sketch_n = 0usize;
    let mut line_n = 0usize;
    let mut circle_n = 0usize;
    let mut constraint_n = 0usize;
    let mut plane_n = 1usize;
    let mut extrusion_n = 0usize;
    let mut body_n = 0usize;
    for (rank, kind) in doc.shape_order.iter().enumerate() {
        match kind {
            ShapeKind::Sketch => {
                ranks.sketches.insert(sketch_n, rank);
                sketch_n += 1;
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
            ShapeKind::Extrusion => {
                ranks.extrusions.insert(extrusion_n, rank);
                extrusion_n += 1;
            }
            ShapeKind::Body => {
                ranks.bodies.insert(body_n, rank);
                body_n += 1;
            }
            ShapeKind::Parameter => {}
            // A plane edit is not a created shape; it only marks an undoable edit.
            ShapeKind::ConstructionPlaneEdit => {}
        }
    }
    ranks
}

fn creation_rank(ranks: &CreationRanks, node: HierarchyNode) -> usize {
    match node {
        // Always the sole tree root; rank is irrelevant since it has no siblings.
        HierarchyNode::Document => 0,
        HierarchyNode::ConstructionPlane(i) => *ranks.planes.get(&i).unwrap_or(&i),
        HierarchyNode::Sketch(i) => *ranks.sketches.get(&i).unwrap_or(&i),
        HierarchyNode::Line(i) => *ranks.lines.get(&i).unwrap_or(&i),
        HierarchyNode::Circle(i) => *ranks.circles.get(&i).unwrap_or(&i),
        HierarchyNode::Constraint(i) => *ranks.constraints.get(&i).unwrap_or(&i),
        HierarchyNode::Extrusion(i) => *ranks.extrusions.get(&i).unwrap_or(&i),
        HierarchyNode::Body(i) => *ranks.bodies.get(&i).unwrap_or(&i),
    }
}

/// Build the hierarchy tree for the current view context.
///
/// Returns a single-element vec: the synthetic [`HierarchyNode::Document`] root, with every
/// former top-level item (root construction planes, orphaned extrusions, orphaned bodies)
/// nested as its children (#87).
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
    // Extrusions nest under the sketch they were built from (see
    // build_sketch_entry). Any extrusion whose sketch is no longer reachable is
    // surfaced at the top level so it never disappears from the tree.
    for (i, extrusion) in doc.extrusions.iter().enumerate() {
        if extrusion.deleted || sketch_alive(doc, extrusion.sketch) {
            continue;
        }
        roots.push(HierarchyEntry {
            node: HierarchyNode::Extrusion(i),
            children: build_sketch_extrusions(doc, extrusion.sketch, sketch_session)
                .into_iter()
                .find(|e| e.node == HierarchyNode::Extrusion(i))
                .map(|e| e.children)
                .unwrap_or_default(),
        });
    }
    // Bodies with no source extrusion (e.g. STL imports, #70) have no sketch/feature to nest
    // under, so they surface at the top level, same as an orphaned extrusion above.
    for (bi, body) in doc.bodies.iter().enumerate() {
        if !body.deleted && body.source.extrusion_indices().is_empty() {
            roots.push(HierarchyEntry {
                node: HierarchyNode::Body(bi),
                children: Vec::new(),
            });
        }
    }
    vec![HierarchyEntry {
        node: HierarchyNode::Document,
        children: roots,
    }]
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
        // Rank orders by creation; the node itself is a deterministic, creation-ordered
        // tiebreak when ranks collide (e.g. the default plane vs. the first shape_order slot).
        ready.sort_by_key(|node| (rank(*node), *node));
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
        SceneElement::Line(index) => doc
            .lines
            .get(index)
            .map(|line| SceneElement::Sketch(line.sketch)),
        SceneElement::Circle(index) => doc
            .circles
            .get(index)
            .map(|circle| SceneElement::Sketch(circle.sketch)),
        SceneElement::Constraint(index) => doc
            .constraints
            .get(index)
            .map(|c| SceneElement::Sketch(c.sketch)),
        SceneElement::Point(point) => point_parent_element(doc, point),
        // An extrusion depends on (and nests under) the sketch it was built from.
        SceneElement::Extrusion(index) => doc
            .extrusions
            .get(index)
            .map(|extrusion| SceneElement::Sketch(extrusion.sketch)),
        // A body depends on (and nests under) the feature that produced it; a merged body
        // nests under its first (originating) extrusion.
        SceneElement::Body(index) => doc.bodies.get(index).and_then(|body| {
            body.source
                .extrusion_indices()
                .first()
                .map(|&ei| SceneElement::Extrusion(ei))
        }),
        // A face's own edge isn't a hierarchy-pane node in its own right (it's a constraint
        // reference, not an independently listed element) — no parent to nest under.
        SceneElement::FaceEdge(_) => None,
    }
}

fn point_parent_element(doc: &Document, point: ConstraintPoint) -> Option<SceneElement> {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => doc
            .lines
            .get(line)
            .map(|_| SceneElement::Line(line)),
        ConstraintPoint::CircleCenter(circle) => Some(SceneElement::Circle(circle)),
        // A face's own vertex nests under the extrusion that produced its face.
        ConstraintPoint::FaceVertex { face, .. } => {
            face.extrusion_index().map(SceneElement::Extrusion)
        }
    }
}

fn collect_ancestors(doc: &Document, element: SceneElement, out: &mut HashSet<SceneElement>) {
    let mut current = element;
    while let Some(parent) = parent_element(doc, current) {
        out.insert(parent.clone());
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
            for (ei, extrusion) in doc.extrusions.iter().enumerate() {
                if !extrusion.deleted && extrusion.sketch == sketch {
                    out.insert(SceneElement::Extrusion(ei));
                    collect_descendants(doc, SceneElement::Extrusion(ei), out);
                }
            }
        }
        SceneElement::Circle(index) => {
            for sketch in doc.sketches_on_face(FaceId::Circle(index)) {
                out.insert(SceneElement::Sketch(sketch));
                collect_descendants(doc, SceneElement::Sketch(sketch), out);
            }
        }
        SceneElement::Extrusion(index) => {
            for (bi, body) in doc.bodies.iter().enumerate() {
                if !body.deleted && body.source.owns_extrusion(index) {
                    out.insert(SceneElement::Body(bi));
                }
            }
            // Sketches placed on this extrusion's cap or side-wall faces.
            for (si, sketch) in doc.sketches.iter().enumerate() {
                if !sketch.deleted
                    && matches!(sketch.face,
                        FaceId::ExtrudeCap { extrusion, .. } | FaceId::ExtrudeSide { extrusion, .. }
                        if extrusion == index)
                {
                    out.insert(SceneElement::Sketch(si));
                    collect_descendants(doc, SceneElement::Sketch(si), out);
                }
            }
        }
        SceneElement::Line(_)
        | SceneElement::Constraint(_)
        | SceneElement::Point(_)
        | SceneElement::Body(_)
        | SceneElement::FaceEdge(_) => {}
    }
}

fn selection_anchor(element: &SceneElement) -> SceneElement {
    element.clone()
}

fn distance_target_touches_element(target: &DistanceTarget, element: &SceneElement) -> bool {
    match (target, element) {
        (DistanceTarget::LineLength(i), SceneElement::Line(j)) => i == j,
        (DistanceTarget::CircleDiameter(c), SceneElement::Circle(i)) => c == i,
        (DistanceTarget::LineLineDistance {
            line_a,
            line_b,
            side: _,
        }, element) => {
            constraint_line_touches_element(line_a, element)
                || constraint_line_touches_element(line_b, element)
        }
        (DistanceTarget::PointPointDistance { anchor, mover, .. }, element) => {
            constraint_point_touches_element(anchor, element)
                || constraint_point_touches_element(mover, element)
        }
        (DistanceTarget::PointLineDistance { point, line, .. }, element) => {
            constraint_point_touches_element(point, element)
                || constraint_line_touches_element(line, element)
        }
        _ => false,
    }
}

fn constraint_line_touches_element(line: &ConstraintLine, element: &SceneElement) -> bool {
    match (line, element) {
        (ConstraintLine::Line(i), SceneElement::Line(j)) => i == j,
        (
            ConstraintLine::Line(i),
            SceneElement::Point(ConstraintPoint::LineEndpoint { line, .. }),
        ) => i == line,
        (ConstraintLine::FaceEdge { face, index }, SceneElement::Point(ConstraintPoint::FaceVertex {
            face: f,
            index: i,
        })) => face == f && (*index == *i || (*index + 1) == *i),
        (ConstraintLine::FaceEdge { .. }, _) => false,
        _ => false,
    }
}

fn constraint_point_touches_element(point: &ConstraintPoint, element: &SceneElement) -> bool {
    match (point, element) {
        (p, SceneElement::Point(q)) => p == q,
        (ConstraintPoint::LineEndpoint { line, .. }, SceneElement::Line(i)) => line == i,
        (ConstraintPoint::CircleCenter(c), SceneElement::Circle(i)) => c == i,
        _ => false,
    }
}

fn constraint_entity_touches_element(entity: &ConstraintEntity, element: &SceneElement) -> bool {
    match entity {
        ConstraintEntity::Point(point) => constraint_point_touches_element(point, element),
        ConstraintEntity::Line(line) => constraint_line_touches_element(line, element),
        ConstraintEntity::Circle(circle) => *element == SceneElement::Circle(*circle),
        ConstraintEntity::Origin => false,
    }
}

fn constraint_kind_touches_element(kind: &ConstraintKind, element: &SceneElement) -> bool {
    match kind {
        ConstraintKind::Distance { target } => distance_target_touches_element(target, element),
        ConstraintKind::Parallel { line_a, line_b }
        | ConstraintKind::Perpendicular { line_a, line_b }
        | ConstraintKind::Equal { line_a, line_b } => {
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
        ConstraintKind::Angle {
            line_a,
            line_b,
            rotation_sign: _,
        } => {
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
            constraint_kind_touches_element(&constraint.kind, &element).then_some(index)
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
        let anchor = selection_anchor(&element);
        let anchor_differs = anchor != element;
        related.extend(constraints_for_element(doc, anchor));
        if anchor_differs {
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
        let anchor = selection_anchor(&element);
        context.insert(anchor.clone());
        collect_ancestors(doc, anchor.clone(), &mut context);
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
    UsesVariable,
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
/// Accent for rows whose dimension uses the focused variable.
const USES_VARIABLE_TEXT: Color32 = Color32::from_rgb(120, 215, 230);

fn row_is_selected(element: &SceneElement, selection: &SceneSelection) -> bool {
    selection.is_selected(element.clone())
}

/// Only dim the list when a selected element is actually shown in it.
fn selection_styles_visible_list(elements: &[HierarchyNode], selection: &SceneSelection) -> bool {
    if selection.is_empty() {
        return false;
    }
    let list_elements: HashSet<SceneElement> = elements
        .iter()
        .filter_map(|node| scene_element_for_node(*node))
        .collect();
    selection.iter().any(|element| {
        let anchor = selection_anchor(&element);
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
    highlight_elements: &HashSet<SceneElement>,
) -> RowStyle {
    match health.element_status(element.clone()) {
        HealthStatus::Invalid => return RowStyle::Invalid,
        HealthStatus::Unstable => return RowStyle::Unstable,
        HealthStatus::Healthy => {}
    }
    // A focused variable highlights the elements that use it, dimming the rest.
    if !highlight_elements.is_empty() {
        return if highlight_elements.contains(&element) {
            RowStyle::UsesVariable
        } else {
            RowStyle::Faint
        };
    }
    if !style_selection {
        return RowStyle::Normal;
    }
    if row_is_selected(&element, selection) {
        RowStyle::Selected
    } else if matches!(&element, SceneElement::Constraint(index) if related_constraints.contains(index)) {
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
        RowStyle::UsesVariable => RichText::new(label).color(USES_VARIABLE_TEXT),
        RowStyle::Invalid => RichText::new(label).color(INVALID_TEXT),
        RowStyle::Unstable => RichText::new(label).color(UNSTABLE_TEXT),
        RowStyle::Faint => RichText::new(label).color(Color32::from_gray(120)),
    }
}

fn icon_tint_for_row_style(style: RowStyle) -> Color32 {
    match style {
        RowStyle::Selected | RowStyle::InContext | RowStyle::Normal => Color32::WHITE,
        RowStyle::RelatedConstraint => RELATED_CONSTRAINT_TEXT,
        RowStyle::UsesVariable => USES_VARIABLE_TEXT,
        RowStyle::Invalid => INVALID_TEXT,
        RowStyle::Unstable => UNSTABLE_TEXT,
        RowStyle::Faint => Color32::from_gray(120),
    }
}

/// Icon for a hierarchy row, or `None` when no existing icon fits (the synthetic Document
/// root — nothing in [`IconId`] represents "the whole document", so it renders without one).
fn icon_for_hierarchy_node(doc: &Document, node: HierarchyNode) -> Option<IconId> {
    Some(match node {
        HierarchyNode::Document => return None,
        HierarchyNode::ConstructionPlane(_) => IconId::Plane,
        HierarchyNode::Sketch(_) => IconId::Sketch,
        HierarchyNode::Line(_) => IconId::Line,
        HierarchyNode::Circle(_) => IconId::Circle,
        HierarchyNode::Constraint(index) => doc
            .constraints
            .get(index)
            .map(|constraint| icon_for_constraint_kind(&constraint.kind))
            .unwrap_or(IconId::Constraint),
        HierarchyNode::Extrusion(_) => IconId::Extrude,
        HierarchyNode::Body(_) => IconId::Body,
    })
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
        for (li, line) in doc.lines.iter().enumerate() {
            if line.deleted || line.sketch != sketch {
                continue;
            }
            let entry = HierarchyEntry {
                node: HierarchyNode::Line(li),
                children: vec![],
            };
            // A chamfer/fillet bridging line (#76) nests under the (lower-index) trimmed line
            // it came from, rather than sitting as an ordinary sibling. Since `chamfer_fillet_
            // parent` is always a lower line index, and `doc.lines` is iterated in index order,
            // the parent's entry is always already in `children` by the time we get here. If
            // the parent is gone (tombstoned) or otherwise not found — same graceful-orphan
            // handling as elsewhere in this file — fall back to a top-level sibling instead of
            // dropping the bridging line from the tree.
            if let Some(parent) = line.chamfer_fillet_parent {
                let alive_parent = doc
                    .lines
                    .get(parent)
                    .is_some_and(|p| !p.deleted && p.sketch == sketch);
                if alive_parent {
                    if let Some(parent_entry) = children
                        .iter_mut()
                        .find(|e| e.node == HierarchyNode::Line(parent))
                    {
                        parent_entry.children.push(entry);
                        continue;
                    }
                }
            }
            children.push(entry);
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

    // Extrusions built from this sketch nest under it (each owns its Body).
    children.extend(build_sketch_extrusions(doc, sketch, sketch_session));

    HierarchyEntry {
        node: HierarchyNode::Sketch(sketch),
        children,
    }
}

/// Hierarchy entries for the extrusions produced from `sketch`, each owning the
/// body it created and any sketches placed on its cap faces.
fn build_sketch_extrusions(
    doc: &Document,
    sketch: SketchId,
    sketch_session: Option<SketchSession>,
) -> Vec<HierarchyEntry> {
    doc.extrusions
        .iter()
        .enumerate()
        .filter(|(_, extrusion)| !extrusion.deleted && extrusion.sketch == sketch)
        .map(|(ei, _)| {
            let mut children: Vec<HierarchyEntry> = doc
                .bodies
                .iter()
                .enumerate()
                .filter(|(_, body)| !body.deleted && body.source.owns_extrusion(ei))
                .map(|(bi, _)| HierarchyEntry {
                    node: HierarchyNode::Body(bi),
                    children: Vec::new(),
                })
                .collect();
            for (si, sk) in doc.sketches.iter().enumerate() {
                if !sk.deleted
                    && matches!(sk.face,
                        FaceId::ExtrudeCap { extrusion, .. } | FaceId::ExtrudeSide { extrusion, .. }
                        if extrusion == ei)
                {
                    children.push(build_sketch_entry(doc, si, sketch_session));
                }
            }
            HierarchyEntry {
                node: HierarchyNode::Extrusion(ei),
                children,
            }
        })
        .collect()
}

pub fn node_label(doc: &Document, node: HierarchyNode) -> String {
    names::node_label(doc, node)
}

/// Draw the elements list in a side panel.
#[allow(clippy::too_many_arguments)]
pub fn show_pane(
    ui: &mut egui::Ui,
    doc: &Document,
    sketch_session: Option<SketchSession>,
    visibility: &mut ElementVisibility,
    selection: &SceneSelection,
    health: &DocumentHealth,
    view_mode: &mut HierarchyViewMode,
    graph_layout: &mut GraphLayout,
    on_edit_sketch: &mut impl FnMut(SketchId),
    on_edit_plane: &mut impl FnMut(usize),
    on_edit_extrusion: &mut impl FnMut(usize),
    on_export_body: &mut impl FnMut(usize),
    on_export_body_step: &mut impl FnMut(usize),
    on_toggle_visibility: &mut impl FnMut(SceneElement, bool),
    on_click_element: &mut impl FnMut(SceneElement, bool),
    highlight_elements: &HashSet<SceneElement>,
) {
    ui.horizontal(|ui| {
        ui.heading(PANE_TITLE);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (mode, icon, tooltip) in [
                (HierarchyViewMode::Graph, IconId::ViewGraph, "Graph-node view"),
                (HierarchyViewMode::Tree, IconId::ViewTree, "Tree view"),
                (HierarchyViewMode::List, IconId::ViewList, "List view"),
            ] {
                if selectable_icon_button(ui, icon, *view_mode == mode, tooltip).clicked() {
                    *view_mode = mode;
                }
            }
        });
    });
    ui.separator();

    let context = selection_context_elements(doc, selection);
    let related_constraints = selection_related_constraints(doc, selection);

    match view_mode {
        HierarchyViewMode::List => {
            let elements = build_element_list(doc, sketch_session);
            let style_selection = selection_styles_visible_list(&elements, selection);
            egui::ScrollArea::vertical().show(ui, |ui| {
                for node in elements {
                    show_row(
                        ui,
                        doc,
                        node,
                        1,
                        visibility,
                        selection,
                        health,
                        &context,
                        &related_constraints,
                        style_selection,
                        on_edit_sketch,
                        on_edit_plane,
                        on_edit_extrusion,
                        on_export_body,
                        on_export_body_step,
                        on_toggle_visibility,
                        on_click_element,
                        highlight_elements,
                    );
                }
            });
        }
        HierarchyViewMode::Tree => {
            let tree = build_hierarchy(doc, sketch_session);
            let flat_elements = build_element_list(doc, sketch_session);
            let style_selection = selection_styles_visible_list(&flat_elements, selection);
            egui::ScrollArea::vertical().show(ui, |ui| {
                show_tree_entries(
                    ui,
                    doc,
                    &tree,
                    0,
                    visibility,
                    selection,
                    health,
                    &context,
                    &related_constraints,
                    style_selection,
                    on_edit_sketch,
                    on_edit_plane,
                    on_edit_extrusion,
                    on_export_body,
                    on_export_body_step,
                    on_toggle_visibility,
                    on_click_element,
                    highlight_elements,
                );
            });
        }
        HierarchyViewMode::Graph => {
            let tree = build_hierarchy(doc, sketch_session);
            show_graph_view(
                ui,
                doc,
                &tree,
                graph_layout,
                selection,
                health,
                &context,
                &related_constraints,
                on_click_element,
                highlight_elements,
            );
        }
    }
}

/// Recursively render `entries` (and their nested children) at increasing indent, per #34's
/// Tree view — depth 0 is the synthetic Document root, depth 1 its direct children, etc.
#[allow(clippy::too_many_arguments)]
fn show_tree_entries(
    ui: &mut egui::Ui,
    doc: &Document,
    entries: &[HierarchyEntry],
    depth: usize,
    visibility: &mut ElementVisibility,
    selection: &SceneSelection,
    health: &DocumentHealth,
    context: &HashSet<SceneElement>,
    related_constraints: &HashSet<usize>,
    style_selection: bool,
    on_edit_sketch: &mut impl FnMut(SketchId),
    on_edit_plane: &mut impl FnMut(usize),
    on_edit_extrusion: &mut impl FnMut(usize),
    on_export_body: &mut impl FnMut(usize),
    on_export_body_step: &mut impl FnMut(usize),
    on_toggle_visibility: &mut impl FnMut(SceneElement, bool),
    on_click_element: &mut impl FnMut(SceneElement, bool),
    highlight_elements: &HashSet<SceneElement>,
) {
    for entry in entries {
        show_row(
            ui,
            doc,
            entry.node,
            depth,
            visibility,
            selection,
            health,
            context,
            related_constraints,
            style_selection,
            on_edit_sketch,
            on_edit_plane,
            on_edit_extrusion,
            on_export_body,
            on_export_body_step,
            on_toggle_visibility,
            on_click_element,
            highlight_elements,
        );
        show_tree_entries(
            ui,
            doc,
            &entry.children,
            depth + 1,
            visibility,
            selection,
            health,
            context,
            related_constraints,
            style_selection,
            on_edit_sketch,
            on_edit_plane,
            on_edit_extrusion,
            on_export_body,
            on_export_body_step,
            on_toggle_visibility,
            on_click_element,
            highlight_elements,
        );
    }
}

/// Accent stroke for graph-view edges/nodes among the selected node's ancestors and
/// descendants. Row styling has no direct line-drawing equivalent to reuse, so this is a
/// dedicated bold accent, distinct from the node fill colors (which do reuse
/// [`icon_tint_for_row_style`] for consistency with the List/Tree views).
const GRAPH_RELATED_EDGE: Color32 = Color32::from_rgb(120, 200, 255);

/// Render the graph-node view: a force-directed node-link diagram (#94). Nodes are pulled into
/// depth-ordered horizontal layers (so the graph flows top-to-bottom, "somewhat vertical"),
/// repelled from one another, and joined by parent↔child springs; the simulation animates each
/// frame ("bounce around") until its kinetic energy decays below a threshold, then settles and
/// stops requesting repaints. x is contained to the pane width; height scrolls vertically (#34).
#[allow(clippy::too_many_arguments)]
fn show_graph_view(
    ui: &mut egui::Ui,
    doc: &Document,
    tree: &[HierarchyEntry],
    graph_layout: &mut GraphLayout,
    selection: &SceneSelection,
    health: &DocumentHealth,
    context: &HashSet<SceneElement>,
    related_constraints: &HashSet<usize>,
    on_click_element: &mut impl FnMut(SceneElement, bool),
    highlight_elements: &HashSet<SceneElement>,
) {
    let positions = graph_node_positions(tree);
    if positions.is_empty() {
        return;
    }

    const NODE_RADIUS: f32 = 9.0;
    const TOP_PADDING: f32 = 24.0;
    const BOTTOM_PADDING: f32 = 24.0;
    // Per-frame integration: a handful of small substeps keeps the sim stable while settling
    // within a second or so of wall-clock animation.
    const SUBSTEPS: u32 = 6;
    const DT: f32 = 0.16;
    // Below this total kinetic energy the layout is considered settled; stop animating so an
    // idle pane doesn't busy-repaint.
    const SETTLE_KE: f32 = 0.05;

    // Nodes matching the current selection, plus their tree ancestors/descendants (#34): the
    // set of related nodes whose edges/fills get the bold accent.
    let mut related_nodes: HashSet<HierarchyNode> = HashSet::new();
    for position in &positions {
        if let Some(element) = scene_element_for_node(position.node) {
            if row_is_selected(&element, selection) {
                related_nodes.extend(graph_related_nodes(tree, position.node));
            }
        }
    }
    // Only dim unrelated nodes once something is actually selected — same convention as
    // `selection_styles_visible_list` uses for the List/Tree rows.
    let style_selection = !selection.is_empty();

    let available_width = ui.available_width().max(2.0 * GRAPH_MARGIN + 1.0);

    // Advance the physics, then keep animating until it settles.
    let kinetic = graph_layout.sync_and_step(&positions, available_width, SUBSTEPS, DT);
    if kinetic > SETTLE_KE {
        ui.ctx().request_repaint();
    }

    // Content height from the current simulated y-extent so tall graphs scroll (#34).
    let max_y = positions
        .iter()
        .filter_map(|p| graph_layout.pos_of(p.node).map(|v| v.y))
        .fold(0.0_f32, f32::max);
    let content_width = available_width;
    let content_height = max_y + TOP_PADDING + BOTTOM_PADDING + NODE_RADIUS;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let (rect, _response) =
                ui.allocate_exact_size(egui::vec2(content_width, content_height), egui::Sense::hover());
            let painter = ui.painter_at(rect);

            let pos_of = |node: HierarchyNode| -> egui::Pos2 {
                let local = graph_layout.pos_of(node).unwrap_or(egui::Vec2::ZERO);
                egui::pos2(rect.left() + local.x, rect.top() + TOP_PADDING + local.y)
            };

            // Edges first, so node dots paint over the line endpoints.
            for position in &positions {
                let Some(parent) = position.parent else { continue };
                let highlighted =
                    related_nodes.contains(&position.node) && related_nodes.contains(&parent);
                let stroke = if highlighted {
                    egui::Stroke::new(2.5, GRAPH_RELATED_EDGE)
                } else {
                    egui::Stroke::new(1.0, Color32::from_gray(110))
                };
                painter.line_segment([pos_of(parent), pos_of(position.node)], stroke);
            }

            for position in &positions {
                let center = pos_of(position.node);
                let element = scene_element_for_node(position.node);
                let style = element.clone().map(|el| {
                    row_style(
                        el,
                        selection,
                        context,
                        related_constraints,
                        style_selection,
                        health,
                        highlight_elements,
                    )
                });
                let selected = style == Some(RowStyle::Selected);
                let related = related_nodes.contains(&position.node);
                let fill = if selected {
                    Color32::WHITE
                } else if related {
                    GRAPH_RELATED_EDGE
                } else {
                    style.map(icon_tint_for_row_style).unwrap_or(Color32::from_gray(170))
                };

                let node_rect =
                    egui::Rect::from_center_size(center, egui::Vec2::splat(NODE_RADIUS * 2.0));
                let id = ui.id().with(("hierarchy_graph_node", position.node));
                let response = ui.interact(node_rect, id, egui::Sense::click());
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if let Some(element) = element {
                    let response = response.on_hover_text(node_label(doc, position.node));
                    if response.clicked() {
                        let additive = ui.input(|i| additive_click_modifiers(&i.modifiers));
                        on_click_element(element, additive);
                    }
                }

                painter.circle_filled(center, NODE_RADIUS, fill);
                painter.circle_stroke(center, NODE_RADIUS, egui::Stroke::new(1.0, Color32::from_gray(30)));

                let label = node_label(doc, position.node);
                // Keep the label inside the pane's right edge (#34).
                let max_label_width =
                    (rect.right() - (center.x + NODE_RADIUS + 4.0) - 4.0).max(20.0);
                let truncated = truncate_label(&label, max_label_width, &painter);
                painter.text(
                    center + egui::vec2(NODE_RADIUS + 4.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    truncated,
                    egui::FontId::default(),
                    if selected || related { Color32::WHITE } else { Color32::from_gray(200) },
                );
            }
        });
}

/// Truncate `label` (with an ellipsis) so it fits within `max_width` pixels at the default
/// font — graph-node labels must stay inside their column (#34).
fn truncate_label(label: &str, max_width: f32, painter: &egui::Painter) -> String {
    let font_id = egui::FontId::default();
    let galley_width =
        |s: &str| -> f32 { painter.layout_no_wrap(s.to_string(), font_id.clone(), Color32::WHITE).size().x };
    if galley_width(label) <= max_width {
        return label.to_string();
    }
    let mut truncated = String::new();
    for ch in label.chars() {
        let candidate = format!("{truncated}{ch}…");
        if galley_width(&candidate) > max_width {
            break;
        }
        truncated.push(ch);
    }
    format!("{truncated}…")
}

fn show_row(
    ui: &mut egui::Ui,
    doc: &Document,
    node: HierarchyNode,
    depth: usize,
    visibility: &mut ElementVisibility,
    selection: &SceneSelection,
    health: &DocumentHealth,
    context: &HashSet<SceneElement>,
    related_constraints: &HashSet<usize>,
    style_selection: bool,
    on_edit_sketch: &mut impl FnMut(SketchId),
    on_edit_plane: &mut impl FnMut(usize),
    on_edit_extrusion: &mut impl FnMut(usize),
    on_export_body: &mut impl FnMut(usize),
    on_export_body_step: &mut impl FnMut(usize),
    on_toggle_visibility: &mut impl FnMut(SceneElement, bool),
    on_click_element: &mut impl FnMut(SceneElement, bool),
    highlight_elements: &HashSet<SceneElement>,
) {
    // The synthetic Document root has no SceneElement — it isn't selectable, hideable, or
    // otherwise dispatched through the scene graph — so it gets a minimal, always-shown row
    // and returns before any of the SceneElement-keyed lookups below. Every other row is
    // indented `depth` levels (List always passes 1, matching #87's original single level;
    // Tree passes the node's real depth in the nested hierarchy, #34).
    if matches!(node, HierarchyNode::Document) {
        ui.horizontal(|ui| {
            if let Some(icon) = icon_for_hierarchy_node(doc, node) {
                ui.add(egui::Image::new(sized_texture(ui.ctx(), icon)));
            }
            ui.label(RichText::new(node_label(doc, node)).strong());
        });
        return;
    }

    let element = scene_element_for_node(node)
        .expect("non-Document HierarchyNode always maps to a SceneElement");
    if !element_alive(doc, element.clone()) {
        return;
    }
    let visible = visibility.effective_visible(doc, element.clone());
    let style = row_style(
        element.clone(),
        selection,
        context,
        related_constraints,
        style_selection,
        health,
        highlight_elements,
    );

    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 18.0);
        if icon_button(
            ui,
            icon_for_visibility(visible),
            if visible { "Hide" } else { "Show" },
        )
        .clicked()
        {
            let next = visibility.toggle(element.clone());
            on_toggle_visibility(element.clone(), next);
        }

        if let Some(icon) = icon_for_hierarchy_node(doc, node) {
            ui.add(
                egui::Image::new(sized_texture(ui.ctx(), icon))
                    .tint(icon_tint_for_row_style(style)),
            );
        }

        let label = node_label(doc, node);
        let response = ui.selectable_label(
            style == RowStyle::Selected,
            styled_label(&label, style),
        );
        match node {
            HierarchyNode::Document => unreachable!("handled by the early return above"),
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
            HierarchyNode::Extrusion(index) => {
                if row_primary_double_clicked(&response, ui) {
                    on_edit_extrusion(index);
                } else if response.clicked() {
                    let additive = ui.input(|i| additive_click_modifiers(&i.modifiers));
                    on_click_element(element, additive);
                }
                response.context_menu(|ui| {
                    if ui.button("Edit extrusion").clicked() {
                        on_edit_extrusion(index);
                        ui.close_menu();
                    }
                });
            }
            HierarchyNode::Body(index) => {
                if response.clicked() {
                    let additive = ui.input(|i| additive_click_modifiers(&i.modifiers));
                    on_click_element(element, additive);
                }
                response.context_menu(|ui| {
                    if ui.button("Export STL…").clicked() {
                        on_export_body(index);
                        ui.close_menu();
                    }
                    if ui.button("Export STEP…").clicked() {
                        on_export_body_step(index);
                        ui.close_menu();
                    }
                });
            }
            HierarchyNode::Line(_)
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
    fn default_document_hierarchy_has_single_document_root() {
        let doc = Document::default();
        let tree = build_hierarchy(&doc, None);
        assert_eq!(tree.len(), 1, "hierarchy should have exactly one root: {tree:?}");
        assert_eq!(tree[0].node, HierarchyNode::Document);
        // The default document's lone construction plane nests under Document rather than
        // sitting as a second root (#87).
        assert_eq!(
            tree[0].children.iter().map(|c| c.node).collect::<Vec<_>>(),
            vec![HierarchyNode::ConstructionPlane(0)]
        );

        let list = build_element_list(&doc, None);
        assert_eq!(
            list,
            vec![HierarchyNode::Document, HierarchyNode::ConstructionPlane(0)]
        );
    }

    #[test]
    fn root_level_items_nest_under_document_root() {
        use crate::document_lifecycle::tombstone_element;

        let mut doc = Document::default();
        // A second root-level construction plane (#87: root planes nest under Document,
        // not as separate roots).
        doc.construction_planes.push(default_xy_plane());
        doc.shape_order.push(ShapeKind::ConstructionPlane);

        // An orphaned extrusion: its sketch is tombstoned (unreachable), but the extrusion
        // itself is not cascaded away, so it must still surface — as a Document child, not
        // a top-level root.
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.extrusions.push(crate::model::Extrusion {
            sketch,
            faces: Vec::new(),
            distance: 5.0,
            target: None,
            expression: String::new(),
            name: None,
            deleted: false,
            edge_treatments: Vec::new(),
        });
        assert!(tombstone_element(&mut doc, SceneElement::Sketch(sketch)));
        assert!(!sketch_alive(&doc, sketch));

        // An orphaned body (STL import, no source extrusion, #70) also nests under Document.
        doc.imported_meshes.push(crate::model::ImportedMesh {
            triangles: vec![[glam::Vec3::ZERO, glam::Vec3::X, glam::Vec3::Y]],
            source_name: "part".to_string(),
        });
        doc.bodies.push(crate::model::Body {
            source: crate::model::BodySource::Imported(0),
            name: None,
            deleted: false,
        });

        let tree = build_hierarchy(&doc, None);
        assert_eq!(tree.len(), 1, "hierarchy should have exactly one root: {tree:?}");
        assert_eq!(tree[0].node, HierarchyNode::Document);
        let children: Vec<HierarchyNode> = tree[0].children.iter().map(|c| c.node).collect();
        assert!(children.contains(&HierarchyNode::ConstructionPlane(0)));
        assert!(children.contains(&HierarchyNode::ConstructionPlane(1)));
        assert!(children.contains(&HierarchyNode::Extrusion(0)));
        assert!(children.contains(&HierarchyNode::Body(0)));
    }

    #[test]
    fn imported_mesh_body_surfaces_at_top_level() {
        let mut doc = Document::default();
        doc.imported_meshes.push(crate::model::ImportedMesh {
            triangles: vec![[glam::Vec3::ZERO, glam::Vec3::X, glam::Vec3::Y]],
            source_name: "part".to_string(),
        });
        doc.bodies.push(crate::model::Body {
            source: crate::model::BodySource::Imported(0),
            name: Some("part".to_string()),
            deleted: false,
        });
        doc.shape_order.push(ShapeKind::Body);

        let list = build_element_list(&doc, None);
        assert!(
            list.contains(&HierarchyNode::Body(0)),
            "imported body should be visible in the elements list, got {list:?}"
        );
        assert_eq!(parent_element(&doc, SceneElement::Body(0)), None);
    }

    #[test]
    fn construction_plane_ordering_is_deterministic_by_creation() {
        let mut doc = Document::default();
        // Two planes created before any other geometry: the first shape_order slot is
        // rank 0, which used to tie with the default plane's hardcoded rank 0 and order
        // randomly (HashSet iteration). Ordering must be stable and by creation order.
        doc.construction_planes.push(default_xy_plane());
        doc.shape_order.push(ShapeKind::ConstructionPlane);
        doc.construction_planes.push(default_xy_plane());
        doc.shape_order.push(ShapeKind::ConstructionPlane);

        let expected = vec![
            HierarchyNode::Document,
            HierarchyNode::ConstructionPlane(0),
            HierarchyNode::ConstructionPlane(1),
            HierarchyNode::ConstructionPlane(2),
        ];
        // Repeat: HashSet iteration order is randomized per run, so a non-deterministic
        // sort would eventually disagree.
        for _ in 0..50 {
            assert_eq!(build_element_list(&doc, None), expected);
        }
    }

    #[test]
    fn hierarchy_node_icons_match_element_types() {
        use crate::model::{Constraint, ConstraintKind, ConstraintLine, ShapeKind};

        let mut doc = doc_with_plane_sketches();
        let sketch = 1;
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 1.0, 5.0, 1.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Parallel {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });

        assert_eq!(
            icon_for_hierarchy_node(&doc, HierarchyNode::ConstructionPlane(0)),
            Some(IconId::Plane)
        );
        assert_eq!(
            icon_for_hierarchy_node(&doc, HierarchyNode::Sketch(0)),
            Some(IconId::Sketch)
        );
        assert_eq!(
            icon_for_hierarchy_node(&doc, HierarchyNode::Rect(0)),
            Some(IconId::Rectangle)
        );
        assert_eq!(
            icon_for_hierarchy_node(&doc, HierarchyNode::Line(0)),
            Some(IconId::Line)
        );
        assert_eq!(
            icon_for_hierarchy_node(&doc, HierarchyNode::Constraint(0)),
            Some(IconId::Parallel)
        );
        assert_eq!(icon_for_hierarchy_node(&doc, HierarchyNode::Document), None);
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
        assert_eq!(list.len(), 4);
        assert_eq!(list[0], HierarchyNode::Document);
        assert_eq!(list[1], HierarchyNode::ConstructionPlane(0));
        assert_eq!(list[2], HierarchyNode::Sketch(0));
        assert_eq!(list[3], HierarchyNode::Sketch(1));
    }

    #[test]
    fn sketch_view_lists_geometry_of_active_sketch() {
        let doc = doc_with_plane_sketches();
        let list = build_element_list(&doc, Some(SketchSession { sketch: 0 }));
        assert_eq!(
            list,
            vec![
                HierarchyNode::Document,
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
                HierarchyNode::Document,
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
                HierarchyNode::Document,
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
                HierarchyNode::Document,
                HierarchyNode::ConstructionPlane(0),
                HierarchyNode::Sketch(0),
                HierarchyNode::Rect(0),
                HierarchyNode::Sketch(1),
            ]
        );
        let _ = s1;
    }

    #[test]
    fn extrusion_and_body_nest_under_source_sketch() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 20.0, 20.0));
        doc.extrusions.push(crate::model::Extrusion {
            sketch,
            faces: vec![crate::model::ExtrudeFace::Rect(0)],
            distance: 10.0,
            target: None,
            expression: String::new(),
            name: None,
            deleted: false,
            edge_treatments: Vec::new(),
        });
        doc.bodies.push(crate::model::Body {
            source: crate::model::BodySource::Extrusion(0),
            name: None,
            deleted: false,
        });

        assert_eq!(
            parent_element(&doc, SceneElement::Extrusion(0)),
            Some(SceneElement::Sketch(sketch))
        );
        assert_eq!(
            parent_element(&doc, SceneElement::Body(0)),
            Some(SceneElement::Extrusion(0))
        );

        let list = build_element_list(&doc, None);
        let si = list.iter().position(|n| *n == HierarchyNode::Sketch(0)).unwrap();
        let ei = list.iter().position(|n| *n == HierarchyNode::Extrusion(0)).unwrap();
        let bi = list.iter().position(|n| *n == HierarchyNode::Body(0)).unwrap();
        assert!(si < ei && ei < bi, "sketch -> extrusion -> body order: {list:?}");
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
                HierarchyNode::Document,
                HierarchyNode::ConstructionPlane(0),
                HierarchyNode::Sketch(0),
                HierarchyNode::ConstructionPlane(1),
            ]
        );
    }

    /// Recursively finds `node`'s entry anywhere in the tree (entries aren't just roots — e.g.
    /// a sketch nests under its construction-plane root).
    fn find_entry(entries: &[HierarchyEntry], node: HierarchyNode) -> Option<&HierarchyEntry> {
        for entry in entries {
            if entry.node == node {
                return Some(entry);
            }
            if let Some(found) = find_entry(&entry.children, node) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn chamfer_fillet_bridge_line_nests_under_lower_index_trimmed_line() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 10.0, 0.0, 10.0, 10.0));
        let mut bridge = Line::from_local_endpoints(sketch, 7.0, 0.0, 10.0, 3.0);
        bridge.chamfer_fillet_parent = Some(0);
        doc.lines.push(bridge);
        doc.shape_order.extend([ShapeKind::Line, ShapeKind::Line, ShapeKind::Line]);

        let tree = build_hierarchy(&doc, Some(SketchSession { sketch }));
        let sketch_entry = find_entry(&tree, HierarchyNode::Sketch(sketch)).expect("sketch entry");
        // The bridge (line 2) is *not* a top-level sibling of the sketch's lines...
        assert!(!sketch_entry
            .children
            .iter()
            .any(|c| c.node == HierarchyNode::Line(2)));
        // ...it nests under line 0 (the lower-index trimmed line, #76).
        let line0_entry = sketch_entry
            .children
            .iter()
            .find(|c| c.node == HierarchyNode::Line(0))
            .expect("line 0 entry");
        assert_eq!(line0_entry.children, vec![HierarchyEntry {
            node: HierarchyNode::Line(2),
            children: vec![],
        }]);

        // The flat list keeps line 0 before its nested bridge, and still includes line 1.
        let list = build_element_list(&doc, Some(SketchSession { sketch }));
        let l0 = list.iter().position(|n| *n == HierarchyNode::Line(0)).unwrap();
        let l1 = list.iter().position(|n| *n == HierarchyNode::Line(1));
        let l2 = list.iter().position(|n| *n == HierarchyNode::Line(2)).unwrap();
        assert!(l0 < l2, "parent line must come before the nested bridge");
        assert!(l1.is_some(), "the other trimmed line must still be listed");
    }

    #[test]
    fn chamfer_fillet_bridge_line_falls_back_to_top_level_when_parent_is_gone() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        let mut bridge = Line::from_local_endpoints(sketch, 7.0, 0.0, 10.0, 3.0);
        // Points at a parent index that doesn't exist (e.g. the parent line was later removed
        // by undo) — must degrade gracefully to a top-level row, not panic or vanish.
        bridge.chamfer_fillet_parent = Some(99);
        doc.lines.push(bridge);
        doc.shape_order.extend([ShapeKind::Line, ShapeKind::Line]);

        let tree = build_hierarchy(&doc, Some(SketchSession { sketch }));
        let sketch_entry = find_entry(&tree, HierarchyNode::Sketch(sketch)).expect("sketch entry");
        assert!(sketch_entry
            .children
            .iter()
            .any(|c| c.node == HierarchyNode::Line(1)));

        // Also degrades gracefully when the recorded parent line exists but is tombstoned.
        let mut doc2 = Document::default();
        let sketch2 = doc2.add_sketch(FaceId::ConstructionPlane(0));
        doc2.lines
            .push(Line::from_local_endpoints(sketch2, 0.0, 0.0, 10.0, 0.0));
        doc2.lines[0].deleted = true;
        let mut bridge2 = Line::from_local_endpoints(sketch2, 7.0, 0.0, 10.0, 3.0);
        bridge2.chamfer_fillet_parent = Some(0);
        doc2.lines.push(bridge2);
        doc2.shape_order.extend([ShapeKind::Line, ShapeKind::Line]);
        let tree2 = build_hierarchy(&doc2, Some(SketchSession { sketch: sketch2 }));
        let sketch_entry2 =
            find_entry(&tree2, HierarchyNode::Sketch(sketch2)).expect("sketch entry");
        assert!(sketch_entry2
            .children
            .iter()
            .any(|c| c.node == HierarchyNode::Line(1)));
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
                &HashSet::new(),
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
                &HashSet::new(),
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
                &HashSet::new(),
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
                &HashSet::new(),
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
                &HashSet::new(),
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
                &HashSet::new(),
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
                &HashSet::new(),
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
                &HashSet::new(),
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
                &HashSet::new(),
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

    /// A plane, a sketch with a rect, and an extrusion (owning a body) built from it — the
    /// small fixture #34's Graph-view layout/highlight tests exercise, built with the sketch
    /// under an active session so its rect shows up in the tree (see `build_sketch_entry`:
    /// plain sketch geometry is only listed while that sketch is being edited).
    fn doc_with_plane_sketch_rect_and_extrusion() -> (Document, SketchId) {
        use crate::model::{Body, BodySource, ExtrudeFace, Extrusion};

        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 10.0));
        doc.extrusions.push(Extrusion {
            sketch,
            faces: vec![ExtrudeFace::Rect(0)],
            distance: 5.0,
            target: None,
            expression: String::new(),
            name: None,
            deleted: false,
            edge_treatments: Vec::new(),
        });
        doc.bodies.push(Body {
            source: BodySource::Extrusion(0),
            name: None,
            deleted: false,
        });
        (doc, sketch)
    }

    #[test]
    fn graph_layout_assigns_column_by_depth_and_row_by_visit_order() {
        let (doc, sketch) = doc_with_plane_sketch_rect_and_extrusion();
        let tree = build_hierarchy(&doc, Some(SketchSession { sketch }));
        let positions = graph_node_positions(&tree);

        let find = |node: HierarchyNode| {
            positions
                .iter()
                .find(|p| p.node == node)
                .unwrap_or_else(|| panic!("missing position for {node:?}: {positions:?}"))
        };

        assert_eq!(positions.len(), 6, "{positions:?}");

        let root = find(HierarchyNode::Document);
        assert_eq!((root.depth, root.column, root.row, root.parent), (0, 0, 0, None));

        let plane = find(HierarchyNode::ConstructionPlane(0));
        assert_eq!(
            (plane.depth, plane.column, plane.row, plane.parent),
            (1, 1, 0, Some(HierarchyNode::Document))
        );

        let sketch_pos = find(HierarchyNode::Sketch(0));
        assert_eq!(
            (sketch_pos.depth, sketch_pos.column, sketch_pos.row, sketch_pos.parent),
            (2, 2, 0, Some(HierarchyNode::ConstructionPlane(0)))
        );

        // Rect and Extrusion are siblings under Sketch(0), so they share a column (depth 3)
        // but get distinct, visit-order rows.
        let rect = find(HierarchyNode::Rect(0));
        let extrusion = find(HierarchyNode::Extrusion(0));
        assert_eq!((rect.depth, rect.column, rect.parent), (3, 3, Some(HierarchyNode::Sketch(0))));
        assert_eq!(
            (extrusion.depth, extrusion.column, extrusion.parent),
            (3, 3, Some(HierarchyNode::Sketch(0)))
        );
        assert_ne!(rect.row, extrusion.row, "siblings in the same column need distinct rows");

        let body = find(HierarchyNode::Body(0));
        assert_eq!(
            (body.depth, body.column, body.row, body.parent),
            (4, 4, 0, Some(HierarchyNode::Extrusion(0)))
        );
    }

    #[test]
    fn graph_highlight_includes_ancestors_and_descendants_but_not_siblings() {
        let (doc, sketch) = doc_with_plane_sketch_rect_and_extrusion();
        let tree = build_hierarchy(&doc, Some(SketchSession { sketch }));

        // Selecting the leaf Rect(0) highlights its ancestor chain up to the root, but not
        // its sibling Extrusion(0) (or Extrusion's own descendant Body(0)).
        let related = graph_related_nodes(&tree, HierarchyNode::Rect(0));
        assert_eq!(
            related,
            HashSet::from([
                HierarchyNode::Rect(0),
                HierarchyNode::Sketch(0),
                HierarchyNode::ConstructionPlane(0),
                HierarchyNode::Document,
            ])
        );

        // Selecting an internal node (Sketch(0)) highlights the full subtree under it, plus
        // its own ancestors.
        let related = graph_related_nodes(&tree, HierarchyNode::Sketch(0));
        assert_eq!(
            related,
            HashSet::from([
                HierarchyNode::Sketch(0),
                HierarchyNode::ConstructionPlane(0),
                HierarchyNode::Document,
                HierarchyNode::Rect(0),
                HierarchyNode::Extrusion(0),
                HierarchyNode::Body(0),
            ])
        );

        // The root has no ancestors, only its whole subtree.
        let related = graph_related_nodes(&tree, HierarchyNode::Document);
        assert_eq!(related.len(), 6, "{related:?}");
    }

    #[test]
    fn hierarchy_view_mode_defaults_to_list() {
        assert_eq!(HierarchyViewMode::default(), HierarchyViewMode::List);
    }

    /// Drive the force layout to rest and return the final state, using the same fixture as the
    /// static-layout tests (plane → sketch → rect + extrusion → body).
    fn settle_graph_layout(width: f32, steps: u32) -> (GraphLayout, Vec<GraphNodePosition>) {
        let (doc, sketch) = doc_with_plane_sketch_rect_and_extrusion();
        let tree = build_hierarchy(&doc, Some(SketchSession { sketch }));
        let positions = graph_node_positions(&tree);
        let mut layout = GraphLayout::default();
        // First call seeds all nodes; subsequent calls just step.
        for _ in 0..steps {
            layout.sync_and_step(&positions, width, 1, 0.16);
        }
        (layout, positions)
    }

    #[test]
    fn force_layout_settles_stays_contained_and_flows_top_to_bottom() {
        let width = 300.0;
        let (layout, positions) = settle_graph_layout(width, 4000);

        // Kinetic energy has decayed toward zero — the sim settled rather than oscillating.
        let depth_of: HashMap<HierarchyNode, usize> =
            positions.iter().map(|p| (p.node, p.depth)).collect();
        let edges: Vec<(HierarchyNode, HierarchyNode)> = positions
            .iter()
            .filter_map(|p| p.parent.map(|parent| (p.node, parent)))
            .collect();
        let mut nodes = layout.nodes.clone();
        let ke = step_graph_layout(&mut nodes, &edges, &depth_of, width, 0.16);
        assert!(ke < 1e-2, "layout should settle, residual KE = {ke}");

        for p in &positions {
            let pos = layout.pos_of(p.node).expect("node has a settled position");
            assert!(pos.x.is_finite() && pos.y.is_finite(), "finite pos for {:?}: {pos:?}", p.node);
            assert!(
                (0.0..=width).contains(&pos.x),
                "x contained to pane for {:?}: {}",
                p.node,
                pos.x
            );
        }

        // Vertical-layering invariant: every parent settles strictly above (smaller y than)
        // each of its children.
        for p in &positions {
            let Some(parent) = p.parent else { continue };
            let child_y = layout.pos_of(p.node).unwrap().y;
            let parent_y = layout.pos_of(parent).unwrap().y;
            assert!(
                parent_y < child_y,
                "parent {parent:?} (y={parent_y}) must sit above child {:?} (y={child_y})",
                p.node
            );
        }
    }

    #[test]
    fn force_layout_is_deterministic() {
        let width = 320.0;
        let (a, positions) = settle_graph_layout(width, 1500);
        let (b, _) = settle_graph_layout(width, 1500);
        for p in &positions {
            let pa = a.pos_of(p.node).unwrap();
            let pb = b.pos_of(p.node).unwrap();
            assert!(
                (pa.x - pb.x).abs() < 1e-4 && (pa.y - pb.y).abs() < 1e-4,
                "same seed must give same settled position for {:?}: {pa:?} vs {pb:?}",
                p.node
            );
        }
    }

    #[test]
    fn force_layout_syncs_added_and_removed_nodes() {
        let (doc, sketch) = doc_with_plane_sketch_rect_and_extrusion();
        let tree = build_hierarchy(&doc, Some(SketchSession { sketch }));
        let positions = graph_node_positions(&tree);
        let mut layout = GraphLayout::default();
        layout.sync_and_step(&positions, 300.0, 1, 0.16);
        assert_eq!(layout.nodes.len(), positions.len());

        // A smaller node set (just the Document root) drops the departed nodes.
        let root_only = vec![GraphNodePosition {
            node: HierarchyNode::Document,
            parent: None,
            depth: 0,
            column: 0,
            row: 0,
        }];
        layout.sync_and_step(&root_only, 300.0, 1, 0.16);
        assert_eq!(layout.nodes.len(), 1);
        assert!(layout.pos_of(HierarchyNode::Document).is_some());
    }
}