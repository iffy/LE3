# BearCAD 

<p align="left">
  <img src="src/assets/appicon.png" alt="BearCAD app icon" width="128" height="128">
</p>

Local-first, parametric CAD. Built by robots.

Source: [github.com/iffy/BearCAD](https://github.com/iffy/BearCAD)

## Download

| Platform | Download |
|----------|----------|
| macOS (Apple Silicon) | [bearcad-macos-aarch64.dmg](https://github.com/iffy/BearCAD/releases/latest/download/bearcad.dmg) |
| Windows (x86_64) | [bearcad.exe](https://github.com/iffy/BearCAD/releases/latest/download/bearcad.exe) |
| Linux (x86_64) | [bearcad-linux-x86_64.tar.gz](https://github.com/iffy/BearCAD/releases/latest/download/bearcad-linux-x86_64.tar.gz) |

Extract the Linux archive or mount the macOS disk image, then run `bearcad`. On Windows,
download and run `bearcad.exe`.

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
- **Save / Open** documents as `.bearcad` files (SQLite, per SPEC §7).
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
- **Save / Save As…** writes a `.bearcad` SQLite file; **Open…** loads one back.
- **Clear** resets the document; **Undo last** removes the most recently committed shape.

```sh
cargo run -- --help    # usage and exit
cargo test
```

## Script quickstart

Scripts are **Lua** files (`.lua`) that call the global `bearcad` API. They drive the same
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
bearcad.new()
bearcad.begin_sketch("construction_plane", 0)
bearcad.tool("rectangle")
bearcad.click(480, 320)
bearcad.move(580, 380)
bearcad.set_dim("width", "80")
bearcad.key("tab")
bearcad.set_dim("height", "50")
bearcad.key("enter")
bearcad.exit_sketch()
bearcad.wait_ms(100)
bearcad.screenshot("rectangle_preview.png")
```

Use `bearcad.click_ground(50, 25)` / `bearcad.move_ground(…)` for millimetre positions on
the active sketch plane (XY on the default construction plane). Use `bearcad.click(480, 320)`
for pixel coordinates in the 3D viewport panel.

**Named elements** — set a name when creating geometry or after committing a sketch shape,
then look it up later:

```lua
-- Programmatic create with name:
bearcad.begin_sketch("construction_plane", 0)
bearcad.rect({ width = 80, height = 50, name = "Main box" })

-- Or name after interactive draw:
bearcad.set_name(bearcad.element("rect", 0), "Main box")
local box = bearcad.find("Main box")
bearcad.select(box)
```

More examples: [examples/rectangle.lua](examples/rectangle.lua),
[examples/line.lua](examples/line.lua).

The Lua bindings live in `src/lua_script.rs`; the internal instruction runner is in
`src/script.rs`.

## Lua API reference

All functions are on the global `bearcad` table. Call `bearcad.import()` once at the top of a
script to copy those functions into the global namespace, so you can write `new()` instead
of `bearcad.new()`. You can also bind individual functions with `local new, tool = bearcad.new,
bearcad.tool`.

Scripts run in a coroutine; calls that need to wait (`wait`, `wait_ms`, `screenshot`,
camera `view` commands) yield until the next frame.

### Document

| Function | Description |
|---|---|
| `bearcad.new()` | New empty document |
| `bearcad.open(path)` | Open a document (no file dialog) |
| `bearcad.save()` / `bearcad.save(path)` | Save / Save As |
| `bearcad.clear()` | Reset the document |
| `bearcad.undo()` | Undo the last committed shape |
| `bearcad.quit()` | Close the app when the script ends |

### Tools and sketching

| Function | Description |
|---|---|
| `bearcad.tool("rectangle")` | Select a tool (`select`, `line`, `circle`, `sketch`, …) |
| `bearcad.begin_sketch("construction_plane", 0)` | Start sketching on a face |
| `bearcad.open_sketch(0)` | Re-open an existing sketch |
| `bearcad.exit_sketch()` | Leave the active sketch |

### Elements and names

| Function | Description |
|---|---|
| `bearcad.element("rect", 0)` | Reference an element by kind and index |
| `bearcad.find("Name")` | Look up an element by custom name (or `nil`) |
| `bearcad.set_name(element, "Name")` | Set or rename an element |
| `bearcad.select(element)` | Select an element (`{ additive = true }` to add) |
| `bearcad.clear_selection()` | Clear scene selection |
| `bearcad.set_visible(element, "hide")` | Show / hide / toggle visibility |
| `bearcad.set_construction(element, true)` | Mark element or edge as construction |
| `bearcad.rect({ width=80, height=50, name="Box" })` | Create a rectangle (optional `name`) |
| `bearcad.line({ length=80, name="Guide" })` | Create a line (optional `name`) |

Element kinds: `construction_plane`, `sketch`, `rect`, `line`, `circle`, `constraint`.
Pass a table `{ kind = "rect", index = 0, edge = "bottom" }` when an edge is needed.

### Dimensions and constraints

| Function | Description |
|---|---|
| `bearcad.set_dim("width", "80")` | Set a dimension while drawing |
| `bearcad.focus_dim("length")` | Focus a dimension field |
| `bearcad.edit_dim("width")` / `bearcad.commit_dim()` | Edit a committed dimension label |
| `bearcad.add_constraint({ kind="line", index=0 }, "25mm")` | Add a distance constraint |
| `bearcad.add_geometric_constraint("parallel")` | Add a geometric constraint |
| `bearcad.drag_vertex({ kind="line", index=0, end="end" }, u, v)` | Drag a constrained point |
| `bearcad.drag_line({ kind="line", index=0 }, au, av, u, v)` | Drag a line segment |

### Parameters

| Function | Description |
|---|---|
| `bearcad.parameter("add", "A", "5mm")` | Add a named parameter |
| `bearcad.parameter("value", 0, "A + 5in")` | Set a parameter expression |
| `bearcad.parameter("name", 0, "Len")` | Rename a parameter |
| `bearcad.parameter("delete", 1)` | Delete a parameter |

### Camera, UI, and input

| Function | Description |
|---|---|
| `bearcad.orbit(dx, dy)` / `bearcad.pan(dx, dy)` | Camera motion |
| `bearcad.wheel(scroll)` | Mouse wheel zoom |
| `bearcad.view("front")` | Standard view (waits for animation) |
| `bearcad.view("edge", "front_top")` | View-cube edge |
| `bearcad.view_home()` | Return to home view |
| `bearcad.pane("hierarchy", "hide")` | Show / hide / toggle a pane |
| `bearcad.palette("run", "view top")` | Run a palette command |
| `bearcad.click(x, y)` / `bearcad.move(x, y)` | Synthetic viewport input |
| `bearcad.click_ground(x, y)` | Click on sketch plane (mm) |
| `bearcad.key("enter")` / `bearcad.type("12.5")` | Keyboard / text input |
| `bearcad.wait(5)` | Wait 5 UI frames |
| `bearcad.wait_ms(100)` | Wait 100 milliseconds |
| `bearcad.screenshot("out.png")` | Capture the viewport |

Use `cargo run -- --show-commands` to echo GUI actions as `bearcad.*` calls on stdout.