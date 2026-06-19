//! GPU-accelerated view-cube HUD rendering.

mod bear_renderer;

pub use bear_renderer::{paint_bear, BearGpuScene};

use eframe::CreationContext;
use eframe::egui_wgpu;

/// Returns true when GPU bear resources were registered with egui.
pub fn install(cc: &CreationContext<'_>) -> bool {
    let Some(render_state) = cc.wgpu_render_state.as_ref() else {
        return false;
    };
    let resources = bear_renderer::BearGpuResources::install(render_state);
    render_state
        .renderer
        .write()
        .callback_resources
        .insert(resources);
    true
}

/// Draw the HUD bear via GPU when resources are available.
pub fn paint(
    render_state: Option<&egui_wgpu::RenderState>,
    painter: &egui::Painter,
    rect: egui::Rect,
    scene: BearGpuScene,
) -> bool {
    let Some(render_state) = render_state else {
        return false;
    };
    let renderer = render_state.renderer.read();
    let resources = renderer
        .callback_resources
        .get::<bear_renderer::BearGpuResources>();
    let Some(resources) = resources else {
        return false;
    };
    paint_bear(resources, painter, rect, scene)
}