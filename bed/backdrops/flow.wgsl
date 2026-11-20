// Slow clouds of the theme's muted and accent colours, folding into each
// other over its background.
// gradient.wgsl says what a backdrop can read.

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash2(cell), hash2(cell + vec2<f32>(1.0, 0.0)), u.x),
        mix(hash2(cell + vec2<f32>(0.0, 1.0)), hash2(cell + vec2<f32>(1.0, 1.0)), u.x),
        u.y,
    );
}

// Noise with detail: several octaves, each finer and fainter.
fn fbm(p: vec2<f32>) -> f32 {
    var total = 0.0;
    var amplitude = 0.5;
    var at = p;
    for (var octave = 0; octave < 4; octave++) {
        total += amplitude * noise(at);
        at = at * 2.02 + vec2<f32>(13.1, 7.7);
        amplitude *= 0.5;
    }
    return total;
}

fn backdrop(uv: vec2<f32>, pixel: vec2<f32>) -> vec4<f32> {
    let p = vec2<f32>(uv.x * ui.resolution.x / ui.resolution.y, uv.y) * 1.6;
    let t = ui.time * 0.04;
    // Noise looked up where other noise says: that is what folds it.
    let fold = vec2<f32>(fbm(p + vec2<f32>(t, -t)), fbm(p + vec2<f32>(5.2 - t, 1.3 + t)));
    let cloud = fbm(p + 2.5 * fold);
    let tint = mix(ui.muted.rgb, ui.accent.rgb, smoothstep(0.3, 0.7, fold.x));
    return vec4<f32>(mix(ui.background.rgb, tint, smoothstep(0.3, 0.75, cloud) * 0.4), 1.0);
}
