
// ── Added after a backdrop shader's own source, which defines
//
//     fn backdrop(uv: vec2<f32>, pixel: vec2<f32>) -> vec4<f32>
//
// `uv` runs from (0, 0) at the window's top left to (1, 1) at its bottom
// right; `pixel` is the same place in physical pixels. The result is linear
// RGB with straight (not premultiplied) alpha.

struct Ui {
    // The window, in physical pixels.
    resolution: vec2<f32>,
    // The pointer, in physical pixels from the top left; (-1, -1) when away.
    pointer: vec2<f32>,
    // Seconds since the app started; 0 with animations turned off.
    time: f32,
    // Seconds since the last frame.
    delta: f32,
    // Physical pixels per logical point.
    scale: f32,
    // Frames drawn so far.
    frame: f32,
    // The theme's colours, linear RGB.
    background: vec4<f32>,
    panel: vec4<f32>,
    text: vec4<f32>,
    muted: vec4<f32>,
    accent: vec4<f32>,
}

@group(0) @binding(0) var<uniform> ui: Ui;

struct BackdropVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn backdrop_vertex(@builtin(vertex_index) index: u32) -> BackdropVertex {
    // One triangle covering the target.
    let uv = vec2<f32>(f32(index >> 1u), f32(index & 1u)) * 2.0;
    return BackdropVertex(vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0), uv);
}

@fragment
fn backdrop_fragment(in: BackdropVertex) -> @location(0) vec4<f32> {
    let color = backdrop(in.uv, in.position.xy);
    let alpha = clamp(color.a, 0.0, 1.0);
    return vec4<f32>(max(color.rgb, vec3<f32>(0.0)) * alpha, alpha);
}
