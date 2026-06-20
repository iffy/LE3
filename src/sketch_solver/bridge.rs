//! Bridge between [`Document`] sketch geometry and the numeric solver.

use super::dof::{dof_remaining, vars_can_move_together};
use super::newton::{solve_lm, SolveReport, SolverConfig};
use super::residuals::{Equation, DEFAULT_WEIGHT, DRAG_PIN_WEIGHT, REFERENCE_HOLD_WEIGHT};
use crate::geometric_constraints::parallel_reference_and_movable;
use super::system::{System, VarId};
use crate::document_lifecycle::constraint_kind_applicable;
use crate::geometric_constraints::point_uv;
use crate::model::{
    ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, DistanceTarget, Document,
    LineEnd, RectEdge, SketchId,
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
        let pinned: HashSet<ConstraintPoint> = pins.iter().map(|(point, _)| *point).collect();
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
            match constraint.kind {
                ConstraintKind::Distance {
                    target: DistanceTarget::PointPointDistance { anchor, mover, .. },
                } => {
                    if pinned.contains(&mover) && !pinned.contains(&anchor) {
                        let _ = self.anchor_point(anchor, REFERENCE_HOLD_WEIGHT);
                    } else if pinned.contains(&anchor) && !pinned.contains(&mover) {
                        let _ = self.anchor_point(mover, REFERENCE_HOLD_WEIGHT);
                    }
                }
                ConstraintKind::Distance {
                    target: DistanceTarget::PointLineDistance { point, line, .. },
                } => self.hold_reference_when_point_dragged(line, point, &pinned),
                ConstraintKind::Midpoint { point, line } => {
                    self.hold_reference_when_point_dragged(line, point, &pinned)
                }
                ConstraintKind::Distance {
                    target: DistanceTarget::LineLineDistance { line_a, line_b, .. },
                }
                | ConstraintKind::Parallel { line_a, line_b }
                | ConstraintKind::Perpendicular { line_a, line_b }
                | ConstraintKind::Angle { line_a, line_b, .. } => {
                    let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
                    self.hold_reference_when_movable_dragged(reference, movable, &pinned);
                }
                _ => {}
            }
        }
    }

    /// Hold a constraint's reference line if the dragged geometry is the movable line (and the
    /// reference itself isn't being dragged).
    fn hold_reference_when_movable_dragged(
        &mut self,
        reference: ConstraintLine,
        movable: ConstraintLine,
        pinned: &HashSet<ConstraintPoint>,
    ) {
        let reference_points = line_endpoint_points(reference);
        let movable_points = line_endpoint_points(movable);
        let movable_dragged = movable_points.iter().any(|p| pinned.contains(p));
        let reference_dragged = reference_points.iter().any(|p| pinned.contains(p));
        if movable_dragged && !reference_dragged {
            for point in reference_points {
                let _ = self.anchor_point(point, REFERENCE_HOLD_WEIGHT);
            }
        }
    }

    /// Hold a reference line if the dragged geometry is the constrained point (and the line
    /// itself isn't being dragged).
    fn hold_reference_when_point_dragged(
        &mut self,
        line: ConstraintLine,
        point: ConstraintPoint,
        pinned: &HashSet<ConstraintPoint>,
    ) {
        let reference_points = line_endpoint_points(line);
        let reference_dragged = reference_points.iter().any(|p| pinned.contains(p));
        if pinned.contains(&point) && !reference_dragged {
            for reference_point in reference_points {
                let _ = self.anchor_point(reference_point, REFERENCE_HOLD_WEIGHT);
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

    pub fn point_solver_vars(&self, point: ConstraintPoint) -> Result<(VarId, VarId), String> {
        self.point_vars(point)
    }

    pub fn apply_to_document(&self, doc: &mut Document) -> Result<(), String> {
        for (point, (u_id, v_id)) in &self.point_vars {
            if let ConstraintPoint::LineEndpoint { .. } = point {
                set_point_uv_from_solver(doc, *point, self.system.value(*u_id), self.system.value(*v_id))?;
            }
        }

        let mut rect_corners: HashMap<usize, [(f64, f64); 4]> = HashMap::new();
        for (point, (u_id, v_id)) in &self.point_vars {
            if let ConstraintPoint::RectCorner { rect, corner } = point {
                let entry = rect_corners.entry(*rect).or_insert([(0.0, 0.0); 4]);
                entry[*corner as usize] =
                    (self.system.value(*u_id), self.system.value(*v_id));
            }
        }
        for (rect, corners) in rect_corners {
            apply_rect_corners(doc, rect, corners)?;
        }

        for (circle, radius_var) in &self.circle_radius {
            let center = ConstraintPoint::CircleCenter(*circle);
            if let Some((u_id, v_id)) = self.point_vars.get(&center) {
                set_point_uv_from_solver(
                    doc,
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
        for (index, rect) in doc.rects.iter().enumerate() {
            if rect.deleted || rect.sketch != self.sketch {
                continue;
            }
            for corner in 0..4u8 {
                self.ensure_rect_corner(doc, index, corner)?;
            }
            self.add_rect_rigidity(index)?;
        }
        for (index, circle) in doc.circles.iter().enumerate() {
            if circle.deleted || circle.sketch != self.sketch {
                continue;
            }
            let center = ConstraintPoint::CircleCenter(index);
            if !self.point_vars.contains_key(&center) {
                let (u, v) = point_uv(doc, center)?;
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
        let (u, v) = point_uv(doc, point)?;
        let (u_id, v_id) = self.system.add_point(u as f64, v as f64, false);
        self.point_vars.insert(point, (u_id, v_id));
        Ok(())
    }

    fn ensure_rect_corner(&mut self, doc: &Document, rect: usize, corner: u8) -> Result<(), String> {
        let point = ConstraintPoint::RectCorner { rect, corner };
        if self.point_vars.contains_key(&point) {
            return Ok(());
        }
        let (u, v) = point_uv(doc, point)?;
        let (u_id, v_id) = self.system.add_point(u as f64, v as f64, false);
        self.point_vars.insert(point, (u_id, v_id));
        Ok(())
    }

    /// A `Rect` is stored axis-aligned (x, y, w, h), so its four solver corners must stay a
    /// rigid axis-aligned rectangle: bottom/top edges horizontal, left/right edges vertical.
    /// Without this the corners drift independently — a corner moved by a constraint detaches
    /// from the rest of the rectangle, and `apply_rect_corners`' bounding box collapses it.
    fn add_rect_rigidity(&mut self, rect: usize) -> Result<(), String> {
        let corner_vars = |corner: u8| ConstraintPoint::RectCorner { rect, corner };
        let (c0x, c0y) = self.point_vars(corner_vars(0))?;
        let (c1x, c1y) = self.point_vars(corner_vars(1))?;
        let (c2x, c2y) = self.point_vars(corner_vars(2))?;
        let (c3x, c3y) = self.point_vars(corner_vars(3))?;
        // Bottom and top edges horizontal.
        self.system.add_equation(Equation::Horizontal {
            y0: c0y,
            y1: c1y,
            weight: DEFAULT_WEIGHT,
        });
        self.system.add_equation(Equation::Horizontal {
            y0: c3y,
            y1: c2y,
            weight: DEFAULT_WEIGHT,
        });
        // Left and right edges vertical.
        self.system.add_equation(Equation::Vertical {
            x0: c0x,
            x1: c3x,
            weight: DEFAULT_WEIGHT,
        });
        self.system.add_equation(Equation::Vertical {
            x0: c1x,
            x1: c2x,
            weight: DEFAULT_WEIGHT,
        });
        Ok(())
    }

    fn add_constraints(&mut self, doc: &Document) -> Result<(), String> {
        for (index, constraint) in doc.constraints.iter().enumerate() {
            if constraint.deleted || constraint.sketch != self.sketch {
                continue;
            }
            if !constraint_kind_applicable(doc, constraint.kind) {
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
        match constraint.kind {
            ConstraintKind::Distance { target } => {
                self.add_distance_constraint(doc, constraint, target)?;
            }
            ConstraintKind::Horizontal { line } => {
                let ((x0, y0), (x1, y1)) = self.line_vars(line)?;
                self.system.add_equation(Equation::Horizontal {
                    y0,
                    y1,
                    weight: DEFAULT_WEIGHT,
                });
                let _ = (x0, x1);
            }
            ConstraintKind::Vertical { line } => {
                let ((x0, y0), (x1, y1)) = self.line_vars(line)?;
                self.system.add_equation(Equation::Vertical {
                    x0,
                    x1,
                    weight: DEFAULT_WEIGHT,
                });
                let _ = (y0, y1);
            }
            ConstraintKind::Parallel { line_a, line_b } => {
                let (reference, movable) = parallel_reference_and_movable(line_a, line_b);
                self.hold_line(reference, REFERENCE_HOLD_WEIGHT)?;
                let a = self.line_vars(reference)?;
                let b = self.line_vars(movable)?;
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
                self.hold_line(reference, REFERENCE_HOLD_WEIGHT)?;
                let a = self.line_vars(reference)?;
                let b = self.line_vars(movable)?;
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
            ConstraintKind::Coincident { a, b } => self.add_coincident(a, b)?,
            ConstraintKind::Midpoint { point, line } => {
                self.hold_line(line, REFERENCE_HOLD_WEIGHT)?;
                let (pu, pv) = self.point_vars(point)?;
                let ((x0, y0), (x1, y1)) = self.line_vars(line)?;
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
                self.hold_line(reference, REFERENCE_HOLD_WEIGHT)?;
                let a = self.line_vars(reference)?;
                let b = self.line_vars(movable)?;
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
                    ConstraintPoint::LineEndpoint {
                        line: index,
                        end: LineEnd::Start,
                    },
                    REFERENCE_HOLD_WEIGHT,
                )?;
                let line = ConstraintLine::Line(index);
                let ((x0, y0), (x1, y1)) = self.line_vars(line)?;
                self.system.add_equation(Equation::LineLength {
                    x0,
                    y0,
                    x1,
                    y1,
                    length: value,
                    weight: DEFAULT_WEIGHT,
                });
            }
            DistanceTarget::RectWidth(index) => {
                self.anchor_point(
                    ConstraintPoint::RectCorner { rect: index, corner: 0 },
                    REFERENCE_HOLD_WEIGHT,
                )?;
                let bottom = ConstraintLine::RectEdge {
                    rect: index,
                    edge: RectEdge::Bottom,
                };
                let left = ConstraintLine::RectEdge {
                    rect: index,
                    edge: RectEdge::Left,
                };
                let ((bx0, by0), (bx1, by1)) = self.line_vars(bottom)?;
                let ((lx0, _ly0), (lx1, _ly1)) = self.line_vars(left)?;
                self.system.add_equation(Equation::Horizontal {
                    y0: by0,
                    y1: by1,
                    weight: DEFAULT_WEIGHT,
                });
                self.system.add_equation(Equation::Vertical {
                    x0: lx0,
                    x1: lx1,
                    weight: DEFAULT_WEIGHT,
                });
                self.system.add_equation(Equation::LineLength {
                    x0: bx0,
                    y0: by0,
                    x1: bx1,
                    y1: by1,
                    length: value,
                    weight: DEFAULT_WEIGHT,
                });
            }
            DistanceTarget::RectHeight(index) => {
                self.anchor_point(
                    ConstraintPoint::RectCorner { rect: index, corner: 0 },
                    REFERENCE_HOLD_WEIGHT,
                )?;
                self.anchor_point_v(
                    ConstraintPoint::RectCorner { rect: index, corner: 1 },
                    REFERENCE_HOLD_WEIGHT,
                )?;
                let right = ConstraintLine::RectEdge {
                    rect: index,
                    edge: RectEdge::Right,
                };
                let ((rx0, ry0), (rx1, ry1)) = self.line_vars(right)?;
                self.system.add_equation(Equation::Vertical {
                    x0: rx0,
                    x1: rx1,
                    weight: DEFAULT_WEIGHT,
                });
                self.system.add_equation(Equation::LineLength {
                    x0: rx0,
                    y0: ry0,
                    x1: rx1,
                    y1: ry1,
                    length: value,
                    weight: DEFAULT_WEIGHT,
                });
            }
            DistanceTarget::CircleDiameter(index) => {
                self.anchor_point(ConstraintPoint::CircleCenter(index), REFERENCE_HOLD_WEIGHT)?;
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
                self.hold_line(reference, REFERENCE_HOLD_WEIGHT)?;
                let a = self.line_vars(reference)?;
                let b = self.line_vars(movable)?;
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
                self.hold_point(anchor, REFERENCE_HOLD_WEIGHT)?;
                let (ax, ay) = self.point_vars(anchor)?;
                let (mx, my) = self.point_vars(mover)?;
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
                self.hold_line(line, REFERENCE_HOLD_WEIGHT)?;
                let (px, py) = self.point_vars(point)?;
                let ((x0, y0), (x1, y1)) = self.line_vars(line)?;
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

    fn hold_line(&mut self, line: ConstraintLine, weight: f64) -> Result<(), String> {
        if !self.hold_references {
            return Ok(());
        }
        let ((u0, v0), (u1, v1)) = self.line_vars(line)?;
        for var in [u0, v0, u1, v1] {
            self.hold_var(var, weight);
        }
        Ok(())
    }

    fn anchor_point(&mut self, point: ConstraintPoint, weight: f64) -> Result<(), String> {
        let (u, v) = self.point_vars(point)?;
        self.hold_var(u, weight);
        self.hold_var(v, weight);
        Ok(())
    }

    fn anchor_point_v(&mut self, point: ConstraintPoint, weight: f64) -> Result<(), String> {
        let (_, v) = self.point_vars(point)?;
        self.hold_var(v, weight);
        Ok(())
    }

    fn hold_point(&mut self, point: ConstraintPoint, weight: f64) -> Result<(), String> {
        if !self.hold_references {
            return Ok(());
        }
        self.anchor_point(point, weight)
    }

    fn hold_var(&mut self, var: VarId, weight: f64) {
        self.system.add_equation(Equation::Pin {
            var,
            target: self.system.value(var),
            weight,
        });
    }

    fn add_coincident(&mut self, a: ConstraintEntity, b: ConstraintEntity) -> Result<(), String> {
        match (a, b) {
            (ConstraintEntity::Point(pa), ConstraintEntity::Point(pb)) => {
                use crate::geometric_constraints::coincident_mover_and_anchor;
                let (_mover, anchor) = coincident_mover_and_anchor(pa, pb);
                self.hold_point(anchor, REFERENCE_HOLD_WEIGHT)?;
                let (au, av) = self.point_vars(pa)?;
                let (bu, bv) = self.point_vars(pb)?;
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
                self.hold_line(line, REFERENCE_HOLD_WEIGHT)?;
                let (px, py) = self.point_vars(point)?;
                let ((x0, y0), (x1, y1)) = self.line_vars(line)?;
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
            (ConstraintEntity::Line(_), ConstraintEntity::Line(_)) => {
                return Err("Coincident line-line is not supported".to_string());
            }
        }
        Ok(())
    }

    fn point_vars(&self, point: ConstraintPoint) -> Result<(VarId, VarId), String> {
        self.point_vars
            .get(&point)
            .copied()
            .ok_or_else(|| format!("Point {point:?} not in solver graph"))
    }

    fn line_vars(&self, line: ConstraintLine) -> Result<((VarId, VarId), (VarId, VarId)), String> {
        match line {
            ConstraintLine::Line(index) => {
                let start = self.point_vars(ConstraintPoint::LineEndpoint {
                    line: index,
                    end: LineEnd::Start,
                })?;
                let end = self.point_vars(ConstraintPoint::LineEndpoint {
                    line: index,
                    end: LineEnd::End,
                })?;
                Ok((start, end))
            }
            ConstraintLine::RectEdge { rect, edge } => {
                let (c0, c1) = edge.corner_indices();
                let start = self.point_vars(ConstraintPoint::RectCorner { rect, corner: c0 })?;
                let end = self.point_vars(ConstraintPoint::RectCorner { rect, corner: c1 })?;
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
    let bridge = SketchBridge::from_document(doc, sketch, true)?;
    match bridge.point_solver_vars(point) {
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
    let bridge = SketchBridge::from_document(doc, sketch, true)?;
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
        if let Ok((u, v)) = bridge.point_solver_vars(point) {
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
    for (point, _) in pins {
        if let Ok((u, v)) = point_uv(doc, *point) {
            let _ = (u, v);
            if let Some(sketch) = point_sketch(doc, *point) {
                sketches.insert(sketch);
            }
        }
    }
    let mut ordered: Vec<SketchId> = sketches.into_iter().collect();
    ordered.sort_unstable();
    ordered
}

/// The two endpoint points that define a constraint line (line endpoints or rect-edge corners).
fn line_endpoint_points(line: ConstraintLine) -> [ConstraintPoint; 2] {
    match line {
        ConstraintLine::Line(index) => [
            ConstraintPoint::LineEndpoint {
                line: index,
                end: LineEnd::Start,
            },
            ConstraintPoint::LineEndpoint {
                line: index,
                end: LineEnd::End,
            },
        ],
        ConstraintLine::RectEdge { rect, edge } => {
            let (c0, c1) = edge.corner_indices();
            [
                ConstraintPoint::RectCorner { rect, corner: c0 },
                ConstraintPoint::RectCorner { rect, corner: c1 },
            ]
        }
    }
}

fn point_sketch(doc: &Document, point: ConstraintPoint) -> Option<SketchId> {
    match point {
        ConstraintPoint::LineEndpoint { line, .. } => doc.lines.get(line).map(|l| l.sketch),
        ConstraintPoint::RectCorner { rect, .. } => doc.rects.get(rect).map(|r| r.sketch),
        ConstraintPoint::CircleCenter(circle) => doc.circles.get(circle).map(|c| c.sketch),
    }
}

fn solve_one_sketch(
    doc: &mut Document,
    sketch: SketchId,
    pins: &[(ConstraintPoint, (f32, f32))],
) -> Result<(), String> {
    let sketch_pins: Vec<_> = pins
        .iter()
        .filter(|(point, _)| point_sketch(doc, *point) == Some(sketch))
        .copied()
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
    point: ConstraintPoint,
    u: f64,
    v: f64,
) -> Result<(), String> {
    crate::geometric_constraints::set_point_uv(doc, point, u as f32, v as f32)
}

fn apply_rect_corners(doc: &mut Document, rect: usize, corners: [(f64, f64); 4]) -> Result<(), String> {
    let entity = doc
        .rects
        .get_mut(rect)
        .ok_or_else(|| format!("Rectangle {rect} not found"))?;
    let min_u = corners
        .iter()
        .map(|(u, _)| *u as f32)
        .fold(f32::INFINITY, f32::min);
    let max_u = corners
        .iter()
        .map(|(u, _)| *u as f32)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_v = corners
        .iter()
        .map(|(_, v)| *v as f32)
        .fold(f32::INFINITY, f32::min);
    let max_v = corners
        .iter()
        .map(|(_, v)| *v as f32)
        .fold(f32::NEG_INFINITY, f32::max);
    entity.x = min_u;
    entity.y = min_v;
    entity.w = (max_u - min_u).max(1e-3);
    entity.h = (max_v - min_v).max(1e-3);
    Ok(())
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
        click_scene_selection(&mut sel, SceneElement::Point(corner2), false);
        click_scene_selection(&mut sel, SceneElement::Point(line_start), true);
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

        let (lsu, lsv) = point_uv(&doc, line_start).unwrap();
        let (c2u, c2v) = point_uv(&doc, corner2).unwrap();
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
                    ConstraintPoint::RectCorner { rect: 0, corner: 0 },
                )
                .unwrap(),
                point_uv(
                    &doc,
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
        let (edu, edv) = line_direction_uv(&doc, edge).unwrap();
        let (ldu, ldv) = line_direction_uv(&doc, ConstraintLine::Line(0)).unwrap();
        let cross = edu * ldv - edv * ldu;
        assert!(cross.abs() < EPS, "cross={cross}");
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
                point,
                line: ConstraintLine::Line(0),
                side: 1,
            },
            "3mm".to_string(),
        )
        .unwrap();
        solve_bridge(&mut doc, sketch);
        let (_pu, pv) = point_uv(&doc, point).unwrap();
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

        let (adu, adv) = line_direction_uv(&doc, ConstraintLine::Line(0)).unwrap();
        let (bdu, bdv) = line_direction_uv(&doc, ConstraintLine::Line(1)).unwrap();
        assert!((adu * bdu + adv * bdv).abs() < EPS);

        let pins = [(
            ConstraintPoint::LineEndpoint {
                line: 1,
                end: LineEnd::End,
            },
            (45.0_f32, 100.0_f32),
        )];
        solve_document_sketches(&mut doc, &pins).unwrap();
        let (adu, adv) = line_direction_uv(&doc, ConstraintLine::Line(0)).unwrap();
        let (bdu, bdv) = line_direction_uv(&doc, ConstraintLine::Line(1)).unwrap();
        assert!((adu * bdu + adv * bdv).abs() < 0.05, "perpendicular broken after drag");
        let line = &doc.lines[1];
        assert!((line.x1 - 45.0).abs() < 0.1);
        assert!((line.y1 - 100.0).abs() < 0.1);
    }
}