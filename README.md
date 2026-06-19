# LE3 — Local CAD

On-device parametric CAD. See [SPEC.md](SPEC.md) for the full design.

## Status

Very early prototype. Currently implemented:

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

Scripts are **Lua** files (`.lua`) that call the global `paramcad` API. They drive the same
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
paramcad.new()
paramcad.begin_sketch("construction_plane", 0)
paramcad.tool("rectangle")
paramcad.click(480, 320)
paramcad.move(580, 380)
paramcad.set_dim("width", "80")
paramcad.key("tab")
paramcad.set_dim("height", "50")
paramcad.key("enter")
paramcad.exit_sketch()
paramcad.wait_ms(100)
paramcad.screenshot("rectangle_preview.png")
```

Use `paramcad.click_ground(50, 25)` / `paramcad.move_ground(…)` for millimetre positions on
the active sketch plane (XY on the default construction plane). Use `paramcad.click(480, 320)`
for pixel coordinates in the 3D viewport panel.

**Named elements** — set a name when creating geometry or after committing a sketch shape,
then look it up later:

```lua
-- Programmatic create with name:
paramcad.begin_sketch("construction_plane", 0)
paramcad.rect({ width = 80, height = 50, name = "Main box" })

-- Or name after interactive draw:
paramcad.set_name(paramcad.element("rect", 0), "Main box")
local box = paramcad.find("Main box")
paramcad.select(box)
```

More examples: [examples/rectangle.lua](examples/rectangle.lua),
[examples/line.lua](examples/line.lua).

The Lua bindings live in `src/lua_script.rs`; the internal instruction runner is in
`src/script.rs`.

## Lua API reference

All functions are on the global `paramcad` table. Scripts run in a coroutine; calls that
need to wait (`wait`, `wait_ms`, `screenshot`, camera `view` commands) yield until the
next frame.

### Document

| Function | Description |
|---|---|
| `paramcad.new()` | New empty document |
| `paramcad.open(path)` | Open a document (no file dialog) |
| `paramcad.save()` / `paramcad.save(path)` | Save / Save As |
| `paramcad.clear()` | Reset the document |
| `paramcad.undo()` | Undo the last committed shape |
| `paramcad.quit()` | Close the app when the script ends |

### Tools and sketching

| Function | Description |
|---|---|
| `paramcad.tool("rectangle")` | Select a tool (`select`, `line`, `circle`, `sketch`, …) |
| `paramcad.begin_sketch("construction_plane", 0)` | Start sketching on a face |
| `paramcad.open_sketch(0)` | Re-open an existing sketch |
| `paramcad.exit_sketch()` | Leave the active sketch |

### Elements and names

| Function | Description |
|---|---|
| `paramcad.element("rect", 0)` | Reference an element by kind and index |
| `paramcad.find("Name")` | Look up an element by custom name (or `nil`) |
| `paramcad.set_name(element, "Name")` | Set or rename an element |
| `paramcad.select(element)` | Select an element (`{ additive = true }` to add) |
| `paramcad.clear_selection()` | Clear scene selection |
| `paramcad.set_visible(element, "hide")` | Show / hide / toggle visibility |
| `paramcad.set_construction(element, true)` | Mark element or edge as construction |
| `paramcad.rect({ width=80, height=50, name="Box" })` | Create a rectangle (optional `name`) |
| `paramcad.line({ length=80, name="Guide" })` | Create a line (optional `name`) |

Element kinds: `construction_plane`, `sketch`, `rect`, `line`, `circle`, `constraint`.
Pass a table `{ kind = "rect", index = 0, edge = "bottom" }` when an edge is needed.

### Dimensions and constraints

| Function | Description |
|---|---|
| `paramcad.set_dim("width", "80")` | Set a dimension while drawing |
| `paramcad.focus_dim("length")` | Focus a dimension field |
| `paramcad.edit_dim("width")` / `paramcad.commit_dim()` | Edit a committed dimension label |
| `paramcad.add_constraint({ kind="line", index=0 }, "25mm")` | Add a distance constraint |
| `paramcad.add_geometric_constraint("parallel")` | Add a geometric constraint |
| `paramcad.drag_vertex({ kind="line", index=0, end="end" }, u, v)` | Drag a constrained point |
| `paramcad.drag_line({ kind="line", index=0 }, au, av, u, v)` | Drag a line segment |

### Parameters

| Function | Description |
|---|---|
| `paramcad.parameter("add", "A", "5mm")` | Add a named parameter |
| `paramcad.parameter("value", 0, "A + 5in")` | Set a parameter expression |
| `paramcad.parameter("name", 0, "Len")` | Rename a parameter |
| `paramcad.parameter("delete", 1)` | Delete a parameter |

### Camera, UI, and input

| Function | Description |
|---|---|
| `paramcad.orbit(dx, dy)` / `paramcad.pan(dx, dy)` | Camera motion |
| `paramcad.wheel(scroll)` | Mouse wheel zoom |
| `paramcad.view("front")` | Standard view (waits for animation) |
| `paramcad.view("edge", "front_top")` | View-cube edge |
| `paramcad.view_home()` | Return to home view |
| `paramcad.pane("hierarchy", "hide")` | Show / hide / toggle a pane |
| `paramcad.palette("run", "view top")` | Run a palette command |
| `paramcad.click(x, y)` / `paramcad.move(x, y)` | Synthetic viewport input |
| `paramcad.click_ground(x, y)` | Click on sketch plane (mm) |
| `paramcad.key("enter")` / `paramcad.type("12.5")` | Keyboard / text input |
| `paramcad.wait(5)` | Wait 5 UI frames |
| `paramcad.wait_ms(100)` | Wait 100 milliseconds |
| `paramcad.screenshot("out.png")` | Capture the viewport |

Use `cargo run -- --show-commands` to echo GUI actions as `paramcad.*` calls on stdout.