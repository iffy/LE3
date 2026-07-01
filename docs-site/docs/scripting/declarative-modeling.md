---
sidebar_position: 2
title: Declarative modeling
---

# Declarative modeling

The top-level `bearcad.*` table is the primary API: OpenSCAD-style, describe geometry directly.
These examples are adapted from the project's own Lua test suite (`src/lua_script.rs`) and the
example scripts under `examples/`, so the syntax shown here is exercised by CI.

## A rectangle, extruded and exported

This is `examples/export_step.lua` end to end:

```lua
-- Run: cargo run -- --script examples/export_step.lua --exit

bearcad.new()

bearcad.rect{ width = 80, height = 50, name = "Base" }
bearcad.extrude{ rect = 0, distance = 20, name = "Block" }

bearcad.export_step("block.step")

-- A single named body can be exported on its own:
-- bearcad.export_step("block.step", "Block")

bearcad.quit()
```

`bearcad.export_stl(path, [body])` works the same way for STL. Both mirror the GUI's
**File → Export STL…** / STEP export, and export just one body if a name is given (matching what
right-clicking a body row in the Elements pane does).

## Sketch, draw, and name elements

```lua
bearcad.new()
bearcad.rect{ width = 80, height = 50, name = "Main box" }

-- Named lookup:
local box = bearcad.find("Main box")
bearcad.select(box)

-- Rename after the fact, or name a shape created without a `name` field:
bearcad.set_name(bearcad.element("rect", 0), "Main box")
```

Geometry-creation helpers are single calls that enter a ground-plane sketch automatically if none
is open — no simulated mouse/keyboard required:

```lua
bearcad.rect{ width = 80, height = 50, x = 0, y = 0, name = "Box" }
bearcad.line{ length = 80, angle = 45, name = "Diagonal" }
bearcad.line{ x = 0, y = 0, x1 = 10, y1 = 0 } -- explicit endpoints
bearcad.circle{ x = 10, y = 5, r = 12, name = "Hole" }
```

To sketch on a specific plane instead of the default ground plane:

```lua
bearcad.begin_sketch("construction_plane", 0)
bearcad.rect{ width = 80, height = 50, name = "Main box" }
```

## A closed polygon from plain lines, extruded

Any set of plain lines that connects end-to-end into a closed loop is a usable, extrudable face —
see [point-level selection](./point-selection) for how to close the loop purely from a script:

```lua
bearcad.new()
bearcad.line{ x = 0, y = 0, x1 = 10, y1 = 0 }
bearcad.line{ x = 10, y = 0, x1 = 5, y1 = 8 }
bearcad.line{ x = 5, y = 8, x1 = 0, y1 = 0 }
bearcad.extrude{ polygon = {0, 1, 2}, distance = 6 }
```

## Bezier curves

```lua
bearcad.line{
  x = 0, y = 0, x1 = 10, y1 = 0,
  bezier = { {3, 4}, {7, 4} },
  name = "Curve",
}
```

## Chamfer and fillet

Both operate on a sketch vertex where exactly two plain lines meet:

```lua
local corner = { kind = "line", index = 0, ["end"] = "end" }
bearcad.chamfer_vertex{ point = corner, distance = 3 }
-- or:
bearcad.fillet_vertex{ point = corner, radius = 3 }
```

## Constraints and parameters

```lua
bearcad.select{ kind = "line", index = 0 }
bearcad.select({ kind = "line", index = 1 }, true)
bearcad.add_geometric_constraint("parallel")

bearcad.add_constraint({ kind = "line", index = 0 }, "25mm")

bearcad.parameter("add", "A", "5mm")
bearcad.parameter("value", 0, "A + 5in")
```

## Visibility and construction geometry

```lua
bearcad.set_visible(box, "hide")       -- "show" | "hide" | "toggle"
bearcad.set_construction(box, true)
```

## Import

```lua
bearcad.new()
bearcad.import_stl("part.stl")
bearcad.import_step("part.step")
```

With the OCCT kernel compiled in (`--features occt`), STEP export writes **real BREP** (planar
and curved surfaces) from a body's OCCT solid, and STEP import reads **real BREP including
curved/NURBS surfaces**, tessellating it into a new body — so files from other CAD tools round-trip.

Without the kernel (the default build), export/import use the hand-rolled faceted path: export
writes a triangulated `FACETED_BREP`, and import only round-trips that same subset
(`POLY_LOOP`-bounded planar `FACE_SURFACE`s) — files using curved/NURBS `ADVANCED_FACE`
geometry are rejected with a clear error rather than approximated.

## Document lifecycle

```lua
bearcad.new()
bearcad.open("path/to/file.bearcad")
bearcad.save()                 -- Save
bearcad.save("other.bearcad")  -- Save As
bearcad.clear()
bearcad.undo()
bearcad.quit()                 -- close the app when the script ends
```
