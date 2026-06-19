struct BearHudUniforms {
    center: vec2f,
    scale: f32,
    z_min: f32,
    z_max: f32,
    rect_min: vec2f,
    rect_size: vec2f,
}

@group(0) @binding(0) var<uniform> uniforms: BearHudUniforms;

struct BearVertexInput {
    @location(0) view_position: vec3f,
    @location(1) color: vec4f,
}

struct BearVertexOutput {
    @builtin(position) clip_position: vec4f,
    @location(0) color: vec4f,
}

@vertex
fn vs_bear(input: BearVertexInput) -> BearVertexOutput {
    var out: BearVertexOutput;
    let screen = vec2f(
        uniforms.center.x + input.view_position.x * uniforms.scale,
        uniforms.center.y - input.view_position.y * uniforms.scale,
    );
    let local = screen - uniforms.rect_min;
    let ndc_x = 2.0 * local.x / uniforms.rect_size.x - 1.0;
    let ndc_y = 1.0 - 2.0 * local.y / uniforms.rect_size.y;
    let z_span = max(uniforms.z_max - uniforms.z_min, 1e-4);
    let depth = (input.view_position.z - uniforms.z_min) / z_span;
    out.clip_position = vec4f(ndc_x, ndc_y, depth, 1.0);
    out.color = input.color;
    return out;
}

@fragment
fn fs_bear(input: BearVertexOutput) -> @location(0) vec4f {
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

@group(0) @binding(0) var bear_texture: texture_2d<f32>;
@group(0) @binding(1) var bear_sampler: sampler;

@fragment
fn fs_blit(input: BlitVertexOutput) -> @location(0) vec4f {
    return textureSample(bear_texture, bear_sampler, input.uv);
}