struct Camera { view_projection: mat4x4<f32>, eye: vec4<f32> };
struct Material { color: vec4<f32>, options: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var base_color: texture_2d<f32>;
@group(1) @binding(2) var base_sampler: sampler;

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};
@vertex fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>, @location(3) color: vec4<f32>) -> VertexOutput {
    return VertexOutput(camera.view_projection * vec4(position, 1.0), position, normal, uv, color);
}
fn srgb(linear: vec3<f32>) -> vec3<f32> {
    let color = max(linear, vec3(0.0));
    return select(1.055 * pow(color, vec3(1.0 / 2.4)) - 0.055, color * 12.92, color <= vec3(0.0031308));
}
@fragment fn fs_main(vertex: VertexOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let base = textureSample(base_color, base_sampler, vertex.uv) * material.color * vertex.color;
    if material.options.z == 1.0 && base.a < material.options.x { discard; }
    var color = base.rgb;
    if material.options.y == 0.0 {
        let n = normalize(select(-vertex.normal, vertex.normal, front));
        let key = max(dot(n, normalize(vec3(0.5, 0.8, 0.6))), 0.0);
        let fill = max(dot(n, normalize(vec3(-0.6, 0.3, -0.5))), 0.0);
        color *= vec3(0.25) + vec3(0.65, 0.62, 0.58) * key + vec3(0.15, 0.18, 0.23) * fill;
    }
    return vec4(srgb(color), select(1.0, base.a, material.options.z == 2.0));
}
