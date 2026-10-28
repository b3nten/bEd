//! Colors in theme files: `"#rrggbb"`, or `"#rrggbbaa"` when not opaque.
use super::Rgba;
use serde::{Deserialize, Deserializer, Serializer, de::Error};

pub(super) fn serialize<S: Serializer>(color: &Rgba, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format(color))
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Rgba, D::Error> {
    let text = String::deserialize(deserializer)?;
    parse(&text).ok_or_else(|| D::Error::custom(format!("{text:?} isn't a #rrggbb color")))
}

/// A list of colors, such as the terminal's sixteen.
pub(super) mod list {
    use super::*;
    use serde::ser::SerializeSeq;

    pub(in crate::theme) fn serialize<S: Serializer, const N: usize>(
        colors: &[Rgba; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(N))?;
        for color in colors {
            seq.serialize_element(&format(color))?;
        }
        seq.end()
    }

    pub(in crate::theme) fn deserialize<'de, D: Deserializer<'de>, const N: usize>(
        deserializer: D,
    ) -> Result<[Rgba; N], D::Error> {
        let texts = Vec::<String>::deserialize(deserializer)?;
        if texts.len() != N {
            return Err(D::Error::custom(format!(
                "needs {N} colors, not {}",
                texts.len()
            )));
        }
        let mut colors = [[0.0; 4]; N];
        for (color, text) in colors.iter_mut().zip(&texts) {
            *color = parse(text)
                .ok_or_else(|| D::Error::custom(format!("{text:?} isn't a #rrggbb color")))?;
        }
        Ok(colors)
    }
}

fn format(color: &Rgba) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let [r, g, b, a] = color.map(byte);
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

fn parse(text: &str) -> Option<Rgba> {
    let digits = text.strip_prefix('#')?;
    if !matches!(digits.len(), 6 | 8) || !digits.is_ascii() {
        return None;
    }
    let channel = |i: usize| {
        u8::from_str_radix(&digits[i..i + 2], 16)
            .ok()
            .map(|v| v as f32 / 255.0)
    };
    Some([
        channel(0)?,
        channel(2)?,
        channel(4)?,
        if digits.len() == 8 { channel(6)? } else { 1.0 },
    ])
}
