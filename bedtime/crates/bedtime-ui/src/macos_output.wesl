import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import bevy_render::color_operations::{linear_to_srgb, srgb_to_linear};

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@fragment
fn fs_main(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(source, source_sampler, in.uv);
    if color.a <= 0.0 {
        return vec4<f32>(0.0);
    }
    // AppKit composites premultiplied display-space RGB. Bevy's target holds
    // premultiplied linear RGB. Unmultiply before encoding, then premultiply
    // in display space. Decode once so the sRGB attachment's encoding leaves
    // exactly those bytes for the window compositor. Alpha is never gamma encoded.
    let display = linear_to_srgb(color.rgb / color.a) * color.a;
    return vec4<f32>(srgb_to_linear(display), color.a);
}
