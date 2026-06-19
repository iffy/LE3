//! wgpu offscreen renderer for the view-cube HUD bear.

use crate::view_cube::BearGpuMesh;
use eframe::egui_wgpu::wgpu::util::DeviceExt as _;
use eframe::egui_wgpu::{self, wgpu};
use egui::Rect;
use std::num::NonZeroU64;
use std::sync::Mutex;

/// WGSL aligns `vec2f` fields to 8 bytes; pad after the first `vec3` of scalars.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BearHudUniforms {
    center: [f32; 2],
    scale: f32,
    z_min: f32,
    z_max: f32,
    _pad0: f32,
    rect_min: [f32; 2],
    rect_size: [f32; 2],
}

pub struct BearGpuScene {
    pub mesh: BearGpuMesh,
    pub rect: Rect,
    pub center: egui::Pos2,
    pub scale: f32,
}

pub struct BearGpuResources {
    target_format: wgpu::TextureFormat,
    bear_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    color_texture: Option<wgpu::Texture>,
    color_view: Option<wgpu::TextureView>,
    depth_texture: Option<wgpu::Texture>,
    depth_view: Option<wgpu::TextureView>,
    blit_bind_group: Option<wgpu::BindGroup>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    vertex_capacity: u64,
    index_capacity: u64,
    texture_size: [u32; 2],
    pending_scene: Mutex<Option<BearGpuScene>>,
}

impl BearGpuResources {
    pub fn install(render_state: &egui_wgpu::RenderState) -> Self {
        let device = &render_state.device;
        let target_format = render_state.target_format;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("le3_bear_hud_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("le3_bear_hud_uniform_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<BearHudUniforms>() as u64),
                    },
                    count: None,
                }],
            });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("le3_bear_hud_uniform"),
            contents: bytemuck::bytes_of(&BearHudUniforms {
                center: [0.0, 0.0],
                scale: 1.0,
                z_min: 0.0,
                z_max: 1.0,
                _pad0: 0.0,
                rect_min: [0.0, 0.0],
                rect_size: [1.0, 1.0],
            }),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::UNIFORM,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("le3_bear_hud_uniform_bind_group"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let bear_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("le3_bear_hud_bear_layout"),
                bind_group_layouts: &[&uniform_bind_group_layout],
                push_constant_ranges: &[],
            });

        let bear_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("le3_bear_hud_bear_pipeline"),
            layout: Some(&bear_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_bear",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<crate::view_cube::GpuBearVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_bear",
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let blit_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("le3_bear_hud_blit_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let blit_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("le3_bear_hud_blit_layout"),
                bind_group_layouts: &[&blit_bind_group_layout],
                push_constant_ranges: &[],
            });

        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("le3_bear_hud_blit_pipeline"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_blit",
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_blit",
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("le3_bear_hud_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("le3_bear_hud_vertices"),
            size: 4096,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("le3_bear_hud_indices"),
            size: 4096,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            target_format,
            bear_pipeline,
            blit_pipeline,
            uniform_buffer,
            uniform_bind_group,
            sampler,
            color_texture: None,
            color_view: None,
            depth_texture: None,
            depth_view: None,
            blit_bind_group: None,
            vertex_buffer,
            index_buffer,
            vertex_capacity: 4096,
            index_capacity: 4096,
            texture_size: [0, 0],
            pending_scene: Mutex::new(None),
        }
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if self.texture_size == [width, height] {
            return;
        }
        self.texture_size = [width, height];

        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let color_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("le3_bear_hud_color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color_texture.create_view(&Default::default());

        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("le3_bear_hud_depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&Default::default());

        let blit_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("le3_bear_hud_blit_layout_runtime"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("le3_bear_hud_blit_bind_group"),
            layout: &blit_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        self.color_texture = Some(color_texture);
        self.color_view = Some(color_view);
        self.depth_texture = Some(depth_texture);
        self.depth_view = Some(depth_view);
        self.blit_bind_group = Some(blit_bind_group);
    }

    fn ensure_buffer_capacity(
        &mut self,
        device: &wgpu::Device,
        vertex_bytes: u64,
        index_bytes: u64,
    ) {
        if vertex_bytes > self.vertex_capacity {
            self.vertex_capacity = vertex_bytes.next_power_of_two().max(4096);
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("le3_bear_hud_vertices"),
                size: self.vertex_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if index_bytes > self.index_capacity {
            self.index_capacity = index_bytes.next_power_of_two().max(4096);
            self.index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("le3_bear_hud_indices"),
                size: self.index_capacity,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
    }

    fn render_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &BearGpuScene,
        width: u32,
        height: u32,
    ) {
        self.ensure_targets(device, width, height);
        if width == 0 || height == 0 || scene.mesh.indices.is_empty() {
            return;
        }

        let vertex_bytes =
            (scene.mesh.vertices.len() * std::mem::size_of::<crate::view_cube::GpuBearVertex>()) as u64;
        let index_bytes = (scene.mesh.indices.len() * std::mem::size_of::<u32>()) as u64;
        self.ensure_buffer_capacity(device, vertex_bytes, index_bytes);

        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&BearHudUniforms {
                center: [scene.center.x, scene.center.y],
                scale: scene.scale,
                z_min: scene.mesh.z_min,
                z_max: scene.mesh.z_max,
                _pad0: 0.0,
                rect_min: [scene.rect.min.x, scene.rect.min.y],
                rect_size: [scene.rect.width(), scene.rect.height()],
            }),
        );
        queue.write_buffer(
            &self.vertex_buffer,
            0,
            bytemuck::cast_slice(&scene.mesh.vertices),
        );
        queue.write_buffer(
            &self.index_buffer,
            0,
            bytemuck::cast_slice(&scene.mesh.indices),
        );

        let color_view = self.color_view.as_ref().expect("color view");
        let depth_view = self.depth_view.as_ref().expect("depth view");

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("le3_bear_hud_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("le3_bear_hud_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.bear_pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..scene.mesh.indices.len() as u32, 0, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

pub struct BearPaintCallback {
    rect: Rect,
}

impl egui_wgpu::CallbackTrait for BearPaintCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let resources: &mut BearGpuResources = callback_resources.get_mut().unwrap();
        let scene = resources.pending_scene.lock().unwrap().take();
        let Some(scene) = scene else {
            return Vec::new();
        };
        let width = (self.rect.width() * screen_descriptor.pixels_per_point).round() as u32;
        let height = (self.rect.height() * screen_descriptor.pixels_per_point).round() as u32;
        resources.render_scene(device, queue, &scene, width, height);
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let resources: &BearGpuResources = callback_resources.get().unwrap();
        let Some(blit_bind_group) = resources.blit_bind_group.as_ref() else {
            return;
        };
        let viewport = info.viewport_in_pixels();
        if viewport.width_px == 0 || viewport.height_px == 0 {
            return;
        }
        render_pass.set_viewport(
            viewport.left_px as f32,
            viewport.top_px as f32,
            viewport.width_px as f32,
            viewport.height_px as f32,
            0.0,
            1.0,
        );
        render_pass.set_pipeline(&resources.blit_pipeline);
        render_pass.set_bind_group(0, blit_bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::BearHudUniforms;

    #[test]
    fn bear_hud_uniforms_match_wgsl_layout() {
        assert_eq!(std::mem::size_of::<BearHudUniforms>(), 40);
    }
}

pub fn paint_bear(
    resources: &BearGpuResources,
    painter: &egui::Painter,
    rect: Rect,
    scene: BearGpuScene,
) -> bool {
    if scene.mesh.indices.is_empty() {
        return false;
    }
    *resources.pending_scene.lock().unwrap() = Some(scene);
    painter.add(egui_wgpu::Callback::new_paint_callback(
        rect,
        BearPaintCallback { rect },
    ));
    true
}