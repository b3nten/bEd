struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var output: VertexOutput;
    output.position = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    output.uv = uv;
    return output;
}
@group(0) @binding(0) var preview: texture_2d<f32>;
@group(0) @binding(1) var preview_sampler: sampler;
@fragment fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(preview, preview_sampler, input.uv);
}
