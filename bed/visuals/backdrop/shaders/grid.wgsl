// A glowing grid running to the horizon under a low sun, in the theme's
// accent colour.
// gradient.wgsl says what a backdrop can read.

const HORIZON: f32 = 0.56;

fn backdrop(uv: vec2<f32>, pixel: vec2<f32>) -> vec4<f32> {
    let aspect = ui.resolution.x / ui.resolution.y;
    let x = (uv.x - 0.5) * aspect;
    var color = ui.background.rgb * 0.7;

    if uv.y < HORIZON {
        // The sky, brighter towards the horizon, and the sun, cut by bands
        // that widen towards its foot.
        color = mix(color, ui.accent.rgb, 0.18 * pow(uv.y / HORIZON, 3.0));
        let from_sun = vec2<f32>(x, uv.y - (HORIZON - 0.13));
        let disc = 1.0 - smoothstep(0.2, 0.205, length(from_sun));
        let down = clamp(from_sun.y / 0.2 + 0.2, 0.0, 1.0);
        let bands = smoothstep(down * 0.7, down * 0.7 + 0.08, abs(fract(from_sun.y * 22.0 - ui.time * 0.15) - 0.5) * 2.0);
        color = mix(color, mix(ui.accent.rgb, ui.text.rgb, 0.35 - from_sun.y), disc * bands * 0.75);
    } else {
        // The ground: lines every unit across and away, the far ones moving
        // slower, fading into the horizon.
        let depth = (uv.y - HORIZON) / (1.0 - HORIZON);
        let ground = vec2<f32>(x / depth * 1.6, 1.0 / depth + ui.time * 0.35);
        let from_line = abs(fract(ground) - 0.5);
        let width = fwidth(ground) * 1.2;
        let lines = max(
            smoothstep(0.5 - width.x, 0.5, from_line.x),
            smoothstep(0.5 - width.y, 0.5, from_line.y),
        );
        color = mix(color, ui.accent.rgb, lines * smoothstep(0.0, 0.35, depth) * 0.7);
        color += ui.accent.rgb * 0.12 * (1.0 - smoothstep(0.0, 0.25, depth));
    }
    return vec4<f32>(color, 1.0);
}
