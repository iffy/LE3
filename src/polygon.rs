//! Closed-polygon face detection (#66): any set of plain `Line` entities that connect
//! end-to-end into a closed loop (via `Coincident` point constraints) can be used as a
//! face, the same way a `Rect` or `Circle` profile can.

use crate::document_lifecycle::line_alive;
use crate::model::{ConstraintPoint, Document, LineEnd, SketchId};
use crate::vertex_drag::coincident_group;

/// Canonical id for the vertex group a line endpoint belongs to: the lexicographically
/// smallest `(line, is_end)` among every `LineEndpoint` transitively coincident with it
/// (via `Coincident` constraints). Two endpoints share a vertex iff this key matches.
fn vertex_key(doc: &Document, sketch: SketchId, line: usize, end: LineEnd) -> (usize, bool) {
    coincident_group(doc, sketch, ConstraintPoint::LineEndpoint { line, end })
        .into_iter()
        .filter_map(|p| match p {
            ConstraintPoint::LineEndpoint { line, end } => {
                Some((line, matches!(end, LineEnd::End)))
            }
            _ => None,
        })
        .min()
        .unwrap_or((line, matches!(end, LineEnd::End)))
}

/// Every closed loop of connected `Line`s in `sketch`, as ordered line indices.
///
/// A loop is any simple cycle in the graph whose nodes are vertex groups and whose edges
/// are lines (no line repeated within a loop). Loops are deduped by their line-index set
/// (so the same polygon found by walking it in either direction, or starting from a
/// different line, is reported once), and returned in a deterministic order: sorted by
/// their lowest-numbered line, then by length.
pub fn closed_line_loops(doc: &Document, sketch: SketchId) -> Vec<Vec<usize>> {
    let lines: Vec<usize> = doc
        .lines
        .iter()
        .enumerate()
        .filter(|(i, l)| l.sketch == sketch && line_alive(doc, *i))
        .map(|(i, _)| i)
        .collect();
    if lines.len() < 3 {
        return Vec::new();
    }

    // For each line, the vertex key at its start and end.
    let endpoints: std::collections::HashMap<usize, ((usize, bool), (usize, bool))> = lines
        .iter()
        .map(|&i| {
            (
                i,
                (
                    vertex_key(doc, sketch, i, LineEnd::Start),
                    vertex_key(doc, sketch, i, LineEnd::End),
                ),
            )
        })
        .collect();

    // Lines incident to each vertex key, paired with which of their own endpoints sits there.
    let mut incident: std::collections::HashMap<(usize, bool), Vec<(usize, bool)>> =
        std::collections::HashMap::new();
    for (&line, &(start_key, end_key)) in &endpoints {
        incident.entry(start_key).or_default().push((line, false));
        incident.entry(end_key).or_default().push((line, true));
    }

    let mut found: Vec<Vec<usize>> = Vec::new();
    let mut seen_sets: std::collections::HashSet<Vec<usize>> = std::collections::HashSet::new();

    for &start_line in &lines {
        // Walk from `start_line`'s end vertex, looking for a path back to its start vertex.
        let mut path = vec![start_line];
        let mut used: std::collections::HashSet<usize> = std::collections::HashSet::new();
        used.insert(start_line);
        let (_, first_end_key) = endpoints[&start_line];
        walk(
            &incident,
            &endpoints,
            first_end_key,
            &mut path,
            &mut used,
            &mut found,
            &mut seen_sets,
        );
    }

    found.sort_by(|a, b| {
        let min_a = *a.iter().min().unwrap();
        let min_b = *b.iter().min().unwrap();
        min_a.cmp(&min_b).then(a.len().cmp(&b.len()))
    });
    found
}

fn walk(
    incident: &std::collections::HashMap<(usize, bool), Vec<(usize, bool)>>,
    endpoints: &std::collections::HashMap<usize, ((usize, bool), (usize, bool))>,
    current: (usize, bool),
    path: &mut Vec<usize>,
    used: &mut std::collections::HashSet<usize>,
    found: &mut Vec<Vec<usize>>,
    seen_sets: &mut std::collections::HashSet<Vec<usize>>,
) {
    if path.len() > 64 {
        // Defensive bound against pathological inputs; real sketches are tiny.
        return;
    }
    let Some(candidates) = incident.get(&current) else {
        return;
    };
    for &(next_line, at_end) in candidates {
        if next_line == *path.last().unwrap() {
            continue;
        }
        if next_line == path[0] {
            // Back to the start: only a real loop once we've used at least 3 lines.
            if path.len() >= 3 {
                let mut set: Vec<usize> = path.clone();
                set.sort_unstable();
                if seen_sets.insert(set) {
                    found.push(path.clone());
                }
            }
            continue;
        }
        if used.contains(&next_line) {
            continue;
        }
        let (start_key, end_key) = endpoints[&next_line];
        let next_vertex = if at_end { start_key } else { end_key };
        path.push(next_line);
        used.insert(next_line);
        walk(
            incident, endpoints, next_vertex, path, used, found, seen_sets,
        );
        used.remove(&next_line);
        path.pop();
    }
}

/// The boundary vertices (local sketch coordinates) of a closed loop, in order: vertex `i`
/// is the endpoint of `lines[i]` shared with `lines[i - 1]` (wrapping around) — i.e. each
/// line is walked in whichever direction continues the loop, regardless of which endpoint
/// is stored as that line's `Start`/`End`. A curved (bezier) line contributes its entry
/// point plus intermediate sampled points (its exit point is the next line's entry point),
/// so the returned vertex count can exceed `lines.len()`.
///
/// Returns `None` if the lines don't actually form a closed loop (consecutive lines, with
/// wraparound, must share a vertex via a `Coincident` constraint).
pub fn loop_vertices_uv(doc: &Document, sketch: SketchId, lines: &[usize]) -> Option<Vec<(f32, f32)>> {
    if lines.len() < 3 {
        return None;
    }
    let keys: Vec<((usize, bool), (usize, bool))> = lines
        .iter()
        .map(|&i| {
            (
                vertex_key(doc, sketch, i, LineEnd::Start),
                vertex_key(doc, sketch, i, LineEnd::End),
            )
        })
        .collect();

    let mut vertices = Vec::new();
    for i in 0..lines.len() {
        let prev = (i + lines.len() - 1) % lines.len();
        let (prev_start, prev_end) = keys[prev];
        let (start, end) = keys[i];
        let reversed = if start == prev_start || start == prev_end {
            false
        } else if end == prev_start || end == prev_end {
            true
        } else {
            return None;
        };
        let line = doc.lines.get(lines[i])?;
        let mut sampled = line.sample_local(crate::model::BEZIER_SEGMENTS);
        if reversed {
            sampled.reverse();
        }
        sampled.pop(); // the exit point is the next line's entry point
        vertices.extend(sampled);
    }
    Some(vertices)
}

/// Ear-clipping triangulation of a simple (possibly concave) 2D polygon. `vertices` are
/// ordered boundary points; returns `n - 2` triangles as index triples into `vertices`.
pub fn triangulate_uv(vertices: &[(f32, f32)]) -> Vec<[usize; 3]> {
    let n = vertices.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[0, 1, 2]];
    }

    let ccw = signed_area_2d(vertices) > 0.0;
    let mut indices: Vec<usize> = (0..n).collect();
    let mut triangles = Vec::with_capacity(n - 2);

    let mut guard = 0;
    while indices.len() > 3 {
        if guard > n * n {
            break;
        }
        guard += 1;
        let mut ear_found = false;
        let len = indices.len();
        for i in 0..len {
            let prev = indices[(i + len - 1) % len];
            let curr = indices[i];
            let next = indices[(i + 1) % len];
            if !is_convex_vertex_2d(vertices[prev], vertices[curr], vertices[next], ccw) {
                continue;
            }
            let tri = [vertices[prev], vertices[curr], vertices[next]];
            let contains_other = indices.iter().any(|&idx| {
                idx != prev
                    && idx != curr
                    && idx != next
                    && point_in_triangle_2d(vertices[idx], tri[0], tri[1], tri[2])
            });
            if contains_other {
                continue;
            }
            triangles.push([prev, curr, next]);
            indices.remove(i);
            ear_found = true;
            break;
        }
        if !ear_found {
            break;
        }
    }
    if indices.len() == 3 {
        triangles.push([indices[0], indices[1], indices[2]]);
    }
    triangles
}

/// Triangulate a simple planar polygon in world space (same winding as the boundary loop).
pub fn triangulate_planar(vertices: &[glam::Vec3], normal: glam::Vec3) -> Vec<[usize; 3]> {
    if vertices.len() < 3 {
        return Vec::new();
    }
    let uv = project_planar_uv(vertices, normal);
    triangulate_uv(&uv)
}

fn project_planar_uv(vertices: &[glam::Vec3], normal: glam::Vec3) -> Vec<(f32, f32)> {
    let n = normal.normalize_or_zero();
    let mut u_axis = if n.z.abs() < 0.9 {
        glam::Vec3::Z.cross(n)
    } else {
        glam::Vec3::X.cross(n)
    };
    u_axis = u_axis.normalize_or_zero();
    let v_axis = n.cross(u_axis).normalize_or_zero();
    let origin = vertices[0];
    vertices
        .iter()
        .map(|p| {
            let rel = *p - origin;
            (rel.dot(u_axis), rel.dot(v_axis))
        })
        .collect()
}

fn signed_area_2d(vertices: &[(f32, f32)]) -> f32 {
    let mut area = 0.0;
    for i in 0..vertices.len() {
        let j = (i + 1) % vertices.len();
        area += vertices[i].0 * vertices[j].1 - vertices[j].0 * vertices[i].1;
    }
    area * 0.5
}

fn is_convex_vertex_2d(prev: (f32, f32), curr: (f32, f32), next: (f32, f32), ccw: bool) -> bool {
    let cross = (curr.0 - prev.0) * (next.1 - prev.1) - (curr.1 - prev.1) * (next.0 - prev.0);
    if ccw {
        cross > 1e-6
    } else {
        cross < -1e-6
    }
}

pub(crate) fn point_in_triangle_2d(
    p: (f32, f32),
    a: (f32, f32),
    b: (f32, f32),
    c: (f32, f32),
) -> bool {
    let v0 = (c.0 - a.0, c.1 - a.1);
    let v1 = (b.0 - a.0, b.1 - a.1);
    let v2 = (p.0 - a.0, p.1 - a.1);
    let dot00 = v0.0 * v0.0 + v0.1 * v0.1;
    let dot01 = v0.0 * v1.0 + v0.1 * v1.1;
    let dot02 = v0.0 * v2.0 + v0.1 * v2.1;
    let dot11 = v1.0 * v1.0 + v1.1 * v1.1;
    let dot12 = v1.0 * v2.0 + v1.1 * v2.1;
    let denom = dot00 * dot11 - dot01 * dot01;
    if denom.abs() < 1e-8 {
        return false;
    }
    let inv = 1.0 / denom;
    let u = (dot11 * dot02 - dot01 * dot12) * inv;
    let v = (dot00 * dot12 - dot01 * dot02) * inv;
    u >= -1e-4 && v >= -1e-4 && (u + v) <= 1.0 + 1e-4
}

/// Even-odd (ray-casting) point-in-polygon test; winding-independent. Used both by tests and,
/// at runtime, to resolve which atomic boolean region (#16/#62) a click landed in.
pub(crate) fn point_in_polygon_2d(p: (f32, f32), vertices: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = vertices.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let (xi, yi) = vertices[i];
        let (xj, yj) = vertices[j];
        let intersects = (yi > p.1) != (yj > p.1)
            && p.0 < (xj - xi) * (p.1 - yi) / (yj - yi) + xi;
        if intersects {
            inside = !inside;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Constraint, ConstraintEntity, ConstraintKind, Line};

    fn coincident(sketch: SketchId, a: ConstraintPoint, b: ConstraintPoint) -> Constraint {
        Constraint {
            sketch,
            kind: ConstraintKind::Coincident {
                a: ConstraintEntity::Point(a),
                b: ConstraintEntity::Point(b),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        }
    }

    fn line(sketch: SketchId, x0: f32, y0: f32, x1: f32, y1: f32) -> Line {
        Line::from_local_endpoints(sketch, x0, y0, x1, y1)
    }

    fn point(line: usize, end: LineEnd) -> ConstraintPoint {
        ConstraintPoint::LineEndpoint { line, end }
    }

    #[test]
    fn three_lines_closed_into_a_triangle_form_one_loop() {
        let mut doc = Document::default();
        doc.add_sketch(crate::model::FaceId::ConstructionPlane(0));
        // Three lines, each one's end coincident with the next one's start, closing back.
        doc.lines.push(line(0, 0.0, 0.0, 10.0, 0.0));
        doc.lines.push(line(0, 10.0, 0.0, 5.0, 8.0));
        doc.lines.push(line(0, 5.0, 8.0, 0.0, 0.0));
        doc.constraints.push(coincident(
            0,
            point(0, LineEnd::End),
            point(1, LineEnd::Start),
        ));
        doc.constraints.push(coincident(
            0,
            point(1, LineEnd::End),
            point(2, LineEnd::Start),
        ));
        doc.constraints.push(coincident(
            0,
            point(2, LineEnd::End),
            point(0, LineEnd::Start),
        ));

        let loops = closed_line_loops(&doc, 0);
        assert_eq!(loops.len(), 1);
        let mut sorted = loops[0].clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2]);
    }

    #[test]
    fn open_chain_of_lines_has_no_loop() {
        let mut doc = Document::default();
        doc.add_sketch(crate::model::FaceId::ConstructionPlane(0));
        doc.lines.push(line(0, 0.0, 0.0, 10.0, 0.0));
        doc.lines.push(line(0, 10.0, 0.0, 5.0, 8.0));
        doc.constraints.push(coincident(
            0,
            point(0, LineEnd::End),
            point(1, LineEnd::Start),
        ));

        assert!(closed_line_loops(&doc, 0).is_empty());
    }

    #[test]
    fn unconnected_lines_form_no_loop() {
        let mut doc = Document::default();
        doc.add_sketch(crate::model::FaceId::ConstructionPlane(0));
        doc.lines.push(line(0, 0.0, 0.0, 10.0, 0.0));
        doc.lines.push(line(0, 100.0, 0.0, 110.0, 0.0));
        doc.lines.push(line(0, 200.0, 0.0, 210.0, 0.0));

        assert!(closed_line_loops(&doc, 0).is_empty());
    }

    #[test]
    fn deleted_line_does_not_participate_in_a_loop() {
        let mut doc = Document::default();
        doc.add_sketch(crate::model::FaceId::ConstructionPlane(0));
        doc.lines.push(line(0, 0.0, 0.0, 10.0, 0.0));
        doc.lines.push(line(0, 10.0, 0.0, 5.0, 8.0));
        doc.lines.push(line(0, 5.0, 8.0, 0.0, 0.0));
        doc.lines[2].deleted = true;
        doc.constraints.push(coincident(
            0,
            point(0, LineEnd::End),
            point(1, LineEnd::Start),
        ));
        doc.constraints.push(coincident(
            0,
            point(1, LineEnd::End),
            point(2, LineEnd::Start),
        ));
        doc.constraints.push(coincident(
            0,
            point(2, LineEnd::End),
            point(0, LineEnd::Start),
        ));

        assert!(closed_line_loops(&doc, 0).is_empty());
    }

    #[test]
    fn four_lines_closed_into_a_quad_form_one_loop() {
        let mut doc = Document::default();
        doc.add_sketch(crate::model::FaceId::ConstructionPlane(0));
        doc.lines.push(line(0, 0.0, 0.0, 10.0, 0.0));
        doc.lines.push(line(0, 10.0, 0.0, 10.0, 10.0));
        doc.lines.push(line(0, 10.0, 10.0, 0.0, 10.0));
        doc.lines.push(line(0, 0.0, 10.0, 0.0, 0.0));
        for i in 0..4 {
            doc.constraints.push(coincident(
                0,
                point(i, LineEnd::End),
                point((i + 1) % 4, LineEnd::Start),
            ));
        }

        let loops = closed_line_loops(&doc, 0);
        assert_eq!(loops.len(), 1);
        let mut sorted = loops[0].clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2, 3]);
    }

    #[test]
    fn concave_polygon_triangulation_stays_inside_boundary() {
        // L-shaped hexagon: convex fan from the first vertex fills the missing notch.
        let pts = vec![
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 1.0),
            (1.0, 1.0),
            (1.0, 4.0),
            (0.0, 4.0),
        ];
        let tris = triangulate_uv(&pts);
        assert_eq!(tris.len(), 4);
        for [a, b, c] in &tris {
            let centroid = (
                (pts[*a].0 + pts[*b].0 + pts[*c].0) / 3.0,
                (pts[*a].1 + pts[*b].1 + pts[*c].1) / 3.0,
            );
            assert!(
                point_in_polygon_2d(centroid, &pts),
                "centroid {centroid:?} outside polygon"
            );
        }
        let leak = (2.0, 2.0);
        assert!(!point_in_polygon_2d(leak, &pts), "notch point should lie outside the L");
        for [a, b, c] in &tris {
            assert!(!point_in_triangle_2d(leak, pts[*a], pts[*b], pts[*c]));
        }
    }

    #[test]
    fn concave_loop_inside_a_split_quad_is_detected_and_triangulated() {
        // Outer quad A-B-C-D with a concave inner loop A-P-E-F-A where P lies on edge B-C.
        let mut doc = Document::default();
        doc.add_sketch(crate::model::FaceId::ConstructionPlane(0));
        // Outer quad edges 0..3
        doc.lines.push(line(0, 0.0, 0.0, 10.0, 0.0)); // A-B
        doc.lines.push(line(0, 10.0, 0.0, 10.0, 10.0)); // B-C
        doc.lines.push(line(0, 10.0, 10.0, 0.0, 10.0)); // C-D
        doc.lines.push(line(0, 0.0, 10.0, 0.0, 0.0)); // D-A
        // Inner concave loop edges 4..7
        doc.lines.push(line(0, 0.0, 0.0, 10.0, 5.0)); // A-P
        doc.lines.push(line(0, 10.0, 5.0, 6.0, 8.0)); // P-E
        doc.lines.push(line(0, 6.0, 8.0, 2.0, 6.0)); // E-F
        doc.lines.push(line(0, 2.0, 6.0, 0.0, 0.0)); // F-A
        doc.constraints.push(coincident(0, point(0, LineEnd::End), point(1, LineEnd::Start)));
        doc.constraints.push(coincident(0, point(1, LineEnd::End), point(2, LineEnd::Start)));
        doc.constraints.push(coincident(0, point(2, LineEnd::End), point(3, LineEnd::Start)));
        doc.constraints.push(coincident(0, point(3, LineEnd::End), point(0, LineEnd::Start)));
        doc.constraints.push(coincident(0, point(4, LineEnd::End), point(1, LineEnd::Start)));
        doc.constraints.push(coincident(0, point(4, LineEnd::Start), point(0, LineEnd::Start)));
        doc.constraints.push(coincident(0, point(5, LineEnd::End), point(6, LineEnd::Start)));
        doc.constraints.push(coincident(0, point(6, LineEnd::End), point(7, LineEnd::Start)));
        doc.constraints.push(coincident(0, point(7, LineEnd::End), point(4, LineEnd::Start)));

        let loops = closed_line_loops(&doc, 0);
        assert!(loops.len() >= 2, "expected outer and inner loops, got {loops:?}");
        let inner = loops
            .iter()
            .find(|l| l.len() == 4 && l.contains(&4))
            .expect("inner concave loop");
        let uv = loop_vertices_uv(&doc, 0, inner).unwrap();
        assert_eq!(uv.len(), 4);
        let tris = triangulate_uv(&uv);
        assert_eq!(tris.len(), 2);
        for [a, b, c] in &tris {
            let centroid = (
                (uv[*a].0 + uv[*b].0 + uv[*c].0) / 3.0,
                (uv[*a].1 + uv[*b].1 + uv[*c].1) / 3.0,
            );
            assert!(
                point_in_polygon_2d(centroid, &uv),
                "inner face centroid {centroid:?} leaked outside loop"
            );
        }
    }
}
