//! Toolbar and pane icons rasterized from bundled SVG assets.

use crate::geometric_constraints::GeometricConstraintType;
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
    Coincident,
    Midpoint,
    Vertical,
    Horizontal,
    Home,
    Perspective,
    Orthographic,
    Sketch,
    Plus,
    Showing,
    Hidden,
}

impl IconId {
    #[cfg(test)]
    pub const ALL: [Self; 20] = [
        Self::Select,
        Self::Rectangle,
        Self::Line,
        Self::Circle,
        Self::Dimension,
        Self::Constraint,
        Self::Plane,
        Self::Parallel,
        Self::Perpendicular,
        Self::Coincident,
        Self::Midpoint,
        Self::Vertical,
        Self::Horizontal,
        Self::Home,
        Self::Perspective,
        Self::Orthographic,
        Self::Sketch,
        Self::Plus,
        Self::Showing,
        Self::Hidden,
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
            Self::Coincident => include_str!("assets/icons/coincident.svg"),
            Self::Midpoint => include_str!("assets/icons/midpoint.svg"),
            Self::Vertical => include_str!("assets/icons/vertical.svg"),
            Self::Horizontal => include_str!("assets/icons/horizontal.svg"),
            Self::Home => include_str!("assets/icons/home.svg"),
            Self::Perspective => include_str!("assets/icons/perspective.svg"),
            Self::Orthographic => include_str!("assets/icons/orthographic.svg"),
            Self::Sketch => include_str!("assets/icons/sketch.svg"),
            Self::Plus => include_str!("assets/icons/plus.svg"),
            Self::Showing => include_str!("assets/icons/showing.svg"),
            Self::Hidden => include_str!("assets/icons/hidden.svg"),
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
            Self::Coincident => "Coincident",
            Self::Midpoint => "Midpoint",
            Self::Vertical => "Vertical",
            Self::Horizontal => "Horizontal",
            Self::Home => "Home",
            Self::Perspective => "Perspective",
            Self::Orthographic => "Orthographic",
            Self::Sketch => "Sketch",
            Self::Plus => "Plus",
            Self::Showing => "Showing",
            Self::Hidden => "Hidden",
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

pub fn icon_for_constraint(kind: GeometricConstraintType) -> IconId {
    match kind {
        GeometricConstraintType::Parallel => IconId::Parallel,
        GeometricConstraintType::Perpendicular => IconId::Perpendicular,
        GeometricConstraintType::Coincident => IconId::Coincident,
        GeometricConstraintType::Midpoint => IconId::Midpoint,
        GeometricConstraintType::Vertical => IconId::Vertical,
        GeometricConstraintType::Horizontal => IconId::Horizontal,
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
}