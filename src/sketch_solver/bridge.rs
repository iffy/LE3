//! Bridge between [`Document`] sketch geometry and the numeric solver.

use super::dof::{dof_remaining, vars_can_move_together};
use super::newton::{solve_lm, SolveReport, SolverConfig};
use super::residuals::{
    Equation, DEFAULT_WEIGHT, DRAG_PIN_WEIGHT, GAUGE_HOLD_WEIGHT, REFERENCE_HOLD_WEIGHT,
};
use crate::geometric_constraints::parallel_reference_and_movable;
use super::system::{System, VarId};
use crate::document_lifecycle::constraint_kind_applicable;
use crate::geometric_constraints::point_uv;
use crate::model::{
    ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, DistanceTarget, Document,
    LineEnd, SketchId,
};
use crate::value::{eval_angle_rad_in_doc, eval_length_mm_in_doc};
use std::collections::{HashMap, HashSet};

/// Solver graph for one sketch, with stable point-variable mapping.
pub struct SketchBridge {
    pub system: System,
    sketch: SketchId,
    point_vars: HashMap<ConstraintPoint, (VarId, VarId)>,
    circle_radius: HashMap<usize, VarId>,
    hold_references: bool,
    constraint_equations: HashMap<usize, Vec<usize>>,
}

impl SketchBridge {
    pub fn from_document(
        doc: &Document,
        sketch: SketchId,
        hold_references: bool,
    ) -> Result<Self, String> {
        let mut bridge = Self {
            system: System::new(),
            sketch,
            point_vars: HashMap::new(),
            circle_radius: HashMap::new(),
            hold_references,
            constraint_equations: HashMap::new(),
        };
        bridge.seed_entities(doc)?;
        bridge.add_constraints(doc)?;
        Ok(bridge)
    }

    pub fn add_drag_pins(
        &mut self,
        doc: &Document,
        pins: &[(ConstraintPoint, (f32, f32))],
    ) {
        let pinned: HashSet<ConstraintPoint> = pins.iter().map(|(point, _)| point.clone()).collect();
        for (point, (u, v)) in pins {
            if let Some((u_id, v_id)) = self.point_vars.get(point).copied() {
                self.system.add_equation(Equation::Pin {
                    var: u_id,
                    target: *u as f64,
                    weight: DRAG_PIN_WEIGHT,
                });
                self.system.add_equation(Equation::Pin {
                    var: v_id,
                    target: *v as f64,
                    weight: DRAG_PIN_WEIGHT,
                });
            }
        }
        if self.hold_references {
            return;
        }
        // Reference geometry must stay put while the movable side is dragged. Holds were
        // dropped for the whole sketch (hold_references = false during drag), so re-pin the
        // reference of each direction/distance constraint when its movable side is the one
        // being dragged. Coincident anchors are intentionally left free so they still follow.
        for constraint in &doc.constraints {
            if constraint.deleted || constraint.sketch != self.sketch {
                continue;
            }
            match &constraint.kind {
                ConstraintKind::Distance {
                    target: DistanceTarget::PointPointDistance { anchor, mover, .. },
                } => {
                    if pinned.contains(mover) && !pinned.contains(anchor) {
                        let _ = self.anchor_point(doc, anchor.clone(), REFERENCE_HOLD_WEIGHT);
                    } else if pinned.contains(anchor) && !pinned.contains(mover) {
                        let _ = self.anchor_point(doc, mover.clone(), REFERENCE_HOLD_WEIGHT);
                    }
                }
                ConstraintKind::Distance {
                    target: DistanceTarget::PointLineDistance { point, line, .. },
                } => self.hold_reference_when_point_dragged(doc, line.clone(), point.clone(), &pinned),
                ConstraintKind::Midpoint { point, line } => {
                    self.hold_reference_when_point_dragged(doc, line.clone(), point.clone(), &pinned)
                }
                ConstraintKind::Distance {
                    target: DistanceTarget::LineLineDistance { line_a, line_b, .. },
                }
                | ConstraintKind::Parallel { line_a, line_b }
                | ConstraintKind::Perpendicular { line_a, line_b }
                | ConstraintKind::Equal { line_a, line_b }
                | ConstraintKind::Angle { line_a, line_b, .. } => {
                    let (reference, movable) =
                        parallel_reference_and_movable(line_a.clone(), line_b.clone());
                    self.hold_reference_when_movable_dragged(doc, reference, movable, &pinned);
                }
                _ => {}
            }
        }
    }

    /// Hold a constraint's reference line if the dragged geometry is the movable line (and the
    /// reference itself isn't being dragged).
    fn hold_reference_when_movable_dragged(
        &mut self,
        doc: &Document,
        reference: ConstraintLine,
        movable: ConstraintLine,
        pinned: &HashSet<ConstraintPoint>,
    ) {
        let reference_points = line_endpoint_points(doc, reference);
        let movable_points = line_endpoint_points(doc, movable);
        let movable_dragged = movable_points.iter().any(|p| pinned.contains(p));
        let reference_dragged = reference_points.iter().any(|p| pinned.contains(p));
        if movable_dragged && !reference_dragged {
            for point in reference_points {
                let _ = self.anchor_point(doc, point, REFERENCE_HOLD_WEIGHT);
            }
        }
    }

    /// Hold a reference line if the dragged geometry is the constrained point (and the line
    /// itself isn't being dragged).
    fn hold_reference_when_point_dragged(
        &mut self,
        doc: &Document,
        line: ConstraintLine,
        point: ConstraintPoint,
        pinned: &HashSet<ConstraintPoint>,
    ) {
        let reference_points = line_endpoint_points(doc, line);
        let reference_dragged = reference_points.iter().any(|p| pinned.contains(p));
        if pinned.contains(&point) && !reference_dragged {
            for reference_point in reference_points {
                let _ = self.anchor_point(doc, reference_point, REFERENCE_HOLD_WEIGHT);
            }
        }
    }

    pub fn solve(&mut self) -> SolveReport {
        let mut report = solve_lm(&mut self.system, SolverConfig::default());
        report.dof_remaining = dof_remaining(&self.system);
        if !report.success {
            report.failed_constraints = self.conflicting_constraints();
        }
        report
    }

    /// Constraint indices sorted by largest residual contribution (failed solves only).
    pub fn conflicting_constraints(&self) -> Vec<usize> {
        let residuals = self.system.residual_values();
        let mut scored: Vec<(usize, f64)> = self
            .constraint_equations
            .iter()
            .map(|(id, equations)| {
                let score = equations
                    .iter()
                    .map(|index| residuals[*index].abs())
                    .fold(0.0f64, f64::max);
                (*id, score)
            })
            .filter(|(_, score)| *score > 1e-9)
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(id, _)| id).collect()
    }

    pub fn point_solver_vars(&mut self, doc: &Document, point: ConstraintPoint) -> Result<(VarId, VarId), String> {
        self.point_vars(doc, point)
    }

    pub fn apply_to_document(&self, doc: &mut Document) -> Result<(), String> {
        for (point, (u_id, v_id)) in &self.point_vars {
            if let ConstraintPoint::LineEndpoint { .. } = point {
                set_point_uv_from_solver(doc, self.sketch, point.clone(), self.system.value(*u_id), self.system.value(*v_id))?;
            }
        }

        for (circle, radius_var) in &self.circle_radius {
            let center = ConstraintPoint::CircleCenter(*circle);
            if let Some((u_id, v_id)) = self.point_vars.get(&center) {
                set_point_uv_from_solver(
                    doc,
                    self.sketch,
                    center,
                    self.system.value(*u_id),
                    self.system.value(*v_id),
                )?;
            }
            let entity = doc
                .circles
                .get_mut(*circle)
                .ok_or_else(|| format!("Circle {circle} not found"))?;
            entity.r = self.system.value(*radius_var) as f32;
        }
        Ok(())
    }

    fn seed_entities(&mut self, doc: &Document) -> Result<(), String> {
        for (index, line) in doc.lines.iter().enumerate() {
            if line.deleted || line.sketch != self.sketch {
                continue;
            }
            self.ensure_line_endpoint(doc, index, LineEnd::Start)?;
            self.ensure_line_endpoint(doc, index, LineEnd::End)?;
        }
        for (index, circle) in doc.circles.iter().enumerate() {
            if circle.deleted || circle.sketch != self.sketch {
                continue;
            }
            let center = ConstraintPoint::CircleCenter(index);
            if !self.point_vars.contains_key(&center) {
                let (u, v) = point_uv(doc, self.sketch, center.clone())?;
                let (u_id, v_id) = self.system.add_point(u as f64, v as f64, false);
                self.point_vars.insert(center, (u_id, v_id));
            }
            let radius_var = self.system.add_var(circle.r as f64, false);
            self.circle_radius.insert(index, radius_var);
        }
        Ok(())
    }

    fn ensure_line_endpoint(
        &mut self,
        doc: &Document,
        line: usize,
        end: LineEnd,
    ) -> Result<(), String> {
        let point = ConstraintPoint::LineEndpoint { line, end };
        if self.point_vars.contains_key(&point) {
            return Ok(());
        }
        let (u, v) = point_uv(doc, self.sketch, point.clone())?;
        let (u_id, v_id) = self.system.add_point(u as f64, v as f64, false);
        self.point_vars.insert(point, (u_id, v_id));
        Ok(())
    }

    fn add_constraints(&mut self, doc: &Document) -> Result<(), String> {
        for (index, constraint) in doc.constraints.iter().enumerate() {
            if constraint.deleted || constraint.sketch != self.sketch {
                continue;
            }
            if !constraint_kind_applicable(doc, &constraint.kind) {
                continue;
            }
            self.add_constraint(doc, index, constraint)?;
        }
        Ok(())
    }

    fn add_constraint(
        &mut self,
        doc: &Document,
        constraint_id: usize,
        constraint: &crate::model::Constraint,
    ) -> Result<(), String> {
        let eq_start = self.system.equations.len();
        let result = self.add_constraint_body(doc, constraint);
        let eq_end = self.system.equations.len();
        if eq_end > eq_start {
            self.constraint_equations
                .insert(constraint_id, (eq_start..eq_end).collect());
        }
        result
    }

    fn add_constraint_body(
        &mut self,
        doc: &Document,
        constraint: &crate::model::Constraint,
    ) -> Result<(), String> {
        match constraint.kind.clone() {
            ConstraintKind::Distance { target } => {
                self.add_distance_constraint(doc, constraint, target)?;
            }
            ConstraintKind::Horizontal { line } => {
                let ((x0, y0), (x1, y1)) = self.line_vars(doc, line)?;
                self.system.add_equation(Equation::Horizontal {
                    y0,
                    y1,
                    weight: DEFAULT_WEIGHT,
                });
                let _ = (x0, x1);
            }
            ConstraintKind::Vertical { line } => {
                let ((x0, y0), (x1, y1)) = self.line_vars(doc, line)?;
                self.system.add_equation(Equation::Vertical {
                    x0,
                    x1,
                    weight: DEFAULT_WEIGHT,
                });
                let _ = (y0, y1);
            }
            ConstraintKind::Parallel { line_a, line_b } => {
                let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
                self.hold_line(doc, reference.clone())?;
                let a = self.line_vars(doc, reference)?;
                let b = self.line_vars(doc, movable)?;
                self.system.add_equation(Equation::Parallel {
                    ax0: a.0.0,
                    ay0: a.0.1,
                    ax1: a.1.0,
                    ay1: a.1.1,
                    bx0: b.0.0,
                    by0: b.0.1,
                    bx1: b.1.0,
                    by1: b.1.1,
                    weight: DEFAULT_WEIGHT,
                });
            }
            ConstraintKind::Perpendicular { line_a, line_b } => {
                let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
                self.hold_line(doc, reference.clone())?;
                let a = self.line_vars(doc, reference)?;
                let b = self.line_vars(doc, movable)?;
                self.system.add_equation(Equation::Perpendicular {
                    ax0: a.0.0,
                    ay0: a.0.1,
                    ax1: a.1.0,
                    ay1: a.1.1,
                    bx0: b.0.0,
                    by0: b.0.1,
                    bx1: b.1.0,
                    by1: b.1.1,
                    weight: DEFAULT_WEIGHT,
                });
            }
            ConstraintKind::Equal { line_a, line_b } => {
                let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
                self.hold_line(doc, reference.clone())?;
                let a = self.line_vars(doc, reference)?;
                let b = self.line_vars(doc, movable)?;
                self.system.add_equation(Equation::EqualLength {
                    ax0: a.0.0,
                    ay0: a.0.1,
                    ax1: a.1.0,
                    ay1: a.1.1,
                    bx0: b.0.0,
                    by0: b.0.1,
                    bx1: b.1.0,
                    by1: b.1.1,
                    weight: DEFAULT_WEIGHT,
                });
            }
            ConstraintKind::Coincident { a, b } => self.add_coincident(doc, a, b)?,
            ConstraintKind::Midpoint { point, line } => {
                self.hold_line(doc, line.clone())?;
                let (pu, pv) = self.point_vars(doc, point)?;
                let ((x0, y0), (x1, y1)) = self.line_vars(doc, line)?;
                self.system.add_equation(Equation::MidpointU {
                    px: pu,
                    x0,
                    x1,
                    weight: DEFAULT_WEIGHT,
                });
                self.system.add_equation(Equation::MidpointV {
                    py: pv,
                    y0,
                    y1,
                    weight: DEFAULT_WEIGHT,
                });
            }
            ConstraintKind::Angle {
                line_a,
                line_b,
                rotation_sign,
            } => {
                let Some(angle) = eval_angle_rad_in_doc(&constraint.expression, doc) else {
                    return Ok(());
                };
                if angle <= 0.0 || angle >= std::f32::consts::PI {
                    return Ok(());
                }
                let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
                self.hold_line(doc, reference.clone())?;
                let a = self.line_vars(doc, reference)?;
                let b = self.line_vars(doc, movable)?;
                self.system.add_equation(Equation::Angle {
                    ax0: a.0.0,
                    ay0: a.0.1,
                    ax1: a.1.0,
                    ay1: a.1.1,
                    bx0: b.0.0,
                    by0: b.0.1,
                    bx1: b.1.0,
                    by1: b.1.1,
                    angle: rotation_sign as f64 * angle as f64,
                    weight: DEFAULT_WEIGHT,
                });
            }
        }
        Ok(())
    }

    fn add_distance_constraint(
        &mut self,
        doc: &Document,
        constraint: &crate::model::Constraint,
        target: DistanceTarget,
    ) -> Result<(), String> {
        let Some(value) = eval_length_mm_in_doc(&constraint.expression, doc) else {
            return Ok(());
        };
        if value <= 0.0 {
            return Ok(());
        }
        let value = value as f64;
        match target {
            DistanceTarget::LineLength(index) => {
                self.anchor_point(
                    doc,
                    ConstraintPoint::LineEndpoint {
                        line: index,
                        end: LineEnd::Start,
                    },
                    REFERENCE_HOLD_WEIGHT,
                )?;
                let line = ConstraintLine::Line(index);
                let ((x0, y0), (x1, y1)) = self.line_vars(doc, line)?;
                self.system.add_equation(Equation::LineLength {
                    x0,
                    y0,
                    x1,
                    y1,
                    length: value,
                    weight: DEFAULT_WEIGHT,
                });
            }
            DistanceTarget::CircleDiameter(index) => {
                self.anchor_point(doc, ConstraintPoint::CircleCenter(index), REFERENCE_HOLD_WEIGHT)?;
                let radius = self
                    .circle_radius
                    .get(&index)
                    .copied()
                    .ok_or_else(|| format!("Circle {index} not in solver graph"))?;
                self.system.add_equation(Equation::CircleDiameter {
                    radius,
                    diameter: value,
                    weight: DEFAULT_WEIGHT,
                });
            }
            DistanceTarget::LineLineDistance {
                line_a,
                line_b,
                side,
            } => {
                let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
                self.hold_line(doc, reference.clone())?;
                let a = self.line_vars(doc, reference)?;
                let b = self.line_vars(doc, movable)?;
                self.system.add_equation(Equation::LineLineDistance {
                    ax0: a.0.0,
                    ay0: a.0.1,
                    ax1: a.1.0,
                    ay1: a.1.1,
                    bx0: b.0.0,
                    by0: b.0.1,
                    bx1: b.1.0,
                    by1: b.1.1,
                    distance: value,
                    side: side as f64,
                    weight: DEFAULT_WEIGHT,
                });
            }
            DistanceTarget::PointPointDistance {
                anchor,
                mover,
                dir_u: _,
                dir_v: _,
            } => {
                self.hold_point(doc, anchor.clone())?;
                let (ax, ay) = self.point_vars(doc, anchor)?;
                let (mx, my) = self.point_vars(doc, mover)?;
                self.system.add_equation(Equation::PointPointDistance {
                    mx,
                    my,
                    ax,
                    ay,
                    distance: value,
                    weight: DEFAULT_WEIGHT,
                });
            }
            DistanceTarget::PointLineDistance {
                point,
                line,
                side,
            } => {
                self.hold_line(doc, line.clone())?;
                let (px, py) = self.point_vars(doc, point)?;
                let ((x0, y0), (x1, y1)) = self.line_vars(doc, line)?;
                self.system.add_equation(Equation::PointLineDistance {
                    px,
                    py,
                    x0,
                    y0,
                    x1,
                    y1,
                    distance: value,
                    side: side as f64,
                    weight: DEFAULT_WEIGHT,
                });
            }
        }
        Ok(())
    }

    /// Hold a relational constraint's reference line during a full solve (no-op during a drag),
    /// so the dependent geometry moves to it rather than the reference moving. This must stay
    /// strong (a relational constraint like coincident/parallel carries `DEFAULT_WEIGHT`, and
    /// the reference must win so it stays put).
    fn hold_line(&mut self, doc: &Document, line: ConstraintLine) -> Result<(), String> {
        if !self.hold_references {
            return Ok(());
        }
        let ((u0, v0), (u1, v1)) = self.line_vars(doc, line)?;
        for var in [u0, v0, u1, v1] {
            self.hold_var(var, REFERENCE_HOLD_WEIGHT);
        }
        Ok(())
    }

    fn anchor_point(&mut self, doc: &Document, point: ConstraintPoint, weight: f64) -> Result<(), String> {
        let (u, v) = self.point_vars(doc, point)?;
        self.hold_var(u, weight);
        self.hold_var(v, weight);
        Ok(())
    }

    /// Gauge-hold a reference point during a full solve (no-op during a drag). Uses the weak
    /// gauge weight so it stabilises free geometry without fighting real constraints.
    fn hold_point(&mut self, doc: &Document, point: ConstraintPoint) -> Result<(), String> {
        if !self.hold_references {
            return Ok(());
        }
        self.anchor_point(doc, point, GAUGE_HOLD_WEIGHT)
    }

    fn hold_var(&mut self, var: VarId, weight: f64) {
        self.system.add_equation(Equation::Pin {
            var,
            target: self.system.value(var),
            weight,
        });
    }

    fn add_coincident(
        &mut self,
        doc: &Document,
        a: ConstraintEntity,
        b: ConstraintEntity,
    ) -> Result<(), String> {
        match (a, b) {
            (ConstraintEntity::Point(pa), ConstraintEntity::Point(pb)) => {
                use crate::geometric_constraints::coincident_mover_and_anchor;
                let (_mover, anchor) = coincident_mover_and_anchor(pa.clone(), pb.clone());
                self.hold_point(doc, anchor)?;
                let (au, av) = self.point_vars(doc, pa)?;
                let (bu, bv) = self.point_vars(doc, pb)?;
                self.system.add_equation(Equation::CoincidentU {
                    a: au,
                    b: bu,
                    weight: DEFAULT_WEIGHT,
                });
                self.system.add_equation(Equation::CoincidentV {
                    a: av,
                    b: bv,
                    weight: DEFAULT_WEIGHT,
                });
            }
            (ConstraintEntity::Point(point), ConstraintEntity::Line(line))
            | (ConstraintEntity::Line(line), ConstraintEntity::Point(point)) => {
                self.hold_line(doc, line.clone())?;
                let (px, py) = self.point_vars(doc, point)?;
                let ((x0, y0), (x1, y1)) = self.line_vars(doc, line)?;
                self.system.add_equation(Equation::PointLineDistance {
                    px,
                    py,
                    x0,
                    y0,
                    x1,
                    y1,
                    distance: 0.0,
                    side: 1.0,
                    weight: DEFAULT_WEIGHT,
                });
            }
            (ConstraintEntity::Point(point), ConstraintEntity::Circle(circle))
            | (ConstraintEntity::Circle(circle), ConstraintEntity::Point(point)) => {
                let center = ConstraintPoint::CircleCenter(circle);
                self.hold_point(doc, center.clone())?;
                let (px, py) = self.point_vars(doc, point)?;
                let (cx, cy) = self.point_vars(doc, center)?;
                let radius = self
                    .circle_radius
                    .get(&circle)
                    .copied()
                    .ok_or_else(|| format!("Circle {circle} not in solver graph"))?;
                // The circle is the reference: hold its radius so the point moves to the
                // perimeter rather than the circle shrinking to meet the point.
                if self.hold_references {
                    self.hold_var(radius, GAUGE_HOLD_WEIGHT);
                }
                self.system.add_equation(Equation::PointOnCircle {
                    px,
                    py,
                    cx,
                    cy,
                    radius,
                    weight: DEFAULT_WEIGHT,
                });
            }
            (ConstraintEntity::Point(point), ConstraintEntity::Origin)
            | (ConstraintEntity::Origin, ConstraintEntity::Point(point)) => {
                // Pin the point to the sketch origin via a fixed (0, 0) helper point.
                let (px, py) = self.point_vars(doc, point)?;
                let (ox, oy) = self.system.add_point(0.0, 0.0, true);
                self.system.add_equation(Equation::CoincidentU {
                    a: px,
                    b: ox,
                    weight: DEFAULT_WEIGHT,
                });
                self.system.add_equation(Equation::CoincidentV {
                    a: py,
                    b: oy,
                    weight: DEFAULT_WEIGHT,
                });
            }
            (ConstraintEntity::Line(_), ConstraintEntity::Line(_))
            | (ConstraintEntity::Circle(_), ConstraintEntity::Circle(_))
            | (ConstraintEntity::Line(_), ConstraintEntity::Circle(_))
            | (ConstraintEntity::Circle(_), ConstraintEntity::Line(_))
            | (ConstraintEntity::Origin, _)
            | (_, ConstraintEntity::Origin) => {
                return Err("Unsupported coincident entity pair".to_string());
            }
        }
        Ok(())
    }

    /// Resolve a point's solver variables, lazily seeding a `FaceVertex` the first time it's
    /// referenced (#26/#27): unlike sketch-native points, a face's own vertex isn't discovered
    /// by `seed_entities` walking `doc.lines`/`doc.rects`/`doc.circles`, so it's seeded here on
    /// first use instead — as a **fixed** point (mirrors how `add_coincident`'s `Origin` arm
    /// above adds a fixed helper point), since it's not draggable/settable.
    fn point_vars(&mut self, doc: &Document, point: ConstraintPoint) -> Result<(VarId, VarId), String> {
        if let Some(vars) = self.point_vars.get(&point) {
            return Ok(*vars);
        }
        if let ConstraintPoint::FaceVertex { .. } = &point {
            let (u, v) = point_uv(doc, self.sketch, point.clone())?;
            let vars = self.system.add_point(u as f64, v as f64, true);
            self.point_vars.insert(point, vars);
            return Ok(vars);
        }
        Err(format!("Point {point:?} not in solver graph"))
    }

    fn line_vars(
        &mut self,
        doc: &Document,
        line: ConstraintLine,
    ) -> Result<((VarId, VarId), (VarId, VarId)), String> {
        match line {
            ConstraintLine::Line(index) => {
                let start = self.point_vars(
                    doc,
                    ConstraintPoint::LineEndpoint {
                        line: index,
                        end: LineEnd::Start,
                    },
                )?;
                let end = self.point_vars(
                    doc,
                    ConstraintPoint::LineEndpoint {
                        line: index,
                        end: LineEnd::End,
                    },
                )?;
                Ok((start, end))
            }
            // A face's own edge runs between two of its boundary loop's vertices (#26/#27);
            // each resolves (and lazily seeds, if new) through the same `FaceVertex` path above.
            ConstraintLine::FaceEdge { face, index } => {
                let boundary = crate::extrude::face_boundary_loop_world(doc, &face)
                    .ok_or_else(|| "Face boundary not available".to_string())?;
                let n = boundary.len();
                if n == 0 || index >= n {
                    return Err(format!("Face edge {index} out of range"));
                }
                let start = self.point_vars(
                    doc,
                    ConstraintPoint::FaceVertex {
                        face: face.clone(),
                        index,
                    },
                )?;
                let end = self.point_vars(
                    doc,
                    ConstraintPoint::FaceVertex {
                        face,
                        index: (index + 1) % n,
                    },
                )?;
                Ok((start, end))
            }
        }
    }
}

/// Constraint indices with the largest residuals when the sketch fails to solve.
pub fn sketch_conflicting_constraints(
    doc: &Document,
    sketch: SketchId,
) -> Result<Vec<usize>, String> {
    let mut bridge = SketchBridge::from_document(doc, sketch, true)?;
    let report = bridge.solve();
    if report.success {
        return Ok(Vec::new());
    }
    Ok(report.failed_constraints)
}

/// Remaining degrees of freedom for one sketch's constraint system.
pub fn sketch_dof_remaining(doc: &Document, sketch: SketchId) -> Result<i32, String> {
    let bridge = SketchBridge::from_document(doc, sketch, true)?;
    Ok(dof_remaining(&bridge.system))
}

/// Whether a sketch point can still move under the current constraints (reference geometry held).
pub fn sketch_point_movable(
    doc: &Document,
    sketch: SketchId,
    point: ConstraintPoint,
) -> Result<bool, String> {
    let mut bridge = SketchBridge::from_document(doc, sketch, true)?;
    match bridge.point_solver_vars(doc, point) {
        Ok((u, v)) => Ok(vars_can_move_together(&bridge.system, &[u, v])),
        Err(_) => Ok(false),
    }
}

/// Whether a sketch line's endpoints still have any freedom to move.
pub fn sketch_line_vertex_drag_blocked(
    doc: &Document,
    sketch: SketchId,
    line_index: usize,
) -> Result<bool, String> {
    use crate::constraints::find_distance_constraint;
    if find_distance_constraint(doc, DistanceTarget::LineLength(line_index)).is_none() {
        return Ok(false);
    }
    let mut bridge = SketchBridge::from_document(doc, sketch, true)?;
    let start = ConstraintPoint::LineEndpoint {
        line: line_index,
        end: LineEnd::Start,
    };
    let end = ConstraintPoint::LineEndpoint {
        line: line_index,
        end: LineEnd::End,
    };
    let mut line_vars = Vec::new();
    for point in [start, end] {
        if let Ok((u, v)) = bridge.point_solver_vars(doc, point) {
            line_vars.push(u);
            line_vars.push(v);
        }
    }
    Ok(!vars_can_move_together(&bridge.system, &line_vars))
}

/// Solve all sketches in `doc`, optionally pinning points during drag.
pub fn solve_document_sketches(
    doc: &mut Document,
    pins: &[(ConstraintPoint, (f32, f32))],
) -> Result<(), String> {
    let sketches = sketches_to_solve(doc, pins);
    for sketch in sketches {
        solve_one_sketch(doc, sketch, pins)?;
    }
    Ok(())
}

fn sketches_to_solve(doc: &Document, pins: &[(ConstraintPoint, (f32, f32))]) -> Vec<SketchId> {
    let mut sketches = HashSet::new();
    for constraint in &doc.constraints {
        if !constraint.deleted {
            sketches.insert(constraint.sketch);
        }
    }
    // Points being dragged are always sketch-native (a `FaceVertex` is fixed, never a drag
    // pin), so `point_sketch` alone — no need for a `point_uv` existence check too — decides
    // which sketch's solve this pin belongs to.
    for (point, _) in pins {
        if let Some(sketch) = point_sketch(doc, point.clone()) {
            sketches.insert(sketch);
        }
    }
    let mut ordered: Vec<SketchId> = sketches.into_iter().collect();
    ordered.sort_unstable();
    ordered
}

/// The two endpoint points that define a constraint line (line endpoints, rect-edge corners, or
/// — #26/#27 — the two boundary-loop vertices of a face's own edge).
fn line_endpoint_points(doc: &Document, line: ConstraintLine) -> Vec<ConstraintPoint> {
    match line {
        ConstraintLine::Line(index) => vec![
            ConstraintPoint::LineEndpoint {
                line: index,
                end: LineEnd::Start,
            },
            ConstraintPoint::LineEndpoint {
                line: index,
                end: LineEnd::End,
            },
        ],
        ConstraintLine::FaceEdge { face, index } => {
            let Some(boundary) = crate::extrude::face_boundary_loop_world(doc, &face) else {
                return Vec::new();
            };
            let n = boundary.len();
            if n == 0 || index >= n {
                return Vec::new();
            }
            vec![
                ConstraintPoint::FaceVertex {
                    face: face.clone(),
                    index,
                },
                ConstraintPoint::FaceVertex {
                    face,
                    index: (index + 1) % n,
                },
            ]
        }
    }
}

fn point_sketch(doc: &Document, point: ConstraintPoint) -> Option<SketchId> {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => doc.lines.get(line).map(|l| l.sketch),
        ConstraintPoint::CircleCenter(circle) => doc.circles.get(circle).map(|c| c.sketch),
        // A face's own vertex has no owning sketch — it's referenced *from* whichever sketch a
        // constraint projects it into, not owned by one (mirrors `construction::point_sketch`).
        ConstraintPoint::FaceVertex { .. } => None,
    }
}

fn solve_one_sketch(
    doc: &mut Document,
    sketch: SketchId,
    pins: &[(ConstraintPoint, (f32, f32))],
) -> Result<(), String> {
    let sketch_pins: Vec<_> = pins
        .iter()
        .filter(|(point, _)| point_sketch(doc, point.clone()) == Some(sketch))
        .cloned()
        .collect();
    let hold_references = sketch_pins.is_empty();
    let mut bridge = SketchBridge::from_document(doc, sketch, hold_references)?;
    bridge.add_drag_pins(doc, &sketch_pins);
    let _report = bridge.solve();
    bridge.apply_to_document(doc)?;
    Ok(())
}

fn set_point_uv_from_solver(
    doc: &mut Document,
    sketch: SketchId,
    point: ConstraintPoint,
    u: f64,
    v: f64,
) -> Result<(), String> {
    // `sketch` is only meaningful for `FaceVertex` (fixed, so `set_point_uv` always errors on
    // it anyway) — every point this is actually called with (`LineEndpoint`/`RectCorner`/
    // `CircleCenter`) ignores it.
    crate::geometric_constraints::set_point_uv(doc, sketch, point, u as f32, v as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::{add_distance_constraint, find_distance_constraint};
    use crate::geometric_constraints::{
        add_geometric_constraint_from_selection, line_direction_uv, GeometricConstraintType,
    };
    use crate::hierarchy::SceneElement;
    use crate::model::{Constraint, ConstraintKind, Document, FaceId, Line, Rect};
    use crate::selection::{click_scene_selection, SceneSelection};

    const EPS: f32 = 1e-2;

    fn sketch_doc() -> (Document, SketchId) {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        (doc, sketch)
    }

    fn solve_bridge(doc: &mut Document, _sketch: SketchId) {
        solve_document_sketches(doc, &[]).expect("solve");
    }

    /// A rectangle has 4 degrees of freedom (x, y, w, h), not 8 — its corners are not
    /// four independent points. The solver must keep it rigid/axis-aligned.
    #[test]
    fn free_rectangle_reports_four_dof() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 6.0));
        assert_eq!(sketch_dof_remaining(&doc, sketch).unwrap(), 4);
    }

    /// When a constraint pulls one rectangle corner, the rectangle must stay rigid so the
    /// corner actually tracks what it is tied to (here, a coincident line endpoint).
    #[test]
    fn solver_keeps_rect_rigid_when_corner_pulled_by_coincidence() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 6.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 10.0, 6.0, 16.0, 6.0));
        doc.shape_order.push(crate::model::ShapeKind::Rect);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        let corner2 = ConstraintPoint::RectCorner { rect: 0, corner: 2 };
        let line_start = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::Start,
        };
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Point(corner2.clone()), false);
        click_scene_selection(&mut sel, SceneElement::Point(line_start.clone()), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Horizontal,
            &sel,
        )
        .unwrap();

        // Pull the far line endpoint down; the near end (and the rect corner) must follow.
        let line_end = ConstraintPoint::LineEndpoint {
            line: 0,
            end: LineEnd::End,
        };
        let pins = [(line_end, (16.0_f32, 3.0_f32))];
        solve_document_sketches(&mut doc, &pins).unwrap();

        let (lsu, lsv) = point_uv(&doc, sketch, line_start).unwrap();
        let (c2u, c2v) = point_uv(&doc, sketch, corner2).unwrap();
        assert!(
            (c2u - lsu).abs() < 0.1 && (c2v - lsv).abs() < 0.1,
            "rect corner detached from coincident line start: corner=({c2u},{c2v}) line_start=({lsu},{lsv})"
        );
        assert!((c2v - 3.0).abs() < 0.3, "rect corner should follow to v=3, got {c2v}");
    }

    /// Dragging the movable line of a parallel pair must not drag the reference line.
    #[test]
    fn drag_parallel_movable_does_not_move_reference() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 100.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 40.0, 100.0, 40.0));
        doc.shape_order.push(crate::model::ShapeKind::Line);
        doc.shape_order.push(crate::model::ShapeKind::Line);
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Parallel,
            &sel,
        )
        .unwrap();

        let pins = [(
            ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::End,
            },
            (100.0_f32, 80.0_f32),
        )];
        solve_document_sketches(&mut doc, &pins).unwrap();

        let a = &doc.lines[0];
        assert!(
            a.x0.abs() < 0.5 && a.y0.abs() < 0.5 && (a.x1 - 100.0).abs() < 0.5 && a.y1.abs() < 0.5,
            "reference line A drifted to ({},{})-({},{})",
            a.x0,
            a.y0,
            a.x1,
            a.y1
        );
    }

    #[test]
    fn bridge_conflicting_constraints_reports_largest_residuals() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Distance {
                target: DistanceTarget::LineLength(0),
            },
            expression: "10mm".to_string(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Distance {
                target: DistanceTarget::LineLength(0),
            },
            expression: "12mm".to_string(),
            dim_offset: None,
            name: None,
            deleted: false,
        });

        let mut bridge = SketchBridge::from_document(&doc, sketch, true).unwrap();
        let report = bridge.solve();
        assert!(!report.success);
        assert_eq!(report.failed_constraints.len(), 2);
        assert!(report.failed_constraints.contains(&0));
        assert!(report.failed_constraints.contains(&1));
    }

    #[test]
    #[ignore = "run with `cargo test --release solve_perf -- --ignored`"]
    fn solve_perf_100_constraints_under_5ms() {
        use std::time::Instant;

        let (mut doc, sketch) = sketch_doc();
        for i in 0..50 {
            let y = i as f32 * 5.0;
            doc.lines.push(Line::from_local_endpoints(
                sketch,
                0.0,
                y,
                100.0,
                y + 3.0,
            ));
        }
        for index in 0..doc.lines.len() {
            add_distance_constraint(
                &mut doc,
                sketch,
                DistanceTarget::LineLength(index),
                "100mm".to_string(),
            )
            .unwrap();
        }
        let start = Instant::now();
        solve_document_sketches(&mut doc, &[]).unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 5,
            "solve took {} ms",
            elapsed.as_millis()
        );
    }

    #[test]
    fn bridge_sketch_dof_remaining_reports_underconstrained_line() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        assert!(sketch_dof_remaining(&doc, sketch).unwrap() > 0);
    }

    #[test]
    fn bridge_rect_width_updates_with_height_constraint() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectWidth(0),
            "10mm".to_string(),
        )
        .unwrap();
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::RectHeight(0),
            "5mm".to_string(),
        )
        .unwrap();
        let width_id = find_distance_constraint(&doc, DistanceTarget::RectWidth(0)).unwrap();
        crate::constraints::set_constraint_expression(&mut doc, width_id, "20mm".to_string()).unwrap();
        assert!(
            (doc.rects[0].w - 20.0).abs() < EPS,
            "w={} corners: {:?}",
            doc.rects[0].w,
            (
                point_uv(
                    &doc,
                    sketch,
                    ConstraintPoint::RectCorner { rect: 0, corner: 0 },
                )
                .unwrap(),
                point_uv(
                    &doc,
                    sketch,
                    ConstraintPoint::RectCorner { rect: 0, corner: 1 },
                )
                .unwrap(),
            )
        );
    }

    #[test]
    fn bridge_round_trip_line() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::LineLength(0),
            "10mm".to_string(),
        )
        .unwrap();
        doc.lines[0].x1 = 7.0;
        solve_bridge(&mut doc, sketch);
        assert!((doc.lines[0].length() - 10.0).abs() < EPS);
    }

    #[test]
    fn bridge_rect_edge_parallel() {
        let (mut doc, sketch) = sketch_doc();
        doc.rects
            .push(Rect::from_local_corners(sketch, 0.0, 0.0, 10.0, 5.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 10.0, 3.0, 13.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Bottom), false);
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Parallel,
            &sel,
        )
        .unwrap();
        solve_bridge(&mut doc, sketch);
        let edge = ConstraintLine::RectEdge {
            rect: 0,
            edge: RectEdge::Bottom,
        };
        let (edu, edv) = line_direction_uv(&doc, sketch, edge).unwrap();
        let (ldu, ldv) = line_direction_uv(&doc, sketch, ConstraintLine::Line(0)).unwrap();
        let cross = edu * ldv - edv * ldu;
        assert!(cross.abs() < EPS, "cross={cross}");
    }

    #[test]
    fn bridge_equal_makes_two_lines_equal_length() {
        let (mut doc, sketch) = sketch_doc();
        // A horizontal line of length 10 and a horizontal line of length 4.
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 5.0, 4.0, 5.0));
        doc.shape_order.push(crate::model::ShapeKind::Line);
        doc.shape_order.push(crate::model::ShapeKind::Line);
        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Equal,
            &sel,
        )
        .unwrap();
        solve_bridge(&mut doc, sketch);
        assert!(
            (doc.lines[0].length() - doc.lines[1].length()).abs() < EPS,
            "lengths: {} vs {}",
            doc.lines[0].length(),
            doc.lines[1].length()
        );
    }

    #[test]
    fn bridge_point_line_distance() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 5.0, -4.0, 6.0, -4.0));
        doc.shape_order.push(crate::model::ShapeKind::Line);
        doc.shape_order.push(crate::model::ShapeKind::Line);
        let point = ConstraintPoint::LineEndpoint {
            line: 1,
            end: LineEnd::Start,
        };
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::PointLineDistance {
                point: point.clone(),
                line: ConstraintLine::Line(0),
                side: 1,
            },
            "3mm".to_string(),
        )
        .unwrap();
        solve_bridge(&mut doc, sketch);
        let (_pu, pv) = point_uv(&doc, sketch, point).unwrap();
        assert!((pv + 3.0).abs() < 0.2, "pv={pv}");
    }

    #[test]
    fn bridge_midpoint() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 4.0, 8.0, 5.0, 9.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(
            &mut sel,
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::Start,
            }),
            false,
        );
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Midpoint,
            &sel,
        )
        .unwrap();
        solve_bridge(&mut doc, sketch);
        let (pu, pv) = point_uv(
            &doc,
            sketch,
            ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::Start,
            },
        )
        .unwrap();
        assert!((pu - 5.0).abs() < EPS);
        assert!(pv.abs() < EPS);
    }

    #[test]
    fn bridge_coincident_point_on_line() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 5.0, 8.0, 6.0, 9.0));
        let mut sel = SceneSelection::default();
        click_scene_selection(
            &mut sel,
            SceneElement::Point(ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::Start,
            }),
            false,
        );
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Coincident,
            &sel,
        )
        .unwrap();
        solve_bridge(&mut doc, sketch);
        let (pu, pv) = point_uv(
            &doc,
            sketch,
            ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::Start,
            },
        )
        .unwrap();
        assert!((pu - 5.0).abs() < EPS);
        assert!(pv.abs() < EPS);
    }

    #[test]
    fn bridge_rect_parallel_perpendicular_point_line_distance() {
        let (mut doc, sketch) = sketch_doc();
        doc.lines
            .push(Line::from_local_endpoints(sketch, 0.0, 0.0, 100.0, 0.0));
        doc.rects
            .push(Rect::from_local_corners(sketch, 20.0, 10.0, 70.0, 40.0));
        doc.lines
            .push(Line::from_local_endpoints(sketch, 30.0, 55.0, 30.0, 85.0));
        doc.shape_order.push(crate::model::ShapeKind::Line);
        doc.shape_order.push(crate::model::ShapeKind::Rect);
        doc.shape_order.push(crate::model::ShapeKind::Line);

        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::RectEdge(0, RectEdge::Top), false);
        click_scene_selection(&mut sel, SceneElement::Line(0), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Parallel,
            &sel,
        )
        .unwrap();

        let mut sel = SceneSelection::default();
        click_scene_selection(&mut sel, SceneElement::Line(0), false);
        click_scene_selection(&mut sel, SceneElement::Line(1), true);
        add_geometric_constraint_from_selection(
            &mut doc,
            sketch,
            GeometricConstraintType::Perpendicular,
            &sel,
        )
        .unwrap();

        let rect_top = ConstraintLine::RectEdge {
            rect: 0,
            edge: RectEdge::Top,
        };
        add_distance_constraint(
            &mut doc,
            sketch,
            DistanceTarget::PointLineDistance {
                point: ConstraintPoint::LineEndpoint {
                    line: 1,
                    end: LineEnd::Start,
                },
                line: rect_top,
                side: 1,
            },
            "50mm".to_string(),
        )
        .unwrap();

        solve_bridge(&mut doc, sketch);

        let (adu, adv) = line_direction_uv(&doc, sketch, ConstraintLine::Line(0)).unwrap();
        let (bdu, bdv) = line_direction_uv(&doc, sketch, ConstraintLine::Line(1)).unwrap();
        assert!((adu * bdu + adv * bdv).abs() < EPS);

        let pins = [(
            ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::End,
            },
            (45.0_f32, 100.0_f32),
        )];
        solve_document_sketches(&mut doc, &pins).unwrap();
        let (adu, adv) = line_direction_uv(&doc, sketch, ConstraintLine::Line(0)).unwrap();
        let (bdu, bdv) = line_direction_uv(&doc, sketch, ConstraintLine::Line(1)).unwrap();
        assert!((adu * bdu + adv * bdv).abs() < 0.05, "perpendicular broken after drag");
        let line = &doc.lines[1];
        assert!((line.x1 - 45.0).abs() < 0.1);
        assert!((line.y1 - 100.0).abs() < 0.1);
    }
}