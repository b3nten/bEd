// Original formulas retained for GPU equivalence tests of the optimized shader.
// Origins: nealmick/ned shaders/fragment.glsl; MIT/X Consortium, see NOTICE.
fn sample_bloom(uv: vec2<f32>, offset: f32) -> vec3<f32> {
    var bloom = vec3(0.0);
    var total = 0.0;
    for (var x: i32 = -2; x <= 2; x += 1) {
        for (var y: i32 = -2; y <= 2; y += 1) {
            let xy = vec2(f32(x), f32(y));
            let sample_uv = uv + xy * offset / params.resolution;
            let weight = 1.0 - length(xy) * 0.1;
            if weight > 0.0 { bloom += sample_current(sample_uv).rgb * weight; total += weight; }
        }
    }
    return bloom / total;
}
fn apply_curvature(uv: vec2<f32>, intensity: f32) -> vec2<f32> {
    if intensity == 0.0 { return uv; }
    let center = uv - vec2(0.5);
    var radius = length(center);
    let angle = atan2(center.y, center.x);
    let distortion = intensity * 0.25 * pow(radius, 2.0);
    radius *= 1.0 + distortion;
    let distorted = vec2(0.5) + radius * vec2(cos(angle), sin(angle));
    return clamp(distorted, vec2(0.001), vec2(0.999));
}
