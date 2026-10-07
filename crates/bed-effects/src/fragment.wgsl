// CRT formula origins: nealmick/ned shaders/fragment.glsl.
// MIT/X Consortium; see LICENSE and NOTICE. Bed removes overwritten pixelation,
// makes pulse optional and preserves exact coordinates for the sharp preset.
fn gl_mod(value: f32, divisor: f32) -> f32 { return value - divisor * floor(value / divisor); }
fn random(co: vec2<f32>) -> f32 {
    let dt = dot(co, vec2(12.9898, 78.233));
    let sn = gl_mod(dt, 3.14);
    return fract(sin(sn) * 43758.5453);
}
fn random2(co: vec2<f32>) -> f32 { return fract(sin(dot(co, vec2(12.9898, 78.233))) * 43758.5453123); }
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
fn add_jitter(uv: vec2<f32>, time: f32) -> vec2<f32> {
    if params.jitter_intensity == 0.0 { return uv; }
    let jitter_speed = 2.0;
    let jitter_amount = 0.0009 * params.jitter_intensity;
    let rand = random(vec2(gl_mod(time * 0.1, 100.0)));
    if rand > 0.97 {
        let jitter = vec2(random(uv + vec2(gl_mod(time * jitter_speed, 10.0))) - 0.5,
            random(uv + vec2(gl_mod(time * jitter_speed + 1.0, 10.0))) - 0.5) * jitter_amount;
        return uv + jitter;
    }
    return uv;
}
fn generate_static(uv: vec2<f32>, time: f32) -> f32 {
    let t1 = gl_mod(time * 60.0, 10.0);
    let t2 = gl_mod(time * 55.0, 10.0);
    let t3 = gl_mod(time * 0.1, 5.0);
    let noise1 = random(uv * 2.5 + vec2(t1));
    let noise2 = random2(uv * 3.7 + vec2(t2));
    let noise3 = random((uv + vec2(t3)) * 1.5);
    let static_noise = noise1 * 0.5 + noise2 * 0.3 + noise3 * 0.2;
    let intensity_mod = mix(0.8, 1.0, sin(time * 0.5) * 0.5 + 0.5);
    return static_noise * intensity_mod;
}
fn get_bloom(uv: vec2<f32>) -> vec3<f32> {
    if params.bloom_intensity == 0.0 { return vec3(0.0); }
    return sample_bloom(uv, 2.0) * params.bloom_intensity;
}
fn apply_vignette(uv: vec2<f32>) -> f32 {
    let vig_uv = uv * (vec2(1.0) - uv.yx);
    return pow(vig_uv.x * vig_uv.y * 15.0, params.vignet_intensity);
}
fn calculate_scanline(uv: vec2<f32>, time: f32) -> f32 {
    let scan_speed = 0.2;
    let cycle_time = gl_mod(time, 1.0 / scan_speed + 3.0);
    var scanline = 1.0;
    if cycle_time < 1.0 / scan_speed {
        let scan_pos = cycle_time * scan_speed;
        let scan_dist = (uv.y - (1.0 - fract(scan_pos))) * params.resolution.y;
        if scan_dist >= 0.0 && scan_dist < 130.0 {
            var fade_out = 1.0 - scan_dist / 130.0;
            fade_out = pow(smoothstep(0.2, 1.0, fade_out), 1.2);
            var fade_in = min(scan_dist / 7.0, 1.0);
            fade_in = smoothstep(0.0, 1.0, fade_in);
            scanline = mix(1.0, 1.7, fade_in * fade_out * params.scanline_intensity);
        }
    }
    return scanline;
}
fn apply_static_noise(color: vec3<f32>, time: f32, gl_frag_coord: vec2<f32>) -> vec3<f32> {
    if params.static_intensity == 0.0 { return color; }
    let static_noise = generate_static(gl_frag_coord / params.resolution, time);
    return color + vec3((static_noise - 0.5) * params.static_intensity);
}
fn apply_pulse(color: vec3<f32>, time: f32) -> vec3<f32> { return color * (sin(time * 0.5) * params.pulse_intensity + 1.0); }
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
fn apply_color_shift(uv: vec2<f32>, time: f32) -> vec3<f32> {
    if params.colorshift_intensity == 0.0 { return sample_current(uv).rgb; }
    let shift_amount = 0.001 * params.colorshift_intensity;
    let shift = sin(time * 0.5) * shift_amount;
    return vec3(sample_current(uv + vec2(shift, 0.0)).r,
        sample_current(uv + vec2(shift * 0.3, 0.0)).g,
        sample_current(uv - vec2(shift, 0.0)).b);
}
fn apply_grid(color: vec3<f32>, gl_frag_coord: vec2<f32>) -> vec3<f32> {
    let cell_size = params.resolution / 250.0;
    let grid_pos = gl_frag_coord - cell_size * floor(gl_frag_coord / cell_size);
    if grid_pos.x < 1.0 || grid_pos.y < 1.0 { return mix(color, vec3(0.0), params.pixelation_intensity); }
    return color;
}
fn present_color(color: vec4<f32>) -> vec4<f32> {
    if params.output_srgb < 0.5 { return color; }
    // GL wrote display-referred values to a non-sRGB backbuffer. Cancel the
    // wgpu sRGB attachment transfer while preserving every effect calculation.
    let rgb = max(color.rgb, vec3(0.0));
    let linear = select(rgb / 12.92, pow((rgb + vec3(0.055)) / 1.055, vec3(2.4)), rgb > vec3(0.04045));
    return vec4(linear, color.a);
}
@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if params.effects_enabled > 0.5 {
        var uv = apply_curvature(input.tex_coords, params.curvature_intensity);
        uv = add_jitter(uv, params.time);
        var color = apply_color_shift(uv, params.time);
        color += get_bloom(uv);
        color *= apply_vignette(uv) * calculate_scanline(uv, params.time);
        let gl_frag_coord = vec2(input.position.x, params.resolution.y - input.position.y);
        color = apply_static_noise(color, params.time, gl_frag_coord);
        color = apply_pulse(color, params.time);
        color = apply_grid(color, gl_frag_coord);
        return present_color(vec4(color, 1.0));
    }
    return present_color(sample_current(input.tex_coords));
}
