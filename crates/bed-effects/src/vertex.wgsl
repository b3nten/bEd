// Translated from nealmick/ned shaders/vertex.glsl and ShaderQuad's positions.
// Upstream MIT/X Consortium; see NOTICE.
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tex_coords: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    let pos = positions[index];
    // Keep GLSL's bottom-origin TexCoords for the effect formulas. The texture
    // helpers below flip only at sampling, because wgpu textures have top origin.
    return VertexOutput(vec4(pos, 0.0, 1.0), pos * 0.5 + vec2(0.5));
}

struct Parameters {
    resolution: vec2<f32>, time: f32, effects_enabled: f32,
    scanline_intensity: f32, vignet_intensity: f32, bloom_intensity: f32, static_intensity: f32,
    colorshift_intensity: f32, jitter_intensity: f32, curvature_intensity: f32, pixelation_intensity: f32,
    pixel_width: f32, decay: f32, output_srgb: f32, pulse_intensity: f32,
}
@group(0) @binding(0) var<uniform> params: Parameters;
@group(0) @binding(1) var current_frame: texture_2d<f32>;
@group(0) @binding(2) var previous_frame: texture_2d<f32>;
@group(0) @binding(3) var frame_sampler: sampler;

fn texture_uv(gl_uv: vec2<f32>) -> vec2<f32> { return vec2(gl_uv.x, 1.0 - gl_uv.y); }
fn sample_current(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(current_frame, frame_sampler, texture_uv(uv), 0.0);
}
