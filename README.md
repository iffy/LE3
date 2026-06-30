# LE3 — Local CAD

<p align="center">
  <img src="src/assets/appicon.png" alt="LE3 app icon" width="128" height="128">
</p>

Local-first, parametric CAD. Built by robots.

## Download

| Platform | Download |
|----------|----------|
| macOS (Apple Silicon) | [le3-macos-aarch64.dmg](https://github.com/iffy/LE3/releases/latest/download/le3-macos-aarch64.dmg) |
| Windows (x86_64) | [le3-windows-x86_64.exe](https://github.com/iffy/LE3/releases/latest/download/le3-windows-x86_64.exe) |
| Linux (x86_64) | [le3-linux-x86_64.tar.gz](https://github.com/iffy/LE3/releases/latest/download/le3-linux-x86_64.tar.gz) |

Extract the Linux archive or mount the macOS disk image, then run `le3`. On Windows,
download and run `le3-windows-x86_64.exe`.

## Status

- **GUI** with a **wgpu**-accelerated 3D viewport (orbit/pan/zoom, view cube, HUD bear).
- **Sketch tools** on construction planes and face-hosted sketches: **rectangle**, **line**,
  and **circle**.
- **Construction geometry**: construction planes, per-edge construction flags, dashed
  construction lines.
- **Dimension constraints** on lines, rectangle edges, and circle diameters; draggable
  dimension labels.
- **Named parameters** with unit expressions (`mm`, `in`, arithmetic, parameter references).
- **Elements tree**, **Context** pane, **Parameters** table, and **command palette**.
- **Save / Open** documents as `.le3` files (SQLite, per SPEC §7).
- **Lua scripts** (SPEC §8): drive the live UI from a `.lua` file.

Not yet implemented: OCCT B-rep kernel, action DAG, assemblies, and the full CLI
from SPEC §9 (`--help` and script mode work today).

## Run

```sh
cargo run
```

- Pick a face with the **Sketch** tool (or start on the default XY construction plane),
  then draw with **Rectangle**, **Line**, or **Circle**.
- Type dimensions while drawing; **Tab** cycles fields; **Enter** commits.
- **Right-drag** to orbit; **Shift+right-drag** to pan; **mouse wheel** to zoom.
- **Escape** cancels an in-progress draw; press again to exit sketch mode or return to
  Select.
- **Save / Save As…** writes a `.le3` SQLite file; **Open…** loads one back.
- **Clear** resets the document; **Undo last** removes the most recently committed shape.

```sh
cargo run -- --help    # usage and exit
cargo test
```

## Script quickstart

Scripts are **Lua** files (`.lua`) that call the global `le3` API. They drive the same
actions and synthetic input as the GUI, which makes them useful for automation and
regression tests.

**Run a script and quit when it finishes:**

```sh
cargo run -- --script examples/rectangle.lua --exit
# same thing:
cargo run -- examples/rectangle.lua --exit
```

**Minimal script** — open a sketch, draw an 80×50 mm rectangle, save a screenshot:

```lua
le3.new()
le3.begin_sketch("construction_plane", 0)
le3.tool("rectangle")
le3.click(480, 320)
le3.move(580, 380)
le3.set_dim("width", "80")
le3.key("tab")
le3.set_dim("height", "50")
le3.key("enter")
le3.exit_sketch()
le3.wait_ms(100)
le3.screenshot("rectangle_preview.png")
```

Use `le3.click_ground(50, 25)` / `le3.move_ground(…)` for millimetre positions on
the active sketch plane (XY on the default construction plane). Use `le3.click(480, 320)`
for pixel coordinates in the 3D viewport panel.

**Named elements** — set a name when creating geometry or after committing a sketch shape,
then look it up later:

```lua
-- Programmatic create with name:
le3.begin_sketch("construction_plane", 0)
le3.rect({ width = 80, height = 50, name = "Main box" })

-- Or name after interactive draw:
le3.set_name(le3.element("rect", 0), "Main box")
local box = le3.find("Main box")
le3.select(box)
```

More examples: [examples/rectangle.lua](examples/rectangle.lua),
[examples/line.lua](examples/line.lua).

The Lua bindings live in `src/lua_script.rs`; the internal instruction runner is in
`src/script.rs`.

## Lua API reference

All functions are on the global `le3` table. Call `le3.import()` once at the top of a
script to copy those functions into the global namespace, so you can write `new()` instead
of `le3.new()`. You can also bind individual functions with `local new, tool = le3.new,
le3.tool`.

Scripts run in a coroutine; calls that need to wait (`wait`, `wait_ms`, `screenshot`,
camera `view` commands) yield until the next frame.

### Document

| Function | Description |
|---|---|
| `le3.new()` | New empty document |
| `le3.open(path)` | Open a document (no file dialog) |
| `le3.save()` / `le3.save(path)` | Save / Save As |
| `le3.clear()` | Reset the document |
| `le3.undo()` | Undo the last committed shape |
| `le3.quit()` | Close the app when the script ends |

### Tools and sketching

| Function | Description |
|---|---|
| `le3.tool("rectangle")` | Select a tool (`select`, `line`, `circle`, `sketch`, …) |
| `le3.begin_sketch("construction_plane", 0)` | Start sketching on a face |
| `le3.open_sketch(0)` | Re-open an existing sketch |
| `le3.exit_sketch()` | Leave the active sketch |

### Elements and names

| Function | Description |
|---|---|
| `le3.element("rect", 0)` | Reference an element by kind and index |
| `le3.find("Name")` | Look up an element by custom name (or `nil`) |
| `le3.set_name(element, "Name")` | Set or rename an element |
| `le3.select(element)` | Select an element (`{ additive = true }` to add) |
| `le3.clear_selection()` | Clear scene selection |
| `le3.set_visible(element, "hide")` | Show / hide / toggle visibility |
| `le3.set_construction(element, true)` | Mark element or edge as construction |
| `le3.rect({ width=80, height=50, name="Box" })` | Create a rectangle (optional `name`) |
| `le3.line({ length=80, name="Guide" })` | Create a line (optional `name`) |

Element kinds: `construction_plane`, `sketch`, `rect`, `line`, `circle`, `constraint`.
Pass a table `{ kind = "rect", index = 0, edge = "bottom" }` when an edge is needed.

### Dimensions and constraints

| Function | Description |
|---|---|
| `le3.set_dim("width", "80")` | Set a dimension while drawing |
| `le3.focus_dim("length")` | Focus a dimension field |
| `le3.edit_dim("width")` / `le3.commit_dim()` | Edit a committed dimension label |
| `le3.add_constraint({ kind="line", index=0 }, "25mm")` | Add a distance constraint |
| `le3.add_geometric_constraint("parallel")` | Add a geometric constraint |
| `le3.drag_vertex({ kind="line", index=0, end="end" }, u, v)` | Drag a constrained point |
| `le3.drag_line({ kind="line", index=0 }, au, av, u, v)` | Drag a line segment |

### Parameters

| Function | Description |
|---|---|
| `le3.parameter("add", "A", "5mm")` | Add a named parameter |
| `le3.parameter("value", 0, "A + 5in")` | Set a parameter expression |
| `le3.parameter("name", 0, "Len")` | Rename a parameter |
| `le3.parameter("delete", 1)` | Delete a parameter |

### Camera, UI, and input

| Function | Description |
|---|---|
| `le3.orbit(dx, dy)` / `le3.pan(dx, dy)` | Camera motion |
| `le3.wheel(scroll)` | Mouse wheel zoom |
| `le3.view("front")` | Standard view (waits for animation) |
| `le3.view("edge", "front_top")` | View-cube edge |
| `le3.view_home()` | Return to home view |
| `le3.pane("hierarchy", "hide")` | Show / hide / toggle a pane |
| `le3.palette("run", "view top")` | Run a palette command |
| `le3.click(x, y)` / `le3.move(x, y)` | Synthetic viewport input |
| `le3.click_ground(x, y)` | Click on sketch plane (mm) |
| `le3.key("enter")` / `le3.type("12.5")` | Keyboard / text input |
| `le3.wait(5)` | Wait 5 UI frames |
| `le3.wait_ms(100)` | Wait 100 milliseconds |
| `le3.screenshot("out.png")` | Capture the viewport |

Use `cargo run -- --show-commands` to echo GUI actions as `le3.*` calls on stdout.