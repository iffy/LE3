# Third-Party Licenses

BearCAD is distributed under the terms of **MIT OR Apache-2.0** (at your option).
It builds on open-source components whose licenses are reproduced or referenced
below. This document exists to satisfy the attribution and notice requirements of
those licenses; it is linked from **Help ▸ Licenses** in the app.

If you redistribute BearCAD (source or binary), include this file (or an
equivalent notice) so downstream recipients receive the same information.

---

## OpenCASCADE Technology (OCCT) — LGPL 2.1 with exception

The geometry kernel is [OpenCASCADE Technology](https://dev.opencascade.org/),
licensed under the **GNU Lesser General Public License, version 2.1**, with the
[OCCT LGPL exception](third_party/OCCT/OCCT_LGPL_EXCEPTION.txt). Full text:

- LGPL 2.1: [`third_party/OCCT/LICENSE_LGPL_21.txt`](third_party/OCCT/LICENSE_LGPL_21.txt)
- OCCT exception: [`third_party/OCCT/OCCT_LGPL_EXCEPTION.txt`](third_party/OCCT/OCCT_LGPL_EXCEPTION.txt)

BearCAD links OCCT **statically**. The LGPL permits static linking provided
recipients can relink the application against a modified or different version of
OCCT. BearCAD satisfies this by:

1. building OCCT from pinned, publicly available source — the
   [`third_party/OCCT`](third_party/OCCT) git submodule (upstream:
   <https://github.com/Open-Cascade-SAS/OCCT>); and
2. supporting relinking against any OCCT build via the `OCCT_DIR` environment
   variable, with full instructions in [`README.md`](README.md#building-with-the-occt-kernel).

The OCCT source corresponding to any binary release is the submodule commit
recorded in that release's git tree. OCCT's own copyright notices are retained in
the submodule source.

---

## Rust dependencies

The following direct dependencies are compiled into BearCAD. Each is used
unmodified from [crates.io](https://crates.io), where its source and full license
text are available. (This lists notable direct dependencies; the complete
transitive graph — reproducible with `cargo tree` / `cargo about` — is dominated
by the permissive MIT / Apache-2.0 / BSD family.)

| Component | Role | License |
| --- | --- | --- |
| eframe / egui | GUI framework | MIT OR Apache-2.0 |
| wgpu (via eframe) | GPU rendering backend | MIT OR Apache-2.0 |
| rusqlite | SQLite document storage | MIT |
| SQLite (bundled by rusqlite) | embedded database engine | Public Domain |
| rfd | native file dialogs | MIT |
| serde / serde_json | serialization | MIT OR Apache-2.0 |
| glam | vector/matrix math | MIT OR Apache-2.0 |
| image | image decoding (icons) | MIT OR Apache-2.0 |
| resvg | SVG rasterization (icons) | **MPL-2.0** |
| usvg | SVG parsing (icons) | **MPL-2.0** |
| tiny-skia | 2D rasterizer (icons) | BSD-3-Clause |
| bytemuck | POD casting for GPU buffers | Zlib OR Apache-2.0 OR MIT |
| mlua | Lua scripting bindings | MIT |
| Lua 5.4 (vendored by mlua) | scripting language | MIT |
| muda | native menu bar | Apache-2.0 OR MIT |
| ico / winres | Windows icon/resource embedding (build) | MIT |
| cc | C++ shim compilation (build, `occt` feature) | MIT OR Apache-2.0 |

### A note on the MPL-2.0 components (resvg, usvg)

The Mozilla Public License 2.0 is file-level copyleft. BearCAD uses these crates
**unmodified**, so the obligation is limited to preserving their notices and
making their source available — which crates.io does. No BearCAD source is
affected by the MPL.

---

## Full license texts

- Apache-2.0: <https://www.apache.org/licenses/LICENSE-2.0>
- MIT: <https://opensource.org/license/mit>
- BSD-3-Clause: <https://opensource.org/license/bsd-3-clause>
- MPL-2.0: <https://www.mozilla.org/MPL/2.0/>
- LGPL-2.1: [`third_party/OCCT/LICENSE_LGPL_21.txt`](third_party/OCCT/LICENSE_LGPL_21.txt)
- Zlib: <https://opensource.org/license/zlib>
