// CRT formulas adapted from nealmick/ned and the former application; MIT/X Consortium, see NOTICE.
// Bed2 owns linear/premultiplied conversion, history timing and input mapping.
import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import bevy_render::color_operations::{linear_to_srgb, srgb_to_linear};

struct Parameters {
    light: vec4<f32>,
    geometry: vec4<f32>,
    clock: vec4<f32>,
}
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Parameters;
@group(0) @binding(3) var previous_frame: texture_2d<f32>;
@group(0) @binding(4) var bloom_frame: texture_2d<f32>;

fn display_color(color: vec4<f32>) -> vec4<f32> {
    if color.a <= 0.0 { return vec4<f32>(0.0); }
    return vec4<f32>(linear_to_srgb(max(color.rgb / color.a, vec3<f32>(0.0))) * color.a, color.a);
}
fn linear_color(color: vec4<f32>) -> vec4<f32> {
    if color.a <= 0.0 { return vec4<f32>(0.0); }
    return vec4<f32>(srgb_to_linear(clamp(color.rgb / color.a, vec3<f32>(0.0), vec3<f32>(1.0))) * color.a, color.a);
}
fn sample_display(uv: vec2<f32>) -> vec4<f32> {
    return display_color(textureSampleLevel(source, source_sampler, uv, 0.0));
}

// Keep source_position in effects.rs in agreement with this display→source map.
fn curvature(uv: vec2<f32>) -> vec2<f32> {
    if params.geometry.x == 0.0 { return uv; }
    let center = uv - vec2<f32>(0.5);
    return clamp(vec2<f32>(0.5) + center * (1.0 + params.geometry.x * 0.25 * dot(center, center)), vec2<f32>(0.001), vec2<f32>(0.999));
}
// Bloom intermediates store display-premultiplied color in float textures.
// Convert only during downsampling; blur and final sampling need no gamma math.
@fragment
fn bloom_downsample(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let half_texel = 0.5 / vec2<f32>(textureDimensions(source));
    return (sample_display(in.uv + half_texel * vec2<f32>(-1.0, -1.0))
          + sample_display(in.uv + half_texel * vec2<f32>( 1.0, -1.0))
          + sample_display(in.uv + half_texel * vec2<f32>(-1.0,  1.0))
          + sample_display(in.uv + half_texel * vec2<f32>( 1.0,  1.0))) * 0.25;
}

fn blur(uv: vec2<f32>, axis: vec2<f32>) -> vec4<f32> {
    // Separable [1,4,6,4,1]/16 kernel, pairing adjacent taps with bilinear
    // filtering. Half-resolution texels give a roughly four-source-pixel radius.
    let offset = axis * 1.2 / vec2<f32>(textureDimensions(source));
    return textureSampleLevel(source, source_sampler, uv, 0.0) * 0.375
         + textureSampleLevel(source, source_sampler, uv - offset, 0.0) * 0.3125
         + textureSampleLevel(source, source_sampler, uv + offset, 0.0) * 0.3125;
}
@fragment
fn bloom_horizontal(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return blur(in.uv, vec2<f32>(1.0, 0.0));
}
@fragment
fn bloom_vertical(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return blur(in.uv, vec2<f32>(0.0, 1.0));
}
fn scanline(uv: vec2<f32>, resolution: vec2<f32>, time: f32) -> f32 {
    let cycle = time - 8.0 * floor(time / 8.0);
    if cycle >= 5.0 || params.light.x == 0.0 { return 1.0; }
    let distance = (cycle * 0.2 - uv.y) * resolution.y;
    if distance < 0.0 || distance >= 130.0 { return 1.0; }
    let fade_out = pow(smoothstep(0.2, 1.0, 1.0 - distance / 130.0), 1.2);
    let fade_in = smoothstep(0.0, 1.0, min(distance / 7.0, 1.0));
    return mix(1.0, 1.7, fade_in * fade_out * params.light.x);
}
fn random(co: vec2<f32>) -> f32 {
    let dt = dot(co, vec2<f32>(12.9898, 78.233));
    return fract(sin(dt - 3.14 * floor(dt / 3.14)) * 43758.5453);
}
fn noise(uv: vec2<f32>, time: f32) -> f32 {
    let t = vec3<f32>(time * 60.0, time * 55.0, time * 0.1);
    let wrapped = t - vec3<f32>(10.0, 10.0, 5.0) * floor(t / vec3<f32>(10.0, 10.0, 5.0));
    let n1 = random(uv * 2.5 + vec2<f32>(wrapped.x));
    let n2 = fract(sin(dot(uv * 3.7 + vec2<f32>(wrapped.y), vec2<f32>(12.9898, 78.233))) * 43758.5453123);
    let n3 = random((uv + vec2<f32>(wrapped.z)) * 1.5);
    return (n1 * 0.5 + n2 * 0.3 + n3 * 0.2) * mix(0.8, 1.0, sin(time * 0.5) * 0.5 + 0.5);
}

@fragment
fn burn(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let current = sample_display(in.uv);
    let previous = display_color(textureSampleLevel(previous_frame, source_sampler, in.uv, 0.0));
    // Current coverage is authoritative: old frames cannot leave opaque ghosts
    // in transparent regions. Accumulate before curvature to avoid repeated warps.
    let rgb = min(max(current.rgb, previous.rgb * params.clock.y), vec3<f32>(current.a));
    return linear_color(vec4<f32>(rgb, current.a));
}

@fragment
fn crt(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let uv = curvature(in.uv);
    let resolution = vec2<f32>(textureDimensions(source));
    let time = params.clock.x;
    let sample = sample_display(uv);
    var color = sample.rgb;
    if params.light.z > 0.0 {
        color += textureSampleLevel(bloom_frame, source_sampler, uv, 0.0).rgb * params.light.z;
    }
    let vig_uv = uv * (vec2<f32>(1.0) - uv.yx);
    color *= pow(vig_uv.x * vig_uv.y * 15.0, params.light.y) * scanline(uv, resolution, time);
    if params.light.w > 0.0 {
        color += vec3<f32>((noise(vec2<f32>(in.uv.x, 1.0 - in.uv.y), time) - 0.5) * params.light.w);
    }
    color *= sin(time * 0.5) * params.geometry.w + 1.0;
    if params.geometry.y > 0.0 {
        let pixel = vec2<f32>(in.position.x, resolution.y - in.position.y);
        let cell = resolution / 250.0;
        let grid = pixel - cell * floor(pixel / cell);
        if grid.x < 1.0 || grid.y < 1.0 { color *= 1.0 - params.geometry.y; }
    }
    return linear_color(vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(sample.a)), sample.a));
}
