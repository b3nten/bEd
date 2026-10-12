@group(0) @binding(0) var<uniform> rect: vec4<f32>;
@group(0) @binding(1) var image: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    let uv = corners[index];
    let p = rect.xy + uv * rect.zw;
    return VertexOutput(vec4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0), uv);
}
@fragment fn fs_main(vertex: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(image, image_sampler, vertex.uv);
}
