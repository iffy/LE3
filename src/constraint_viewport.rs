//! Viewport decorations for geometric constraints on selected elements.

use crate::construction::point_world_position;
use crate::constraints::angle_constraint_display;
use crate::document_health::{constraint_annotation_color, DocumentHealth};
use crate::document_lifecycle::constraint_alive;
use crate::face::{local_to_world, sketch_geometry_frame};
use crate::geometric_constraints::line_uv_endpoints;
use crate::hierarchy::{selection_related_constraints, ElementVisibility, SceneElement};
use crate::icons::{icon_for_constraint_kind, paint_icon, IconId};
use crate::model::{
    ConstraintEntity, ConstraintKind, ConstraintLine, ConstraintPoint, Document, SketchId,
};
use crate::selection::SceneSelection;
use eframe::egui::{self, Color32, Context, Painter, Pos2, Rect};
use glam::Vec3;
use std::collections::HashSet;

pub const CONSTRAINT_ICON_SCREEN_SIZE: f32 = 20.0;
pub const CONSTRAINT_ICON_HIT_PADDING: f32 = 4.0;
const COINCIDENT_ICON_OFFSET_PX: f32 = 14.0;

#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintConnector {
    pub a: Vec3,
    pub b: Vec3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintIconPlacement {
    pub constraint_index: usize,
    pub world: Vec3,
    pub icon: IconId,
    /// When set, the icon is drawn offset from `world` toward this partner (coincident).
    pub offset_toward: Option<Vec3>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstraintViewportGraphic {
    pub constraint_index: usize,
    pub connectors: Vec<ConstraintConnector>,
    pub icons: Vec<ConstraintIconPlacement>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConstraintIconHit {
    pub constraint_index: usize,
    pub rect: Rect,
}

pub fn constraint_line_world_endpoints(
    doc: &Document,
    sketch: SketchId,
    line: ConstraintLine,
) -> Option<(Vec3, Vec3)> {
    let ((u0, v0), (u1, v1)) = line_uv_endpoints(doc, sketch, line).ok()?;
    let frame = sketch_geometry_frame(doc, sketch)?;
    Some((
        local_to_world(&frame, u0, v0),
        local_to_world(&frame, u1, v1),
    ))
}

fn midpoint(a: Vec3, b: Vec3) -> Vec3 {
    (a + b) * 0.5
}

fn entity_world_position(
    doc: &Document,
    entity: ConstraintEntity,
    sketch: SketchId,
) -> Option<Vec3> {
    match entity {
        ConstraintEntity::Point(point) => point_world_position(doc, point),
        ConstraintEntity::Line(line) => {
            let (a, b) = constraint_line_world_endpoints(doc, sketch, line)?;
            Some(midpoint(a, b))
        }
        ConstraintEntity::Circle(circle) => {
            point_world_position(doc, ConstraintPoint::CircleCenter(circle))
        }
        ConstraintEntity::Origin => {
            let frame = sketch_geometry_frame(doc, sketch)?;
            Some(local_to_world(&frame, 0.0, 0.0))
        }
    }
}

fn build_graphic(doc: &Document, index: usize) -> Option<ConstraintViewportGraphic> {
    if !constraint_alive(doc, index) {
        return None;
    }
    let constraint = doc.constraints.get(index)?;
    let icon = icon_for_constraint_kind(&constraint.kind);
    let sketch = constraint.sketch;

    match constraint.kind.clone() {
        ConstraintKind::Distance { .. } => None,
        ConstraintKind::Parallel { line_a, line_b }
        | ConstraintKind::Perpendicular { line_a, line_b }
        | ConstraintKind::Equal { line_a, line_b } => {
            let (a0, a1) = constraint_line_world_endpoints(doc, sketch, line_a)?;
            let (b0, b1) = constraint_line_world_endpoints(doc, sketch, line_b)?;
            let ma = midpoint(a0, a1);
            let mb = midpoint(b0, b1);
            Some(ConstraintViewportGraphic {
                constraint_index: index,
                connectors: vec![ConstraintConnector { a: ma, b: mb }],
                icons: vec![ConstraintIconPlacement {
                    constraint_index: index,
                    world: midpoint(ma, mb),
                    icon,
                    offset_toward: None,
                }],
            })
        }
        ConstraintKind::Coincident { a, b } => {
            let pa = entity_world_position(doc, a, sketch)?;
            let pb = entity_world_position(doc, b, sketch)?;
            Some(ConstraintViewportGraphic {
                constraint_index: index,
                connectors: if (pa - pb).length_squared() > 1e-6 {
                    vec![ConstraintConnector { a: pa, b: pb }]
                } else {
                    vec![]
                },
                // One icon for the one constraint, placed at the coincidence (nudged off the
                // vertex toward the other entity when they are at distinct positions).
                icons: vec![ConstraintIconPlacement {
                    constraint_index: index,
                    world: pa,
                    icon,
                    offset_toward: Some(pb),
                }],
            })
        }
        ConstraintKind::Midpoint { point, line } => {
            let pw = point_world_position(doc, point)?;
            let (la, lb) = constraint_line_world_endpoints(doc, sketch, line)?;
            let lm = midpoint(la, lb);
            Some(ConstraintViewportGraphic {
                constraint_index: index,
                connectors: vec![ConstraintConnector { a: pw, b: lm }],
                icons: vec![ConstraintIconPlacement {
                    constraint_index: index,
                    world: pw,
                    icon,
                    offset_toward: None,
                }],
            })
        }
        ConstraintKind::Horizontal { line } | ConstraintKind::Vertical { line } => {
            let (a, b) = constraint_line_world_endpoints(doc, sketch, line)?;
            Some(ConstraintViewportGraphic {
                constraint_index: index,
                connectors: vec![],
                icons: vec![ConstraintIconPlacement {
                    constraint_index: index,
                    world: midpoint(a, b),
                    icon,
                    offset_toward: None,
                }],
            })
        }
        ConstraintKind::Angle {
            line_a,
            line_b,
            rotation_sign,
        } => {
            let display = angle_constraint_display(doc, line_a, line_b, rotation_sign)?;
            Some(ConstraintViewportGraphic {
                constraint_index: index,
                connectors: vec![
                    ConstraintConnector {
                        a: display.center,
                        b: display.leg_a_root,
                    },
                    ConstraintConnector {
                        a: display.center,
                        b: display.leg_b_root,
                    },
                ],
                icons: vec![ConstraintIconPlacement {
                    constraint_index: index,
                    world: display.center,
                    icon,
                    offset_toward: None,
                }],
            })
        }
    }
}

pub fn viewport_constraints_for_selection(
    doc: &Document,
    visibility: &ElementVisibility,
    selection: &SceneSelection,
    exclude: &HashSet<usize>,
) -> Vec<ConstraintViewportGraphic> {
    let mut indices = HashSet::new();
    indices.extend(selection_related_constraints(doc, selection));
    for element in selection.iter() {
        if let SceneElement::Constraint(i) = element {
            indices.insert(i);
        }
    }
    indices
        .into_iter()
        .filter(|index| !exclude.contains(index))
        .filter(|index| {
            constraint_alive(doc, *index)
                && visibility.effective_visible(doc, SceneElement::Constraint(*index))
        })
        .filter_map(|index| build_graphic(doc, index))
        .collect()
}

fn placement_screen(
    project: &impl Fn(Vec3) -> Option<Pos2>,
    placement: &ConstraintIconPlacement,
    icon_index: usize,
) -> Option<Pos2> {
    let center = project(placement.world)?;
    let Some(partner_world) = placement.offset_toward else {
        return Some(center);
    };
    if (placement.world - partner_world).length_squared() < 1e-6 {
        let offset = if icon_index == 0 {
            egui::vec2(-COINCIDENT_ICON_OFFSET_PX, -COINCIDENT_ICON_OFFSET_PX)
        } else {
            egui::vec2(COINCIDENT_ICON_OFFSET_PX, COINCIDENT_ICON_OFFSET_PX)
        };
        return Some(center + offset);
    }
    let partner = project(partner_world)?;
    let away = center - partner;
    if away.length_sq() < 4.0 {
        let offset = if icon_index == 0 {
            egui::vec2(-COINCIDENT_ICON_OFFSET_PX, 0.0)
        } else {
            egui::vec2(COINCIDENT_ICON_OFFSET_PX, 0.0)
        };
        return Some(center + offset);
    }
    Some(center + away.normalized() * COINCIDENT_ICON_OFFSET_PX)
}

pub fn build_constraint_icon_hits(
    project: &impl Fn(Vec3) -> Option<Pos2>,
    graphics: &[ConstraintViewportGraphic],
) -> Vec<ConstraintIconHit> {
    let half = CONSTRAINT_ICON_SCREEN_SIZE * 0.5 + CONSTRAINT_ICON_HIT_PADDING;
    let mut hits = Vec::new();
    for graphic in graphics {
        for (icon_index, placement) in graphic.icons.iter().enumerate() {
            let Some(screen) = placement_screen(project, placement, icon_index) else {
                continue;
            };
            hits.push(ConstraintIconHit {
                constraint_index: placement.constraint_index,
                rect: Rect::from_center_size(screen, egui::vec2(half * 2.0, half * 2.0)),
            });
        }
    }
    hits
}

pub fn pointer_over_constraint_icon(hits: &[ConstraintIconHit], pointer: Pos2) -> Option<usize> {
    hits.iter()
        .rev()
        .find(|hit| hit.rect.contains(pointer))
        .map(|hit| hit.constraint_index)
}

pub fn draw_constraint_connectors(
    painter: &Painter,
    project: &impl Fn(Vec3) -> Option<Pos2>,
    health: &DocumentHealth,
    selection: &SceneSelection,
    graphics: &[ConstraintViewportGraphic],
    base_color: Color32,
) {
    use crate::construction::{CONSTRUCTION_DASH_GAP_PX, CONSTRUCTION_DASH_LENGTH_PX};
    for graphic in graphics {
        let color = constraint_annotation_color(health, graphic.constraint_index, base_color);
        let selected =
            selection.is_selected(SceneElement::Constraint(graphic.constraint_index));
        let width = if selected { 2.5 } else { 1.5 };
        for connector in &graphic.connectors {
            let Some(pa) = project(connector.a) else {
                continue;
            };
            let Some(pb) = project(connector.b) else {
                continue;
            };
            painter.add(egui::Shape::dashed_line(
                &[pa, pb],
                egui::Stroke::new(width, color),
                CONSTRUCTION_DASH_LENGTH_PX,
                CONSTRUCTION_DASH_GAP_PX,
            ));
        }
    }
}

pub fn draw_constraint_icons(
    painter: &Painter,
    ctx: &Context,
    project: &impl Fn(Vec3) -> Option<Pos2>,
    health: &DocumentHealth,
    selection: &SceneSelection,
    graphics: &[ConstraintViewportGraphic],
    hovered_index: Option<usize>,
    base_color: Color32,
    selected_color: Color32,
) {
    for graphic in graphics {
        let selected =
            selection.is_selected(SceneElement::Constraint(graphic.constraint_index));
        let hovered = hovered_index == Some(graphic.constraint_index);
        let tint = constraint_annotation_color(
            health,
            graphic.constraint_index,
            if selected || hovered {
                selected_color
            } else {
                base_color
            },
        );
        for (icon_index, placement) in graphic.icons.iter().enumerate() {
            let Some(screen) = placement_screen(project, placement, icon_index) else {
                continue;
            };
            let size = CONSTRAINT_ICON_SCREEN_SIZE;
            let rect = Rect::from_center_size(screen, egui::vec2(size, size));
            if selected || hovered {
                painter.rect_filled(
                    rect.expand(2.0),
                    4.0,
                    Color32::from_rgba_premultiplied(tint.r(), tint.g(), tint.b(), 40),
                );
            }
            paint_icon(painter, ctx, placement.icon, rect, tint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Constraint, ConstraintEntity, ConstraintLine, ConstraintPoint, FaceId, Line, ShapeKind,
    };

    fn doc_with_parallel_lines() -> (Document, usize) {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 5.0, 10.0, 5.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Parallel {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        doc.shape_order.push(ShapeKind::Constraint);
        (doc, 0)
    }

    #[test]
    fn parallel_constraint_places_icon_between_line_midpoints() {
        let (doc, index) = doc_with_parallel_lines();
        let graphic = build_graphic(&doc, index).unwrap();
        assert_eq!(graphic.connectors.len(), 1);
        assert_eq!(graphic.icons.len(), 1);
        assert_eq!(graphic.icons[0].icon, IconId::Parallel);
    }

    #[test]
    fn coincident_constraint_places_single_icon() {
        let mut doc = Document::default();
        let sketch = doc.add_sketch(FaceId::ConstructionPlane(0));
        doc.lines.push(Line::from_local_endpoints(sketch, 0.0, 0.0, 10.0, 0.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.lines.push(Line::from_local_endpoints(sketch, 10.0, 0.0, 10.0, 5.0));
        doc.shape_order.push(ShapeKind::Line);
        doc.constraints.push(Constraint {
            sketch,
            kind: ConstraintKind::Coincident {
                a: ConstraintEntity::Point(ConstraintPoint::LineEndpoint {
                    line: 0,
                    end: crate::model::LineEnd::End,
                }),
                b: ConstraintEntity::Point(ConstraintPoint::LineEndpoint {
                    line: 1,
                    end: crate::model::LineEnd::Start,
                }),
            },
            expression: String::new(),
            dim_offset: None,
            name: None,
            deleted: false,
        });
        let graphic = build_graphic(&doc, 0).unwrap();
        // One constraint → one icon (not one per endpoint).
        assert_eq!(graphic.icons.len(), 1);
        assert!(graphic.icons[0].offset_toward.is_some());
    }

    #[test]
    fn viewport_constraints_follow_selection() {
        let (doc, _) = doc_with_parallel_lines();
        let visibility = ElementVisibility::default();
        let mut selection = SceneSelection::default();
        crate::selection::click_scene_selection(
            &mut selection,
            SceneElement::Line(0),
            false,
        );
        let graphics =
            viewport_constraints_for_selection(&doc, &visibility, &selection, &HashSet::new());
        assert_eq!(graphics.len(), 1);
    }

    #[test]
    fn constraint_icon_hit_detects_pointer_over_rect() {
        let hits = vec![ConstraintIconHit {
            constraint_index: 3,
            rect: Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(30.0, 30.0)),
        }];
        assert_eq!(
            pointer_over_constraint_icon(&hits, Pos2::new(20.0, 20.0)),
            Some(3)
        );
        assert_eq!(pointer_over_constraint_icon(&hits, Pos2::new(0.0, 0.0)), None);
    }
}