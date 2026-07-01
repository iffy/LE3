---
sidebar_position: 13
title: Navigation
---

# Navigation

How to move around BearCAD's 3D viewport, read the view-cube HUD, and recognize sketch mode.

## Camera controls

All of these are rebindable (every action is part of the shared action layer), but these are the
defaults:

| Input | Action |
|---|---|
| Right-drag | Orbit the camera |
| **Shift + right-drag** | Pan the camera (slide the view target in the view plane) |
| Mouse wheel | Zoom (dolly in/out) |
| Left-drag (with an active draw tool) | Use the tool, e.g. draw a rectangle on the active plane |
| **X** | Toggle construction/substantial on the in-progress draw op, or on selected constructable items |
| **Escape** | Cancel the in-progress operation; if none, deactivate the current tool (back to Select) |

**Select** is the only tool that navigates the camera on left-drag — every other tool's left-drag
is drawing/manipulation, which is why switching tools deliberately (rather than clicking near
geometry by accident) is how BearCAD avoids creating geometry while you're just looking around.

## View-cube HUD

The view-cube HUD (top corner of the viewport) offers standard views and a **Home** view.

- Click a cube face/edge/corner to snap to that standard view (animated).
- **Home** returns to the saved home view.

### Settings popup (gear icon)

Where a projection toggle button used to sit (bottom-left of the view-cube HUD), a **gear icon**
opens a popup with two icon-button rows (icons + tooltips, no words):

- **Projection** — orthographic vs. perspective. The active one is highlighted; click the other
  to switch.
- **Shading** — how committed bodies render:
  - *Wireframe* — edges only, no fill.
  - *Transparent solid* — translucent fill with edges visible through it.
  - *Solid* — opaque fill, no edge overlay (the default).
  - *Solid + wireframe* — opaque fill plus an edge overlay that stays visible through the body
    (edges on the far side aren't occluded by near faces), using the same depth-test-disabled
    technique gizmos use to draw through bodies.
  - *Realistic* — ambient + diffuse + specular (Blinn-Phong-ish) lighting instead of Solid's
    flat shading, giving bodies a matte/satin look with a camera-dependent specular highlight.
    Still flat-shaded per triangle (faceted, not smoothly lit) and every body uses the same
    fixed gloss — no per-material/textures yet.

Both rows are viewport display preferences (not saved model geometry) and are fully scriptable:

```lua
bearcad.ui.toggle_projection()
bearcad.ui.view("orthographic")   -- or "natural" for perspective
bearcad.ui.shading("wireframe")   -- "transparent" | "solid" | "solid_wireframe" | "realistic"
```

## Standard views and camera scripting

```lua
bearcad.ui.view("front")               -- standard view, waits for the animation
bearcad.ui.view("edge", "front_top")   -- a view-cube edge
bearcad.ui.view_home()                 -- return to the home view
bearcad.ui.orbit(dx, dy)
bearcad.ui.pan(dx, dy)
bearcad.ui.wheel(scroll)
```

## Sketch-mode border

While a sketch is open (see [Sketch](./sketch.md)), the 3D viewport is outlined in a bright
**orange border** — a mode indicator distinct from every other viewport accent color, so sketch
mode is never mistaken for ordinary 3D navigation at a glance. Camera controls (orbit/pan/zoom)
keep working normally while a sketch is open; the border is purely an indicator, not a
navigation restriction.

## Hover and picking

- **Selectable hover feedback** — in any tool mode where you can click to select geometry (e.g.
  picking a reference face or axis for a construction plane), every pickable target under the
  cursor highlights before you click, using a distinct accent color that follows the shape of the
  target.
- **Proximity picking** — thin or point-like geometry (lines, endpoints, vertices) is pickable
  within a screen-space tolerance; you don't have to land exactly on the stroke. Hover and click
  share the same pick resolver, so what highlights is exactly what a click would select.
- **Shape edges take precedence** over a shape's own face when the cursor is near the edge, for
  tools that accept a line/axis reference.
- **Global axes** — the origin X/Y/Z triad is pickable as an axis reference when creating
  construction planes; its handles show a hover affordance (bright ring, thicker stroke).
