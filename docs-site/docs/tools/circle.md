---
sidebar_position: 6
title: Circle
---

# Circle

**Shortcut:** `O`

Click to fix the center, move the mouse to set the radius (a live preview follows), then click —
or type a diameter and press **Enter** — to commit. The on-screen dimension field is a
**diameter** input, matching the toolbar/context-pane label.

- **Esc** cancels the in-progress circle.
- **X** toggles construction geometry on the in-progress circle (or on selected circles).

A committed circle behaves like a closed profile the same way a rectangle does: it's pickable as
a sketch face and extrudable, producing a cylinder.

## Scripting

```lua
bearcad.new()
bearcad.circle{ x = 10, y = 5, r = 12, name = "Hole" }   -- radius
bearcad.circle{ diameter = 30 }                          -- or diameter
```

A circle's whole-element selection targets the circle itself; to target just its **center
point** (e.g. to constrain it coincident with something else), pass `point = true`:

```lua
bearcad.select{ kind = "circle", index = 0, point = true }
```
