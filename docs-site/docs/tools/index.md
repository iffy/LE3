---
slug: /tools
sidebar_position: 1
title: Tools & Navigation
---

# Tools & Navigation

BearCAD's 3D viewport has an active **tool** at all times — **Select** is the default and only
orbits/pans/zooms, so navigating the camera never accidentally creates geometry. Switching to a
drawing tool (Rectangle, Line, Circle, …) is what enables clicking in the viewport to create or
edit geometry. Tools are part of the shared action layer, so every tool is also available from
the command palette, the toolbar, a keyboard shortcut, and the Lua scripting API
(`bearcad.ui.tool("rectangle")`).

## Tool reference

| Tool | Shortcut | What it does |
|---|---|---|
| [Select](/tools/select) | — | Orbit/pan/zoom and pick geometry; the default tool. |
| [Sketch](/tools/sketch) | `S` | Pick a face (or the ground plane) to enter sketch mode. |
| [Rectangle](/tools/rectangle) | `R` | Draw a rectangle by two corners. |
| [Line](/tools/line) | `L` | Draw connected line segments (polylines), straight or curved. |
| [Circle](/tools/circle) | `O` | Draw a circle by center and radius/diameter. |
| [Construction Plane](/tools/construction-plane) | `P` | Create a datum plane from a face or axis. |
| [Dimension](/tools/dimension) | `D` | Add or edit a distance/length/angle constraint. |
| [Constraint](/tools/constraint) | `C` | Apply geometric constraints (parallel, coincident, …). |
| [Extrude](/tools/extrude) | `E` | Turn one or more coplanar sketch faces into a solid body. |
| [Chamfer](/tools/chamfer) | `K` | Truncate a sketch corner with a straight cut. |
| [Fillet](/tools/fillet) | `F` | Round a sketch corner with a bezier-approximated arc. |

Every shortcut above is the platform-independent single-letter binding shown on the toolbar
buttons (all shortcuts are rebindable; see [Navigation](/tools/navigation) for the camera/mouse
bindings, which are separate from the tool-select letters above).

## Common tool UX patterns

A few conventions apply across most of the drawing tools:

- **Click to start, move to preview, click/type to finish.** Rectangle, Line, and Circle all
  follow "click first point → move mouse for a live preview → click (or type a dimension and
  press Enter) to commit."
- **On-screen dimension typing.** While drawing, you can type a number directly to constrain the
  in-progress shape (width/height, length, radius/diameter). **Tab** cycles between input fields;
  **Enter** commits the shape.
- **Escape cancels, then exits.** Pressing **Esc** cancels the current in-progress draw
  operation; pressing it again (with nothing in progress) deactivates the current tool and
  returns to **Select**.
- **`X` toggles construction.** Press **X** to mark the in-progress draw operation — or each
  currently-selected constructable item — as construction geometry (dashed, non-solid reference
  geometry) instead of substantial geometry.
- **Context pane.** Whatever tool is active (or whatever is selected, if no draw tool is active)
  drives the contents of the Context pane, which shows the union of editable properties for the
  current tool/selection.

See [Navigation](/tools/navigation) for camera controls, the view-cube HUD, and sketch mode's
viewport border.
