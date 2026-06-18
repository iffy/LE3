# LE3 — Local CAD

On-device parametric CAD. See [SPEC.md](SPEC.md) for the full design.

## Status

Very early prototype. Currently implemented:

- An egui GUI with a 3D viewport (orbit camera, projected with egui's painter).
- A **Rectangle** tool: draw rectangles on the ground plane (XY, z = 0).
- **Save / Open** documents as `.le3` files (SQLite, per SPEC §7).
- **Instruction scripts** (SPEC §9.3): drive the live UI from a file — mouse,
  keyboard, camera, document actions, and screenshots.

Not yet implemented: the wgpu/OCCT 3D viewport, action DAG, parameters,
constraints, Lua API, CLI subcommands, and everything else in the spec.

## Run

```sh
cargo run
```

- Select the **Rectangle** tool, **left-click** to fix the first corner, move the
  mouse to size, type dimensions if needed, then **Enter** to commit.
- **Right-drag** to orbit; **Shift+right-drag** to pan; **mouse wheel** to zoom.
- **Escape** cancels an in-progress draw; press again to return to the Select tool.
- **Save / Save As…** writes a `.le3` SQLite file; **Open…** loads one back.
- **Clear** removes all rectangles; **Undo last** drops the most recent.

## Scripting

The app can be driven programmatically with a human-readable instruction file.
This is intended for automation and visual-regression testing (SPEC §9.3).

```sh
# Run a script and exit when it finishes
cargo run -- --script examples/rectangle.le3script --exit

# Same thing — positional script path also works
cargo run -- examples/rectangle.le3script --exit
```

Scripts are one instruction per line. Lines starting with `#` are comments.
Viewport `x`/`y` coordinates are pixels relative to the 3D panel (below the
toolbar). Use `click_ground` / `move_ground` for millimetre positions on the
XY ground plane.

See [examples/rectangle.le3script](examples/rectangle.le3script) for a full
example that draws a rectangle (click, move, dimension entry, Enter) and saves
a screenshot.

Other useful instructions:

| Instruction | Description |
|---|---|
| `open path/to/doc.le3` | Open a document (no file dialog) |
| `save [path]` | Save to the given path, or the current path |
| `key escape` | Cancel in-progress operation / switch to Select |
| `orbit 10 5` | Orbit camera by (dx, dy) pixels |
| `pan 10 5` | Pan camera |
| `wheel 120` | Zoom with mouse wheel delta |
| `click_ground 0 0` | Left-click at world position (mm) |
| `pane view_cube hide` | Show/hide/`toggle` a pane (View ▸ Panes menu) |
| `wait 100ms` | Pause for 100 milliseconds |
| `quit` | Close the app when the script ends |

See `src/script.rs` for the full instruction vocabulary.

## Test

```sh
cargo test
```
