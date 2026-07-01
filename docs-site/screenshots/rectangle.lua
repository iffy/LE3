-- Documentation screenshot: the Rectangle tool.
--
-- Builds a single locked 80 x 50 mm rectangle on the ground plane and captures
-- it from a fixed top-down view so the output is deterministic (SPEC §8).
--
-- The output directory comes from $BEARCAD_SCREENSHOT_OUT (set by
-- scripts/gen-doc-screenshots.sh); it falls back to "." so the script can be
-- run by hand for testing. The PNG is only written where a real GPU frame
-- renders (a display, or CI Linux with xvfb + software Vulkan); in a
-- display-less environment the capture never resolves and --timeout force-exits
-- without a PNG, which is expected.

local out = (os.getenv("BEARCAD_SCREENSHOT_OUT") or ".") .. "/rectangle.png"

bearcad.new()
bearcad.rect{ width = 80, height = 50, name = "Plate" }

bearcad.ui.view("top")
bearcad.ui.wait(2)
bearcad.ui.screenshot(out)

bearcad.quit()
