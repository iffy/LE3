---
sidebar_position: 5
title: Line
---

# Line

**Shortcut:** `L`

Click to fix the first endpoint, move the mouse for direction and length (a live preview
follows), then click — or type a length and press **Enter** — to commit the segment.

## Polyline chaining

The Line tool draws **connected polylines**: after a segment is committed, the next segment
starts automatically at that endpoint (coincident with it), so a polygon is drawn with successive
clicks. Chaining stops when a segment's end snaps onto an existing vertex, which closes or joins
the shape. Press **Esc** to finish the polyline early, keeping the segments already drawn.

Any set of plain lines that connects end-to-end into a closed loop (via `Coincident` constraints
on their endpoints) is itself a usable face — filled the same as a rectangle or circle profile,
pickable for sketching-on-face, and extrudable. A line shared by two loops (e.g. a rectangle
split by a diagonal) yields multiple selectable polygon faces.

## Snapping

While drawing, the cursor snaps to nearby vertices, line midpoints, and lines — vertices take
priority, then midpoints, then anywhere on a line. Leaving a point on a snap adds the implied
constraint (coincident for a vertex/on-line snap, midpoint for a midpoint snap). A ring marks the
active snap; snapping is toggleable from the context pane.

Hovering a vertex while drawing also arms its incident edges as **extension guides** — pulling
away snaps the point onto the infinite extension of those edges (within a perpendicular
tolerance), shown with a dashed guide line. Leaving the point there adds a point-on-line
coincidence, so e.g. touching a rectangle corner lets the next point be placed in line with one of
its sides.

Touching a line's **midpoint** similarly arms a **normal-at-midpoint guide**: pulling away snaps
the point onto the infinite line perpendicular to that edge, through its midpoint, again shown
with a dashed guide line. Leaving the point there invents a dashed construction line from the
edge's midpoint out to the placed point, pinned perpendicular to the edge and through its
midpoint — since there's no single constraint for "perpendicular through a midpoint," this is
built from three ordinary constraints (`Midpoint`, `Perpendicular`, and a point-on-line
`Coincident`) rather than a new one.

## Bezier curves

A curved line is a plain `Line` with an optional pair of cubic tangent-handle control points —
its two endpoints stay ordinary constrainable vertices, so coincidence/distance constraints,
dragging, undo, and persistence all work exactly like a straight line. The Line tool always
places points with plain click-click; curves come from two toggles instead of a drag gesture:

- **Curve mode (`B`, default off).** Shown as a checkbox in the Context pane (above
  Construction) while the Line tool is active. When on, the *next* point placed gets bezier
  handles on both sides of it — or just the outgoing side if it's a fresh chain's starting
  point, since there's no previous segment yet to derive a tangent from. Concretely: committing
  the *n*-th point of a chain (n ≥ 3) retroactively smooths the shared vertex between the
  (n-2)→(n-1) and (n-1)→n segments, so a segment only curves once a further point makes its
  tangent meaningful. The toggle persists across chained segments, like Construction.
- **Tangent constraint (`T`, default on).** While curve mode is on, controls *how* each shared
  vertex is curved. On: both sides' handles are mirrored/tangent-continuous (the same smoothing
  as **Convert to bezier curve** below). Off: the previous segment's handle is left alone and the
  new segment gets an independent "corner" handle a third of the way along its own chord — a
  barely-curved starting shape meant to be reshaped by hand.
- **Live preview.** As the mouse moves before the next point is placed, the in-progress segment
  previews its live curve toward the cursor, and — when curve mode smooths a shared vertex — the
  previous segment's end visibly bends to stay consistent with it, updating every frame.
- **On a selection, retroactively.** With the Select tool, in sketch mode, with one or more
  vertices selected: `B` toggles the selected vertex(es) between curved and straight; `T` toggles
  between tangent-continuous and independent handles at the vertex. Vertices that don't join
  exactly two plain lines are skipped.
- **Draggable handles on a committed curve.** A curved line shows its two tangent handles (while
  its sketch is open) as small discs with dashed guides back to their endpoint — drag one to
  reshape the curve live. Clicking (rather than dragging) a handle selects it; pressing
  Delete/Backspace, or right-clicking it and choosing **Delete handle**, straightens the line. A
  curve is either both handles or neither, so there's no partial/one-handle state.
- **Right-click a vertex.** Right-clicking a vertex where exactly two plain lines meet offers
  **Convert to bezier curve**, smoothing the joint into a tangent-continuous pair of curves. The
  reverse, **Straighten curve**, is offered when right-clicking an existing curved line.

A curved line is faceted into 24 straight sub-segments for rendering, hit-testing, and — when
part of a closed polygon loop — extrusion tessellation. Side walls swept from a curved profile
edge are correspondingly multi-faceted rather than a single flat quad; sketching on the side wall
of a curved extrusion edge is not currently supported. Inference/extension snapping onto a curved
line still uses its straight chord, not the true curve.

## Scripting

```lua
bearcad.new()
bearcad.line{ length = 80, name = "Guide" }                 -- length/angle form
bearcad.line{ x = 0, y = 0, x1 = 10, y1 = 0, name = "Edge" } -- explicit endpoints

-- A curved line — same tangent-handle pairs the draggable-handle UI edits:
bearcad.line{
  x = 0, y = 0, x1 = 10, y1 = 0,
  bezier = { {3, 4}, {7, 4} },
  name = "Curve",
}
```

Reference a line's endpoints individually with `["end"] = "start"|"end"` — useful for closing a
polygon loop purely from a script, see
[Scripting → point-level selection](/docs/scripting/point-selection):

```lua
bearcad.select{ kind = "line", index = 0, ["end"] = "end" }
bearcad.select({ kind = "line", index = 1, ["end"] = "start" }, true)
bearcad.add_geometric_constraint("coincident")
```
