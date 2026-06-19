struct Uniforms {
    view_proj: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3f,
    @location(1) color: vec4f,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4f,
    @location(0) color: vec4f,
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4f(input.position, 1.0);
    out.color = input.color;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4f {
    return input.color;
}

struct BlitVertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_blit(@builtin(vertex_index) vertex_index: u32) -> BlitVertexOutput {
    var positions = array<vec2f, 3>(
        vec2f(-1.0, -1.0),
        vec2f(3.0, -1.0),
        vec2f(-1.0, 3.0),
    );
    var out: BlitVertexOutput;
    let pos = positions[vertex_index];
    out.position = vec4f(pos, 0.0, 1.0);
    out.uv = pos * vec2f(0.5, -0.5) + vec2f(0.5, 0.5);
    return out;
}

@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;

@fragment
fn fs_blit(input: BlitVertexOutput) -> @location(0) vec4f {
    return textureSample(scene_texture, scene_sampler, input.uv);
}

struct TextVertexInput {
    @location(0) position: vec3f,
    @location(1) uv: vec2f,
    @location(2) color: vec4f,
}

struct TextVertexOutput {
    @builtin(position) clip_position: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
}

@vertex
fn vs_text(input: TextVertexInput) -> TextVertexOutput {
    var out: TextVertexOutput;
    out.clip_position = uniforms.view_proj * vec4f(input.position, 1.0);
    out.uv = input.uv;
    out.color = input.color;
    return out;
}

@group(1) @binding(0) var font_texture: texture_2d<f32>;
@group(1) @binding(1) var font_sampler: sampler;

@fragment
fn fs_text(input: TextVertexOutput) -> @location(0) vec4f {
    let glyph = textureSample(font_texture, font_sampler, input.uv);
    return vec4f(input.color.rgb * glyph.a, input.color.a * glyph.a);
}