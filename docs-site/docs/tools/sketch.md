---
sidebar_position: 3
title: Sketch
---

# Sketch

**Shortcut:** `S`

The Sketch tool picks a face to draw on. Click a face — a construction plane, or the planar cap
of an extruded body — and BearCAD enters **sketch mode** on that face's plane; the Line,
Rectangle, and Circle tools then draw into that sketch.

## What's pickable as a sketch face

- **Construction planes**, including the default ground (XY) plane.
- **The planar cap faces of an extruded body** — the base and offset ends of each extruded
  profile. The new sketch's frame inherits the profile's in-plane axes, offset along the
  extrusion normal, and behaves exactly like any other sketch. Such a sketch (and anything built
  from it) nests under, and depends on, the extrusion whose face it sits on.
- A solid cap occludes the construction plane behind it for picking; when several faces project
  onto the cursor (e.g. near and far faces of a solid), the one nearest the camera wins, so you
  never pick a face that's hidden behind the body.

Entering a sketch reorients the camera head-on to the face. For a near-vertical face (like a side
wall) the view keeps world-up (+Z) toward the top of the screen, so the ground stays at the
bottom and orbiting still feels normal instead of rolling sideways.

## While a sketch is open

- The viewport is outlined in a bright **orange border** — a mode indicator distinct from every
  other viewport accent color, so you can never mistake sketch mode for ordinary 3D navigation at
  a glance. See [Navigation](./navigation.md#sketch-mode-border).
- Line, Rectangle, and Circle draw into the active sketch's plane.
- Press **Esc** (with nothing in progress) to leave the sketch and return to Select.
- **Sketching on a body's own cap/side face:** its corners and edges become valid
  [Constraint](./constraint#selecting) targets, so you can pin sketch geometry to that face
  directly — see [Scripting → face vertex/edge selection](/docs/scripting/point-selection#selecting-a-faces-own-vertex-or-edge).

## Re-opening and leaving sketches

- Clicking an existing sketch face with the Sketch tool re-opens that sketch for further editing.
- **Esc** exits the currently open sketch.

## Scripting

```lua
-- Start a sketch on a construction plane by index:
bearcad.begin_sketch("construction_plane", 0)

-- Re-open an existing sketch, or leave the current one:
bearcad.open_sketch(0)
bearcad.exit_sketch()

-- Sketch on a solid's face (cap or side wall) — useful for testing without a mouse:
bearcad.begin_sketch{
  kind = "extrude_cap",
  extrusion = 0,
  profile = "rect",
  profile_index = 0,
  top = true,
}
```

Most scripts never need to call `begin_sketch` explicitly: the declarative geometry helpers
(`bearcad.rect{}`, `bearcad.line{}`, `bearcad.circle{}`) enter a ground-plane sketch automatically
if none is open.
