---
sidebar_position: 2
title: Select
---

# Select

**Shortcut:** none (it's the default tool — pressing **Esc** with nothing in progress always
returns to Select).

Select is the default viewport tool: it orbits/pans/zooms the camera and lets you click to pick
geometry, but it never creates anything. This split — navigation only happens in Select, drawing
only happens in a drawing tool — means moving the camera around can never accidentally create
geometry.

## What you can pick

- **Sketch points** — line endpoints, rectangle corners, circle centers.
- **Lines and rectangle edges.**
- **Faces** — including the planar cap and side faces of extruded bodies, and construction
  planes.
- **Bodies** and other hierarchy elements, via the Elements pane as well as the viewport.

Point picks take precedence near vertices (within a screen-space pick tolerance), so clicking
close to a corner selects the point rather than the edge it sits on. Hovering any pickable target
highlights it before you click, using a distinct accent color that follows the shape of the
target (line stroke, face outline, ground crosshair, etc.) — hover and click share the same pick
resolution logic, so what highlights is exactly what a click would select.

## Selecting for other tools

Select's picks feed directly into other tools and panes:

- The **Constraint** tool needs points/lines/edges selected before its buttons enable.
- The **Dimension** tool needs a line (or two lines, for an angle) selected.
- Right-clicking a sketch vertex where exactly two plain lines meet offers **Convert to bezier
  curve** (or **Straighten curve** on an existing curve) directly from Select.
- The Elements pane, Context pane, and Parameters table all react to the current selection.
- The Elements pane has three view modes, toggled via icon buttons next to its heading: **List**
  (flat, the default), **Tree** (nested, each level indented under its parent), and **Graph** (a
  2D node-link diagram). In Graph view, clicking a node selects it like any other row, and
  selecting a node highlights its ancestor/descendant nodes and edges.

## Scripting

```lua
bearcad.select(bearcad.find("Main box"))
bearcad.clear_selection()

-- Point-level selection (rather than the whole element):
bearcad.select{ kind = "line", index = 0, ["end"] = "end" }
bearcad.select({ kind = "rect", index = 0, corner = 2 }, true) -- additive
```

See [Scripting → Point-level selection](/docs/scripting/point-selection) for the full picture,
including how this powers joining two line endpoints purely from a script.
