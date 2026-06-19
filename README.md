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
- **Instruction scripts** (SPEC §9.3): drive the live UI from a `.le3script` file.

Not yet implemented: OCCT B-rep kernel, action DAG, assemblies, Lua API, and the full CLI
from SPEC §9 (script mode and `--help` work today).

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

Scripts are plain-text `.le3script` files — one instruction per line. They drive the same
actions and synthetic input as the GUI, which makes them useful for automation and
regression tests.

**Run a script and quit when it finishes:**

```sh
cargo run -- --script examples/rectangle.le3script --exit
# same thing:
cargo run -- examples/rectangle.le3script --exit
```

**Minimal script** — open a sketch, draw an 80×50 mm rectangle, save a screenshot:

```text
new
begin_sketch construction_plane 0
tool rectangle
click 480 320
move 580 380
set_dim width 80
key tab
set_dim height 50
key enter
exit_sketch
wait 100ms
screenshot rectangle_preview.png
```

Use `click_ground 50 25` / `move_ground …` for millimetre positions on the active sketch
plane (XY on the default construction plane). Use `click 480 320` for pixel coordinates in
the 3D viewport panel.

More examples: [examples/rectangle.le3script](examples/rectangle.le3script),
[examples/line.le3script](examples/line.le3script).

The full instruction reference is below. The parser and runner live in `src/script.rs`; add
new instructions there first (with tests), then document them here.

## Scripting reference

### Format

- One instruction per line.
- Lines starting with `#` are comments; blank lines are ignored.
- Instruction names are case-insensitive.
- Many instructions accept aliases (for example `tool rect` and `tool rectangle`).
- Expressions in dimension and parameter values support units (`mm`, `in`, etc.) and
  parameter names.

### Coordinates

- **Viewport** (`click`, `move`, `drag`, …): pixel coordinates relative to the 3D panel
  (below the toolbar).
- **Ground** (`click_ground`, `move_ground`): millimetre positions on the active sketch
  face's plane (XY when sketching on the default construction plane).
- **Camera** (`orbit`, `pan`, `wheel`): `orbit`/`pan` apply camera motion directly; `wheel`,
  `zoom`, and `scroll` adjust zoom.

### Instruction reference

#### Document

| Instruction | Description |
|---|---|
| `new` | New empty document |
| `open path/to/doc.le3` | Open a document (no file dialog) |
| `save` | Save to the current path |
| `save path/to/doc.le3` | Save As to a path |
| `clear` | Reset the document (all geometry and sketches) |
| `undo` | Undo the last committed shape |
| `quit` / `exit` | Close the app when the script ends |

#### Tools and sketching

| Instruction | Description |
|---|---|
| `tool select` | Select tool |
| `tool rectangle` / `tool rect` | Rectangle tool |
| `tool line` | Line tool |
| `tool circle` | Circle tool |
| `tool plane` / `tool construction_plane` | Construction plane tool |
| `tool sketch` | Sketch tool (pick a face to enter sketch mode) |
| `tool dimension` / `tool dim` | Dimension constraint tool |
| `begin_sketch construction_plane 0` | Start sketching on a face (`construction_plane`, `rect`, or `circle` + index) |
| `open_sketch 0` / `edit_sketch 0` | Re-open an existing sketch for editing |
| `exit_sketch` | Leave the active sketch |

#### Scene elements

Element kinds: `construction_plane`, `sketch`, `rect`, `line`, `circle`, `constraint`.
Indices are zero-based.

| Instruction | Description |
|---|---|
| `select rect 0` | Select an element (replaces current selection) |
| `select rect 0 bottom` | Select a rectangle edge (`bottom`, `right`, `top`, `left`) |
| `select line 1 add` | Add to selection (`add`, `additive`, or `+`) |
| `clear_selection` / `deselect` | Clear scene selection |
| `element rect 0 hide` | Show / hide / toggle element visibility |
| `set_construction rect 0 top true` | Mark element or edge as construction geometry |
| `apply_construction true` | Set construction flag on draw op or selected targets |
| `toggle_construction` | Toggle construction on draw op or selected targets |
| `set_name line 0 Guide` / `rename rect 1 My box` | Rename an element |
| `focus_name` | Focus the name field in the Context pane |

Visibility and construction values accept `show`/`hide`/`toggle` (or `on`/`off`,
`true`/`false`).

#### Dimensions and constraints

While drawing or editing, set dimensions with expressions. Use `focus_dim` to focus the
corresponding input field.

| Instruction | Description |
|---|---|
| `set_dim width 80` | Rectangle width (also `w`) |
| `set_dim height 50` | Rectangle height (also `h`) |
| `set_dim length 25` | Line length (also `len`, `l`) |
| `set_dim diameter 40` | Circle diameter (also `diam`, `d`) |
| `set_dim offset 12` | Construction plane offset |
| `set_dim angle 45` | Construction plane angle |
| `focus_dim width` | Focus a dimension field (`width`, `height`, `length`, `diameter`, `offset`, `angle`) |
| `edit_dim width` | Begin editing a committed dimension label |
| `commit_dim` | Commit the in-progress dimension edit |
| `set_dim_label_offset width 48` | Nudge a committed dimension label (pixels) |
| `add_constraint line 0 25mm` | Add a distance constraint |
| `add_constraint rect 0 width 2*A` | Constrain rectangle width |
| `add_constraint rect 0 height 50mm` | Constrain rectangle height |
| `add_constraint circle 0 40mm` | Constrain circle diameter |
| `edit_plane 1` | Begin editing a construction plane |
| `commit_plane` | Commit construction plane edits |

#### Parameters

| Instruction | Description |
|---|---|
| `parameter add A 5mm` | Add a named parameter |
| `parameter value 0 A + 5in` | Set a parameter expression by index |
| `parameter name 0 Len` | Rename a parameter |
| `parameter delete 1` | Delete a parameter by index |

#### Camera and view

View commands wait until animated transitions finish before advancing.

| Instruction | Description |
|---|---|
| `orbit 10 5` | Orbit camera (also `right_drag`) |
| `pan 10 5` | Pan camera (also `right_drag_shift`) |
| `wheel 120` | Mouse wheel zoom (also `zoom`, `scroll`) |
| `view front` | Standard view (`front`, `back`, `left`, `right`, `top`, `bottom`; single-letter aliases work) |
| `view edge front_top` | View from a view-cube edge (e.g. `front_bottom`, `right_top`, …) |
| `view corner front_left_top` | View from a view-cube corner (abbreviations like `frt` work) |
| `view orthographic` / `view natural` | Set projection mode |
| `toggle_projection` | Toggle orthographic / natural |
| `view_home` / `home` | Return to the stored home view |
| `set_home_view` / `set_home` | Store the current camera as home |

Synthetic right-drag variants (`right_drag_rel`, `right_drag_pan`) replay pointer input
instead of applying camera actions directly.

#### UI panes and command palette

| Instruction | Description |
|---|---|
| `pane view_cube show` | Show / hide / toggle a pane |
| `pane hierarchy hide` | Elements tree (also `tree`, `dag`) |
| `pane parameters toggle` | Parameters table (also `params`, `param`) |
| `pane context show` | Context pane (also `properties`, `props`) |
| `palette show` / `palette hide` / `palette` | Open, close, or toggle the command palette |
| `palette run view top` | Run the best-matching palette command for a query |

Pane visibility accepts `show`, `hide`, or `toggle` (or `on`/`off`, `true`/`false`).

#### Synthetic input

| Instruction | Description |
|---|---|
| `move 480 320` | Move pointer in the viewport |
| `click 480 320` | Left-click in the viewport |
| `move_ground 0 0` | Move pointer to a ground-plane position (mm) |
| `click_ground 50 25` | Left-click on the ground plane (mm) |
| `drag 100 200 300 400` | Left-button drag from (x0, y0) to (x1, y1) |
| `key enter` | Press and release a key |
| `keydown tab` / `keyup tab` | Hold or release a key |
| `type 12.5` / `type "2in + 5mm"` | Type text into the focused field |

Supported key names: `enter`, `tab`, `escape`/`esc`, `backspace`, `delete`/`del`,
arrow keys (`left`, `right`, `up`, `down`), `space`, letters `a`–`z`, digits `0`–`9`.

#### Sequencing and output

| Instruction | Description |
|---|---|
| `wait 5` | Wait 5 UI frames |
| `wait 100ms` | Wait 100 milliseconds |
| `screenshot path.png` | Capture the viewport to an image file |