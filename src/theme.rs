//! Application-wide dark theme, tuned to match the viewport drawing area.

use eframe::egui::{self, style::WidgetVisuals, Color32, Rounding, Stroke, Theme, ThemePreference};

/// Viewport background (`main::col::BG`).
pub const VIEWPORT_BG: Color32 = Color32::from_gray(28);

/// Command palette console background.
const PALETTE_CONSOLE_BG: Color32 = Color32::from_rgb(22, 24, 30);

/// Slightly lifted surface for chrome panels (toolbar, hierarchy, status).
const PANEL_BG: Color32 = Color32::from_gray(32);

/// Interactive widget fill.
const WIDGET_BG: Color32 = Color32::from_gray(40);
const WIDGET_HOVER: Color32 = Color32::from_gray(48);
const WIDGET_ACTIVE: Color32 = Color32::from_gray(56);

/// Subtle separators and widget outlines.
const BORDER: Color32 = Color32::from_gray(55);

/// Selected toolbar tools: faint blue tint behind bright label text.
const SELECTION_BG: Color32 = Color32::from_rgba_premultiplied(55, 95, 170, 48);
const SELECTION_TEXT: Color32 = Color32::WHITE;

fn set_widget_visuals(
    w: &mut WidgetVisuals,
    bg: Color32,
    border: Color32,
    fg: Color32,
    rounding: f32,
    expansion: f32,
) {
    // Buttons paint `weak_bg_fill`, not `bg_fill`.
    w.weak_bg_fill = bg;
    w.bg_fill = bg;
    w.bg_stroke = Stroke::new(1.0, border);
    w.fg_stroke = Stroke::new(1.0, fg);
    w.rounding = Rounding::same(rounding);
    w.expansion = expansion;
}

/// Build the dark [`egui::Visuals`] used across the app.
pub fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL_BG;
    v.window_fill = VIEWPORT_BG;
    v.extreme_bg_color = Color32::from_gray(18);
    v.faint_bg_color = Color32::from_gray(24);
    v.code_bg_color = Color32::from_rgb(22, 24, 30);

    set_widget_visuals(
        &mut v.widgets.noninteractive,
        PANEL_BG,
        BORDER,
        Color32::from_gray(200),
        2.0,
        0.0,
    );
    set_widget_visuals(
        &mut v.widgets.inactive,
        WIDGET_BG,
        BORDER,
        Color32::from_gray(210),
        2.0,
        0.0,
    );
    set_widget_visuals(
        &mut v.widgets.hovered,
        WIDGET_HOVER,
        Color32::from_gray(75),
        Color32::from_gray(235),
        3.0,
        1.0,
    );
    set_widget_visuals(
        &mut v.widgets.active,
        WIDGET_ACTIVE,
        Color32::from_gray(90),
        Color32::WHITE,
        2.0,
        1.0,
    );
    v.widgets.active.fg_stroke = Stroke::new(2.0, Color32::WHITE);
    set_widget_visuals(
        &mut v.widgets.open,
        WIDGET_ACTIVE,
        BORDER,
        Color32::from_gray(220),
        2.0,
        0.0,
    );

    v.selection.bg_fill = SELECTION_BG;
    v.selection.stroke = Stroke::new(1.0, SELECTION_TEXT);

    v.hyperlink_color = Color32::from_rgb(120, 170, 240);
    v.warn_fg_color = Color32::from_rgb(240, 200, 120);
    v.error_fg_color = Color32::from_rgb(220, 90, 90);

    v
}

/// Command palette console fill.
pub fn palette_console_fill() -> Color32 {
    PALETTE_CONSOLE_BG
}

/// Panel chrome frame (toolbar, status bar, side panes).
pub fn panel_frame() -> egui::Frame {
    egui::Frame {
        fill: PANEL_BG,
        stroke: Stroke::new(1.0, BORDER),
        inner_margin: egui::Margin::symmetric(8.0, 6.0),
        ..Default::default()
    }
}

/// Apply dark theme to an egui context.
pub fn apply(ctx: &egui::Context) {
    // Stay on dark visuals even when the OS prefers light mode.
    ctx.set_theme(ThemePreference::Dark);
    let v = visuals();
    ctx.set_visuals_of(Theme::Dark, v.clone());
    ctx.set_visuals_of(Theme::Light, v.clone());
    ctx.set_visuals(v);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_is_dark_and_matches_viewport() {
        let v = visuals();
        assert!(v.dark_mode);
        assert_eq!(v.window_fill, VIEWPORT_BG);
        assert_eq!(v.panel_fill, PANEL_BG);
    }

    #[test]
    fn button_fill_uses_weak_bg_fill() {
        let v = visuals();
        assert_eq!(v.widgets.inactive.weak_bg_fill, WIDGET_BG);
        assert_eq!(v.widgets.inactive.weak_bg_fill, v.widgets.inactive.bg_fill);
    }

    #[test]
    fn selected_tool_has_high_contrast() {
        let v = visuals();
        assert_eq!(v.selection.stroke.color, SELECTION_TEXT);
        assert!(
            v.selection.bg_fill.a() < 70,
            "selection background should stay faint"
        );
    }
}