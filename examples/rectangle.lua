-- Example Lua script — sketch on the default XY plane, draw a rectangle, screenshot.
-- Run: cargo run -- --script examples/rectangle.lua --exit

paramcad.new()
paramcad.begin_sketch("construction_plane", 0)
paramcad.tool("rectangle")

-- Viewport coordinates are relative to the 3D panel (below the toolbar).
paramcad.click(480, 320)
paramcad.wait(2)
paramcad.move(580, 380)
paramcad.wait(2)
paramcad.set_dim("width", "80")
paramcad.key("tab")
paramcad.set_dim("height", "50")
paramcad.key("enter")
paramcad.exit_sketch()

-- Name the committed rectangle for later lookup.
paramcad.set_name(paramcad.element("rect", 0), "Preview box")

paramcad.wait_ms(100)
paramcad.screenshot("rectangle_preview.png")