-- Example — make a line on the default ground plane with a single call.
-- Run: cargo run -- --script examples/line.lua --exit

bearcad.new()

-- One call: enters a ground-plane sketch if needed, then creates an 80 mm line (horizontal
-- by default; pass `angle` in degrees, or explicit `x1`/`y1` endpoints) and names it.
bearcad.line{ length = 80, name = "Guide line" }
assert(bearcad.find("Guide line") ~= nil)

bearcad.wait_ms(100)
bearcad.screenshot("line_preview.png")
bearcad.quit()
