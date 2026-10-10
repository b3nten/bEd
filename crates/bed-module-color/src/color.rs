//! Conversions at the text-input boundary; internally all colors are finite RGBA.
pub const LABELS: [&str; 7] = ["HEX", "RGB", "RGBA", "HSL", "HSLA", "HSV", "Float"];

pub fn valid(color: [f32; 4]) -> bool {
    color
        .iter()
        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
}

fn number(text: &str, maximum: f32) -> Result<f32, &'static str> {
    let percent = text.ends_with('%');
    let value: f32 = text
        .strip_suffix('%')
        .unwrap_or(text)
        .parse()
        .map_err(|_| "Enter a complete color")?;
    let value = value / if percent { 100.0 } else { maximum };
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err("Color components are out of range");
    }
    Ok(value)
}

pub fn parse(text: &str, field: usize, previous_alpha: f32) -> Result<[f32; 4], &'static str> {
    let text = text.trim();
    let bare = text.strip_prefix('#').unwrap_or(text);
    if matches!(bare.len(), 3 | 4 | 6 | 8) && bare.bytes().all(|v| v.is_ascii_hexdigit()) {
        let mut color = [0.0, 0.0, 0.0, previous_alpha];
        let stride = if bare.len() <= 4 { 1 } else { 2 };
        for (index, component) in bare.as_bytes().chunks(stride).enumerate() {
            let value = u8::from_str_radix(std::str::from_utf8(component).unwrap(), 16).unwrap();
            color[index] = f32::from(if stride == 1 { value * 17 } else { value }) / 255.0;
        }
        return Ok(color);
    }
    let lower = text.to_ascii_lowercase();
    let (kind, content) = if let Some((kind, rest)) = lower.split_once('(') {
        (
            kind.trim(),
            rest.strip_suffix(')').ok_or("Enter a complete color")?,
        )
    } else {
        if lower.starts_with('[') != lower.ends_with(']') {
            return Err("Enter a complete color");
        }
        (
            if field == 6 || lower.starts_with('[') {
                "float"
            } else if field == 3 || field == 4 {
                "hsl"
            } else if field == 5 {
                "hsv"
            } else {
                "rgb"
            },
            lower.trim_matches(['[', ']']),
        )
    };
    let values: Vec<_> = content
        .split([',', '/', ' ', '\t'])
        .filter(|v| !v.is_empty())
        .collect();
    if !matches!(values.len(), 3 | 4) {
        return Err("Use three components, with optional alpha");
    }
    let alpha = if values.len() == 4 {
        number(values[3], 1.0)?
    } else {
        previous_alpha
    };
    let color = match kind {
        "rgb" | "rgba" | "float" => {
            let scale = if kind == "float" || field == 6 && !lower.contains('(') {
                1.0
            } else {
                255.0
            };
            [
                number(values[0], scale)?,
                number(values[1], scale)?,
                number(values[2], scale)?,
                alpha,
            ]
        }
        "hsl" | "hsla" | "hsv" | "hsva" => {
            let hue: f32 = values[0]
                .trim_end_matches("deg")
                .parse()
                .map_err(|_| "Enter a hue in degrees")?;
            if !hue.is_finite() {
                return Err("Hue must be finite");
            }
            let h = hue.rem_euclid(360.0) / 60.0;
            let s = number(values[1], 100.0)?;
            let l = number(values[2], 100.0)?;
            let c = if kind.starts_with("hsl") {
                (1.0 - (2.0 * l - 1.0).abs()) * s
            } else {
                l * s
            };
            let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
            let m = if kind.starts_with("hsl") {
                l - c / 2.0
            } else {
                l - c
            };
            let rgb = match h as u32 {
                0 => [c, x, 0.0],
                1 => [x, c, 0.0],
                2 => [0.0, c, x],
                3 => [0.0, x, c],
                4 => [x, 0.0, c],
                _ => [c, 0.0, x],
            };
            [rgb[0] + m, rgb[1] + m, rgb[2] + m, alpha]
        }
        _ => return Err("Use HEX, RGB, HSL, HSV, or normalized RGBA"),
    };
    Ok(color.map(|v| v.clamp(0.0, 1.0)))
}

pub fn outputs([r, g, b, a]: [f32; 4]) -> [String; 7] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let light = (max + min) / 2.0;
    let sl = if delta == 0.0 {
        0.0
    } else {
        delta / (1.0 - (2.0 * light - 1.0).abs())
    };
    let sv = if max == 0.0 { 0.0 } else { delta / max };
    let byte = |v: f32| (v * 255.0).round() as u8;
    let hex = if a == 1.0 {
        format!("#{:02X}{:02X}{:02X}", byte(r), byte(g), byte(b))
    } else {
        format!(
            "#{:02X}{:02X}{:02X}{:02X}",
            byte(r),
            byte(g),
            byte(b),
            byte(a)
        )
    };
    [
        hex,
        format!("rgb({}, {}, {})", byte(r), byte(g), byte(b)),
        format!("rgba({}, {}, {}, {a:.4})", byte(r), byte(g), byte(b)),
        format!("hsl({hue:.2}, {:.2}%, {:.2}%)", sl * 100.0, light * 100.0),
        format!(
            "hsla({hue:.2}, {:.2}%, {:.2}%, {a:.4})",
            sl * 100.0,
            light * 100.0
        ),
        format!("hsv({hue:.2}, {:.2}%, {:.2}%)", sv * 100.0, max * 100.0),
        format!("[{r:.5}, {g:.5}, {b:.5}, {a:.5}]"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_pastes_preserve_or_set_alpha() {
        assert_eq!(
            parse("#f08", 0, 0.3).unwrap(),
            [1.0, 0.0, 136.0 / 255.0, 0.3]
        );
        assert_eq!(
            parse("rgba(255, 0, 128, 50%)", 0, 1.0).unwrap(),
            [1.0, 0.0, 128.0 / 255.0, 0.5]
        );
        assert_eq!(
            parse("rgb(100% 0% 0% / 25%)", 2, 1.0).unwrap(),
            [1.0, 0.0, 0.0, 0.25]
        );
        assert_eq!(
            parse("hsl(-120deg, 100%, 50%)", 0, 0.7).unwrap(),
            [0.0, 0.0, 1.0, 0.7]
        );
        assert_eq!(
            parse("[0.1,0.2,0.3,0.4]", 6, 1.0).unwrap(),
            [0.1, 0.2, 0.3, 0.4]
        );
    }
    #[test]
    fn conversions_round_trip_and_reject_invalid_input() {
        for color in [
            [0.23, 0.72, 0.51, 0.47],
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 1.0, 1.0, 1.0],
        ] {
            for (field, text) in outputs(color).iter().enumerate() {
                let result = parse(text, field, color[3]).unwrap();
                for (actual, expected) in result.into_iter().zip(color) {
                    assert!((actual - expected).abs() < 0.004, "{text}");
                }
            }
        }
        for bad in [
            "#zzzzzz",
            "rgb(999,0,0)",
            "rgba(1,2,3,NaN)",
            "hsl(inf,10%,20%)",
            "rgb(1,2)",
            "#",
            "[0.1, 0.2, 0.3, 0.4",
            "0.1, 0.2, 0.3, 0.4]",
            "rgb(10%%, 20%, 30%)",
        ] {
            assert!(parse(bad, 0, 1.0).is_err(), "{bad}");
        }
    }
}
