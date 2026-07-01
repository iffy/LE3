---
sidebar_position: 8
title: Chamfer
---

# Chamfer

**Shortcut:** `K`

Chamfer cuts a **2D sketch vertex** where exactly two plain lines meet, using the same
"push/pull" gizmo plus text-entry input as [Fillet](./fillet.md) and Extrude. Click the vertex,
then drag the gizmo or type a distance; **Enter** commits, **Esc** cancels. A live preview of the
resulting cut corner follows the gizmo as you drag, and the finished chamfer nests under its
trimmed line in the Elements pane (labeled "Chamfer N") rather than sitting as an ordinary
sibling.

## How it works

Chamfer truncates each of the two adjacent lines back along itself by the typed distance, then
bridges the two new endpoints with a new, **straight** `Line`. (Fillet is the same operation,
except the bridging line is curved instead of straight — see [Fillet](./fillet.md) for the shared
mechanics: clamping against the adjacent lines' far endpoints, the degenerate-corner rejection
near 0°/180°, and the note about constraints on the old vertex not being auto-repaired.)

## 3D solid edges

Like Fillet, the `K` tool also works on solid edges when no sketch is open — click a vertical
side edge or a side/cap edge, then drag or type a distance. It's the same mesh-bevel
approximation described in [Fillet's 3D section](./fillet.md#3d-solid-edges): a flat bevel quad
connecting the two originally adjacent faces (not a true BREP chamfer), parametric and scoped the
same way (`Rect`/`Polygon`-profile extrusions only, no vertex miters).

## Scope: 2D sketch vertices only

The straight-bridging-line behavior and shared mechanics described above (clamping, degenerate
corner rejection) are specific to the **2D sketch-vertex** case.

## Scripting

```lua
bearcad.chamfer_vertex{
  point = { kind = "line", index = 0, ["end"] = "end" },
  distance = 3,
}

bearcad.chamfer_edge{
  extrusion = 0,
  edge = { kind = "vertical", face = 0, edge = 1 },
  distance = 3,
}
```

`edge` is `{ kind = "vertical", face =, edge = }` or `{ kind = "cap", face =, edge =, top = }`.
