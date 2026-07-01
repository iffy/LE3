# BearCAD — Specification

BearCAD is an on-device, parametric CAD program comparable to Autodesk Fusion, FreeCAD,
and OpenSCAD. This document is the implementation specification: it should contain
enough detail for an engineer to build BearCAD without further design decisions. Where a
section says **TBD**, that detail is deliberately deferred and must be resolved before
the relevant work begins.

---

## 1. Technology decisions (fixed)

These are settled. Do not re-litigate them during implementation.

| Concern | Decision | Notes |
|---|---|---|
| Implementation language | **Rust** | Produces a single self-contained executable; strong cross-platform GUI/3D ecosystem; good C/C++ FFI for the geometry kernel. |
| Geometry kernel | **OpenCASCADE (OCCT)** | B-rep solids, NURBS, booleans, fillets, and native STEP/IGES I/O. Used from Rust via FFI bindings (see §10). |
| Embedded scripting | **Lua** | Small, fast, sandboxable. No custom DSL. See §8. |
| GUI toolkit | **egui** | Immediate-mode; easy tiling/docking, command palette, theming. |
| 3D rendering | **wgpu** | Cross-platform GPU backend; the 3D viewport is a wgpu surface composited with egui. |
| Save file | **SQLite**, extension `.bearcad` | Schema in §7. |
| License | **MIT OR Apache-2.0** (dual) | BearCAD's own code is permissively licensed. OCCT is LGPL 2.1 and is **statically linked** under the LGPL's relink provision — BearCAD ships the pinned OCCT source (submodule), a build script, and an `OCCT_DIR` relink override (see §10). Bundle the LGPL + OCCT-exception text and all dependency notices via `THIRD_PARTY_LICENSES.md` (Help ▸ Licenses). Audit STEP/3MF/AMF library licenses for the same constraint. |

### 1.1 Platforms

Must build and run on **macOS, Linux, and Windows**, producing a single self-contained
executable per platform (kernel and other native libs may be dynamically linked but must
be bundled with the distributable). The executable launches the GUI by default and acts
as a CLI when given a subcommand (see §9).

**macOS packaging:** the `.app` bundle inside the distributed `.dmg` must be code-signed.
Absent a paid Apple Developer certificate, it must at minimum be **ad-hoc signed**
(`codesign --force --deep --sign -`) so that a quarantined download is not rejected by
Gatekeeper as *"'BearCAD' is damaged and can't be opened"* (the message macOS shows for an
unsigned or signature-invalidated bundle on Apple Silicon). The signature must be applied to
the fully assembled bundle (after the executable, icons, and `Info.plist` are in place) and
verified with `codesign --verify --deep --strict`. The `.dmg` volume must also contain an
`Applications` symlink (→ `/Applications`) alongside the app so the user can drag
`BearCAD.app` straight into Applications from the mounted volume.

---

## 2. Core concepts and domain model

### 2.1 Document

A document is one `.bearcad` file. A document contains:

- One or more **components**.
- A set of document-level **parameters** (see §5).
- The full **action DAG** (see §4).
- **UI/view state** (pane layout, camera, theme, custom shortcuts).

### 2.2 Component

A **component** is an independent unit of geometry with its own coordinate system,
its own parameters, its own sketches and features, and its own subgraph within the
action DAG (see §4.2). A component may **reference** other components; such a reference
creates a dependency edge in the DAG, and the referenced component's geometry/parameters
become inputs to the referencing component.

### 2.3 Assembly

Components can be placed into an **assembly**: instances of components positioned in
space and related by **joints/mates** (e.g. rigid, revolute, slider, coincident-face).
Joints are themselves parametric and participate in the DAG. A document may contain
multiple assemblies. (Detailed joint catalog: **TBD**, but at minimum rigid and revolute
for v1.)

### 2.4 Feature

A **feature** is a single modeling operation that produces or modifies geometry — a
sketch, an extrude, a fillet, a boolean, etc. Features are the primary nodes of the
action DAG (§4). The current geometry of a component is the result of evaluating its
features in dependency order.

### 2.5 World coordinate system

- The world is **right-handed with Z up**. The **ground plane is XY** (z = 0) and is the
  default sketching plane when none is chosen. X and Y span the ground; Z is height.
- Internal canonical length unit is millimetres (§5.3); the ground plane and all geometry
  are expressed in this convention.

---

## 3. Geometry & modeling operations (v1 scope)

All geometry is B-rep via OCCT. The following operations are **in scope for v1**:

### 3.1 Sketching (2D)
- Sketches are created on a datum plane or a planar face.
- **Sketching on body faces:** the planar cap faces of an extruded body (the base and
  offset ends of each extruded profile) are selectable sketch faces. Clicking one with the
  Sketch tool starts a sketch on that face — its frame inherits the profile's in-plane axes,
  offset along the extrusion normal — and the geometry drawn there behaves exactly like any
  other sketch. Such a sketch (and anything built from it) nests under, and depends on, the
  extrusion whose face it sits on. A solid cap occludes the datum plane behind it for picking.
  When several faces project onto the cursor (e.g. the near and far faces of a solid), face
  picking resolves to the one nearest the camera, so a hover/click never selects a face hidden
  behind the body. Entering a sketch reorients the camera head-on to the face; for a near-vertical
  face (such as a side wall) the view is oriented with world up (+Z) toward the top of the screen
  so the ground stays at the bottom and orbit behaves normally, rather than rolling sideways.
- **Constraining to the sketched-on face itself (#26/#27):** while a sketch is open on one of
  a body's own faces (an extrusion cap or side wall — not a construction plane), that face's
  own analytic boundary loop (the same one used for its cap/side-wall geometry) is available as
  constraint targets: `ConstraintPoint::FaceVertex` for a corner and `ConstraintLine::FaceEdge`
  for an edge, both resolved by projecting the face's world-space boundary into the sketch's
  frame. They plug into the existing constraint machinery like any other point/line — a sketch
  point can be **Coincident** to a face vertex, and the **Midpoint**/**PointLineDistance**
  constraints work against a face edge unchanged (e.g. "30mm from the top edge"). Both are
  fixed by the body's geometry (not draggable/settable), the same treatment `Coincident`'s
  `Origin` entity already gets. Picking is scoped to the *active sketch's own face* only (not
  arbitrary other faces), with vertices taking precedence over edges like other sketch points.
  Out of scope: imported STL/STEP bodies have no analytic face/edge structure to reference.
- Sketch entities: line, arc, circle, ellipse, spline, point, and construction-geometry
  variants. Convenience primitives (e.g. **rectangle**, drawn as four constrained lines)
  may be offered as tools that emit the underlying entities.
- **Line tool chaining:** the line tool draws connected polylines — after a segment is
  committed, the next segment starts automatically at that endpoint (coincident with it), so
  a polygon is drawn with successive clicks. Chaining stops when the segment's end snaps onto
  an existing vertex (closing/joining the shape); **Esc** finishes the polyline, keeping the
  segments already drawn.
- Sketches are fully constraint-driven (see §6).
- **Snapping:** while drawing or dragging sketch geometry, the cursor snaps to nearby
  vertices, line midpoints, and lines (vertices take priority, then midpoints, then
  anywhere on a line). Leaving a point on a snap adds the implied constraint (coincident
  for a vertex or on-line snap, midpoint for a midpoint snap), deduped against existing
  constraints. A ring marks the active snap. Snapping is toggleable from the context pane
  and the toggle only appears for tools that snap (Select, Line, Rectangle, Circle) while a
  sketch is open.
- **Inference / extension snapping:** hovering a vertex while drawing arms its incident edges
  as extension guides; pulling away then snaps the point onto the **infinite extension** of
  those edges (within a perpendicular tolerance), with a dashed guide line from the edge to the
  point. Leaving the point there adds a point-on-line coincidence (collinear with the edge), so
  e.g. touching a rectangle corner lets the next point be placed in line with one of its sides.
- **Inference snapping onto a normal-at-midpoint guide (#41):** touching a line/edge's
  **midpoint** arms it as a normal-inference anchor; pulling away then snaps the point onto the
  **infinite line perpendicular to that edge, through its midpoint** (same touch-then-track
  interaction as the extension guide above, with its own dashed guide line). There's no single
  constraint primitive for "perpendicular through a midpoint", so leaving the point there instead
  invents a construction `Line` from the anchor's midpoint out toward the placed point (dashed,
  `construction: true`) and pins it with three existing constraints: `Midpoint` (its start at the
  anchor's midpoint), `Perpendicular` (to the anchor), and `Coincident` (the placed point onto the
  new line's carrier) — no new `ConstraintKind` needed.
- **Polygon faces from closed line loops (#66):** any set of plain `Line`s that connect
  end-to-end into a closed loop, via `Coincident` constraints on their endpoints, is itself a
  usable face — filled the same as a rect/circle profile (shared blue styling, construction
  loops dashed/dimmed like other construction geometry), pickable for sketching-on-face, and
  extrudable. Loops are detected on the fly (not a stored entity) as every simple cycle in the
  sketch's line-connectivity graph; a line shared by two loops (e.g. a rectangle split by a
  diagonal) yields multiple selectable polygon faces. Scriptable via
  `bearcad.extrude{ polygon = {line_index, ...} }`, which takes an explicit ordered line list
  rather than relying on auto-detection.
- **Bezier curves (#54):** a curve is a `Line` with an optional pair of cubic tangent-handle
  control points (`[0]` near `(x0,y0)`, `[1]` near `(x1,y1)`) — its two endpoints stay ordinary
  constrainable vertices, so coincidence/distance constraints, dragging, undo, and persistence
  all work unchanged. Curves are made three ways:
  - **Curve-mode toggle with the Line tool (#73):** the Line tool always places points with
    plain click-click (no click-drag gesture). Two independent toggles, shown as checkboxes in
    the Context pane (above Construction) while the Line tool is active and bound to keyboard
    shortcuts `B`/`T`, control what happens at each shared vertex of a drawn polyline:
    - **Curve mode (`B`, default off):** when on, the *next* point placed gets bezier handles on
      both sides of it (or just the outgoing side, if it's a fresh chain's starting point, since
      there's no previous segment to derive a tangent from yet). Concretely: committing the
      *n*-th point of a chain (n ≥ 3) retroactively smooths the shared vertex between the
      (n-2)→(n-1) and (n-1)→n segments — so a segment only curves once a further point makes its
      tangent meaningful. The toggle persists across chained segments (like Construction) and is
      read/written by `Action::ApplyCurveMode`/`ToggleCurveMode`.
    - **Tangent constraint (`T`, default on):** while curve mode is on, controls *how* each
      shared vertex is curved. On: both sides' handles are mirrored/tangent-continuous via the
      same smoothing used by "Convert to bezier curve" below. Off: the previous segment's handle
      is left alone and the new segment gets an independent "corner" handle a third of the way
      along its own chord — a barely-curved starting shape meant to be reshaped by hand via the
      draggable handles below.
    - **Live preview:** as the mouse moves before the next point is placed, the in-progress
      segment previews its live curve toward the cursor, and — when curve mode smooths a shared
      vertex — the previous segment's end visibly bends to stay smooth/corner-consistent with it,
      updating every frame.
    - Both toggles also work retroactively: with the Select tool, in sketch mode, with one or
      more vertices selected, `B` toggles the selected vertex(es) between curved and straight
      (straightens both incident lines if either is already curved, else smooths them — see
      `Action::SetVertexTangent`/`ConvertVertexToBezier`/`StraightenLine`) and `T` toggles
      between tangent-continuous (re-smoothed) and independent handles at the vertex. Vertices
      that don't join exactly two plain lines are skipped (no-op).
  - **Draggable handles:** once committed, a curved line's two tangent handles are shown (in the
    active sketch) as small discs with dashed guides back to their endpoint; dragging one
    reshapes the curve live. Clicking (rather than dragging) a handle selects it; pressing
    Delete/Backspace, or right-clicking it and choosing "Delete handle", straightens the line
    (#75) — a curve is either both handles or neither, so there's no independent per-handle
    state to remove, only the whole curve.
  - **Right-click a vertex:** right-clicking a vertex where exactly two plain lines meet offers
    "Convert to bezier curve", which smooths the joint into a tangent-continuous pair of curves
    (Catmull-Rom-style, using the two lines' far endpoints to set the tangent direction through
    the shared vertex). The reverse, "Straighten curve", is offered when right-clicking an
    existing curved line.
  - A curved line is faceted into `BEZIER_SEGMENTS` (24) straight sub-segments for rendering,
    hit-testing, and — when part of a closed polygon loop — extrusion tessellation (the same
    style of approximation already used for circular profiles). Side walls of an extrusion
    swept from a curved profile edge are correspondingly multi-faceted, not a single flat quad;
    however, per-edge affordances that still assume one edge = one flat quad (e.g. sketching on
    an extrusion's side-wall face) are not curve-aware — sketching on the side wall of a curved
    extrusion edge is not currently supported. Inference/extension snapping onto a curved line
    still uses its straight chord (not the true curve) for the midpoint/on-line snap targets.
  - Scriptable via `bearcad.line{ x=, y=, x1=, y1=, bezier = { {cx0, cy0}, {cx1, cy1} } }`.
- **Chamfer and fillet (#37/#38), 2D sketch vertices only:** both are tools ("push/pull" gizmo
  + text-entry input, mirroring the extrude tool) that operate on a sketch vertex where exactly
  two plain lines meet. Both truncate each line's endpoint back along itself and bridge the two
  new endpoints with a new `Line`: a **chamfer** truncates by the typed distance and bridges with
  a **straight** line; a **fillet** truncates by the tangent length implied by the requested
  radius and bridges with a line whose `bezier` field is set to a **single-cubic-bezier
  approximation of the circular arc** (accurate for realistic corner angles, not a true NURBS
  arc) — this reuses the bezier-curve machinery above (rendering, hit-testing, extrusion
  tessellation) for free, since a filleted corner is, to the rest of the app, just another curved
  `Line`. The tangent length is clamped so it never cuts back past either adjacent line's own far
  endpoint; a corner within ~1° of straight (0°/180°, i.e. parallel/anti-parallel edges) is
  rejected as degenerate. Only the `Coincident` constraint directly between the two treated
  endpoints is removed on commit — other constraints that happened to reference the old vertex
  position are **not** automatically fixed up (a known, documented limitation; the resulting
  sketch may need manual re-constraining). This is specifically the **2D sketch-vertex** case;
  the same Chamfer/Fillet tool also does a **3D solid-edge** mesh-bevel approximation on an
  extrusion's analytic side/cap edges when no sketch is open — see §3.4, which is *not* a true
  kernel-backed BREP fillet (BearCAD has no BREP/NURBS kernel — see §10). Scriptable via
  `bearcad.chamfer_vertex{ point = {...}, distance = }`
  and `bearcad.fillet_vertex{ point = {...}, radius = }`, where `point` is the usual
  `ConstraintPoint` table (e.g. `{ kind = "line", index = 0, ["end"] = "end" }`).
  - **Live geometry preview (#76):** while the gizmo is being placed or dragged (before commit),
    the actual treated-corner shape is drawn as a preview overlay — the two truncated points and
    the bridge between them (straight for a chamfer, sampled from the fillet's bezier arc) — not
    just the gizmo arrow. It's recomputed every frame from the live drag amount, so pulling the
    handle further visibly grows the cut/round before you commit.
  - **Elements pane nesting (#76):** the bridging `Line` a chamfer/fillet creates is nested under
    the trimmed line it came from, instead of appearing as an ordinary flat sibling. Since a
    corner is shared by two trimmed lines, the tie is broken deterministically by nesting under
    whichever of the two has the lower index in `doc.lines` (recorded once at commit time via
    `Line.chamfer_fillet_parent: Option<usize>`); if that parent line is later deleted, the
    bridging line falls back to a top-level row rather than disappearing. Its default label is
    also "Chamfer N"/"Fillet N" instead of the generic "Line N".
  - **Document root row (#87):** the Elements pane's sole top-level row is a synthetic
    **Document** node (not individually selectable or hideable); every root construction
    plane, orphaned extrusion, and orphaned body (e.g. STL/STEP imports) nests under it
    instead of appearing as a separate root.

### 3.2 Solid creation from sketches
- **Extrude** — blind, symmetric, to-object, with optional draft angle.
  - An **Extrusion** is a first-class feature element (own hierarchy row, nameable, undoable):
    it references one or more coplanar sketch faces (closed rect/circle/polygon profiles) and a
    signed distance along the plane normal, and generates a solid mesh (prism per rect or
    polygon, cylinder per circle). Each extrusion produces a **Body** (the solid result) that
    depends on it: the body nests under the extrusion in the Elements pane and is removed if the
    extrusion is deleted.
    Created in script via
    `bearcad.extrude{ rect|circle|polygon|rects|circles, distance, name?, body? }`.
  - Implemented: the data model (Extrusion + Body) with `.bearcad` persistence; mesh generation;
    both hierarchy elements; depth-tested flat-shaded rendering; and the interactive **Extrude
    tool** (`E`): click coplanar faces to toggle inclusion (hover-highlighted), drag the normal
    gizmo or type a distance (expressions/variables) to set the depth (positive or negative),
    with a live **semi-transparent** preview solid that updates as you type; Enter commits, Esc
    cancels; double-click / right-click → Edit re-opens an extrusion for changing faces or
    length. While an extrusion is being edited its committed body is hidden, so only the
    semi-transparent ghost preview is shown (the preview, not the old solid, reflects the
    in-progress edit). The gizmo handle floats a little above the solid's top face (rather than
    sitting on it), and typing a digit while the tool is active focuses the distance field and
    overwrites its value. The extrusion (and its body) nests under the sketch it was built from.
  - **Extrude-to-object**: during a gizmo drag, hovering a vertex/face/plane snaps the depth to
    that object and, on release, constrains the extrusion to it (`ExtrudeTarget`). The effective
    depth is then derived from the target's extended plane — to a vertex's perpendicular plane,
    or where the extrusion axis meets a face/construction-plane — and recomputes if that geometry
    moves. A free gizmo drag (no object) leaves a plain unconstrained distance. The live ghost
    preview reflects the snapped target immediately while still dragging (not just after
    release), so extruding to a slanted or irregular target shows the actual resulting shape —
    e.g. a slanted top cap — rather than a generic blind/rectangular extrude (#63).
  - **Body target (#32/#35)**: a `Body`'s source is one or more extrusions (`BodySource::Extrusion`
    for one, `BodySource::Extrusions` for several; `BodySource::Solid { add, cut }` once some of
    its extrusions are subtracted rather than added — see §3.3). Extruding from a sketch on an
    existing body's face (a cap or side face) defaults to joining that body instead of creating a
    new one; the context pane shows three (icon-labelled) choices while extruding or editing an
    extrusion — **New body**, **Add to `<body>`**, and **Cut `<body>`** — to override the choice
    (editing can also split a merged/cut extrusion back out into its own body). The **Cut** option
    is only offered when the OCCT kernel is compiled in, since a non-kernel build can't perform
    the subtraction (see §3.3). Deleting one extrusion of a multi-extrusion body only drops that
    extrusion's contribution — the body survives as long as it still has at least one added
    extrusion. Scriptable via `bearcad.extrude{ ..., body = "merge" | "cut" }` (`"merge"` joins,
    `"cut"` subtracts from, the face's body if there is one; omitted or any other value always
    creates a new body, matching the declarative/OpenSCAD-style default).
  - **Boolean-region face picking (#16/#62)**: when exactly two coplanar sketch shapes overlap
    with nonzero area (and no third shape also overlaps that pair — see scope below), clicking
    inside their combined footprint with the Extrude tool resolves to the specific atomic region
    under the cursor instead of a whole shape: their shared intersection, or one shape minus the
    other, via two point-in-polygon tests against the picked point. This is `ExtrudeFace::
    Boolean { op: BooleanOp::Intersection | Difference, a, b }` (`a`/`b` boxed `ExtrudeFace`s,
    recursive so the type stays general, though the interactive picker only ever constructs
    depth-1 combinations of two raw `Rect`/`Circle`/`Polygon` shapes) — toggled into
    `Extrusion::faces` exactly like any other face (multi-face selection already lets a union of
    two whole shapes be built by toggling both, so no separate `Union` variant is needed). The
    region's boundary is computed on demand by `crate::polygon_boolean::polygon_boolean`, a
    two-simple-polygon Weiler-Atherton clip (`Difference` reverses the clip polygon's winding —
    the standard trick that turns the same intersection-walk into a subtraction); its resolved
    loop feeds mesh generation, fill rendering, and hover-highlighting the same way a `Polygon`
    face's loop already does. Scriptable via `bearcad.extrude{ boolean = { op = "intersection" |
    "difference", a = <face spec>, b = <face spec> }, distance }`, where a face spec is `{rect=
    i}`/`{circle=i}`/`{polygon={...}}`/a nested `{boolean={...}}`.
    - **Scope (deliberate, not yet general N-way arrangements)**: only ever two shapes at a
      time — a sketch with three or more mutually-overlapping shapes falls back to today's
      whole-shape picking instead. `polygon_boolean` itself only produces a result when the
      boolean combination reduces to a **single simple polygon loop**; it returns `None`
      (falling back the same way) for a multi-part (disjoint-piece) result, a result with a
      hole (e.g. subtracting a shape strictly interior to another, which would leave an
      annulus), or a near-zero-area/degenerate result — these are deliberately rejected rather
      than approximated. No flat side-wall sketching is offered on a boolean-derived extrusion
      (`side_face_count` is 0 for it, mirroring `Circle`'s curved walls) since its edge count
      depends on the resolved (Document-dependent) geometry; the extrusion mesh itself is
      unaffected, since it walks the resolved profile loop directly.
- **Revolve** — about an axis, full or partial angle.

### 3.3 Combining solids
- **Boolean**: union, cut (subtract), intersect.
- **Extrude body modes (#32/#35)**: an extrusion commits into a body one of three ways — **New
  body** (its own body), **Add to body** (fused into an existing body's solid), or **Cut body**
  (subtracted from an existing body's solid). A body records its additive vs. subtracted
  extrusions in `BodySource::Solid { add, cut }`; `body_solid_mesh` fuses the added extrusions
  into one solid and then subtracts each cut extrusion via the kernel's `Shape::boolean(_,
  BoolOp::Cut)`, producing one watertight result instead of overlapping triangle soup. **Cut
  requires the OCCT kernel**: the hand-rolled non-kernel mesher can't subtract solids, so in a
  non-`occt` build a body with cut extrusions renders its additive geometry only (the cut is
  ignored) and the GUI doesn't offer the Cut option — a known limitation resolved once the kernel
  is the default (#89). The cut list round-trips through save/load regardless of build.

### 3.4 Modifying solids
- **Fillet** and **Chamfer**, 2D sketch vertices: the tools described in §3.1 (#37/#38) —
  truncate-and-bridge on a sketch vertex where two lines meet, with the fillet arc approximated
  by a single bezier segment on the bridging `Line`.
- **Fillet** and **Chamfer**, 3D solid edges (#77): with the OCCT kernel linked (`--features
  occt`, see §10) these are **true BREP fillets/chamfers** — the extrusion builds a real OCCT
  solid and `BRepFilletAPI_MakeFillet`/`MakeChamfer` is applied to the matched edges (matched by
  their analytic world-space endpoints), producing genuine tangent-continuous rounded / flat
  beveled surfaces, then tessellated for the viewport. In the default build (no kernel) the same
  edges get a **mesh-bevel approximation** instead: it doesn't attempt a tangent-continuous
  curved surface, correct face trimming, or vertex-miter blending where 3+ edges meet; it
  directly reshapes the extrusion's own triangle mesh. If the kernel can't place a treatment (an
  edge it can't match, or an OCCT error) that extrusion falls back to the mesh-bevel path, so
  broken geometry never ships. Both paths are scoped to bodies whose source is one or more
  `Extrusion`s with a `Rect` or `Polygon` profile, and to the two edge families that have a clean
  analytic definition there (see `crate::extrude::side_quad_world`/`cap_polygon_world`):
  - a **vertical side edge**, where two adjacent flat side walls of the profile meet, and
  - a **side/cap edge**, where a side wall meets the top or bottom cap.

  In the mesh-bevel fallback, **Chamfer** replaces the edge with a single flat bevel quad
  connecting the two originally adjacent faces, offset back from the edge by the chamfer distance
  on each side (the same truncate-by-`amount` math as the 2D vertex case,
  `crate::model::vertex_treatment_geometry`, generalized to arbitrary 3D corners via
  `crate::extrude::corner_bevel_3d` — any two rays from a shared point span a flat 2D subspace,
  so this is an exact, not approximated, embedding). **Fillet** replaces it with an N-segment
  faceted rounded bevel instead of a true curved surface, sampling the same cubic-bezier arc
  approximation the 2D fillet uses, faceted at `EDGE_TREATMENT_FILLET_SEGMENTS` (= `BEZIER_
  SEGMENTS`, the existing curve-faceting precedent). The `occt` build instead produces the true
  BREP fillet/chamfer surface described above.
  - **Explicitly out of scope**: `Circle`-profile edges (curved, no discrete side walls to
    bevel — `side_face_count` is 0); STL/STEP-imported bodies (pure triangle soup, no analytic
    profile to derive an edge from — #31's generic mesh-feature-edge extraction still works for
    *picking/hovering* those edges for plane-referencing, just not for beveling them); and a
    **vertex miter** where 3+ treated edges would meet at a shared corner — rejected at commit
    time (`crate::extrude::edge_treatment_conflicts`) rather than attempting to blend three
    bevels together, a documented limitation rather than a crash or wrong-looking result.
  - **Data model**: parametric, like everything else in this app (re-evaluated from the document
    every frame, not a one-time mesh edit). Each `Extrusion` carries `edge_treatments: Vec<
    EdgeTreatment>`, where `EdgeTreatment { edge: ExtrusionEdgeRef, kind: VertexTreatmentKind,
    amount: f32 }` and `ExtrusionEdgeRef` names the analytic edge family + index (`Vertical {
    face, edge }` or `Cap { face, edge, top }`, `face` indexing `Extrusion::faces`). `kind`
    reuses `VertexTreatmentKind` (Chamfer/Fillet) from the 2D case directly. `crate::extrude::
    extrusion_mesh` applies every treatment on a face while building its mesh.
  - **Interactive tool**: the same Chamfer/Fillet tool (`K`/`F`) as the 2D case — when a sketch
    is open it behaves exactly as §3.1 describes; when no sketch is open, clicking a body's
    analytic edge (picked directly from the edge list, not the generic mesh-feature-edge
    extraction, since the structured `ExtrusionEdgeRef` is needed) starts a parallel in-progress
    state and shows the same push/pull gizmo (anchored at the edge midpoint, pointing along the
    inward bisector of the two adjacent faces) with a live semi-transparent ghost-preview solid
    (reusing the extrude tool's `preview_extrusion`/`editing_extrusion` mechanism: a clone of the
    extrusion with the live treatment spliced in, the committed body hidden meanwhile) — drag or
    type an amount, Enter/click commits, Esc cancels.
  - Scriptable via `bearcad.chamfer_edge{ extrusion =, edge = {...}, distance = }` and
    `bearcad.fillet_edge{ extrusion =, edge = {...}, radius = }`, where `edge` is `{ kind =
    "vertical", face =, edge = }` or `{ kind = "cap", face =, edge =, top = }`.
- **Shell** — hollow a solid to a wall thickness, removing selected faces.

### 3.5 Advanced features
- **Sweep** — sweep a profile along a path.
- **Loft** — blend between two or more profiles.
- **Pattern** — linear and circular patterns of features/bodies.

Each operation is exposed identically through the GUI, the action DAG, and the scripting
API (§8). Failures from the kernel (e.g. a fillet that can't be applied) must surface as a
recoverable error on the relevant feature node, not a crash.

---

## 4. Action DAG (history & non-linear undo)

BearCAD replaces Fusion's linear timeline with a **directed acyclic graph of actions**. This
is the source of truth for the model; geometry is derived from it (see §4.4).

### 4.1 Nodes and edges
- A **node** is an action: creating/editing a feature, creating/editing a parameter,
  creating a component, defining a joint, etc. **Parameter creation and every parameter
  change are nodes**, exactly like geometric features.
- A **directed edge** `A → B` means *B depends on A* — i.e. B consumes an output of A
  (a body, a face/edge reference, a parameter value, a sketch, etc.). Dependencies are
  derived from real data references, not from authoring order.
- The graph is acyclic. Attempting an edit that would create a cycle is rejected.

### 4.2 Per-component subgraphs
- Each component has its own connected subgraph. Two independent components show two
  independent graphs. When component C references components A and B, C's subgraph shows
  dependency edges into A's and B's outputs.

### 4.3 Undo / redo / time travel
- Undo is **infinite and persistent** — it survives closing and reopening the file
  (the full history lives in the `.bearcad`; see §7).
- The history is a **commit graph**: each user-visible change creates a new state. Undo
  moves to the parent state; redo moves forward. Because history is a graph (branches
  allowed) rather than a line, redo may present multiple forward branches; the UI MUST
  let the user choose among them.
- Editing the *value* of an existing feature/parameter does **not** destroy downstream
  work — it re-evaluates dependents (§4.4). This is the key difference from a linear
  timeline: rolling "back" to edit a node does not discard later, independent nodes.

### 4.4 Evaluation, caching & recompute
- The **action DAG is the source of truth**; evaluated geometry is **derived and cached**.
  Evaluated geometry **is persisted in the `.bearcad`** so files open fast — open should
  display cached geometry without a full rebuild. Speed is a priority for this app.
- Each DAG node caches its evaluated output (per-node BREP and/or tessellation; granularity
  **TBD**, but at least per-feature). Editing a node invalidates only that node and its
  transitive dependents (dirty-propagation); unaffected branches keep their cache and are
  not recomputed. The same in-memory cache is used during a session.
- **Cache validity** is tracked per node by a fingerprint of (the node's inputs/payload +
  its upstream dependencies' fingerprints + the **OCCT version**). On open, any node whose
  fingerprint no longer matches its cached entry is recomputed; everything else loads from
  cache. This keeps cached geometry correct across edits and across OCCT upgrades.
- Because the DAG fully determines geometry, the cache is always reconstructible: a
  "force rebuild" command (and CLI flag, §9) discards the cache and replays the DAG.
- Evaluation must be **deterministic** given the same DAG and the same OCCT version, so
  that a rebuild, a headless CLI run, and the GUI all agree. Record the OCCT version in
  the file (§7).

### 4.5 Topological references (naming)
- Feature inputs that reference faces/edges (e.g. "fillet this edge") must use **stable
  topological identifiers**, not raw OCCT indices, so that upstream edits don't silently
  re-target downstream features. Define a persistent-naming scheme that maps user/feature
  references to topology across recomputes. (Algorithm: **TBD** — candidate: hash of
  generating feature + geometric signature. This is a known-hard CAD problem and must be
  designed explicitly.)

---

## 5. Parameters, expressions & units

### 5.1 Parameters
- Parameters are a first-class feature with their own pane in the GUI.
- Parameters exist at **document** and **component** scope; component parameters may
  shadow document ones.
- A parameter has: name, expression (text), evaluated value, unit, and optional
  description.
- Parameter changes are DAG nodes (§4.1).
- When a parameter's name or value field is focused in the Parameters pane, the Elements
  pane highlights every element that uses that parameter (the dimensions referencing it and
  the geometry they drive), dimming the rest.

#### 5.1.1 Inline parameter creation
- In **any value input** (GUI field or scripting), prefixing the entry with
  `name=` creates a new parameter on the spot and uses it for that input. For example,
  typing `width=20mm` in an extrude-distance field creates a parameter `width = 20mm` and
  binds the field to it (the field now holds the expression `width`). This mirrors
  Autodesk Fusion's inline-parameter behavior.
- The assignment target follows the normal scoping rules (§5.1); creation is a DAG node
  like any other parameter creation.
- If `name` already exists, the input must either **reuse** it (binding the field to the
  existing parameter) or, if a value is also supplied, treat `name=value` as redefining
  that parameter — the UI must make which one is happening unambiguous (e.g. reuse on
  bare `name=`, redefine on `name=value`, with a clear indicator). Reject names that
  collide with reserved words or that would create an expression cycle (§4.1).

### 5.2 Expressions
- **Any input that accepts a value accepts an expression**, e.g. `1 + 2 + lengthOfThing / 2`.
- Expressions may reference parameters and other values by name.
- Expressions support `+ - * /`, parentheses, and a standard math function library
  (trig, sqrt, min/max, etc. — full list **TBD**).
- The **raw expression text is stored verbatim** so the user sees and can edit exactly
  what they typed (e.g. `3mm + 2in`), alongside the evaluated value (§7).
- **Variable-name autocomplete**: while typing an identifier in an expression field, a
  dropdown offers matching parameter names (best match on top). Arrow keys move the
  highlight; **Space** or **Tab** completes the highlighted name and keeps editing;
  **Enter** completes the highlighted name *and* commits the field in a single keystroke.

### 5.3 Units
- Strong unit support with mixed units. `3mm + 2in` is valid and evaluates correctly.
- Every component has **default units**; a bare number inherits the contextually relevant
  default unit.
- Units are dimension-checked: adding a length to an angle is an error.
- Supported unit families for v1: length (mm, cm, m, in, ft), angle (deg, rad). Extend as
  needed.
- Internal canonical storage units: **TBD** (recommend millimeters for length, radians for
  angle), but the stored expression text is always preserved.
- **Default-unit picker (#52):** the Context pane lets the user choose default length/angle
  units. With nothing selected, it edits the document-wide defaults
  (`bearcad.set_units{ length = "mm", angle = "deg" }`). With exactly one **sketch** selected,
  it edits that sketch's own override instead, offering a "Follow document" entry per axis
  (length and angle can be overridden independently) that clears back to inheriting the
  document default (`bearcad.set_units{ sketch = N, length = "in" }`; omitting an axis on a
  sketch call means "follow document" for that axis, since Lua can't distinguish an omitted
  table field from an explicit `nil`). Any other selection hides the picker. **Scope note
  (#85):** dimension labels and the Elements pane now format geometry in the effective unit
  (document default, or the owning sketch's override) instead of always showing mm/degrees.
  This does **not** change the bare-number parsing fallback, which is still hardcoded to
  mm/degrees (per above) — internal storage stays mm/radians regardless of display unit.

---

## 6. Constraints

BearCAD has a geometric **constraint solver** supporting both 2D (sketch) and 3D constraints,
modeled on SolveSpace (https://solvespace.com).

### 6.0 Constraint tool (implemented subset)

- **Tool:** Constraint, shortcut **`C`**. Distance/dimensional constraints remain on the
  **Dimension** tool (`D`).
- **Angle dimensions — placement phase:** pressing `D` with two non-parallel lines selected
  (and no existing angle constraint between them) does not commit a value immediately.
  Instead the angle preview follows the mouse: two lines crossing have two distinct angle
  magnitudes (supplementary, one on each pair of opposite wedges), and whichever wedge
  encloses the cursor is the one previewed. Clicking commits that choice and moves to typing
  the value, the same as other dimensions (#40).
- **Selection:** Sketch points (line endpoints, rectangle corners, circle centres), lines,
  and rectangle edges are selectable in the viewport. Point picks take precedence near
  vertices within the point pick tolerance.
- **Context pane:** While the constraint tool is active, the context pane lists geometric
  constraint types as buttons (text labels for now; icons later).
  - **Always all types:** every constraint type is **always listed**, in fixed order.
    Types the current selection cannot satisfy (including when nothing is selected) appear
    **disabled/faded**, with a hint beside the button describing what must be selected
    (e.g. `line, line` for Parallel). Buttons are **enabled** only when the selection
    satisfies that constraint.
  - **Shortcuts:** each type has a fixed **mnemonic letter** shown left of its button —
    Parallel `A`, Perpendicular `T`, Equal `Q`, Coincident `I`, Midpoint `M`, Vertical `V`,
    Horizontal `H` (chosen to avoid the global tool keys). Pressing the letter while the
    constraint tool is active applies that constraint if it is currently enabled.
- **Geometric types (v1):**
  - **Parallel** — `line`, `line`
  - **Perpendicular** — `line`, `line`
  - **Equal** — `line`, `line` (the two edges are constrained to equal length; rect edges
    count as lines). See #47.
  - **Coincident** — `point`, `point`; `point`, `line`; or `point`, `circle` (point on the
    circle's perimeter). A `point`/`line` operand may be the sketch's own face's vertex/edge
    (#26/#27, see §3.1) — picked the same way as any other sketch point/line.
  - **Midpoint** — `point`, `line`
  - **Vertical** — `line`
  - **Horizontal** — `line`
- **Redundant-constraint cleanup:** when a point already constrained coincident with a line
  is then constrained to a *specific* point on that same line (one of its endpoints, or its
  midpoint), the earlier generic point-on-line coincidence is removed in favor of the more
  specific constraint.
- **Scripting:** `tool constraint`; `select point line 0 start`; `add_geometric_constraint
  parallel` (uses current selection). Circle tool shortcut is **`O`** (`C` is constraint).

### 6.1 2D sketch constraints (full set)
Coincident, point-on-entity, parallel, perpendicular, horizontal, vertical, tangent,
equal, concentric, symmetric, midpoint, and dimensional constraints (distance, length,
radius/diameter, angle). Dimensional constraints may be driven by parameters/expressions
(§5), so parameters can drive sketch geometry.

### 6.2 3D constraints
SolveSpace-style 3D constraints between 3D entities (points, lines, planes, faces):
coincident, parallel, perpendicular, distance, angle, point-on-plane/line, etc. These
back the assembly joints/mates (§2.3).

### 6.3 Solver
- A native Rust numeric constraint solver (`sketch_solver`) resolves sketch constraint
  systems by minimizing weighted residuals with dense Levenberg–Marquardt (SolveSpace-style).
- Rectangles decompose to four corner points; circles use centre point + radius variable.
- Interactive drag adds high-weight pin residuals; reference geometry uses softer holds that
  are skipped during drag so the solver can rebalance.
- The UI must report **under-** and **over-constrained** states and indicate conflicting
  constraints. `sketch_degrees_of_freedom()` exposes remaining DOF from Jacobian rank analysis.
- The solver is deterministic for headless/script use (fixed iteration order, fixed LM damping).

---

## 7. File format (`.bearcad` / SQLite)

A `.bearcad` is a SQLite database. The schema below is the starting point; refine during
implementation but keep the migration mechanism.

### 7.1 Versioning & migrations
- A `schema_migrations` table records every patch applied, so older files can be upgraded:
  ```sql
  CREATE TABLE schema_migrations (
    id          INTEGER PRIMARY KEY,   -- ordered migration id
    name        TEXT NOT NULL,         -- human-readable migration name
    applied_at  TEXT NOT NULL          -- ISO-8601 timestamp
  );
  ```
- On open, BearCAD applies any migrations whose id is newer than the file's latest applied
  migration. A file from a newer BearCAD than the running binary must be detected and refused
  (or opened read-only) rather than corrupted.
- A `meta` key/value table records app version, **OCCT version used** (for deterministic
  recompute, §4.4), document units defaults, etc.

### 7.2 What is persisted
- **Full action DAG / undo history** — every node and edge, enough to reconstruct all
  states and support infinite persistent undo.
- **Parameters** — name, raw expression text, evaluated value, unit, scope.
- **UI/view state** — pane layout, camera position(s), active theme, and per-document
  custom shortcuts.
- **Cached evaluated geometry** — per-node BREP and/or tessellation blobs plus their
  validity fingerprint (§4.4), so files open fast without a full rebuild. The cache is
  derived data: it can always be regenerated from the DAG and may be discarded
  (force-rebuild) or stripped to shrink a file.

### 7.3 Indicative schema (refine as needed)
```sql
CREATE TABLE meta            (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE components      (id INTEGER PRIMARY KEY, name TEXT, parent_id INTEGER, default_units TEXT);
CREATE TABLE parameters      (id INTEGER PRIMARY KEY, scope_component_id INTEGER, name TEXT,
                              expression TEXT, value REAL, unit TEXT, description TEXT);
CREATE TABLE dag_nodes       (id INTEGER PRIMARY KEY, component_id INTEGER, kind TEXT,
                              payload JSON);          -- feature/param/joint definition
CREATE TABLE dag_edges       (from_node INTEGER, to_node INTEGER,
                              PRIMARY KEY (from_node, to_node));
CREATE TABLE history_commits (id INTEGER PRIMARY KEY, parent_id INTEGER,
                              node_id INTEGER, created_at TEXT);  -- commit graph for undo/redo
CREATE TABLE ui_state        (key TEXT PRIMARY KEY, value JSON);
CREATE TABLE geometry_cache  (node_id INTEGER PRIMARY KEY, fingerprint TEXT NOT NULL,
                              brep BLOB, mesh BLOB, occt_version TEXT);  -- derived; rebuildable
```
The exact `payload`/`kind` encoding for each feature type is **TBD** but must round-trip
losslessly.

---

## 8. Scripting (Lua API)

Everything achievable in the GUI must be achievable by programming, and vice versa.

- The Lua API exposes the full document model: create/edit components, parameters,
  sketches, constraints, features; run booleans; export; etc.
- Scripted actions create DAG nodes identical to GUI actions — there is one model, two
  front ends.
- The interpreter is **sandboxed** (no arbitrary filesystem/network access by default;
  explicit, opt-in capabilities only).
- The API surface is versioned and documented. Exact module layout and function signatures
  are **TBD**, but must be designed so that the GUI's command set maps 1:1 onto API calls
  (this also powers the CLI, §9, and the command palette, §11).
- **Namespace split.** The primary API is *declarative modeling*, in the spirit of OpenSCAD:
  geometry/document operations live at the top level (`bearcad.new`, `bearcad.rect`,
  `bearcad.extrude`, `bearcad.add_constraint`, `bearcad.parameter`, `bearcad.select`, …).
  All **GUI/UI manipulation** — simulated mouse/keyboard, camera, tools, panes, the command
  palette, and viewport drags — lives under the `bearcad.ui.*` sub-namespace
  (`bearcad.ui.move`, `bearcad.ui.click`, `bearcad.ui.key`, `bearcad.ui.type`,
  `bearcad.ui.orbit`, `bearcad.ui.pan`, `bearcad.ui.wheel`, `bearcad.ui.view`,
  `bearcad.ui.tool`, `bearcad.ui.pane`, `bearcad.ui.palette`, `bearcad.ui.drag_vertex`,
  `bearcad.ui.wait`, `bearcad.ui.screenshot`, …). Examples and documentation should model
  with the top-level API and avoid `bearcad.ui.*` except where a UI interaction is the point.
- `bearcad.ui.screenshot([path], [whole_window])` captures the 3D viewport only by default (the
  view-cube HUD is suppressed for that frame); passing `whole_window = true` captures the
  entire window. With no `path`, the image is written to `screenshot-bearcad.png`.
- Geometry-creation helpers are single calls that create the thing directly (no simulated
  mouse/keyboard) and enter a ground-plane sketch if none is open: `bearcad.rect{ width, height,
  x?, y?, name? }` and `bearcad.line{ length, angle?, x?, y?, name? }` (or explicit endpoints
  `bearcad.line{ x, y, x1, y1 }`).
- `bearcad.begin_sketch{ … }` starts a sketch on any face. Besides `kind = "rect"|"circle"|"plane"`
  with `index`, it accepts **3D body faces**: `kind = "extrude_cap", extrusion, profile =
  "rect"|"circle", profile_index, top?` and `kind = "extrude_side", extrusion, profile,
  profile_index, edge?`. (This makes sketching on a solid's face scriptable, e.g. for testing.)
- **Point-level selection (#68):** `bearcad.select{ kind = "line", index, ["end"] = "start"|"end" }`
  or `bearcad.select{ kind = "rect", index, corner = 0..3 }` selects an individual vertex (a
  `ConstraintPoint`) rather than the whole element, so e.g. `bearcad.select{...}` +
  `bearcad.select({...}, true)` + `bearcad.add_geometric_constraint("coincident")` can join two
  line endpoints (closing a polygon loop) purely from a script — the same point-numbering the
  interactive Constraint tool uses (a rect's corners are numbered 0–3 counterclockwise starting
  at its `(x, y)` origin corner; a line's two points are `start`/`end`, i.e. `(x0,y0)`/`(x1,y1)`).
  A table with neither `end` nor `corner` still resolves to the whole element as before; pass an
  explicit `point = true` to target a point that has no such field (e.g. a circle's center).
- **Face vertex/edge selection (#26/#27):** `bearcad.select{ kind = "face", face = { … }, index }`
  selects a corner of the *sketched-on* face's own boundary loop (a `ConstraintPoint::FaceVertex`);
  add `edge = true` to select the edge from that corner to the next instead
  (`ConstraintLine::FaceEdge`). `face` is a nested table in the same shape `begin_sketch` takes
  for a 3D body face (`kind = "extrude_cap"|"extrude_side", extrusion, profile, profile_index,
  top?/edge?`). Combine with the point-level selection above to build the constraint purely from
  a script, e.g. pinning a sketch point coincident to the face's corner 2.

---

## 9. Command-line interface

**Guiding principle:** the CLI can do *anything the GUI can do except operations that
inherently require mouse interaction* (e.g. free dragging in the viewport). The CLI and
GUI share the same model and the same action set; most CLI subcommands are thin wrappers
over scripting (§8).

Instruction scripts (§9.3) are the deliberate exception to the "no mouse interaction" rule;
they exist specifically so that interactive flows can be driven programmatically for testing
and automation (including screenshot capture of the live UI).

### 9.1 v1 subcommands
- `export` — export a `.bearcad` to `.3mf`, `.stl`, `.obj`, `.amf`, or `.step`/`.stp`.
- `run` — execute a Lua script headless against a new or existing `.bearcad`.
- `render` — render the model to an image (e.g. PNG) from a specified camera.
- `set` / parameter override + re-export — override named parameters from the command line
  and export, enabling part families from one file.
- `import` / `convert` — import STEP/STL/etc. into a `.bearcad`, or convert between formats.
- `install-cli` / `uninstall-cli` — symlink the running executable onto PATH as `bearcad`
  (default `/usr/local/bin/bearcad`), and remove it. Because macOS drag-to-Applications
  installs run no code, this is how the bundled binary becomes usable from a terminal; it is
  also exposed as **Help → Install "bearcad" Command in PATH**. Refuses to clobber a
  non-symlink at the target, and reports a sudo hint on permission errors.

The command set is expected to **grow over time** toward full GUI parity. New GUI actions
should be added to the shared action layer so they become available headlessly by default.

- `--timeout <seconds>` — force-exit (non-zero) if the app hasn't closed on its own within
  the given duration, so an unattended/CI launch can't hang forever (#61).

### 9.2 Export formats (required)
`.3mf`, `.stl`, `.obj`, `.amf`, `.step`/`.stp`. STEP via OCCT; mesh formats via OCCT
tessellation + writers (or dedicated libraries — license-audited per §1).

### 9.3 Instruction scripts (for automation & testing)

**Directive:** The app should be fully scriptable. One must be able to run the app with a set of instructions (from a file) and the app must open and run each of the instructions. One must be able to export a screenshot of how the app looks as one of the instructions. This can then be leveraged for testing.

The application must be fully scriptable via a file containing a sequence of instructions.

- Invocation: `bearcad <script-file>` or `bearcad --script <script-file>` (or equivalent).
- When a script is provided the app shall open, sequentially execute every instruction in order,
  and apply the effects exactly as a user would (updating document, tools, camera, in-progress
  interactions, UI state, etc.).
- One supported instruction must be screenshot/export of the app's current visual appearance:
  `screenshot <output-path>` (PNG or other common image format). The captured image must be a
  faithful rendering of the full window (or primary viewport + overlays) at the moment the
  instruction is executed, suitable for visual regression testing.
- Scripts shall support at minimum:
  - Core actions (new, open, save, clear, tool selection, rectangle creation flow including
    the click-to-place, mouse-move preview, dimension typing, tab, enter steps, etc.).
  - Camera/view control.
  - File I/O and export.
  - The screenshot instruction above.
  - Simple sequencing / waits if needed for UI settling or animations.
- This mechanism exists primarily to enable automated testing. Test scripts can drive the exact
  interactive flows (e.g. the rectangle tool's click → move → type → enter sequence) and emit
  screenshots that can be compared against golden images in CI.
- Execution must be deterministic (fixed random seeds, consistent layout, theme, DPI, camera,
  font rasterization, etc.) so that screenshots are reproducible.
- The precise syntax and full instruction vocabulary are **TBD** but must be simple,
  human-readable, versioned, and documented. The implementation must keep the set of
  instructions in sync with GUI actions.

The guiding principle in §9 still applies for normal CLI; instruction scripts are the
explicit exception that lets us drive "mouse/keyboard" flows for testing purposes.

---

## 10. Geometry kernel integration (OCCT)

- Integrate OCCT via Rust FFI through a **hand-written thin C++ shim** exposing only the
  operations BearCAD needs (sketch profiles, prism/revol, boolean, fillet/chamfer, shell,
  sweep/loft, STEP/mesh I/O, tessellation). All `unsafe`/FFI is isolated behind a safe Rust
  `kernel` module (`src/kernel/`, shim in `cpp/`). The shim presents a flat `extern "C"` C
  ABI (no C++ types cross the boundary), so no `bindgen` is required.
- OCCT is **statically linked**, gated behind the off-by-default **`occt` Cargo feature** so
  the default build/CI need no C++ toolchain or built OCCT. Static linking is permitted under
  OCCT's LGPL 2.1 because BearCAD ships the means to relink against a different OCCT: the
  pinned OCCT source (the `third_party/OCCT` git submodule), a build script
  (`scripts/build-occt.sh`), and an `OCCT_DIR` env override that repoints the link at any
  OCCT install prefix. See `README.md` ("Building with the OCCT kernel") and
  `THIRD_PARTY_LICENSES.md`. (This supersedes the earlier dynamic-linking plan in §1; the
  LGPL obligation is met by relink-ability rather than by dynamic linking.)
- A **Help ▸ Licenses** menu item links to `THIRD_PARTY_LICENSES.md`, which reproduces/points
  to the LGPL 2.1 + OCCT exception text and every other dependency's license, satisfying the
  attribution/notice obligations.
- Record the OCCT version in the file (§7.1) to support deterministic recompute (§4.4).
- Kernel errors must be converted into typed Rust errors attached to the failing DAG node —
  the shim catches OCCT C++ exceptions at the boundary and returns error sentinels rather than
  unwinding across FFI.

---

## 11. GUI

### 11.1 Layout
- **Tiled panes only** — avoid floating windows and modals. Use docking/splitting.
- Core panes: 3D viewport, action-DAG/history graph, parameters, feature/constraint
  properties, component/assembly browser.
- **Context pane:** shows the **union** of editable properties for everything currently
  selected (or for the active draw tool — including before the first click — and for
  in-progress draw operations). If selected items disagree on a property, the control
  shows a mixed/indeterminate state; applying a new value sets that property on all
  applicable targets. Draw-tool mode takes precedence over selection when both apply.
- A standard **application menu bar** (File / Edit / View / Help) sits above the
  workspace. Menu items dispatch the shared action layer (§8) so menu, toolbar,
  shortcuts, and scripting stay in sync. The **View** menu contains a **Panes**
  submenu that shows/hides each available pane via a checkbox. (The menu bar is
  drawn in-window rather than as a native OS menu so it appears in screenshot
  regression tests, §9.3, and stays consistent across platforms.)
- **STL export from the GUI:** **File → Export STL…** exports all bodies (via a save
  dialog); right-clicking a **body** row in the Elements pane exports just that body. Both
  mirror the scriptable `bearcad.export_stl` (§8, §9.2).
- **STL import (#70):** **File → Import STL…** (open dialog) reads an STL file — ASCII or
  binary, auto-detected by exact byte-length match against the binary format's
  header+triangle-count framing — and adds it as a new **Body** with no source feature (no
  sketch/extrusion to nest under, so it nests directly under the Elements pane's Document
  root (#87), named after the file). Scriptable via `bearcad.import_stl(path)`. The mesh is
  stored and rendered as-is (no auto-centering/scaling); it participates in STL/STEP export,
  visibility, renaming, and deletion exactly like any other body, but — since it has no
  sketch/distance parameters — can't be edited or merged into by a further extrude the way
  an extrusion-backed body can (#32).
- **STEP export/import (#65/#71):** **File → Export STEP…** / **Import STEP…** (and the
  per-body Elements-pane export). With the OCCT kernel compiled in (`--features occt`, §10),
  a single-body STEP export writes **real BREP** (planar *and* curved surfaces) straight from
  the body's OCCT solid via `STEPControl_Writer`, and import reads **real BREP incl.
  curved/NURBS surfaces** via `STEPControl_Reader`, tessellating the result into a new **Body**
  (nests under the Document root, named after the file). Scriptable via `bearcad.import_step`
  / `bearcad.export_step`.
  - **No-kernel fallback:** builds without OCCT (and the whole-document/multi-body export path,
    plus any body whose geometry isn't kernel-representable) use the hand-rolled `step.rs`
    path — export writes an AP203 `FACETED_BREP` (tessellated triangles), and import reads only
    that same `POLY_LOOP`-bounded planar `FACE_SURFACE` subset. In this mode, STEP files using
    full BREP geometry (`ADVANCED_FACE` with curved/NURBS surfaces, as most CAD tools export)
    are rejected with a clear error rather than approximated. Imported bodies behave like STL
    imports (no analytic face/edge structure to sketch or edit against).
- **Export session commands:** **Help → Export Session Commands…** (also a command-palette
  entry, "Export Session Commands…") writes everything done since the app opened as a
  timestamped, replayable Lua script (the same instructions as `--show-commands`, §9). Useful
  for reproducing a bug by pasting the steps, or for turning an interactively-modeled part into
  a script. The session is always recorded interactively, including the interactive draw/extrude
  tools (#59): committing a rectangle/line/circle/extrusion logs the equivalent declarative
  `bearcad.rect{}`/`line{}`/`circle{}`/`extrude{}` call built from the as-committed geometry (not
  the in-progress drag), so a script-recorded session and a hand-written script produce the same
  document when replayed. Editing an already-committed extrusion isn't yet representable by a
  declarative call, so re-commits from the Edit flow aren't re-logged (a known gap, not a second,
  wrong instruction).
- **Elements pane view modes (#34):** three icon-toggle buttons next to the pane heading switch
  between **List** (the default flat, topologically-sorted view), **Tree** (the real nested
  hierarchy, each level indented farther than its parent — planes/sketches/extrusions/bodies
  nest exactly as the Document root tree does, #87), and **Graph** (a 2D node-link diagram:
  column = tree depth, row = position within that column, width-constrained to the pane so it
  never scrolls horizontally, only vertically). Clicking a node in Graph view selects it like any
  other row; selecting a node highlights its ancestor and descendant nodes/edges with a distinct
  accent color/stroke. This is a per-session UI preference, not saved with the document.

### 11.2 Command palette
- VS Code-style palette listing **context-pertinent** commands. Commands come from the
  shared action layer (§8) so palette, shortcuts, GUI buttons, and scripting stay in sync.

### 11.3 Shortcuts
- Sensible defaults for the most common actions.
- **Every action is rebindable**; custom bindings persist (per §7.2, in-document; global
  defaults in app settings).

### 11.4 Theming
- Light and dark modes, ideally a general theme system.

### 11.5 3D interaction
- Orbit/pan/zoom the 3D rendering; select faces/edges/vertices; manipulate sketches and
  features directly in the viewport.
- **Default viewport bindings** (all rebindable per §11.3):

  | Input | Action |
  |---|---|
  | Right-drag | Orbit the camera |
  | **Shift + right-drag** | Pan the camera (slide the view target in the view plane) |
  | Mouse wheel | Zoom (dolly in/out) |
  | Left-drag (with an active draw tool) | Use the tool, e.g. draw a rectangle on the active plane |
  | **X** | Toggle construction/substantial on the in-progress draw op, or on each constructable selected item |
  | Escape | Cancel the in-progress operation; if none, deactivate the current tool (back to *Select*) |

- **Tooling model:** the viewport has an active **tool** (e.g. *Select*, *Rectangle*).
  *Select* is the default and only orbits/pans/zooms — geometry is created only when a
  drawing tool is active, so navigation never creates geometry by accident. Tools are part
  of the shared action layer (§8) so they appear in the palette and are rebindable.
- **Sketch-mode border (#74):** while a sketch is open, the 3D viewport is outlined in a
  bright orange border — a mode indicator distinct from every other viewport accent color, so
  sketch mode is never mistaken for ordinary 3D navigation at a glance.
- **Selectable hover feedback:** in any tool mode where the user can click to select
  geometry (e.g. picking a reference face or axis for a construction plane), every
  pickable target under the cursor is highlighted before click. The highlight uses a
  distinct accent colour and follows the shape of the target (line stroke, face outline,
  ground crosshair, etc.).
- **Proximity picking:** thin or point-like geometry (lines, endpoints, vertices) must
  be pickable within a screen-space tolerance — the pointer need not land exactly on the
  stroke. Lines use a pixel-radius threshold around the segment and its endpoints; faces
  use a margin around their projected edges. Hover resolution and click picking share the
  same resolver so feedback matches what a click would select.
- **Shape edges:** when a tool accepts a line or axis reference (e.g. construction-plane
  creation), standalone sketch lines and individual edges of shapes (rectangle sides,
  construction-plane borders, etc.) are all valid picks. Shape edges take precedence over
  the shape's face when the cursor is near the edge.
- **3D body edges (#31):** any edge of any 3D body — not just 2D sketch geometry — is a valid
  axis reference for a construction plane, including STL/STEP-imported bodies. An edge here is
  a *feature* edge of the body's triangle mesh (a mesh boundary, or a crease between two
  non-coplanar triangles) — the same extraction `ShadingMode::Wireframe` uses to draw a body's
  edges — so it works uniformly for any body regardless of how it was created, without needing
  an analytic profile.
- **Global axes:** the origin X/Y/Z triad is pickable as an axis reference when creating
  construction planes. Axis gizmo handles show a hover affordance (bright ring and thicker
  stroke) so the user can see which handle will be grabbed on click.
- **Gizmos draw through bodies:** manipulation gizmos and their grab handles (plane-making,
  extrusion offset/angle, and any future gizmo) render with depth testing disabled, so they
  stay visible and clickable even when a body would otherwise occlude them.
- **View-cube HUD settings popup (#33):** where the projection (orthographic/perspective)
  toggle button used to sit (bottom-left of the view-cube HUD), a gear icon instead opens a
  popup with two icon-button rows (words are avoided in favour of icons + tooltips):
  - **Projection** — the same orthographic/perspective choice the old button toggled
    directly; the active one is highlighted, click the other to switch.
  - **Shading** — how committed bodies render, one of:
    - *Wireframe*: edges only, no fill.
    - *Transparent solid*: translucent fill with edges visible through it.
    - *Solid*: opaque fill, no edge overlay (the default — today's existing look).
    - *Solid + wireframe*: opaque fill plus an edge overlay that stays visible through the
      body, using the same depth-test-disabled technique as gizmos drawing through bodies
      (above) so the far-side edges aren't occluded by the near faces.
    - *Realistic (#83)*: ambient + diffuse + specular (Blinn-Phong-ish) lighting instead of
      `Solid`'s flat/Lambert-ish term, giving bodies a matte/satin "painted object" look with a
      camera-dependent specular highlight. Still flat-shaded per triangle (no shared vertex
      normals exist on the mesh), so it reads as faceted rather than smoothly lit. No
      materials/textures yet — every body renders with the same fixed gloss; per-body/per-face
      materials are future work.

  Both rows are backed by `Camera` state (a viewport display preference, alongside
  projection mode — not saved model geometry) and are fully scriptable:
  `bearcad.ui.toggle_projection()` / `bearcad.ui.view("orthographic" | "natural")` for
  projection, and `bearcad.ui.shading("wireframe" | "transparent" | "solid" |
  "solid_wireframe" | "realistic")` for shading.

---

## 12. Technical drawings & printable schematics

BearCAD supports **2D technical drawings** derived from 3D models — dimensioned, annotated
sheets suitable for printing/manufacturing.

### 12.1 Model
- A **drawing** is a first-class document object (alongside components/assemblies),
  consisting of one or more **sheets** at standard paper sizes (ISO A-series, ANSI A–E)
  with a title block.
- A sheet contains **views** placed on it: orthographic projections (front/top/side/
  iso), section views, detail views, and a configurable projection convention (first- vs
  third-angle).
- Views are **associative**: each view references a component/assembly and recomputes
  when the source model changes (the reference is a DAG dependency edge, §4). Views have
  a scale (e.g. `1:2`), independent of model units.

### 12.2 Annotations
- Dimensions (linear, aligned, angular, radial/diameter), driven from real geometry and
  shown with the document's units; tolerances; leaders/notes; centerlines/center marks;
  surface-finish and datum/GD&T symbols (GD&T depth: **TBD**); a bill of materials /
  parts list for assemblies.

### 12.3 Output
- **Print** and **export to PDF** (vector) and **SVG/DXF** for the 2D content. PDF/SVG/DXF
  drawing export must be available from the CLI as well (§9), consistent with the
  GUI-parity principle.
- Drawing definitions (sheets, views, annotations, placements) are persisted in the
  `.bearcad` (§7); like geometry, computed view projections (HLR vector output) are **cached**
  in the file and invalidated when the source model changes, so drawings open fast (cache
  strategy mirrors §4.4). HLR is expensive, so caching it is especially important here.

### 12.4 Library notes
- Hidden-line removal / projected-edge generation comes from OCCT (e.g. its HLR
  facilities). DXF/SVG/PDF writers must be license-audited per §1.

---

## 13. Out of scope for v1 (record for later)
- Variable-radius fillets, simulation/FEA, rendering beyond basic shaded/snapshot,
  collaboration/multi-user, cloud sync, plugin marketplace. (Adjust as priorities change.)
- Technical drawings are **in scope** (§12). If schedule pressure arises, the minimum
  drawing v1 is: orthographic + iso views, linear/angular/radial dimensions, a title
  block, and PDF export.

---

## 14. Open items (TBD) — must be resolved before building the relevant area
1. Topological persistent-naming algorithm (§4.5).
2. ~~Constraint solver implementation choice (§6.3).~~ **Resolved:** native Rust LM solver.
3. Canonical internal units & full math function library (§5.2–5.3).
4. Full assembly joint catalog (§2.3).
5. OCCT binding strategy and the exact C++ shim surface (§10).
6. Lua API module layout and function signatures (§8).
7. Per-feature `payload` encoding in the SQLite schema (§7.3).
8. GD&T symbol coverage and standard for technical drawings (§12.2).
9. DXF/SVG/PDF writer library selection and licensing for drawing export (§12.3–12.4).
10. Geometry cache granularity — per-feature (floor) vs. per-body and/or tessellation-LOD
    entries, and the BREP/mesh blob encoding (§4.4, §7.3).
