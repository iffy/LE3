//! In-memory document model.
//!
//! This is the very first slice of LE3 (see SPEC.md): a document is a flat list
//! of rectangles and lines on a single 2D sketch. As the action-DAG, components,
//! and the OCCT kernel come online this will grow, but the persistence boundary
//! (`storage.rs`) is kept narrow so the file format can evolve underneath it.

use serde::{Deserialize, Serialize};

/// An axis-aligned rectangle in sketch coordinates (millimetres, per SPEC §5.3).
///
/// Stored by its origin (`x`, `y`) and signed `w`/`h` extents. We normalise on
/// creation so width/height are always positive, which keeps hit-testing simple.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Build a normalised rectangle from two opposite corners.
    pub fn from_corners(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Rect {
            x: x0.min(x1),
            y: y0.min(y1),
            w: (x1 - x0).abs(),
            h: (y1 - y0).abs(),
        }
    }
}

/// A line segment on the ground plane (millimetres, per SPEC §5.3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Line {
    pub fn from_endpoints(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn length(&self) -> f32 {
        let dx = self.x1 - self.x0;
        let dy = self.y1 - self.y0;
        (dx * dx + dy * dy).sqrt()
    }
}

/// Which sketch primitive was created, in chronological order (for undo).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    Rect,
    Line,
}

/// The whole document: rectangles and lines on one sketch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub rects: Vec<Rect>,
    pub lines: Vec<Line>,
    pub shape_order: Vec<ShapeKind>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_length_from_endpoints() {
        let line = Line::from_endpoints(0.0, 0.0, 3.0, 4.0);
        assert!((line.length() - 5.0).abs() < 1e-4);
    }
}