//! Colours and sizes shared by the viewer. The colours are the theme's:
//! [`set`] takes them from it before each draw.

use bedtime_ui::{
    Theme,
    egui::{Color32, Stroke},
    theme::{self, Appearance, Rgba},
};
use std::cell::Cell;

/// The colours the views draw with.
#[derive(Clone, Copy)]
struct Colors {
    read: Color32,
    write: Color32,
    filter: Color32,
    warn: Color32,
    ambiguity: Color32,
    node_fill: Color32,
    node_fill_hover: Color32,
    node_fill_set: Color32,
    node_fill_sync: Color32,
    node_stroke: Color32,
    node_text: Color32,
    node_text_alt: Color32,
    node_text_dim: Color32,
    selected: Color32,
    exclusive: Color32,
    edge: Color32,
    edge_dim: Color32,
    edge_sync: Color32,
    canvas_bg: Color32,
    /// Whether the theme is a light one: hashed hues are darkened on it.
    light: bool,
}

impl Colors {
    fn new(theme: &Theme) -> Self {
        let p = &theme.palette;
        let color = theme::color;
        let faded = |rgba: Rgba, alpha: f32| color(rgba).gamma_multiply(alpha);
        // The terminal colours are the theme's hues: 4 is blue, 5 magenta.
        let (blue, magenta) = (p.terminal[4], p.terminal[5]);
        Self {
            read: color(blue),
            write: color(theme::mix(p.warning, p.error, 0.5)),
            filter: color(p.muted),
            warn: color(p.warning),
            ambiguity: color(p.error),
            node_fill: color(p.surface),
            node_fill_hover: color(p.elevated),
            node_fill_set: color(theme::mix(p.surface, magenta, 0.2)),
            node_fill_sync: color(theme::mix(p.surface, p.warning, 0.25)),
            node_stroke: color(p.border),
            node_text: color(p.text),
            node_text_alt: color(theme::mix(p.text, p.muted, 0.5)),
            node_text_dim: color(theme::mix(p.muted, p.background, 0.4)),
            selected: color(p.accent),
            exclusive: color(magenta),
            edge: faded(p.muted, 0.65),
            edge_dim: faded(p.muted, 0.3),
            edge_sync: faded(blue, 0.6),
            canvas_bg: color(p.background),
            light: theme.appearance == Appearance::Light,
        }
    }
}

thread_local! {
    /// The colours of the draw under way. Views are drawn one at a time, on
    /// one thread, each after a [`set`].
    static COLORS: Cell<Colors> = Cell::new(Colors::new(&Theme::dark()));
}

/// Takes the colours from `theme`, for what's drawn next on this thread.
pub fn set(theme: &Theme) {
    COLORS.set(Colors::new(theme));
}

macro_rules! colors {
    ($($name:ident),* $(,)?) => {
        $(pub fn $name() -> Color32 {
            COLORS.get().$name
        })*
    };
}
colors!(
    read,
    write,
    filter,
    warn,
    ambiguity,
    node_fill,
    node_fill_hover,
    node_fill_set,
    node_fill_sync,
    node_stroke,
    node_text,
    node_text_alt,
    node_text_dim,
    selected,
    exclusive,
    edge,
    edge_dim,
    edge_sync,
    canvas_bg,
);

/// An ordering edge into the node looked at: the neighbour runs before it.
pub fn edge_in() -> Color32 {
    read()
}

/// An ordering edge out of the node looked at: the neighbour runs after it.
pub fn edge_out() -> Color32 {
    write()
}

/// Axes, ticks and separators.
pub fn rule() -> Color32 {
    node_stroke()
}

pub fn edge_stroke(color: Color32, width: f32) -> Stroke {
    Stroke::new(width, color)
}

/// Stable pseudo-random hue per name, for cluster tints and component colours.
pub fn hue_color(name: &str, saturation: f32, value: f32, alpha: u8) -> Color32 {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32;
    // Bright hues wash out on a light background.
    let value = if COLORS.get().light {
        value * 0.72
    } else {
        value
    };
    let (r, g, b) = hsv_to_rgb(hue, saturation, value);
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

/// Cold-to-hot colour for a normalised duration `t` in `0..=1`.
pub fn heat_color(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let stops: [(f32, [f32; 3]); 4] = [
        (0.0, [64.0, 84.0, 128.0]),
        (0.45, [80.0, 150.0, 140.0]),
        (0.75, [232.0, 164.0, 60.0]),
        (1.0, [240.0, 70.0, 50.0]),
    ];
    let mut color = stops[0].1;
    for pair in stops.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        if t >= t0 && t <= t1 {
            let k = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            color = [
                c0[0] + (c1[0] - c0[0]) * k,
                c0[1] + (c1[1] - c0[1]) * k,
                c0[2] + (c1[2] - c0[2]) * k,
            ];
        }
    }
    Color32::from_rgb(color[0] as u8, color[1] as u8, color[2] as u8)
}
