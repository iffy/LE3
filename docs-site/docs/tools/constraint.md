---
sidebar_position: 12
title: Constraint
---

# Constraint

**Shortcut:** `C`

The Constraint tool applies **geometric** constraints (parallel, perpendicular, equal,
coincident, midpoint, vertical, horizontal) to selected sketch geometry. Distance/dimensional
constraints stay on the [Dimension](./dimension.md) tool (`D`).

## Selecting

Sketch points (line endpoints, rectangle corners, circle centers), lines, and rectangle edges are
all selectable in the viewport. Point picks take precedence near vertices, within the point-pick
tolerance.

**Constraining to the face you're sketching on:** while a sketch is open directly on one of a
body's own faces (an extrusion cap or side wall, not a construction plane), that face's own
corners and edges are selectable too — so you can pin a sketch point to a corner of the face, or
constrain a distance from sketch geometry to one of the face's own edges (e.g. "30mm from the top
edge"). Vertices win over edges, same as sketch-native points. Both are fixed by the body's
geometry — they can't be dragged — but otherwise work with Coincident, Midpoint, and distance
constraints exactly like any other point/line. This is scoped to the sketch's own face only, and
to extrusion-backed bodies (imported STL/STEP bodies have no analytic face/edge structure).

## The context pane

While the Constraint tool is active, the context pane lists every geometric constraint type as a
button, in a fixed order — **always all of them**, even when nothing is selected. Types the
current selection can't satisfy appear disabled/faded, with a hint describing what selection they
need (e.g. "line, line" for Parallel). A button enables only once the current selection satisfies
that type.

| Constraint | Needs | Shortcut |
|---|---|---|
| Parallel | line, line | `A` |
| Perpendicular | line, line | `T` |
| Equal | line, line (rect edges count as lines) | `Q` |
| Coincident | point+point, point+line, or point+circle (on its perimeter) | `I` |
| Midpoint | point, line | `M` |
| Vertical | line | `V` |
| Horizontal | line | `H` |

Each mnemonic letter (chosen to avoid the global tool keys) applies that constraint immediately if
it's currently enabled — you don't need to click the button.

## Redundant-constraint cleanup

When a point already constrained coincident with a line is then constrained to a *specific* point
on that same line (one of its endpoints, or its midpoint), the earlier generic point-on-line
coincidence is automatically removed in favor of the more specific constraint.

## Scripting

```lua
bearcad.ui.tool("constraint")           -- or work purely declaratively, see below

bearcad.select{ kind = "line", index = 0 }
bearcad.select({ kind = "line", index = 1 }, true)
bearcad.add_geometric_constraint("parallel")
```

Point-level selection (`bearcad.select{ kind = "line", index = 0, ["end"] = "end" }`, `corner =
N`, `point = true`) targets an individual `ConstraintPoint` instead of a whole element, and
`bearcad.select{ kind = "face", face = {...}, index }` (add `edge = true` for the edge instead of
the vertex) targets the sketched-on face's own boundary — see
[Scripting → point-level selection](/docs/scripting/point-selection) for the full API and a worked
example of closing a polygon loop purely from a script.
