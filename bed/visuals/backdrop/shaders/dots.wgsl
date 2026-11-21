// A field of dots that breathe in a slow wave, and swell and take the theme's
// accent colour near the pointer.
// gradient.wgsl says what a backdrop can read.

fn backdrop(uv: vec2<f32>, pixel: vec2<f32>) -> vec4<f32> {
    let spacing = 22.0 * ui.scale;
    let cell = floor(pixel / spacing);
    let center = (cell + 0.5) * spacing;
    // How near the pointer this dot is: 1 under it, 0 far off or with it away.
    var near = 0.0;
    if ui.pointer.x >= 0.0 {
        near = exp(-distance(center, ui.pointer) / (120.0 * ui.scale));
    }
    let wave = 0.5 + 0.5 * sin(ui.time * 0.6 + (cell.x + cell.y) * 0.35);
    let radius = spacing * (0.05 + 0.05 * wave + 0.3 * near);
    let dot = 1.0 - smoothstep(radius - ui.scale, radius + ui.scale, distance(pixel, center));
    let tint = mix(ui.muted.rgb, ui.accent.rgb, near);
    return vec4<f32>(mix(ui.background.rgb, tint, dot * (0.3 + 0.7 * near)), 1.0);
}
