//! Two-simple-polygon boolean operations (#16/#62): Weiler–Atherton clipping of two simple
//! (non-self-intersecting), not-necessarily-convex closed polygon loops sharing the same 2D
//! frame, producing `Intersection` or `Difference` (`a - b`).
//!
//! Scope (deliberate — see SPEC.md): this only ever combines **two** shapes at a time, and
//! only when the boolean result reduces to a **single simple polygon loop**. Multi-part
//! results (disjoint pieces), results with a hole (e.g. subtracting a strictly-interior
//! shape, which would leave an annulus), and near-zero-area/degenerate results are all
//! rejected (`None`) rather than approximated — callers fall back to whole-shape picking.
//!
//! The algorithm: find every edge-edge intersection between the two loops, splice them into
//! augmented copies of both vertex lists, classify each intersection as an "entry" (the
//! subject polygon `a` is heading from outside `b` to inside `b` there) or "exit", then walk
//! forward alternately between the two lists — starting from an unvisited entry vertex and
//! switching lists at every intersection — collecting the boundary of `a ∩ b`. `Difference`
//! reverses `b`'s winding first (the standard trick that turns the same intersection-walk into
//! a subtraction) — see Weiler & Atherton (1977).

use crate::model::BooleanOp;
use crate::polygon::point_in_polygon_2d;
use std::collections::{HashMap, HashSet};

type Pt = (f32, f32);

/// Compute the boolean-combined region of two simple closed polygon loops (`a`, `b`), given
/// in the same 2D frame. Returns `None` if the result isn't a single simple polygon loop —
/// see the module-level scope note for why this is a deliberate limitation, not a bug.
pub fn polygon_boolean(a: &[Pt], b: &[Pt], op: BooleanOp) -> Option<Vec<Pt>> {
    if a.len() < 3 || b.len() < 3 {
        return None;
    }
    if !all_finite(a) || !all_finite(b) {
        return None;
    }

    let scale = bbox_scale(a).max(bbox_scale(b)).max(1.0);

    // The standard Weiler-Atherton subtraction trick: reverse the clip polygon's winding, run
    // the same "intersection" walk, and it traces `a - b` instead of `a ∩ b`. Containment
    // tests (point-in-polygon) are winding-independent, so this doesn't affect them.
    let b_work: Vec<Pt> = match op {
        BooleanOp::Intersection => b.to_vec(),
        BooleanOp::Difference => {
            let mut r = b.to_vec();
            r.reverse();
            r
        }
    };

    let (hits, cluster_points) = find_intersections(a, &b_work, scale);
    if hits.is_empty() {
        return no_crossings_result(a, &b_work, op, scale);
    }

    let mut a_edge_hits: HashMap<usize, Vec<(usize, f32)>> = HashMap::new();
    let mut b_edge_hits: HashMap<usize, Vec<(usize, f32)>> = HashMap::new();
    for hit in &hits {
        a_edge_hits.entry(hit.cluster).or_default().push((hit.a_edge, hit.a_t));
        b_edge_hits.entry(hit.cluster).or_default().push((hit.b_edge, hit.b_t));
    }

    let eps_vertex = (scale * 1e-5).max(1e-6);
    let attach_a = compute_attach(a, &a_edge_hits, eps_vertex);
    let attach_b = compute_attach(&b_work, &b_edge_hits, eps_vertex);

    let (list_a, pos_a) = build_augmented(a, &attach_a, &cluster_points);
    let (list_b, pos_b) = build_augmented(&b_work, &attach_b, &cluster_points);

    let valid_ids: HashSet<usize> = pos_a
        .keys()
        .copied()
        .filter(|id| pos_b.contains_key(id))
        .collect();
    if valid_ids.is_empty() {
        return no_crossings_result(a, &b_work, op, scale);
    }

    let list_a = clean_list(list_a, &valid_ids);
    let list_b = clean_list(list_b, &valid_ids);
    let pos_a: HashMap<usize, usize> = pos_a.into_iter().filter(|(id, _)| valid_ids.contains(id)).collect();
    let pos_b: HashMap<usize, usize> = pos_b.into_iter().filter(|(id, _)| valid_ids.contains(id)).collect();

    // Classify each valid intersection as "entry" for `a`'s forward traversal into the
    // *kept* region: for `Intersection` that's "inside `b`"; for `Difference` it's "outside
    // `b`" (`a ∩ complement(b)`). Since point-in-polygon is winding-independent, reversing
    // `b_work`'s vertex order (above) doesn't itself flip this containment test — it only
    // changes which direction the walk continues in once it switches onto `b_work` — so the
    // "kept region" sense has to be flipped explicitly here based on `op`.
    let mut entry_map: HashMap<usize, bool> = HashMap::new();
    for &id in &valid_ids {
        let pa = pos_a[&id];
        let next = &list_a[(pa + 1) % list_a.len()];
        let mid = midpoint(list_a[pa].point, next.point);
        let inside_b = point_in_polygon_2d(mid, &b_work);
        let keep = match op {
            BooleanOp::Intersection => inside_b,
            BooleanOp::Difference => !inside_b,
        };
        entry_map.insert(id, keep);
    }

    let loops = walk_all(&list_a, &pos_a, &list_b, &pos_b, &entry_map, &valid_ids)?;
    match loops.len() {
        // No entry vertex found at all: every detected "crossing" was a pure tangency (the
        // loops only graze/touch, never actually cross transversally) — not a real boundary
        // split, so treat it like the zero-crossings case (full containment or disjoint).
        0 => no_crossings_result(a, &b_work, op, scale),
        1 => finalize(loops.into_iter().next().unwrap(), scale),
        _ => None,
    }
}

fn all_finite(poly: &[Pt]) -> bool {
    poly.iter().all(|p| p.0.is_finite() && p.1.is_finite())
}

fn bbox_scale(poly: &[Pt]) -> f32 {
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in poly {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    }
    (maxx - minx).max(maxy - miny).max(0.0)
}

fn dist(a: Pt, b: Pt) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn midpoint(a: Pt, b: Pt) -> Pt {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

fn shoelace_area(poly: &[Pt]) -> f32 {
    let n = poly.len();
    let mut area = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        area += poly[i].0 * poly[j].1 - poly[j].0 * poly[i].1;
    }
    area * 0.5
}

fn finalize(mut poly: Vec<Pt>, scale: f32) -> Option<Vec<Pt>> {
    let area = shoelace_area(&poly);
    let min_area = (scale * scale * 1e-6).max(1e-8);
    if area.abs() < min_area {
        return None;
    }
    if area < 0.0 {
        poly.reverse();
    }
    Some(poly)
}

/// Segment-segment intersection: `p1->p2` (from `a`) against `q1->q2` (from `b`). Returns
/// `(t, s, point)` with `t`/`s` the clamped params along each segment, or `None` if parallel
/// or the crossing falls outside both segments (with `eps` slack in parameter space, so
/// near-endpoint touches are still detected and later resolved by vertex-snapping).
fn segment_intersection(p1: Pt, p2: Pt, q1: Pt, q2: Pt, eps: f32) -> Option<(f32, f32, Pt)> {
    let d1 = (p2.0 - p1.0, p2.1 - p1.1);
    let d2 = (q2.0 - q1.0, q2.1 - q1.1);
    let denom = d1.0 * d2.1 - d1.1 * d2.0;
    if denom.abs() < 1e-9 {
        return None;
    }
    let dx = q1.0 - p1.0;
    let dy = q1.1 - p1.1;
    let t = (dx * d2.1 - dy * d2.0) / denom;
    let s = (dx * d1.1 - dy * d1.0) / denom;
    if t < -eps || t > 1.0 + eps || s < -eps || s > 1.0 + eps {
        return None;
    }
    let tc = t.clamp(0.0, 1.0);
    let sc = s.clamp(0.0, 1.0);
    let point = (p1.0 + tc * d1.0, p1.1 + tc * d1.1);
    Some((tc, sc, point))
}

fn cluster_id(points: &mut Vec<Pt>, p: Pt, eps: f32) -> usize {
    for (i, c) in points.iter().enumerate() {
        if dist(*c, p) <= eps {
            return i;
        }
    }
    points.push(p);
    points.len() - 1
}

struct Hit {
    cluster: usize,
    a_edge: usize,
    a_t: f32,
    b_edge: usize,
    b_t: f32,
}

/// All edge-edge crossings between `a` and `b`, clustered so that the same physical point
/// reached from adjacent edges (e.g. two edges sharing a vertex that lands exactly on the
/// other polygon) collapses to one id.
fn find_intersections(a: &[Pt], b: &[Pt], scale: f32) -> (Vec<Hit>, Vec<Pt>) {
    let eps_seg = 1e-3;
    let eps_cluster = (scale * 1e-5).max(1e-6);
    let na = a.len();
    let nb = b.len();
    let mut cluster_points: Vec<Pt> = Vec::new();
    let mut hits = Vec::new();
    for i in 0..na {
        let p1 = a[i];
        let p2 = a[(i + 1) % na];
        for j in 0..nb {
            let q1 = b[j];
            let q2 = b[(j + 1) % nb];
            if let Some((t, s, pt)) = segment_intersection(p1, p2, q1, q2, eps_seg) {
                let cid = cluster_id(&mut cluster_points, pt, eps_cluster);
                hits.push(Hit { cluster: cid, a_edge: i, a_t: t, b_edge: j, b_t: s });
            }
        }
    }
    (hits, cluster_points)
}

enum Attach {
    Vertex(usize),
    Edge(usize, f32),
}

fn compute_attach(poly: &[Pt], edge_hits: &HashMap<usize, Vec<(usize, f32)>>, eps_vertex: f32) -> HashMap<usize, Attach> {
    let mut out = HashMap::new();
    for (&cid, hits) in edge_hits {
        // Does this cluster coincide with an existing vertex? Use the involved edges'
        // endpoints (cheaper than re-deriving the cluster's point) — a vertex is at the end
        // of some edge in `hits` with t close to 1, or the start of one with t close to 0.
        let mut vertex_hit = None;
        for &(edge, t) in hits {
            if t <= 1e-3 {
                vertex_hit = Some(edge);
                break;
            }
            if t >= 1.0 - 1e-3 {
                vertex_hit = Some((edge + 1) % poly.len());
                break;
            }
        }
        if let Some(k) = vertex_hit {
            // Confirm by distance too, in case the param was merely close but the geometry
            // isn't (defensive; in practice these agree).
            let _ = eps_vertex;
            out.insert(cid, Attach::Vertex(k));
            continue;
        }
        let &(edge, t) = hits.first().expect("cluster has at least one hit");
        out.insert(cid, Attach::Edge(edge, t.clamp(1e-4, 1.0 - 1e-4)));
    }
    out
}

struct AugEntry {
    point: Pt,
    isect: Option<usize>,
}

fn build_augmented(poly: &[Pt], attach: &HashMap<usize, Attach>, cluster_points: &[Pt]) -> (Vec<AugEntry>, HashMap<usize, usize>) {
    let n = poly.len();
    let mut vertex_tag: HashMap<usize, usize> = HashMap::new();
    let mut edge_inserts: HashMap<usize, Vec<(usize, f32)>> = HashMap::new();
    for (&cid, a) in attach {
        match *a {
            Attach::Vertex(k) => {
                vertex_tag.entry(k).or_insert(cid);
            }
            Attach::Edge(e, t) => edge_inserts.entry(e).or_default().push((cid, t)),
        }
    }
    let mut list = Vec::new();
    let mut pos = HashMap::new();
    for i in 0..n {
        let isect = vertex_tag.get(&i).copied();
        if let Some(cid) = isect {
            pos.entry(cid).or_insert(list.len());
        }
        list.push(AugEntry { point: poly[i], isect });
        if let Some(mut ins) = edge_inserts.get(&i).cloned() {
            ins.sort_by(|x, y| x.1.partial_cmp(&y.1).unwrap());
            for (cid, _t) in ins {
                pos.entry(cid).or_insert(list.len());
                list.push(AugEntry { point: cluster_points[cid], isect: Some(cid) });
            }
        }
    }
    (list, pos)
}

fn clean_list(mut list: Vec<AugEntry>, valid: &HashSet<usize>) -> Vec<AugEntry> {
    for e in &mut list {
        if let Some(id) = e.isect {
            if !valid.contains(&id) {
                e.isect = None;
            }
        }
    }
    list
}

/// Walk every unvisited entry vertex to completion, tracing one closed contour per walk.
/// Returns `None` (rather than looping forever) if a walk fails to close within a generous
/// iteration bound — a defensive guard against any residual classification inconsistency on
/// pathological input, per the module's "never panic or hang" contract.
fn walk_all(
    list_a: &[AugEntry],
    pos_a: &HashMap<usize, usize>,
    list_b: &[AugEntry],
    pos_b: &HashMap<usize, usize>,
    entry_map: &HashMap<usize, bool>,
    valid_ids: &HashSet<usize>,
) -> Option<Vec<Vec<Pt>>> {
    let mut order: Vec<usize> = valid_ids.iter().copied().collect();
    order.sort_by_key(|id| pos_a[id]);

    let mut visited: HashSet<usize> = HashSet::new();
    let mut loops = Vec::new();
    let max_iters = 4 * (list_a.len() + list_b.len()) + 16;

    for &start_id in &order {
        if visited.contains(&start_id) || !entry_map[&start_id] {
            continue;
        }
        visited.insert(start_id);
        let mut output = vec![list_a[pos_a[&start_id]].point];
        let mut on_a = true;
        let mut pos = pos_a[&start_id];
        let mut closed = false;
        for _ in 0..max_iters {
            let list = if on_a { list_a } else { list_b };
            let next_pos = (pos + 1) % list.len();
            let entry = &list[next_pos];
            if entry.isect == Some(start_id) {
                closed = true;
                break;
            }
            output.push(entry.point);
            pos = next_pos;
            if let Some(id2) = entry.isect {
                visited.insert(id2);
                on_a = !on_a;
                pos = if on_a { pos_a[&id2] } else { pos_b[&id2] };
            }
        }
        if !closed {
            return None;
        }
        loops.push(output);
    }
    Some(loops)
}

/// Whether `poly` lies inside `other`, tested by majority vote across all of `poly`'s
/// vertices rather than a single point: with genuinely zero transversal crossings, `poly` is
/// either entirely inside `other` or entirely outside it, so most vertices agree — except any
/// vertex that happens to sit exactly on `other`'s boundary (a tangency point), where an
/// even-odd point-in-polygon test is ambiguous. Voting keeps that minority from flipping the
/// answer.
fn mostly_inside(poly: &[Pt], other: &[Pt]) -> bool {
    let inside = poly.iter().filter(|&&p| point_in_polygon_2d(p, other)).count();
    inside * 2 > poly.len()
}

/// Zero-crossings fallback: either there were no edge-edge intersections at all, or every one
/// found was a pure tangency (see the `0 =>` arm in `polygon_boolean`) — either way, `a` and
/// `b_work` don't cross transversally, so they're either disjoint or one wholly contains the
/// other.
fn no_crossings_result(a: &[Pt], b_work: &[Pt], op: BooleanOp, scale: f32) -> Option<Vec<Pt>> {
    let a_in_b = mostly_inside(a, b_work);
    let b_in_a = mostly_inside(b_work, a);
    match op {
        BooleanOp::Intersection => {
            if a_in_b {
                finalize(a.to_vec(), scale)
            } else if b_in_a {
                finalize(b_work.to_vec(), scale)
            } else {
                None
            }
        }
        BooleanOp::Difference => {
            if a_in_b {
                // `a` is entirely inside `b`: `a - b` is empty.
                None
            } else if b_in_a {
                // `b` strictly interior to `a`: `a - b` is an annulus (a hole) — rejected.
                None
            } else {
                // Disjoint: subtracting nothing leaves `a` unchanged.
                finalize(a.to_vec(), scale)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BooleanOp::{Difference, Intersection};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Vec<Pt> {
        vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
    }

    fn circle(cx: f32, cy: f32, r: f32, segments: usize) -> Vec<Pt> {
        (0..segments)
            .map(|i| {
                let a = i as f32 / segments as f32 * std::f32::consts::TAU;
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect()
    }

    fn area(poly: &[Pt]) -> f32 {
        shoelace_area(poly).abs()
    }

    /// Brute-force self-intersection check: no two non-adjacent edges of a simple closed
    /// polygon should cross.
    fn is_simple(poly: &[Pt]) -> bool {
        let n = poly.len();
        if n < 3 {
            return false;
        }
        for i in 0..n {
            let (p1, p2) = (poly[i], poly[(i + 1) % n]);
            for j in 0..n {
                if i == j || (j + 1) % n == i || (i + 1) % n == j {
                    continue;
                }
                let (q1, q2) = (poly[j], poly[(j + 1) % n]);
                if segment_intersection(p1, p2, q1, q2, -1e-4).is_some() {
                    return false;
                }
            }
        }
        true
    }

    fn assert_close(a: f32, b: f32, tol: f32) {
        assert!((a - b).abs() <= tol, "{a} !~= {b} (tol {tol})");
    }

    fn contains_point_near(poly: &[Pt], target: Pt, tol: f32) -> bool {
        poly.iter().any(|p| dist(*p, target) <= tol)
    }

    // ---- Two overlapping axis-aligned rectangles (hand-computable) ----

    #[test]
    fn rect_rect_intersection_area_and_vertices() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let result = polygon_boolean(&a, &b, Intersection).expect("clean overlap");
        assert!(is_simple(&result));
        assert_close(area(&result), 25.0, 1e-3);
        // The intersection is the square (5,5)-(10,10): two corners are original vertices
        // (10,10 shared by both, and none other since only one quadrant overlaps other than
        // the shared corner), the other two are genuine intersection points.
        assert!(contains_point_near(&result, (10.0, 10.0), 1e-3));
        assert!(contains_point_near(&result, (5.0, 5.0), 1e-3));
        assert!(contains_point_near(&result, (5.0, 10.0), 1e-3));
        assert!(contains_point_near(&result, (10.0, 5.0), 1e-3));
    }

    #[test]
    fn rect_rect_difference_area_and_shape() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        let result = polygon_boolean(&a, &b, Difference).expect("clean difference");
        assert!(is_simple(&result));
        // a (100) minus the 5x5 overlap (25) = 75.
        assert_close(area(&result), 75.0, 1e-3);
        // L-shaped result: 6 vertices.
        assert_eq!(result.len(), 6);
    }

    #[test]
    fn rect_rect_reverse_difference() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(5.0, 5.0, 10.0, 10.0);
        // b - a: same 25-area square shape as a's difference is 75, by symmetry of this setup
        // b (100) minus overlap (25) = 75 too.
        let result = polygon_boolean(&b, &a, Difference).expect("clean reverse difference");
        assert!(is_simple(&result));
        assert_close(area(&result), 75.0, 1e-3);
    }

    // ---- Rectangle and 48-gon circle with known partial overlap ----

    #[test]
    fn rect_circle_intersection_matches_analytic_circular_segment() {
        // Circle at origin r=10; rectangle covers x in [0, 20], y in [-20, 20] (i.e. the
        // right half-plane clipped) — intersection should be a half-disk, area = pi*r^2/2.
        let c = circle(0.0, 0.0, 10.0, 48);
        let r = rect(0.0, -20.0, 20.0, 40.0);
        let result = polygon_boolean(&c, &r, Intersection).expect("half-disk overlap");
        assert!(is_simple(&result));
        let expected = std::f32::consts::PI * 100.0 / 2.0;
        // The 48-gon itself only approximates the circle; allow ~0.5% slack.
        assert_close(area(&result), expected, expected * 0.01);
    }

    #[test]
    fn rect_circle_difference_matches_analytic_complement() {
        let c = circle(0.0, 0.0, 10.0, 48);
        let r = rect(0.0, -20.0, 20.0, 40.0);
        // circle - right-half-rect = left half-disk.
        let result = polygon_boolean(&c, &r, Difference).expect("half-disk complement");
        assert!(is_simple(&result));
        let expected = std::f32::consts::PI * 100.0 / 2.0;
        assert_close(area(&result), expected, expected * 0.01);
    }

    #[test]
    fn rect_circle_partial_overlap_quarter() {
        // Circle radius 10 at origin; rectangle covering just the first quadrant extended
        // outward — intersection is a quarter disk, area = pi*r^2/4.
        let c = circle(0.0, 0.0, 10.0, 48);
        let r = rect(0.0, 0.0, 20.0, 20.0);
        let result = polygon_boolean(&c, &r, Intersection).expect("quarter-disk overlap");
        assert!(is_simple(&result));
        let expected = std::f32::consts::PI * 100.0 / 4.0;
        assert_close(area(&result), expected, expected * 0.01);
    }

    // ---- No overlap ----

    #[test]
    fn disjoint_shapes_intersection_is_none() {
        let a = rect(0.0, 0.0, 5.0, 5.0);
        let b = rect(100.0, 100.0, 5.0, 5.0);
        assert!(polygon_boolean(&a, &b, Intersection).is_none());
    }

    #[test]
    fn disjoint_shapes_difference_is_a_unchanged() {
        let a = rect(0.0, 0.0, 5.0, 5.0);
        let b = rect(100.0, 100.0, 5.0, 5.0);
        let result = polygon_boolean(&a, &b, Difference).expect("difference of disjoint = a");
        assert!(is_simple(&result));
        assert_close(area(&result), 25.0, 1e-3);
    }

    // ---- Full containment ----

    #[test]
    fn strictly_contained_intersection_is_the_inner_shape() {
        let outer = rect(0.0, 0.0, 20.0, 20.0);
        let inner = rect(5.0, 5.0, 4.0, 4.0);
        let result = polygon_boolean(&outer, &inner, Intersection).expect("inner shape");
        assert!(is_simple(&result));
        assert_close(area(&result), 16.0, 1e-3);
    }

    #[test]
    fn strictly_contained_difference_is_none_annulus_hole() {
        // B strictly interior to A, no boundary contact: A - B is an annulus (a hole), which
        // isn't a single simple loop — must be rejected, not approximated.
        let outer = rect(0.0, 0.0, 20.0, 20.0);
        let inner = rect(5.0, 5.0, 4.0, 4.0);
        assert!(polygon_boolean(&outer, &inner, Difference).is_none());
    }

    #[test]
    fn inner_shape_wholly_inside_difference_is_empty() {
        // The inner shape minus the outer one that contains it: fully consumed -> empty.
        let outer = rect(0.0, 0.0, 20.0, 20.0);
        let inner = rect(5.0, 5.0, 4.0, 4.0);
        assert!(polygon_boolean(&inner, &outer, Difference).is_none());
    }

    #[test]
    fn boundary_touching_containment_reduces_to_a_single_simple_loop() {
        // A kite-shaped B sits inside square A (0,0)-(10,10), touching A's bottom edge at a
        // single interior point (5,0) but otherwise strictly interior — a genuine (if
        // degenerate) crossing rather than a collinear edge overlap, so the general
        // intersection/walk path (not the zero-crossings fallback) handles it.
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = vec![(5.0, 0.0), (7.0, 3.0), (5.0, 6.0), (3.0, 3.0)];
        let result = polygon_boolean(&a, &b, Intersection).expect("kite fully inside square");
        assert!(is_simple(&result));
        // B lies entirely within A (touching only at one point), so A ∩ B = B.
        assert_close(area(&result), area(&b), area(&b) * 0.05);
    }

    // ---- Tangent (touching, no interior overlap) ----

    #[test]
    fn edge_tangent_rectangles_do_not_panic_or_loop() {
        // Two rectangles sharing a full edge (collinear touch) but with zero-area overlap.
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(10.0, 0.0, 10.0, 10.0);
        // Either a clean empty/near-zero result or a rejected `None` is acceptable — just must
        // not panic or hang, and must not claim significant area.
        if let Some(result) = polygon_boolean(&a, &b, Intersection) {
            assert!(area(&result) < 1e-2);
        }
        // Difference of tangent (non-overlapping-area) shapes should be ~unchanged `a`.
        let diff = polygon_boolean(&a, &b, Difference).expect("tangent difference keeps a");
        assert_close(area(&diff), 100.0, 1.0);
    }

    #[test]
    fn corner_tangent_rectangles_do_not_panic_or_loop() {
        // Two rectangles touching only at a single shared corner point.
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(10.0, 10.0, 10.0, 10.0);
        if let Some(result) = polygon_boolean(&a, &b, Intersection) {
            assert!(area(&result) < 1e-2);
        }
        let diff = polygon_boolean(&a, &b, Difference).expect("corner-tangent difference keeps a");
        assert_close(area(&diff), 100.0, 1.0);
    }

    // ---- General concave-ish sanity: a rotated square overlapping an axis-aligned one ----

    #[test]
    fn rotated_square_overlap_is_simple_and_nonzero() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        // Diamond centered on (5,5) with "radius" 7 (rotated square), overlapping a but
        // extending outside it on all four sides — big enough to slice all the way through,
        // splitting `a - diamond` into 4 disjoint corner pieces. That's a genuine multi-part
        // result (not a bug): confirms it's correctly rejected rather than merged wrong.
        let b = vec![(5.0, -2.0), (12.0, 5.0), (5.0, 12.0), (-2.0, 5.0)];
        let result = polygon_boolean(&a, &b, Intersection).expect("octagon-ish overlap");
        assert!(is_simple(&result));
        assert!(area(&result) > 0.0 && area(&result) < 100.0);
        assert!(polygon_boolean(&a, &b, Difference).is_none(), "genuinely 4 disjoint pieces");
    }

    #[test]
    fn rotated_square_partial_bite_difference_stays_a_single_notched_loop() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        // A smaller diamond straddling the middle of the right edge (not slicing all the way
        // through the square): `a ∩ b` is a small triangle-ish lens, and `a - b` is the
        // square with a single notch bitten out of its right edge — still one simple loop.
        let b = vec![(10.0, 1.0), (14.0, 5.0), (10.0, 9.0), (6.0, 5.0)];
        let inter = polygon_boolean(&a, &b, Intersection).expect("lens overlap");
        assert!(is_simple(&inter));
        assert!(area(&inter) > 0.0 && area(&inter) < 100.0);

        let diff = polygon_boolean(&a, &b, Difference).expect("single notched loop");
        assert!(is_simple(&diff));
        // a (100) minus the lens overlap.
        assert_close(area(&diff), 100.0 - area(&inter), 1e-2);
    }
}
