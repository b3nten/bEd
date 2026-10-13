//! Color formats and indexed-color boundaries described by XParseColor/xterm.
use bed_terminal::terminal::{Terminal, parse_color};

#[test]
fn color_formats_preserve_channel_precision_and_x11_aliases() {
    for (spec, rgb) in [
        ("#a3f", [170, 51, 255]),
        ("#1a3b5c", [26, 59, 92]),
        ("#123456789abc", [18, 86, 154]),
        ("rgb:f/80/1234", [255, 128, 18]),
        (" Light Goldenrod Yellow ", [250, 250, 210]),
        ("gReY50", [127; 3]),
        ("gray90", [229; 3]),
        ("red3", [205, 0, 0]),
    ] {
        assert_eq!(parse_color(spec), Some(rgb), "{spec}");
    }
    for spec in [
        "",
        "#abcd",
        "#123456789",
        "#🦀",
        "rgb:/0/0",
        "rgb:1/2/3/4",
        "rgb:fffff/0/0",
        "rgb:+1/2/3",
        "absent-color",
    ] {
        assert_eq!(parse_color(spec), None, "{spec}");
    }
}

#[test]
fn default_palette_keeps_ansi_cube_ramp_and_dynamic_colors() {
    let terminal = Terminal::new(12, 2);
    for (index, expected) in [
        (1, [205, 0, 0]),
        (7, [229; 3]),
        (12, [92, 92, 255]),
        (16, [0; 3]),
        (21, [0, 0, 255]),
        (52, [95, 0, 0]),
        (231, [255; 3]),
        (232, [8; 3]),
        (255, [238; 3]),
        (256, [204; 3]),
        (257, [85; 3]),
        (258, [229; 3]),
        (259, [0; 3]),
    ] {
        assert_eq!(terminal.palette()[index], expected, "palette entry {index}");
    }
}
