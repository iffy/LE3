//! ASCII STL parsing and mesh normalization for HUD assets, plus STL export of bodies.

use crate::extrude::SolidMesh;
use glam::Vec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshTriangle {
    pub vertices: [Vec3; 3],
    pub normal: Vec3,
}

/// Parse triangles from an ASCII STL document.
pub fn parse_ascii_stl(data: &str) -> Result<Vec<MeshTriangle>, String> {
    let mut triangles = Vec::new();
    let mut normal = Vec3::ZERO;
    let mut vertices = Vec::with_capacity(3);

    for line in data.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("facet normal ") {
            normal = parse_vec3(rest).ok_or_else(|| format!("bad facet normal: {line}"))?;
            vertices.clear();
        } else if let Some(rest) = line.strip_prefix("vertex ") {
            let v = parse_vec3(rest).ok_or_else(|| format!("bad vertex: {line}"))?;
            vertices.push(v);
        } else if line == "endfacet" {
            if vertices.len() != 3 {
                return Err(format!(
                    "facet had {} vertices, expected 3",
                    vertices.len()
                ));
            }
            triangles.push(MeshTriangle {
                vertices: [vertices[0], vertices[1], vertices[2]],
                normal,
            });
            vertices.clear();
        }
    }

    if triangles.is_empty() {
        return Err("no triangles found".into());
    }
    Ok(triangles)
}

/// Serialize a solid mesh as an ASCII STL document named `name`. Each triangle's facet
/// normal is derived from its winding (right-hand rule); degenerate triangles get a zero
/// normal. The output round-trips through [`parse_ascii_stl`].
pub fn write_ascii_stl(name: &str, mesh: &SolidMesh) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "solid {name}");
    for tri in &mesh.triangles {
        let [a, b, c] = *tri;
        let n = (b - a).cross(c - a).normalize_or_zero();
        let _ = writeln!(out, "  facet normal {} {} {}", n.x, n.y, n.z);
        out.push_str("    outer loop\n");
        for v in [a, b, c] {
            let _ = writeln!(out, "      vertex {} {} {}", v.x, v.y, v.z);
        }
        out.push_str("    endloop\n");
        out.push_str("  endfacet\n");
    }
    let _ = writeln!(out, "endsolid {name}");
    out
}

fn parse_vec3(s: &str) -> Option<Vec3> {
    let mut parts = s.split_whitespace();
    let x: f32 = parts.next()?.parse().ok()?;
    let y: f32 = parts.next()?.parse().ok()?;
    let z: f32 = parts.next()?.parse().ok()?;
    Some(Vec3::new(x, y, z))
}

/// Center and uniformly scale mesh vertices to fit inside `[-half, half]³`.
pub fn fit_mesh_to_unit_cube(triangles: &[MeshTriangle], half: f32, margin: f32) -> Vec<MeshTriangle> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for tri in triangles {
        for v in tri.vertices {
            min = min.min(v);
            max = max.max(v);
        }
    }
    let center = (min + max) * 0.5;
    let extent = (max - min).max_element();
    let target = (half - margin).max(1e-6);
    let scale = if extent > 1e-8 { target / extent } else { 1.0 };

    triangles
        .iter()
        .map(|tri| MeshTriangle {
            vertices: [
                (tri.vertices[0] - center) * scale,
                (tri.vertices[1] - center) * scale,
                (tri.vertices[2] - center) * scale,
            ],
            normal: tri.normal.normalize_or_zero(),
        })
        .collect()
}

/// Uniformly scale mesh vertices about the origin.
pub fn scale_mesh(triangles: &[MeshTriangle], scale: f32) -> Vec<MeshTriangle> {
    triangles
        .iter()
        .map(|tri| MeshTriangle {
            vertices: tri.vertices.map(|v| v * scale),
            normal: tri.normal,
        })
        .collect()
}

/// Orient the mesh so its longest horizontal axis points toward −Y (HUD front).
#[cfg(test)]
pub fn orient_mesh_front_negative_y(triangles: &[MeshTriangle]) -> Vec<MeshTriangle> {
    triangles
        .iter()
        .map(|tri| MeshTriangle {
            vertices: tri.vertices.map(front_negative_y),
            normal: front_negative_y(tri.normal).normalize_or_zero(),
        })
        .collect()
}

#[cfg(test)]
fn front_negative_y(v: Vec3) -> Vec3 {
    // Rotate +90° about Z so +X (typical STL forward) maps to −Y.
    Vec3::new(v.y, -v.x, v.z)
}

#[cfg(test)]
pub fn mesh_bounds(triangles: &[MeshTriangle]) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for tri in triangles {
        for v in tri.vertices {
            min = min.min(v);
            max = max.max(v);
        }
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEAR_STL: &str = include_str!("assets/bear.stl");

    #[test]
    fn parse_bear_stl_loads_triangles() {
        let tris = parse_ascii_stl(BEAR_STL).expect("bear stl");
        assert!(tris.len() >= 100, "expected many facets, got {}", tris.len());
        assert!(tris.iter().all(|t| t.normal.length() > 0.9));
    }

    #[test]
    fn bear_mesh_fits_in_unit_cube() {
        let raw = parse_ascii_stl(BEAR_STL).expect("bear stl");
        let mesh = orient_mesh_front_negative_y(&fit_mesh_to_unit_cube(&raw, 0.5, 0.04));
        let (min, max) = mesh_bounds(&mesh);
        assert!(min.x >= -0.5 - 1e-5, "min.x = {}", min.x);
        assert!(min.y >= -0.5 - 1e-5, "min.y = {}", min.y);
        assert!(min.z >= -0.5 - 1e-5, "min.z = {}", min.z);
        assert!(max.x <= 0.5 + 1e-5, "max.x = {}", max.x);
        assert!(max.y <= 0.5 + 1e-5, "max.y = {}", max.y);
        assert!(max.z <= 0.5 + 1e-5, "max.z = {}", max.z);
    }

    #[test]
    fn bear_mesh_has_volume() {
        let raw = parse_ascii_stl(BEAR_STL).expect("bear stl");
        let (min, max) = mesh_bounds(&raw);
        let extent = max - min;
        assert!(extent.x > 1.0);
        assert!(extent.y > 0.5);
        assert!(extent.z > 1.0);
    }

    #[test]
    fn parse_ascii_stl_rejects_empty() {
        assert!(parse_ascii_stl("solid empty\nendsolid empty").is_err());
    }

    #[test]
    fn write_ascii_stl_round_trips() {
        let mesh = SolidMesh {
            triangles: vec![
                [
                    Vec3::new(0.0, 0.0, 0.0),
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                ],
                [
                    Vec3::new(0.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                    Vec3::new(0.0, 0.0, 1.0),
                ],
            ],
        };
        let text = write_ascii_stl("part", &mesh);
        assert!(text.starts_with("solid part"));
        assert!(text.trim_end().ends_with("endsolid part"));
        let parsed = parse_ascii_stl(&text).expect("round-trip parse");
        assert_eq!(parsed.len(), 2);
        // First triangle lies in the z=0 plane, so its normal is ±Z (here +Z by winding).
        assert!((parsed[0].normal - Vec3::Z).length() < 1e-5, "normal {:?}", parsed[0].normal);
        assert_eq!(parsed[0].vertices[1], Vec3::new(1.0, 0.0, 0.0));
    }
}