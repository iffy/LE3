---
sidebar_position: 11
title: Dimension
---

# Dimension

**Shortcut:** `D`

Click a line segment (or rectangle edge, or circle) to add or edit a **distance/length/radius**
constraint, or select two non-parallel lines and press `D` for an **angle** constraint. Dimension
labels are draggable once placed.

Dimensional constraints may be driven by [parameters and expressions](#parameters), so a named
parameter can drive sketch geometry directly.

## Angle dimensions

Pressing `D` with two non-parallel lines selected (and no existing angle constraint between them)
doesn't commit a value immediately. Two crossing lines have two distinct angle magnitudes
(supplementary — one on each pair of opposite wedges); whichever wedge encloses the cursor is the
one previewed as you move the mouse. Clicking commits that choice and moves on to typing the
value, the same flow as other dimensions.

## Parameters

Named parameters (with unit expressions — `mm`, `in`, arithmetic, references to other parameters)
can be created inline while typing a dimension, or managed from the Parameters table. A
dimension's value field accepts an expression, not just a bare number.

## Scripting

```lua
bearcad.set_dim("width", "80")            -- set a dimension field while drawing
bearcad.edit_dim("width")                 -- reopen a committed dimension label
bearcad.commit_dim()

bearcad.add_constraint({ kind = "line", index = 0 }, "25mm")

-- Parameters:
bearcad.parameter("add", "A", "5mm")
bearcad.parameter("value", 0, "A + 5in")
bearcad.parameter("name", 0, "Len")
```

`bearcad.ui.focus_dim("length")` is the simulated-interaction equivalent of clicking into a
dimension field, for scripts that specifically need to exercise UI focus behavior rather than set
the value directly.
