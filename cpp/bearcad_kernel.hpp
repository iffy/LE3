// Thin C ABI over the OpenCASCADE (OCCT) geometry kernel.
//
// This header is the *entire* FFI surface BearCAD's Rust code links against when
// built with `--features occt`. Keeping it a flat `extern "C"` C ABI (no C++
// types cross the boundary) is deliberate: it isolates OCCT's heavy C++ API
// behind a stable, `bindgen`-free seam, per SPEC.md §10 ("isolate FFI behind a
// safe `kernel` module").
//
// Everything here must stay ABI-compatible with the `extern "C"` block in
// `src/kernel/mod.rs`.

#ifndef BEARCAD_KERNEL_HPP
#define BEARCAD_KERNEL_HPP

#ifdef __cplusplus
extern "C" {
#endif

// Build an axis-aligned box of the given extents via OCCT and return its solid
// volume (BRepGProp mass properties). This is the pilot round-trip that proves
// the whole FoundationClasses -> ModelingData -> ModelingAlgorithms toolchain is
// linked and callable. Returns a negative value if OCCT threw.
double bearcad_kernel_box_volume(double dx, double dy, double dz);

// OCCT version string (e.g. "8.0.0"), as a static NUL-terminated buffer owned by
// the shim (never freed by the caller).
const char* bearcad_kernel_occt_version(void);

// ---------------------------------------------------------------------------
// Solid modeling: build real BREP solids, combine them with booleans, and read
// back volume / a triangulated mesh. `BearcadShape` is an opaque owned handle
// (a heap TopoDS_Shape); free every non-NULL handle with bearcad_shape_free.
// ---------------------------------------------------------------------------

typedef struct BearcadShape BearcadShape;

// Extrude a closed planar profile (a loop of `n_pts` 3D points, `xyz` laid out
// x,y,z,x,y,z,...; the loop is closed implicitly, do not repeat the first point)
// along the vector (dx,dy,dz). Returns NULL on failure (degenerate profile, OCCT
// error, fewer than 3 points).
BearcadShape* bearcad_shape_prism(const double* xyz, unsigned long n_pts,
                                  double dx, double dy, double dz);

// Solid lofted (ruled ThruSections) between a bottom and a top loop, each a
// closed `n_pts`-point loop in point-for-point correspondence (`bottom_xyz` /
// `top_xyz`, x,y,z,...). Handles a slanted top (per-vertex offset), unlike the
// single-vector prism. NULL on failure.
BearcadShape* bearcad_shape_loft(const double* bottom_xyz, const double* top_xyz,
                                 unsigned long n_pts);

// Boolean combine two shapes into a new owned shape (inputs untouched). `op`:
// 0 = fuse (a ∪ b), 1 = cut (a − b), 2 = common (a ∩ b). NULL on failure.
BearcadShape* bearcad_shape_boolean(const BearcadShape* a, const BearcadShape* b, int op);

// Apply fillets of the given radii to the shape's edges whose two endpoints match
// each (ax,ay,az,bx,by,bz) 6-tuple in `edges` (n edges, radii[n]). Edge match =
// both endpoints within tol of the OCCT edge's two vertices (either order). All
// requested edges are added to one BRepFilletAPI_MakeFillet before building.
// Returns a new owned shape, or NULL on failure (an edge not found, OCCT error).
BearcadShape* bearcad_shape_fillet(const BearcadShape* s, const double* edges,
                                   const double* radii, unsigned long n);

// Same as bearcad_shape_fillet, but a symmetric chamfer of the given distances via
// BRepFilletAPI_MakeChamfer::Add(dist, edge). NULL on failure.
BearcadShape* bearcad_shape_chamfer(const BearcadShape* s, const double* edges,
                                    const double* dists, unsigned long n);

// Solid volume via BRepGProp mass properties. Negative on error.
double bearcad_shape_volume(const BearcadShape* shape);

// Triangulate the shape at the given linear deflection and return a freshly
// allocated flat array of 9*(*out_tri_count) doubles (three xyz vertices per
// triangle, outward-oriented). Free with bearcad_tri_free. NULL / zero count on
// failure or an empty shape.
double* bearcad_shape_tessellate(const BearcadShape* shape, double deflection,
                                 unsigned long* out_tri_count);
void bearcad_tri_free(double* tris);

void bearcad_shape_free(BearcadShape* shape);

// ---------------------------------------------------------------------------
// STEP (ISO 10303-21) data exchange via OCCT (#65/#71): real BREP, including
// curved surfaces, not just the hand-rolled faceted subset.
// ---------------------------------------------------------------------------

// Write `s` to `path` as an AP214 STEP file. Returns 0 on success, nonzero on
// failure (OCCT error, write failure).
int bearcad_shape_write_step(const BearcadShape* s, const char* path);

// Read the first/combined shape from a STEP file at `path` (curved surfaces
// included). Returns a new owned shape, or NULL on failure / an empty file.
BearcadShape* bearcad_read_step(const char* path);

#ifdef __cplusplus
}
#endif

#endif // BEARCAD_KERNEL_HPP
