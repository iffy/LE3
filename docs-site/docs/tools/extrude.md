---
sidebar_position: 10
title: Extrude
---

# Extrude

**Shortcut:** `E`

Extrude turns one or more coplanar sketch faces (closed rectangle, circle, or polygon profiles)
into a solid **Body**. Click coplanar faces to toggle inclusion (hover-highlighted), then drag the
normal gizmo or type a distance — expressions and parameter references work — to set the depth
(positive or negative). A live **semi-transparent** preview solid updates as you type. **Enter**
commits, **Esc** cancels; double-click or right-click → **Edit** re-opens a committed extrusion to
change its faces or length.

- The gizmo handle floats a little above the solid's top face rather than sitting on it.
- Typing a digit while the tool is active focuses the distance field and overwrites its value.
- While an extrusion is being edited, its committed body is hidden — only the semi-transparent
  ghost preview is shown, so the preview (not the old solid) reflects the in-progress edit.

An **Extrusion** is a first-class feature element: its own row in the Elements pane, nameable,
undoable. It generates a mesh (a prism per rectangle/polygon profile, a cylinder per circle
profile) and produces a **Body** that depends on it — the body nests under the extrusion in the
Elements pane, and is removed if the extrusion is deleted. The extrusion itself nests under the
sketch it was built from.

## Extrude-to-object

During a gizmo drag, hovering a vertex/face/plane snaps the depth to that object; on release, the
extrusion is constrained to it. The effective depth is then derived from the target's extended
plane (a vertex's perpendicular plane, or where the extrusion axis meets a face/construction
plane) and recomputes if that geometry moves later. A free gizmo drag with no target leaves a
plain unconstrained distance. The live ghost preview reflects the snapped target immediately
while still dragging, not just after release — so extruding to a slanted or irregular target
shows the actual resulting shape (e.g. a slanted top cap) rather than a generic blind extrude.

## Joining an existing body

A `Body`'s source is one or more extrusions. Extruding from a sketch on an existing body's face
(a cap or side face) **defaults to joining that body** instead of creating a new one — the
context pane shows "Add to `<body>`" vs. "New body" while extruding, or while editing an
extrusion, to override the choice (editing can also split a merged extrusion back out into its
own body). Deleting one extrusion of a multi-extrusion body only drops that extrusion's
contribution; the body survives as long as at least one extrusion remains.

## Overlapping shapes: intersection and difference regions

If exactly two coplanar shapes in a sketch overlap with nonzero area (and no third shape also
overlaps that pair), clicking inside their combined footprint toggles the specific region under
the cursor instead of a whole shape: their shared **intersection**, or one shape **minus** the
other. This lets you extrude, say, only the overlap of a rectangle and a circle, or the circle
with the rectangle's overlap cut out — without a separate boolean-operation UI, since it's just
where you click. Toggling both whole shapes (rather than clicking inside the overlap) still
unions them, the same as it always has — multi-face selection already supports that.

This only applies to exactly two overlapping shapes at a time; a sketch where three or more
shapes all overlap the same region falls back to plain whole-shape picking. The computed region
also has to reduce to a single simple polygon (no separate disjoint pieces, and no hole — e.g.
subtracting a shape that's strictly interior to another would leave a hole, which isn't
representable here) or picking likewise falls back to the whole shape.

## Scripting

```lua
bearcad.new()
bearcad.rect{ width = 80, height = 50, name = "Base" }
bearcad.extrude{ rect = 0, distance = 20, name = "Boss" }

-- Extrude an explicit set of closed-loop lines (rather than relying on auto-detected polygons):
bearcad.extrude{ polygon = {0, 1, 2}, distance = 6 }

-- Multiple profiles, and joining an existing body explicitly:
bearcad.extrude{ rects = {0, 1}, distance = 10, body = "merge" }

-- Extrude just the intersection of a rect and a circle:
bearcad.extrude{ boolean = { op = "intersection", a = {rect = 0}, b = {circle = 0} }, distance = 5 }

-- Or the rect minus the circle (`op = "difference"` is `a` minus `b`):
bearcad.extrude{ boolean = { op = "difference", a = {rect = 0}, b = {circle = 0} }, distance = 5 }
```

`body = "merge"` joins the face's body if there is one; omitted (or any other value) always
creates a new body, matching the declarative/OpenSCAD-style default of "each call produces new
geometry unless you say otherwise."

See [Sketch → scripting](./sketch.md#scripting) for `bearcad.begin_sketch{ kind = "extrude_cap" |
"extrude_side", ... }`, which lets a script sketch on a solid's face the same way a user could by
clicking it.
