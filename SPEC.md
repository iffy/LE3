# LE3 — Specification

LE3 is an on-device, parametric CAD program comparable to Autodesk Fusion, FreeCAD,
and OpenSCAD. This document is the implementation specification: it should contain
enough detail for an engineer to build LE3 without further design decisions. Where a
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
| Save file | **SQLite**, extension `.le3` | Schema in §7. |
| License | **MIT OR Apache-2.0** (dual) | LE3's own code is permissively licensed. OCCT is LGPL and MUST be **dynamically linked** so the permissive license is preserved; bundle the LGPL text and OCCT's. Audit STEP/3MF/AMF library licenses for the same constraint. |

### 1.1 Platforms

Must build and run on **macOS, Linux, and Windows**, producing a single self-contained
executable per platform (kernel and other native libs may be dynamically linked but must
be bundled with the distributable). The executable launches the GUI by default and acts
as a CLI when given a subcommand (see §9).

---

## 2. Core concepts and domain model

### 2.1 Document

A document is one `.le3` file. A document contains:

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
- Sketch entities: line, arc, circle, ellipse, spline, point, and construction-geometry
  variants. Convenience primitives (e.g. **rectangle**, drawn as four constrained lines)
  may be offered as tools that emit the underlying entities.
- Sketches are fully constraint-driven (see §6).

### 3.2 Solid creation from sketches
- **Extrude** — blind, symmetric, to-object, with optional draft angle.
- **Revolve** — about an axis, full or partial angle.

### 3.3 Combining solids
- **Boolean**: union, cut (subtract), intersect.

### 3.4 Modifying solids
- **Fillet** and **Chamfer** on selected edges (constant radius/distance for v1; variable
  is a stretch goal).
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

LE3 replaces Fusion's linear timeline with a **directed acyclic graph of actions**. This
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
  (the full history lives in the `.le3`; see §7).
- The history is a **commit graph**: each user-visible change creates a new state. Undo
  moves to the parent state; redo moves forward. Because history is a graph (branches
  allowed) rather than a line, redo may present multiple forward branches; the UI MUST
  let the user choose among them.
- Editing the *value* of an existing feature/parameter does **not** destroy downstream
  work — it re-evaluates dependents (§4.4). This is the key difference from a linear
  timeline: rolling "back" to edit a node does not discard later, independent nodes.

### 4.4 Evaluation, caching & recompute
- The **action DAG is the source of truth**; evaluated geometry is **derived and cached**.
  Evaluated geometry **is persisted in the `.le3`** so files open fast — open should
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

### 5.3 Units
- Strong unit support with mixed units. `3mm + 2in` is valid and evaluates correctly.
- Every component has **default units**; a bare number inherits the contextually relevant
  default unit.
- Units are dimension-checked: adding a length to an angle is an error.
- Supported unit families for v1: length (mm, cm, m, in, ft), angle (deg, rad). Extend as
  needed.
- Internal canonical storage units: **TBD** (recommend millimeters for length, radians for
  angle), but the stored expression text is always preserved.

---

## 6. Constraints

LE3 has a geometric **constraint solver** supporting both 2D (sketch) and 3D constraints,
modeled on SolveSpace (https://solvespace.com).

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
- A numeric constraint solver resolves a constraint system to satisfy all constraints.
- The UI must report **under-** and **over-constrained** states and indicate conflicting
  constraints.
- Solver choice: **TBD** (candidates: port SolveSpace's solver approach, or a
  Newton/least-squares solver over the DOF system). Must be deterministic for headless use.

---

## 7. File format (`.le3` / SQLite)

A `.le3` is a SQLite database. The schema below is the starting point; refine during
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
- On open, LE3 applies any migrations whose id is newer than the file's latest applied
  migration. A file from a newer LE3 than the running binary must be detected and refused
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
- `export` — export a `.le3` to `.3mf`, `.stl`, `.obj`, `.amf`, or `.step`/`.stp`.
- `run` — execute a Lua script headless against a new or existing `.le3`.
- `render` — render the model to an image (e.g. PNG) from a specified camera.
- `set` / parameter override + re-export — override named parameters from the command line
  and export, enabling part families from one file.
- `import` / `convert` — import STEP/STL/etc. into a `.le3`, or convert between formats.

The command set is expected to **grow over time** toward full GUI parity. New GUI actions
should be added to the shared action layer so they become available headlessly by default.

### 9.2 Export formats (required)
`.3mf`, `.stl`, `.obj`, `.amf`, `.step`/`.stp`. STEP via OCCT; mesh formats via OCCT
tessellation + writers (or dedicated libraries — license-audited per §1).

### 9.3 Instruction scripts (for automation & testing)

**Directive:** The app should be fully scriptable. One must be able to run the app with a set of instructions (from a file) and the app must open and run each of the instructions. One must be able to export a screenshot of how the app looks as one of the instructions. This can then be leveraged for testing.

The application must be fully scriptable via a file containing a sequence of instructions.

- Invocation: `le3 <script-file>` or `le3 --script <script-file>` (or equivalent).
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

- Integrate OCCT via Rust FFI. Either use/extend an existing binding crate or generate a
  thin C++ shim exposing only the operations LE3 needs (sketch profiles, prism/revol,
  boolean, fillet/chamfer, shell, sweep/loft, STEP/mesh I/O, tessellation). Binding
  strategy: **TBD**, but isolate all `unsafe`/FFI behind a safe Rust `kernel` module.
- OCCT must be **dynamically linked** (license, §1) and bundled in each platform package.
- Record the OCCT version in the file (§7.1) to support deterministic recompute (§4.4).
- Kernel errors must be converted into typed Rust errors attached to the failing DAG node.

---

## 11. GUI

### 11.1 Layout
- **Tiled panes only** — avoid floating windows and modals. Use docking/splitting.
- Core panes: 3D viewport, action-DAG/history graph, parameters, feature/constraint
  properties, component/assembly browser.
- A standard **application menu bar** (File / Edit / View / Help) sits above the
  workspace. Menu items dispatch the shared action layer (§8) so menu, toolbar,
  shortcuts, and scripting stay in sync. The **View** menu contains a **Panes**
  submenu that shows/hides each available pane via a checkbox. (The menu bar is
  drawn in-window rather than as a native OS menu so it appears in screenshot
  regression tests, §9.3, and stays consistent across platforms.)

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
  | Escape | Cancel the in-progress operation; if none, deactivate the current tool (back to *Select*) |

- **Tooling model:** the viewport has an active **tool** (e.g. *Select*, *Rectangle*).
  *Select* is the default and only orbits/pans/zooms — geometry is created only when a
  drawing tool is active, so navigation never creates geometry by accident. Tools are part
  of the shared action layer (§8) so they appear in the palette and are rebindable.

---

## 12. Technical drawings & printable schematics

LE3 supports **2D technical drawings** derived from 3D models — dimensioned, annotated
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
  `.le3` (§7); like geometry, computed view projections (HLR vector output) are **cached**
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
2. Constraint solver implementation choice (§6.3).
3. Canonical internal units & full math function library (§5.2–5.3).
4. Full assembly joint catalog (§2.3).
5. OCCT binding strategy and the exact C++ shim surface (§10).
6. Lua API module layout and function signatures (§8).
7. Per-feature `payload` encoding in the SQLite schema (§7.3).
8. GD&T symbol coverage and standard for technical drawings (§12.2).
9. DXF/SVG/PDF writer library selection and licensing for drawing export (§12.3–12.4).
10. Geometry cache granularity — per-feature (floor) vs. per-body and/or tessellation-LOD
    entries, and the BREP/mesh blob encoding (§4.4, §7.3).
