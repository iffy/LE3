-- Documentation screenshot: the Extrude tool.
--
-- Extrudes an 80 x 50 mm rectangle 20 mm into a solid body and captures it from
-- a fixed front-top-right corner view so the 3D form is visible and the output
-- is deterministic (SPEC §8).
--
-- Output dir: $BEARCAD_SCREENSHOT_OUT (set by scripts/gen-doc-screenshots.sh),
-- falling back to ".". The PNG is only written where a real GPU frame renders
-- (a display, or CI Linux with xvfb + software Vulkan); otherwise the capture
-- never resolves and --timeout force-exits without a PNG, which is expected.

local out = (os.getenv("BEARCAD_SCREENSHOT_OUT") or ".") .. "/extrude.png"

bearcad.new()
bearcad.rect{ width = 80, height = 50, name = "Base" }
-- Extrude the rectangle's four lines as an explicit closed loop. (The `rect = 0`
-- shorthand builds the same body but currently wedges the screenshot render, so
-- the docs harness uses the explicit polygon form.)
bearcad.extrude{ polygon = { 0, 1, 2, 3 }, distance = 20, name = "Block" }

bearcad.ui.view("corner", "front_right_top")
bearcad.ui.wait(2)
bearcad.ui.screenshot(out)

bearcad.quit()
