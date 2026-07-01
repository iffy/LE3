---
sidebar_position: 9
title: Construction Plane
---

# Construction Plane

**Shortcut:** `P`

Click a face or an axis/line reference, then set an offset (and, for an axis, an angle); press
**Enter** to commit. Construction planes are datum geometry — they don't render as solid, and
they're the surfaces you sketch on with the [Sketch](./sketch.md) tool (alongside the planar
faces of extruded bodies).

## Picking a reference

- **Faces** — an existing construction plane, or a body's face — offset the new plane along that
  face's normal.
- **Lines and axes** — standalone sketch lines, individual shape edges (rectangle sides,
  construction-plane borders), the origin **X/Y/Z triad**, and **any edge of any 3D body**
  (#31) — including STL/STEP-imported bodies — are all valid axis references. A body edge is a
  *feature* edge of its triangle mesh (the same extraction the Wireframe shading mode uses), so
  it works uniformly regardless of how the body was created. Axis gizmo handles highlight on
  hover so you can see which one will be grabbed.
- Shape edges take precedence over the shape's own face when the cursor is near the edge.

Manipulation gizmos (including the plane-making gizmo) render with depth testing disabled, so
they stay visible and clickable even when a body would otherwise occlude them.

## Scripting

Construction planes are referenced by index once created, e.g. as the target of
`bearcad.begin_sketch("construction_plane", 0)` (index `0` is the default ground/XY plane created
implicitly the first time a script draws geometry with no sketch open).
