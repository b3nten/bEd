// Stars drifting past in three layers, the near ones larger and faster, in
// the theme's text colour with the odd one in its accent.
// gradient.wgsl says what a backdrop can read.

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

// A layer of stars: one, at most, in each `spacing`-point cell. Returns its
// brightness here and a number that is the star's own.
fn stars(pixel: vec2<f32>, spacing: f32, speed: f32) -> vec2<f32> {
    let p = pixel / (spacing * ui.scale) + vec2<f32>(ui.time * speed, spacing);
    let cell = floor(p);
    let seed = hash2(cell);
    let center = vec2<f32>(hash2(cell + 1.7), hash2(cell + 9.2)) * 0.7 + 0.15;
    let twinkle = 0.7 + 0.3 * sin(ui.time * (1.0 + 2.0 * seed) + seed * 50.0);
    let glow = 1.0 - smoothstep(0.015, 0.065, distance(fract(p), center));
    // Two cells in five are empty.
    return vec2<f32>(glow * twinkle * step(0.4, seed), seed);
}

fn backdrop(uv: vec2<f32>, pixel: vec2<f32>) -> vec4<f32> {
    // A faint glow low in the sky.
    var color = mix(ui.background.rgb * 0.6, ui.background.rgb + ui.accent.rgb * 0.04, uv.y);
    for (var layer = 0; layer < 3; layer++) {
        let depth = f32(layer);
        let star = stars(pixel, 26.0 + 22.0 * depth, 0.02 + 0.03 * depth);
        let tint = mix(ui.text.rgb, ui.accent.rgb, step(0.85, star.y));
        color = mix(color, tint, star.x * (0.35 + 0.25 * depth));
    }
    return vec4<f32>(color, 1.0);
}
