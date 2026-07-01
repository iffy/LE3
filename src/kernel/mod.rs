//! Safe Rust surface over the OpenCASCADE (OCCT) geometry kernel.
//!
//! OCCT is an optional, statically-linked native dependency gated behind the
//! `occt` Cargo feature (off by default so the normal build and CI don't need a
//! C++ toolchain or a built OCCT). All `unsafe` FFI lives here; the rest of the
//! app calls the safe functions below and gets a graceful "not available" answer
//! when the kernel wasn't compiled in — see SPEC.md §10.
//!
//! To build with the kernel: see `README.md` ("Building with the OCCT kernel").

#[cfg(feature = "occt")]
mod ffi {
    use std::os::raw::{c_char, c_int, c_ulong};

    /// Opaque owned BREP shape handle (a heap `TopoDS_Shape` in the shim).
    #[repr(C)]
    pub struct BearcadShape {
        _private: [u8; 0],
    }

    // Must stay ABI-compatible with cpp/bearcad_kernel.hpp.
    unsafe extern "C" {
        pub fn bearcad_kernel_box_volume(dx: f64, dy: f64, dz: f64) -> f64;
        pub fn bearcad_kernel_occt_version() -> *const c_char;

        pub fn bearcad_shape_prism(
            xyz: *const f64,
            n_pts: c_ulong,
            dx: f64,
            dy: f64,
            dz: f64,
        ) -> *mut BearcadShape;
        pub fn bearcad_shape_loft(
            bottom_xyz: *const f64,
            top_xyz: *const f64,
            n_pts: c_ulong,
        ) -> *mut BearcadShape;
        pub fn bearcad_shape_boolean(
            a: *const BearcadShape,
            b: *const BearcadShape,
            op: c_int,
        ) -> *mut BearcadShape;
        pub fn bearcad_shape_volume(shape: *const BearcadShape) -> f64;
        pub fn bearcad_shape_tessellate(
            shape: *const BearcadShape,
            deflection: f64,
            out_tri_count: *mut c_ulong,
        ) -> *mut f64;
        pub fn bearcad_tri_free(tris: *mut f64);
        pub fn bearcad_shape_free(shape: *mut BearcadShape);
    }
}

/// Volume of an axis-aligned box, computed by the OCCT kernel. `None` when the
/// kernel isn't compiled in; `None` also on a kernel-side failure (the shim
/// returns a negative sentinel rather than unwinding a C++ exception across FFI).
///
/// Part of the kernel's public API surface; only exercised (by [`selftest`] and
/// the pilot tests) in `occt` builds, hence inert/dead in the default build.
#[cfg_attr(not(feature = "occt"), allow(dead_code))]
pub fn box_volume(dx: f64, dy: f64, dz: f64) -> Option<f64> {
    #[cfg(feature = "occt")]
    {
        let v = unsafe { ffi::bearcad_kernel_box_volume(dx, dy, dz) };
        (v >= 0.0).then_some(v)
    }
    #[cfg(not(feature = "occt"))]
    {
        let _ = (dx, dy, dz);
        None
    }
}

/// Linked OCCT version string (e.g. `"8.0.0"`), or `None` when the kernel isn't
/// compiled in. Inert/dead in the default build, like [`box_volume`].
#[cfg_attr(not(feature = "occt"), allow(dead_code))]
pub fn occt_version() -> Option<String> {
    #[cfg(feature = "occt")]
    {
        let ptr = unsafe { ffi::bearcad_kernel_occt_version() };
        if ptr.is_null() {
            return None;
        }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) };
        s.to_str().ok().map(str::to_owned)
    }
    #[cfg(not(feature = "occt"))]
    {
        None
    }
}

/// One-line human-readable kernel status, used by the Help ▸ About message so a
/// user (or a bug report) can tell at a glance whether this build has a real
/// geometry kernel. Doubles as the pilot round-trip self-check: with the kernel
/// linked it actually calls OCCT (build a 1×2×3 box, expect volume ≈ 6).
pub fn selftest() -> String {
    #[cfg(feature = "occt")]
    {
        match box_volume(1.0, 2.0, 3.0) {
            Some(v) if (v - 6.0).abs() < 1e-6 => {
                let ver = occt_version().unwrap_or_else(|| "unknown".to_string());
                format!("OCCT kernel {ver}: OK (box self-check passed)")
            }
            Some(v) => format!("OCCT kernel: self-check FAILED (box volume {v} != 6)"),
            None => "OCCT kernel: self-check FAILED (kernel error)".to_string(),
        }
    }
    #[cfg(not(feature = "occt"))]
    {
        "OCCT kernel: not compiled in (build with --features occt)".to_string()
    }
}

/// Boolean operation on two [`Shape`]s. `Fuse` drives body union today; `Cut`
/// and `Common` are exercised by tests and land in app code with extrude
/// cut/intersect mode (#35), hence `allow(dead_code)` for the unused variants.
#[cfg(feature = "occt")]
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    /// `a ∪ b`.
    Fuse,
    /// `a − b`.
    Cut,
    /// `a ∩ b`.
    Common,
}

/// An owned OCCT BREP solid. Real geometry, not a mesh: built from profiles,
/// combined with booleans, and only tessellated into triangles at the end for the
/// viewport. Only available in `occt` builds — the migration off the hand-rolled
/// mesh code onto this type is incremental and feature-gated (#86).
#[cfg(feature = "occt")]
pub struct Shape {
    raw: *mut ffi::BearcadShape,
}

#[cfg(feature = "occt")]
impl Shape {
    /// Extrude a closed planar profile loop (world-space points, first point not
    /// repeated) along `dir`. `None` on a degenerate profile or kernel failure.
    pub fn prism(profile: &[glam::Vec3], dir: glam::Vec3) -> Option<Shape> {
        if profile.len() < 3 {
            return None;
        }
        let mut xyz = Vec::with_capacity(profile.len() * 3);
        for p in profile {
            xyz.push(p.x as f64);
            xyz.push(p.y as f64);
            xyz.push(p.z as f64);
        }
        let raw = unsafe {
            ffi::bearcad_shape_prism(
                xyz.as_ptr(),
                profile.len() as std::os::raw::c_ulong,
                dir.x as f64,
                dir.y as f64,
                dir.z as f64,
            )
        };
        (!raw.is_null()).then_some(Shape { raw })
    }

    /// Solid lofted between a bottom and top loop in point-for-point
    /// correspondence (same length ≥ 3). Handles a slanted top, unlike
    /// [`Shape::prism`]. `None` on mismatch or kernel failure.
    pub fn loft(bottom: &[glam::Vec3], top: &[glam::Vec3]) -> Option<Shape> {
        if bottom.len() < 3 || bottom.len() != top.len() {
            return None;
        }
        let flat = |pts: &[glam::Vec3]| -> Vec<f64> {
            pts.iter()
                .flat_map(|p| [p.x as f64, p.y as f64, p.z as f64])
                .collect()
        };
        let b = flat(bottom);
        let t = flat(top);
        let raw = unsafe {
            ffi::bearcad_shape_loft(
                b.as_ptr(),
                t.as_ptr(),
                bottom.len() as std::os::raw::c_ulong,
            )
        };
        (!raw.is_null()).then_some(Shape { raw })
    }

    /// Boolean-combine `self` and `other` into a new shape. `None` on failure.
    pub fn boolean(&self, other: &Shape, op: BoolOp) -> Option<Shape> {
        let code = match op {
            BoolOp::Fuse => 0,
            BoolOp::Cut => 1,
            BoolOp::Common => 2,
        };
        let raw = unsafe { ffi::bearcad_shape_boolean(self.raw, other.raw, code) };
        (!raw.is_null()).then_some(Shape { raw })
    }

    /// Solid volume, or `None` on a kernel error (negative sentinel).
    /// (Kernel API; exercised by tests, consumed by app code incrementally.)
    #[allow(dead_code)]
    pub fn volume(&self) -> Option<f64> {
        let v = unsafe { ffi::bearcad_shape_volume(self.raw) };
        (v >= 0.0).then_some(v)
    }

    /// Triangulate into outward-oriented triangles (world space) at the given
    /// linear deflection. Empty on failure or an empty shape.
    pub fn tessellate(&self, deflection: f64) -> Vec<[glam::Vec3; 3]> {
        let mut count: std::os::raw::c_ulong = 0;
        let ptr = unsafe { ffi::bearcad_shape_tessellate(self.raw, deflection, &mut count) };
        if ptr.is_null() || count == 0 {
            return Vec::new();
        }
        let n = count as usize;
        let doubles = unsafe { std::slice::from_raw_parts(ptr, n * 9) };
        let mut tris = Vec::with_capacity(n);
        for t in 0..n {
            let b = t * 9;
            let v = |o: usize| {
                glam::Vec3::new(
                    doubles[b + o] as f32,
                    doubles[b + o + 1] as f32,
                    doubles[b + o + 2] as f32,
                )
            };
            tris.push([v(0), v(3), v(6)]);
        }
        unsafe { ffi::bearcad_tri_free(ptr) };
        tris
    }
}

#[cfg(feature = "occt")]
impl Drop for Shape {
    fn drop(&mut self) {
        unsafe { ffi::bearcad_shape_free(self.raw) }
    }
}

#[cfg(all(test, feature = "occt"))]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn box_volume_round_trips_through_occt() {
        let v = box_volume(2.0, 3.0, 4.0).expect("kernel available in occt build");
        assert!((v - 24.0).abs() < 1e-6, "box volume {v} != 24");
    }

    #[test]
    fn selftest_passes_when_kernel_linked() {
        assert!(selftest().contains("OK"), "{}", selftest());
    }

    fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> [Vec3; 4] {
        [
            Vec3::new(x0, y0, 0.0),
            Vec3::new(x1, y0, 0.0),
            Vec3::new(x1, y1, 0.0),
            Vec3::new(x0, y1, 0.0),
        ]
    }

    /// Signed volume of a triangle soup via the divergence theorem — a mesh
    /// integrity check independent of OCCT's own volume computation.
    fn mesh_volume(tris: &[[Vec3; 3]]) -> f32 {
        tris.iter()
            .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
            .sum::<f32>()
            .abs()
    }

    #[test]
    fn prism_from_square_has_expected_volume() {
        let sh = Shape::prism(&square(0.0, 0.0, 1.0, 1.0), Vec3::new(0.0, 0.0, 5.0))
            .expect("prism built");
        assert!((sh.volume().unwrap() - 5.0).abs() < 1e-6);
    }

    #[test]
    fn prism_tessellation_is_watertight_by_volume() {
        let sh = Shape::prism(&square(0.0, 0.0, 2.0, 3.0), Vec3::new(0.0, 0.0, 4.0))
            .expect("prism built");
        let tris = sh.tessellate(0.01);
        assert!(!tris.is_empty());
        // A watertight closed mesh's divergence-theorem volume matches the solid.
        assert!((mesh_volume(&tris) - 24.0).abs() < 1e-3, "mesh vol {}", mesh_volume(&tris));
    }

    #[test]
    fn loft_with_slanted_top_has_average_height_volume() {
        // Unit-square base at z=0; top square with the same x,y but z rising
        // linearly 1→2 across x. Volume = base area (1) × average height (1.5).
        let bottom = square(0.0, 0.0, 1.0, 1.0);
        let top = [
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 2.0),
            Vec3::new(1.0, 1.0, 2.0),
            Vec3::new(0.0, 1.0, 1.0),
        ];
        let sh = Shape::loft(&bottom, &top).expect("loft built");
        assert!((sh.volume().unwrap() - 1.5).abs() < 1e-4, "vol {:?}", sh.volume());
    }

    #[test]
    fn booleans_of_two_overlapping_boxes_have_expected_volumes() {
        // Box A: [0,2]×[0,2]×[0,2] (vol 8). Box B: [1,3]×[0,2]×[0,2] (vol 8).
        // Overlap [1,2]×[0,2]×[0,2] = vol 4.
        let a = Shape::prism(&square(0.0, 0.0, 2.0, 2.0), Vec3::new(0.0, 0.0, 2.0)).unwrap();
        let b = Shape::prism(&square(1.0, 0.0, 3.0, 2.0), Vec3::new(0.0, 0.0, 2.0)).unwrap();

        let fuse = a.boolean(&b, BoolOp::Fuse).unwrap().volume().unwrap();
        let cut = a.boolean(&b, BoolOp::Cut).unwrap().volume().unwrap();
        let common = a.boolean(&b, BoolOp::Common).unwrap().volume().unwrap();

        assert!((fuse - 12.0).abs() < 1e-4, "fuse {fuse}");
        assert!((cut - 4.0).abs() < 1e-4, "cut {cut}");
        assert!((common - 4.0).abs() < 1e-4, "common {common}");
    }
}
