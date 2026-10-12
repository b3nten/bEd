//! Small sRGB helpers for readable theme colors.

fn channel(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Interpolate RGBA channels; `amount` is the foreground's weight.
pub fn blend(foreground: [f32; 4], background: [f32; 4], amount: f32) -> [f32; 4] {
    let amount = channel(amount);
    std::array::from_fn(|i| {
        channel(foreground[i]) * amount + channel(background[i]) * (1.0 - amount)
    })
}

/// WCAG relative luminance of sRGB channels. Alpha is evaluated by the caller
/// against the actual surface before measuring a rendered color.
pub fn relative_luminance(color: [f32; 4]) -> f32 {
    let linear = |value| {
        let value = channel(value);
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color[0]) + 0.7152 * linear(color[1]) + 0.0722 * linear(color[2])
}

/// Contrast between two rendered sRGB colors; their alpha channels are ignored.
pub fn contrast_ratio(a: [f32; 4], b: [f32; 4]) -> f32 {
    let a = relative_luminance(a);
    let b = relative_luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn rendered(foreground: [f32; 4], background: [f32; 4]) -> [f32; 4] {
    let mut color = blend(foreground, background, foreground[3]);
    color[3] = 1.0;
    color
}

/// Reach the requested contrast with the smallest mix toward black or white.
/// Keep the original alpha when feasible; otherwise make the color opaque.
/// Background RGB describes the actual surface, even for transparent windows.
pub fn ensure_contrast(foreground: [f32; 4], background: [f32; 4], min_ratio: f32) -> [f32; 4] {
    let foreground = foreground.map(channel);
    let background = background.map(channel);
    let target = if min_ratio.is_finite() {
        min_ratio.clamp(1.0, 21.0)
    } else {
        1.0
    };
    if contrast_ratio(rendered(foreground, background), background) >= target {
        return foreground;
    }
    let mut best = None;
    let mut fallback = foreground;
    let mut fallback_ratio = 0.0;
    for level in [0.0, 1.0] {
        let mut source = foreground;
        let mut endpoint = [level, level, level, source[3]];
        if contrast_ratio(rendered(endpoint, background), background) < target {
            source[3] = 1.0;
            endpoint[3] = 1.0;
        }
        let endpoint_ratio = contrast_ratio(rendered(endpoint, background), background);
        if endpoint_ratio > fallback_ratio {
            fallback = endpoint;
            fallback_ratio = endpoint_ratio;
        }
        if endpoint_ratio < target {
            continue;
        }
        let mut low = 0.0;
        let mut high = 1.0;
        for _ in 0..24 {
            let mid = (low + high) * 0.5;
            let color = blend(endpoint, source, mid);
            if contrast_ratio(rendered(color, background), background) >= target {
                high = mid;
            } else {
                low = mid;
            }
        }
        let color = blend(endpoint, source, high);
        let distance = color
            .iter()
            .zip(foreground)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>();
        if best.is_none_or(|(_, previous)| distance < previous) {
            best = Some((color, distance));
        }
    }
    best.map_or(fallback, |(color, _)| color)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_srgb_luminance_and_contrast() {
        assert_eq!(relative_luminance([0.0, 0.0, 0.0, 1.0]), 0.0);
        assert_eq!(relative_luminance([1.0; 4]), 1.0);
        assert!((relative_luminance([1.0, 0.0, 0.0, 1.0]) - 0.2126).abs() < 0.0001);
        assert!((contrast_ratio([0.0; 4], [1.0; 4]) - 21.0).abs() < 0.0001);
        assert_eq!(blend([1.0; 4], [0.0; 4], 0.25), [0.25; 4]);
    }
    #[test]
    fn readable_colors_stay_unchanged_and_low_contrast_colors_adapt_on_both_surfaces() {
        for background in [[0.02, 0.02, 0.03, 1.0], [0.96, 0.94, 0.90, 1.0]] {
            for color in [
                [0.8, 0.7, 0.9, 1.0],
                [0.2, 0.3, 0.4, 0.5],
                [1.0, 0.1, 0.7, 0.0],
            ] {
                let adjusted = ensure_contrast(color, background, 4.5);
                assert!(contrast_ratio(rendered(adjusted, background), background) >= 4.499);
            }
        }
        let readable = [0.9, 0.9, 0.9, 0.8];
        assert_eq!(ensure_contrast(readable, [0.0; 4], 4.5), readable);
    }
}
