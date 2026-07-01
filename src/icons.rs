//! Toolbar and pane icons rasterized from bundled SVG assets.

use crate::geometric_constraints::GeometricConstraintType;
use crate::model::ConstraintKind;
use eframe::egui::{
    self, Color32, ColorImage, Context, Id, Painter, Rect, TextureHandle, TextureOptions, Ui,
    WidgetText,
};
use std::collections::HashMap;

pub const ICON_DISPLAY_SIZE: f32 = 18.0;
const ICON_RASTER_SIZE: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconId {
    Select,
    Rectangle,
    Line,
    Circle,
    Dimension,
    Constraint,
    Plane,
    Parallel,
    Perpendicular,
    Equal,
    Coincident,
    Midpoint,
    Vertical,
    Horizontal,
    Home,
    Perspective,
    Orthographic,
    Sketch,
    Extrude,
    Body,
    Plus,
    Showing,
    Hidden,
    Chamfer,
    Fillet,
    Gear,
    ShadingWireframe,
    ShadingTransparentSolid,
    ShadingSolid,
    ShadingSolidWireframe,
    ShadingRealistic,
    ViewList,
    ViewTree,
    ViewGraph,
    /// Extrude body-mode picker icons (#35).
    NewBody,
    AddToBody,
    CutBody,
}

impl IconId {
    #[cfg(test)]
    pub const ALL: [Self; 37] = [
        Self::Select,
        Self::Rectangle,
        Self::Line,
        Self::Circle,
        Self::Dimension,
        Self::Constraint,
        Self::Plane,
        Self::Parallel,
        Self::Perpendicular,
        Self::Equal,
        Self::Coincident,
        Self::Midpoint,
        Self::Vertical,
        Self::Horizontal,
        Self::Home,
        Self::Perspective,
        Self::Orthographic,
        Self::Sketch,
        Self::Extrude,
        Self::Body,
        Self::Plus,
        Self::Showing,
        Self::Hidden,
        Self::Chamfer,
        Self::Fillet,
        Self::Gear,
        Self::ShadingWireframe,
        Self::ShadingTransparentSolid,
        Self::ShadingSolid,
        Self::ShadingSolidWireframe,
        Self::ShadingRealistic,
        Self::ViewList,
        Self::ViewTree,
        Self::ViewGraph,
        Self::NewBody,
        Self::AddToBody,
        Self::CutBody,
    ];

    pub fn svg_source(self) -> &'static str {
        match self {
            Self::Select => include_str!("assets/icons/select.svg"),
            Self::Rectangle => include_str!("assets/icons/rectangle.svg"),
            Self::Line => include_str!("assets/icons/line.svg"),
            Self::Circle => include_str!("assets/icons/circle.svg"),
            Self::Dimension => include_str!("assets/icons/dimension.svg"),
            Self::Constraint => include_str!("assets/icons/constraint.svg"),
            Self::Plane => include_str!("assets/icons/plane.svg"),
            Self::Parallel => include_str!("assets/icons/parallel.svg"),
            Self::Perpendicular => include_str!("assets/icons/perpendicular.svg"),
            Self::Equal => include_str!("assets/icons/equal.svg"),
            Self::Coincident => include_str!("assets/icons/coincident.svg"),
            Self::Midpoint => include_str!("assets/icons/midpoint.svg"),
            Self::Vertical => include_str!("assets/icons/vertical.svg"),
            Self::Horizontal => include_str!("assets/icons/horizontal.svg"),
            Self::Home => include_str!("assets/icons/home.svg"),
            Self::Perspective => include_str!("assets/icons/perspective.svg"),
            Self::Orthographic => include_str!("assets/icons/orthographic.svg"),
            Self::Sketch => include_str!("assets/icons/sketch.svg"),
            Self::Extrude => include_str!("assets/icons/extrude.svg"),
            Self::Body => include_str!("assets/icons/body.svg"),
            Self::Plus => include_str!("assets/icons/plus.svg"),
            Self::Showing => include_str!("assets/icons/showing.svg"),
            Self::Hidden => include_str!("assets/icons/hidden.svg"),
            Self::Chamfer => include_str!("assets/icons/chamfer.svg"),
            Self::Fillet => include_str!("assets/icons/fillet.svg"),
            Self::Gear => include_str!("assets/icons/gear.svg"),
            Self::ShadingWireframe => include_str!("assets/icons/wireframe.svg"),
            Self::ShadingTransparentSolid => include_str!("assets/icons/transparent_solid.svg"),
            Self::ShadingSolid => include_str!("assets/icons/solid.svg"),
            Self::ShadingSolidWireframe => include_str!("assets/icons/solid_wireframe.svg"),
            Self::ShadingRealistic => include_str!("assets/icons/realistic.svg"),
            Self::ViewList => include_str!("assets/icons/view_list.svg"),
            Self::ViewTree => include_str!("assets/icons/view_tree.svg"),
            Self::ViewGraph => include_str!("assets/icons/view_graph.svg"),
            Self::NewBody => include_str!("assets/icons/new_body.svg"),
            Self::AddToBody => include_str!("assets/icons/add_to_body.svg"),
            Self::CutBody => include_str!("assets/icons/cut_body.svg"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Rectangle => "Rectangle",
            Self::Line => "Line",
            Self::Circle => "Circle",
            Self::Dimension => "Dimension",
            Self::Constraint => "Constraint",
            Self::Plane => "Plane",
            Self::Parallel => "Parallel",
            Self::Perpendicular => "Perpendicular",
            Self::Equal => "Equal",
            Self::Coincident => "Coincident",
            Self::Midpoint => "Midpoint",
            Self::Vertical => "Vertical",
            Self::Horizontal => "Horizontal",
            Self::Home => "Home",
            Self::Perspective => "Perspective",
            Self::Orthographic => "Orthographic",
            Self::Sketch => "Sketch",
            Self::Extrude => "Extrude",
            Self::Body => "Body",
            Self::Plus => "Plus",
            Self::Showing => "Showing",
            Self::Hidden => "Hidden",
            Self::Chamfer => "Chamfer",
            Self::Fillet => "Fillet",
            Self::Gear => "Gear",
            Self::ShadingWireframe => "Wireframe",
            Self::ShadingTransparentSolid => "Transparent solid",
            Self::ShadingSolid => "Solid",
            Self::ShadingSolidWireframe => "Solid + wireframe",
            Self::ShadingRealistic => "Realistic",
            Self::ViewList => "List view",
            Self::ViewTree => "Tree view",
            Self::ViewGraph => "Graph view",
            Self::NewBody => "New body",
            Self::AddToBody => "Add to body",
            Self::CutBody => "Cut body",
        }
    }
}

pub fn icon_for_visibility(visible: bool) -> IconId {
    if visible {
        IconId::Showing
    } else {
        IconId::Hidden
    }
}

pub fn icon_for_projection_mode(mode: crate::camera::ProjectionMode) -> IconId {
    match mode {
        crate::camera::ProjectionMode::Natural => IconId::Perspective,
        crate::camera::ProjectionMode::Orthographic => IconId::Orthographic,
    }
}

pub fn icon_for_shading_mode(mode: crate::camera::ShadingMode) -> IconId {
    match mode {
        crate::camera::ShadingMode::Wireframe => IconId::ShadingWireframe,
        crate::camera::ShadingMode::TransparentSolid => IconId::ShadingTransparentSolid,
        crate::camera::ShadingMode::Solid => IconId::ShadingSolid,
        crate::camera::ShadingMode::SolidWireframe => IconId::ShadingSolidWireframe,
        crate::camera::ShadingMode::Realistic => IconId::ShadingRealistic,
    }
}

pub fn icon_for_constraint(kind: GeometricConstraintType) -> IconId {
    match kind {
        GeometricConstraintType::Parallel => IconId::Parallel,
        GeometricConstraintType::Perpendicular => IconId::Perpendicular,
        GeometricConstraintType::Equal => IconId::Equal,
        GeometricConstraintType::Coincident => IconId::Coincident,
        GeometricConstraintType::Midpoint => IconId::Midpoint,
        GeometricConstraintType::Vertical => IconId::Vertical,
        GeometricConstraintType::Horizontal => IconId::Horizontal,
    }
}

pub fn icon_for_constraint_kind(kind: &ConstraintKind) -> IconId {
    match kind {
        ConstraintKind::Distance { .. } => IconId::Dimension,
        ConstraintKind::Parallel { .. } => IconId::Parallel,
        ConstraintKind::Perpendicular { .. } => IconId::Perpendicular,
        ConstraintKind::Equal { .. } => IconId::Equal,
        ConstraintKind::Coincident { .. } => IconId::Coincident,
        ConstraintKind::Midpoint { .. } => IconId::Midpoint,
        ConstraintKind::Horizontal { .. } => IconId::Horizontal,
        ConstraintKind::Vertical { .. } => IconId::Vertical,
        ConstraintKind::Angle { .. } => IconId::Constraint,
    }
}

fn rasterize_svg(svg: &str, size: u32) -> ColorImage {
    let svg = svg.replace("currentColor", "#ffffff");
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).expect("valid svg");
    let mut pixmap =
        tiny_skia::Pixmap::new(size, size).expect("pixmap allocation should succeed");
    pixmap.fill(tiny_skia::Color::TRANSPARENT);

    let svg_size = tree.size();
    let scale = (size as f32 / svg_size.width()).min(size as f32 / svg_size.height());
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let pixels = pixmap
        .pixels()
        .iter()
        .map(|pixel| {
            Color32::from_rgba_unmultiplied(pixel.red(), pixel.green(), pixel.blue(), pixel.alpha())
        })
        .collect();

    ColorImage {
        size: [size as usize, size as usize],
        pixels,
        ..Default::default()
    }
}

fn texture_for_icon(ctx: &Context, id: IconId) -> egui::TextureId {
    let cache_id = Id::new("icon_textures");
    let mut cache = ctx
        .data(|d| d.get_temp::<HashMap<IconId, TextureHandle>>(cache_id))
        .unwrap_or_default();

    if let Some(handle) = cache.get(&id) {
        return handle.id();
    }

    let image = rasterize_svg(id.svg_source(), ICON_RASTER_SIZE);
    let handle = ctx.load_texture(
        format!("icon_{}", id.label()),
        image,
        TextureOptions::LINEAR,
    );
    let texture_id = handle.id();
    cache.insert(id, handle);
    ctx.data_mut(|d| d.insert_temp(cache_id, cache));
    texture_id
}

pub fn sized_texture(ctx: &Context, id: IconId) -> egui::load::SizedTexture {
    egui::load::SizedTexture::new(
        texture_for_icon(ctx, id),
        egui::vec2(ICON_DISPLAY_SIZE, ICON_DISPLAY_SIZE),
    )
}

pub fn paint_icon(painter: &Painter, ctx: &Context, id: IconId, rect: Rect, tint: Color32) {
    let texture_id = texture_for_icon(ctx, id);
    painter.image(
        texture_id,
        rect,
        Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        tint,
    );
}

pub fn selectable_icon_button(
    ui: &mut Ui,
    id: IconId,
    selected: bool,
    tooltip: impl Into<WidgetText>,
) -> egui::Response {
    let response = ui.add(
        egui::ImageButton::new(sized_texture(ui.ctx(), id))
            .frame(true)
            .selected(selected),
    );
    response.on_hover_text(tooltip)
}

pub fn icon_button(ui: &mut Ui, id: IconId, tooltip: impl Into<WidgetText>) -> egui::Response {
    ui.add(
        egui::ImageButton::new(sized_texture(ui.ctx(), id)).frame(false),
    )
    .on_hover_text(tooltip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_icons_rasterize_with_visible_pixels() {
        for id in IconId::ALL {
            let image = rasterize_svg(id.svg_source(), ICON_RASTER_SIZE);
            assert_eq!(image.size, [ICON_RASTER_SIZE as usize, ICON_RASTER_SIZE as usize]);
            assert!(
                image.pixels.iter().any(|pixel| pixel.a() > 0),
                "{} should rasterize visible pixels",
                id.label()
            );
        }
    }

    #[test]
    fn hud_icons_map_to_projection_modes() {
        use crate::camera::ProjectionMode;

        assert_eq!(
            icon_for_projection_mode(ProjectionMode::Natural),
            IconId::Perspective
        );
        assert_eq!(
            icon_for_projection_mode(ProjectionMode::Orthographic),
            IconId::Orthographic
        );
    }

    #[test]
    fn hud_icons_map_to_shading_modes() {
        use crate::camera::ShadingMode;

        assert_eq!(
            icon_for_shading_mode(ShadingMode::Wireframe),
            IconId::ShadingWireframe
        );
        assert_eq!(
            icon_for_shading_mode(ShadingMode::TransparentSolid),
            IconId::ShadingTransparentSolid
        );
        assert_eq!(
            icon_for_shading_mode(ShadingMode::Solid),
            IconId::ShadingSolid
        );
        assert_eq!(
            icon_for_shading_mode(ShadingMode::SolidWireframe),
            IconId::ShadingSolidWireframe
        );
        assert_eq!(
            icon_for_shading_mode(ShadingMode::Realistic),
            IconId::ShadingRealistic
        );
    }

    #[test]
    fn visibility_icons_reflect_state() {
        assert_eq!(icon_for_visibility(true), IconId::Showing);
        assert_eq!(icon_for_visibility(false), IconId::Hidden);
    }

    #[test]
    fn constraint_icons_map_to_expected_assets() {
        assert_eq!(
            icon_for_constraint(GeometricConstraintType::Parallel),
            IconId::Parallel
        );
        assert_eq!(
            icon_for_constraint(GeometricConstraintType::Perpendicular),
            IconId::Perpendicular
        );
        assert_eq!(
            icon_for_constraint(GeometricConstraintType::Coincident),
            IconId::Coincident
        );
        assert_eq!(
            icon_for_constraint(GeometricConstraintType::Midpoint),
            IconId::Midpoint
        );
        assert_eq!(
            icon_for_constraint(GeometricConstraintType::Vertical),
            IconId::Vertical
        );
        assert_eq!(
            icon_for_constraint(GeometricConstraintType::Horizontal),
            IconId::Horizontal
        );
    }

    #[test]
    fn stored_constraint_kinds_map_to_expected_icons() {
        use crate::model::{
            ConstraintEntity, ConstraintLine, ConstraintPoint, DistanceTarget, LineEnd,
        };

        assert_eq!(
            icon_for_constraint_kind(&ConstraintKind::Distance {
                target: DistanceTarget::LineLength(0),
            }),
            IconId::Dimension
        );
        assert_eq!(
            icon_for_constraint_kind(&ConstraintKind::Parallel {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
            }),
            IconId::Parallel
        );
        assert_eq!(
            icon_for_constraint_kind(&ConstraintKind::Angle {
                line_a: ConstraintLine::Line(0),
                line_b: ConstraintLine::Line(1),
                rotation_sign: 1,
            }),
            IconId::Constraint
        );
        assert_eq!(
            icon_for_constraint_kind(&ConstraintKind::Coincident {
                a: ConstraintEntity::Point(ConstraintPoint::LineEndpoint {
                    line: 0,
                    end: LineEnd::Start,
                }),
                b: ConstraintEntity::Point(ConstraintPoint::LineEndpoint {
                    line: 1,
                    end: LineEnd::End,
                }),
            }),
            IconId::Coincident
        );
    }
}