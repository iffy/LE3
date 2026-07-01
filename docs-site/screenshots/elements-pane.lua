-- Documentation screenshot: the Elements pane (the model hierarchy).
--
-- Builds a small feature tree (sketch -> extrusion -> body) and captures the
-- WHOLE WINDOW (the `true` second arg) so the Elements pane and its rows are
-- visible alongside the viewport. A fixed corner view keeps the output
-- deterministic (SPEC §8).
--
-- Output dir: $BEARCAD_SCREENSHOT_OUT (set by scripts/gen-doc-screenshots.sh),
-- falling back to ".". The PNG is only written where a real GPU frame renders
-- (a display, or CI Linux with xvfb + software Vulkan); otherwise the capture
-- never resolves and --timeout force-exits without a PNG, which is expected.

local out = (os.getenv("BEARCAD_SCREENSHOT_OUT") or ".") .. "/elements-pane.png"

bearcad.new()
bearcad.rect{ width = 80, height = 50, name = "Plate" }
-- Explicit closed-loop extrude (the `rect = 0` shorthand currently wedges the
-- screenshot render); this builds the same sketch -> extrusion -> body tree.
bearcad.extrude{ polygon = { 0, 1, 2, 3 }, distance = 20, name = "Block" }

-- Make sure the Elements pane is shown (it is by default).
bearcad.ui.pane("elements", "show")

bearcad.ui.view("corner", "front_right_top")
bearcad.ui.wait(2)
-- Capture the full window so the pane is included, not just the 3D viewport.
bearcad.ui.screenshot(out, true)

bearcad.quit()
