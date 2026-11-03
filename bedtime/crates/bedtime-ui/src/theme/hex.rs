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

/// The terminal's sixteen colors, by name: `black` … `white`, then
/// `bright_black` … `bright_white`. A list of sixteen in that order (the
/// earlier format) is read too.
pub(super) mod terminal {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Named {
        #[serde(with = "super")]
        black: Rgba,
        #[serde(with = "super")]
        red: Rgba,
        #[serde(with = "super")]
        green: Rgba,
        #[serde(with = "super")]
        yellow: Rgba,
        #[serde(with = "super")]
        blue: Rgba,
        #[serde(with = "super")]
        magenta: Rgba,
        #[serde(with = "super")]
        cyan: Rgba,
        #[serde(with = "super")]
        white: Rgba,
        #[serde(with = "super")]
        bright_black: Rgba,
        #[serde(with = "super")]
        bright_red: Rgba,
        #[serde(with = "super")]
        bright_green: Rgba,
        #[serde(with = "super")]
        bright_yellow: Rgba,
        #[serde(with = "super")]
        bright_blue: Rgba,
        #[serde(with = "super")]
        bright_magenta: Rgba,
        #[serde(with = "super")]
        bright_cyan: Rgba,
        #[serde(with = "super")]
        bright_white: Rgba,
    }

    pub(in crate::theme) fn serialize<S: Serializer>(
        colors: &[Rgba; 16],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Named {
            black: colors[0],
            red: colors[1],
            green: colors[2],
            yellow: colors[3],
            blue: colors[4],
            magenta: colors[5],
            cyan: colors[6],
            white: colors[7],
            bright_black: colors[8],
            bright_red: colors[9],
            bright_green: colors[10],
            bright_yellow: colors[11],
            bright_blue: colors[12],
            bright_magenta: colors[13],
            bright_cyan: colors[14],
            bright_white: colors[15],
        }
        .serialize(serializer)
    }

    pub(in crate::theme) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<[Rgba; 16], D::Error> {
        deserializer.deserialize_any(Colors)
    }

    /// Named (a map), or the earlier list. Not `untagged`, so a mistake in
    /// either says what it is.
    struct Colors;

    impl<'de> serde::de::Visitor<'de> for Colors {
        type Value = [Rgba; 16];

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("the sixteen terminal colors, by name")
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            let named = Named::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
            Ok([
                named.black,
                named.red,
                named.green,
                named.yellow,
                named.blue,
                named.magenta,
                named.cyan,
                named.white,
                named.bright_black,
                named.bright_red,
                named.bright_green,
                named.bright_yellow,
                named.bright_blue,
                named.bright_magenta,
                named.bright_cyan,
                named.bright_white,
            ])
        }

        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut colors = [[0.0; 4]; 16];
            let mut count = 0;
            while let Some(text) = seq.next_element::<String>()? {
                let color = parse(&text)
                    .ok_or_else(|| A::Error::custom(format!("{text:?} isn't a #rrggbb color")))?;
                if let Some(slot) = colors.get_mut(count) {
                    *slot = color;
                }
                count += 1;
            }
            if count != 16 {
                return Err(A::Error::custom(format!("needs 16 colors, not {count}")));
            }
            Ok(colors)
        }
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
