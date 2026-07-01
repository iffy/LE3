---
sidebar_position: 7
title: Fillet
---

# Fillet

**Shortcut:** `F`

Fillet rounds a **2D sketch vertex** where exactly two plain lines meet, using a "push/pull"
gizmo plus text-entry input, mirroring the Extrude tool. Click the vertex, then drag the gizmo or
type a radius; **Enter** commits, **Esc** cancels. A live preview of the resulting rounded corner
follows the gizmo as you drag, and the finished fillet nests under its trimmed line in the
Elements pane (labeled "Fillet N") rather than sitting as an ordinary sibling.

## How it works

Fillet truncates each of the two adjacent lines back along itself by the tangent length implied
by the requested radius, then bridges the two new endpoints with a new `Line` whose curve is a
**single-cubic-bezier approximation of the circular arc** — accurate for realistic corner angles,
not a true NURBS arc. This reuses the same bezier-curve machinery described in
[Line](./line.md#bezier-curves) (rendering, hit-testing, extrusion tessellation) for free, since a
filleted corner is, to the rest of the app, just another curved `Line`.

- The tangent length is clamped so it never cuts back past either adjacent line's own far
  endpoint.
- A corner within ~1° of straight (0°/180°, i.e. parallel or anti-parallel edges) is rejected as
  degenerate.
- Only the `Coincident` constraint directly between the two treated endpoints is removed on
  commit — other constraints that happened to reference the old vertex position are **not**
  automatically fixed up. This is a known, documented limitation: the resulting sketch may need
  manual re-constraining afterward.

## 3D solid edges

The same `F` tool also fillets a solid's edges when no sketch is open: click a vertical side
edge, or a side/cap edge where a wall meets the top or bottom face, then drag the gizmo or type
a radius. This is a **mesh-bevel approximation**, not a true BREP fillet — BearCAD has no
BREP/NURBS kernel (see [Construction Plane](./construction-plane.md) and SPEC §10) — so it
directly reshapes the extrusion's triangle mesh with a faceted rounded bevel rather than a
tangent-continuous curved surface. It's parametric like everything else: the treatment is stored
on the `Extrusion` and re-evaluated every frame, not baked into the mesh once.

Scoped to bodies sourced from `Extrusion`s with a `Rect` or `Polygon` profile — `Circle` profiles
and STL/STEP-imported meshes have no analytic edge to bevel and are out of scope. A **vertex
miter**, where 3+ treated edges would meet at a shared corner, is rejected at commit time rather
than blended.

## Scope: 2D sketch vertices only

The bezier-arc approximation, tangent-length clamping, and degenerate-corner rejection described
above are specific to the **2D sketch-vertex** case.

## Scripting

```lua
bearcad.fillet_vertex{
  point = { kind = "line", index = 0, ["end"] = "end" },
  radius = 3,
}

bearcad.fillet_edge{
  extrusion = 0,
  edge = { kind = "vertical", face = 0, edge = 1 },
  radius = 3,
}
```

`point` uses the same `ConstraintPoint`-style table as
[point-level selection](/docs/scripting/point-selection) — a line endpoint, a rect corner, etc. `edge`
is `{ kind = "vertical", face =, edge = }` or `{ kind = "cap", face =, edge =, top = }`.
