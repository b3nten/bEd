// The cat's image is painted over the window by egui, which takes colours
// premultiplied in gamma space: sRGB(colour) × alpha. Bevy blends in linear
// light and leaves sRGB(colour × alpha), which for a partly covered pixel is
// lighter (a whisker's edge, off the cat, glows). This turns one into the
// other. The image is sRGB, so the value read here is linear and the one
// written is encoded again on the way out.
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import bevy_render::color_operations::{linear_to_srgb, srgb_to_linear};

struct Unused {
    unused: f32,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> unused: Unused;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(source, source_sampler, in.uv);
    if color.a <= 0.0 { return vec4<f32>(0.0); }
    let straight = clamp(color.rgb / color.a, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(srgb_to_linear(linear_to_srgb(straight) * color.a), color.a);
}
