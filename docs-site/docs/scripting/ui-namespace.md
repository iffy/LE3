---
sidebar_position: 3
title: The bearcad.ui.* namespace
---

# The `bearcad.ui.*` namespace

Everything under `bearcad.ui` simulates a real user driving the GUI — mouse motion and clicks,
keyboard input, camera drags, tool selection, showing/hiding panes, and running palette commands.
Reach for it when the UI interaction itself is what you're testing or automating (e.g. "does
click-dragging the Line tool produce a curve"), not for ordinary modeling — use the
[declarative API](./declarative-modeling) for that.

## Tools and synthetic input

```lua
bearcad.ui.tool("rectangle")            -- select, line, circle, sketch, rectangle, ...
bearcad.ui.click_ground(0, 0)           -- click on the active sketch plane, in millimetres
bearcad.ui.move_ground(80, 50)
bearcad.ui.click(x, y)                  -- viewport pixel coordinates instead
bearcad.ui.move(x, y)
bearcad.ui.key("enter")
bearcad.ui.type("12.5")
```

## Camera

```lua
bearcad.ui.orbit(dx, dy)
bearcad.ui.pan(dx, dy)
bearcad.ui.wheel(scroll)
bearcad.ui.view("front")                -- standard view; waits for the camera animation
bearcad.ui.view("edge", "front_top")    -- a view-cube edge
bearcad.ui.view_home()
bearcad.ui.toggle_projection()
bearcad.ui.shading("solid_wireframe")   -- "wireframe" | "transparent" | "solid" | "solid_wireframe"
```

See [Navigation](/tools/navigation) for what these correspond to in the GUI, including the
view-cube HUD's gear/shading-modes popup.

## Panes and the command palette

```lua
bearcad.ui.pane("hierarchy", "hide")    -- show / hide / toggle a pane
bearcad.ui.palette("run", "view top")   -- run a command palette entry by name
```

## Dragging constrained geometry

```lua
bearcad.ui.drag_vertex({ kind = "line", index = 0, ["end"] = "end" }, u, v)
bearcad.ui.drag_line({ kind = "line", index = 0 }, au, av, u, v)
bearcad.ui.focus_dim("length")          -- focus a dimension input field
```

## Waiting

Because scripts run in a coroutine, these calls yield until the condition is met rather than
blocking the interpreter:

```lua
bearcad.ui.wait(5)        -- wait 5 UI frames
bearcad.ui.wait_ms(100)   -- wait 100 milliseconds
```

## Screenshots

```lua
bearcad.ui.screenshot()                       -- writes screenshot-bearcad.png
bearcad.ui.screenshot("out.png")
bearcad.ui.screenshot("out.png", true)        -- whole_window = true: capture the entire window
```

By default, `screenshot` captures the 3D viewport only (the view-cube HUD is suppressed for that
frame); pass `whole_window = true` to capture the entire window instead. This is the mechanism
behind BearCAD's visual regression testing: an instruction script can drive an exact interactive
flow (e.g. the rectangle tool's click → move → type → enter sequence) and emit a screenshot to
compare against a golden image in CI.

## A worked example

Recreating the same 80×50&nbsp;mm rectangle two ways — the declarative call, and the equivalent
simulated interaction:

```lua
-- Declarative (one call):
bearcad.rect{ width = 80, height = 50, name = "Main box" }

-- Simulated interaction (bearcad.ui.*):
bearcad.ui.tool("rectangle")
bearcad.ui.click_ground(0, 0)
bearcad.ui.move_ground(80, 50)
bearcad.ui.key("enter")
```

Both produce the identical committed rectangle in the document — the namespace split is about
*how* you describe the action, not about a different underlying model.
