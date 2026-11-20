// The simplest backdrop, to start your own from: the theme's background,
// warming to its accent colour towards the bottom.
//
// A backdrop is a function giving each pixel its colour. `uv` runs from (0, 0)
// at the window's top left to (1, 1) at its bottom right, `pixel` is the same
// place in pixels, and the result is linear RGB with alpha. `ui` has:
//
//   ui.resolution  the window's size, in pixels
//   ui.pointer     the pointer, in pixels; (-1, -1) when away
//   ui.time        seconds since bEd started
//   ui.delta       seconds since the last frame
//   ui.scale       pixels per point
//   ui.frame       frames drawn so far
//   ui.background, ui.panel, ui.text, ui.muted, ui.accent
//                  the theme's colours
//
// Any `<name>.wgsl` in this folder can be chosen in Settings (Window,
// Backdrop). Save the file and bEd shows the change; a mistake is reported
// with its line. Panels cover the backdrop until their opacity is lowered
// (Panels, Opacity).
fn backdrop(uv: vec2<f32>, pixel: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(mix(ui.background.rgb, ui.accent.rgb, uv.y * uv.y * 0.3), 1.0);
}
