// Translated from nealmick/ned shaders/burn_in.frag; see LICENSE and NOTICE.
@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let current = sample_current(input.tex_coords).rgb;
    let accum = textureSampleLevel(previous_frame, frame_sampler, texture_uv(input.tex_coords), 0.0).rgb;
    let result = max(current, accum * pow(params.decay, 4.0));
    return vec4(clamp(result, vec3(0.0), vec3(1.0)), 1.0);
}
