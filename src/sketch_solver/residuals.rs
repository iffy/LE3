//! Constraint residuals and analytic Jacobian rows.

use super::system::{System, VarId};

/// One scalar equation that should be driven to zero.
#[derive(Clone, Debug)]
pub enum Equation {
    CoincidentU {
        a: VarId,
        b: VarId,
        weight: f64,
    },
    CoincidentV {
        a: VarId,
        b: VarId,
        weight: f64,
    },
    Horizontal {
        y0: VarId,
        y1: VarId,
        weight: f64,
    },
    Vertical {
        x0: VarId,
        x1: VarId,
        weight: f64,
    },
    LineLength {
        x0: VarId,
        y0: VarId,
        x1: VarId,
        y1: VarId,
        length: f64,
        weight: f64,
    },
    Parallel {
        ax0: VarId,
        ay0: VarId,
        ax1: VarId,
        ay1: VarId,
        bx0: VarId,
        by0: VarId,
        bx1: VarId,
        by1: VarId,
        weight: f64,
    },
    Perpendicular {
        ax0: VarId,
        ay0: VarId,
        ax1: VarId,
        ay1: VarId,
        bx0: VarId,
        by0: VarId,
        bx1: VarId,
        by1: VarId,
        weight: f64,
    },
    PointLineDistance {
        px: VarId,
        py: VarId,
        x0: VarId,
        y0: VarId,
        x1: VarId,
        y1: VarId,
        distance: f64,
        side: f64,
        weight: f64,
    },
    MidpointU {
        px: VarId,
        x0: VarId,
        x1: VarId,
        weight: f64,
    },
    MidpointV {
        py: VarId,
        y0: VarId,
        y1: VarId,
        weight: f64,
    },
    Angle {
        ax0: VarId,
        ay0: VarId,
        ax1: VarId,
        ay1: VarId,
        bx0: VarId,
        by0: VarId,
        bx1: VarId,
        by1: VarId,
        angle: f64,
        weight: f64,
    },
    PointPointDistance {
        mx: VarId,
        my: VarId,
        ax: VarId,
        ay: VarId,
        distance: f64,
        weight: f64,
    },
    LineLineDistance {
        ax0: VarId,
        ay0: VarId,
        ax1: VarId,
        ay1: VarId,
        bx0: VarId,
        by0: VarId,
        bx1: VarId,
        by1: VarId,
        distance: f64,
        side: f64,
        weight: f64,
    },
    CircleDiameter {
        radius: VarId,
        diameter: f64,
        weight: f64,
    },
    /// A point lies on a circle's perimeter: `|p - center| = radius`.
    PointOnCircle {
        px: VarId,
        py: VarId,
        cx: VarId,
        cy: VarId,
        radius: VarId,
        weight: f64,
    },
    Pin {
        var: VarId,
        target: f64,
        weight: f64,
    },
}

fn wrap_angle(mut angle: f64) -> f64 {
    let pi = std::f64::consts::PI;
    while angle > pi {
        angle -= 2.0 * pi;
    }
    while angle < -pi {
        angle += 2.0 * pi;
    }
    angle
}

fn line_angle(x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    (y1 - y0).atan2(x1 - x0)
}

fn line_angle_derivs(x0: f64, y0: f64, x1: f64, y1: f64) -> (f64, f64, f64, f64) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let denom = dx * dx + dy * dy;
    if denom < 1e-24 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let inv = 1.0 / denom;
    (dy * inv, -dx * inv, -dy * inv, dx * inv)
}

fn signed_line_offset(
    px: f64,
    py: f64,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
) -> (f64, f64, f64, f64, f64, f64) {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = dx.hypot(dy);
    if len < 1e-12 {
        return (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }
    let perp_u = -dy / len;
    let perp_v = dx / len;
    let signed = (px - x0) * perp_u + (py - y0) * perp_v;
    (signed, perp_u, perp_v, dx, dy, len)
}

impl Equation {
    pub fn residual(&self, system: &System) -> f64 {
        let v = |id: VarId| system.value(id);
        let weighted = |raw: f64, weight: f64| raw * weight.sqrt();
        match self {
            Equation::CoincidentU { a, b, weight } => weighted(v(*a) - v(*b), *weight),
            Equation::CoincidentV { a, b, weight } => weighted(v(*a) - v(*b), *weight),
            Equation::Horizontal { y0, y1, weight } => weighted(v(*y1) - v(*y0), *weight),
            Equation::Vertical { x0, x1, weight } => weighted(v(*x1) - v(*x0), *weight),
            Equation::LineLength {
                x0,
                y0,
                x1,
                y1,
                length,
                weight,
            } => {
                let dx = v(*x1) - v(*x0);
                let dy = v(*y1) - v(*y0);
                weighted(dx.hypot(dy) - length, *weight)
            }
            Equation::Parallel {
                ax0,
                ay0,
                ax1,
                ay1,
                bx0,
                by0,
                bx1,
                by1,
                weight,
            } => {
                let adu = v(*ax1) - v(*ax0);
                let adv = v(*ay1) - v(*ay0);
                let bdu = v(*bx1) - v(*bx0);
                let bdv = v(*by1) - v(*by0);
                weighted(adu * bdv - adv * bdu, *weight)
            }
            Equation::Perpendicular {
                ax0,
                ay0,
                ax1,
                ay1,
                bx0,
                by0,
                bx1,
                by1,
                weight,
            } => {
                let adu = v(*ax1) - v(*ax0);
                let adv = v(*ay1) - v(*ay0);
                let bdu = v(*bx1) - v(*bx0);
                let bdv = v(*by1) - v(*by0);
                weighted(adu * bdu + adv * bdv, *weight)
            }
            Equation::PointLineDistance {
                px,
                py,
                x0,
                y0,
                x1,
                y1,
                distance,
                side,
                weight,
            } => {
                let pxv = v(*px);
                let pyv = v(*py);
                let lx0 = v(*x0);
                let ly0 = v(*y0);
                let lx1 = v(*x1);
                let ly1 = v(*y1);
                let dx = lx1 - lx0;
                let dy = ly1 - ly0;
                let len = dx.hypot(dy);
                if len < 1e-12 {
                    return weighted(0.0, *weight);
                }
                let perp_u = -dy / len;
                let perp_v = dx / len;
                let signed = (pxv - lx0) * perp_u + (pyv - ly0) * perp_v;
                weighted(signed - side * distance, *weight)
            }
            Equation::MidpointU {
                px,
                x0,
                x1,
                weight,
            } => weighted(v(*px) - (v(*x0) + v(*x1)) * 0.5, *weight),
            Equation::MidpointV {
                py,
                y0,
                y1,
                weight,
            } => weighted(v(*py) - (v(*y0) + v(*y1)) * 0.5, *weight),
            Equation::Angle {
                ax0,
                ay0,
                ax1,
                ay1,
                bx0,
                by0,
                bx1,
                by1,
                angle,
                weight,
            } => {
                let a_angle = line_angle(v(*ax0), v(*ay0), v(*ax1), v(*ay1));
                let b_angle = line_angle(v(*bx0), v(*by0), v(*bx1), v(*by1));
                weighted(wrap_angle(b_angle - a_angle - angle), *weight)
            }
            Equation::PointPointDistance {
                mx,
                my,
                ax,
                ay,
                distance,
                weight,
            } => {
                let du = v(*mx) - v(*ax);
                let dv = v(*my) - v(*ay);
                let len = du.hypot(dv);
                let residual = if len < 1e-12 {
                    -*distance
                } else {
                    len - distance
                };
                weighted(residual, *weight)
            }
            Equation::LineLineDistance {
                ax0,
                ay0,
                ax1,
                ay1,
                bx0,
                by0,
                bx1,
                by1,
                distance,
                side,
                weight,
            } => {
                let bmu = (v(*bx0) + v(*bx1)) * 0.5;
                let bmv = (v(*by0) + v(*by1)) * 0.5;
                let (signed, _, _, _, _, _) = signed_line_offset(
                    bmu,
                    bmv,
                    v(*ax0),
                    v(*ay0),
                    v(*ax1),
                    v(*ay1),
                );
                weighted(signed - side * distance, *weight)
            }
            Equation::CircleDiameter {
                radius,
                diameter,
                weight,
            } => weighted(v(*radius) * 2.0 - diameter, *weight),
            Equation::PointOnCircle {
                px,
                py,
                cx,
                cy,
                radius,
                weight,
            } => {
                let du = v(*px) - v(*cx);
                let dv = v(*py) - v(*cy);
                weighted(du.hypot(dv) - v(*radius), *weight)
            }
            Equation::Pin { var, target, weight } => weighted(v(*var) - target, *weight),
        }
    }

    pub fn jacobian_row(&self, system: &System, accum: &mut Vec<(VarId, f64)>) {
        accum.clear();
        let v = |id: VarId| system.value(id);
        let push = |accum: &mut Vec<(VarId, f64)>, var: VarId, deriv: f64, weight: f64| {
            if deriv.abs() > 0.0 {
                accum.push((var, deriv * weight.sqrt()));
            }
        };

        match self {
            Equation::CoincidentU { a, b, weight } => {
                push(accum, *a, 1.0, *weight);
                push(accum, *b, -1.0, *weight);
            }
            Equation::CoincidentV { a, b, weight } => {
                push(accum, *a, 1.0, *weight);
                push(accum, *b, -1.0, *weight);
            }
            Equation::Horizontal { y0, y1, weight } => {
                push(accum, *y1, 1.0, *weight);
                push(accum, *y0, -1.0, *weight);
            }
            Equation::Vertical { x0, x1, weight } => {
                push(accum, *x1, 1.0, *weight);
                push(accum, *x0, -1.0, *weight);
            }
            Equation::LineLength {
                x0,
                y0,
                x1,
                y1,
                length: _,
                weight,
            } => {
                let dx = v(*x1) - v(*x0);
                let dy = v(*y1) - v(*y0);
                let dist = dx.hypot(dy);
                if dist < 1e-12 {
                    push(accum, *x1, 1.0, *weight);
                    return;
                }
                let inv = 1.0 / dist;
                push(accum, *x0, -dx * inv, *weight);
                push(accum, *y0, -dy * inv, *weight);
                push(accum, *x1, dx * inv, *weight);
                push(accum, *y1, dy * inv, *weight);
            }
            Equation::Parallel {
                ax0,
                ay0,
                ax1,
                ay1,
                bx0,
                by0,
                bx1,
                by1,
                weight,
            } => {
                let adu = v(*ax1) - v(*ax0);
                let adv = v(*ay1) - v(*ay0);
                let bdu = v(*bx1) - v(*bx0);
                let bdv = v(*by1) - v(*by0);
                push(accum, *ax0, -bdv, *weight);
                push(accum, *ax1, bdv, *weight);
                push(accum, *ay0, bdu, *weight);
                push(accum, *ay1, -bdu, *weight);
                push(accum, *bx0, adv, *weight);
                push(accum, *bx1, -adv, *weight);
                push(accum, *by0, -adu, *weight);
                push(accum, *by1, adu, *weight);
            }
            Equation::Perpendicular {
                ax0,
                ay0,
                ax1,
                ay1,
                bx0,
                by0,
                bx1,
                by1,
                weight,
            } => {
                let adu = v(*ax1) - v(*ax0);
                let adv = v(*ay1) - v(*ay0);
                let bdu = v(*bx1) - v(*bx0);
                let bdv = v(*by1) - v(*by0);
                push(accum, *ax0, -bdu, *weight);
                push(accum, *ax1, bdu, *weight);
                push(accum, *ay0, -bdv, *weight);
                push(accum, *ay1, bdv, *weight);
                push(accum, *bx0, -adu, *weight);
                push(accum, *bx1, adu, *weight);
                push(accum, *by0, -adv, *weight);
                push(accum, *by1, adv, *weight);
            }
            Equation::PointLineDistance {
                px,
                py,
                x0,
                y0,
                x1,
                y1,
                distance: _,
                side: _,
                weight,
            } => {
                let pxv = v(*px);
                let pyv = v(*py);
                let lx0 = v(*x0);
                let ly0 = v(*y0);
                let lx1 = v(*x1);
                let ly1 = v(*y1);
                let dx = lx1 - lx0;
                let dy = ly1 - ly0;
                let len = dx.hypot(dy);
                if len < 1e-12 {
                    return;
                }
                let perp_u = -dy / len;
                let perp_v = dx / len;
                push(accum, *px, perp_u, *weight);
                push(accum, *py, perp_v, *weight);
                let dperp_u_dx = dy * dy / (len * len * len);
                let dperp_u_dy = -dx * dy / (len * len * len);
                let dperp_v_dx = -dx * dx / (len * len * len);
                let dperp_v_dy = dx * dy / (len * len * len);
                let signed_dx0 =
                    -perp_u + (pxv - lx0) * dperp_u_dx + (pyv - ly0) * dperp_v_dx;
                let signed_dy0 =
                    -perp_v + (pxv - lx0) * dperp_u_dy + (pyv - ly0) * dperp_v_dy;
                let signed_dx1 = (pxv - lx0) * (-dperp_u_dx) + (pyv - ly0) * (-dperp_v_dx);
                let signed_dy1 = (pxv - lx0) * (-dperp_u_dy) + (pyv - ly0) * (-dperp_v_dy);
                push(accum, *x0, signed_dx0, *weight);
                push(accum, *y0, signed_dy0, *weight);
                push(accum, *x1, signed_dx1, *weight);
                push(accum, *y1, signed_dy1, *weight);
            }
            Equation::MidpointU { px, x0, x1, weight } => {
                push(accum, *px, 1.0, *weight);
                push(accum, *x0, -0.5, *weight);
                push(accum, *x1, -0.5, *weight);
            }
            Equation::MidpointV { py, y0, y1, weight } => {
                push(accum, *py, 1.0, *weight);
                push(accum, *y0, -0.5, *weight);
                push(accum, *y1, -0.5, *weight);
            }
            Equation::Angle {
                ax0,
                ay0,
                ax1,
                ay1,
                bx0,
                by0,
                bx1,
                by1,
                angle: _,
                weight,
            } => {
                let (adx0, ady0, adx1, ady1) =
                    line_angle_derivs(v(*ax0), v(*ay0), v(*ax1), v(*ay1));
                let (bdx0, bdy0, bdx1, bdy1) =
                    line_angle_derivs(v(*bx0), v(*by0), v(*bx1), v(*by1));
                push(accum, *ax0, -adx0, *weight);
                push(accum, *ay0, -ady0, *weight);
                push(accum, *ax1, -adx1, *weight);
                push(accum, *ay1, -ady1, *weight);
                push(accum, *bx0, bdx0, *weight);
                push(accum, *by0, bdy0, *weight);
                push(accum, *bx1, bdx1, *weight);
                push(accum, *by1, bdy1, *weight);
            }
            Equation::PointPointDistance {
                mx,
                my,
                ax,
                ay,
                distance: _,
                weight,
            } => {
                let du = v(*mx) - v(*ax);
                let dv = v(*my) - v(*ay);
                let len = du.hypot(dv);
                let (dmx, dmy) = if len < 1e-12 {
                    (1.0, 0.0)
                } else {
                    (du / len, dv / len)
                };
                push(accum, *mx, dmx, *weight);
                push(accum, *my, dmy, *weight);
                push(accum, *ax, -dmx, *weight);
                push(accum, *ay, -dmy, *weight);
            }
            Equation::LineLineDistance {
                ax0,
                ay0,
                ax1,
                ay1,
                bx0,
                by0,
                bx1,
                by1,
                distance: _,
                side: _,
                weight,
            } => {
                let bmu = (v(*bx0) + v(*bx1)) * 0.5;
                let bmv = (v(*by0) + v(*by1)) * 0.5;
                let lx0 = v(*ax0);
                let ly0 = v(*ay0);
                let lx1 = v(*ax1);
                let ly1 = v(*ay1);
                let dx = lx1 - lx0;
                let dy = ly1 - ly0;
                let len = dx.hypot(dy);
                if len < 1e-12 {
                    return;
                }
                let perp_u = -dy / len;
                let perp_v = dx / len;
                let dperp_u_dx = dy * dy / (len * len * len);
                let dperp_u_dy = -dx * dy / (len * len * len);
                let dperp_v_dx = -dx * dx / (len * len * len);
                let dperp_v_dy = dx * dy / (len * len * len);
                let signed_dx0 =
                    -perp_u + (bmu - lx0) * dperp_u_dx + (bmv - ly0) * dperp_v_dx;
                let signed_dy0 =
                    -perp_v + (bmu - lx0) * dperp_u_dy + (bmv - ly0) * dperp_v_dy;
                let signed_dx1 = (bmu - lx0) * (-dperp_u_dx) + (bmv - ly0) * (-dperp_v_dx);
                let signed_dy1 = (bmu - lx0) * (-dperp_u_dy) + (bmv - ly0) * (-dperp_v_dy);
                push(accum, *bx0, perp_u * 0.5, *weight);
                push(accum, *bx1, perp_u * 0.5, *weight);
                push(accum, *by0, perp_v * 0.5, *weight);
                push(accum, *by1, perp_v * 0.5, *weight);
                push(accum, *ax0, signed_dx0, *weight);
                push(accum, *ay0, signed_dy0, *weight);
                push(accum, *ax1, signed_dx1, *weight);
                push(accum, *ay1, signed_dy1, *weight);
            }
            Equation::CircleDiameter {
                radius,
                diameter: _,
                weight,
            } => {
                push(accum, *radius, 2.0, *weight);
            }
            Equation::PointOnCircle {
                px,
                py,
                cx,
                cy,
                radius,
                weight,
            } => {
                let du = v(*px) - v(*cx);
                let dv = v(*py) - v(*cy);
                let len = du.hypot(dv);
                let (nu, nv) = if len < 1e-12 {
                    (1.0, 0.0)
                } else {
                    (du / len, dv / len)
                };
                push(accum, *px, nu, *weight);
                push(accum, *py, nv, *weight);
                push(accum, *cx, -nu, *weight);
                push(accum, *cy, -nv, *weight);
                push(accum, *radius, -1.0, *weight);
            }
            Equation::Pin { var, target: _, weight } => {
                push(accum, *var, 1.0, *weight);
            }
        }
    }
}

pub const DEFAULT_WEIGHT: f64 = 1.0;
/// Strong anchor for reference geometry during a drag (reference stays put while the
/// movable side is dragged). Sits between the constraint weight and the drag pin.
pub const REFERENCE_HOLD_WEIGHT: f64 = 1e4;
/// Gauge anchor applied during a non-drag full solve to keep otherwise-free reference
/// geometry stable. Must be far weaker than `DEFAULT_WEIGHT` so it only breaks ties among
/// free degrees of freedom and never fights a real constraint — e.g. a parameter change
/// driving a rectangle whose corner is also a distance anchor (#53).
pub const GAUGE_HOLD_WEIGHT: f64 = 1e-6;
pub const DRAG_PIN_WEIGHT: f64 = 1e6;