-- Example — draw a line and save a screenshot.
-- Run: cargo run -- --script examples/line.lua --exit

paramcad.new()
paramcad.begin_sketch("construction_plane", 0)
paramcad.tool("line")

paramcad.click(480, 320)
paramcad.wait(2)
paramcad.move(580, 360)
paramcad.wait(2)
paramcad.set_dim("length", "80")
paramcad.key("enter")
paramcad.exit_sketch()

paramcad.set_name(paramcad.element("line", 0), "Guide line")
assert(paramcad.find("Guide line") ~= nil)

paramcad.wait_ms(100)
paramcad.screenshot("line_preview.png")