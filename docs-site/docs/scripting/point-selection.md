---
sidebar_position: 4
title: Point-level selection
---

# Point-level selection

`bearcad.select` normally targets a whole element (a line, a rect, a circle). Point-level
selection targets an individual **vertex** — a `ConstraintPoint` — instead: a line endpoint, a
rectangle corner, or (with an explicit flag) a circle's center. This uses the same point
numbering the interactive [Constraint](/tools/constraint) tool uses, so a script can drive exactly
the same constraint flows a user would with the mouse.

## Selecting a line endpoint

```lua
bearcad.select{ kind = "line", index = 0, ["end"] = "start" }  -- or "end"
```

A line's two points are `start`/`end`, i.e. `(x0, y0)`/`(x1, y1)`.

## Selecting a rectangle corner

```lua
bearcad.select{ kind = "rect", index = 0, corner = 2 }
```

A rectangle's corners are numbered **0–3 counterclockwise starting at its `(x, y)` origin
corner** — the same numbering shown when the interactive Constraint tool highlights a rect's
points.

## Selecting a circle's center

`kind = "circle"` alone still selects the whole circle. Pass `point = true` to target just its
center point:

```lua
bearcad.select{ kind = "circle", index = 0, point = true }
```

`point = true` is the general escape hatch for targeting a point that has no `end`/`corner` field
of its own — a table with neither still resolves to the whole element, as before.

## Selecting a face's own vertex or edge

While a sketch is open directly on one of a body's own faces (an extrusion cap or side wall —
not a construction plane), that face's own boundary loop is selectable too, so a sketch can be
constrained against the face it's drawn on (e.g. "30mm from the top edge"):

```lua
bearcad.select{
    kind = "face",
    face = { kind = "extrude_cap", extrusion = 0, profile = "rect", profile_index = 0, top = true },
    index = 2,
}
```

`face` takes the same table shape [`bearcad.begin_sketch`](./declarative-modeling) does for a 3D
body face. `index` numbers the face's boundary loop the same way `cap_polygon_world`/
`side_quad_world` order it. This selects the vertex (a `ConstraintPoint::FaceVertex`); add
`edge = true` to select the edge running from that corner to the next instead
(`ConstraintLine::FaceEdge`):

```lua
bearcad.select{
    kind = "face",
    face = { kind = "extrude_side", extrusion = 0, profile = "rect", profile_index = 0, edge = 0 },
    index = 0,
    edge = true,
}
```

Both are fixed by the body's own geometry — not draggable or settable — but otherwise plug into
`Coincident`, `Midpoint`, and distance constraints exactly like any other sketch point/line.
Picking (interactive or scripted) is scoped to the *sketch's own face* only, not arbitrary other
faces in the scene; imported STL/STEP bodies have no analytic boundary to reference here.

## Additive selection

Pass `true` as the second argument to `bearcad.select` to add to the current selection instead of
replacing it — this is how you build up a two-point (or two-line) selection for a constraint:

```lua
bearcad.select({ kind = "line", index = 1 }, true)
```

## Worked example: closing a polygon loop purely from a script

This is the motivating case the feature was built for (#68) — joining two line endpoints with a
`Coincident` constraint, needed to test closed-loop polygon-face detection end to end without
simulating any mouse clicks:

```lua
bearcad.new()
bearcad.line{ x = 0, y = 0, x1 = 10, y1 = 0, name = "a" }
bearcad.line{ x = 20, y = 0, x1 = 30, y1 = 0, name = "b" }

bearcad.select{ kind = "line", index = 0, ["end"] = "end" }
bearcad.select({ kind = "line", index = 1, ["end"] = "start" }, true)
bearcad.add_geometric_constraint("coincident")
```

After this, line `0`'s end and line `1`'s start are coincident — exactly as if you'd clicked both
endpoints in the viewport with the Constraint tool active and pressed `I`. Combine this with
[`bearcad.extrude{ polygon = {...} }`](./declarative-modeling#a-closed-polygon-from-plain-lines-extruded)
to build and extrude an arbitrary closed profile without any GUI interaction at all.
