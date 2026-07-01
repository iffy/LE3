//! GPU-accelerated 3D viewport rendering via wgpu paint callbacks.

mod dim_labels;
mod renderer;
mod scene;

pub use dim_labels::{build_planar_label_mesh, ViewportDimLabel};
pub use renderer::paint_viewport;
pub use scene::{
    fill_color, sketch_ground_color, solid_mesh_unique_edges, ViewportHoverHighlight,
    ViewportPalette, ViewportExtrudeGizmo, ViewportPlaneGizmo, ViewportPlanePreview,
    ViewportScene, ViewportSceneInput, VertexTreatmentPreviewGeom,
    DEFAULT_CONSTRUCTION_PLANE_OPACITY, GRID_EXTENT, GRID_STEP, SKETCH_DIMMED,
};

use eframe::CreationContext;
use eframe::egui_wgpu;

/// Returns true when GPU viewport resources were registered with egui.
pub fn install(cc: &CreationContext<'_>) -> bool {
    let Some(render_state) = cc.wgpu_render_state.as_ref() else {
        return false;
    };
    let resources = renderer::ViewportGpuResources::install(render_state);
    render_state
        .renderer
        .write()
        .callback_resources
        .insert(resources);
    true
}

/// Draw the 3D scene into the viewport via GPU when resources are available.
pub fn paint(
    render_state: Option<&egui_wgpu::RenderState>,
    painter: &egui::Painter,
    rect: egui::Rect,
    scene: ViewportScene,
) -> bool {
    let Some(render_state) = render_state else {
        return false;
    };
    let renderer = render_state.renderer.read();
    let resources = renderer
        .callback_resources
        .get::<renderer::ViewportGpuResources>();
    let Some(resources) = resources else {
        return false;
    };
    paint_viewport(resources, render_state, painter, rect, scene);
    true
}