//! wgpu offscreen renderer and egui paint callback.

use super::dim_labels::GpuTextVertex;
use super::scene::{GpuVertex, ViewportScene};
use eframe::egui_wgpu::wgpu::util::DeviceExt as _;
use eframe::egui_wgpu::{self, wgpu};
use egui::Rect;
use glam::Mat4;
use std::num::NonZeroU64;
use std::sync::Mutex;

/// Preferred MSAA sample count for viewport line/edge anti-aliasing.
pub const VIEWPORT_MSAA_SAMPLES: u32 = 4;

/// Depth-stencil format for the viewport. Needs a stencil aspect so coplanar
/// sketch fills can be masked to paint each pixel once (#3).
pub const VIEWPORT_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuUniforms {
    view_proj: [[f32; 4]; 4],
}

pub struct ViewportGpuResources {
    target_format: wgpu::TextureFormat,
    msaa_sample_count: u32,
    scene_pipeline: wgpu::RenderPipeline,
    /// Stencil-masked pipeline for coplanar sketch fills: each pixel is painted
    /// exactly once so translucent overlaps don't double-blend (#3).
    sketch_fill_pipeline: wgpu::RenderPipeline,
    scene_transparent_pipeline: wgpu::RenderPipeline,
    text_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    text_texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    font_sampler: wgpu::Sampler,
    /// Single-sample resolve target sampled by the blit pass.
    color_texture: Option<wgpu::Texture>,
    color_view: Option<wgpu::TextureView>,
    msaa_color_texture: Option<wgpu::Texture>,
    msaa_color_view: Option<wgpu::TextureView>,
    depth_texture: Option<wgpu::Texture>,
    depth_view: Option<wgpu::TextureView>,
    blit_bind_group: Option<wgpu::BindGroup>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    text_vertex_buffer: wgpu::Buffer,
    text_index_buffer: wgpu::Buffer,
    vertex_capacity: u64,
    index_capacity: u64,
    text_vertex_capacity: u64,
    text_index_capacity: u64,
    texture_size: [u32; 2],
    pending_scene: Mutex<Option<ViewportScene>>,
    font_bind_group: Mutex<Option<wgpu::BindGroup>>,
}

/// Pick the highest MSAA count supported by the device, capped at [`VIEWPORT_MSAA_SAMPLES`].
pub fn clamp_msaa_sample_count(max_supported: u32) -> u32 {
    if max_supported >= VIEWPORT_MSAA_SAMPLES {
        VIEWPORT_MSAA_SAMPLES
    } else if max_supported >= 2 {
        2
    } else {
        1
    }
}

/// Pick the MSAA sample count for a render target format, or `1` when resolve is unsupported.
pub fn msaa_sample_count_for_format(features: &wgpu::TextureFormatFeatures) -> u32 {
    if !features
        .flags
        .contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE)
    {
        return 1;
    }
    let max_supported = features
        .flags
        .supported_sample_counts()
        .into_iter()
        .max()
        .unwrap_or(1);
    clamp_msaa_sample_count(max_supported)
}

fn multisample_state(sample_count: u32) -> wgpu::MultisampleState {
    wgpu::MultisampleState {
        count: sample_count,
        mask: !0,
        // MSAA resolve still anti-aliases opaque line quads; alpha-to-coverage
        // thins semi-transparent face fills to near-invisibility on dark backgrounds.
        alpha_to_coverage_enabled: false,
    }
}

impl ViewportGpuResources {
    pub fn install(render_state: &egui_wgpu::RenderState) -> Self {
        let device = &render_state.device;
        let target_format = render_state.target_format;
        let format_features = render_state
            .adapter
            .get_texture_format_features(target_format);
        let msaa_sample_count = msaa_sample_count_for_format(&format_features);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bearcad_viewport_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("bearcad_viewport_uniform_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<GpuUniforms>() as u64),
                    },
                    count: None,
                }],
            });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bearcad_viewport_uniform"),
            contents: bytemuck::bytes_of(&GpuUniforms {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            }),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::UNIFORM,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bearcad_viewport_uniform_bind_group"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let scene_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("bearcad_viewport_scene_layout"),
                bind_group_layouts: &[&uniform_bind_group_layout],
                push_constant_ranges: &[],
            });

        let scene_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bearcad_viewport_scene_pipeline"),
            layout: Some(&scene_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
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
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
                format: VIEWPORT_DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: multisample_state(msaa_sample_count),
            multiview: None,
            cache: None,
        });

        // Coplanar sketch fills: keep the depth test, but use the stencil buffer
        // so that the first fill to cover a pixel paints it (stencil 0 -> 1) and
        // any later coplanar fill at that pixel is rejected (stencil != 0). This
        // prevents translucent overlap regions from being alpha-blended twice,
        // which previously made overlaps render darker (#3).
        let sketch_fill_stencil = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::Equal,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op: wgpu::StencilOperation::IncrementClamp,
        };
        let sketch_fill_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bearcad_viewport_sketch_fill_pipeline"),
            layout: Some(&scene_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
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
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
                format: VIEWPORT_DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState {
                    front: sketch_fill_stencil,
                    back: sketch_fill_stencil,
                    read_mask: 0xff,
                    write_mask: 0xff,
                },
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: multisample_state(msaa_sample_count),
            multiview: None,
            cache: None,
        });

        let scene_transparent_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("bearcad_viewport_scene_transparent_pipeline"),
                layout: Some(&scene_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<GpuVertex>() as u64,
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
                    entry_point: "fs_main",
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
                    format: VIEWPORT_DEPTH_FORMAT,
                    depth_write_enabled: false,
                    // Bias construction-plane fills away from the camera so a coplanar
                    // sketch face (drawn first, into the depth buffer) deterministically
                    // wins the overlap instead of z-fighting. Faces are preferred to planes.
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 64,
                        slope_scale: 2.0,
                        clamp: 0.0,
                    },
                }),
                multisample: multisample_state(msaa_sample_count),
                multiview: None,
                cache: None,
            });

        let text_texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("bearcad_viewport_text_texture_layout"),
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

        let text_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("bearcad_viewport_text_layout"),
                bind_group_layouts: &[&uniform_bind_group_layout, &text_texture_bind_group_layout],
                push_constant_ranges: &[],
            });

        let text_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bearcad_viewport_text_pipeline"),
            layout: Some(&text_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_text",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuTextVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 12,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 20,
                            shader_location: 2,
                        },
                    ],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_text",
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
                format: VIEWPORT_DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: multisample_state(msaa_sample_count),
            multiview: None,
            cache: None,
        });

        let blit_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("bearcad_viewport_blit_layout"),
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
                label: Some("bearcad_viewport_blit_layout"),
                bind_group_layouts: &[&blit_bind_group_layout],
                push_constant_ranges: &[],
            });

        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bearcad_viewport_blit_pipeline"),
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
                    blend: Some(wgpu::BlendState::REPLACE),
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
            label: Some("bearcad_viewport_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let font_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bearcad_viewport_font_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bearcad_viewport_vertices"),
            size: 4096,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bearcad_viewport_indices"),
            size: 4096,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let text_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bearcad_viewport_text_vertices"),
            size: 4096,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let text_index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bearcad_viewport_text_indices"),
            size: 4096,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            target_format,
            msaa_sample_count,
            scene_pipeline,
            sketch_fill_pipeline,
            scene_transparent_pipeline,
            text_pipeline,
            blit_pipeline,
            uniform_buffer,
            uniform_bind_group,
            text_texture_bind_group_layout,
            sampler,
            font_sampler,
            color_texture: None,
            color_view: None,
            msaa_color_texture: None,
            msaa_color_view: None,
            depth_texture: None,
            depth_view: None,
            blit_bind_group: None,
            vertex_buffer,
            index_buffer,
            text_vertex_buffer,
            text_index_buffer,
            vertex_capacity: 4096,
            index_capacity: 4096,
            text_vertex_capacity: 4096,
            text_index_capacity: 4096,
            texture_size: [0, 0],
            pending_scene: Mutex::new(None),
            font_bind_group: Mutex::new(None),
        }
    }

    fn update_font_bind_group(
        &self,
        device: &wgpu::Device,
        render_state: &egui_wgpu::RenderState,
    ) {
        let renderer = render_state.renderer.read();
        let Some(tex) = renderer.texture(&egui::TextureId::default()) else {
            return;
        };
        let Some(wgpu_tex) = tex.texture.as_ref() else {
            return;
        };
        let view = wgpu_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bearcad_viewport_font_bind_group"),
            layout: &self.text_texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.font_sampler),
                },
            ],
        });
        *self.font_bind_group.lock().unwrap() = Some(bind_group);
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
            label: Some("bearcad_viewport_color_resolve"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color_texture.create_view(&Default::default());

        let (msaa_color_texture, msaa_color_view) = if self.msaa_sample_count > 1 {
            let msaa_color_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("bearcad_viewport_color_msaa"),
                size: extent,
                mip_level_count: 1,
                sample_count: self.msaa_sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: self.target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let msaa_color_view =
                msaa_color_texture.create_view(&wgpu::TextureViewDescriptor::default());
            (Some(msaa_color_texture), Some(msaa_color_view))
        } else {
            (None, None)
        };

        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bearcad_viewport_depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: self.msaa_sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: VIEWPORT_DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&Default::default());

        let blit_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bearcad_viewport_blit_layout_runtime"),
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
            label: Some("bearcad_viewport_blit_bind_group"),
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
        self.msaa_color_texture = msaa_color_texture;
        self.msaa_color_view = msaa_color_view;
        self.depth_texture = Some(depth_texture);
        self.depth_view = Some(depth_view);
        self.blit_bind_group = Some(blit_bind_group);
    }

    fn ensure_text_buffer_capacity(
        &mut self,
        device: &wgpu::Device,
        vertex_bytes: u64,
        index_bytes: u64,
    ) {
        if vertex_bytes > self.text_vertex_capacity {
            self.text_vertex_capacity = vertex_bytes.next_power_of_two().max(4096);
            self.text_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bearcad_viewport_text_vertices"),
                size: self.text_vertex_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if index_bytes > self.text_index_capacity {
            self.text_index_capacity = index_bytes.next_power_of_two().max(4096);
            self.text_index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bearcad_viewport_text_indices"),
                size: self.text_index_capacity,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
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
                label: Some("bearcad_viewport_vertices"),
                size: self.vertex_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if index_bytes > self.index_capacity {
            self.index_capacity = index_bytes.next_power_of_two().max(4096);
            self.index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bearcad_viewport_indices"),
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
        scene: &ViewportScene,
        width: u32,
        height: u32,
    ) {
        self.ensure_targets(device, width, height);
        if width == 0 || height == 0 {
            return;
        }

        let vertex_bytes = (scene.vertices.len() * std::mem::size_of::<GpuVertex>()) as u64;
        let base_index_count = scene.indices.len();
        let sketch_fill_index_count = scene.sketch_fill_indices.len();
        let plane_fill_index_count = scene.plane_fill_indices.len();
        let overlay_index_count = scene.overlay_indices.len();
        let total_index_count =
            base_index_count + sketch_fill_index_count + plane_fill_index_count + overlay_index_count;
        let index_bytes = (total_index_count * std::mem::size_of::<u32>()) as u64;
        let text_vertex_bytes =
            (scene.text_vertices.len() * std::mem::size_of::<GpuTextVertex>()) as u64;
        let text_index_bytes = (scene.text_indices.len() * std::mem::size_of::<u32>()) as u64;
        self.ensure_buffer_capacity(device, vertex_bytes, index_bytes);
        self.ensure_text_buffer_capacity(device, text_vertex_bytes, text_index_bytes);

        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&GpuUniforms {
                view_proj: scene.view_proj.to_cols_array_2d(),
            }),
        );
        if !scene.vertices.is_empty() {
            queue.write_buffer(
                &self.vertex_buffer,
                0,
                bytemuck::cast_slice(&scene.vertices),
            );
        }
        if total_index_count > 0 {
            let mut combined_indices = Vec::with_capacity(total_index_count);
            combined_indices.extend_from_slice(&scene.indices);
            combined_indices.extend_from_slice(&scene.sketch_fill_indices);
            combined_indices.extend_from_slice(&scene.plane_fill_indices);
            combined_indices.extend_from_slice(&scene.overlay_indices);
            queue.write_buffer(
                &self.index_buffer,
                0,
                bytemuck::cast_slice(&combined_indices),
            );
        }
        if !scene.text_vertices.is_empty() {
            queue.write_buffer(
                &self.text_vertex_buffer,
                0,
                bytemuck::cast_slice(&scene.text_vertices),
            );
        }
        if !scene.text_indices.is_empty() {
            queue.write_buffer(
                &self.text_index_buffer,
                0,
                bytemuck::cast_slice(&scene.text_indices),
            );
        }

        let color_view = self.color_view.as_ref().expect("color view");
        let depth_view = self.depth_view.as_ref().expect("depth view");
        let (color_attachment_view, resolve_target, color_store) =
            if let Some(msaa_view) = self.msaa_color_view.as_ref() {
                (
                    msaa_view,
                    Some(color_view),
                    wgpu::StoreOp::Discard,
                )
            } else {
                (color_view, None, wgpu::StoreOp::Store)
            };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bearcad_viewport_scene_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bearcad_viewport_scene_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_attachment_view,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: scene.clear_color[0] as f64,
                            g: scene.clear_color[1] as f64,
                            b: scene.clear_color[2] as f64,
                            a: scene.clear_color[3] as f64,
                        }),
                        store: color_store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0),
                        store: wgpu::StoreOp::Discard,
                    }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if total_index_count > 0 {
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    self.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                let base_end = base_index_count as u32;
                let sketch_fill_end = (base_index_count + sketch_fill_index_count) as u32;
                let plane_end =
                    (base_index_count + sketch_fill_index_count + plane_fill_index_count) as u32;
                let total_end = total_index_count as u32;
                if base_end > 0 {
                    pass.set_pipeline(&self.scene_pipeline);
                    pass.draw_indexed(0..base_end, 0, 0..1);
                }
                if sketch_fill_end > base_end {
                    // Stencil ref 0: only fragments where the stencil is still 0 pass,
                    // and each one bumps the stencil to 1, so coplanar sketch fills paint
                    // each pixel exactly once instead of double-blending overlaps (#3).
                    pass.set_pipeline(&self.sketch_fill_pipeline);
                    pass.set_stencil_reference(0);
                    pass.draw_indexed(base_end..sketch_fill_end, 0, 0..1);
                }
                if plane_end > sketch_fill_end {
                    pass.set_pipeline(&self.scene_transparent_pipeline);
                    pass.draw_indexed(sketch_fill_end..plane_end, 0, 0..1);
                }
                if total_end > plane_end {
                    pass.set_pipeline(&self.scene_pipeline);
                    pass.draw_indexed(plane_end..total_end, 0, 0..1);
                }
            }
            if !scene.text_indices.is_empty() {
                if let Some(font_bind_group) = self.font_bind_group.lock().unwrap().as_ref() {
                    pass.set_pipeline(&self.text_pipeline);
                    pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                    pass.set_bind_group(1, font_bind_group, &[]);
                    pass.set_vertex_buffer(0, self.text_vertex_buffer.slice(..));
                    pass.set_index_buffer(
                        self.text_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.draw_indexed(0..scene.text_indices.len() as u32, 0, 0..1);
                }
            }
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

pub struct ViewportPaintCallback {
    rect: Rect,
}

impl egui_wgpu::CallbackTrait for ViewportPaintCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let resources: &mut ViewportGpuResources = callback_resources.get_mut().unwrap();
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
        let resources: &ViewportGpuResources = callback_resources.get().unwrap();
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

pub fn paint_viewport(
    resources: &ViewportGpuResources,
    render_state: &egui_wgpu::RenderState,
    painter: &egui::Painter,
    rect: Rect,
    scene: ViewportScene,
) {
    resources.update_font_bind_group(&render_state.device, render_state);
    *resources.pending_scene.lock().unwrap() = Some(scene);
    painter.add(egui_wgpu::Callback::new_paint_callback(
        rect,
        ViewportPaintCallback { rect },
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_msaa_prefers_four_samples_when_supported() {
        assert_eq!(clamp_msaa_sample_count(8), VIEWPORT_MSAA_SAMPLES);
        assert_eq!(clamp_msaa_sample_count(4), VIEWPORT_MSAA_SAMPLES);
    }

    #[test]
    fn clamp_msaa_falls_back_to_two_or_one() {
        assert_eq!(clamp_msaa_sample_count(3), 2);
        assert_eq!(clamp_msaa_sample_count(2), 2);
        assert_eq!(clamp_msaa_sample_count(1), 1);
        assert_eq!(clamp_msaa_sample_count(0), 1);
    }

    #[test]
    fn multisample_state_keeps_alpha_to_coverage_off_for_transparent_fills() {
        let msaa = multisample_state(4);
        assert_eq!(msaa.count, 4);
        assert!(!msaa.alpha_to_coverage_enabled);
        let single = multisample_state(1);
        assert_eq!(single.count, 1);
        assert!(!single.alpha_to_coverage_enabled);
    }

    #[test]
    fn msaa_sample_count_for_format_requires_resolve_support() {
        let no_resolve = wgpu::TextureFormatFeatures {
            allowed_usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
            flags: wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4,
        };
        assert_eq!(msaa_sample_count_for_format(&no_resolve), 1);

        let with_resolve = wgpu::TextureFormatFeatures {
            allowed_usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
            flags: wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4
                | wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE,
        };
        assert_eq!(
            msaa_sample_count_for_format(&with_resolve),
            VIEWPORT_MSAA_SAMPLES
        );
    }
}