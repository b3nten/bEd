// Northern lights: curtains of green light fringed with violet, drifting over
// a starry sky the colour of the theme's background.
// gradient.wgsl says what a backdrop can read.

fn hash(n: f32) -> f32 {
    return fract(sin(n * 127.1) * 43758.5453);
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn noise(x: f32) -> f32 {
    let f = fract(x);
    return mix(hash(floor(x)), hash(floor(x) + 1.0), f * f * (3.0 - 2.0 * f));
}

// Noise with detail: several octaves, each finer and fainter.
fn fbm(x: f32) -> f32 {
    var total = 0.0;
    var amplitude = 0.5;
    var at = x;
    for (var octave = 0; octave < 4; octave++) {
        total += amplitude * noise(at);
        at = at * 2.03 + 17.0;
        amplitude *= 0.5;
    }
    return total;
}

// One curtain: how bright it is here, and how far above its lower edge.
fn curtain(p: vec2<f32>, layer: f32) -> vec2<f32> {
    let t = ui.time * (0.05 + 0.02 * layer);
    let x = p.x * (0.9 + 0.3 * layer) + layer * 7.3;
    // The lower edge wanders.
    let edge = 0.72 - 0.17 * layer + (fbm(x * 1.2 + t) - 0.47) * 0.5;
    let height = edge - p.y;
    // Sharp below, fading upwards; how far up it reaches varies along it.
    let reach = 2.5 + 4.5 * fbm(x * 2.5 - t * 1.5);
    let body = smoothstep(-0.02, 0.015, height) * exp(-max(height, 0.0) * reach);
    // Vertical rays, shimmering sideways.
    let rays = 0.4 + 0.6 * smoothstep(0.25, 0.7, fbm(x * 18.0 + fbm(x * 3.0 + t * 4.0) * 3.0));
    return vec2<f32>(body * rays, max(height, 0.0));
}

fn backdrop(uv: vec2<f32>, pixel: vec2<f32>) -> vec4<f32> {
    let p = vec2<f32>(uv.x * ui.resolution.x / ui.resolution.y, uv.y);
    // The sky darkens upwards.
    var color = ui.background.rgb * mix(0.45, 1.0, uv.y);

    // Stars, where the theme is dark enough for them.
    let brightness = dot(ui.background.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let cell = floor(pixel / (2.0 * ui.scale));
    let seed = hash2(cell);
    let twinkle = 0.6 + 0.4 * sin(ui.time * (1.0 + 3.0 * seed) + seed * 400.0);
    color += step(0.9975, seed) * twinkle * (1.0 - uv.y) * (1.0 - smoothstep(0.05, 0.3, brightness)) * 0.6;

    let green = vec3<f32>(0.05, 0.9, 0.32);
    let violet = vec3<f32>(0.4, 0.08, 0.75);
    var light = vec3<f32>(0.0);
    for (var layer = 0; layer < 3; layer++) {
        let lit = curtain(p, f32(layer));
        light += mix(green, violet, smoothstep(0.03, 0.42, lit.y)) * lit.x * (1.0 - 0.25 * f32(layer));
    }
    let cover = clamp(max(light.r, max(light.g, light.b)), 0.0, 1.0);
    return vec4<f32>(color * (1.0 - 0.8 * cover) + light * 0.6, 1.0);
}
