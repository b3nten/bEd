//! Translated from ned editor/views/diagnostic_style.h; see LICENSE and NOTICE.
pub fn severity_mark(severity: i32) -> u32 {
    let (r, g, b, a) = match severity {
        2 => (210, 160, 50, 230),
        3 => (70, 140, 210, 230),
        value if value >= 4 => (120, 160, 120, 220),
        _ => (220, 70, 70, 240),
    };
    r | (g << 8) | (b << 16) | (a << 24)
}
pub fn severity_color(severity: i32) -> [f32; 4] {
    let mark = severity_mark(severity);
    std::array::from_fn(|index| ((mark >> (index * 8)) & 255) as f32 / 255.0)
}
pub fn severity_label(severity: i32) -> &'static str {
    match severity {
        2 => "Warning",
        3 => "Info",
        4 => "Hint",
        _ => "Error",
    }
}
