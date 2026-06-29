-- Example — extrude a rectangle into a box and export it as an STL file.
-- Run: cargo run -- --script examples/export_stl.lua --exit

le3.new()

-- 80 x 50 mm rectangle on the default ground plane...
le3.rect{ width = 80, height = 50, name = "Base" }

-- ...extruded 20 mm into a solid body.
le3.extrude{ rect = 0, distance = 20, name = "Block" }

-- Export every body in the document to an STL file.
le3.export_stl("block.stl")

-- A single named body can be exported on its own:
-- le3.export_stl("block.stl", "Block")

le3.quit()
