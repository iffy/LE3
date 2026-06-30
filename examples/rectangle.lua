-- Example — make a rectangle on the default ground plane with a single call.
-- Run: cargo run -- --script examples/rectangle.lua --exit

bearcad.new()

-- One call: enters a sketch on the default (XY) ground plane if needed, then creates an
-- 80 x 50 mm rectangle with locked dimensions and names it.
bearcad.rect{ width = 80, height = 50, name = "Preview box" }

bearcad.quit()
