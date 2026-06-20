//! Native Rust sketch constraint solver (Levenberg–Marquardt).
//!
//! Replaces the procedural `apply_*` constraint functions with a numeric system
//! that minimizes weighted residuals simultaneously. See `plan.md` for the full
//! implementation roadmap.

mod bridge;
mod dof;
mod newton;
mod residuals;
mod system;

pub use bridge::{
    sketch_conflicting_constraints, sketch_dof_remaining, sketch_line_vertex_drag_blocked,
    sketch_point_movable, solve_document_sketches,
};

#[cfg(test)]
mod tests {
    use super::newton::{solve_lm, SolveReport, SolverConfig};
    use super::residuals::{Equation, DEFAULT_WEIGHT};
    use super::system::System;

    const EPS: f64 = 1e-5;
    const PIN_WEIGHT: f64 = 1e6;

    fn solve(system: &mut System) -> SolveReport {
        solve_lm(system, SolverConfig::default())
    }

    #[test]
    fn horizontal_line_solves() {
        let mut sys = System::new();
        let (_x0, y0) = sys.add_point(0.0, 0.0, false);
        let (_x1, y1) = sys.add_point(10.0, 3.0, false);
        sys.add_equation(Equation::Horizontal {
            y0,
            y1,
            weight: DEFAULT_WEIGHT,
        });
        let report = solve(&mut sys);
        assert!(report.success, "residual={}", report.residual_norm);
        assert!((sys.value(y0) - sys.value(y1)).abs() < EPS);
    }

    #[test]
    fn vertical_line_solves() {
        let mut sys = System::new();
        let (x0, _y0) = sys.add_point(0.0, 0.0, false);
        let (x1, _y1) = sys.add_point(4.0, 10.0, false);
        sys.add_equation(Equation::Vertical {
            x0,
            x1,
            weight: DEFAULT_WEIGHT,
        });
        let report = solve(&mut sys);
        assert!(report.success);
        assert!((sys.value(x0) - sys.value(x1)).abs() < EPS);
    }

    #[test]
    fn coincident_points_merge() {
        let mut sys = System::new();
        let (ax, ay) = sys.add_point(1.0, 2.0, false);
        let (bx, by) = sys.add_point(8.0, 9.0, false);
        sys.add_equation(Equation::CoincidentU {
            a: ax,
            b: bx,
            weight: DEFAULT_WEIGHT,
        });
        sys.add_equation(Equation::CoincidentV {
            a: ay,
            b: by,
            weight: DEFAULT_WEIGHT,
        });
        let report = solve(&mut sys);
        assert!(report.success);
        assert!((sys.value(ax) - sys.value(bx)).abs() < EPS);
        assert!((sys.value(ay) - sys.value(by)).abs() < EPS);
    }

    #[test]
    fn line_length_enforced() {
        let mut sys = System::new();
        let (x0, y0) = sys.add_point(0.0, 0.0, true);
        let (x1, y1) = sys.add_point(5.0, 5.0, false);
        sys.add_equation(Equation::LineLength {
            x0,
            y0,
            x1,
            y1,
            length: 10.0,
            weight: DEFAULT_WEIGHT,
        });
        let report = solve(&mut sys);
        assert!(report.success);
        let dx = sys.value(x1) - sys.value(x0);
        let dy = sys.value(y1) - sys.value(y0);
        assert!((dx.hypot(dy) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn parallel_lines_align() {
        let mut sys = System::new();
        let (ax0, ay0) = sys.add_point(0.0, 0.0, true);
        let (ax1, ay1) = sys.add_point(10.0, 0.0, true);
        let (bx0, by0) = sys.add_point(0.0, 5.0, false);
        let (bx1, by1) = sys.add_point(2.0, 8.0, false);
        sys.add_equation(Equation::Parallel {
            ax0,
            ay0,
            ax1,
            ay1,
            bx0,
            by0,
            bx1,
            by1,
            weight: DEFAULT_WEIGHT,
        });
        let report = solve(&mut sys);
        assert!(report.success);
        let adu = sys.value(ax1) - sys.value(ax0);
        let adv = sys.value(ay1) - sys.value(ay0);
        let bdu = sys.value(bx1) - sys.value(bx0);
        let bdv = sys.value(by1) - sys.value(by0);
        let cross = adu * bdv - adv * bdu;
        assert!(cross.abs() < EPS, "cross={cross}");
    }

    #[test]
    fn perpendicular_lines_align() {
        let mut sys = System::new();
        let (ax0, ay0) = sys.add_point(0.0, 0.0, true);
        let (ax1, ay1) = sys.add_point(10.0, 0.0, true);
        let (bx0, by0) = sys.add_point(0.0, 5.0, false);
        let (bx1, by1) = sys.add_point(1.0, 8.0, false);
        sys.add_equation(Equation::Perpendicular {
            ax0,
            ay0,
            ax1,
            ay1,
            bx0,
            by0,
            bx1,
            by1,
            weight: DEFAULT_WEIGHT,
        });
        let report = solve(&mut sys);
        assert!(report.success);
        let adu = sys.value(ax1) - sys.value(ax0);
        let adv = sys.value(ay1) - sys.value(ay0);
        let bdu = sys.value(bx1) - sys.value(bx0);
        let bdv = sys.value(by1) - sys.value(by0);
        let dot = adu * bdu + adv * bdv;
        assert!(dot.abs() < EPS, "dot={dot}");
    }

    #[test]
    fn compound_perpendicular_and_point_line_distance() {
        // Isolated version of the user-reported bug scenario.
        let mut sys = System::new();
        // Reference line A (horizontal).
        let (ax0, ay0) = sys.add_point(0.0, 0.0, true);
        let (ax1, ay1) = sys.add_point(100.0, 0.0, true);
        // Rect top edge (horizontal at y=40).
        let (rx0, ry0) = sys.add_point(20.0, 40.0, true);
        let (rx1, ry1) = sys.add_point(70.0, 40.0, true);
        // Line B (vertical).
        let (bx0, by0) = sys.add_point(30.0, 55.0, false);
        let (bx1, by1) = sys.add_point(30.0, 85.0, false);
        // B vertex on top.
        let (px, py) = (bx0, by0);

        sys.add_equation(Equation::Parallel {
            ax0: rx0,
            ay0: ry0,
            ax1: rx1,
            ay1: ry1,
            bx0: ax0,
            by0: ay0,
            bx1: ax1,
            by1: ay1,
            weight: DEFAULT_WEIGHT,
        });
        sys.add_equation(Equation::Perpendicular {
            ax0,
            ay0,
            ax1,
            ay1,
            bx0,
            by0,
            bx1,
            by1,
            weight: DEFAULT_WEIGHT,
        });
        sys.add_equation(Equation::PointLineDistance {
            px,
            py,
            x0: rx0,
            y0: ry0,
            x1: rx1,
            y1: ry1,
            distance: 50.0,
            side: 1.0,
            weight: DEFAULT_WEIGHT,
        });

        let report = solve(&mut sys);
        assert!(report.success, "residual={}", report.residual_norm);

        let adu = sys.value(ax1) - sys.value(ax0);
        let adv = sys.value(ay1) - sys.value(ay0);
        let bdu = sys.value(bx1) - sys.value(bx0);
        let bdv = sys.value(by1) - sys.value(by0);
        let dot = adu * bdu + adv * bdv;
        assert!(dot.abs() < EPS, "perpendicular broken: dot={dot}");

        // Drag B end while pinning it.
        sys.add_equation(Equation::Pin {
            var: bx1,
            target: 45.0,
            weight: PIN_WEIGHT,
        });
        sys.add_equation(Equation::Pin {
            var: by1,
            target: 100.0,
            weight: PIN_WEIGHT,
        });
        let drag_report = solve(&mut sys);
        assert!(
            drag_report.residual_norm < 1e-3,
            "drag residual={}",
            drag_report.residual_norm
        );
        assert!((sys.value(bx1) - 45.0).abs() < 1e-3);
        assert!((sys.value(by1) - 100.0).abs() < 1e-3);
        let bdu = sys.value(bx1) - sys.value(bx0);
        let bdv = sys.value(by1) - sys.value(by0);
        let dot = adu * bdu + adv * bdv;
        assert!(dot.abs() < 1e-2, "perpendicular broken after drag: dot={dot}");
    }

    #[test]
    fn fixed_variable_honored() {
        let mut sys = System::new();
        let (x0, y0) = sys.add_point(3.0, 4.0, true);
        let (x1, y1) = sys.add_point(0.0, 0.0, false);
        sys.add_equation(Equation::LineLength {
            x0,
            y0,
            x1,
            y1,
            length: 5.0,
            weight: DEFAULT_WEIGHT,
        });
        let _ = solve(&mut sys);
        assert!((sys.value(x0) - 3.0).abs() < EPS);
        assert!((sys.value(y0) - 4.0).abs() < EPS);
    }

    #[test]
    fn solve_is_deterministic_for_same_sketch() {
        let mut sys = System::new();
        let (x0, y0) = sys.add_point(0.0, 0.0, true);
        let (x1, y1) = sys.add_point(1.0, 99.0, false);
        sys.add_equation(Equation::Horizontal {
            y0,
            y1,
            weight: DEFAULT_WEIGHT,
        });
        sys.add_equation(Equation::LineLength {
            x0,
            y0,
            x1,
            y1,
            length: 10.0,
            weight: DEFAULT_WEIGHT,
        });

        let mut reference = sys.clone();
        solve(&mut reference);
        let ref_x1 = reference.value(x1) as f32;
        let ref_y1 = reference.value(y1) as f32;

        for _ in 0..100 {
            let mut trial = sys.clone();
            solve(&mut trial);
            assert!((trial.value(x1) as f32).to_bits() == ref_x1.to_bits());
            assert!((trial.value(y1) as f32).to_bits() == ref_y1.to_bits());
        }
    }

    #[test]
    fn lm_converges_from_poor_initial() {
        let mut sys = System::new();
        let (x0, y0) = sys.add_point(0.0, 0.0, true);
        let (x1, y1) = sys.add_point(1.0, 99.0, false);
        sys.add_equation(Equation::Horizontal {
            y0,
            y1,
            weight: DEFAULT_WEIGHT,
        });
        sys.add_equation(Equation::LineLength {
            x0,
            y0,
            x1,
            y1,
            length: 10.0,
            weight: DEFAULT_WEIGHT,
        });
        let report = solve(&mut sys);
        assert!(report.success, "residual={}", report.residual_norm);
        assert!((sys.value(y0) - sys.value(y1)).abs() < EPS);
        assert!((sys.value(x1) - 10.0).abs() < 1e-3);
    }
}