---
sidebar_position: 4
title: Rectangle
---

# Rectangle

**Shortcut:** `R`

Draws a rectangle as four constrained lines. Click to fix the first corner, move the mouse to
position the opposite corner (a live preview follows the cursor), then click — or type width and
height and press **Enter** — to commit.

- **Tab** cycles between the width and height input fields while drawing.
- **Enter** commits the rectangle.
- **Esc** cancels the in-progress rectangle.
- **X** toggles construction geometry on the in-progress rectangle (or on selected rectangles),
  rendering it dashed/dimmed as reference geometry instead of substantial geometry.

A committed rectangle's four corners are numbered **0–3 counterclockwise starting at its `(x, y)`
origin corner** — this numbering is what the [Constraint](./constraint.md) tool and
`bearcad.select{ kind = "rect", corner = N }` both use.

## Scripting

The declarative one-call form enters a ground-plane sketch automatically if none is open:

```lua
bearcad.new()
bearcad.rect{ width = 80, height = 50, name = "Main box" }
```

Position it explicitly with `x`/`y`, or draw it on a specific sketch first:

```lua
bearcad.begin_sketch("construction_plane", 0)
bearcad.rect{ x = 10, y = 10, width = 80, height = 50, name = "Main box" }
```

The simulated-interaction equivalent (only needed when the UI interaction itself is the point —
see [Scripting → declarative vs. UI](/scripting#namespace-split)):

```lua
bearcad.ui.tool("rectangle")
bearcad.ui.click_ground(0, 0)
bearcad.ui.move_ground(80, 50)
bearcad.ui.key("enter")
```

A rectangle's edges can be referenced individually, e.g. for a dimension constraint or as an
extrusion face:

```lua
bearcad.select{ kind = "rect", index = 0, edge = "bottom" }
```
